use crate::notification_daemon::DaemonCommand;
use crate::summary_thread::{SummaryResult, SummaryThreadMsg};
use crate::widgets::notifications::NotificationInput;
use gdk4::Monitor;
use gtk4::prelude::*;
use gtk4::{
    Box as GtkBox, Button, EventControllerFocus, Label, Orientation, ScrolledWindow, Window,
};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use relm4::prelude::*;
use rusqlite::Connection as DbConnection;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

pub struct NotificationCenterInit {
    pub monitor: Monitor,
    pub notif_sender: relm4::Sender<NotificationInput>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ViewMode {
    List,
    Summary,
}

pub struct NotificationCenterModel {
    unread_count: u32,
    popup_visible: bool,
    items: Vec<NotifItem>,
    daemon_tx: Option<mpsc::Sender<DaemonCommand>>,
    notif_sender: relm4::Sender<NotificationInput>,
    db: Option<DbConnection>,
    view_mode: ViewMode,
    summary_text: Option<String>,
    summary_loading: bool,
    summary_error: Option<String>,
    has_api_key: bool,
}

struct NotifItem {
    id: u32,
    app_name: String,
    summary: String,
    body: String,
    created_at: String,
    read: bool,
}

#[derive(Debug)]
pub enum NotificationCenterInput {
    SetDaemonChannel(mpsc::Sender<DaemonCommand>),
    TogglePopup,
    HidePopup,
    FocusLeave,
    FocusEnter,
    Refresh,
    NewNotification(u32),
    MarkAllRead,
    ClearAll,
    MarkItemRead(u32),
    ToggleViewMode,
    RefreshSummary,
    SummaryResult(SummaryResult),
}

pub struct NotificationCenterWidgets {
    trigger: Button,
    count_label: Label,
    popup: Window,
    popup_box: GtkBox,
    close_timer: Rc<RefCell<Option<glib::SourceId>>>,
    summary_thread_tx: tokio::sync::mpsc::Sender<SummaryThreadMsg>,
}

impl Component for NotificationCenterModel {
    type Init = NotificationCenterInit;
    type Input = NotificationCenterInput;
    type Output = ();
    type CommandOutput = ();
    type Root = GtkBox;
    type Widgets = NotificationCenterWidgets;

    fn init_root() -> Self::Root {
        let b = GtkBox::new(Orientation::Horizontal, 0);
        b.set_widget_name("notif-center");
        b.set_valign(gtk4::Align::Center);
        b
    }

