//! Looking at the disk: `Populate.FromADir`, `FromAFile` and `FromAZipFileArchive`
//! (`RomVaultCore/Scanner/Populate.cs`, `FileScanner/FileScan.cs`).
//!
//! Nothing here writes: ROMVault's scanner deletes stale `__RomVault.*.tmp` files it finds
//! (`Populate.cs:108-119`); this one skips them and says so, and leaves the cleaning to the
//! fixer.

use std::fs::{self, File};
use std::io::{self, BufReader, Read};
use std::path::Path;

use rv_archive::chd::Chd;
use rv_archive::sevenzip::SevenZipArchive;
use rv_archive::zip::ZipArchive;
use rv_archive::{ArchiveError, ChdError};
use rv_core::time::ticks_of;
use rv_core::{FileStatus, FileType, GotStatus, HeaderFileType, ScanLevel, ZipStructure};

use super::scanned::ScannedFile;
use crate::scan::{scan, ScanOptions};

/// Why an archive or file could not be opened (the `ZipReturn` values `Populate` tells apart).
#[derive(Debug)]
pub(crate) enum OpenFailure {
    /// `ZipFileLocked`, `ZipErrorOpeningFile`: the file could not be opened; it may be in use.
    Unreadable(String),
    /// `ZipErrorTimeStamp`: the file changed between listing and opening.
    Modified,
    /// Any other `ZipReturn`: not an archive ROMVault can read.
    Corrupt(String),
}

/// `ZipFileOpen(filename, timestamp)`: check that the file is there and unchanged, then open it.
/// A time stamp of `-1` is not checked.
fn open_checked(path: &Path, expected_ticks: i64) -> Result<File, OpenFailure> {
    let meta = match fs::metadata(path) {
        Ok(m) if m.is_file() => m,
        _ => return Err(OpenFailure::Corrupt("File Not Found".to_owned())),
    };
    if expected_ticks != -1 && ticks_of(&meta) != expected_ticks {
        return Err(OpenFailure::Modified);
    }
    File::open(path).map_err(|e| OpenFailure::Unreadable(e.to_string()))
}

fn archive_failure(e: ArchiveError) -> OpenFailure {
    match e {
        ArchiveError::Io(io) if io.kind() == io::ErrorKind::PermissionDenied => {
            OpenFailure::Unreadable(io.to_string())
        }
        other => OpenFailure::Corrupt(other.to_string()),
    }
}

/// Hash `r` into `sf` and set the flags for what was computed (`FileScan.CheckSumRead`,
/// `FileScan.cs:228-531`). On a read error nothing is set and the error is returned, which the
/// caller turns into `Corrupt` (ROMVault's return code `0x17`).
pub(crate) fn hash_into(sf: &mut ScannedFile, r: &mut dyn Read, deep: bool) -> io::Result<()> {
    sf.md5 = None;
    sf.sha1 = None;
    sf.crc = None;
    let mut r = r;
    let res = scan(
        &mut r,
        ScanOptions {
            full_scan: deep,
            sha256: false,
        },
    )?;
    sf.header_file_type = res.header_type;
    if let Some(alt) = &res.alt {
        sf.file_status_set(FileStatus::HEADER_FILE_TYPE_FROM_HEADER);
        sf.alt_size = Some(alt.size);
        sf.file_status_set(FileStatus::ALT_SIZE_FROM_HEADER.with(FileStatus::ALT_SIZE_VERIFIED));
        if let Some(c) = alt.digests.crc {
            sf.alt_crc = Some(c.to_vec());
            sf.file_status_set(FileStatus::ALT_CRC_VERIFIED);
        }
        if let Some(m) = alt.digests.md5 {
            sf.alt_md5 = Some(m.to_vec());
            sf.file_status_set(FileStatus::ALT_MD5_VERIFIED);
        }
        if let Some(s) = alt.digests.sha1 {
            sf.alt_sha1 = Some(s.to_vec());
            sf.file_status_set(FileStatus::ALT_SHA1_VERIFIED);
        }
    }
    if let Some(c) = res.digests.crc {
        sf.crc = Some(c.to_vec());
        sf.file_status_set(FileStatus::CRC_VERIFIED);
    }
    if let Some(m) = res.digests.md5 {
        sf.md5 = Some(m.to_vec());
        sf.file_status_set(FileStatus::MD5_VERIFIED);
    }
    if let Some(s) = res.digests.sha1 {
        sf.sha1 = Some(s.to_vec());
        sf.file_status_set(FileStatus::SHA1_VERIFIED);
    }
    Ok(())
}

