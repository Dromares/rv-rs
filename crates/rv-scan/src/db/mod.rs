//! The directory scan: bring the `GotStatus` of the database tree in line with the files on disk
//! (`RomVaultCore/Scanner/FileScanning.cs`, `Populate.cs`, `Compare.cs`,
//! `docs/scanning-and-findfixes.md` section 3).
//!
//! [`scan_files`] is `FileScanning.ScanFiles`:
//!
//! 1. every item gets its baseline `RepStatus` again;
//! 2. the directories to scan are picked: the rows of the tree that are ticked, or the one
//!    named by [`ScanTarget::Dir`];
//! 3. each is listed and walked side by side with the tree (`check_a_dir`): names are paired,
//!    exact case first when a name occurs twice; a pair of loose files is compared by time stamp
//!    and size at level 1, and by hash when that is not enough; archives are opened when they
//!    changed (or at level 3, or when they were never fully hashed); a file nobody knows is
//!    hashed and added as `NotInDat` (`InToSort` below a ToSort directory); an item that has no
//!    file loses what the scanner knew about it but keeps what its DAT said;
//! 4. `RepStatus` is set from `(FileType, DatStatus, GotStatus)` as items change.
//!
//! The scan keeps no state of its own and writes nothing: the caller saves the database.
//! It looks at the cancel flag once per name group, once per directory and once after a listing;
//! a file that is being hashed is finished first, and the database is consistent when the scan
//! returns, cancelled or not.
//!
//! Differences from 3.8.0, on purpose:
//!
//! * No `__RomVault.*.tmp` file is deleted; they are reported and ignored.
//! * Directory existence is checked with the on-disk case of the name, and a directory is
//!   listed with the case it has on disk even before the database knows it (ROMVault looks with
//!   the DAT's case and finds nothing on a case-sensitive file system).
//! * DAT rules are looked up with `/`, so a rule for a sub-directory works for the scan ignore
//!   list and for "files only" (`docs/linux-port.md` section 3.3 lists the `\` that breaks it).
//! * The MD5 comparison of copier-header matches is the right way round, and SNES and PCE
//!   headers are detected (see the crate documentation).
//! * A CHD this crate cannot check (child CHD, unknown codec or version) stays `Got` with its
//!   header hashes unverified; ROMVault 3.8.0 would stop the program.
//! * A scan is not saved every ten minutes; the caller decides when to save.

mod check;
mod merge;
mod populate;
mod scanned;

use std::collections::HashMap;
use std::path::Path;

use rv_core::progress::{CancelFlag, ProgressEvent, ProgressSink};
use rv_core::settings::path_combine;
use rv_core::{
    dat_data, Cx, DatStatus, Database, FileType, GotStatus, RvFile, ScanLevel, Settings, TreeSelect,
};

use check::{to_sort_file_only, Ancestor, Scan};

pub use scanned::ScannedFile;

/// Which directories to scan.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ScanTarget {
    /// Every row of the tree that is ticked or locked (`StartAt == null`).
    #[default]
    Selected,
    /// One tree row, like a right-click scan: its tree path, e.g. `RomVault/Console/NES/Roms`,
    /// whether or not it is ticked, and the ticked rows below it.
    Dir(String),
}

/// What to scan.
#[derive(Clone, Debug, Default)]
pub struct ScanRequest {
    pub level: ScanLevel,
    pub target: ScanTarget,
}

/// What a scan did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScanReport {
    /// Tree paths of the root directories that were looked at.
    pub roots_scanned: Vec<String>,
    /// Root directories that have no directory on disk; their items are now missing.
    pub roots_missing: Vec<String>,
    pub dirs_scanned: usize,
    pub archives_scanned: usize,
    /// Loose files hashed.
    pub files_hashed: usize,
    /// Files and archives that could not be used, with the reason.
    pub errors: Vec<(String, String)>,
    pub cancelled: bool,
    /// A file changed under the scanner (`fileErrorAbort`); the scan stopped there.
    pub aborted: bool,
}

/// A scan that could not start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScanError {
    /// [`ScanTarget::Dir`] names no directory of the tree.
    NoSuchDir(String),
}

impl std::fmt::Display for ScanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScanError::NoSuchDir(p) => write!(f, "no directory {p} in the tree"),
        }
    }
}

impl std::error::Error for ScanError {}

/// A directory to scan, with what the scan needs to know about the way down to it.
struct Root {
    names: Vec<String>,
    tree: String,
    tree_case: String,
    chain: Vec<Ancestor>,
    to_sort_file_only: bool,
}

/// `DBHelper.GetSelectedDirList` (`DBHelper.cs:36-62`): the ticked rows below `dir`. Nothing under
/// a directory that is not `InDatCollect` is looked at (that skips everything below ToSort; its
/// sub-directories are reached from inside the scan of it).
fn selected_dirs(dir: &RvFile, path: &mut Vec<String>, out: &mut Vec<Vec<String>>) {
    for child in &dir.children {
        if dir.dat_status != DatStatus::IN_DAT_COLLECT {
            continue;
        }
        if !child.is_directory() {
            continue;
        }
        let Some(row) = &child.tree else { continue };
        path.push(child.name.clone());
        if row.checked != TreeSelect::UN_SELECTED {
            out.push(path.clone());
        }
        selected_dirs(child, path, out);
        path.pop();
    }
}

