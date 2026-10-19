//! The in-memory database: the primitives that the DAT update and the scanner share
//! (`RomVaultCore/RvDB/rvFile.cs`, `RvFileCode.cs`, `RvFileDatCode.cs`, `DB.cs`).
//!
//! ROMVault's `RvFile` has a `Parent` pointer and keeps `RepStatus` and the directory counters
//! up to date as a side effect of setting `DatStatus` or `GotStatus`. This tree has no parent
//! pointers, so every operation that needs something from above takes it as an argument:
//!
//! * [`Cx`] carries the settings and the `TreeFullName` of the parent, which the ignore-file rule
//!   of `RepStatusReset` needs (`rvFile.cs:764-783`);
//! * `parent_got` is the parent's `GotStatus`, which `TestRemove` reads (`RvFileCode.cs:376`).
//!
//! `RepStatus` is stored on the node and set at the same points as in ROMVault. The counters
//! ([`DirStatus`]) are computed on request.

use std::collections::HashMap;
use std::fmt;

use crate::enums::{
    DatStatus, FileStatus, FileType, GotStatus, HeaderFileType, MiaStatus, ZipStructure,
};
use crate::model::{RvDat, RvFile, RvTreeRow};
use crate::rep_status::{baseline_status, ui_status, DirStatus, RepStatus};
use crate::settings::{dat_tree_path, path_combine, Settings};
use crate::sorters::{compare_name, list_search};

/// `EFile` (`DBHelper.cs:13-17`): what the caller does with an item after a removal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EFile {
    /// Keep the item: it comes from a DAT.
    Keep,
    /// Remove the item from its parent.
    Delete,
}

/// `DatUpdateStatus` (`RvDat.cs:12-16`): transient, never written to the cache.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DatUpdateStatus {
    /// What every DAT is after a cache load, until it is matched with a file on disk.
    #[default]
    Delete,
    Correct,
}

/// What an operation on a node needs from its surroundings.
#[derive(Clone, Copy)]
pub struct Cx<'a> {
    pub settings: &'a Settings,
    /// `TreeFullName` of the node's parent, with `/`: `""` for the root's children.
    pub parent_tree: &'a str,
}

impl<'a> Cx<'a> {
    pub fn new(settings: &'a Settings, parent_tree: &'a str) -> Cx<'a> {
        Cx {
            settings,
            parent_tree,
        }
    }

    /// `TreeFullName` of a child of this node's parent named `name`.
    pub fn tree_name(&self, name: &str) -> String {
        path_combine(self.parent_tree, name)
    }
}

impl RvFile {
    /// `IsDirectory`: Dir, Zip or SevenZip (`rvFile.cs:791`).
    pub fn is_directory(&self) -> bool {
        self.file_type == FileType::DIR
            || self.file_type == FileType::ZIP
            || self.file_type == FileType::SEVEN_ZIP
    }

    /// `IsFile`: File, FileZip or FileSevenZip (`rvFile.cs:915`).
    pub fn is_file(&self) -> bool {
        self.file_type == FileType::FILE
            || self.file_type == FileType::FILE_ZIP
            || self.file_type == FileType::FILE_SEVEN_ZIP
    }

    /// `NameCase`: the name as found on disk (`rvFile.cs:172`).
    pub fn name_case(&self) -> &str {
        if self.file_name.trim().is_empty() {
            &self.name
        } else {
            &self.file_name
        }
    }

    /// `RvFile.HeaderFileType`: the type without the `Required` bit (`rvFile.cs:47`).
    pub fn header_type(&self) -> HeaderFileType {
        HeaderFileType(self.header_file_type.map_or(0, |h| h.0) & HeaderFileType::HEADER_MASK.0)
    }

    /// `HeaderFileTypeRequired` (`rvFile.cs:48`).
    pub fn header_type_required(&self) -> bool {
        self.header_file_type
            .is_some_and(|h| h.0 & HeaderFileType::REQUIRED.0 != 0)
    }

