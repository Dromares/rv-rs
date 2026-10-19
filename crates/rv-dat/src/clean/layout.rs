//! Steps 9 to 18 and the tail: where games sit and what they are called
//! (`DatClean.cs`, `DatSetMakeSingleLevel.cs`, `DatSetCompressionType.cs`, `DatSetRemove.cs`,
//! `DatSetMoveCHDs.cs`, `DatSetCleanFilenames.cs`, `DatSetExtension.cs`,
//! `DatClearDescription.cs`).

use rv_core::{FileType, HeaderFileType, RemoveSubType, ZipStructure};

use super::basic::find_category;
use super::{CleanError, CR};
use crate::{DatDir, DatFile, DatGame, DatHeader, DatNode};

/// `StructuredZip.TrrntzipDosDateTime` (`StructuredZip.cs:414`): the DOS date and time that
/// every entry of a TorrentZip carries.
const TRRNTZIP_DOS_DATE_TIME: i64 = 563_657_728;

/// `System.IO.Path.GetExtension` on Unix: from the last `.` after the last `/`, or empty.
fn path_extension(name: &str) -> &str {
    let file_start = name.rfind('/').map_or(0, |i| i + 1);
    match name[file_start..].rfind('.') {
        Some(i) if file_start + i + 1 < name.len() => &name[file_start + i..],
        _ => "",
    }
}

/// `System.IO.Path.GetFileNameWithoutExtension`.
fn file_name_without_extension(name: &str) -> &str {
    let file_start = name.rfind('/').map_or(0, |i| i + 1);
    let file = &name[file_start..];
    &file[..file.len() - path_extension(file).len()]
}

/// `DatClean.DirectoryFlattern` (`DatClean.cs:19-49`, step 10, `SingleArchive` only): every game
/// is lifted to the root and named `"<its directories>/<game>"`; loose files are dropped.
pub fn directory_flatten(root: &mut DatDir) {
    fn flat(children: Vec<DatNode>, sub_dir: &str, out: &mut Vec<DatDir>) {
        for child in children {
            let DatNode::Dir(mut d) = child else { continue };
            let prefix = if sub_dir.trim().is_empty() {
                String::new()
            } else {
                format!("{sub_dir}/")
            };
            if d.game.is_some() {
                d.base.name = format!("{prefix}{}", d.base.name);
                out.push(d);
            } else {
                let sub = format!("{prefix}{}", d.base.name);
                flat(std::mem::take(&mut d.children), &sub, out);
            }
        }
    }
    let old = std::mem::take(&mut root.children);
    let mut list = Vec::new();
    flat(old, "", &mut list);
    for d in list {
        root.child_add(DatNode::Dir(d));
    }
}

