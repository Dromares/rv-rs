//! The settings that the DAT update and the scanner read: `RomVault3cfg.xml`
//! (`RomVaultCore/Settings.cs`).
//!
//! The XML is what .NET's `XmlSerializer` writes for `Settings`: one element per field, lists as
//! a wrapper element holding one element per item (`<DatRules><DatRule>...`), enums by name.
//! [`Settings::from_xml`] reads the fields this crate uses and ignores the rest (window layout,
//! emulator paths, update keys, ...); a field that is missing keeps the C# field initializer.
//!
//! Differences from ROMVault, on purpose:
//!
//! * Separators. `DirKey` and `DirPath` may hold `\` (a file written on Windows); they are read as
//!   `/`, the one separator this code base uses (README section 5.3, `docs/linux-port.md`
//!   section 3.3).
//! * Ignore patterns are compiled with the `regex` crate, which has no look-around and no
//!   back-references; a `regex:` pattern that uses them is a [`SettingsError`], not a crash.
//! * Nothing here touches the file system; paths stay relative to the process directory like
//!   ROMVault's.

use std::fmt;

use quick_xml::events::Event;
use quick_xml::Reader;
use regex::{Regex, RegexBuilder};

use crate::enums::{FileType, ZipStructure};
use crate::sorters::ordinal_compare;

/// `EScanLevel` (`Settings.cs:12-17`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum ScanLevel {
    /// "Scan Quick (Headers Only)".
    Level1,
    /// "Scan", the default.
    #[default]
    Level2,
    /// "Scan Full (Complete Re-Scan)".
    Level3,
}

/// `EFixLevel` (`Settings.cs:20-25`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FixLevel {
    Level1,
    #[default]
    Level2,
    Level3,
}

/// `MergeType` (`Settings.cs:27-34`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MergeType {
    #[default]
    None,
    Split,
    Merge,
    NonMerged,
    /// Legacy; read as `Merge` with the `CHDsOnly` filter.
    ChdsMerge,
}

/// `FilterType` (`Settings.cs:36-41`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FilterType {
    #[default]
    KeepAll,
    RomsOnly,
    ChdsOnly,
}

/// `HeaderType` (`Settings.cs:43-48`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HeaderType {
    #[default]
    Optional,
    Headered,
    Headerless,
}

/// `RemoveSubType` (`DAT/DATReader/DatClean/DatClean.cs:8-15`). The number is what a cache
/// stores in `RvDat` (`RvDat.cs:114-124`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum RemoveSubType {
    #[default]
    KeepAllSubDirs = 0,
    RemoveAllSubDirs = 1,
    RemoveAllIfNoConflicts = 2,
    RemoveSubIfSingleFiles = 3,
    RemoveSubIfNameMatches = 4,
}

impl RemoveSubType {
    pub fn from_number(n: i32) -> Option<RemoveSubType> {
        Some(match n {
            0 => RemoveSubType::KeepAllSubDirs,
            1 => RemoveSubType::RemoveAllSubDirs,
            2 => RemoveSubType::RemoveAllIfNoConflicts,
            3 => RemoveSubType::RemoveSubIfSingleFiles,
            4 => RemoveSubType::RemoveSubIfNameMatches,
            _ => return None,
        })
    }
}

/// `DatRule` (`Settings.cs:439-485`): how the DATs below one directory are loaded.
#[derive(Clone, Debug)]
pub struct DatRule {
    /// Tree path starting with `RomVault`, with `/`.
    pub dir_key: String,
    /// Legacy; moved into [`DirMapping`]s when there are none.
    pub dir_path: Option<String>,
    /// `Zip`, `SevenZip` or `FileOnly` in practice.
    pub compression: FileType,
    pub compression_override_dat: bool,
    pub compression_sub: ZipStructure,
    pub convert_while_fixing: bool,
    pub merge: MergeType,
    pub filter: FilterType,
    pub header_type: HeaderType,
    pub merge_override_dat: bool,
    pub single_archive: bool,
    pub sub_dir_type: RemoveSubType,
    pub multi_dat_dir_override: bool,
    pub use_description_as_dir_name: bool,
    pub use_id_for_name: bool,
    pub complete_only: bool,
    pub ignore_files: Vec<String>,
    pub add_category_sub_dirs: bool,
    pub category_order: Vec<String>,
    ignore_regex: Vec<Regex>,
    ignore_scan_regex: Vec<Regex>,
}

