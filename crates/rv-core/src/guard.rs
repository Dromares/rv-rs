//! A refusal to work on paths the maintainer has fenced off.
//!
//! The maintainer's real ROM collection lives under [`PROTECTED_PREFIXES`]. While the tools are
//! young, a front end must not read a cache from there, scan there, or write there by accident
//! (a settings file copied from the real installation points at the real directories). A front
//! end calls [`check_settings`] before it loads a cache or starts an operation and refuses when
//! the answer is not empty, unless the user has switched the guard off on purpose.
//!
//! Paths are compared after resolving them: relative paths are taken from `base`, `.` and `..`
//! are folded, and the longest part of the path that exists is canonicalised, so a symlink into a
//! protected directory is caught as well as a path that spells the prefix out. A path that does
//! not exist yet is checked through the part of it that does.

use std::path::{Component, Path, PathBuf};

use crate::Settings;

/// Directories that are refused unless the guard is switched off. Whole components are compared:
/// `/mnt/HHD16TB-other` is not protected, `/mnt/HHD16TB/x` is.
pub const PROTECTED_PREFIXES: &[&str] = &["/mnt/HHD16TB"];

/// One setting that points into a protected directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Violation {
    /// Which setting: `"DatRoot"`, `"CacheFile"` or `"DirMapping RomVault/Console/NES"`.
    pub setting: String,
    /// The path as written in the settings.
    pub path: String,
    /// The protected prefix it falls under.
    pub prefix: &'static str,
}

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} ({}) is under the protected directory {}",
            self.setting, self.path, self.prefix
        )
    }
}

/// Fold `.` and `..` without touching the file system. `..` at the root stays at the root.
fn fold(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The absolute, symlink-free form of `path` as far as the file system can say: the longest
/// existing ancestor is canonicalised and the rest is appended as written.
fn resolve(path: &Path, base: &Path) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    let folded = fold(&joined);
    let mut existing = folded.as_path();
    let mut rest: Vec<&std::ffi::OsStr> = Vec::new();
    loop {
        if let Ok(real) = existing.canonicalize() {
            let mut out = real;
            for part in rest.iter().rev() {
                out.push(part);
            }
            return out;
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name);
                existing = parent;
            }
            _ => return folded,
        }
    }
}

/// The protected prefix that `path` (relative to `base`) falls under, if any.
pub fn protected_prefix(path: &Path, base: &Path) -> Option<&'static str> {
    let resolved = resolve(path, base);
    PROTECTED_PREFIXES.iter().copied().find(|prefix| {
        let prefix_path = Path::new(prefix);
        // Compare both the spelling and the resolved form of the prefix, so a `/mnt` that is
        // itself a symlink does not open a gap.
        resolved.starts_with(prefix_path) || resolved.starts_with(resolve(prefix_path, base))
    })
}

/// Every path in `settings` that falls under a protected prefix: the DAT root, the cache file and
/// the ROM directory mappings (which include ToSort). `base` is the directory the relative paths
/// in the settings are taken from.
pub fn check_settings(settings: &Settings, base: &Path) -> Vec<Violation> {
    let mut found = Vec::new();
    let mut test = |setting: String, path: &str| {
        if path.trim().is_empty() {
            return;
        }
        if let Some(prefix) = protected_prefix(Path::new(path), base) {
            found.push(Violation {
                setting,
                path: path.to_owned(),
                prefix,
            });
        }
    };
    test("DatRoot".to_owned(), &settings.dat_root);
    test("CacheFile".to_owned(), &settings.cache_file);
    for m in &settings.dir_mappings {
        test(format!("DirMapping {}", m.dir_key), &m.dir_path);
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DirMapping;

    fn base() -> PathBuf {
        std::env::temp_dir()
    }

    #[test]
    fn the_prefix_and_everything_below_it_is_protected() {
        for p in [
            "/mnt/HHD16TB",
            "/mnt/HHD16TB/",
            "/mnt/HHD16TB/RomVault/NES",
            "/mnt/HHD16TB/does/not/exist.Cache",
        ] {
            assert_eq!(
                protected_prefix(Path::new(p), &base()),
                Some("/mnt/HHD16TB"),
                "{p}"
            );
        }
    }

    #[test]
    fn a_sibling_with_the_same_leading_letters_is_not() {
        for p in [
            "/mnt/HHD16TB-copy/x",
            "/mnt/HHD16TBx",
            "/mnt",
            "/tmp/HHD16TB",
        ] {
            assert_eq!(protected_prefix(Path::new(p), &base()), None, "{p}");
        }
    }

    #[test]
    fn dot_dot_cannot_walk_into_it() {
        assert!(protected_prefix(Path::new("/tmp/../mnt/HHD16TB/x"), &base()).is_some());
        assert!(protected_prefix(Path::new("/mnt/HHD16TB/a/../b"), &base()).is_some());
        // A relative path is taken from the base.
        assert!(protected_prefix(Path::new("x/y"), Path::new("/mnt/HHD16TB")).is_some());
        assert!(protected_prefix(Path::new("../HHD16TB/y"), Path::new("/mnt/other")).is_some());
        assert!(protected_prefix(Path::new("../../x"), Path::new("/mnt/HHD16TB/a")).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_into_it_is_caught_and_an_unrelated_one_is_not() {
        // Only meaningful where the protected directory exists, and the test must not create it.
        if !Path::new("/mnt/HHD16TB").exists() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("rv-guard-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::os::unix::fs::symlink("/mnt/HHD16TB", dir.join("into")).unwrap();
        std::os::unix::fs::symlink("/tmp", dir.join("away")).unwrap();
        let hit = protected_prefix(&dir.join("into/new.Cache"), &base());
        let miss = protected_prefix(&dir.join("away/new.Cache"), &base());
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(hit, Some("/mnt/HHD16TB"));
        assert_eq!(miss, None);
    }

    #[test]
    fn settings_are_checked_field_by_field() {
        let mut s = Settings::default();
        assert!(check_settings(&s, &base()).is_empty());

        s.dat_root = "/mnt/HHD16TB/DatRoot".to_owned();
        s.cache_file = "/mnt/HHD16TB/RomVault3_3.Cache".to_owned();
        s.dir_mappings.push(DirMapping {
            dir_key: "RomVault/Console/NES".to_owned(),
            dir_path: "/mnt/HHD16TB/RomRoot/NES".to_owned(),
        });
        s.dir_mappings.push(DirMapping {
            dir_key: "ToSort".to_owned(),
            dir_path: "/home/x/ToSort".to_owned(),
        });
        let found: Vec<String> = check_settings(&s, &base())
            .into_iter()
            .map(|v| v.setting)
            .collect();
        assert_eq!(
            found,
            ["DatRoot", "CacheFile", "DirMapping RomVault/Console/NES"]
        );
    }

    #[test]
    fn the_message_names_the_setting_and_the_prefix() {
        let v = Violation {
            setting: "DatRoot".to_owned(),
            path: "/mnt/HHD16TB/x".to_owned(),
            prefix: "/mnt/HHD16TB",
        };
        assert_eq!(
            v.to_string(),
            "DatRoot (/mnt/HHD16TB/x) is under the protected directory /mnt/HHD16TB"
        );
    }
}
