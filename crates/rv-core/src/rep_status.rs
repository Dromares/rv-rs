//! `RepStatus`, the table of statuses an item may have, and the per-directory counters
//! (`RomVaultCore/RepairStatus.cs`, `docs/scanning-and-findfixes.md` sections 1.3 to 1.5).

use crate::enums::{DatStatus, FileType, GotStatus, MiaStatus};

/// `RepStatus` (`RepairStatus.cs:8-61`): what the user sees for an item. Not stored in the
/// cache; it is derived from `(FileType, DatStatus, GotStatus)` by [`status_check`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RepStatus(pub u8);

macro_rules! rep_names {
    ($($name:ident = $val:expr, $text:expr;)*) => {
        impl RepStatus {
            $(pub const $name: RepStatus = RepStatus($val);)*

            /// The C# member name.
            pub const fn name(self) -> &'static str {
                match self.0 {
                    $($val => $text,)*
                    _ => "Invalid",
                }
            }
        }
    };
}

rep_names! {
    ERROR = 0, "Error";
    UN_SET = 1, "UnSet";
    UN_SCANNED = 2, "UnScanned";
    DIR_CORRECT = 3, "DirCorrect";
    DIR_MISSING = 4, "DirMissing";
    DIR_UNKNOWN = 5, "DirUnknown";
    DIR_IN_TO_SORT = 6, "DirInToSort";
    DIR_CORRUPT = 7, "DirCorrupt";
    MISSING = 8, "Missing";
    CORRECT = 9, "Correct";
    NOT_COLLECTED = 10, "NotCollected";
    UN_NEEDED = 11, "UnNeeded";
    UNKNOWN = 12, "Unknown";
    IN_TO_SORT = 13, "InToSort";
    CORRUPT = 14, "Corrupt";
    IGNORE = 15, "Ignore";
    CAN_BE_FIXED = 16, "CanBeFixed";
    MOVE_TO_SORT = 17, "MoveToSort";
    DELETE = 18, "Delete";
    NEEDED_FOR_FIX = 19, "NeededForFix";
    RENAME = 20, "Rename";
    CORRUPT_CAN_BE_FIXED = 21, "CorruptCanBeFixed";
    MOVE_TO_CORRUPT = 22, "MoveToCorrupt";
    DELETED = 23, "Deleted";
    INCOMPLETE_REMOVE = 24, "IncompleteRemove";
    INCOMPLETE = 25, "Incomplete";
    MISSING_MIA = 26, "MissingMIA";
    CORRECT_MIA = 27, "CorrectMIA";
    CAN_BE_FIXED_MIA = 28, "CanBeFixedMIA";
    MISSING_NEW = 29, "MissingNew";
}

impl RepStatus {
    /// `RepStatus.EndValue`, the size of a [`DirStatus`].
    pub const END_VALUE: usize = 30;
}

impl Default for RepStatus {
    /// The initial field value of an `RvFile` (`rvFile.cs:54`).
    fn default() -> RepStatus {
        RepStatus::UN_SET
    }
}

use RepStatus as R;

// Containers (`RepairStatus.cs:80-111`).
const DIR_COLLECT_NOT_GOT: &[RepStatus] = &[R::DIR_MISSING];
const DIR_COLLECT_GOT: &[RepStatus] = &[R::DIR_CORRECT];
const DELETED_ONLY: &[RepStatus] = &[R::DELETED];
const DIR_IN_TO_SORT: &[RepStatus] = &[R::DIR_IN_TO_SORT];
const DIR_UNKNOWN: &[RepStatus] = &[R::DIR_UNKNOWN];
const DIR_CORRUPT: &[RepStatus] = &[R::DIR_CORRUPT];
const UN_SCANNED: &[RepStatus] = &[R::UN_SCANNED];

// Leaves, common to `File`, `FileZip` and `FileSevenZip` (`RepairStatus.cs:121-178`).
const COLLECT_NOT_GOT: &[RepStatus] = &[R::MISSING, R::CAN_BE_FIXED, R::INCOMPLETE];
const COLLECT_GOT: &[RepStatus] = &[
    R::CORRECT,
    R::INCOMPLETE_REMOVE,
    R::MOVE_TO_SORT,
    R::NEEDED_FOR_FIX,
    R::DELETE,
];
const COLLECT_CORRUPT: &[RepStatus] = &[R::CORRUPT, R::MOVE_TO_CORRUPT, R::CORRUPT_CAN_BE_FIXED];
const MERGED_NOT_GOT: &[RepStatus] = &[R::NOT_COLLECTED];
const FILE_MERGED_GOT: &[RepStatus] =
    &[R::UN_NEEDED, R::DELETE, R::MOVE_TO_SORT, R::NEEDED_FOR_FIX];