impl Default for DatRule {
    /// The C# field initializers (`Settings.cs:439-485`).
    fn default() -> DatRule {
        DatRule {
            dir_key: String::new(),
            dir_path: None,
            compression: FileType::ZIP,
            compression_override_dat: false,
            compression_sub: ZipStructure::ZIP_TRRNT,
            convert_while_fixing: true,
            merge: MergeType::None,
            filter: FilterType::KeepAll,
            header_type: HeaderType::Optional,
            merge_override_dat: false,
            single_archive: false,
            sub_dir_type: RemoveSubType::KeepAllSubDirs,
            multi_dat_dir_override: false,
            use_description_as_dir_name: false,
            use_id_for_name: false,
            complete_only: false,
            ignore_files: Vec::new(),
            add_category_sub_dirs: false,
            category_order: Vec::new(),
            ignore_regex: Vec::new(),
            ignore_scan_regex: Vec::new(),
        }
    }
}

impl DatRule {
    /// The compiled "label as Ignore" patterns of this rule.
    pub fn ignore_regex(&self) -> &[Regex] {
        &self.ignore_regex
    }

    /// The compiled "do not scan" (`ignore:`) patterns of this rule.
    pub fn ignore_scan_regex(&self) -> &[Regex] {
        &self.ignore_scan_regex
    }
}

/// `DirMapping` (`Settings.cs:427-437`): where a tree directory lives on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirMapping {
    pub dir_key: String,
    pub dir_path: String,
}

/// The settings of one ROMVault.
#[derive(Clone, Debug)]
pub struct Settings {
    pub files_only: bool,
    pub dat_root: String,
    pub cache_file: String,
    pub fix_level: FixLevel,
    pub dat_rules: Vec<DatRule>,
    pub dir_mappings: Vec<DirMapping>,
    pub ignore_files: Vec<String>,
    pub cache_save_timer_enabled: bool,
    /// Minutes.
    pub cache_save_time_period: i32,
    pub check_chd_version: bool,
    pub show_new_mia: bool,
    pub seven_z_default_struct: i32,
    ignore_regex: Vec<Regex>,
    ignore_scan_regex: Vec<Regex>,
}

/// A settings file that could not be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsError(pub String);

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "settings: {}", self.0)
    }
}

impl std::error::Error for SettingsError {}

impl Default for Settings {
    /// What ROMVault creates when there is no config file (`Settings.SetDefaults`,
    /// `Settings.cs:150-170`, `ResetDatRules`, `ResetDirMappings`).
    fn default() -> Settings {
        let mut s = Settings {
            files_only: false,
            dat_root: "DatRoot".to_owned(),
            cache_file: "RomVault3_3.Cache".to_owned(),
            fix_level: FixLevel::Level2,
            dat_rules: vec![DatRule {
                dir_key: "RomVault".to_owned(),
                ..DatRule::default()
            }],
            dir_mappings: vec![DirMapping {
                dir_key: "RomVault".to_owned(),
                dir_path: "RomRoot".to_owned(),
            }],
            ignore_files: Vec::new(),
            cache_save_timer_enabled: true,
            cache_save_time_period: 10,
            check_chd_version: false,
            show_new_mia: true,
            seven_z_default_struct: 3,
            ignore_regex: Vec::new(),
            ignore_scan_regex: Vec::new(),
        };
        // The default patterns are valid.
        let _ = s.set_regex_rules();
        s
    }
}

/// `Settings.WildcardToRegex` (`Settings.cs:278-286`).
fn wildcard_to_regex(pattern: &str) -> Result<Regex, SettingsError> {
    let source = if pattern.len() >= 6 && pattern[..6].eq_ignore_ascii_case("regex:") {
        pattern[6..].to_owned()
    } else {
        // `"^" + Regex.Escape(pattern).Replace("\\*", ".*").Replace("\\?", ".") + "$"`
        let mut out = String::from("^");
        for c in pattern.chars() {
            match c {
                '*' => out.push_str(".*"),
                '?' => out.push('.'),
                c => out.push_str(&regex::escape(&c.to_string())),
            }
        }
        out.push('$');
        out
    };
    RegexBuilder::new(&source)
        .case_insensitive(true)
        .build()
        .map_err(|e| SettingsError(format!("ignore pattern {pattern:?}: {e}")))
}

