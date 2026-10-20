//! Fixing zips: rebuilt as TorrentZips, with what leaves them going to ToSort and nothing lost.

mod common;

use common::*;
use rv_core::{GotStatus, RepStatus, ScanLevel};

const A: &[u8] = b"rom a contents rom a contents";
const B: &[u8] = b"second rom is a bit longer";
const C: &[u8] = b"third";
const D: &[u8] = b"fourth rom data data data";
const J: &[u8] = b"junk junk junk";

fn lib(tag: &str) -> Lib {
    let mut l = Lib::new(tag);
    l.dat(
        "nes.dat",
        &[
            ("Alpha", &[("a.nes", A), ("b.nes", B)]),
            ("Beta", &[("c.nes", C)]),
            ("Gamma", &[("d.nes", D)]),
        ],
    );
    l.update_dats();
    l
}

fn zip_is_trrntzip(l: &Lib, rel: &str) -> bool {
    let f = std::fs::File::open(l.path(rel)).unwrap();
    let z = rv_archive::zip::ZipArchive::open(std::io::BufReader::new(f)).unwrap();
    z.structure() == rv_archive::zip::ZipStructure::ZipTrrnt
}

#[test]
fn a_missing_game_is_made_from_the_wrong_archive_and_the_rest_goes_to_tosort() {
    let mut l = lib("wrong-archive");
    l.zip(
        "RomRoot/Console/NES/Roms/Alpha.zip",
        &[("a.nes", A), ("b.nes", B)],
    );
    l.zip(
        "RomRoot/Console/NES/Roms/Wrong.zip",
        &[("c.nes", C), ("j.bin", J)],
    );
    l.scan(ScanLevel::Level2);

    let r = l.fix();
    assert!(r.error.is_none(), "{:?}", r.error);
    assert!(r.save_error.is_none(), "{:?}", r.save_error);

    // Beta.zip exists, is a TorrentZip, and is correct.
    assert!(zip_is_trrntzip(&l, "RomRoot/Console/NES/Roms/Beta.zip"));
    assert_eq!(
        l.status(&format!("{ROMS}/Beta.zip/c.nes")),
        RepStatus::CORRECT
    );
    // Wrong.zip is gone from the library; the junk is in ToSort, whole.
    assert!(!l.exists("RomRoot/Console/NES/Roms/Wrong.zip"));
    assert!(l.node(&format!("{ROMS}/Wrong.zip")).is_none());
    let tos = "ToSort/Console/NES/Roms/Wrong.zip";
    assert!(
        l.exists(tos),
        "ToSort holds the unwanted file: {:?}",
        l.manifest("ToSort").keys().collect::<Vec<_>>()
    );
    assert!(zip_is_trrntzip(&l, tos));
    // Nothing is left half done on disk.
    for k in l.manifest(".").keys() {
        assert!(!k.ends_with(".rvtmp"), "{k}");
    }
    l.assert_rescan_agrees();
}

#[test]
fn an_archive_with_nothing_wanted_moves_to_tosort_byte_for_byte() {
    let mut l = lib("whole-move");
    l.zip(
        "RomRoot/Console/NES/Roms/Alpha.zip",
        &[("a.nes", A), ("b.nes", B)],
    );
    let junk = l.zip("RomRoot/Console/NES/Roms/Stranger.zip", &[("j.bin", J)]);
    let before = std::fs::read(&junk).unwrap();
    l.scan(ScanLevel::Level2);

    let r = l.fix();
    assert!(r.error.is_none(), "{:?}", r.error);
    assert_eq!(r.count(rv_fix::action::ARCHIVE_TO_SORT), 1);
    assert!(!l.exists("RomRoot/Console/NES/Roms/Stranger.zip"));
    assert_eq!(l.read("ToSort/Console/NES/Roms/Stranger.zip"), before);
    assert_eq!(
        l.status("ToSort/Console/NES/Roms/Stranger.zip/j.bin"),
        RepStatus::IN_TO_SORT
    );
    l.assert_rescan_agrees();
}