    /// `HeaderFileTypeSet`: store the raw byte; `Nothing` means absent (the cache stores the
    /// field only when it is not zero, `rvFile.cs:425`).
    pub fn set_header_type_raw(&mut self, raw: HeaderFileType) {
        self.header_file_type = (raw.0 != 0).then_some(raw);
    }

    /// The MIA flags; none when absent.
    pub fn mia(&self) -> MiaStatus {
        self.mia_status.unwrap_or(MiaStatus::NONE)
    }

    /// `MIAStatusSet`.
    pub fn mia_set(&mut self, flag: MiaStatus) {
        let v = self.mia().with(flag);
        self.mia_status = (v != MiaStatus::NONE).then_some(v);
    }

    /// `MIAStatusClear`.
    pub fn mia_clear(&mut self, flag: MiaStatus) {
        let v = self.mia().without(flag);
        self.mia_status = (v != MiaStatus::NONE).then_some(v);
    }

    /// `FileStatusSet`.
    pub fn file_status_set(&mut self, flag: FileStatus) {
        self.file_status = self.file_status.with(flag);
    }

    /// `FileStatusClear`.
    pub fn file_status_clear(&mut self, flag: FileStatus) {
        self.file_status = self.file_status.without(flag);
    }

    /// `FileStatusIs`.
    pub fn file_status_is(&self, flag: FileStatus) -> bool {
        self.file_status.has(flag)
    }

    /// `ZipDatStruct` and `ZipDatStructFix` (`rvFile.cs:62-63`). Only Zip and SevenZip nodes
    /// have the byte.
    pub fn zip_dat_struct_value(&self) -> (ZipStructure, bool) {
        self.zip_dat_struct_parts()
            .unwrap_or((ZipStructure::NONE, false))
    }

    /// `SetZipDatStruct` (`rvFile.cs:65-70`). A Dir keeps no such byte in the cache, so the
    /// call is a no-op there.
    pub fn set_zip_dat_struct(&mut self, structure: ZipStructure, fix: bool) {
        if self.file_type.is_compressed_dir() {
            self.zip_dat_struct = Some(structure.0 | if fix { 0x80 } else { 0 });
        }
    }

    /// `ZipStruct = value` for an archive; other types have none.
    pub fn set_zip_struct(&mut self, structure: ZipStructure) {
        if self.file_type.is_compressed_dir() {
            self.zip_struct = Some(structure);
        }
    }

    /// `ZipFileIndex` (`-1` when unset).
    pub fn zip_index(&self) -> i32 {
        self.zip_file_index.unwrap_or(-1)
    }

    pub fn set_zip_index(&mut self, index: i32) {
        self.zip_file_index = (index >= 0).then_some(index);
    }

    /// `FileName = null` or the on-disk name.
    pub fn set_file_name(&mut self, name: Option<&str>) {
        self.file_name = name.unwrap_or("").to_owned();
    }

    // ---- statuses -------------------------------------------------------------------------

    /// `RepStatusReset` (`rvFile.cs:764-787`): clear `SearchFound` and set `RepStatus` to the
    /// baseline of `(FileType, DatStatus, GotStatus)`. A loose `Got` file that no DAT knows,
    /// whose status is still `UnSet`, `Unknown` or `Ignore`, is first tested against the ignore
    /// patterns of its DAT rule (or the global ones when no rule covers it); a match makes it
    /// `Ignore`.
    pub fn rep_status_reset(&mut self, cx: &Cx<'_>) {
        self.search_found = false;
        if (self.rep_status == RepStatus::UN_SET
            || self.rep_status == RepStatus::UNKNOWN
            || self.rep_status == RepStatus::IGNORE)
            && self.file_type == FileType::FILE
            && self.got_status == GotStatus::GOT
            && self.dat_status == DatStatus::NOT_IN_DAT
        {
            let dat_path = dat_tree_path(&cx.tree_name(&self.name));
            let list = match cx.settings.find_dat_rule(&dat_path) {
                Some(rule) => rule.ignore_regex(),
                None => cx.settings.ignore_regex(),
            };
            if list.iter().any(|re| re.is_match(&self.name)) {
                self.rep_status = RepStatus::IGNORE;
                return;
            }
        }
        self.rep_status = baseline_status(self.file_type, self.dat_status, self.got_status);
    }

