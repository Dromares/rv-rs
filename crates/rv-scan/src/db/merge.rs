//! Putting what the scanner found into the tree: `RvFile.FileAdd`, `FileMergeIn`,
//! `MergeInArchive` and the match predicates (`RomVaultCore/RvDB/RvFileCode.cs`,
//! `RomVaultCore/Scanner/Compare.cs`).

use rv_core::header::{alt_header_file, file_header_length};
use rv_core::sorters::compare_name;
use rv_core::{Cx, DatStatus, HeaderFileType, RvFile, ScanLevel, Settings, ZipStructure};

use super::scanned::ScannedFile;

fn eq_both(a: &Option<Vec<u8>>, b: &Option<Vec<u8>>, found: &mut bool) -> bool {
    if let (Some(x), Some(y)) = (a, b) {
        *found = true;
        return x == y;
    }
    true
}

/// `CompareHash` (`Compare.cs:162-204`, `RvFileCode.cs:420-460`): size, CRC, SHA1 and MD5 must
/// agree where both sides have them, and at least one hash must have been compared. A size-only
/// DAT file of size 0 matches an empty file.
pub(crate) fn compare_hash(db: &RvFile, test: &ScannedFile) -> bool {
    let mut found = false;
    if let (Some(a), Some(b)) = (db.size, test.size) {
        if a != b {
            return false;
        }
        if b == 0 && db.crc.is_none() && db.sha1.is_none() && db.md5.is_none() {
            return true;
        }
    }
    eq_both(&db.crc, &test.crc, &mut found)
        && eq_both(&db.sha1, &test.sha1, &mut found)
        && eq_both(&db.md5, &test.md5, &mut found)
        && found
}

/// `CompareAltHash` (`Compare.cs:206-244`, `RvFileCode.cs:461-499`): the item's hashes against
/// those of the file without its copier header. The MD5 test is the right way round here;
/// ROMVault's loose-file version has it inverted (`Compare.cs:239`).
pub(crate) fn compare_alt_hash(db: &RvFile, test: &ScannedFile) -> bool {
    if !alt_header_file(test.header_file_type) {
        return false;
    }
    if db.header_type() != test.header_file_type {
        return false;
    }
    let mut found = false;
    if let (Some(a), Some(b)) = (db.size, test.alt_size) {
        if a != b {
            return false;
        }
    }
    eq_both(&db.crc, &test.alt_crc, &mut found)
        && eq_both(&db.sha1, &test.alt_sha1, &mut found)
        && eq_both(&db.md5, &test.alt_md5, &mut found)
        && found
}

/// `CompareWithAlt` (`Compare.cs:143-159`): `Some(false)` a plain match, `Some(true)` a match
/// without the copier header.
pub(crate) fn compare_with_alt(db: &RvFile, test: &ScannedFile) -> Option<bool> {
    if compare_hash(db, test) {
        Some(false)
    } else if compare_alt_hash(db, test) {
        Some(true)
    } else {
        None
    }
}

/// `RvFile.CompareWithAlt` (`RvFileCode.cs:396-419`), for archive entries: the same, and an item
/// that has a real time stamp must also have the entry's.
fn compare_entry_with_alt(db: &RvFile, test: &ScannedFile) -> Option<bool> {
    if db.file_mod_time_stamp != i64::MIN
        && db.file_mod_time_stamp != 0
        && db.file_mod_time_stamp != test.file_mod_time_stamp
    {
        return None;
    }
    compare_with_alt(db, test)
}

/// `FileCompare.Phase1Test` (`Compare.cs:46-107`): does the item match the scanned file without
/// hashing anything more? `index_case` 0 compares the names exactly, 1 case-insensitively.
/// Returns `Some(alt)` for a match.
pub(crate) fn phase1(
    settings: &Settings,
    db: &RvFile,
    test: &ScannedFile,
    level: ScanLevel,
    index_case: u8,
) -> Option<bool> {
    let same_name = if index_case == 0 {
        db.name == test.name
    } else {
        compare_name(db.file_type, &db.name, test.file_type, &test.name) == 0
    };
    if !same_name || db.file_type != test.file_type {
        return None;
    }
    // Directories and archives match on name and type; their content is looked at later.
    if db.is_directory() {
        return Some(false);
    }
    if db.header_type_required() && db.header_type() != test.header_file_type {
        return None;
    }
    // Anything that came from an archive, or was hashed up front at level 3, has a CRC and can
    // be compared in full.
    if test.crc.is_some() {
        return compare_with_alt(db, test);
    }
    // A loose file not hashed yet: match on time stamp and size, unless the item was only ever
    // scanned quickly and this scan goes deeper.
    if level != ScanLevel::Level1 && !db.is_deep_scanned(settings) {
        return None;
    }
    if db.file_mod_time_stamp != test.file_mod_time_stamp {
        return None;
    }
    if db.size == test.size {
        return Some(false);
    }
    let header = file_header_length(db.header_type()) as u64;
    if Some(db.size.unwrap_or(0) + header) != test.size {
        return None;
    }
    Some(true)
}

