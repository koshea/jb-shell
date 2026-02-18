use crate::google_calendar::{self, CalendarEvent, CalendarResult, CalendarThreadMsg};
use crate::widgets::notifications::{
    ActionCallback, NotificationAction, NotificationInput, NotificationKind,
    NotificationRequest, format_countdown, hash_event_id,
};
use chrono::Local;
use gdk4::Monitor;
use gtk4::prelude::*;
use gtk4::{Box as GtkBox, Button, EventControllerFocus, Label, Orientation, Window};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use relm4::prelude::*;
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use tokio::sync::mpsc;
use std::time::Duration;

pub struct CalendarInit {
    pub monitor: Monitor,
    pub notif_sender: relm4::Sender<NotificationInput>,
}

pub struct CalendarModel {
    events: Vec<CalendarEvent>,
    authenticated: bool,
    auth_in_progress: bool,
    has_credentials: bool,
    notified_5min: HashSet<String>,
    notified_1min: HashSet<String>,
    popup_visible: bool,
    notif_sender: relm4::Sender<NotificationInput>,
}

#[derive(Debug)]
pub enum CalendarInput {
    EventsUpdated(Vec<CalendarEvent>),
    AuthComplete,
    AuthFailed(String),
    AuthRevoked,
    NeedsAuth,
    NoCredentials,
    TogglePopup,
    HidePopup,
    FocusLeave,
    FocusEnter,
    CheckNotifications,
}

pub struct CalendarWidgets {
    trigger: Button,
    indicator_label: Label,
    popup: Window,
    popup_box: GtkBox,
    close_timer: Rc<RefCell<Option<glib::SourceId>>>,
    thread_tx: mpsc::Sender<CalendarThreadMsg>,
}

impl Component for CalendarModel {
    type Init = CalendarInit;
    type Input = CalendarInput;
    type Output = ();
    type CommandOutput = ();
    type Root = GtkBox;
    type Widgets = CalendarWidgets;