/// A directory entry of a zip or 7z: the zero-length constants (`FileScan.cs:149-161`).
fn directory_entry(sf: &mut ScannedFile) {
    sf.header_file_type = HeaderFileType::NOTHING;
    sf.got_status = GotStatus::GOT;
    sf.size = Some(0);
    sf.crc = Some(vec![0; 4]);
    sf.sha1 = Some(hex_const("da39a3ee5e6b4b0d3255bfef95601890afd80709"));
    sf.md5 = Some(hex_const("d41d8cd98f00b204e9800998ecf8427e"));
    sf.file_status_set(
        FileStatus::CRC_FROM_HEADER
            .with(FileStatus::SIZE_VERIFIED)
            .with(FileStatus::CRC_VERIFIED)
            .with(FileStatus::SHA1_VERIFIED)
            .with(FileStatus::MD5_VERIFIED)
            .with(FileStatus::SHA256_VERIFIED),
    );
}

fn hex_const(s: &str) -> Vec<u8> {
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).expect("constant"))
        .collect()
}

/// The outcome for an entry after hashing it (`FileScan.scanAFileInAnArchive`,
/// `FileScan.cs:163-210`). `header_crc` is the CRC the archive claims (none for some 7z entries).
fn finish_entry(
    sf: &mut ScannedFile,
    hashed: io::Result<()>,
    header_crc: Option<[u8; 4]>,
    deep: bool,
) {
    if hashed.is_err() {
        sf.got_status = GotStatus::CORRUPT;
        // A corrupt entry still reports its CRC, otherwise a fix could not move it out.
        if sf.crc.is_none() {
            sf.crc = header_crc.map(|c| c.to_vec());
            sf.file_status_set(FileStatus::CRC_FROM_HEADER);
        }
        return;
    }
    // Not deep scanning: the header's CRC is taken on trust.
    if !deep {
        sf.crc = header_crc.map(|c| c.to_vec());
        sf.file_status_set(FileStatus::CRC_FROM_HEADER);
    }
    match header_crc {
        None => {
            sf.file_status_set(FileStatus::SIZE_VERIFIED);
            sf.got_status = GotStatus::GOT;
        }
        Some(h) if sf.crc.as_deref() == Some(&h[..]) => {
            sf.file_status_set(FileStatus::SIZE_VERIFIED.with(FileStatus::CRC_FROM_HEADER));
            sf.got_status = GotStatus::GOT;
        }
        Some(_) => sf.got_status = GotStatus::CORRUPT,
    }
}

/// Read a zip's directory, and every entry's data when `deep`. `db_ticks` is the time stamp the
/// listing saw; a different one on the file means it changed in between.
pub(crate) fn scan_zip(path: &Path, db_ticks: i64, deep: bool) -> Result<ScannedFile, OpenFailure> {
    let file = open_checked(path, db_ticks)?;
    let mut z =
        ZipArchive::open(BufReader::with_capacity(1 << 16, file)).map_err(archive_failure)?;
    let mut out = ScannedFile::new(FileType::ZIP, &path.to_string_lossy());
    out.zip_struct = ZipStructure(z.structure() as u8);
    let entries: Vec<_> = z.entries().to_vec();
    for (i, e) in entries.iter().enumerate() {
        let mut sf = ScannedFile::new(FileType::FILE_ZIP, &e.name);
        sf.deep_scanned = deep;
        sf.index = i as i32;
        sf.local_header_offset = e.local_head();
        sf.file_mod_time_stamp = e.last_modified();
        if e.is_directory {
            directory_entry(&mut sf);
            out.children.push(sf);
            continue;
        }
        sf.size = Some(e.uncompressed_size);
        sf.file_status_set(FileStatus::SIZE_FROM_HEADER);
        let hashed = match z.open_entry(i) {
            // The CRC is compared below, so the data of a bad entry is still hashed.
            Ok(r) => hash_into(&mut sf, &mut r.without_crc_check(), deep),
            Err(e) => Err(io::Error::other(e)),
        };
        finish_entry(&mut sf, hashed, Some(e.crc32.to_be_bytes()), deep);
        out.children.push(sf);
    }
    Ok(out)
}

