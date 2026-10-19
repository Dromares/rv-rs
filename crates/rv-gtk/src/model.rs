//! What the windows show, as plain data: the tree rows, the game list of a directory and the ROM
//! list of a game. No GTK in here, so it is tested without a display.
//!
//! Everything comes from the database tree in `rv-core` (its `RepStatus`, `DirStatus` and
//! `RvFile` fields); this module only arranges it the way ROMVault's main window does
//! (`ROMVault/rvTree.cs`, `FrmMain_GameGrid.cs`, `FrmMain_RomGrid.cs`). It does not decide what a
//! status is, hash anything or touch a file.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;

use rv_core::{
    dat_data, game_data, DatFlags, DatStatus, DirStatus, FileStatus, FileType, GotStatus,
    HeaderFileType, MiaStatus, RepStatus, RvFile, ToSortDirType, TreeSelect, ZipStructure,
};

/// An integer with thousands separators (`ToRvString`).
pub fn grouped(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 {
        format!("-{out}")
    } else {
        out
    }
}

/// Lower-case hex of a hash, empty when there is none (`ToHexString`).
pub fn hex(bytes: &Option<Vec<u8>>) -> String {
    bytes
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `CompressUtils.zipDateTimeToString` (`Compress/CompressUtils.cs:153-181`): .NET ticks, or a
/// packed DOS date and time, as `YYYY/MM/DD HH:MM:SS`. Empty for "not set".
pub fn time_text(stamp: i64) -> String {
    if stamp == 0 || stamp == i64::MIN {
        return String::new();
    }
    if stamp > 0xffff_ffff {
        // .NET ticks since 0001-01-01.
        const TICKS_PER_SECOND: i64 = 10_000_000;
        const DAYS_TO_1970: i64 = 719_162;
        let secs = stamp / TICKS_PER_SECOND;
        let days = secs.div_euclid(86_400) - DAYS_TO_1970;
        let rem = secs.rem_euclid(86_400);
        let (y, m, d) = civil_from_days(days);
        return format!(
            "{y:04}/{m:02}/{d:02} {:02}:{:02}:{:02}",
            rem / 3600,
            rem % 3600 / 60,
            rem % 60
        );
    }
    let date = (stamp >> 16) & 0xffff;
    let time = stamp & 0xffff;
    format!(
        "{:04}/{:02}/{:02} {:02}:{:02}:{:02}",
        ((date >> 9) & 0x7f) + 1980,
        (date >> 5) & 0x0f,
        date & 0x1f,
        (time >> 11) & 0x1f,
        (time >> 5) & 0x3f,
        (time & 0x1f) << 1
    )
}

/// Days since 1970-01-01 to a proleptic Gregorian date (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

// ---- the tree --------------------------------------------------------------------------

/// Which of ROMVault's two folder icon families a tree row uses (`rvTree.cs:268-285`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FolderKind {
    /// A directory above the DATs (`DirectoryTree1..5`).
    Directory,
    /// A directory a DAT made or that holds DATs (`Tree1..5`).
    Dat,
}

/// The five folder icon states (`rvTree.cs:251-266`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeIcon {
    /// 1: nothing of it is correct and something is missing.
    Missing,
    /// 2: the default, partly there.
    Partial,
    /// 3: nothing is missing.
    Complete,
    /// 4: files in ToSort.
    ToSort,
    /// 5: nothing missing but MIA ROMs are.
    Mia,
}

/// `rvTree.cs:251-266`, in that order of precedence.
pub fn tree_icon(status: &DirStatus) -> TreeIcon {
    if status.has_in_to_sort() {
        TreeIcon::ToSort
    } else if !status.has_correct() && status.has_missing(false) {
        TreeIcon::Missing
    } else if !status.has_missing(false) && status.has_mia() {
        TreeIcon::Mia
    } else if !status.has_missing(false) {
        TreeIcon::Complete
    } else {
        TreeIcon::Partial
    }
}

