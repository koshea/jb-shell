use std::process::Command;
use std::time::Duration;

use super::switcher::{SwitcherModel, SwitcherProvider};

pub struct KubeProvider;

impl SwitcherProvider for KubeProvider {
    const WIDGET_NAME: &'static str = "kube-context";
    const TRIGGER_NAME: &'static str = "kube-trigger";
    const POPUP_NAME: &'static str = "kube-popup";
    const MENU_ITEM_NAME: &'static str = "kube-menu-item";
    const MENU_BOX_NAME: &'static str = "kube-menu";
    const ICON: &'static str = "\u{2388}";
    const ICON_CSS_CLASSES: &'static [&'static str] = &["kube-helm"];
    const FALLBACK_LABEL: &'static str = "no context";
    const MAX_LABEL_LEN: usize = 24;
    const POLL_INTERVAL: Duration = Duration::from_secs(5);

    fn poll() -> (String, Vec<String>) {
        let current = Command::new("kubectl")
            .args(["config", "current-context"])
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

        let items = Command::new("kubectl")
            .args(["config", "get-contexts", "-o", "name"])
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
        let _ = Command::new("kubectl")
            .args(["config", "use-context", name])
            .output();
    }
}

pub type KubeModel = SwitcherModel<KubeProvider>;