    fn init_root() -> Self::Root {
        let b = GtkBox::new(Orientation::Horizontal, 0);
        b.set_widget_name("calendar-indicator");
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

        // Trigger button
        let trigger_box = GtkBox::new(Orientation::Horizontal, 4);
        let icon_label = Label::new(Some("\u{f073}"));
        let indicator_label = Label::new(Some("..."));
        trigger_box.append(&icon_label);
        trigger_box.append(&indicator_label);

        let trigger = Button::new();
        trigger.set_child(Some(&trigger_box));
        root.append(&trigger);

        // Trigger click
        let trigger_sender = sender.input_sender().clone();
        trigger.connect_clicked(move |_| {
            trigger_sender.emit(CalendarInput::TogglePopup);
        });

        // Event list popup
        let popup = Window::new();
        popup.set_widget_name("calendar-popup-window");
        popup.init_layer_shell();
        popup.set_layer(Layer::Overlay);
        popup.set_exclusive_zone(-1);
        popup.set_anchor(Edge::Top, true);
        popup.set_anchor(Edge::Left, true);
        popup.set_keyboard_mode(KeyboardMode::OnDemand);
        popup.set_monitor(Some(&monitor));

        let popup_box = GtkBox::new(Orientation::Vertical, 2);
        popup_box.set_widget_name("calendar-popup");
        popup.set_child(Some(&popup_box));
        popup.set_visible(false);

        // Focus handlers on popup
        let focus = EventControllerFocus::new();
        let leave_sender = sender.input_sender().clone();
        focus.connect_leave(move |_| {
            leave_sender.emit(CalendarInput::FocusLeave);
        });
        let enter_sender = sender.input_sender().clone();
        focus.connect_enter(move |_| {
            enter_sender.emit(CalendarInput::FocusEnter);
        });
        popup.add_controller(focus);

        // Spawn calendar thread
        let input_sender = sender.input_sender().clone();
        let thread_tx = google_calendar::spawn_calendar_thread(move |result| {
            match result {
                CalendarResult::EventsUpdated(e) => {
                    input_sender.emit(CalendarInput::EventsUpdated(e))
                }
                CalendarResult::AuthComplete => input_sender.emit(CalendarInput::AuthComplete),
                CalendarResult::AuthFailed(s) => input_sender.emit(CalendarInput::AuthFailed(s)),
                CalendarResult::AuthRevoked => input_sender.emit(CalendarInput::AuthRevoked),
                CalendarResult::NeedsAuth => input_sender.emit(CalendarInput::NeedsAuth),
                CalendarResult::NoCredentials => input_sender.emit(CalendarInput::NoCredentials),
            }
        });

        // 1-second notification check timer
        let check_sender = sender.input_sender().clone();
        glib::timeout_add_local(Duration::from_secs(1), move || {
            check_sender.emit(CalendarInput::CheckNotifications);
            glib::ControlFlow::Continue
        });

        let model = CalendarModel {
            events: Vec::new(),
            authenticated: false,
            auth_in_progress: false,
            has_credentials: true,
            notified_5min: HashSet::new(),
            notified_1min: HashSet::new(),
            popup_visible: false,
            notif_sender,
        };

        let widgets = CalendarWidgets {
            trigger,
            indicator_label,
            popup,
            popup_box,
            close_timer: Rc::new(RefCell::new(None)),
            thread_tx,
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
            CalendarInput::FocusLeave => {
                cancel_timer(&widgets.close_timer);
                let hide_sender = sender.input_sender().clone();
                let timer_ref = widgets.close_timer.clone();
                let id =
                    glib::timeout_add_local_once(Duration::from_millis(500), move || {
                        hide_sender.emit(CalendarInput::HidePopup);
                        *timer_ref.borrow_mut() = None;
                    });
                *widgets.close_timer.borrow_mut() = Some(id);
                return;
            }
            CalendarInput::FocusEnter => {
                cancel_timer(&widgets.close_timer);
                return;
            }
            CalendarInput::CheckNotifications => {
                self.check_notifications();
                return;
            }
            CalendarInput::EventsUpdated(events) => {
                // Clear notifications for removed or rescheduled events
                let old_times: std::collections::HashMap<&str, _> = self
                    .events
                    .iter()
                    .map(|e| (e.id.as_str(), e.start))
                    .collect();
                let new_times: std::collections::HashMap<&str, _> =
                    events.iter().map(|e| (e.id.as_str(), e.start)).collect();
                self.notified_5min.retain(|id| {
                    new_times.get(id.as_str()) == old_times.get(id.as_str())
                });
                self.notified_1min.retain(|id| {
                    new_times.get(id.as_str()) == old_times.get(id.as_str())
                });
                self.events = events;
                self.authenticated = true;
            }
            CalendarInput::AuthComplete => {
                self.authenticated = true;
                self.auth_in_progress = false;
            }
            CalendarInput::AuthFailed(e) => {
                self.auth_in_progress = false;
                eprintln!("jb-shell: Google auth failed: {e}");
            }
            CalendarInput::AuthRevoked => {
                self.authenticated = false;
            }
            CalendarInput::NeedsAuth => {
                self.authenticated = false;
            }
            CalendarInput::NoCredentials => {
                self.has_credentials = false;
            }
            CalendarInput::TogglePopup => {
                if !self.has_credentials {
                    // Show setup instructions in popup
                    self.popup_visible = !self.popup_visible;
                    if self.popup_visible {
                        show_setup_instructions(widgets);
                        position_popup(&widgets.popup, &widgets.trigger);
                        widgets.popup.set_visible(true);
                    } else {
                        widgets.popup.set_visible(false);
                    }
                    return;
                }
                if !self.authenticated && !self.auth_in_progress {
                    self.auth_in_progress = true;
                    let _ = widgets.thread_tx.try_send(CalendarThreadMsg::TriggerAuth);
                    self.update_view(widgets, sender);
                    return;
                }
                self.popup_visible = !self.popup_visible;
            }
            CalendarInput::HidePopup => {
                self.popup_visible = false;
            }
        }

        self.update_view(widgets, sender);
    }

