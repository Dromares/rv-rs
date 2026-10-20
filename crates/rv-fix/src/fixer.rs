//! Fix ROMs (`RomVaultCore/FixFile/Fix.cs`, `docs/fixing.md` sections 3, 4, 9 and 10): walk the
//! tree in order and make every item what Find Fixes said it should be.
//!
//! What this does differently from 3.8.0, on purpose (`docs/fixing.md` sections 13, 15 and 17):
//!
//! * **Nothing is written at its final name.** Every new file and archive is written to a temp
//!   file next to its place, `fsync`ed, read back and compared with what the DAT wants, and only
//!   then renamed into place. 3.8.0 writes loose files in place and replaces an archive by
//!   deleting it first.
//! * **A source goes only after its copy is on disk.** A file that was only moved is put at its
//!   new path before the old one is removed; across file systems it is copied and read back
//!   first. A file that ToSort or the corrupt folder receives is kept whole.
//! * **The run never exits without saving.** The "impossible" branches that end the process in
//!   3.8.0 are errors here; the first error stops the run, and the caller saves the cache.
//! * **The corrupt folder is found like every other folder**, through the directory mappings, and
//!   a name that is taken gets a number on both the disk and the database (3.8.0 numbers the
//!   file `game1.zip` and records `game.zip1`).
//! * **A corrupt file or archive is never overwritten.** Fixing over one first puts the old bytes
//!   into `ToSort/Corrupt`.

use std::collections::VecDeque;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rv_core::progress::{CancelFlag, ProgressEvent, ProgressSink};
use rv_core::settings::dat_tree_path;
use rv_core::time::ticks_of;
use rv_core::{
    status_check, Cx, DatStatus, Database, EFile, FileType, GotStatus, RepStatus, RvFile,
    ScanLevel, Settings, ToSortDirType, TreeSelect,
};
use rv_scan::db::item;

use crate::dbhelper::{can_fix, is_zero_length};
use crate::error::{FixError, Res};
use crate::fsx::Failpoints;
use crate::groups::Groups;
use crate::nodepath::{get, get_mut, physical, tree_select, NodePath, Seg};
use crate::source::{scanned_from_node, SourceCache};

/// How a fix run behaves.
#[derive(Clone)]
pub struct FixOptions {
    pub failpoints: Failpoints,
    /// Save the cache between items when this much time has passed (`CacheSaveTimePeriod`).
    pub save_every: Option<Duration>,
}

impl Default for FixOptions {
    fn default() -> FixOptions {
        FixOptions {
            failpoints: Failpoints::none(),
            save_every: Some(Duration::from_secs(10 * 60)),
        }
    }
}

/// One thing the fix did, for the log and for the tests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionRow {
    pub action: &'static str,
    /// The tree path of the item that was acted on.
    pub target: String,
    /// The tree path of the source, or a note.
    pub detail: String,
}

/// The `action` values of an [`ActionRow`].
pub mod action {
    pub const COPY: &str = "Copy";
    pub const MOVE: &str = "Move";
    pub const EMPTY: &str = "Empty";
    pub const DELETE: &str = "Delete";
    pub const TO_SORT: &str = "MoveToSort";
    pub const TO_CORRUPT: &str = "MoveToCorrupt";
    pub const REBUILD: &str = "Rebuild";
    pub const ARCHIVE_TO_SORT: &str = "MoveArchiveToSort";
    pub const ARCHIVE_TO_CORRUPT: &str = "MoveArchiveToCorrupt";
    pub const RENAME: &str = "Rename";
    pub const RETORRENTZIP: &str = "Retorrentzip";
}

/// What a fix run did.
#[derive(Debug, Default)]
pub struct FixReport {
    pub rows: Vec<ActionRow>,
    /// Missing files made (`totalFixed`).
    pub fixed: u64,
    /// Items left alone because Fix cannot do what they need, with the reason. Nothing was lost.
    pub skipped: Vec<(String, String)>,
    pub warnings: Vec<String>,
    pub cancelled: bool,
    /// The error that stopped the run, if one did.
    pub error: Option<FixError>,
    /// The cache could not be saved at the end of the run.
    pub save_error: Option<String>,
}

