import re

from fabric import Fabricator
from fabric.widgets.box import Box
from fabric.widgets.label import Label
from fabric.widgets.image import Image
from fabric.utils import exec_shell_command


class NetworkWidget(Box):
    def __init__(self):
        super().__init__(name="network", spacing=4)
        self.icon = Image(icon_name="network-wireless-offline-symbolic", icon_size=16)
        self.label = Label(label="Offline")
        self.children = (self.icon, self.label)

        self._fabricator = Fabricator(
            poll_from=self._get_status, interval=5000
        )
        self._fabricator.connect("changed", self._update)

    def _get_status(self, *_):
        try:
            output = exec_shell_command("iwctl station wlan0 show")
            if not output:
                return {"state": "disconnected", "ssid": "", "rssi": -100}
            state = re.search(r"State\s+(.*)", output)
            ssid = re.search(r"Connected network\s+(.*)", output)
            rssi = re.search(r"RSSI\s+(-?\d+)", output)
            return {
                "state": state.group(1).strip() if state else "disconnected",
                "ssid": ssid.group(1).strip() if ssid else "",
                "rssi": int(rssi.group(1)) if rssi else -100,
            }
        except Exception:
            return {"state": "disconnected", "ssid": "", "rssi": -100}

    def _update(self, _, value):
        if value["state"] == "connected":
            self.label.set_label(value["ssid"])
            rssi = value["rssi"]
            if rssi >= -50:
                icon = "network-wireless-signal-excellent-symbolic"
            elif rssi >= -60:
                icon = "network-wireless-signal-good-symbolic"
            elif rssi >= -70:
                icon = "network-wireless-signal-ok-symbolic"
            else:
                icon = "network-wireless-signal-none-symbolic"
        else:
            self.label.set_label("Offline")
            icon = "network-wireless-offline-symbolic"
        self.icon.set_from_icon_name(icon, 16)
