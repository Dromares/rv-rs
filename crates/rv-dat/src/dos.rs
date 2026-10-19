//! DOSCenter DATs (`DAT/DATReader/DatReader/DatDOSReader.cs`).

use rv_core::FileType;

use crate::error::{Abort, Report, R};
use crate::model::{DatDir, DatFile, DatHeader, DatNode};
use crate::text::Loader;
use crate::time::string_date_time_to_ticks;
use crate::varfix::{clean_hash, ulong};

struct Ctx<'a> {
    dfl: Loader,
    filename: &'a str,
    rep: &'a mut Report,
}

impl Ctx<'_> {
    fn say(&mut self, text: String) {
        self.rep.say(format!("{}: {}", self.filename, text));
    }

    fn check_eof(&self, what: &str) -> R<()> {
        if self.dfl.next.is_empty() && self.dfl.stuck() {
            Err(Abort(format!(
                "{}: unexpected end of file in {what}, on line {}",
                self.filename, self.dfl.line_number
            )))
        } else {
            Ok(())
        }
    }
}

/// `DatDOSReader.ReadDat` (`DatDOSReader.cs:10-58`).
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
    };
    c.dfl.gn();
    if c.dfl.end_of_stream() {
        return Ok(false);
    }
    if c.dfl.next.eq_ignore_ascii_case("doscenter") {
        c.dfl.gn();
        if !load_header(&mut c, filename, header)? {
            return Ok(false);
        }
        c.dfl.gn();
    }
    while !c.dfl.end_of_stream() {
        if c.dfl.next.eq_ignore_ascii_case("game") {
            c.dfl.gn();
            if !load_game(&mut c, &mut header.base_dir)? {
                return Ok(false);
            }
            c.dfl.gn();
        } else {
            let msg = format!(
                "Error: key word '{}' not known, on line {}",
                c.dfl.next, c.dfl.line_number
            );
            c.say(msg);
            c.dfl.gn();
        }
    }
    Ok(true)
}

fn load_header(c: &mut Ctx, filename: &str, h: &mut DatHeader) -> R<bool> {
    if c.dfl.next != "(" {
        let msg = format!("( not found after DOSCenter, on line {}", c.dfl.line_number);
        c.say(msg);
        return Ok(false);
    }
    c.dfl.gn();
    h.filename = filename.to_owned();
    h.compression = "TDC".into();
    while c.dfl.next != ")" {
        c.check_eof("the DOSCenter header")?;
        let nextstr = c.dfl.next.to_lowercase();
        // `name:` with no space before its value (`:76-80`)
        if nextstr.chars().count() > 5 && nextstr.starts_with("name:") {
            let tail: String = c.dfl.next.chars().skip(5).collect();
            let rest = c.dfl.gn_rest();
            h.name = format!("{tail} {rest}").trim().to_owned();
            c.dfl.gn();
        } else {
            match nextstr.as_str() {
                "name" | "name:" => h.name = c.dfl.gn_rest(),
                "description" | "description:" => h.description = c.dfl.gn_rest(),
                "version" | "version:" => h.version = c.dfl.gn_rest(),
                "date" | "date:" => h.date = c.dfl.gn_rest(),
                "author" | "author:" => h.author = c.dfl.gn_rest(),
                "homepage" | "homepage:" => h.homepage = c.dfl.gn_rest(),
                "comment" | "comment:" => h.comment = c.dfl.gn_rest(),
                _ => {
                    let msg = format!(
                        "Error: key word '{}' not known in DOSReader, on line {}",
                        c.dfl.next, c.dfl.line_number
                    );
                    c.say(msg);
                }
            }
            c.dfl.gn();
        }
    }
    Ok(true)
}

fn load_game(c: &mut Ctx, parent: &mut DatDir) -> R<bool> {
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
    let mut name = c.dfl.gn_rest();
    // A trailing `.zip` is dropped when the name is longer than 4 characters (`:148-151`).
    let n = name.chars().count();
    if n > 4 && name.to_lowercase().ends_with(".zip") {
        name = name.chars().take(n - 4).collect();
    }
    c.dfl.gn();
    let mut dir = DatDir::new(&name, FileType::UNSET);
    dir.game = Some(Box::default());
    while c.dfl.next != ")" {
        c.check_eof("game")?;
        match c.dfl.next.to_lowercase().as_str() {
            "file" | "rom" => {
                c.dfl.gn();
                if !load_file(c, &mut dir)? {
                    return Ok(false);
                }
                c.dfl.gn();
            }
            _ => {
                let msg = format!(
                    "Error: key word '{}' not known in game, on line {}",
                    c.dfl.next, c.dfl.line_number
                );
                c.say(msg);
                c.dfl.gn();
            }
        }
    }
    parent.child_add(DatNode::Dir(dir));
    Ok(true)
}

fn load_file(c: &mut Ctx, parent: &mut DatDir) -> R<bool> {
    if c.dfl.next != "(" {
        let msg = format!("( not found after file, on line {}", c.dfl.line_number);
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
    let mut rom = DatFile::new(&c.dfl.gn_name_to_size()?);
    c.dfl.gn();
    while c.dfl.next != ")" {
        c.check_eof("file")?;
        match c.dfl.next.to_lowercase().as_str() {
            "size" => {
                rom.size = ulong(&c.dfl.gn());
                c.dfl.gn();
            }
            "crc" => {
                rom.crc = clean_hash(&c.dfl.gn(), 8);
                c.dfl.gn();
            }
            "sha1" => {
                rom.sha1 = clean_hash(&c.dfl.gn(), 40);
                c.dfl.gn();
            }
            "date" => {
                let d = c.dfl.gn();
                let t = c.dfl.gn();
                rom.base.date_modified = string_date_time_to_ticks(&format!("{d} {t}"))?;
                c.dfl.gn();
            }
            _ => {
                let msg = format!(
                    "Error: key word '{}' not known in rom, on line {}",
                    c.dfl.next, c.dfl.line_number
                );
                c.say(msg);
                c.dfl.gn();
            }
        }
    }
    parent.child_add(DatNode::File(rom));
    Ok(true)
}
