use gtk4::prelude::*;
use gtk4::{Box as GtkBox, Button, EventControllerScroll, EventControllerScrollFlags, Label, Orientation};
use hyprland::data::{Workspace, Workspaces};
use hyprland::dispatch::{Dispatch, DispatchType, WorkspaceIdentifierWithSpecial};
use hyprland::shared::{HyprData, HyprDataActive, HyprDataVec};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

pub struct WorkspacesWidget {
    pub container: GtkBox,
    inner: GtkBox,
    monitor_name: String,
    buttons: Rc<RefCell<BTreeMap<i32, Button>>>,
    active_id: Rc<RefCell<i32>>,
}

impl WorkspacesWidget {
    pub fn new(monitor_name: &str) -> Self {
        let container = GtkBox::new(Orientation::Horizontal, 4);
        container.set_widget_name("workspaces");

        let inner = GtkBox::new(Orientation::Horizontal, 4);
        container.append(&inner);

        let buttons: Rc<RefCell<BTreeMap<i32, Button>>> = Rc::new(RefCell::new(BTreeMap::new()));
        let active_id = Rc::new(RefCell::new(0));

        let widget = Self {
            container,
            inner,
            monitor_name: monitor_name.to_string(),
            buttons,
            active_id,
        };

        widget.init_workspaces();
        widget.setup_scroll();
        widget
    }

    fn init_workspaces(&self) {
        let workspaces = match Workspaces::get() {
            Ok(ws) => ws,
            Err(_) => return,
        };

        let active_ws = Workspace::get_active()
            .ok()
            .map(|w| w.id)
            .unwrap_or(0);

        for ws in workspaces.to_vec() {
            if ws.monitor == self.monitor_name {
                self.add_workspace(ws.id);
            }
        }

        self.set_active(active_ws);
    }

    fn setup_scroll(&self) {
        let scroll = EventControllerScroll::new(EventControllerScrollFlags::VERTICAL);
        let monitor = self.monitor_name.clone();
        scroll.connect_scroll(move |_, _, dy| {
            let _ = monitor; // keep for potential future per-monitor scroll
            if dy > 0.0 {
                let _ = Dispatch::call(DispatchType::Workspace(
                    WorkspaceIdentifierWithSpecial::Relative(1),
                ));
            } else if dy < 0.0 {
                let _ = Dispatch::call(DispatchType::Workspace(
                    WorkspaceIdentifierWithSpecial::Relative(-1),
                ));
            }
            gtk4::glib::Propagation::Stop
        });
        self.container.add_controller(scroll);
    }

    pub fn add_workspace(&self, ws_id: i32) {
        let mut buttons = self.buttons.borrow_mut();
        if buttons.contains_key(&ws_id) {
            return;
        }

        let btn = Button::new();
        btn.set_valign(gtk4::Align::Center);
        let label = Label::new(Some(&ws_id.to_string()));
        btn.set_child(Some(&label));
        btn.add_css_class("occupied");

        let id = ws_id;
        btn.connect_clicked(move |_| {
            let _ = Dispatch::call(DispatchType::Workspace(
                WorkspaceIdentifierWithSpecial::Id(id),
            ));
        });

        buttons.insert(ws_id, btn);
        drop(buttons);

        self.rebuild_order();
    }

    pub fn remove_workspace(&self, ws_id: i32) {
        let mut buttons = self.buttons.borrow_mut();
        if let Some(btn) = buttons.remove(&ws_id) {
            self.inner.remove(&btn);
        }
    }

    pub fn set_active(&self, ws_id: i32) {
        let buttons = self.buttons.borrow();
        let old_id = *self.active_id.borrow();

        // Remove active class from old
        if let Some(old_btn) = buttons.get(&old_id) {
            old_btn.remove_css_class("active");
        }

        // Add active class to new
        if let Some(new_btn) = buttons.get(&ws_id) {
            new_btn.add_css_class("active");
        }

        drop(buttons);
        *self.active_id.borrow_mut() = ws_id;
    }

    fn rebuild_order(&self) {
        // Remove all children first
        while let Some(child) = self.inner.first_child() {
            self.inner.remove(&child);
        }

        // Re-add in sorted order
        let buttons = self.buttons.borrow();
        for btn in buttons.values() {
            self.inner.append(btn);
        }
    }

    #[allow(dead_code)]
    pub fn monitor_name(&self) -> &str {
        &self.monitor_name
    }
}
