//! DAT writers (`DAT/DATReader/DatWriter/DatXMLWriter.cs`, `DatMAMEXMLWriter.cs`,
//! `DatDOSWriter.cs`). Output uses `\n` line ends (what .NET writes on Linux) and no BOM.
//!
//! ROMVault has no writer for ClrMamePro text, RomCenter or software lists, and the JSON
//! writer is inside a block comment (`DatJSONWriter.cs`), so those formats cannot be written.

use rv_core::FileType;

use crate::model::{DatDir, DatFile, DatGame, DatHeader, DatNode};
use crate::time::zip_date_time_to_string;

/// `StructuredZip.TrrntzipDateTime` (`Compress/StructuredZip/StructuredZip.cs:413`): a date
/// equal to this is not written.
const TRRNTZIP_DATE_TIME: i64 = 629_870_671_200_000_000;

/// `DatStreamWriter` (`DatXMLWriter.cs:244-337`): tab-indented lines with a depth counter.
struct Out {
    buf: String,
    depth: usize,
}

impl Out {
    fn new() -> Out {
        Out {
            buf: String::new(),
            depth: 0,
        }
    }

    fn tabs(&self) -> String {
        "\t".repeat(self.depth)
    }

    /// `WriteLine(value)`
    fn line(&mut self, value: &str) {
        self.buf.push_str(&self.tabs());
        self.buf.push_str(value);
        self.buf.push('\n');
    }

    /// `WriteLine(value, tabDir)`
    fn line_dir(&mut self, value: &str, dir: i32) {
        if dir == -1 {
            self.depth = self.depth.saturating_sub(1);
        }
        self.line(value);
        if dir == 1 {
            self.depth += 1;
        }
    }

    /// `Write(value)`: indent, no newline.
    fn write(&mut self, value: &str) {
        self.buf.push_str(&self.tabs());
        self.buf.push_str(value);
    }

    /// `WriteEnd(value)`: no indent, newline.
    fn end(&mut self, value: &str) {
        self.buf.push_str(value);
        self.buf.push('\n');
    }

    /// `WriteEnd(value, tabDir)`
    fn end_dir(&mut self, value: &str, dir: i32) {
        if dir == -1 {
            self.depth = self.depth.saturating_sub(1);
        }
        self.end(value);
        if dir == 1 {
            self.depth += 1;
        }
    }

    /// `WriteNode(name, value)`: `<name>text</name>` unless blank.
    fn node(&mut self, name: &str, value: &str) {
        if value.trim().is_empty() {
            return;
        }
        self.line(&format!("<{name}>{}</{name}>", etxt(value)));
    }

    /// `WriteItem(name, string)`: ` name="text"` unless blank.
    fn item(&mut self, name: &str, value: &str) {
        if value.trim().is_empty() {
            return;
        }
        self.buf.push_str(&format!(" {name}=\"{}\"", etxt(value)));
    }

    fn item_u64(&mut self, name: &str, value: Option<u64>) {
        if let Some(v) = value {
            self.buf.push_str(&format!(" {name}=\"{v}\""));
        }
    }

    fn item_hash(&mut self, name: &str, value: &Option<Vec<u8>>) {
        if let Some(v) = value {
            self.buf.push_str(&format!(" {name}=\"{}\"", hex(v)));
        }
    }
}

fn hex(b: &[u8]) -> String {
    use std::fmt::Write;
    b.iter().fold(String::new(), |mut s, x| {
        let _ = write!(s, "{x:02x}");
        s
    })
}

/// `Etxt` (`DatXMLWriter.cs:222-242`). Control characters become character references; the
/// source writes `&#0A;` (hex digits, no `x`), which XML readers reject, this writes `&#x0A;`.
fn etxt(e: &str) -> String {
    let mut ret = String::with_capacity(e.len());
    for c in e.chars() {
        match c {
            '&' => ret.push_str("&amp;"),
            '"' => ret.push_str("&quot;"),
            '\'' => ret.push_str("&apos;"),
            '<' => ret.push_str("&lt;"),
            '>' => ret.push_str("&gt;"),
            c if c < ' ' => ret.push_str(&format!("&#x{:02X};", c as u32)),
            c => ret.push(c),
        }
    }
    ret
}

/// `Etxt` of `DatMAMEXMLWriter.cs:166-186`, which also escapes DEL.
fn etxt_mame(e: &str) -> String {
    etxt(e).replace('\u{7f}', "&#x7F;")
}

fn not_blank_and_not(value: &str, other: &str) -> bool {
    !value.trim().is_empty() && value != other
}