    fn init(
        init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let monitor = init.monitor;
        let notif_sender = init.notif_sender;

        // Trigger button: bell icon + count
        let trigger_box = GtkBox::new(Orientation::Horizontal, 4);
        let icon_label = Label::new(Some("\u{f0f3}"));
        let count_label = Label::new(Some(""));
        trigger_box.append(&icon_label);
        trigger_box.append(&count_label);

        let trigger = Button::new();
        trigger.set_child(Some(&trigger_box));
        root.append(&trigger);

        let trigger_sender = sender.input_sender().clone();
        trigger.connect_clicked(move |_| {
            trigger_sender.emit(NotificationCenterInput::TogglePopup);
        });

        // Popup window
        let popup = Window::new();
        popup.set_widget_name("notif-center-popup-window");
        popup.init_layer_shell();
        popup.set_layer(Layer::Overlay);
        popup.set_exclusive_zone(-1);
        popup.set_anchor(Edge::Top, true);
        popup.set_anchor(Edge::Left, true);
        popup.set_keyboard_mode(KeyboardMode::OnDemand);
        popup.set_monitor(Some(&monitor));

        let popup_box = GtkBox::new(Orientation::Vertical, 2);
        popup_box.set_widget_name("notif-center-popup");
        popup.set_child(Some(&popup_box));
        popup.set_visible(false);

        // Focus handlers
        let focus = EventControllerFocus::new();
        let leave_sender = sender.input_sender().clone();
        focus.connect_leave(move |_| {
            leave_sender.emit(NotificationCenterInput::FocusLeave);
        });
        let enter_sender = sender.input_sender().clone();
        focus.connect_enter(move |_| {
            enter_sender.emit(NotificationCenterInput::FocusEnter);
        });
        popup.add_controller(focus);

        // Open read-only DB connection
        let db = DbConnection::open_with_flags(
            crate::notification_daemon::db_path(),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .ok();

        // 5-second refresh timer
        let refresh_sender = sender.input_sender().clone();
        glib::timeout_add_local(Duration::from_secs(5), move || {
            refresh_sender.emit(NotificationCenterInput::Refresh);
            glib::ControlFlow::Continue
        });

        // Spawn summary thread
        let summary_sender = sender.input_sender().clone();
        let summary_thread_tx = crate::summary_thread::spawn_summary_thread(move |result| {
            summary_sender.emit(NotificationCenterInput::SummaryResult(result));
        });

        // Initial count query
        let mut model = NotificationCenterModel {
            unread_count: 0,
            popup_visible: false,
            items: Vec::new(),
            daemon_tx: None,
            notif_sender,
            db,
            view_mode: ViewMode::List,
            summary_text: None,
            summary_loading: false,
            summary_error: None,
            has_api_key: true, // assume true until thread tells us otherwise
        };
        model.refresh_count();

        let widgets = NotificationCenterWidgets {
            trigger,
            count_label,
            popup,
            popup_box,
            close_timer: Rc::new(RefCell::new(None)),
            summary_thread_tx,
        };

        ComponentParts { model, widgets }
    }

    fn update_with_view(
        &mut self,
        widgets: &mut Self::Widgets,
        message: Self::Input,
        sender: ComponentSender<Self>,
        _root: &Self::Root,
    ) {
        match message {
            NotificationCenterInput::FocusLeave => {
                cancel_timer(&widgets.close_timer);
                let hide_sender = sender.input_sender().clone();
                let timer_ref = widgets.close_timer.clone();
                let id = glib::timeout_add_local_once(Duration::from_millis(500), move || {
                    hide_sender.emit(NotificationCenterInput::HidePopup);
                    *timer_ref.borrow_mut() = None;
                });
                *widgets.close_timer.borrow_mut() = Some(id);
                return;
            }
            NotificationCenterInput::FocusEnter => {
                cancel_timer(&widgets.close_timer);
                return;
            }
            NotificationCenterInput::SetDaemonChannel(tx) => {
                self.daemon_tx = Some(tx);
            }
            NotificationCenterInput::TogglePopup => {
                self.popup_visible = !self.popup_visible;
                if self.popup_visible {
                    self.refresh_items();
                    self.notif_sender
                        .emit(NotificationInput::SetCenterOpen(true));
                } else {
                    self.notif_sender
                        .emit(NotificationInput::SetCenterOpen(false));
                }
            }
            NotificationCenterInput::HidePopup => {
                if self.popup_visible {
                    self.popup_visible = false;
                    self.notif_sender
                        .emit(NotificationInput::SetCenterOpen(false));
                }
            }
            NotificationCenterInput::Refresh => {
                self.refresh_count();
                if self.popup_visible {
                    self.refresh_items();
                }
            }
            NotificationCenterInput::NewNotification(_fd_id) => {
                self.refresh_count();
                if self.popup_visible {
                    self.refresh_items();
                }
                let _ = widgets
                    .summary_thread_tx
                    .try_send(SummaryThreadMsg::NewNotification(_fd_id));
            }
            NotificationCenterInput::ToggleViewMode => {
                self.view_mode = match self.view_mode {
                    ViewMode::List => ViewMode::Summary,
                    ViewMode::Summary => ViewMode::List,
                };
                // Auto-trigger refresh when switching to summary with no text yet
                if self.view_mode == ViewMode::Summary
                    && self.summary_text.is_none()
                    && self.has_api_key
                    && !self.summary_loading
                {
                    let _ = widgets
                        .summary_thread_tx
                        .try_send(SummaryThreadMsg::ManualRefresh);
                }
            }
            NotificationCenterInput::RefreshSummary => {
                let _ = widgets
                    .summary_thread_tx
                    .try_send(SummaryThreadMsg::ManualRefresh);
            }
            NotificationCenterInput::SummaryResult(result) => match result {
                SummaryResult::Updated(text) => {
                    self.summary_text = Some(text);
                    self.summary_loading = false;
                    self.summary_error = None;
                }
                SummaryResult::Loading => {
                    self.summary_loading = true;
                    self.summary_error = None;
                }
                SummaryResult::Error(e) => {
                    self.summary_loading = false;
                    self.summary_error = Some(e);
                }
                SummaryResult::NoApiKey => {
                    self.has_api_key = false;
                    self.summary_loading = false;
                }
            },
            NotificationCenterInput::MarkAllRead => {
                if let Some(tx) = &self.daemon_tx {
                    let _ = tx.send(DaemonCommand::MarkAllRead);
                }
                // Small delay then refresh to let daemon process
                let refresh_sender = sender.input_sender().clone();
                glib::timeout_add_local_once(Duration::from_millis(50), move || {
                    refresh_sender.emit(NotificationCenterInput::Refresh);
                });
            }
            NotificationCenterInput::ClearAll => {
                if let Some(tx) = &self.daemon_tx {
                    let _ = tx.send(DaemonCommand::ClearAll);
                }
                self.popup_visible = false;
                self.notif_sender
                    .emit(NotificationInput::SetCenterOpen(false));
                let refresh_sender = sender.input_sender().clone();
                glib::timeout_add_local_once(Duration::from_millis(50), move || {
                    refresh_sender.emit(NotificationCenterInput::Refresh);
                });
            }
            NotificationCenterInput::MarkItemRead(id) => {
                if let Some(tx) = &self.daemon_tx {
                    let _ = tx.send(DaemonCommand::MarkRead { id });
                }
                let refresh_sender = sender.input_sender().clone();
                glib::timeout_add_local_once(Duration::from_millis(50), move || {
                    refresh_sender.emit(NotificationCenterInput::Refresh);
                });
            }
        }

        self.update_view(widgets, sender);
    }

    fn update_view(&self, widgets: &mut Self::Widgets, sender: ComponentSender<Self>) {
        // Update badge
        if self.unread_count > 0 {
            widgets
                .count_label
                .set_label(&self.unread_count.to_string());
            widgets.trigger.add_css_class("has-unread");
        } else {
            widgets.count_label.set_label("");
            widgets.trigger.remove_css_class("has-unread");
        }

        if self.popup_visible {
            self.rebuild_popup(widgets, &sender);
            position_popup(&widgets.popup, &widgets.trigger);
            widgets.popup.set_visible(true);
        } else {
            cancel_timer(&widgets.close_timer);
            widgets.popup.set_visible(false);
        }
    }
}

impl NotificationCenterModel {
    fn refresh_count(&mut self) {
        let Some(db) = &self.db else { return };
        let today = crate::notification_daemon::today_start_utc();
        self.unread_count = db
            .query_row(
                "SELECT COUNT(*) FROM notifications \
                 WHERE created_at >= ?1 AND read = 0",
                rusqlite::params![today],
                |row| row.get(0),
            )
            .unwrap_or(0);
    }

