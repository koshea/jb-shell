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

jb-shell is a Wayland status bar for Hyprland, built with GTK4, gtk4-layer-shell, and relm4. It runs on the **glib main loop only** — no async runtime (no tokio).

### Threading Model

- **Main thread**: GTK4 glib event loop — all UI updates, component lifecycle, timers
- **Hyprland listener thread**: `std::thread::spawn` running `EventListener::start_listener()` (blocking), sends `HyprlandMsg` via `std::sync::mpsc`. Main loop drains the channel every 16ms with `glib::timeout_add_local`.
- **Polling threads**: Each polling widget (battery, volume, network, kube) spawns a dedicated thread that loops with `sleep()`, sending results back via `sender.input_sender().clone().emit()`

### Multi-Monitor

GDK monitors are matched to Hyprland monitors by `(x, y)` position with index fallback. One `StatusBar` per monitor. Hyprland events are filtered by monitor name.

### Bar Layout

`StatusBar` (`bar.rs`) creates a layer-shell window (Top layer, anchored left+top+right, exclusive zone) containing a `CenterBox`:
- **Start**: workspaces + kube context
- **Center**: active window title
- **End**: volume, network, battery, clock

### Widget Patterns

**relm4 SimpleComponent** (clock, battery, volume, network): Standard init/update/update_view cycle. Polling widgets spawn a background thread in `init()` that sends input messages.

**relm4 Component** (kube_context): Uses `update_with_view` for direct widget access during updates (needed for popup focus timer management). The popup is a separate layer-shell window on the Overlay layer, positioned via margins relative to bar height and trigger button position.

**Plain structs** (workspaces, active_window): Not relm4 components. Workspaces uses `BTreeMap<i32, Button>` with direct method calls from `StatusBar::handle_hyprland_msg()`. ActiveWindow is just a Label.

### Layer-Shell Popup Pattern

The kube context popup is a separate `Window` with `Layer::Overlay`, anchored top+left, positioned via `set_margin(Edge::Top, bar_height)` and `set_margin(Edge::Left, trigger_x)`. Focus leave/enter with a 500ms debounce timer controls dismissal.

### External Commands

- `wpctl get-volume @DEFAULT_AUDIO_SINK@` — volume widget
- `iwctl station <iface> show` — wireless SSID/RSSI
- `kubectl config current-context` / `get-contexts -o name` — kube widget

### String Truncation

Use `char_indices()` for truncation, never byte slicing — window titles contain emoji.

### CSS

`style.css` uses GTK4 `@define-color` (not GTK3 `:vars`). Color names use underscores. Catppuccin Mocha theme. Widget IDs match `set_widget_name()` calls.
