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
    assert!(
        opened.created,
        "no file: a new empty database, not a loaded cache"
    );
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
    assert!(!again.created, "the saved file is a cache now");
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

// ---- the online tasks, against a mock of the service on 127.0.0.1 ----------------------------

mod online {
    use super::*;
    use rv_core::MiaStatus;
    use rv_online::config::OnlineConfig;
    use rv_online::datvault::{encode_group_info, DatInfo};
    use rv_online::mia::testing::blob;
    use rv_online::mia::MiaRecord;
    use rv_online::mock;
    use rv_online::salt::{sha256_hex, synthetic_exe, Salt};

    const KEY: &str = "WINDOW-KEY-do-not-leak";
    const SALT: [u8; 13] = [2, 7, 1, 8, 2, 8, 1, 8, 2, 8, 4, 5, 9];

    fn config(t: &Tmp, srv: &mock::MockServer) -> OnlineConfig {
        OnlineConfig {
            dat_vault_key: KEY.into(),
            endpoint: Some(srv.url()),
            allow_plain_http: true,
            mia_cache: Some(t.path("MIA.Cache")),
            ..OnlineConfig::default()
        }
    }

    fn zip(name: &str, data: &[u8]) -> Vec<u8> {
        use rv_archive::zip::{TorrentZipWriter, ZipStructure};
        let mut w = TorrentZipWriter::new(std::io::Cursor::new(Vec::new()), ZipStructure::ZipTrrnt)
            .unwrap();
        w.add_bytes(name, data, None).unwrap();
        w.finish().unwrap().into_inner()
    }

    #[test]
    fn the_datvault_task_downloads_newer_dats_then_updates_the_database() {
        let t = Tmp::new("dv");
        let s = t.settings();
        let st = mock::state(KEY);
        let old = DatInfo {
            group: "ni".into(),
            system: "nes".into(),
            version: "1".into(),
            date: "d1".into(),
            dat_name: "nes.dat".into(),
            dat_url: "u1".into(),
            ..DatInfo::default()
        };
        let new = DatInfo {
            version: "2".into(),
            date: "d2".into(),
            dat_url: "u2".into(),
            ..old.clone()
        };
        // The seed and the DAT of version 1 are here.
        t.write("DatRoot/Console/ni/nes.dat", b"stale, not a DAT");
        t.write(
            "DatRoot/Console/ni/nes.json",
            br#"{"group":"ni","system":"nes","date":"d1","version":"1","datURL":"u1","datName":"nes.dat"}"#,
        );
        {
            let mut g = st.lock().unwrap();
            g.group_info.insert(
                "ni".into(),
                Some(encode_group_info(std::slice::from_ref(&new))),
            );
            g.files
                .insert("ni|nes|u2".into(), zip("nes.dat", DAT.as_bytes()));
        }
        let srv = mock::start_service(&st);
        let mut opened = engine::open_cache(&s, false).unwrap();
        let out = engine::run_task(
            &mut opened.db,
            &s,
            &Task::DatVaultUpdate {
                online: config(&t, &srv),
            },
            false,
            &|_: ProgressEvent| {},
            &CancelFlag::new(),
        )
        .expect("datvault update");
        assert!(
            out.summary()
                .starts_with("DatVault: 1 DATs downloaded, 0 removed, 0 failed; 1 DATs added"),
            "{}",
            out.summary()
        );
        assert_eq!(
            fs::read(t.path("DatRoot/Console/ni/nes.dat")).unwrap(),
            DAT.as_bytes()
        );
        assert!(t.path("test.Cache").exists());
        assert!(!out.summary().contains(KEY));
    }

    #[test]
    fn check_key_says_valid_invalid_or_why_not() {
        let t = Tmp::new("ck");
        let st = mock::state(KEY);
        let srv = mock::start_service(&st);
        let mut c = config(&t, &srv);
        assert_eq!(engine::check_key(&c), "The DatVault key is valid.");
        c.dat_vault_key = "other".into();
        assert_eq!(engine::check_key(&c), "The DatVault key is not valid.");
        c.dat_vault_key = "  ".into();
        assert_eq!(engine::check_key(&c), "Enter a DatVault key first.");
        // Plain http without the switch: refused, with the reason, and nothing is sent.
        let before = srv.requests().len();
        c.dat_vault_key = KEY.into();
        c.allow_plain_http = false;
        let text = engine::check_key(&c);
        assert!(
            text.contains("allow_plain_http") && !text.contains(KEY),
            "{text}"
        );
        assert_eq!(srv.requests().len(), before);
        // A service fault is not "invalid".
        c.allow_plain_http = true;
        st.lock().unwrap().faulting.insert("CheckDVUser".into());
        let text = engine::check_key(&c);
        assert!(
            text.starts_with("The key could not be checked") && !text.contains(KEY),
            "{text}"
        );
    }