const ARCHIVED_MERGED_GOT: &[RepStatus] = &[
    R::UN_NEEDED,
    R::DELETE,
    R::MOVE_TO_SORT,
    R::NEEDED_FOR_FIX,
    R::RENAME,
];
const MERGED_CORRUPT: &[RepStatus] = &[R::CORRUPT, R::MOVE_TO_CORRUPT, R::DELETE];
const FILE_TO_SORT_GOT: &[RepStatus] = &[R::IN_TO_SORT, R::IGNORE, R::NEEDED_FOR_FIX, R::DELETE];
const ARCHIVED_TO_SORT_GOT: &[RepStatus] = &[R::IN_TO_SORT, R::NEEDED_FOR_FIX, R::DELETE];
const TO_SORT_CORRUPT: &[RepStatus] = &[R::CORRUPT, R::DELETE];
const FILE_UNKNOWN_GOT: &[RepStatus] = &[
    R::UNKNOWN,
    R::IGNORE,
    R::DELETE,
    R::MOVE_TO_SORT,
    R::NEEDED_FOR_FIX,
    R::RENAME,
];
const ARCHIVED_UNKNOWN_GOT: &[RepStatus] = &[
    R::UNKNOWN,
    R::DELETE,
    R::MOVE_TO_SORT,
    R::NEEDED_FOR_FIX,
    R::RENAME,
];
const UNKNOWN_CORRUPT: &[RepStatus] = &[R::CORRUPT, R::MOVE_TO_CORRUPT, R::DELETE];

/// `RepairStatus.StatusCheck[fileType, datStatus, gotStatus]` (`RepairStatus.cs:70-179`): the
/// statuses an item may have. **The first entry is the baseline** that `RepStatusReset`
/// assigns. An empty slice means "not coded": the item gets [`RepStatus::ERROR`]. That
/// includes every `InDatNoDump` row, which ROMVault comments out, and the retired
/// `InDatMIA` (value 5).
pub fn status_check(
    file_type: FileType,
    dat_status: DatStatus,
    got_status: GotStatus,
) -> &'static [RepStatus] {
    let ds = dat_status;
    let gs = got_status;
    let collect = ds == DatStatus::IN_DAT_COLLECT;
    let merged = ds == DatStatus::IN_DAT_MERGED;
    let to_sort = ds == DatStatus::IN_TO_SORT;
    let not_in_dat = ds == DatStatus::NOT_IN_DAT;
    let (not_got, got, corrupt, locked) = (
        gs == GotStatus::NOT_GOT,
        gs == GotStatus::GOT,
        gs == GotStatus::CORRUPT,
        gs == GotStatus::FILE_LOCKED,
    );
    match file_type {
        FileType::DIR => match () {
            _ if collect && not_got => DIR_COLLECT_NOT_GOT,
            _ if collect && got => DIR_COLLECT_GOT,
            _ if (to_sort || not_in_dat) && not_got => DELETED_ONLY,
            _ if to_sort && got => DIR_IN_TO_SORT,
            _ if not_in_dat && got => DIR_UNKNOWN,
            _ => &[],
        },
        FileType::ZIP | FileType::SEVEN_ZIP => {
            if !(collect || to_sort || not_in_dat) {
                return &[];
            }
            match () {
                _ if locked => UN_SCANNED,
                _ if corrupt => DIR_CORRUPT,
                _ if not_got && collect => DIR_COLLECT_NOT_GOT,
                _ if not_got => DELETED_ONLY,
                _ if got && collect => DIR_COLLECT_GOT,
                _ if got && to_sort => DIR_IN_TO_SORT,
                _ if got => DIR_UNKNOWN,
                _ => &[],
            }
        }
        FileType::FILE | FileType::FILE_ZIP | FileType::FILE_SEVEN_ZIP => {
            let plain = file_type == FileType::FILE;
            if locked && (collect || merged || to_sort || not_in_dat) {
                return UN_SCANNED;
            }
            match () {
                _ if collect && not_got => COLLECT_NOT_GOT,
                _ if collect && got => COLLECT_GOT,
                _ if collect && corrupt => COLLECT_CORRUPT,
                _ if merged && not_got => MERGED_NOT_GOT,
                _ if merged && got && plain => FILE_MERGED_GOT,
                _ if merged && got => ARCHIVED_MERGED_GOT,
                _ if merged && corrupt => MERGED_CORRUPT,
                _ if to_sort && not_got => DELETED_ONLY,
                _ if to_sort && got && plain => FILE_TO_SORT_GOT,
                _ if to_sort && got => ARCHIVED_TO_SORT_GOT,
                _ if to_sort && corrupt => TO_SORT_CORRUPT,
                _ if not_in_dat && not_got => DELETED_ONLY,
                _ if not_in_dat && got && plain => FILE_UNKNOWN_GOT,
                _ if not_in_dat && got => ARCHIVED_UNKNOWN_GOT,
                _ if not_in_dat && corrupt => UNKNOWN_CORRUPT,
                _ => &[],
            }
        }
        _ => &[],
    }
}