    /// The `DatStatus` setter (`rvFile.cs:328-336`).
    pub fn set_dat_status(&mut self, dat_status: DatStatus, cx: &Cx<'_>) {
        self.dat_status = dat_status;
        self.rep_status_reset(cx);
    }

    /// The `GotStatus` setter (`rvFile.cs:339-350`).
    pub fn set_got_status(&mut self, got_status: GotStatus, cx: &Cx<'_>) {
        self.got_status = got_status;
        self.rep_status_reset(cx);
    }

    /// `SetDatGotStatus` (`rvFile.cs:753-762`).
    pub fn set_dat_got_status(
        &mut self,
        dat_status: DatStatus,
        got_status: GotStatus,
        cx: &Cx<'_>,
    ) {
        self.dat_status = dat_status;
        self.got_status = got_status;
        self.rep_status_reset(cx);
    }

    /// The counts of the statuses of every descendant (`RvFile.DirStatus`,
    /// `rvFile.cs:869-895`). Each descendant counts once, as `UIStatus(mia, rep)`.
    pub fn dir_status(&self, show_new_mia: bool) -> DirStatus {
        let mut out = DirStatus::default();
        self.add_descendants(&mut out, show_new_mia);
        out
    }

    fn add_descendants(&self, out: &mut DirStatus, show_new_mia: bool) {
        for child in &self.children {
            out.add(child.rep_status, child.mia(), show_new_mia);
            child.add_descendants(out, show_new_mia);
        }
    }

    /// `RepairStatus.ReportStatusReset` (`RepairStatus.cs:216-233`) for this subtree:
    /// `RepStatusReset` on every node, top down. `cx` is this node's context.
    pub fn report_status_reset(&mut self, cx: &Cx<'_>) {
        self.rep_status_reset(cx);
        if !self.is_directory() {
            return;
        }
        let tree = cx.tree_name(&self.name);
        let child_cx = Cx::new(cx.settings, &tree);
        for child in &mut self.children {
            child.report_status_reset(&child_cx);
        }
    }

    // ---- children -------------------------------------------------------------------------

    /// `ChildNameSearch(type, name)` (`rvFile.cs:838-841`): `(found, index)` in the sorted
    /// children; `index` is the first of equal names, else the insertion point.
    pub fn child_name_search(&self, file_type: FileType, name: &str) -> (bool, usize) {
        let (res, index) = list_search(&self.children, |c| {
            compare_name(file_type, name, c.file_type, &c.name)
        });
        (res == 0, index)
    }

    /// `ChildAdd(child)` (`rvFile.cs:802-807`): insert in name order, before equal names.
    pub fn child_add(&mut self, child: RvFile) -> usize {
        let (_, index) = self.child_name_search(child.file_type, &child.name);
        self.children.insert(index, child);
        index
    }

    /// `ChildAdd(child, index)`.
    pub fn child_add_at(&mut self, child: RvFile, index: usize) {
        self.children.insert(index, child);
    }

    /// `DatAddDirectory` (`RvFileDatCode.cs:43-53`): a Dir that a DAT's location implies.
    pub fn dat_add_directory(&mut self, dir_name: &str, index: usize, cx: &Cx<'_>) {
        let mut dir = RvFile::new(FileType::DIR, dir_name);
        dir.tree = Some(RvTreeRow::default());
        dir.set_dat_status(DatStatus::IN_DAT_COLLECT, cx);
        self.children.insert(index, dir);
    }

    // ---- removal and merge ----------------------------------------------------------------