    #[test]
    fn a_service_error_stops_the_task_with_words_that_do_not_hold_the_key() {
        let t = Tmp::new("dverr");
        let s = t.settings();
        let st = mock::state(KEY);
        t.write(
            "DatRoot/x/s.json",
            br#"{"group":"ni","system":"s","version":"1"}"#,
        );
        st.lock()
            .unwrap()
            .faulting
            .insert("GetDVGroupInfoV2".into());
        let srv = mock::start_service(&st);
        let mut opened = engine::open_cache(&s, false).unwrap();
        let e = engine::run_task(
            &mut opened.db,
            &s,
            &Task::DatVaultUpdate {
                online: config(&t, &srv),
            },
            false,
            &|_: ProgressEvent| {},
            &CancelFlag::new(),
        )
        .unwrap_err();
        assert!(matches!(e, EngineError::Online(_)));
        assert!(!e.to_string().contains(KEY), "{e}");
        assert!(!format!("{e:?}").contains(KEY));
    }

    #[test]
    fn the_online_tasks_do_not_run_in_a_protected_directory() {
        let t = Tmp::new("dvguard");
        let mut s = t.settings();
        s.dat_root = "/mnt/HHD16TB/DatRoot".to_owned();
        let st = mock::state(KEY);
        let srv = mock::start_service(&st);
        let mut db = rv_core::Database::default_db();
        for task in [
            Task::DatVaultUpdate {
                online: config(&t, &srv),
            },
            Task::UpdateMia {
                force: false,
                online: config(&t, &srv),
            },
        ] {
            let e = engine::run_task(
                &mut db,
                &s,
                &task,
                false,
                &|_: ProgressEvent| {},
                &CancelFlag::new(),
            )
            .unwrap_err();
            assert!(matches!(e, EngineError::Protected(_)), "{e}");
        }
        assert!(srv.requests().is_empty());
    }

    #[test]
    fn the_mia_task_marks_files_in_the_database() {
        let t = Tmp::new("mia");
        let s = t.settings();
        let exe = synthetic_exe(0x1234, &SALT);
        fs::write(t.path("stand-in.exe"), &exe).unwrap();
        std::env::set_var(
            "RV_TEST_SALT_SOURCE",
            format!("{}:1234:13", sha256_hex(&exe)),
        );
        let st = mock::state(KEY);
        let rec = MiaRecord {
            size: Some(4),
            crc: Some([0xed, 0x82, 0xcd, 0x11]),
            date_ticks: Some(1),
            ..MiaRecord::default()
        };
        {
            let mut g = st.lock().unwrap();
            g.mia_version = Some(900);
            g.mia_blob = Some(blob(900, &[rec], KEY, &Salt::from_bytes(&SALT)));
        }
        let srv = mock::start_service(&st);
        let mut cfg = config(&t, &srv);
        cfg.rom_vault_exe = Some(t.path("stand-in.exe"));

        t.write("DatRoot/Console/NES/Roms/nes.dat", DAT.as_bytes());
        t.write("RomRoot/Console/NES/Roms/Alpha/a.rom", b"abcd");
        let mut opened = engine::open_cache(&s, false).unwrap();
        let none = &|_: ProgressEvent| {};
        engine::run_task(
            &mut opened.db,
            &s,
            &Task::UpdateDats { all: false },
            false,
            none,
            &CancelFlag::new(),
        )
        .unwrap();
        engine::run_task(
            &mut opened.db,
            &s,
            &Task::Scan {
                level: ScanLevel::Level2,
                target: ScanTarget::Selected,
            },
            false,
            none,
            &CancelFlag::new(),
        )
        .unwrap();

        let out = engine::run_task(
            &mut opened.db,
            &s,
            &Task::UpdateMia {
                force: false,
                online: cfg.clone(),
            },
            false,
            none,
            &CancelFlag::new(),
        )
        .expect("mia");
        assert!(
            out.summary().contains("files checked, 1 on the list"),
            "{}",
            out.summary()
        );
        // The list's record is far in the past: the file is MIA now.
        let nes = model::dir_at(
            &opened.db.root,
            &[
                "RomVault".into(),
                "Console".into(),
                "NES".into(),
                "Roms".into(),
            ],
        )
        .unwrap();
        let alpha = &nes.children[0];
        let flags: Vec<(String, MiaStatus)> = alpha
            .children
            .iter()
            .map(|f| (f.name.clone(), f.mia()))
            .collect();
        assert_eq!(
            flags,
            [
                ("a.rom".to_owned(), MiaStatus::MIA),
                ("b.rom".to_owned(), MiaStatus::NONE)
            ]
        );
        // The saved cache has the flags.
        let again = engine::open_cache(&s, false).unwrap();
        let nes = model::dir_at(
            &again.db.root,
            &[
                "RomVault".into(),
                "Console".into(),
                "NES".into(),
                "Roms".into(),
            ],
        )
        .unwrap();
        assert_eq!(nes.children[0].children[0].mia(), MiaStatus::MIA);

        // The same version again: nothing to apply.
        let out = engine::run_task(
            &mut opened.db,
            &s,
            &Task::UpdateMia {
                force: false,
                online: cfg.clone(),
            },
            false,
            none,
            &CancelFlag::new(),
        )
        .unwrap();
        assert!(
            out.summary().contains("nothing changed"),
            "{}",
            out.summary()
        );

        // No exe set: the words say so, and they do not hold the key.
        let mut no_exe = cfg;
        no_exe.rom_vault_exe = None;
        let e = engine::run_task(
            &mut opened.db,
            &s,
            &Task::UpdateMia {
                force: true,
                online: no_exe,
            },
            false,
            none,
            &CancelFlag::new(),
        )
        .unwrap_err();
        assert!(e.to_string().contains("rom_vault_exe"), "{e}");
        assert!(!e.to_string().contains(KEY));
    }
}

