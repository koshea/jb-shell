from datetime import datetime

from fabric import Fabricator
from fabric.widgets.box import Box
from fabric.widgets.label import Label


class ClockWidget(Box):
    def __init__(self):
        super().__init__(name="clock", spacing=8)
        self.date_label = Label(name="clock-date")
        self.time_label = Label(name="clock-time")
        self.children = (self.date_label, self.time_label)

        self._fabricator = Fabricator(
            poll_from=self._get_time, interval=1000
        )
        self._fabricator.connect("changed", self._update)

    def _get_time(self, *_):
        now = datetime.now()
        return {
            "date": now.strftime("%a, %b %-d"),
            "time": now.strftime("%-I:%M %p"),
        }

    def _update(self, _, value):
        self.date_label.set_label(value["date"])
        self.time_label.set_label(value["time"])