/// One row of the DAT tree.
#[derive(Debug)]
pub struct DirNode {
    pub name: String,
    /// Names from the root down to this directory, e.g. `["RomVault", "Console", "NES"]`.
    pub path: Vec<String>,
    pub kind: FolderKind,
    /// What the row says before the counts: the name, plus the DAT description when exactly one
    /// DAT sits here, plus the ToSort role.
    pub label: String,
    /// `( Have: n \ Missing: n )`, or `(Files: n)` in ToSort.
    pub counts: String,
    /// All the DATs of a directory with several, for the tooltip.
    pub dats: Vec<String>,
    /// The tick box and the open state change while the window is up; the rest of a row does not.
    pub checked: AtomicU8,
    pub expanded: AtomicBool,
    /// ToSort sets the checkbox aside: it can not be locked (`rvTree.cs:458-459`).
    pub is_to_sort_primary_or_cache: bool,
    pub status: DirStatus,
    pub icon: TreeIcon,
    pub children: Vec<Arc<DirNode>>,
}

impl DirNode {
    pub fn checked(&self) -> TreeSelect {
        TreeSelect(self.checked.load(Ordering::Relaxed))
    }

    pub fn set_checked(&self, value: TreeSelect) {
        self.checked.store(value.0, Ordering::Relaxed);
    }

    pub fn expanded(&self) -> bool {
        self.expanded.load(Ordering::Relaxed)
    }

    pub fn set_expanded(&self, value: bool) {
        self.expanded.store(value, Ordering::Relaxed);
    }

    /// The tree path as ROMVault writes it, `RomVault/Console/NES`.
    pub fn tree_path(&self) -> String {
        self.path.join("/")
    }
}

fn dat_title(dat: &rv_core::RvDat) -> String {
    let title = dat.get(dat_data::DESCRIPTION).unwrap_or("").trim();
    if title.is_empty() {
        dat.get(dat_data::DAT_NAME).unwrap_or("").to_owned()
    } else {
        title.to_owned()
    }
}

/// The labels of a tree row (`rvTree.cs:312-415`).
fn labels(node: &RvFile, parent_dat_count: usize, status: &DirStatus) -> (String, String) {
    let dat_count = node.dir_dats.len();
    let has_dat = node.dat_uid != 0;
    let mut label = if !has_dat && dat_count == 1 {
        let desc = node.dir_dats[0].get(dat_data::DESCRIPTION).unwrap_or("");
        format!("{}: {}", node.name, desc)
    } else if has_dat
        && node
            .dir_dats
            .first()
            .is_some_and(|d| d.dat_flags.contains(DatFlags::AUTO_ADDED_DIRECTORY))
        && parent_dat_count > 1
    {
        format!("{}: ", node.name)
    } else {
        node.name.clone()
    };
    let in_to_sort = node.dat_status == DatStatus::IN_TO_SORT;
    let to_sort = node.to_sort_type.unwrap_or(ToSortDirType::NONE);
    if to_sort.contains(ToSortDirType::PRIMARY) {
        label.push_str(" (Primary)");
    } else if to_sort.contains(ToSortDirType::CACHE) {
        label.push_str(" (Cache)");
    } else if to_sort.contains(ToSortDirType::FILE_ONLY) {
        label.push_str(" (File Only)");
    }
    let counts = if in_to_sort {
        format!("(Files: {})", grouped(i64::from(status.count_in_to_sort())))
    } else {
        let mia = status.count_mia();
        let found = status.count_found_mia();
        format!(
            "( Have: {}{} \\ Missing: {}{} )",
            grouped(i64::from(status.count_correct())),
            if found > 0 {
                format!(" \\ Found MIA: {}", grouped(i64::from(found)))
            } else {
                String::new()
            },
            grouped(i64::from(status.count_missing(false))),
            if mia > 0 {
                format!(" \\ MIA: {}", grouped(i64::from(mia)))
            } else {
                String::new()
            },
        )
    };
    (label, counts)
}

struct Walk {
    show_new_mia: bool,
}

