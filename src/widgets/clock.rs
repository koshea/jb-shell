use chrono::Local;
use gtk4::prelude::*;
use gtk4::{Box as GtkBox, Label, Orientation};
use relm4::prelude::*;

pub struct ClockModel {
    date: String,
    time: String,
}

#[derive(Debug)]
pub enum ClockInput {
    Tick,
}

pub struct ClockWidgets {
    date_label: Label,
    time_label: Label,
}

impl SimpleComponent for ClockModel {
    type Init = ();
    type Input = ClockInput;
    type Output = ();
    type Root = GtkBox;
    type Widgets = ClockWidgets;

    fn init_root() -> Self::Root {
        let b = GtkBox::new(Orientation::Horizontal, 8);
        b.set_widget_name("clock");
        b
    }

    fn init(
        _init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let date_label = Label::new(None);
        date_label.set_widget_name("clock-date");
        let time_label = Label::new(None);
        time_label.set_widget_name("clock-time");

        root.append(&date_label);
        root.append(&time_label);

        let now = Local::now();
        let model = ClockModel {
            date: now.format("%a, %b %-d").to_string(),
            time: now.format("%-I:%M %p").to_string(),
        };

        // Clock doesn't do blocking I/O, so a main-thread timer is fine
        let input_sender = sender.input_sender().clone();
        glib::timeout_add_seconds_local(1, move || {
            input_sender.emit(ClockInput::Tick);
            glib::ControlFlow::Continue
        });

        let widgets = ClockWidgets {
            date_label,
            time_label,
        };
        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, _sender: ComponentSender<Self>) {
        match msg {
            ClockInput::Tick => {
                let now = Local::now();
                self.date = now.format("%a, %b %-d").to_string();
                self.time = now.format("%-I:%M %p").to_string();
            }
        }
    }

    fn update_view(&self, widgets: &mut Self::Widgets, _sender: ComponentSender<Self>) {
        widgets.date_label.set_label(&self.date);
        widgets.time_label.set_label(&self.time);
    }
}