    /// `FileRemove` (`RvFileCode.cs:300-366`): forget what the scanner knew about this item,
    /// keeping what a DAT supplied. `parent_got` is the parent's `GotStatus`. `Delete` means
    /// the caller removes the item from its parent.
    pub fn file_remove(&mut self, cx: &Cx<'_>, parent_got: GotStatus) -> EFile {
        if self.is_file() {
            self.zip_file_index = None;
            self.zip_file_header_position = None;

            // `TestRemove` also sets `GotStatus` to `NotGot` (unless the file is locked).
            if self.test_remove(cx, parent_got) == EFile::Delete {
                return EFile::Delete;
            }

            if !self.file_status_is(FileStatus::DATE_FROM_DAT) {
                self.file_mod_time_stamp = i64::MIN;
            }

            // If none of the primary metadata is from the DAT, delete it.
            if !self.file_status_is(FileStatus::HEADER_FILE_TYPE_FROM_DAT) {
                self.set_header_type_raw(HeaderFileType::NOTHING);
            }
            if !self.file_status_is(FileStatus::SIZE_FROM_DAT) {
                self.size = None;
            }
            if !self.file_status_is(FileStatus::CRC_FROM_DAT) {
                self.crc = None;
            }
            if !self.file_status_is(FileStatus::SHA1_FROM_DAT) {
                self.sha1 = None;
            }
            if !self.file_status_is(FileStatus::MD5_FROM_DAT) {
                self.md5 = None;
            }

            // If the alt metadata is from the DAT, move it up to be the primary data.
            if self.file_status_is(FileStatus::ALT_SIZE_FROM_DAT) {
                self.size = self.alt_size;
                self.file_status_set(FileStatus::SIZE_FROM_DAT);
            }
            if self.file_status_is(FileStatus::ALT_CRC_FROM_DAT) {
                self.crc = self.alt_crc.clone();
                self.file_status_set(FileStatus::CRC_FROM_DAT);
            }
            if self.file_status_is(FileStatus::ALT_SHA1_FROM_DAT) {
                self.sha1 = self.alt_sha1.clone();
                self.file_status_set(FileStatus::SHA1_FROM_DAT);
            }
            if self.file_status_is(FileStatus::ALT_MD5_FROM_DAT) {
                self.md5 = self.alt_md5.clone();
                self.file_status_set(FileStatus::MD5_FROM_DAT);
            }

            // Remove all alt data.
            self.alt_size = None;
            self.alt_crc = None;
            self.alt_sha1 = None;
            self.alt_md5 = None;

            self.chd_version = None;
            self.file_status_clear(
                FileStatus::DAT_ALT_FLAGS
                    .with(FileStatus::HEADER_FLAGS)
                    .with(FileStatus::VERIFIED_FLAGS),
            );

            self.set_zip_struct(ZipStructure::NONE);
            return EFile::Keep;
        }

        // A directory or archive.
        self.set_zip_struct(ZipStructure::NONE);
        self.test_remove(cx, parent_got)
    }

    /// `TestRemove` (`RvFileCode.cs:368-391`).
    fn test_remove(&mut self, cx: &Cx<'_>, parent_got: GotStatus) -> EFile {
        if !self.file_status_is(FileStatus::DATE_FROM_DAT) {
            self.file_mod_time_stamp = i64::MIN;
        }
        self.file_name.clear();

        let got = if parent_got == GotStatus::FILE_LOCKED {
            GotStatus::FILE_LOCKED
        } else {
            GotStatus::NOT_GOT
        };
        self.set_got_status(got, cx);
        match self.dat_status {
            DatStatus::IN_DAT_COLLECT | DatStatus::IN_DAT_MERGED | DatStatus::IN_DAT_NO_DUMP => {
                EFile::Keep
            }
            // Not in a DAT: the item goes. Anything else cannot happen; ROMVault shows a dialog
            // and keeps the item.
            DatStatus::NOT_IN_DAT | DatStatus::IN_TO_SORT => EFile::Delete,
            _ => EFile::Keep,
        }
    }

    /// `MarkAsMissing` (`RvFileCode.cs:142-169`): every child is removed or marked `NotGot`;
    /// archives and the directories that are not rows of the tree are searched too. `cx` is
    /// this node's context.
    pub fn mark_as_missing(&mut self, cx: &Cx<'_>) {
        let tree = cx.tree_name(&self.name);
        let child_cx = Cx::new(cx.settings, &tree);
        let parent_got = self.got_status;
        let mut i = 0;
        while i < self.children.len() {
            if self.children[i].file_remove(&child_cx, parent_got) == EFile::Delete {
                self.children.remove(i);
                continue;
            }
            let child = &mut self.children[i];
            if child.file_type == FileType::ZIP
                || child.file_type == FileType::SEVEN_ZIP
                || (child.file_type == FileType::DIR && child.tree.is_none())
            {
                child.mark_as_missing(&child_cx);
            }
            i += 1;
        }
    }

