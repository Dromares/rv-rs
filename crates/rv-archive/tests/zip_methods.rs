//! Zip reading against small synthetic zips made by independent encoders (`tests/data/gen_zips.py`:
//! Python's `zipfile`, p7zip and the zstd CLI), and the error paths: unsupported methods, bad
//! CRCs, truncation and corruption.

use std::io::{Cursor, Read};

use rv_archive::zip::{ZipArchive, ZipStructure};
use rv_archive::ArchiveError;

fn sample_text() -> Vec<u8> {
    (0..200)
        .map(|i| format!("line {i}: the quick brown fox jumps over the lazy dog\n"))
        .collect::<String>()
        .into_bytes()
}

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR")))
        .unwrap_or_else(|e| panic!("cannot read fixture {name}: {e}"))
}

fn open(name: &str) -> ZipArchive<Cursor<Vec<u8>>> {
    ZipArchive::open(Cursor::new(fixture(name))).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn check_method(file: &str, method: u16) {
    let mut zip = open(file);
    assert_eq!(zip.len(), 1);
    let e = zip.entries()[0].clone();
    assert_eq!(e.name, "sample.txt");
    assert_eq!(e.method, method, "{file}: method");
    assert_eq!(e.uncompressed_size, sample_text().len() as u64);
    assert_eq!(zip.read_entry(0).unwrap(), sample_text(), "{file}: data");
    assert_eq!(zip.verify_entry(0).unwrap(), sample_text().len() as u64);
}

#[test]
fn stored() {
    check_method("method_stored.zip", 0);
}

#[test]
fn deflate() {
    check_method("method_deflate.zip", 8);
}

#[test]
fn deflate64() {
    check_method("method_deflate64.zip", 9);
}

#[test]
fn bzip2() {
    check_method("method_bzip2.zip", 12);
}

#[test]
fn lzma() {
    check_method("method_lzma.zip", 14);
}

#[test]
fn zstandard() {
    check_method("method_zstd.zip", 93);
}

#[test]
fn ppmd() {
    check_method("method_ppmd.zip", 98);
}

#[test]
fn zip64_local_extra_field_is_read() {
    // Python writes the zip64 extra only in the local header of a small file; the central
    // directory has no zip64 field (`src/zip/tests.rs` covers the central one).
    let mut zip = open("zip64_forced.zip");
    assert!(
        !zip.entries()[0].extra_data_found,
        "zip64 alone is not extra data"
    );
    assert_eq!(zip.read_entry(0).unwrap(), sample_text());
}

#[test]
fn data_descriptor_entries_use_the_central_values() {
    let mut zip = open("data_descriptor.zip");
    let e = zip.entries()[0].clone();
    assert_eq!(e.flags & 8, 8);
    assert_eq!(e.local_head(), None, "no fast open with a data descriptor");
    assert_eq!(e.uncompressed_size, sample_text().len() as u64);
    assert_eq!(zip.read_entry(0).unwrap(), sample_text());
}

#[test]
fn utf8_flagged_names() {
    let zip = open("names.zip");
    let names: Vec<_> = zip.entries().iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["caf\u{e9}.txt", "\u{3042}.txt"]);
    assert!(zip.entries().iter().all(|e| e.flags & 0x800 != 0));
}

#[test]
fn raw_stream_is_the_stored_bytes() {
    let mut zip = open("method_stored.zip");
    let mut raw = Vec::new();
    zip.open_raw(0).unwrap().read_to_end(&mut raw).unwrap();
    assert_eq!(raw, sample_text());
}

#[test]
fn not_torrentzip() {
    for f in [
        "method_deflate.zip",
        "method_stored.zip",
        "zip64_forced.zip",
    ] {
        let zip = open(f);
        assert_eq!(zip.structure(), ZipStructure::None, "{f}");
        assert_eq!(zip.header_structure(), ZipStructure::None, "{f}");
    }
}

/// Patch a field of a one-entry zip: the method in both headers.
fn with_method(mut z: Vec<u8>, method: u16) -> Vec<u8> {
    let local = 8;
    let cd = z
        .windows(4)
        .rposition(|w| w == [0x50, 0x4b, 0x01, 0x02])
        .unwrap();
    z[local..local + 2].copy_from_slice(&method.to_le_bytes());
    z[cd + 10..cd + 12].copy_from_slice(&method.to_le_bytes());
    z
}

