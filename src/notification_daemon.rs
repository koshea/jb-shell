use crate::widgets::notifications::{
    ActionCallback, NotificationAction, NotificationId, NotificationInput, NotificationKind,
    NotificationRequest, NotificationSource,
};
use chrono::TimeZone;
use rusqlite::Connection as DbConnection;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{mpsc, Mutex};
use std::thread;
use zbus::blocking;
use zbus::interface;
use zbus::zvariant;

#[derive(Debug)]
pub enum DaemonCommand {
    NotificationClosed { id: u32, reason: u32 },
    ActionInvoked { id: u32, action_key: String },
}

struct NotificationServer {
    notif_sender: relm4::Sender<NotificationInput>,
    db: Mutex<DbConnection>,
    next_id: AtomicU32,
}

#[interface(name = "org.freedesktop.Notifications")]
impl NotificationServer {
    fn get_capabilities(&self) -> Vec<String> {
        vec!["actions".into(), "body".into(), "body-markup".into()]
    }

    #[allow(clippy::too_many_arguments)]
    async fn notify(
        &self,
        #[zbus(connection)] conn: &zbus::Connection,
        #[zbus(header)] header: zbus::message::Header<'_>,
        app_name: &str,
        replaces_id: u32,
        _app_icon: &str,
        summary: &str,
        body: &str,
        actions: Vec<String>,
        hints: HashMap<String, zvariant::OwnedValue>,
        expire_timeout: i32,
    ) -> u32 {
        // Resolve the sender's PID for window focusing
        let sender_pid = if let Some(sender) = header.sender() {
            conn.call_method(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                Some("org.freedesktop.DBus"),
                "GetConnectionUnixProcessID",
                &(sender.as_str(),),
            )
            .await
            .ok()
            .and_then(|reply| reply.body().deserialize::<u32>().ok())
        } else {
            None
        };

        let id = if replaces_id != 0 {
            replaces_id
        } else {
            self.next_id.fetch_add(1, Ordering::Relaxed)
        };

        // Parse hints
        let urgency: u8 = hints
            .get("urgency")
            .and_then(|v| <u8>::try_from(v).ok())
            .unwrap_or(1);

        let category: Option<String> = hints
            .get("category")
            .and_then(|v| v.try_clone().ok())
            .and_then(|v| String::try_from(v).ok());

        let desktop_entry: Option<String> = hints
            .get("desktop-entry")
            .and_then(|v| v.try_clone().ok())
            .and_then(|v| String::try_from(v).ok());

        let transient: bool = hints
            .get("transient")
            .and_then(|v| v.try_clone().ok())
            .and_then(|v| bool::try_from(v).ok())
            .unwrap_or(false);

        let resident: bool = hints
            .get("resident")
            .and_then(|v| v.try_clone().ok())
            .and_then(|v| bool::try_from(v).ok())
            .unwrap_or(false);

        let actions_json = serialize_actions_json(&actions);

        // Store in DB
        if let Ok(db) = self.db.lock() {
            if replaces_id != 0 {
                let _ = db.execute(
                    "UPDATE notifications SET app_name=?1, app_icon=?2, summary=?3, body=?4, \
                     urgency=?5, category=?6, desktop_entry=?7, actions=?8, transient=?9, \
                     resident=?10, expire_timeout=?11 WHERE id=?12",
                    rusqlite::params![
                        app_name,
                        _app_icon,
                        summary,
                        body,
                        urgency,
                        category,
                        desktop_entry,
                        actions_json,
                        transient,
                        resident,
                        expire_timeout,
                        id,
                    ],
                );
            } else {
                let _ = db.execute(
                    "INSERT INTO notifications (id, app_name, app_icon, summary, body, urgency, \
                     category, desktop_entry, actions, transient, resident, expire_timeout) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                    rusqlite::params![
                        id,
                        app_name,
                        _app_icon,
                        summary,
                        body,
                        urgency,
                        category,
                        desktop_entry,
                        actions_json,
                        transient,
                        resident,
                        expire_timeout,
                    ],
                );
            }
        }

        let request = fd_notification_to_request(
            id,
            app_name,
            summary,
            body,
            &actions,
            urgency,
            expire_timeout,
            desktop_entry,
            sender_pid,
        );
        self.notif_sender.emit(NotificationInput::Show(request));

