//! ClrMamePro text DATs (`DAT/DATReader/DatReader/DatCmpReader.cs`).
//!
//! Every `while (Next != ")")` loop in the C# spins forever when the file ends inside the block
//! (`Gn()` returns `""` for ever). Here such a file is a reader error instead.

use rv_core::FileType;

use crate::error::{Abort, Report, R};
use crate::model::{DatDir, DatFile, DatGame, DatHeader, DatNode};
use crate::text::Loader;
use crate::varfix::{clean_chd, clean_hash, to_lower, ulong};

/// Deepest `dir ( ... )` nesting accepted; 3.8.0 recurses without a limit. Real DATs nest a
/// few levels.
const MAX_DEPTH: usize = 256;

struct Ctx<'a> {
    dfl: Loader,
    filename: &'a str,
    rep: &'a mut Report,
    depth: usize,
}

impl Ctx<'_> {
    fn say(&mut self, text: String) {
        self.rep.say(format!("{}: {}", self.filename, text));
    }

    fn eof(&self, what: &str) -> Abort {
        Abort(format!(
            "{}: unexpected end of file in {what}, on line {}",
            self.filename, self.dfl.line_number
        ))
    }

    /// The infinite-loop guard.
    fn check_eof(&self, what: &str) -> R<()> {
        if self.dfl.next.is_empty() && self.dfl.stuck() {
            Err(self.eof(what))
        } else {
            Ok(())
        }
    }
}

