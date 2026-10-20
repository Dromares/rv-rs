//! `FileGroup` (`RomVaultCore/FindFix/FileGroup.cs`): the files that are the same file, by CRC
//! and size, whether we have them or want them.

use std::collections::HashMap;

use rv_core::header::alt_header_file;
use rv_core::{HeaderFileType, RvFile};

use crate::dbhelper::{bytes_equal_or_absent, size_equal_or_absent, ZERO_CRC, ZERO_MD5, ZERO_SHA1};
use crate::nodepath::NodePath;

/// One group. Values are copied from the first file and filled in from the others, never
/// overwritten.
#[derive(Clone, Debug, Default)]
pub struct FileGroup {
    pub size: Option<u64>,
    pub crc: Option<Vec<u8>>,
    pub sha1: Option<Vec<u8>>,
    pub md5: Option<Vec<u8>>,
    pub header: HeaderFileType,
    pub alt_size: Option<u64>,
    pub alt_crc: Option<Vec<u8>>,
    pub alt_sha1: Option<Vec<u8>>,
    pub alt_md5: Option<Vec<u8>>,
    /// Every file of the group, found or wanted, in tree order.
    pub files: Vec<NodePath>,
}

fn header_of(file: &RvFile) -> HeaderFileType {
    if alt_header_file(file.header_type()) {
        file.header_type()
    } else {
        HeaderFileType::NOTHING
    }
}

impl FileGroup {
    /// `new FileGroup(sourceFile)`: the file must have a CRC.
    fn new(file: &RvFile, path: &NodePath) -> FileGroup {
        FileGroup {
            size: file.size,
            crc: file.crc.clone(),
            sha1: file.sha1.clone(),
            md5: file.md5.clone(),
            header: header_of(file),
            alt_size: file.alt_size,
            alt_crc: file.alt_crc.clone(),
            alt_sha1: file.alt_sha1.clone(),
            alt_md5: file.alt_md5.clone(),
            files: vec![path.clone()],
        }
    }

    /// The group of the zero-length file (`RvFile.MakeFileZero`), which is always group 0. It has
    /// no file of its own.
    fn zero() -> FileGroup {
        FileGroup {
            size: Some(0),
            crc: Some(ZERO_CRC.to_vec()),
            sha1: Some(ZERO_SHA1.to_vec()),
            md5: Some(ZERO_MD5.to_vec()),
            ..FileGroup::default()
        }
    }

    pub fn is_zero(&self) -> bool {
        self.size == Some(0) && self.crc.as_deref() == Some(&ZERO_CRC[..])
    }

    /// `MergeFileIntoGroup`.
    fn merge(&mut self, file: &RvFile, path: &NodePath) {
        if self.size.is_none() {
            self.size = file.size;
        }
        if self.crc.is_none() {
            self.crc = file.crc.clone();
        }
        if self.sha1.is_none() {
            self.sha1 = file.sha1.clone();
        }
        if self.md5.is_none() {
            self.md5 = file.md5.clone();
        }
        if self.header == HeaderFileType::NOTHING && alt_header_file(file.header_type()) {
            self.header = file.header_type();
        }
        if self.alt_size.is_none() {
            self.alt_size = file.alt_size;
        }
        if self.alt_crc.is_none() {
            self.alt_crc = file.alt_crc.clone();
        }
        if self.alt_sha1.is_none() {
            self.alt_sha1 = file.alt_sha1.clone();
        }
        if self.alt_md5.is_none() {
            self.alt_md5 = file.alt_md5.clone();
        }
        self.files.push(path.clone());
    }

    /// `MergeAltFileIntoGroup`: the wanted file's own values are the group's alt values.
    fn merge_alt(&mut self, file: &RvFile, path: &NodePath) {
        if self.header == HeaderFileType::NOTHING && alt_header_file(file.header_type()) {
            self.header = file.header_type();
        }
        if self.alt_size.is_none() {
            self.alt_size = file.size;
        }
        if self.alt_crc.is_none() {
            self.alt_crc = file.crc.clone();
        }
        if self.alt_sha1.is_none() {
            self.alt_sha1 = file.sha1.clone();
        }
        if self.alt_md5.is_none() {
            self.alt_md5 = file.md5.clone();
        }
        self.files.push(path.clone());
    }

