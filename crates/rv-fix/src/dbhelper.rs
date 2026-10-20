//! The predicates on two nodes that Find Fixes and the fixer share (`RomVaultCore/RvDB/DBHelper.cs`).

use rv_core::{FileStatus, HeaderFileType, RvFile};

pub const ZERO_CRC: [u8; 4] = [0; 4];
pub const ZERO_MD5: [u8; 16] = [
    0xd4, 0x1d, 0x8c, 0xd9, 0x8f, 0x00, 0xb2, 0x04, 0xe9, 0x80, 0x09, 0x98, 0xec, 0xf8, 0x42, 0x7e,
];
pub const ZERO_SHA1: [u8; 20] = [
    0xda, 0x39, 0xa3, 0xee, 0x5e, 0x6b, 0x4b, 0x0d, 0x32, 0x55, 0xbf, 0xef, 0x95, 0x60, 0x18, 0x90,
    0xaf, 0xd8, 0x07, 0x09,
];

/// `ByteUtils.ByteArrEquals`: false when either side is absent.
pub fn bytes_equal(a: &Option<Vec<u8>>, b: &Option<Vec<u8>>) -> bool {
    matches!((a, b), (Some(x), Some(y)) if x == y)
}

/// `ByteUtils.ByteArrEqualsNull`: true when either side is absent.
pub fn bytes_equal_or_absent(a: &Option<Vec<u8>>, b: &Option<Vec<u8>>) -> bool {
    match (a, b) {
        (Some(x), Some(y)) => x == y,
        _ => true,
    }
}

/// `FileGroup.Equal`: sizes agree, or one of them is unknown.
pub fn size_equal_or_absent(a: Option<u64>, b: Option<u64>) -> bool {
    match (a, b) {
        (Some(x), Some(y)) => x == y,
        _ => true,
    }
}

/// `DBHelper.CheckIfMissingFileCanBeFixedByGotFile` (`DBHelper.cs:164-189`): can `got` be used to
/// make `missing`? A source that was not fully scanned passes on CRC and size alone.
pub fn can_fix(missing: &RvFile, got: &RvFile) -> bool {
    if missing.header_type() != HeaderFileType::NOTHING
        && got.header_type() != HeaderFileType::NOTHING
        && missing.header_type() != got.header_type()
    {
        return false;
    }
    if missing.header_type_required()
        && (got.header_type() == HeaderFileType::NOTHING
            || !got.file_status_is(FileStatus::HEADER_FILE_TYPE_FROM_HEADER))
    {
        return false;
    }
    if missing.file_status_is(FileStatus::SHA1_FROM_DAT)
        && got.file_status_is(FileStatus::SHA1_VERIFIED)
        && !bytes_equal(&missing.sha1, &got.sha1)
        && got.file_status_is(FileStatus::ALT_SHA1_VERIFIED)
        && !bytes_equal(&missing.sha1, &got.alt_sha1)
    {
        return false;
    }
    if missing.file_status_is(FileStatus::MD5_FROM_DAT)
        && got.file_status_is(FileStatus::MD5_VERIFIED)
        && !bytes_equal(&missing.md5, &got.md5)
        && got.file_status_is(FileStatus::ALT_MD5_VERIFIED)
        && !bytes_equal(&missing.md5, &got.alt_md5)
    {
        return false;
    }
    true
}

/// `DBHelper.CheckIfGotfileAndMatchingFileAreFullMatches` (`DBHelper.cs:192-200`).
pub fn full_match(got: &RvFile, matching: &RvFile) -> bool {
    if got.file_status_is(FileStatus::SHA1_VERIFIED)
        && matching.file_status_is(FileStatus::SHA1_VERIFIED)
        && !bytes_equal(&got.sha1, &matching.sha1)
    {
        return false;
    }
    if got.file_status_is(FileStatus::MD5_VERIFIED)
        && matching.file_status_is(FileStatus::MD5_VERIFIED)
        && !bytes_equal(&got.md5, &matching.md5)
    {
        return false;
    }
    true
}

/// `DBHelper.IsZeroLengthFile` (`DBHelper.cs:205-251`): every known hash and the size say
/// "empty", and at least one is known; or a name that ends in `/` (a directory entry).
pub fn is_zero_length(f: &RvFile) -> bool {
    let mut found = false;
    if let Some(md5) = &f.md5 {
        if md5[..] != ZERO_MD5 {
            return false;
        }
        found = true;
    }
    if let Some(sha1) = &f.sha1 {
        if sha1[..] != ZERO_SHA1 {
            return false;
        }
        found = true;
    }
    if let Some(crc) = &f.crc {
        if crc[..] != ZERO_CRC {
            return false;
        }
        found = true;
    }
    if let Some(size) = f.size {
        if size != 0 {
            return false;
        }
        found = true;
    }
    if found {
        return true;
    }
    f.name.len() > 1 && f.name.ends_with('/')
}

#[cfg(test)]
mod tests {
    use super::*;
    use rv_core::FileType;

    fn file(sha1: Option<u8>) -> RvFile {
        let mut f = RvFile::new(FileType::FILE_ZIP, "a");
        f.sha1 = sha1.map(|b| vec![b; 20]);
        f
    }

    #[test]
    fn an_unverified_source_passes_on_crc_and_size() {
        let mut missing = file(Some(1));
        missing.file_status_set(FileStatus::SHA1_FROM_DAT);
        let got = file(Some(2));
        assert!(can_fix(&missing, &got), "no SHA1Verified on the source");
    }

    #[test]
    fn a_verified_source_with_a_different_sha1_and_no_alt_still_passes() {
        // `DBHelper.cs:176-180`: the nested alt test must also fail to reject.
        let mut missing = file(Some(1));
        missing.file_status_set(FileStatus::SHA1_FROM_DAT);
        let mut got = file(Some(2));
        got.file_status_set(FileStatus::SHA1_VERIFIED);
        assert!(can_fix(&missing, &got));
        got.file_status_set(FileStatus::ALT_SHA1_VERIFIED);
        assert!(!can_fix(&missing, &got));
    }

    #[test]
    fn empty_means_every_known_value_is_empty() {
        let mut f = RvFile::new(FileType::FILE_ZIP, "a");
        assert!(!is_zero_length(&f));
        f.size = Some(0);
        assert!(is_zero_length(&f));
        f.crc = Some(vec![0, 0, 0, 1]);
        assert!(!is_zero_length(&f));
        let d = RvFile::new(FileType::FILE_ZIP, "dir/");
        assert!(is_zero_length(&d));
    }
}
