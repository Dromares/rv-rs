//! The DAT update on small trees in a temp directory, and the predicates it is built from.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use rv_core::progress::{CancelFlag, NoProgress};
use rv_core::{
    dat_data, DatFlags, DatStatus, Database, FileStatus, FileType, GotStatus, HeaderFileType,
    MetaEntry, RepStatus, RvFile, Settings, TreeSelect,
};

use super::*;

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Tmp {
        let dir = std::env::temp_dir().join(format!("rv-dat-update-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Tmp(dir)
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.0.join(rel)
    }

    fn settings(&self) -> Settings {
        let mut s = Settings::default();
        s.dat_root = self.path("DatRoot").to_string_lossy().into_owned();
        s.dir_mappings[0].dir_path = self.path("RomRoot").to_string_lossy().into_owned();
        s
    }

    fn write_dat(&self, rel: &str, text: &str, secs: u64) -> PathBuf {
        let p = self.path(&format!("DatRoot/{rel}"));
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, text).unwrap();
        let f = fs::OpenOptions::new().write(true).open(&p).unwrap();
        f.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
            .unwrap();
        p
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn dat_text(games: &[(&str, &[(&str, u64, &str)])]) -> String {
    let mut s = String::from("clrmamepro (\n\tname \"T\"\n\tdescription \"T\"\n)\n");
    for (name, roms) in games {
        s.push_str(&format!(
            "game (\n\tname \"{name}\"\n\tdescription \"{name}\"\n"
        ));
        for (r, size, crc) in *roms {
            s.push_str(&format!("\trom ( name \"{r}\" size {size} crc {crc} )\n"));
        }
        s.push_str(")\n");
    }
    s
}

fn update(db: &mut Database, s: &Settings) -> UpdateReport {
    update_dats(
        db,
        s,
        &UpdateOptions::default(),
        &NoProgress,
        &CancelFlag::new(),
    )
    .unwrap()
}

fn node<'a>(db: &'a Database, path: &str) -> &'a RvFile {
    let mut cur = &db.root;
    for part in path.split('/') {
        cur = cur
            .children
            .iter()
            .find(|c| c.name == part)
            .unwrap_or_else(|| {
                panic!(
                    "{part} not in {:?} (path {path})",
                    cur.children.iter().map(|c| &c.name).collect::<Vec<_>>()
                )
            });
    }
    cur
}

fn node_mut<'a>(db: &'a mut Database, path: &str) -> &'a mut RvFile {
    let mut cur = &mut db.root;
    for part in path.split('/') {
        let i = cur.children.iter().position(|c| c.name == part).unwrap();
        cur = &mut cur.children[i];
    }
    cur
}

const PATH: &str = "RomVault/Console/NES/Roms";

fn fresh() -> Database {
    let mut db = Database::default_db();
    db.report_status_reset(&Settings::default());
    db
}

// ---- the first load --------------------------------------------------------------------------

#[test]
fn a_first_update_builds_the_dat_side_of_the_tree() {
    let t = Tmp::new("first");
    t.write_dat(
        "Console/NES/Roms/nes.dat",
        &dat_text(&[
            ("Zed", &[("z.nes", 16, "0000000f")]),
            (
                "Alpha",
                &[("a.nes", 16, "0000000a"), ("b.nes", 32, "0000000b")],
            ),
        ]),
        1_700_000_000,
    );
    let s = t.settings();
    let mut db = fresh();
    let r = update(&mut db, &s);
    assert_eq!(r.dats_found, 1);
    assert_eq!(r.dats_added, ["DatRoot/Console/NES/Roms/nes.dat"]);
    assert!(r.dats_failed.is_empty() && r.dats_removed.is_empty() && !r.cancelled);

    // The directories on the way down are tree rows, claimed by the DAT.
    for p in ["RomVault", "RomVault/Console", "RomVault/Console/NES", PATH] {
        let n = node(&db, p);
        assert_eq!(n.dat_status, DatStatus::IN_DAT_COLLECT, "{p}");
        assert!(n.tree.is_some(), "{p}");
        assert_eq!(n.rep_status, RepStatus::DIR_MISSING, "{p}");
    }
    let roms = node(&db, PATH);
    assert_eq!(roms.dir_dats.len(), 1);
    let dat = &roms.dir_dats[0];
    assert_eq!(dat.get(dat_data::DAT_NAME), Some("T"));
    assert_eq!(
        dat.get(dat_data::DAT_ROOT_FULL_NAME),
        Some("DatRoot/Console/NES/Roms/nes.dat")
    );
    assert_eq!(dat.get(dat_data::SUB_DIR_TYPE), Some("0"));
    assert_eq!(
        dat.time_stamp,
        621_355_968_000_000_000 + 1_700_000_000 * 10_000_000
    );

    // Games are zips, sorted, with their DAT; ROMs are missing archive members.
    let names: Vec<&str> = roms.children.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["Alpha.zip", "Zed.zip"]);
    let alpha = &roms.children[0];
    assert_eq!(alpha.file_type, FileType::ZIP);
    assert_eq!(alpha.dat_uid, dat.uid);
    assert!(alpha.game.is_some() && alpha.tree.is_none());
    assert_eq!(alpha.zip_dat_struct, Some(0x81));
    assert_eq!(alpha.zip_struct, Some(rv_core::ZipStructure::NONE));
    assert_eq!(alpha.got_status, GotStatus::NOT_GOT);
    let a = &alpha.children[0];
    assert_eq!(
        (a.name.as_str(), a.file_type),
        ("a.nes", FileType::FILE_ZIP)
    );
    assert_eq!(a.size, Some(16));
    assert_eq!(a.crc.as_deref(), Some(&[0, 0, 0, 0x0a][..]));
    assert!(a
        .file_status
        .has(FileStatus::SIZE_FROM_DAT.with(FileStatus::CRC_FROM_DAT)));
    assert_eq!(a.rep_status, RepStatus::MISSING);
    assert_eq!(a.zip_file_index, None);
    assert_eq!(a.file_mod_time_stamp, i64::MIN);

    // The counters see every ROM and archive below the directory.
    let counts = roms.dir_status(true);
    assert_eq!(counts.get(RepStatus::MISSING), 3);
    assert_eq!(counts.get(RepStatus::DIR_MISSING), 2);
}

