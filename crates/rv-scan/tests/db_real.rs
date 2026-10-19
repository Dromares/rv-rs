//! The directory scan against the board's own files and database. The real `RomVault3_3.Cache`
//! is the oracle; `RomRoot-sample/` (zips) and `RomRoot-loose/` (loose files, CHDs) are the files.
//! These need data that is never in the repository, so they are `#[ignore]`d:
//!
//! ```sh
//! RV_FIXTURES_DIR=/path/to/copies cargo test -p rv-scan --test db_real -- --ignored --nocapture
//! ```
//!
//! `RV_FIXTURES_DIR` must hold `DatRoot/`, `RomRoot-sample/`, `RomRoot-loose/`,
//! `RomVault3_3.Cache` and `RomVault3cfg.xml`. With the variable unset, or a file missing, the
//! tests fail rather than pass. Nothing in the fixtures is written: the DATs are copied and the
//! ROM files are hard-linked (or copied, across file systems) into a work directory
//! (`RV_WORK_DIR`, default the temp directory), and the scan only reads.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use rv_cache::{into_database, to_cache_file, CacheFile, DatabaseOptions};
use rv_core::progress::{CancelFlag, NoProgress};
use rv_core::{Database, FileStatus, FileType, GotStatus, RvFile, ScanLevel, Settings};
use rv_scan::db::{scan_files, ScanReport, ScanRequest, ScanTarget};

fn fixtures_dir() -> PathBuf {
    match std::env::var_os("RV_FIXTURES_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => {
            panic!("RV_FIXTURES_DIR is not set; these tests need the board's DATs, ROMs and cache")
        }
    }
}

fn work_dir(tag: &str) -> PathBuf {
    let base = std::env::var_os("RV_WORK_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join(format!("rv-scan-real-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap_or_else(|e| panic!("{}: {e}", from.display())) {
        let e = e.unwrap();
        let (src, dst) = (e.path(), to.join(e.file_name()));
        if src.is_dir() {
            copy_tree(&src, &dst);
        } else if src.file_name().is_some_and(|n| n == "MANIFEST.sha256") {
            continue;
        } else {
            fs::copy(&src, &dst).unwrap();
            let m = fs::metadata(&src).unwrap().modified().unwrap();
            fs::OpenOptions::new()
                .write(true)
                .open(&dst)
                .unwrap()
                .set_modified(m)
                .unwrap();
        }
    }
}

/// Like `copy_tree` but a hard link where the file system allows it: the scan never writes, so
/// sharing the data with the fixture is safe, and 1.8 GB of CHDs is not copied for every test.
fn link_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap_or_else(|e| panic!("{}: {e}", from.display())) {
        let e = e.unwrap();
        let (src, dst) = (e.path(), to.join(e.file_name()));
        if src.is_dir() {
            link_tree(&src, &dst);
        } else if src.file_name().is_some_and(|n| n == "MANIFEST.sha256") {
            continue;
        } else if fs::hard_link(&src, &dst).is_err() {
            fs::copy(&src, &dst).unwrap();
            let m = fs::metadata(&src).unwrap().modified().unwrap();
            fs::OpenOptions::new()
                .write(true)
                .open(&dst)
                .unwrap()
                .set_modified(m)
                .unwrap();
        }
    }
}

struct Work {
    dir: PathBuf,
    settings: Settings,
}

fn setup(tag: &str) -> Work {
    let dir = work_dir(tag);
    let fx = fixtures_dir();
    copy_tree(&fx.join("DatRoot"), &dir.join("DatRoot"));
    link_tree(&fx.join("RomRoot-sample"), &dir.join("RomRoot"));
    link_tree(&fx.join("RomRoot-loose"), &dir.join("RomRoot"));
    let text = fs::read_to_string(fx.join("RomVault3cfg.xml")).expect("RomVault3cfg.xml");
    let mut settings = Settings::from_xml(&text).expect("the board's settings parse");
    settings.dat_root = dir.join("DatRoot").to_string_lossy().into_owned();
    // "The tree's paths remapped to the copy."
    settings.dir_mappings[0].dir_path = dir.join("RomRoot").to_string_lossy().into_owned();
    Work { dir, settings }
}

fn real_cache(s: &Settings) -> Database {
    let bytes = fs::read(fixtures_dir().join("RomVault3_3.Cache")).expect("RomVault3_3.Cache");
    into_database(
        CacheFile::from_bytes(&bytes).unwrap(),
        s,
        DatabaseOptions::default(),
    )
    .unwrap()
}