/// `DatClean.MakeDatSingleLevel` (`DatSetMakeSingleLevel.cs:9-137`, step 10): all the games of a
/// DAT become one game (or, for loose files, one directory), with the source games kept,
/// flattened or merged by name according to `sub_dir_type`.
pub fn make_dat_single_level(
    header: &mut DatHeader,
    use_description: bool,
    sub_dir_type: RemoveSubType,
    is_files: bool,
    add_category: bool,
    cat_order: &[String],
) {
    let original: Vec<DatNode> = std::mem::take(&mut header.base_dir.children);
    header.dir = "noautodir".to_owned();
    let mut sub_dir_type = sub_dir_type;

    let mut root_dir_name = String::new();
    if use_description && !header.description.trim().is_empty() {
        root_dir_name = header.description.clone();
    }
    if root_dir_name.is_empty() {
        root_dir_name = header.name.clone();
    }

    // A pre-check: would removing every sub-directory give two files the same name?
    if sub_dir_type == RemoveSubType::RemoveAllIfNoConflicts {
        let mut seen: std::collections::HashSet<(String, u8)> = std::collections::HashSet::new();
        'check: for set in &original {
            let DatNode::Dir(rom_set) = set else { continue };
            for rom in &rom_set.children {
                if !seen.insert((rom.name().to_owned(), rom.base().file_type.0)) {
                    sub_dir_type = RemoveSubType::KeepAllSubDirs;
                    break 'check;
                }
            }
        }
    }

    let game = DatGame {
        description: header.description.clone(),
        ..DatGame::default()
    };
    let mut out_dir = if is_files {
        None
    } else {
        let mut d = DatDir::new(&root_dir_name, FileType::UNSET);
        d.game = Some(Box::new(game));
        Some(d)
    };
    let mut base_dir = std::mem::replace(&mut header.base_dir, DatDir::new("", FileType::UNSET));

    for set in original {
        let DatNode::Dir(mut dir_set) = set else {
            continue;
        };
        let mut set_category: Option<Option<String>> = None;
        let rom_count = dir_set.children.len();
        let roms = std::mem::take(&mut dir_set.children);
        for rom in roms {
            let mut rom = rom;
            let mut keep_dir = false;
            match sub_dir_type {
                RemoveSubType::KeepAllSubDirs => keep_dir = true,
                RemoveSubType::RemoveSubIfSingleFiles => keep_dir = rom_count != 1,
                RemoveSubType::RemoveSubIfNameMatches => {
                    if rom_count != 1 {
                        keep_dir = true;
                    } else {
                        // Does the ROM's name (with the category in front, if the game name got
                        // one) match the game's name?
                        let mut test = file_name_without_extension(rom.name()).to_owned();
                        if add_category {
                            let cat = set_category
                                .get_or_insert_with(|| find_category(&dir_set, cat_order));
                            if let Some(c) = cat.as_ref().filter(|c| !c.is_empty()) {
                                test = format!("{c}/{test}");
                            }
                        }
                        keep_dir = test != dir_set.base.name;
                    }
                }
                RemoveSubType::RemoveAllSubDirs | RemoveSubType::RemoveAllIfNoConflicts => {}
            }
            if keep_dir {
                add_back_dir(is_files, &mut base_dir, out_dir.as_mut(), &dir_set, rom);
                continue;
            }
            // The directory (and a category on it) goes: the category moves into the name.
            if add_category {
                let cat = set_category.get_or_insert_with(|| find_category(&dir_set, cat_order));
                if let Some(c) = cat.as_ref().filter(|c| !c.is_empty()) {
                    let name = format!("{c}/{}", rom.name());
                    set_name(&mut rom, name);
                }
            }
            match out_dir.as_mut() {
                Some(o) => {
                    o.child_add(rom);
                }
                None => {
                    base_dir.child_add(rom);
                }
            }
        }
    }
    if let Some(o) = out_dir {
        base_dir.child_add(DatNode::Dir(o));
    }
    header.base_dir = base_dir;
}

fn set_name(node: &mut DatNode, name: String) {
    match node {
        DatNode::Dir(d) => d.base.name = name,
        DatNode::File(f) => f.base.name = name,
    }
}

/// `addBackDir` (`DatSetMakeSingleLevel.cs:140-156`).
fn add_back_dir(
    is_files: bool,
    base_dir: &mut DatDir,
    out_dir: Option<&mut DatDir>,
    dir_set: &DatDir,
    mut rom: DatNode,
) {
    if is_files {
        // Put the original game directory back in place.
        let i = base_dir.dir_search_add(&dir_set.base.name, FileType::UNSET, || {
            let mut d = DatDir::new(&dir_set.base.name, FileType::UNSET);
            d.game = dir_set.game.clone();
            d
        });
        if let DatNode::Dir(d) = &mut base_dir.children[i] {
            d.child_add(rom);
        }
        return;
    }
    // In an archive the extra layer is part of the file name.
    let name = format!("{}/{}", dir_set.base.name, rom.name());
    set_name(&mut rom, name);
    if let Some(o) = out_dir {
        o.child_add(rom);
    }
}

fn file_type_from_dir(ft: FileType) -> CR<FileType> {
    match ft {
        FileType::UNSET => Ok(FileType::UNSET),
        FileType::DIR => Ok(FileType::FILE),
        FileType::ZIP => Ok(FileType::FILE_ZIP),
        FileType::SEVEN_ZIP => Ok(FileType::FILE_SEVEN_ZIP),
        other => Err(CleanError(format!(
            "cannot store files of type {} in a DAT",
            other.0
        ))),
    }
}

/// `DatSetCompressionType.SetType` (`DatSetCompressionType.cs:18-72`, step 11): give every
/// directory and file its type and sort each directory for it. A game keeps the type its DAT
/// chose (`type="zip"`) over the rule's.
pub fn set_type(dir: &mut DatDir, file_type: FileType, zs: ZipStructure, fix: bool) -> CR<()> {
    set_dir_type(dir, file_type, zs, fix)
}

