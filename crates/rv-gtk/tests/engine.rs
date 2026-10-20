//! The engine calls the window makes, on a small synthetic collection in a temp directory: load,
//! update the DATs, scan, save, load again, and the path guard in front of all of it.

use std::fs;
use std::path::PathBuf;

use rv_core::progress::{CancelFlag, ProgressEvent};
use rv_core::{FileType, RepStatus, ScanLevel, Settings};
use rv_gtk::engine::{self, EngineError, Task};
use rv_gtk::model;
use rv_scan::db::ScanTarget;

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Tmp {
        let dir = std::env::temp_dir().join(format!("rv-gtk-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Tmp(dir)
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.0.join(rel)
    }

    fn write(&self, rel: &str, bytes: &[u8]) {
        let p = self.path(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, bytes).unwrap();
    }

    fn settings(&self) -> Settings {
        let mut s = Settings::default();
        s.dat_root = self.path("DatRoot").to_string_lossy().into_owned();
        s.cache_file = self.path("test.Cache").to_string_lossy().into_owned();
        s.dir_mappings[0].dir_path = self.path("RomRoot").to_string_lossy().into_owned();
        s.dat_rules[0].compression = FileType::FILE_ONLY;
        s
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// `a.rom` is `abcd` (CRC32 `ed82cd11`) and `b.rom` is `efgh` (`08337bb5`), values from
/// Python's `zlib.crc32`, not from the code under test.
const DAT: &str = "clrmamepro (\n\tname \"T\"\n)\n\
game (\n\tname \"Alpha\"\n\
\trom ( name \"a.rom\" size 4 crc ed82cd11 )\n\
\trom ( name \"b.rom\" size 4 crc 08337bb5 )\n)\n";

#[test]
fn update_scan_save_and_load_again_through_the_window_calls() {
    let t = Tmp::new("flow");
    let s = t.settings();
    t.write("DatRoot/Console/NES/Roms/nes.dat", DAT.as_bytes());
    t.write("RomRoot/Console/NES/Roms/Alpha/a.rom", b"abcd");

    // No cache yet: the default database, as ROMVault's first run.
    let mut opened = engine::open_cache(&s, false).expect("a missing cache is a new one");
    assert!(opened.db.root.children.iter().any(|c| c.name == "RomVault"));

    let events = std::cell::RefCell::new(Vec::new());
    let sink = |e: ProgressEvent| events.borrow_mut().push(e);
    let out = engine::run_task(
        &mut opened.db,
        &s,
        &Task::UpdateDats { all: false },
        false,
        &sink,
        &CancelFlag::new(),
    )
    .expect("update");
    assert!(
        out.summary().starts_with("1 DATs found, 1 added"),
        "{}",
        out.summary()
    );
    assert!(
        t.path("test.Cache").exists(),
        "the cache is saved after an operation"
    );
    assert!(!events.borrow().is_empty(), "progress reaches the sink");

    let out = engine::run_task(
        &mut opened.db,
        &s,
        &Task::Scan {
            level: ScanLevel::Level2,
            target: ScanTarget::Dir("RomVault/Console/NES/Roms".to_owned()),
        },
        false,
        &sink,
        &CancelFlag::new(),
    )
    .expect("scan");
    assert!(!out.cancelled());

    // What the window shows: the tree row and the game's ROMs.
    let rows = model::build_tree(&opened.db.root, true);
    let nes = &rows[0].children[0].children[0].children[0];
    assert_eq!(nes.path, ["RomVault", "Console", "NES", "Roms"]);
    assert_eq!(nes.status.count_correct(), 1, "{}", nes.counts);
    assert_eq!(nes.status.count_missing(false), 1);
    let dir = model::dir_at(&opened.db.root, &nes.path).unwrap();
    let games = model::game_list(dir, &model::GameFilter::default(), true);
    assert_eq!(games.rows.len(), 1);
    let roms = model::rom_list(&dir.children[games.rows[0].index], false, true);
    let status: Vec<(String, RepStatus)> = roms
        .rows
        .iter()
        .map(|r| (r.name.clone(), r.rep_status))
        .collect();
    assert_eq!(
        status,
        [
            ("a.rom".to_owned(), RepStatus::CORRECT),
            ("b.rom".to_owned(), RepStatus::MISSING)
        ]
    );

    // A new window reads the saved cache and shows the same thing.
    let again = engine::open_cache(&s, false).expect("reload");
    let rows2 = model::build_tree(&again.db.root, true);
    assert_eq!(
        rows2[0].children[0].children[0].children[0].counts,
        nes.counts
    );
}

#[test]
fn a_corrupt_cache_with_a_backup_loads_the_backup_and_says_so() {
    let t = Tmp::new("backup");
    let s = t.settings();
    t.write("DatRoot/Console/NES/Roms/nes.dat", DAT.as_bytes());
    let mut opened = engine::open_cache(&s, false).unwrap();
    assert!(
        opened.fallback_notice().is_none(),
        "no cache yet is not a fallback"
    );
    let sink = |_: ProgressEvent| {};
    // Two saves, so that the first state is kept as `test.CacheBackup`.
    for _ in 0..2 {
        engine::run_task(
            &mut opened.db,
            &s,
            &Task::UpdateDats { all: false },
            false,
            &sink,
            &CancelFlag::new(),
        )
        .expect("update");
    }
    assert!(t.path("test.CacheBackup").exists());
    assert!(engine::open_cache(&s, false)
        .unwrap()
        .fallback_notice()
        .is_none());

    t.write("test.Cache", b"not a cache");
    let again = engine::open_cache(&s, false).expect("the backup loads");
    let notice = again.fallback_notice().expect("the fallback is reported");
    assert!(notice.contains("backup"), "{notice}");
    assert!(notice.contains("overwrite"), "{notice}");
}

#[test]
fn a_cancelled_task_reports_it_and_leaves_a_loadable_cache() {
    let t = Tmp::new("cancel");
    let s = t.settings();
    t.write("DatRoot/Console/NES/Roms/nes.dat", DAT.as_bytes());
    let mut db = engine::open_cache(&s, false).unwrap().db;
    let cancel = CancelFlag::new();
    cancel.cancel();
    let out = engine::run_task(
        &mut db,
        &s,
        &Task::UpdateDats { all: true },
        false,
        &|_: ProgressEvent| {},
        &cancel,
    )
    .unwrap();
    assert!(out.cancelled());
    assert!(out.summary().ends_with("(cancelled)"));
    engine::open_cache(&s, false).expect("the cache saved after a cancel loads");
}

#[test]
fn a_protected_path_stops_every_call_before_a_file_is_touched() {
    let t = Tmp::new("guard");
    let mut s = t.settings();
    s.dat_root = "/mnt/HHD16TB/DatRoot".to_owned();
    let mut db = rv_core::Database::default_db();
    let nothing = |_: ProgressEvent| {};

    for result in [
        engine::open_cache(&s, false).map(|_| ()),
        engine::save_cache(&s, &mut db, false),
        engine::run_task(
            &mut db,
            &s,
            &Task::UpdateDats { all: false },
            false,
            &nothing,
            &CancelFlag::new(),
        )
        .map(|_| ()),
    ] {
        match result {
            Err(EngineError::Protected(found)) => {
                assert_eq!(found.len(), 1);
                assert_eq!(found[0].setting, "DatRoot");
            }
            other => panic!("expected the guard to refuse, got {other:?}"),
        }
    }
    assert!(!t.path("test.Cache").exists(), "nothing was written");

    // A protected cache file is refused too, with the cache itself named.
    let mut c = t.settings();
    c.cache_file = "/mnt/HHD16TB/RomVault3_3.Cache".to_owned();
    match engine::open_cache(&c, false) {
        Err(EngineError::Protected(found)) => assert_eq!(found[0].setting, "CacheFile"),
        other => panic!("expected the guard to refuse, got {:?}", other.map(|_| ())),
    }
}

#[test]
fn settings_are_written_back_with_what_the_file_already_had() {
    let t = Tmp::new("cfg");
    let file = t.path("RomVault3cfg.xml");
    fs::write(
        &file,
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<Settings>\n  <DatRoot>DatRoot</DatRoot>\n  <EInfo>keep me</EInfo>\n</Settings>\n",
    )
    .unwrap();
    let mut s = engine::read_settings(&file).unwrap();
    s.dat_root = "Dats".to_owned();
    engine::write_settings(&file, &s).unwrap();
    let text = fs::read_to_string(&file).unwrap();
    assert!(text.contains("<EInfo>keep me</EInfo>"), "{text}");
    assert_eq!(engine::read_settings(&file).unwrap().dat_root, "Dats");
    assert!(
        !t.path("RomVault3cfg.xml.tmp").exists(),
        "no temp file is left"
    );
    // A file that is not there gives the first-run defaults.
    assert_eq!(
        engine::read_settings(&t.path("none.xml")).unwrap().dat_root,
        "DatRoot"
    );
    // One that is not XML is an error, not the defaults.
    fs::write(&file, "not xml").unwrap();
    assert!(engine::read_settings(&file).is_err());
}
