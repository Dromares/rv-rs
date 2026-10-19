//! Shared helpers of the integration tests: the "what can this writer represent" projections.
//!
//! A writer round trip is `read(write(m)) == project(m)`, where `project` clears exactly the
//! fields the ROMVault writer has no place for. Each projection lists those fields, and the
//! list is the gap list of the PR.

#![allow(dead_code)]

use rv_core::FileType;
use rv_dat::{DatDir, DatFile, DatGame, DatHeader, DatNode};

fn clean_game_common(g: &mut DatGame) {
    // `isbios="no"`, `isdevice="no"` and `runnable="yes"` are the defaults and are not written
    // (`DatXMLWriter.cs:117-119`).
    if g.is_bios == "no" {
        g.is_bios.clear();
    }
    if g.is_device == "no" {
        g.is_device.clear();
    }
    if g.runnable == "yes" {
        g.runnable.clear();
    }
}

fn clean_file_common(f: &mut DatFile) {
    // `status="good"` is not written, `mia` only when `yes`.
    if f.status.eq_ignore_ascii_case("good") {
        f.status.clear();
    }
    if f.mia != "yes" {
        f.mia.clear();
    }
}

// ---------------------------------------------------------------------------------------
// XML writer (old style, the only one ROMVault uses)
// ---------------------------------------------------------------------------------------

/// Header fields `DatXMLWriter.WriteHeader` does not write: `subset`, `forcemerging`, `dir`,
/// `notzipped`, `split`, `nodump`. `forcepacking` is written and read back lower-cased.
pub fn xml_header(h: &DatHeader) -> DatHeader {
    let mut p = h.clone();
    p.subset.clear();
    p.merge_type.clear();
    p.dir.clear();
    p.not_zipped = false;
    p.split.clear();
    p.no_dump.clear();
    p.mame_xml = false;
    p.compression = p.compression.to_lowercase();
    p.base_dir = xml_dir(&h.base_dir);
    p
}

/// Not written by the XML writer: game `sampleof`, `sourcefile`, `board`, `history`; the
/// container `type` (old style); file `merge`, `region`; file dates (the reader skips them).
/// EmuArc games lose `manufacturer`, `serial`, `version`, `game_id`.
///
/// Not read back, although written: files inside a plain `<dir>`, and directories inside a
/// `<game>` (`LoadDirFromDat` selects `dir`/`game`/`machine`, `LoadGameFromDat` selects
/// `rom`/`disk`). The order of children changes to what the reader produces: it takes the
/// elements grouped by name, `dir`, `game`, `rom`, `disk`.
pub fn xml_dir(d: &DatDir) -> DatDir {
    xml_dir_at(d, true)
}