fn with_tosort(t: &Tmp) -> Settings {
    let mut s = t.settings();
    s.dir_mappings.push(rv_core::DirMapping {
        dir_key: "ToSort".to_owned(),
        dir_path: t.path("ToSort").to_string_lossy().into_owned(),
    });
    s
}

/// A collection with something to fix: `b.rom` is in the DAT but the file `efgh` is called
/// `bee.rom`, and `junk.rom` is in no DAT. The scan has run; the database is returned.
fn fixable(t: &Tmp) -> (Settings, rv_core::Database) {
    let s = with_tosort(t);
    t.write("DatRoot/Console/NES/Roms/nes.dat", DAT.as_bytes());
    t.write("RomRoot/Console/NES/Roms/Alpha/a.rom", b"abcd");
    t.write("RomRoot/Console/NES/Roms/Alpha/bee.rom", b"efgh");
    t.write("RomRoot/Console/NES/Roms/Alpha/junk.rom", b"junk!");
    let mut db = engine::open_cache(&s, false).unwrap().db;
    let nothing = |_: ProgressEvent| {};
    for task in [
        Task::UpdateDats { all: false },
        Task::Scan {
            level: ScanLevel::Level2,
            target: ScanTarget::Dir("RomVault/Console/NES/Roms".to_owned()),
        },
    ] {
        engine::run_task(&mut db, &s, &task, false, &nothing, &CancelFlag::new()).unwrap();
    }
    (s, db)
}

#[test]
fn find_fixes_gives_the_plan_and_writes_nothing() {
    let t = Tmp::new("find");
    let (s, mut db) = fixable(&t);
    let cache_before = fs::read(t.path("test.Cache")).unwrap();
    let out = engine::run_task(
        &mut db,
        &s,
        &Task::FindFixes { review: true },
        false,
        &|_: ProgressEvent| {},
        &CancelFlag::new(),
    )
    .unwrap();
    let engine::Outcome::FoundFixes { plan, review, .. } = &out else {
        panic!("not a Find Fixes outcome: {out:?}");
    };
    assert!(*review);
    let sum = engine::PlanSummary::of(plan);
    // b.rom is made from bee.rom (the plan counts it as a file written), junk.rom goes to ToSort.
    assert_eq!(sum.written, 1, "{plan:?}");
    assert_eq!(sum.moved, 1, "{plan:?}");
    assert_eq!(sum.deleted, 0, "{plan:?}");
    assert!(!sum.is_empty());
    assert!(
        out.summary().starts_with("Find Fixes: "),
        "{}",
        out.summary()
    );

    // The statuses are on the tree, for the lists to show.
    let dir = model::dir_at(
        &db.root,
        &["RomVault", "Console", "NES", "Roms"].map(String::from),
    )
    .unwrap();
    let alpha = &dir.children[0];
    assert!(alpha
        .children
        .iter()
        .any(|c| c.rep_status == RepStatus::CAN_BE_FIXED));

    assert_eq!(
        fs::read(t.path("test.Cache")).unwrap(),
        cache_before,
        "cache untouched"
    );
    assert!(t.path("RomRoot/Console/NES/Roms/Alpha/bee.rom").exists());
    assert!(!t.path("RomRoot/Console/NES/Roms/Alpha/b.rom").exists());
    assert!(!t.path("ToSort").exists());
    assert!(!Task::FindFixes { review: false }.saves_cache());
    assert!(Task::Fix.saves_cache());
}

