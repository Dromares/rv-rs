//! Find Fixes (`RomVaultCore/FindFix/*`, `docs/scanning-and-findfixes.md` section 4): decide, for
//! every file the tree knows about, whether it is where it belongs, can be used to fix something,
//! or has to go. Nothing on disk is touched; the result is the `RepStatus` of every item and the
//! [`Groups`] the fixer takes its sources from.
//!
//! Differences from 3.8.0, on purpose:
//!
//! * One thread. The order of the files in a group is the order of the tree, so the source the
//!   fixer picks is the same on every run (3.8.0 builds the groups in parallel).
//! * A wanted file with alt values (not possible after a status reset, but the cache can hold it)
//!   is left out of the groups with a warning; 3.8.0 throws and abandons the whole run.
//! * A corrupt zero-length DAT entry becomes `CorruptCanBeFixed`, which is a status the table
//!   allows; 3.8.0 assigns `CanBeFixed`, which its own table rejects as `Error`.

use std::collections::HashSet;

use rv_core::progress::{CancelFlag, ProgressEvent, ProgressSink};
use rv_core::settings::dat_tree_path;
use rv_core::{
    status_check, DatStatus, Database, FileType, GotStatus, RepStatus, RvFile, Settings, TreeSelect,
};

use crate::dbhelper::{can_fix, full_match, is_zero_length};
use crate::groups::Groups;
use crate::nodepath::{get, get_mut, tree_select, NodePath, Seg};

/// What Find Fixes found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FindReport {
    pub files_got: usize,
    pub files_missing: usize,
    /// Groups, the zero-length group included.
    pub groups: usize,
    /// Things that should not happen and that were stepped over (a status the table rejects).
    pub warnings: Vec<String>,
    pub cancelled: bool,
}

/// The outcome of a Find Fixes run: the groups the fixer needs, and what was found.
#[derive(Clone, Debug)]
pub struct FindFixes {
    pub groups: Groups,
    pub report: FindReport,
}

/// A leaf of the tree as the group checks see it: a copy of the node, with where it is.
#[derive(Clone)]
struct Item {
    path: NodePath,
    select: TreeSelect,
    node: RvFile,
}

impl Item {
    fn load(root: &RvFile, path: &NodePath) -> Option<Item> {
        let node = get(root, path)?;
        let mut leaf = node.clone();
        leaf.children.clear();
        Some(Item {
            path: path.clone(),
            select: tree_select(root, path),
            node: leaf,
        })
    }

    fn locked(&self) -> bool {
        self.select == TreeSelect::LOCKED
    }

    fn rep(&self) -> RepStatus {
        self.node.rep_status
    }
}

/// `RepStatus = x` with the check the setter makes against the status table
/// (`rvFile.cs:353-376`): a value the table does not allow becomes `Error`.
fn assign(item: &mut Item, status: RepStatus, warnings: &mut Vec<String>) {
    let allowed = status_check(
        item.node.file_type,
        item.node.dat_status,
        item.node.got_status,
    );
    if allowed.contains(&status) {
        item.node.rep_status = status;
    } else {
        item.node.rep_status = RepStatus::ERROR;
        warnings.push(format!(
            "{}: {} is not a status a {:?}/{:?}/{:?} item can have",
            item.path,
            status.name(),
            item.node.file_type,
            item.node.dat_status,
            item.node.got_status
        ));
    }
}

