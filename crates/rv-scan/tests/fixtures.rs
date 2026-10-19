//! Real-data tests: loose files from the board's `RomRoot/` and NES ROMs from its NES set.
//! They need files that are never in the repository, so they are `#[ignore]`d. Run them with
//!
//! ```sh
//! RV_FIXTURES_DIR=/path/to/copies cargo test -p rv-scan --test fixtures -- --ignored --nocapture
//! ```
//!
//! The directory must hold `scan-MANIFEST.tsv`, `RomRoot-loose/`, `nes/`, `RomVault3_3.Cache`
//! and `DatRoot/`. The manifest's hashes come from `md5sum`, `sha1sum`, `sha256sum` and
//! `cksum -a crc32b`, never from this crate. With the variable unset, or a file missing,
//! these tests fail rather than pass.

use std::fs::File;
use std::path::PathBuf;

use rv_cache::CacheFile;
use rv_core::{HeaderFileType, RvFile};
use rv_scan::compare::{compare_alt_hash, compare_with_alt, Known};
use rv_scan::hash::to_hex;
use rv_scan::{scan, ScanOptions};

fn fixtures_dir() -> PathBuf {
    match std::env::var_os("RV_FIXTURES_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => panic!("RV_FIXTURES_DIR is not set; these tests need the board's files"),
    }
}

struct Row {
    kind: String,
    rel: String,
    size: u64,
    crc: String,
    md5: String,
    sha1: String,
    sha256: String,
}

fn manifest() -> Vec<Row> {
    let path = fixtures_dir().join("scan-MANIFEST.tsv");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    text.lines()
        .skip(1)
        .map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            assert_eq!(f.len(), 7, "{l}");
            Row {
                kind: f[0].into(),
                rel: f[1].into(),
                size: f[2].parse().unwrap(),
                crc: f[3].into(),
                md5: f[4].into(),
                sha1: f[5].into(),
                sha256: f[6].into(),
            }
        })
        .collect()
}

fn file_of(row: &Row) -> PathBuf {
    let dir = if row.kind == "nes" {
        "nes"
    } else {
        "RomRoot-loose"
    };
    fixtures_dir().join(dir).join(&row.rel)
}

fn scan_file(row: &Row) -> rv_scan::ScanResult {
    let p = file_of(row);
    let mut f = File::open(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    scan(&mut f, ScanOptions::default()).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn hex<const N: usize>(d: Option<[u8; N]>) -> String {
    to_hex(&d.expect("hash present"))
}

fn cache() -> CacheFile {
    let p = fixtures_dir().join("RomVault3_3.Cache");
    CacheFile::from_bytes(&std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display())))
        .expect("cache parses")
}

fn find<'a>(root: &'a RvFile, path: &str) -> Option<&'a RvFile> {
    let mut n = root;
    for part in path.split('/') {
        n = n.children.iter().find(|c| c.name == part)?;
    }
    Some(n)
}

fn h(b: &Option<Vec<u8>>) -> String {
    b.as_ref().map(|b| to_hex(b)).unwrap_or_default()
}

#[test]
#[ignore = "needs RV_FIXTURES_DIR"]
fn loose_files_match_the_system_tools() {
    let rows: Vec<Row> = manifest()
        .into_iter()
        .filter(|r| r.kind == "loose")
        .collect();
    assert!(rows.len() >= 20, "{} loose files", rows.len());
    let total: u64 = rows.iter().map(|r| r.size).sum();
    assert!(total <= 2_000_000_000, "{total} bytes");
    println!("{} loose files, {total} bytes", rows.len());
    println!("| file | size | crc32 | md5 | sha1 | sha256 |");
    println!("|---|---|---|---|---|---|");
    for r in &rows {
        let s = scan_file(r);
        assert_eq!(s.size, r.size, "{}", r.rel);
        assert_eq!(hex(s.digests.crc), r.crc, "{}: crc", r.rel);
        assert_eq!(hex(s.digests.md5), r.md5, "{}: md5", r.rel);
        assert_eq!(hex(s.digests.sha1), r.sha1, "{}: sha1", r.rel);
        assert_eq!(hex(s.digests.sha256), r.sha256, "{}: sha256", r.rel);
        let short: String = r.rel.rsplit('/').next().unwrap().chars().take(40).collect();
        println!("| {short} | {} | ok | ok | ok | ok |", r.size);
    }
}

#[test]
#[ignore = "needs RV_FIXTURES_DIR"]
fn loose_files_match_the_hashes_the_cache_holds() {
    let cache = cache();
    let mut compared = 0;
    for r in manifest().iter().filter(|r| r.kind == "loose") {
        let Some(node) = find(&cache.root, &format!("RomVault/{}", r.rel)) else {
            println!("not in the cache (never scanned by ROMVault): {}", r.rel);
            continue;
        };
        let s = scan_file(r);
        assert_eq!(node.size, Some(s.size), "{}: size", r.rel);
        assert_eq!(h(&node.crc), hex(s.digests.crc), "{}: crc", r.rel);
        assert_eq!(h(&node.sha1), hex(s.digests.sha1), "{}: sha1", r.rel);
        assert_eq!(h(&node.md5), hex(s.digests.md5), "{}: md5", r.rel);
        // The cache's header type is what the detector says: CHD for .chd, none for .rvz.
        let cache_type = node.header_file_type.map_or(HeaderFileType::NOTHING, |t| {
            HeaderFileType(t.0 & HeaderFileType::HEADER_MASK.0)
        });
        assert_eq!(cache_type, s.header_type, "{}: header type", r.rel);
        compared += 1;
        println!(
            "cache == scan: {} ({} bytes, header type {})",
            r.rel, s.size, s.header_type.0
        );
    }
    // Every loose file in the cache is >= 156 MB, so only these 7 fit the 2 GB budget.
    assert_eq!(compared, 7);
}