/// Read a 7z's directory, and every entry's data when `deep`.
pub(crate) fn scan_7z(path: &Path, db_ticks: i64, deep: bool) -> Result<ScannedFile, OpenFailure> {
    let file = open_checked(path, db_ticks)?;
    let mut z =
        SevenZipArchive::open(BufReader::with_capacity(1 << 16, file)).map_err(archive_failure)?;
    let mut out = ScannedFile::new(FileType::SEVEN_ZIP, &path.to_string_lossy());
    out.zip_struct = ZipStructure(z.structure() as u8);
    let entries: Vec<_> = z.entries().to_vec();
    // Every entry starts out corrupt with its header CRC; the ones the decoder reaches are
    // filled in. A damaged solid stream stops the walk and leaves the rest as they are.
    let mut found: Vec<ScannedFile> = entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let mut sf = ScannedFile::new(FileType::FILE_SEVEN_ZIP, &e.name);
            sf.deep_scanned = deep;
            sf.index = i as i32;
            sf.file_mod_time_stamp = e.modified_ticks.unwrap_or(0);
            if e.is_directory {
                directory_entry(&mut sf);
            } else {
                sf.size = Some(e.size);
                sf.file_status_set(FileStatus::SIZE_FROM_HEADER);
                finish_entry(
                    &mut sf,
                    Err(io::Error::other("not reached")),
                    e.crc32.map(u32::to_be_bytes),
                    deep,
                );
            }
            sf
        })
        .collect();
    let mut visited = 0usize;
    // An error from the walk (a damaged solid stream) leaves the entries it did not reach as
    // they were made above: corrupt.
    let _ = z.for_each_entry_with(false, |e, r| {
        // Entries come in archive order, so the i-th call is entry i.
        let i = visited;
        visited += 1;
        if e.is_directory || i >= found.len() {
            return Ok(());
        }
        let sf = &mut found[i];
        sf.got_status = GotStatus::NOT_GOT;
        sf.crc = None;
        sf.status_flags = sf.status_flags.without(FileStatus::CRC_FROM_HEADER);
        let hashed = hash_into(sf, r, deep);
        finish_entry(sf, hashed, e.crc32.map(u32::to_be_bytes), deep);
        Ok(())
    });
    out.children = found;
    Ok(out)
}

/// What the CHD check found.
pub(crate) struct ChdFacts {
    pub version: u32,
    pub sha1: Option<[u8; 20]>,
    pub md5: Option<[u8; 16]>,
    /// The whole CHD was decoded and its hashes matched.
    pub verified: bool,
    /// The CHD is damaged (ROMVault's `Corrupt` outcomes).
    pub corrupt: Option<String>,
    /// The CHD could not be checked this crate can read (child CHD, unknown codec); it is kept
    /// as it is and not marked verified.
    pub note: Option<String>,
}

/// `CHD.CheckFile` as `Populate.FromAFile` uses it (`Populate.cs:234-322`).
pub(crate) fn check_chd(path: &Path, deep: bool) -> Result<ChdFacts, String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    let mut chd = match Chd::open(BufReader::with_capacity(1 << 16, file)) {
        Ok(c) => c,
        Err(ArchiveError::Chd(ChdError::UnsupportedVersion(v))) => {
            return Ok(ChdFacts {
                version: v,
                sha1: None,
                md5: None,
                verified: false,
                corrupt: None,
                note: Some(format!("CHD version {v} is not supported")),
            })
        }
        Err(e) => {
            return Ok(ChdFacts {
                version: 0,
                sha1: None,
                md5: None,
                verified: false,
                corrupt: Some(e.to_string()),
                note: None,
            })
        }
    };
    let h = chd.header();
    let mut facts = ChdFacts {
        version: h.version,
        sha1: h.dat_sha1(),
        md5: h.md5,
        verified: false,
        corrupt: None,
        note: None,
    };
    if h.requires_parent() {
        // Treated as fine, but the data is not checked (`Populate.cs:285-289`).
        return Ok(facts);
    }
    if deep {
        match chd.verify() {
            Ok(_) => facts.verified = true,
            Err(ArchiveError::Chd(ChdError::UnsupportedCodec(c))) => {
                facts.note = Some(format!("CHD codec {c} is not supported"));
            }
            Err(ArchiveError::Chd(ChdError::RequiresParent)) => {}
            Err(e) => facts.corrupt = Some(e.to_string()),
        }
    }
    Ok(facts)
}

