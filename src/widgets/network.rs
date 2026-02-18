use gtk4::prelude::*;
use gtk4::{Box as GtkBox, Image, Label, Orientation};
use relm4::prelude::*;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

const SKIP_PREFIXES: &[&str] = &["lo", "docker", "br-", "veth", "tailscale", "virbr"];

pub struct NetworkModel {
    icon_name: String,
    label_text: String,
}

#[derive(Debug)]
pub enum NetworkInput {
    PollResult { icon_name: String, label_text: String },
}

pub struct NetworkWidgets {
    icon: Image,
    label: Label,
}

impl SimpleComponent for NetworkModel {
    type Init = ();
    type Input = NetworkInput;
    type Output = ();
    type Root = GtkBox;
    type Widgets = NetworkWidgets;

    fn init_root() -> Self::Root {
        let b = GtkBox::new(Orientation::Horizontal, 4);
        b.set_widget_name("network");
        b
    }

    fn init(
        _init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let icon = Image::from_icon_name("network-offline-symbolic");
        icon.set_pixel_size(16);
        let label = Label::new(Some("Offline"));

        root.append(&icon);
        root.append(&label);

        // Background polling thread
        let input_sender = sender.input_sender().clone();
        std::thread::spawn(move || loop {
            let (icon_name, label_text) = detect_network();
            input_sender.emit(NetworkInput::PollResult {
                icon_name,
                label_text,
            });
            std::thread::sleep(Duration::from_secs(5));
        });

        let model = NetworkModel {
            icon_name: "network-offline-symbolic".to_string(),
            label_text: "Offline".to_string(),
        };
        let widgets = NetworkWidgets { icon, label };
        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, _sender: ComponentSender<Self>) {
        match msg {
            NetworkInput::PollResult {
                icon_name,
                label_text,
            } => {
                self.icon_name = icon_name;
                self.label_text = label_text;
            }
        }
    }

    fn update_view(&self, widgets: &mut Self::Widgets, _sender: ComponentSender<Self>) {
        widgets.icon.set_icon_name(Some(&self.icon_name));
        widgets.label.set_label(&self.label_text);
    }
}

fn detect_network() -> (String, String) {
    let net_dir = Path::new("/sys/class/net");
    if !net_dir.is_dir() {
        return ("network-offline-symbolic".into(), "Offline".into());
    }

    let entries = match fs::read_dir(net_dir) {
        Ok(e) => e,
        Err(_) => return ("network-offline-symbolic".into(), "Offline".into()),
    };

    let mut wired_up: Option<String> = None;
    let mut wireless_up: Option<String> = None;

    for entry in entries.flatten() {
        let iface = entry.file_name().to_string_lossy().to_string();
        if SKIP_PREFIXES.iter().any(|p| iface.starts_with(p)) {
            continue;
        }

        let iface_path = entry.path();
        let operstate = match fs::read_to_string(iface_path.join("operstate")) {
            Ok(s) => s.trim().to_string(),
            Err(_) => continue,
        };

        if operstate != "up" {
            continue;
        }

        if iface_path.join("wireless").is_dir() {
            if wireless_up.is_none() {
                wireless_up = Some(iface);
            }
        } else if wired_up.is_none() {
            wired_up = Some(iface);
        }
    }

    if wired_up.is_some() {
        return ("network-wired-symbolic".into(), "Wired".into());
    }

    if let Some(iface) = wireless_up {
        let (ssid, rssi) = get_wireless_info(&iface);
        let icon = if rssi >= -50 {
            "network-wireless-signal-excellent-symbolic"
        } else if rssi >= -60 {
            "network-wireless-signal-good-symbolic"
        } else if rssi >= -70 {
            "network-wireless-signal-ok-symbolic"
        } else {
            "network-wireless-signal-none-symbolic"
        };
        return (icon.into(), ssid);
    }

    ("network-offline-symbolic".into(), "Offline".into())
}

fn get_wireless_info(iface: &str) -> (String, i32) {
    let output = Command::new("iwctl")
        .args(["station", iface, "show"])
        .output();

    match output {
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.stdout);
            let mut ssid = iface.to_string();
            let mut rssi = -100i32;

            for line in text.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with("Connected network") {
                    if let Some(val) = trimmed.strip_prefix("Connected network") {
                        ssid = val.trim().to_string();
                    }
                } else if trimmed.starts_with("RSSI") {
                    if let Some(val) = trimmed.strip_prefix("RSSI") {
                        if let Some(num_str) = val.trim().split_whitespace().next() {
                            if let Ok(n) = num_str.parse::<i32>() {
                                rssi = n;
                            }
                        }
                    }
                }
            }

            (ssid, rssi)
        }
        Err(_) => (iface.to_string(), -100),
    }
}