    /// `SetAsAltFile` (`RvFileCode.cs:239-268`): the DAT's hashes become the alt hashes, because
    /// the file on disk matched them only without its copier header.
    pub fn set_as_alt_file(&mut self) {
        if self.file_status_is(FileStatus::SIZE_FROM_DAT) {
            self.alt_size = self.size;
            self.file_status_set(FileStatus::ALT_SIZE_FROM_DAT);
        }
        if self.file_status_is(FileStatus::CRC_FROM_DAT) {
            self.alt_crc = self.crc.clone();
            self.file_status_set(FileStatus::ALT_CRC_FROM_DAT);
        }
        if self.file_status_is(FileStatus::SHA1_FROM_DAT) {
            self.alt_sha1 = self.sha1.clone();
            self.file_status_set(FileStatus::ALT_SHA1_FROM_DAT);
        }
        if self.file_status_is(FileStatus::MD5_FROM_DAT) {
            self.alt_md5 = self.md5.clone();
            self.file_status_set(FileStatus::ALT_MD5_FROM_DAT);
        }
        self.size = None;
        self.crc = None;
        self.sha1 = None;
        self.md5 = None;
        self.file_status_clear(
            FileStatus::SIZE_FROM_DAT
                .with(FileStatus::CRC_FROM_DAT)
                .with(FileStatus::SHA1_FROM_DAT)
                .with(FileStatus::MD5_FROM_DAT),
        );
    }

    /// `FileCheckName` (`RvFileCode.cs:271-278`): an item no DAT knows takes the real name; a
    /// DAT item remembers the real name in `file_name` when only the case differs.
    pub fn file_check_name(&mut self, found_name: &str) {
        if self.dat_status == DatStatus::NOT_IN_DAT || self.dat_status == DatStatus::IN_TO_SORT {
            self.name = found_name.to_owned();
        }
        if self.name == found_name {
            self.file_name.clear();
        } else {
            self.file_name = found_name.to_owned();
        }
    }

    /// `DatRemove` (`RvFileDatCode.cs:195-240`): the inverse of merging a DAT in. `Delete` means
    /// nothing is on disk for this item, so the caller removes it.
    pub fn dat_remove(&mut self, cx: &Cx<'_>) -> EFile {
        self.tree = None;
        self.game = None;
        self.set_zip_dat_struct(ZipStructure::NONE, false);
        self.dir_dats.clear();

        // This removes the Required flag, as the DAT is going away.
        let masked = self.header_type();
        self.set_header_type_raw(masked);
        if !self.file_status_is(FileStatus::HEADER_FILE_TYPE_FROM_HEADER)
            && self.file_status_is(FileStatus::HEADER_FILE_TYPE_FROM_DAT)
        {
            self.set_header_type_raw(HeaderFileType::NOTHING);
        }

        if !self.file_status_is(FileStatus::SIZE_FROM_HEADER)
            && !self.file_status_is(FileStatus::SIZE_VERIFIED)
        {
            self.size = None;
        }
        if !self.file_status_is(FileStatus::CRC_FROM_HEADER)
            && !self.file_status_is(FileStatus::CRC_VERIFIED)
        {
            self.crc = None;
        }
        if !self.file_status_is(FileStatus::SHA1_FROM_HEADER)
            && !self.file_status_is(FileStatus::SHA1_VERIFIED)
        {
            self.sha1 = None;
        }
        if !self.file_status_is(FileStatus::MD5_FROM_HEADER)
            && !self.file_status_is(FileStatus::MD5_VERIFIED)
        {
            self.md5 = None;
        }
        // ROMVault does not do this for AltSize and AltCRC (`RvFileDatCode.cs:208`, "Why not do
        // this?"); neither does this.
        if !self.file_status_is(FileStatus::ALT_SHA1_FROM_HEADER)
            && !self.file_status_is(FileStatus::ALT_SHA1_VERIFIED)
        {
            self.alt_sha1 = None;
        }
        if !self.file_status_is(FileStatus::ALT_MD5_FROM_HEADER)
            && !self.file_status_is(FileStatus::ALT_MD5_VERIFIED)
        {
            self.alt_md5 = None;
        }
        if self.file_status_is(FileStatus::DATE_FROM_DAT)
            && self.is_file()
            && !(self.got_status == GotStatus::GOT || self.got_status == GotStatus::CORRUPT)
        {
            self.file_mod_time_stamp = i64::MIN;
        }

        self.mia_clear(MiaStatus::MIA_FROM_DAT);
        self.file_status_clear(FileStatus::DAT_FLAGS);

        self.merge = None;
        self.status = None;
        self.dat_uid = 0;
        self.dat_index = None;

        if self.got_status == GotStatus::NOT_GOT {
            return EFile::Delete;
        }

        if !self.file_name.is_empty() {
            self.name = std::mem::take(&mut self.file_name);
        }
        self.set_dat_status(DatStatus::NOT_IN_DAT, cx);
        EFile::Keep
    }

