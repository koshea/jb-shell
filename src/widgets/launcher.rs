use gdk4::Monitor;
use gtk4::prelude::*;
use gtk4::{Box as GtkBox, EventControllerKey, Image, Label, Orientation, SearchEntry, Window};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use relm4::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

// ── Desktop app ──────────────────────────────────────────────────────

#[derive(Clone, Debug)]
#[allow(dead_code)]
struct DesktopApp {
    id: String, // e.g. "firefox.desktop"
    name: String,
    exec: String,
    icon: Option<String>,
    comment: Option<String>,
    categories: Vec<String>,
    keywords: Vec<String>,
}

// ── Frecency ─────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
struct FrecencyEntry {
    count: u32,
    last_used: u64,
}

fn frecency_path() -> PathBuf {
    let data_dir = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
                .join(".local/share")
        })
        .join("jb-shell");
    std::fs::create_dir_all(&data_dir).ok();
    data_dir.join("launcher_frecency.json")
}

fn load_frecency() -> HashMap<String, FrecencyEntry> {
    std::fs::read_to_string(frecency_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_frecency(frecency: &HashMap<String, FrecencyEntry>) {
    if let Ok(json) = serde_json::to_string_pretty(frecency) {
        let _ = std::fs::write(frecency_path(), json);
    }
}

fn frecency_score(entry: &FrecencyEntry) -> f64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let age_secs = now.saturating_sub(entry.last_used);
    let recency_weight = if age_secs < 3600 {
        1.0
    } else if age_secs < 86400 {
        0.8
    } else if age_secs < 604800 {
        0.5
    } else {
        0.2
    };
    entry.count as f64 * recency_weight
}

// ── .desktop file parsing ────────────────────────────────────────────

fn xdg_app_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    if let Ok(data_home) = std::env::var("XDG_DATA_HOME") {
        dirs.push(PathBuf::from(data_home).join("applications"));
    } else if let Ok(home) = std::env::var("HOME") {
        dirs.push(PathBuf::from(home).join(".local/share/applications"));
    }

    let data_dirs = std::env::var("XDG_DATA_DIRS")
        .unwrap_or_else(|_| "/usr/local/share:/usr/share".to_string());
    for dir in data_dirs.split(':') {
        if !dir.is_empty() {
            dirs.push(PathBuf::from(dir).join("applications"));
        }
    }

    dirs
}

fn scan_desktop_files() -> Vec<DesktopApp> {
    let mut apps = Vec::new();
    let mut seen_ids = std::collections::HashSet::new();

    for dir in xdg_app_dirs() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("desktop") {
                continue;
            }
            let id = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            // First occurrence wins (XDG_DATA_HOME overrides system)
            if seen_ids.contains(&id) {
                continue;
            }
            if let Some(app) = parse_desktop_file(&path, &id) {
                seen_ids.insert(id);
                apps.push(app);
            }
        }
    }

    apps.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    apps
}

fn parse_desktop_file(path: &std::path::Path, id: &str) -> Option<DesktopApp> {
    let content = std::fs::read_to_string(path).ok()?;
    let mut in_desktop_entry = false;
    let mut name = None;
    let mut exec = None;
    let mut icon = None;
    let mut comment = None;
    let mut categories = Vec::new();
    let mut keywords = Vec::new();
    let mut app_type = None;
    let mut no_display = false;
    let mut hidden = false;

    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_desktop_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_desktop_entry {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            let value = value.trim();
            // Skip localized keys (e.g. Name[de])
            if key.contains('[') {
                continue;
            }
            match key {
                "Name" => name = Some(value.to_string()),
                "Exec" => exec = Some(value.to_string()),
                "Icon" => icon = Some(value.to_string()),
                "Comment" => comment = Some(value.to_string()),
                "Categories" => {
                    categories = value
                        .split(';')
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string())
                        .collect();
                }
                "Keywords" => {
                    keywords = value
                        .split(';')
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string())
                        .collect();
                }
                "Type" => app_type = Some(value.to_string()),
                "NoDisplay" => no_display = value.eq_ignore_ascii_case("true"),
                "Hidden" => hidden = value.eq_ignore_ascii_case("true"),
                _ => {}
            }
        }
    }

    if app_type.as_deref() != Some("Application") || no_display || hidden {
        return None;
    }

    Some(DesktopApp {
        id: id.to_string(),
        name: name?,
        exec: exec?,
        icon,
        comment,
        categories,
        keywords,
    })
}

// ── Search / ranking ─────────────────────────────────────────────────

const MAX_RESULTS: usize = 8;

