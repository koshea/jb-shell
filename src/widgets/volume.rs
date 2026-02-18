use gtk4::prelude::*;
use gtk4::{Box as GtkBox, Image, Label, Orientation};
use relm4::prelude::*;
use std::process::Command;
use std::time::Duration;

pub struct VolumeModel {
    volume: u32,
    muted: bool,
}

#[derive(Debug)]
pub enum VolumeInput {
    PollResult(u32, bool),
}

pub struct VolumeWidgets {
    icon: Image,
    label: Label,
}

impl SimpleComponent for VolumeModel {
    type Init = ();
    type Input = VolumeInput;
    type Output = ();
    type Root = GtkBox;
    type Widgets = VolumeWidgets;

    fn init_root() -> Self::Root {
        let b = GtkBox::new(Orientation::Horizontal, 4);
        b.set_widget_name("volume");
        b
    }

    fn init(
        _init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let icon = Image::from_icon_name("audio-volume-medium-symbolic");
        icon.set_pixel_size(16);
        let label = Label::new(Some("0%"));

        root.append(&icon);
        root.append(&label);

        // Background polling thread
        let input_sender = sender.input_sender().clone();
        std::thread::spawn(move || loop {
            let result = get_volume();
            input_sender.emit(VolumeInput::PollResult(result.0, result.1));
            std::thread::sleep(Duration::from_secs(1));
        });

        let model = VolumeModel {
            volume: 0,
            muted: false,
        };
        let widgets = VolumeWidgets { icon, label };
        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, _sender: ComponentSender<Self>) {
        match msg {
            VolumeInput::PollResult(volume, muted) => {
                self.volume = volume;
                self.muted = muted;
            }
        }
    }

    fn update_view(&self, widgets: &mut Self::Widgets, _sender: ComponentSender<Self>) {
        let icon_name = if self.muted {
            "audio-volume-muted-symbolic"
        } else if self.volume < 33 {
            "audio-volume-low-symbolic"
        } else if self.volume < 66 {
            "audio-volume-medium-symbolic"
        } else {
            "audio-volume-high-symbolic"
        };
        widgets.icon.set_icon_name(Some(icon_name));
        widgets.label.set_label(&format!("{}%", self.volume));
    }
}

fn get_volume() -> (u32, bool) {
    let output = Command::new("wpctl")
        .args(["get-volume", "@DEFAULT_AUDIO_SINK@"])
        .output();

    match output {
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.stdout);
            let muted = text.contains("[MUTED]");
            let volume = text
                .split_whitespace()
                .nth(1)
                .and_then(|v| v.parse::<f64>().ok())
                .map(|v| (v * 100.0).round() as u32)
                .unwrap_or(0);
            (volume, muted)
        }
        Err(_) => (0, false),
    }
}