fn dir_at<'a>(root: &'a RvFile, names: &[String]) -> Option<&'a RvFile> {
    let mut node = root;
    for n in names {
        let (found, i) = node.child_name_search(FileType::DIR, n);
        if !found {
            return None;
        }
        node = &node.children[i];
    }
    Some(node)
}

fn dir_at_mut<'a>(root: &'a mut RvFile, names: &[String]) -> Option<&'a mut RvFile> {
    let mut node = root;
    for n in names {
        let (found, i) = node.child_name_search(FileType::DIR, n);
        if !found {
            return None;
        }
        node = &mut node.children[i];
    }
    Some(node)
}

fn collect_compression(node: &RvFile, out: &mut HashMap<u32, String>) {
    for d in &node.dir_dats {
        out.insert(d.uid, d.get(dat_data::COMPRESSION).unwrap_or("").to_owned());
    }
    for c in &node.children {
        if c.is_directory() {
            collect_compression(c, out);
        }
    }
}

/// `FileScanning.ScanFiles` (`FileScanning.cs:24-104`). Does not write the cache. `progress`
/// and `cancel` are described in the module documentation.
pub fn scan_files(
    db: &mut Database,
    settings: &Settings,
    request: &ScanRequest,
    progress: &dyn ProgressSink,
    cancel: &CancelFlag,
) -> Result<ScanReport, ScanError> {
    progress.report(ProgressEvent::Text("Clearing DB Status".to_owned()));
    db.report_status_reset(settings);

    // The directories to scan.
    let mut targets: Vec<Vec<String>> = Vec::new();
    match &request.target {
        ScanTarget::Selected => selected_dirs(&db.root, &mut Vec::new(), &mut targets),
        ScanTarget::Dir(path) => {
            let names: Vec<String> = path
                .split('/')
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect();
            let node =
                dir_at(&db.root, &names).ok_or_else(|| ScanError::NoSuchDir(path.clone()))?;
            if node.tree.is_some() {
                targets.push(names.clone());
            }
            let mut below = names.clone();
            let mut found = Vec::new();
            selected_dirs(node, &mut below, &mut found);
            targets.extend(found);
        }
    }

    let mut compression = HashMap::new();
    collect_compression(&db.root, &mut compression);

    let mut roots: Vec<Root> = Vec::new();
    for names in targets {
        let mut chain = Vec::new();
        let mut tree_case = String::new();
        let mut node = &db.root;
        let mut to_sort_file_only_flag = false;
        for (depth, n) in names.iter().enumerate() {
            let (_, i) = node.child_name_search(FileType::DIR, n);
            node = &node.children[i];
            if depth == 0 {
                to_sort_file_only_flag = to_sort_file_only(node.to_sort_type);
            }
            tree_case = path_combine(&tree_case, node.name_case());
            chain.push(Ancestor::of(node, &compression));
        }
        roots.push(Root {
            tree: names.join("/"),
            names,
            tree_case,
            chain,
            to_sort_file_only: to_sort_file_only_flag,
        });
    }

    let mut scan = Scan {
        settings,
        level: request.level,
        progress,
        cancel,
        abort: false,
        report: ScanReport::default(),
        compression,
        chain: Vec::new(),
        to_sort_file_only: false,
        depth: 0,
    };

    let total = roots.len() as u64;
    for (i, root) in roots.iter().enumerate() {
        if cancel.is_cancelled() {
            scan.report.cancelled = true;
            break;
        }
        if scan.abort {
            scan.report.aborted = true;
            break;
        }
        progress.report(ProgressEvent::Overall {
            done: i as u64,
            total,
        });
        let Some(node) = dir_at_mut(&mut db.root, &root.names) else {
            continue;
        };
        let parent_tree = match root.tree.rfind('/') {
            Some(at) => &root.tree[..at],
            None => "",
        };
        let cx = Cx::new(settings, parent_tree);
        let physical = settings.physical_path(&root.tree_case);
        if Path::new(&physical).is_dir() {
            node.set_got_status(GotStatus::GOT, &cx);
            scan.chain = root.chain.clone();
            scan.to_sort_file_only = root.to_sort_file_only;
            scan.depth = 0;
            scan.check_a_dir(node, &root.tree, &root.tree_case);
            scan.report.roots_scanned.push(root.tree.clone());
        } else {
            node.mark_as_missing(&cx);
            scan.report.roots_missing.push(root.tree.clone());
        }
        if scan.abort {
            scan.report.aborted = true;
            break;
        }
    }
    if cancel.is_cancelled() {
        scan.report.cancelled = true;
    }
    progress.report(ProgressEvent::Text("Scan Complete".to_owned()));
    Ok(scan.report)
}