        id
    }

    fn close_notification(&self, id: u32) {
        let notif_id = id as NotificationId;
        self.notif_sender.emit(NotificationInput::Dismiss(notif_id));

        if let Ok(db) = self.db.lock() {
            let _ = db.execute(
                "UPDATE notifications SET closed_at = datetime('now'), close_reason = 3 WHERE id = ?1",
                rusqlite::params![id],
            );
        }
    }

    fn get_server_information(&self) -> (String, String, String, String) {
        (
            "jb-shell".into(),
            "jb".into(),
            env!("CARGO_PKG_VERSION").into(),
            "1.2".into(),
        )
    }

    #[zbus(signal)]
    async fn notification_closed(
        signal_emitter: &zbus::object_server::SignalEmitter<'_>,
        id: u32,
        reason: u32,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn action_invoked(
        signal_emitter: &zbus::object_server::SignalEmitter<'_>,
        id: u32,
        action_key: &str,
    ) -> zbus::Result<()>;
}

fn serialize_actions_json(actions: &[String]) -> String {
    let pairs: Vec<(&str, &str)> = actions
        .chunks(2)
        .filter_map(|chunk| {
            if chunk.len() == 2 {
                Some((chunk[0].as_str(), chunk[1].as_str()))
            } else {
                None
            }
        })
        .collect();
    serde_json::to_string(&pairs).unwrap_or_else(|_| "[]".into())
}

fn fd_notification_to_request(
    fd_id: u32,
    app_name: &str,
    summary: &str,
    body: &str,
    actions: &[String],
    urgency: u8,
    expire_timeout: i32,
    desktop_entry: Option<String>,
    sender_pid: Option<u32>,
) -> NotificationRequest {
    let has_actions = actions.len() >= 2;
    let timeout_ms = match expire_timeout {
        -1 => Some(if has_actions { 15000 } else { 5000 }),
        0 => None,
        ms if ms > 0 => Some(ms as u32),
        _ => Some(if has_actions { 15000 } else { 5000 }),
    };

    let notif_id = fd_id as NotificationId;

    let mut notif_actions: Vec<NotificationAction> = actions
        .chunks(2)
        .filter_map(|chunk| {
            if chunk.len() == 2 {
                let key = &chunk[0];
                let label = &chunk[1];
                let display_label = if label.is_empty() {
                    if key == "default" {
                        "Open".to_string()
                    } else {
                        key.clone()
                    }
                } else {
                    label.clone()
                };
                let css_class = if key == "default" {
                    "notif-default-action"
                } else {
                    "notif-action"
                };
                Some(NotificationAction {
                    label: display_label,
                    css_class: css_class.to_string(),
                    callback: ActionCallback::FdAction {
                        fd_id,
                        action_key: key.clone(),
                    },
                })
            } else {
                None
            }
        })
        .collect();

    notif_actions.push(NotificationAction {
        label: "Dismiss".to_string(),
        css_class: "notif-action".to_string(),
        callback: ActionCallback::Dismiss,
    });

    let urgency_class = match urgency {
        0 => Some("urgency-low".to_string()),
        2 => Some("urgency-critical".to_string()),
        _ => None,
    };

    NotificationRequest {
        id: notif_id,
        kind: NotificationKind::Toast,
        icon: None,
        title: summary.to_string(),
        body: if body.is_empty() {
            None
        } else {
            Some(body.to_string())
        },
        subtitle: None,
        countdown_target: None,
        actions: notif_actions,
        css_window_name: None,
        css_box_name: Some("fd-notification".to_string()),
        css_card_class: urgency_class,
        timeout_ms,
        source: NotificationSource::Freedesktop {
            fd_id,
            app_name: app_name.to_string(),
            desktop_entry,
            sender_pid,
        },
    }
}

/// Returns today's local midnight as a UTC datetime string (for SQL `created_at >= ?`).
/// This ensures timezone-correct "today" filtering since `created_at` is stored in UTC.
pub fn today_start_utc() -> String {
    let today_local = chrono::Local::now().date_naive();
    let midnight_local = today_local
        .and_hms_opt(0, 0, 0)
        .expect("valid midnight time");
    let midnight_utc = chrono::Local
        .from_local_datetime(&midnight_local)
        .unwrap()
        .with_timezone(&chrono::Utc);
    midnight_utc.format("%Y-%m-%d %H:%M:%S").to_string()
}

pub fn db_path() -> std::path::PathBuf {
    let data_dir = std::env::var("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            std::path::PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
                .join(".local/share")
        })
        .join("jb-shell");

    std::fs::create_dir_all(&data_dir).ok();

    data_dir.join("notifications.db")
}

