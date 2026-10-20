//! Reading one file or archive on its own and putting it into the tree, for the fixer.
//!
//! The directory scan ([`super::scan_files`]) pairs a whole directory with the tree. The fixer
//! writes one file or one archive at a time and then has to make the tree say what is on disk.
//! It does that with the scanner's own code: read the item back from disk (that read is also its
//! validation), then merge it in exactly as a scan would, so a later scan agrees with the tree
//! by construction.

use std::fs;
use std::path::Path;

use rv_core::time::ticks_of;
use rv_core::{Cx, FileStatus, FileType, GotStatus, RvFile, ScanLevel};

use super::merge::{compare_with_alt, file_add, file_merge_in, merge_in_archive};
use super::populate::{from_a_file, scan_7z, scan_zip, OpenFailure};
pub use super::scanned::ScannedFile;

/// An item that could not be read back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadError(pub String);

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ReadError {}

fn failure(f: OpenFailure) -> ReadError {
    match f {
        OpenFailure::Unreadable(m) | OpenFailure::Corrupt(m) => ReadError(m),
        OpenFailure::Modified => ReadError("the file changed while it was read".to_owned()),
    }
}

/// Read the archive at `path` completely (every entry is hashed, like a level 2 scan). The
/// result is named `name`. A zip or 7z that cannot be read is an error.
pub fn read_archive(
    path: &Path,
    name: &str,
    file_type: FileType,
) -> Result<ScannedFile, ReadError> {
    let meta = fs::metadata(path).map_err(|e| ReadError(e.to_string()))?;
    let mut out = match file_type {
        FileType::SEVEN_ZIP => scan_7z(path, -1, true),
        _ => scan_zip(path, -1, true),
    }
    .map_err(failure)?;
    out.name = name.to_owned();
    out.file_type = file_type;
    out.size = Some(meta.len());
    out.file_mod_time_stamp = ticks_of(&meta);
    out.got_status = GotStatus::GOT;
    out.file_status_set(FileStatus::SIZE_VERIFIED);
    Ok(out)
}

/// Hash the loose file at `path` (and check it if it is a CHD). The result is named `name`.
pub fn read_loose_file(
    path: &Path,
    name: &str,
    level: ScanLevel,
) -> Result<ScannedFile, ReadError> {
    let meta = fs::metadata(path).map_err(|e| ReadError(e.to_string()))?;
    let mut f = ScannedFile::new(FileType::FILE, name);
    f.size = Some(meta.len());
    f.file_mod_time_stamp = ticks_of(&meta);
    f.got_status = GotStatus::GOT;
    f.file_status_set(FileStatus::SIZE_VERIFIED);
    let mut errors = Vec::new();
    let abort = from_a_file(&mut f, path, level, &mut errors);
    if abort || f.got_status != GotStatus::GOT {
        let why = errors
            .pop()
            .unwrap_or_else(|| "the file is not readable".to_owned());
        return Err(ReadError(why));
    }
    Ok(f)
}

/// Does the item match what was read: `Some(false)` a plain match, `Some(true)` a match without
/// the copier header, `None` no match (`CompareWithAlt`).
pub fn matches(db: &RvFile, scanned: &ScannedFile) -> Option<bool> {
    compare_with_alt(db, scanned)
}

/// The item takes what the scan found (`FileMergeIn`). `cx` is the item's own context.
pub fn merge_file(db: &mut RvFile, scanned: &ScannedFile, alt: bool, cx: &Cx<'_>) {
    file_merge_in(db, scanned, alt, cx);
}

/// Add a loose file the tree does not know, as `NotInDat` (`InToSort` below a ToSort
/// directory), at its place in `parent`'s order. Returns the index. `cx` is the new item's
/// context.
pub fn add_file(parent: &mut RvFile, scanned: &ScannedFile, cx: &Cx<'_>) -> usize {
    let (_, index) = parent.child_name_search(scanned.file_type, &scanned.name);
    file_add(parent, scanned, index, cx);
    index
}

/// The archive item takes what was found in its file: the archive's own values, then its
/// entries paired with the items in it (`MatchFound` and `CheckAnArchive`).
pub fn merge_archive(db: &mut RvFile, scanned: &mut ScannedFile, cx: &Cx<'_>) {
    db.mark_as_missing(cx);
    file_merge_in(db, scanned, false, cx);
    db.set_zip_struct(scanned.zip_struct);
    merge_in_archive(db, scanned, cx);
}

/// Add an archive the tree does not know, with all its entries as `NotInDat` (`InToSort` below
/// a ToSort directory). Returns the index. `cx` is the new item's context.
pub fn add_archive(parent: &mut RvFile, scanned: &mut ScannedFile, cx: &Cx<'_>) -> usize {
    let (_, index) = parent.child_name_search(scanned.file_type, &scanned.name);
    file_add(parent, scanned, index, cx);
    let new = &mut parent.children[index];
    new.set_zip_struct(scanned.zip_struct);
    merge_in_archive(new, scanned, cx);
    index
}
