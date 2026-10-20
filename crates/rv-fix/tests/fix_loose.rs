//! Fixing loose files (a DAT directory that holds files, not archives).

mod common;

use common::*;
use rv_core::{DatRule, FileType, RepStatus, ScanLevel};

const A: &[u8] = b"aaaa aaaa";
const B: &[u8] = b"bbbb bbbb";
const J: &[u8] = b"junk junk junk";
const LOOSE: &str = "RomVault/Console/NES/Loose";

fn lib(tag: &str, games: &[Game<'_>]) -> Lib {
    let mut l = Lib::new(tag);
    let mut rule = DatRule::default();
    rule.dir_key = LOOSE.to_owned();
    rule.compression = FileType::FILE_ONLY;
    l.settings.dat_rules.push(rule);
    l.write(
        "DatRoot/Console/NES/Loose/loose.dat",
        dat_text(games).as_bytes(),
    );
    l.update_dats();
    l
}

#[test]
fn a_misnamed_loose_file_is_moved_to_where_the_dat_wants_it() {
    let mut l = lib("move", &[("G1", &[("a.bin", A), ("b.bin", B)])]);
    l.write("RomRoot/Console/NES/Loose/G1/a.bin", A);
    l.write("RomRoot/Console/NES/Loose/G1/bee.bin", B);
    l.scan(ScanLevel::Level2);
    let ff = l.find();
    assert_eq!(
        l.status(&format!("{LOOSE}/G1/b.bin")),
        RepStatus::CAN_BE_FIXED
    );
    assert_eq!(l.status(&format!("{LOOSE}/G1/bee.bin")), RepStatus::RENAME);

    let r = l.fix_with(ff, &Default::default());
    assert!(r.error.is_none(), "{:?}", r.error);
    assert_eq!(r.count(rv_fix::action::MOVE), 1);
    assert_eq!(l.read("RomRoot/Console/NES/Loose/G1/b.bin"), B);
    assert!(!l.exists("RomRoot/Console/NES/Loose/G1/bee.bin"));
    assert!(l.manifest("ToSort").is_empty());
    l.assert_rescan_agrees();
}

#[test]
fn a_file_that_is_wanted_twice_is_copied_and_the_original_stays() {
    let mut l = lib("copy", &[("G1", &[("b.bin", B)]), ("G2", &[("b.bin", B)])]);
    l.write("RomRoot/Console/NES/Loose/G1/b.bin", B);
    l.scan(ScanLevel::Level2);
    let r = l.fix();
    assert!(r.error.is_none(), "{:?}", r.error);
    assert_eq!(r.count(rv_fix::action::COPY), 1);
    assert_eq!(l.read("RomRoot/Console/NES/Loose/G1/b.bin"), B);
    assert_eq!(l.read("RomRoot/Console/NES/Loose/G2/b.bin"), B);
    l.assert_rescan_agrees();
}

#[test]
fn an_unwanted_loose_file_goes_to_tosort_and_a_duplicate_is_deleted() {
    let mut l = lib("tosort", &[("G1", &[("a.bin", A)])]);
    l.write("RomRoot/Console/NES/Loose/G1/a.bin", A);
    l.write("RomRoot/Console/NES/Loose/G1/junk.bin", J);
    l.write("RomRoot/Console/NES/Loose/G1/again.bin", A);
    l.scan(ScanLevel::Level2);
    let ff = l.find();
    assert_eq!(
        l.status(&format!("{LOOSE}/G1/junk.bin")),
        RepStatus::MOVE_TO_SORT
    );
    assert_eq!(
        l.status(&format!("{LOOSE}/G1/again.bin")),
        RepStatus::DELETE
    );

    let r = l.fix_with(ff, &Default::default());
    assert!(r.error.is_none(), "{:?}", r.error);
    assert_eq!(l.read("ToSort/Console/NES/Loose/G1/junk.bin"), J);
    assert!(!l.exists("RomRoot/Console/NES/Loose/G1/junk.bin"));
    assert!(!l.exists("RomRoot/Console/NES/Loose/G1/again.bin"));
    assert_eq!(l.read("RomRoot/Console/NES/Loose/G1/a.bin"), A);
    // Only the unwanted file was kept; the duplicate is not in ToSort.
    let keys: Vec<String> = l.manifest("ToSort").into_keys().collect();
    assert_eq!(keys, ["Console/NES/Loose/G1/junk.bin"]);
    l.assert_rescan_agrees();
}

#[test]
fn a_name_that_is_taken_in_tosort_gets_a_number() {
    let mut l = lib("taken", &[("G1", &[("a.bin", A)])]);
    l.write("RomRoot/Console/NES/Loose/G1/a.bin", A);
    l.write("RomRoot/Console/NES/Loose/G1/junk.bin", J);
    l.write("ToSort/Console/NES/Loose/G1/junk.bin", b"older junk");
    l.scan(ScanLevel::Level2);
    // The ToSort directory is scanned too.
    let r = l.fix();
    assert!(r.error.is_none(), "{:?}", r.error);
    assert_eq!(
        l.read("ToSort/Console/NES/Loose/G1/junk.bin"),
        b"older junk"
    );
    assert_eq!(l.read("ToSort/Console/NES/Loose/G1/junk_0.bin"), J);
    l.assert_rescan_agrees();
}

#[test]
fn a_file_in_tosort_that_is_wanted_comes_back() {
    let mut l = lib("back", &[("G1", &[("a.bin", A), ("b.bin", B)])]);
    l.write("RomRoot/Console/NES/Loose/G1/a.bin", A);
    l.write("ToSort/stray/anything.bin", B);
    l.scan(ScanLevel::Level2);
    let r = l.fix();
    assert!(r.error.is_none(), "{:?}", r.error);
    assert_eq!(l.read("RomRoot/Console/NES/Loose/G1/b.bin"), B);
    assert!(!l.exists("ToSort/stray/anything.bin"));
    l.assert_rescan_agrees();
}