#[test]
fn a_second_update_with_the_same_dats_changes_nothing() {
    let t = Tmp::new("second");
    t.write_dat(
        "Console/NES/Roms/nes.dat",
        &dat_text(&[("Alpha", &[("a.nes", 16, "0000000a")])]),
        1_700_000_000,
    );
    let s = t.settings();
    let mut db = fresh();
    update(&mut db, &s);
    let before = rv_cache::to_cache_file(&mut db)
        .unwrap()
        .to_bytes()
        .unwrap();
    let r = update(&mut db, &s);
    assert_eq!(r.dats_unchanged, ["DatRoot/Console/NES/Roms/nes.dat"]);
    assert!(r.dats_added.is_empty() && r.dats_removed.is_empty());
    let after = rv_cache::to_cache_file(&mut db)
        .unwrap()
        .to_bytes()
        .unwrap();
    assert_eq!(before, after);

    // And it survives the cache: written, read, updated again, written again.
    let mut again = rv_cache::into_database(
        rv_cache::CacheFile::from_bytes(&after).unwrap(),
        &s,
        rv_cache::DatabaseOptions {
            apply_load_fixups: false,
        },
    )
    .unwrap();
    let r = update(&mut again, &s);
    assert_eq!(r.dats_unchanged.len(), 1);
    assert!(r.dats_added.is_empty() && r.dats_removed.is_empty());
    assert_eq!(
        rv_cache::to_cache_file(&mut again)
            .unwrap()
            .to_bytes()
            .unwrap(),
        after
    );
}

#[test]
fn a_cache_written_on_windows_matches_the_dats_with_forward_slashes() {
    let t = Tmp::new("windows");
    t.write_dat(
        "Console/NES/Roms/nes.dat",
        &dat_text(&[("Alpha", &[("a.nes", 16, "0000000a")])]),
        1_700_000_000,
    );
    let s = t.settings();
    let mut db = fresh();
    update(&mut db, &s);
    // What ROMVault on Windows would have stored.
    let roms = node_mut(&mut db, PATH);
    for e in &mut roms.dir_dats[0].entries {
        if e.id == dat_data::DAT_ROOT_FULL_NAME {
            e.value = e.value.replace('/', "\\");
        }
    }
    let r = update(&mut db, &s);
    assert_eq!(r.dats_unchanged.len(), 1, "{r:?}");
    assert!(r.dats_added.is_empty() && r.dats_removed.is_empty());
}

// ---- changed, new and removed DATs -----------------------------------------------------------

#[test]
fn a_changed_dat_is_read_again_and_its_removed_rom_is_gone() {
    let t = Tmp::new("changed");
    t.write_dat(
        "Console/NES/Roms/nes.dat",
        &dat_text(&[(
            "Alpha",
            &[("a.nes", 16, "0000000a"), ("b.nes", 32, "0000000b")],
        )]),
        1_700_000_000,
    );
    let s = t.settings();
    let mut db = fresh();
    update(&mut db, &s);
    t.write_dat(
        "Console/NES/Roms/nes.dat",
        &dat_text(&[("Alpha", &[("a.nes", 16, "0000000a")])]),
        1_700_000_100,
    );
    let r = update(&mut db, &s);
    // A changed DAT is one removed and one added.
    assert_eq!(r.dats_removed, ["DatRoot/Console/NES/Roms/nes.dat"]);
    assert_eq!(r.dats_added, ["DatRoot/Console/NES/Roms/nes.dat"]);
    let alpha = node(&db, &format!("{PATH}/Alpha.zip"));
    assert_eq!(alpha.children.len(), 1);
    assert_eq!(alpha.children[0].name, "a.nes");
    assert_eq!(node(&db, PATH).dir_dats.len(), 1);
    // Saving still works: every node points at the one DAT in the list.
    rv_cache::to_cache_file(&mut db).unwrap();
}

