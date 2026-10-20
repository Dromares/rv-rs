//! The dry-run plan and the FixDat report.

mod common;

use common::*;
use rv_core::ScanLevel;
use rv_fix::action;
use rv_fix::fixdat::fix_dats;
use rv_fix::plan::{plan, Action};

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
    l.zip(
        "RomRoot/Console/NES/Roms/Alpha.zip",
        &[("a.nes", A), ("b.nes", B), ("j.bin", J)],
    );
    l.zip("RomRoot/Console/NES/Roms/Wrong.zip", &[("c.nes", C)]);
    l.zip("RomRoot/Console/NES/Roms/Stranger.zip", &[("x.bin", J)]);
    l.scan(ScanLevel::Level2);
    l
}

#[test]
fn the_plan_changes_nothing_and_matches_what_the_fix_then_does() {
    let mut l = lib("plan");
    let ff = l.find();
    let tree_before = flatten(&l.db.root);
    let disk_before = l.manifest(".");
    let p = plan(&l.db.root, &l.settings);
    assert_eq!(flatten(&l.db.root), tree_before);
    assert_eq!(l.manifest("."), disk_before);

    assert_eq!(p.count(Action::MoveArchiveToSort), 1, "Stranger.zip");
    assert_eq!(
        p.count(Action::RebuildArchive),
        2,
        "Alpha (junk out), Beta (made)"
    );
    assert_eq!(
        p.count(Action::DeferredArchive),
        1,
        "Wrong.zip waits until c.nes is used"
    );
    assert_eq!(p.count(Action::MakeMember), 1, "c.nes in Beta.zip");
    assert_eq!(p.still_missing, 1, "d.nes has no source");

    let r = l.fix_with(ff, &Default::default());
    assert!(r.error.is_none(), "{:?}", r.error);
    // The archive that waited is rebuilt once what it holds has been used.
    assert_eq!(
        r.count(action::REBUILD),
        p.count(Action::RebuildArchive) + p.count(Action::DeferredArchive)
    );
    assert_eq!(
        r.count(action::ARCHIVE_TO_SORT),
        p.count(Action::MoveArchiveToSort)
    );
    assert_eq!(r.fixed as usize, p.count(Action::MakeMember));
}

#[test]
fn an_archive_with_a_structure_the_fixer_cannot_write_is_left_alone_and_reported() {
    // A plain zip (stored entries, no TorrentZip) in ToSort holds what Beta needs. Beta.zip is
    // made from it; the plain zip is then to lose that entry, and rv-archive has no writer for
    // an unstructured zip, so it is left as it is.
    let mut l = Lib::new("unsupported");
    l.dat("nes.dat", &[("Beta", &[("c.nes", C)])]);
    l.update_dats();
    let plain = {
        let mut out = Vec::new();
        drop(zip_stored(&mut out, &[("c.nes", C), ("j.bin", J)]));
        out
    };
    l.write("ToSort/plain.zip", &plain);
    l.scan(ScanLevel::Level2);
    let ff = l.find();
    let r = l.fix_with(ff, &Default::default());
    assert!(r.error.is_none(), "{:?}", r.error);
    assert_eq!(
        l.status("RomVault/Console/NES/Roms/Beta.zip/c.nes"),
        rv_core::RepStatus::CORRECT
    );
    assert_eq!(l.read("ToSort/plain.zip"), plain, "left exactly as it was");
    assert!(
        r.skipped
            .iter()
            .any(|(what, why)| what.ends_with("plain.zip") && why.contains("structure")),
        "{:?}",
        r.skipped
    );
}

/// A zip with stored entries, written by hand, so it is not a TorrentZip.
fn zip_stored<'a>(out: &'a mut Vec<u8>, entries: &[(&str, &[u8])]) -> impl std::io::Write + 'a {
    struct Z<'a>(&'a mut Vec<u8>);
    impl std::io::Write for Z<'_> {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut central = Vec::new();
    let mut count = 0u16;
    for (name, data) in entries {
        let off = out.len() as u32;
        let crc = crc32fast::hash(data);
        out.extend(0x0403_4b50u32.to_le_bytes());
        out.extend([20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        out.extend(crc.to_le_bytes());
        out.extend((data.len() as u32).to_le_bytes());
        out.extend((data.len() as u32).to_le_bytes());
        out.extend((name.len() as u16).to_le_bytes());
        out.extend([0, 0]);
        out.extend(name.as_bytes());
        out.extend(*data);
        central.extend(0x0201_4b50u32.to_le_bytes());
        central.extend([20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        central.extend(crc.to_le_bytes());
        central.extend((data.len() as u32).to_le_bytes());
        central.extend((data.len() as u32).to_le_bytes());
        central.extend((name.len() as u16).to_le_bytes());
        central.extend([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        central.extend(off.to_le_bytes());
        central.extend(name.as_bytes());
        count += 1;
    }
    let cd_off = out.len() as u32;
    let cd_len = central.len() as u32;
    out.extend(central);
    out.extend(0x0605_4b50u32.to_le_bytes());
    out.extend([0, 0, 0, 0]);
    out.extend(count.to_le_bytes());
    out.extend(count.to_le_bytes());
    out.extend(cd_len.to_le_bytes());
    out.extend(cd_off.to_le_bytes());
    out.extend([0, 0]);
    Z(out)
}

#[test]
fn fixdats_list_what_is_missing_and_leave_out_what_can_be_fixed() {
    let mut l = lib("fixdat");
    l.find();
    let red = fix_dats(&l.db.root, true, "2026-10-19");
    assert_eq!(red.len(), 1);
    assert_eq!(red[0].file_name, "fixDat_Console_NES_Roms_nes.dat");
    let h = rv_dat::read_dat(red[0].text.as_bytes(), &red[0].file_name).unwrap();
    assert_eq!(h.name, "FixDat_T");
    assert_eq!(h.author, "RomVault");
    assert_eq!(h.date, "2026-10-19");
    let games: Vec<&str> = h.base_dir.children.iter().map(|c| c.name()).collect();
    assert_eq!(
        games,
        ["Gamma"],
        "Beta can be fixed from Wrong.zip, Alpha is right"
    );
    assert_eq!(red[0].missing, 1);

    let all = fix_dats(&l.db.root, false, "2026-10-19");
    let h = rv_dat::read_dat(all[0].text.as_bytes(), &all[0].file_name).unwrap();
    let games: Vec<&str> = h.base_dir.children.iter().map(|c| c.name()).collect();
    assert_eq!(games, ["Beta", "Gamma"]);
}

#[test]
fn a_library_with_nothing_missing_has_no_fixdat() {
    let mut l = Lib::new("nofixdat");
    l.dat("nes.dat", &[("Alpha", &[("a.nes", A)])]);
    l.update_dats();
    l.zip("RomRoot/Console/NES/Roms/Alpha.zip", &[("a.nes", A)]);
    l.scan(ScanLevel::Level2);
    l.find();
    assert!(fix_dats(&l.db.root, false, "2026-10-19").is_empty());
}
