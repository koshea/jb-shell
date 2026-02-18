use gdk4::Monitor;
use gtk4::prelude::*;
use gtk4::{Box as GtkBox, Button, EventControllerFocus, Label, Orientation, Window};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use relm4::prelude::*;
use std::cell::RefCell;
use std::marker::PhantomData;
use std::rc::Rc;
use std::time::Duration;

pub trait SwitcherProvider: 'static {
    const WIDGET_NAME: &'static str;
    const TRIGGER_NAME: &'static str;
    const POPUP_NAME: &'static str;
    const MENU_ITEM_NAME: &'static str;
    const MENU_BOX_NAME: &'static str;
    const ICON: &'static str;
    const ICON_CSS_CLASSES: &'static [&'static str];
    const FALLBACK_LABEL: &'static str;
    const MAX_LABEL_LEN: usize;
    const POLL_INTERVAL: Duration;

    /// Called on a background thread. Returns (current, all_items).
    fn poll() -> (String, Vec<String>);

    /// Called on a background thread to switch to the given item.
    fn switch(name: &str);
}

pub struct SwitcherModel<P: SwitcherProvider> {
    current: String,
    items: Vec<String>,
    popup_visible: bool,
    _phantom: PhantomData<P>,
}

#[derive(Debug)]
pub enum SwitcherInput {
    PollResult { current: String, items: Vec<String> },
    SwitchItem(String),
    TogglePopup,
    HidePopup,
    FocusLeave,
    FocusEnter,
}

pub struct SwitcherWidgets {
    item_label: Label,
    trigger: Button,
    popup: Window,
    popup_box: GtkBox,
    close_timer: Rc<RefCell<Option<glib::SourceId>>>,
}

impl<P: SwitcherProvider> Component for SwitcherModel<P> {
    type Init = Monitor;
    type Input = SwitcherInput;
    type Output = ();
    type CommandOutput = ();
    type Root = GtkBox;
    type Widgets = SwitcherWidgets;

    fn init_root() -> Self::Root {
        let b = GtkBox::new(Orientation::Horizontal, 0);
        b.set_widget_name(P::WIDGET_NAME);
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
        let icon_label = Label::new(Some(P::ICON));
        icon_label.set_css_classes(P::ICON_CSS_CLASSES);
        let item_label = Label::new(Some(P::FALLBACK_LABEL));

        trigger_box.append(&icon_label);
        trigger_box.append(&item_label);

        let trigger = Button::new();
        trigger.set_widget_name(P::TRIGGER_NAME);
        trigger.set_child(Some(&trigger_box));
        root.append(&trigger);

        // Trigger click
        let trigger_sender = sender.input_sender().clone();
        trigger.connect_clicked(move |_| {
            trigger_sender.emit(SwitcherInput::TogglePopup);
        });

        // Popup window — layer shell overlay on same monitor as bar
        let popup = Window::new();
        popup.set_widget_name(P::POPUP_NAME);
        popup.init_layer_shell();
        popup.set_layer(Layer::Overlay);
        popup.set_exclusive_zone(-1);
        popup.set_anchor(Edge::Top, true);
        popup.set_anchor(Edge::Left, true);
        popup.set_keyboard_mode(KeyboardMode::OnDemand);
        popup.set_monitor(Some(&monitor));

        let popup_box = GtkBox::new(Orientation::Vertical, 2);
        popup_box.set_widget_name(P::MENU_BOX_NAME);
        popup.set_child(Some(&popup_box));
        popup.set_visible(false);

        // Focus handlers on popup
        let focus = EventControllerFocus::new();
        let leave_sender = sender.input_sender().clone();
        focus.connect_leave(move |_| {
            leave_sender.emit(SwitcherInput::FocusLeave);
        });
        let enter_sender = sender.input_sender().clone();
        focus.connect_enter(move |_| {
            enter_sender.emit(SwitcherInput::FocusEnter);
        });
        popup.add_controller(focus);

        // Background polling thread
        let input_sender = sender.input_sender().clone();
        std::thread::spawn(move || loop {
            let (current, items) = P::poll();
            input_sender.emit(SwitcherInput::PollResult { current, items });
            std::thread::sleep(P::POLL_INTERVAL);
        });

        let model = SwitcherModel {
            current: String::new(),
            items: Vec::new(),
            popup_visible: false,
            _phantom: PhantomData,
        };
        let close_timer = Rc::new(RefCell::new(None));
        let widgets = SwitcherWidgets {
            item_label,
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
            SwitcherInput::FocusLeave => {
                cancel_close_timer(&widgets.close_timer);
                let hide_sender = sender.input_sender().clone();
                let timer_ref = widgets.close_timer.clone();
                let id = glib::timeout_add_local_once(Duration::from_millis(500), move || {
                    hide_sender.emit(SwitcherInput::HidePopup);
                    *timer_ref.borrow_mut() = None;
                });
                *widgets.close_timer.borrow_mut() = Some(id);
                return;
            }
            SwitcherInput::FocusEnter => {
                cancel_close_timer(&widgets.close_timer);
                return;
            }
            other => match other {
                SwitcherInput::PollResult { current, items } => {
                    self.current = current;
                    self.items = items;
                }
                SwitcherInput::SwitchItem(name) => {
                    self.current = name.clone();
                    self.popup_visible = false;
                    std::thread::spawn(move || {
                        P::switch(&name);
                    });
                }
                SwitcherInput::TogglePopup => {
                    self.popup_visible = !self.popup_visible;
                }
                SwitcherInput::HidePopup => {
                    self.popup_visible = false;
                }
                _ => unreachable!(),
            },
        }

        self.update_view(widgets, sender);
    }

    fn update_view(&self, widgets: &mut Self::Widgets, sender: ComponentSender<Self>) {
        widgets.item_label.set_label(&if self.current.is_empty() {
            P::FALLBACK_LABEL.to_string()
        } else {
            truncate_middle(&self.current, P::MAX_LABEL_LEN)
        });

        if self.popup_visible {
            // Rebuild menu items
            while let Some(child) = widgets.popup_box.first_child() {
                widgets.popup_box.remove(&child);
            }
            for item in &self.items {
                let is_active = *item == self.current;
                let label = if is_active {
                    format!("  \u{2713}  {item}")
                } else {
                    format!("      {item}")
                };
                let btn = Button::with_label(&label);
                btn.set_widget_name(P::MENU_ITEM_NAME);
                if is_active {
                    btn.add_css_class("active");
                }
                let item_name = item.clone();
                let switch_sender = sender.input_sender().clone();
                btn.connect_clicked(move |_| {
                    switch_sender.emit(SwitcherInput::SwitchItem(item_name.clone()));
                });
                widgets.popup_box.append(&btn);
            }

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

pub fn truncate_middle(name: &str, max_len: usize) -> String {
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

fn position_popup(popup: &Window, trigger: &Button) {
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
