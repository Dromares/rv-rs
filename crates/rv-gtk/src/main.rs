//! `rv-gtk`: the GTK4 front end of rv-rs. See the library crate for the windows.

fn main() {
    match rv_gtk::ui::Args::parse(std::env::args()) {
        Ok(args) => rv_gtk::ui::run(args),
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    }
}