    /// `IsDeepScanned` (`RvFileCode.cs:550-580`).
    pub fn is_deep_scanned(&self, settings: &Settings) -> bool {
        if self.is_file() {
            if settings.check_chd_version
                && self.header_type() == HeaderFileType::CHD
                && self.chd_version.is_none()
            {
                return false;
            }
            for (from, verified) in [
                (
                    FileStatus::ALT_CRC_FROM_HEADER,
                    FileStatus::ALT_CRC_VERIFIED,
                ),
                (
                    FileStatus::ALT_SHA1_FROM_HEADER,
                    FileStatus::ALT_SHA1_VERIFIED,
                ),
                (
                    FileStatus::ALT_MD5_FROM_HEADER,
                    FileStatus::ALT_MD5_VERIFIED,
                ),
            ] {
                if self.file_status_is(from) && !self.file_status_is(verified) {
                    return false;
                }
            }
            return self.file_status_is(FileStatus::SIZE_VERIFIED)
                && self.file_status_is(FileStatus::CRC_VERIFIED)
                && self.file_status_is(FileStatus::SHA1_VERIFIED)
                && self.file_status_is(FileStatus::MD5_VERIFIED);
        }
        // A directory: every file in it that we have must be deep scanned.
        self.children.iter().all(|c| {
            !(c.is_file() && c.got_status == GotStatus::GOT && !c.is_deep_scanned(settings))
        })
    }

    /// Number of nodes in this subtree whose `RepStatus` is `status`, for tests and reports.
    pub fn count_rep_status(&self, status: RepStatus) -> usize {
        usize::from(self.rep_status == status)
            + self
                .children
                .iter()
                .map(|c| c.count_rep_status(status))
                .sum::<usize>()
    }

    /// `ui_status` of this node, as counted in its parent's [`DirStatus`].
    pub fn ui_rep_status(&self, show_new_mia: bool) -> RepStatus {
        ui_status(self.mia(), self.rep_status, show_new_mia)
    }
}

// ---- the database --------------------------------------------------------------------------

/// A problem found while linking DATs to nodes or back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DbError {
    /// A node's `dat_index` is outside the DAT list it refers to.
    BadDatIndex {
        node: String,
        index: i32,
        list_len: usize,
    },
    /// A node points at a DAT that is not in the list of its nearest ancestor with DATs, so it
    /// cannot be saved.
    DanglingDat { node: String, uid: u32 },
}

impl fmt::Display for DbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DbError::BadDatIndex {
                node,
                index,
                list_len,
            } => write!(
                f,
                "{node}: DAT index {index} is outside a list of {list_len} DATs"
            ),
            DbError::DanglingDat { node, uid } => {
                write!(f, "{node}: DAT {uid} is not in an ancestor's DAT list")
            }
        }
    }
}

impl std::error::Error for DbError {}

