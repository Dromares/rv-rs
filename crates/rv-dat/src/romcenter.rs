//! RomCenter DATs (`DAT/DATReader/DatReader/DatROMCenterReader.cs`).
//!
//! The C# `Gn()` throws on a blank or missing line at the end of the file and
//! `line.Substring(0, 1)` throws on an empty line; those are [`Abort`]s here, and the partly
//! filled header stays, as in the C#.

use rv_core::FileType;

use crate::error::{Abort, Report, R};
use crate::model::{DatDir, DatFile, DatGame, DatHeader, DatNode};
use crate::text::LineLoader;
use crate::varfix::{clean_chd, clean_hash, ulong};

struct Ctx<'a> {
    dfl: LineLoader,
    filename: &'a str,
    rep: &'a mut Report,
}

impl Ctx<'_> {
    fn say(&mut self, text: String) {
        self.rep.say(format!("{}: {}", self.filename, text));
    }
}

/// `line.Substring(0, 1) == "["` (throws on an empty line).
fn starts_section(line: &str) -> R<bool> {
    match line.chars().next() {
        Some(ch) => Ok(ch == '['),
        None => Err(Abort(
            "Index and length must refer to a location within the string".into(),
        )),
    }
}

/// `DatROMCenterReader.splitLine` (`:354-366`).
fn split_line(s: &str) -> Option<(&str, &str)> {
    s.find('=').map(|i| (&s[..i], &s[i + 1..]))
}

/// `DatROMCenterReader.ReadDat` (`:10-66`).
pub(crate) fn read(
    text: &str,
    filename: &str,
    rep: &mut Report,
    header: &mut DatHeader,
) -> R<bool> {
    header.filename = filename.to_owned();
    let mut c = Ctx {
        dfl: LineLoader::new(text),
        filename,
        rep,
    };
    c.dfl.gn()?;
    while !c.dfl.end_of_stream() {
        let ok = match c.dfl.next.to_lowercase().as_str() {
            "[credits]" => load_credits(&mut c, header)?,
            "[dat]" => load_dat(&mut c, header)?,
            "[emulator]" => load_emulator(&mut c, header)?,
            "[games]" | "[resources]" => load_game(&mut c, &mut header.base_dir, false)?,
            "[disks]" => load_game(&mut c, &mut header.base_dir, true)?,
            _ => {
                let msg = format!("Unknown Line {} , {}", c.dfl.next, c.dfl.line_number);
                c.say(msg);
                return Ok(false);
            }
        };
        if !ok {
            return Ok(false);
        }
    }
    Ok(true)
}

/// The three key=value sections share their loop (`:69-203`). `set` returns false for an
/// unknown key.
fn load_section(c: &mut Ctx, section: &str, mut set: impl FnMut(&str, &str) -> bool) -> R<bool> {
    while !c.dfl.end_of_stream() {
        let line = c.dfl.gn()?;
        if starts_section(&line)? {
            return Ok(true);
        }
        let Some((element, value)) = split_line(&line) else {
            return Ok(false);
        };
        if !set(&element.to_lowercase(), value) {
            let msg = format!(
                "Unknown Line {} found in [{section}], {}",
                c.dfl.next, c.dfl.line_number
            );
            c.say(msg);
            return Ok(false);
        }
    }
    Ok(true)
}

fn load_credits(c: &mut Ctx, h: &mut DatHeader) -> R<bool> {
    load_section(c, "CREDITS", |k, v| {
        match k {
            "author" => h.author = v.into(),
            "email" => h.email = v.into(),
            "homepage" | "url" => h.url = v.into(),
            "version" => h.version = v.into(),
            "date" => h.date = v.into(),
            "comment" => h.comment = v.into(),
            _ => return false,
        }
        true
    })
}

fn load_dat(c: &mut Ctx, h: &mut DatHeader) -> R<bool> {
    load_section(c, "DAT", |k, v| {
        match k {
            "version" | "plugin" => {}
            "split" => h.split = v.into(),
            "merge" => h.merge_type = v.into(),
            _ => return false,
        }
        true
    })
}

fn load_emulator(c: &mut Ctx, h: &mut DatHeader) -> R<bool> {
    load_section(c, "EMULATOR", |k, v| {
        match k {
            "refname" => h.name = v.into(),
            "version" => h.description = v.into(),
            "category" => h.category = v.into(),
            "exe" | "runcmd" | "romspaths" => {}
            _ => return false,
        }
        true
    })
}

/// `[GAMES]`/`[RESOURCES]` (`LoadGame`, `:205-275`) and `[DISKS]` (`LoadDisks`, `:277-351`).
fn load_game(c: &mut Ctx, parent: &mut DatDir, disks: bool) -> R<bool> {
    while !c.dfl.end_of_stream() {
        let line = c.dfl.gn()?;
        if starts_section(&line)? {
            return Ok(true);
        }
        // The separator is U+FFFD if the line has one (a Latin-1 0xAC byte decoded as UTF-8),
        // else U+00AC (`:221-226`).
        let parts: Vec<&str> = if line.contains('\u{FFFD}') {
            line.split('\u{FFFD}').collect()
        } else if line.contains('\u{00AC}') {
            line.split('\u{00AC}').collect()
        } else {
            continue;
        };
        // Fields 1..=9; index 0 is the empty text before the leading separator.
        if parts.len() < 10 {
            return Err(Abort("Index was outside the bounds of the array.".into()));
        }
        let parent_name = parts[1];
        let game_name = parts[3];
        let game_description = parts[4];
        let rom_name = parts[5];
        let rom_crc = parts[6];
        let rom_size = parts[7];
        let merge = parts[9];

        // `[GAMES]` searches with `UnSet`, `[DISKS]` with `Dir` (`:250`, `:323`); games are
        // stored as `UnSet`, so the `[DISKS]` search never matches and every row creates a
        // new game directory.
        let search_type = if disks {
            FileType::DIR
        } else {
            FileType::UNSET
        };
        let index = match parent.child_name_search(game_name, search_type) {
            Some(i) => i,
            None => {
                let mut dir = DatDir::new(game_name, FileType::UNSET);
                let mut game = DatGame {
                    description: game_description.to_owned(),
                    ..DatGame::default()
                };
                if parent_name != game_name {
                    game.clone_of = parent_name.to_owned();
                }
                dir.game = Some(Box::new(game));
                parent.child_add(DatNode::Dir(dir))
            }
        };
        let DatNode::Dir(dir) = &mut parent.children[index] else {
            return Err(Abort("Unable to cast object to type DatDir".into()));
        };
        if disks {
            let mut rom = DatFile::new(&clean_chd(rom_name));
            rom.is_disk = true;
            rom.sha1 = clean_hash(rom_crc, 40);
            rom.merge = clean_chd(merge);
            dir.child_add(DatNode::File(rom));
        } else {
            let mut rom = DatFile::new(rom_name);
            rom.crc = clean_hash(rom_crc, 8);
            rom.size = ulong(rom_size);
            rom.merge = merge.to_owned();
            dir.child_add(DatNode::File(rom));
        }
    }
    Ok(true)
}
