import json
from loguru import logger
from collections.abc import Callable

from fabric.widgets.box import Box
from fabric.hyprland.service import HyprlandEvent
from fabric.hyprland.widgets import get_hyprland_connection, WorkspaceButton
from fabric.core.widgets.wm import Workspaces
from fabric.utils.helpers import bulk_connect


class MonitorWorkspaces(Workspaces):
    """HyprlandWorkspaces filtered to a single monitor."""

    def __init__(
        self,
        monitor_name: str,
        buttons_factory: Callable[[int], WorkspaceButton | None]
        | None = Workspaces.default_buttons_factory,
        invert_scroll: bool = False,
        empty_scroll: bool = False,
        **kwargs,
    ):
        super().__init__(None, buttons_factory, invert_scroll, **kwargs)
        self.connection = get_hyprland_connection()
        self._monitor_name = monitor_name
        self._empty_scroll = empty_scroll

        bulk_connect(
            self.connection,
            {
                "event::workspacev2": self.on_workspace,
                "event::focusedmonv2": self.on_monitor,
                "event::createworkspacev2": self.on_create_workspace,
                "event::destroyworkspacev2": self.on_destroy_workspace,
                "event::moveworkspacev2": self.on_move_workspace,
                "event::urgent": self.on_urgent,
            },
        )

        if self.connection.ready:
            self.on_ready()
        else:
            self.connection.connect("notify::ready", self.on_ready)
        self.connect("scroll-event", self.do_handle_scroll)

    def _get_workspace_monitor(self, ws_id: int) -> str | None:
        """Query Hyprland for which monitor a workspace is on."""
        try:
            workspaces = json.loads(
                self.connection.send_command("j/workspaces").reply.decode()
            )
            for ws in workspaces:
                if ws["id"] == ws_id:
                    return ws.get("monitor", "")
        except Exception:
            pass
        return None

    def _is_mine(self, ws_id: int) -> bool:
        return self._get_workspace_monitor(ws_id) == self._monitor_name

    def on_ready(self, *_):
        workspaces = json.loads(
            self.connection.send_command("j/workspaces").reply.decode()
        )
        active_workspace = json.loads(
            self.connection.send_command("j/activeworkspace").reply.decode()
        )["id"]

        for ws in workspaces:
            if ws["monitor"] == self._monitor_name:
                self.workspace_created(ws["id"])
                if ws["id"] == active_workspace:
                    self.workspace_activated(ws["id"])

    def on_monitor(self, _, event: HyprlandEvent):
        if len(event.data) != 2:
            return
        mon_name, ws_id = event.data[0], int(event.data[1])
        if mon_name == self._monitor_name:
            self.workspace_activated(ws_id)

    def on_workspace(self, _, event: HyprlandEvent):
        if len(event.data) != 2:
            return
        ws_id = int(event.data[0])
        if self._is_mine(ws_id):
            self.workspace_activated(ws_id)

    def on_create_workspace(self, _, event: HyprlandEvent):
        if len(event.data) != 2:
            return
        ws_id = int(event.data[0])
        if self._is_mine(ws_id):
            self.workspace_created(ws_id)

    def on_destroy_workspace(self, _, event: HyprlandEvent):
        if len(event.data) != 2:
            return
        ws_id = int(event.data[0])
        # Always process destroy - if we have the button, remove it
        if self._buttons.get(ws_id):
            self.workspace_destroyed(ws_id)

    def on_move_workspace(self, _, event: HyprlandEvent):
        """Handle workspace moving between monitors."""
        if len(event.data) != 2:
            return
        ws_id = int(event.data[0])
        target_mon = event.data[1]

        if target_mon == self._monitor_name:
            # Workspace moved TO our monitor - add it
            self.workspace_created(ws_id)
        elif self._buttons.get(ws_id):
            # Workspace moved AWAY from our monitor - remove it
            self.workspace_destroyed(ws_id)

    def on_urgent(self, _, event: HyprlandEvent):
        if len(event.data) != 1:
            return
        clients = json.loads(self.connection.send_command("j/clients").reply.decode())
        clients_map = {client["address"]: client for client in clients}
        urgent_client = clients_map.get("0x" + event.data[0], {})
        if not (raw_workspace := urgent_client.get("workspace")):
            return
        ws_id = int(raw_workspace["id"])
        if self._buttons.get(ws_id):
            self.urgent(ws_id)

    def do_action_next(self):
        return self.connection.send_command(
            f"batch/dispatch workspace {'e' if not self._empty_scroll else ''}+1"
        )

    def do_action_previous(self):
        return self.connection.send_command(
            f"batch/dispatch workspace {'e' if not self._empty_scroll else ''}-1"
        )

    def do_button_clicked(self, button: WorkspaceButton):
        return self.connection.send_command(f"batch/dispatch workspace {button.id}")


class WorkspacesWidget(Box):
    def __init__(self, monitor_name: str | None = None):
        super().__init__(name="workspaces", spacing=4)
        self.workspaces = MonitorWorkspaces(
            monitor_name=monitor_name or "",
            spacing=4,
            buttons_factory=self._make_button,
        )
        self.children = self.workspaces

    @staticmethod
    def _make_button(ws_id):
        btn = WorkspaceButton(id=ws_id, label=str(ws_id), v_align="center")
        btn.connect("notify::empty", lambda b, *_: WorkspacesWidget._update_occupied(b))
        WorkspacesWidget._update_occupied(btn)
        return btn

    @staticmethod
    def _update_occupied(btn):
        if btn.empty:
            btn.remove_style_class("occupied")
        else:
            btn.add_style_class("occupied")