#[test]
fn a_new_dat_in_a_new_directory_adds_a_directory_and_leaves_the_rest() {
    let t = Tmp::new("newdat");
    t.write_dat(
        "Console/NES/Roms/nes.dat",
        &dat_text(&[("Alpha", &[("a.nes", 16, "0000000a")])]),
        1_700_000_000,
    );
    let s = t.settings();
    let mut db = fresh();
    update(&mut db, &s);
    t.write_dat(
        "Console/SNES/Roms/snes.dat",
        &dat_text(&[("Super", &[("s.sfc", 64, "000000aa")])]),
        1_700_000_001,
    );
    let r = update(&mut db, &s);
    assert_eq!(r.dats_added, ["DatRoot/Console/SNES/Roms/snes.dat"]);
    assert_eq!(r.dats_unchanged, ["DatRoot/Console/NES/Roms/nes.dat"]);
    assert!(r.dats_removed.is_empty());
    let console: Vec<&str> = node(&db, "RomVault/Console")
        .children
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(console, ["NES", "SNES"]);
}

#[test]
fn a_removed_dat_takes_missing_items_and_keeps_what_was_found_on_disk() {
    let t = Tmp::new("removed");
    let dat = t.write_dat(
        "Console/NES/Roms/nes.dat",
        &dat_text(&[
            ("Have", &[("h.nes", 16, "0000000a")]),
            ("Lack", &[("l.nes", 16, "0000000b")]),
        ]),
        1_700_000_000,
    );
    let s = t.settings();
    let mut db = fresh();
    update(&mut db, &s);
    // As a scan would leave it: one game is on disk.
    {
        let have = node_mut(&mut db, &format!("{PATH}/Have.zip"));
        have.got_status = GotStatus::GOT;
        have.file_mod_time_stamp = 123;
        let rom = &mut have.children[0];
        rom.got_status = GotStatus::GOT;
        rom.zip_file_index = Some(0);
    }
    fs::remove_file(dat).unwrap();
    let r = update(&mut db, &s);
    assert_eq!(r.dats_removed, ["DatRoot/Console/NES/Roms/nes.dat"]);
    assert_eq!(r.dats_found, 0);

    // The game that is not on disk is gone, and so is the directory it left empty... but the
    // directory still holds the game that is.
    let roms = node(&db, PATH);
    assert!(roms.dir_dats.is_empty());
    let kept: Vec<&str> = roms.children.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(kept, ["Have.zip"], "{kept:?}");
    let have = &roms.children[0];
    assert_eq!(have.dat_status, DatStatus::NOT_IN_DAT);
    assert_eq!(have.rep_status, RepStatus::DIR_UNKNOWN);
    assert!(have.game.is_none() && have.dat_uid == 0);
    assert_eq!(have.children[0].rep_status, RepStatus::UNKNOWN);
    assert_eq!(have.children[0].dat_status, DatStatus::NOT_IN_DAT);
    // What the DAT said about the ROM is gone; what the file told is kept only if scanned.
    assert_eq!(have.children[0].size, None);
    // No DAT claims the directories any more, so they keep no tree row, but the one with the
    // file in it (and the directories above it) stay.
    for p in ["RomVault/Console", "RomVault/Console/NES", PATH] {
        let n = node(&db, p);
        assert!(n.tree.is_none(), "{p}");
        assert_eq!(n.dat_status, DatStatus::NOT_IN_DAT, "{p}");
    }
}

#[test]
fn removing_every_dat_removes_every_empty_directory() {
    let t = Tmp::new("empty");
    let dat = t.write_dat(
        "Console/NES/Roms/nes.dat",
        &dat_text(&[("Lack", &[("l.nes", 16, "0000000b")])]),
        1_700_000_000,
    );
    let s = t.settings();
    let mut db = fresh();
    update(&mut db, &s);
    fs::remove_file(dat).unwrap();
    update(&mut db, &s);
    // The RomVault root stays, and so does ToSort.
    assert!(node(&db, "RomVault").children.is_empty());
    assert_eq!(db.root.children.len(), 2);
}

// ---- directories, rules, conflicts -----------------------------------------------------------

