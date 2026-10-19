//! `rv-gtk`: GTK4 front end. Placeholder: opens an empty window.

use gtk4::prelude::*;
use relm4::prelude::*;

/// Window title, also the application name shown by the desktop.
const WINDOW_TITLE: &str = "rv-rs";

struct App;

#[relm4::component]
impl SimpleComponent for App {
    type Init = ();
    type Input = ();
    type Output = ();

    view! {
        gtk4::Window {
            set_title: Some(WINDOW_TITLE),
            set_default_size: (800, 600),
        }
    }

    fn init(
        _init: Self::Init,
        root: Self::Root,
        _sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let model = App;
        let widgets = view_output!();
        ComponentParts { model, widgets }
    }
}

fn main() {
    RelmApp::new("rs.rv.gtk").run::<App>(());
}
