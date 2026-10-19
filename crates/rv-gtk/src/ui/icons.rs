//! The small pictures: a folder per tree row, a status chip per ROM, a strip of counts per game.
//!
//! ROMVault draws these from bitmaps in its resources (`R_<DatStatus>_<RepStatus>` for the ROM
//! list, `G_<RepStatus>` for the game list, `Tree1..5` for folders; `docs/scanning-and-findfixes.md`
//! section 1.6). Those pictures are not part of this repository, so these are drawn with cairo:
//! the same inputs choose the picture (the status, the DAT status, the folder state), the colour
//! of a status is ROMVault's ([`crate::colours`]) and the symbol says what kind of status it is.
//! Drawing needs no display, so it is tested on an image surface.

use gtk4::cairo::{Context, FontSlant, FontWeight};
use rv_core::{DatStatus, DirStatus, RepStatus};

use crate::colours::{contrasty, status_colour, Rgb, DISPLAY_ORDER};
use crate::model::{FolderKind, TreeIcon};

/// What a status chip shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Symbol {
    Check,
    Cross,
    Bang,
    Question,
    Arrow,
    Minus,
    Dot,
}

/// The symbol of a status. Fix statuses are arrows, the good ones ticks, the bad ones crosses.
pub fn symbol(status: RepStatus) -> Symbol {
    match status {
        RepStatus::CORRECT | RepStatus::DIR_CORRECT | RepStatus::CORRECT_MIA => Symbol::Check,
        RepStatus::MISSING
        | RepStatus::DIR_MISSING
        | RepStatus::MISSING_MIA
        | RepStatus::MISSING_NEW
        | RepStatus::INCOMPLETE
        | RepStatus::INCOMPLETE_REMOVE
        | RepStatus::DELETE => Symbol::Cross,
        RepStatus::CORRUPT | RepStatus::DIR_CORRUPT | RepStatus::UN_SCANNED => Symbol::Bang,
        RepStatus::UNKNOWN | RepStatus::DIR_UNKNOWN => Symbol::Question,
        RepStatus::CAN_BE_FIXED
        | RepStatus::CAN_BE_FIXED_MIA
        | RepStatus::CORRUPT_CAN_BE_FIXED
        | RepStatus::NEEDED_FOR_FIX
        | RepStatus::MOVE_TO_SORT
        | RepStatus::MOVE_TO_CORRUPT
        | RepStatus::RENAME => Symbol::Arrow,
        RepStatus::NOT_COLLECTED | RepStatus::UN_NEEDED | RepStatus::IGNORE | RepStatus::DELETED => {
            Symbol::Minus
        }
        _ => Symbol::Dot,
    }
}

fn set(cr: &Context, c: Rgb) {
    let (r, g, b) = c.unit();
    cr.set_source_rgb(r, g, b);
}

fn rounded(cr: &Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    let r = r.min(w / 2.0).min(h / 2.0);
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -std::f64::consts::FRAC_PI_2, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, std::f64::consts::FRAC_PI_2);
    cr.arc(x + r, y + h - r, r, std::f64::consts::FRAC_PI_2, std::f64::consts::PI);
    cr.arc(x + r, y + r, r, std::f64::consts::PI, 3.0 * std::f64::consts::FRAC_PI_2);
    cr.close_path();
}

fn draw_symbol(cr: &Context, sym: Symbol, x: f64, y: f64, w: f64, h: f64, ink: Rgb) {
    set(cr, ink);
    cr.set_line_width(1.8);
    cr.set_line_cap(gtk4::cairo::LineCap::Round);
    cr.set_line_join(gtk4::cairo::LineJoin::Round);
    let (cx, cy) = (x + w / 2.0, y + h / 2.0);
    let s = h.min(w) * 0.28;
    match sym {
        Symbol::Check => {
            cr.move_to(cx - s, cy);
            cr.line_to(cx - s * 0.3, cy + s * 0.8);
            cr.line_to(cx + s, cy - s * 0.8);
            let _ = cr.stroke();
        }
        Symbol::Cross => {
            cr.move_to(cx - s, cy - s);
            cr.line_to(cx + s, cy + s);
            cr.move_to(cx + s, cy - s);
            cr.line_to(cx - s, cy + s);
            let _ = cr.stroke();
        }
        Symbol::Arrow => {
            cr.move_to(cx - s, cy);
            cr.line_to(cx + s, cy);
            cr.move_to(cx + s * 0.1, cy - s * 0.9);
            cr.line_to(cx + s, cy);
            cr.line_to(cx + s * 0.1, cy + s * 0.9);
            let _ = cr.stroke();
        }
        Symbol::Minus => {
            cr.move_to(cx - s, cy);
            cr.line_to(cx + s, cy);
            let _ = cr.stroke();
        }
        Symbol::Dot => {
            cr.arc(cx, cy, s * 0.4, 0.0, std::f64::consts::TAU);
            let _ = cr.fill();
        }
        Symbol::Bang | Symbol::Question => {
            cr.select_font_face("Sans", FontSlant::Normal, FontWeight::Bold);
            cr.set_font_size(h * 0.78);
            let text = if sym == Symbol::Bang { "!" } else { "?" };
            if let Ok(e) = cr.text_extents(text) {
                cr.move_to(cx - e.width() / 2.0 - e.x_bearing(), cy - e.height() / 2.0 - e.y_bearing());
                let _ = cr.show_text(text);
            }
        }
    }
}