impl Walk {
    /// Count everything below `node` and build the tree rows among it. One pass, each node once.
    fn visit(
        &self,
        node: &RvFile,
        path: &mut Vec<String>,
        out: &mut Vec<Arc<DirNode>>,
    ) -> DirStatus {
        let mut total = DirStatus::default();
        let parent_dats = node.dir_dats.len();
        for child in &node.children {
            total.add(child.rep_status, child.mia(), self.show_new_mia);
            if child.file_type == FileType::DIR && child.tree.is_some() {
                path.push(child.name.clone());
                let mut kids = Vec::new();
                let status = self.visit(child, path, &mut kids);
                let (label, counts) = labels(child, parent_dats, &status);
                let tree = child.tree.unwrap_or_default();
                let to_sort = child.to_sort_type.unwrap_or(ToSortDirType::NONE);
                out.push(Arc::new(DirNode {
                    name: child.name.clone(),
                    path: path.clone(),
                    kind: if child.dat_uid == 0 && child.dir_dats.is_empty() {
                        FolderKind::Directory
                    } else {
                        FolderKind::Dat
                    },
                    label,
                    counts,
                    dats: if child.dir_dats.len() > 1 {
                        child
                            .dir_dats
                            .iter()
                            .filter(|d| !d.dat_flags.contains(DatFlags::AUTO_ADDED_DIRECTORY))
                            .map(dat_title)
                            .collect()
                    } else {
                        Vec::new()
                    },
                    checked: AtomicU8::new(tree.checked.0),
                    expanded: AtomicBool::new(tree.expanded),
                    is_to_sort_primary_or_cache: to_sort.contains(ToSortDirType::PRIMARY)
                        || to_sort.contains(ToSortDirType::CACHE),
                    icon: tree_icon(&status),
                    status: status.clone(),
                    children: kids,
                }));
                path.pop();
                total.add_all(&status);
            } else if !child.children.is_empty() {
                // An archive or a directory without a tree row: counted, never shown.
                let mut none = Vec::new();
                let below = self.visit(child, path, &mut none);
                total.add_all(&below);
            }
        }
        total
    }
}

/// The rows at the top of the tree (ROMVault shows the children of the root: `RomVault`,
/// `ToSort`), each with its counts, built in one pass over the database. A row is a `Dir` with a
/// tree row; the counts of a row are those of everything below it, each node counted once as its
/// `ui_status`, which is what `RvFile::dir_status` gives for it.
pub fn build_tree(root: &RvFile, show_new_mia: bool) -> Vec<Arc<DirNode>> {
    let mut rows = Vec::new();
    Walk { show_new_mia }.visit(root, &mut Vec::new(), &mut rows);
    rows
}

// ---- finding and changing a row ------------------------------------------------------------

/// The directory at `path` (names from the root, as in [`DirNode::path`]).
pub fn dir_at<'a>(root: &'a RvFile, path: &[String]) -> Option<&'a RvFile> {
    let mut node = root;
    for name in path {
        let (found, i) = node.child_name_search(FileType::DIR, name);
        if !found {
            return None;
        }
        node = &node.children[i];
    }
    Some(node)
}

/// [`dir_at`] for a change.
pub fn dir_at_mut<'a>(root: &'a mut RvFile, path: &[String]) -> Option<&'a mut RvFile> {
    let mut node = root;
    for name in path {
        let (found, i) = node.child_name_search(FileType::DIR, name);
        if !found {
            return None;
        }
        node = &mut node.children[i];
    }
    Some(node)
}

/// Tick a tree row (`rvTree.SetCheckedRecurse`, `rvTree.cs:540-553`): the row and, unless
/// `just_this`, every row below it. Returns false when there is no such row.
pub fn set_checked(
    root: &mut RvFile,
    path: &[String],
    value: TreeSelect,
    just_this: bool,
) -> bool {
    fn apply(node: &mut RvFile, value: TreeSelect, just_this: bool) {
        if let Some(row) = node.tree.as_mut() {
            row.checked = value;
        }
        if just_this {
            return;
        }
        for child in &mut node.children {
            if child.file_type == FileType::DIR && child.tree.is_some() {
                apply(child, value, false);
            }
        }
    }
    match dir_at_mut(root, path) {
        Some(node) if node.tree.is_some() => {
            apply(node, value, just_this);
            true
        }
        _ => false,
    }
}

