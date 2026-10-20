use super::*;

const CFG: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<Settings xmlns:xsd="http://www.w3.org/2001/XMLSchema" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
  <FilesOnly>false</FilesOnly>
  <DatRoot>DatRoot</DatRoot>
  <CacheFile>RomVault3_3.Cache</CacheFile>
  <FixLevel>Level3</FixLevel>
  <DatRules>
    <DatRule>
      <DirKey>RomVault\Console\DS\Roms</DirKey>
      <DirPath />
      <Compression>SevenZip</Compression>
      <CompressionSub>SevenZipSLZMA</CompressionSub>
      <Merge>CHDsMerge</Merge>
      <IgnoreFiles><string>*.txt</string><string>ignore:Thumbs.db</string><string>regex:^a.c$</string></IgnoreFiles>
      <SubDirType>RemoveSubIfNameMatches</SubDirType>
    </DatRule>
    <DatRule>
      <DirKey>RomVault\</DirKey>
      <Compression>Zip</Compression>
    </DatRule>
    <DatRule>
      <DirKey>RomVault</DirKey>
      <Compression>FileOnly</Compression>
    </DatRule>
  </DatRules>
  <DirMappings>
    <DirMapping><DirKey>RomVault</DirKey><DirPath>RomRoot</DirPath></DirMapping>
    <DirMapping><DirKey>RomVault\Console\DS</DirKey><DirPath>D:\Roms\DS</DirPath></DirMapping>
  </DirMappings>
  <IgnoreFiles><string>*.bak</string></IgnoreFiles>
  <Darkness>true</Darkness>
</Settings>"#;

#[test]
fn reads_rules_mappings_and_normalises_separators() {
    let s = Settings::from_xml(CFG).expect("parses");
    assert_eq!(s.dat_root, "DatRoot");
    assert_eq!(s.fix_level, FixLevel::Level3);
    // `RomVault\` lost its separator, became `RomVault`, and the later duplicate was dropped.
    let keys: Vec<&str> = s.dat_rules.iter().map(|r| r.dir_key.as_str()).collect();
    assert_eq!(keys, ["RomVault", "RomVault/Console/DS/Roms"]);
    // The first of the duplicates wins: `Zip`, not `FileOnly`.
    assert_eq!(s.dat_rules[0].compression, FileType::ZIP);
    let ds = &s.dat_rules[1];
    assert_eq!(ds.compression, FileType::SEVEN_ZIP);
    assert_eq!(ds.compression_sub, ZipStructure::SEVEN_ZIP_SLZMA);
    // CHDsMerge is rewritten to Merge + CHDsOnly.
    assert_eq!(ds.merge, MergeType::Merge);
    assert_eq!(ds.filter, FilterType::ChdsOnly);
    assert_eq!(ds.sub_dir_type, RemoveSubType::RemoveSubIfNameMatches);
    assert!(
        ds.convert_while_fixing,
        "C# field initializer survives a missing element"
    );
    assert_eq!(
        s.dir_mappings,
        [
            DirMapping {
                dir_key: "RomVault".into(),
                dir_path: "RomRoot".into()
            },
            DirMapping {
                dir_key: "RomVault/Console/DS".into(),
                dir_path: "D:/Roms/DS".into()
            },
        ]
    );
}

#[test]
fn ignore_patterns_split_into_label_and_scan_lists() {
    let s = Settings::from_xml(CFG).unwrap();
    assert_eq!(s.ignore_regex().len(), 1);
    assert!(s.ignore_regex()[0].is_match("OLD.BAK"), "case-insensitive");
    assert!(
        !s.ignore_regex()[0].is_match("x.bak.zip"),
        "anchored wildcard"
    );
    let ds = &s.dat_rules[1];
    assert_eq!(ds.ignore_regex().len(), 2);
    assert_eq!(ds.ignore_scan_regex().len(), 1);
    assert!(ds.ignore_scan_regex()[0].is_match("thumbs.db"));
    assert!(ds.ignore_regex()[0].is_match("readme.TXT"));
    // `regex:` is used as written, unanchored.
    assert!(ds.ignore_regex()[1].is_match("abc"));
    assert!(!ds.ignore_regex()[1].is_match("zabcz"));
}