impl FixReport {
    pub fn count(&self, action: &str) -> usize {
        self.rows.iter().filter(|r| r.action == action).count()
    }
}

/// Saves the database; the fixer calls it between items and at the end.
pub type Saver<'a> = &'a mut dyn FnMut(&mut Database) -> Result<(), String>;

pub(crate) struct Fixer<'a> {
    pub db: &'a mut Database,
    pub settings: &'a Settings,
    pub groups: Groups,
    pub queue: VecDeque<NodePath>,
    pub opts: &'a FixOptions,
    pub progress: &'a dyn ProgressSink,
    pub cancel: &'a CancelFlag,
    pub saver: Saver<'a>,
    pub report: FixReport,
    pub last_save: Instant,
    pub total: u64,
    pub cache: SourceCache,
}

/// `Fix.PerformFixes` (`Fix.cs:19-84`). Every `RepStatus` and the groups come from
/// [`crate::find_fixes`], which must have run on this `db` just before.
///
/// The first error stops the run. Whether it stops or not, the cache is saved at the end through
/// `saver`; a cancelled run is a normal one.
pub fn perform_fixes(
    db: &mut Database,
    settings: &Settings,
    groups: Groups,
    opts: &FixOptions,
    progress: &dyn ProgressSink,
    cancel: &CancelFlag,
    saver: Saver<'_>,
) -> FixReport {
    let mut fixer = Fixer {
        db,
        settings,
        groups,
        queue: VecDeque::new(),
        opts,
        progress,
        cancel,
        saver,
        report: FixReport::default(),
        last_save: Instant::now(),
        total: 0,
        cache: SourceCache::default(),
    };
    fixer
        .progress
        .report(ProgressEvent::Text("Fixing Files".to_owned()));
    let total = count_fix(&fixer.db.root, true) as u64;
    fixer
        .progress
        .report(ProgressEvent::Overall { done: 0, total });

    let roots: Vec<(Seg, bool)> = fixer
        .db
        .root
        .children
        .iter()
        .map(|c| {
            (
                NodePath::of(c),
                c.tree.is_some_and(|t| t.checked == TreeSelect::SELECTED),
            )
        })
        .collect();
    for (seg, selected) in roots {
        let path = NodePath(vec![seg]);
        match fixer.fix_dir(&path, selected) {
            Ok(()) => {}
            Err(FixError::Cancelled) => {
                fixer.report.cancelled = true;
                break;
            }
            Err(e) => {
                fixer.progress.report(ProgressEvent::Error {
                    file: path.tree_name(),
                    message: e.to_string(),
                });
                fixer.report.error = Some(e);
                break;
            }
        }
        if fixer.cancel.is_cancelled() {
            fixer.report.cancelled = true;
            break;
        }
    }

    // The database is saved whatever happened above.
    fixer
        .progress
        .report(ProgressEvent::Text("Updating Cache".to_owned()));
    let saved = fixer
        .opts
        .failpoints
        .hit("before_cache_save")
        .map_err(|e| e.to_string())
        .and_then(|()| (fixer.saver)(fixer.db));
    if let Err(e) = saved {
        fixer.report.save_error = Some(e);
    }
    fixer
        .progress
        .report(ProgressEvent::Text("Complete".to_owned()));
    fixer.report
}

/// `Fix.CountFixDir`: how many missing files this fix is expected to make.
fn count_fix(node: &RvFile, selected: bool) -> usize {
    let selected = match node.tree {
        Some(t) => t.checked == TreeSelect::SELECTED,
        None => selected,
    };
    let mut n = 0;
    for c in &node.children {
        match c.file_type {
            FileType::ZIP | FileType::SEVEN_ZIP => {
                if selected {
                    n += c.dir_status(false).count_can_be_fixed() as usize;
                }
            }
            FileType::DIR => n += count_fix(c, selected),
            FileType::FILE if selected && c.rep_status == RepStatus::CAN_BE_FIXED => n += 1,
            _ => {}
        }
    }
    n
}

