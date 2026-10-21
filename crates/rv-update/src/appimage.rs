//! `rv-gtk` as an AppImage: how to tell that it is running as one, and which release file
//! replaces it.
//!
//! An AppImage mounts itself read-only and runs `rv-gtk` from the mount, so `current_exe()` is
//! a path inside a squashfs that nothing can replace. The file to replace is the AppImage
//! itself, which the runtime names in the `APPIMAGE` environment variable. It is replaced with
//! the release file [`ASSET`] and never with the bare `rv-gtk-linux-x86_64` binary: that would
//! destroy the bundle.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::release::Product;

/// The release file an AppImage is updated from.
pub const ASSET: &str = "rv-gtk-x86_64.AppImage";

impl Product {
    /// The AppImage file of a release for `os`/`arch`; only `rv-gtk` has one, and only for
    /// Linux x86_64 (see `RELEASING.md`).
    pub fn appimage_asset_name_for(self, os: &str, arch: &str) -> Option<&'static str> {
        (self == Product::RvGtk && os == "linux" && arch == "x86_64").then_some(ASSET)
    }

    /// The AppImage file of a release for the system this is running on.
    pub fn appimage_asset_name(self) -> Option<&'static str> {
        self.appimage_asset_name_for(std::env::consts::OS, std::env::consts::ARCH)
    }
}

/// The AppImage this process runs from: `value` is the `APPIMAGE` variable, and it counts only
/// if it names a file that exists. An unset, empty or stale variable means "not an AppImage",
/// so a plain binary is never mistaken for one.
pub fn path_from(value: Option<OsString>) -> Option<PathBuf> {
    let path = PathBuf::from(value.filter(|v| !v.is_empty())?);
    path.is_file().then_some(path)
}

/// [`path_from`] on the real environment.
pub fn running() -> Option<PathBuf> {
    path_from(std::env::var_os("APPIMAGE"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_rv_gtk_on_linux_x86_64_has_an_appimage() {
        assert_eq!(
            Product::RvGtk.appimage_asset_name_for("linux", "x86_64"),
            Some("rv-gtk-x86_64.AppImage")
        );
        assert_eq!(Product::Rv.appimage_asset_name_for("linux", "x86_64"), None);
        assert_eq!(
            Product::RvGtk.appimage_asset_name_for("linux", "aarch64"),
            None
        );
        assert_eq!(
            Product::RvGtk.appimage_asset_name_for("windows", "x86_64"),
            None
        );
    }

    #[test]
    fn the_variable_counts_only_if_it_names_an_existing_file() {
        let dir = std::env::temp_dir().join(format!("rv-appimage-env-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("rv-gtk-x86_64.AppImage");
        std::fs::write(&file, b"x").unwrap();
        assert_eq!(path_from(Some(file.clone().into())), Some(file.clone()));
        assert_eq!(path_from(None), None);
        assert_eq!(path_from(Some(OsString::new())), None);
        assert_eq!(path_from(Some(dir.join("gone").into())), None);
        // A directory is not an AppImage.
        assert_eq!(path_from(Some(dir.clone().into())), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