#[test]
fn two_dats_with_the_same_name_in_one_directory_want_the_same_auto_directory() {
    let t = Tmp::new("multi");
    t.write_dat(
        "Console/Both/one.dat",
        &dat_text(&[("G1", &[("r1", 1, "00000001")])]),
        10,
    );
    t.write_dat(
        "Console/Both/two.dat",
        &dat_text(&[("G2", &[("r2", 2, "00000002")])]),
        11,
    );
    let s = t.settings();
    let mut db = fresh();
    let r = update(&mut db, &s);
    // Both are called `T` inside, so both are wrapped in a directory `T`. The first is merged;
    // the second finds the directory taken by another DAT and is refused.
    assert_eq!(r.dats_added, ["DatRoot/Console/Both/one.dat"]);
    assert_eq!(r.dats_failed.len(), 1, "{r:?}");
    let both = node(&db, "RomVault/Console/Both");
    assert_eq!(both.dir_dats.len(), 1);
    let d = &both.dir_dats[0];
    assert!(d.dat_flags.contains(DatFlags::MULTI_DATS_IN_DIRECTORY));
    assert!(d.dat_flags.contains(DatFlags::AUTO_ADDED_DIRECTORY));
    let names: Vec<&str> = both.children.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["T"]);
}

#[test]
fn dats_with_different_names_each_get_their_own_directory() {
    let t = Tmp::new("multi2");
    let named = |n: &str, g: &str| {
        format!("clrmamepro (\n\tname \"{n}\"\n)\ngame (\n\tname \"{g}\"\n\trom ( name r size 1 crc 00000001 )\n)\n")
    };
    t.write_dat("Console/Both/one.dat", &named("First", "G1"), 10);
    t.write_dat("Console/Both/two.dat", &named("Second", "G2"), 11);
    let s = t.settings();
    let mut db = fresh();
    let r = update(&mut db, &s);
    assert_eq!(r.dats_added.len(), 2);
    let both = node(&db, "RomVault/Console/Both");
    let names: Vec<&str> = both.children.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["First", "Second"]);
    assert!(both
        .children
        .iter()
        .all(|c| c.tree.is_some() && c.game.is_none()));
    assert_eq!(
        node(&db, "RomVault/Console/Both/First").children[0].name,
        "G1.zip"
    );
    // Each game is claimed by its own DAT.
    let uid = |p: &str| node(&db, p).dat_uid;
    assert_ne!(
        uid("RomVault/Console/Both/First/G1.zip"),
        uid("RomVault/Console/Both/Second/G2.zip")
    );
    assert_eq!(both.dir_dats.len(), 2);
}

#[test]
fn a_dat_that_claims_a_name_another_dat_has_is_refused() {
    let t = Tmp::new("conflict");
    let mut s = t.settings();
    s.dat_rules[0].multi_dat_dir_override = true; // no auto directories: both land in one place
    t.write_dat(
        "X/one.dat",
        &dat_text(&[("Same", &[("a", 1, "00000001")])]),
        10,
    );
    t.write_dat(
        "X/two.dat",
        &dat_text(&[("Same", &[("b", 2, "00000002")])]),
        11,
    );
    let mut db = fresh();
    let r = update(&mut db, &s);
    assert_eq!(r.dats_added, ["DatRoot/X/one.dat"]);
    assert_eq!(r.dats_failed.len(), 1);
    assert_eq!(r.dats_failed[0].0, "DatRoot/X/two.dat");
    assert!(
        r.dats_failed[0].1.contains("Dat Merge conflict"),
        "{:?}",
        r.dats_failed
    );
    let x = node(&db, "RomVault/X");
    assert_eq!(x.dir_dats.len(), 1);
    assert_eq!(node(&db, "RomVault/X/Same.zip").children[0].name, "a");
}

