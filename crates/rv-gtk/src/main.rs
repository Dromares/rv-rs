//! `rv-gtk`: the GTK4 front end of rv-rs. See the library crate for the windows.

fn main() {
    if std::env::args()
        .nth(1)
        .is_some_and(|a| a == "-V" || a == "--version")
    {
        println!("rv-gtk {}", rv_update::current_version());
        return;
    }
    match rv_gtk::ui::Args::parse(std::env::args()) {
        Ok(args) => rv_gtk::ui::run(args),
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    }
}
