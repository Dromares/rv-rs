//! Synthetic tests: hash vectors, one positive and one negative vector per copier-header
//! type, and regression tests for the two ROMVault 3.8.0 defects. No board data.

use std::io::{self, Read};

use rv_core::HeaderFileType;
use rv_scan::compare::{compare_alt_hash, compare_hash, compare_with_alt, Known};
use rv_scan::hash::to_hex;
use rv_scan::header::{
    alt_header_file, detect, file_header_length, file_type_from_header, DETECTORS,
};
use rv_scan::{scan, Digests, HashKinds, HashSet, ScanOptions, ScanResult, MAX_HEADER_PROBE};

fn hex<const N: usize>(d: Option<[u8; N]>) -> String {
    to_hex(&d.expect("hash present"))
}

fn all(data: &[u8]) -> ScanResult {
    scan(&mut &data[..], ScanOptions::default()).expect("scan")
}

fn digests(data: &[u8]) -> Digests {
    let mut h = HashSet::new(HashKinds::ALL);
    h.update(data);
    h.finish()
}

fn pattern251(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 251) as u8).collect()
}

fn pattern7(len: usize) -> Vec<u8> {
    (0..len).map(|i| ((i * 7 + 3) & 0xff) as u8).collect()
}

struct Vector {
    data: Vec<u8>,
    crc: &'static str,
    md5: &'static str,
    sha1: &'static str,
    sha256: &'static str,
}

fn vectors() -> Vec<(&'static str, Vector)> {
    // Expected values come from python hashlib/zlib, not from this crate.
    vec![
        (
            "empty",
            Vector {
                data: vec![],
                crc: "00000000",
                md5: "d41d8cd98f00b204e9800998ecf8427e",
                sha1: "da39a3ee5e6b4b0d3255bfef95601890afd80709",
                sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            },
        ),
        (
            "abc",
            Vector {
                data: b"abc".to_vec(),
                crc: "352441c2",
                md5: "900150983cd24fb0d6963f7d28e17f72",
                sha1: "a9993e364706816aba3e25717850c26c9cd0d89d",
                sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            },
        ),
        (
            "1MiB i%251",
            Vector {
                data: pattern251(1 << 20),
                crc: "ef0e6054",
                md5: "8f293a2f6c19b345152f7a49bb4c643c",
                sha1: "c2fc4cb20f1301a6b0dd211c19e69a13925dbe40",
                sha256: "631b84027d6b9e52b539c4e8373622d23032dfadc64d60af87339c9037e4f769",
            },
        ),
        (
            "1MiB (i*7+3)&255",
            Vector {
                data: pattern7(1 << 20),
                crc: "4a24d8fa",
                md5: "d8a469bb35fb5fad7156842ba2242434",
                sha1: "9d4a07b141606cf1c54373230e895e684c654460",
                sha256: "172c15dc2e12b50e523d8e657cbe7fbb11c1053252bbf1e1431077d57d8128fd",
            },
        ),
    ]
}

/// A reader that returns at most `n` bytes per call, which `Stream.Read` is allowed to do and
/// which 3.8.0's `CheckSumRead` does not cope with (`FileScan.cs:256,366,387`).
struct Dribble<'a> {
    data: &'a [u8],
    n: usize,
}

impl Read for Dribble<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let k = self.n.min(buf.len()).min(self.data.len());
        buf[..k].copy_from_slice(&self.data[..k]);
        self.data = &self.data[k..];
        Ok(k)
    }
}

#[test]
fn standard_vectors() {
    for (name, v) in vectors() {
        let r = all(&v.data);
        assert_eq!(r.size, v.data.len() as u64, "{name}: size");
        assert_eq!(hex(r.digests.crc), v.crc, "{name}: crc");
        assert_eq!(hex(r.digests.md5), v.md5, "{name}: md5");
        assert_eq!(hex(r.digests.sha1), v.sha1, "{name}: sha1");
        assert_eq!(hex(r.digests.sha256), v.sha256, "{name}: sha256");
        assert_eq!(r.header_type, HeaderFileType::NOTHING, "{name}");
        assert!(r.alt.is_none(), "{name}");
    }
}