fn set_dir_type(
    d: &mut DatDir,
    mut file_type: FileType,
    mut zs: ZipStructure,
    fix: bool,
) -> CR<()> {
    if d.game.is_none() || file_type == FileType::DIR {
        d.base.file_type = FileType::DIR;
    } else {
        if d.base.file_type != FileType::UNSET {
            if d.base.file_type == FileType::DIR {
                file_type = FileType::DIR;
                zs = ZipStructure::NONE;
            }
            if d.base.file_type == FileType::ZIP {
                file_type = FileType::ZIP;
                zs = ZipStructure::ZIP_TRRNT;
            }
            if d.base.file_type == FileType::SEVEN_ZIP {
                file_type = FileType::SEVEN_ZIP;
                zs = ZipStructure::SEVEN_ZIP_NZSTD;
            }
        }
        d.base.file_type = file_type;
        let checked = if is_trrntzip_date_times(d, zs) {
            ZipStructure::ZIP_TRRNT
        } else {
            zs
        };
        d.dat_struct = checked.0 | if fix { 0x80 } else { 0 };
    }

    let children = std::mem::take(&mut d.children);
    for mut child in children {
        match &mut child {
            DatNode::File(f) => f.base.file_type = file_type_from_dir(file_type)?,
            DatNode::Dir(sub) => set_dir_type(sub, file_type, zs, fix)?,
        }
        d.child_add(child);
    }
    Ok(())
}

/// `IsTrrntzipDateTimes` (`DatSetCompressionType.cs:75-94`): a TDC zip whose files all carry
/// the TorrentZip date is a TorrentZip.
fn is_trrntzip_date_times(d: &DatDir, zs: ZipStructure) -> bool {
    if d.base.file_type != FileType::ZIP || zs != ZipStructure::ZIP_TDC {
        return false;
    }
    d.children.iter().all(|c| match c {
        DatNode::File(f) => f.base.date_modified == Some(TRRNTZIP_DOS_DATE_TIME),
        DatNode::Dir(_) => false,
    })
}

fn is_tdc_like(d: &DatDir) -> bool {
    let s = ZipStructure(d.dat_struct & 0x7f);
    s == ZipStructure::ZIP_TDC || s == ZipStructure::ZIP_DTD || s == ZipStructure::ZIP_DTZ
}

/// `DatClean.RemoveUnNeededDirectories` (`DatSetRemove.cs:251-298`, step 12): in a zip, an
/// explicit `"dir/"` entry of size 0 is dropped when a file below it exists. TDC, DTD and DTZ
/// zips keep theirs.
pub fn remove_un_needed_directories(dir: &mut DatDir) -> CR<()> {
    for child in &mut dir.children {
        let DatNode::Dir(game) = child else { continue };
        if game.base.file_type == FileType::DIR
            || (game.base.file_type == FileType::UNSET && game.game.is_none())
        {
            remove_un_needed_directories(game)?;
        } else {
            if is_tdc_like(game) {
                continue;
            }
            remove_un_needed_from_zip(game)?;
        }
    }
    Ok(())
}

fn remove_un_needed_from_zip(game: &mut DatDir) -> CR<()> {
    let mut r = 0;
    while r < game.children.len() {
        let DatNode::File(df1) = &game.children[r] else {
            return Err(CleanError(format!(
                "{:?} is a directory inside an archive",
                game.children[r].name()
            )));
        };
        let name = df1.base.name.clone();
        if df1.size != Some(0) || name.is_empty() || !name.ends_with('/') {
            r += 1;
            continue;
        }
        let found =
            game.children.iter().enumerate().any(|(r1, c)| {
                r1 != r && c.name().len() > name.len() && c.name().starts_with(&name)
            });
        if found {
            game.children.remove(r);
        } else {
            r += 1;
        }
    }
    Ok(())
}

/// `DatClean.RemoveAllDateTime` (`DatSetRemove.cs:237-249`, step 13): dates only matter inside
/// TDC, DTD and DTZ zips; elsewhere they are dropped.
pub fn remove_all_date_time(dir: &mut DatDir) {
    for child in &mut dir.children {
        match child {
            DatNode::File(f) => f.base.date_modified = None,
            DatNode::Dir(d) => {
                d.base.date_modified = None;
                if is_tdc_like(d) {
                    continue;
                }
                remove_all_date_time(d);
            }
        }
    }
}

