//! The DAT update: bring the database tree in line with the DATs under `DatRoot`
//! (`RomVaultCore/ReadDat/DatUpdate.cs`, `DatReader.cs`, `docs/dat-loading.md` sections 4 to 9).
//!
//! [`update_dats`] is `DatUpdate.UpdateDat`:
//!
//! 1. every item gets its baseline `RepStatus` again;
//! 2. the user's tree check boxes are remembered;
//! 3. `DatRoot` is walked ([`recursive_dat_tree`]);
//! 4. DATs in the database that are gone or changed on disk are removed, with the items that
//!    came from them ([`remove_old_dats`], clean-up);
//! 5. each new or changed DAT is read, cleaned ([`crate::clean`]) and merged into the tree;
//! 6. tree rows of directories that no DAT claims any more go, and the remembered check boxes
//!    come back.
//!
//! A DAT is "the same" when its name, modification time and the four rule flags that shape it
//! are equal ([`dat_compare`]); a DAT that is the same is not read again, and the items under
//! it are not touched. The rule's compression, merge and filter settings are not part of that
//! test, so a change to them takes effect only for DATs that are re-read (use
//! [`UpdateOptions::invalidate`]).
//!
//! The update owns no state: it takes the database, the settings, a progress sink and a cancel
//! flag. It checks the flag before each DAT, so a cancelled update has merged whole DATs only
//! and the database can be saved.
//!
//! Differences from 3.8.0, on purpose:
//!
//! * A DAT that fails to clean up (see [`crate::clean`]) is reported and skipped; ROMVault stops
//!   the whole update.
//! * A DAT with a parse error is imported as far as it was read, like ROMVault; the messages
//!   are in the report.
//! * `DatRoot` paths use `/`. A cache written on Windows holds `\` in the DAT names and the
//!   comparison treats the two alike.

mod import;
mod node;

use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt;
use std::path::Path;

use rv_core::progress::{CancelFlag, ProgressEvent, ProgressSink};
use rv_core::sorters::{compare_dat_name, compare_name, directory_name_compare, list_search};
use rv_core::{
    dat_data, Cx, DatFlags, DatStatus, DatTable, DatUpdateStatus, Database, DbError, EFile,
    FileType, GotStatus, MetaEntry, MiaStatus, RemoveSubType, RvDat, RvFile, RvTreeRow, Settings,
};

use crate::clean::{self, CleanRule};
use crate::varfix;
use crate::DatNode;

pub use import::{recursive_dat_tree, ImportDat, ImportDir, ImportError};
pub use node::dat_merge_compare;

/// What to do beyond a plain update.
#[derive(Clone, Debug, Default)]
pub struct UpdateOptions {
    /// Force every DAT whose `DatRoot` name starts with this (e.g. `"DatRoot"` for all of them,
    /// `"DatRoot/Console/NES"` for one tree) to be read again, as the "Update All DATs" menu item
    /// and the settings dialogs do (`DatUpdate.InvalidateAllDATs`). The match is a plain prefix,
    /// so `DatRoot/Console/NES` also covers `DatRoot/Console/NESExtra`.
    pub invalidate: Option<String>,
}

/// What an update did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UpdateReport {
    /// DATs found under `DatRoot`.
    pub dats_found: usize,
    /// DATs in the database that matched a file on disk and were left alone.
    pub dats_unchanged: Vec<String>,
    /// `DatRoot` names of DATs read and merged in (new or changed).
    pub dats_added: Vec<String>,
    /// `DatRoot` names of DATs taken out of the database (gone, or changed on disk).
    pub dats_removed: Vec<String>,
    /// DATs that could not be loaded, with the reason. They are tried again next time.
    pub dats_failed: Vec<(String, String)>,
    /// Parse messages and merge warnings, in order.
    pub messages: Vec<String>,
    /// The cancel flag was set; the DATs after it were not processed.
    pub cancelled: bool,
}

/// An update that could not run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateError {
    Import(ImportError),
    Db(DbError),
}