#[test]
fn a_file_found_on_disk_before_its_dat_arrives_is_claimed_by_it() {
    let t = Tmp::new("claim");
    let s = t.settings();
    let mut db = fresh();
    // A directory the scanner found, with a game zip and a rom in it, not in any DAT yet.
    {
        let mut roms = RvFile::new(FileType::DIR, "Roms");
        let mut zip = RvFile::new(FileType::ZIP, "Alpha.zip");
        zip.zip_struct = Some(rv_core::ZipStructure::ZIP_TRRNT);
        zip.zip_dat_struct = Some(0);
        zip.got_status = GotStatus::GOT;
        zip.file_mod_time_stamp = 99;
        let mut rom = RvFile::new(FileType::FILE_ZIP, "a.nes");
        rom.size = Some(16);
        rom.crc = Some(vec![0, 0, 0, 0x0a]);
        rom.got_status = GotStatus::GOT;
        rom.zip_file_index = Some(0);
        rom.file_status = FileStatus::SIZE_VERIFIED.with(FileStatus::CRC_VERIFIED);
        zip.children.push(rom);
        roms.children.push(zip);
        let mut nes = RvFile::new(FileType::DIR, "NES");
        nes.children.push(roms);
        nes.got_status = GotStatus::GOT;
        let rv = &mut db.root.children[0];
        let mut console = RvFile::new(FileType::DIR, "Console");
        console.children.push(nes);
        rv.children.push(console);
    }
    t.write_dat(
        "Console/NES/Roms/nes.dat",
        &dat_text(&[(
            "Alpha",
            &[("a.nes", 16, "0000000a"), ("b.nes", 8, "0000000b")],
        )]),
        5,
    );
    update(&mut db, &s);
    let alpha = node(&db, &format!("{PATH}/Alpha.zip"));
    assert_eq!(alpha.dat_status, DatStatus::IN_DAT_COLLECT);
    assert_eq!(
        alpha.got_status,
        GotStatus::GOT,
        "the zip on disk is the DAT's game now"
    );
    assert_eq!(alpha.rep_status, RepStatus::DIR_CORRECT);
    assert_eq!(alpha.file_mod_time_stamp, 99);
    let a = &alpha.children[0];
    assert_eq!(
        (a.dat_status, a.rep_status),
        (DatStatus::IN_DAT_COLLECT, RepStatus::CORRECT)
    );
    assert!(a
        .file_status
        .has(FileStatus::CRC_FROM_DAT.with(FileStatus::CRC_VERIFIED)));
    let b = &alpha.children[1];
    assert_eq!(
        (b.name.as_str(), b.rep_status),
        ("b.nes", RepStatus::MISSING)
    );
}

#[test]
fn tree_rows_keep_the_users_check_boxes_when_a_dat_is_replaced() {
    let t = Tmp::new("rows");
    let dat = t.write_dat(
        "Console/NES/Roms/nes.dat",
        &dat_text(&[("Alpha", &[("a.nes", 16, "0000000a")])]),
        1,
    );
    let s = t.settings();
    let mut db = fresh();
    update(&mut db, &s);
    node_mut(&mut db, "RomVault/Console/NES")
        .tree
        .as_mut()
        .unwrap()
        .checked = TreeSelect::LOCKED;
    node_mut(&mut db, PATH).tree.as_mut().unwrap().expanded = false;
    // The DAT changes: its directory loses its rows and gets new ones in the same update, and
    // the check boxes come back (`DatTreeStatusStore`).
    t.write_dat(
        "Console/NES/Roms/nes.dat",
        &dat_text(&[
            ("Alpha", &[("a.nes", 16, "0000000a")]),
            ("Beta", &[("b.nes", 1, "00000001")]),
        ]),
        2,
    );
    update(&mut db, &s);
    assert_eq!(
        node(&db, "RomVault/Console/NES").tree.unwrap().checked,
        TreeSelect::LOCKED
    );
    assert!(!node(&db, PATH).tree.unwrap().expanded);
    // A DAT that goes away takes the rows with it, and a different one starts with fresh rows.
    fs::remove_file(&dat).unwrap();
    update(&mut db, &s);
    assert!(db.root.children[0].children.is_empty());
}

// ---- failures --------------------------------------------------------------------------------

#[test]
fn a_dat_that_is_not_a_dat_is_reported_and_the_others_still_load() {
    let t = Tmp::new("bad");
    t.write_dat("A/bad.dat", "this is not a DAT\n", 1);
    t.write_dat(
        "B/good.dat",
        &dat_text(&[("G", &[("r", 1, "00000001")])]),
        1,
    );
    let s = t.settings();
    let mut db = fresh();
    let r = update(&mut db, &s);
    assert_eq!(r.dats_added, ["DatRoot/B/good.dat"]);
    assert_eq!(r.dats_failed.len(), 1);
    assert_eq!(r.dats_failed[0].0, "DatRoot/A/bad.dat");
    assert!(
        r.messages.iter().any(|m| m.contains("Invalid DAT File")),
        "{:?}",
        r.messages
    );
    // Its directory was made but holds nothing; the next update removes it, tries the DAT
    // again, and makes the directory again.
    let a = node(&db, "RomVault/A");
    assert!(a.children.is_empty() && a.dir_dats.is_empty());
    assert_eq!(update(&mut db, &s).dats_failed.len(), 1);
}

#[test]
fn a_dat_with_nothing_in_it_is_not_recorded() {
    let t = Tmp::new("hollow");
    t.write_dat("A/hollow.dat", "clrmamepro (\n\tname \"H\"\n)\n", 1);
    let s = t.settings();
    let mut db = fresh();
    let r = update(&mut db, &s);
    assert!(r.dats_added.is_empty() && r.dats_failed.is_empty());
    assert!(node(&db, "RomVault/A").dir_dats.is_empty());
}

