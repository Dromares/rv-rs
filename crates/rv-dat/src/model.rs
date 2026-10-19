//! The in-memory DAT (`DAT/DATReader/DatStore/*.cs`).
//!
//! ROMVault's strings are `null` or text; here an absent string is `""` and an absent list is
//! empty. Nothing in the readers, the writers or the cleaning code distinguishes the two
//! except `string.IsNullOrWhiteSpace` style tests, which treat them alike.

use rv_core::{DatStatus, FileType, HeaderFileType, MiaStatus};

/// `DatBase`, `DatStore/DatBase.cs:5-25`: what a file and a directory share.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DatBase {
    pub name: String,
    /// Default `InDatCollect`.
    pub dat_status: DatStatus,
    pub file_type: FileType,
    /// `long? DateModified`: .NET ticks, or the packed MS-DOS value `(date << 16) | time`
    /// that the DOSCenter reader produces.
    pub date_modified: Option<i64>,
}

impl DatBase {
    pub fn new(name: &str, file_type: FileType) -> DatBase {
        DatBase {
            name: name.to_owned(),
            dat_status: DatStatus::IN_DAT_COLLECT,
            file_type,
            date_modified: None,
        }
    }
}

/// `DatFile`, `DatStore/DatFile.cs:3-40`: a ROM or a disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DatFile {
    pub base: DatBase,
    pub size: Option<u64>,
    /// Big-endian bytes, `None` when the DAT has no value.
    pub crc: Option<Vec<u8>>,
    pub sha1: Option<Vec<u8>>,
    pub md5: Option<Vec<u8>>,
    pub sha256: Option<Vec<u8>>,
    pub merge: String,
    /// Lower-case: `nodump`, `baddump`, `good`, `verified`, `deduped`, ...
    pub status: String,
    pub region: String,
    /// `"yes"` or empty.
    pub mia: String,
    pub is_disk: bool,
    pub header_file_type: HeaderFileType,
    pub mia_status: MiaStatus,
}

impl DatFile {
    pub fn new(name: &str) -> DatFile {
        DatFile {
            base: DatBase::new(name, FileType::UNSET),
            size: None,
            crc: None,
            sha1: None,
            md5: None,
            sha256: None,
            merge: String::new(),
            status: String::new(),
            region: String::new(),
            mia: String::new(),
            is_disk: false,
            header_file_type: HeaderFileType::NOTHING,
            mia_status: MiaStatus::NONE,
        }
    }

    pub fn name(&self) -> &str {
        &self.base.name
    }
}

/// `DatGame`, `DatStore/DatGame.cs:5-45`. Strings are `""` when absent.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DatGame {
    pub id: String,
    pub description: String,
    pub manufacturer: String,
    pub history: String,
    pub clone_of: String,
    pub clone_of_id: String,
    pub rom_of: String,
    pub sample_of: String,
    pub source_file: String,
    pub is_bios: String,
    pub is_device: String,
    pub board: String,
    pub year: String,
    pub runnable: String,
    /// Redump.
    pub serial: String,
    /// Redump.
    pub version: String,
    /// No-Intro.
    pub game_id: String,
    pub category: Vec<String>,
    pub device_ref: Vec<String>,
    pub is_emu_arc: bool,
    pub publisher: String,
    pub developer: String,
    pub genre: String,
    pub sub_genre: String,
    pub ratings: String,
    pub score: String,
    pub players: String,
    pub enabled: String,
    pub crc: String,
    pub source: String,
    pub related_to: String,
}

/// A child of a [`DatDir`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DatNode {
    Dir(DatDir),
    File(DatFile),
}

impl DatNode {
    pub fn name(&self) -> &str {
        match self {
            DatNode::Dir(d) => &d.base.name,
            DatNode::File(f) => &f.base.name,
        }
    }

    pub fn base(&self) -> &DatBase {
        match self {
            DatNode::Dir(d) => &d.base,
            DatNode::File(f) => &f.base,
        }
    }
}