    /// `FindExactMatch`: every value both have must agree; an absent value agrees with anything.
    pub fn exact_match(&self, file: &RvFile) -> bool {
        size_equal_or_absent(file.size, self.size)
            && bytes_equal_or_absent(&file.crc, &self.crc)
            && bytes_equal_or_absent(&file.sha1, &self.sha1)
            && bytes_equal_or_absent(&file.md5, &self.md5)
            && size_equal_or_absent(file.alt_size, self.alt_size)
            && bytes_equal_or_absent(&file.alt_crc, &self.alt_crc)
            && bytes_equal_or_absent(&file.alt_sha1, &self.alt_sha1)
            && bytes_equal_or_absent(&file.alt_md5, &self.alt_md5)
    }

    /// `FindAltExactMatch`: the file's own values against the group's alt values.
    pub fn alt_exact_match(&self, file: &RvFile) -> bool {
        size_equal_or_absent(file.size, self.alt_size)
            && bytes_equal_or_absent(&file.crc, &self.alt_crc)
            && bytes_equal_or_absent(&file.sha1, &self.alt_sha1)
            && bytes_equal_or_absent(&file.md5, &self.alt_md5)
    }
}

/// All the groups of one Find Fixes run, and which group each file is in.
#[derive(Clone, Debug)]
pub struct Groups {
    /// Group 0 is the zero-length group.
    pub groups: Vec<FileGroup>,
    of: HashMap<NodePath, usize>,
    by_crc: HashMap<Vec<u8>, Vec<usize>>,
    by_sha1: HashMap<Vec<u8>, Vec<usize>>,
    by_md5: HashMap<Vec<u8>, Vec<usize>>,
    by_alt_crc: HashMap<Vec<u8>, Vec<usize>>,
    by_alt_sha1: HashMap<Vec<u8>, Vec<usize>>,
    by_alt_md5: HashMap<Vec<u8>, Vec<usize>>,
}

type Index = HashMap<Vec<u8>, Vec<usize>>;

impl Default for Groups {
    fn default() -> Groups {
        Groups::new()
    }
}

fn index_add(map: &mut HashMap<Vec<u8>, Vec<usize>>, key: &Option<Vec<u8>>, gid: usize) {
    if let Some(k) = key {
        let v = map.entry(k.clone()).or_default();
        if !v.contains(&gid) {
            v.push(gid);
        }
    }
}

impl Groups {
    pub fn new() -> Groups {
        let mut g = Groups {
            groups: Vec::new(),
            of: HashMap::new(),
            by_crc: HashMap::new(),
            by_sha1: HashMap::new(),
            by_md5: HashMap::new(),
            by_alt_crc: HashMap::new(),
            by_alt_sha1: HashMap::new(),
            by_alt_md5: HashMap::new(),
        };
        g.groups.push(FileGroup::zero());
        g.index(0);
        g
    }

    fn index(&mut self, gid: usize) {
        let g = &self.groups[gid];
        let keys = [
            g.crc.clone(),
            g.sha1.clone(),
            g.md5.clone(),
            g.alt_crc.clone(),
            g.alt_sha1.clone(),
            g.alt_md5.clone(),
        ];
        index_add(&mut self.by_crc, &keys[0], gid);
        index_add(&mut self.by_sha1, &keys[1], gid);
        index_add(&mut self.by_md5, &keys[2], gid);
        index_add(&mut self.by_alt_crc, &keys[3], gid);
        index_add(&mut self.by_alt_sha1, &keys[4], gid);
        index_add(&mut self.by_alt_md5, &keys[5], gid);
    }

    /// The group of the file at `path`.
    pub fn group_of(&self, path: &NodePath) -> Option<usize> {
        self.of.get(path).copied()
    }

    pub fn group(&self, gid: usize) -> &FileGroup {
        &self.groups[gid]
    }

    /// Add a file that has a CRC (`AddFindWithExact`): into the first group of that CRC that it
    /// matches exactly, else a group of its own. A file without a CRC is not grouped.
    pub fn add_got(&mut self, file: &RvFile, path: &NodePath) -> Option<usize> {
        let crc = file.crc.as_ref()?;
        let found = self.by_crc.get(crc).and_then(|ids| {
            ids.iter()
                .copied()
                .find(|&g| self.groups[g].exact_match(file))
        });
        let gid = match found {
            Some(g) => {
                self.groups[g].merge(file, path);
                g
            }
            None => {
                self.groups.push(FileGroup::new(file, path));
                let g = self.groups.len() - 1;
                self.index(g);
                g
            }
        };
        // Values the group learned from this file are searchable now.
        self.index(gid);
        self.of.insert(path.clone(), gid);
        Some(gid)
    }