impl fmt::Display for UpdateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UpdateError::Import(e) => write!(f, "{e}"),
            UpdateError::Db(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for UpdateError {}

impl From<ImportError> for UpdateError {
    fn from(e: ImportError) -> Self {
        UpdateError::Import(e)
    }
}

// ---- comparing DATs ------------------------------------------------------------------------

/// `RvDat.SubDirType` (`RvDat.cs:114-124`).
fn rv_sub_dir_type(dat: &RvDat) -> RemoveSubType {
    dat.get(dat_data::SUB_DIR_TYPE)
        .and_then(|s| s.trim().parse::<i32>().ok())
        .and_then(RemoveSubType::from_number)
        .unwrap_or(RemoveSubType::KeepAllSubDirs)
}

fn flag_order(a: bool, b: bool) -> Ordering {
    a.cmp(&b)
}

fn ordering_of(res: i32) -> Ordering {
    res.cmp(&0)
}

fn compare_flags(
    a_flags: DatFlags,
    a_sub: RemoveSubType,
    b_flags: DatFlags,
    b_sub: RemoveSubType,
) -> Ordering {
    [
        DatFlags::MULTI_DATS_IN_DIRECTORY,
        DatFlags::MULTI_DAT_OVERRIDE,
        DatFlags::USE_DESCRIPTION_AS_DIR_NAME,
        DatFlags::SINGLE_ARCHIVE,
    ]
    .iter()
    .map(|f| flag_order(a_flags.contains(*f), b_flags.contains(*f)))
    .find(|o| o.is_ne())
    .unwrap_or_else(|| a_sub.cmp(&b_sub))
}

/// `DBHelper.DatCompare(RvDat, DatImportDat)` (`DBHelper.cs:114-159`): is the DAT in the
/// database the DAT on disk? `Equal` means yes. The order is name, time stamp, then the flags
/// `MultiDatsInDirectory`, `MultiDatOverride`, `UseDescriptionAsDirName`, `SingleArchive` and
/// the sub-directory type; nothing else about the rule counts.
pub fn dat_compare(db: &RvDat, disk: &ImportDat) -> Ordering {
    let name = db.get(dat_data::DAT_ROOT_FULL_NAME).unwrap_or("");
    ordering_of(compare_dat_name(name, &disk.full_name))
        .then(db.time_stamp.cmp(&disk.time_stamp))
        .then(compare_flags(
            db.dat_flags,
            rv_sub_dir_type(db),
            disk.flags,
            disk.sub_dir_type,
        ))
}

/// `DBHelper.DatCompare(RvDat, RvDat)` (`DBHelper.cs:66-111`): the order of a directory's DAT
/// list.
pub fn dat_list_compare(a: &RvDat, b: &RvDat) -> Ordering {
    let an = a.get(dat_data::DAT_ROOT_FULL_NAME).unwrap_or("");
    let bn = b.get(dat_data::DAT_ROOT_FULL_NAME).unwrap_or("");
    ordering_of(compare_dat_name(an, bn))
        .then(a.time_stamp.cmp(&b.time_stamp))
        .then(compare_flags(
            a.dat_flags,
            rv_sub_dir_type(a),
            b.dat_flags,
            rv_sub_dir_type(b),
        ))
}

/// `new RvDat(DatImportDat)` (`RvDat.cs:81-106`): the metadata ids are stored ascending; blank
/// values are not stored; the sub-directory type is always stored.
fn rv_dat_from_import(dat: &ImportDat, uid: u32) -> RvDat {
    let h = dat.header.as_ref().expect("a DAT that was read");
    let mut entries: Vec<MetaEntry> = Vec::new();
    let mut set = |id: u8, value: &str| {
        if value.trim().is_empty() {
            return;
        }
        let pos = entries.iter().take_while(|e| e.id < id).count();
        let entry = MetaEntry {
            id,
            value: value.to_owned(),
        };
        if entries.get(pos).is_some_and(|e| e.id == id) {
            entries[pos] = entry;
        } else {
            entries.insert(pos, entry);
        }
    };
    set(dat_data::ID, &h.id);
    set(dat_data::DAT_NAME, &h.name);
    set(dat_data::DAT_ROOT_FULL_NAME, &dat.full_name);
    set(dat_data::ROOT_DIR, &h.root_dir);
    set(dat_data::DESCRIPTION, &h.description);
    set(dat_data::CATEGORY, &h.category);
    set(dat_data::VERSION, &h.version);
    set(dat_data::DATE, &h.date);
    set(dat_data::AUTHOR, &h.author);
    set(dat_data::EMAIL, &h.email);
    set(dat_data::HOME_PAGE, &h.homepage);
    set(dat_data::URL, &h.url);
    set(dat_data::DIR_SETUP, &h.dir);
    set(dat_data::HEADER, &h.header);
    set(dat_data::COMPRESSION, &h.compression);
    set(
        dat_data::SUB_DIR_TYPE,
        &(dat.sub_dir_type as i32).to_string(),
    );
    let mut out = RvDat::new(dat.time_stamp, dat.flags, entries);
    out.uid = uid;
    out
}

// ---- reading and cleaning one DAT ---------------------------------------------------------

/// `DatReader.ReadInDatFile` (`DatReader.cs:65-293`): read the DAT on disk, clean it with the
/// rule that covers it, and leave it in `dat.header`. `Ok(false)` means the file is not a DAT.
/// Parse messages go to `messages`.
pub fn read_in_dat_file(
    settings: &Settings,
    dat: &mut ImportDat,
    messages: &mut Vec<String>,
) -> Result<bool, clean::CleanError> {
    let full_path = settings.dat_physical_path(&dat.full_name);
    let outcome = crate::read_dat_file(Path::new(&full_path));
    messages.extend(outcome.messages);
    // ROMVault ignores the success flag and imports what was read (`DatReader.cs:76-81`).
    let Some(mut header) = outcome.header else {
        dat.header = None;
        return Ok(false);
    };

    let mut dir_name_rule = match dat.full_name.rfind('/') {
        Some(i) => format!("{}/", &dat.full_name[..i]),
        None => "/".to_owned(),
    };
    let mut extra_dir_name: Option<String> = None;
    if !dat.flag(DatFlags::MULTI_DAT_OVERRIDE)
        && header.dir != "noautodir"
        && (dat.flag(DatFlags::MULTI_DATS_IN_DIRECTORY) || !header.root_dir.is_empty())
    {
        // Auto-add a directory for this DAT.
        let mut extra = String::new();
        if dat.flag(DatFlags::USE_DESCRIPTION_AS_DIR_NAME) && !header.description.trim().is_empty()
        {
            extra = header.description.clone();
        }
        if extra.is_empty() {
            extra = header.root_dir.clone();
        }
        if extra.is_empty() {
            extra = header.name.clone();
        }
        if extra.is_empty() {
            let file = Path::new(&full_path)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            extra = file;
        }
        dir_name_rule.push_str(&varfix::clean_file_name(&extra));
        dir_name_rule.push('/');
        extra_dir_name = Some(extra);
    }

    let rule = settings
        .find_dat_rule(&dir_name_rule)
        .ok_or_else(|| clean::CleanError(format!("no DAT rule covers {dir_name_rule}")))?;
    let clean_rule = CleanRule::from(rule);
    clean::run(&clean_rule, &mut header)
        .map_err(|e| clean::CleanError(format!("Error is DAT {}: {e}", dat.full_name)))?;
    let wrapped = extra_dir_name
        .as_deref()
        .is_some_and(|n| !n.trim().is_empty());
    clean::finish(&clean_rule, &mut header, extra_dir_name.as_deref());
    dat.set_flag(DatFlags::AUTO_ADDED_DIRECTORY, wrapped);
    dat.header_type = rule.header_type;
    dat.header = Some(header);
    Ok(true)
}

// ---- the update -----------------------------------------------------------------------------

struct Run<'a> {
    settings: &'a Settings,
    progress: &'a dyn ProgressSink,
    cancel: &'a CancelFlag,
    dats: &'a mut DatTable,
    report: UpdateReport,
    processed: u64,
    total: u64,
}

