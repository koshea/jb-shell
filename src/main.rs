mod bar;
mod hyprland_listener;
mod widgets;

use bar::StatusBar;
use hyprland::data::Monitors;
use hyprland::shared::{HyprData, HyprDataVec};
use hyprland_listener::HyprlandMsg;

use gdk4::prelude::*;
use gtk4::prelude::*;
use gtk4::{gdk, Application, CssProvider};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;

const APP_ID: &str = "dev.jb.shell";

fn main() {
    let app = Application::builder().application_id(APP_ID).build();

    app.connect_activate(move |app| {
        // Load CSS
        let css_provider = CssProvider::new();
        let css_path = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.join("../style.css")))
            .unwrap_or_else(|| std::path::PathBuf::from("style.css"));

        let css_candidates = [
            css_path,
            std::path::PathBuf::from("style.css"),
            std::env::current_dir()
                .unwrap_or_default()
                .join("style.css"),
        ];

        for candidate in &css_candidates {
            if candidate.exists() {
                css_provider.load_from_path(candidate.to_str().unwrap_or("style.css"));
                break;
            }
        }

        gtk4::style_context_add_provider_for_display(
            &gdk::Display::default().expect("Could not get default display"),
            &css_provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        // Match GDK monitors to Hyprland monitors
        let display = gdk::Display::default().expect("Could not get default display");
        let gdk_monitors = display.monitors();

        let hypr_monitors = Monitors::get()
            .map(|m| m.to_vec())
            .unwrap_or_default();

        let bars: Rc<RefCell<Vec<StatusBar>>> = Rc::new(RefCell::new(Vec::new()));

        for i in 0..gdk_monitors.n_items() {
            let gdk_mon = gdk_monitors
                .item(i)
                .and_then(|obj| obj.downcast::<gdk4::Monitor>().ok());

            let gdk_mon = match gdk_mon {
                Some(m) => m,
                None => continue,
            };

            let geo = gdk_mon.geometry();

            // Find matching Hyprland monitor by (x, y) position
            let hypr_name = hypr_monitors
                .iter()
                .find(|hm| hm.x == geo.x() && hm.y == geo.y())
                .map(|hm| hm.name.clone());

            let hypr_name = match hypr_name {
                Some(name) => name,
                None => {
                    // Fallback: match by index
                    hypr_monitors
                        .get(i as usize)
                        .map(|hm| hm.name.clone())
                        .unwrap_or_else(|| format!("unknown-{i}"))
                }
            };

            let bar = StatusBar::new(&gdk_mon, &hypr_name);
            bar.window.set_application(Some(app));
            bar.window.present();
            bars.borrow_mut().push(bar);
        }

        // Set up Hyprland event channel using std::sync::mpsc
        let (tx, rx) = mpsc::channel::<HyprlandMsg>();

        hyprland_listener::spawn_listener(tx);

        // Poll the channel from the GTK main loop
        let bars_clone = bars.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(16), move || {
            while let Ok(msg) = rx.try_recv() {
                let bars = bars_clone.borrow();
                for bar in bars.iter() {
                    bar.handle_hyprland_msg(&msg);
                }
            }
            glib::ControlFlow::Continue
        });
    });

    app.run_with_args::<&str>(&[]);
}