#[test]
fn a_duplicate_member_is_dropped_once_its_twin_is_proven_and_the_rest_kept() {
    let mut l = lib("duplicate");
    l.zip(
        "RomRoot/Console/NES/Roms/Alpha.zip",
        &[("a.nes", A), ("b.nes", B), ("a2.nes", A), ("j.bin", J)],
    );
    l.scan(ScanLevel::Level2);
    // a2.nes is Delete (a.nes is next to it), j.bin MoveToSort.
    let ff = l.find();
    assert_eq!(
        l.status(&format!("{ROMS}/Alpha.zip/a2.nes")),
        RepStatus::DELETE
    );
    assert_eq!(
        l.status(&format!("{ROMS}/Alpha.zip/j.bin")),
        RepStatus::MOVE_TO_SORT
    );

    let r = l.fix_with(ff, &Default::default());
    assert!(r.error.is_none(), "{:?}", r.error);
    assert_eq!(
        l.status(&format!("{ROMS}/Alpha.zip/a.nes")),
        RepStatus::CORRECT
    );
    assert!(l.node(&format!("{ROMS}/Alpha.zip/a2.nes")).is_none());
    assert!(l.node(&format!("{ROMS}/Alpha.zip/j.bin")).is_none());
    assert!(zip_is_trrntzip(&l, "RomRoot/Console/NES/Roms/Alpha.zip"));
    // j.bin is in ToSort and a2.nes (a duplicate) is not kept anywhere.
    let keys: Vec<String> = l.manifest("ToSort").into_keys().collect();
    assert_eq!(keys, ["Console/NES/Roms/Alpha.zip"], "{keys:?}");
    l.assert_rescan_agrees();
}

#[test]
fn a_member_that_only_needs_another_name_is_renamed_inside_its_archive() {
    let mut l = lib("rename");
    l.zip(
        "RomRoot/Console/NES/Roms/Alpha.zip",
        &[("a.nes", A), ("bee.nes", B)],
    );
    l.scan(ScanLevel::Level2);
    let ff = l.find();
    assert_eq!(
        l.status(&format!("{ROMS}/Alpha.zip/b.nes")),
        RepStatus::CAN_BE_FIXED
    );
    assert_eq!(
        l.status(&format!("{ROMS}/Alpha.zip/bee.nes")),
        RepStatus::RENAME
    );

    let r = l.fix_with(ff, &Default::default());
    assert!(r.error.is_none(), "{:?}", r.error);
    assert_eq!(
        l.status(&format!("{ROMS}/Alpha.zip/b.nes")),
        RepStatus::CORRECT
    );
    assert!(l.node(&format!("{ROMS}/Alpha.zip/bee.nes")).is_none());
    assert!(
        l.manifest("ToSort").is_empty(),
        "a rename keeps nothing in ToSort"
    );
    l.assert_rescan_agrees();
}

#[test]
fn an_empty_rom_is_made_without_a_source() {
    let mut l = Lib::new("empty");
    l.dat("nes.dat", &[("Alpha", &[("a.nes", A), ("empty.bin", b"")])]);
    l.update_dats();
    l.zip("RomRoot/Console/NES/Roms/Alpha.zip", &[("a.nes", A)]);
    l.scan(ScanLevel::Level2);
    let r = l.fix();
    assert!(r.error.is_none(), "{:?}", r.error);
    assert_eq!(
        l.status(&format!("{ROMS}/Alpha.zip/empty.bin")),
        RepStatus::CORRECT
    );
    assert_eq!(
        l.got(&format!("{ROMS}/Alpha.zip/empty.bin")),
        GotStatus::GOT
    );
    l.assert_rescan_agrees();
}

#[test]
fn a_corrupt_archive_is_moved_to_the_corrupt_folder_unchanged() {
    let mut l = lib("corrupt");
    let bad = b"this is not a zip at all, but it is named like one";
    l.write("RomRoot/Console/NES/Roms/Alpha.zip", bad);
    l.scan(ScanLevel::Level2);
    assert_eq!(l.got(&format!("{ROMS}/Alpha.zip")), GotStatus::CORRUPT);
    let r = l.fix();
    assert!(r.error.is_none(), "{:?}", r.error);
    assert!(!l.exists("RomRoot/Console/NES/Roms/Alpha.zip"));
    assert_eq!(l.read("ToSort/Corrupt/Alpha.zip"), bad);
    l.assert_rescan_agrees();
}