/// `DatUpdate.UpdateDat` (`DatUpdate.cs:35-108`). Does not write the cache: the caller saves
/// `db` (after [`Database::stamp_dat_indexes`]) when it is happy, which also is true when the
/// report says `cancelled`.
pub fn update_dats(
    db: &mut Database,
    settings: &Settings,
    options: &UpdateOptions,
    progress: &dyn ProgressSink,
    cancel: &CancelFlag,
) -> Result<UpdateReport, UpdateError> {
    progress.report(ProgressEvent::Text("Clearing DB Status".to_owned()));
    db.report_status_reset(settings);

    let Database { root, dats } = db;
    let Some(rom_vault) = root.children.first_mut() else {
        return Ok(UpdateReport::default());
    };

    let mut tree_rows: HashMap<String, Option<RvTreeRow>> = HashMap::new();
    pre_store_tree_values(rom_vault, "RomVault", &mut tree_rows);

    progress.report(ProgressEvent::Text("Finding Dats".to_owned()));
    let (mut import_root, dat_count) = recursive_dat_tree(settings)?;

    if let Some(prefix) = &options.invalidate {
        invalidate_dats(rom_vault, "RomVault", prefix);
    }

    let mut run = Run {
        settings,
        progress,
        cancel,
        dats,
        report: UpdateReport {
            dats_found: dat_count,
            ..UpdateReport::default()
        },
        processed: 0,
        total: dat_count as u64,
    };

    progress.report(ProgressEvent::Text("Scanning Dats".to_owned()));
    run.remove_old_dats(rom_vault, Some(&import_root));
    // Everything under a removed DAT goes, and every directory is `NotInDat` until it is claimed
    // again below.
    let cx = Cx::new(settings, "");
    run.clean_up_files(rom_vault, &cx);
    run.update_dirs(rom_vault, &mut import_root, "");
    remove_old_tree(rom_vault);
    set_back_tree_values(rom_vault, "RomVault", &tree_rows);

    progress.report(ProgressEvent::Text("Dat Update Complete".to_owned()));
    Ok(run.report)
}

