//! What a fix would do, without doing it (`rv fix --dry-run`).
//!
//! The plan is read from the statuses Find Fixes left on the tree, the way [`crate::perform_fixes`]
//! reads them: nothing is written, the tree is not changed, and no file is opened. It says, per
//! action, how many files and archives the fix will touch, and which archives it will leave alone
//! because it cannot rewrite them ([`Action::Unsupported`]).

use std::collections::BTreeMap;

use rv_core::{FileType, GotStatus, RepStatus, RvFile, Settings, TreeSelect, ZipStructure};

use crate::nodepath::NodePath;

/// What the fix does to one item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Action {
    /// A loose file is made, moved or copied into place.
    MakeFile,
    DeleteFile,
    MoveFileToSort,
    MoveFileToCorrupt,
    RenameFileCase,
    /// An archive is rebuilt.
    RebuildArchive,
    /// An archive in which nothing is wanted moves to ToSort as it is.
    MoveArchiveToSort,
    /// A corrupt archive moves to ToSort/Corrupt as it is.
    MoveArchiveToCorrupt,
    RenameArchiveCase,
    /// An archive that waits until what is needed from it has been used.
    DeferredArchive,
    /// A missing entry is made (inside a rebuilt archive).
    MakeMember,
    /// An entry is dropped from its archive (a proven duplicate, or a rename).
    DropMember,
    MoveMemberToSort,
    MoveMemberToCorrupt,
    /// Fix cannot rewrite this archive (7z, or a structure it has no writer for): it is left as
    /// it is.
    Unsupported,
}

impl Action {
    pub fn name(self) -> &'static str {
        match self {
            Action::MakeFile => "make file",
            Action::DeleteFile => "delete file",
            Action::MoveFileToSort => "move file to ToSort",
            Action::MoveFileToCorrupt => "move file to ToSort/Corrupt",
            Action::RenameFileCase => "rename file (case)",
            Action::RebuildArchive => "rebuild archive",
            Action::MoveArchiveToSort => "move archive to ToSort",
            Action::MoveArchiveToCorrupt => "move archive to ToSort/Corrupt",
            Action::RenameArchiveCase => "rename archive (case)",
            Action::DeferredArchive => "archive waits for its files to be used",
            Action::MakeMember => "make archive entry",
            Action::DropMember => "drop archive entry",
            Action::MoveMemberToSort => "move archive entry to ToSort",
            Action::MoveMemberToCorrupt => "keep archive for corrupt entry",
            Action::Unsupported => "left alone (cannot be rewritten)",
        }
    }
}

/// The plan: a count per action and the items behind them.
#[derive(Clone, Debug, Default)]
pub struct FixPlan {
    pub counts: BTreeMap<Action, usize>,
    pub items: Vec<(Action, NodePath)>,
    /// Wanted files no source was found for.
    pub still_missing: usize,
}

impl FixPlan {
    fn add(&mut self, a: Action, p: &NodePath) {
        *self.counts.entry(a).or_default() += 1;
        self.items.push((a, p.clone()));
    }

    pub fn count(&self, a: Action) -> usize {
        self.counts.get(&a).copied().unwrap_or(0)
    }

    /// Everything the fix would change.
    pub fn total(&self) -> usize {
        self.counts
            .iter()
            .filter(|(a, _)| !matches!(a, Action::Unsupported | Action::DeferredArchive))
            .map(|(_, n)| *n)
            .sum()
    }
}

fn supported(archive: &RvFile) -> bool {
    if archive.file_type != FileType::ZIP {
        return false;
    }
    let own = if matches!(
        archive.dat_status,
        rv_core::DatStatus::NOT_IN_DAT | rv_core::DatStatus::IN_TO_SORT
    ) {
        archive.zip_struct.unwrap_or(ZipStructure::NONE)
    } else {
        archive.zip_dat_struct_value().0
    };
    matches!(own.0, 1..=3)
}

/// The plan for the tree. Find Fixes must have run on `root` just before (the statuses are what
/// it reads).
pub fn plan(root: &RvFile, settings: &Settings) -> FixPlan {
    let mut p = FixPlan::default();
    for c in &root.children {
        let path = NodePath::root().child(c.file_type, &c.name);
        let selected = c.tree.is_some_and(|t| t.checked == TreeSelect::SELECTED);
        walk(c, &path, selected, settings, &mut p);
    }
    p
}