/// `RvFile.FileMergeIn(ScannedFile, altFile)` (`RvFileCode.cs:171-195`): the item takes what the
/// scan found. Values it already has (from a DAT) are kept; the rest are filled in.
pub(crate) fn file_merge_in(db: &mut RvFile, file: &ScannedFile, alt_file: bool, cx: &Cx<'_>) {
    if alt_file {
        db.set_as_alt_file();
    }
    db.file_check_name(&file.name);

    if db.size.is_none() {
        db.size = file.size;
    }
    if db.crc.is_none() {
        db.crc = file.crc.clone();
    }
    if db.sha1.is_none() {
        db.sha1 = file.sha1.clone();
    }
    if db.md5.is_none() {
        db.md5 = file.md5.clone();
    }
    if db.alt_size.is_none() {
        db.alt_size = file.alt_size;
    }
    if db.alt_crc.is_none() {
        db.alt_crc = file.alt_crc.clone();
    }
    if db.alt_sha1.is_none() {
        db.alt_sha1 = file.alt_sha1.clone();
    }
    if db.alt_md5.is_none() {
        db.alt_md5 = file.alt_md5.clone();
    }
    if db.header_type() == HeaderFileType::NOTHING
        && file.header_file_type != HeaderFileType::NOTHING
    {
        db.set_header_type_raw(file.header_file_type);
    }

    db.set_zip_index(file.index);
    db.zip_file_header_position = file.local_header_offset;

    db.file_mod_time_stamp = file.file_mod_time_stamp;
    db.set_got_status(file.got_status, cx);
    db.file_status_set(file.status_flags);
}

/// The `RvFile(ScannedFile)` constructor (`RvFileCode.cs:112-139`).
fn from_scanned(sf: &ScannedFile) -> RvFile {
    let mut n = RvFile::new(sf.file_type, &sf.name);
    n.set_header_type_raw(sf.header_file_type);
    n.set_zip_index(sf.index);
    n.zip_file_header_position = sf.local_header_offset;
    n.size = sf.size;
    n.crc = sf.crc.clone();
    n.sha1 = sf.sha1.clone();
    n.md5 = sf.md5.clone();
    n.alt_size = sf.alt_size;
    n.alt_crc = sf.alt_crc.clone();
    n.alt_sha1 = sf.alt_sha1.clone();
    n.alt_md5 = sf.alt_md5.clone();
    n.file_mod_time_stamp = sf.file_mod_time_stamp;
    n.chd_version = sf.chd_version;
    n.file_status_set(sf.status_flags);
    if n.file_type.is_compressed_dir() {
        n.zip_struct = Some(ZipStructure::NONE);
        n.zip_dat_struct = Some(0);
    }
    n
}

/// `RvFile.FileAdd(ScannedFile, index)` (`RvFileCode.cs:100-110`): a new item that no DAT knows
/// (or, below a ToSort directory, a ToSort item). `cx` is the context of the new item.
pub(crate) fn file_add(parent: &mut RvFile, sf: &ScannedFile, index: usize, cx: &Cx<'_>) {
    let mut n = from_scanned(sf);
    let dat_status = if parent.dat_status == DatStatus::IN_TO_SORT {
        DatStatus::IN_TO_SORT
    } else {
        DatStatus::NOT_IN_DAT
    };
    n.set_dat_got_status(dat_status, sf.got_status, cx);
    parent.child_add_at(n, index);
}

/// `RvFile.MergeInArchive` (`RvFileCode.cs:18-98`): the archive's entries against the items in
/// it. `cx` is the context of the archive itself.
pub(crate) fn merge_in_archive(archive: &mut RvFile, scanned: &mut ScannedFile, cx: &Cx<'_>) {
    archive.mark_as_missing(cx);
    scanned.sort();

    let tree = cx.tree_name(&archive.name);
    let child_cx = Cx::new(cx.settings, &tree);

    let (mut dbi, mut si) = (0usize, 0usize);
    loop {
        let res = if dbi < archive.children.len() && si < scanned.children.len() {
            let (d, s) = (&archive.children[dbi], &scanned.children[si]);
            compare_name(d.file_type, &d.name, s.file_type, &s.name)
        } else if si < scanned.children.len() {
            1
        } else if dbi < archive.children.len() {
            -1
        } else {
            break;
        };
        match res {
            0 => {
                // Several entries may have exactly the same name: all of them are tried.
                let mut count = 1;
                while si + count < scanned.children.len() {
                    let (d, s) = (&archive.children[dbi], &scanned.children[si + count]);
                    if compare_name(d.file_type, &d.name, s.file_type, &s.name) != 0 {
                        break;
                    }
                    count += 1;
                }
                let mut found = vec![false; count];
                for (k, f) in found.iter_mut().enumerate() {
                    let entry = &scanned.children[si + k];
                    if let Some(alt) = compare_entry_with_alt(&archive.children[dbi], entry) {
                        *f = true;
                        file_merge_in(&mut archive.children[dbi], entry, alt, &child_cx);
                    }
                }
                dbi += 1;
                for (k, f) in found.iter().enumerate() {
                    if !*f {
                        file_add(archive, &scanned.children[si + k], dbi, &child_cx);
                    }
                }
                si += count;
            }
            1 => {
                // Only in the archive.
                file_add(archive, &scanned.children[si], dbi, &child_cx);
                dbi += 1;
                si += 1;
            }
            _ => dbi += 1, // only in the database: stays missing
        }
    }
}