#[test]
fn wildcard_metacharacters_are_escaped() {
    let r = wildcard_to_regex("a.b(1)?*").unwrap();
    assert!(r.is_match("a.b(1)X"));
    assert!(!r.is_match("aXb(1)X"));
}

#[test]
fn find_dat_rule_is_the_longest_directory_prefix() {
    let s = Settings::from_xml(CFG).unwrap();
    let key = |n: &str| s.find_dat_rule(n).map(|r| r.dir_key.as_str());
    assert_eq!(
        key("DatRoot/Console/DS/Roms/x.dat"),
        Some("RomVault/Console/DS/Roms")
    );
    assert_eq!(
        key("DatRoot/Console/DS/Roms/"),
        Some("RomVault/Console/DS/Roms")
    );
    assert_eq!(key("DatRoot/Console/DS/RomsX/x.dat"), Some("RomVault"));
    // The prefix test is case-sensitive.
    assert_eq!(key("DatRoot/console/ds/roms/x.dat"), Some("RomVault"));
    assert_eq!(key("Error"), None);
    assert_eq!(key("ToSort/x"), None);
}

#[test]
fn physical_path_uses_the_longest_mapping() {
    let s = Settings::from_xml(CFG).unwrap();
    assert_eq!(s.physical_path("RomVault"), "RomRoot");
    assert_eq!(
        s.physical_path("RomVault/Console/GB/Roms"),
        "RomRoot/Console/GB/Roms"
    );
    assert_eq!(
        s.physical_path("RomVault/Console/DS/Roms"),
        "D:/Roms/DS/Roms"
    );
    assert_eq!(
        s.physical_path("RomVault/Console/DSX"),
        "RomRoot/Console/DSX"
    );
    assert_eq!(s.physical_path("ToSort"), "ToSort");
}

#[test]
fn dat_paths() {
    let s = Settings::from_xml(CFG).unwrap();
    assert_eq!(dat_tree_path(""), "DatRoot");
    assert_eq!(dat_tree_path("RomVault/A/B"), "DatRoot/A/B");
    assert_eq!(dat_tree_path("ToSort"), "Error");
    assert_eq!(s.dat_physical_path("DatRoot/A/x.dat"), "DatRoot/A/x.dat");
    assert_eq!(s.dat_physical_path(""), "DatRoot");
}

#[test]
fn legacy_dir_path_becomes_a_mapping_and_bad_input_is_an_error() {
    let xml = r#"<Settings><DatRules><DatRule><DirKey>RomVault</DirKey><DirPath>X:\Roms</DirPath></DatRule></DatRules></Settings>"#;
    let s = Settings::from_xml(xml).unwrap();
    assert_eq!(s.dir_mappings.len(), 1);
    assert_eq!(s.dir_mappings[0].dir_path, "X:/Roms");
    assert!(Settings::from_xml("<Other/>").is_err());
    assert!(Settings::from_xml("<Settings><FilesOnly>maybe</FilesOnly></Settings>").is_err());
    assert!(Settings::from_xml(
        "<Settings><IgnoreFiles><string>regex:(?=x)</string></IgnoreFiles></Settings>"
    )
    .is_err());
    assert!(Settings::from_xml("<Settings>").is_err());
}

#[test]
fn defaults_match_a_first_run() {
    let s = Settings::default();
    assert_eq!(s.dat_rules.len(), 1);
    assert_eq!(s.dat_rules[0].dir_key, "RomVault");
    assert_eq!(
        s.find_dat_rule("DatRoot/x/y.dat")
            .map(|r| r.dir_key.as_str()),
        Some("RomVault")
    );
    assert_eq!(s.physical_path("RomVault/a"), "RomRoot/a");
}

// ---- writing ----

