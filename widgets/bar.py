import json

from gi.repository import Gdk

from fabric.widgets.box import Box
from fabric.widgets.centerbox import CenterBox
from fabric.widgets.wayland import WaylandWindow
from fabric.hyprland.widgets import get_hyprland_connection

from widgets.workspaces import WorkspacesWidget
from widgets.active_window import ActiveWindowWidget
from widgets.clock import ClockWidget
from widgets.battery import BatteryWidget
from widgets.volume import VolumeWidget
from widgets.network import NetworkWidget
from widgets.kube_context import KubeContextWidget


class StatusBar(WaylandWindow):
    def __init__(
        self,
        monitor: int | Gdk.Monitor | None = None,
        hyprland_monitor_name: str | None = None,
    ):
        super().__init__(
            layer="top",
            anchor="left top right",
            exclusivity="auto",
            title="jb-shell",
            monitor=monitor,
            child=CenterBox(
                name="bar-inner",
                start_children=Box(
                    spacing=12,
                    children=[
                        WorkspacesWidget(monitor_name=hyprland_monitor_name),
                        KubeContextWidget(),
                    ],
                ),
                center_children=Box(
                    children=ActiveWindowWidget(),
                ),
                end_children=Box(
                    spacing=8,
                    children=[
                        VolumeWidget(),
                        NetworkWidget(),
                        BatteryWidget(),
                        ClockWidget(),
                    ],
                ),
            ),
        )
        self.show_all()


def create_bars() -> list[StatusBar]:
    """Create a StatusBar for each connected monitor, matched to Hyprland monitors."""
    display = Gdk.Display.get_default()
    conn = get_hyprland_connection()
    hypr_monitors = json.loads(conn.send_command("j/monitors").reply.decode())

    # Match GDK monitors to Hyprland monitors by (x, y) position
    gdk_to_hypr: dict[int, str] = {}
    for i in range(display.get_n_monitors()):
        geo = display.get_monitor(i).get_geometry()
        for hm in hypr_monitors:
            if hm["x"] == geo.x and hm["y"] == geo.y:
                gdk_to_hypr[i] = hm["name"]
                break

    # Fallback: assign by order if position matching failed
    if not gdk_to_hypr:
        for i, hm in enumerate(hypr_monitors):
            if i < display.get_n_monitors():
                gdk_to_hypr[i] = hm["name"]

    bars = []
    for gdk_idx, hypr_name in gdk_to_hypr.items():
        bars.append(StatusBar(monitor=gdk_idx, hyprland_monitor_name=hypr_name))
    return bars