fn filter_and_rank(
    apps: &[DesktopApp],
    query: &str,
    frecency: &HashMap<String, FrecencyEntry>,
) -> Vec<usize> {
    if query.is_empty() {
        // Return top frecent apps
        let mut indices: Vec<usize> = (0..apps.len()).collect();
        indices.sort_by(|&a, &b| {
            let sa = frecency
                .get(&apps[a].id)
                .map(frecency_score)
                .unwrap_or(0.0);
            let sb = frecency
                .get(&apps[b].id)
                .map(frecency_score)
                .unwrap_or(0.0);
            sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
        });
        indices.truncate(MAX_RESULTS);
        return indices;
    }

    let q = query.to_lowercase();

    // Tier 1: exact prefix match on name
    let mut tier1 = Vec::new();
    // Tier 2: word-boundary match on name
    let mut tier2 = Vec::new();
    // Tier 3: substring match on name
    let mut tier3 = Vec::new();
    // Tier 4: match on keywords/categories
    let mut tier4 = Vec::new();

    for (i, app) in apps.iter().enumerate() {
        let name_lower = app.name.to_lowercase();
        if name_lower.starts_with(&q) {
            tier1.push(i);
        } else if word_boundary_match(&name_lower, &q) {
            tier2.push(i);
        } else if name_lower.contains(&q) {
            tier3.push(i);
        } else if app
            .keywords
            .iter()
            .any(|k| k.to_lowercase().contains(&q))
            || app
                .categories
                .iter()
                .any(|c| c.to_lowercase().contains(&q))
        {
            tier4.push(i);
        }
    }

    let sort_by_frecency = |indices: &mut Vec<usize>| {
        indices.sort_by(|&a, &b| {
            let sa = frecency
                .get(&apps[a].id)
                .map(frecency_score)
                .unwrap_or(0.0);
            let sb = frecency
                .get(&apps[b].id)
                .map(frecency_score)
                .unwrap_or(0.0);
            sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
        });
    };

    sort_by_frecency(&mut tier1);
    sort_by_frecency(&mut tier2);
    sort_by_frecency(&mut tier3);
    sort_by_frecency(&mut tier4);

    let mut result = Vec::new();
    for tier in [tier1, tier2, tier3, tier4] {
        for idx in tier {
            if result.len() >= MAX_RESULTS {
                break;
            }
            result.push(idx);
        }
        if result.len() >= MAX_RESULTS {
            break;
        }
    }
    result
}

fn word_boundary_match(name: &str, query: &str) -> bool {
    // Check if query matches starting at any word boundary in name
    for (i, _) in name.char_indices() {
        if i == 0 {
            continue; // Skip — prefix match is tier 1
        }
        let before = name.as_bytes().get(i.wrapping_sub(1)).copied().unwrap_or(0);
        if before == b' ' || before == b'-' || before == b'_' {
            if name[i..].starts_with(query) {
                return true;
            }
        }
    }
    false
}

// ── Exec field processing ────────────────────────────────────────────

fn process_exec(exec: &str) -> Vec<String> {
    // Strip field codes
    let cleaned: String = exec
        .split_whitespace()
        .filter(|tok| {
            !matches!(
                *tok,
                "%f" | "%F" | "%u" | "%U" | "%i" | "%c" | "%k" | "%d" | "%D" | "%n" | "%N"
                    | "%v" | "%m"
            )
        })
        .collect::<Vec<_>>()
        .join(" ");

    // Strip env VAR=val prefixes
    let mut parts: Vec<&str> = cleaned.split_whitespace().collect();
    while parts.len() > 1 && parts[0].contains('=') && !parts[0].starts_with('/') {
        // Looks like "env" or "VAR=val"
        if parts[0] == "env" {
            parts.remove(0);
        } else {
            parts.remove(0);
        }
    }

    parts.iter().map(|s| s.to_string()).collect()
}

fn launch_app(app: &DesktopApp, frecency: &mut HashMap<String, FrecencyEntry>) {
    let args = process_exec(&app.exec);
    if args.is_empty() {
        return;
    }

    let program = &args[0];
    let cmd_args = &args[1..];

    match std::process::Command::new(program)
        .args(cmd_args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0) // setsid equivalent — detach from parent
        .spawn()
    {
        Ok(_) => {
            eprintln!("jb-shell: [launcher] launched {}", app.id);
        }
        Err(e) => {
            eprintln!("jb-shell: [launcher] failed to launch {}: {e}", app.id);
        }
    }

    // Bump frecency
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let entry = frecency.entry(app.id.clone()).or_insert(FrecencyEntry {
        count: 0,
        last_used: now,
    });
    entry.count += 1;
    entry.last_used = now;
    save_frecency(frecency);
}

// ── D-Bus activation ─────────────────────────────────────────────────

struct LauncherDbus {
    sender: relm4::Sender<LauncherInput>,
}

#[zbus::interface(name = "dev.jb.shell.Launcher")]
impl LauncherDbus {
    fn toggle(&self) {
        self.sender.emit(LauncherInput::Toggle);
    }
}