#[test]
#[ignore = "needs RV_FIXTURES_DIR"]
fn nes_files_header_stripped_hashes_match_the_dat_and_the_cache() {
    let cache = cache();
    // The board's NES (Headered) DAT.
    let datroot = fixtures_dir().join("DatRoot/Console/NES/Roms");
    let dat_path = std::fs::read_dir(&datroot)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.to_string_lossy().contains("(Headered)"))
        .expect("NES (Headered) DAT");
    let dat = rv_dat::read_dat_file(&dat_path).header.expect("dat");
    // This DAT carries no `<clrmamepro header=...>`: it is the "Headered" No-Intro set, whose
    // hashes cover the whole file, 16-byte iNES header included.
    assert_eq!(dat.header, "");

    let rows: Vec<Row> = manifest().into_iter().filter(|r| r.kind == "nes").collect();
    assert!(rows.len() >= 5, "{} NES files", rows.len());
    println!("| rom | size | alt size | whole-file SHA1 = DAT | header-stripped SHA1 = cache alt | alt MD5/CRC = cache |");
    println!("|---|---|---|---|---|---|");
    for r in &rows {
        let s = scan_file(r);
        let game = r.rel.trim_end_matches(".nes");
        // 1. what the scanner makes of the file
        assert_eq!(s.header_type, HeaderFileType::NES, "{}", r.rel);
        assert_eq!(
            hex(s.digests.sha1),
            r.sha1,
            "{}: whole-file sha1 vs sha1sum",
            r.rel
        );
        let alt = s.alt.as_ref().expect("alt hashes");
        assert_eq!(alt.size, s.size - 16, "{}: header is 16 bytes", r.rel);

        // 2. the headered DAT lists the whole-file hashes
        let dat_rom = find_dat_rom(&dat.base_dir, game, &r.rel)
            .unwrap_or_else(|| panic!("{} not in DAT", r.rel));
        assert_eq!(h(&dat_rom.sha1), hex(s.digests.sha1), "{}: DAT sha1", r.rel);
        assert_eq!(dat_rom.size, Some(s.size), "{}: DAT size", r.rel);

        // 3. the cache holds the header-stripped hashes ROMVault 3.7 computed
        let node = find(
            &cache.root,
            &format!("RomVault/Console/NES/Roms/{game}.zip/{}", r.rel),
        )
        .unwrap_or_else(|| panic!("{} not in the cache", r.rel));
        assert_eq!(node.alt_size, Some(alt.size), "{}: alt size", r.rel);
        assert_eq!(
            h(&node.alt_sha1),
            hex(alt.digests.sha1),
            "{}: alt sha1",
            r.rel
        );
        assert_eq!(h(&node.alt_md5), hex(alt.digests.md5), "{}: alt md5", r.rel);
        assert_eq!(h(&node.alt_crc), hex(alt.digests.crc), "{}: alt crc", r.rel);
        assert_eq!(h(&node.sha1), hex(s.digests.sha1), "{}: cache sha1", r.rel);

        // 4. a headerless-DAT style entry built from the cache's alt hashes matches by
        //    `compare_with_alt` as an alternate match, by SHA1, by CRC and (the 3.8.0
        //    defect) by MD5 alone.
        let headerless = |sha1: bool, crc: bool, md5: bool| Known {
            size: Some(alt.size),
            sha1: sha1.then(|| node.alt_sha1.clone().unwrap()),
            crc: crc.then(|| node.alt_crc.clone().unwrap()),
            md5: md5.then(|| node.alt_md5.clone().unwrap()),
            header_type: HeaderFileType::NES,
        };
        assert_eq!(
            compare_with_alt(&headerless(true, true, true), &s),
            Some(true),
            "{}",
            r.rel
        );
        assert_eq!(
            compare_with_alt(&headerless(true, false, false), &s),
            Some(true)
        );
        assert_eq!(
            compare_with_alt(&headerless(false, true, false), &s),
            Some(true)
        );
        assert!(
            compare_alt_hash(&headerless(false, false, true), &s),
            "{}: md5 only",
            r.rel
        );
        println!(
            "| {} | {} | {} | ok | ok | ok |",
            r.rel.chars().take(40).collect::<String>(),
            s.size,
            alt.size
        );
    }
}

fn find_dat_rom<'a>(d: &'a rv_dat::DatDir, game: &str, rom: &str) -> Option<&'a rv_dat::DatFile> {
    d.children.iter().find_map(|c| match c {
        rv_dat::DatNode::Dir(x) if x.base.name == game => x.children.iter().find_map(|f| match f {
            rv_dat::DatNode::File(f) if f.base.name == rom => Some(f),
            _ => None,
        }),
        _ => None,
    })
}