/// A rounded square in the colour of `status` with its symbol.
pub fn draw_chip(cr: &Context, x: f64, y: f64, w: f64, h: f64, status: RepStatus) {
    let bg = status_colour(status).unwrap_or(Rgb(235, 235, 235));
    rounded(cr, x + 0.5, y + 0.5, w - 1.0, h - 1.0, 3.0);
    set(cr, bg);
    let _ = cr.fill_preserve();
    cr.set_line_width(1.0);
    cr.set_source_rgba(0.0, 0.0, 0.0, 0.35);
    let _ = cr.stroke();
    // Bright red and brown are dark enough to want a white symbol.
    let ink = contrasty(bg);
    draw_symbol(cr, symbol(status), x, y, w, h, if ink == Rgb::WHITE { ink } else { Rgb(40, 40, 40) });
}

fn dat_tag(status: DatStatus) -> (&'static str, Rgb) {
    match status {
        DatStatus::IN_DAT_COLLECT => ("D", Rgb(190, 205, 235)),
        DatStatus::IN_DAT_MERGED => ("M", Rgb(205, 205, 205)),
        DatStatus::IN_TO_SORT => ("T", Rgb(255, 214, 255)),
        _ => ("-", Rgb(240, 240, 240)),
    }
}

fn text_in(cr: &Context, text: &str, x: f64, y: f64, w: f64, h: f64, size: f64) {
    cr.select_font_face("Sans", FontSlant::Normal, FontWeight::Bold);
    cr.set_font_size(size);
    if let Ok(e) = cr.text_extents(text) {
        cr.move_to(x + (w - e.width()) / 2.0 - e.x_bearing(), y + (h - e.height()) / 2.0 - e.y_bearing());
        let _ = cr.show_text(text);
    }
}

/// The picture of the ROM list's first column: where the DAT puts the ROM, then its status. A
/// 54 by 18 picture, the size ROMVault uses.
pub const GOT_SIZE: (i32, i32) = (54, 18);

pub fn draw_got(cr: &Context, dat: DatStatus, status: RepStatus, mia_suffix: &str) {
    let (w, h) = (f64::from(GOT_SIZE.0), f64::from(GOT_SIZE.1));
    let (tag, tag_bg) = dat_tag(dat);
    rounded(cr, 0.5, 0.5, 17.0, h - 1.0, 3.0);
    set(cr, tag_bg);
    let _ = cr.fill_preserve();
    cr.set_line_width(1.0);
    cr.set_source_rgba(0.0, 0.0, 0.0, 0.35);
    let _ = cr.stroke();
    set(cr, Rgb(40, 40, 40));
    text_in(cr, tag, 0.0, 0.0, 18.0, h, 11.0);
    draw_chip(cr, 22.0, 0.0, 32.0, h, status);
    // MIA and new ROMs get a mark in the corner of the chip.
    if !mia_suffix.is_empty() {
        set(cr, Rgb(60, 60, 60));
        text_in(cr, if mia_suffix == "_MIA" { "M" } else { "N" }, 22.0, 0.0, 10.0, 11.0, 8.0);
    }
    let _ = w;
}

/// How wide the strip of a game's counts is.
pub fn strip_width(status: &DirStatus) -> i32 {
    let mut w = 4;
    for s in DISPLAY_ORDER {
        let n = status.get(s);
        if n > 0 {
            w += 24 + 7 * n.to_string().len() as i32 + 6;
        }
    }
    w
}

/// A chip and a count for every status that the game has any of, in ROMVault's order.
pub fn draw_strip(cr: &Context, h: f64, status: &DirStatus) {
    let mut x = 2.0;
    for s in DISPLAY_ORDER {
        let n = status.get(s);
        if n <= 0 {
            continue;
        }
        let ch = (h - 2.0).min(16.0);
        let y = (h - ch) / 2.0;
        draw_chip(cr, x, y, 22.0, ch, s);
        let text = n.to_string();
        set(cr, Rgb(30, 30, 30));
        cr.select_font_face("Sans", FontSlant::Normal, FontWeight::Normal);
        cr.set_font_size(11.0);
        if let Ok(e) = cr.text_extents(&text) {
            cr.move_to(x + 25.0, y + (ch - e.height()) / 2.0 - e.y_bearing());
            let _ = cr.show_text(&text);
        }
        x += 24.0 + 7.0 * text.len() as f64 + 6.0;
    }
}

