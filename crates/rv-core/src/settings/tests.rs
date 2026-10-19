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
