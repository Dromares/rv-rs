//! Folder and file pickers. `gtk::FileDialog` goes through the XDG desktop portal when it is
//! asked to (`GTK_USE_PORTAL=1`, which [`super::run`] sets), so on Plasma the dialog is Plasma's
//! own. This replaces ROMVault's Windows folder-picker (`FolderBrowser*.cs`, a P/Invoke of the
//! shell's `IFileDialog`).

use std::path::{Path, PathBuf};

use gtk4::prelude::*;
use gtk4::{gio, Window};

fn dialog(title: &str, current: &str) -> gtk4::FileDialog {
    let d = gtk4::FileDialog::builder().title(title).modal(true).build();
    let current = Path::new(current);
    let start = if current.is_dir() {
        Some(current)
    } else {
        current.parent()
    };
    if let Some(dir) = start.filter(|d| d.is_dir()) {
        d.set_initial_folder(Some(&gio::File::for_path(dir)));
    }
    d
}

/// Ask for a directory; `done` gets it unless the dialog was cancelled.
pub fn pick_folder(
    parent: &Window,
    title: &str,
    current: &str,
    done: impl FnOnce(PathBuf) + 'static,
) {
    dialog(title, current).select_folder(Some(parent), gio::Cancellable::NONE, move |res| {
        if let Some(path) = res.ok().and_then(|f| f.path()) {
            done(path);
        }
    });
}

/// Ask for a file that may not exist yet (the cache is created on the first save).
pub fn pick_new_file(
    parent: &Window,
    title: &str,
    current: &str,
    done: impl FnOnce(PathBuf) + 'static,
) {
    let d = dialog(title, current);
    if let Some(name) = Path::new(current).file_name().and_then(|n| n.to_str()) {
        d.set_initial_name(Some(name));
    }
    d.save(Some(parent), gio::Cancellable::NONE, move |res| {
        if let Some(path) = res.ok().and_then(|f| f.path()) {
            done(path);
        }
    });
}
