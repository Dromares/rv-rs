//! The tree that one `.Cache` file holds (`docs/cache-format.md` section 3).

use crate::enums::{
    DatFlags, DatStatus, FileStatus, FileType, GotStatus, HeaderFileType, MiaStatus, ToSortDirType,
    TreeSelect, ZipStructure,
};

/// One `(id, value)` metadata pair of a [`RvDat`] or [`RvGame`] (`RvDat.cs:150-154`,
/// `RvGame.cs:130-134`). Ids are stored ascending and must stay that way, because the
/// C# `GetData` stops at the first larger id (`RvDat.cs:179-193`, `RvGame.cs:153-167`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MetaEntry {
    pub id: u8,
    pub value: String,
}

fn find_meta(entries: &[MetaEntry], id: u8) -> Option<&str> {
    entries
        .iter()
        .find(|entry| entry.id == id)
        .map(|entry| entry.value.as_str())
}

/// A DAT attached to a directory (`RvDat`, cache section 3.4, `RvDat.cs:147-154`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RvDat {
    /// UTC `DateTime.Ticks` of the DAT file; `i64::MAX` forces a reload (`RvDat.cs:108-111`).
    pub time_stamp: i64,
    pub dat_flags: DatFlags,
    pub entries: Vec<MetaEntry>,
}

impl RvDat {
    /// The value for a `dat_data` id.
    pub fn get(&self, id: u8) -> Option<&str> {
        find_meta(&self.entries, id)
    }
}

/// Game metadata attached to a container node (`RvGame`, cache section 3.6, `RvGame.cs:130-134`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RvGame {
    pub entries: Vec<MetaEntry>,
}

impl RvGame {
    /// The value for a `game_data` id.
    pub fn get(&self, id: u8) -> Option<&str> {
        find_meta(&self.entries, id)
    }
}

/// The user's tree expand and checkbox state (`RvTreeRow`, cache section 3.5, `RvTreeRow.cs:71-72`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RvTreeRow {
    pub expanded: bool,
    pub checked: TreeSelect,
}

impl Default for RvTreeRow {
    /// Defaults of a new row (`RvTreeRow.cs:27-31`).
    fn default() -> Self {
        RvTreeRow {
            expanded: true,
            checked: TreeSelect::SELECTED,
        }
    }
}

/// One node of the cache tree: a directory, archive, file or archive member (`RvFile`,
/// cache section 3.1, `rvFile.cs:405-509`).
///
/// Optional fields are `Some` exactly when the file stores them. The cache writer derives
/// the flag word from this presence, so a node written back has the flags it was read with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RvFile {
    /// Written as a prefix by the parent; the root has none and is always `DIR`.
    pub file_type: FileType,
    /// Bits 21-31 of the flag word, which `FileFlags` does not define (`rvFile.cs:378-402`).
    /// Kept so an unexpected file still writes back unchanged.
    pub unknown_flag_bits: u32,
    pub name: String,
    pub file_name: String,
    /// UTC ticks; `i64::MIN` = unset (`rvFile.cs:41`).
    pub file_mod_time_stamp: i64,
    /// Index into the nearest ancestor's `dir_dats` (cache section 3.4); `-1` is "no index".
    pub dat_index: Option<i32>,
    pub dat_status: DatStatus,
    pub got_status: GotStatus,
    /// Raw `_ZipDatStruct` byte: bits 0-6 structure, bit 7 "fix" (`rvFile.cs:61-70`).
    /// Only Zip/SevenZip nodes of a version >= 3 file have it.
    pub zip_dat_struct: Option<u8>,
    /// Only Zip/SevenZip nodes have it (`rvFile.cs:458`).
    pub zip_struct: Option<ZipStructure>,
    pub tree: Option<RvTreeRow>,
    pub game: Option<RvGame>,
    pub dir_dats: Vec<RvDat>,
    pub children: Vec<RvFile>,
    pub size: Option<u64>,
    pub crc: Option<Vec<u8>>,
    pub sha1: Option<Vec<u8>>,
    pub md5: Option<Vec<u8>>,
    /// Raw byte including the `Required` (0x80) bit.
    pub header_file_type: Option<HeaderFileType>,
    pub alt_size: Option<u64>,
    pub alt_crc: Option<Vec<u8>>,
    pub alt_sha1: Option<Vec<u8>>,
    pub alt_md5: Option<Vec<u8>>,
    pub merge: Option<String>,
    pub status: Option<String>,
    pub zip_file_index: Option<i32>,
    pub zip_file_header_position: Option<u64>,
    pub chd_version: Option<u32>,
    pub to_sort_type: Option<ToSortDirType>,
    pub mia_status: Option<MiaStatus>,
    pub file_status: FileStatus,
}

impl RvFile {
    /// A node of `file_type` with every optional field absent and the C# field defaults
    /// (`DatStatus.NotInDat`, `GotStatus.NotGot`, `rvFile.cs:53-54`).
    pub fn new(file_type: FileType, name: &str) -> RvFile {
        RvFile {
            file_type,
            unknown_flag_bits: 0,
            name: name.to_owned(),
            file_name: String::new(),
            file_mod_time_stamp: i64::MIN,
            dat_index: None,
            dat_status: DatStatus::NOT_IN_DAT,
            got_status: GotStatus::NOT_GOT,
            zip_dat_struct: None,
            zip_struct: None,
            tree: None,
            game: None,
            dir_dats: Vec::new(),
            children: Vec::new(),
            size: None,
            crc: None,
            sha1: None,
            md5: None,
            header_file_type: None,
            alt_size: None,
            alt_crc: None,
            alt_sha1: None,
            alt_md5: None,
            merge: None,
            status: None,
            zip_file_index: None,
            zip_file_header_position: None,
            chd_version: None,
            to_sort_type: None,
            mia_status: None,
            file_status: FileStatus::default(),
        }
    }

    /// The database that ROMVault creates when no cache exists (`DB.OpenDefaultDB`,
    /// `DB.cs:31-55`): an unnamed root with the `RomVault` and `ToSort` directories.
    pub fn default_root() -> RvFile {
        let mut root = RvFile::new(FileType::DIR, "");
        root.dat_status = DatStatus::IN_DAT_COLLECT;
        root.tree = Some(RvTreeRow::default());

        let mut rom_vault = RvFile::new(FileType::DIR, "RomVault");
        rom_vault.dat_status = DatStatus::IN_DAT_COLLECT;
        rom_vault.tree = Some(RvTreeRow::default());

        let mut to_sort = RvFile::new(FileType::DIR, "ToSort");
        to_sort.dat_status = DatStatus::IN_TO_SORT;
        to_sort.tree = Some(RvTreeRow::default());
        to_sort.to_sort_type = Some(ToSortDirType::PRIMARY.with(ToSortDirType::CACHE));

        root.children = vec![rom_vault, to_sort];
        root
    }

    /// Number of nodes in this subtree, itself included.
    pub fn node_count(&self) -> usize {
        1 + self.children.iter().map(RvFile::node_count).sum::<usize>()
    }

    /// `_ZipDatStruct` as `(structure, fix)` (`rvFile.cs:61-63`).
    pub fn zip_dat_struct_parts(&self) -> Option<(ZipStructure, bool)> {
        self.zip_dat_struct
            .map(|raw| (ZipStructure(raw & 0x7f), raw & 0x80 != 0))
    }
}