/// `DatUpdate.InvalidateAllDATs` (`DatUpdate.cs:716-747`): make every DAT whose name starts
/// with `prefix` look changed, so the next pass reads it again.
fn invalidate_dats(dir: &mut RvFile, tree_name: &str, prefix: &str) {
    if !dir.is_directory() {
        return;
    }
    let dat_path = rv_core::settings::dat_tree_path(tree_name);
    if !dir.dir_dats.is_empty() && prefix.len() <= dat_path.len() && dat_path.starts_with(prefix) {
        for d in &mut dir.dir_dats {
            d.time_stamp = i64::MAX;
        }
    }
    for dat in &mut dir.dir_dats {
        let name = dat
            .get(dat_data::DAT_ROOT_FULL_NAME)
            .unwrap_or("")
            .replace('\\', "/");
        if prefix.len() <= name.len() && name.starts_with(prefix) {
            dat.time_stamp = i64::MAX;
        }
    }
    for child in &mut dir.children {
        let child_tree = rv_core::settings::path_combine(tree_name, &child.name);
        invalidate_dats(child, &child_tree, prefix);
    }
}

/// `DatTreeStatusStore.PreStoreTreeValue` (`DatTreeStatusStore.cs:74-97`): the check boxes of
/// the tree rows below `RomVault`, by path; a path that occurs twice is ambiguous and is not
/// restored.
fn pre_store_tree_values(
    dir: &RvFile,
    tree_name: &str,
    rows: &mut HashMap<String, Option<RvTreeRow>>,
) {
    for child in &dir.children {
        let child_tree = rv_core::settings::path_combine(tree_name, &child.name);
        if let Some(row) = child.tree {
            match rows.get_mut(&child_tree) {
                Some(slot) => *slot = None,
                None => {
                    rows.insert(child_tree.clone(), Some(row));
                }
            }
        }
        if child.file_type == FileType::DIR {
            pre_store_tree_values(child, &child_tree, rows);
        }
    }
}

/// `DatTreeStatusStore.SetBackTreeValues` (`DatTreeStatusStore.cs:98-122`).
fn set_back_tree_values(
    dir: &mut RvFile,
    tree_name: &str,
    rows: &HashMap<String, Option<RvTreeRow>>,
) {
    for child in &mut dir.children {
        let child_tree = rv_core::settings::path_combine(tree_name, &child.name);
        if let Some(row) = &mut child.tree {
            if let Some(Some(stored)) = rows.get(&child_tree) {
                *row = *stored;
            }
        }
        if child.file_type == FileType::DIR {
            set_back_tree_values(child, &child_tree, rows);
        }
    }
}

/// `DatUpdate.RemoveOldTree` (`DatUpdate.cs:691-708`): a directory that no DAT claims has no
/// tree row.
fn remove_old_tree(dir: &mut RvFile) {
    if !dir.is_directory() {
        return;
    }
    if dir.dat_status == DatStatus::NOT_IN_DAT && dir.tree.is_some() {
        dir.tree = None;
    }
    for child in &mut dir.children {
        remove_old_tree(child);
    }
}

