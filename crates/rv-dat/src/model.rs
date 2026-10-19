//! The in-memory DAT (`DAT/DATReader/DatStore/*.cs`).
//!
//! ROMVault's strings are `null` or text; here an absent string is `""` and an absent list is
//! empty. Nothing in the readers, the writers or the cleaning code distinguishes the two
//! except `string.IsNullOrWhiteSpace` style tests, which treat them alike.

use rv_core::sorters;
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

    /// `DatDir.ChildAdd` (`DatDir.cs:55-75`): an `UNSET` directory appends (DAT order); a
    /// directory with a type keeps its children sorted for that type and inserts after any
    /// children with an equal name and type. Returns the index.
    pub fn child_add(&mut self, node: DatNode) -> usize {
        if self.base.file_type == FileType::UNSET {
            self.children.push(node);
            return self.children.len() - 1;
        }
        let index = self.insert_index(node.base());
        self.children.insert(index, node);
        index
    }

    /// `DatDir.SortCompareName` (`DatDir.cs:201-240`) for this directory's type.
    pub(crate) fn sort_compare(&self, a: &DatBase, b: &DatBase) -> i32 {
        let res = match self.base.file_type {
            FileType::UNSET => sorters::string_compare(&a.name, &b.name),
            FileType::DIR => sorters::directory_name_compare_case(&a.name, &b.name),
            FileType::ZIP => sorters::trrntzip_string_compare_case(&a.name, &b.name),
            FileType::SEVEN_ZIP => sorters::trrnt_7zip_string_compare(&a.name, &b.name),
            // ROMVault throws "Invalid directory compare type"; a name compare is the closest
            // thing that does not abort the DAT.
            _ => sorters::string_compare(&a.name, &b.name),
        };
        if res != 0 {
            return res;
        }
        i32::from(a.file_type.0.cmp(&b.file_type.0) as i8)
    }

    /// `ChildNameBinarySearch(item, useIndex: false, findFirst: false)` on a sorted directory
    /// (`DatDir.cs:129-199`): the index after the last child that compares equal, or the
    /// insertion point.
    fn insert_index(&self, item: &DatBase) -> usize {
        let (mut bottom, mut top, mut mid, mut res) = (0usize, self.children.len(), 0usize, -1i32);
        while bottom < top && res != 0 {
            mid = (bottom + top) / 2;
            res = self.sort_compare(item, self.children[mid].base());
            if res < 0 {
                top = mid;
            } else if res > 0 {
                bottom = mid + 1;
            }
        }
        let mut index = mid;
        if res == 0 {
            let mut res1 = 0;
            while index + 1 < self.children.len() && res1 == 0 {
                res1 = self.sort_compare(item, self.children[index + 1].base());
                if res1 == 0 {
                    index += 1;
                }
            }
            if res1 == 0 {
                index += 1;
            }
        } else if res > 0 {
            index += 1;
        }
        index
    }

    /// `ChildNameSearch` (`DatDir.cs:111-119`): the first child with this name and file type,
    /// compared the way this directory's type sorts. The sort orders end in a case-sensitive
    /// tie-break, so names that differ only by case are different children. `UNSET` directories
    /// are searched in DAT order, which finds the same child as the sorted index does.
    pub fn child_name_search(&self, name: &str, file_type: FileType) -> Option<usize> {
        let probe = DatBase::new(name, file_type);
        if self.base.file_type == FileType::UNSET {
            return self
                .children
                .iter()
                .position(|c| c.name() == name && c.base().file_type == file_type);
        }
        let (mut bottom, mut top, mut mid, mut res) = (0usize, self.children.len(), 0usize, -1i32);
        while bottom < top && res != 0 {
            mid = (bottom + top) / 2;
            res = self.sort_compare(&probe, self.children[mid].base());
            if res < 0 {
                top = mid;
            } else if res > 0 {
                bottom = mid + 1;
            }
        }
        if res != 0 {
            return None;
        }
        let mut index = mid;
        while index > 0 && self.sort_compare(&probe, self.children[index - 1].base()) == 0 {
            index -= 1;
        }
        Some(index)
    }

    /// `ChildNameSearchAdd` for a directory child: the index of the existing directory called
    /// `name` of type `file_type`, or of a new empty one made by `make` and added.
    pub fn dir_search_add(
        &mut self,
        name: &str,
        file_type: FileType,
        make: impl FnOnce() -> DatDir,
    ) -> usize {
        match self.child_name_search(name, file_type) {
            Some(i) if matches!(self.children[i], DatNode::Dir(_)) => i,
            _ => self.child_add(DatNode::Dir(make())),
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str) -> DatNode {
        DatNode::File(DatFile::new(name))
    }

    fn names(d: &DatDir) -> Vec<&str> {
        d.children.iter().map(DatNode::name).collect()
    }

    #[test]
    fn an_unset_directory_keeps_dat_order() {
        let mut d = DatDir::new("", FileType::UNSET);
        assert_eq!(d.child_add(file("b")), 0);
        assert_eq!(d.child_add(file("a")), 1);
        assert_eq!(names(&d), ["b", "a"]);
        assert_eq!(d.child_name_search("a", FileType::UNSET), Some(1));
        assert_eq!(d.child_name_search("a", FileType::DIR), None);
    }

    #[test]
    fn a_typed_directory_sorts_for_its_type_and_puts_equal_names_last() {
        // Zip: TorrentZip order, ASCII case folded first.
        let mut z = DatDir::new("z", FileType::ZIP);
        for n in ["b", "A", "a", "a"] {
            z.child_add(file(n));
        }
        assert_eq!(names(&z), ["A", "a", "a", "b"]);
        // Dir: lower-cased ordinal, then ordinal.
        let mut d = DatDir::new("d", FileType::DIR);
        for n in ["B", "a", "A", "_x"] {
            d.child_add(file(n));
        }
        assert_eq!(names(&d), ["_x", "A", "a", "B"]);

        // A search uses the same order, including its final case-sensitive tie-break: names that
        // differ only by case are two items in every kind of directory.
        assert_eq!(d.child_name_search("B", FileType::UNSET), Some(3));
        assert_eq!(d.child_name_search("a", FileType::UNSET), Some(2));
        assert_eq!(d.child_name_search("b", FileType::UNSET), None);
        assert_eq!(
            d.child_name_search("B", FileType::DIR),
            None,
            "the type must match too"
        );
    }

    #[test]
    fn dir_search_add_finds_or_makes_one_directory() {
        let mut d = DatDir::new("", FileType::DIR);
        let a = d.dir_search_add("Sub", FileType::DIR, || DatDir::new("Sub", FileType::DIR));
        let b = d.dir_search_add("Sub", FileType::DIR, || panic!("it exists"));
        assert_eq!(a, b);
        assert_eq!(d.children.len(), 1);
        // Another case, or another type, is another item.
        d.dir_search_add("SUB", FileType::DIR, || DatDir::new("SUB", FileType::DIR));
        d.dir_search_add("Sub", FileType::ZIP, || DatDir::new("Sub", FileType::ZIP));
        assert_eq!(d.children.len(), 3);
    }
}