#[test]
fn methods_romvault_does_not_read_fail_the_open() {
    for m in [10u16, 95, 99] {
        let err = ZipArchive::open(Cursor::new(with_method(fixture("method_stored.zip"), m)))
            .err()
            .unwrap_or_else(|| panic!("method {m} opened"));
        assert!(
            matches!(err, ArchiveError::UnsupportedCompression { .. }),
            "{m}: {err}"
        );
    }
}

#[test]
fn shrink_reduce_implode_open_but_do_not_decode() {
    // ROMVault reads these (`ZipOpenRead.cs:235-255`); no permitted crate does, so listing works
    // and reading the data is the typed error.
    for m in [1u16, 2, 3, 4, 5, 6] {
        let mut zip =
            ZipArchive::open(Cursor::new(with_method(fixture("method_stored.zip"), m))).unwrap();
        assert_eq!(zip.entries()[0].method, m);
        let err = zip.read_entry(0).unwrap_err();
        assert!(
            matches!(err, ArchiveError::UnsupportedCompression { .. }),
            "{m}: {err}"
        );
    }
}

#[test]
fn a_flipped_data_byte_is_a_crc_error() {
    let mut z = fixture("method_stored.zip");
    z[30 + "sample.txt".len() + 100] ^= 0x01;
    let mut zip = ZipArchive::open(Cursor::new(z)).unwrap();
    match zip.read_entry(0).unwrap_err() {
        ArchiveError::CrcMismatch {
            entry,
            expected,
            actual,
        } => {
            assert_eq!(entry, "sample.txt");
            assert_ne!(expected, actual);
        }
        e => panic!("{e}"),
    }
}

#[test]
fn a_corrupt_deflate_stream_is_an_error() {
    let mut z = fixture("method_deflate.zip");
    let at = 30 + "sample.txt".len() + 10;
    for b in &mut z[at..at + 20] {
        *b = !*b;
    }
    let mut zip = ZipArchive::open(Cursor::new(z)).unwrap();
    assert!(zip.read_entry(0).is_err());
}

#[test]
fn every_truncation_is_an_error_not_a_panic() {
    for f in [
        "method_deflate.zip",
        "zip64_forced.zip",
        "names.zip",
        "data_descriptor.zip",
    ] {
        let z = fixture(f);
        for len in 0..z.len() {
            let r = ZipArchive::open(Cursor::new(z[..len].to_vec()));
            assert!(r.is_err(), "{f} cut to {len} bytes opened");
        }
    }
}

/// xorshift, so the corruption pass is the same on every run.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

#[test]
fn random_corruption_never_panics() {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    for f in [
        "method_stored.zip",
        "method_deflate.zip",
        "method_deflate64.zip",
        "method_bzip2.zip",
        "method_lzma.zip",
        "method_zstd.zip",
        "method_ppmd.zip",
        "zip64_forced.zip",
        "data_descriptor.zip",
    ] {
        let orig = fixture(f);
        for _ in 0..300 {
            let mut z = orig.clone();
            for _ in 0..1 + rng.next() % 4 {
                let at = (rng.next() % z.len() as u64) as usize;
                z[at] ^= 1 << (rng.next() % 8);
            }
            if let Ok(mut zip) = ZipArchive::open(Cursor::new(z)) {
                for i in 0..zip.len() {
                    let _ = zip.read_entry(i);
                }
            }
        }
    }
}

#[test]
fn a_prefix_before_the_zip_shifts_the_offsets() {
    let mut z = b"MZ stub that is not part of the zip".to_vec();
    z.extend(fixture("method_deflate.zip"));
    let mut zip = ZipArchive::open(Cursor::new(z)).unwrap();
    assert_eq!(zip.prefix_offset(), 35);
    assert_eq!(zip.read_entry(0).unwrap(), sample_text());
}

#[test]
fn trailing_data_after_the_comment_is_reported() {
    let mut z = fixture("method_deflate.zip");
    z.extend(b"junk");
    let zip = ZipArchive::open(Cursor::new(z)).unwrap();
    assert!(zip.has_trailing_data());
}