/// `FindFixesListCheck.ListCheck` (`FindFixesListCheck.cs:14-288`) over one group.
fn list_check(zero: bool, items: &mut [Item], warnings: &mut Vec<String>) {
    if zero {
        for it in items.iter_mut() {
            if it.locked() {
                continue;
            }
            match it.rep() {
                RepStatus::MISSING => assign(it, RepStatus::CAN_BE_FIXED, warnings),
                RepStatus::CORRUPT => {
                    if it.node.dat_status == DatStatus::IN_DAT_COLLECT {
                        assign(it, RepStatus::CORRUPT_CAN_BE_FIXED, warnings);
                    } else {
                        assign(it, RepStatus::DELETE, warnings);
                    }
                }
                RepStatus::UN_NEEDED
                | RepStatus::UNKNOWN
                | RepStatus::INCOMPLETE_REMOVE
                | RepStatus::IN_TO_SORT => assign(it, RepStatus::DELETE, warnings),
                RepStatus::UN_SCANNED
                | RepStatus::CORRECT
                | RepStatus::NOT_COLLECTED
                | RepStatus::INCOMPLETE
                | RepStatus::IGNORE => {}
                other => {
                    warnings.push(format!("{}: unknown test status {}", it.path, other.name()))
                }
            }
        }
        return;
    }

    let mut missing: Vec<usize> = Vec::new();
    let mut correct: Vec<usize> = Vec::new();
    let mut unneeded: Vec<usize> = Vec::new();
    let mut in_to_sort: Vec<usize> = Vec::new();
    let mut corrupt: Vec<usize> = Vec::new();
    for (i, it) in items.iter().enumerate() {
        match it.rep() {
            RepStatus::UN_SCANNED | RepStatus::NOT_COLLECTED | RepStatus::INCOMPLETE => {}
            RepStatus::IGNORE => {}
            RepStatus::MISSING => missing.push(i),
            RepStatus::CORRECT => correct.push(i),
            RepStatus::CORRUPT => {
                if it.node.dat_status == DatStatus::IN_DAT_COLLECT {
                    missing.push(i);
                } else {
                    corrupt.push(i);
                }
            }
            RepStatus::UN_NEEDED | RepStatus::UNKNOWN | RepStatus::INCOMPLETE_REMOVE => {
                unneeded.push(i)
            }
            RepStatus::IN_TO_SORT => in_to_sort.push(i),
            other => warnings.push(format!("{}: unknown test status {}", it.path, other.name())),
        }
    }
    let all_got: Vec<usize> = correct
        .iter()
        .chain(unneeded.iter())
        .chain(in_to_sort.iter())
        .copied()
        .collect();

    // Step 1: can a wanted file be made from something we have?
    for &m in &missing {
        if items[m].locked() {
            continue;
        }
        for &g in &all_got {
            if !can_fix(&items[m].node, &items[g].node) {
                continue;
            }
            let to = if items[m].rep() == RepStatus::CORRUPT {
                RepStatus::CORRUPT_CAN_BE_FIXED
            } else {
                RepStatus::CAN_BE_FIXED
            };
            assign(&mut items[m], to, warnings);
            break;
        }
        if items[m].rep() == RepStatus::CORRUPT {
            assign(&mut items[m], RepStatus::MOVE_TO_CORRUPT, warnings);
        }
    }

    // Step 2: corrupt files nobody wants. A good copy somewhere means they can go; otherwise
    // they leave for ToSort/Corrupt, unless they are in ToSort already.
    for &c in &corrupt {
        if items[c].locked() {
            continue;
        }
        if !all_got.is_empty() {
            assign(&mut items[c], RepStatus::DELETE, warnings);
        }
        if items[c].rep() == RepStatus::CORRUPT && items[c].node.dat_status != DatStatus::IN_TO_SORT
        {
            assign(&mut items[c], RepStatus::MOVE_TO_CORRUPT, warnings);
        }
    }

    // Step 3: files in the wrong place.
    for &u in &unneeded {
        if missing
            .iter()
            .any(|&m| can_fix(&items[m].node, &items[u].node))
        {
            assign(&mut items[u], RepStatus::NEEDED_FOR_FIX, warnings);
            continue;
        }
        if items[u].locked() {
            continue;
        }
        if correct
            .iter()
            .any(|&c| full_match(&items[u].node, &items[c].node))
        {
            assign(&mut items[u], RepStatus::DELETE, warnings);
            continue;
        }
        if in_to_sort
            .iter()
            .any(|&t| full_match(&items[u].node, &items[t].node))
        {
            assign(&mut items[u], RepStatus::DELETE, warnings);
            continue;
        }
        assign(&mut items[u], RepStatus::MOVE_TO_SORT, warnings);
    }

    // Step 4: files in ToSort.
    for &t in &in_to_sort {
        let needed = missing
            .iter()
            .any(|&m| !items[m].locked() && can_fix(&items[m].node, &items[t].node));
        if needed {
            assign(&mut items[t], RepStatus::NEEDED_FOR_FIX, warnings);
            continue;
        }
        if items[t].locked() {
            continue;
        }
        if correct
            .iter()
            .any(|&c| full_match(&items[t].node, &items[c].node))
        {
            assign(&mut items[t], RepStatus::DELETE, warnings);
        }
    }

    // A file that only has to be renamed inside its own archive must not wait for itself.
    let can_be_fixed: Vec<usize> = (0..items.len())
        .filter(|&i| items[i].rep() == RepStatus::CAN_BE_FIXED)
        .collect();
    for o in 0..items.len() {
        if items[o].rep() != RepStatus::NEEDED_FOR_FIX {
            continue;
        }
        for &i in &can_be_fixed {
            if items[o].path.parent() != items[i].path.parent() {
                continue;
            }
            if !can_fix(&items[i].node, &items[o].node) {
                continue;
            }
            assign(&mut items[o], RepStatus::RENAME, warnings);
        }
    }
}