/// `DatSetMoveCHDs.MoveUpCHDs` (`DatSetMoveCHDs.cs:10-53`, step 14): a disk inside an archive is
/// moved out into a directory named like the game, next to the archive, and becomes a loose
/// file.
pub fn move_up_chds(root: &mut DatDir) {
    fn rec(parent: &mut DatDir) {
        // (game name, game info, disks) to add to `parent` once the walk of it is over.
        let mut pending: Vec<(String, Option<Box<DatGame>>, Vec<DatFile>)> = Vec::new();
        for child in &mut parent.children {
            let DatNode::Dir(d) = child else { continue };
            rec(d);
            let mut moved = Vec::new();
            let mut i = 0;
            while i < d.children.len() {
                let is_archived_disk = matches!(&d.children[i],
                    DatNode::File(f) if f.is_disk && f.base.file_type != FileType::FILE);
                if is_archived_disk {
                    if let DatNode::File(f) = d.children.remove(i) {
                        moved.push(f);
                    }
                } else {
                    i += 1;
                }
            }
            if !moved.is_empty() {
                pending.push((d.base.name.clone(), d.game.clone(), moved));
            }
        }
        for (name, game, disks) in pending {
            let i = parent.dir_search_add(&name, FileType::DIR, || {
                let mut t = DatDir::new(&name, FileType::DIR);
                t.game = game;
                t
            });
            if let DatNode::Dir(tmp) = &mut parent.children[i] {
                for mut disk in disks {
                    disk.base.file_type = FileType::FILE;
                    tmp.child_add(DatNode::File(disk));
                }
            }
        }
    }
    rec(root);
}

/// `DatClean.DirectoryExpand` (`DatClean.cs:113-164`, step 15): an item with `/` in its name
/// moves into nested directories, except the files inside an archive.
pub fn directory_expand(dir: &mut DatDir) {
    let expandable = |n: &DatNode| -> bool {
        n.name().contains('/')
            && !matches!(
                n.base().file_type,
                FileType::FILE_ZIP | FileType::FILE_SEVEN_ZIP
            )
    };
    if dir.children.iter().any(expandable) {
        let old = std::mem::take(&mut dir.children);
        for mut node in old {
            if !expandable(&node) {
                dir.child_add(node);
                continue;
            }
            let full = node.name().to_owned();
            let split = full.find('/').expect("checked");
            let (part0, part1) = (full[..split].to_owned(), full[split + 1..].to_owned());
            set_name(&mut node, part1.clone());
            let i =
                dir.dir_search_add(&part0, FileType::DIR, || DatDir::new(&part0, FileType::DIR));
            if !part1.is_empty() {
                if let DatNode::Dir(d) = &mut dir.children[i] {
                    d.child_add(node);
                }
            }
        }
    }
    for child in &mut dir.children {
        if let DatNode::Dir(d) = child {
            directory_expand(d);
        }
    }
}

/// Re-sort the node at original position `k` after a rename, like `ChildRemove` then `ChildAdd`,
/// and keep `pos`, the current index of every original node, up to date.
fn resort(dir: &mut DatDir, pos: &mut [usize], k: usize) {
    let from = pos[k];
    let node = dir.children.remove(from);
    let to = dir.child_add(node);
    for (j, p) in pos.iter_mut().enumerate() {
        if j == k {
            *p = to;
            continue;
        }
        let mut q = *p;
        if q > from {
            q -= 1;
        }
        if q >= to {
            q += 1;
        }
        *p = q;
    }
}

