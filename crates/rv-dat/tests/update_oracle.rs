//! The DAT update against the board's own database. The real `RomVault3_3.Cache` is the oracle.
//! These need files that are never in the repository, so they are `#[ignore]`d:
//!
//! ```sh
//! RV_FIXTURES_DIR=/path/to/copies cargo test -p rv-dat --test update_oracle -- --ignored --nocapture
//! ```
//!
//! `RV_FIXTURES_DIR` must hold `DatRoot/` (14 DATs, modification times kept), `RomVault3_3.Cache`
//! and `RomVault3cfg.xml`. With the variable unset, or a file missing, the tests fail rather
//! than pass. Nothing in the fixtures directory is written: the DATs are copied to a temp
//! directory and the cache is read into memory.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rv_cache::{into_database, to_cache_file, CacheFile, DatabaseOptions};
use rv_core::progress::{CancelFlag, NoProgress};
use rv_core::{dat_data, game_data, DatStatus, Database, FileStatus, RvFile, Settings};
use rv_dat::update::{update_dats, UpdateOptions};

fn fixtures_dir() -> PathBuf {
    match std::env::var_os("RV_FIXTURES_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => panic!("RV_FIXTURES_DIR is not set; these tests need the board's DATs and cache"),
    }
}

fn work_dir(tag: &str) -> PathBuf {
    let base = std::env::var_os("RV_WORK_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join(format!("rv-update-oracle-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Copy a tree of files, keeping the modification times (a DAT's time stamp is part of how the
/// update recognises it).
fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap_or_else(|e| panic!("{}: {e}", from.display())) {
        let e = e.unwrap();
        let (src, dst) = (e.path(), to.join(e.file_name()));
        if src.is_dir() {
            copy_tree(&src, &dst);
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

/// The board's settings, pointed at the copies.
fn settings(work: &Path) -> Settings {
    let text = fs::read_to_string(fixtures_dir().join("RomVault3cfg.xml"))
        .expect("RomVault3cfg.xml in RV_FIXTURES_DIR");
    let mut s = Settings::from_xml(&text).expect("the board's settings parse");
    s.dat_root = work.join("DatRoot").to_string_lossy().into_owned();
    s.dir_mappings[0].dir_path = work.join("RomRoot").to_string_lossy().into_owned();
    s
}

fn real_cache_bytes() -> Vec<u8> {
    fs::read(fixtures_dir().join("RomVault3_3.Cache"))
        .expect("RomVault3_3.Cache in RV_FIXTURES_DIR")
}

fn database(bytes: &[u8], s: &Settings, fixups: bool) -> Database {
    into_database(
        CacheFile::from_bytes(bytes).unwrap(),
        s,
        DatabaseOptions {
            apply_load_fixups: fixups,
        },
    )
    .unwrap()
}

fn run_update(db: &mut Database, s: &Settings) -> rv_dat::update::UpdateReport {
    update_dats(
        db,
        s,
        &UpdateOptions::default(),
        &NoProgress,
        &CancelFlag::new(),
    )
    .unwrap()
}

fn find_dat_dirs<'a>(n: &'a RvFile, path: &str, out: &mut Vec<(String, &'a RvFile)>) {
    let p = if path.is_empty() {
        n.name.clone()
    } else {
        format!("{path}/{}", n.name)
    };
    if !n.dir_dats.is_empty() {
        out.push((p.clone(), n));
    }
    for c in &n.children {
        if c.is_directory() {
            find_dat_dirs(c, &p, out);
        }
    }
}

// ---- 1. from an empty database ------------------------------------------------------------------

/// The DAT side of a cache node: a value counts as the DAT's when the cache says it came from
/// the DAT (`FromDAT`, or `AltFromDAT` for a file the scanner matched through its alt hashes).
fn dat_view<T: Clone>(
    n: &RvFile,
    base: FileStatus,
    alt: FileStatus,
    main: &Option<T>,
    alt_value: &Option<T>,
) -> Option<T> {
    if n.file_status.has(alt) {
        alt_value.clone()
    } else if n.file_status.has(base) {
        main.clone()
    } else {
        None
    }
}

/// `DatFlags` with the alt flags folded onto the main ones.
fn dat_flags(n: &RvFile) -> u32 {
    let f = n.file_status.0;
    let mut v = f & FileStatus::DAT_FLAGS.0;
    for (alt, base) in [
        (FileStatus::ALT_SIZE_FROM_DAT, FileStatus::SIZE_FROM_DAT),
        (FileStatus::ALT_CRC_FROM_DAT, FileStatus::CRC_FROM_DAT),
        (FileStatus::ALT_SHA1_FROM_DAT, FileStatus::SHA1_FROM_DAT),
        (FileStatus::ALT_MD5_FROM_DAT, FileStatus::MD5_FROM_DAT),
    ] {
        if f & alt.0 != 0 {
            v = (v & !alt.0) | base.0;
        }
    }
    v
}

#[derive(Default)]
struct Diffs {
    /// column -> (count, example)
    unexplained: BTreeMap<String, (usize, String)>,
    /// column -> (count, why, example)
    explained: BTreeMap<String, (usize, &'static str, String)>,
    compared_nodes: usize,
}

impl Diffs {
    fn unexplained(&mut self, col: &str, example: String) {
        let e = self
            .unexplained
            .entry(col.to_owned())
            .or_insert((0, example));
        e.0 += 1;
    }

    fn explained(&mut self, col: &str, why: &'static str, example: String) {
        let e = self
            .explained
            .entry(col.to_owned())
            .or_insert((0, why, example));
        e.0 += 1;
    }
}

fn dat_children(n: &RvFile) -> Vec<&RvFile> {
    n.children
        .iter()
        .filter(|c| c.dat_status != DatStatus::NOT_IN_DAT && c.dat_status != DatStatus::IN_TO_SORT)
        .collect()
}

fn compare_node(ours: &RvFile, cache: &RvFile, path: &str, d: &mut Diffs) {
    let p = format!("{path}/{}", ours.name);
    d.compared_nodes += 1;
    if ours.dat_status != cache.dat_status {
        d.unexplained(
            "dat_status",
            format!("{p}: {} vs {}", ours.dat_status.0, cache.dat_status.0),
        );
    }
    if ours.is_file() {
        let views = [
            (
                "size",
                ours.file_status
                    .has(FileStatus::SIZE_FROM_DAT)
                    .then_some(ours.size)
                    .flatten()
                    .map(|v| v.to_string()),
                dat_view(
                    cache,
                    FileStatus::SIZE_FROM_DAT,
                    FileStatus::ALT_SIZE_FROM_DAT,
                    &cache.size,
                    &cache.alt_size,
                )
                .map(|v| v.to_string()),
            ),
            (
                "crc",
                ours.file_status
                    .has(FileStatus::CRC_FROM_DAT)
                    .then(|| ours.crc.clone())
                    .flatten()
                    .map(hex),
                dat_view(
                    cache,
                    FileStatus::CRC_FROM_DAT,
                    FileStatus::ALT_CRC_FROM_DAT,
                    &cache.crc,
                    &cache.alt_crc,
                )
                .map(hex),
            ),
            (
                "sha1",
                ours.file_status
                    .has(FileStatus::SHA1_FROM_DAT)
                    .then(|| ours.sha1.clone())
                    .flatten()
                    .map(hex),
                dat_view(
                    cache,
                    FileStatus::SHA1_FROM_DAT,
                    FileStatus::ALT_SHA1_FROM_DAT,
                    &cache.sha1,
                    &cache.alt_sha1,
                )
                .map(hex),
            ),
            (
                "md5",
                ours.file_status
                    .has(FileStatus::MD5_FROM_DAT)
                    .then(|| ours.md5.clone())
                    .flatten()
                    .map(hex),
                dat_view(
                    cache,
                    FileStatus::MD5_FROM_DAT,
                    FileStatus::ALT_MD5_FROM_DAT,
                    &cache.md5,
                    &cache.alt_md5,
                )
                .map(hex),
            ),
        ];
        for (col, a, b) in views {
            if a != b {
                d.unexplained(col, format!("{p}: {a:?} vs {b:?}"));
            }
        }
        // The header type: the cache holds it from the DAT, or set by the scanner.
        if ours.header_file_type != cache.header_file_type {
            if ours.header_file_type.is_none()
                && cache.header_file_type.is_some()
                && !cache.file_status.has(FileStatus::HEADER_FILE_TYPE_FROM_DAT)
            {
                d.explained(
                    "header_file_type",
                    "set by the scan of a headered file, not by the DAT (FromHeader flag, no FromDAT flag)",
                    p.clone(),
                );
            } else {
                d.unexplained(
                    "header_file_type",
                    format!(
                        "{p}: {:?} vs {:?}",
                        ours.header_file_type, cache.header_file_type
                    ),
                );
            }
        }
        if ours.merge != cache.merge {
            d.unexplained(
                "merge",
                format!("{p}: {:?} vs {:?}", ours.merge, cache.merge),
            );
        }
        if ours.status != cache.status {
            d.unexplained(
                "status",
                format!("{p}: {:?} vs {:?}", ours.status, cache.status),
            );
        }
        if ours.mia_status != cache.mia_status {
            d.unexplained(
                "mia",
                format!("{p}: {:?} vs {:?}", ours.mia_status, cache.mia_status),
            );
        }
        if dat_flags(ours) != dat_flags(cache) {
            if cache.header_file_type.is_some()
                && !cache.file_status.has(FileStatus::HEADER_FILE_TYPE_FROM_DAT)
                && dat_flags(ours) == dat_flags(cache) | FileStatus::HEADER_FILE_TYPE_FROM_DAT.0
            {
                d.explained(
                    "dat_flags",
                    "HeaderFileTypeFromDAT: see header_file_type",
                    p.clone(),
                );
            } else {
                d.unexplained(
                    "dat_flags",
                    format!("{p}: {:#x} vs {:#x}", dat_flags(ours), dat_flags(cache)),
                );
            }
        }
        if (ours.dat_uid == 0) != (cache.dat_uid == 0) {
            d.unexplained("dat_link", p.clone());
        }
        return;
    }

    if ours.zip_dat_struct != cache.zip_dat_struct {
        d.unexplained(
            "zip_dat_struct",
            format!(
                "{p}: {:?} vs {:?}",
                ours.zip_dat_struct, cache.zip_dat_struct
            ),
        );
    }
    if ours.tree.is_some() != cache.tree.is_some() {
        d.unexplained("tree_row_presence", p.clone());
    }
    if ours.game.is_some() != cache.game.is_some() {
        d.unexplained("game_presence", p.clone());
    }
    if (ours.dat_uid == 0) != (cache.dat_uid == 0) {
        d.unexplained("dat_link", p.clone());
    }
    if let (Some(a), Some(b)) = (&ours.game, &cache.game) {
        let mut ids: Vec<u8> = a.entries.iter().chain(&b.entries).map(|e| e.id).collect();
        ids.sort_unstable();
        ids.dedup();
        for id in ids {
            let (x, y) = (a.get(id), b.get(id));
            if x == y {
                continue;
            }
            let col = format!("game_meta[{id}]");
            let e = format!("{p}: {x:?} vs {y:?}");
            if id == game_data::GAME_ID && y.is_none() {
                d.explained(
                    &col,
                    "GameId is a 3.8 field; the 3.7 cache does not store it",
                    e,
                );
            } else if id == game_data::DESCRIPTION
                && x == Some("\u{a4}")
                && y.is_some_and(|desc| ours.name.starts_with(desc))
            {
                d.explained(
                    &col,
                    "3.8 stores \u{a4} for a description that repeats the name; 3.7 stores the text",
                    e,
                );
            } else {
                d.unexplained(&col, e);
            }
        }
    }

    // Children, in order, by name and type.
    let (oc, cc) = (dat_children(ours), dat_children(cache));
    let (mut i, mut j) = (0, 0);
    while i < oc.len() || j < cc.len() {
        let res = if i < oc.len() && j < cc.len() {
            rv_core::sorters::compare_name(
                oc[i].file_type,
                &oc[i].name,
                cc[j].file_type,
                &cc[j].name,
            )
        } else if i < oc.len() {
            -1
        } else {
            1
        };
        match res {
            0 => {
                if oc[i].name != cc[j].name {
                    d.unexplained("name", format!("{p}: {:?} vs {:?}", oc[i].name, cc[j].name));
                }
                compare_node(oc[i], cc[j], &p, d);
                i += 1;
                j += 1;
            }
            r if r < 0 => {
                d.unexplained("only_in_ours", format!("{p}/{}", oc[i].name));
                i += 1;
            }
            _ => {
                d.unexplained("only_in_cache", format!("{p}/{}", cc[j].name));
                j += 1;
            }
        }
    }
}

fn hex(b: Vec<u8>) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// One DAT's `RvDat` against the cache's.
fn compare_dat_entries(ours: &rv_core::RvDat, cache: &rv_core::RvDat, d: &mut Diffs, p: &str) {
    if ours.time_stamp != cache.time_stamp {
        d.unexplained(
            "dat.time_stamp",
            format!("{p}: {} vs {}", ours.time_stamp, cache.time_stamp),
        );
    }
    if ours.dat_flags != cache.dat_flags {
        d.unexplained(
            "dat.flags",
            format!("{p}: {} vs {}", ours.dat_flags.0, cache.dat_flags.0),
        );
    }
    let mut ids: Vec<u8> = ours
        .entries
        .iter()
        .chain(&cache.entries)
        .map(|e| e.id)
        .collect();
    ids.sort_unstable();
    ids.dedup();
    for id in ids {
        let (x, y) = (ours.get(id), cache.get(id));
        let (nx, ny) = (
            x.map(|s| s.replace('\\', "/")),
            y.map(|s| s.replace('\\', "/")),
        );
        if nx == ny {
            if x != y {
                d.explained(
                    "dat.DatRootFullName",
                    "the cache was written on Windows with `\\`; this code base uses `/`",
                    format!("{p}: {x:?} vs {y:?}"),
                );
            }
            continue;
        }
        let col = format!("dat.meta[{id}]");
        if id == dat_data::SUB_DIR_TYPE && y.is_none() && x == Some("0") {
            d.explained(
                &col,
                "the SubDirType entry was added after this DAT was loaded",
                p.to_owned(),
            );
        } else {
            d.unexplained(&col, format!("{p}: {x:?} vs {y:?}"));
        }
    }
}

#[test]
#[ignore = "needs RV_FIXTURES_DIR"]
fn from_an_empty_database_the_dat_side_matches_the_real_cache_for_every_dat() {
    let work = work_dir("empty");
    copy_tree(&fixtures_dir().join("DatRoot"), &work.join("DatRoot"));
    let s = settings(&work);

    let mut db = Database::default_db();
    db.report_status_reset(&s);
    let t = std::time::Instant::now();
    let report = run_update(&mut db, &s);
    eprintln!("update from empty: {:?}, {report:?}", t.elapsed());
    assert_eq!(report.dats_added.len(), 14, "{report:?}");
    assert!(
        report.dats_failed.is_empty() && report.messages.is_empty(),
        "{report:?}"
    );

    let cache = database(&real_cache_bytes(), &s, true);
    let (mut ours_dirs, mut cache_dirs) = (Vec::new(), Vec::new());
    find_dat_dirs(&db.root, "", &mut ours_dirs);
    find_dat_dirs(&cache.root, "", &mut cache_dirs);
    assert_eq!(ours_dirs.len(), 14);
    assert_eq!(
        ours_dirs.iter().map(|(p, _)| p).collect::<Vec<_>>(),
        cache_dirs.iter().map(|(p, _)| p).collect::<Vec<_>>(),
        "the same 14 directories hold DATs"
    );

    let mut total = Diffs::default();
    eprintln!(
        "{:<34} {:>7} {:>7}  unexplained / explained columns",
        "DAT directory", "games", "nodes"
    );
    for ((path, o), (_, c)) in ours_dirs.iter().zip(&cache_dirs) {
        let mut d = Diffs::default();
        assert_eq!(o.dir_dats.len(), c.dir_dats.len(), "{path}: DAT list");
        for (a, b) in o.dir_dats.iter().zip(&c.dir_dats) {
            compare_dat_entries(a, b, &mut d, path);
        }
        // The directory with the DAT in it, compared as a node of its own name.
        let (mut o2, mut c2) = ((*o).clone(), (*c).clone());
        o2.name = "R".into();
        c2.name = "R".into();
        compare_node(&o2, &c2, "", &mut d);
        let games = o.children.len();
        eprintln!(
            "{:<34} {:>7} {:>7}  {} / {}",
            path.trim_start_matches('/'),
            games,
            d.compared_nodes,
            d.unexplained.values().map(|v| v.0).sum::<usize>(),
            d.explained.values().map(|v| v.0).sum::<usize>(),
        );
        for (k, v) in d.unexplained {
            total.unexplained.entry(k).or_insert((0, v.1.clone())).0 += v.0;
        }
        for (k, v) in d.explained {
            let e = total.explained.entry(k).or_insert((0, v.1, v.2.clone()));
            e.0 += v.0;
        }
        total.compared_nodes += d.compared_nodes;
    }
    eprintln!("\nexplained differences (column: count, reason, example):");
    for (k, (n, why, ex)) in &total.explained {
        eprintln!("  {k}: {n}  {why}\n      e.g. {ex}");
    }
    eprintln!("nodes compared: {}", total.compared_nodes);
    assert!(
        total.unexplained.is_empty(),
        "unexplained differences: {:#?}",
        total.unexplained
    );
    let _ = fs::remove_dir_all(&work);
}

// ---- 2. idempotent on the real cache ---------------------------------------------------------------

#[test]
#[ignore = "needs RV_FIXTURES_DIR"]
fn updating_the_real_cache_against_the_same_dats_writes_it_back_byte_for_byte() {
    let work = work_dir("idem");
    copy_tree(&fixtures_dir().join("DatRoot"), &work.join("DatRoot"));
    let s = settings(&work);
    let input = real_cache_bytes();

    // Without ROMVault's load-time repairs: nothing at all may change.
    let mut db = database(&input, &s, false);
    let t = std::time::Instant::now();
    let report = run_update(&mut db, &s);
    eprintln!("idempotent update: {:?}", t.elapsed());
    assert_eq!(report.dats_unchanged.len(), 14, "{report:?}");
    assert!(report.dats_added.is_empty() && report.dats_removed.is_empty());
    let out = to_cache_file(&mut db).unwrap().to_bytes().unwrap();
    assert_eq!(out.len(), input.len());
    assert!(out == input, "the written cache differs from the input");
    eprintln!("without load fix-ups: byte-identical, {} bytes", out.len());

    // With them (what ROMVault itself does on load): exactly the 26 nodes that still carry the
    // retired DatStatus 5 change, each by its MIA byte.
    let mut db = database(&input, &s, true);
    run_update(&mut db, &s);
    let fixed = to_cache_file(&mut db).unwrap().to_bytes().unwrap();
    let a = CacheFile::from_bytes(&input).unwrap().root;
    let b = CacheFile::from_bytes(&fixed).unwrap().root;
    let mut changed: Vec<String> = Vec::new();
    fn diff(a: &RvFile, b: &RvFile, path: &str, out: &mut Vec<String>) {
        let p = format!("{path}/{}", a.name);
        assert_eq!(a.children.len(), b.children.len(), "{p}");
        let mut x = a.clone();
        let mut y = b.clone();
        x.children.clear();
        y.children.clear();
        x.dat_uid = 0;
        y.dat_uid = 0;
        if x != y {
            out.push(format!(
                "{p}: dat_status {}->{}, mia {:?}->{:?}",
                a.dat_status.0, b.dat_status.0, a.mia_status, b.mia_status
            ));
            assert_eq!(a.dat_status, DatStatus::RETIRED_IN_DAT_MIA);
            assert_eq!(b.dat_status, DatStatus::IN_DAT_COLLECT);
            assert_eq!(b.mia_status, Some(rv_core::MiaStatus::MIA_FROM_DAT));
        }
        for (c, e) in a.children.iter().zip(&b.children) {
            diff(c, e, &p, out);
        }
    }
    diff(&a, &b, "", &mut changed);
    eprintln!(
        "with load fix-ups: {} nodes differ (+{} bytes)",
        changed.len(),
        fixed.len() - input.len()
    );
    for c in changed.iter().take(3) {
        eprintln!("  {c}");
    }
    assert_eq!(changed.len(), 26);
    assert_eq!(fixed.len() - input.len(), 26, "one MIA byte each");
    let _ = fs::remove_dir_all(&work);
}

// ---- 3. a changed DAT -----------------------------------------------------------------------------

/// Everything about a node that the DAT update decides, by path.
fn snapshot(n: &RvFile, path: &str, out: &mut BTreeMap<String, String>) {
    let p = if path.is_empty() {
        n.name.clone()
    } else {
        format!("{path}/{}", n.name)
    };
    let hash = |v: &Option<Vec<u8>>| v.as_ref().map(|b| hex(b.clone()));
    out.insert(
        format!("{p}\u{0}{}", n.file_type.0),
        format!(
            "ds={} gs={} rep={} size={:?} crc={:?} sha1={:?} merge={:?} status={:?} fs={:#x} dat={} tree={} game={} zds={:?}",
            n.dat_status.0,
            n.got_status.0,
            n.rep_status.name(),
            n.size,
            hash(&n.crc),
            hash(&n.sha1),
            n.merge,
            n.status,
            n.file_status.0,
            n.dat_uid != 0,
            n.tree.is_some(),
            n.game.is_some(),
            n.zip_dat_struct
        ),
    );
    for c in &n.children {
        snapshot(c, &p, out);
    }
}

#[test]
#[ignore = "needs RV_FIXTURES_DIR"]
fn a_changed_dat_and_a_new_dat_change_exactly_what_they_say() {
    let work = work_dir("changed");
    copy_tree(&fixtures_dir().join("DatRoot"), &work.join("DatRoot"));
    let s = settings(&work);
    let input = real_cache_bytes();
    let mut db = database(&input, &s, true);
    let mut before = BTreeMap::new();
    snapshot(&db.root, "", &mut before);

    // Remove one ROM from one DAT: a track of a Sega CD game that the cache has.
    let dat = work
        .join("DatRoot/Console/Genesis/Sega CD")
        .read_dir()
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let text = fs::read_to_string(&dat).unwrap();
    let victim_rom = "3 Ninjas Kick Back (USA) (Track 03).bin";
    let victim_game = "3 Ninjas Kick Back (USA)";
    let lines: Vec<&str> = text.lines().collect();
    let at = lines
        .iter()
        .position(|l| l.contains(&format!("name=\"{victim_rom}\"")))
        .unwrap_or_else(|| panic!("{victim_rom} is not in {}", dat.display()));
    let edited: String = lines
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != at)
        .map(|(_, l)| format!("{l}\n"))
        .collect();
    fs::write(&dat, edited).unwrap();
    // A new DAT in a new directory.
    let new_dat = work.join("DatRoot/Console/GB/Extra/extra.dat");
    fs::create_dir_all(new_dat.parent().unwrap()).unwrap();
    fs::write(
        &new_dat,
        "clrmamepro (\n\tname \"Extra\"\n)\ngame (\n\tname \"Control Game\"\n\trom ( name \"c1.gb\" size 4 crc 0badf00d )\n\trom ( name \"c2.gb\" size 8 crc 0badf00e )\n)\n",
    )
    .unwrap();
    // Make sure the edited file's time stamp differs from the cache's even on a coarse clock.
    let later = std::time::SystemTime::now() + Duration::from_secs(5);
    fs::OpenOptions::new()
        .write(true)
        .open(&dat)
        .unwrap()
        .set_modified(later)
        .unwrap();

    let report = run_update(&mut db, &s);
    assert_eq!(report.dats_found, 15);
    assert_eq!(report.dats_unchanged.len(), 13, "{report:?}");
    assert_eq!(report.dats_removed.len(), 1, "{report:?}");
    assert!(report.dats_removed[0].starts_with("DatRoot/Console/Genesis/Sega CD/"));
    let mut added = report.dats_added.clone();
    added.sort();
    assert_eq!(added.len(), 2, "{added:?}");
    assert_eq!(added[0], "DatRoot/Console/GB/Extra/extra.dat");
    assert_eq!(
        added[1], report.dats_removed[0],
        "the changed DAT is read again"
    );
    assert!(
        report.dats_failed.is_empty() && report.messages.is_empty(),
        "{report:?}"
    );
    eprintln!(
        "changed-DAT control: removed {:?}, added {:?}",
        report.dats_removed, report.dats_added
    );

    let mut after = BTreeMap::new();
    snapshot(&db.root, "", &mut after);
    let removed: Vec<_> = before.keys().filter(|k| !after.contains_key(*k)).collect();
    let added_nodes: Vec<_> = after.keys().filter(|k| !before.contains_key(*k)).collect();
    let changed: Vec<_> = before
        .iter()
        .filter(|(k, v)| after.get(*k).is_some_and(|w| w != *v))
        .collect();
    eprintln!(
        "tree diff: {} removed, {} added, {} changed",
        removed.len(),
        added_nodes.len(),
        changed.len()
    );
    for k in &removed {
        eprintln!("  - {}", k.replace('\0', " [type "));
    }
    for k in &added_nodes {
        eprintln!("  + {}", k.replace('\0', " [type "));
    }
    for (k, v) in &changed {
        eprintln!(
            "  ~ {}\n      was {v}\n      now {}",
            k.replace('\0', " [type "),
            after[*k]
        );
    }

    // The new DAT: a directory `Extra` with the game's zip and two ROMs, nothing else new.
    let new_paths: Vec<String> = added_nodes
        .iter()
        .map(|k| k.split('\0').next().unwrap().to_owned())
        .collect();
    assert_eq!(
        new_paths,
        [
            "/RomVault/Console/GB/Extra",
            "/RomVault/Console/GB/Extra/Control Game.zip",
            "/RomVault/Console/GB/Extra/Control Game.zip/c1.gb",
            "/RomVault/Console/GB/Extra/Control Game.zip/c2.gb",
        ]
        .map(|p| p.trim_start_matches('/').to_owned())
    );
    // The removed ROM is still in the game's zip, as an unknown file: it is on disk, no DAT has it.
    assert!(removed.is_empty(), "{removed:?}");
    let changed_paths: Vec<String> = changed
        .iter()
        .map(|(k, _)| k.split('\0').next().unwrap().to_owned())
        .collect();
    let victim_path = format!("RomVault/Console/Genesis/Sega CD/{victim_game}.zip/{victim_rom}");
    assert_eq!(
        changed_paths,
        std::slice::from_ref(&victim_path),
        "only the ROM that left the DAT changes"
    );
    let (_, was) = changed[0];
    let now = &after[changed[0].0];
    assert!(was.contains("ds=0") && was.contains("rep=Correct"), "{was}");
    assert!(
        now.contains("ds=3") && now.contains("gs=1") && now.contains("rep=Unknown"),
        "{now}"
    );
    let _ = fs::remove_dir_all(&work);
}