fn compile_ignores(
    patterns: &[String],
    regex: &mut Vec<Regex>,
    scan: &mut Vec<Regex>,
) -> Result<(), SettingsError> {
    for p in patterns {
        // `str.StartsWith("ignore:", OrdinalIgnoreCase)`
        if p.len() >= 7 && p.is_char_boundary(7) && p[..7].eq_ignore_ascii_case("ignore:") {
            scan.push(wildcard_to_regex(&p[7..])?);
        } else {
            regex.push(wildcard_to_regex(p)?);
        }
    }
    Ok(())
}

/// `System.IO.Path.Combine(a, b)` for the relative paths used here.
pub fn path_combine(a: &str, b: &str) -> String {
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

/// `RvFile.GetDatTreePath` (`rvFile.cs:235-247`): the `DatRoot/...` name of a tree path.
pub fn dat_tree_path(tree_full_name: &str) -> String {
    if tree_full_name.is_empty() {
        "DatRoot".to_owned()
    } else if let Some(rest) = tree_full_name.strip_prefix("RomVault") {
        format!("DatRoot{rest}")
    } else {
        "Error".to_owned()
    }
}

impl Settings {
    /// `Settings.SetRegExRules` (`Settings.cs:250-276`).
    pub fn set_regex_rules(&mut self) -> Result<(), SettingsError> {
        self.ignore_regex.clear();
        self.ignore_scan_regex.clear();
        compile_ignores(
            &self.ignore_files,
            &mut self.ignore_regex,
            &mut self.ignore_scan_regex,
        )?;
        for rule in &mut self.dat_rules {
            rule.ignore_regex.clear();
            rule.ignore_scan_regex.clear();
            compile_ignores(
                &rule.ignore_files,
                &mut rule.ignore_regex,
                &mut rule.ignore_scan_regex,
            )?;
        }
        Ok(())
    }

    /// The global "label as Ignore" patterns.
    pub fn ignore_regex(&self) -> &[Regex] {
        &self.ignore_regex
    }

    /// The global "do not scan" patterns.
    pub fn ignore_scan_regex(&self) -> &[Regex] {
        &self.ignore_scan_regex
    }

    /// `DatReader.FindDatRule` (`ReadDat/DatReader.cs:32-63`): the rule with the longest
    /// directory prefix that `dat_name` starts with, ordinally and case-sensitively. The
    /// prefix is `DatRoot` + the key without `RomVault` + `/`, so `DatRoot/Foo/` does not match
    /// `DatRoot/FooBar/x.dat`. Only keys that start with `RomVault` count. A tie goes to the
    /// later rule.
    pub fn find_dat_rule(&self, dat_name: &str) -> Option<&DatRule> {
        let mut found: Option<&DatRule> = None;
        let mut longest = 0usize;
        for rule in &self.dat_rules {
            let Some(rest) = rule.dir_key.strip_prefix("RomVault") else {
                continue;
            };
            let key = format!("DatRoot{rest}/");
            if key.len() > dat_name.len() || !dat_name.starts_with(&key) {
                continue;
            }
            if found.is_some() && key.len() < longest {
                continue;
            }
            longest = key.len();
            found = Some(rule);
        }
        found
    }

    /// `RvFile.GetPhysicalPath` (`rvFile.cs:183-223`): map a tree path through the longest
    /// matching directory mapping. A path no mapping covers is returned unchanged.
    pub fn physical_path(&self, tree_path: &str) -> String {
        let mut found_len = 0usize;
        let mut full = String::new();
        for map in &self.dir_mappings {
            if map.dir_path.trim().is_empty() {
                continue;
            }
            let key = map.dir_key.as_str();
            if tree_path.len() == key.len() && tree_path == key {
                if found_len < key.len() {
                    found_len = key.len();
                    full = map.dir_path.clone();
                }
                continue;
            }
            if tree_path.len() > key.len()
                && tree_path.starts_with(key)
                && tree_path.as_bytes()[key.len()] == b'/'
                && found_len < key.len()
            {
                found_len = key.len();
                full = path_combine(&map.dir_path, &tree_path[key.len() + 1..]);
            }
        }
        if full.is_empty() {
            tree_path.to_owned()
        } else {
            full
        }
    }

    /// `RvFile.GetDatPhysicalPath` (`rvFile.cs:256-272`): a `DatRoot/...` name on disk.
    pub fn dat_physical_path(&self, dat_root_path: &str) -> String {
        if dat_root_path.is_empty() {
            return self.dat_root.clone();
        }
        if dat_root_path.starts_with("ToSort") {
            return "Error".to_owned();
        }
        if let Some(rest) = dat_root_path.strip_prefix("DatRoot") {
            return format!("{}{}", self.dat_root, rest);
        }
        self.dat_root.clone()
    }

    /// Read `RomVault3cfg.xml` text, apply the clean-up of `Settings.SetDefaults`
    /// (`Settings.cs:172-247`): strip a trailing separator from every key, sort rules and
    /// mappings by key and drop later duplicates, migrate the legacy `DirPath`, convert the
    /// `CHDsMerge` rule, compile the patterns.
    pub fn from_xml(xml: &str) -> Result<Settings, SettingsError> {
        let root = parse_dom(xml)?;
        if root.name != "Settings" {
            return Err(SettingsError(format!(
                "root element is {:?}, not Settings",
                root.name
            )));
        }
        let mut s = Settings {
            files_only: false,
            dat_root: String::new(),
            cache_file: String::new(),
            fix_level: FixLevel::Level1,
            dat_rules: Vec::new(),
            dir_mappings: Vec::new(),
            ignore_files: Vec::new(),
            cache_save_timer_enabled: true,
            cache_save_time_period: 10,
            check_chd_version: false,
            show_new_mia: true,
            seven_z_default_struct: 3,
            ignore_regex: Vec::new(),
            ignore_scan_regex: Vec::new(),
        };
        let mut had_rules = false;
        let mut had_mappings = false;
        for child in &root.children {
            match child.name.as_str() {
                "FilesOnly" => s.files_only = parse_bool(child)?,
                "DatRoot" => s.dat_root = child.text.clone(),
                "CacheFile" => s.cache_file = child.text.clone(),
                "FixLevel" => s.fix_level = parse_fix_level(&child.text)?,
                "DatRules" => {
                    had_rules = true;
                    for item in child.children.iter().filter(|c| c.name == "DatRule") {
                        s.dat_rules.push(parse_rule(item)?);
                    }
                }
                "DirMappings" => {
                    had_mappings = true;
                    for item in child.children.iter().filter(|c| c.name == "DirMapping") {
                        s.dir_mappings.push(DirMapping {
                            dir_key: child_text(item, "DirKey").replace('\\', "/"),
                            dir_path: child_text(item, "DirPath").replace('\\', "/"),
                        });
                    }
                }
                "IgnoreFiles" => s.ignore_files = parse_strings(child),
                "CacheSaveTimerEnabled" => s.cache_save_timer_enabled = parse_bool(child)?,
                "CacheSavePeriod" | "CacheSaveTimePeriod" => {
                    s.cache_save_time_period = parse_int(child)?
                }
                "CheckCHDVersion" => s.check_chd_version = parse_bool(child)?,
                "ShowNewMIA" => s.show_new_mia = parse_bool(child)?,
                "sevenZDefaultStruct" => s.seven_z_default_struct = parse_int(child)?,
                _ => {}
            }
        }
        if !had_rules || s.dat_rules.is_empty() {
            s.dat_rules = Settings::default().dat_rules;
        }
        if !had_mappings {
            s.dir_mappings.clear();
        }
        s.finish()?;
        Ok(s)
    }

    /// The tail of `Settings.SetDefaults` and `ReadConfig`.
    fn finish(&mut self) -> Result<(), SettingsError> {
        for rule in &mut self.dat_rules {
            // `CHDsMerge` becomes Merge + CHDsOnly (`Settings.cs:375-381`).
            if rule.merge == MergeType::ChdsMerge {
                rule.merge = MergeType::Merge;
                rule.filter = FilterType::ChdsOnly;
            }
            while rule.dir_key.len() > 1 && rule.dir_key.ends_with('/') {
                rule.dir_key.pop();
            }
        }
        self.dat_rules
            .sort_by(|a, b| ordinal_compare(&a.dir_key, &b.dir_key).cmp(&0));
        self.dat_rules
            .dedup_by(|later, first| later.dir_key == first.dir_key);

        if self.dir_mappings.is_empty() {
            for rule in &mut self.dat_rules {
                if let Some(path) = rule.dir_path.take().filter(|p| !p.is_empty()) {
                    self.dir_mappings.push(DirMapping {
                        dir_key: rule.dir_key.clone(),
                        dir_path: path,
                    });
                }
            }
        }
        self.dir_mappings
            .sort_by(|a, b| ordinal_compare(&a.dir_key, &b.dir_key).cmp(&0));
        self.dir_mappings
            .dedup_by(|later, first| later.dir_key == first.dir_key);
        self.set_regex_rules()
    }
}

// ---- a small XML tree, enough for the serializer's output ----

#[derive(Debug, Default)]
struct XmlNode {
    name: String,
    text: String,
    children: Vec<XmlNode>,
}

fn parse_dom(xml: &str) -> Result<XmlNode, SettingsError> {
    let mut reader = Reader::from_str(xml);
    let mut stack: Vec<XmlNode> = Vec::new();
    let mut root: Option<XmlNode> = None;
    loop {
        match reader.read_event() {
            Err(e) => return Err(SettingsError(format!("XML: {e}"))),
            Ok(Event::Eof) => break,
            Ok(Event::Start(e)) => stack.push(XmlNode {
                name: String::from_utf8_lossy(e.name().as_ref()).into_owned(),
                ..XmlNode::default()
            }),
            Ok(Event::Empty(e)) => {
                let node = XmlNode {
                    name: String::from_utf8_lossy(e.name().as_ref()).into_owned(),
                    ..XmlNode::default()
                };
                match stack.last_mut() {
                    Some(parent) => parent.children.push(node),
                    None => root = Some(node),
                }
            }
            Ok(Event::End(_)) => {
                let node = stack
                    .pop()
                    .ok_or_else(|| SettingsError("XML: unbalanced end tag".to_owned()))?;
                match stack.last_mut() {
                    Some(parent) => parent.children.push(node),
                    None => root = Some(node),
                }
            }
            Ok(Event::Text(t)) => {
                if let Some(top) = stack.last_mut() {
                    let text = t
                        .unescape()
                        .map_err(|e| SettingsError(format!("XML: {e}")))?;
                    top.text.push_str(&text);
                }
            }
            Ok(Event::CData(t)) => {
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&String::from_utf8_lossy(&t));
                }
            }
            Ok(_) => {}
        }
    }
    root.ok_or_else(|| SettingsError("XML: no root element".to_owned()))
}

