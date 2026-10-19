//! `rv-dat`: reads the DAT formats ROMVault 3.8.0 reads and writes the ones it writes.
//!
//! Readers (`DAT/DATReader/DatReader/*.cs`, dispatched as in `DAT/DATReader/DatReader.cs`):
//! ClrMamePro text (also RAINE), XML (`<datafile>`, RVDatFile, MAME `<mame>`/`<mess>`), MAME
//! software lists, DOSCenter and RomCenter. Writers (`DAT/DATReader/DatWriter/*.cs`): XML,
//! MAME XML and DOSCenter.
//!
//! This crate stops at the *parsed* DAT. ROMVault's 18-step clean-up pipeline (`DatClean`) and
//! the merge into the database tree are a later phase and are not ported here.
//!
//! Citations are `src/` paths of the RVWorld 3.8.0 source through `docs/dat-loading.md` of the
//! decompile notes, which is not part of this repository.
//!
//! # Differences from 3.8.0, on purpose
//!
//! * Where a C# loop would never end (a text DAT that stops inside a `( ... )` block; a RAINE
//!   file with no `emulator` keyword) the read fails with an error message instead.
//! * [`read_dat`] fails on a reader error. 3.8.0 imports the half-read DAT instead
//!   (`RomVaultCore/ReadDat/DatReader.cs:76-81`); [`read_dat_report`] returns the same
//!   half-read header together with the error, so a caller can choose.
//! * The writers escape control characters as valid character references (`&#x0A;`); 3.8.0
//!   writes `&#0A;`, which is not well-formed (`DatXMLWriter.cs:233-236`). They also escape
//!   `build`, `version`, `date` of `<mame>` and `device_ref` names, which 3.8.0 writes raw.
//! * Nesting is capped (256 levels of ClrMamePro `dir ( ... )`, 512 of XML elements); 3.8.0
//!   recurses without a limit and a hostile file would overflow the stack.
//! * `.datz` files with a zstd stream (gzip method 93, `Compress/gZip/gZip.cs:324-333`) are
//!   not supported; plain deflate gzip is.

mod cmp;
mod dos;
mod error;
mod model;
mod romcenter;
mod text;
mod time;
pub mod varfix;
mod write;
mod xml;
mod xmldom;

use std::io::Read;
use std::path::Path;

pub use error::{DatError, ReadOutcome};
pub use model::{DatBase, DatDir, DatFile, DatGame, DatHeader, DatNode};
pub use text::{decode_text, split_lines};
pub use write::{write_dos, write_mame_xml, write_xml};

use error::{Abort, Report, R};

/// Read a DAT, strictly: a parse error is an error.
pub fn read_dat(bytes: &[u8], filename: &str) -> Result<DatHeader, DatError> {
    read_dat_report(bytes, filename).into_result()
}

/// Read a DAT the way ROMVault does and report everything: the header as far as it was read,
/// whether the reader succeeded, and the messages.
pub fn read_dat_report(bytes: &[u8], filename: &str) -> ReadOutcome {
    let mut rep = Report::default();
    let mut header: Option<DatHeader> = None;
    let result = dispatch(bytes, filename, &mut rep, &mut header);
    let ok = match result {
        Ok(ok) => ok,
        Err(Abort(msg)) => {
            // `DatRead.ReadDat(string)` catches and reports (`DatReader.cs:39-43`).
            rep.say(format!(
                "{filename}: Error Occured Opening Dat:\r\n{msg}\r\n"
            ));
            false
        }
    };
    ReadOutcome {
        header,
        ok,
        messages: rep.messages,
    }
}

/// Read a DAT file. `.datz` (case-insensitive) is a gzip stream holding the DAT
/// (`DatReader.cs:24-33`).
pub fn read_dat_file(path: &Path) -> ReadOutcome {
    let name = path.to_string_lossy().into_owned();
    let raw = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            return ReadOutcome::failed(format!("{name}: Error Occured Opening Dat:\r\n{e}\r\n"))
        }
    };
    let is_datz = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("datz"));
    let bytes = if is_datz {
        let mut out = Vec::new();
        if let Err(e) = flate2::read::MultiGzDecoder::new(&raw[..]).read_to_end(&mut out) {
            return ReadOutcome::failed(format!("{name}: Error Occured Opening Dat:\r\n{e}\r\n"));
        }
        out
    } else {
        raw
    };
    read_dat_report(&bytes, &name)
}

/// `DatRead.ReadDat(byte[], ...)` (`DatReader.cs:46-110`): sniff the first non-blank line.
fn dispatch(
    bytes: &[u8],
    filename: &str,
    rep: &mut Report,
    header: &mut Option<DatHeader>,
) -> R<bool> {
    let text = decode_text(bytes);
    let lines = split_lines(&text);
    // `while (IsNullOrWhiteSpace(strLine) && !EndOfStream) strLine = ReadLine()`
    let mut str_line: Option<&str> = None;
    let mut it = lines.iter();
    while str_line.is_none_or(|l| l.trim().is_empty()) {
        match it.next() {
            Some(l) => str_line = Some(l),
            None => break,
        }
    }
    let Some(line) = str_line else {
        return Ok(false);
    };
    let lower = line.to_lowercase();
    let has = |needle: &str| text::find_ignore_ascii_case(line, needle).is_some();

    if has("xml") {
        return read_xml(bytes, filename, rep, header);
    }
    if has("clrmamepro")
        || has("clrmame")
        || lower.contains("romvault")
        || lower.contains("game")
        || lower.contains("machine")
    {
        let h = header.insert(DatHeader::default());
        return cmp::read(&text, filename, rep, h);
    }
    if has("doscenter") {
        let h = header.insert(DatHeader::default());
        return dos::read(&text, filename, rep, h);
    }
    if has("[credits]") {
        let h = header.insert(DatHeader::default());
        return romcenter::read(&text, filename, rep, h);
    }
    if has("raine (680x0 arcade emulation)") {
        let h = header.insert(DatHeader::default());
        return cmp::read(&text, filename, rep, h);
    }
    rep.say(format!("{filename}: Invalid DAT File"));
    Ok(false)
}

/// `ReadXMLDatFromStream` (`DatReader.cs:112-161`).
fn read_xml(
    bytes: &[u8],
    filename: &str,
    rep: &mut Report,
    header: &mut Option<DatHeader>,
) -> R<bool> {
    let doc = match xmldom::parse(bytes) {
        Ok(d) => d,
        Err(e) => {
            rep.say(format!("{filename}: Error Occured Reading Dat:\r\n{e}\r\n"));
            return Ok(false);
        }
    };
    let root = &doc.root;
    let named = |n: &str| root.name == n && !root.in_namespace;
    let h = header.insert(DatHeader::default());
    if named("mame") || named("mess") {
        return xml::read_mame(root, filename, h);
    }
    if root.select_one("header").is_some() {
        return xml::read_datafile(root, filename, h);
    }
    // `doc.SelectNodes("softwarelist")` is never null, so any other document ends up in the
    // software list reader, which fails when the root is not `<softwarelist>`.
    xml::read_softwarelist(named("softwarelist").then_some(root), filename, h)
}