/// Remember that a row is open or closed (`RvTreeRow.SetTreeExpanded`).
pub fn set_expanded(root: &mut RvFile, path: &[String], expanded: bool) -> bool {
    match dir_at_mut(root, path).and_then(|n| n.tree.as_mut()) {
        Some(row) => {
            row.expanded = expanded;
            true
        }
        None => false,
    }
}

// ---- the game list ---------------------------------------------------------------------------

/// The tick boxes and the search box above the game list (`FrmMain.cs`, `chkBoxShow*`,
/// `txtFilter`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameFilter {
    pub complete: bool,
    pub partial: bool,
    pub empty: bool,
    pub fixes: bool,
    pub mia: bool,
    pub merged: bool,
    pub text: String,
}

impl Default for GameFilter {
    /// The settings file's defaults (`chkBoxShowMerged` is off).
    fn default() -> Self {
        GameFilter {
            complete: true,
            partial: true,
            empty: true,
            fixes: true,
            mia: true,
            merged: false,
            text: String::new(),
        }
    }
}

/// One line of the game list.
#[derive(Clone, Debug)]
pub struct GameRow {
    /// Position among the children of the directory.
    pub index: usize,
    pub name: String,
    pub description: String,
    /// The archive kind as the type column names it: `Dir`, `Zip`, `TZip`, `7z`.
    pub kind: &'static str,
    pub modified: i64,
    pub status: DirStatus,
    /// The game's own status, which colours the name cell.
    pub rep_status: RepStatus,
}

/// The game list of one directory.
#[derive(Clone, Debug, Default)]
pub struct GameList {
    pub rows: Vec<GameRow>,
    /// Some game has a description, so the column is worth showing.
    pub show_description: bool,
}

fn kind_name(file_type: FileType, zip: Option<ZipStructure>) -> &'static str {
    match file_type {
        FileType::ZIP => match zip.unwrap_or(ZipStructure::NONE) {
            ZipStructure::ZIP_TRRNT => "TZip",
            ZipStructure::ZIP_TDC => "TDC",
            ZipStructure::ZIP_ZSTD => "ZSTD",
            ZipStructure::ZIP_DTD => "DTD",
            ZipStructure::ZIP_DTZ => "DTZ",
            _ => "Zip",
        },
        FileType::SEVEN_ZIP => match zip.unwrap_or(ZipStructure::NONE) {
            ZipStructure::SEVEN_ZIP_TRRNT => "T7z",
            ZipStructure::SEVEN_ZIP_SLZMA => "7z-sL",
            ZipStructure::SEVEN_ZIP_NLZMA => "7z-nL",
            ZipStructure::SEVEN_ZIP_SZSTD => "7z-sZ",
            ZipStructure::SEVEN_ZIP_NZSTD => "7z-nZ",
            _ => "7z",
        },
        _ => "Dir",
    }
}

/// `UpdateGameGrid` (`FrmMain_GameGrid.cs:134-262`): the children of `dir` that are directories
/// or archives and pass the filter.
pub fn game_list(dir: &RvFile, filter: &GameFilter, show_new_mia: bool) -> GameList {
    let needle = filter.text.to_lowercase();
    let mut list = GameList::default();
    for (index, child) in dir.children.iter().enumerate() {
        if !child.is_directory() {
            continue;
        }
        if !needle.is_empty() && !child.name.to_lowercase().contains(&needle) {
            continue;
        }
        let description = child
            .game
            .as_ref()
            .and_then(|g| g.get(game_data::DESCRIPTION))
            .unwrap_or("")
            .to_owned();
        let status = child.dir_status(show_new_mia);

        let correct = status.has_correct();
        let missing = status.has_missing(false);
        let unknown = status.has_unknown();
        let in_to_sort = status.has_in_to_sort();
        let fixes = status.has_fixes_needed();
        let mia = status.has_mia();
        let all_merged = status.has_all_merged();
        let show = (filter.complete && correct && !missing && !fixes)
            || (filter.partial && missing && correct)
            || (filter.empty && missing && !correct)
            || (filter.fixes && fixes)
            || (filter.mia && mia)
            || (filter.merged && all_merged)
            || unknown
            || in_to_sort
            || child.got_status == GotStatus::CORRUPT
            || !(correct || missing || unknown || in_to_sort || fixes || mia || all_merged);
        if !show {
            continue;
        }
        if !list.show_description && !description.trim().is_empty() && description != "\u{a4}" {
            list.show_description = true;
        }
        list.rows.push(GameRow {
            index,
            name: child.name.clone(),
            description,
            kind: kind_name(child.file_type, child.zip_struct),
            modified: child.file_mod_time_stamp,
            status,
            rep_status: child.rep_status,
        });
    }
    list
}