fn walk(dir: &RvFile, path: &NodePath, last: bool, settings: &Settings, plan: &mut FixPlan) {
    let selected = match dir.tree {
        Some(t) => t.checked == TreeSelect::SELECTED,
        None => last,
    };
    for c in &dir.children {
        let cp = path.child(c.file_type, &c.name);
        match c.file_type {
            FileType::DIR => walk(c, &cp, selected, settings, plan),
            FileType::ZIP | FileType::SEVEN_ZIP => {
                if selected {
                    archive(c, &cp, settings, plan);
                }
            }
            FileType::FILE if selected => file(c, &cp, plan),
            _ => {}
        }
    }
}

fn file(c: &RvFile, p: &NodePath, plan: &mut FixPlan) {
    match c.rep_status {
        RepStatus::CAN_BE_FIXED | RepStatus::CORRUPT_CAN_BE_FIXED => plan.add(Action::MakeFile, p),
        RepStatus::DELETE => plan.add(Action::DeleteFile, p),
        RepStatus::MOVE_TO_SORT => plan.add(Action::MoveFileToSort, p),
        RepStatus::MOVE_TO_CORRUPT => plan.add(Action::MoveFileToCorrupt, p),
        RepStatus::CORRECT if !c.file_name.is_empty() => plan.add(Action::RenameFileCase, p),
        RepStatus::MISSING => plan.still_missing += 1,
        _ => {}
    }
}

/// The gate of `FixAZip.FixZip` (`FixAZip.cs:27-95`), as a decision only.
fn archive(a: &RvFile, p: &NodePath, settings: &Settings, plan: &mut FixPlan) {
    let ds = a.dir_status(settings.show_new_mia);
    plan.still_missing += ds.get(RepStatus::MISSING) as usize;
    let (dat_struct, fix) = a.zip_dat_struct_value();
    let retorrentzip = a.got_status == GotStatus::GOT
        && a.dat_status == rv_core::DatStatus::IN_DAT_COLLECT
        && fix
        && a.zip_struct.unwrap_or(ZipStructure::NONE) != dat_struct;

    if a.got_status == GotStatus::CORRUPT
        && a.dat_status != rv_core::DatStatus::IN_TO_SORT
        && !ds.fix_check_files_can_be_fixed()
    {
        plan.add(Action::MoveArchiveToCorrupt, p);
        return;
    }
    if !ds.fix_check_files_can_be_fixed() {
        if ds.fix_check_has_needed_for_fix() {
            plan.add(Action::DeferredArchive, p);
            return;
        }
        if !ds.fix_check_has_files_to_be_removed() && !retorrentzip {
            if !a.file_name.is_empty() {
                plan.add(Action::RenameArchiveCase, p);
            }
            return;
        }
    }
    // Everything unwanted and nothing to make: the archive moves whole.
    let only_unwanted = a.children.iter().all(|c| {
        matches!(
            c.rep_status,
            RepStatus::MOVE_TO_SORT | RepStatus::MISSING | RepStatus::NOT_COLLECTED
        )
    }) && a
        .children
        .iter()
        .any(|c| c.rep_status == RepStatus::MOVE_TO_SORT);
    if only_unwanted {
        plan.add(Action::MoveArchiveToSort, p);
        return;
    }
    // What `rebuild_archive` leaves alone: a structure it cannot write, or an entry that must be
    // copied but is corrupt.
    let corrupt_keeper = a.children.iter().any(|c| {
        matches!(
            c.rep_status,
            RepStatus::CORRECT
                | RepStatus::IN_TO_SORT
                | RepStatus::NEEDED_FOR_FIX
                | RepStatus::CORRUPT
        ) && c.got_status != GotStatus::GOT
    });
    if !supported(a) || corrupt_keeper {
        plan.add(Action::Unsupported, p);
        return;
    }
    plan.add(Action::RebuildArchive, p);
    for c in &a.children {
        let mp = p.child(c.file_type, &c.name);
        match c.rep_status {
            RepStatus::CAN_BE_FIXED | RepStatus::CORRUPT_CAN_BE_FIXED => {
                plan.add(Action::MakeMember, &mp)
            }
            RepStatus::DELETE | RepStatus::RENAME => plan.add(Action::DropMember, &mp),
            RepStatus::MOVE_TO_SORT => plan.add(Action::MoveMemberToSort, &mp),
            RepStatus::MOVE_TO_CORRUPT => plan.add(Action::MoveMemberToCorrupt, &mp),
            _ => {}
        }
    }
}
