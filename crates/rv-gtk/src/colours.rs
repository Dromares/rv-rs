//! Status colours: the table ROMVault paints its grids with (`ROMVault/FrmMain.cs:29-46,137-174`,
//! `docs/scanning-and-findfixes.md` section 1.3), as plain RGB values and the CSS that uses them.
//!
//! A status ROMVault gives no colour (`Error`, `UnSet`, `DirUnknown`, `DirInToSort`,
//! `IncompleteRemove`) has none here either; the widget keeps the theme background.

use rv_core::RepStatus;

/// An RGB colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub const BLACK: Rgb = Rgb(0, 0, 0);
    pub const WHITE: Rgb = Rgb(255, 255, 255);

    /// `0.0..=1.0` components, for cairo.
    pub fn unit(self) -> (f64, f64, f64) {
        (
            f64::from(self.0) / 255.0,
            f64::from(self.1) / 255.0,
            f64::from(self.2) / 255.0,
        )
    }

    /// `rgb(r, g, b)` for CSS.
    pub fn css(self) -> String {
        format!("rgb({}, {}, {})", self.0, self.1, self.2)
    }
}

const BLUE: Rgb = Rgb(214, 214, 255);
const GREY_BLUE: Rgb = Rgb(214, 224, 255);
const RED: Rgb = Rgb(255, 214, 214);
const BRIGHT_RED: Rgb = Rgb(255, 0, 0);
const GREEN: Rgb = Rgb(214, 255, 214);
const NEON_GREEN: Rgb = Rgb(100, 255, 100);
const LIGHT_RED: Rgb = Rgb(255, 235, 235);
const SOFT_GREEN: Rgb = Rgb(150, 200, 150);
const GREY: Rgb = Rgb(214, 214, 214);
const CYAN: Rgb = Rgb(214, 255, 255);
const CYAN_GREY: Rgb = Rgb(214, 225, 225);
const MAGENTA: Rgb = Rgb(255, 214, 255);
const BROWN: Rgb = Rgb(140, 80, 80);
const PURPLE: Rgb = Rgb(214, 140, 214);
const YELLOW: Rgb = Rgb(255, 255, 214);
const DARK_YELLOW: Rgb = Rgb(255, 255, 100);
const ORANGE: Rgb = Rgb(255, 214, 140);
const WHITE: Rgb = Rgb(255, 255, 255);

/// The background colour of a status, `None` where ROMVault sets none.
pub fn status_colour(status: RepStatus) -> Option<Rgb> {
    Some(match status {
        RepStatus::UN_SCANNED => BLUE,
        RepStatus::DIR_CORRECT => GREEN,
        RepStatus::DIR_MISSING => RED,
        RepStatus::DIR_CORRUPT => BRIGHT_RED,
        RepStatus::MISSING => RED,
        RepStatus::MISSING_NEW => SOFT_GREEN,
        RepStatus::CORRECT => GREEN,
        RepStatus::CORRECT_MIA => NEON_GREEN,
        RepStatus::NOT_COLLECTED => GREY,
        RepStatus::UN_NEEDED => CYAN_GREY,
        RepStatus::UNKNOWN => CYAN,
        RepStatus::IN_TO_SORT => MAGENTA,
        RepStatus::MISSING_MIA => SOFT_GREEN,
        RepStatus::CORRUPT => BRIGHT_RED,
        RepStatus::IGNORE => GREY_BLUE,
        RepStatus::CAN_BE_FIXED => YELLOW,
        RepStatus::CAN_BE_FIXED_MIA => DARK_YELLOW,
        RepStatus::MOVE_TO_SORT => PURPLE,
        RepStatus::DELETE => BROWN,
        RepStatus::NEEDED_FOR_FIX => ORANGE,
        RepStatus::RENAME => ORANGE,
        RepStatus::CORRUPT_CAN_BE_FIXED => YELLOW,
        RepStatus::MOVE_TO_CORRUPT => PURPLE,
        RepStatus::INCOMPLETE => LIGHT_RED,
        RepStatus::DELETED => WHITE,
        _ => return None,
    })
}

/// `FrmMain.Contrasty`: white text on a dark colour, black on a light one.
pub fn contrasty(c: Rgb) -> Rgb {
    let weight = (u32::from(c.0) << 1) + u32::from(c.2) + u32::from(c.1) + (u32::from(c.1) << 2);
    if weight < 1024 {
        Rgb::WHITE
    } else {
        Rgb::BLACK
    }
}