/// Run [`list_check`] over group `gid`: load its files, check, write the statuses back.
pub(crate) fn check_group(
    db: &mut Database,
    groups: &Groups,
    gid: usize,
    warnings: &mut Vec<String>,
) {
    let g = groups.group(gid);
    let mut items: Vec<Item> = g
        .files
        .iter()
        .filter_map(|p| Item::load(&db.root, p))
        .collect();
    list_check(g.is_zero(), &mut items, warnings);
    for it in items {
        if let Some(n) = get_mut(&mut db.root, &it.path) {
            n.rep_status = it.node.rep_status;
        }
    }
}

fn collect(
    node: &RvFile,
    path: &mut Vec<Seg>,
    selected: bool,
    got: &mut Vec<NodePath>,
    missing: &mut Vec<NodePath>,
) {
    if selected && node.is_file() {
        match node.got_status {
            GotStatus::GOT | GotStatus::CORRUPT => got.push(NodePath(path.clone())),
            GotStatus::NOT_GOT => missing.push(NodePath(path.clone())),
            _ => {}
        }
    }
    if !node.is_directory() {
        return;
    }
    let next = match node.tree {
        Some(row) => row.checked != TreeSelect::UN_SELECTED,
        None => selected,
    };
    for child in &node.children {
        path.push(NodePath::of(child));
        collect(child, path, next, got, missing);
        path.pop();
    }
}

/// `FindFixes.ScanFiles` (`FindFixes.cs:32-156`). Every item gets its baseline status, then the
/// fix statuses; the groups are returned for the fixer. The cancel flag is looked at between
/// phases; a cancelled run leaves every status at its baseline.
pub fn find_fixes(
    db: &mut Database,
    settings: &Settings,
    progress: &dyn ProgressSink,
    cancel: &CancelFlag,
) -> FindFixes {
    let mut report = FindReport::default();
    progress.report(ProgressEvent::Text("Clearing DB Status".to_owned()));
    db.report_status_reset(settings);

    progress.report(ProgressEvent::Text("Getting Selected Files".to_owned()));
    let (mut got, mut missing) = (Vec::new(), Vec::new());
    collect(&db.root, &mut Vec::new(), true, &mut got, &mut missing);
    report.files_got = got.len();
    report.files_missing = missing.len();
    if cancel.is_cancelled() {
        report.cancelled = true;
        return FindFixes {
            groups: Groups::new(),
            report,
        };
    }

    progress.report(ProgressEvent::Text(
        "Group all files by CRC (Sorted)".to_owned(),
    ));
    let mut groups = Groups::new();
    for p in &got {
        if let Some(n) = get(&db.root, p) {
            groups.add_got(n, p);
        }
    }

    progress.report(ProgressEvent::Text(
        "Merging in missing file list".to_owned(),
    ));
    for p in &missing {
        let Some(n) = get(&db.root, p) else { continue };
        if n.alt_size.is_some()
            || n.alt_crc.is_some()
            || n.alt_sha1.is_some()
            || n.alt_md5.is_some()
        {
            report
                .warnings
                .push(format!("{p}: a wanted file has alt values; left out"));
            continue;
        }
        groups.add_missing(n, p);
    }
    if cancel.is_cancelled() {
        db.report_status_reset(settings);
        report.cancelled = true;
        return FindFixes { groups, report };
    }

    progress.report(ProgressEvent::Text("Finding Fixes".to_owned()));
    let mut warnings = Vec::new();
    for gid in 0..groups.groups.len() {
        check_group(db, &groups, gid, &mut warnings);
    }

    clear_partial(db, settings, &groups, &mut warnings);
    to_sort_zero_byte_files_dont_fix(db, &groups);

    report.groups = groups.groups.len();
    report.warnings.extend(warnings);
    progress.report(ProgressEvent::Text(format!(
        "Complete (Unique Files {})",
        report.groups
    )));
    FindFixes { groups, report }
}

