use gdk4::{MemoryFormat, MemoryTexture, Monitor};
use gtk4::prelude::*;
use gtk4::{
    Box as GtkBox, Button, EventControllerMotion, EventControllerScroll,
    EventControllerScrollFlags, GestureClick, Label, Orientation, Picture, Window,
};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use hyprland::data::{Clients, Workspace, Workspaces};
use hyprland::dispatch::{
    Dispatch, DispatchType, WindowIdentifier, WorkspaceIdentifierWithSpecial,
};
use hyprland::shared::{Address, HyprData, HyprDataActive, HyprDataVec};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use crate::workspace_capture::{CaptureRequest, CaptureResult};

const PREVIEW_WIDTH: f64 = 640.0;

/// Hit region for click-to-focus on the composite thumbnail.
struct ClickRegion {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    address: Address,
}

pub struct WorkspacesWidget {
    pub container: GtkBox,
    inner: GtkBox,
    monitor_name: String,
    buttons: Rc<RefCell<BTreeMap<i32, Button>>>,
    active_id: Rc<RefCell<i32>>,
    popup: Window,
    popup_labels_box: GtkBox,
    preview_picture: Picture,
    capture_tx: mpsc::Sender<CaptureRequest>,
    close_timer: Rc<RefCell<Option<glib::SourceId>>>,
    hovered_ws: Rc<RefCell<Option<i32>>>,
    popup_items: Rc<RefCell<Vec<(Address, Button)>>>,
}

