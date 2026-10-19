//! Matching a scanned file against a DAT or database entry (`RomVaultCore/Scanner/Compare.cs`).

use rv_core::HeaderFileType;

use crate::header::alt_header_file;
use crate::scan::ScanResult;

/// The hashes of a database / DAT file (`RvFile` fields `Size`, `CRC`, `SHA1`, `MD5`,
/// `HeaderFileType`). `None` is "not known". Hashes are big-endian byte strings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Known {
    pub size: Option<u64>,
    pub crc: Option<Vec<u8>>,
    pub sha1: Option<Vec<u8>>,
    pub md5: Option<Vec<u8>>,
    pub header_type: HeaderFileType,
}

fn eq(known: &Option<Vec<u8>>, got: Option<&[u8]>) -> Option<bool> {
    match (known, got) {
        (Some(k), Some(g)) => Some(k.as_slice() == g),
        _ => None,
    }
}

/// `Compare.CompareHash` (`Scanner/Compare.cs:162-204`).
pub fn compare_hash(db: &Known, test: &ScanResult) -> bool {
    let mut test_found = false;
    if let Some(db_size) = db.size {
        if db_size != test.size {
            return false;
        }
        // Zero-size special case: a DAT file of size 0 with no hashes matches (`:173-177`).
        if test.size == 0 && db.crc.is_none() && db.sha1.is_none() && db.md5.is_none() {
            return true;
        }
    }
    for want in [
        eq(&db.crc, test.digests.crc.as_ref().map(|d| d.as_slice())),
        eq(&db.sha1, test.digests.sha1.as_ref().map(|d| d.as_slice())),
        eq(&db.md5, test.digests.md5.as_ref().map(|d| d.as_slice())),
    ]
    .into_iter()
    .flatten()
    {
        test_found = true;
        if !want {
            return false;
        }
    }
    test_found
}

/// `Compare.CompareAltHash` (`Scanner/Compare.cs:206-244`) with the MD5 test fixed.
///
/// 3.8.0 has `if (ByteArrEqualsQuick(db.MD5, test.AltMD5)) return false;` at
/// `Scanner/Compare.cs:239`: an equal MD5 *rejects* the match and a different one passes. The
/// archive-entry twin is correct (`RvDB/RvFileCode.cs:491-496`) and so is this.
pub fn compare_alt_hash(db: &Known, test: &ScanResult) -> bool {
    if !alt_header_file(test.header_type) {
        return false;
    }
    if db.header_type != test.header_type {
        return false;
    }
    let Some(alt) = &test.alt else {
        return false;
    };
    let mut test_found = false;
    if let Some(db_size) = db.size {
        if db_size != alt.size {
            return false;
        }
    }
    for want in [
        eq(&db.crc, alt.digests.crc.as_ref().map(|d| d.as_slice())),
        eq(&db.sha1, alt.digests.sha1.as_ref().map(|d| d.as_slice())),
        eq(&db.md5, alt.digests.md5.as_ref().map(|d| d.as_slice())),
    ]
    .into_iter()
    .flatten()
    {
        test_found = true;
        if !want {
            return false;
        }
    }
    test_found
}

/// `Compare.CompareWithAlt` (`Scanner/Compare.cs:143-159`): `Some(false)` is a plain match,
/// `Some(true)` an alt (header-stripped) match, `None` no match.
pub fn compare_with_alt(db: &Known, test: &ScanResult) -> Option<bool> {
    if compare_hash(db, test) {
        Some(false)
    } else if compare_alt_hash(db, test) {
        Some(true)
    } else {
        None
    }
}