    fn refresh_items(&mut self) {
        let Some(db) = &self.db else { return };
        let today = crate::notification_daemon::today_start_utc();

        let mut stmt = match db.prepare(
            "SELECT id, app_name, summary, body, created_at, read \
             FROM notifications WHERE created_at >= ?1 \
             ORDER BY created_at DESC",
        ) {
            Ok(s) => s,
            Err(_) => return,
        };

        let items: Vec<NotifItem> = stmt
            .query_map(rusqlite::params![today], |row| {
                Ok(NotifItem {
                    id: row.get(0)?,
                    app_name: row.get(1)?,
                    summary: row.get(2)?,
                    body: row.get(3)?,
                    created_at: row.get(4)?,
                    read: row.get::<_, i32>(5)? != 0,
                })
            })
            .ok()
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default();

        drop(stmt);
        self.items = items;
        self.refresh_count();
    }

    fn rebuild_popup(&self, widgets: &NotificationCenterWidgets, sender: &ComponentSender<Self>) {
        while let Some(child) = widgets.popup_box.first_child() {
            widgets.popup_box.remove(&child);
        }

        // Header: title + view toggle button
        let header = GtkBox::new(Orientation::Horizontal, 0);
        header.set_widget_name("notif-center-popup-header");

        let title = Label::new(Some(match self.view_mode {
            ViewMode::List => "Notifications",
            ViewMode::Summary => "AI Summary",
        }));
        title.set_halign(gtk4::Align::Start);
        title.set_hexpand(true);
        header.append(&title);

        let toggle_icon = match self.view_mode {
            ViewMode::List => "\u{f0d0}",    // sparkles — switch to summary
            ViewMode::Summary => "\u{f03a}", // list — switch to list
        };
        let toggle_btn = Button::with_label(toggle_icon);
        toggle_btn.set_widget_name("notif-center-view-toggle");
        let toggle_sender = sender.input_sender().clone();
        toggle_btn.connect_clicked(move |_| {
            toggle_sender.emit(NotificationCenterInput::ToggleViewMode);
        });
        header.append(&toggle_btn);

        widgets.popup_box.append(&header);

        match self.view_mode {
            ViewMode::List => self.rebuild_list_view(widgets, sender),
            ViewMode::Summary => self.rebuild_summary_view(widgets, sender),
        }
    }

