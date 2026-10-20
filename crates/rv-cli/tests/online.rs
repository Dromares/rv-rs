//! `rv datvault ...` and `rv mia update` against a mock of the service on 127.0.0.1.
//!
//! Nothing here reaches any other host: the online settings of every run name the mock's
//! address (`http://127.0.0.1:<port>/...`, with `allow_plain_http=1`, which is the only way a
//! plain address is accepted), and the real hostnames never appear in this file.
//! No test passes `--allow-live-library`.

use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use rv_archive::zip::{TorrentZipWriter, ZipStructure};
use rv_cache::{load_database, save_database, DatabaseOptions};
use rv_core::ZipStructure as CoreZipStructure;
use rv_core::{Database, FileType, MiaStatus, RvFile, Settings};
use rv_online::datvault::DatInfo;
use rv_online::mia::testing::blob;
use rv_online::mia::MiaRecord;
use rv_online::mock::{self, MockServer, SharedState};
use rv_online::salt::{sha256_hex, synthetic_exe, Salt};
use rv_online::Group;

const KEY: &str = "TEST-KEY-do-not-leak-0123";
/// Synthetic: not ROMVault's salt.
const SYNTH_SALT: [u8; 13] = [11, 22, 33, 44, 55, 66, 77, 88, 99, 110, 121, 132, 143];
const RVA: u32 = 0x1234;

struct Work(PathBuf);

