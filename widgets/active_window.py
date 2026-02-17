from fabric.widgets.box import Box
from fabric.hyprland.widgets import HyprlandActiveWindow
from fabric.utils.helpers import FormattedString, truncate


class ActiveWindowWidget(Box):
    def __init__(self):
        super().__init__(name="active-window")
        self.children = HyprlandActiveWindow(
            formatter=FormattedString(
                "{'Desktop' if not win_title else truncate(win_title, 60)}",
                truncate=truncate,
            ),
        )
