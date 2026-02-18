use gdk4::Monitor;
use gtk4::prelude::*;
use gtk4::{Box as GtkBox, Button, EventControllerFocus, Label, Orientation, Window};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use relm4::prelude::*;
use std::cell::RefCell;
use std::process::Command;
use std::rc::Rc;
use std::time::Duration;

fn truncate_middle(name: &str, max_len: usize) -> String {
    let char_count = name.chars().count();
    if char_count <= max_len {
        return name.to_string();
    }
    let keep = (max_len - 3) / 2;
    let tail_len = max_len - 3 - keep;
    let start_end: usize = name
        .char_indices()
        .nth(keep)
        .map(|(i, _)| i)
        .unwrap_or(name.len());
    let tail_start: usize = name
        .char_indices()
        .nth(char_count - tail_len)
        .map(|(i, _)| i)
        .unwrap_or(0);
    format!("{}...{}", &name[..start_end], &name[tail_start..])
}

pub struct KubeModel {
    current: String,
    contexts: Vec<String>,
    popup_visible: bool,
}

#[derive(Debug)]
pub enum KubeInput {
    PollResult {
        current: String,
        contexts: Vec<String>,
    },
    SwitchContext(String),
    TogglePopup,
    HidePopup,
    FocusLeave,
    FocusEnter,
}

pub struct KubeWidgets {
    context_label: Label,
    trigger: Button,
    popup: Window,
    popup_box: GtkBox,
    close_timer: Rc<RefCell<Option<glib::SourceId>>>,
}

impl Component for KubeModel {
    type Init = Monitor;
    type Input = KubeInput;
    type Output = ();
    type CommandOutput = ();
    type Root = GtkBox;
    type Widgets = KubeWidgets;

    fn init_root() -> Self::Root {
        let b = GtkBox::new(Orientation::Horizontal, 0);
        b.set_widget_name("kube-context");
        b.set_valign(gtk4::Align::Center);
        b
    }

    fn init(
        monitor: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        // Trigger button
        let trigger_box = GtkBox::new(Orientation::Horizontal, 4);
        let helm_label = Label::new(Some("\u{2388}"));
        helm_label.set_css_classes(&["kube-helm"]);
        let context_label = Label::new(Some("no context"));

        trigger_box.append(&helm_label);
        trigger_box.append(&context_label);

        let trigger = Button::new();
        trigger.set_widget_name("kube-trigger");
        trigger.set_child(Some(&trigger_box));
        root.append(&trigger);

        // Trigger click
        let trigger_sender = sender.input_sender().clone();
        trigger.connect_clicked(move |_| {
            trigger_sender.emit(KubeInput::TogglePopup);
        });

        // Popup window — layer shell overlay on same monitor as bar
        let popup = Window::new();
        popup.set_widget_name("kube-popup");
        popup.init_layer_shell();
        popup.set_layer(Layer::Overlay);
        popup.set_exclusive_zone(-1); // ignore bar's exclusive zone, position from screen edge
        popup.set_anchor(Edge::Top, true);
        popup.set_anchor(Edge::Left, true);
        popup.set_keyboard_mode(KeyboardMode::OnDemand);
        popup.set_monitor(Some(&monitor));

        let popup_box = GtkBox::new(Orientation::Vertical, 2);
        popup_box.set_widget_name("kube-menu");
        popup.set_child(Some(&popup_box));
        popup.set_visible(false);

        // Focus handlers on popup
        let focus = EventControllerFocus::new();
        let leave_sender = sender.input_sender().clone();
        focus.connect_leave(move |_| {
            leave_sender.emit(KubeInput::FocusLeave);
        });
        let enter_sender = sender.input_sender().clone();
        focus.connect_enter(move |_| {
            enter_sender.emit(KubeInput::FocusEnter);
        });
        popup.add_controller(focus);

        // Background polling thread
        let input_sender = sender.input_sender().clone();
        std::thread::spawn(move || loop {
            let (current, contexts) = poll_kube();
            input_sender.emit(KubeInput::PollResult { current, contexts });
            std::thread::sleep(Duration::from_secs(5));
        });

        let model = KubeModel {
            current: String::new(),
            contexts: Vec::new(),
            popup_visible: false,
        };
        let close_timer = Rc::new(RefCell::new(None));
        let widgets = KubeWidgets {
            context_label,
            trigger,
            popup,
            popup_box,
            close_timer,
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
            KubeInput::FocusLeave => {
                cancel_close_timer(&widgets.close_timer);
                let hide_sender = sender.input_sender().clone();
                let timer_ref = widgets.close_timer.clone();
                let id = glib::timeout_add_local_once(
                    std::time::Duration::from_millis(500),
                    move || {
                        hide_sender.emit(KubeInput::HidePopup);
                        *timer_ref.borrow_mut() = None;
                    },
                );
                *widgets.close_timer.borrow_mut() = Some(id);
                return;
            }
            KubeInput::FocusEnter => {
                cancel_close_timer(&widgets.close_timer);
                return;
            }
            other => {
                match other {
                    KubeInput::PollResult { current, contexts } => {
                        self.current = current;
                        self.contexts = contexts;
                    }
                    KubeInput::SwitchContext(name) => {
                        self.current = name.clone();
                        self.popup_visible = false;
                        std::thread::spawn(move || {
                            let _ = Command::new("kubectl")
                                .args(["config", "use-context", &name])
                                .output();
                        });
                    }
                    KubeInput::TogglePopup => {
                        self.popup_visible = !self.popup_visible;
                    }
                    KubeInput::HidePopup => {
                        self.popup_visible = false;
                    }
                    _ => unreachable!(),
                }
            }
        }

        self.update_view(widgets, sender);
    }