impl Work {
    fn new(tag: &str) -> Work {
        let base = std::env::var_os("RV_WORK_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let dir = base.join(format!("rv-cli-online-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Work(dir)
    }
    fn p(&self, rel: &str) -> PathBuf {
        self.0.join(rel)
    }
    /// Online settings for the mock `server`, plus `extra` lines.
    fn config(&self, server: &MockServer, extra: &str) -> PathBuf {
        let text = format!(
            "dat_vault_key={KEY}\nendpoint={}\nallow_plain_http=1\nmia_cache={}\n{extra}",
            server.url(),
            self.p("MIA.Cache").display()
        );
        let path = self.p("online.conf");
        fs::write(&path, text).unwrap();
        path
    }
    fn rv(&self, args: &[&str]) -> Output {
        let exe = synthetic_exe(RVA, &SYNTH_SALT);
        fs::write(self.p("stand-in.exe"), &exe).unwrap();
        Command::new(env!("CARGO_BIN_EXE_rv"))
            .args(args)
            .env(
                "RV_TEST_SALT_SOURCE",
                format!("{}:{RVA:x}:13", sha256_hex(&exe)),
            )
            .output()
            .unwrap()
    }
}

impl Drop for Work {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn text(o: &Output) -> (String, String) {
    (
        String::from_utf8_lossy(&o.stdout).into_owned(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
    )
}

fn assert_no_key(o: &Output) {
    let (out, err) = text(o);
    assert!(
        !out.contains(KEY) && !err.contains(KEY),
        "the key leaked:\n{out}\n{err}"
    );
}

fn service() -> (SharedState, MockServer) {
    let st = mock::state(KEY);
    let srv = mock::start_service(&st);
    (st, srv)
}

fn info(group: &str, system: &str, version: &str) -> DatInfo {
    DatInfo {
        group: group.into(),
        system: system.into(),
        date: "2024-01-01".into(),
        version: version.into(),
        dat_url: format!("u-{system}-{version}"),
        dat_name: format!("{system}.dat"),
        ..DatInfo::default()
    }
}

fn zst(infos: &[DatInfo]) -> Vec<u8> {
    rv_online::datvault::encode_group_info(infos)
}

fn zip(name: &str, data: &[u8]) -> Vec<u8> {
    let mut w = TorrentZipWriter::new(Cursor::new(Vec::new()), ZipStructure::ZipTrrnt).unwrap();
    w.add_bytes(name, data, None).unwrap();
    w.finish().unwrap().into_inner()
}

fn publish(st: &SharedState, i: &DatInfo, contents: &[u8]) {
    let mut s = st.lock().unwrap();
    s.files.insert(
        format!("{}|{}|{}", i.group, i.system, i.dat_url),
        zip(&i.dat_name, contents),
    );
}

// ---- datvault -------------------------------------------------------------------------------

#[test]
fn check_says_valid_or_not_and_exits_accordingly() {
    let (_st, srv) = service();
    let w = Work::new("check");
    let cfg = w.config(&srv, "");
    let o = w.rv(&[
        "datvault",
        "check",
        "--online-config",
        cfg.to_str().unwrap(),
    ]);
    assert_eq!(o.status.code(), Some(0), "{:?}", text(&o));
    assert!(text(&o).0.contains("is valid"));
    assert_no_key(&o);

    let bad = fs::read_to_string(&cfg)
        .unwrap()
        .replace(KEY, "some-other-key");
    fs::write(&cfg, bad).unwrap();
    let o = w.rv(&[
        "datvault",
        "check",
        "--online-config",
        cfg.to_str().unwrap(),
    ]);
    assert_eq!(o.status.code(), Some(1));
    assert!(text(&o).0.contains("not valid"));
    assert_eq!(srv.count("CheckDVUser"), 2);
}

#[test]
fn a_service_error_that_echoes_the_key_never_reaches_the_terminal() {
    let (st, srv) = service();
    st.lock()
        .unwrap()
        .faulting
        .extend(["CheckDVUser".to_owned(), "ListOfGroupsV2".to_owned()]);
    let w = Work::new("echo");
    let cfg = w.config(&srv, "");
    for args in [&["datvault", "check"][..], &["datvault", "list"]] {
        let mut a = args.to_vec();
        a.extend(["--online-config", cfg.to_str().unwrap(), "--verbose"]);
        let o = w.rv(&a);
        assert_eq!(o.status.code(), Some(1));
        let (_, err) = text(&o);
        assert!(err.contains("fault") && err.contains("***"), "{err}");
        assert_no_key(&o);
    }
}

#[test]
fn a_plain_http_address_is_refused_unless_the_setting_is_on() {
    let (_st, srv) = service();
    let w = Work::new("plain");
    let cfg = w.config(&srv, "");
    let off = fs::read_to_string(&cfg)
        .unwrap()
        .replace("allow_plain_http=1", "allow_plain_http=0");
    fs::write(&cfg, off).unwrap();
    let o = w.rv(&[
        "datvault",
        "check",
        "--online-config",
        cfg.to_str().unwrap(),
    ]);
    assert_eq!(o.status.code(), Some(1));
    let (_, err) = text(&o);
    assert!(err.contains("allow_plain_http"), "{err}");
    assert!(srv.requests().is_empty(), "nothing was sent");
    assert_no_key(&o);
}

#[test]
fn no_key_and_no_config_are_told_so_without_a_request() {
    let (_st, srv) = service();
    let w = Work::new("nokey");
    fs::write(
        w.p("online.conf"),
        format!("endpoint={}\nallow_plain_http=1\n", srv.url()),
    )
    .unwrap();
    let o = w.rv(&[
        "datvault",
        "check",
        "--online-config",
        w.p("online.conf").to_str().unwrap(),
    ]);
    assert_eq!(o.status.code(), Some(1));
    assert!(
        text(&o).1.contains("DatVault key is required"),
        "{:?}",
        text(&o)
    );
    let o = w.rv(&[
        "datvault",
        "check",
        "--online-config",
        w.p("missing.conf").to_str().unwrap(),
    ]);
    assert_eq!(o.status.code(), Some(1));
    assert!(srv.requests().is_empty());
}

#[test]
fn list_then_fetch_a_group_then_update_it() {
    let (st, srv) = service();
    {
        let mut s = st.lock().unwrap();
        s.groups = vec![
            Group {
                basepath: "Console".into(),
                display_name: "No-Intro".into(),
                group: "ni".into(),
                url: String::new(),
            },
            Group {
                basepath: "Arcade".into(),
                display_name: "MAME".into(),
                group: "mame".into(),
                url: String::new(),
            },
        ];
        s.group_info.insert(
            "ni".into(),
            Some(zst(&[info("ni", "nes", "1"), info("ni", "snes", "1")])),
        );
    }
    publish(&st, &info("ni", "nes", "1"), b"nes dat v1");
    publish(&st, &info("ni", "snes", "1"), b"snes dat v1");
    let w = Work::new("fetch");
    let cfg = w.config(&srv, "");
    let cfg = cfg.to_str().unwrap();
    let dat_root = w.p("DatRoot");
    let root = dat_root.to_str().unwrap();

    let o = w.rv(&["datvault", "list", "--online-config", cfg]);
    assert_eq!(o.status.code(), Some(0));
    let (out, _) = text(&o);
    assert!(
        out.contains("Arcade/mame\tMAME")
            && out.contains("Console/ni\tNo-Intro")
            && out.contains("2 groups"),
        "{out}"
    );
    assert!(out.find("Arcade/mame").unwrap() < out.find("Console/ni").unwrap());

    let o = w.rv(&["datvault", "list", "--group", "ni", "--online-config", cfg]);
    let (out, _) = text(&o);
    assert!(
        out.contains("available") && out.contains("ni|nes") && out.contains("ni|snes"),
        "{out}"
    );

    let o = w.rv(&[
        "datvault",
        "fetch",
        "--group",
        "ni",
        "--dat-root",
        root,
        "--online-config",
        cfg,
    ]);
    assert_eq!(o.status.code(), Some(0), "{:?}", text(&o));
    assert!(text(&o)
        .0
        .contains("2 DATs downloaded, 0 removed, 0 failed"));
    let nes = dat_root.join("Console").join("ni").join("nes.dat");
    assert_eq!(fs::read(&nes).unwrap(), b"nes dat v1");
    assert!(dat_root
        .join("Console")
        .join("ni")
        .join("snes.json")
        .is_file());

    // Nothing changed: nothing to do.
    let o = w.rv(&[
        "datvault",
        "fetch",
        "--updates",
        "--dat-root",
        root,
        "--online-config",
        cfg,
    ]);
    assert!(text(&o).0.contains("0 DATs downloaded"), "{:?}", text(&o));

    // A new version of one.
    st.lock().unwrap().group_info.insert(
        "ni".into(),
        Some(zst(&[info("ni", "nes", "2"), info("ni", "snes", "1")])),
    );
    publish(&st, &info("ni", "nes", "2"), b"nes dat v2");
    let o = w.rv(&[
        "datvault",
        "list",
        "--group",
        "ni",
        "--dat-root",
        root,
        "--online-config",
        cfg,
    ]);
    assert!(text(&o).0.contains("update"), "{:?}", text(&o));
    let o = w.rv(&[
        "datvault",
        "fetch",
        "--updates",
        "--dat-root",
        root,
        "--online-config",
        cfg,
    ]);
    assert!(text(&o).0.contains("1 DATs downloaded"), "{:?}", text(&o));
    assert_eq!(fs::read(&nes).unwrap(), b"nes dat v2");
    assert_no_key(&o);
}

#[test]
fn a_dat_root_in_the_live_library_is_refused() {
    let (_st, srv) = service();
    let w = Work::new("guard");
    let cfg = w.config(&srv, "");
    let o = w.rv(&[
        "datvault",
        "fetch",
        "--group",
        "g",
        "--dat-root",
        "/mnt/HHD16TB/DatRoot",
        "--online-config",
        cfg.to_str().unwrap(),
    ]);
    assert_eq!(o.status.code(), Some(3));
    assert!(srv.requests().is_empty());
}

// ---- mia ------------------------------------------------------------------------------------

fn record(n: u8, days_ago: u64) -> MiaRecord {
    MiaRecord {
        size: Some(100 + u64::from(n)),
        crc: Some([n, 0, 0, 1]),
        sha1: Some([n; 20]),
        md5: Some([n; 16]),
        date_ticks: Some(rv_online::mia::now_ticks() as u64 - days_ago * 864_000_000_000),
    }
}

fn rom(name: &str, n: u8) -> RvFile {
    let mut f = RvFile::new(FileType::FILE_ZIP, name);
    f.size = Some(100 + u64::from(n));
    f.crc = Some(vec![n, 0, 0, 1]);
    f.sha1 = Some(vec![n; 20]);
    f.md5 = Some(vec![n; 16]);
    f
}

/// A cache with `young.rom` (on the list, 2 days old), `old.rom` (on the list, 90 days old) and
/// `other.rom` (not on it).
fn make_cache(path: &Path) {
    let mut db = Database::default_db();
    let mut game = RvFile::new(FileType::ZIP, "game.zip");
    game.zip_struct = Some(CoreZipStructure::ZIP_TRRNT);
    game.zip_dat_struct = Some(CoreZipStructure::ZIP_TRRNT.0);
    game.children = vec![rom("young.rom", 1), rom("old.rom", 2), rom("other.rom", 3)];
    db.root.children[0].children.push(game);
    save_database(path, &mut db).unwrap();
}

fn flags(cache: &Path) -> Vec<(String, MiaStatus)> {
    let (db, _) = load_database(cache, &Settings::default(), DatabaseOptions::default()).unwrap();
    db.root.children[0].children[0]
        .children
        .iter()
        .map(|f| (f.name.clone(), f.mia()))
        .collect()
}

fn mia_service(st: &SharedState, version: i64) {
    let salt = Salt::from_bytes(&SYNTH_SALT);
    let mut s = st.lock().unwrap();
    s.mia_version = Some(version);
    s.mia_blob = Some(blob(
        version as u64,
        &[record(1, 2), record(2, 90)],
        KEY,
        &salt,
    ));
}

fn mia_args<'a>(w: &'a Work, cfg: &'a Path, extra: &[&'a str]) -> Vec<String> {
    let mut v: Vec<String> = [
        "mia",
        "update",
        "--cache",
        w.p("lib.Cache").to_str().unwrap(),
        "--root",
        w.p("RomRoot").to_str().unwrap(),
        "--online-config",
        cfg.to_str().unwrap(),
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    v.extend(extra.iter().map(|s| (*s).to_owned()));
    v
}

fn run(w: &Work, a: &[String]) -> Output {
    let refs: Vec<&str> = a.iter().map(String::as_str).collect();
    w.rv(&refs)
}

#[test]
fn mia_update_marks_the_cache_and_a_second_run_changes_nothing() {
    let (st, srv) = service();
    mia_service(&st, 700);
    let w = Work::new("mia");
    make_cache(&w.p("lib.Cache"));
    let exe = w.p("stand-in.exe");
    let cfg = w.config(&srv, &format!("rom_vault_exe={}\n", exe.display()));
    // `rv` writes the stand-in itself; make sure it exists for the settings check.
    fs::write(&exe, synthetic_exe(RVA, &SYNTH_SALT)).unwrap();

    let o = run(&w, &mia_args(&w, &cfg, &[]));
    assert_eq!(o.status.code(), Some(0), "{:?}", text(&o));
    let (out, _) = text(&o);
    assert!(
        out.contains("Update complete, MIA release")
            && out.contains("3 files checked, 2 on the list"),
        "{out}"
    );
    assert_no_key(&o);
    let f = flags(&w.p("lib.Cache"));
    assert_eq!(f[0], ("young.rom".to_owned(), MiaStatus::NEW));
    assert_eq!(f[1], ("old.rom".to_owned(), MiaStatus::MIA));
    assert_eq!(f[2], ("other.rom".to_owned(), MiaStatus::NONE));
    assert!(w.p("MIA.Cache").is_file());
    assert_eq!(srv.count("GetAutoMIA"), 1);

    // The same version again: the cache is used, nothing is downloaded or applied.
    let o = run(&w, &mia_args(&w, &cfg, &[]));
    assert_eq!(o.status.code(), Some(0));
    assert!(
        text(&o).0.contains("Nothing changed, MIA release"),
        "{:?}",
        text(&o)
    );
    assert_eq!(srv.count("GetAutoMIA"), 1);
    // --force applies it again.
    let o = run(&w, &mia_args(&w, &cfg, &["--force"]));
    assert!(text(&o).0.contains("Update complete"), "{:?}", text(&o));
    assert_eq!(srv.count("GetAutoMIA"), 1);
}

#[test]
fn mia_update_says_what_is_missing() {
    let (st, srv) = service();
    mia_service(&st, 700);
    let w = Work::new("miamissing");
    make_cache(&w.p("lib.Cache"));
    // No key.
    fs::write(
        w.p("online.conf"),
        format!("endpoint={}\nallow_plain_http=1\n", srv.url()),
    )
    .unwrap();
    let o = run(&w, &mia_args(&w, &w.p("online.conf"), &[]));
    assert_eq!(o.status.code(), Some(1));
    assert!(text(&o).1.contains("DatVault key is required"));
    // A key, but no exe named.
    let cfg = w.config(&srv, "");
    let o = run(&w, &mia_args(&w, &cfg, &[]));
    assert_eq!(o.status.code(), Some(1));
    assert!(text(&o).1.contains("rom_vault_exe"), "{:?}", text(&o));
    // An exe that is not the one this version knows.
    fs::write(w.p("other.exe"), b"MZ not the right file").unwrap();
    let cfg = w.config(
        &srv,
        &format!("rom_vault_exe={}\n", w.p("other.exe").display()),
    );
    let o = run(&w, &mia_args(&w, &cfg, &[]));
    assert_eq!(o.status.code(), Some(1));
    assert!(
        text(&o).1.contains("not the ROMVault 3.8.0 exe"),
        "{:?}",
        text(&o)
    );
    assert!(
        srv.requests().is_empty(),
        "nothing was asked of the service"
    );
}

#[test]
fn a_wrong_key_for_the_list_is_a_failure_not_a_crash() {
    let (st, srv) = service();
    // The service holds a list made with another key.
    let salt = Salt::from_bytes(&SYNTH_SALT);
    {
        let mut s = st.lock().unwrap();
        s.mia_version = Some(5);
        s.mia_blob = Some(blob(5, &[record(1, 2)], "someone else's key", &salt));
    }
    let w = Work::new("miawrong");
    make_cache(&w.p("lib.Cache"));
    let exe = w.p("stand-in.exe");
    fs::write(&exe, synthetic_exe(RVA, &SYNTH_SALT)).unwrap();
    let cfg = w.config(&srv, &format!("rom_vault_exe={}\n", exe.display()));
    let o = run(&w, &mia_args(&w, &cfg, &[]));
    assert_eq!(o.status.code(), Some(1));
    assert!(text(&o).1.contains("MIA fetch failed"), "{:?}", text(&o));
    assert_no_key(&o);
    assert!(flags(&w.p("lib.Cache"))
        .iter()
        .all(|(_, m)| *m == MiaStatus::NONE));
    assert!(!w.p("MIA.Cache").exists());
}

#[test]
fn an_unreachable_service_with_no_cache_is_expired() {
    let w = Work::new("mianet");
    make_cache(&w.p("lib.Cache"));
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    fs::write(
        w.p("online.conf"),
        format!(
            "dat_vault_key={KEY}\nendpoint=http://127.0.0.1:{port}/x\nallow_plain_http=1\nmia_cache={}\nrom_vault_exe={}\n",
            w.p("MIA.Cache").display(),
            w.p("stand-in.exe").display()
        ),
    )
    .unwrap();
    fs::write(w.p("stand-in.exe"), synthetic_exe(RVA, &SYNTH_SALT)).unwrap();
    let o = run(&w, &mia_args(&w, &w.p("online.conf"), &[]));
    assert_eq!(o.status.code(), Some(1));
    assert!(
        text(&o).1.contains("invalid or has expired"),
        "{:?}",
        text(&o)
    );
}

#[test]
fn the_online_config_in_the_live_library_is_refused() {
    let w = Work::new("miaguard");
    make_cache(&w.p("lib.Cache"));
    let a = mia_args(&w, Path::new("/mnt/HHD16TB/online.conf"), &[]);
    let o = run(&w, &a);
    assert_eq!(o.status.code(), Some(3));
}
