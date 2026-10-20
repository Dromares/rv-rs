//! The live library is never touched by accident.
//!
//! Every path a run is configured with is resolved and compared with [`LIVE_LIBRARY`]; one that
//! is inside it stops the run before anything is read or written, unless `--allow-live-library`
//! was given. The first comparison is on the *text* of the path, so a path that names the live
//! library is refused without a single file system call on it.

use std::path::{Component, Path, PathBuf};

/// The board's library. Nothing under it is read or written without `--allow-live-library`.
pub const LIVE_LIBRARY: &str = "/mnt/HHD16TB";

/// `path` made absolute against the current directory, with `.` and `..` resolved by text.
pub fn lexical_absolute(path: &Path) -> PathBuf {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("/"))
            .join(path)
    };
    let mut out = PathBuf::new();
    for c in abs.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn under(path: &Path, live: &Path) -> bool {
    let lower = |p: &Path| {
        p.components()
            .map(|c| c.as_os_str().to_string_lossy().to_ascii_lowercase())
            .collect::<Vec<_>>()
    };
    let (p, l) = (lower(path), lower(live));
    p.len() >= l.len() && p[..l.len()] == l[..]
}

/// Does `path` resolve to somewhere under the live library? By text first. Only if that says no
/// is the longest existing part of the path resolved through symlinks (a link in a harmless
/// place may lead there); a path that names the library never gets that far.
pub fn resolves_under_live(path: &Path) -> bool {
    resolves_under(path, Path::new(LIVE_LIBRARY))
}

/// [`resolves_under_live`] for any `live` directory.
pub fn resolves_under(path: &Path, live: &Path) -> bool {
    let abs = lexical_absolute(path);
    if under(&abs, live) {
        return true;
    }
    let mut probe = abs.as_path();
    let mut rest: Vec<&std::ffi::OsStr> = Vec::new();
    loop {
        if let Ok(real) = std::fs::canonicalize(probe) {
            let mut full = real;
            for part in rest.iter().rev() {
                full.push(part);
            }
            return under(&full, live);
        }
        match (probe.parent(), probe.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name);
                probe = parent;
            }
            _ => return false,
        }
    }
}

/// Refuse to start when any of `paths` (label, path) is inside the live library.
pub fn check(paths: &[(String, PathBuf)], allow_live_library: bool) -> Result<(), String> {
    if allow_live_library {
        return Ok(());
    }
    for (label, p) in paths {
        if resolves_under_live(p) {
            return Err(format!(
                "{label} ({}) is inside {LIVE_LIBRARY}, the live library. Refusing to start. \
                 Pass --allow-live-library only when you mean to work on it.",
                p.display()
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_library_and_everything_below_it_is_refused_by_text() {
        for p in [
            "/mnt/HHD16TB",
            "/mnt/HHD16TB/",
            "/mnt/HHD16TB/RomVault/RomRoot",
            "/mnt/HHD16TB/../HHD16TB/x",
            "/tmp/../mnt/HHD16TB/x",
            "/mnt/./HHD16TB/x",
            "/MNT/hhd16tb/x",
        ] {
            assert!(resolves_under_live(Path::new(p)), "{p}");
        }
    }

    #[test]
    fn neighbours_are_not_the_library() {
        for p in [
            "/mnt/HHD16TBx/a",
            "/mnt/HHD16",
            "/mnt",
            "/tmp/HHD16TB",
            "relative/dir",
        ] {
            assert!(!resolves_under_live(Path::new(p)), "{p}");
        }
    }

    #[test]
    fn a_link_to_the_library_is_refused() {
        // A stand-in for the library: the real one is never looked at by a test.
        let dir = std::env::temp_dir().join(format!("rv-cli-guard-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let live = dir.join("fake-live");
        std::fs::create_dir_all(live.join("RomRoot")).unwrap();
        std::os::unix::fs::symlink(&live, dir.join("sneaky")).unwrap();
        let through_link = resolves_under(&dir.join("sneaky").join("RomRoot").join("new"), &live);
        let elsewhere = resolves_under(&dir.join("other"), &live);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(through_link);
        assert!(!elsewhere);
    }

    #[test]
    fn the_flag_lets_it_through_and_the_message_names_the_path() {
        let paths = vec![("--root".to_owned(), PathBuf::from("/mnt/HHD16TB/RomRoot"))];
        let e = check(&paths, false).unwrap_err();
        assert!(
            e.contains("--root") && e.contains("/mnt/HHD16TB/RomRoot"),
            "{e}"
        );
        assert!(check(&paths, true).is_ok());
    }
}