fn spawn_launcher_dbus(sender: relm4::Sender<LauncherInput>) {
    std::thread::spawn(move || {
        let server = LauncherDbus { sender };
        let _conn = match zbus::blocking::connection::Builder::session()
            .expect("failed to create session bus builder")
            .serve_at("/dev/jb/shell/Launcher", server)
            .expect("failed to register launcher interface")
            .name("dev.jb.shell.Launcher")
            .expect("failed to set launcher bus name")
            .build()
        {
            Ok(conn) => conn,
            Err(e) => {
                eprintln!("jb-shell: [launcher] failed to acquire bus name: {e}");
                return;
            }
        };

        eprintln!("jb-shell: [launcher] D-Bus interface listening");

        // Block forever — zbus dispatches on its own executor
        loop {
            std::thread::park();
        }
    });
}

// ── relm4 Component ──────────────────────────────────────────────────

pub struct LauncherModel {
    visible: bool,
    search_text: String,
    apps: Vec<DesktopApp>,
    filtered: Vec<usize>,
    selected_index: usize,
    frecency: HashMap<String, FrecencyEntry>,
    last_scan: Instant,
}

#[derive(Debug)]
pub enum LauncherInput {
    Toggle,
    SearchChanged(String),
    Activate,
    MoveUp,
    MoveDown,
    Hide,
}

pub struct LauncherWidgets {
    overlay: Window,
    search_entry: SearchEntry,
    results_box: GtkBox,
}

impl Component for LauncherModel {
    type Init = Monitor;
    type Input = LauncherInput;
    type Output = ();
    type CommandOutput = ();
    type Root = GtkBox;
    type Widgets = LauncherWidgets;

    fn init_root() -> Self::Root {
        // Invisible root — the real UI is the overlay window
        GtkBox::new(Orientation::Horizontal, 0)
    }