#[test]
fn short_reads_do_not_change_the_hashes() {
    for (name, v) in vectors() {
        for n in [1usize, 7, 511, 512, 513, 4096] {
            let r = scan(&mut Dribble { data: &v.data, n }, ScanOptions::default()).unwrap();
            assert_eq!(r.size, v.data.len() as u64, "{name} n={n}");
            assert_eq!(hex(r.digests.sha1), v.sha1, "{name} n={n}");
            assert_eq!(hex(r.digests.crc), v.crc, "{name} n={n}");
        }
    }
}

#[test]
fn input_larger_than_one_buffer() {
    // Three 4 MiB buffers plus a bit: the chunk loop must carry on to the end.
    let data = pattern251(3 * 4096 * 1024 + 12345);
    let r = all(&data);
    assert_eq!(r.size, data.len() as u64);
    assert_eq!(r.digests, digests(&data));
}

#[test]
fn romvault_default_leaves_sha256_out() {
    let r = scan(
        &mut &b"abc"[..],
        ScanOptions {
            full_scan: true,
            sha256: false,
        },
    )
    .unwrap();
    assert!(r.digests.sha256.is_none());
    assert_eq!(
        hex(r.digests.sha1),
        "a9993e364706816aba3e25717850c26c9cd0d89d"
    );
}

// ---------------------------------------------------------------------------------------
// Copier headers: one positive and one negative vector per detector row.
// ---------------------------------------------------------------------------------------

/// A file whose first `header_length` bytes are zeros except for `magic` at `magic_offset`,
/// followed by a recognisable body.
fn headered(d: &rv_scan::header::Detector, body_len: usize) -> Vec<u8> {
    let mut data = vec![0u8; d.header_length];
    data[d.magic_offset..d.magic_offset + d.magic.len()].copy_from_slice(d.magic);
    data.extend(pattern251(body_len));
    data
}

#[test]
fn every_detector_has_a_positive_and_a_negative_vector() {
    for (i, d) in DETECTORS.iter().enumerate() {
        let id = format!("row {i} ({:?}, magic@{})", d.file_type, d.magic_offset);

        // Positive: the magic is there and the probe is longer than the header.
        let data = headered(d, 1000);
        let (t, off) = detect(&data[..MAX_HEADER_PROBE.min(data.len())]);
        // An earlier row can legitimately claim the same bytes (the four FDS rows and the
        // No-Intro FDS row share a prefix), so require the type, not the row.
        assert_eq!(t, d.file_type, "{id}: positive type");
        assert_eq!(off, d.file_offset, "{id}: positive offset");

        let r = all(&data);
        assert_eq!(r.header_type, d.file_type, "{id}: scan type");
        if d.file_offset == 0 {
            // Archive and CHD types: typed, but no alt hashes (`FileScan.cs:261`).
            assert!(r.alt.is_none(), "{id}: no alt for container types");
        } else {
            let alt = r.alt.as_ref().expect("alt hashes");
            let body = &data[d.file_offset..];
            assert_eq!(alt.size, body.len() as u64, "{id}: alt size");
            assert_eq!(alt.digests, digests(body), "{id}: alt digests");
            assert_eq!(
                r.digests,
                digests(&data),
                "{id}: main digests cover the header"
            );
        }

        // Negative 1: one magic byte flipped.
        let mut bad = data.clone();
        bad[d.magic_offset] ^= 0xff;
        let (t, _) = detect(&bad[..MAX_HEADER_PROBE]);
        let expect_other = DETECTORS.iter().any(|o| {
            !std::ptr::eq(o, d) && o.file_type == d.file_type && o.magic_offset != d.magic_offset
        });
        if !expect_other {
            assert_eq!(t, HeaderFileType::NOTHING, "{id}: flipped magic");
        }

        // Negative 2: a file that is only the header (probe not longer than the header).
        let only_header = &data[..d.header_length];
        let (t, _) = detect(only_header);
        assert_eq!(t, HeaderFileType::NOTHING, "{id}: header-only file");
    }
}