    fn rebuild_list_view(
        &self,
        widgets: &NotificationCenterWidgets,
        sender: &ComponentSender<Self>,
    ) {
        // Scrolled list
        let scroll = ScrolledWindow::new();
        scroll.set_vexpand(true);
        scroll.set_min_content_height(100);
        scroll.set_max_content_height(600);
        scroll.set_propagate_natural_height(true);

        let list_box = GtkBox::new(Orientation::Vertical, 2);

        if self.items.is_empty() {
            let empty = Label::new(Some("No notifications today"));
            empty.set_widget_name("notif-item");
            empty.add_css_class("read");
            empty.set_halign(gtk4::Align::Start);
            list_box.append(&empty);
        } else {
            for item in &self.items {
                let row = self.build_item_row(item, sender);
                list_box.append(&row);
            }
        }

        scroll.set_child(Some(&list_box));
        widgets.popup_box.append(&scroll);

        // Footer with action buttons
        let footer = GtkBox::new(Orientation::Horizontal, 8);
        footer.set_widget_name("notif-center-popup-footer");
        footer.set_halign(gtk4::Align::End);

        let mark_all_btn = Button::with_label("Mark all read");
        let mark_sender = sender.input_sender().clone();
        mark_all_btn.connect_clicked(move |_| {
            mark_sender.emit(NotificationCenterInput::MarkAllRead);
        });
        footer.append(&mark_all_btn);

        let clear_btn = Button::with_label("Clear all");
        let clear_sender = sender.input_sender().clone();
        clear_btn.connect_clicked(move |_| {
            clear_sender.emit(NotificationCenterInput::ClearAll);
        });
        footer.append(&clear_btn);

        widgets.popup_box.append(&footer);
    }

    fn rebuild_summary_view(
        &self,
        widgets: &NotificationCenterWidgets,
        sender: &ComponentSender<Self>,
    ) {
        let scroll = ScrolledWindow::new();
        scroll.set_vexpand(true);
        scroll.set_min_content_height(300);
        scroll.set_max_content_height(600);

        let summary_label = Label::new(None);
        summary_label.set_widget_name("notif-summary-text");
        summary_label.set_halign(gtk4::Align::Start);
        summary_label.set_valign(gtk4::Align::Start);
        summary_label.set_wrap(true);
        summary_label.set_wrap_mode(gtk4::pango::WrapMode::WordChar);

        if !self.has_api_key {
            summary_label.set_label(
                "Add API key to ~/.config/jb-shell/cerebras.json\n\
                 {\"api_key\": \"csk-...\"}",
            );
            summary_label.add_css_class("summary-setup");
        } else if self.summary_loading {
            summary_label.set_label("Generating summary...");
            summary_label.add_css_class("summary-loading");
        } else if let Some(ref err) = self.summary_error {
            summary_label.set_label(err);
            summary_label.add_css_class("summary-error");
        } else if let Some(ref text) = self.summary_text {
            summary_label.set_label(text);
        } else {
            summary_label.set_label("No summary yet. Click Refresh to generate.");
            summary_label.add_css_class("summary-loading");
        }

        scroll.set_child(Some(&summary_label));
        widgets.popup_box.append(&scroll);

        // Footer with Refresh button
        let footer = GtkBox::new(Orientation::Horizontal, 8);
        footer.set_widget_name("notif-center-popup-footer");
        footer.set_halign(gtk4::Align::End);

        if self.has_api_key {
            let refresh_btn = Button::with_label("Refresh");
            let refresh_sender = sender.input_sender().clone();
            refresh_btn.connect_clicked(move |_| {
                refresh_sender.emit(NotificationCenterInput::RefreshSummary);
            });
            if self.summary_loading {
                refresh_btn.set_sensitive(false);
            }
            footer.append(&refresh_btn);
        }

        widgets.popup_box.append(&footer);
    }

