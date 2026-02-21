use crate::widgets::notifications::focus_app_window;
use gtk4::prelude::*;
use gtk4::{Box as GtkBox, Label, Orientation};
use relm4::prelude::*;
use std::collections::HashMap;
use std::time::Duration;
use zbus::zvariant::OwnedValue;

pub struct MprisModel {
    playing: bool,
    artist: String,
    title: String,
    focus_hints: Vec<String>,
    title_keywords: Vec<String>,
}

#[derive(Debug)]
pub enum MprisInput {
    Update {
        artist: String,
        title: String,
        focus_hints: Vec<String>,
        title_keywords: Vec<String>,
    },
    Inactive,
    Raise,
}

pub struct MprisWidgets {
    root: GtkBox,
    label: Label,
}

impl SimpleComponent for MprisModel {
    type Init = ();
    type Input = MprisInput;
    type Output = ();
    type Root = GtkBox;
    type Widgets = MprisWidgets;

    fn init_root() -> Self::Root {
        let b = GtkBox::new(Orientation::Horizontal, 8);
        b.set_widget_name("mpris-player");
        b.set_valign(gtk4::Align::Center);
        b.set_visible(false);
        b
    }

    fn init(
        _init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let icon = Label::new(Some("\u{f001}"));
        let label = Label::new(None);

        root.append(&icon);
        root.append(&label);

        let click = gtk4::GestureClick::new();
        let click_sender = sender.input_sender().clone();
        click.connect_released(move |_, _, _, _| {
            click_sender.emit(MprisInput::Raise);
        });
        root.add_controller(click);

        let input_sender = sender.input_sender().clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("mpris tokio runtime");
            rt.block_on(mpris_poll_loop(input_sender));
        });

        let model = MprisModel {
            playing: false,
            artist: String::new(),
            title: String::new(),
            focus_hints: Vec::new(),
            title_keywords: Vec::new(),
        };
        let widgets = MprisWidgets {
            root: root.clone(),
            label,
        };
        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, _sender: ComponentSender<Self>) {
        match msg {
            MprisInput::Update {
                artist,
                title,
                focus_hints,
                title_keywords,
            } => {
                self.playing = true;
                self.artist = artist;
                self.title = title;
                self.focus_hints = focus_hints;
                self.title_keywords = title_keywords;
            }
            MprisInput::Inactive => {
                self.playing = false;
                self.focus_hints.clear();
                self.title_keywords.clear();
            }
            MprisInput::Raise => {
                if !self.focus_hints.is_empty() {
                    let class_refs: Vec<&str> =
                        self.focus_hints.iter().map(|s| s.as_str()).collect();
                    let title_refs: Vec<&str> =
                        self.title_keywords.iter().map(|s| s.as_str()).collect();
                    focus_app_window(&class_refs, &title_refs, None);
                }
            }
        }
    }

    fn update_view(&self, widgets: &mut Self::Widgets, _sender: ComponentSender<Self>) {
        if self.playing {
            let text = if self.artist.is_empty() {
                self.title.clone()
            } else {
                format!("{} — {}", self.artist, self.title)
            };
            let truncated = truncate_str(&text, 40);
            widgets.label.set_label(&truncated);
            widgets.root.set_visible(true);
            widgets.root.add_css_class("playing");
        } else {
            widgets.root.set_visible(false);
            widgets.root.remove_css_class("playing");
        }
    }
}

fn truncate_str(s: &str, max_chars: usize) -> String {
    if let Some((idx, _)) = s.char_indices().nth(max_chars) {
        format!("{}…", &s[..idx])
    } else {
        s.to_string()
    }
}

struct MprisInfo {
    artist: String,
    title: String,
    focus_hints: Vec<String>,
    title_keywords: Vec<String>,
}

