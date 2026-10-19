//! Writing `RomVault3cfg.xml` back (`Settings.WriteConfig`, `Settings.cs`).
//!
//! The file belongs to ROMVault as much as to this program: it also holds the window layout, the
//! emulator list, update keys and other things that [`Settings::from_xml`] does not model. So
//! [`Settings::to_xml`] takes the text of the file it is about to replace and keeps every
//! top-level element it does not own byte for byte, in place; only the elements it models are
//! written again. Without that text it writes a new file in the layout .NET's `XmlSerializer`
//! uses (two-space indent, empty lists as `<IgnoreFiles />`).
//!
//! `DirKey` is written with `\` between the parts (the way ROMVault on Windows stores it);
//! [`Settings::from_xml`] reads both separators. A rule's legacy `DirPath` is not written: reading
//! moved it into a directory mapping already.

use quick_xml::escape::escape;
use quick_xml::events::Event;
use quick_xml::Reader;

use super::{
    DatRule, FixLevel, Settings, COMPRESSION_NAMES, COMPRESSION_SUB_NAMES, FILTER_NAMES,
    HEADER_TYPE_NAMES, MERGE_NAMES, SUB_DIR_TYPE_NAMES,
};

const FRESH_HEAD: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<Settings xmlns:xsd=\"http://www.w3.org/2001/XMLSchema\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\">";

/// The top-level elements this crate owns, in the order a new file gets them.
const OWNED: &[&str] = &[
    "FilesOnly",
    "DatRoot",
    "CacheFile",
    "FixLevel",
    "DatRules",
    "DirMappings",
    "IgnoreFiles",
    "CacheSaveTimerEnabled",
    "CacheSaveTimePeriod",
    "CheckCHDVersion",
    "ShowNewMIA",
    "sevenZDefaultStruct",
];

/// The name `Settings.from_xml` also accepts for `CacheSaveTimePeriod`.
const OLD_PERIOD_NAME: &str = "CacheSavePeriod";

/// The byte range of one top-level child of `<Settings>` in the old text.
struct Span {
    name: String,
    start: usize,
    end: usize,
}

/// Where the children of the old `<Settings>` element are.
struct Layout {
    /// Just after the `<Settings ...>` start tag.
    head_end: usize,
    children: Vec<Span>,
    /// The start of `</Settings>`.
    tail_start: usize,
}

fn layout(old: &str) -> Option<Layout> {
    let mut reader = Reader::from_str(old);
    let mut depth = 0usize;
    let mut head_end = None;
    let mut open: Option<(String, usize)> = None;
    let mut children = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let event = reader.read_event().ok()?;
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(e) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                if depth == 0 {
                    if name != "Settings" {
                        return None;
                    }
                    head_end = Some(after);
                } else if depth == 1 {
                    open = Some((name, before));
                }
                depth += 1;
            }
            Event::Empty(e) => match depth {
                0 => return None,
                1 => children.push(Span {
                    name: String::from_utf8_lossy(e.name().as_ref()).into_owned(),
                    start: before,
                    end: after,
                }),
                _ => {}
            },
            Event::End(_) => {
                depth = depth.checked_sub(1)?;
                if depth == 1 {
                    let (name, start) = open.take()?;
                    children.push(Span {
                        name,
                        start,
                        end: after,
                    });
                } else if depth == 0 {
                    return Some(Layout {
                        head_end: head_end?,
                        children,
                        tail_start: before,
                    });
                }
            }
            Event::Eof => return None,
            _ => {}
        }
    }
}

fn leaf(indent: usize, name: &str, text: &str) -> String {
    let pad = "  ".repeat(indent);
    if text.is_empty() {
        format!("{pad}<{name} />")
    } else {
        format!("{pad}<{name}>{}</{name}>", escape(text))
    }
}

fn flag(indent: usize, name: &str, value: bool) -> String {
    leaf(indent, name, if value { "true" } else { "false" })
}

fn string_list(indent: usize, name: &str, items: &[String]) -> String {
    let pad = "  ".repeat(indent);
    if items.is_empty() {
        return format!("{pad}<{name} />");
    }
    let mut out = format!("{pad}<{name}>");
    for item in items {
        out.push('\n');
        out.push_str(&leaf(indent + 1, "string", item));
    }
    out.push_str(&format!("\n{pad}</{name}>"));
    out
}

fn name_of<T: Copy + PartialEq>(table: &'static [(&'static str, T)], value: T) -> &'static str {
    table
        .iter()
        .find(|(_, v)| *v == value)
        .map_or(table[0].0, |(name, _)| name)
}