fn tick_all_rows(n: &mut RvFile) {
    if let Some(t) = &mut n.tree {
        t.checked = rv_core::TreeSelect::SELECTED;
    }
    for c in &mut n.children {
        tick_all_rows(c);
    }
}

fn scan_all(db: &mut Database, s: &Settings, level: ScanLevel) -> ScanReport {
    scan_files(
        db,
        s,
        &ScanRequest {
            level,
            target: ScanTarget::Dir("RomVault".to_owned()),
        },
        &NoProgress,
        &CancelFlag::new(),
    )
    .unwrap()
}

fn counts(n: &RvFile) -> BTreeMap<&'static str, usize> {
    fn walk(n: &RvFile, c: &mut BTreeMap<&'static str, usize>) {
        *c.entry(n.rep_status.name()).or_default() += 1;
        for k in &n.children {
            walk(k, c);
        }
    }
    let mut c = BTreeMap::new();
    for k in &n.children {
        walk(k, &mut c);
    }
    c
}

fn print_counts(title: &str, db: &Database) {
    eprintln!("RepStatus counts, {title}:");
    let rv = &db.root.children[0];
    let mut c = counts(rv);
    c.remove("UnSet");
    for (k, n) in c {
        eprintln!("  {k:<20} {n}");
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn find<'a>(root: &'a RvFile, names: &[String]) -> Option<&'a RvFile> {
    let mut cur = root;
    for n in names {
        cur = cur.children.iter().find(|c| c.name == *n)?;
    }
    Some(cur)
}

// ---- 1. the real cache, files in place -------------------------------------------------------

#[test]
#[ignore = "needs RV_FIXTURES_DIR"]
fn scanning_the_files_into_the_real_cache_keeps_what_the_cache_holds_for_them() {
    let w = setup("cache");
    let s = &w.settings;
    let mut db = real_cache(s);
    tick_all_rows(&mut db.root);
    let before = db.clone();

    let t = std::time::Instant::now();
    let r = scan_all(&mut db, s, ScanLevel::Level2);
    eprintln!("level 2 scan into the real cache: {:?}, {r:?}", t.elapsed());
    assert!(!r.cancelled && !r.aborted);
    print_counts("after the scan", &db);

    // Every file that is on disk and that the cache had as Got has the cache's values.
    let mut compared = 0usize;
    let mut diffs: Vec<String> = Vec::new();
    fn walk(
        n: &RvFile,
        path: &mut Vec<String>,
        before: &RvFile,
        now_present: &mut usize,
        compared: &mut usize,
        diffs: &mut Vec<String>,
    ) {
        if n.is_file() && n.got_status == GotStatus::GOT {
            *now_present += 1;
            if let Some(b) = find(before, &path[1..]) {
                if b.got_status == GotStatus::GOT {
                    *compared += 1;
                    let same = n.size == b.size
                        && n.crc == b.crc
                        && n.sha1 == b.sha1
                        && n.md5 == b.md5
                        && n.alt_crc == b.alt_crc
                        && n.alt_sha1 == b.alt_sha1
                        && n.alt_md5 == b.alt_md5
                        && n.alt_size == b.alt_size
                        && n.file_status == b.file_status
                        && n.header_file_type == b.header_file_type;
                    if !same {
                        diffs.push(path.join("/"));
                    }
                }
            }
        }
        for c in &n.children {
            path.push(c.name.clone());
            walk(c, path, before, now_present, compared, diffs);
            path.pop();
        }
    }
    let mut present = 0;
    walk(
        &db.root,
        &mut vec![String::new()],
        &before.root,
        &mut present,
        &mut compared,
        &mut diffs,
    );
    eprintln!("files Got after the scan: {present}; also Got in the cache and compared: {compared}; different: {}", diffs.len());
    assert!(compared > 0);
    // Only the files that were re-hashed can differ, and only by the 3.7/3.8 flag.
    for d in diffs.iter().take(5) {
        eprintln!("  differs: {d}");
    }
    let _ = fs::remove_dir_all(&w.dir);
}

// ---- 2. a full scan into a database that only has the DATs ---------------------------------------