    fn init(
        init: Self::Init,
        _root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let monitor = init;

        // ── Build overlay window ──
        let overlay = Window::new();
        overlay.set_widget_name("launcher-overlay");
        overlay.init_layer_shell();
        overlay.set_layer(Layer::Overlay);
        overlay.set_exclusive_zone(-1);
        overlay.set_anchor(Edge::Top, true);
        overlay.set_anchor(Edge::Bottom, true);
        overlay.set_anchor(Edge::Left, true);
        overlay.set_anchor(Edge::Right, true);
        overlay.set_keyboard_mode(KeyboardMode::Exclusive);
        overlay.set_monitor(Some(&monitor));

        // Outer container — centers the card
        let outer = GtkBox::new(Orientation::Vertical, 0);
        outer.set_valign(gtk4::Align::Center);
        outer.set_halign(gtk4::Align::Center);
        outer.set_vexpand(true);
        outer.set_hexpand(true);

        // Card
        let card = GtkBox::new(Orientation::Vertical, 8);
        card.set_widget_name("launcher-card");

        // Search entry
        let search_entry = SearchEntry::new();
        search_entry.set_widget_name("launcher-search");
        search_entry.set_placeholder_text(Some("Search applications..."));
        card.append(&search_entry);

        let search_sender = sender.input_sender().clone();
        search_entry.connect_search_changed(move |entry| {
            search_sender.emit(LauncherInput::SearchChanged(entry.text().to_string()));
        });

        // Results list
        let results_box = GtkBox::new(Orientation::Vertical, 0);
        results_box.set_widget_name("launcher-results");
        card.append(&results_box);

        outer.append(&card);
        overlay.set_child(Some(&outer));
        overlay.set_visible(false);

        // ── Keyboard handling ──
        // Attach to SearchEntry in capture phase so we intercept before it eats
        // Escape/Enter/arrows.
        let key_ctl = EventControllerKey::new();
        key_ctl.set_propagation_phase(gtk4::PropagationPhase::Capture);
        let key_sender = sender.input_sender().clone();
        key_ctl.connect_key_pressed(move |_, keyval, _keycode, state| {
            let ctrl = state.contains(gdk4::ModifierType::CONTROL_MASK);
            match keyval {
                gdk4::Key::Escape => {
                    key_sender.emit(LauncherInput::Hide);
                    glib::Propagation::Stop
                }
                gdk4::Key::Return | gdk4::Key::KP_Enter => {
                    key_sender.emit(LauncherInput::Activate);
                    glib::Propagation::Stop
                }
                gdk4::Key::Up => {
                    key_sender.emit(LauncherInput::MoveUp);
                    glib::Propagation::Stop
                }
                gdk4::Key::Down => {
                    key_sender.emit(LauncherInput::MoveDown);
                    glib::Propagation::Stop
                }
                gdk4::Key::j if ctrl => {
                    key_sender.emit(LauncherInput::MoveDown);
                    glib::Propagation::Stop
                }
                gdk4::Key::k if ctrl => {
                    key_sender.emit(LauncherInput::MoveUp);
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        search_entry.add_controller(key_ctl);

        // ── Scan apps + load frecency ──
        let apps = scan_desktop_files();
        let frecency = load_frecency();
        let filtered = filter_and_rank(&apps, "", &frecency);

        eprintln!(
            "jb-shell: [launcher] scanned {} desktop apps",
            apps.len()
        );

        // ── Spawn D-Bus thread ──
        spawn_launcher_dbus(sender.input_sender().clone());

        let model = LauncherModel {
            visible: false,
            search_text: String::new(),
            apps,
            filtered,
            selected_index: 0,
            frecency,
            last_scan: Instant::now(),
        };

        let widgets = LauncherWidgets {
            overlay,
            search_entry,
            results_box,
        };

        ComponentParts { model, widgets }
    }

    fn update_with_view(
        &mut self,
        widgets: &mut Self::Widgets,
        message: Self::Input,
        _sender: ComponentSender<Self>,
        _root: &Self::Root,
    ) {
        match message {
            LauncherInput::Toggle => {
                if self.visible {
                    self.visible = false;
                } else {
                    // Re-scan if >30s since last
                    if self.last_scan.elapsed().as_secs() > 30 {
                        self.apps = scan_desktop_files();
                        self.last_scan = Instant::now();
                    }
                    self.search_text.clear();
                    self.filtered = filter_and_rank(&self.apps, "", &self.frecency);
                    self.selected_index = 0;
                    self.visible = true;
                    widgets.search_entry.set_text("");
                }
            }
            LauncherInput::SearchChanged(text) => {
                self.search_text = text;
                self.filtered = filter_and_rank(&self.apps, &self.search_text, &self.frecency);
                self.selected_index = 0;
            }
            LauncherInput::MoveDown => {
                if !self.filtered.is_empty() && self.selected_index + 1 < self.filtered.len() {
                    self.selected_index += 1;
                }
            }
            LauncherInput::MoveUp => {
                if self.selected_index > 0 {
                    self.selected_index -= 1;
                }
            }
            LauncherInput::Activate => {
                if let Some(&app_idx) = self.filtered.get(self.selected_index) {
                    let app = self.apps[app_idx].clone();
                    launch_app(&app, &mut self.frecency);
                    self.visible = false;
                }
            }
            LauncherInput::Hide => {
                self.visible = false;
            }
        }

        self.update_view(widgets, _sender);
    }

    fn update_view(&self, widgets: &mut Self::Widgets, _sender: ComponentSender<Self>) {
        if self.visible {
            self.rebuild_results(&widgets.results_box);
            widgets.overlay.set_visible(true);
            widgets.search_entry.grab_focus();
        } else {
            widgets.overlay.set_visible(false);
        }
    }
}

impl LauncherModel {
    fn rebuild_results(&self, results_box: &GtkBox) {
        // Clear existing children
        while let Some(child) = results_box.first_child() {
            results_box.remove(&child);
        }

        if self.filtered.is_empty() {
            let empty = Label::new(Some("No matches"));
            empty.add_css_class("launcher-empty");
            empty.set_halign(gtk4::Align::Start);
            results_box.append(&empty);
            return;
        }

        for (i, &app_idx) in self.filtered.iter().enumerate() {
            let app = &self.apps[app_idx];
            let row = GtkBox::new(Orientation::Horizontal, 8);
            row.add_css_class("launcher-item");
            if i == self.selected_index {
                row.add_css_class("selected");
            }

            // Icon
            let icon = if let Some(ref icon_name) = app.icon {
                if icon_name.starts_with('/') {
                    Image::from_file(icon_name)
                } else {
                    Image::from_icon_name(icon_name)
                }
            } else {
                Image::from_icon_name("application-x-executable")
            };
            icon.set_pixel_size(24);
            icon.add_css_class("app-icon");
            row.append(&icon);

            // Text column
            let text_box = GtkBox::new(Orientation::Vertical, 0);

            let name_label = Label::new(Some(&app.name));
            name_label.add_css_class("app-name");
            name_label.set_halign(gtk4::Align::Start);
            text_box.append(&name_label);

            // Show comment or first category as secondary text
            let secondary = app
                .comment
                .as_deref()
                .or_else(|| app.categories.first().map(|s| s.as_str()));
            if let Some(text) = secondary {
                let desc_label = Label::new(Some(&truncate_str(text, 60)));
                desc_label.add_css_class("app-comment");
                desc_label.set_halign(gtk4::Align::Start);
                desc_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
                text_box.append(&desc_label);
            }

            row.append(&text_box);
            results_box.append(&row);
        }
    }
}

fn truncate_str(s: &str, max_len: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_len {
        return s.to_string();
    }
    let end: usize = s
        .char_indices()
        .nth(max_len)
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    format!("{}...", &s[..end])
}