impl Run<'_> {
    fn text(&self, s: String) {
        self.progress.report(ProgressEvent::Text(s));
    }

    fn step(&mut self, what: String) {
        self.processed += 1;
        self.progress.report(ProgressEvent::Overall {
            done: self.processed,
            total: self.total,
        });
        self.text(what);
    }

    /// `DatUpdate.RemoveOldDats` (`DatUpdate.cs:113-236`): compare the DATs of each directory
    /// with the files on disk. A DAT that matches is `Correct`; one that does not is `Delete` and
    /// leaves the list.
    fn remove_old_dats(&mut self, db_dir: &mut RvFile, disk: Option<&ImportDir>) {
        if !db_dir.is_directory() {
            return;
        }
        let disk_dats: &[ImportDat] = disk.map_or(&[], |d| &d.dats);
        let (mut dbi, mut fi) = (0usize, 0usize);
        loop {
            let res = if dbi < db_dir.dir_dats.len() && fi < disk_dats.len() {
                dat_compare(&db_dir.dir_dats[dbi], &disk_dats[fi])
            } else if fi < disk_dats.len() {
                Ordering::Greater
            } else if dbi < db_dir.dir_dats.len() {
                Ordering::Less
            } else {
                break;
            };
            match res {
                Ordering::Equal => {
                    let uid = db_dir.dir_dats[dbi].uid;
                    self.dats.set_status(uid, DatUpdateStatus::Correct);
                    dbi += 1;
                    fi += 1;
                }
                // A DAT that is new on disk (or changed): added later.
                Ordering::Greater => fi += 1,
                Ordering::Less => {
                    let dat = db_dir.dir_dats.remove(dbi);
                    self.dats.set_status(dat.uid, DatUpdateStatus::Delete);
                    self.report.dats_removed.push(
                        dat.get(dat_data::DAT_ROOT_FULL_NAME)
                            .unwrap_or("")
                            .replace('\\', "/"),
                    );
                }
            }
        }

        let disk_dirs: &[ImportDir] = disk.map_or(&[], |d| &d.dirs);
        let (mut dbi, mut fi) = (0usize, 0usize);
        loop {
            let res = if dbi < db_dir.children.len() && fi < disk_dirs.len() {
                directory_name_compare(&db_dir.children[dbi].name, &disk_dirs[fi].name)
            } else if fi < disk_dirs.len() {
                1
            } else if dbi < db_dir.children.len() {
                -1
            } else {
                break;
            };
            match res {
                0 => {
                    self.remove_old_dats(&mut db_dir.children[dbi], Some(&disk_dirs[fi]));
                    dbi += 1;
                    fi += 1;
                }
                1 => fi += 1,
                _ => {
                    let child = &mut db_dir.children[dbi];
                    if child.file_type == FileType::DIR && child.dat_uid == 0 {
                        self.remove_old_dats(child, None);
                    }
                    dbi += 1;
                }
            }
        }
    }

    /// `DatUpdate.RemoveOldDatsCleanUpFiles` (`DatUpdate.cs:240-289`). `cx` is the context of
    /// `node`. Items of a DAT that is `Correct` are left as they are; items of a DAT that was
    /// dropped lose what it gave them; everything else becomes `NotInDat`.
    fn clean_up_files(&mut self, node: &mut RvFile, cx: &Cx<'_>) -> EFile {
        if node.dat_uid != 0 {
            match self.dats.status(node.dat_uid) {
                DatUpdateStatus::Correct => return EFile::Keep,
                DatUpdateStatus::Delete => {
                    if node.dat_remove(cx) == EFile::Delete {
                        return EFile::Delete;
                    }
                }
            }
        }
        if !node.is_directory() {
            return EFile::Keep;
        }
        // Every DatStatus is cleared here; the DATs that are still there set them back below.
        node.set_dat_status(DatStatus::NOT_IN_DAT, cx);
        node.mia_clear(MiaStatus::MIA_FROM_DAT);

        let tree = cx.tree_name(&node.name);
        let child_cx = Cx::new(cx.settings, &tree);
        let mut i = 0;
        while i < node.children.len() {
            if self.clean_up_files(&mut node.children[i], &child_cx) == EFile::Keep {
                i += 1;
            } else {
                node.children.remove(i);
            }
        }
        if (node.file_type == FileType::ZIP || node.file_type == FileType::SEVEN_ZIP)
            && node.got_status == GotStatus::CORRUPT
        {
            return EFile::Keep;
        }
        if node.children.is_empty() {
            EFile::Delete
        } else {
            EFile::Keep
        }
    }

    /// `DatUpdate.UpdateDirs` (`DatUpdate.cs:294-399`). `parent_tree` is the tree name of
    /// `db_dir`'s parent. After a cancel the walk goes on without reading any more DATs, so every
    /// directory is claimed again and the database stays consistent.
    fn update_dirs(&mut self, db_dir: &mut RvFile, disk: &mut ImportDir, parent_tree: &str) {
        let settings = self.settings;
        let cx = Cx::new(settings, parent_tree);
        self.add_the_dats_in_this_dir(db_dir, disk, &cx);

        db_dir.set_dat_status(DatStatus::IN_DAT_COLLECT, &cx);
        if db_dir.tree.is_none() {
            db_dir.tree = Some(RvTreeRow::default());
        }

        let tree = cx.tree_name(&db_dir.name);
        let child_cx = Cx::new(settings, &tree);
        let (mut dbi, mut fi) = (0usize, 0usize);
        loop {
            let res = if dbi < db_dir.children.len() && fi < disk.dirs.len() {
                directory_name_compare(&db_dir.children[dbi].name, &disk.dirs[fi].name)
            } else if fi < disk.dirs.len() {
                1
            } else if dbi < db_dir.children.len() {
                -1
            } else {
                break;
            };
            match res {
                0 => {
                    let child = &mut db_dir.children[dbi];
                    if child.dat_status == DatStatus::IN_DAT_COLLECT {
                        self.report.messages.push(format!(
                            "DAT directory conflict: An auto-created virtual directory is using the same name as a real directory. Resolve the conflict and refresh DATs: {}",
                            rv_core::settings::dat_tree_path(&child_cx.tree_name(&child.name))
                        ));
                    } else {
                        // Take the case of the DAT root's name. The `FileName` branches are
                        // ROMVault's (`DatUpdate.cs:353-366`), including the one that can
                        // never fire.
                        let disk_name = disk.dirs[fi].name.clone();
                        if child.got_status == GotStatus::GOT {
                            if child.name != disk_name {
                                if !child.file_name.is_empty() {
                                    child.file_name = child.name.clone();
                                } else if child.file_name == disk_name {
                                    child.file_name.clear();
                                }
                                child.name = disk_name;
                            }
                        } else {
                            child.name = disk_name;
                        }
                        if db_dir.tree.is_none() {
                            db_dir.tree = Some(RvTreeRow::default());
                        }
                        let child = &mut db_dir.children[dbi];
                        self.update_dirs(child, &mut disk.dirs[fi], &tree);
                    }
                    dbi += 1;
                    fi += 1;
                }
                1 => {
                    let name = disk.dirs[fi].name.clone();
                    db_dir.dat_add_directory(&name, dbi, &child_cx);
                    self.update_dirs(&mut db_dir.children[dbi], &mut disk.dirs[fi], &tree);
                    dbi += 1;
                    fi += 1;
                }
                _ => dbi += 1,
            }
        }
    }

    /// `DatUpdate.AddTheDatsInThisDir` (`DatUpdate.cs:409-486`): the DATs of this directory that
    /// the database does not have yet are read and merged in.
    fn add_the_dats_in_this_dir(&mut self, db_dir: &mut RvFile, disk: &mut ImportDir, cx: &Cx<'_>) {
        let (mut dbi, mut fi) = (0usize, 0usize);
        loop {
            let res = if dbi < db_dir.dir_dats.len() && fi < disk.dats.len() {
                dat_compare(&db_dir.dir_dats[dbi], &disk.dats[fi])
            } else if fi < disk.dats.len() {
                Ordering::Greater
            } else if dbi < db_dir.dir_dats.len() {
                Ordering::Less
            } else {
                break;
            };
            match res {
                Ordering::Equal => {
                    let name = disk.dats[fi].full_name.clone();
                    self.step(format!("Dat : {}", file_stem(&name)));
                    let uid = db_dir.dir_dats[dbi].uid;
                    self.dats.set_status(uid, DatUpdateStatus::Correct);
                    self.report.dats_unchanged.push(name);
                    dbi += 1;
                    fi += 1;
                }
                Ordering::Greater => {
                    if self.cancel.is_cancelled() {
                        self.report.cancelled = true;
                        return;
                    }
                    let name = disk.dats[fi].full_name.clone();
                    self.step(format!("Scanning New Dat : {}", file_stem(&name)));
                    if self.load_new_dat(db_dir, &mut disk.dats[fi], cx) {
                        dbi += 1;
                    }
                    fi += 1;
                }
                Ordering::Less => {
                    // The DATs that are gone were taken out above, so this cannot happen;
                    // ROMVault reports it and loops. Skip the entry instead.
                    self.report
                        .messages
                        .push("ERROR Deleting a DAT that should already be deleted.".to_owned());
                    dbi += 1;
                }
            }
        }
    }

    /// `DatUpdate.LoadNewDat` (`DatUpdate.cs:488-525`).
    fn load_new_dat(&mut self, db_dir: &mut RvFile, dat: &mut ImportDat, cx: &Cx<'_>) -> bool {
        let mut messages = Vec::new();
        let read = read_in_dat_file(self.settings, dat, &mut messages);
        for m in &messages {
            self.progress.report(ProgressEvent::Error {
                file: dat.full_name.clone(),
                message: m.clone(),
            });
        }
        self.report.messages.extend(messages);
        match read {
            Ok(true) => {}
            Ok(false) => {
                self.fail(dat, "Error reading Dat".to_owned());
                return false;
            }
            Err(e) => {
                self.fail(dat, e.to_string());
                return false;
            }
        }
        let empty = dat
            .header
            .as_ref()
            .is_none_or(|h| h.base_dir.children.is_empty());
        if empty {
            // A DAT that cleans down to nothing is not recorded.
            return false;
        }
        if db_dir.tree.is_none() {
            db_dir.tree = Some(RvTreeRow::default());
        }

        let uid = self.dats.new_uid();
        let this_dat = rv_dat_from_import(dat, uid);
        let header = dat.header.as_ref().expect("read above");

        // A dry run first: does the DAT claim names that another DAT already has?
        let mut merge = Merge {
            settings: self.settings,
            dat_uid: uid,
            messages: Vec::new(),
        };
        if let Some(conflict) = merge.merge_in_dat(db_dir, &header.base_dir, cx, true) {
            let other = conflict_name(db_dir, conflict);
            self.fail(
                dat,
                format!(
                    "Dat Merge conflict occured Cache contains {other} new dat {} is trying to use the same directory and so will be ignored.",
                    dat.full_name
                ),
            );
            return false;
        }

        // Add the new DAT to the directory's list, then merge its files in.
        let (_, at) = list_search(&db_dir.dir_dats, |d| match dat_list_compare(&this_dat, d) {
            Ordering::Less => -1,
            Ordering::Equal => 0,
            Ordering::Greater => 1,
        });
        db_dir.dir_dats.insert(at, this_dat);
        self.dats.set_status(uid, DatUpdateStatus::Correct);
        merge.merge_in_dat(db_dir, &header.base_dir, cx, false);
        self.report.messages.extend(merge.messages);
        self.report.dats_added.push(dat.full_name.clone());
        true
    }

    fn fail(&mut self, dat: &ImportDat, why: String) {
        self.progress.report(ProgressEvent::Error {
            file: dat.full_name.clone(),
            message: why.clone(),
        });
        self.report.dats_failed.push((dat.full_name.clone(), why));
    }
}