#[test]
fn a_missing_dat_root_is_an_error() {
    let t = Tmp::new("noroot");
    let s = t.settings();
    let mut db = fresh();
    let e = update_dats(
        &mut db,
        &s,
        &UpdateOptions::default(),
        &NoProgress,
        &CancelFlag::new(),
    )
    .unwrap_err();
    assert!(
        matches!(e, UpdateError::Import(ImportError::PathNotFound(_))),
        "{e}"
    );
}

#[test]
fn cancelling_before_the_first_dat_adds_none_and_leaves_a_savable_database() {
    let t = Tmp::new("cancel");
    t.write_dat("A/a.dat", &dat_text(&[("G", &[("r", 1, "00000001")])]), 1);
    t.write_dat("B/b.dat", &dat_text(&[("G", &[("r", 1, "00000001")])]), 1);
    let s = t.settings();
    let mut db = fresh();
    let cancel = CancelFlag::new();
    cancel.cancel();
    let r = update_dats(&mut db, &s, &UpdateOptions::default(), &NoProgress, &cancel).unwrap();
    assert!(r.cancelled);
    assert!(r.dats_added.is_empty());
    rv_cache::to_cache_file(&mut db).unwrap();
}

#[test]
fn cancelling_between_dats_keeps_the_ones_already_merged() {
    let t = Tmp::new("cancel2");
    t.write_dat("A/a.dat", &dat_text(&[("G", &[("r", 1, "00000001")])]), 1);
    t.write_dat("B/b.dat", &dat_text(&[("H", &[("r", 1, "00000001")])]), 1);
    let s = t.settings();
    let mut db = fresh();
    let cancel = CancelFlag::new();
    let c2 = cancel.clone();
    let progress = move |e: ProgressEvent| {
        if matches!(&e, ProgressEvent::Text(t) if t.starts_with("Scanning New Dat")) {
            c2.cancel();
        }
    };
    let r = update_dats(&mut db, &s, &UpdateOptions::default(), &progress, &cancel).unwrap();
    assert!(r.cancelled);
    // The first DAT was already being read when the flag was set and is merged whole; the second
    // is not started.
    assert_eq!(r.dats_added, ["DatRoot/A/a.dat"]);
    let b = node(&db, "RomVault/B");
    assert!(
        b.children.is_empty() && b.dir_dats.is_empty(),
        "B was not started"
    );
    rv_cache::to_cache_file(&mut db).unwrap();
}

#[test]
fn invalidating_makes_unchanged_dats_load_again() {
    let t = Tmp::new("invalidate");
    t.write_dat("A/a.dat", &dat_text(&[("G", &[("r", 1, "00000001")])]), 1);
    t.write_dat("B/b.dat", &dat_text(&[("H", &[("r", 1, "00000001")])]), 1);
    let s = t.settings();
    let mut db = fresh();
    update(&mut db, &s);
    let opts = UpdateOptions {
        invalidate: Some("DatRoot/A".to_owned()),
    };
    let r = update_dats(&mut db, &s, &opts, &NoProgress, &CancelFlag::new()).unwrap();
    assert_eq!(r.dats_added, ["DatRoot/A/a.dat"]);
    assert_eq!(r.dats_removed, ["DatRoot/A/a.dat"]);
    assert_eq!(r.dats_unchanged, ["DatRoot/B/b.dat"]);
}

#[test]
fn rules_per_directory_change_how_a_dat_is_loaded() {
    let t = Tmp::new("rules");
    t.write_dat(
        "Console/NES/Roms/nes.dat",
        &dat_text(&[("G", &[("r", 1, "00000001")])]),
        1,
    );
    let mut s = t.settings();
    let mut sub = s.dat_rules[0].clone();
    sub.dir_key = "RomVault/Console/NES".to_owned();
    sub.compression = FileType::SEVEN_ZIP;
    sub.compression_sub = rv_core::ZipStructure::SEVEN_ZIP_SLZMA;
    s.dat_rules.push(sub);
    let mut db = fresh();
    update(&mut db, &s);
    let g = node(&db, &format!("{PATH}/G.7z"));
    assert_eq!(g.file_type, FileType::SEVEN_ZIP);
    assert_eq!(g.zip_dat_struct, Some(0x80 | 8));
    assert_eq!(g.children[0].file_type, FileType::FILE_SEVEN_ZIP);
}

// ---- the predicates -----------------------------------------------------------------------------

fn rv_dat(name: &str, ts: i64, flags: DatFlags, sub: Option<&str>) -> RvDat {
    let mut entries = vec![MetaEntry {
        id: dat_data::DAT_ROOT_FULL_NAME,
        value: name.to_owned(),
    }];
    if let Some(sub) = sub {
        entries.push(MetaEntry {
            id: dat_data::SUB_DIR_TYPE,
            value: sub.to_owned(),
        });
    }
    RvDat::new(ts, flags, entries)
}