fn child_text(node: &XmlNode, name: &str) -> String {
    node.children
        .iter()
        .find(|c| c.name == name)
        .map(|c| c.text.clone())
        .unwrap_or_default()
}

fn parse_bool(node: &XmlNode) -> Result<bool, SettingsError> {
    match node.text.trim() {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        other => Err(SettingsError(format!(
            "{}: {other:?} is not a boolean",
            node.name
        ))),
    }
}

fn parse_int(node: &XmlNode) -> Result<i32, SettingsError> {
    node.text
        .trim()
        .parse()
        .map_err(|_| SettingsError(format!("{}: {:?} is not a number", node.name, node.text)))
}

fn parse_strings(node: &XmlNode) -> Vec<String> {
    node.children
        .iter()
        .filter(|c| c.name == "string")
        .map(|c| c.text.clone())
        .collect()
}

fn parse_fix_level(text: &str) -> Result<FixLevel, SettingsError> {
    match text.trim() {
        "Level1" => Ok(FixLevel::Level1),
        "Level2" => Ok(FixLevel::Level2),
        "Level3" => Ok(FixLevel::Level3),
        other => Err(SettingsError(format!("FixLevel: unknown value {other:?}"))),
    }
}

fn parse_enum<T: Copy>(name: &str, text: &str, table: &[(&str, T)]) -> Result<T, SettingsError> {
    let t = text.trim();
    table
        .iter()
        .find(|(n, _)| *n == t)
        .map(|(_, v)| *v)
        .ok_or_else(|| SettingsError(format!("{name}: unknown value {t:?}")))
}

