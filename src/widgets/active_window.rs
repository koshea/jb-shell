use gtk4::prelude::*;
use gtk4::{Box as GtkBox, Label, Orientation};

pub struct ActiveWindowWidget {
    pub container: GtkBox,
    label: Label,
}

impl ActiveWindowWidget {
    pub fn new() -> Self {
        let container = GtkBox::new(Orientation::Horizontal, 0);
        container.set_widget_name("active-window");

        let label = Label::new(Some("Desktop"));
        container.append(&label);

        Self { container, label }
    }

    pub fn set_title(&self, title: &str) {
        let display = if title.is_empty() {
            "Desktop".to_string()
        } else if title.chars().count() > 60 {
            let end: usize = title
                .char_indices()
                .nth(57)
                .map(|(i, _)| i)
                .unwrap_or(title.len());
            format!("{}...", &title[..end])
        } else {
            title.to_string()
        };
        self.label.set_label(&display);
    }
}
