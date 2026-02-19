# jb-shell

A personal, opinionated, vibe-coded Wayland status bar and notification daemon for [Hyprland](https://hyprland.org/), built with Rust, GTK4, and [relm4](https://relm4.org/).

This is my daily-driver desktop shell. It does exactly what I want and nothing more. Feel free to fork it and make it your own.

## What it does

- Status bar with workspaces, active window title, clock, battery, volume, network, kube context, and gcloud config
- Google Calendar integration with toast and fullscreen meeting notifications
- Freedesktop notification daemon (`org.freedesktop.Notifications` over D-Bus) with SQLite history
- Workspace preview thumbnails on hover via Hyprland's toplevel export protocol
- Multi-monitor support with hotplug handling

## Building

```bash
cargo build --release
```

## Running

```bash
cargo run --release
```

Drop a `style.css` in `$XDG_CONFIG_HOME/jb-shell/` to customize the theme, or it'll pick up the one next to the binary or in the working directory.

## License

MIT -- see [LICENSE](LICENSE).
