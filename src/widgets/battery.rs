use gtk4::prelude::*;
use gtk4::{Box as GtkBox, Image, Label, Orientation};
use relm4::prelude::*;
use std::time::Duration;

pub struct BatteryModel {
    pct: u32,
    icon_name: String,
    visible: bool,
}

#[derive(Debug)]
pub enum BatteryInput {
    PollResult { pct: u32, icon_name: String },
    NoBattery,
}

pub struct BatteryWidgets {
    icon: Image,
    label: Label,
}

impl SimpleComponent for BatteryModel {
    type Init = ();
    type Input = BatteryInput;
    type Output = ();
    type Root = GtkBox;
    type Widgets = BatteryWidgets;

    fn init_root() -> Self::Root {
        let b = GtkBox::new(Orientation::Horizontal, 4);
        b.set_widget_name("battery");
        b
    }

    fn init(
        _init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let icon = Image::from_icon_name("battery-full-symbolic");
        icon.set_pixel_size(16);
        let label = Label::new(Some(""));

        root.append(&icon);
        root.append(&label);

        // Battery crate types are !Send, so init on a dedicated thread that owns them
        let input_sender = sender.input_sender().clone();
        std::thread::spawn(move || {
            let manager = match battery::Manager::new() {
                Ok(m) => m,
                Err(_) => {
                    input_sender.emit(BatteryInput::NoBattery);
                    return;
                }
            };

            let mut batteries = match manager.batteries() {
                Ok(b) => b,
                Err(_) => {
                    input_sender.emit(BatteryInput::NoBattery);
                    return;
                }
            };

            let mut bat = match batteries.next() {
                Some(Ok(b)) => b,
                _ => {
                    input_sender.emit(BatteryInput::NoBattery);
                    return;
                }
            };

            loop {
                let _ = manager.refresh(&mut bat);
                let pct = (bat.state_of_charge().value * 100.0).round() as u32;
                let icon_name = match bat.state() {
                    battery::State::Charging => "battery-charging-symbolic",
                    _ if pct <= 10 => "battery-empty-symbolic",
                    _ if pct <= 30 => "battery-caution-symbolic",
                    _ if pct <= 60 => "battery-low-symbolic",
                    _ if pct <= 90 => "battery-good-symbolic",
                    _ => "battery-full-symbolic",
                };
                input_sender.emit(BatteryInput::PollResult {
                    pct,
                    icon_name: icon_name.to_string(),
                });
                std::thread::sleep(Duration::from_secs(30));
            }
        });

        let model = BatteryModel {
            pct: 0,
            icon_name: "battery-full-symbolic".to_string(),
            visible: true,
        };
        let widgets = BatteryWidgets { icon, label };
        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, _sender: ComponentSender<Self>) {
        match msg {
            BatteryInput::PollResult { pct, icon_name } => {
                self.pct = pct;
                self.icon_name = icon_name;
                self.visible = true;
            }
            BatteryInput::NoBattery => {
                self.visible = false;
            }
        }
    }

    fn update_view(&self, widgets: &mut Self::Widgets, _sender: ComponentSender<Self>) {
        widgets.icon.parent().unwrap().set_visible(self.visible);
        if self.visible {
            widgets.icon.set_icon_name(Some(&self.icon_name));
            widgets.label.set_label(&format!("{}%", self.pct));
        }
    }
}