    /// `MergeInMissingFiles` for one wanted file (`FindFixes.cs:255-328`).
    pub fn add_missing(&mut self, file: &RvFile, path: &NodePath) -> Option<usize> {
        let find = |maps: [(&Option<Vec<u8>>, &Index); 3], alt: bool| {
            maps.into_iter().find_map(|(key, map)| {
                let ids = map.get(key.as_ref()?)?;
                ids.iter().copied().find(|&g| {
                    if alt {
                        self.groups[g].alt_exact_match(file)
                    } else {
                        self.groups[g].exact_match(file)
                    }
                })
            })
        };
        if file.header_type() != HeaderFileType::NOTHING {
            let hit = find(
                [
                    (&file.crc, &self.by_alt_crc),
                    (&file.sha1, &self.by_alt_sha1),
                    (&file.md5, &self.by_alt_md5),
                ],
                true,
            );
            if let Some(g) = hit {
                self.groups[g].merge_alt(file, path);
                self.index(g);
                self.of.insert(path.clone(), g);
                return Some(g);
            }
        }
        let hit = find(
            [
                (&file.crc, &self.by_crc),
                (&file.sha1, &self.by_sha1),
                (&file.md5, &self.by_md5),
            ],
            false,
        );
        if let Some(g) = hit {
            self.groups[g].merge(file, path);
            self.of.insert(path.clone(), g);
            return Some(g);
        }
        if file.crc.is_none() && file.sha1.is_none() && file.md5.is_none() {
            let zero_size = file.size == Some(0);
            let dir_entry = file.size.is_none() && file.name.len() > 1 && file.name.ends_with('/');
            if zero_size || dir_entry {
                self.groups[0].files.push(path.clone());
                self.of.insert(path.clone(), 0);
                return Some(0);
            }
        }
        None
    }

    /// A file that now exists (a fix made it): join its group, or start one.
    pub fn add_new(&mut self, file: &RvFile, path: &NodePath) -> Option<usize> {
        if let Some(g) = self.of.get(path) {
            return Some(*g);
        }
        if file.crc.is_none() && crate::dbhelper::is_zero_length(file) {
            self.groups[0].files.push(path.clone());
            self.of.insert(path.clone(), 0);
            return Some(0);
        }
        self.add_got(file, path)
    }

    /// Forget the file at `path` and everything below it (it was removed from the tree).
    pub fn forget(&mut self, path: &NodePath) {
        let gone: Vec<(NodePath, usize)> = self
            .of
            .iter()
            .filter(|(p, _)| p.starts_with(path))
            .map(|(p, g)| (p.clone(), *g))
            .collect();
        for (p, g) in gone {
            self.of.remove(&p);
            self.groups[g].files.retain(|f| *f != p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rv_core::FileType;

    fn f(name: &str, crc: u8, size: u64, sha1: Option<u8>) -> (RvFile, NodePath) {
        let mut n = RvFile::new(FileType::FILE_ZIP, name);
        n.size = Some(size);
        n.crc = Some(vec![crc; 4]);
        n.sha1 = sha1.map(|b| vec![b; 20]);
        (n, NodePath::root().child(FileType::FILE_ZIP, name))
    }

    #[test]
    fn same_crc_with_contradicting_sha1_makes_two_groups() {
        let mut g = Groups::new();
        let (a, pa) = f("a", 7, 3, Some(1));
        let (b, pb) = f("b", 7, 3, Some(2));
        let (c, pc) = f("c", 7, 3, None);
        let ga = g.add_got(&a, &pa).unwrap();
        let gb = g.add_got(&b, &pb).unwrap();
        let gc = g.add_got(&c, &pc).unwrap();
        assert_ne!(ga, gb);
        assert_eq!(
            gc, ga,
            "a CRC-only file joins the first group it does not contradict"
        );
    }

    #[test]
    fn a_wanted_file_joins_a_group_and_an_empty_one_the_zero_group() {
        let mut g = Groups::new();
        let (a, pa) = f("a", 7, 3, Some(1));
        g.add_got(&a, &pa).unwrap();
        let (m, pm) = f("m", 7, 3, None);
        assert_eq!(g.add_missing(&m, &pm), g.group_of(&pa));
        let mut z = RvFile::new(FileType::FILE_ZIP, "z");
        z.size = Some(0);
        let pz = NodePath::root().child(FileType::FILE_ZIP, "z");
        assert_eq!(g.add_missing(&z, &pz), Some(0));
        assert!(g.group(0).is_zero());
    }

    #[test]
    fn forgetting_a_path_removes_it_from_its_group() {
        let mut g = Groups::new();
        let (a, pa) = f("a", 7, 3, None);
        let gid = g.add_got(&a, &pa).unwrap();
        g.forget(&pa);
        assert!(g.group_of(&pa).is_none());
        assert!(g.group(gid).files.is_empty());
    }
}
