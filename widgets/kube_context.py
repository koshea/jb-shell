from gi.repository import Gtk

from fabric import Fabricator
from fabric.widgets.box import Box
from fabric.widgets.button import Button
from fabric.widgets.label import Label
from fabric.utils import exec_shell_command, exec_shell_command_async


class KubeContextWidget(Box):
    def __init__(self):
        super().__init__(name="kube-context", v_align="center")
        self.helm_label = Label(label="\u2388", style="font-size: 14px;")
        self.context_label = Label(label="no context")

        self.popover = Gtk.Popover()
        self.menu_btn = Gtk.MenuButton()
        self.menu_btn.set_popover(self.popover)
        inner = Gtk.Box(spacing=4)
        inner.pack_start(self.helm_label, False, False, 0)
        inner.pack_start(self.context_label, False, False, 0)
        inner.show_all()
        self.menu_btn.add(inner)
        self.menu_btn.show()

        self.menu_btn.connect("toggled", lambda *_: self._build_menu())
        self.add(self.menu_btn)

        self.contexts = []
        self.current = ""

        self._fabricator = Fabricator(
            poll_from=self._poll, interval=5000
        )
        self._fabricator.connect("changed", self._update)

    def _poll(self, *_):
        try:
            current = exec_shell_command("kubectl config current-context")
            if not current or current is False:
                return {"current": "", "contexts": []}
            current = current.strip()
            raw = exec_shell_command("kubectl config get-contexts -o name")
            contexts = [c for c in (raw or "").strip().split("\n") if c]
            return {"current": current, "contexts": contexts}
        except Exception:
            return {"current": "", "contexts": []}

    def _update(self, _, value):
        self.current = value["current"]
        self.contexts = value["contexts"]
        self.context_label.set_label(self.current or "no context")

    def _build_menu(self):
        old = self.popover.get_child()
        if old:
            self.popover.remove(old)

        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=2)
        box.set_name("kube-menu")
        for ctx in self.contexts:
            btn = Button(label=ctx, name="kube-menu-item")
            if ctx == self.current:
                btn.add_style_class("active")
            btn.connect("clicked", lambda _, c=ctx: self._switch(c))
            box.add(btn)
        box.show_all()
        self.popover.add(box)

    def _switch(self, ctx):
        exec_shell_command_async(f"kubectl config use-context {ctx}")
        self.popover.popdown()