/// A column of the game list that a click on its header sorts by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GameSort {
    Kind,
    Name,
    Description,
    Modified,
}

/// Sort in place; `descending` flips it. The sort is stable, so equal rows stay in name order.
pub fn sort_games(rows: &mut [GameRow], by: GameSort, descending: bool) {
    rows.sort_by(|a, b| {
        let o = match by {
            GameSort::Kind => a.kind.cmp(b.kind),
            GameSort::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            GameSort::Description => a
                .description
                .to_lowercase()
                .cmp(&b.description.to_lowercase()),
            GameSort::Modified => a.modified.cmp(&b.modified),
        };
        if descending {
            o.reverse()
        } else {
            o
        }
    });
}

// ---- the ROM list ----------------------------------------------------------------------------

/// One line of the ROM list, every cell already text.
#[derive(Clone, Debug)]
pub struct RomRow {
    /// Path inside the game, then the name; `(Found: x)` and the CHD and header notes follow.
    pub name: String,
    pub merge: String,
    pub size: String,
    pub crc: String,
    pub sha1: String,
    pub md5: String,
    pub alt_size: String,
    pub alt_crc: String,
    pub alt_sha1: String,
    pub alt_md5: String,
    pub status: String,
    pub modified: String,
    pub zip_index: String,
    pub dat_status: DatStatus,
    /// The status as counted (`ui_status`): MIA and new ROMs have their own.
    pub rep_status: RepStatus,
    /// `_MIA` or `_NEW`, only for items that a DAT lists (`FrmMain_RomGrid.cs:180-189`).
    pub icon_suffix: &'static str,
}

/// The ROMs of a game and which optional columns have something to show.
#[derive(Clone, Debug, Default)]
pub struct RomList {
    pub rows: Vec<RomRow>,
    pub show_merge: bool,
    pub show_alt: bool,
    pub show_status: bool,
    pub show_modified: bool,
}

/// `SetCell`: a value followed by where it came from, `(D)` DAT, `(F)` file header, `(V)` verified.
fn with_flags(text: String, status: FileStatus, dat: FileStatus, file: FileStatus, verified: FileStatus) -> String {
    let mut flags = String::new();
    if dat.0 != 0 && status.has(dat) {
        flags.push('D');
    }
    if file.0 != 0 && status.has(file) {
        flags.push('F');
    }
    if verified.0 != 0 && status.has(verified) {
        flags.push('V');
    }
    if flags.is_empty() {
        text
    } else {
        format!("{text} ({flags})")
    }
}

fn size_text(size: Option<u64>) -> String {
    size.map(|s| grouped(i64::try_from(s).unwrap_or(i64::MAX)))
        .unwrap_or_default()
}