    fn update_view(&self, widgets: &mut Self::Widgets, sender: ComponentSender<Self>) {
        // Update context label
        widgets.context_label.set_label(&if self.current.is_empty() {
            "no context".to_string()
        } else {
            truncate_middle(&self.current, 24)
        });

        if self.popup_visible {
            // Rebuild menu items
            while let Some(child) = widgets.popup_box.first_child() {
                widgets.popup_box.remove(&child);
            }
            for ctx in &self.contexts {
                let is_active = *ctx == self.current;
                let label = if is_active {
                    format!("  \u{2713}  {ctx}")
                } else {
                    format!("      {ctx}")
                };
                let btn = Button::with_label(&label);
                btn.set_widget_name("kube-menu-item");
                if is_active {
                    btn.add_css_class("active");
                }
                let ctx_name = ctx.clone();
                let switch_sender = sender.input_sender().clone();
                btn.connect_clicked(move |_| {
                    switch_sender.emit(KubeInput::SwitchContext(ctx_name.clone()));
                });
                widgets.popup_box.append(&btn);
            }

            // Position popup below the trigger
            position_popup(&widgets.popup, &widgets.trigger);
            widgets.popup.set_visible(true);
        } else {
            cancel_close_timer(&widgets.close_timer);
            widgets.popup.set_visible(false);
        }
    }
}

fn cancel_close_timer(timer: &Rc<RefCell<Option<glib::SourceId>>>) {
    if let Some(id) = timer.borrow_mut().take() {
        id.remove();
    }
}

fn poll_kube() -> (String, Vec<String>) {
    let current = Command::new("kubectl")
        .args(["config", "current-context"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_default();

    let contexts = Command::new("kubectl")
        .args(["config", "get-contexts", "-o", "name"])
        .output()
        .ok()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .filter(|l| !l.is_empty())
                .map(|l| l.to_string())
                .collect()
        })
        .unwrap_or_default();

    (current, contexts)
}

fn position_popup(popup: &Window, trigger: &Button) {
    // Both bar and popup are layer-shell windows on the same monitor.
    // The popup is anchored top+left, so margins position it from screen edges.
    // Place it directly below the trigger button.
    let Some(root) = trigger.root() else {
        popup.set_margin(Edge::Top, 32);
        return;
    };

    if let Some(bounds) = trigger.compute_bounds(root.upcast_ref::<gtk4::Widget>()) {
        popup.set_margin(Edge::Top, (bounds.y() + bounds.height()) as i32);
        popup.set_margin(Edge::Left, bounds.x() as i32);
    } else {
        popup.set_margin(Edge::Top, 32);
        popup.set_margin(Edge::Left, 0);
    }
}
