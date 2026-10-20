//! The FixDat report (`RomVaultCore/FixDatReport.cs`, `docs/fixing.md` section 14): DATs that
//! list what is still missing, one per DAT of the library. Read only: nothing in the tree or on
//! disk changes except the DAT files written into the output directory, and an existing file is
//! never overwritten.
//!
//! Differences from 3.8.0: `DatClean.ArchiveDirectoryFlattern` is not applied (directories inside
//! an archive are single entries in this tree), and the file name is built with `/`.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use rv_core::{
    dat_data, game_data, DatFlags, DatStatus, FileType, GotStatus, RepStatus, RvDat, RvFile,
};
use rv_dat::{DatDir, DatFile, DatGame, DatHeader, DatNode};

/// One FixDat: its file name and the DAT text.
#[derive(Clone, Debug)]
pub struct FixDat {
    pub file_name: String,
    pub text: String,
    /// Wanted files that are listed in it.
    pub missing: usize,
}

/// Build the FixDats of everything below the root of the tree. With `red_only`, a missing file
/// that Find Fixes found a source for is left out (it is about to be fixed); without, every
/// missing file is listed. `today` is the `yyyy-MM-dd` date for the header.
pub fn fix_dats(root: &RvFile, red_only: bool, today: &str) -> Vec<FixDat> {
    let mut dats: HashMap<u32, &RvDat> = HashMap::new();
    gather_dats(root, &mut dats);
    let mut out = Vec::new();
    let Some(first) = root.children.first() else {
        return out;
    };
    recursive(first, &dats, red_only, today, &mut out);
    out
}

fn gather_dats<'a>(n: &'a RvFile, dats: &mut HashMap<u32, &'a RvDat>) {
    for d in &n.dir_dats {
        dats.insert(d.uid, d);
    }
    for c in &n.children {
        if c.is_directory() {
            gather_dats(c, dats);
        }
    }
}

fn recursive(
    dir: &RvFile,
    dats: &HashMap<u32, &RvDat>,
    red_only: bool,
    today: &str,
    out: &mut Vec<FixDat>,
) {
    if !dir.is_directory() {
        return;
    }
    if dir.dat_uid != 0 {
        if let Some(d) = dats.get(&dir.dat_uid) {
            extract(d, dir, red_only, today, out);
        }
        return;
    }
    for d in &dir.dir_dats {
        extract(d, dir, red_only, today, out);
    }
    for c in &dir.children {
        if c.is_directory() && c.dat_uid == 0 {
            recursive(c, dats, red_only, today, out);
        }
    }
}

fn dat_type(t: FileType) -> FileType {
    match t {
        FileType::ZIP | FileType::SEVEN_ZIP => t,
        _ => FileType::DIR,
    }
}

/// `RecursiveDatTreeFindingDat` (`FixDatReport.cs:90-125`).
fn collect(dir: &RvFile, uid: u32, red_only: bool, out: &mut DatDir) -> usize {
    let mut found = 0;
    for child in &dir.children {
        if child.dat_uid != uid {
            continue;
        }
        if child.is_directory() {
            // A game is named without the extension of its archive.
            let name = match child.file_type {
                FileType::ZIP => child.name.strip_suffix(".zip").unwrap_or(&child.name),
                FileType::SEVEN_ZIP => child.name.strip_suffix(".7z").unwrap_or(&child.name),
                _ => &child.name,
            };
            let mut d = DatDir::new(name, dat_type(child.file_type));
            d.dat_struct = child.zip_dat_struct.unwrap_or(0);
            if let Some(g) = &child.game {
                d.game = Some(Box::new(DatGame {
                    description: g.get(game_data::DESCRIPTION).unwrap_or("").to_owned(),
                    ..DatGame::default()
                }));
            }
            let n = collect(child, uid, red_only, &mut d);
            if n > 0 {
                found += n;
                out.child_add(DatNode::Dir(d));
            }
            continue;
        }
        let wanted = child.dat_status == DatStatus::IN_DAT_COLLECT
            && child.got_status != GotStatus::GOT
            && (!red_only
                || !(child.rep_status == RepStatus::CAN_BE_FIXED
                    || child.rep_status == RepStatus::CORRUPT_CAN_BE_FIXED));
        if wanted {
            let mut f = DatFile::new(&child.name);
            f.size = child.size;
            f.crc = child.crc.clone();
            f.sha1 = child.sha1.clone();
            f.md5 = child.md5.clone();
            f.base.file_type = FileType::FILE;
            out.child_add(DatNode::File(f));
            found += 1;
        }
    }
    found
}