/// `DatCmpReader.ReadDat` (`DatCmpReader.cs:11-65`). Returns `Ok(false)` where C# returns
/// `false`, `Err` where it throws or would not terminate.
pub(crate) fn read(
    text: &str,
    filename: &str,
    rep: &mut Report,
    header: &mut DatHeader,
) -> R<bool> {
    let mut c = Ctx {
        dfl: Loader::new(text),
        filename,
        rep,
        depth: 0,
    };
    c.dfl.gn();
    if c.dfl.end_of_stream() {
        return Ok(false);
    }
    let first = c.dfl.next.to_lowercase();
    if first == "clrmamepro" || first == "clrmame" {
        if !load_header(&mut c, filename, header)? {
            return Ok(false);
        }
        c.dfl.gn();
    }
    if c.dfl.next.eq_ignore_ascii_case("raine") {
        while !c.dfl.next.eq_ignore_ascii_case("emulator") {
            c.check_eof("the RAINE header")?;
            c.dfl.gn();
        }
        if !load_header(&mut c, filename, header)? {
            return Ok(false);
        }
        c.dfl.gn();
    }
    if c.dfl.next.eq_ignore_ascii_case("romvault") {
        if !load_header(&mut c, filename, header)? {
            return Ok(false);
        }
        c.dfl.gn();
    }
    while !c.dfl.end_of_stream() {
        if !read_next_block(&mut c, &mut header.base_dir)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn read_next_block(c: &mut Ctx, parent: &mut DatDir) -> R<bool> {
    match c.dfl.next.to_lowercase().as_str() {
        "dir" => {
            if !load_dir(c, parent)? {
                return Ok(false);
            }
        }
        "game" | "machine" | "resource" => {
            if !load_game(c, parent)? {
                return Ok(false);
            }
        }
        "emulator" => {
            if !load_emulator(c)? {
                return Ok(false);
            }
        }
        "#" => {
            c.dfl.gn_rest();
        }
        _ => {
            let msg = format!(
                "Error Keyword {} not know in dir, on line {}",
                c.dfl.next, c.dfl.line_number
            );
            c.say(msg);
        }
    }
    c.dfl.gn();
    Ok(true)
}

fn load_header(c: &mut Ctx, filename: &str, h: &mut DatHeader) -> R<bool> {
    c.dfl.gn();
    if c.dfl.next != "(" {
        let msg = format!(
            "( not found after clrmamepro, on line {}",
            c.dfl.line_number
        );
        c.say(msg);
        return Ok(false);
    }
    c.dfl.gn();
    h.filename = filename.to_owned();
    while c.dfl.next != ")" {
        c.check_eof("the clrmamepro header")?;
        match c.dfl.next.to_lowercase().as_str() {
            "name" => h.name = c.dfl.gn_rest(),
            "description" => h.description = c.dfl.gn_rest(),
            "category" => h.category = c.dfl.gn_rest(),
            "version" => h.version = c.dfl.gn_rest(),
            "date" => h.date = c.dfl.gn_rest(),
            "author" => h.author = c.dfl.gn_rest(),
            "email" => h.email = c.dfl.gn_rest(),
            "homepage" => h.homepage = c.dfl.gn_rest(),
            "url" => h.url = c.dfl.gn_rest(),
            "comment" => h.comment = c.dfl.gn_rest(),
            "header" => h.header = c.dfl.gn_rest(),
            // Not lower-cased here, unlike the XML reader; the consumers lower-case on use.
            "forcezipping" => h.compression = c.dfl.gn_rest(),
            "forcepacking" => h.compression = c.dfl.gn_rest(), // "incorrect usage"
            "forcemerging" => h.merge_type = c.dfl.gn_rest(),
            "forcenodump" => h.no_dump = c.dfl.gn_rest(),
            "dir" => h.dir = c.dfl.gn_rest(),
            "games" => {
                c.dfl.gn_rest();
            }
            _ => {
                let msg = format!(
                    "Error: key word '{}' not known in clrmamepro, on line {}",
                    c.dfl.next, c.dfl.line_number
                );
                c.say(msg);
            }
        }
        c.dfl.gn();
    }
    Ok(true)
}

fn load_emulator(c: &mut Ctx) -> R<bool> {
    c.dfl.gn();
    if c.dfl.next != "(" {
        let msg = format!("( not found after emulator, on line {}", c.dfl.line_number);
        c.say(msg);
        return Ok(false);
    }
    c.dfl.gn();
    while c.dfl.next != ")" {
        c.check_eof("emulator")?;
        match c.dfl.next.to_lowercase().as_str() {
            "name" | "version" | "debug" => {
                c.dfl.gn_rest();
            }
            _ => {
                let msg = format!(
                    "Error: key word '{}' not known in emulator, on line {}",
                    c.dfl.next, c.dfl.line_number
                );
                c.say(msg);
            }
        }
        c.dfl.gn();
    }
    Ok(true)
}

fn load_dir(c: &mut Ctx, parent: &mut DatDir) -> R<bool> {
    c.dfl.gn();
    if c.dfl.next != "(" {
        let msg = format!("( not found after game, on line {}", c.dfl.line_number);
        c.say(msg);
        return Ok(false);
    }
    c.dfl.gn();
    if !c.dfl.next.eq_ignore_ascii_case("name") {
        let msg = format!(
            "Name not found as first object in ( ), on line {}",
            c.dfl.line_number
        );
        c.say(msg);
        return Ok(false);
    }
    let mut dir = DatDir::new(&c.dfl.gn_rest(), FileType::UNSET);
    c.dfl.gn();
    if c.depth >= MAX_DEPTH {
        return Err(Abort(format!(
            "{}: directories nested more than {MAX_DEPTH} deep, on line {}",
            c.filename, c.dfl.line_number
        )));
    }
    c.depth += 1;
    // C# adds the directory to its parent before reading its contents (`:238`), so a block
    // that fails half way keeps what was read; add it afterwards on every exit.
    let result = (|| -> R<bool> {
        while c.dfl.next != ")" {
            c.check_eof("dir")?;
            if !read_next_block(c, &mut dir)? {
                return Ok(false);
            }
        }
        Ok(true)
    })();
    c.depth -= 1;
    parent.child_add(DatNode::Dir(dir));
    result
}

fn load_game(c: &mut Ctx, parent: &mut DatDir) -> R<bool> {
    c.dfl.gn();
    if c.dfl.next != "(" {
        let msg = format!("( not found after game, on line {}", c.dfl.line_number);
        c.say(msg);
        return Ok(false);
    }
    c.dfl.gn();
    let mut snext = c.dfl.next.clone();
    let mut pathextra = String::new();
    if snext.eq_ignore_ascii_case("rebuildto") {
        pathextra = c.dfl.gn();
        c.dfl.gn();
        snext = c.dfl.next.clone();
    }
    if !snext.eq_ignore_ascii_case("name") {
        let msg = format!(
            "Name not found as first object in ( ), on line {}",
            c.dfl.line_number
        );
        c.say(msg);
        return Ok(false);
    }
    let name = path_combine(&pathextra, &c.dfl.gn_rest());
    c.dfl.gn();
    let mut dir = DatDir::new(&name, FileType::UNSET);
    dir.game = Some(Box::default());

    while c.dfl.next != ")" && !c.dfl.end_of_stream() {
        match c.dfl.next.to_lowercase().as_str() {
            "romof" => dir.game_mut().rom_of = c.dfl.gn_rest(),
            "description" => {
                let mut description = c.dfl.gn_rest_q();
                if let Some(i1) = description.find('"') {
                    if let Some(i2) = description[i1 + 1..].find('"') {
                        description = description[i1 + 1..i1 + 1 + i2].to_owned();
                    }
                }
                dir.game_mut().description = description;
            }
            "sourcefile" => dir.game_mut().source_file = c.dfl.gn_rest(),
            "cloneof" => dir.game_mut().clone_of = c.dfl.gn_rest(),
            "sampleof" => dir.game_mut().sample_of = c.dfl.gn_rest(),
            "board" => dir.game_mut().board = c.dfl.gn_rest(),
            "year" => dir.game_mut().year = c.dfl.gn_rest(),
            "manufacturer" => dir.game_mut().manufacturer = c.dfl.gn_rest(),
            "history" => dir.game_mut().history = c.dfl.gn_rest(),
            "isdevice" => dir.game_mut().is_device = c.dfl.gn_rest(),
            "serial" | "rebuildto" | "sample" | "biosset" | "chip" | "video" | "sound"
            | "input" | "dipswitch" | "driver" | "display" | "comment" | "releaseyear"
            | "releasemonth" | "releaseday" | "genre" | "developer" | "publisher" | "homepage"
            | "users" | "version" | "license" | "device_ref" | "driverstatus" | "ismechanical"
            | "note" | "#" => {
                c.dfl.gn_rest();
            }
            "/*" => {
                let mut next = c.dfl.gn();
                while next != "*/" {
                    c.check_eof("a /* comment")?;
                    next = c.dfl.gn();
                }
            }
            "name" => {
                let tmp = c.dfl.gn_rest();
                let msg = format!(
                    "Error: multiple names found in one game '{tmp}' will be ignored, on line {}",
                    c.dfl.line_number
                );
                c.say(msg);
            }
            "rom" | "badrom" | "unusedrom" | "znbiosrom" | "s16sndrom" | "s16gfxrom"
            | "neosndrom" => {
                if !load_rom(c, &mut dir)? {
                    return Ok(false);
                }
            }
            "disk" => {
                if !load_disk(c, &mut dir)? {
                    return Ok(false);
                }
            }
            "archive" => {
                if !load_archive(c)? {
                    return Ok(false);
                }
            }
            _ => {
                let msg = format!(
                    "Error: key word '{}' not known in game, on line {}",
                    c.dfl.next, c.dfl.line_number
                );
                c.say(msg);
            }
        }
        c.dfl.gn();
    }
    parent.child_add(DatNode::Dir(dir));
    Ok(true)
}

impl DatDir {
    fn game_mut(&mut self) -> &mut DatGame {
        self.game.as_mut().expect("game directory")
    }
}

/// `RVIO.Path.Combine` on Unix (`libraries/RVIO/RVIO.cs:275-278`): an empty first part is
/// dropped, otherwise joined with `/`.
fn path_combine(a: &str, b: &str) -> String {
    if a.is_empty() {
        b.to_owned()
    } else if b.is_empty() {
        a.to_owned()
    } else if a.ends_with('/') {
        format!("{a}{b}")
    } else {
        format!("{a}/{b}")
    }
}

fn load_rom(c: &mut Ctx, parent: &mut DatDir) -> R<bool> {
    c.dfl.gn();
    if c.dfl.next != "(" {
        let msg = format!("( not found after rom, on line {}", c.dfl.line_number);
        c.say(msg);
        return Ok(false);
    }
    c.dfl.gn();
    if !c.dfl.next.eq_ignore_ascii_case("name") {
        let msg = format!(
            "Name not found as first object in ( ), on line {}",
            c.dfl.line_number
        );
        c.say(msg);
        return Ok(false);
    }
    let mut rom = DatFile::new(&c.dfl.gn());
    c.dfl.gn();
    while c.dfl.next != ")" {
        c.check_eof("rom")?;
        match c.dfl.next.to_lowercase().as_str() {
            "size" => rom.size = ulong(&c.dfl.gn()),
            "hash" | "date" | "bios" | "region" | "offs" => {
                c.dfl.gn();
            }
            "crc" | "crc32" => rom.crc = clean_hash(&c.dfl.gn(), 8),
            "sha1" => rom.sha1 = clean_hash(&c.dfl.gn(), 40),
            "md5" => rom.md5 = clean_hash(&c.dfl.gn(), 32),
            "merge" => rom.merge = c.dfl.gn(),
            "flags" => {
                let flags = to_lower(&c.dfl.gn());
                if rom.status.trim().is_empty() {
                    rom.status = flags;
                }
            }
            "regiona" | "regionb" => {
                while c.dfl.next != ")" {
                    c.check_eof("rom")?;
                    c.dfl.gn();
                }
                continue;
            }
            "nodump" => rom.status = "nodump".into(),
            "baddump" => rom.status = "baddump".into(),
            _ => {
                let msg = format!(
                    "Error: key word '{}' not known in rom, on line {}",
                    c.dfl.next, c.dfl.line_number
                );
                c.say(msg);
            }
        }
        c.dfl.gn();
    }
    parent.child_add(DatNode::File(rom));
    Ok(true)
}

fn load_disk(c: &mut Ctx, parent: &mut DatDir) -> R<bool> {
    c.dfl.gn();
    if c.dfl.next != "(" {
        let msg = format!("( not found after rom, on line {}", c.dfl.line_number);
        c.say(msg);
        return Ok(false);
    }
    c.dfl.gn();
    if !c.dfl.next.eq_ignore_ascii_case("name") {
        let msg = format!(
            "Name not found as first object in ( ), on line {}",
            c.dfl.line_number
        );
        c.say(msg);
        return Ok(false);
    }
    let mut rom = DatFile::new(&clean_chd(&c.dfl.gn()));
    rom.is_disk = true;
    c.dfl.gn();
    while c.dfl.next != ")" {
        c.check_eof("disk")?;
        match c.dfl.next.to_lowercase().as_str() {
            "sha1" => rom.sha1 = clean_hash(&c.dfl.gn(), 40),
            "md5" => rom.md5 = clean_hash(&c.dfl.gn(), 32),
            "region" => rom.region = c.dfl.gn(),
            "merge" => rom.merge = clean_chd(&c.dfl.gn()),
            "index" => {
                c.dfl.gn();
            }
            "flags" => rom.status = to_lower(&c.dfl.gn()),
            "nodump" => rom.status = "nodump".into(),
            _ => {
                let msg = format!(
                    "Error: key word '{}' not known in rom, on line {}",
                    c.dfl.next, c.dfl.line_number
                );
                c.say(msg);
            }
        }
        c.dfl.gn();
    }
    parent.child_add(DatNode::File(rom));
    Ok(true)
}

fn load_archive(c: &mut Ctx) -> R<bool> {
    c.dfl.gn();
    if c.dfl.next != "(" {
        let msg = format!("( not found after Archive, on line {}", c.dfl.line_number);
        c.say(msg);
        return Ok(false);
    }
    c.dfl.gn();
    while c.dfl.next != ")" {
        c.check_eof("archive")?;
        match c.dfl.next.to_lowercase().as_str() {
            "name" => {
                c.dfl.gn();
            }
            _ => {
                let msg = format!(
                    "Error: key word '{}' not know in Archive, on line {}",
                    c.dfl.next, c.dfl.line_number
                );
                c.say(msg);
            }
        }
        c.dfl.gn();
    }
    Ok(true)
}