    fn update_view(&self, widgets: &mut Self::Widgets, sender: ComponentSender<Self>) {
        let now = Local::now();

        // Update indicator label
        if !self.has_credentials {
            widgets.indicator_label.set_label("No Config");
            set_trigger_class(&widgets.trigger, "calendar-error");
        } else if !self.authenticated && self.auth_in_progress {
            widgets.indicator_label.set_label("...");
            set_trigger_class(&widgets.trigger, "");
        } else if !self.authenticated {
            widgets.indicator_label.set_label("Connect");
            set_trigger_class(&widgets.trigger, "calendar-connect");
        } else {
            let upcoming: Vec<&CalendarEvent> = self
                .events
                .iter()
                .filter(|e| !e.is_all_day && e.end > now)
                .collect();

            let in_meeting = upcoming.iter().any(|e| e.start <= now && e.end > now);

            if in_meeting {
                widgets.indicator_label.set_label("Meeting");
                set_trigger_class(&widgets.trigger, "calendar-active");
            } else if let Some(next) = upcoming.iter().find(|e| e.start > now) {
                let mins = (next.start - now).num_minutes();
                if mins < 10 {
                    widgets.indicator_label.set_label(&format!("{mins}m"));
                    set_trigger_class(&widgets.trigger, "calendar-soon");
                } else {
                    let count = upcoming.iter().filter(|e| e.start > now).count();
                    widgets.indicator_label.set_label(&count.to_string());
                    set_trigger_class(&widgets.trigger, "");
                }
            } else {
                widgets.indicator_label.set_label("Free");
                set_trigger_class(&widgets.trigger, "");
            }
        }

        // Update popup
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

impl CalendarModel {
    fn rebuild_popup(&self, widgets: &CalendarWidgets, _sender: &ComponentSender<Self>) {
        while let Some(child) = widgets.popup_box.first_child() {
            widgets.popup_box.remove(&child);
        }

        let now = Local::now();

        let header = Label::new(Some(&format!("Today \u{b7} {}", now.format("%a %b %-d"))));
        header.set_widget_name("calendar-popup-header");
        header.set_halign(gtk4::Align::Start);
        widgets.popup_box.append(&header);

        let mut upcoming_count = 0;
        for event in &self.events {
            if event.start > now && !event.is_all_day {
                upcoming_count += 1;
            }

            let time_str = if event.is_all_day {
                "All day".to_string()
            } else {
                event.start.format("%H:%M").to_string()
            };

            let duration_str = if event.is_all_day {
                String::new()
            } else {
                let mins = (event.end - event.start).num_minutes();
                if mins >= 60 {
                    let h = mins / 60;
                    let m = mins % 60;
                    if m > 0 {
                        format!("{h}h{m}m")
                    } else {
                        format!("{h}h")
                    }
                } else {
                    format!("{mins}m")
                }
            };

            let status = if event.is_all_day {
                " "
            } else if event.end <= now {
                "\u{2713}"
            } else if event.start <= now {
                "\u{25cf}"
            } else {
                " "
            };

            let label_text = format!(
                "{status} {time_str}  {}  {duration_str}",
                truncate_title(&event.title, 24)
            );

            let btn = Button::with_label(&label_text);
            btn.set_widget_name("calendar-event-item");

            if event.end <= now && !event.is_all_day {
                btn.add_css_class("past");
            } else if event.start <= now && event.end > now && !event.is_all_day {
                btn.add_css_class("current");
            }

            if let Some(url) = event.meeting_link.clone() {
                let notif_sender = self.notif_sender.clone();
                let notif_id = hash_event_id(&event.id, "popup-join");
                btn.connect_clicked(move |_| {
                    notif_sender.emit(NotificationInput::ActionTriggered(
                        notif_id,
                        ActionCallback::OpenUrl(url.clone()),
                    ));
                });
            }

            widgets.popup_box.append(&btn);
        }

        let footer = Label::new(Some(&format!("{upcoming_count} upcoming")));
        footer.set_widget_name("calendar-popup-footer");
        footer.set_halign(gtk4::Align::Start);
        widgets.popup_box.append(&footer);
    }

    fn check_notifications(&mut self) {
        let now = Local::now();

        for event in &self.events {
            if event.is_all_day || event.start <= now {
                continue;
            }

            let secs_until = (event.start - now).num_seconds();

            if secs_until <= 310 && secs_until > 0 && secs_until % 30 == 0 {
                eprintln!(
                    "jb-shell: notif check: {} in {}s, 5min_notified={}, 1min_notified={}",
                    event.title, secs_until,
                    self.notified_5min.contains(&event.id),
                    self.notified_1min.contains(&event.id),
                );
            }

            if secs_until <= 300 && !self.notified_5min.contains(&event.id) {
                eprintln!("jb-shell: firing 5min notification for {}", event.title);
                self.notified_5min.insert(event.id.clone());
                self.notif_sender.emit(NotificationInput::Show(
                    self.build_5min_notification(event),
                ));
            }
            if secs_until <= 60 && !self.notified_1min.contains(&event.id) {
                eprintln!("jb-shell: firing 1min notification for {}", event.title);
                self.notified_1min.insert(event.id.clone());
                // Dismiss the 5-min toast for this event before showing fullscreen
                let toast_id = hash_event_id(&event.id, "5min");
                self.notif_sender.emit(NotificationInput::Dismiss(toast_id));
                if !is_meeting_focused() {
                    self.notif_sender.emit(NotificationInput::Show(
                        self.build_fullscreen_notification(event),
                    ));
                }
            }
        }
    }

    fn build_5min_notification(&self, event: &CalendarEvent) -> NotificationRequest {
        let id = hash_event_id(&event.id, "5min");
        let now = Local::now();
        let title = format_countdown(event.start, now);
        let body = format!(
            "{} \u{b7} {}-{}",
            event.title,
            event.start.format("%H:%M"),
            event.end.format("%H:%M")
        );

        let mut actions = Vec::new();
        if let Some(url) = &event.meeting_link {
            actions.push(NotificationAction {
                label: "Join Meeting".to_string(),
                css_class: "join-btn".to_string(),
                callback: ActionCallback::OpenUrl(url.clone()),
            });
        }
        actions.push(NotificationAction {
            label: "Dismiss".to_string(),
            css_class: "dismiss-btn".to_string(),
            callback: ActionCallback::Dismiss,
        });

        NotificationRequest {
            id,
            kind: NotificationKind::Toast,
            icon: None,
            title,
            body: Some(body),
            subtitle: None,
            countdown_target: Some(event.start),
            actions,
            css_window_name: None,
            css_box_name: Some("calendar-notif".to_string()),
            css_card_class: None,
        }
    }

    fn build_fullscreen_notification(&self, event: &CalendarEvent) -> NotificationRequest {
        let id = hash_event_id(&event.id, "1min");
        let now = Local::now();
        let title = format_countdown(event.start, now);
        let subtitle = format!(
            "{} - {}",
            event.start.format("%H:%M"),
            event.end.format("%H:%M")
        );

        let mut actions = Vec::new();
        if let Some(url) = &event.meeting_link {
            actions.push(NotificationAction {
                label: "Join Meeting".to_string(),
                css_class: "join-btn".to_string(),
                callback: ActionCallback::OpenUrl(url.clone()),
            });
        }
        actions.push(NotificationAction {
            label: "Dismiss".to_string(),
            css_class: "dismiss-btn".to_string(),
            callback: ActionCallback::Dismiss,
        });

        NotificationRequest {
            id,
            kind: NotificationKind::Fullscreen,
            icon: Some("\u{f073}".to_string()),
            title,
            body: Some(event.title.clone()),
            subtitle: Some(subtitle),
            countdown_target: Some(event.start),
            actions,
            css_window_name: Some("calendar-fullscreen".to_string()),
            css_box_name: None,
            css_card_class: Some("fullscreen-card".to_string()),
        }
    }
}

fn show_setup_instructions(widgets: &CalendarWidgets) {
    while let Some(child) = widgets.popup_box.first_child() {
        widgets.popup_box.remove(&child);
    }

    let cred_path = crate::google_calendar::credentials_path();

    let header = Label::new(Some("Calendar Setup"));
    header.set_widget_name("calendar-popup-header");
    header.set_halign(gtk4::Align::Start);
    widgets.popup_box.append(&header);

    let steps = [
        "1. Create a Google Cloud project",
        "2. Enable the Google Calendar API",
        "3. Create Desktop OAuth credentials",
        "4. Download the JSON file to:",
    ];
    for step in &steps {
        let l = Label::new(Some(step));
        l.set_widget_name("calendar-event-item");
        l.set_halign(gtk4::Align::Start);
        widgets.popup_box.append(&l);
    }

    let path_label = Label::new(Some(&format!("   {}", cred_path.display())));
    path_label.set_widget_name("calendar-event-item");
    path_label.add_css_class("current");
    path_label.set_halign(gtk4::Align::Start);
    path_label.set_selectable(true);
    widgets.popup_box.append(&path_label);

    let footer = Label::new(Some("Then restart jb-shell"));
    footer.set_widget_name("calendar-popup-footer");
    footer.set_halign(gtk4::Align::Start);
    widgets.popup_box.append(&footer);
}

fn cancel_timer(timer: &Rc<RefCell<Option<glib::SourceId>>>) {
    if let Some(id) = timer.borrow_mut().take() {
        id.remove();
    }
}

fn set_trigger_class(trigger: &Button, class: &str) {
    for c in &[
        "calendar-error",
        "calendar-connect",
        "calendar-soon",
        "calendar-active",
    ] {
        trigger.remove_css_class(c);
    }
    if !class.is_empty() {
        trigger.add_css_class(class);
    }
}

fn truncate_title(title: &str, max_len: usize) -> String {
    let char_count = title.chars().count();
    if char_count <= max_len {
        return title.to_string();
    }
    let end: usize = title
        .char_indices()
        .nth(max_len)
        .map(|(i, _)| i)
        .unwrap_or(title.len());
    format!("{}...", &title[..end])
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
        let popup_w = popup_natural.max(200);
        let left = (bounds.x() as i32).min(screen_w - popup_w).max(0);
        popup.set_margin(Edge::Left, left);
    } else {
        popup.set_margin(Edge::Top, 32);
        popup.set_margin(Edge::Left, 0);
    }
}

fn is_meeting_focused() -> bool {
    use hyprland::shared::HyprDataActiveOptional;
    let active = hyprland::data::Client::get_active().ok().flatten();
    let Some(client) = active else {
        return false;
    };
    let class = client.class.to_lowercase();
    let title = client.title.to_lowercase();

    let browsers = ["firefox", "google-chrome", "chromium", "brave"];
    let meeting_urls = ["meet.google.com", "zoom.us", "teams.microsoft.com"];
    if browsers.iter().any(|b| class.contains(b))
        && meeting_urls.iter().any(|k| title.contains(k))
    {
        return true;
    }

    let meeting_classes = ["zoom", "teams", "slack"];
    meeting_classes.iter().any(|c| class.contains(c))
}