fn parse_rule(item: &XmlNode) -> Result<DatRule, SettingsError> {
    let mut rule = DatRule::default();
    for c in &item.children {
        match c.name.as_str() {
            "DirKey" => rule.dir_key = c.text.replace('\\', "/"),
            "DirPath" => rule.dir_path = Some(c.text.replace('\\', "/")).filter(|p| !p.is_empty()),
            "Compression" => {
                rule.compression = parse_enum(
                    "Compression",
                    &c.text,
                    &[
                        ("UnSet", FileType::UNSET),
                        ("Dir", FileType::DIR),
                        ("Zip", FileType::ZIP),
                        ("SevenZip", FileType::SEVEN_ZIP),
                        ("File", FileType::FILE),
                        ("FileZip", FileType::FILE_ZIP),
                        ("FileSevenZip", FileType::FILE_SEVEN_ZIP),
                        ("FileOnly", FileType::FILE_ONLY),
                    ],
                )?
            }
            "CompressionOverrideDAT" => rule.compression_override_dat = parse_bool(c)?,
            "CompressionSub" => {
                rule.compression_sub = parse_enum(
                    "CompressionSub",
                    &c.text,
                    &[
                        ("None", ZipStructure::NONE),
                        ("ZipTrrnt", ZipStructure::ZIP_TRRNT),
                        ("ZipTDC", ZipStructure::ZIP_TDC),
                        ("ZipDTD", ZipStructure::ZIP_DTD),
                        ("SevenZipTrrnt", ZipStructure::SEVEN_ZIP_TRRNT),
                        ("ZipZSTD", ZipStructure::ZIP_ZSTD),
                        ("ZipDTZ", ZipStructure::ZIP_DTZ),
                        ("SevenZipSLZMA", ZipStructure::SEVEN_ZIP_SLZMA),
                        ("SevenZipNLZMA", ZipStructure::SEVEN_ZIP_NLZMA),
                        ("SevenZipSZSTD", ZipStructure::SEVEN_ZIP_SZSTD),
                        ("SevenZipNZSTD", ZipStructure::SEVEN_ZIP_NZSTD),
                    ],
                )?
            }
            "ConvertWhileFixing" => rule.convert_while_fixing = parse_bool(c)?,
            "Merge" => {
                rule.merge = parse_enum(
                    "Merge",
                    &c.text,
                    &[
                        ("None", MergeType::None),
                        ("Split", MergeType::Split),
                        ("Merge", MergeType::Merge),
                        ("NonMerged", MergeType::NonMerged),
                        ("CHDsMerge", MergeType::ChdsMerge),
                    ],
                )?
            }
            "Filter" => {
                rule.filter = parse_enum(
                    "Filter",
                    &c.text,
                    &[
                        ("KeepAll", FilterType::KeepAll),
                        ("RomsOnly", FilterType::RomsOnly),
                        ("CHDsOnly", FilterType::ChdsOnly),
                    ],
                )?
            }
            "HeaderType" => {
                rule.header_type = parse_enum(
                    "HeaderType",
                    &c.text,
                    &[
                        ("Optional", HeaderType::Optional),
                        ("Headered", HeaderType::Headered),
                        ("Headerless", HeaderType::Headerless),
                    ],
                )?
            }
            "MergeOverrideDAT" => rule.merge_override_dat = parse_bool(c)?,
            "SingleArchive" => rule.single_archive = parse_bool(c)?,
            "SubDirType" => {
                rule.sub_dir_type = parse_enum(
                    "SubDirType",
                    &c.text,
                    &[
                        ("KeepAllSubDirs", RemoveSubType::KeepAllSubDirs),
                        ("RemoveAllSubDirs", RemoveSubType::RemoveAllSubDirs),
                        (
                            "RemoveAllIfNoConflicts",
                            RemoveSubType::RemoveAllIfNoConflicts,
                        ),
                        (
                            "RemoveSubIfSingleFiles",
                            RemoveSubType::RemoveSubIfSingleFiles,
                        ),
                        (
                            "RemoveSubIfNameMatches",
                            RemoveSubType::RemoveSubIfNameMatches,
                        ),
                    ],
                )?
            }
            "MultiDATDirOverride" => rule.multi_dat_dir_override = parse_bool(c)?,
            "UseDescriptionAsDirName" => rule.use_description_as_dir_name = parse_bool(c)?,
            "UseIdForName" => rule.use_id_for_name = parse_bool(c)?,
            "CompleteOnly" => rule.complete_only = parse_bool(c)?,
            "IgnoreFiles" => rule.ignore_files = parse_strings(c),
            "AddCategorySubDirs" => rule.add_category_sub_dirs = parse_bool(c)?,
            "CategoryOrder" => rule.category_order = parse_strings(c),
            _ => {}
        }
    }
    Ok(rule)
}

#[cfg(test)]
mod tests;
