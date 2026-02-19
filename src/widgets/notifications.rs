use chrono::{DateTime, Local};
use gdk4::Monitor;
use gtk4::prelude::*;
use gtk4::{Box as GtkBox, Button, Label, Orientation, Window};
use gtk4_layer_shell::{Edge, Layer, LayerShell};
use relm4::prelude::*;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

pub type NotificationId = u64;

#[derive(Clone, Debug)]
pub enum NotificationKind {
    Toast,
    Fullscreen,
}

#[derive(Clone, Debug)]
pub enum NotificationSource {
    Internal,
    Freedesktop { fd_id: u32 },
}

#[derive(Clone, Debug)]
pub enum ActionCallback {
    Dismiss,
    OpenUrl(String),
    FdAction { fd_id: u32, action_key: String },
}

#[derive(Clone, Debug)]
pub struct NotificationAction {
    pub label: String,
    pub css_class: String,
    pub callback: ActionCallback,
}

#[derive(Clone, Debug)]
pub struct NotificationRequest {
    pub id: NotificationId,
    pub kind: NotificationKind,
    pub icon: Option<String>,
    pub title: String,
    pub body: Option<String>,
    pub subtitle: Option<String>,
    pub countdown_target: Option<DateTime<Local>>,
    pub actions: Vec<NotificationAction>,
    pub css_window_name: Option<String>,
    pub css_box_name: Option<String>,
    pub css_card_class: Option<String>,
    pub timeout_ms: Option<u32>,
    pub source: NotificationSource,
}

#[derive(Debug)]
pub enum NotificationInput {
    Show(NotificationRequest),
    Dismiss(NotificationId),
    Tick,
    ActionTriggered(NotificationId, ActionCallback),
    SetDaemonChannel(std::sync::mpsc::Sender<crate::notification_daemon::DaemonCommand>),
}

pub struct NotificationModel {
    active: Vec<ActiveNotification>,
    daemon_tx: Option<std::sync::mpsc::Sender<crate::notification_daemon::DaemonCommand>>,
}

struct ActiveNotification {
    request: NotificationRequest,
    window: Window,
    title_label: Label,
    expires_at: Option<Instant>,
}

pub struct NotificationWidgets {
    monitor: Monitor,
}

impl Component for NotificationModel {
    type Init = Monitor;
    type Input = NotificationInput;
    type Output = ();
    type CommandOutput = ();
    type Root = GtkBox;
    type Widgets = NotificationWidgets;

    fn init_root() -> Self::Root {
        GtkBox::new(Orientation::Horizontal, 0)
    }

    fn init(
        monitor: Self::Init,
        _root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let tick_sender = sender.input_sender().clone();
        glib::timeout_add_local(Duration::from_secs(1), move || {
            tick_sender.emit(NotificationInput::Tick);
            glib::ControlFlow::Continue
        });

        let model = NotificationModel {
            active: Vec::new(),
            daemon_tx: None,
        };
        let widgets = NotificationWidgets { monitor };
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
            NotificationInput::Show(request) => {
                // Dismiss existing notification with same ID
                self.dismiss_by_id_with_reason(request.id, 0);

                let expires_at = request
                    .timeout_ms
                    .map(|ms| Instant::now() + Duration::from_millis(ms as u64));

                let window = build_notification_window(&widgets.monitor, &request, &sender);
                let title_label = find_title_label(&window);

                window.set_visible(true);

                self.active.push(ActiveNotification {
                    request,
                    window,
                    title_label,
                    expires_at,
                });

                restack_toasts(&self.active);
            }
            NotificationInput::Dismiss(id) => {
                self.dismiss_by_id_with_reason(id, 2);
                restack_toasts(&self.active);
            }
            NotificationInput::Tick => {
                let now_chrono = Local::now();
                let now_instant = Instant::now();
                let mut expired_ids = Vec::new();
                for notif in &self.active {
                    if let Some(target) = notif.request.countdown_target {
                        notif
                            .title_label
                            .set_label(&format_countdown(target, now_chrono));
                    }
                    if let Some(exp) = notif.expires_at {
                        if now_instant >= exp {
                            expired_ids.push(notif.request.id);
                        }
                    }
                }
                if !expired_ids.is_empty() {
                    for id in expired_ids {
                        self.dismiss_by_id_with_reason(id, 1);
                    }
                    restack_toasts(&self.active);
                }
            }
            NotificationInput::ActionTriggered(id, callback) => {
                match &callback {
                    ActionCallback::Dismiss => {}
                    ActionCallback::OpenUrl(url) => {
                        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
                    }
                    ActionCallback::FdAction {
                        fd_id,
                        action_key,
                    } => {
                        if let Some(tx) = &self.daemon_tx {
                            let _ = tx.send(
                                crate::notification_daemon::DaemonCommand::ActionInvoked {
                                    id: *fd_id,
                                    action_key: action_key.clone(),
                                },
                            );
                        }
                    }
                }
                self.dismiss_by_id_with_reason(id, 2);
                restack_toasts(&self.active);
            }
            NotificationInput::SetDaemonChannel(tx) => {
                self.daemon_tx = Some(tx);
            }
        }
    }
}