fn rom_row(file: &RvFile, prefix: &str, show_new_mia: bool) -> RomRow {
    let fs = file.file_status;
    let mut name = format!("{prefix}{}", file.name);
    if !file.file_name.is_empty() {
        name.push_str(&format!(" (Found: {})", file.file_name));
    }
    if let Some(v) = file.chd_version {
        name.push_str(&format!(" (V{v})"));
    }
    let d = if fs.has(FileStatus::HEADER_FILE_TYPE_FROM_DAT) { "D" } else { "" };
    let f = if fs.has(FileStatus::HEADER_FILE_TYPE_FROM_HEADER) { "F" } else { "" };
    let header = file.header_type();
    if header != HeaderFileType::NOTHING || !d.is_empty() || !f.is_empty() {
        let required = if file.header_type_required() { ",Required" } else { "" };
        name.push_str(&format!(" ({}{required} {d}{f})", header_name(header)));
    }
    let mia = file.mia();
    let in_dat = file.dat_status == DatStatus::IN_DAT_COLLECT
        || file.dat_status == DatStatus::IN_DAT_MERGED;
    let icon_suffix = if in_dat && (mia.contains(MiaStatus::MIA) || mia.contains(MiaStatus::MIA_FROM_DAT)) {
        "_MIA"
    } else if in_dat && mia.contains(MiaStatus::NEW) {
        "_NEW"
    } else {
        ""
    };
    RomRow {
        name,
        merge: file.merge.clone().unwrap_or_default(),
        size: with_flags(size_text(file.size), fs, FileStatus::SIZE_FROM_DAT, FileStatus::SIZE_FROM_HEADER, FileStatus::SIZE_VERIFIED),
        crc: with_flags(hex(&file.crc), fs, FileStatus::CRC_FROM_DAT, FileStatus::CRC_FROM_HEADER, FileStatus::CRC_VERIFIED),
        sha1: with_flags(hex(&file.sha1), fs, FileStatus::SHA1_FROM_DAT, FileStatus::SHA1_FROM_HEADER, FileStatus::SHA1_VERIFIED),
        md5: with_flags(hex(&file.md5), fs, FileStatus::MD5_FROM_DAT, FileStatus::MD5_FROM_HEADER, FileStatus::MD5_VERIFIED),
        alt_size: with_flags(size_text(file.alt_size), fs, FileStatus::ALT_SIZE_FROM_DAT, FileStatus::ALT_SIZE_FROM_HEADER, FileStatus::ALT_SIZE_VERIFIED),
        alt_crc: with_flags(hex(&file.alt_crc), fs, FileStatus::ALT_CRC_FROM_DAT, FileStatus::ALT_CRC_FROM_HEADER, FileStatus::ALT_CRC_VERIFIED),
        alt_sha1: with_flags(hex(&file.alt_sha1), fs, FileStatus::ALT_SHA1_FROM_DAT, FileStatus::ALT_SHA1_FROM_HEADER, FileStatus::ALT_SHA1_VERIFIED),
        alt_md5: with_flags(hex(&file.alt_md5), fs, FileStatus::ALT_MD5_FROM_DAT, FileStatus::ALT_MD5_FROM_HEADER, FileStatus::ALT_MD5_VERIFIED),
        status: file.status.clone().unwrap_or_default(),
        modified: match file.file_mod_time_stamp {
            0 | i64::MIN => String::new(),
            // ROMVault's TorrentZip stamps, in ticks and as a DOS value.
            629_870_671_200_000_000 | 563_657_728 => with_flags(
                "Trrntziped".to_owned(),
                fs,
                FileStatus::DATE_FROM_DAT,
                FileStatus(0),
                FileStatus(0),
            ),
            t => with_flags(time_text(t), fs, FileStatus::DATE_FROM_DAT, FileStatus(0), FileStatus(0)),
        },
        zip_index: if file.file_type == FileType::FILE_ZIP {
            file.zip_file_index
                .filter(|i| *i != -1)
                .map(|i| i.to_string())
                .unwrap_or_default()
        } else {
            String::new()
        },
        dat_status: file.dat_status,
        rep_status: file.ui_rep_status(show_new_mia),
        icon_suffix,
    }
}

fn header_name(h: HeaderFileType) -> &'static str {
    match HeaderFileType(h.0 & HeaderFileType::HEADER_MASK.0) {
        HeaderFileType::NOTHING => "Nothing",
        HeaderFileType::ZIP => "ZIP",
        HeaderFileType::GZ => "GZ",
        HeaderFileType::SEVEN_ZIP => "SevenZip",
        HeaderFileType::RAR => "RAR",
        HeaderFileType::CHD => "CHD",
        HeaderFileType::A7800 => "A7800",
        HeaderFileType::LYNX => "LYNX",
        HeaderFileType::NES => "NES",
        HeaderFileType::FDS => "FDS",
        HeaderFileType::PCE => "PCE",
        HeaderFileType::PSID => "PSID",
        HeaderFileType::SNES => "SNES",
        HeaderFileType::SPC => "SPC",
        _ => "?",
    }
}

