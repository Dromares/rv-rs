//! `ScannedFile` (`FileScanner/ScannedFile.cs`): what the scanner found on disk or inside an
//! archive, before it is merged into the database tree.

use rv_core::sorters::{
    directory_name_compare_case, list_search, trrnt_7zip_string_compare,
    trrntzip_string_compare_case,
};
use rv_core::{FileStatus, FileType, GotStatus, HeaderFileType, ZipStructure};

/// A directory, archive, file or archive entry found by the scanner.
#[derive(Clone, Debug)]
pub struct ScannedFile {
    pub name: String,
    /// UTC ticks of the last write; for an archive entry the time stored in the archive.
    pub file_mod_time_stamp: i64,
    pub file_type: FileType,
    pub got_status: GotStatus,
    /// The structure the archive was found to have (TorrentZip, ...).
    pub zip_struct: ZipStructure,
    /// Directory entries or archive entries, in the order they were found until [`sort`](Self::sort).
    pub children: Vec<ScannedFile>,
    /// Offset of the entry's local header, when it has no data descriptor.
    pub local_header_offset: Option<u64>,
    /// The hashes are of the whole content (not copied from an archive header).
    pub deep_scanned: bool,
    pub status_flags: FileStatus,
    /// The entry's position in the archive. `0` for everything that is not an entry, like
    /// ROMVault's field default; it ends up in the cache as `ZipFileIndex`.
    pub index: i32,
    pub header_file_type: HeaderFileType,
    pub size: Option<u64>,
    pub crc: Option<Vec<u8>>,
    pub sha1: Option<Vec<u8>>,
    pub md5: Option<Vec<u8>>,
    pub alt_size: Option<u64>,
    pub alt_crc: Option<Vec<u8>>,
    pub alt_sha1: Option<Vec<u8>>,
    pub alt_md5: Option<Vec<u8>>,
    pub chd_version: Option<u32>,
    /// Marks the item as paired during `CheckADir`.
    pub search_found: bool,
}

impl ScannedFile {
    pub fn new(file_type: FileType, name: &str) -> ScannedFile {
        ScannedFile {
            name: name.to_owned(),
            file_mod_time_stamp: 0,
            file_type,
            got_status: GotStatus::NOT_GOT,
            zip_struct: ZipStructure::NONE,
            children: Vec::new(),
            local_header_offset: None,
            deep_scanned: false,
            status_flags: FileStatus::default(),
            index: 0,
            header_file_type: HeaderFileType::NOTHING,
            size: None,
            crc: None,
            sha1: None,
            md5: None,
            alt_size: None,
            alt_crc: None,
            alt_sha1: None,
            alt_md5: None,
            chd_version: None,
            search_found: false,
        }
    }

    pub fn is_directory(&self) -> bool {
        self.file_type == FileType::DIR
            || self.file_type == FileType::ZIP
            || self.file_type == FileType::SEVEN_ZIP
    }

    pub fn file_status_set(&mut self, flag: FileStatus) {
        self.status_flags = self.status_flags.with(flag);
    }

    /// `ScannedFile.Sort` (`ScannedFile.cs:76-113`): insertion sort with `ListSearch`, which puts
    /// an item before the items that already compare equal. Entries with the same name end up in
    /// the reverse of the order they were found in. The order is the one of a directory (case
    /// folded), of a TorrentZip, or of a 7z.
    pub fn sort(&mut self) {
        let cmp: fn(&str, &str) -> i32 = match self.file_type {
            FileType::SEVEN_ZIP => trrnt_7zip_string_compare,
            FileType::ZIP => trrntzip_string_compare_case,
            _ => directory_name_compare_case,
        };
        let items = std::mem::take(&mut self.children);
        for item in items {
            let (_, at) = list_search(&self.children, |c| cmp(&item.name, &c.name));
            self.children.insert(at, item);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(n: &str, size: u64) -> ScannedFile {
        let mut f = ScannedFile::new(FileType::FILE_ZIP, n);
        f.size = Some(size);
        f
    }

    #[test]
    fn zip_entries_sort_like_a_torrentzip_and_equal_names_reverse() {
        let mut z = ScannedFile::new(FileType::ZIP, "x.zip");
        z.children = vec![
            named("b", 1),
            named("A", 2),
            named("a", 3),
            named("a", 4),
            named("c", 5),
        ];
        z.sort();
        let order: Vec<(String, u64)> = z
            .children
            .iter()
            .map(|c| (c.name.clone(), c.size.unwrap()))
            .collect();
        assert_eq!(
            order,
            [
                ("A".into(), 2),
                // Exact duplicates come out in reverse order of discovery (size 4 before 3).
                ("a".into(), 4),
                ("a".into(), 3),
                ("b".into(), 1),
                ("c".into(), 5)
            ]
        );
    }
}