/// The baseline status of `(file_type, dat_status, got_status)`: the first table entry, or
/// [`RepStatus::ERROR`] when the combination is not coded (`rvFile.cs:785-786`).
pub fn baseline_status(
    file_type: FileType,
    dat_status: DatStatus,
    got_status: GotStatus,
) -> RepStatus {
    status_check(file_type, dat_status, got_status)
        .first()
        .copied()
        .unwrap_or(RepStatus::ERROR)
}

/// `ReportStatus.UIStatus` (`RepairStatus.cs:249-278`): the status as counted for reporting.
pub fn ui_status(mia: MiaStatus, rep: RepStatus, show_new_mia: bool) -> RepStatus {
    if mia.contains(MiaStatus::MIA_FROM_DAT) {
        match rep {
            RepStatus::MISSING => return RepStatus::MISSING_MIA,
            RepStatus::CAN_BE_FIXED => return RepStatus::CAN_BE_FIXED_MIA,
            RepStatus::CORRECT => return RepStatus::CORRECT_MIA,
            _ => {}
        }
    }
    if show_new_mia && mia.0 & MiaStatus::NEW.0 != 0 && rep == RepStatus::MISSING {
        return RepStatus::MISSING_NEW;
    }
    if mia.0 & MiaStatus::MIA.0 != 0 {
        match rep {
            RepStatus::MISSING => return RepStatus::MISSING_MIA,
            RepStatus::CAN_BE_FIXED => return RepStatus::CAN_BE_FIXED_MIA,
            RepStatus::CORRECT => return RepStatus::CORRECT_MIA,
            _ => {}
        }
    }
    rep
}

/// `ReportStatus`: how many descendants of a directory have each status
/// (`RepairStatus.cs:236-462`). `RvFile::dir_status` builds it. In ROMVault the counters are
/// kept up to date incrementally; a fresh count is the same numbers, minus the negative
/// `UnSet` bucket that a cache load leaves behind (cache section 6, step 9).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirStatus {
    counts: [i32; RepStatus::END_VALUE],
}

impl Default for DirStatus {
    fn default() -> Self {
        DirStatus {
            counts: [0; RepStatus::END_VALUE],
        }
    }
}

impl DirStatus {
    /// Count one item (`RepStatusAddRemove`).
    pub fn add(&mut self, status: RepStatus, mia: MiaStatus, show_new_mia: bool) {
        self.counts[usize::from(ui_status(mia, status, show_new_mia).0)] += 1;
    }

    /// Add the counts of `other` (`RepStatusArrayAddRemove`).
    pub fn add_all(&mut self, other: &DirStatus) {
        for (a, b) in self.counts.iter_mut().zip(other.counts.iter()) {
            *a += *b;
        }
    }

    pub fn get(&self, status: RepStatus) -> i32 {
        self.counts.get(usize::from(status.0)).copied().unwrap_or(0)
    }

    /// Every non-zero count, in status order.
    pub fn non_zero(&self) -> Vec<(RepStatus, i32)> {
        self.counts
            .iter()
            .enumerate()
            .filter(|(_, n)| **n != 0)
            .map(|(i, n)| (RepStatus(i as u8), *n))
            .collect()
    }

    pub fn count_correct(&self) -> i32 {
        self.get(RepStatus::CORRECT) + self.get(RepStatus::CORRECT_MIA)
    }

    pub fn count_mia(&self) -> i32 {
        self.get(RepStatus::MISSING_MIA)
    }

    pub fn count_found_mia(&self) -> i32 {
        self.get(RepStatus::CORRECT_MIA)
    }

    pub fn has_correct(&self) -> bool {
        self.count_correct() > 0
    }