#[test]
fn header_ids_map_back_to_their_type() {
    for d in DETECTORS.iter().filter(|d| !d.header_id.is_empty()) {
        assert_eq!(file_type_from_header(d.header_id), d.file_type);
        assert_eq!(
            file_type_from_header(&d.header_id.to_uppercase()),
            d.file_type,
            "case-insensitive"
        );
    }
    assert_eq!(file_type_from_header(""), HeaderFileType::NOTHING);
    assert_eq!(file_type_from_header("  "), HeaderFileType::NOTHING);
    assert_eq!(file_type_from_header("nosuch.xml"), HeaderFileType::NOTHING);
    // `FileHeaders.cs:53-62`: the first row of the type.
    assert_eq!(file_header_length(HeaderFileType::NES), 16);
    assert_eq!(file_header_length(HeaderFileType::SNES), 512);
    assert_eq!(file_header_length(HeaderFileType::ZIP), 0);
    assert_eq!(file_header_length(HeaderFileType::NOTHING), 0);
}

#[test]
fn alt_header_types_are_the_nine_in_the_source() {
    for t in [
        HeaderFileType::A7800,
        HeaderFileType::FDS,
        HeaderFileType::LYNX,
        HeaderFileType::NES,
        HeaderFileType::PCE,
        HeaderFileType::PSID,
        HeaderFileType::SNES,
        HeaderFileType::SPC,
        HeaderFileType::CHD,
    ] {
        assert!(alt_header_file(t));
        // The `Required` bit is masked off (`FileHeaders.cs:87`).
        assert!(alt_header_file(HeaderFileType(
            t.0 | HeaderFileType::REQUIRED.0
        )));
    }
    for t in [
        HeaderFileType::NOTHING,
        HeaderFileType::ZIP,
        HeaderFileType::GZ,
        HeaderFileType::SEVEN_ZIP,
        HeaderFileType::RAR,
    ] {
        assert!(!alt_header_file(t));
    }
}

#[test]
fn nes_header_alt_hashes_are_the_headerless_hashes() {
    let mut rom = b"NES\x1a".to_vec();
    rom.extend([2u8, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]); // 16-byte iNES header
    let body = pattern7(40_000);
    rom.extend(&body);
    let r = all(&rom);
    assert_eq!(r.header_type, HeaderFileType::NES);
    assert_eq!(r.size, 16 + 40_000);
    let alt = r.alt.expect("alt");
    assert_eq!(alt.size, 40_000);
    assert_eq!(alt.digests, digests(&body));
    assert_eq!(r.digests, digests(&rom));
}

#[test]
fn quick_scan_matches_the_level1_rules() {
    let quick = ScanOptions {
        full_scan: false,
        sha256: true,
    };
    // No header: nothing is hashed (`FileScan.cs:261-269`).
    let r = scan(&mut &pattern251(10_000)[..], quick).unwrap();
    assert_eq!(r.digests, Digests::default());
    assert!(r.alt.is_none());
    // Header: only the alt CRC (`FileScan.cs:295-304`).
    let mut nes = b"NES\x1a".to_vec();
    nes.extend([0u8; 12]);
    nes.extend(pattern251(5_000));
    let r = scan(&mut &nes[..], quick).unwrap();
    assert_eq!(r.digests, Digests::default());
    let alt = r.alt.expect("alt");
    assert_eq!(alt.size, 5_000);
    assert_eq!(alt.digests.crc, digests(&pattern251(5_000)).crc);
    assert!(
        alt.digests.md5.is_none() && alt.digests.sha1.is_none() && alt.digests.sha256.is_none()
    );
}

// ---------------------------------------------------------------------------------------
// Regression: 3.8.0 defect (b), SNES / PCE detection can never fire.
// ---------------------------------------------------------------------------------------

fn snes_rom(body_len: usize) -> Vec<u8> {
    // 512-byte copier header with the 8-byte signature at offset 22 (`FileHeaders.cs:47`).
    let mut data = vec![0u8; 512];
    data[22..30].copy_from_slice(&[0xAA, 0xBB, 0x04, 0, 0, 0, 0, 0]);
    data.extend(pattern251(body_len));
    data
}

