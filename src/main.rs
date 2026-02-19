mod bar;
mod google_calendar;
mod hyprland_listener;
mod notification_daemon;
mod summary_thread;
mod widgets;
mod workspace_capture;

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

fn match_hyprland_monitor(
    gdk_mon: &gdk4::Monitor,
    hypr_monitors: &[hyprland::data::Monitor],
    index: u32,
) -> String {
    let geo = gdk_mon.geometry();
    hypr_monitors
        .iter()
        .find(|hm| hm.x == geo.x() && hm.y == geo.y())
        .map(|hm| hm.name.clone())
        .unwrap_or_else(|| {
            hypr_monitors
                .get(index as usize)
                .map(|hm| hm.name.clone())
                .unwrap_or_else(|| format!("unknown-{index}"))
        })
}

fn main() {
    let app = Application::builder().application_id(APP_ID).build();

    app.connect_shutdown(|_| {
        eprintln!("jb-shell: [lifecycle] Application::shutdown fired");
    });

    app.connect_activate(move |app| {
        // Prevent app from quitting when all windows are destroyed (e.g. DPMS monitor off).
        // The guard must be kept alive for the duration of the app.
        let _hold = app.hold();
        eprintln!("jb-shell: [lifecycle] activate — hold guard acquired");
        // Load CSS
        let css_provider = CssProvider::new();
        let config_dir = std::env::var("XDG_CONFIG_HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                std::path::PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
                    .join(".config")
            });

        let css_candidates = [
            config_dir.join("jb-shell/style.css"),
            std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|p| p.join("style.css")))
                .unwrap_or_default(),
            std::path::PathBuf::from("style.css"),
        ];

        let mut css_loaded = false;
        for candidate in &css_candidates {
            if candidate.exists() {
                eprintln!("jb-shell: loading CSS from {}", candidate.display());
                css_provider.load_from_path(candidate.to_str().unwrap());
                css_loaded = true;
                break;
            }
        }
        if !css_loaded {
            eprintln!(
                "jb-shell: no style.css found, searched: {}",
                css_candidates
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }

        gtk4::style_context_add_provider_for_display(
            &gdk::Display::default().expect("Could not get default display"),
            &css_provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        // Match GDK monitors to Hyprland monitors
        let display = gdk::Display::default().expect("Could not get default display");
        let gdk_monitors = display.monitors();

        let hypr_monitors = Monitors::get().map(|m| m.to_vec()).unwrap_or_default();
        eprintln!(
            "jb-shell: [lifecycle] startup: gdk_monitors={} hypr_monitors=[{}]",
            gdk_monitors.n_items(),
            hypr_monitors
                .iter()
                .map(|m| format!("{}@{}x{}", m.name, m.x, m.y))
                .collect::<Vec<_>>()
                .join(", ")
        );

        let bars: Rc<RefCell<Vec<StatusBar>>> = Rc::new(RefCell::new(Vec::new()));

        for i in 0..gdk_monitors.n_items() {
            let gdk_mon = gdk_monitors
                .item(i)
                .and_then(|obj| obj.downcast::<gdk4::Monitor>().ok());

            let gdk_mon = match gdk_mon {
                Some(m) => m,
                None => continue,
            };

            let hypr_name = match_hyprland_monitor(&gdk_mon, &hypr_monitors, i);

            let bar = StatusBar::new(&gdk_mon, &hypr_name);
            bar.window.set_application(Some(app));
            bar.window.present();
            bars.borrow_mut().push(bar);
        }

        // Start notification daemon using the first bar's notification sender
        if !bars.borrow().is_empty() {
            let notif_sender = bars.borrow()[0].notification_sender().clone();
            let daemon_tx = notification_daemon::spawn_notification_daemon(notif_sender.clone());
            notif_sender.emit(
                crate::widgets::notifications::NotificationInput::SetDaemonChannel(
                    daemon_tx.clone(),
                ),
            );
            let center_sender = bars.borrow()[0].notification_center_sender().clone();
            center_sender.emit(
                crate::widgets::notification_center::NotificationCenterInput::SetDaemonChannel(
                    daemon_tx,
                ),
            );
        }

        // Listen for monitor additions/removals (DPMS, hotplug)
        let bars_for_signal = bars.clone();
        let app_for_signal = app.clone();
        gdk_monitors.connect_items_changed(move |list, position, removed, added| {
            let total_gdk = list.n_items();
            eprintln!(
                "jb-shell: [monitor] items_changed: pos={position} removed={removed} added={added} total_gdk_monitors={total_gdk}"
            );
            let mut bars = bars_for_signal.borrow_mut();
            eprintln!(
                "jb-shell: [monitor] bars before processing: {} — [{}]",
                bars.len(),
                bars.iter()
                    .map(|b| b.monitor_name().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            );

            // Remove bars for monitors that no longer exist
            if removed > 0 {
                let valid_monitors: Vec<gdk4::Monitor> = (0..list.n_items())
                    .filter_map(|i| {
                        let mon = list.item(i)?.downcast::<gdk4::Monitor>().ok()?;
                        let geo = mon.geometry();
                        let valid = mon.is_valid();
                        eprintln!(
                            "jb-shell: [monitor]   gdk monitor {i}: valid={valid} geo={}x{}+{}+{}",
                            geo.width(),
                            geo.height(),
                            geo.x(),
                            geo.y()
                        );
                        Some(mon)
                    })
                    .collect();

                bars.retain(|bar| {
                    let still_valid = valid_monitors.iter().any(|vm| vm == &bar.monitor);
                    let mon_valid = bar.monitor.is_valid();
                    if !still_valid {
                        eprintln!(
                            "jb-shell: [monitor] removing bar for disconnected monitor: {} (monitor.is_valid={})",
                            bar.monitor_name(),
                            mon_valid,
                        );
                        bar.destroy();
                    } else {
                        eprintln!(
                            "jb-shell: [monitor] keeping bar: {} (monitor.is_valid={})",
                            bar.monitor_name(),
                            mon_valid,
                        );
                    }
                    still_valid
                });
            }

            // Add bars for new monitors
            if added > 0 {
                let hypr_monitors = Monitors::get().map(|m| m.to_vec()).unwrap_or_default();
                eprintln!(
                    "jb-shell: [monitor] hyprland monitors: [{}]",
                    hypr_monitors
                        .iter()
                        .map(|m| format!("{}@{}x{}", m.name, m.x, m.y))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                for i in position..(position + added) {
                    if let Some(gdk_mon) = list
                        .item(i)
                        .and_then(|o| o.downcast::<gdk4::Monitor>().ok())
                    {
                        let hypr_name = match_hyprland_monitor(&gdk_mon, &hypr_monitors, i);
                        eprintln!("jb-shell: [monitor] adding bar for new monitor: {hypr_name}");
                        let bar = StatusBar::new(&gdk_mon, &hypr_name);
                        bar.window.set_application(Some(&app_for_signal));
                        bar.window.present();
                        bars.push(bar);
                    }
                }
            }

            eprintln!(
                "jb-shell: [monitor] bars after processing: {} — [{}]",
                bars.len(),
                bars.iter()
                    .map(|b| b.monitor_name().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            eprintln!(
                "jb-shell: [monitor] app is_registered={} windows={}",
                app_for_signal.is_registered(),
                app_for_signal.windows().len()
            );
        });

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

    let exit_code = app.run_with_args::<&str>(&[]);
    eprintln!("jb-shell: [lifecycle] app.run_with_args returned, exit_code={exit_code:?}");
}