    fn build_item_row(&self, item: &NotifItem, sender: &ComponentSender<Self>) -> GtkBox {
        let row = GtkBox::new(Orientation::Vertical, 1);
        row.set_widget_name("notif-item");

        if item.read {
            row.add_css_class("read");
        } else {
            row.add_css_class("unread");
        }

        // Top line: app_name + relative time
        let top = GtkBox::new(Orientation::Horizontal, 0);
        let app_label = Label::new(Some(&item.app_name));
        app_label.add_css_class("notif-item-app");
        app_label.set_halign(gtk4::Align::Start);
        app_label.set_hexpand(true);

        let time_label = Label::new(Some(&format_relative_time(&item.created_at)));
        time_label.add_css_class("notif-item-time");
        time_label.set_halign(gtk4::Align::End);

        top.append(&app_label);
        top.append(&time_label);
        row.append(&top);

        // Summary
        let summary_label = Label::new(Some(&truncate_str(&item.summary, 50)));
        summary_label.add_css_class("notif-item-summary");
        summary_label.set_halign(gtk4::Align::Start);
        summary_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        row.append(&summary_label);

        // Body (if any, truncated)
        if !item.body.is_empty() {
            let body_label = Label::new(Some(&truncate_str(&item.body, 80)));
            body_label.add_css_class("notif-item-body");
            body_label.set_halign(gtk4::Align::Start);
            body_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            row.append(&body_label);
        }

        // Click handler to mark as read
        if !item.read {
            let click = gtk4::GestureClick::new();
            let item_id = item.id;
            let click_sender = sender.input_sender().clone();
            click.connect_released(move |_, _, _, _| {
                click_sender.emit(NotificationCenterInput::MarkItemRead(item_id));
            });
            row.add_controller(click);
        }

        row
    }
}

fn cancel_timer(timer: &Rc<RefCell<Option<glib::SourceId>>>) {
    if let Some(id) = timer.borrow_mut().take() {
        id.remove();
    }
}

fn position_popup(popup: &Window, trigger: &Button) {
    let Some(root) = trigger.root() else {
        popup.set_margin(Edge::Top, 32);
        return;
    };
    if let Some(bounds) = trigger.compute_bounds(root.upcast_ref::<gtk4::Widget>()) {
        popup.set_margin(Edge::Top, (bounds.y() + bounds.height()) as i32);

        let screen_w = root.width();
        let (_, popup_natural, _, _) = popup.measure(gtk4::Orientation::Horizontal, -1);
        let popup_w = popup_natural.max(340);
        let left = (bounds.x() as i32).min(screen_w - popup_w).max(0);
        popup.set_margin(Edge::Left, left);
    } else {
        popup.set_margin(Edge::Top, 32);
        popup.set_margin(Edge::Left, 0);
    }
}

fn format_relative_time(created_at: &str) -> String {
    let Ok(dt) = chrono::NaiveDateTime::parse_from_str(created_at, "%Y-%m-%d %H:%M:%S") else {
        return created_at.to_string();
    };
    // datetime('now') stores UTC — interpret as UTC then convert to local
    let created = dt.and_utc().with_timezone(&chrono::Local);
    let now = chrono::Local::now();
    let diff = now - created;

    let mins = diff.num_minutes();
    if mins < 1 {
        "just now".to_string()
    } else if mins < 60 {
        format!("{mins}m ago")
    } else {
        let hours = diff.num_hours();
        if hours < 24 {
            format!("{hours}h ago")
        } else {
            format!("{}d ago", diff.num_days())
        }
    }
}

fn truncate_str(s: &str, max_len: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_len {
        return s.to_string();
    }
    let end: usize = s
        .char_indices()
        .nth(max_len)
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    format!("{}...", &s[..end])
}