#[test]
fn fix_makes_the_files_through_rv_fix_and_the_cache_agrees() {
    let t = Tmp::new("fix");
    let (s, mut db) = fixable(&t);
    let out = engine::run_task(
        &mut db,
        &s,
        &Task::Fix,
        false,
        &|_: ProgressEvent| {},
        &CancelFlag::new(),
    )
    .unwrap();
    let engine::Outcome::Fixed(report) = &out else {
        panic!("not a Fix outcome: {out:?}");
    };
    assert!(
        report.error.is_none() && report.save_error.is_none(),
        "{report:?}"
    );
    assert!(out.problem().is_none());
    assert_eq!(
        fs::read(t.path("RomRoot/Console/NES/Roms/Alpha/b.rom")).unwrap(),
        b"efgh"
    );
    assert!(!t.path("RomRoot/Console/NES/Roms/Alpha/bee.rom").exists());
    assert!(!t.path("RomRoot/Console/NES/Roms/Alpha/junk.rom").exists());
    assert!(
        t.path("ToSort").exists(),
        "the junk file was moved, not deleted"
    );

    // The saved cache is what the fix left: nothing left to fix, everything correct.
    let mut again = engine::open_cache(&s, false).unwrap().db;
    let out = engine::run_task(
        &mut again,
        &s,
        &Task::FindFixes { review: false },
        false,
        &|_: ProgressEvent| {},
        &CancelFlag::new(),
    )
    .unwrap();
    let engine::Outcome::FoundFixes { plan, .. } = out else {
        panic!()
    };
    assert!(engine::PlanSummary::of(&plan).is_empty(), "{plan:?}");
}

#[test]
fn a_cancelled_fix_changes_no_file() {
    let t = Tmp::new("fixcancel");
    let (s, mut db) = fixable(&t);
    let cancel = CancelFlag::new();
    cancel.cancel();
    let out = engine::run_task(
        &mut db,
        &s,
        &Task::Fix,
        false,
        &|_: ProgressEvent| {},
        &cancel,
    )
    .unwrap();
    assert!(out.cancelled());
    assert!(out.summary().ends_with("(cancelled)"), "{}", out.summary());
    assert!(t.path("RomRoot/Console/NES/Roms/Alpha/bee.rom").exists());
    assert!(t.path("RomRoot/Console/NES/Roms/Alpha/junk.rom").exists());
    assert!(!t.path("ToSort").exists());
}

#[test]
fn fix_and_find_fixes_refuse_a_path_under_the_live_library() {
    let t = Tmp::new("fixguard");
    let mut db = rv_core::Database::default_db();
    for edit in [
        (|s: &mut Settings| s.dir_mappings[0].dir_path = "/mnt/HHD16TB/RomRoot".to_owned())
            as fn(&mut Settings),
        |s| s.dir_mappings[1].dir_path = "/mnt/HHD16TB/ToSort".to_owned(),
        |s| s.cache_file = "/mnt/HHD16TB/RomVault3_3.Cache".to_owned(),
    ] {
        let mut s = with_tosort(&t);
        edit(&mut s);
        for task in [Task::Fix, Task::FindFixes { review: true }] {
            let r = engine::run_task(
                &mut db,
                &s,
                &task,
                false,
                &|_: ProgressEvent| panic!("the guard comes before any progress"),
                &CancelFlag::new(),
            );
            assert!(
                matches!(r, Err(EngineError::Protected(_))),
                "{task:?} must be refused, got {:?}",
                r.map(|_| ())
            );
        }
    }
    assert!(!t.path("test.Cache").exists(), "nothing was written");
}

#[test]
fn the_question_states_the_counts_and_a_stopped_fix_is_a_problem() {
    let plan = {
        let t = Tmp::new("question");
        let (s, mut db) = fixable(&t);
        match engine::run_task(
            &mut db,
            &s,
            &Task::FindFixes { review: true },
            false,
            &|_: ProgressEvent| {},
            &CancelFlag::new(),
        )
        .unwrap()
        {
            engine::Outcome::FoundFixes { plan, .. } => plan,
            other => panic!("{other:?}"),
        }
    };
    let (message, detail) = engine::PlanSummary::of(&plan).question(plan.still_missing);
    assert_eq!(message, "Fix the ticked directories?");
    assert!(
        detail.starts_with("Fix will write 1 file (a file with the right content is copied or moved into place), move 1 file or archive to ToSort"),
        "{detail}"
    );
    assert!(
        detail.contains("and delete 0 files or archive entries"),
        "{detail}"
    );
    let text = engine::plan_text(&plan, 1);
    assert!(text.starts_with("Dry run: the fix would do this (nothing is written):"));
    assert!(text.contains("... and 1 more"), "{text}");

    let stopped = engine::Outcome::Fixed(rv_fix::FixReport {
        error: Some(rv_fix::FixError::RescanNeeded("a.rom changed.".to_owned())),
        ..Default::default()
    });
    assert_eq!(
        stopped.problem().as_deref(),
        Some("The fix stopped: a.rom changed. Rescan needed.")
    );
    assert!(stopped.summary().contains("(stopped)"));
}