fn disk_dat(name: &str, ts: i64, flags: DatFlags, sub: RemoveSubType) -> ImportDat {
    ImportDat {
        full_name: name.to_owned(),
        time_stamp: ts,
        flags,
        sub_dir_type: sub,
        header_type: rv_core::HeaderType::Optional,
        header: None,
    }
}

#[test]
fn dat_compare_is_name_then_time_then_four_flags_then_sub_dir_type() {
    use std::cmp::Ordering::*;
    let f = DatFlags::default();
    let base = rv_dat("DatRoot/a.dat", 5, f, Some("0"));
    let same = disk_dat("datroot\\A.DAT", 5, f, RemoveSubType::KeepAllSubDirs);
    assert_eq!(
        dat_compare(&base, &same),
        Equal,
        "case and separator do not count"
    );
    assert_eq!(
        dat_compare(
            &base,
            &disk_dat("DatRoot/b.dat", 5, f, RemoveSubType::KeepAllSubDirs)
        ),
        Less
    );
    assert_eq!(
        dat_compare(
            &base,
            &disk_dat("DatRoot/a.dat", 6, f, RemoveSubType::KeepAllSubDirs)
        ),
        Less
    );
    assert_eq!(
        dat_compare(
            &base,
            &disk_dat("DatRoot/a.dat", 4, f, RemoveSubType::KeepAllSubDirs)
        ),
        Greater
    );
    for flag in [
        DatFlags::MULTI_DATS_IN_DIRECTORY,
        DatFlags::MULTI_DAT_OVERRIDE,
        DatFlags::USE_DESCRIPTION_AS_DIR_NAME,
        DatFlags::SINGLE_ARCHIVE,
    ] {
        assert_eq!(
            dat_compare(
                &base,
                &disk_dat("DatRoot/a.dat", 5, flag, RemoveSubType::KeepAllSubDirs)
            ),
            Less,
            "{flag:?}"
        );
    }
    // The flags that do not count: UseIdForName and AutoAddedDirectory.
    for flag in [DatFlags::USE_ID_FOR_NAME, DatFlags::AUTO_ADDED_DIRECTORY] {
        assert_eq!(
            dat_compare(
                &base,
                &disk_dat("DatRoot/a.dat", 5, flag, RemoveSubType::KeepAllSubDirs)
            ),
            Equal,
            "{flag:?}"
        );
    }
    assert_eq!(
        dat_compare(
            &base,
            &disk_dat("DatRoot/a.dat", 5, f, RemoveSubType::RemoveAllSubDirs)
        ),
        Less
    );
    // No SubDirType stored means KeepAllSubDirs.
    let bare = rv_dat("DatRoot/a.dat", 5, f, None);
    assert_eq!(dat_compare(&bare, &same), Equal);
    assert_eq!(dat_list_compare(&base, &bare), Equal);
}

fn dat_file(name: &str, size: Option<u64>, crc: Option<u8>) -> DatNode {
    let mut f = crate::DatFile::new(name);
    f.base.file_type = FileType::FILE_ZIP;
    f.size = size;
    f.crc = crc.map(|c| vec![0, 0, 0, c]);
    DatNode::File(f)
}

fn db_file(name: &str, size: Option<u64>, crc: Option<u8>) -> RvFile {
    let mut f = RvFile::new(FileType::FILE_ZIP, name);
    f.size = size;
    f.crc = crc.map(|c| vec![0, 0, 0, c]);
    f.got_status = GotStatus::GOT;
    f.dat_status = DatStatus::NOT_IN_DAT;
    f
}

