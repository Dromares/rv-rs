//! Real-data checks against copies of the board's ROM library. These need files that are never in
//! the repository, so they are `#[ignore]`d. Run them with:
//!
//! ```sh
//! RV_FIXTURES_DIR=/path/to/fixtures cargo test --release -p rv-archive --test fixtures \
//!     -- --ignored --nocapture
//! ```
//!
//! `RV_FIXTURES_DIR` must hold `RomRoot-sample/` (copies of TorrentZipped zips, with a
//! `MANIFEST.sha256`). With the variable unset, or a listed file missing, these tests fail rather
//! than pass, so an ignored run never looks green by accident.

use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use rv_archive::zip::TorrentZipWriter;
use rv_archive::zip::{ZipArchive, ZipStructure};
use sha2::{Digest, Sha256};

fn sample_dir() -> PathBuf {
    let dir = match std::env::var_os("RV_FIXTURES_DIR") {
        Some(d) => PathBuf::from(d).join("RomRoot-sample"),
        None => panic!("RV_FIXTURES_DIR is not set; these tests need the RomRoot sample"),
    };
    assert!(dir.is_dir(), "{} is not a directory", dir.display());
    dir
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}

/// Every file under the sample with this extension; fails if the manifest lists a file that is
/// not there or the sample has none.
fn sample_files(ext: &str) -> Vec<PathBuf> {
    let root = sample_dir();
    let manifest = fs::read_to_string(root.join("MANIFEST.sha256"))
        .unwrap_or_else(|e| panic!("cannot read MANIFEST.sha256: {e}"));
    for line in manifest.lines() {
        let rel = line.split_once("  ").expect("manifest line").1;
        assert!(
            root.join(rel).is_file(),
            "listed in MANIFEST.sha256 but missing: {rel}"
        );
    }
    let mut all = Vec::new();
    walk(&root, &mut all);
    let found: Vec<PathBuf> = all
        .into_iter()
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext)))
        .collect();
    assert!(
        !found.is_empty(),
        "no .{ext} files under {}",
        root.display()
    );
    found
}

fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn first_diff(a: &[u8], b: &[u8]) -> Option<usize> {
    a.iter()
        .zip(b)
        .position(|(x, y)| x != y)
        .or_else(|| (a.len() != b.len()).then_some(a.len().min(b.len())))
}

/// Decompress every member of `orig` and rebuild the zip with the TorrentZip writer.
fn rewrite(orig: &[u8], label: &str) -> Vec<u8> {
    let mut zip = ZipArchive::open(Cursor::new(orig)).unwrap_or_else(|e| panic!("{label}: {e}"));
    assert_eq!(
        zip.header_structure(),
        ZipStructure::ZipTrrnt,
        "{label}: comment"
    );
    assert_eq!(
        zip.structure(),
        ZipStructure::ZipTrrnt,
        "{label}: file rules"
    );
    let crc = zip.central_directory_crc().unwrap();
    assert_eq!(
        zip.comment(),
        format!("TORRENTZIPPED-{crc:08X}"),
        "{label}: comment CRC"
    );
    let mut w = TorrentZipWriter::new(Cursor::new(Vec::new()), ZipStructure::ZipTrrnt).unwrap();
    for i in 0..zip.len() {
        let (name, size, want_crc) = {
            let e = &zip.entries()[i];
            (e.name.clone(), e.uncompressed_size, e.crc32)
        };
        let mut r = zip
            .open_entry(i)
            .unwrap_or_else(|e| panic!("{label}: {name}: {e}"));
        let added = w
            .add_file(&name, size, &mut r, None, Some(want_crc))
            .unwrap_or_else(|e| panic!("{label}: {name}: {e}"));
        assert_eq!(added.crc32, want_crc);
    }
    w.finish().unwrap().into_inner()
}

#[test]
#[ignore = "needs RV_FIXTURES_DIR"]
fn real_torrentzips_are_rewritten_byte_identical() {
    let root = sample_dir();
    let files = sample_files("zip");
    let mut rows = Vec::new();
    let mut bad = Vec::new();
    for path in &files {
        let rel = path.strip_prefix(&root).unwrap().display().to_string();
        let orig = fs::read(path).unwrap();
        let members = ZipArchive::open(Cursor::new(&orig[..])).unwrap().len();
        let out = rewrite(&orig, &rel);
        let (a, b) = (sha256_hex(&orig), sha256_hex(&out));
        let same = out == orig;
        rows.push(format!(
            "| {rel} | {members} | {} | {} | {} |",
            orig.len(),
            &a[..16],
            if same {
                "identical".to_string()
            } else {
                format!("DIFFERS at {:?}", first_diff(&orig, &out))
            }
        ));
        if !same || a != b {
            bad.push(rel);
        }
    }
    eprintln!("| file | members | bytes | sha256 (first 16) | rewritten |");
    eprintln!("|---|---|---|---|---|");
    for r in &rows {
        eprintln!("{r}");
    }
    eprintln!("{} zips, {} not byte-identical", rows.len(), bad.len());
    assert!(bad.is_empty(), "not byte-identical: {bad:?}");
}
