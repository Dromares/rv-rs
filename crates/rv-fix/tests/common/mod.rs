//! A small library in a temp directory: DATs, files, a cache, and the steps of ROMVault run on it
//! with the real crates (DAT update, scan, Find Fixes, Fix).
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use rv_archive::zip::{TorrentZipWriter, ZipStructure};
use rv_cache::{load_database, save_database, DatabaseOptions};
use rv_core::progress::{CancelFlag, NoProgress};
use rv_core::{Database, DirMapping, FileType, GotStatus, RepStatus, RvFile, ScanLevel, Settings};
use rv_fix::{find_fixes, perform_fixes, FindFixes, FixOptions, FixReport};
use rv_scan::db::{scan_files, ScanRequest, ScanTarget};
use rv_scan::{HashKinds, HashSet};

pub const ROMS: &str = "RomVault/Console/NES/Roms";

pub struct Lib {
    pub dir: PathBuf,
    pub settings: Settings,
    pub db: Database,
    pub cache: PathBuf,
}

impl Lib {
    pub fn new(tag: &str) -> Lib {
        let dir = std::env::temp_dir().join(format!("rv-fix-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut settings = Settings::default();
        settings.dat_root = dir.join("DatRoot").to_string_lossy().into_owned();
        settings.dir_mappings[0].dir_path = dir.join("RomRoot").to_string_lossy().into_owned();
        settings.dir_mappings.push(DirMapping {
            dir_key: "ToSort".to_owned(),
            dir_path: dir.join("ToSort").to_string_lossy().into_owned(),
        });
        let cache = dir.join("test.Cache");
        Lib {
            dir,
            settings,
            db: Database::default_db(),
            cache,
        }
    }

    /// The library that is already in `dir` (a child process of a crash test opens the one its
    /// parent built).
    pub fn at(dir: &Path) -> Lib {
        let mut l = Lib::new("unused");
        let _ = fs::remove_dir_all(&l.dir);
        l.dir = dir.to_path_buf();
        l.settings.dat_root = dir.join("DatRoot").to_string_lossy().into_owned();
        l.settings.dir_mappings[0].dir_path = dir.join("RomRoot").to_string_lossy().into_owned();
        l.settings.dir_mappings[1].dir_path = dir.join("ToSort").to_string_lossy().into_owned();
        l.cache = dir.join("test.Cache");
        let (db, _) = load_database(&l.cache, &l.settings, DatabaseOptions::default()).unwrap();
        l.db = db;
        l
    }

    pub fn save(&mut self) {
        save_database(&self.cache, &mut self.db).unwrap();
    }

    pub fn path(&self, rel: &str) -> PathBuf {
        self.dir.join(rel)
    }

    pub fn write(&self, rel: &str, bytes: &[u8]) -> PathBuf {
        let p = self.path(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, bytes).unwrap();
        p
    }

    pub fn zip(&self, rel: &str, entries: &[(&str, &[u8])]) -> PathBuf {
        self.write(rel, &zip_bytes(entries))
    }

    pub fn read(&self, rel: &str) -> Vec<u8> {
        fs::read(self.path(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
    }

    pub fn exists(&self, rel: &str) -> bool {
        self.path(rel).exists()
    }

    /// A DAT with games `(name, [(rom, data)])`, written as `DatRoot/Console/NES/Roms/<file>`.
    pub fn dat(&self, file: &str, games: &[Game<'_>]) {
        self.write(
            &format!("DatRoot/Console/NES/Roms/{file}"),
            dat_text(games).as_bytes(),
        );
    }

    /// Start a database from the DATs.
    pub fn update_dats(&mut self) {
        self.db = Database::default_db();
        self.db.report_status_reset(&self.settings);
        rv_dat::update::update_dats(
            &mut self.db,
            &self.settings,
            &Default::default(),
            &NoProgress,
            &CancelFlag::new(),
        )
        .unwrap();
    }

    pub fn scan(&mut self, level: ScanLevel) {
        let r = scan_files(
            &mut self.db,
            &self.settings,
            &ScanRequest {
                level,
                target: ScanTarget::Selected,
            },
            &NoProgress,
            &CancelFlag::new(),
        )
        .unwrap();
        assert!(!r.aborted, "{r:?}");
    }

    pub fn find(&mut self) -> FindFixes {
        find_fixes(
            &mut self.db,
            &self.settings,
            &NoProgress,
            &CancelFlag::new(),
        )
    }

    pub fn fix_with(&mut self, ff: FindFixes, opts: &FixOptions) -> FixReport {
        let cache = self.cache.clone();
        let mut saver = |db: &mut Database| save_database(&cache, db).map_err(|e| e.to_string());
        perform_fixes(
            &mut self.db,
            &self.settings,
            ff.groups,
            opts,
            &NoProgress,
            &CancelFlag::new(),
            &mut saver,
        )
    }

    pub fn fix(&mut self) -> FixReport {
        let ff = self.find();
        self.fix_with(ff, &FixOptions::default())
    }

    pub fn node(&self, path: &str) -> Option<&RvFile> {
        let mut cur = &self.db.root;
        for part in path.split('/') {
            cur = cur.children.iter().find(|c| c.name == part)?;
        }
        Some(cur)
    }

    pub fn status(&self, path: &str) -> RepStatus {
        self.node(path)
            .unwrap_or_else(|| panic!("{path} is not in the tree"))
            .rep_status
    }

    pub fn got(&self, path: &str) -> GotStatus {
        self.node(path).unwrap().got_status
    }

    /// `(path, status)` of every file below `path`, for readable asserts.
    pub fn statuses(&self, path: &str) -> Vec<(String, RepStatus)> {
        let mut out = Vec::new();
        fn walk(n: &RvFile, prefix: &str, out: &mut Vec<(String, RepStatus)>) {
            for c in &n.children {
                let p = format!("{prefix}/{}", c.name);
                if c.is_file() {
                    out.push((p, c.rep_status));
                } else {
                    walk(c, &p, out);
                }
            }
        }
        walk(self.node(path).unwrap(), path, &mut out);
        out
    }

    /// The saved cache, loaded again, with a full scan: what a fresh start would know.
    pub fn rescanned(&self) -> Database {
        let (mut db, _) =
            load_database(&self.cache, &self.settings, DatabaseOptions::default()).unwrap();
        scan_files(
            &mut db,
            &self.settings,
            &ScanRequest {
                level: ScanLevel::Level3,
                target: ScanTarget::Selected,
            },
            &NoProgress,
            &CancelFlag::new(),
        )
        .unwrap();
        db
    }

    /// Check that the tree in memory says the same as a scan from scratch of what is on disk.
    pub fn assert_rescan_agrees(&mut self) {
        save_database(&self.cache, &mut self.db).unwrap();
        let fresh = self.rescanned();
        let a = flatten(&self.db.root);
        let b = flatten(&fresh.root);
        let only_a: Vec<_> = a.iter().filter(|(k, v)| b.get(*k) != Some(v)).collect();
        let only_b: Vec<_> = b.iter().filter(|(k, v)| a.get(*k) != Some(v)).collect();
        assert!(
            only_a.is_empty() && only_b.is_empty(),
            "the tree and a fresh scan disagree.\nin the tree only: {only_a:#?}\nin the scan only: {only_b:#?}"
        );
    }

    /// SHA-256 of the path and bytes of every file below `rel`, sorted: a manifest.
    pub fn manifest(&self, rel: &str) -> BTreeMap<String, String> {
        manifest(&self.path(rel))
    }
}

pub type Rom<'a> = (&'a str, &'a [u8]);
pub type Game<'a> = (&'a str, &'a [Rom<'a>]);

pub fn zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut w = TorrentZipWriter::new(Cursor::new(Vec::new()), ZipStructure::ZipTrrnt).unwrap();
    let mut entries = entries.to_vec();
    entries.sort_by(|a, b| rv_archive::sort::trrntzip_compare_case(a.0, b.0));
    for (name, data) in entries {
        w.add_bytes(name, data, None).unwrap();
    }
    w.finish().unwrap().into_inner()
}

pub fn hex(bytes: &[u8]) -> String {
    rv_scan::hash::to_hex(bytes)
}

pub fn sha256(data: &[u8]) -> String {
    let mut h = HashSet::new(HashKinds::ALL);
    h.update(data);
    hex(&h.finish().sha256.unwrap())
}

/// A ClrMamePro DAT with CRC, size and SHA1 for every rom.
pub fn dat_text(games: &[Game<'_>]) -> String {
    let mut s = "clrmamepro (\n\tname \"T\"\n)\n".to_owned();
    for (name, roms) in games {
        s.push_str(&format!("game (\n\tname \"{name}\"\n"));
        for (r, data) in *roms {
            let mut h = HashSet::new(HashKinds::ROMVAULT);
            h.update(data);
            let d = h.finish();
            s.push_str(&format!(
                "\trom ( name \"{r}\" size {} crc {} sha1 {} )\n",
                data.len(),
                hex(&d.crc.unwrap()),
                hex(&d.sha1.unwrap()),
            ));
        }
        s.push_str(")\n");
    }
    s
}

pub fn manifest(root: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, String>) {
        let Ok(rd) = fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(root, &p, out);
            } else {
                let rel = p.strip_prefix(root).unwrap().to_string_lossy().into_owned();
                out.insert(rel, sha256(&fs::read(&p).unwrap()));
            }
        }
    }
    walk(root, root, &mut out);
    out
}

/// Every file of the tree with what a scan decides: where it is and what it is.
pub fn flatten(root: &RvFile) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    fn walk(n: &RvFile, prefix: &str, out: &mut BTreeMap<String, String>) {
        for c in &n.children {
            let p = if prefix.is_empty() {
                c.name.clone()
            } else {
                format!("{prefix}/{}", c.name)
            };
            let desc = format!(
                "{:?} dat={:?} got={:?} size={:?} crc={:?} sha1={:?} {}",
                c.file_type,
                c.dat_status,
                c.got_status,
                c.size,
                c.crc.as_ref().map(|b| hex(b)),
                c.sha1.as_ref().map(|b| hex(b)),
                match c.file_type {
                    FileType::ZIP => format!("struct={:?}", c.zip_struct),
                    _ => String::new(),
                }
            );
            out.insert(p.clone(), desc);
            walk(c, &p, out);
        }
    }
    walk(root, "", &mut out);
    out
}