/// `ClearPartial.CheckRemovePartial` (`CleanPartial.cs:27-49`), for the rules with
/// `CompleteOnly`: a game that is missing something and has something is partial, so what is
/// there stops being a source or a keeper.
fn clear_partial(
    db: &mut Database,
    settings: &Settings,
    groups: &Groups,
    warnings: &mut Vec<String>,
) {
    let mut recheck: Vec<usize> = Vec::new();
    let mut seen: HashSet<usize> = HashSet::new();
    if let Some(first) = db.root.children.first() {
        let path = NodePath::root().child(first.file_type, &first.name);
        let mut partial: Vec<NodePath> = Vec::new();
        find_partial_sets(&path, first, settings, &mut partial);
        for set in partial {
            let Some(dir) = get_mut(&mut db.root, &set) else {
                continue;
            };
            let mut leaves = Vec::new();
            set_partial(dir, &set, &mut leaves);
            for leaf in leaves {
                if let Some(g) = groups.group_of(&leaf) {
                    if seen.insert(g) {
                        recheck.push(g);
                    }
                }
            }
        }
    }
    for gid in recheck {
        let files = groups.group(gid).files.clone();
        let cx_settings = settings;
        for p in &files {
            let parent_tree = p.parent_tree_name();
            if let Some(n) = get_mut(&mut db.root, p) {
                if n.rep_status == RepStatus::INCOMPLETE
                    || n.rep_status == RepStatus::INCOMPLETE_REMOVE
                {
                    continue;
                }
                let cx = rv_core::Cx::new(cx_settings, &parent_tree);
                n.rep_status_reset(&cx);
            }
        }
        check_group(db, groups, gid, warnings);
    }
}

fn find_partial_sets(path: &NodePath, base: &RvFile, settings: &Settings, out: &mut Vec<NodePath>) {
    let next_select = base
        .tree
        .is_some_and(|row| row.checked == TreeSelect::SELECTED);
    let complete_only = next_select
        && settings
            .find_dat_rule(&format!("{}/", dat_tree_path(&path.tree_name())))
            .is_some_and(|r| r.complete_only);
    for child in &base.children {
        let child_path = path.child(child.file_type, &child.name);
        if child.game.is_some() {
            if complete_only && is_partial(child) {
                out.push(child_path);
            }
        } else {
            find_partial_sets(&child_path, child, settings, out);
        }
    }
}

fn is_partial(dir: &RvFile) -> bool {
    let (mut missing, mut got_or_fixable) = (false, false);
    fn walk(f: &RvFile, missing: &mut bool, got: &mut bool) {
        for c in &f.children {
            if c.is_directory() {
                walk(c, missing, got);
            } else {
                match c.rep_status {
                    RepStatus::MISSING => *missing = true,
                    RepStatus::CORRECT | RepStatus::CAN_BE_FIXED => *got = true,
                    _ => {}
                }
            }
        }
    }
    walk(dir, &mut missing, &mut got_or_fixable);
    missing && got_or_fixable
}

fn set_partial(dir: &mut RvFile, path: &NodePath, leaves: &mut Vec<NodePath>) {
    for c in &mut dir.children {
        let p = path.child(c.file_type, &c.name);
        if c.is_directory() {
            set_partial(c, &p, leaves);
        } else {
            match c.rep_status {
                RepStatus::CAN_BE_FIXED => {
                    c.rep_status = RepStatus::INCOMPLETE;
                    leaves.push(p);
                }
                RepStatus::CORRECT => {
                    c.rep_status = RepStatus::INCOMPLETE_REMOVE;
                    leaves.push(p);
                }
                _ => {}
            }
        }
    }
}

/// `ToSortZeroByteFilesDontFix.Clear` (`ToSortZeroByteFilesDontFix.cs:8-60`): every empty file in
/// ToSort archives was marked `Delete` because empty files need no source. An archive that would
/// not be rewritten for any other reason keeps its empty files, so it is left alone.
fn to_sort_zero_byte_files_dont_fix(db: &mut Database, groups: &Groups) {
    let zero = groups.group(0);
    let mut parents: Vec<NodePath> = Vec::new();
    for p in &zero.files {
        let Some(last) = p.last() else { continue };
        if last.file_type != FileType::FILE_ZIP && last.file_type != FileType::FILE_SEVEN_ZIP {
            continue;
        }
        let Some(n) = get(&db.root, p) else { continue };
        if n.dat_status != DatStatus::IN_TO_SORT {
            continue;
        }
        let parent = p.parent();
        if !parents.contains(&parent) {
            parents.push(parent);
        }
    }
    for parent in parents {
        let Some(archive) = get_mut(&mut db.root, &parent) else {
            continue;
        };
        let mut found_fixable = false;
        let mut found_non_zero = false;
        for c in &archive.children {
            if c.size != Some(0) {
                found_non_zero = true;
            }
            if c.size == Some(0) {
                continue;
            }
            if c.rep_status != RepStatus::IN_TO_SORT && c.rep_status != RepStatus::CORRUPT {
                found_fixable = true;
                break;
            }
        }
        if found_fixable || !found_non_zero {
            continue;
        }
        for c in &mut archive.children {
            if c.size == Some(0) {
                c.rep_status = RepStatus::IN_TO_SORT;
            }
        }
    }
}

/// Is `node` a zero-length wanted or found file (used by the fixer to skip source selection)?
pub fn zero_length(node: &RvFile) -> bool {
    is_zero_length(node)
}
