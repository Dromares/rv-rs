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

// ---- CHD -------------------------------------------------------------------------------------

use std::io::{Read, Seek, SeekFrom};
use std::time::Instant;

use rv_archive::chd::{codec_name, Chd, CompType};
use rv_archive::{ArchiveError, ChdError};

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// A reader that flips one bit of the byte at `at`, so a copy of a 110 MB file is not needed.
struct Flip<R> {
    inner: R,
    at: u64,
}

impl<R: Read + Seek> Read for Flip<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let start = self.inner.stream_position()?;
        let n = self.inner.read(buf)?;
        if (start..start + n as u64).contains(&self.at) {
            buf[(self.at - start) as usize] ^= 0x01;
        }
        Ok(n)
    }
}

impl<R: Seek> Seek for Flip<R> {
    fn seek(&mut self, p: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(p)
    }
}

#[test]
#[ignore = "needs RV_FIXTURES_DIR"]
fn real_chds_verify_against_their_headers() {
    let root = sample_dir();
    let files = sample_files("chd");
    eprintln!("| file | version | hunks by type | hunk bytes | hunks | bytes | raw SHA-1 (= header) | combined SHA-1 (= header) | seconds |");
    eprintln!("|---|---|---|---|---|---|---|---|---|");
    for path in &files {
        let rel = path.strip_prefix(&root).unwrap().display().to_string();
        let mut chd =
            Chd::open(fs::File::open(path).unwrap()).unwrap_or_else(|e| panic!("{rel}: {e}"));
        let h = chd.header().clone();
        assert_eq!(h.version, 5, "{rel}");
        assert!(!h.requires_parent(), "{rel}");
        assert!(
            chd.unsupported_codec().is_none(),
            "{rel}: {:?}",
            chd.unsupported_codec()
        );
        // How many hunks use each codec slot, or are copies of another hunk.
        let mut by_type: Vec<String> = Vec::new();
        for (slot, &c) in h.compressors.iter().enumerate().filter(|(_, &c)| c != 0) {
            let n = h
                .map
                .iter()
                .filter(|m| m.comp == CompType::Codec(slot as u8))
                .count();
            by_type.push(format!("{} {n}", codec_name(c)));
        }
        let selfs = h.map.iter().filter(|m| m.comp == CompType::SelfRef).count();
        by_type.push(format!("self {selfs}"));
        let codecs = by_type;
        let t = Instant::now();
        let r = chd.verify().unwrap_or_else(|e| panic!("{rel}: {e}"));
        let secs = t.elapsed().as_secs_f32();
        assert_eq!(Some(r.raw_sha1), h.raw_sha1, "{rel}: raw SHA-1");
        assert_eq!(r.combined_sha1, h.sha1, "{rel}: combined SHA-1");
        assert_eq!(r.bytes, h.total_bytes);
        eprintln!(
            "| {rel} | {} | {} | {} | {} | {} | `{}` | `{}` | {secs:.1} |",
            h.version,
            codecs.join(" "),
            h.hunk_bytes,
            h.total_hunks,
            h.total_bytes,
            hex(&r.raw_sha1),
            hex(&r.combined_sha1.unwrap()),
        );
    }
}

#[test]
#[ignore = "needs RV_FIXTURES_DIR"]
fn a_flipped_hunk_byte_fails_verification_on_a_real_chd() {
    let mut flipped_codecs = std::collections::BTreeSet::new();
    for path in &sample_files("chd") {
        let chd = Chd::open(fs::File::open(path).unwrap()).unwrap();
        // One flipped bit in the middle of a stored hunk of each codec slot in use.
        for slot in 0..4u8 {
            let Some(hunk) = chd
                .header()
                .map
                .iter()
                .filter(|m| m.comp == CompType::Codec(slot) && m.length > 64)
                .nth(3)
                .cloned()
            else {
                continue;
            };
            let at = hunk.offset + hunk.length as u64 / 2;
            let mut bad = Chd::open(Flip {
                inner: fs::File::open(path).unwrap(),
                at,
            })
            .unwrap();
            let err = bad.verify().expect_err("a flipped byte must not verify");
            let name = codec_name(chd.header().compressors[slot as usize]);
            eprintln!(
                "{}: {name} hunk at {}: flipped the bit at byte {at}: {err}",
                path.display(),
                hunk.offset
            );
            assert!(
                matches!(
                    err,
                    ArchiveError::Chd(ChdError::Decompression(_))
                        | ArchiveError::Chd(ChdError::InvalidData(_))
                ),
                "{err:?}"
            );
            flipped_codecs.insert(name);
        }
    }
    eprintln!("flipped bits in hunks of: {flipped_codecs:?}");
    assert!(
        ["cdlz", "cdzl", "cdfl"]
            .iter()
            .all(|c| flipped_codecs.contains(*c)),
        "{flipped_codecs:?}"
    );
    let path = &sample_files("chd")[0];
    // And the untouched file still verifies.
    Chd::open(fs::File::open(path).unwrap())
        .unwrap()
        .verify()
        .unwrap();
}

// ---- 7z --------------------------------------------------------------------------------------

use rv_archive::sevenzip::SevenZipArchive;

#[test]
#[ignore = "needs RV_FIXTURES_DIR"]
fn real_7z_lists_and_extracts_with_matching_crcs() {
    let root = sample_dir();
    for path in sample_files("7z") {
        let rel = path.strip_prefix(&root).unwrap().display().to_string();
        let mut z = SevenZipArchive::open(fs::File::open(&path).unwrap())
            .unwrap_or_else(|e| panic!("{rel}: {e}"));
        eprintln!("| file | entry | size | CRC32 (from the 7z header) | coders |");
        eprintln!("|---|---|---|---|---|");
        let coders = z
            .folder_info(0)
            .map(|f| f.coders.join("+"))
            .unwrap_or_default();
        for e in z.entries() {
            let crc = e.crc32.map_or("-".to_string(), |c| format!("{c:08X}"));
            eprintln!("| {rel} | {} | {} | {crc} | {coders} |", e.name, e.size);
            assert!(
                e.crc32.is_some() || e.is_directory || e.size == 0,
                "{rel}: {} has no CRC",
                e.name
            );
        }
        let t = Instant::now();
        // Extracting checks each member's CRC32 against the header (a mismatch is an error).
        let bytes = z.verify_all().unwrap_or_else(|e| panic!("{rel}: {e}"));
        let total: u64 = z.entries().iter().map(|e| e.size).sum();
        assert_eq!(bytes, total);
        eprintln!(
            "{rel}: {bytes} bytes extracted and CRC-checked in {:.1} s",
            t.elapsed().as_secs_f32()
        );
    }
}