    /// `CountMissing` (`RepairStatus.cs:330-342`).
    pub fn count_missing(&self, include_mia: bool) -> i32 {
        self.get(RepStatus::MISSING)
            + self.get(RepStatus::MISSING_NEW)
            + if include_mia {
                self.get(RepStatus::MISSING_MIA)
            } else {
                0
            }
            + self.get(RepStatus::DIR_CORRUPT)
            + self.get(RepStatus::CORRUPT)
            + self.get(RepStatus::CAN_BE_FIXED)
            + self.get(RepStatus::CAN_BE_FIXED_MIA)
            + self.get(RepStatus::CORRUPT_CAN_BE_FIXED)
            + self.get(RepStatus::MOVE_TO_CORRUPT)
            + self.get(RepStatus::INCOMPLETE)
    }

    pub fn has_missing(&self, include_mia: bool) -> bool {
        self.count_missing(include_mia) > 0
    }

    pub fn has_mia(&self) -> bool {
        self.count_mia() > 0
    }

    /// `CountFixesNeeded` (`RepairStatus.cs:354-364`).
    pub fn count_fixes_needed(&self) -> i32 {
        self.get(RepStatus::CAN_BE_FIXED)
            + self.get(RepStatus::CAN_BE_FIXED_MIA)
            + self.get(RepStatus::MOVE_TO_SORT)
            + self.get(RepStatus::DELETE)
            + self.get(RepStatus::NEEDED_FOR_FIX)
            + self.get(RepStatus::RENAME)
            + self.get(RepStatus::CORRUPT_CAN_BE_FIXED)
            + self.get(RepStatus::MOVE_TO_CORRUPT)
    }

    pub fn has_fixes_needed(&self) -> bool {
        self.count_fixes_needed() > 0
    }

    pub fn count_can_be_fixed(&self) -> i32 {
        self.get(RepStatus::CAN_BE_FIXED)
            + self.get(RepStatus::CAN_BE_FIXED_MIA)
            + self.get(RepStatus::CORRUPT_CAN_BE_FIXED)
    }

    fn count_fixable(&self) -> i32 {
        self.count_can_be_fixed()
            + self.get(RepStatus::MOVE_TO_SORT)
            + self.get(RepStatus::DELETE)
            + self.get(RepStatus::MOVE_TO_CORRUPT)
    }

    pub fn has_fixable(&self) -> bool {
        self.count_fixable() > 0
    }

    pub fn fix_check_files_can_be_fixed(&self) -> bool {
        self.count_can_be_fixed() > 0
    }

    pub fn fix_check_has_needed_for_fix(&self) -> bool {
        self.get(RepStatus::NEEDED_FOR_FIX) > 0
    }

    pub fn fix_check_has_files_to_be_removed(&self) -> bool {
        self.get(RepStatus::MOVE_TO_SORT)
            + self.get(RepStatus::DELETE)
            + self.get(RepStatus::MOVE_TO_CORRUPT)
            > 0
    }

    /// `CountAnyFiles` (`RepairStatus.cs:421-436`).
    pub fn count_any_files(&self) -> i32 {
        self.get(RepStatus::CORRECT)
            + self.get(RepStatus::CORRECT_MIA)
            + self.get(RepStatus::UN_NEEDED)
            + self.get(RepStatus::UNKNOWN)
            + self.get(RepStatus::IN_TO_SORT)
            + self.get(RepStatus::CORRUPT)
            + self.get(RepStatus::IGNORE)
            + self.get(RepStatus::MOVE_TO_SORT)
            + self.get(RepStatus::DELETE)
            + self.get(RepStatus::NEEDED_FOR_FIX)
            + self.get(RepStatus::RENAME)
            + self.get(RepStatus::MOVE_TO_CORRUPT)
    }

    pub fn has_any_files(&self) -> bool {
        self.count_any_files() > 0
    }

    pub fn has_all_merged(&self) -> bool {
        self.get(RepStatus::NOT_COLLECTED) > 0 && self.count_any_files() == 0
    }

    pub fn count_unknown(&self) -> i32 {
        self.get(RepStatus::UNKNOWN)
    }

    pub fn has_unknown(&self) -> bool {
        self.count_unknown() > 0
    }

    pub fn count_in_to_sort(&self) -> i32 {
        self.get(RepStatus::IN_TO_SORT)
    }

    pub fn has_in_to_sort(&self) -> bool {
        self.count_in_to_sort() > 0
    }
}