/// `DatDir`, `DatStore/DatDir.cs:13-242`: a directory, or (with `game`) a game / set.
///
/// While `base.file_type` is `UNSET`, which every reader leaves it, `children` is in DAT order
/// (`DatDir.cs:8-11`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DatDir {
    pub base: DatBase,
    /// `_datStruct`: low 7 bits `ZipStructure`, bit 0x80 the "fix" flag (`DatDir.cs:16-24`).
    pub dat_struct: u8,
    pub game: Option<Box<DatGame>>,
    pub children: Vec<DatNode>,
}

impl DatDir {
    pub fn new(name: &str, file_type: FileType) -> DatDir {
        DatDir {
            base: DatBase::new(name, file_type),
            dat_struct: 0,
            game: None,
            children: Vec::new(),
        }
    }

    pub fn name(&self) -> &str {
        &self.base.name
    }

    /// `DatDir.ChildAdd` for an `UNSET` directory: appends, returns the index
    /// (`DatDir.cs:60-69`).
    pub fn child_add(&mut self, node: DatNode) -> usize {
        self.children.push(node);
        self.children.len() - 1
    }

    /// `DatDir.ChildNameSearch` on an `UNSET` directory (`DatDir.cs:111-119`, `:239`): the
    /// first child with this exact name *and file type*. Returns its insertion index.
    pub fn child_name_search(&self, name: &str, file_type: FileType) -> Option<usize> {
        self.children
            .iter()
            .position(|c| c.name() == name && c.base().file_type == file_type)
    }

    /// Number of game directories (those with a [`DatGame`]) in this subtree.
    pub fn game_count(&self) -> usize {
        self.children
            .iter()
            .map(|c| match c {
                DatNode::Dir(d) => usize::from(d.game.is_some()) + d.game_count(),
                DatNode::File(_) => 0,
            })
            .sum()
    }

    /// Number of ROM files (not disks) in this subtree.
    pub fn rom_count(&self) -> usize {
        self.file_count(false)
    }

    /// Number of disks in this subtree.
    pub fn disk_count(&self) -> usize {
        self.file_count(true)
    }

    fn file_count(&self, disks: bool) -> usize {
        self.children
            .iter()
            .map(|c| match c {
                DatNode::Dir(d) => d.file_count(disks),
                DatNode::File(f) => usize::from(f.is_disk == disks),
            })
            .sum()
    }
}

/// `DatHeader`, `DatStore/DatHeader.cs:3-29`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DatHeader {
    pub id: String,
    pub filename: String,
    pub mame_xml: bool,
    pub name: String,
    pub r#type: String,
    pub root_dir: String,
    pub description: String,
    pub subset: String,
    pub category: String,
    pub version: String,
    pub date: String,
    pub author: String,
    pub email: String,
    pub homepage: String,
    pub url: String,
    pub comment: String,
    /// The detector name, e.g. `No-Intro_NES.xml`.
    pub header: String,
    pub compression: String,
    pub merge_type: String,
    /// RomCenter.
    pub split: String,
    pub no_dump: String,
    pub dir: String,
    pub not_zipped: bool,
    pub base_dir: DatDir,
}

impl Default for DatHeader {
    /// `new DatHeader { BaseDir = new DatDir("", FileType.UnSet) }`, which every reader starts
    /// with.
    fn default() -> DatHeader {
        DatHeader {
            id: String::new(),
            filename: String::new(),
            mame_xml: false,
            name: String::new(),
            r#type: String::new(),
            root_dir: String::new(),
            description: String::new(),
            subset: String::new(),
            category: String::new(),
            version: String::new(),
            date: String::new(),
            author: String::new(),
            email: String::new(),
            homepage: String::new(),
            url: String::new(),
            comment: String::new(),
            header: String::new(),
            compression: String::new(),
            merge_type: String::new(),
            split: String::new(),
            no_dump: String::new(),
            dir: String::new(),
            not_zipped: false,
            base_dir: DatDir::new("", FileType::UNSET),
        }
    }
}
