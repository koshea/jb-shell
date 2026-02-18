use hyprland::data::{Workspace, Workspaces};
use hyprland::event_listener::EventListener;
use hyprland::shared::{HyprData, HyprDataActive, HyprDataVec};
use std::sync::mpsc::Sender;

#[derive(Debug, Clone)]
pub enum HyprlandMsg {
    WorkspaceChanged {
        monitor_name: String,
        workspace_id: i32,
    },
    WorkspaceCreated {
        workspace_id: i32,
        monitor_name: String,
    },
    WorkspaceDestroyed {
        workspace_id: i32,
    },
    WorkspaceMoved {
        workspace_id: i32,
        monitor_name: String,
    },
    ActiveWindowChanged {
        title: String,
    },
    MonitorFocusChanged {
        monitor_name: String,
        workspace_id: i32,
    },
}

fn workspace_monitor(ws_id: i32) -> Option<String> {
    let workspaces = Workspaces::get().ok()?;
    workspaces
        .to_vec()
        .into_iter()
        .find(|ws| ws.id == ws_id)
        .map(|ws| ws.monitor.clone())
}

pub fn spawn_listener(tx: Sender<HyprlandMsg>) {
    std::thread::spawn(move || {
        let mut listener = EventListener::new();

        // Workspace changed (activated)
        {
            let tx = tx.clone();
            listener.add_workspace_changed_handler(move |data| {
                let ws_id = data.id;
                if let Some(monitor_name) = workspace_monitor(ws_id) {
                    let _ = tx.send(HyprlandMsg::WorkspaceChanged {
                        monitor_name,
                        workspace_id: ws_id,
                    });
                }
            });
        }

        // Workspace created
        {
            let tx = tx.clone();
            listener.add_workspace_added_handler(move |data| {
                let ws_id = data.id;
                if let Some(monitor_name) = workspace_monitor(ws_id) {
                    let _ = tx.send(HyprlandMsg::WorkspaceCreated {
                        workspace_id: ws_id,
                        monitor_name,
                    });
                }
            });
        }

        // Workspace destroyed
        {
            let tx = tx.clone();
            listener.add_workspace_deleted_handler(move |data| {
                let _ = tx.send(HyprlandMsg::WorkspaceDestroyed {
                    workspace_id: data.id,
                });
            });
        }

        // Workspace moved to another monitor
        {
            let tx = tx.clone();
            listener.add_workspace_moved_handler(move |data| {
                let _ = tx.send(HyprlandMsg::WorkspaceMoved {
                    workspace_id: data.id,
                    monitor_name: data.monitor.clone(),
                });
            });
        }

        // Active window changed
        {
            let tx = tx.clone();
            listener.add_active_window_changed_handler(move |data| {
                let title = data.as_ref().map(|d| d.title.clone()).unwrap_or_default();
                let _ = tx.send(HyprlandMsg::ActiveWindowChanged { title });
            });
        }

        // Monitor focus changed
        {
            let tx = tx.clone();
            listener.add_active_monitor_changed_handler(move |data| {
                if let Ok(active) = Workspace::get_active() {
                    let _ = tx.send(HyprlandMsg::MonitorFocusChanged {
                        monitor_name: data.monitor_name.clone(),
                        workspace_id: active.id,
                    });
                }
            });
        }

        if let Err(e) = listener.start_listener() {
            eprintln!("Hyprland listener error: {e}");
        }
    });
}