/// `FixSingleLevelDat` (`FixDatReport.cs:127-163`): loose files in a directory that is not a game
/// become one-file games named after the file.
fn single_level(dir: &mut DatDir) {
    let old = std::mem::take(&mut dir.children);
    let mut loose: Vec<DatFile> = Vec::new();
    for c in old {
        match c {
            DatNode::Dir(mut d) => {
                if d.game.is_none() {
                    single_level(&mut d);
                }
                dir.children.push(DatNode::Dir(d));
            }
            DatNode::File(f) => loose.push(f),
        }
    }
    for f in loose {
        let stem = match f.base.name.rfind('.') {
            Some(i) if i > 0 => f.base.name[..i].to_owned(),
            _ => f.base.name.clone(),
        };
        let existing = dir
            .children
            .iter()
            .position(|c| matches!(c, DatNode::Dir(d) if d.base.name == stem && d.game.is_some()));
        let at = match existing {
            Some(i) => i,
            None => {
                let mut d = DatDir::new(&stem, FileType::DIR);
                d.game = Some(Box::new(DatGame::default()));
                dir.children.push(DatNode::Dir(d));
                dir.children.len() - 1
            }
        };
        if let DatNode::Dir(d) = &mut dir.children[at] {
            d.children.push(DatNode::File(f));
        }
    }
}

/// `FixDatReport.ExtractDat` (`FixDatReport.cs:48-85`).
fn extract(dat: &RvDat, dir: &RvFile, red_only: bool, today: &str, out: &mut Vec<FixDat>) {
    let mut base = DatDir::new("", FileType::UNSET);
    let found = collect(dir, dat.uid, red_only, &mut base);
    if found == 0 {
        return;
    }
    // A DAT whose only top directory was added for it is not shown as one.
    if dat.dat_flags.contains(DatFlags::AUTO_ADDED_DIRECTORY)
        && base.children.len() == 1
        && matches!(&base.children[0], DatNode::Dir(d) if d.game.is_none())
    {
        if let Some(DatNode::Dir(d)) = base.children.pop() {
            base = d;
        }
    }
    single_level(&mut base);
    let _ = rv_dat::clean::remove_un_needed_directories(&mut base);

    let get = |id| dat.get(id).unwrap_or("").to_owned();
    let h = DatHeader {
        name: format!("FixDat_{}", get(dat_data::DAT_NAME)),
        description: format!("FixDat_{}", get(dat_data::DESCRIPTION)),
        category: get(dat_data::CATEGORY),
        version: get(dat_data::VERSION),
        author: "RomVault".to_owned(),
        date: today.to_owned(),
        base_dir: base,
        ..DatHeader::default()
    };

    let full = get(dat_data::DAT_ROOT_FULL_NAME);
    let rel = full.strip_prefix("DatRoot/").unwrap_or(&full);
    let (dat_dir, file) = match rel.rfind('/') {
        Some(i) => (&rel[..i], &rel[i + 1..]),
        None => ("", rel),
    };
    let stem = match file.rfind('.') {
        Some(i) if i > 0 => &file[..i],
        _ => file,
    };
    out.push(FixDat {
        file_name: format!("fixDat_{}_{}.dat", dat_dir.replace('/', "_"), stem),
        text: rv_dat::write_xml(&h, false),
        missing: found,
    });
}

/// Write the FixDats into `out_dir` (created if needed), `(n)` after the name when a file of
/// that name is already there. Returns the paths written.
pub fn write_fix_dats(
    root: &RvFile,
    red_only: bool,
    today: &str,
    out_dir: &Path,
) -> io::Result<Vec<PathBuf>> {
    fs::create_dir_all(out_dir)?;
    let mut written = Vec::new();
    for fd in fix_dats(root, red_only, today) {
        let stem = fd.file_name.trim_end_matches(".dat").to_owned();
        let mut path = out_dir.join(&fd.file_name);
        let mut n = 0;
        loop {
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut f) => {
                    io::Write::write_all(&mut f, fd.text.as_bytes())?;
                    f.sync_all()?;
                    break;
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    n += 1;
                    path = out_dir.join(format!("{stem}({n}).dat"));
                }
                Err(e) => return Err(e),
            }
        }
        written.push(path);
    }
    Ok(written)
}

/// Today's date as `yyyy-MM-dd` (UTC).
pub fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let days = secs.div_euclid(86_400);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}