fn date_item(out: &mut Out, f: &DatFile) {
    if let Some(d) = f.base.date_modified {
        if d != TRRNTZIP_DATE_TIME {
            out.item("date", &zip_date_time_to_string(Some(d)));
        }
    }
}

// ---------------------------------------------------------------------------------------
// XML
// ---------------------------------------------------------------------------------------

/// `DatXMLWriter.WriteDat` (`DatXMLWriter.cs:11-46`). `new_style` writes the RVDatFile
/// variant (`<RVDatFile>`, `<set>`, `<file>`).
pub fn write_xml(h: &DatHeader, new_style: bool) -> String {
    let mut o = Out::new();
    o.line("<?xml version=\"1.0\"?>");
    o.line_dir(
        if new_style {
            "<RVDatFile>"
        } else {
            "<datafile>"
        },
        1,
    );
    // header (`:48-69`)
    o.line_dir("<header>", 1);
    o.node("id", &h.id);
    o.node("name", &h.name);
    o.node("type", &h.r#type);
    o.node("rootdir", &h.root_dir);
    o.node("description", &h.description);
    o.node("category", &h.category);
    o.node("version", &h.version);
    o.node("date", &h.date);
    o.node("author", &h.author);
    o.node("email", &h.email);
    o.node("homepage", &h.homepage);
    o.node("url", &h.url);
    o.node("comment", &h.comment);
    o.write("<romvault");
    o.item("header", &h.header);
    o.item("forcepacking", &h.compression);
    o.end("/>");
    o.line_dir("</header>", -1);
    xml_base(&mut o, &h.base_dir, new_style);
    o.line_dir(
        if new_style {
            "</RVDatFile>"
        } else {
            "</datafile>"
        },
        -1,
    );
    o.buf
}

fn xml_base(o: &mut Out, dir: &DatDir, new_style: bool) {
    for node in &dir.children {
        match node {
            DatNode::Dir(d) => xml_dir(o, d, new_style),
            DatNode::File(f) => xml_file(o, f, new_style),
        }
    }
}

fn xml_dir(o: &mut Out, d: &DatDir, new_style: bool) {
    let g = d.game.as_ref();
    match g {
        None => o.write("<dir"),
        Some(_) => o.write(if new_style { "<set" } else { "<game" }),
    }
    o.item("name", &d.base.name);
    if new_style {
        if d.base.file_type == FileType::ZIP {
            o.item("type", "trrntzip");
        } else if d.base.file_type == FileType::SEVEN_ZIP {
            o.item("type", "7zip");
        } else if d.base.file_type == FileType::DIR {
            o.item("type", "dir");
        }
        if let Some(date) = d.base.date_modified {
            o.item("date", &date.to_string());
        }
    }
    if let Some(g) = g {
        if !g.is_emu_arc {
            o.item("id", &g.id);
            o.item("cloneof", &g.clone_of);
            o.item("cloneofid", &g.clone_of_id);
            o.item("romof", &g.rom_of);
        }
        if not_blank_and_not(&g.is_bios, "no") {
            o.item("isbios", &g.is_bios);
        }
        if not_blank_and_not(&g.is_device, "no") {
            o.item("isdevice", &g.is_device);
        }
        if not_blank_and_not(&g.runnable, "yes") {
            o.item("runnable", &g.runnable);
        }
    }
    o.end_dir(">", 1);

    if let Some(g) = g {
        for c in &g.category {
            o.node("category", c);
        }
        o.node("description", &g.description);
        if g.is_emu_arc {
            o.line_dir("<trurip>", 1);
            o.node("titleid", &g.id);
            o.node("source", &g.source);
            o.node("publisher", &g.publisher);
            o.node("developer", &g.developer);
            o.node("year", &g.year);
            o.node("genre", &g.genre);
            o.node("subgenre", &g.sub_genre);
            o.node("ratings", &g.ratings);
            o.node("score", &g.score);
            o.node("players", &g.players);
            o.node("enabled", &g.enabled);
            o.node("crc", &g.crc);
            o.node("cloneof", &g.clone_of);
            o.node("relatedto", &g.related_to);
            o.line_dir("</trurip>", -1);
        } else {
            o.node("year", &g.year);
            o.node("manufacturer", &g.manufacturer);
            o.node("serial", &g.serial);
            o.node("version", &g.version);
            o.node("game_id", &g.game_id);
        }
    }

    xml_base(o, d, new_style);

    if let Some(g) = g {
        for r in &g.device_ref {
            o.line(&format!("<device_ref name=\"{}\"/>", etxt(r)));
        }
    }
    match g {
        None => o.line_dir("</dir>", -1),
        Some(_) => o.line_dir(if new_style { "</set>" } else { "</game>" }, -1),
    }
}

fn xml_file(o: &mut Out, f: &DatFile, new_style: bool) {
    if f.base.name.ends_with('/') && new_style {
        o.write("<dir");
        o.item("name", &f.base.name[..f.base.name.len() - 1]);
        date_item(o, f);
        o.end("/>");
        return;
    }
    if f.is_disk {
        o.write("<disk");
    } else {
        o.write(if new_style { "<file" } else { "<rom" });
    }
    o.item("name", &f.base.name);
    o.item_u64("size", f.size);
    o.item_hash("crc", &f.crc);
    o.item_hash("sha1", &f.sha1);
    o.item_hash("sha256", &f.sha256);
    o.item_hash("md5", &f.md5);
    date_item(o, f);
    if !f.status.eq_ignore_ascii_case("good") {
        o.item("status", &f.status);
    }
    if f.mia == "yes" {
        o.item("mia", "yes");
    }
    o.end("/>");
}

// ---------------------------------------------------------------------------------------
// MAME XML
// ---------------------------------------------------------------------------------------

const MAME_DOCTYPE: &str = r#"<!DOCTYPE mame [
<!ELEMENT mame (machine+)>
	<!ATTLIST mame build CDATA #IMPLIED>
	<!ATTLIST mame version CDATA #IMPLIED>
	<!ATTLIST mame date CDATA #IMPLIED>
	<!ATTLIST mame debug (yes|no) "no">
	<!ELEMENT machine (description, year?, manufacturer?, biosset*, rom*, disk*, device_ref*, sample*, chip*, display*, sound?, input?, dipswitch*, configuration*, port*, adjuster*, driver?, feature*, device*, slot*, softwarelist*, ramoption*)>
		<!ATTLIST machine name CDATA #REQUIRED>
		<!ATTLIST machine isbios (yes|no) "no">
		<!ATTLIST machine isdevice (yes|no) "no">
		<!ATTLIST machine runnable (yes|no) "yes">
		<!ATTLIST machine cloneof CDATA #IMPLIED>
		<!ATTLIST machine romof CDATA #IMPLIED>
		<!ELEMENT description (#PCDATA)>
		<!ELEMENT year (#PCDATA)>
		<!ELEMENT manufacturer (#PCDATA)>
		<!ELEMENT rom EMPTY>
			<!ATTLIST rom name CDATA #REQUIRED>
			<!ATTLIST rom bios CDATA #IMPLIED>
			<!ATTLIST rom size CDATA #REQUIRED>
			<!ATTLIST rom crc CDATA #IMPLIED>
			<!ATTLIST rom sha1 CDATA #IMPLIED>
			<!ATTLIST rom merge CDATA #IMPLIED>
			<!ATTLIST rom region CDATA #IMPLIED>
			<!ATTLIST rom offset CDATA #IMPLIED>
			<!ATTLIST rom status (baddump|nodump|good) "good">
			<!ATTLIST rom optional (yes|no) "no">
		<!ELEMENT disk EMPTY>
			<!ATTLIST disk name CDATA #REQUIRED>
			<!ATTLIST disk sha1 CDATA #IMPLIED>
			<!ATTLIST disk merge CDATA #IMPLIED>
			<!ATTLIST disk region CDATA #IMPLIED>
			<!ATTLIST disk index CDATA #IMPLIED>
			<!ATTLIST disk writable (yes|no) "no">
			<!ATTLIST disk status (baddump|nodump|good) "good">
			<!ATTLIST disk optional (yes|no) "no">
		<!ELEMENT device_ref EMPTY>
			<!ATTLIST device_ref name CDATA #REQUIRED>
]>

"#;

/// `DatMAMEXMLWriter.WriteDat` (`DatMAMEXMLWriter.cs:11-88`).
pub fn write_mame_xml(h: &DatHeader) -> String {
    let mut o = Out::new();
    o.line("<?xml version=\"1.0\"?>");
    // `WriteLine(@"<!DOCTYPE ...]>" + blank line)`: the literal ends with two newlines.
    o.line(MAME_DOCTYPE);
    let mut mame = format!("<mame build=\"{}\"", etxt_mame(&h.name));
    if !h.version.trim().is_empty() {
        mame.push_str(&format!(" version=\"{}\"", etxt_mame(&h.version)));
    }
    if !h.date.trim().is_empty() {
        mame.push_str(&format!(" date=\"{}\"", etxt_mame(&h.date)));
    }
    mame.push('>');
    o.line(&mame);
    mame_base(&mut o, &h.base_dir);
    o.line_dir("</mame>", -1);
    o.buf
}

fn mame_base(o: &mut Out, dir: &DatDir) {
    for node in &dir.children {
        match node {
            DatNode::Dir(d) => match &d.game {
                Some(g) => mame_machine(o, d, g),
                None => {
                    o.line_dir(&format!("<dir name=\"{}\">", etxt_mame(&d.base.name)), 1);
                    mame_base(o, d);
                    o.line_dir("</dir>", -1);
                }
            },
            DatNode::File(f) => mame_file(o, f),
        }
    }
}

fn mame_machine(o: &mut Out, d: &DatDir, g: &DatGame) {
    o.write("<machine");
    o.item("name", &d.base.name);
    if !g.is_emu_arc {
        o.item("cloneof", &g.clone_of);
        o.item("romof", &g.rom_of);
    }
    if not_blank_and_not(&g.is_bios, "no") {
        o.item("isbios", &g.is_bios);
    }
    if not_blank_and_not(&g.is_device, "no") {
        o.item("isdevice", &g.is_device);
    }
    if not_blank_and_not(&g.runnable, "yes") {
        o.item("runnable", &g.runnable);
    }
    o.end_dir(">", 1);
    o.node("description", &g.description);
    o.node("year", &g.year);
    o.node("manufacturer", &g.manufacturer);
    mame_base(o, d);
    for r in &g.device_ref {
        o.line(&format!("<device_ref name=\"{}\"/>", etxt_mame(r)));
    }
    o.line_dir("</machine>", -1);
}

fn mame_file(o: &mut Out, f: &DatFile) {
    o.write(if f.is_disk { "<disk" } else { "<rom" });
    o.item("name", &f.base.name);
    o.item("merge", &f.merge);
    o.item_u64("size", f.size);
    o.item_hash("crc", &f.crc);
    o.item_hash("sha1", &f.sha1);
    o.item_hash("md5", &f.md5);
    date_item(o, f);
    if !f.status.eq_ignore_ascii_case("good") {
        o.item("status", &f.status);
    }
    o.end("/>");
}

// ---------------------------------------------------------------------------------------
// DOSCenter
// ---------------------------------------------------------------------------------------

/// `DatDOSWriter.WriteDat` (`DatDOSWriter.cs:12-105`). Directories without a game are skipped
/// together with everything below them (`:60-77`).
pub fn write_dos(h: &DatHeader) -> String {
    let mut o = Out::new();
    o.line_dir("DOSCenter (", 1);
    for (name, value) in [
        ("name", &h.name),
        ("description", &h.description),
        ("version", &h.version),
        ("date", &h.date),
        ("author", &h.author),
        ("homepage", &h.homepage),
        ("comment", &h.comment),
    ] {
        if !value.trim().is_empty() {
            o.line(&format!("{name}: {value}"));
        }
    }
    o.line_dir(")", -1);
    dos_base(&mut o, &h.base_dir);
    o.buf
}

fn dos_base(o: &mut Out, dir: &DatDir) {
    for node in &dir.children {
        match node {
            DatNode::Dir(d) => {
                if d.game.is_some() {
                    o.line("");
                    o.line_dir("game (", 1);
                    if !d.base.name.trim().is_empty() {
                        o.line(&format!("name \"{}.zip\"", d.base.name));
                    }
                    dos_base(o, d);
                    o.line_dir(")", -1);
                }
            }
            DatNode::File(f) => {
                o.write("file (");
                // `WriteItem("name", name, force: true)`
                o.buf.push_str(&format!(" name {}", f.base.name));
                if let Some(s) = f.size {
                    o.buf.push_str(&format!(" size {s}"));
                }
                if let Some(d) = f.base.date_modified {
                    if d != TRRNTZIP_DATE_TIME {
                        let s = zip_date_time_to_string(Some(d));
                        if !s.trim().is_empty() {
                            o.buf.push_str(&format!(" date {s}"));
                        }
                    }
                }
                for (name, v) in [
                    ("crc", &f.crc),
                    ("sha1", &f.sha1),
                    ("sha256", &f.sha256),
                    ("md5", &f.md5),
                ] {
                    if let Some(v) = v {
                        o.buf.push_str(&format!(" {name} {}", hex(v)));
                    }
                }
                o.end(" )");
            }
        }
    }
}