async fn mpris_poll_loop(input_sender: relm4::Sender<MprisInput>) {
    let mut conn: Option<zbus::Connection> = None;
    // Cache focus hints per player bus name (these don't change)
    let mut cached_hints: HashMap<String, Vec<String>> = HashMap::new();

    loop {
        if conn.is_none() {
            conn = zbus::Connection::session().await.ok();
        }

        if let Some(ref c) = conn {
            match poll_mpris(c, &mut cached_hints).await {
                Ok(Some(info)) => {
                    input_sender.emit(MprisInput::Update {
                        artist: info.artist,
                        title: info.title,
                        focus_hints: info.focus_hints,
                        title_keywords: info.title_keywords,
                    });
                }
                Ok(None) => {
                    input_sender.emit(MprisInput::Inactive);
                }
                Err(_) => {
                    conn = None;
                    input_sender.emit(MprisInput::Inactive);
                }
            }
        } else {
            input_sender.emit(MprisInput::Inactive);
        }

        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

async fn read_string_prop(
    conn: &zbus::Connection,
    dest: &str,
    interface: &str,
    prop: &str,
) -> Option<String> {
    let reply = conn
        .call_method(
            Some(dest),
            "/org/mpris/MediaPlayer2",
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &(interface, prop),
        )
        .await
        .ok()?;
    let val: OwnedValue = reply.body().deserialize().ok()?;
    String::try_from(val).ok()
}

async fn poll_mpris(
    conn: &zbus::Connection,
    cached_hints: &mut HashMap<String, Vec<String>>,
) -> zbus::Result<Option<MprisInfo>> {
    let reply = conn
        .call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "ListNames",
            &(),
        )
        .await?;

    let names: Vec<String> = reply.body().deserialize()?;
    let mpris_name = match names
        .iter()
        .find(|n| n.starts_with("org.mpris.MediaPlayer2."))
    {
        Some(n) => n,
        None => return Ok(None),
    };

    // Read PlaybackStatus
    let status = read_string_prop(
        conn,
        mpris_name,
        "org.mpris.MediaPlayer2.Player",
        "PlaybackStatus",
    )
    .await;
    if status.as_deref() != Some("Playing") {
        return Ok(None);
    }

    // Read Metadata
    let meta_reply = conn
        .call_method(
            Some(mpris_name.as_str()),
            "/org/mpris/MediaPlayer2",
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &("org.mpris.MediaPlayer2.Player", "Metadata"),
        )
        .await?;

    let meta_val: OwnedValue = meta_reply.body().deserialize()?;
    let meta_dict: HashMap<String, OwnedValue> = match meta_val.try_into() {
        Ok(d) => d,
        Err(_) => return Ok(None),
    };

    let title = meta_dict
        .get("xesam:title")
        .and_then(|v| String::try_from(v.clone()).ok())
        .unwrap_or_default();

    let artist = meta_dict
        .get("xesam:artist")
        .and_then(|v| <Vec<String>>::try_from(v.clone()).ok())
        .map(|a| a.join(", "))
        .unwrap_or_default();

    if title.is_empty() {
        return Ok(None);
    }

    // Get or compute focus hints (cached per player bus name)
    let focus_hints = cached_hints
        .entry(mpris_name.clone())
        .or_insert_with(|| {
            // Can't await in or_insert_with, compute synchronously from bus name
            let mut hints = Vec::new();
            if let Some(player) = mpris_name
                .strip_prefix("org.mpris.MediaPlayer2.")
                .map(|s| s.split('.').next().unwrap_or(s).to_string())
            {
                hints.push(player);
            }
            hints
        })
        .clone();

    // Fetch DesktopEntry/Identity once if not yet in cache
    if cached_hints.get(mpris_name).map_or(true, |h| h.len() <= 1) {
        let mut hints = cached_hints.get(mpris_name).cloned().unwrap_or_default();
        if let Some(entry) = read_string_prop(
            conn,
            mpris_name,
            "org.mpris.MediaPlayer2",
            "DesktopEntry",
        )
        .await
        {
            hints.insert(0, entry);
        }
        if let Some(identity) =
            read_string_prop(conn, mpris_name, "org.mpris.MediaPlayer2", "Identity").await
        {
            hints.push(identity);
        }
        cached_hints.insert(mpris_name.clone(), hints);
    }

    let focus_hints = cached_hints.get(mpris_name).cloned().unwrap_or(focus_hints);

    let mut title_keywords = Vec::new();
    if !title.is_empty() {
        title_keywords.push(title.clone());
    }
    if !artist.is_empty() {
        title_keywords.push(artist.clone());
    }

    Ok(Some(MprisInfo {
        artist,
        title,
        focus_hints,
        title_keywords,
    }))
}
