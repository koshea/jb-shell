from gi.repository import Gio
from fabric.widgets.box import Box
from fabric.widgets.label import Label
from fabric.widgets.image import Image


class BatteryWidget(Box):
    def __init__(self):
        super().__init__(name="battery", spacing=4)
        self.icon = Image(icon_name="battery-full-symbolic", icon_size=16)
        self.label = Label(label="")
        self.children = (self.icon, self.label)

        try:
            bus = Gio.bus_get_sync(Gio.BusType.SYSTEM, None)
            self.proxy = Gio.DBusProxy.new_sync(
                bus, 0, None,
                "org.freedesktop.UPower",
                "/org/freedesktop/UPower/devices/DisplayDevice",
                "org.freedesktop.UPower.Device",
                None,
            )
            self.proxy.connect("g-properties-changed", lambda *_: self._update())
            self._update()
        except Exception:
            self.set_visible(False)

    def _update(self):
        is_present = self.proxy.get_cached_property("IsPresent")
        if is_present and not is_present.unpack():
            self.set_visible(False)
            return

        self.set_visible(True)
        percentage = self.proxy.get_cached_property("Percentage")
        icon_name = self.proxy.get_cached_property("IconName")

        if percentage:
            self.label.set_label(f"{round(percentage.unpack())}%")
        if icon_name:
            self.icon.set_from_icon_name(icon_name.unpack(), 16)