impl NotificationModel {
    fn dismiss_by_id_with_reason(&mut self, id: NotificationId, reason: u32) {
        let mut i = 0;
        while i < self.active.len() {
            if self.active[i].request.id == id {
                let notif = self.active.remove(i);
                notif.window.set_visible(false);
                if reason > 0 {
                    if let NotificationSource::Freedesktop { fd_id } = notif.request.source {
                        if let Some(tx) = &self.daemon_tx {
                            let _ = tx.send(
                                crate::notification_daemon::DaemonCommand::NotificationClosed {
                                    id: fd_id,
                                    reason,
                                },
                            );
                        }
                    }
                }
            } else {
                i += 1;
            }
        }
    }
}

fn build_notification_window(
    monitor: &Monitor,
    request: &NotificationRequest,
    sender: &ComponentSender<NotificationModel>,
) -> Window {
    let window = Window::new();
    window.init_layer_shell();
    window.set_layer(Layer::Overlay);
    window.set_exclusive_zone(-1);
    window.set_monitor(Some(monitor));
    match &request.kind {
        NotificationKind::Toast => {
            window.set_anchor(Edge::Top, true);
            window.set_anchor(Edge::Right, true);
            window.set_margin(Edge::Top, 8);
            window.set_margin(Edge::Right, 8);

            let inner = GtkBox::new(Orientation::Vertical, 4);
            if let Some(name) = &request.css_box_name {
                inner.set_widget_name(name);
            }
            if let Some(class) = &request.css_card_class {
                inner.add_css_class(class);
            }

            build_notification_content(&inner, request, sender);
            window.set_child(Some(&inner));
        }
        NotificationKind::Fullscreen => {
            window.set_anchor(Edge::Top, true);
            window.set_anchor(Edge::Bottom, true);
            window.set_anchor(Edge::Left, true);
            window.set_anchor(Edge::Right, true);

            if let Some(name) = &request.css_window_name {
                window.set_widget_name(name);
            }

            let inner = GtkBox::new(Orientation::Vertical, 12);
            if let Some(class) = &request.css_card_class {
                inner.add_css_class(class);
            }
            inner.set_halign(gtk4::Align::Center);
            inner.set_valign(gtk4::Align::Center);

            build_notification_content(&inner, request, sender);
            window.set_child(Some(&inner));
        }
    }

    window
}

fn build_notification_content(
    container: &GtkBox,
    request: &NotificationRequest,
    sender: &ComponentSender<NotificationModel>,
) {
    if let Some(icon) = &request.icon {
        let icon_label = Label::new(Some(icon));
        icon_label.add_css_class("fs-icon");
        container.append(&icon_label);
    }

    let title_label = Label::new(Some(&request.title));
    title_label.add_css_class("notif-title-label");
    container.append(&title_label);

    if let Some(body) = &request.body {
        let body_label = Label::new(Some(body));
        body_label.add_css_class("notif-event");
        container.append(&body_label);
    }

    if let Some(subtitle) = &request.subtitle {
        let sub_label = Label::new(Some(subtitle));
        sub_label.add_css_class("fs-time");
        container.append(&sub_label);
    }

    if !request.actions.is_empty() {
        let button_row = GtkBox::new(Orientation::Horizontal, 8);
        if matches!(request.kind, NotificationKind::Fullscreen) {
            button_row.set_halign(gtk4::Align::Center);
        }

        for action in &request.actions {
            let btn = Button::with_label(&action.label);
            btn.add_css_class(&action.css_class);

            let id = request.id;
            let cb = action.callback.clone();
            let action_sender = sender.input_sender().clone();
            btn.connect_clicked(move |_| {
                action_sender.emit(NotificationInput::ActionTriggered(id, cb.clone()));
            });

            button_row.append(&btn);
        }

        container.append(&button_row);
    }
}

fn find_title_label(window: &Window) -> Label {
    // The title label is the one with css class "notif-title-label" inside the window's child box.
    // We walk the children to find it.
    if let Some(inner) = window.child().and_then(|c| c.downcast::<GtkBox>().ok()) {
        let mut child = inner.first_child();
        while let Some(widget) = child {
            if let Ok(label) = widget.clone().downcast::<Label>() {
                if label.has_css_class("notif-title-label") {
                    return label;
                }
            }
            child = widget.next_sibling();
        }
    }
    // Fallback — should never happen
    Label::new(None)
}

const BAR_HEIGHT_OFFSET: i32 = 40; // ~31px bar + 8px gap + 1px breathing room

fn restack_toasts(active: &[ActiveNotification]) {
    let mut top_offset = BAR_HEIGHT_OFFSET;
    for notif in active {
        match &notif.request.kind {
            NotificationKind::Toast => {
                notif.window.set_margin(Edge::Top, top_offset);
                let (_, natural, _, _) = notif.window.measure(gtk4::Orientation::Vertical, -1);
                let height = natural.max(60);
                top_offset += height + 8;
            }
            NotificationKind::Fullscreen => {}
        }
    }
}

pub fn format_countdown(target: DateTime<Local>, now: DateTime<Local>) -> String {
    let secs = (target - now).num_seconds().max(0);
    let mins = secs / 60;
    let remaining_secs = secs % 60;
    if mins > 0 {
        format!("Starting in {}m {}s", mins, remaining_secs)
    } else {
        format!("Starting in {}s", remaining_secs)
    }
}

pub fn hash_event_id(event_id: &str, suffix: &str) -> NotificationId {
    let mut hasher = DefaultHasher::new();
    event_id.hash(&mut hasher);
    suffix.hash(&mut hasher);
    hasher.finish()
}