/// `Populate.FromAFile` (`Populate.cs:161-326`): hash a loose file (and check it if it is a CHD),
/// filling `file`. A file that cannot be opened is `FileLocked`; a failed read half way is
/// `Corrupt`. Messages for the caller go to `errors`. Returns true when the scan has to stop:
/// the file vanished or changed between listing and hashing (`fileErrorAbort`).
pub(crate) fn from_a_file(
    file: &mut ScannedFile,
    path: &Path,
    level: ScanLevel,
    errors: &mut Vec<String>,
) -> bool {
    let opened = open_checked(path, file.file_mod_time_stamp);
    let f = match opened {
        Ok(f) => f,
        Err(OpenFailure::Unreadable(why)) => {
            errors.push(format!("File Locked: {why}"));
            file.got_status = GotStatus::FILE_LOCKED;
            return false;
        }
        Err(OpenFailure::Modified) | Err(OpenFailure::Corrupt(_)) => {
            errors.push("File changed while it was being scanned. Scan Aborted.".to_owned());
            file.got_status = GotStatus::FILE_LOCKED;
            return true;
        }
    };
    let mut tmp = ScannedFile::new(FileType::FILE, &file.name);
    let mut reader = BufReader::with_capacity(1 << 20, f);
    let hashed = hash_into(&mut tmp, &mut reader, true);
    if hashed.is_err() {
        errors.push("Error Scanning File".to_owned());
        file.got_status = GotStatus::CORRUPT;
        return false;
    }
    tmp.got_status = GotStatus::GOT;
    file.deep_scanned = true;
    file.header_file_type = tmp.header_file_type;
    file.size = tmp.size.or(file.size);
    file.crc = tmp.crc;
    file.sha1 = tmp.sha1;
    file.md5 = tmp.md5;
    file.alt_size = tmp.alt_size;
    file.alt_crc = tmp.alt_crc;
    file.alt_sha1 = tmp.alt_sha1;
    file.alt_md5 = tmp.alt_md5;
    file.got_status = GotStatus::GOT;

    file.file_status_set(FileStatus::SIZE_VERIFIED);
    if file.header_file_type != HeaderFileType::NOTHING {
        file.file_status_set(FileStatus::HEADER_FILE_TYPE_FROM_HEADER);
    }
    for (present, flag) in [
        (file.crc.is_some(), FileStatus::CRC_VERIFIED),
        (file.sha1.is_some(), FileStatus::SHA1_VERIFIED),
        (file.md5.is_some(), FileStatus::MD5_VERIFIED),
        (file.alt_size.is_some(), FileStatus::ALT_SIZE_VERIFIED),
        (file.alt_crc.is_some(), FileStatus::ALT_CRC_VERIFIED),
        (file.alt_sha1.is_some(), FileStatus::ALT_SHA1_VERIFIED),
        (file.alt_md5.is_some(), FileStatus::ALT_MD5_VERIFIED),
    ] {
        if present {
            file.file_status_set(flag);
        }
    }

    if file.header_file_type == HeaderFileType::CHD {
        let deep = level != ScanLevel::Level1;
        match check_chd(path, deep) {
            Err(why) => {
                errors.push(format!("CHD ERROR : {why}"));
                file.got_status = GotStatus::FILE_LOCKED;
                return true;
            }
            Ok(facts) => {
                if let Some(why) = &facts.corrupt {
                    errors.push(format!("CHD ERROR : {why}"));
                    file.got_status = GotStatus::CORRUPT;
                }
                if let Some(note) = &facts.note {
                    errors.push(note.clone());
                }
                file.chd_version = Some(facts.version);
                let verified = facts.verified && facts.corrupt.is_none();
                if let Some(s) = facts.sha1 {
                    file.alt_sha1 = Some(s.to_vec());
                    file.file_status_set(FileStatus::ALT_SHA1_FROM_HEADER);
                    if verified {
                        file.file_status_set(FileStatus::ALT_SHA1_VERIFIED);
                    }
                }
                if let Some(m) = facts.md5 {
                    file.alt_md5 = Some(m.to_vec());
                    file.file_status_set(FileStatus::ALT_MD5_FROM_HEADER);
                    if verified {
                        file.file_status_set(FileStatus::ALT_MD5_VERIFIED);
                    }
                }
            }
        }
    }
    false
}
