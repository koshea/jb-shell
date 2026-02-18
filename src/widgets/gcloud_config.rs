use std::process::Command;
use std::time::Duration;

use super::switcher::{SwitcherModel, SwitcherProvider};

pub struct GcloudProvider;

impl SwitcherProvider for GcloudProvider {
    const WIDGET_NAME: &'static str = "gcloud-config";
    const TRIGGER_NAME: &'static str = "gcloud-trigger";
    const POPUP_NAME: &'static str = "gcloud-popup";
    const MENU_ITEM_NAME: &'static str = "gcloud-menu-item";
    const MENU_BOX_NAME: &'static str = "gcloud-menu";
    const ICON: &'static str = "\u{2601}";
    const ICON_CSS_CLASSES: &'static [&'static str] = &["gcloud-icon"];
    const FALLBACK_LABEL: &'static str = "no config";
    const MAX_LABEL_LEN: usize = 20;
    const POLL_INTERVAL: Duration = Duration::from_secs(5);

    fn poll() -> (String, Vec<String>) {
        let current = Command::new("gcloud")
            .args([
                "config",
                "configurations",
                "list",
                "--filter=is_active=true",
                "--format=value(name)",
            ])
            .output()
            .ok()
            .and_then(|o| {
                if o.status.success() {
                    Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
                } else {
                    None
                }
            })
            .unwrap_or_default();

        let items = Command::new("gcloud")
            .args(["config", "configurations", "list", "--format=value(name)"])
            .output()
            .ok()
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .filter(|l| !l.is_empty())
                    .map(|l| l.to_string())
                    .collect()
            })
            .unwrap_or_default();

        (current, items)
    }

    fn switch(name: &str) {
        let _ = Command::new("gcloud")
            .args(["config", "configurations", "activate", name])
            .output();
    }
}

pub type GcloudModel = SwitcherModel<GcloudProvider>;