fn open_db() -> Result<DbConnection, rusqlite::Error> {
    let db = DbConnection::open(db_path())?;

    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS notifications (
            id              INTEGER PRIMARY KEY,
            app_name        TEXT NOT NULL DEFAULT '',
            app_icon        TEXT NOT NULL DEFAULT '',
            summary         TEXT NOT NULL,
            body            TEXT NOT NULL DEFAULT '',
            urgency         INTEGER NOT NULL DEFAULT 1,
            category        TEXT,
            desktop_entry   TEXT,
            actions         TEXT,
            transient       INTEGER NOT NULL DEFAULT 0,
            resident        INTEGER NOT NULL DEFAULT 0,
            expire_timeout  INTEGER NOT NULL DEFAULT -1,
            created_at      TEXT NOT NULL DEFAULT (datetime('now')),
            closed_at       TEXT,
            close_reason    INTEGER
        );
        CREATE INDEX IF NOT EXISTS idx_notifications_app ON notifications(app_name);
        CREATE INDEX IF NOT EXISTS idx_notifications_created ON notifications(created_at);",
    )?;

    // Migration: add read column (silently fails if already exists)
    let _ =
        db.execute_batch("ALTER TABLE notifications ADD COLUMN read INTEGER NOT NULL DEFAULT 0;");

    Ok(db)
}

pub fn spawn_notification_daemon(
    notif_sender: relm4::Sender<NotificationInput>,
) -> mpsc::Sender<DaemonCommand> {
    let (cmd_tx, cmd_rx) = mpsc::channel::<DaemonCommand>();

    thread::spawn(move || {
        let db = match open_db() {
            Ok(db) => db,
            Err(e) => {
                eprintln!("jb-shell: notification daemon failed to open DB: {e}");
                return;
            }
        };

        // Seed next_id from DB
        let max_id: u32 = db
            .query_row(
                "SELECT COALESCE(MAX(id), 0) FROM notifications",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let next_id = AtomicU32::new(max_id + 1);

        let server = NotificationServer {
            notif_sender,
            db: Mutex::new(db),
            next_id,
        };

        let conn = match blocking::connection::Builder::session()
            .expect("failed to create session bus builder")
            .serve_at("/org/freedesktop/Notifications", server)
            .expect("failed to register notification interface")
            .name("org.freedesktop.Notifications")
            .expect("failed to set bus name")
            .build()
        {
            Ok(conn) => conn,
            Err(e) => {
                eprintln!("jb-shell: notification daemon failed to acquire bus name: {e}");
                return;
            }
        };

        eprintln!("jb-shell: notification daemon listening on D-Bus");

        // Keep a reference to the interface for signal emission and DB access
        let iface_ref = conn
            .object_server()
            .interface::<_, NotificationServer>("/org/freedesktop/Notifications")
            .expect("failed to get interface ref");

        // Process DaemonCommands from the UI thread.
        // zbus dispatches incoming D-Bus method calls on its own internal executor,
        // so blocking here on cmd_rx is fine.
        loop {
            match cmd_rx.recv() {
                Ok(DaemonCommand::NotificationClosed { id, reason }) => {
                    // Update DB with close info + read status
                    {
                        let iface = iface_ref.get();
                        let db = iface.db.lock();
                        if let Ok(db) = db {
                            if reason == 2 || reason == 3 {
                                // User dismissed/acted or caller closed — mark read
                                let _ = db.execute(
                                    "UPDATE notifications SET closed_at = datetime('now'), \
                                     close_reason = ?1, read = 1 WHERE id = ?2",
                                    rusqlite::params![reason, id],
                                );
                            } else if reason == 1 {
                                // Expired — unread only if had real actions
                                let _ = db.execute(
                                    "UPDATE notifications SET closed_at = datetime('now'), \
                                     close_reason = ?1, \
                                     read = CASE WHEN actions = '[]' OR actions IS NULL THEN 1 ELSE 0 END \
                                     WHERE id = ?2",
                                    rusqlite::params![reason, id],
                                );
                            } else {
                                let _ = db.execute(
                                    "UPDATE notifications SET closed_at = datetime('now'), \
                                     close_reason = ?1 WHERE id = ?2",
                                    rusqlite::params![reason, id],
                                );
                            }
                        }
                    }
                    // Emit D-Bus signal via raw connection API
                    let _ = conn.emit_signal(
                        None::<zbus::names::BusName>,
                        "/org/freedesktop/Notifications",
                        "org.freedesktop.Notifications",
                        "NotificationClosed",
                        &(id, reason),
                    );
                }
                Ok(DaemonCommand::ActionInvoked { id, action_key }) => {
                    let _ = conn.emit_signal(
                        None::<zbus::names::BusName>,
                        "/org/freedesktop/Notifications",
                        "org.freedesktop.Notifications",
                        "ActionInvoked",
                        &(id, action_key.as_str()),
                    );
                }
                Err(_) => break,
            }
        }
    });

    cmd_tx
}
