use chrono::{DateTime, Local, TimeZone, Utc};
use google_calendar3::{hyper_rustls, hyper_util, yup_oauth2 as oauth2, CalendarHub};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use tokio::sync::mpsc;

#[derive(Clone, Debug)]
pub struct CalendarEvent {
    pub id: String,
    pub title: String,
    pub start: DateTime<Local>,
    pub end: DateTime<Local>,
    pub meeting_link: Option<String>,
    pub is_all_day: bool,
}

pub enum CalendarThreadMsg {
    TriggerAuth,
}

#[derive(Debug)]
pub enum CalendarResult {
    EventsUpdated(Vec<CalendarEvent>),
    AuthComplete,
    AuthFailed(String),
    AuthRevoked,
    NeedsAuth,
    NoCredentials,
}

struct BrowserFlowDelegate;

impl oauth2::authenticator_delegate::InstalledFlowDelegate for BrowserFlowDelegate {
    fn present_user_url<'a>(
        &'a self,
        url: &'a str,
        _need_code: bool,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
        let url = url.to_string();
        Box::pin(async move {
            eprintln!("jb-shell: opening Google OAuth URL in browser");
            if let Ok(mut child) = std::process::Command::new("xdg-open").arg(&url).spawn() {
                let _ = child.wait();
            }
            Ok(String::new())
        })
    }
}

pub fn credentials_path() -> PathBuf {
    config_dir().join("google-credentials.json")
}

fn config_dir() -> PathBuf {
    std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())).join(".config")
        })
        .join("jb-shell")
}

fn data_dir() -> PathBuf {
    std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())).join(".local/share")
        })
        .join("jb-shell")
}

pub fn spawn_calendar_thread(
    send: impl Fn(CalendarResult) + Send + 'static,
) -> mpsc::Sender<CalendarThreadMsg> {
    let (tx, rx) = mpsc::channel(8);

    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
        rt.block_on(calendar_thread_main(send, rx));
    });

    tx
}

async fn calendar_thread_main(
    send: impl Fn(CalendarResult) + Send + 'static,
    mut rx: mpsc::Receiver<CalendarThreadMsg>,
) {
    let cred_path = config_dir().join("google-credentials.json");

    if !cred_path.exists() {
        eprintln!("jb-shell: no Google credentials at {}", cred_path.display());
        send(CalendarResult::NoCredentials);
        loop {
            std::thread::sleep(std::time::Duration::from_secs(3600));
        }
    }

    let secret = match oauth2::read_application_secret(&cred_path).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("jb-shell: failed to read Google credentials: {e}");
            send(CalendarResult::NoCredentials);
            loop {
                std::thread::sleep(std::time::Duration::from_secs(3600));
            }
        }
    };

    let token_path = data_dir().join("google-tokens.json");
    if let Some(parent) = token_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let auth = match oauth2::InstalledFlowAuthenticator::builder(
        secret,
        oauth2::InstalledFlowReturnMethod::HTTPRedirect,
    )
    .persist_tokens_to_disk(&token_path)
    .flow_delegate(Box::new(BrowserFlowDelegate))
    .build()
    .await
    {
        Ok(a) => a,
        Err(e) => {
            eprintln!("jb-shell: failed to build authenticator: {e}");
            send(CalendarResult::AuthFailed(e.to_string()));
            loop {
                std::thread::sleep(std::time::Duration::from_secs(3600));
            }
        }
    };

    let connector = hyper_rustls::HttpsConnectorBuilder::new()
        .with_native_roots()
        .expect("failed to load native TLS roots")
        .https_or_http()
        .enable_http1()
        .build();
    let client = hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
        .build(connector);
    let hub = CalendarHub::new(client, auth.clone());

    let has_tokens = token_path.exists()
        && std::fs::metadata(&token_path)
            .map(|m| m.len() > 2)
            .unwrap_or(false);

    let mut authenticated = has_tokens;
    if !has_tokens {
        send(CalendarResult::NeedsAuth);
    }

    let mut poll_interval = tokio::time::interval(std::time::Duration::from_secs(60));

    loop {
        tokio::select! {
            Some(msg) = rx.recv() => {
                match msg {
                    CalendarThreadMsg::TriggerAuth => {
                        authenticated = true;
                        send(CalendarResult::AuthComplete);
                        poll_interval.reset();
                    }
                }
            }
            _ = poll_interval.tick() => {}
        }

        if authenticated {
            match fetch_events(&hub).await {
                Ok(events) => {
                    send(CalendarResult::EventsUpdated(events));
                }
                Err(e) => {
                    let err_str = e.to_string();
                    eprintln!("jb-shell: calendar fetch error: {err_str}");
                    if err_str.contains("401") || err_str.contains("nauthorized") {
                        authenticated = false;
                        send(CalendarResult::AuthRevoked);
                    }
                }
            }
        }
    }
}

type HubConnector =
    hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>;

async fn fetch_events(
    hub: &CalendarHub<HubConnector>,
) -> Result<Vec<CalendarEvent>, Box<dyn std::error::Error + Send + Sync>> {
    let now = Local::now();
    let today_start = now.date_naive().and_hms_opt(0, 0, 0).unwrap();
    let tomorrow_start = today_start + chrono::TimeDelta::try_days(1).unwrap();

    let today_start_utc: DateTime<Utc> = Local
        .from_local_datetime(&today_start)
        .unwrap()
        .with_timezone(&Utc);
    let tomorrow_start_utc: DateTime<Utc> = Local
        .from_local_datetime(&tomorrow_start)
        .unwrap()
        .with_timezone(&Utc);

    let (_, event_list) = hub
        .events()
        .list("primary")
        .time_min(today_start_utc)
        .time_max(tomorrow_start_utc)
        .single_events(true)
        .order_by("startTime")
        .doit()
        .await?;

    let mut events = Vec::new();
    for event in event_list.items.unwrap_or_default() {
        // Filter declined events
        if let Some(attendees) = &event.attendees {
            let declined = attendees.iter().any(|a| {
                a.self_.unwrap_or(false) && a.response_status.as_deref() == Some("declined")
            });
            if declined {
                continue;
            }
        }

        let id = event.id.unwrap_or_default();
        let title = event.summary.unwrap_or_else(|| "(no title)".to_string());

        let (start, end, is_all_day) = match (event.start.as_ref(), event.end.as_ref()) {
            (Some(s), Some(e)) => {
                if let (Some(sdt), Some(edt)) = (&s.date_time, &e.date_time) {
                    (sdt.with_timezone(&Local), edt.with_timezone(&Local), false)
                } else if let (Some(sd), Some(ed)) = (&s.date, &e.date) {
                    let start_naive = sd.and_hms_opt(0, 0, 0).unwrap();
                    let end_naive = ed.and_hms_opt(0, 0, 0).unwrap();
                    (
                        Local.from_local_datetime(&start_naive).unwrap(),
                        Local.from_local_datetime(&end_naive).unwrap(),
                        true,
                    )
                } else {
                    continue;
                }
            }
            _ => continue,
        };

        let meeting_link = event.hangout_link.clone().or_else(|| {
            event.conference_data.as_ref().and_then(|cd| {
                cd.entry_points.as_ref().and_then(|eps| {
                    eps.iter()
                        .find(|ep| ep.entry_point_type.as_deref() == Some("video"))
                        .and_then(|ep| ep.uri.clone())
                })
            })
        });

        events.push(CalendarEvent {
            id,
            title,
            start,
            end,
            meeting_link,
            is_all_day,
        });
    }

    Ok(events)
}
