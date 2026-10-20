//! The data-loss fixes of `docs/fixing.md` section 15 and the invariants of section 17, one named
//! test each. The test names start with the invariant's number; the PR body maps them.

mod common;

use std::fs;
use std::time::{Duration, SystemTime};

use common::*;
use rv_cache::{load_database, DatabaseOptions};
use rv_core::progress::{CancelFlag, NoProgress};
use rv_core::{
    DatRule, Database, FileType, GotStatus, RepStatus, ScanLevel, TreeSelect, ZipStructure,
};
use rv_fix::{perform_fixes, FailAction, Failpoints, FixError, FixOptions};

const A: &[u8] = b"rom a contents rom a contents";
const B: &[u8] = b"second rom is a bit longer";
const C: &[u8] = b"third";
const J: &[u8] = b"junk junk junk";
const ROMS_DIR: &str = "RomRoot/Console/NES/Roms";
const LOOSE: &str = "RomVault/Console/NES/Loose";
const LOOSE_DIR: &str = "RomRoot/Console/NES/Loose";

fn inject(step: &str, nth: usize) -> FixOptions {
    FixOptions {
        failpoints: Failpoints::at(step, nth, FailAction::Error),
        save_every: None,
    }
}

fn zips(tag: &str) -> Lib {
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

fn loose_lib(tag: &str, games: &[Game<'_>]) -> Lib {
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

fn no_temps(l: &Lib) {
    for sub in ["RomRoot", "ToSort"] {
        for k in manifest(&l.path(sub)).keys() {
            assert!(!k.ends_with(".rvtmp"), "{sub}/{k} was left behind");
        }
    }
}

// ---- the four fixes the ticket asks for -----------------------------------------------------

#[test]
fn dl1_an_archive_is_replaced_by_one_rename_and_never_deleted_first() {
    let mut l = zips("dl1");
    let alpha = l.zip(
        &format!("{ROMS_DIR}/Alpha.zip"),
        &[("a.nes", A), ("b.nes", B), ("a2.nes", A), ("j.bin", J)],
    );
    let before = fs::read(&alpha).unwrap();
    l.scan(ScanLevel::Level2);

    // Stopped just before the rename: the old archive is whole, there is no half archive.
    let ff = l.find();
    let r = l.fix_with(ff, &inject("before_rename", 1));
    assert_eq!(r.error, Some(FixError::Injected("before_rename".into())));
    assert_eq!(
        fs::read(&alpha).unwrap(),
        before,
        "the old archive is untouched"
    );
    no_temps(&l);

    // Stopped just after: the new archive is whole.
    let mut l2 = zips("dl1b");
    l2.zip(
        &format!("{ROMS_DIR}/Alpha.zip"),
        &[("a.nes", A), ("b.nes", B), ("a2.nes", A)],
    );
    l2.scan(ScanLevel::Level2);
    let ff = l2.find();
    let r = l2.fix_with(ff, &inject("after_rename", 1));
    assert!(r.error.is_some());
    let bytes = l2.read(&format!("{ROMS_DIR}/Alpha.zip"));
    let mut z = rv_archive::zip::ZipArchive::open(std::io::Cursor::new(bytes)).unwrap();
    assert_eq!(z.len(), 2);
    z.verify_entry(0).unwrap();
    z.verify_entry(1).unwrap();
}

#[test]
fn dl2_what_was_written_is_checked_against_the_dat_before_it_takes_its_name() {
    // The DAT lists a SHA1 that the data does not have. A quick scan has not verified the
    // source's SHA1, so Find Fixes believes the CRC; the check after the write does not.
    let mut l = Lib::new("dl2");
    let mut dat = dat_text(&[("Beta", &[("c.nes", C)])]);
    let good = hex(&{
        let mut h = rv_scan::HashSet::new(rv_scan::HashKinds::ROMVAULT);
        h.update(C);
        h.finish().sha1.unwrap()
    });
    dat = dat.replace(&good, &"ab".repeat(20));
    l.write("DatRoot/Console/NES/Roms/nes.dat", dat.as_bytes());
    l.update_dats();
    let wrong = l.zip(
        &format!("{ROMS_DIR}/Wrong.zip"),
        &[("c.nes", C), ("j.bin", J)],
    );
    let wrong_before = fs::read(&wrong).unwrap();
    l.scan(ScanLevel::Level1);
    let ff = l.find();
    assert_eq!(
        l.status(&format!("{}/Beta.zip/c.nes", ROMS)),
        RepStatus::CAN_BE_FIXED
    );

    let r = l.fix_with(ff, &Default::default());
    assert!(
        matches!(r.error, Some(FixError::DestinationCheckSumMismatch(_))),
        "{:?}",
        r.error
    );
    assert!(
        !l.exists(&format!("{ROMS_DIR}/Beta.zip")),
        "nothing took the final name"
    );
    assert_eq!(
        fs::read(&wrong).unwrap(),
        wrong_before,
        "the source is untouched"
    );
    assert_ne!(l.got(&format!("{ROMS}/Beta.zip/c.nes")), GotStatus::GOT);
    no_temps(&l);
}

#[test]
fn dl3_inv01_a_source_stays_until_its_copy_is_in_place() {
    // Two games want the one file that is in a ToSort archive.
    let mut l = Lib::new("dl3");
    l.dat(
        "nes.dat",
        &[("Beta", &[("c.nes", C)]), ("Gamma", &[("c2.nes", C)])],
    );
    l.update_dats();
    let src = l.zip("ToSort/Wrong.zip", &[("c.nes", C), ("j.bin", J)]);
    let before = fs::read(&src).unwrap();
    l.scan(ScanLevel::Level2);
    let ff = l.find();
    assert_eq!(
        l.status("ToSort/Wrong.zip/c.nes"),
        RepStatus::NEEDED_FOR_FIX
    );

    // The second archive is stopped before it is renamed in.
    let r = l.fix_with(ff, &inject("before_rename", 2));
    assert!(r.error.is_some());
    assert_eq!(fs::read(&src).unwrap(), before, "the source is still whole");
    let placed = ["Beta.zip", "Gamma.zip"]
        .iter()
        .filter(|n| l.exists(&format!("{ROMS_DIR}/{n}")))
        .count();
    assert_eq!(placed, 1);

    // Finishing the job is what removes it.
    l.scan(ScanLevel::Level2);
    let r = l.fix();
    assert!(r.error.is_none(), "{:?}", r.error);
    assert!(
        l.exists(&format!("{ROMS_DIR}/Beta.zip")) && l.exists(&format!("{ROMS_DIR}/Gamma.zip"))
    );
    // The source archive keeps only what nobody asked for.
    let left = l.read("ToSort/Wrong.zip");
    let z = rv_archive::zip::ZipArchive::open(std::io::Cursor::new(left)).unwrap();
    let names: Vec<&str> = z.entries().iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["j.bin"]);
    l.assert_rescan_agrees();
}

#[test]
fn dl3b_a_moved_file_is_at_both_places_until_the_old_one_is_removed() {
    let mut l = loose_lib("dl3b", &[("G1", &[("a.bin", A), ("b.bin", B)])]);
    l.write(&format!("{LOOSE_DIR}/G1/a.bin"), A);
    l.write(&format!("{LOOSE_DIR}/G1/bee.bin"), B);
    l.scan(ScanLevel::Level2);
    let ff = l.find();
    let r = l.fix_with(ff, &inject("before_source_delete", 1));
    assert!(r.error.is_some());
    assert_eq!(l.read(&format!("{LOOSE_DIR}/G1/b.bin")), B);
    assert_eq!(
        l.read(&format!("{LOOSE_DIR}/G1/bee.bin")),
        B,
        "duplicates, not a loss"
    );
}

#[test]
fn dl4_the_cache_is_saved_on_an_error_on_a_cancel_and_at_the_end() {
    for case in ["error", "cancel", "done"] {
        let mut l = zips(&format!("dl4-{case}"));
        l.zip(
            &format!("{ROMS_DIR}/Alpha.zip"),
            &[("a.nes", A), ("b.nes", B), ("a2.nes", A)],
        );
        l.zip(&format!("{ROMS_DIR}/Wrong.zip"), &[("c.nes", C)]);
        l.scan(ScanLevel::Level2);
        l.save();
        let ff = l.find();
        let cancel = CancelFlag::new();
        let mut opts = FixOptions::default();
        match case {
            "error" => opts = inject("after_fsync", 1),
            "cancel" => cancel.cancel(),
            _ => {}
        }
        let mut saves = 0;
        let cache = l.cache.clone();
        let mut saver = |db: &mut Database| {
            saves += 1;
            rv_cache::save_database(&cache, db).map_err(|e| e.to_string())
        };
        let r = perform_fixes(
            &mut l.db,
            &l.settings,
            ff.groups,
            &opts,
            &NoProgress,
            &cancel,
            &mut saver,
        );
        assert_eq!(saves, 1, "{case}: saved once, at the end");
        assert!(r.save_error.is_none());
        assert_eq!(r.cancelled, case == "cancel", "{case}");
        assert_eq!(r.error.is_some(), case == "error", "{case}: {:?}", r.error);
        // The cache on disk loads and a scan puts it right.
        let (mut db, _) = load_database(&l.cache, &l.settings, DatabaseOptions::default()).unwrap();
        rv_scan::db::scan_files(
            &mut db,
            &l.settings,
            &Default::default(),
            &NoProgress,
            &CancelFlag::new(),
        )
        .unwrap();
        no_temps(&l);
    }
}

// ---- docs/fixing.md section 17 ---------------------------------------------------------------

#[test]
fn inv02_a_delete_needs_another_copy_that_is_on_disk() {
    let mut l = loose_lib("inv02", &[("G1", &[("a.bin", A)])]);
    l.write(&format!("{LOOSE_DIR}/G1/a.bin"), A);
    l.write(&format!("{LOOSE_DIR}/G1/again.bin"), A);
    l.scan(ScanLevel::Level2);
    let ff = l.find();
    assert_eq!(
        l.status(&format!("{LOOSE}/G1/again.bin")),
        RepStatus::DELETE
    );
    // The copy that makes the delete safe goes away behind the fixer's back.
    fs::remove_file(l.path(&format!("{LOOSE_DIR}/G1/a.bin"))).unwrap();
    let r = l.fix_with(ff, &Default::default());
    assert!(
        matches!(r.error, Some(FixError::RescanNeeded(_))),
        "{:?}",
        r.error
    );
    assert_eq!(
        l.read(&format!("{LOOSE_DIR}/G1/again.bin")),
        A,
        "the only copy is kept"
    );
}

#[test]
fn inv03_a_file_that_changed_since_the_scan_is_not_used() {
    let mut l = loose_lib("inv03", &[("G1", &[("a.bin", A)]), ("G2", &[("a.bin", A)])]);
    let src = l.write(&format!("{LOOSE_DIR}/G1/a.bin"), A);
    l.scan(ScanLevel::Level2);
    let ff = l.find();
    // Same length, new time: changed.
    fs::write(&src, b"AAAA AAAA").unwrap();
    let f = fs::OpenOptions::new().write(true).open(&src).unwrap();
    f.set_modified(SystemTime::now() + Duration::from_secs(5))
        .unwrap();
    drop(f);
    let r = l.fix_with(ff, &Default::default());
    assert!(
        matches!(r.error, Some(FixError::RescanNeeded(_))),
        "{:?}",
        r.error
    );
    assert!(!l.exists(&format!("{LOOSE_DIR}/G2/a.bin")));
    no_temps(&l);
}

#[test]
fn inv04_an_error_or_a_cancel_mid_write_leaves_the_original_and_no_temp() {
    for step in ["after_temp_write", "after_fsync", "before_rename"] {
        let mut l = zips(&format!("inv04-{step}"));
        let alpha = l.zip(
            &format!("{ROMS_DIR}/Alpha.zip"),
            &[("a.nes", A), ("b.nes", B), ("j.bin", J)],
        );
        let before = fs::read(&alpha).unwrap();
        l.scan(ScanLevel::Level2);
        let ff = l.find();
        let r = l.fix_with(ff, &inject(step, 1));
        assert!(r.error.is_some(), "{step}");
        assert_eq!(fs::read(&alpha).unwrap(), before, "{step}");
        no_temps(&l);
    }
    // A cancel while a file is being copied.
    let mut l = zips("inv04-cancel");
    l.zip(
        &format!("{ROMS_DIR}/Alpha.zip"),
        &[("a.nes", A), ("b.nes", B), ("j.bin", J)],
    );
    l.scan(ScanLevel::Level2);
    let ff = l.find();
    let cancel = CancelFlag::new();
    cancel.cancel();
    let cache = l.cache.clone();
    let mut saver =
        |db: &mut Database| rv_cache::save_database(&cache, db).map_err(|e| e.to_string());
    let r = perform_fixes(
        &mut l.db,
        &l.settings,
        ff.groups,
        &FixOptions::default(),
        &NoProgress,
        &cancel,
        &mut saver,
    );
    assert!(r.cancelled && r.error.is_none());
    no_temps(&l);
}

#[test]
fn inv05_nothing_is_overwritten_when_a_file_or_archive_appears_in_the_way() {
    let mut l = zips("inv05");
    l.zip(
        &format!("{ROMS_DIR}/Alpha.zip"),
        &[("a.nes", A), ("b.nes", B)],
    );
    l.zip(&format!("{ROMS_DIR}/Wrong.zip"), &[("c.nes", C)]);
    l.scan(ScanLevel::Level2);
    let ff = l.find();
    // Something takes Beta.zip's place after the scan.
    let intruder = l.write(&format!("{ROMS_DIR}/Beta.zip"), b"someone else's file");
    let r = l.fix_with(ff, &Default::default());
    assert!(
        matches!(r.error, Some(FixError::RescanNeeded(_))),
        "{:?}",
        r.error
    );
    assert_eq!(fs::read(&intruder).unwrap(), b"someone else's file");
    // A name taken in ToSort is numbered (see fix_loose).
    no_temps(&l);
}

#[test]
fn inv06_files_nobody_wants_go_to_tosort_and_are_not_deleted() {
    let mut l = zips("inv06");
    l.zip(
        &format!("{ROMS_DIR}/Alpha.zip"),
        &[("a.nes", A), ("b.nes", B)],
    );
    let stranger = l.zip(&format!("{ROMS_DIR}/Stranger.zip"), &[("x.bin", J)]);
    let loose = l.write(&format!("{ROMS_DIR}/readme.txt"), b"hello");
    let (s, t) = (fs::read(&stranger).unwrap(), fs::read(&loose).unwrap());
    l.scan(ScanLevel::Level2);
    let r = l.fix();
    assert!(r.error.is_none(), "{:?}", r.error);
    assert_eq!(l.read("ToSort/Console/NES/Roms/Stranger.zip"), s);
    assert_eq!(l.read("ToSort/Console/NES/Roms/readme.txt"), t);
    assert!(!stranger.exists() && !loose.exists());
}

#[test]
fn inv07_cleaning_up_a_directory_is_not_recursive_and_never_takes_a_root() {
    let mut l = loose_lib("inv07", &[("G1", &[("a.bin", A)])]);
    l.write(&format!("{LOOSE_DIR}/G1/a.bin"), A);
    l.write(&format!("{LOOSE_DIR}/Old/junk.bin"), J);
    l.write(&format!("{LOOSE_DIR}/Late/junk2.bin"), b"junk two");
    l.scan(ScanLevel::Level2);
    let ff = l.find();
    // A file the scan never saw appears in Late.
    l.write(&format!("{LOOSE_DIR}/Late/new.txt"), b"unscanned");
    let r = l.fix_with(ff, &Default::default());
    assert!(r.error.is_none(), "{:?}", r.error);
    assert!(
        !l.exists(&format!("{LOOSE_DIR}/Old")),
        "an emptied directory goes"
    );
    assert!(
        l.exists(&format!("{LOOSE_DIR}/Late/new.txt")),
        "a directory with a file in it stays"
    );
    assert!(
        l.node(&format!("{LOOSE}/Late")).is_some(),
        "and so does its node"
    );
    assert!(l.exists("RomRoot"), "the root of the tree is never removed");
}

#[test]
fn inv08_only_ticked_directories_are_written_and_locked_ones_only_read() {
    let mut l = Lib::new("inv08");
    for dir in ["Loose", "Keep", "Off"] {
        let mut rule = DatRule::default();
        rule.dir_key = format!("RomVault/Console/NES/{dir}");
        rule.compression = FileType::FILE_ONLY;
        l.settings.dat_rules.push(rule);
    }
    l.write(
        "DatRoot/Console/NES/Loose/l.dat",
        dat_text(&[("G2", &[("a.bin", A)])]).as_bytes(),
    );
    l.write(
        "DatRoot/Console/NES/Keep/k.dat",
        dat_text(&[("G1", &[("a.bin", A)])]).as_bytes(),
    );
    l.write(
        "DatRoot/Console/NES/Off/o.dat",
        dat_text(&[("S1", &[("s.bin", B)])]).as_bytes(),
    );
    l.update_dats();
    l.write("RomRoot/Console/NES/Keep/G1/a.bin", A);
    l.write("RomRoot/Console/NES/Keep/G1/stray.bin", J);
    l.write(
        "RomRoot/Console/NES/Off/S1/other.bin",
        b"in an unticked directory",
    );
    l.write("RomRoot/Console/NES/Off/S1/s_copy.bin", B);

    let set = |l: &mut Lib, dir: &str, sel: TreeSelect| {
        let mut cur = &mut l.db.root;
        for part in dir.split('/') {
            cur = cur.children.iter_mut().find(|c| c.name == part).unwrap();
        }
        cur.tree.as_mut().unwrap().checked = sel;
    };
    set(&mut l, "RomVault/Console/NES/Off", TreeSelect::UN_SELECTED);
    set(&mut l, "RomVault/Console/NES/Keep", TreeSelect::LOCKED);
    l.scan(ScanLevel::Level2);
    let ff = l.find();
    let r = l.fix_with(ff, &Default::default());
    assert!(r.error.is_none(), "{:?}", r.error);
    // G2 (ticked) was made from the locked G1/a.bin, which was copied, not moved.
    assert_eq!(l.read("RomRoot/Console/NES/Loose/G2/a.bin"), A);
    assert_eq!(l.read("RomRoot/Console/NES/Keep/G1/a.bin"), A);
    // The locked directory's stray file was not moved, the unticked one was not touched.
    assert_eq!(l.read("RomRoot/Console/NES/Keep/G1/stray.bin"), J);
    assert_eq!(
        l.read("RomRoot/Console/NES/Off/S1/other.bin"),
        b"in an unticked directory"
    );
    assert!(!l.exists("RomRoot/Console/NES/Off/S1/s.bin"));
    assert!(l.manifest("ToSort").is_empty());
}

#[test]
fn inv09_a_source_that_does_not_read_back_as_recorded_is_corrupt_and_not_copied() {
    let mut l = loose_lib("inv09", &[("G1", &[("a.bin", A)]), ("G2", &[("a.bin", A)])]);
    let src = l.write(&format!("{LOOSE_DIR}/G1/a.bin"), A);
    l.scan(ScanLevel::Level2);
    let ff = l.find();
    // The bytes change under the recorded time stamp and length.
    let stamp = fs::metadata(&src).unwrap().modified().unwrap();
    let changed: Vec<u8> = A.iter().map(|b| b.to_ascii_uppercase()).collect();
    fs::write(&src, changed).unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(&src)
        .unwrap()
        .set_modified(stamp)
        .unwrap();
    let r = l.fix_with(ff, &Default::default());
    assert!(
        matches!(r.error, Some(FixError::SourceDataStreamCorrupt(_))),
        "{:?}",
        r.error
    );
    assert!(!l.exists(&format!("{LOOSE_DIR}/G2/a.bin")));
    assert_eq!(l.got(&format!("{LOOSE}/G1/a.bin")), GotStatus::CORRUPT);
    no_temps(&l);
}

#[test]
fn inv10_a_failed_check_leaves_no_record_that_says_the_file_is_there() {
    // See dl2: the DAT's SHA1 is wrong, the write is refused, the entry is not marked as got.
    let mut l = Lib::new("inv10");
    let good = {
        let mut h = rv_scan::HashSet::new(rv_scan::HashKinds::ROMVAULT);
        h.update(C);
        hex(&h.finish().sha1.unwrap())
    };
    let dat = dat_text(&[("Beta", &[("c.nes", C)])]).replace(&good, &"cd".repeat(20));
    l.write("DatRoot/Console/NES/Roms/nes.dat", dat.as_bytes());
    l.update_dats();
    l.zip(
        &format!("{ROMS_DIR}/Wrong.zip"),
        &[("c.nes", C), ("j.bin", J)],
    );
    l.scan(ScanLevel::Level1);
    let ff = l.find();
    let r = l.fix_with(ff, &Default::default());
    assert!(r.error.is_some());
    assert_ne!(l.got(&format!("{ROMS}/Beta.zip/c.nes")), GotStatus::GOT);
    assert_ne!(l.got(&format!("{ROMS}/Beta.zip")), GotStatus::GOT);
}

#[test]
fn inv11_a_rebuilt_zip_has_its_members_in_torrentzip_order_and_the_tree_records_the_structure() {
    let mut l = Lib::new("inv11");
    l.dat(
        "nes.dat",
        &[(
            "Alpha",
            &[
                ("B.nes", B),
                ("a.nes", A),
                ("sub/c.nes", C),
                ("sub/a.nes", J),
            ],
        )],
    );
    l.update_dats();
    l.zip(
        &format!("{ROMS_DIR}/Alpha.zip"),
        &[("a.nes", A), ("B.nes", B), ("junk", b"zzz")],
    );
    l.zip(&format!("{ROMS_DIR}/Src.zip"), &[("c", C), ("j", J)]);
    l.scan(ScanLevel::Level2);
    let r = l.fix();
    assert!(r.error.is_none(), "{:?}", r.error);
    let bytes = l.read(&format!("{ROMS_DIR}/Alpha.zip"));
    let z = rv_archive::zip::ZipArchive::open(std::io::Cursor::new(bytes)).unwrap();
    assert_eq!(z.structure(), rv_archive::zip::ZipStructure::ZipTrrnt);
    let names: Vec<&str> = z.entries().iter().map(|e| e.name.as_str()).collect();
    let mut sorted = names.clone();
    sorted.sort_by(|a, b| rv_archive::sort::trrntzip_compare_case(a, b));
    assert_eq!(names, sorted);
    assert_eq!(names.len(), 4);
    assert_eq!(
        l.node(&format!("{ROMS}/Alpha.zip")).unwrap().zip_struct,
        Some(ZipStructure::ZIP_TRRNT)
    );
    l.assert_rescan_agrees();
}

#[test]
fn inv12_no_half_applied_state_after_an_error() {
    // The tree after a stopped fix still agrees with the disk once the cache is saved and the
    // disk is scanned; the statuses that were set are re-made by the next Find Fixes.
    let mut l = zips("inv12");
    l.zip(
        &format!("{ROMS_DIR}/Alpha.zip"),
        &[("a.nes", A), ("b.nes", B), ("j.bin", J)],
    );
    l.zip(&format!("{ROMS_DIR}/Wrong.zip"), &[("c.nes", C)]);
    l.scan(ScanLevel::Level2);
    let ff = l.find();
    let r = l.fix_with(ff, &inject("before_rename", 2));
    assert!(r.error.is_some());
    l.assert_rescan_agrees();
}

#[test]
fn inv13_names_take_the_dats_case_not_the_disks() {
    let mut l = loose_lib("inv13", &[("G1", &[("a.bin", A)])]);
    l.write(&format!("{LOOSE_DIR}/g1/A.BIN"), A);
    l.scan(ScanLevel::Level2);
    let ff = l.find();
    let r = l.fix_with(ff, &Default::default());
    assert!(r.error.is_none(), "{:?}", r.error);
    let names: Vec<String> = fs::read_dir(l.path(LOOSE_DIR))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["G1"]);
    assert_eq!(l.read(&format!("{LOOSE_DIR}/G1/a.bin")), A);
    l.assert_rescan_agrees();
}