/// The left-to-right order of the status cells in the game grid (`RepairStatus.DisplayOrder`,
/// `RepairStatus.cs:181-213`).
pub const DISPLAY_ORDER: [RepStatus; 25] = [
    RepStatus::ERROR,
    RepStatus::UN_SET,
    RepStatus::UN_SCANNED,
    RepStatus::DIR_CORRUPT,
    RepStatus::MOVE_TO_CORRUPT,
    RepStatus::CORRUPT_CAN_BE_FIXED,
    RepStatus::CAN_BE_FIXED_MIA,
    RepStatus::CAN_BE_FIXED,
    RepStatus::MOVE_TO_SORT,
    RepStatus::DELETE,
    RepStatus::NEEDED_FOR_FIX,
    RepStatus::RENAME,
    RepStatus::CORRUPT,
    RepStatus::UNKNOWN,
    RepStatus::UN_NEEDED,
    RepStatus::INCOMPLETE,
    RepStatus::MISSING,
    RepStatus::MISSING_NEW,
    RepStatus::MISSING_MIA,
    RepStatus::CORRECT_MIA,
    RepStatus::CORRECT,
    RepStatus::IN_TO_SORT,
    RepStatus::NOT_COLLECTED,
    RepStatus::IGNORE,
    RepStatus::DELETED,
];

/// The CSS class that paints a widget with the colour of `status`.
pub fn css_class(status: RepStatus) -> String {
    format!("rv-st-{}", status.0)
}

/// One rule per coloured status: `.rv-st-9 { background-color: ...; color: ...; }`. The text
/// colour is the contrasting one, so Delete (brown) and BrightRed rows stay readable.
pub fn css() -> String {
    let mut out = String::new();
    for n in 0..RepStatus::END_VALUE as u8 {
        let status = RepStatus(n);
        if let Some(bg) = status_colour(status) {
            out.push_str(&format!(
                ".{} {{ background-color: {}; color: {}; }}\n",
                css_class(status),
                bg.css(),
                contrasty(bg).css()
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_romvaults() {
        assert_eq!(status_colour(RepStatus::CORRECT), Some(Rgb(214, 255, 214)));
        assert_eq!(status_colour(RepStatus::MISSING), Some(Rgb(255, 214, 214)));
        assert_eq!(status_colour(RepStatus::UN_SCANNED), Some(Rgb(214, 214, 255)));
        assert_eq!(status_colour(RepStatus::CORRUPT), Some(Rgb(255, 0, 0)));
        assert_eq!(status_colour(RepStatus::DELETE), Some(Rgb(140, 80, 80)));
        assert_eq!(status_colour(RepStatus::CORRECT_MIA), Some(Rgb(100, 255, 100)));
        assert_eq!(status_colour(RepStatus::INCOMPLETE), Some(Rgb(255, 235, 235)));
    }

    #[test]
    fn statuses_without_a_colour_have_none() {
        for s in [
            RepStatus::ERROR,
            RepStatus::UN_SET,
            RepStatus::DIR_UNKNOWN,
            RepStatus::DIR_IN_TO_SORT,
            RepStatus::INCOMPLETE_REMOVE,
        ] {
            assert_eq!(status_colour(s), None, "{}", s.name());
        }
        // Everything else is coloured: 30 values minus the five above and the sentinel gap.
        let coloured = (0..RepStatus::END_VALUE as u8)
            .filter(|n| status_colour(RepStatus(*n)).is_some())
            .count();
        assert_eq!(coloured, 25);
    }

    #[test]
    fn contrasty_matches_the_formula() {
        assert_eq!(contrasty(Rgb(140, 80, 80)), Rgb::WHITE); // brown
        assert_eq!(contrasty(Rgb(255, 0, 0)), Rgb::WHITE); // bright red
        assert_eq!(contrasty(Rgb(214, 255, 214)), Rgb::BLACK); // green
        assert_eq!(contrasty(Rgb(214, 140, 214)), Rgb::BLACK); // purple
    }

    #[test]
    fn display_order_is_every_status_the_grid_can_show_once() {
        let mut seen = std::collections::HashSet::new();
        for s in DISPLAY_ORDER {
            assert!(seen.insert(s), "{} twice", s.name());
        }
        assert_eq!(DISPLAY_ORDER.first(), Some(&RepStatus::ERROR));
        assert_eq!(DISPLAY_ORDER.last(), Some(&RepStatus::DELETED));
    }

    #[test]
    fn css_has_a_rule_per_coloured_status() {
        let css = css();
        assert_eq!(css.lines().count(), 25);
        assert!(css.contains(".rv-st-9 { background-color: rgb(214, 255, 214); color: rgb(0, 0, 0); }"));
        assert!(css.contains(".rv-st-18 { background-color: rgb(140, 80, 80); color: rgb(255, 255, 255); }"));
    }
}