/// The folder in front of a tree row: its colour says how much of the set is there.
pub fn folder_colour(icon: TreeIcon) -> Rgb {
    match icon {
        TreeIcon::Missing => Rgb(222, 96, 96),
        TreeIcon::Partial => Rgb(236, 186, 72),
        TreeIcon::Complete => Rgb(88, 176, 88),
        TreeIcon::ToSort => Rgb(196, 100, 196),
        TreeIcon::Mia => Rgb(110, 224, 110),
    }
}

pub const FOLDER_SIZE: i32 = 16;

pub fn draw_folder(cr: &Context, icon: TreeIcon, kind: FolderKind) {
    let c = folder_colour(icon);
    // The tab, then the body.
    rounded(cr, 1.0, 2.0, 6.5, 4.0, 1.5);
    set(cr, c);
    let _ = cr.fill();
    rounded(cr, 0.5, 4.5, 15.0, 10.0, 2.0);
    set(cr, c);
    let _ = cr.fill_preserve();
    cr.set_line_width(1.0);
    cr.set_source_rgba(0.0, 0.0, 0.0, 0.4);
    let _ = cr.stroke();
    if kind == FolderKind::Dat {
        // A page with a folded corner: this directory is made by a DAT.
        set(cr, Rgb(255, 255, 255));
        cr.rectangle(5.0, 6.0, 6.0, 7.0);
        let _ = cr.fill_preserve();
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.45);
        cr.set_line_width(0.8);
        let _ = cr.stroke();
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.45);
        cr.move_to(6.5, 8.5);
        cr.line_to(9.5, 8.5);
        cr.move_to(6.5, 10.5);
        cr.line_to(9.5, 10.5);
        let _ = cr.stroke();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gtk4::cairo::{Format, ImageSurface};

    fn inked(w: i32, h: i32, draw: impl Fn(&Context)) -> usize {
        let surface = ImageSurface::create(Format::ARgb32, w, h).unwrap();
        let cr = Context::new(&surface).unwrap();
        draw(&cr);
        drop(cr);
        let mut surface = surface;
        let stride = surface.stride() as usize;
        let data = surface.data().unwrap();
        (0..h as usize)
            .flat_map(|y| (0..w as usize).map(move |x| (x, y)))
            .filter(|(x, y)| data[y * stride + x * 4 + 3] != 0)
            .count()
    }

    #[test]
    fn every_status_has_a_chip_that_draws() {
        for n in 0..RepStatus::END_VALUE as u8 {
            let s = RepStatus(n);
            assert!(inked(24, 18, |cr| draw_chip(cr, 0.0, 0.0, 22.0, 16.0, s)) > 100, "{}", s.name());
        }
    }

    #[test]
    fn symbols_say_what_kind_of_status_it_is() {
        assert_eq!(symbol(RepStatus::CORRECT), Symbol::Check);
        assert_eq!(symbol(RepStatus::MISSING), Symbol::Cross);
        assert_eq!(symbol(RepStatus::CORRUPT), Symbol::Bang);
        assert_eq!(symbol(RepStatus::UNKNOWN), Symbol::Question);
        assert_eq!(symbol(RepStatus::CAN_BE_FIXED), Symbol::Arrow);
        assert_eq!(symbol(RepStatus::NOT_COLLECTED), Symbol::Minus);
    }

    #[test]
    fn folders_differ_by_state_and_kind() {
        let area = |icon, kind| inked(FOLDER_SIZE, FOLDER_SIZE, |cr| draw_folder(cr, icon, kind));
        assert!(area(TreeIcon::Complete, FolderKind::Directory) > 80);
        assert!(area(TreeIcon::Complete, FolderKind::Dat) > 80);
        let colours: std::collections::HashSet<_> = [
            TreeIcon::Missing,
            TreeIcon::Partial,
            TreeIcon::Complete,
            TreeIcon::ToSort,
            TreeIcon::Mia,
        ]
        .map(|i| format!("{:?}", folder_colour(i)))
        .into_iter()
        .collect();
        assert_eq!(colours.len(), 5);
    }

    #[test]
    fn the_strip_is_as_wide_as_the_counts_need() {
        let mut s = DirStatus::default();
        assert_eq!(strip_width(&s), 4);
        s.add(RepStatus::CORRECT, rv_core::MiaStatus::NONE, true);
        s.add(RepStatus::MISSING, rv_core::MiaStatus::NONE, true);
        assert_eq!(strip_width(&s), 4 + 2 * (24 + 7 + 6));
        let w = strip_width(&s);
        assert!(inked(w, 20, |cr| draw_strip(cr, 20.0, &s)) > 200);
    }

    #[test]
    fn the_got_picture_draws_for_each_dat_status() {
        for dat in [
            DatStatus::IN_DAT_COLLECT,
            DatStatus::IN_DAT_MERGED,
            DatStatus::IN_TO_SORT,
            DatStatus::NOT_IN_DAT,
        ] {
            assert!(inked(GOT_SIZE.0, GOT_SIZE.1, |cr| draw_got(cr, dat, RepStatus::CORRECT, "")) > 300);
        }
    }
}