fn dat_rule(rule: &DatRule) -> String {
    let mut out = String::from("    <DatRule>");
    let mut line = |s: String| {
        out.push('\n');
        out.push_str(&s);
    };
    line(leaf(3, "DirKey", &rule.dir_key.replace('/', "\\")));
    line(leaf(
        3,
        "Compression",
        name_of(COMPRESSION_NAMES, rule.compression),
    ));
    line(flag(
        3,
        "CompressionOverrideDAT",
        rule.compression_override_dat,
    ));
    line(leaf(
        3,
        "CompressionSub",
        name_of(COMPRESSION_SUB_NAMES, rule.compression_sub),
    ));
    line(flag(3, "ConvertWhileFixing", rule.convert_while_fixing));
    line(leaf(3, "Merge", name_of(MERGE_NAMES, rule.merge)));
    line(leaf(3, "Filter", name_of(FILTER_NAMES, rule.filter)));
    line(leaf(
        3,
        "HeaderType",
        name_of(HEADER_TYPE_NAMES, rule.header_type),
    ));
    line(flag(3, "MergeOverrideDAT", rule.merge_override_dat));
    line(flag(3, "SingleArchive", rule.single_archive));
    line(leaf(
        3,
        "SubDirType",
        name_of(SUB_DIR_TYPE_NAMES, rule.sub_dir_type),
    ));
    line(flag(3, "MultiDATDirOverride", rule.multi_dat_dir_override));
    line(flag(
        3,
        "UseDescriptionAsDirName",
        rule.use_description_as_dir_name,
    ));
    line(flag(3, "UseIdForName", rule.use_id_for_name));
    line(flag(3, "CompleteOnly", rule.complete_only));
    line(string_list(3, "IgnoreFiles", &rule.ignore_files));
    line(flag(3, "AddCategorySubDirs", rule.add_category_sub_dirs));
    line(string_list(3, "CategoryOrder", &rule.category_order));
    out.push_str("\n    </DatRule>");
    out
}

impl Settings {
    /// The text of `RomVault3cfg.xml` for these settings.
    ///
    /// `previous` is the text of the file being replaced, if there is one: its elements that this
    /// crate does not model are kept as they are, and the ones it models keep their place. A
    /// `previous` that is not a `<Settings>` document is ignored. Reading the result with
    /// [`Settings::from_xml`] gives back the same settings.
    pub fn to_xml(&self, previous: Option<&str>) -> String {
        let old = previous.and_then(|p| layout(p).map(|l| (p, l)));
        let present = |name: &str| {
            old.as_ref()
                .is_some_and(|(_, l)| l.children.iter().any(|c| c.name == name))
        };
        // The two owned elements the reader defaults when they are missing are only written
        // when the file had them or the value is not that default.
        let render = |name: &str| -> Option<String> {
            Some(match name {
                "FilesOnly" => flag(1, name, self.files_only),
                "DatRoot" => leaf(1, name, &self.dat_root),
                "CacheFile" => leaf(1, name, &self.cache_file),
                "FixLevel" => leaf(
                    1,
                    name,
                    match self.fix_level {
                        FixLevel::Level1 => "Level1",
                        FixLevel::Level2 => "Level2",
                        FixLevel::Level3 => "Level3",
                    },
                ),
                "DatRules" => {
                    if self.dat_rules.is_empty() {
                        leaf(1, name, "")
                    } else {
                        let rules: Vec<String> = self.dat_rules.iter().map(dat_rule).collect();
                        format!("  <{name}>\n{}\n  </{name}>", rules.join("\n"))
                    }
                }
                "DirMappings" => {
                    if self.dir_mappings.is_empty() {
                        leaf(1, name, "")
                    } else {
                        let mut out = format!("  <{name}>");
                        for m in &self.dir_mappings {
                            out.push_str("\n    <DirMapping>\n");
                            out.push_str(&leaf(3, "DirKey", &m.dir_key.replace('/', "\\")));
                            out.push('\n');
                            out.push_str(&leaf(3, "DirPath", &m.dir_path));
                            out.push_str("\n    </DirMapping>");
                        }
                        out.push_str(&format!("\n  </{name}>"));
                        out
                    }
                }
                "IgnoreFiles" => string_list(1, name, &self.ignore_files),
                "CacheSaveTimerEnabled" => flag(1, name, self.cache_save_timer_enabled),
                "CacheSaveTimePeriod" | OLD_PERIOD_NAME => {
                    leaf(1, name, &self.cache_save_time_period.to_string())
                }
                "CheckCHDVersion" => {
                    if !(present(name) || self.check_chd_version || old.is_none()) {
                        return None;
                    }
                    flag(1, name, self.check_chd_version)
                }
                "ShowNewMIA" => {
                    if !(present(name) || !self.show_new_mia || old.is_none()) {
                        return None;
                    }
                    flag(1, name, self.show_new_mia)
                }
                "sevenZDefaultStruct" => leaf(1, name, &self.seven_z_default_struct.to_string()),
                _ => return None,
            })
        };

        let mut out = String::new();
        let mut written: Vec<&str> = Vec::new();
        let tail = match &old {
            Some((text, l)) => {
                out.push_str(&text[..l.head_end]);
                for child in &l.children {
                    let key = if child.name == OLD_PERIOD_NAME {
                        "CacheSaveTimePeriod"
                    } else {
                        child.name.as_str()
                    };
                    if let Some(owned) = OWNED.iter().find(|o| **o == key) {
                        // The first occurrence is replaced, a repeat is dropped.
                        if written.contains(owned) {
                            continue;
                        }
                        written.push(owned);
                        if let Some(text) = render(&child.name) {
                            out.push('\n');
                            out.push_str(&text);
                        }
                    } else {
                        out.push('\n');
                        out.push_str("  ");
                        out.push_str(&text[child.start..child.end]);
                    }
                }
                &text[l.tail_start..]
            }
            None => {
                out.push_str(FRESH_HEAD);
                "</Settings>"
            }
        };
        for name in OWNED {
            if !written.contains(name) {
                if let Some(text) = render(name) {
                    out.push('\n');
                    out.push_str(&text);
                }
            }
        }
        out.push('\n');
        out.push_str(tail);
        out
    }
}