impl Fixer<'_> {
    // ---- small helpers ---------------------------------------------------------------------

    pub fn node(&self, p: &NodePath) -> Option<&RvFile> {
        get(&self.db.root, p)
    }

    pub fn node_mut(&mut self, p: &NodePath) -> Option<&mut RvFile> {
        get_mut(&mut self.db.root, p)
    }

    pub fn status(&self, p: &NodePath) -> RepStatus {
        self.node(p).map_or(RepStatus::UN_SET, |n| n.rep_status)
    }

    pub fn phys(&self, p: &NodePath) -> PathBuf {
        physical(&self.db.root, self.settings, p)
    }

    pub fn select_of(&self, p: &NodePath) -> TreeSelect {
        tree_select(&self.db.root, p)
    }

    /// A copy of the node without its children.
    pub fn leaf(&self, p: &NodePath) -> Res<RvFile> {
        let n = self
            .node(p)
            .ok_or_else(|| FixError::Logic(format!("{p} is not in the tree")))?;
        let mut c = n.clone();
        c.children.clear();
        Ok(c)
    }

    /// `RepStatus = status` with the table check of the setter.
    pub fn set_status(&mut self, p: &NodePath, status: RepStatus) {
        if let Some(n) = self.node_mut(p) {
            if status_check(n.file_type, n.dat_status, n.got_status).contains(&status) {
                n.rep_status = status;
            } else {
                n.rep_status = RepStatus::ERROR;
            }
        }
    }

    /// Run `f` on the node with the context a status change needs.
    pub fn with_cx<T>(
        &mut self,
        p: &NodePath,
        f: impl FnOnce(&mut RvFile, &Cx<'_>) -> T,
    ) -> Option<T> {
        let settings = self.settings;
        let parent_tree = p.parent_tree_name();
        let cx = Cx::new(settings, &parent_tree);
        self.node_mut(p).map(|n| f(n, &cx))
    }

    pub fn row(&mut self, action: &'static str, target: &NodePath, detail: impl Into<String>) {
        let detail = detail.into();
        self.progress
            .report(ProgressEvent::Item(format!("{action}: {target} {detail}")));
        self.report.rows.push(ActionRow {
            action,
            target: target.tree_name(),
            detail,
        });
    }

    pub fn skip(&mut self, p: &NodePath, why: impl Into<String>) {
        let why = why.into();
        self.progress.report(ProgressEvent::Error {
            file: p.tree_name(),
            message: format!("not changed: {why}"),
        });
        self.report.skipped.push((p.tree_name(), why));
    }

    pub fn fail(&self, step: &str) -> Res<()> {
        self.opts.failpoints.hit(step)
    }

    /// `Fix.CheckDBWrite` (`Fix.cs:281-291`): save between items when the period has passed.
    fn check_db_write(&mut self) -> Res<()> {
        let Some(every) = self.opts.save_every else {
            return Ok(());
        };
        if self.last_save.elapsed() <= every {
            return Ok(());
        }
        self.progress
            .report(ProgressEvent::Text("Saving Cache".to_owned()));
        (self.saver)(self.db)
            .map_err(|e| FixError::FileSystem(format!("saving the cache: {e}")))?;
        self.last_save = Instant::now();
        Ok(())
    }

    /// Remove the node at `p` from its parent and forget it.
    pub fn remove_node(&mut self, p: &NodePath) {
        let parent = p.parent();
        if let (Some(seg), Some(par)) = (p.last().cloned(), get_mut(&mut self.db.root, &parent)) {
            let (found, i) = par.child_name_search(seg.file_type, &seg.name);
            if found {
                par.children.remove(i);
            }
        }
        self.groups.forget(p);
        self.queue.retain(|q| !q.starts_with(p));
    }

    // ---- the walk --------------------------------------------------------------------------

    /// `Fix.FixDir` (`Fix.cs:134-188`).
    fn fix_dir(&mut self, dir: &NodePath, last_selected: bool) -> Res<()> {
        let Some(node) = self.node(dir) else {
            return Ok(());
        };
        let this_selected = match node.tree {
            Some(t) => t.checked == TreeSelect::SELECTED,
            None => last_selected,
        };
        // Fixing adds and removes children, so work from a snapshot.
        let snapshot: Vec<Seg> = node.children.iter().map(NodePath::of).collect();
        if this_selected {
            self.sweep_temps(dir);
        }
        for seg in snapshot {
            let child = dir.child(seg.file_type, &seg.name);
            self.fix_base(&child, this_selected)?;

            // Whatever the fix made deletable is dealt with next, oldest first.
            while let Some(q) = self.queue.front().cloned() {
                self.fix_base(&q, true)?;
                self.queue.pop_front();
            }
            self.progress.report(ProgressEvent::Overall {
                done: self.report.fixed,
                total: self.total,
            });
            if self.cancel.is_cancelled() {
                return Err(FixError::Cancelled);
            }
        }
        if this_selected {
            self.check_delete_file(dir);
        }
        Ok(())
    }

    /// A crash leaves the temp file of the write it stopped (`__<name>.rvtmp`). The original is
    /// never touched before the temp file is complete, so such a file is never the only copy of
    /// anything and is removed the next time the directory is fixed. (3.8.0's `__RomVault.*.tmp`
    /// can be the only copy, and its scanner deletes it, `docs/fixing.md` section 13.)
    fn sweep_temps(&self, dir: &NodePath) {
        let Ok(rd) = fs::read_dir(self.phys(dir)) else {
            return;
        };
        for e in rd.flatten() {
            let name = e.file_name();
            let Some(name) = name.to_str() else { continue };
            if name.starts_with("__") && name.ends_with(".rvtmp") && e.path().is_file() {
                let _ = fs::remove_file(e.path());
            }
        }
    }

    /// `Fix.FixBase` (`Fix.cs:191-278`).
    fn fix_base(&mut self, p: &NodePath, selected: bool) -> Res<()> {
        let Some(node) = self.node(p) else {
            return Ok(());
        };
        if node.rep_status == RepStatus::DELETED {
            return Ok(());
        }
        let file_type = node.file_type;
        let case_differs = !node.file_name.is_empty();
        self.check_db_write()?;
        match file_type {
            FileType::ZIP | FileType::SEVEN_ZIP => {
                if selected {
                    self.fix_zip(p)?;
                }
            }
            FileType::DIR => {
                if selected && case_differs {
                    self.fix_dir_case(p)?;
                }
                self.fix_dir(p, selected)?;
            }
            FileType::FILE if selected => self.fix_file(p)?,
            _ => {}
        }
        Ok(())
    }

    /// A directory that exists with another case than the DAT's takes the DAT's.
    fn fix_dir_case(&mut self, p: &NodePath) -> Res<()> {
        let from = self.phys(p);
        let to = from.with_file_name(&p.last().expect("a directory has a name").name);
        if from != to {
            if to.exists() {
                return Err(FixError::TreeStructureError(format!(
                    "{} exists next to {}.",
                    to.display(),
                    from.display()
                )));
            }
            fs::rename(&from, &to)?;
        }
        if let Some(n) = self.node_mut(p) {
            n.file_name.clear();
        }
        Ok(())
    }

    // ---- shared steps ----------------------------------------------------------------------

    /// `CheckDeleteFile` (`CheckDeleteFile.cs:10-62`): an empty directory goes from the disk
    /// (never recursively, never a root of the tree, never a locked one); an item nobody wants
    /// goes from the tree, and its parent is looked at next.
    pub fn check_delete_file(&mut self, p: &NodePath) {
        if p.is_root() {
            return;
        }
        let show_new_mia = self.settings.show_new_mia;
        let Some(node) = self.node(p) else { return };
        if node.rep_status == RepStatus::DELETED {
            return;
        }
        if node.file_type == FileType::DIR {
            if node.got_status != GotStatus::GOT {
                return;
            }
            let ds = node.dir_status(show_new_mia);
            if ds.has_any_files() || ds.has_fixable() {
                return;
            }
            if self.select_of(p) == TreeSelect::LOCKED {
                return;
            }
            if p.depth() == 1 {
                return;
            }
            let phys = self.phys(p);
            if phys.is_dir() {
                // Not recursive: a directory with something in it that the tree does not know
                // survives, and so does its node.
                let _ = fs::remove_dir(&phys);
                if phys.exists() {
                    return;
                }
            }
        }
        let parent_got = self
            .node(&p.parent())
            .map_or(GotStatus::NOT_GOT, |n| n.got_status);
        let removed = self
            .with_cx(p, |n, cx| n.file_remove(cx, parent_got))
            .unwrap_or(EFile::Keep);
        if removed == EFile::Delete {
            self.remove_node(p);
            self.check_delete_file(&p.parent());
        }
    }

    /// `CheckCreateDirectories` (`CheckCreateDirectories.cs:10-30`).
    pub fn check_create_directories(&mut self, dir: &NodePath) -> Res<()> {
        if dir.is_root() {
            return Ok(());
        }
        let phys = self.phys(dir);
        let got = self.node(dir).map(|n| n.got_status);
        if phys.is_dir() && got == Some(GotStatus::GOT) {
            return Ok(());
        }
        self.check_create_directories(&dir.parent())?;
        fs::create_dir_all(&phys)
            .map_err(|e| FixError::FileSystem(format!("{}: {e}", phys.display())))?;
        let ticks = fs::metadata(&phys).map(|m| ticks_of(&m)).unwrap_or(0);
        self.with_cx(dir, |n, cx| {
            n.set_got_status(GotStatus::GOT, cx);
            n.file_mod_time_stamp = ticks;
        });
        Ok(())
    }

    /// The root-level ToSort directories: `(primary, file only)` (`DB.GetToSortPrimary`,
    /// `DB.GetToSortFileOnly`).
    pub fn to_sort_roots(&self) -> Option<(NodePath, NodePath)> {
        let kids = &self.db.root.children;
        if kids.len() < 2 {
            return None;
        }
        let flagged = |flag: ToSortDirType| {
            kids.iter()
                .skip(1)
                .find(|c| c.to_sort_type.is_some_and(|t| t.contains(flag)))
        };
        let primary = flagged(ToSortDirType::PRIMARY).unwrap_or(&kids[1]);
        let file_only = flagged(ToSortDirType::FILE_ONLY).unwrap_or(primary);
        Some((
            NodePath(vec![NodePath::of(primary)]),
            NodePath(vec![NodePath::of(file_only)]),
        ))
    }

    fn is_file_only(&self, p: &NodePath) -> bool {
        if self.settings.files_only {
            return true;
        }
        let dat_path = format!("{}/", dat_tree_path(&p.parent_tree_name()));
        self.settings
            .find_dat_rule(&dat_path)
            .is_some_and(|r| r.compression == FileType::FILE_ONLY)
    }

    /// Make sure the directory at `dir` (below a ToSort root) exists in the tree and on disk.
    fn ensure_to_sort_dir(&mut self, dir: &NodePath) -> Res<()> {
        let parent = dir.parent();
        let seg = dir.last().cloned().expect("not the root");
        let exists = self.node(dir).is_some();
        if !exists {
            self.with_cx(&parent, |par, cx| {
                let mut d = RvFile::new(FileType::DIR, &seg.name);
                let tree = cx.tree_name(&par.name);
                let child_cx = Cx::new(cx.settings, &tree);
                d.set_dat_status(DatStatus::IN_TO_SORT, &child_cx);
                par.child_add(d);
            });
        }
        let phys = self.phys(dir);
        if !phys.is_dir() {
            fs::create_dir_all(&phys)
                .map_err(|e| FixError::ToSortNotFound(format!("{}: {e}", phys.display())))?;
        }
        let ticks = fs::metadata(&phys).map(|m| ticks_of(&m)).unwrap_or(0);
        self.with_cx(dir, |n, cx| {
            if n.got_status != GotStatus::GOT || n.dat_status != DatStatus::IN_TO_SORT {
                n.set_dat_got_status(DatStatus::IN_TO_SORT, GotStatus::GOT, cx);
            }
            n.file_mod_time_stamp = ticks;
        });
        Ok(())
    }

    /// `CreateToSortDirs` (`CreateToSortDirs.cs:10-104`): the directory under ToSort that mirrors
    /// the one `inp` is in, and a name for it that nothing on disk has.
    pub fn create_to_sort_dirs(&mut self, inp: &NodePath) -> Res<(NodePath, String)> {
        let (primary, file_only) = self
            .to_sort_roots()
            .ok_or_else(|| FixError::ToSortNotFound("the database has no ToSort".to_owned()))?;
        let root = if self.is_file_only(inp) {
            file_only
        } else {
            primary
        };
        let name_case = self
            .node(inp)
            .map(|n| n.name_case().to_owned())
            .ok_or_else(|| FixError::Logic(format!("{inp} is not in the tree")))?;
        self.ensure_to_sort_dir_root(&root)?;
        let mut dir = root;
        if inp.depth() >= 2 {
            for seg in &inp.0[1..inp.depth() - 1] {
                dir = dir.child(FileType::DIR, &seg.name);
                self.ensure_to_sort_dir(&dir)?;
            }
        }
        let out_phys = self.phys(&dir);
        let mut filename = name_case.clone();
        let mut n = 0;
        while out_phys.join(&filename).exists() {
            let (stem, ext) = match name_case.rfind('.') {
                Some(i) => (&name_case[..i], &name_case[i + 1..]),
                None => (name_case.as_str(), ""),
            };
            filename = format!("{stem}_{n}.{ext}");
            n += 1;
        }
        Ok((dir, filename))
    }

    fn ensure_to_sort_dir_root(&mut self, root: &NodePath) -> Res<()> {
        let phys = self.phys(root);
        if !phys.is_dir() {
            fs::create_dir_all(&phys)
                .map_err(|e| FixError::ToSortNotFound(format!("{}: {e}", phys.display())))?;
        }
        let ticks = fs::metadata(&phys).map(|m| ticks_of(&m)).unwrap_or(0);
        self.with_cx(root, |n, cx| {
            if n.got_status != GotStatus::GOT {
                n.set_got_status(GotStatus::GOT, cx);
            }
            n.file_mod_time_stamp = ticks;
        });
        Ok(())
    }

    /// `ToSort/Corrupt` below the primary ToSort, in the tree and on disk (the path 3.8.0 builds
    /// from the bare name, `FixAFile.cs:197`).
    pub fn corrupt_dir(&mut self) -> Res<NodePath> {
        let (primary, _) = self
            .to_sort_roots()
            .ok_or_else(|| FixError::ToSortNotFound("the database has no ToSort".to_owned()))?;
        self.ensure_to_sort_dir_root(&primary)?;
        let dir = primary.child(FileType::DIR, "Corrupt");
        self.ensure_to_sort_dir(&dir)?;
        Ok(dir)
    }

    /// A name in `dir` that nothing on disk has: `name`, then `name1`, `name2`, ... after the
    /// extension (`FixAFile.cs:203-211`).
    pub fn free_name(&self, dir: &NodePath, name: &str) -> String {
        let out = self.phys(dir);
        let mut candidate = name.to_owned();
        let mut n = 0;
        while out.join(&candidate).exists() {
            n += 1;
            candidate = format!("{name}{n}");
        }
        candidate
    }

    /// Add a loose file record to the directory `dir` of the tree, from what the tree knows about
    /// `src`. Returns the new path.
    pub fn add_loose_copy(
        &mut self,
        dir: &NodePath,
        name: &str,
        src: &RvFile,
        got: GotStatus,
    ) -> NodePath {
        let mut sf = scanned_from_node(src, name);
        sf.file_type = FileType::FILE;
        sf.got_status = got;
        let tree = dir.tree_name();
        let settings = self.settings;
        let cx = Cx::new(settings, &tree);
        if let Some(parent) = get_mut(&mut self.db.root, dir) {
            item::add_file(parent, &sf, &cx);
        }
        let path = dir.child(FileType::FILE, name);
        if let Some(n) = get(&self.db.root, &path) {
            let n = n.clone();
            self.groups.add_new(&n, &path);
        }
        path
    }

    /// `FixFileUtils.CheckFilesUsedForFix` (`CheckFilesUsedForFix.cs:9-107`): sources that are no
    /// longer needed become `Delete` and are queued.
    pub fn check_files_used_for_fix(&mut self, used: &[NodePath], checking_a_file: bool) {
        let mut parents: Vec<NodePath> = Vec::new();
        for p in used {
            let Some(node) = self.node(p) else { continue };
            let st = node.rep_status;
            if st != RepStatus::NEEDED_FOR_FIX && !(checking_a_file && st == RepStatus::RENAME) {
                continue;
            }
            let file_type = node.file_type;
            if self.select_of(p) == TreeSelect::LOCKED {
                self.with_cx(p, |n, cx| n.rep_status_reset(cx));
                continue;
            }
            let wanted_elsewhere = self.groups.group_of(p).is_some_and(|g| {
                self.groups
                    .group(g)
                    .files
                    .iter()
                    .any(|f| self.status(f) == RepStatus::CAN_BE_FIXED)
            });
            if wanted_elsewhere {
                continue;
            }
            self.set_status(p, RepStatus::DELETE);
            match file_type {
                FileType::FILE => {
                    if self.status(p) == RepStatus::DELETE && !self.queue.contains(p) {
                        self.queue.push_back(p.clone());
                    }
                }
                FileType::FILE_ZIP | FileType::FILE_SEVEN_ZIP => {
                    let parent = p.parent();
                    if !parents.contains(&parent) {
                        parents.push(parent);
                    }
                }
                _ => {}
            }
        }
        for archive in parents {
            if self.queue.contains(&archive) {
                continue;
            }
            let Some(a) = self.node(&archive) else {
                continue;
            };
            let (mut has_delete, mut has_needed) = (false, false);
            for c in &a.children {
                match c.rep_status {
                    RepStatus::DELETE => has_delete = true,
                    RepStatus::NEEDED_FOR_FIX | RepStatus::RENAME => {
                        has_needed = true;
                        break;
                    }
                    _ => {}
                }
            }
            if has_delete && !has_needed {
                self.queue.push_back(archive);
            }
        }
    }

    /// `FixFileUtils.DoubleCheckDelete` (`DoubleCheckDelete.cs:12-154`): before a file is deleted,
    /// prove that another copy is on disk and unchanged since the last scan.
    pub fn double_check_delete(&self, p: &NodePath) -> Res<()> {
        let deleting = self.leaf(p)?;
        if deleting.dat_status == DatStatus::IN_TO_SORT
            && deleting.name.to_lowercase().ends_with(".cue")
        {
            return Ok(());
        }
        if is_zero_length(&deleting) {
            return Ok(());
        }
        let gid = self.groups.group_of(p).ok_or_else(|| {
            FixError::FindFixesMissingFileGroups(format!("{p} is not in a file group."))
        })?;
        let mut survivor: Option<&NodePath> = None;
        for f in &self.groups.group(gid).files {
            let Some(n) = self.node(f) else { continue };
            if n.got_status != GotStatus::GOT || !can_fix(&deleting, n) {
                continue;
            }
            match n.rep_status {
                RepStatus::DELETE => continue,
                RepStatus::UNKNOWN
                | RepStatus::CORRECT
                | RepStatus::IN_TO_SORT
                | RepStatus::RENAME
                | RepStatus::NEEDED_FOR_FIX
                | RepStatus::MOVE_TO_SORT
                | RepStatus::IGNORE => {
                    survivor = Some(f);
                    break;
                }
                other => {
                    return Err(FixError::Logic(format!(
                        "double check delete: {f} has status {}",
                        other.name()
                    )))
                }
            }
        }
        let Some(keep) = survivor else {
            return Err(FixError::Logic(format!(
                "Double Check Delete could not find the correct file. ({p})"
            )));
        };
        let keep_node = self.node(keep).expect("found above");
        let (container, stamp) = match keep_node.file_type {
            FileType::FILE_ZIP | FileType::FILE_SEVEN_ZIP => {
                let parent = keep.parent();
                let stamp = self
                    .node(&parent)
                    .map_or(i64::MIN, |a| a.file_mod_time_stamp);
                if parent == p.parent() {
                    // The same archive: it is being rebuilt with the copy in it.
                    return self.check_stamp(&self.phys(&parent), stamp, p, keep);
                }
                (parent, stamp)
            }
            FileType::FILE => (keep.clone(), keep_node.file_mod_time_stamp),
            other => {
                return Err(FixError::Logic(format!(
                    "double check delete: {keep} is a {other:?}"
                )))
            }
        };
        let keep_phys = self.phys(&container);
        self.check_stamp(&keep_phys, stamp, p, keep)?;
        let deleting_container = if deleting.file_type == FileType::FILE {
            p.clone()
        } else {
            p.parent()
        };
        if keep_phys == self.phys(&deleting_container) {
            return Err(FixError::TreeStructureError(format!(
                "Delete Check found multiple tree paths to the same file.\n1st Path = {p}\n2nd Path = {keep}"
            )));
        }
        Ok(())
    }

    fn check_stamp(
        &self,
        path: &Path,
        stamp: i64,
        deleting: &NodePath,
        keep: &NodePath,
    ) -> Res<()> {
        let meta = fs::metadata(path).map_err(|_| {
            FixError::RescanNeeded(format!(
                "Deleting {deleting}: the correct file {keep} is not there."
            ))
        })?;
        if ticks_of(&meta) != stamp {
            return Err(FixError::RescanNeeded(format!(
                "Deleting {deleting}: the correct file {keep} has changed."
            )));
        }
        Ok(())
    }

    /// The loose file or archive at `p` must still be what the last scan saw.
    pub fn check_unchanged(&self, p: &NodePath) -> Res<()> {
        let node = self.leaf(p)?;
        let path = self.phys(p);
        let meta = fs::metadata(&path)
            .map_err(|_| FixError::RescanNeeded(format!("{} is not there.", path.display())))?;
        if ticks_of(&meta) != node.file_mod_time_stamp {
            return Err(FixError::RescanNeeded(format!(
                "{} has changed since the last scan.",
                path.display()
            )));
        }
        if node.file_type == FileType::FILE && node.size.is_some() && node.size != Some(meta.len())
        {
            return Err(FixError::RescanNeeded(format!(
                "{} has changed size since the last scan.",
                path.display()
            )));
        }
        Ok(())
    }

    /// The loose files, archives and entries in `p`'s group that are on disk and can make `p`.
    pub fn fix_file_list(&self, p: &NodePath) -> Vec<NodePath> {
        let (Some(gid), Some(target)) = (self.groups.group_of(p), self.node(p)) else {
            return Vec::new();
        };
        self.groups
            .group(gid)
            .files
            .iter()
            .filter(|f| *f != p)
            .filter(|f| {
                self.node(f)
                    .is_some_and(|n| n.got_status == GotStatus::GOT && can_fix(target, n))
            })
            .cloned()
            .collect()
    }

    /// Order the sources of a fix: a loose file first, then zip entries (the ones that were read
    /// through before first), then 7z entries. Ties keep the order of the tree.
    pub fn best_source(&self, list: &[NodePath]) -> Option<NodePath> {
        list.iter()
            .enumerate()
            .min_by_key(|(i, p)| {
                let rank = match self.node(p) {
                    Some(n) if n.file_type == FileType::FILE => 0,
                    Some(n) if n.file_type == FileType::FILE_ZIP => {
                        if n.is_deep_scanned(self.settings) {
                            1
                        } else {
                            2
                        }
                    }
                    _ => 3,
                };
                (rank, *i)
            })
            .map(|(_, p)| p.clone())
    }

    /// Is the source a loose file that may be moved rather than copied (`TestFileMove`,
    /// `MoveFile.cs:80-97`)?
    pub fn can_move_source(&self, src: &NodePath) -> bool {
        let Some(n) = self.node(src) else {
            return false;
        };
        n.file_type == FileType::FILE
            && self.select_of(src) != TreeSelect::LOCKED
            && matches!(
                n.rep_status,
                RepStatus::NEEDED_FOR_FIX
                    | RepStatus::MOVE_TO_SORT
                    | RepStatus::RENAME
                    | RepStatus::MOVE_TO_CORRUPT
            )
    }

    /// Mark the source `Corrupt` (its data did not read back as the database says).
    pub fn mark_source_corrupt(&mut self, src: &NodePath) {
        self.with_cx(src, |n, cx| n.set_got_status(GotStatus::CORRUPT, cx));
    }
}

/// The level a fix reads files back at: everything is hashed.
pub(crate) const READ_BACK: ScanLevel = ScanLevel::Level2;

pub(crate) fn io_err(e: io::Error, path: &Path) -> FixError {
    FixError::FileSystem(format!("{}: {e}", path.display()))
}