impl WorkspacesWidget {
    pub fn new(monitor_name: &str, gdk_monitor: &Monitor) -> Self {
        let container = GtkBox::new(Orientation::Horizontal, 4);
        container.set_widget_name("workspaces");

        let inner = GtkBox::new(Orientation::Horizontal, 4);
        container.append(&inner);

        let buttons: Rc<RefCell<BTreeMap<i32, Button>>> = Rc::new(RefCell::new(BTreeMap::new()));
        let active_id = Rc::new(RefCell::new(0));

        // Popup window — layer shell overlay on same monitor as bar
        let popup = Window::new();
        popup.set_widget_name("ws-popup");
        popup.init_layer_shell();
        popup.set_layer(Layer::Overlay);
        popup.set_exclusive_zone(-1);
        popup.set_anchor(Edge::Top, true);
        popup.set_anchor(Edge::Left, true);
        popup.set_keyboard_mode(KeyboardMode::None);
        popup.set_monitor(Some(gdk_monitor));
        popup.set_visible(false);

        // Popup layout: single preview picture + text labels
        let popup_box = GtkBox::new(Orientation::Vertical, 2);

        let preview_picture = Picture::new();
        preview_picture.set_widget_name("ws-preview-canvas");
        preview_picture.set_can_shrink(true);
        preview_picture.set_visible(false);

        let popup_labels_box = GtkBox::new(Orientation::Vertical, 2);

        popup_box.append(&preview_picture);
        popup_box.append(&popup_labels_box);
        popup.set_child(Some(&popup_box));

        let close_timer: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));
        let hovered_ws: Rc<RefCell<Option<i32>>> = Rc::new(RefCell::new(None));

        // Click regions for thumbnail hit-testing
        let click_regions: Rc<RefCell<Vec<ClickRegion>>> = Rc::new(RefCell::new(Vec::new()));

        // Track popup label buttons by address for hover highlight
        let popup_items: Rc<RefCell<Vec<(Address, Button)>>> = Rc::new(RefCell::new(Vec::new()));

        // Click-to-focus on thumbnail
        let click = GestureClick::new();
        let regions_ref = click_regions.clone();
        let popup_ref = popup.clone();
        let hovered_ref = hovered_ws.clone();
        let timer_ref = close_timer.clone();
        click.connect_released(move |_, _, x, y| {
            let regions = regions_ref.borrow();
            for region in regions.iter() {
                if x >= region.x
                    && x < region.x + region.w
                    && y >= region.y
                    && y < region.y + region.h
                {
                    let _ = Dispatch::call(DispatchType::FocusWindow(WindowIdentifier::Address(
                        region.address.clone(),
                    )));
                    cancel_close_timer(&timer_ref);
                    popup_ref.set_visible(false);
                    *hovered_ref.borrow_mut() = None;
                    break;
                }
            }
        });
        preview_picture.add_controller(click);

        // Hover highlight: motion over thumbnail highlights corresponding label
        let preview_motion = EventControllerMotion::new();
        let regions_ref = click_regions.clone();
        let items_ref = popup_items.clone();
        preview_motion.connect_motion(move |_, x, y| {
            let regions = regions_ref.borrow();
            let items = items_ref.borrow();
            let mut matched: Option<&Address> = None;
            for region in regions.iter() {
                if x >= region.x
                    && x < region.x + region.w
                    && y >= region.y
                    && y < region.y + region.h
                {
                    matched = Some(&region.address);
                    break;
                }
            }
            for (addr, btn) in items.iter() {
                if matched.is_some_and(|m| m == addr) {
                    btn.add_css_class("preview-highlight");
                } else {
                    btn.remove_css_class("preview-highlight");
                }
            }
        });
        let items_ref = popup_items.clone();
        preview_motion.connect_leave(move |_| {
            let items = items_ref.borrow();
            for (_, btn) in items.iter() {
                btn.remove_css_class("preview-highlight");
            }
        });
        preview_picture.add_controller(preview_motion);

        // Spawn capture thread
        let (capture_tx, capture_rx) = crate::workspace_capture::spawn_capture_thread();

        // Poll capture results from the glib main loop
        let preview_ref = preview_picture.clone();
        let hovered_ref = hovered_ws.clone();
        let regions_ref = click_regions;
        glib::timeout_add_local(Duration::from_millis(32), move || {
            let mut latest: Option<CaptureResult> = None;
            while let Ok(result) = capture_rx.try_recv() {
                if *hovered_ref.borrow() == Some(result.ws_id) {
                    latest = Some(result);
                }
            }
            if let Some(result) = latest {
                apply_capture_result(&preview_ref, &result, &regions_ref);
            }
            glib::ControlFlow::Continue
        });

        // Hover handlers on popup itself — any motion inside cancels close timer.
        // Using connect_motion instead of connect_enter because after hide/re-show
        // the enter event may not fire if the controller's pointer state is stale.
        let motion = EventControllerMotion::new();
        let timer_ref = close_timer.clone();
        motion.connect_motion(move |_, _, _| {
            cancel_close_timer(&timer_ref);
        });
        let timer_ref = close_timer.clone();
        let popup_ref = popup.clone();
        let hovered_ref = hovered_ws.clone();
        motion.connect_leave(move |_| {
            start_close_timer(&timer_ref, &popup_ref, &hovered_ref);
        });
        popup.add_controller(motion);

        let widget = Self {
            container,
            inner,
            monitor_name: monitor_name.to_string(),
            buttons,
            active_id,
            popup,
            popup_labels_box,
            preview_picture,
            capture_tx,
            close_timer,
            hovered_ws,
            popup_items,
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

        let active_ws = Workspace::get_active().ok().map(|w| w.id).unwrap_or(0);

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
            let _ = Dispatch::call(DispatchType::Workspace(WorkspaceIdentifierWithSpecial::Id(
                id,
            )));
        });

        // Hover handlers for workspace preview popup
        let motion = EventControllerMotion::new();
        let popup_ref = self.popup.clone();
        let labels_ref = self.popup_labels_box.clone();
        let preview_ref = self.preview_picture.clone();
        let capture_tx = self.capture_tx.clone();
        let monitor_name = self.monitor_name.clone();
        let timer_ref = self.close_timer.clone();
        let hovered_ref = self.hovered_ws.clone();
        let items_ref = self.popup_items.clone();
        motion.connect_enter(move |ctrl, _, _| {
            cancel_close_timer(&timer_ref);
            if let Some(trigger) = ctrl.widget() {
                show_workspace_popup(
                    &popup_ref,
                    &labels_ref,
                    &preview_ref,
                    &capture_tx,
                    &monitor_name,
                    &hovered_ref,
                    &items_ref,
                    ws_id,
                    &trigger,
                );
            }
        });
        let timer_ref = self.close_timer.clone();
        let popup_ref = self.popup.clone();
        let hovered_ref = self.hovered_ws.clone();
        motion.connect_leave(move |_| {
            start_close_timer(&timer_ref, &popup_ref, &hovered_ref);
        });
        btn.add_controller(motion);

        buttons.insert(ws_id, btn);
        drop(buttons);

        self.rebuild_order();
    }

    pub fn remove_workspace(&self, ws_id: i32) {
        if *self.hovered_ws.borrow() == Some(ws_id) {
            cancel_close_timer(&self.close_timer);
            self.popup.set_visible(false);
            *self.hovered_ws.borrow_mut() = None;
        }

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

fn cancel_close_timer(timer: &Rc<RefCell<Option<glib::SourceId>>>) {
    if let Some(id) = timer.borrow_mut().take() {
        id.remove();
    }
}

fn start_close_timer(
    timer: &Rc<RefCell<Option<glib::SourceId>>>,
    popup: &Window,
    hovered_ws: &Rc<RefCell<Option<i32>>>,
) {
    cancel_close_timer(timer);
    let popup = popup.clone();
    let hovered_ws = hovered_ws.clone();
    let timer_ref = timer.clone();
    let id = glib::timeout_add_local_once(Duration::from_millis(300), move || {
        popup.set_visible(false);
        *hovered_ws.borrow_mut() = None;
        *timer_ref.borrow_mut() = None;
    });
    *timer.borrow_mut() = Some(id);
}

/// Composite all thumbnails into a single image buffer and display on a Picture.
fn apply_capture_result(
    preview: &Picture,
    result: &CaptureResult,
    click_regions: &Rc<RefCell<Vec<ClickRegion>>>,
) {
    let scale = PREVIEW_WIDTH / result.monitor_width as f64;
    let pw = PREVIEW_WIDTH as u32;
    let ph = ((result.monitor_height as f64 * scale) as u32).max(1);
    let stride = pw as usize * 4;
    let mut buf = vec![0u8; stride * ph as usize];

    let mut regions = click_regions.borrow_mut();
    regions.clear();

    for thumb in &result.thumbnails {
        let dst_w = ((thumb.win_width as f64 * scale) as u32).max(1);
        let dst_h = ((thumb.win_height as f64 * scale) as u32).max(1);
        let (scaled, scaled_stride) = downscale_nearest(
            &thumb.data,
            thumb.width,
            thumb.height,
            thumb.stride,
            dst_w,
            dst_h,
        );

        let ox = (thumb.x as f64 * scale) as i32;
        let oy = (thumb.y as f64 * scale) as i32;

        // Blit into composite buffer
        for row in 0..dst_h as i32 {
            let by = oy + row;
            if by < 0 || by >= ph as i32 {
                continue;
            }
            for col in 0..dst_w as i32 {
                let bx = ox + col;
                if bx < 0 || bx >= pw as i32 {
                    continue;
                }
                let src_off = row as usize * scaled_stride + col as usize * 4;
                let dst_off = by as usize * stride + bx as usize * 4;
                if src_off + 4 <= scaled.len() && dst_off + 4 <= buf.len() {
                    buf[dst_off..dst_off + 3].copy_from_slice(&scaled[src_off..src_off + 3]);
                    buf[dst_off + 3] = 0xFF; // force opaque
                }
            }
        }

        regions.push(ClickRegion {
            x: ox.max(0) as f64,
            y: oy.max(0) as f64,
            w: dst_w as f64,
            h: dst_h as f64,
            address: thumb.address.clone(),
        });
    }
    drop(regions);

    let bytes = glib::Bytes::from(&buf);
    let texture = MemoryTexture::new(
        pw as i32,
        ph as i32,
        MemoryFormat::B8g8r8a8Premultiplied,
        &bytes,
        stride,
    );
    preview.set_paintable(Some(&texture));
    preview.set_size_request(pw as i32, ph as i32);
    preview.set_visible(true);
}

#[allow(clippy::too_many_arguments)]
fn show_workspace_popup(
    popup: &Window,
    popup_labels_box: &GtkBox,
    preview_picture: &Picture,
    capture_tx: &mpsc::Sender<CaptureRequest>,
    monitor_name: &str,
    hovered_ws: &Rc<RefCell<Option<i32>>>,
    popup_items: &Rc<RefCell<Vec<(Address, Button)>>>,
    ws_id: i32,
    trigger: &gtk4::Widget,
) {
    *hovered_ws.borrow_mut() = Some(ws_id);

    // Clear previous labels
    while let Some(child) = popup_labels_box.first_child() {
        popup_labels_box.remove(&child);
    }
    popup_items.borrow_mut().clear();

    // Hide preview (will be populated async by capture thread)
    preview_picture.set_paintable(None::<&MemoryTexture>);
    preview_picture.set_visible(false);

    // Fetch clients from Hyprland IPC
    let clients = Clients::get().ok();
    let ws_clients: Vec<_> = clients
        .into_iter()
        .flat_map(|c| c.to_vec())
        .filter(|c| c.workspace.id == ws_id && c.mapped)
        .collect();

    if ws_clients.is_empty() {
        let label = Label::new(Some("(empty)"));
        label.set_widget_name("ws-popup-item");
        label.add_css_class("dim");
        label.set_halign(gtk4::Align::Start);
        popup_labels_box.append(&label);
    } else {
        let mut items = popup_items.borrow_mut();
        for client in &ws_clients {
            let text = format_client_line(client);
            let btn = Button::new();
            btn.set_widget_name("ws-popup-item");
            let label = Label::new(Some(&text));
            label.set_halign(gtk4::Align::Start);
            btn.set_child(Some(&label));

            let address = client.address.clone();
            let popup_clone = popup.clone();
            let hovered_clone = hovered_ws.clone();
            btn.connect_clicked(move |_| {
                let _ = Dispatch::call(DispatchType::FocusWindow(WindowIdentifier::Address(
                    address.clone(),
                )));
                popup_clone.set_visible(false);
                *hovered_clone.borrow_mut() = None;
            });

            items.push((client.address.clone(), btn.clone()));
            popup_labels_box.append(&btn);
        }
        drop(items);

        // Request thumbnail capture
        let _ = capture_tx.send(CaptureRequest {
            ws_id,
            monitor_name: monitor_name.to_string(),
        });
    }

    position_ws_popup(popup, trigger);
    popup.set_visible(true);
}

fn format_client_line(client: &hyprland::data::Client) -> String {
    let class = &client.class;
    let title = truncate_title(&client.title, 40);
    if title.is_empty() {
        class.to_string()
    } else {
        format!("{class}: {title}")
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

fn position_ws_popup(popup: &Window, trigger: &gtk4::Widget) {
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

/// Nearest-neighbor downscale of BGRA pixel data.
fn downscale_nearest(
    src: &[u8],
    src_w: u32,
    src_h: u32,
    src_stride: u32,
    dst_w: u32,
    dst_h: u32,
) -> (Vec<u8>, usize) {
    let dst_stride = dst_w as usize * 4;
    let mut dst = vec![0u8; dst_stride * dst_h as usize];

    for dy in 0..dst_h {
        let sy = ((dy as u64 * src_h as u64) / dst_h as u64) as u32;
        for dx in 0..dst_w {
            let sx = ((dx as u64 * src_w as u64) / dst_w as u64) as u32;
            let src_off = (sy * src_stride + sx * 4) as usize;
            let dst_off = dy as usize * dst_stride + dx as usize * 4;
            if src_off + 4 <= src.len() {
                dst[dst_off..dst_off + 4].copy_from_slice(&src[src_off..src_off + 4]);
            }
        }
    }

    (dst, dst_stride)
}