/// The tree plus the state that is not in it: which DAT is which, and whether each was matched
/// with a file on disk during the current DAT update.
///
/// `DB.DirRoot` and `DatUpdateStatus` in ROMVault; there they are static.
#[derive(Clone, Debug)]
pub struct Database {
    /// The unnamed root. Child 0 is `RomVault`, the others are ToSort directories
    /// (cache section 1.5).
    pub root: RvFile,
    dat_status: HashMap<u32, DatUpdateStatus>,
    next_dat_uid: u32,
}

impl Database {
    /// The database ROMVault creates when there is no cache (`DB.OpenDefaultDB`).
    pub fn default_db() -> Database {
        Database::from_root(RvFile::default_root()).expect("the default root has no DATs")
    }

    /// Take a tree read from a cache: give every DAT an id and point each node at its DAT
    /// (`dat_index` is resolved against the nearest ancestor that has a DAT list, cache
    /// section 3.4). Every DAT starts as [`DatUpdateStatus::Delete`], like after `DB.Read`.
    pub fn from_root(mut root: RvFile) -> Result<Database, DbError> {
        let mut next = 1u32;
        link(&mut root, &[], &mut next, "")?;
        Ok(Database {
            root,
            dat_status: HashMap::new(),
            next_dat_uid: next,
        })
    }

    /// Write each node's position in its DAT list into `dat_index`, so `root` can be saved.
    /// Call it after any change to the DATs and before taking `root` to a cache file.
    pub fn stamp_dat_indexes(&mut self) -> Result<(), DbError> {
        stamp(&mut self.root, &[], "")
    }

    /// A fresh DAT id.
    pub fn new_dat_uid(&mut self) -> u32 {
        let uid = self.next_dat_uid;
        self.next_dat_uid += 1;
        uid
    }

    /// The update status of a DAT; [`DatUpdateStatus::Delete`] until something sets it.
    pub fn dat_update_status(&self, uid: u32) -> DatUpdateStatus {
        self.dat_status.get(&uid).copied().unwrap_or_default()
    }

    pub fn set_dat_update_status(&mut self, uid: u32, status: DatUpdateStatus) {
        self.dat_status.insert(uid, status);
    }

    /// `RepairStatus.ReportStatusReset(DB.DirRoot)`: every item gets its baseline `RepStatus`.
    /// This is what `DB.Read` leaves behind (cache section 6, step 9) and what both the DAT
    /// update and the scan start with.
    pub fn report_status_reset(&mut self, settings: &Settings) {
        let cx = Cx::new(settings, "");
        self.root.report_status_reset(&cx);
    }
}

fn link(node: &mut RvFile, parent_uids: &[u32], next: &mut u32, path: &str) -> Result<(), DbError> {
    let here = path_combine(path, &node.name);
    node.dat_uid = match node.dat_index {
        Some(i) => match usize::try_from(i).ok().and_then(|i| parent_uids.get(i)) {
            Some(uid) => *uid,
            None => {
                return Err(DbError::BadDatIndex {
                    node: here,
                    index: i,
                    list_len: parent_uids.len(),
                })
            }
        },
        None => 0,
    };
    for dat in &mut node.dir_dats {
        dat.uid = *next;
        *next += 1;
    }
    let own: Vec<u32> = node.dir_dats.iter().map(|d| d.uid).collect();
    let for_children: &[u32] = if own.is_empty() { parent_uids } else { &own };
    for child in &mut node.children {
        link(child, for_children, next, &here)?;
    }
    Ok(())
}

fn stamp(node: &mut RvFile, parent_uids: &[u32], path: &str) -> Result<(), DbError> {
    let here = path_combine(path, &node.name);
    node.dat_index = if node.dat_uid == 0 {
        None
    } else {
        match parent_uids.iter().position(|u| *u == node.dat_uid) {
            Some(i) => Some(i as i32),
            None => {
                return Err(DbError::DanglingDat {
                    node: here,
                    uid: node.dat_uid,
                })
            }
        }
    };
    let own: Vec<u32> = node.dir_dats.iter().map(|d: &RvDat| d.uid).collect();
    let for_children: &[u32] = if own.is_empty() { parent_uids } else { &own };
    for child in &mut node.children {
        stamp(child, for_children, &here)?;
    }
    Ok(())
}