#[test]
fn dat_merge_compare_names_types_and_hashes() {
    let db = db_file("Rom.bin", Some(10), Some(1));
    let m = |db: &RvFile, t: &DatNode, case| dat_merge_compare(db, t, case);
    assert_eq!(
        m(&db, &dat_file("Rom.bin", Some(10), Some(1)), 0),
        Some(false)
    );
    // The name: exact in pass 0. In pass 1 a loose file is case-insensitive, but the names
    // inside an archive stay case-sensitive (TorrentZip order has a case tie-break).
    assert_eq!(m(&db, &dat_file("ROM.BIN", Some(10), Some(1)), 0), None);
    assert_eq!(m(&db, &dat_file("ROM.BIN", Some(10), Some(1)), 1), None);
    assert_eq!(m(&db, &dat_file("Other.bin", Some(10), Some(1)), 1), None);
    let mut loose = db_file("Rom.bin", Some(10), Some(1));
    loose.file_type = FileType::FILE;
    let mut loose_dat = dat_file("ROM.BIN", Some(10), Some(1));
    if let DatNode::File(f) = &mut loose_dat {
        f.base.file_type = FileType::FILE;
    }
    assert_eq!(m(&loose, &loose_dat, 0), None);
    assert_eq!(m(&loose, &loose_dat, 1), Some(false));
    // Size and CRC must agree where both sides have them.
    assert_eq!(m(&db, &dat_file("Rom.bin", Some(11), Some(1)), 0), None);
    assert_eq!(m(&db, &dat_file("Rom.bin", Some(10), Some(2)), 0), None);
    // A DAT row with no CRC cannot match: at least one hash has to be compared.
    assert_eq!(m(&db, &dat_file("Rom.bin", Some(10), None), 0), None);
    // The type must agree.
    let mut t = dat_file("Rom.bin", Some(10), Some(1));
    if let DatNode::File(f) = &mut t {
        f.base.file_type = FileType::FILE;
    }
    assert_eq!(m(&db, &t, 0), None);
    // A locked file matches by name alone.
    let mut locked = db_file("Rom.bin", Some(99), Some(9));
    locked.got_status = GotStatus::FILE_LOCKED;
    assert_eq!(
        m(&locked, &dat_file("Rom.bin", Some(10), Some(1)), 0),
        Some(false)
    );
}

#[test]
fn dat_merge_compare_zero_length_dates_and_required_headers() {
    // A zero-size DAT file with no hashes matches a zero-size file.
    let empty = db_file("e", Some(0), Some(0));
    assert_eq!(
        dat_merge_compare(&empty, &dat_file("e", Some(0), None), 0),
        Some(false)
    );
    let nonempty = db_file("e", Some(5), Some(0));
    assert_eq!(
        dat_merge_compare(&nonempty, &dat_file("e", Some(0), None), 0),
        None
    );

    // A DAT date must equal the item's time stamp.
    let mut dated = db_file("d", Some(1), Some(1));
    dated.file_mod_time_stamp = 100;
    let mut t = dat_file("d", Some(1), Some(1));
    if let DatNode::File(f) = &mut t {
        f.base.date_modified = Some(100);
    }
    assert_eq!(dat_merge_compare(&dated, &t, 0), Some(false));
    if let DatNode::File(f) = &mut t {
        f.base.date_modified = Some(101);
    }
    assert_eq!(dat_merge_compare(&dated, &t, 0), None);

    // A required header type must be the item's.
    let mut req = dat_file("h", Some(1), Some(1));
    if let DatNode::File(f) = &mut req {
        f.header_file_type = HeaderFileType(HeaderFileType::NES.0 | HeaderFileType::REQUIRED.0);
    }
    let plain = db_file("h", Some(1), Some(1));
    assert_eq!(dat_merge_compare(&plain, &req, 0), None);
    let mut nes = db_file("h", Some(1), Some(1));
    nes.set_header_type_raw(HeaderFileType::NES);
    assert_eq!(dat_merge_compare(&nes, &req, 0), Some(false));
}

#[test]
fn dat_merge_compare_matches_a_file_with_a_copier_header_through_its_alt_hashes() {
    // The scanner saw a 16-byte-headered NES file: the DAT's headerless hashes are in the alt
    // fields.
    let mut db = db_file("g.nes", Some(272), Some(0xee));
    db.set_header_type_raw(HeaderFileType::NES);
    db.alt_size = Some(256);
    db.alt_crc = Some(vec![0, 0, 0, 0x0a]);
    let mut t = dat_file("g.nes", Some(256), Some(0x0a));
    if let DatNode::File(f) = &mut t {
        f.header_file_type = HeaderFileType::NES;
    }
    assert_eq!(dat_merge_compare(&db, &t, 0), Some(true));
    // Another size, or another header type, is no match.
    if let DatNode::File(f) = &mut t {
        f.size = Some(255);
    }
    assert_eq!(dat_merge_compare(&db, &t, 0), None);
    if let DatNode::File(f) = &mut t {
        f.size = Some(256);
        f.header_file_type = HeaderFileType::SNES;
    }
    assert_eq!(dat_merge_compare(&db, &t, 0), None);
}

#[test]
fn directories_and_archives_match_on_name_and_type_alone() {
    let mut z = RvFile::new(FileType::ZIP, "Game.zip");
    z.dat_status = DatStatus::NOT_IN_DAT;
    let mut d = crate::DatDir::new("game.ZIP", FileType::ZIP);
    d.game = Some(Box::default());
    assert_eq!(
        dat_merge_compare(&z, &DatNode::Dir(d.clone()), 1),
        Some(false)
    );
    assert_eq!(dat_merge_compare(&z, &DatNode::Dir(d.clone()), 0), None);
    d.base.file_type = FileType::DIR;
    assert_eq!(dat_merge_compare(&z, &DatNode::Dir(d), 1), None);
}
