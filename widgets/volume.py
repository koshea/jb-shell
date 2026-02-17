from fabric.audio.service import Audio
from fabric.widgets.box import Box
from fabric.widgets.label import Label
from fabric.widgets.image import Image


class VolumeWidget(Box):
    def __init__(self):
        super().__init__(name="volume", spacing=4)
        self.icon = Image(icon_name="audio-volume-medium-symbolic", icon_size=16)
        self.label = Label(label="0%")
        self.children = (self.icon, self.label)

        self.audio = Audio()
        self.audio.connect("speaker-changed", lambda *_: self._on_speaker_changed())
        self.audio.connect("changed", lambda *_: self._on_audio_changed())

    def _on_speaker_changed(self):
        if not self.audio.speaker:
            return
        self._update_label()
        self._update_icon()

    def _on_audio_changed(self):
        if not self.audio.speaker:
            return
        self._update_label()
        self._update_icon()

    def _update_label(self):
        if not self.audio.speaker:
            return
        self.label.set_label(f"{round(self.audio.speaker.volume)}%")

    def _update_icon(self):
        if not self.audio.speaker:
            return
        vol = self.audio.speaker.volume
        if self.audio.speaker.muted:
            name = "audio-volume-muted-symbolic"
        elif vol < 33:
            name = "audio-volume-low-symbolic"
        elif vol < 66:
            name = "audio-volume-medium-symbolic"
        else:
            name = "audio-volume-high-symbolic"
        self.icon.set_from_icon_name(name, 16)