/// A file with elements this crate does not model, in odd places, and a rule list.
const CFG_WITH_EXTRAS: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<Settings xmlns:xsd="http://www.w3.org/2001/XMLSchema" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
  <FilesOnly>false</FilesOnly>
  <DatRoot>DatRoot</DatRoot>
  <CacheFile>RomVault3_3.Cache</CacheFile>
  <FixLevel>Level3</FixLevel>
  <DatRules>
    <DatRule>
      <DirKey>RomVault\Console\DS\Roms</DirKey>
      <DirPath />
      <Compression>SevenZip</Compression>
      <CompressionSub>SevenZipSLZMA</CompressionSub>
      <Merge>Split</Merge>
      <IgnoreFiles>
        <string>*.txt</string>
        <string>ignore:Thumbs.db</string>
      </IgnoreFiles>
    </DatRule>
  </DatRules>
  <DirMappings>
    <DirMapping>
      <DirKey>RomVault</DirKey>
      <DirPath>RomRoot</DirPath>
    </DirMapping>
  </DirMappings>
  <IgnoreFiles />
  <EInfo>
    <string>kept &amp; untouched</string>
  </EInfo>
  <DoubleCheckDelete>true</DoubleCheckDelete>
  <CacheSavePeriod>7</CacheSavePeriod>
  <Darkness>true</Darkness>
  <sevenZDefaultStruct>3</sevenZDefaultStruct>
</Settings>
"#;

fn rule_summary(s: &Settings) -> Vec<String> {
    s.dat_rules
        .iter()
        .map(|r| {
            format!(
                "{} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {} {} {} {:?}",
                r.dir_key,
                r.compression,
                r.compression_sub,
                r.merge,
                r.filter,
                r.header_type,
                r.sub_dir_type,
                r.ignore_files,
                r.convert_while_fixing,
                r.complete_only,
                r.use_id_for_name,
                r.category_order,
            )
        })
        .collect()
}

fn assert_same(a: &Settings, b: &Settings) {
    assert_eq!(a.files_only, b.files_only);
    assert_eq!(a.dat_root, b.dat_root);
    assert_eq!(a.cache_file, b.cache_file);
    assert_eq!(a.fix_level, b.fix_level);
    assert_eq!(a.dir_mappings, b.dir_mappings);
    assert_eq!(a.ignore_files, b.ignore_files);
    assert_eq!(a.cache_save_timer_enabled, b.cache_save_timer_enabled);
    assert_eq!(a.cache_save_time_period, b.cache_save_time_period);
    assert_eq!(a.check_chd_version, b.check_chd_version);
    assert_eq!(a.show_new_mia, b.show_new_mia);
    assert_eq!(a.seven_z_default_struct, b.seven_z_default_struct);
    assert_eq!(rule_summary(a), rule_summary(b));
}

#[test]
fn a_fresh_file_reads_back_to_the_same_settings() {
    let mut s = Settings::from_xml(CFG).expect("parses");
    s.dat_root = "Dats & <More>".to_owned();
    s.fix_level = FixLevel::Level1;
    s.check_chd_version = true;
    s.show_new_mia = false;
    s.cache_save_time_period = 3;
    s.dat_rules[1].ignore_files = vec!["a&b".to_owned(), "regex:^x$".to_owned()];
    s.dat_rules[1].category_order = vec!["Games".to_owned()];
    s.set_regex_rules().unwrap();

    let xml = s.to_xml(None);
    assert!(xml.starts_with("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<Settings xmlns:xsd="));
    assert!(xml.contains("<DatRoot>Dats &amp; &lt;More&gt;</DatRoot>"));
    assert!(xml.contains("<DirKey>RomVault\\Console\\DS\\Roms</DirKey>"));
    assert!(xml.ends_with("</Settings>"));
    assert_same(&s, &Settings::from_xml(&xml).expect("the output parses"));
}

#[test]
fn writing_twice_changes_nothing() {
    let s = Settings::from_xml(CFG_WITH_EXTRAS).unwrap();
    let once = s.to_xml(Some(CFG_WITH_EXTRAS));
    let twice = Settings::from_xml(&once).unwrap().to_xml(Some(&once));
    assert_eq!(once, twice);
}

