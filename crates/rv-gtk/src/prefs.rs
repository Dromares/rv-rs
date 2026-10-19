//! The interface's own preferences: window size, the game list's tick boxes, the scan level of
//! the Scan button and the switch that lifts the path guard. They live in a small `key=value`
//! file of their own, not in `RomVault3cfg.xml`, which stays ROMVault's.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rv_core::ScanLevel;

use crate::model::GameFilter;

/// Everything the window remembers between runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prefs {
    pub width: i32,
    pub height: i32,
    /// Position of the divider between the tree and the lists, and between the game and ROM list.
    pub tree_width: i32,
    pub games_height: i32,
    pub scan_level: ScanLevel,
    pub filter: GameFilter,
    /// Lifts the guard in `rv_core::guard`. Off unless the user turns it on in the settings.
    pub allow_protected_paths: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            width: 1280,
            height: 800,
            tree_width: 560,
            games_height: 340,
            scan_level: ScanLevel::Level2,
            filter: GameFilter::default(),
            allow_protected_paths: false,
        }
    }
}

fn level_number(level: ScanLevel) -> u8 {
    match level {
        ScanLevel::Level1 => 1,
        ScanLevel::Level2 => 2,
        ScanLevel::Level3 => 3,
    }
}

impl Prefs {
    /// The text of the file.
    pub fn to_text(&self) -> String {
        let f = &self.filter;
        let b = |v: bool| if v { "1" } else { "0" };
        format!(
            "width={}\nheight={}\ntree_width={}\ngames_height={}\nscan_level={}\nshow_complete={}\nshow_partial={}\nshow_empty={}\nshow_fixes={}\nshow_mia={}\nshow_merged={}\nallow_protected_paths={}\n",
            self.width,
            self.height,
            self.tree_width,
            self.games_height,
            level_number(self.scan_level),
            b(f.complete),
            b(f.partial),
            b(f.empty),
            b(f.fixes),
            b(f.mia),
            b(f.merged),
            b(self.allow_protected_paths),
        )
    }

    /// Parse the text of a file. Unknown keys and unreadable values are ignored: the default stays.
    pub fn from_text(text: &str) -> Prefs {
        let map: BTreeMap<&str, &str> = text
            .lines()
            .filter_map(|l| l.split_once('='))
            .map(|(k, v)| (k.trim(), v.trim()))
            .collect();
        let mut p = Prefs::default();
        let int = |k: &str, min: i32, max: i32| {
            map.get(k)
                .and_then(|v| v.parse::<i32>().ok())
                .filter(|v| (min..=max).contains(v))
        };
        let flag = |k: &str| {
            map.get(k)
                .and_then(|v| v.parse::<u8>().ok())
                .map(|v| v != 0)
        };
        p.width = int("width", 400, 10_000).unwrap_or(p.width);
        p.height = int("height", 300, 10_000).unwrap_or(p.height);
        p.tree_width = int("tree_width", 100, 5_000).unwrap_or(p.tree_width);
        p.games_height = int("games_height", 60, 5_000).unwrap_or(p.games_height);
        p.scan_level = match int("scan_level", 1, 3) {
            Some(1) => ScanLevel::Level1,
            Some(3) => ScanLevel::Level3,
            _ => p.scan_level,
        };
        p.filter.complete = flag("show_complete").unwrap_or(p.filter.complete);
        p.filter.partial = flag("show_partial").unwrap_or(p.filter.partial);
        p.filter.empty = flag("show_empty").unwrap_or(p.filter.empty);
        p.filter.fixes = flag("show_fixes").unwrap_or(p.filter.fixes);
        p.filter.mia = flag("show_mia").unwrap_or(p.filter.mia);
        p.filter.merged = flag("show_merged").unwrap_or(p.filter.merged);
        p.allow_protected_paths = flag("allow_protected_paths").unwrap_or(false);
        p
    }

    /// Read a file; a missing or unreadable one gives the defaults.
    pub fn load(path: &Path) -> Prefs {
        std::fs::read_to_string(path)
            .map(|t| Prefs::from_text(&t))
            .unwrap_or_default()
    }

    /// Write the file, making its directory first. Errors are returned for the caller to log;
    /// losing a preference is not worth stopping for.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, self.to_text())
    }
}

/// `$XDG_CONFIG_HOME/rv-rs/rv-gtk.conf`, or `~/.config/rv-rs/rv-gtk.conf`.
pub fn default_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("rv-rs").join("rv-gtk.conf")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_round_trip_keeps_everything() {
        let mut p = Prefs {
            width: 900,
            scan_level: ScanLevel::Level3,
            ..Prefs::default()
        };
        p.filter.merged = true;
        p.filter.mia = false;
        p.allow_protected_paths = true;
        assert_eq!(Prefs::from_text(&p.to_text()), p);
    }

    #[test]
    fn junk_leaves_the_defaults_and_the_guard_stays_on() {
        let p = Prefs::from_text(
            "width=abc\nheight=99999999\nscan_level=7\nallow_protected_paths=yes\nnoise\n=\n",
        );
        assert_eq!(p, Prefs::default());
        assert!(!p.allow_protected_paths);
        assert_eq!(Prefs::from_text(""), Prefs::default());
    }

    #[test]
    fn the_guard_is_off_by_default() {
        assert!(!Prefs::default().allow_protected_paths);
        assert!(Prefs::default()
            .to_text()
            .contains("allow_protected_paths=0"));
    }
}
