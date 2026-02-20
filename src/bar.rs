use gdk4::Monitor;
use gtk4::prelude::*;
use gtk4::{Box as GtkBox, CenterBox, Orientation, Window};
use gtk4_layer_shell::{Edge, Layer, LayerShell};
use relm4::{Component, ComponentController, Controller};

use crate::hyprland_listener::HyprlandMsg;
use crate::widgets::active_window::ActiveWindowWidget;
use crate::widgets::battery::BatteryModel;
use crate::widgets::calendar::{CalendarInit, CalendarModel};
use crate::widgets::clock::ClockModel;
use crate::widgets::gcloud_config::GcloudModel;
use crate::widgets::kube_context::KubeModel;
use crate::widgets::mpris::MprisModel;
use crate::widgets::network::NetworkModel;
use crate::widgets::notification_center::{NotificationCenterInit, NotificationCenterModel};
use crate::widgets::notifications::NotificationModel;
use crate::widgets::volume::VolumeModel;
use crate::widgets::workspaces::WorkspacesWidget;

pub struct StatusBar {
    pub window: Window,
    pub monitor: Monitor,
    workspaces: WorkspacesWidget,
    active_window: ActiveWindowWidget,
    // Keep controllers alive — dropping them stops the component
    _clock: Controller<ClockModel>,
    _battery: Controller<BatteryModel>,
    _volume: Controller<VolumeModel>,
    _network: Controller<NetworkModel>,
    _kube: Controller<KubeModel>,
    _gcloud: Controller<GcloudModel>,
    _mpris: Controller<MprisModel>,
    _notifications: Controller<NotificationModel>,
    _notification_center: Controller<NotificationCenterModel>,
    _calendar: Controller<CalendarModel>,
    monitor_name: String,
}

impl StatusBar {
    pub fn new(monitor: &Monitor, hyprland_monitor_name: &str) -> Self {
        let window = Window::new();
        window.set_title(Some("jb-shell"));

        // Layer shell setup — must be done before present()
        window.init_layer_shell();
        window.set_layer(Layer::Top);
        window.set_anchor(Edge::Left, true);
        window.set_anchor(Edge::Top, true);
        window.set_anchor(Edge::Right, true);
        window.auto_exclusive_zone_enable();
        window.set_monitor(Some(monitor));

        // Build widgets
        let workspaces = WorkspacesWidget::new(hyprland_monitor_name, monitor);
        let active_window = ActiveWindowWidget::new();

        // Create relm4 components
        let clock = ClockModel::builder().launch(()).detach();
        let battery = BatteryModel::builder().launch(()).detach();
        let volume = VolumeModel::builder().launch(()).detach();
        let network = NetworkModel::builder().launch(()).detach();
        let kube = KubeModel::builder().launch(monitor.clone()).detach();
        let gcloud = GcloudModel::builder().launch(monitor.clone()).detach();
        let mpris = MprisModel::builder().launch(()).detach();
        let notifications = NotificationModel::builder()
            .launch(monitor.clone())
            .detach();
        let notif_sender = notifications.sender().clone();

        let notification_center = NotificationCenterModel::builder()
            .launch(NotificationCenterInit {
                monitor: monitor.clone(),
                notif_sender: notif_sender.clone(),
            })
            .detach();

        // Wire center sender into notification model for suppression
        notifications.sender().emit(
            crate::widgets::notifications::NotificationInput::SetCenterSender(
                notification_center.sender().clone(),
            ),
        );

        let calendar = CalendarModel::builder()
            .launch(CalendarInit {
                monitor: monitor.clone(),
                notif_sender,
            })
            .detach();

        // Start box (left)
        let start_box = GtkBox::new(Orientation::Horizontal, 12);
        start_box.append(&workspaces.container);
        start_box.append(kube.widget());
        start_box.append(gcloud.widget());
        start_box.append(mpris.widget());

        // Center box
        let center_box = GtkBox::new(Orientation::Horizontal, 0);
        center_box.append(&active_window.container);

        // End box (right)
        let end_box = GtkBox::new(Orientation::Horizontal, 8);
        end_box.append(notification_center.widget());
        end_box.append(calendar.widget());
        end_box.append(volume.widget());
        end_box.append(network.widget());
        end_box.append(battery.widget());
        end_box.append(clock.widget());

        let center = CenterBox::new();
        center.set_widget_name("bar-inner");
        center.set_start_widget(Some(&start_box));
        center.set_center_widget(Some(&center_box));
        center.set_end_widget(Some(&end_box));

        window.set_child(Some(&center));

        // Debug: log if GTK asks to close this window
        let mon_name_for_signal = hyprland_monitor_name.to_string();
        window.connect_close_request(move |_| {
            eprintln!(
                "jb-shell: [lifecycle] close_request on bar window for monitor: {mon_name_for_signal}"
            );
            gtk4::glib::Propagation::Proceed
        });

        // Debug: log when the window is actually destroyed
        let mon_name_for_destroy = hyprland_monitor_name.to_string();
        window.connect_destroy(move |_| {
            eprintln!("jb-shell: [lifecycle] window destroy for monitor: {mon_name_for_destroy}");
        });

        Self {
            window,
            monitor: monitor.clone(),
            workspaces,
            active_window,
            _clock: clock,
            _battery: battery,
            _volume: volume,
            _network: network,
            _kube: kube,
            _gcloud: gcloud,
            _mpris: mpris,
            _notifications: notifications,
            _notification_center: notification_center,
            _calendar: calendar,
            monitor_name: hyprland_monitor_name.to_string(),
        }
    }

    pub fn handle_hyprland_msg(&self, msg: &HyprlandMsg) {
        match msg {
            HyprlandMsg::WorkspaceChanged {
                monitor_name,
                workspace_id,
            } => {
                if *monitor_name == self.monitor_name {
                    self.workspaces.set_active(*workspace_id);
                }
            }
            HyprlandMsg::WorkspaceCreated {
                workspace_id,
                monitor_name,
            } => {
                if *monitor_name == self.monitor_name {
                    self.workspaces.add_workspace(*workspace_id);
                }
            }
            HyprlandMsg::WorkspaceDestroyed { workspace_id } => {
                self.workspaces.remove_workspace(*workspace_id);
            }
            HyprlandMsg::WorkspaceMoved {
                workspace_id,
                monitor_name,
            } => {
                if *monitor_name == self.monitor_name {
                    self.workspaces.add_workspace(*workspace_id);
                } else {
                    self.workspaces.remove_workspace(*workspace_id);
                }
            }
            HyprlandMsg::ActiveWindowChanged { title } => {
                self.active_window.set_title(title);
            }
            HyprlandMsg::MonitorFocusChanged {
                monitor_name,
                workspace_id,
            } => {
                if *monitor_name == self.monitor_name {
                    self.workspaces.set_active(*workspace_id);
                }
            }
        }
    }

    pub fn notification_sender(
        &self,
    ) -> &relm4::Sender<crate::widgets::notifications::NotificationInput> {
        self._notifications.sender()
    }

    pub fn destroy(&self) {
        eprintln!(
            "jb-shell: [lifecycle] StatusBar::destroy called for monitor: {}",
            self.monitor_name
        );
        self.window.set_visible(false);
        self.window.close();
    }

    pub fn monitor_name(&self) -> &str {
        &self.monitor_name
    }
}