#[test]
fn elements_the_crate_does_not_model_survive_byte_for_byte_and_in_place() {
    let mut s = Settings::from_xml(CFG_WITH_EXTRAS).unwrap();
    s.dat_root = "Other".to_owned();
    let xml = s.to_xml(Some(CFG_WITH_EXTRAS));

    for kept in [
        "  <EInfo>\n    <string>kept &amp; untouched</string>\n  </EInfo>",
        "  <DoubleCheckDelete>true</DoubleCheckDelete>",
        "  <Darkness>true</Darkness>",
    ] {
        assert!(xml.contains(kept), "{kept} is gone from\n{xml}");
    }
    // Order is the file's own: the owned elements stay where they were.
    let at = |needle: &str| xml.find(needle).unwrap_or_else(|| panic!("{needle}"));
    assert!(at("<DatRoot>Other") < at("<DatRules>"));
    assert!(at("<IgnoreFiles />") < at("<EInfo>"));
    assert!(at("<EInfo>") < at("<DoubleCheckDelete>"));
    assert!(at("<DoubleCheckDelete>") < at("<CacheSavePeriod>7"));
    assert!(at("<CacheSavePeriod>7") < at("<Darkness>"));
    // The old name of the period element is kept, and it is not written twice.
    assert!(!xml.contains("<CacheSaveTimePeriod>"));
    assert_eq!(xml.matches("<sevenZDefaultStruct>").count(), 1);
    assert!(xml.starts_with(&CFG_WITH_EXTRAS[..CFG_WITH_EXTRAS.find("<FilesOnly>").unwrap()]));
    assert!(xml.ends_with("</Settings>\n"));
    assert_same(&s, &Settings::from_xml(&xml).unwrap());
}

#[test]
fn an_edit_changes_only_what_was_edited() {
    let mut s = Settings::from_xml(CFG_WITH_EXTRAS).unwrap();
    s.dat_rules[0].merge = MergeType::NonMerged;
    s.dir_mappings.push(DirMapping {
        dir_key: "ToSort".to_owned(),
        dir_path: "ToSort".to_owned(),
    });
    s.ignore_files = vec!["*.bak".to_owned()];
    let back = Settings::from_xml(&s.to_xml(Some(CFG_WITH_EXTRAS))).unwrap();
    assert_eq!(back.dat_rules[0].merge, MergeType::NonMerged);
    assert_eq!(back.dir_mappings.len(), 2);
    assert_eq!(back.ignore_files, ["*.bak"]);
    assert_same(&s, &back);
}

#[test]
fn the_two_defaulted_elements_are_written_only_when_they_matter() {
    let s = Settings::from_xml(CFG_WITH_EXTRAS).unwrap();
    let xml = s.to_xml(Some(CFG_WITH_EXTRAS));
    assert!(!xml.contains("CheckCHDVersion"));
    assert!(!xml.contains("ShowNewMIA"));

    let mut t = s.clone();
    t.check_chd_version = true;
    t.show_new_mia = false;
    let xml = t.to_xml(Some(CFG_WITH_EXTRAS));
    assert!(xml.contains("<CheckCHDVersion>true</CheckCHDVersion>"));
    assert!(xml.contains("<ShowNewMIA>false</ShowNewMIA>"));
    assert_same(&t, &Settings::from_xml(&xml).unwrap());
}

#[test]
fn a_previous_text_that_is_not_a_settings_file_is_ignored() {
    let s = Settings::default();
    for junk in [
        "",
        "not xml",
        "<Other><a/></Other>",
        "<Settings />",
        "<Settings>",
    ] {
        let xml = s.to_xml(Some(junk));
        assert_eq!(xml, s.to_xml(None), "{junk:?}");
    }
}

/// The maintainer's own `RomVault3cfg.xml`: reading and writing it unchanged must keep every
/// element that ROMVault wrote except the empty legacy `<DirPath />` of a rule.
#[test]
#[ignore = "needs RV_FIXTURES_DIR"]
fn the_real_config_survives_a_write_apart_from_empty_legacy_dir_paths() {
    let dir = std::env::var_os("RV_FIXTURES_DIR")
        .expect("RV_FIXTURES_DIR is not set; this test needs RomVault3cfg.xml");
    let path = std::path::Path::new(&dir).join("RomVault3cfg.xml");
    let old = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let s = Settings::from_xml(&old).expect("the real config parses");
    let new = s.to_xml(Some(&old));

    assert_same(
        &s,
        &Settings::from_xml(&new).expect("the written config parses"),
    );
    let lines = |t: &str| -> Vec<String> {
        t.lines()
            .map(|l| l.trim_end().to_owned())
            .filter(|l| l.trim() != "<DirPath />")
            .collect()
    };
    // The only lines that differ are the ones the reader folded away; the ROMVault layout is
    // otherwise reproduced.
    let (a, b) = (lines(&old), lines(&new));
    let extra: Vec<&String> = a.iter().filter(|l| !b.contains(l)).collect();
    assert!(extra.is_empty(), "lines lost in the write: {extra:?}");
    assert_eq!(a.len(), b.len(), "line count changed");
}