fn pce_rom(body_len: usize) -> Vec<u8> {
    // `FileHeaders.cs:41`
    let mut data = vec![0u8; 512];
    data[..11].copy_from_slice(&[0x40, 0, 0, 0, 0, 0, 0, 0, 0xAA, 0xBB, 0x02]);
    data.extend(pattern251(body_len));
    data
}

/// What 3.8.0 does: it reads `min(512, size)` bytes (`FileScan.cs:252-254`) and a detector is
/// skipped unless the byte count is greater than its header length (`FileHeaders.cs:119`).
fn detect_like_3_8_0(file: &[u8]) -> HeaderFileType {
    let sizenow = file.len().min(512);
    detect(&file[..sizenow]).0
}

#[test]
fn snes_and_pce_copier_headers_are_detected() {
    // Fails on 3.8.0's logic: the probe is capped at 512 bytes, the SNES/PCE detectors need
    // more than 512 (`FileScan.cs:252-254` vs `FileHeaders.cs:41,47-48,119`).
    for (name, rom, want) in [
        ("snes", snes_rom(32_768), HeaderFileType::SNES),
        ("pce", pce_rom(32_768), HeaderFileType::PCE),
    ] {
        assert_eq!(
            detect_like_3_8_0(&rom),
            HeaderFileType::NOTHING,
            "{name}: 3.8.0 never sees this header"
        );

        let r = all(&rom);
        assert_eq!(r.header_type, want, "{name}: header detected");
        let alt = r.alt.expect("alt hashes");
        assert_eq!(alt.size, 32_768, "{name}: alt size");
        assert_eq!(
            alt.digests,
            digests(&rom[512..]),
            "{name}: header-stripped hashes"
        );
        assert_eq!(r.digests, digests(&rom), "{name}: full-file hashes");
    }
}

#[test]
fn snes_header_needs_a_body_byte() {
    // A 512-byte file is all header: not a headered ROM. 513 bytes is the smallest.
    assert_eq!(all(&snes_rom(0)).header_type, HeaderFileType::NOTHING);
    assert_eq!(all(&snes_rom(1)).header_type, HeaderFileType::SNES);
}

#[test]
fn other_detectors_are_unchanged_by_the_longer_probe() {
    // The probe grew from 512 to 513 bytes; every other row must decide exactly as before on
    // 512 bytes, because their headers are shorter than 512.
    for d in DETECTORS.iter().filter(|d| d.header_length < 512) {
        let data = headered(d, 2000);
        assert_eq!(detect(&data[..512]), detect(&data[..MAX_HEADER_PROBE]));
    }
}

// ---------------------------------------------------------------------------------------
// Regression: 3.8.0 defect (a), inverted MD5 alternate match.
// ---------------------------------------------------------------------------------------

/// The 3.8.0 test, kept as written: `if (ByteArrEqualsQuick(db.MD5, test.AltMD5)) return false;`
/// (`RomVaultCore/Scanner/Compare.cs:239`). Everything else in `CompareAltHash` is as
/// `rv_scan::compare::compare_alt_hash`.
fn compare_alt_hash_3_8_0(db: &Known, test: &ScanResult) -> bool {
    if !alt_header_file(test.header_type) || db.header_type != test.header_type {
        return false;
    }
    let alt = test.alt.as_ref().unwrap();
    if let Some(size) = db.size {
        if size != alt.size {
            return false;
        }
    }
    let mut found = false;
    if let (Some(a), Some(b)) = (&db.crc, &alt.digests.crc) {
        found = true;
        if a.as_slice() != b {
            return false;
        }
    }
    if let (Some(a), Some(b)) = (&db.sha1, &alt.digests.sha1) {
        found = true;
        if a.as_slice() != b {
            return false;
        }
    }
    if let (Some(a), Some(b)) = (&db.md5, &alt.digests.md5) {
        found = true;
        if a.as_slice() == b {
            return false; // the inversion
        }
    }
    found
}

fn nes_file() -> (Vec<u8>, Vec<u8>) {
    let mut rom = b"NES\x1a".to_vec();
    rom.extend([0u8; 12]);
    let body = pattern7(20_000);
    rom.extend(&body);
    (rom, body)
}

