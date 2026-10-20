//! Find Fixes on small libraries: which status each file gets.

mod common;

use common::*;
use rv_core::{RepStatus, ScanLevel};

const A: &[u8] = b"rom a contents rom a contents";
const B: &[u8] = b"second rom is a bit longer";
const C: &[u8] = b"third";

fn lib(tag: &str) -> Lib {
    let mut l = Lib::new(tag);
    l.dat(
        "nes.dat",
        &[
            ("Alpha", &[("a.nes", A), ("b.nes", B)]),
            ("Beta", &[("c.nes", C)]),
        ],
    );
    l.update_dats();
    l
}

#[test]
fn a_correct_library_needs_nothing() {
    let mut l = lib("correct");
    l.zip(
        "RomRoot/Console/NES/Roms/Alpha.zip",
        &[("a.nes", A), ("b.nes", B)],
    );
    l.zip("RomRoot/Console/NES/Roms/Beta.zip", &[("c.nes", C)]);
    l.scan(ScanLevel::Level2);
    let ff = l.find();
    assert!(ff.report.warnings.is_empty(), "{:?}", ff.report.warnings);
    for (p, s) in l.statuses(ROMS) {
        assert_eq!(s, RepStatus::CORRECT, "{p}");
    }
}

#[test]
fn a_file_in_the_wrong_archive_can_fix_the_missing_one_and_is_then_needed() {
    let mut l = lib("wrong");
    // Beta's rom is sitting in Alpha, which also lacks b.nes.
    l.zip(
        "RomRoot/Console/NES/Roms/Alpha.zip",
        &[("a.nes", A), ("c.nes", C)],
    );
    l.scan(ScanLevel::Level2);
    l.find();
    assert_eq!(
        l.status(&format!("{ROMS}/Beta.zip/c.nes")),
        RepStatus::CAN_BE_FIXED
    );
    assert_eq!(
        l.status(&format!("{ROMS}/Alpha.zip/c.nes")),
        RepStatus::NEEDED_FOR_FIX
    );
    assert_eq!(
        l.status(&format!("{ROMS}/Alpha.zip/b.nes")),
        RepStatus::MISSING
    );
    assert_eq!(
        l.status(&format!("{ROMS}/Alpha.zip/a.nes")),
        RepStatus::CORRECT
    );
}

#[test]
fn an_unknown_file_with_no_use_goes_to_tosort_and_a_duplicate_is_deleted() {
    let mut l = lib("unknown");
    l.zip(
        "RomRoot/Console/NES/Roms/Alpha.zip",
        &[("a.nes", A), ("b.nes", B)],
    );
    l.zip("RomRoot/Console/NES/Roms/Beta.zip", &[("c.nes", C)]);
    l.zip(
        "RomRoot/Console/NES/Roms/Junk.zip",
        &[("j.bin", b"junk junk"), ("a2.nes", A)],
    );
    l.scan(ScanLevel::Level2);
    l.find();
    assert_eq!(
        l.status(&format!("{ROMS}/Junk.zip/j.bin")),
        RepStatus::MOVE_TO_SORT
    );
    assert_eq!(
        l.status(&format!("{ROMS}/Junk.zip/a2.nes")),
        RepStatus::DELETE
    );
}
