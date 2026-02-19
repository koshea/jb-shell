# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build Commands

```bash
cargo build              # Dev build
cargo build --release    # Release build
cargo run                # Build and run
cargo check              # Type-check without building
cargo fmt                # Format code
cargo clippy             # Lint
RUST_BACKTRACE=1 cargo run  # Run with backtraces
```

No test suite, CI, or custom linting config exists.

## Architecture

jb-shell is a Wayland status bar for Hyprland, built with GTK4, gtk4-layer-shell, and relm4. It also acts as a freedesktop notification daemon (D-Bus) and integrates with Google Calendar.

### Threading Model

- **Main thread**: GTK4 glib event loop — all UI updates, component lifecycle, timers
- **Hyprland listener thread**: `std::thread::spawn` blocking on `EventListener::start_listener()`, sends `HyprlandMsg` via `std::sync::mpsc`. Main loop drains every 16ms. Auto-restarts on error with 2s backoff.
- **Polling threads**: Battery (30s), volume (1s), network (5s), kube/gcloud (5s) each spawn a dedicated thread that loops with `sleep()` + `sender.input_sender().clone().emit()`
- **Notification daemon thread**: Owns `zbus::blocking::Connection` for D-Bus and `Mutex<rusqlite::Connection>` for SQLite. Receives `DaemonCommand` from UI via `std::sync::mpsc` to emit D-Bus signals.
- **Google Calendar thread**: Creates a **dedicated `tokio::runtime::Runtime`** (isolated from GTK main loop) for `google-calendar3` async API. Polls every 60s.
- **Workspace capture thread**: Separate `wayland_client::Connection` for `hyprland_toplevel_export_manager_v1` protocol. Uses `memfd` shared memory for pixel buffers.

### Multi-Monitor

GDK monitors are matched to Hyprland monitors by `(x, y)` position with index fallback. One `StatusBar` per monitor. Hyprland events are filtered by monitor name. Monitor hotplug handled via `gdk_monitors.connect_items_changed`.

### Bar Layout

`StatusBar` (`bar.rs`) creates a layer-shell window (Top layer, anchored left+top+right, auto exclusive zone) containing a `CenterBox`:
- **Start**: workspaces + kube context + gcloud config
- **Center**: active window title
- **End**: calendar, volume, network, battery, clock

### Widget Patterns

**relm4 SimpleComponent** (clock, battery, volume, network): Standard init/update/update_view cycle. Polling widgets spawn a background thread in `init()`.

**relm4 Component** (notifications, calendar): Use `update_with_view` for direct widget access. Notifications manages separate layer-shell windows per notification. Calendar fires toast/fullscreen notifications to NotificationModel via `relm4::Sender`.

**Generic Component** (`SwitcherModel<P: SwitcherProvider>` in `switcher.rs`): Trait-parameterized widget with popup menu, polling thread, and 500ms focus-leave debounce. `KubeModel` and `GcloudModel` are type aliases — adding a new switcher only requires implementing `SwitcherProvider`.

**Plain structs** (workspaces, active_window): Not relm4 components. Workspaces uses `BTreeMap<i32, Button>` with direct method calls from `StatusBar::handle_hyprland_msg()`. ActiveWindow is just a Label.

### Layer-Shell Popup Pattern

Popups (kube, gcloud, calendar, workspace preview) are separate `Window`s on `Layer::Overlay`, anchored top+left, positioned via margins. Focus leave/enter with a 500ms debounce timer controls dismissal.

### Notification Daemon

`notification_daemon.rs` implements `org.freedesktop.Notifications` D-Bus interface via `zbus::blocking`. Every notification is persisted to SQLite at `$XDG_DATA_HOME/jb-shell/notifications.db`. The `next_id` counter seeds from `MAX(id)` on startup so IDs survive restarts.

UI-to-daemon reverse channel: `std::sync::mpsc::Sender<DaemonCommand>` lets the UI send `NotificationClosed`/`ActionInvoked` back to the daemon thread for D-Bus signal emission via `conn.emit_signal()`.

Notification IDs: freedesktop uses `u32` cast to `u64`. Internal (calendar) uses hash-based IDs from `hash_event_id()`.

### External Commands

- `wpctl get-volume @DEFAULT_AUDIO_SINK@` — volume widget
- `iwctl station <iface> show` — wireless SSID/RSSI
- `kubectl config current-context` / `get-contexts -o name` / `use-context` — kube widget
- `gcloud config configurations list` / `activate` — gcloud widget
- `xdg-open` — opening URLs (meeting links, OAuth)
- Network also reads `/sys/class/net/*/operstate` and `/sys/class/net/*/wireless`

### String Truncation

Use `char_indices()` for truncation, never byte slicing — window titles contain emoji.

### CSS

`style.css` uses GTK4 `@define-color` (not GTK3 `:vars`). Color names use underscores. Catppuccin Mocha theme. Widget IDs match `set_widget_name()` calls. Global reset via `* { all: unset; }`.

**GTK4 CSS does not support** `overflow` or `max-width` properties — these are web CSS only. Font clipping at small sizes is a font metrics issue (JetBrains Mono Nerd Font has bad ascent metrics at <=12px); use MesloLGS NF or avoid sizes below 13px.

CSS is loaded from the first match: `$XDG_CONFIG_HOME/jb-shell/style.css`, next to the binary, or `./style.css`.