fn collect_roms(game: &RvFile, prefix: &str, show_merged: bool, show_new_mia: bool, out: &mut RomList) {
    for child in &game.children {
        if child.is_file() {
            let hidden = child.dat_status == DatStatus::IN_DAT_MERGED
                && child.rep_status == RepStatus::NOT_COLLECTED
                && !show_merged;
            if hidden {
                continue;
            }
            let row = rom_row(child, prefix, show_new_mia);
            out.show_merge |= !row.merge.trim().is_empty();
            out.show_alt |= child.alt_size.is_some()
                || child.alt_crc.is_some()
                || child.alt_sha1.is_some()
                || child.alt_md5.is_some();
            out.show_status |= !row.status.trim().is_empty();
            out.show_modified |= !row.modified.is_empty();
            out.rows.push(row);
        }
        // A DAT's own sub directories are part of the game (`AddDir`, `FrmMain_RomGrid.cs:100-138`).
        if game.dat_uid != 0 && child.is_directory() && child.game.is_none() {
            collect_roms(child, &format!("{prefix}{}/", child.name), show_merged, show_new_mia, out);
        }
    }
}

/// The ROM list of a game, or of a directory when none is selected.
pub fn rom_list(game: &RvFile, show_merged: bool, show_new_mia: bool) -> RomList {
    let mut list = RomList::default();
    collect_roms(game, "", show_merged, show_new_mia, &mut list);
    list
}

// ---- the DAT information panel -----------------------------------------------------------------

/// What the panel above the game list says about the selected directory.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DirInfo {
    pub title: String,
    /// `(caption, value)` for what the DAT says; empty values are left out.
    pub fields: Vec<(&'static str, String)>,
    /// `ROMs: Got 1 \ Fixable 2 \ Missing 3 \ Unknown 4`.
    pub counts: String,
}

/// The DAT that made the directory at `path`, or the only DAT that sits in it
/// (`UpdateDatMetaData`, `FrmMain.cs:1304-1360`).
fn dat_of<'a>(root: &'a RvFile, path: &[String]) -> Option<&'a rv_core::RvDat> {
    let node = dir_at(root, path)?;
    if node.dat_uid != 0 {
        // The DAT that made this directory is listed by an ancestor.
        for depth in (0..path.len()).rev() {
            let holder = dir_at(root, &path[..depth])?;
            if let Some(d) = holder.dir_dats.iter().find(|d| d.uid == node.dat_uid) {
                return Some(d);
            }
        }
        return None;
    }
    (node.dir_dats.len() == 1).then(|| &node.dir_dats[0])
}

pub fn dir_info(root: &RvFile, path: &[String], status: &DirStatus) -> DirInfo {
    let mut info = DirInfo {
        title: path.last().cloned().unwrap_or_default(),
        counts: format!(
            "ROMs: Got {} \\ Fixable {} \\ Missing {} \\ Unknown {}",
            grouped(i64::from(status.count_correct())),
            grouped(i64::from(status.count_can_be_fixed())),
            grouped(i64::from(status.count_missing(false))),
            grouped(i64::from(status.count_unknown())),
        ),
        fields: Vec::new(),
    };
    if let Some(dat) = dat_of(root, path) {
        let name = dat.get(dat_data::DAT_NAME).unwrap_or("");
        if !name.is_empty() && name != info.title {
            info.title = format!("{}:  {name}", info.title);
        }
        if let Some(id) = dat.get(dat_data::ID).filter(|s| !s.trim().is_empty()) {
            info.title = format!("{} (ID:{id})", info.title);
        }
        for (caption, id) in [
            ("Description", dat_data::DESCRIPTION),
            ("Category", dat_data::CATEGORY),
            ("Version", dat_data::VERSION),
            ("Author", dat_data::AUTHOR),
            ("Date", dat_data::DATE),
            ("Header", dat_data::HEADER),
        ] {
            let v = dat.get(id).unwrap_or("").trim();
            if !v.is_empty() {
                info.fields.push((caption, v.to_owned()));
            }
        }
    }
    info
}