/// `DatClean.CleanFileNamesFull` (`DatSetCleanFilenames.cs:101-146`, step 16): trim spaces and
/// trailing dots, an empty name becomes `_`.
pub fn clean_file_names_full(dir: &mut DatDir) {
    let n = dir.children.len();
    let mut pos: Vec<usize> = (0..n).collect();
    for k in 0..n {
        let original = dir.children[pos[k]].name().to_owned();
        let ft = dir.children[pos[k]].base().file_type;
        let mut name = original.clone();
        match ft {
            FileType::UNSET | FileType::FILE | FileType::DIR => {
                name = name
                    .trim_start_matches(' ')
                    .trim_end_matches(['.', ' '])
                    .to_owned();
            }
            FileType::ZIP | FileType::SEVEN_ZIP => {
                name = name
                    .trim_start_matches(' ')
                    .trim_end_matches(' ')
                    .to_owned();
            }
            _ => {}
        }
        if name.trim().is_empty() {
            name = "_".to_owned();
        }
        let renamed = name != original;
        set_name(&mut dir.children[pos[k]], name);
        if renamed {
            resort(dir, &mut pos, k);
        }
        if ft == FileType::DIR {
            if let DatNode::Dir(d) = &mut dir.children[pos[k]] {
                clean_file_names_full(d);
            }
        }
    }
}

/// `DatClean.FixDupes` (`DatSetCleanFilenames.cs:150-200`, step 17): neighbours with the same
/// type and case-insensitive name are renamed `name_<n>` (`base_<n>.ext` for files).
pub fn fix_dupes(dir: &mut DatDir) {
    let n = dir.children.len();
    let mut pos: Vec<usize> = (0..n).collect();
    let mut last_name = String::new();
    let mut last_type = FileType::UNSET;
    let mut match_count = 0;
    for k in 0..n {
        let this_name = dir.children[pos[k]].name().to_owned();
        let ft = dir.children[pos[k]].base().file_type;
        if last_type == ft && last_name.to_lowercase() == this_name.to_lowercase() {
            let new = match ft {
                FileType::DIR | FileType::ZIP | FileType::SEVEN_ZIP => {
                    format!("{this_name}_{match_count}")
                }
                _ => {
                    let ext = path_extension(&this_name);
                    let stem = &this_name[..this_name.len() - ext.len()];
                    format!("{stem}_{match_count}{ext}")
                }
            };
            set_name(&mut dir.children[pos[k]], new);
            match_count += 1;
            resort(dir, &mut pos, k);
        } else {
            match_count = 0;
            last_name = this_name;
            last_type = ft;
        }
        if let DatNode::Dir(d) = &mut dir.children[pos[k]] {
            fix_dupes(d);
        }
    }
}

/// `DatClean.RemoveEmptyDirectories` (`DatSetRemove.cs:319-339`, step 18): games with no ROMs
/// are dropped.
pub fn remove_empty_directories(dir: &mut DatDir) {
    let mut g = 0;
    while g < dir.children.len() {
        if let DatNode::Dir(d) = &mut dir.children[g] {
            if d.game.is_none() {
                remove_empty_directories(d);
            } else if d.children.is_empty() {
                dir.children.remove(g);
                continue;
            }
        }
        g += 1;
    }
}

/// `DatClean.SetExt` (`DatSetExtension.cs:7-54`): `.zip` and `.7z` go on archive names, `.chd`
/// on disks (and their merge names), and the DAT's copier-header type onto every other file.
/// Children are re-added so that they sort with their new names.
pub fn set_ext(dir: &mut DatDir, header_file_type: HeaderFileType) {
    let old = std::mem::take(&mut dir.children);
    for node in old {
        match node {
            DatNode::File(mut f) => {
                if f.is_disk {
                    f.header_file_type = HeaderFileType::CHD;
                    f.base.name.push_str(".chd");
                    if !f.merge.is_empty() {
                        f.merge.push_str(".chd");
                    }
                } else {
                    f.header_file_type = header_file_type;
                }
                dir.child_add(DatNode::File(f));
            }
            DatNode::Dir(mut d) => {
                match d.base.file_type {
                    FileType::ZIP => d.base.name.push_str(".zip"),
                    FileType::SEVEN_ZIP => d.base.name.push_str(".7z"),
                    _ => {}
                }
                set_ext(&mut d, header_file_type);
                dir.child_add(DatNode::Dir(d));
            }
        }
    }
}

/// `DatClean.ClearDescription` (`DatClearDescription.cs:8-25`): a description that only repeats
/// the game's name is replaced by the marker `¤`, which the writers turn back.
pub fn clear_description(dir: &mut DatDir) {
    for child in &mut dir.children {
        let DatNode::Dir(d) = child else { continue };
        if let Some(game) = &mut d.game {
            if file_name_without_extension(&d.base.name) == game.description {
                game.description = "\u{a4}".to_owned();
            }
            continue;
        }
        clear_description(d);
    }
}