#[test]
fn alt_md5_equal_matches_and_different_does_not() {
    let (rom, body) = nes_file();
    let scanned = all(&rom);
    let alt_md5 = digests(&body).md5.unwrap().to_vec();

    // A DAT entry that has only an MD5 (the case the 3.8.0 bug hits hardest).
    let right = Known {
        md5: Some(alt_md5.clone()),
        header_type: HeaderFileType::NES,
        ..Known::default()
    };
    let mut wrong_md5 = alt_md5;
    wrong_md5[0] ^= 1;
    let wrong = Known {
        md5: Some(wrong_md5),
        header_type: HeaderFileType::NES,
        ..Known::default()
    };

    assert!(
        compare_alt_hash(&right, &scanned),
        "equal alt MD5 must match"
    );
    assert!(
        !compare_alt_hash(&wrong, &scanned),
        "different alt MD5 must not match"
    );
    assert_eq!(compare_with_alt(&right, &scanned), Some(true));
    assert_eq!(compare_with_alt(&wrong, &scanned), None);

    // 3.8.0 gets both wrong.
    assert!(
        !compare_alt_hash_3_8_0(&right, &scanned),
        "3.8.0 rejects an equal MD5"
    );
    assert!(
        compare_alt_hash_3_8_0(&wrong, &scanned),
        "3.8.0 accepts a different MD5"
    );
}

#[test]
fn alt_match_with_all_three_hashes() {
    let (rom, body) = nes_file();
    let scanned = all(&rom);
    let d = digests(&body);
    let known = Known {
        size: Some(body.len() as u64),
        crc: Some(d.crc.unwrap().to_vec()),
        sha1: Some(d.sha1.unwrap().to_vec()),
        md5: Some(d.md5.unwrap().to_vec()),
        header_type: HeaderFileType::NES,
    };
    assert_eq!(compare_with_alt(&known, &scanned), Some(true));
    // 3.8.0 would reject this one because of the MD5 inversion.
    assert!(!compare_alt_hash_3_8_0(&known, &scanned));
    // Plain hash comparison of the same DAT entry fails: the DAT hashes are headerless.
    assert!(!compare_hash(&known, &scanned));
}

#[test]
fn compare_hash_rules() {
    let r = all(b"abc");
    let d = digests(b"abc");
    let good = Known {
        size: Some(3),
        crc: Some(d.crc.unwrap().to_vec()),
        ..Known::default()
    };
    assert!(compare_hash(&good, &r));
    // Size must agree when both are known.
    assert!(!compare_hash(
        &Known {
            size: Some(4),
            ..good.clone()
        },
        &r
    ));
    // A size alone is not a match (`testFound`), and nor is nothing.
    assert!(!compare_hash(
        &Known {
            size: Some(3),
            ..Known::default()
        },
        &r
    ));
    assert!(!compare_hash(&Known::default(), &r));
    // One differing hash rejects even if the others agree.
    let bad_sha1 = Known {
        sha1: Some(vec![0; 20]),
        ..good.clone()
    };
    assert!(!compare_hash(&bad_sha1, &r));
    // Zero-length special case (`Compare.cs:173-177`).
    let empty = all(b"");
    assert!(compare_hash(
        &Known {
            size: Some(0),
            ..Known::default()
        },
        &empty
    ));
}

#[test]
fn alt_compare_requires_matching_header_types() {
    let (rom, body) = nes_file();
    let scanned = all(&rom);
    let md5 = digests(&body).md5.unwrap().to_vec();
    let known = |t| Known {
        md5: Some(md5.clone()),
        header_type: t,
        ..Known::default()
    };
    assert!(compare_alt_hash(&known(HeaderFileType::NES), &scanned));
    assert!(!compare_alt_hash(&known(HeaderFileType::NOTHING), &scanned));
    assert!(!compare_alt_hash(&known(HeaderFileType::SNES), &scanned));
    // The DAT's `Required` bit makes the types unequal, as in `Compare.cs:72`.
    assert!(!compare_alt_hash(
        &known(HeaderFileType(
            HeaderFileType::NES.0 | HeaderFileType::REQUIRED.0
        )),
        &scanned
    ));
    // A file with no header never alt-matches.
    assert!(!compare_alt_hash(&known(HeaderFileType::NES), &all(&body)));
}