/// The `DatRootFullName` of the DAT that owns the conflicting item (`conflict = dbChild.Dat`).
fn conflict_name(db_dir: &RvFile, uid: u32) -> String {
    for d in &db_dir.dir_dats {
        if d.uid == uid {
            return d.get(dat_data::DAT_ROOT_FULL_NAME).unwrap_or("").to_owned();
        }
    }
    // The owner lives in a list further up; the uid is still the best name there is.
    format!("DAT {uid}")
}

fn file_stem(name: &str) -> &str {
    let file = name.rsplit('/').next().unwrap_or(name);
    file.rsplit_once('.').map_or(file, |(s, _)| s)
}

struct Merge<'a> {
    settings: &'a Settings,
    dat_uid: u32,
    messages: Vec<String>,
}

impl Merge<'_> {
    /// `DatUpdate.MergeInDat` (`DatUpdate.cs:527-669`): walk the children of `db_dir` and the
    /// DAT's directory side by side in name order.
    ///
    /// * only in the DAT: add it, with nothing on disk behind it;
    /// * in both: if the database item is a plain unknown item (`NotInDat`) the DAT item
    ///   takes it over when [`dat_merge_compare`] says they are the same; otherwise a new item
    ///   is added next to it, and the unknown one is left for the next scan;
    /// * only in the database: untouched.
    ///
    /// With `check_only` nothing changes: it returns the id of the DAT that owns an item the
    /// new DAT would collide with.
    fn merge_in_dat(
        &mut self,
        db_dir: &mut RvFile,
        file_dir: &crate::DatDir,
        cx: &Cx<'_>,
        check_only: bool,
    ) -> Option<u32> {
        let settings = self.settings;
        let tree = cx.tree_name(&db_dir.name);
        let child_cx = Cx::new(settings, &tree);
        let parent_has_tree = db_dir.tree.is_some();
        let parent_got = db_dir.got_status;

        let (mut dbi, mut fi) = (0usize, 0usize);
        loop {
            let res = if dbi < db_dir.children.len() && fi < file_dir.children.len() {
                compare_name(
                    db_dir.children[dbi].file_type,
                    &db_dir.children[dbi].name,
                    file_dir.children[fi].base().file_type,
                    file_dir.children[fi].name(),
                )
            } else if fi < file_dir.children.len() {
                1
            } else if dbi < db_dir.children.len() {
                -1
            } else {
                break;
            };
            let item = file_dir.children.get(fi);

            if res == 0 {
                let item = item.expect("both sides have an item");
                // All the database items that compare equal (they differ only by case).
                let mut twins = 1;
                while dbi + twins < db_dir.children.len()
                    && compare_name(
                        db_dir.children[dbi].file_type,
                        &db_dir.children[dbi].name,
                        db_dir.children[dbi + twins].file_type,
                        &db_dir.children[dbi + twins].name,
                    ) == 0
                {
                    twins += 1;
                }
                for t in 0..twins {
                    let twin = &db_dir.children[dbi + t];
                    if twin.dat_status == DatStatus::NOT_IN_DAT {
                        continue;
                    }
                    if check_only {
                        return Some(db_dir.children[dbi].dat_uid);
                    }
                    self.messages.push(format!(
                        "Unknown Update Dat Status {}",
                        db_dir.children[dbi].dat_status.0
                    ));
                    break;
                }

                if !check_only {
                    let case_test = twins > 1;
                    let mut found = false;
                    for index_case in (if case_test { 0u8 } else { 1u8 })..2 {
                        for t in 0..twins {
                            let at = dbi + t;
                            if db_dir.children[at].search_found {
                                continue;
                            }
                            let Some(alt_match) =
                                dat_merge_compare(&db_dir.children[at], item, index_case)
                            else {
                                continue;
                            };
                            node::dat_merge_in(
                                &mut db_dir.children[at],
                                item,
                                self.dat_uid,
                                alt_match,
                                parent_has_tree,
                                &child_cx,
                                &mut self.messages,
                            );
                            let ft = db_dir.children[at].file_type;
                            if ft == FileType::ZIP
                                || ft == FileType::SEVEN_ZIP
                                || ft == FileType::DIR
                            {
                                if let DatNode::Dir(sub) = item {
                                    self.merge_in_dat(
                                        &mut db_dir.children[at],
                                        sub,
                                        &child_cx,
                                        false,
                                    );
                                }
                            }
                            db_dir.children[at].search_found = true;
                            found = true;
                            break;
                        }
                        if found {
                            break;
                        }
                    }
                    if !found {
                        self.add_new(db_dir, dbi, item, parent_has_tree, &child_cx, parent_got);
                        dbi += 1;
                    }
                }
                dbi += twins;
                fi += 1;
            } else if res == 1 {
                if !check_only {
                    let item = item.expect("the DAT has an item");
                    self.add_new(db_dir, dbi, item, parent_has_tree, &child_cx, parent_got);
                    dbi += 1;
                }
                fi += 1;
            } else {
                dbi += 1;
            }
        }
        None
    }

    /// `DatAdd` then `SetMissingStatus`.
    fn add_new(
        &mut self,
        db_dir: &mut RvFile,
        at: usize,
        item: &DatNode,
        parent_has_tree: bool,
        child_cx: &Cx<'_>,
        parent_got: GotStatus,
    ) {
        let mut new = node::new_from_dat(item, self.dat_uid, parent_has_tree, child_cx);
        node::set_missing_status(&mut new, child_cx, parent_got, &mut self.messages);
        db_dir.child_add_at(new, at);
    }
}

#[cfg(test)]
mod tests;