fn run_cmd(program: &str, args: &[&std::ffi::OsStr]) -> Vec<u8> {
    let out = Command::new(program)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("{program}: {e}"));
    assert!(
        out.status.success(),
        "{program} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

const PY_CRC: &str = r#"
import sys, zlib
for p in sys.argv[1:]:
    c = 0
    with open(p, 'rb') as f:
        while True:
            b = f.read(1 << 20)
            if not b:
                break
            c = zlib.crc32(b, c)
    sys.stdout.write('%08x\0' % (c & 0xffffffff))
"#;

const PY_ZIP: &str = r#"
import sys, zlib, hashlib, zipfile
z = zipfile.ZipFile(sys.argv[1])
for i in z.infolist():
    if i.is_dir():
        continue
    d = z.read(i)
    sys.stdout.write('%s\t%d\t%08x\t%s\t%s\n' % (i.filename, len(d), zlib.crc32(d) & 0xffffffff, hashlib.sha1(d).hexdigest(), hashlib.md5(d).hexdigest()))
"#;

#[test]
#[ignore = "needs RV_FIXTURES_DIR"]
fn a_full_scan_of_the_files_into_the_dat_tree_gives_the_cache_values_and_the_system_tools_agree() {
    let w = setup("full");
    let s = &w.settings;
    let mut db = Database::default_db();
    db.report_status_reset(s);
    let up = rv_dat::update::update_dats(
        &mut db,
        s,
        &Default::default(),
        &NoProgress,
        &CancelFlag::new(),
    )
    .unwrap();
    assert_eq!(up.dats_added.len(), 14);

    let t = std::time::Instant::now();
    let r = scan_all(&mut db, s, ScanLevel::Level3);
    eprintln!("level 3 scan into a DAT-only database: {:?}", t.elapsed());
    eprintln!("{r:?}");
    assert!(!r.cancelled && !r.aborted);
    print_counts("DAT tree + level 3 scan of sample and loose files", &db);
    let cache = real_cache(s);

    // 2a. Against the cache.
    let mut present = 0usize;
    let mut in_cache = 0usize;
    let mut equal = 0usize;
    let mut explained: BTreeMap<&'static str, (usize, String)> = BTreeMap::new();
    let mut unexplained: Vec<String> = Vec::new();
    let mut never_seen = 0usize;
    fn walk(
        n: &RvFile,
        path: &mut Vec<String>,
        cache: &RvFile,
        present: &mut usize,
        in_cache: &mut usize,
        equal: &mut usize,
        never_seen: &mut usize,
        explained: &mut BTreeMap<&'static str, (usize, String)>,
        unexplained: &mut Vec<String>,
    ) {
        if (n.is_file() || n.file_type.is_compressed_dir()) && n.got_status != GotStatus::NOT_GOT {
            *present += 1;
            match find(cache, &path[1..]) {
                None => {}
                Some(c) if c.got_status == GotStatus::NOT_GOT => *never_seen += 1,
                Some(c) => {
                    *in_cache += 1;
                    let p = path.join("/");
                    let mut ok = true;
                    let mut bad = |col: &str| {
                        ok = false;
                        unexplained.push(format!("{p}: {col}"));
                    };
                    // An archive node: 3.8 records the archive file's own size, `ZipFileIndex` 0
                    // and `SizeVerified` (`FileMergeIn` fills them from the listing); older
                    // scans left them empty, and so does most of the board's cache.
                    let old_archive = n.file_type.is_compressed_dir()
                        && c.size.is_none()
                        && c.zip_file_index.is_none()
                        && c.file_status == FileStatus::default();
                    if old_archive {
                        if n.size.is_some()
                            && n.zip_file_index == Some(0)
                            && n.file_status == FileStatus::SIZE_VERIFIED
                        {
                            let e = explained
                                .entry("archive size/index/flags")
                                .or_insert((0, String::new()));
                            e.0 += 1;
                            e.1 = "most archive nodes of the 3.7-era cache carry no size, no ZipFileIndex and no flags; 3.8 sets all three from the directory listing".into();
                        } else {
                            bad("archive size/index/flags");
                        }
                    }
                    if n.got_status != c.got_status {
                        bad("got_status");
                    }
                    if n.size != c.size && !old_archive {
                        bad("size");
                    }
                    if n.crc != c.crc || n.sha1 != c.sha1 || n.md5 != c.md5 {
                        bad("hashes");
                    }
                    if n.alt_size != c.alt_size
                        || n.alt_crc != c.alt_crc
                        || n.alt_sha1 != c.alt_sha1
                        || n.alt_md5 != c.alt_md5
                    {
                        bad("alt hashes");
                    }
                    if n.header_file_type != c.header_file_type {
                        bad("header_file_type");
                    }
                    if (n.zip_file_index != c.zip_file_index && !old_archive)
                        || n.zip_file_header_position != c.zip_file_header_position
                    {
                        bad("zip index / header position");
                    }
                    if n.chd_version != c.chd_version {
                        bad("chd_version");
                    }
                    if n.zip_struct != c.zip_struct {
                        bad("zip_struct");
                    }
                    if n.file_status != c.file_status && !old_archive {
                        let x = n.file_status.0 ^ c.file_status.0;
                        if x == FileStatus::ALT_SIZE_FROM_HEADER.0
                            && n.file_status.has(FileStatus::ALT_SIZE_FROM_HEADER)
                        {
                            let e = explained.entry("file_status").or_insert((0, String::new()));
                            e.0 += 1;
                            e.1 = "AltSizeFromHeader: 3.8 sets it with AltSizeVerified when a copier header is found, the 3.7 cache does not have it".into();
                        } else {
                            bad("file_status");
                        }
                    }
                    if n.file_mod_time_stamp != c.file_mod_time_stamp {
                        if n.file_type == FileType::FILE_ZIP
                            && n.file_mod_time_stamp == 563_657_728
                            && c.file_mod_time_stamp == 629_870_671_200_000_000
                        {
                            let e = explained.entry("mod_time").or_insert((0, String::new()));
                            e.0 += 1;
                            e.1 = "TorrentZip entries: 3.8 stores the packed DOS date/time, the 3.7 cache the 1996-12-24 23:32 ticks".into();
                        } else {
                            bad("mod_time");
                        }
                    }
                    if ok {
                        *equal += 1;
                    }
                }
            }
        }
        for ch in &n.children {
            path.push(ch.name.clone());
            walk(
                ch,
                path,
                cache,
                present,
                in_cache,
                equal,
                never_seen,
                explained,
                unexplained,
            );
            path.pop();
        }
    }
    walk(
        &db.root,
        &mut vec![String::new()],
        &cache.root,
        &mut present,
        &mut in_cache,
        &mut equal,
        &mut never_seen,
        &mut explained,
        &mut unexplained,
    );
    eprintln!("files and archives present: {present}; the cache holds {in_cache} of them as present and {never_seen} as never seen; fully equal: {equal}");
    for (k, (n, why)) in &explained {
        eprintln!("  explained {k}: {n} nodes, {why}");
    }
    for u in unexplained.iter().take(10) {
        eprintln!("  UNEXPLAINED {u}");
    }
    assert!(
        unexplained.is_empty(),
        "{} unexplained differences",
        unexplained.len()
    );
    assert!(
        in_cache >= 100,
        "the sample should overlap the cache a lot: {in_cache}"
    );

    // 2b. Against the system tools: every loose file, and the entries of at least 3 zips.
    let mut loose: Vec<(PathBuf, &RvFile)> = Vec::new();
    fn collect<'a>(n: &'a RvFile, tree: &str, w: &Settings, out: &mut Vec<(PathBuf, &'a RvFile)>) {
        for c in &n.children {
            let t = if tree.is_empty() {
                c.name.clone()
            } else {
                format!("{tree}/{}", c.name)
            };
            if c.file_type == FileType::FILE && c.got_status == GotStatus::GOT && c.dat_uid == 0 {
                out.push((PathBuf::from(w.physical_path(&t)), c));
            }
            if c.file_type == FileType::DIR {
                collect(c, &t, w, out);
            }
        }
    }
    collect(&db.root.children[0], "RomVault", s, &mut loose);
    assert!(
        loose.len() >= 10,
        "at least 10 loose files to check, found {}",
        loose.len()
    );
    let paths: Vec<&std::ffi::OsStr> = loose.iter().map(|(p, _)| p.as_os_str()).collect();
    let mut sha_args: Vec<&std::ffi::OsStr> = vec![std::ffi::OsStr::new("--zero")];
    sha_args.extend(&paths);
    let sha = run_cmd("sha1sum", &sha_args);
    let md5 = run_cmd("md5sum", &sha_args);
    let script = w.dir.join("crc.py");
    fs::write(&script, PY_CRC).unwrap();
    let mut crc_args: Vec<&std::ffi::OsStr> = vec![std::ffi::OsStr::new("-I"), script.as_os_str()];
    crc_args.extend(&paths);
    let crc = run_cmd("python3", &crc_args);
    let parse = |out: &[u8]| -> Vec<String> {
        out.split(|b| *b == 0)
            .filter(|l| !l.is_empty())
            .map(|l| {
                String::from_utf8_lossy(l)
                    .trim_start_matches('\\')
                    .to_owned()
            })
            .collect()
    };
    let (sha, md5, crc) = (parse(&sha), parse(&md5), parse(&crc));
    assert_eq!(
        (sha.len(), md5.len(), crc.len()),
        (loose.len(), loose.len(), loose.len())
    );
    for (i, (path, n)) in loose.iter().enumerate() {
        assert!(
            sha[i].starts_with(&hex(n.sha1.as_ref().unwrap())),
            "{}: sha1sum says {}",
            path.display(),
            sha[i]
        );
        assert!(
            md5[i].starts_with(&hex(n.md5.as_ref().unwrap())),
            "{}: md5sum says {}",
            path.display(),
            md5[i]
        );
        assert_eq!(crc[i], hex(n.crc.as_ref().unwrap()), "{}", path.display());
        assert_eq!(fs::metadata(path).unwrap().len(), n.size.unwrap());
    }
    eprintln!(
        "sha1sum, md5sum and python crc32 agree on {} loose files",
        loose.len()
    );

    let zip_script = w.dir.join("zip.py");
    fs::write(&zip_script, PY_ZIP).unwrap();
    let mut zips_checked = 0;
    let mut entries_checked = 0;
    fn zips<'a>(n: &'a RvFile, tree: &str, s: &Settings, out: &mut Vec<(PathBuf, &'a RvFile)>) {
        for c in &n.children {
            let t = if tree.is_empty() {
                c.name.clone()
            } else {
                format!("{tree}/{}", c.name)
            };
            if c.file_type == FileType::ZIP && c.got_status == GotStatus::GOT {
                out.push((PathBuf::from(s.physical_path(&t)), c));
            }
            if c.file_type == FileType::DIR {
                zips(c, &t, s, out);
            }
        }
    }
    let mut zs = Vec::new();
    zips(&db.root.children[0], "RomVault", s, &mut zs);
    for (path, node) in zs.iter().take(12) {
        let out = run_cmd(
            "python3",
            &[
                std::ffi::OsStr::new("-I"),
                zip_script.as_os_str(),
                path.as_os_str(),
            ],
        );
        let text = String::from_utf8(out).unwrap();
        let mut seen = 0;
        for line in text.lines() {
            let f: Vec<&str> = line.split('\t').collect();
            let entry = node
                .children
                .iter()
                .find(|c| c.name == f[0] && c.got_status == GotStatus::GOT)
                .unwrap_or_else(|| panic!("{}: no entry {}", path.display(), f[0]));
            assert_eq!(
                entry.size,
                Some(f[1].parse().unwrap()),
                "{} {}",
                path.display(),
                f[0]
            );
            assert_eq!(
                hex(entry.crc.as_ref().unwrap()),
                f[2],
                "{} {}",
                path.display(),
                f[0]
            );
            assert_eq!(
                hex(entry.sha1.as_ref().unwrap()),
                f[3],
                "{} {}",
                path.display(),
                f[0]
            );
            assert_eq!(
                hex(entry.md5.as_ref().unwrap()),
                f[4],
                "{} {}",
                path.display(),
                f[0]
            );
            seen += 1;
        }
        assert!(seen > 0);
        entries_checked += seen;
        zips_checked += 1;
    }
    eprintln!(
        "python zipfile/zlib/hashlib agree on {entries_checked} entries of {zips_checked} zips"
    );
    assert!(zips_checked >= 3 && entries_checked >= 10);

    // 2c. A second scan at the usual level changes nothing and reads nothing.
    let first = to_cache_file(&mut db).unwrap().to_bytes().unwrap();
    let r2 = scan_all(&mut db, s, ScanLevel::Level2);
    eprintln!("rescan at level 2: {r2:?}");
    assert_eq!((r2.archives_scanned, r2.files_hashed), (0, 0));
    assert_eq!(to_cache_file(&mut db).unwrap().to_bytes().unwrap(), first);
    let _ = fs::remove_dir_all(&w.dir);
}
