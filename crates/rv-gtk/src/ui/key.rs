//! The key to the colours and symbols: what each status of a ROM means.

use gtk4::prelude::*;
use rv_core::RepStatus;

use super::icons::draw_chip;
use super::painted::Painted;
use crate::colours::css_class;

/// `(status, what it means)`, in the groups the window shows.
pub const KEY: &[(&str, &[(RepStatus, &str)])] = &[
    (
        "Basic statuses",
        &[
            (RepStatus::CORRECT, "The ROM is there and matches the DAT."),
            (
                RepStatus::CORRECT_MIA,
                "A ROM that was reported missing in action, and you have it.",
            ),
            (
                RepStatus::MISSING,
                "The DAT lists the ROM and it is not there.",
            ),
            (
                RepStatus::MISSING_NEW,
                "Missing, and new in the DAT since the last time.",
            ),
            (
                RepStatus::MISSING_MIA,
                "Missing, and known to be private or lost (MIA).",
            ),
            (
                RepStatus::UNKNOWN,
                "A file that no DAT lists. Find Fixes decides what to do with it.",
            ),
            (
                RepStatus::UN_NEEDED,
                "In a merged set, the ROM belongs to the parent set.",
            ),
            (
                RepStatus::NOT_COLLECTED,
                "In a merged set, the ROM is not collected here and not present.",
            ),
            (RepStatus::IN_TO_SORT, "A file in a ToSort directory."),
            (RepStatus::IGNORE, "A file that matches an ignore rule."),
        ],
    ),
    (
        "Fix statuses (set by Find Fixes)",
        &[
            (
                RepStatus::CAN_BE_FIXED,
                "Missing here, and a copy exists elsewhere that Fix will use.",
            ),
            (
                RepStatus::CAN_BE_FIXED_MIA,
                "A missing-in-action ROM that a copy elsewhere can fix.",
            ),
            (
                RepStatus::NEEDED_FOR_FIX,
                "The file is needed to fix something else.",
            ),
            (
                RepStatus::RENAME,
                "The file is needed here under another name.",
            ),
            (
                RepStatus::MOVE_TO_SORT,
                "Not needed here and no copy elsewhere: Fix moves it to ToSort.",
            ),
            (
                RepStatus::INCOMPLETE,
                "Missing from a set that will not be fixed (complete sets only).",
            ),
            (
                RepStatus::DELETE,
                "Not needed here and a copy is elsewhere: Fix deletes it.",
            ),
        ],
    ),
    (
        "Problem statuses",
        &[
            (RepStatus::CORRUPT, "The file or archive is damaged."),
            (
                RepStatus::UN_SCANNED,
                "The file could not be read (locked, or no permission).",
            ),
        ],
    ),
];

pub fn show(parent: &gtk4::Window) {
    let window = gtk4::Window::builder()
        .title("Key to ROM statuses")
        .transient_for(parent)
        .modal(false)
        .default_width(640)
        .default_height(800)
        .build();
    let page = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
    page.set_margin_start(12);
    page.set_margin_end(12);
    page.set_margin_top(8);
    page.set_margin_bottom(12);
    for (group, rows) in KEY {
        let title = gtk4::Label::new(None);
        title.set_markup(&format!("<b>{group}</b>"));
        title.set_xalign(0.0);
        title.set_margin_top(8);
        page.append(&title);
        for (status, text) in *rows {
            let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 10);
            let chip = Painted::new(30, 22);
            let s = *status;
            chip.set_painter(Box::new(move |cr, _, _| {
                draw_chip(cr, 2.0, 2.0, 26.0, 18.0, s)
            }));
            let name = gtk4::Label::new(Some(status.name()));
            name.set_xalign(0.0);
            name.set_width_chars(20);
            name.add_css_class("rv-key-chip");
            name.add_css_class(&css_class(*status));
            let what = gtk4::Label::new(Some(text));
            what.set_xalign(0.0);
            what.set_wrap(true);
            what.set_hexpand(true);
            row.append(&chip);
            row.append(&name);
            row.append(&what);
            page.append(&row);
        }
    }
    let scroll = gtk4::ScrolledWindow::new();
    scroll.set_child(Some(&page));
    window.set_child(Some(&scroll));
    window.present();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::colours::status_colour;

    #[test]
    fn every_status_a_user_can_see_has_an_entry_and_a_colour() {
        let listed: Vec<RepStatus> = KEY
            .iter()
            .flat_map(|(_, rows)| rows.iter().map(|r| r.0))
            .collect();
        for s in [
            RepStatus::CORRECT,
            RepStatus::MISSING,
            RepStatus::UNKNOWN,
            RepStatus::CAN_BE_FIXED,
            RepStatus::CORRUPT,
            RepStatus::UN_SCANNED,
            RepStatus::DELETE,
        ] {
            assert!(listed.contains(&s), "{} is not in the key", s.name());
        }
        for s in &listed {
            assert!(status_colour(*s).is_some(), "{} has no colour", s.name());
        }
    }
}