fn xml_dir_at(d: &DatDir, is_root: bool) -> DatDir {
    let mut p = d.clone();
    p.base.file_type = FileType::UNSET;
    p.base.date_modified = None;
    p.dat_struct = 0;
    if let Some(g) = &mut p.game {
        g.sample_of.clear();
        g.source_file.clear();
        g.board.clear();
        g.history.clear();
        clean_game_common(g);
        if g.is_emu_arc {
            g.manufacturer.clear();
            g.serial.clear();
            g.version.clear();
            g.game_id.clear();
        }
    }
    let plain_dirs = d.children.iter().filter_map(|c| match c {
        DatNode::Dir(x) if x.game.is_none() => Some(DatNode::Dir(xml_dir_at(x, false))),
        _ => None,
    });
    let games = d.children.iter().filter_map(|c| match c {
        DatNode::Dir(x) if x.game.is_some() => Some(DatNode::Dir(xml_dir_at(x, false))),
        _ => None,
    });
    let files = |disks: bool| {
        d.children
            .iter()
            .filter_map(move |c| match c {
                DatNode::File(f) if f.is_disk == disks => {
                    let mut f = f.clone();
                    f.merge.clear();
                    f.region.clear();
                    f.base.date_modified = None;
                    clean_file_common(&mut f);
                    Some(DatNode::File(f))
                }
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    p.children = if d.game.is_some() {
        files(false).into_iter().chain(files(true)).collect()
    } else if is_root {
        plain_dirs
            .chain(games)
            .chain(files(false))
            .chain(files(true))
            .collect()
    } else {
        plain_dirs.chain(games).collect()
    };
    p
}

// ---------------------------------------------------------------------------------------
// MAME XML writer
// ---------------------------------------------------------------------------------------

/// The MAME header is `build`, `version`, `date`; the reader sets both name and description
/// from `build`.
pub fn mame_header(h: &DatHeader) -> DatHeader {
    DatHeader {
        name: h.name.clone(),
        description: h.name.clone(),
        version: h.version.clone(),
        date: h.date.clone(),
        mame_xml: true,
        filename: h.filename.clone(),
        base_dir: mame_dir(&h.base_dir),
        ..DatHeader::default()
    }
}

/// Machines keep name, cloneof, romof (not for EmuArc games), isbios, isdevice, runnable,
/// description, year, manufacturer, device_ref. Files keep name, merge, size, crc, sha1, md5,
/// status. Files outside a machine are not read back (`ReadMameDat` selects only `dir`, `game`
/// and `machine`), and neither are directories inside a machine. Children come back as
/// `dir`, then `game`/`machine`.
pub fn mame_dir(d: &DatDir) -> DatDir {
    let mut p = d.clone();
    p.base.file_type = FileType::UNSET;
    p.base.date_modified = None;
    p.dat_struct = 0;
    let in_game = d.game.is_some();
    if let Some(g) = &mut p.game {
        let keep = (
            g.description.clone(),
            g.year.clone(),
            g.manufacturer.clone(),
            g.device_ref.clone(),
            g.is_bios.clone(),
            g.is_device.clone(),
            g.runnable.clone(),
            if g.is_emu_arc {
                String::new()
            } else {
                g.clone_of.clone()
            },
            if g.is_emu_arc {
                String::new()
            } else {
                g.rom_of.clone()
            },
        );
        **g = DatGame {
            description: keep.0,
            year: keep.1,
            manufacturer: keep.2,
            device_ref: keep.3,
            is_bios: keep.4,
            is_device: keep.5,
            runnable: keep.6,
            clone_of: keep.7,
            rom_of: keep.8,
            ..DatGame::default()
        };
        clean_game_common(g);
    }
    let file = |f: &DatFile| {
        let mut f = f.clone();
        f.sha256 = None;
        f.region.clear();
        f.mia.clear();
        f.base.date_modified = None;
        clean_file_common(&mut f);
        DatNode::File(f)
    };
    p.children = if in_game {
        let roms = d.children.iter().filter_map(|c| match c {
            DatNode::File(f) if !f.is_disk => Some(file(f)),
            _ => None,
        });
        let disks = d.children.iter().filter_map(|c| match c {
            DatNode::File(f) if f.is_disk => Some(file(f)),
            _ => None,
        });
        roms.chain(disks).collect()
    } else {
        let dirs = d.children.iter().filter_map(|c| match c {
            DatNode::Dir(x) if x.game.is_none() => Some(DatNode::Dir(mame_dir(x))),
            _ => None,
        });
        let games = d.children.iter().filter_map(|c| match c {
            DatNode::Dir(x) if x.game.is_some() => Some(DatNode::Dir(mame_dir(x))),
            _ => None,
        });
        dirs.chain(games).collect()
    };
    p
}

// ---------------------------------------------------------------------------------------
// DOSCenter writer
// ---------------------------------------------------------------------------------------

/// The DOS header has name, description, version, date, author, homepage, comment and the
/// reader sets `Compression = "TDC"`.
pub fn dos_header(h: &DatHeader) -> DatHeader {
    DatHeader {
        name: h.name.clone(),
        description: h.description.clone(),
        version: h.version.clone(),
        date: h.date.clone(),
        author: h.author.clone(),
        homepage: h.homepage.clone(),
        comment: h.comment.clone(),
        compression: "TDC".into(),
        filename: h.filename.clone(),
        base_dir: dos_dir(&h.base_dir),
        ..DatHeader::default()
    }
}

/// Games hold files; a game has no fields of its own in this format; a file keeps name, size,
/// date, crc, sha1. `md5` and `sha256` are written but the reader has no keyword for them, so
/// they cannot round trip. Directories that are not games are skipped by the writer, and a
/// game cannot hold another game.
pub fn dos_dir(d: &DatDir) -> DatDir {
    let mut p = d.clone();
    p.base.file_type = FileType::UNSET;
    p.base.date_modified = None;
    p.dat_struct = 0;
    if d.game.is_some() {
        p.game = Some(Box::default());
    }
    p.children = d
        .children
        .iter()
        .filter_map(|c| match c {
            DatNode::Dir(x) if x.game.is_some() && d.game.is_none() => {
                Some(DatNode::Dir(dos_dir(x)))
            }
            DatNode::Dir(_) => None,
            DatNode::File(_) if d.game.is_none() => None,
            DatNode::File(f) => {
                let mut f = f.clone();
                f.md5 = None;
                f.sha256 = None;
                f.merge.clear();
                f.status.clear();
                f.region.clear();
                f.mia.clear();
                f.is_disk = false;
                Some(DatNode::File(f))
            }
        })
        .collect();
    p
}

// ---------------------------------------------------------------------------------------
// Readable differences
// ---------------------------------------------------------------------------------------

/// The first difference between two models, as text; `None` when equal.
pub fn diff(a: &DatHeader, b: &DatHeader) -> Option<String> {
    let mut ha = a.clone();
    let mut hb = b.clone();
    let ca = std::mem::replace(&mut ha.base_dir, DatDir::new("", FileType::UNSET));
    let cb = std::mem::replace(&mut hb.base_dir, DatDir::new("", FileType::UNSET));
    if ha != hb {
        return Some(format!("header differs:\n  left  {ha:?}\n  right {hb:?}"));
    }
    diff_dir(&ca, &cb, "")
}

fn diff_dir(a: &DatDir, b: &DatDir, path: &str) -> Option<String> {
    let mut sa = a.clone();
    let mut sb = b.clone();
    sa.children.clear();
    sb.children.clear();
    if sa != sb {
        return Some(format!("dir {path:?}:\n  left  {sa:?}\n  right {sb:?}"));
    }
    if a.children.len() != b.children.len() {
        let names = |d: &DatDir| {
            d.children
                .iter()
                .map(|c| c.name().to_owned())
                .collect::<Vec<_>>()
        };
        return Some(format!(
            "dir {path:?}: {} vs {} children\n  left  {:?}\n  right {:?}",
            a.children.len(),
            b.children.len(),
            names(a),
            names(b)
        ));
    }
    for (x, y) in a.children.iter().zip(&b.children) {
        let p = format!("{path}/{}", x.name());
        match (x, y) {
            (DatNode::Dir(x), DatNode::Dir(y)) => {
                if let Some(d) = diff_dir(x, y, &p) {
                    return Some(d);
                }
            }
            (DatNode::File(x), DatNode::File(y)) => {
                if x != y {
                    return Some(format!("file {p:?}:\n  left  {x:?}\n  right {y:?}"));
                }
            }
            _ => return Some(format!("{p:?}: dir vs file")),
        }
    }
    None
}
