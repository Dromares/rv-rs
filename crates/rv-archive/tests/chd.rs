//! CHD reading and verification on synthetic files built by `tests/common/chd.rs`, and the error
//! paths: parents, unsupported codecs and versions, bad hashes, corruption and truncation.
//! (The two real CHDs are in `tests/fixtures.rs`.)

mod common;

use std::io::Cursor;

use common::chd::*;
use common::flac::verbatim_frame;
use rv_archive::chd::{codec, Chd};
use rv_archive::{ArchiveError, ChdError};

const HUNK: u32 = 4096;

fn spec(compressors: [u32; 4], hunks: Vec<Hunk>) -> Spec {
    let total = hunks.len() as u64 * HUNK as u64;
    Spec {
        compressors,
        hunk_bytes: HUNK,
        unit_bytes: 1,
        total_bytes: total,
        hunks,
        meta: Vec::new(),
        parent_sha1: [0; 20],
    }
}

fn slot_hunk(slot: u8, decoded: Vec<u8>, encode: impl Fn(&[u8]) -> Vec<u8>) -> Hunk {
    let stored = encode(&decoded);
    Hunk {
        decoded,
        out: Out::Slot(slot, stored),
    }
}

fn open(bytes: &[u8]) -> Result<Chd<Cursor<Vec<u8>>>, ArchiveError> {
    Chd::open(Cursor::new(bytes.to_vec()))
}

fn chd_err(e: ArchiveError) -> ChdError {
    match e {
        ArchiveError::Chd(c) => c,
        other => panic!("not a CHD error: {other}"),
    }
}

// ---- v5 ---------------------------------------------------------------------------------------

#[test]
fn v5_uncompressed_map() {
    let hunks: Vec<Hunk> = (0..3)
        .map(|i| Hunk {
            decoded: pattern(HUNK as usize, i),
            out: Out::Raw,
        })
        .collect();
    let built = build_v5(&spec([0; 4], hunks));
    let mut chd = open(&built.bytes).unwrap();
    assert_eq!(chd.header().version, 5);
    assert_eq!(chd.header().total_hunks, 3);
    assert_eq!(chd.header().dat_sha1(), Some(built.combined_sha1));
    let r = chd.verify().unwrap();
    assert_eq!(r.raw_sha1, built.raw_sha1);
    assert_eq!(r.combined_sha1, Some(built.combined_sha1));
    assert_eq!(r.hunks, 3);
}

#[test]
fn v5_compressed_map_with_self_references_and_metadata() {
    let a = pattern(HUNK as usize, 1);
    let b = pattern(HUNK as usize, 2);
    let hunks = vec![
        slot_hunk(0, a.clone(), deflate),
        slot_hunk(0, b.clone(), deflate),
        Hunk {
            decoded: a.clone(),
            out: Out::SelfRef(0),
        },
        Hunk {
            decoded: b.clone(),
            out: Out::SelfRef(2 - 1), // hunk 1
        },
        Hunk {
            decoded: a.clone(),
            out: Out::SelfRef(2), // a reference to a reference
        },
        Hunk {
            decoded: pattern(HUNK as usize, 9),
            out: Out::Raw,
        },
    ];
    let mut s = spec([ZLIB, 0, 0, 0], hunks);
    s.meta = vec![
        Meta {
            tag: u32::from_be_bytes(*b"CHT2"),
            flags: 1,
            data: b"track data".to_vec(),
        },
        Meta {
            tag: u32::from_be_bytes(*b"NOTE"),
            flags: 0,
            data: b"not hashed".to_vec(),
        },
        Meta {
            tag: u32::from_be_bytes(*b"AAAA"),
            flags: 1,
            data: b"sorts first".to_vec(),
        },
    ];
    let built = build_v5(&s);
    let mut chd = open(&built.bytes).unwrap();
    let r = chd.verify().unwrap();
    assert_eq!(r.raw_sha1, built.raw_sha1);
    assert_eq!(r.combined_sha1, Some(built.combined_sha1));
    // Single hunks can be read on their own, self references included.
    let mut out = vec![0u8; HUNK as usize];
    chd.read_hunk(4, &mut out).unwrap();
    assert_eq!(out, a);
    chd.read_hunk(3, &mut out).unwrap();
    assert_eq!(out, b);
}

#[test]
fn v5_partial_last_hunk_hashes_only_the_logical_bytes() {
    let hunks = vec![
        slot_hunk(0, pattern(HUNK as usize, 1), deflate),
        slot_hunk(0, pattern(HUNK as usize, 2), deflate),
    ];
    let mut s = spec([ZLIB, 0, 0, 0], hunks);
    s.total_bytes = HUNK as u64 + 100;
    let built = build_v5(&s);
    let r = open(&built.bytes).unwrap().verify().unwrap();
    assert_eq!(r.bytes, HUNK as u64 + 100);
    assert_eq!(r.raw_sha1, built.raw_sha1);
}

#[test]
fn v5_every_codec() {
    // zlib, zstd, lzma and huff, one hunk each, in codec slots 0 to 3.
    let huff_encode = |d: &[u8]| -> Vec<u8> {
        // A tree of 256 codes of 8 bits: the code lengths are sent with a one-symbol small tree.
        let mut b = Bits::new();
        b.put(0, 3); // length of small symbol 0
        b.put(7, 3); // lengths follow from symbol 8
        b.put(0, 3); // symbol 8 is unused
        b.put(1, 3); // symbol 9 has length 1
        b.put(7, 3); // symbol 10 ends the list
        for _ in 0..256 {
            b.put(0, 1); // small symbol 9 = length 8
        }
        for &x in d {
            b.put(x as u64, 8);
        }
        b.finish()
    };
    let flac_encode = |d: &[u8]| -> Vec<u8> {
        // 'B' marks big-endian samples; the data are 16-bit stereo, so 1024 samples.
        let samples: Vec<i16> = d
            .chunks(2)
            .map(|c| i16::from_be_bytes([c[0], c[1]]))
            .collect();
        let mut out = vec![b'B'];
        for (i, f) in samples.chunks(1024).enumerate() {
            out.extend(verbatim_frame(i as u8, f));
        }
        out
    };
    let hunks = vec![
        slot_hunk(0, pattern(HUNK as usize, 1), deflate),
        slot_hunk(1, pattern(HUNK as usize, 2), zstd),
        slot_hunk(2, pattern(HUNK as usize, 3), |d| lzma(d, HUNK)),
        slot_hunk(3, pattern(HUNK as usize, 4), huff_encode),
    ];
    let built = build_v5(&spec([ZLIB, ZSTD, LZMA, HUFF], hunks));
    open(&built.bytes).unwrap().verify().unwrap();

    let hunks = vec![slot_hunk(0, pattern(HUNK as usize, 5), flac_encode)];
    let built = build_v5(&spec([FLAC, 0, 0, 0], hunks));
    open(&built.bytes).unwrap().verify().unwrap();
}

// ---- CD hunks ---------------------------------------------------------------------------------

const FRAMES: usize = 4;
const CD_HUNK: usize = FRAMES * 2448;

/// Sector and subcode areas of a hunk of `FRAMES` frames, and the interleaved decoded hunk.
fn cd_areas(seed: u32) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let sector = pattern(FRAMES * 2352, seed);
    let sub = pattern(FRAMES * 96, seed + 100);
    let mut hunk = Vec::new();
    for f in 0..FRAMES {
        hunk.extend(&sector[f * 2352..(f + 1) * 2352]);
        hunk.extend(&sub[f * 96..(f + 1) * 96]);
    }
    (sector, sub, hunk)
}

fn cd_stored(sector: &[u8], sub: &[u8], bitmap: u8, base: Vec<u8>, subc: Vec<u8>) -> Vec<u8> {
    let _ = (sector, sub);
    let mut out = vec![bitmap];
    out.extend((base.len() as u16).to_be_bytes());
    out.extend(base);
    out.extend(subc);
    out
}

fn cd_spec(codecs: [u32; 4], hunks: Vec<Hunk>) -> Spec {
    let mut s = spec(codecs, hunks);
    s.hunk_bytes = CD_HUNK as u32;
    s.unit_bytes = 2448;
    s.total_bytes = s.hunks.len() as u64 * CD_HUNK as u64;
    s
}

#[test]
fn v5_cd_codecs() {
    let (s0, u0, h0) = cd_areas(1);
    let (s1, u1, h1) = cd_areas(2);
    let (s2, u2, h2) = cd_areas(3);
    let mut flac_stored = Vec::new();
    for (i, f) in s2
        .chunks(2)
        .map(|c| i16::from_be_bytes([c[0], c[1]]))
        .collect::<Vec<_>>()
        .chunks(588)
        .enumerate()
    {
        flac_stored.extend(verbatim_frame(i as u8, f));
    }
    flac_stored.extend(deflate(&u2));
    let hunks = vec![
        Hunk {
            decoded: h0,
            out: Out::Slot(0, cd_stored(&s0, &u0, 0, deflate(&s0), deflate(&u0))),
        },
        Hunk {
            decoded: h1,
            out: Out::Slot(
                1,
                cd_stored(&s1, &u1, 0, lzma(&s1, s1.len() as u32), deflate(&u1)),
            ),
        },
        Hunk {
            decoded: h2,
            out: Out::Slot(2, flac_stored),
        },
    ];
    let built = build_v5(&cd_spec([CD_ZLIB, CD_LZMA, CD_FLAC, 0], hunks));
    open(&built.bytes).unwrap().verify().unwrap();

    // cdzs: zstd for both areas.
    let (s3, u3, h3) = cd_areas(4);
    let hunks = vec![Hunk {
        decoded: h3,
        out: Out::Slot(0, cd_stored(&s3, &u3, 0, zstd(&s3), zstd(&u3))),
    }];
    let built = build_v5(&cd_spec([CD_ZSTD, 0, 0, 0], hunks));
    open(&built.bytes).unwrap().verify().unwrap();
}

#[test]
fn cd_hunks_get_their_sync_header_and_ecc_back() {
    let sector = std::fs::read(format!(
        "{}/tests/data/mode1_sector.bin",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    assert_eq!(sector.len(), 2352);
    // What MAME stores: the sync header and the P/Q parity zeroed, a bit set in the ECC bitmap.
    let mut stripped = sector.clone();
    stripped[..12].fill(0);
    stripped[2076..].fill(0);
    // Four frames: frame 1 has the bitmap bit, the others are plain data.
    let mut sectors = Vec::new();
    for f in 0..FRAMES {
        sectors.extend(if f == 1 { &stripped } else { &sector });
    }
    let sub = pattern(FRAMES * 96, 3);
    let mut decoded = Vec::new();
    for f in 0..FRAMES {
        decoded.extend(&sector[..]);
        decoded.extend(&sub[f * 96..(f + 1) * 96]);
    }
    let stored = cd_stored(
        &sectors,
        &sub,
        0b0000_0010,
        deflate(&sectors),
        deflate(&sub),
    );
    // Frames 0, 2 and 3 are stored complete (not stripped), so only frame 1 is regenerated and
    // every decoded frame must equal the reference sector.
    let hunks = vec![Hunk {
        decoded,
        out: Out::Slot(0, stored),
    }];
    let built = build_v5(&cd_spec([CD_ZLIB, 0, 0, 0], hunks));
    open(&built.bytes)
        .unwrap()
        .verify()
        .expect("regenerated ECC must match the reference");
}

// ---- older versions ---------------------------------------------------------------------------

fn old_hunks() -> Vec<Hunk> {
    let a = pattern(HUNK as usize, 1);
    vec![
        slot_hunk(0, a.clone(), deflate),
        slot_hunk(0, pattern(HUNK as usize, 2), deflate),
        Hunk {
            decoded: a,
            out: Out::SelfRef(0),
        },
        Hunk {
            decoded: pattern(HUNK as usize, 3),
            out: Out::Raw,
        },
    ]
}

#[test]
fn v3_with_mini_and_self_hunks() {
    let mut hunks = old_hunks();
    let word = 0x0123_4567_89ab_cdefu64;
    hunks.push(Hunk {
        decoded: word
            .to_be_bytes()
            .iter()
            .copied()
            .cycle()
            .take(HUNK as usize)
            .collect(),
        out: Out::Mini(word),
    });
    let mut s = spec([ZLIB, 0, 0, 0], hunks);
    s.meta = vec![Meta {
        tag: u32::from_be_bytes(*b"GDDD"),
        flags: 1,
        data: vec![1, 2, 3],
    }];
    let built = build_v3_v4(&s, 3);
    let mut chd = open(&built.bytes).unwrap();
    assert_eq!(chd.header().version, 3);
    assert_eq!(chd.header().md5, Some(built.md5));
    let r = chd.verify().unwrap();
    assert_eq!(r.raw_sha1, built.raw_sha1);
    assert_eq!(r.md5, Some(built.md5));
    // v3 has no combined SHA-1: the metadata is read but there is nothing to compare it with.
    assert_eq!(r.combined_sha1, None);
    assert_eq!(chd.header().dat_sha1(), Some(built.raw_sha1));
}

#[test]
fn v4_checks_the_combined_sha1() {
    let mut s = spec([ZLIB, 0, 0, 0], old_hunks());
    s.meta = vec![Meta {
        tag: u32::from_be_bytes(*b"CHT2"),
        flags: 1,
        data: b"x".to_vec(),
    }];
    let built = build_v3_v4(&s, 4);
    let mut chd = open(&built.bytes).unwrap();
    assert_eq!(chd.header().version, 4);
    let r = chd.verify().unwrap();
    assert_eq!(r.combined_sha1, Some(built.combined_sha1));
    assert_eq!(chd.header().dat_sha1(), Some(built.combined_sha1));
}

#[test]
fn v1_and_v2_hard_disk_chds() {
    let hunks: Vec<Hunk> = old_hunks()
        .into_iter()
        .filter(|h| !matches!(h.out, Out::SelfRef(_)))
        .collect();
    for version in [1u32, 2] {
        let s = spec(
            [ZLIB, 0, 0, 0],
            hunks
                .iter()
                .map(|h| Hunk {
                    decoded: h.decoded.clone(),
                    out: match &h.out {
                        Out::Slot(s, d) => Out::Slot(*s, d.clone()),
                        _ => Out::Raw,
                    },
                })
                .collect(),
        );
        let built = build_v1_v2(&s, version);
        let mut chd = open(&built.bytes).unwrap();
        assert_eq!(chd.header().version, version);
        assert_eq!(chd.header().hunk_bytes, HUNK);
        let r = chd.verify().unwrap();
        assert_eq!(r.md5, Some(built.md5));
    }
    // Equal map entries are references to the first.
    let mut hunks = old_hunks();
    hunks.truncate(3); // a, b, self(0)
    let s = spec([ZLIB, 0, 0, 0], hunks);
    let built = build_v1_v2(&s, 2);
    let mut chd = open(&built.bytes).unwrap();
    assert_eq!(chd.header().map[2].comp, rv_archive::chd::CompType::SelfRef);
    chd.verify().unwrap();
}

// ---- refusals and damage ----------------------------------------------------------------------

fn small_v5() -> Built {
    build_v5(&spec(
        [ZLIB, 0, 0, 0],
        vec![
            slot_hunk(0, pattern(HUNK as usize, 1), deflate),
            Hunk {
                decoded: pattern(HUNK as usize, 2),
                out: Out::Raw,
            },
        ],
    ))
}

#[test]
fn a_child_chd_requires_its_parent() {
    let mut s = spec(
        [ZLIB, 0, 0, 0],
        vec![slot_hunk(0, pattern(HUNK as usize, 1), deflate)],
    );
    s.parent_sha1 = [7; 20];
    let built = build_v5(&s);
    let mut chd = open(&built.bytes).unwrap();
    assert!(chd.header().requires_parent());
    // The hashes are still available, as in ROMVault (`CHD.cs:121-129`).
    assert_eq!(chd.header().dat_sha1(), Some(built.combined_sha1));
    assert_eq!(chd_err(chd.verify().unwrap_err()), ChdError::RequiresParent);
}

#[test]
fn versions_above_5_and_bad_tags() {
    let good = small_v5().bytes;
    let mut v6 = good.clone();
    v6[15] = 6;
    assert_eq!(
        chd_err(open(&v6).err().unwrap()),
        ChdError::UnsupportedVersion(6)
    );
    let mut v0 = good.clone();
    v0[15] = 0;
    assert!(matches!(
        chd_err(open(&v0).err().unwrap()),
        ChdError::UnsupportedVersion(0)
    ));
    let mut bad_tag = good.clone();
    bad_tag[0] = b'X';
    assert!(matches!(
        chd_err(open(&bad_tag).err().unwrap()),
        ChdError::InvalidFile(_)
    ));
    let mut bad_len = good;
    bad_len[11] = 100;
    assert!(matches!(
        chd_err(open(&bad_len).err().unwrap()),
        ChdError::InvalidFile(_)
    ));
}

#[test]
fn avhuff_hunks_are_an_unsupported_codec() {
    let hunks = vec![slot_hunk(0, pattern(HUNK as usize, 1), |d| d.to_vec())];
    let built = build_v5(&spec([AVHU, 0, 0, 0], hunks));
    let mut chd = open(&built.bytes).unwrap();
    assert_eq!(chd.unsupported_codec(), Some(codec::AVHU));
    assert!(
        matches!(chd_err(chd.verify().unwrap_err()), ChdError::UnsupportedCodec(n) if n == "avhu")
    );
}

#[test]
fn a_wrong_header_hash_is_a_verify_failure() {
    let mut b = small_v5().bytes;
    // The raw SHA-1 sits at offset 64.
    b[64] ^= 1;
    match chd_err(open(&b).unwrap().verify().unwrap_err()) {
        ChdError::VerifyFailed { what, .. } => assert_eq!(what, "raw SHA-1"),
        e => panic!("{e}"),
    }
    // A damaged combined SHA-1 (offset 84) is an invalid-metadata error.
    let mut b = small_v5().bytes;
    b[84] ^= 1;
    assert!(matches!(
        chd_err(open(&b).unwrap().verify().unwrap_err()),
        ChdError::InvalidMetadata(_)
    ));
}

#[test]
fn a_flipped_byte_in_any_hunk_fails_the_crc() {
    let built = small_v5();
    let chd = open(&built.bytes).unwrap();
    let entries = chd.header().map.clone();
    for (i, e) in entries.iter().enumerate() {
        let mut b = built.bytes.clone();
        b[(e.offset + e.length as u64 / 2) as usize] ^= 0x04;
        let err = open(&b).unwrap().verify().expect_err(&format!("hunk {i}"));
        assert!(
            matches!(
                chd_err(err),
                ChdError::Decompression(_) | ChdError::InvalidData(_)
            ),
            "hunk {i}"
        );
    }
}

#[test]
fn a_damaged_metadata_entry_is_detected() {
    let mut s = spec(
        [ZLIB, 0, 0, 0],
        vec![slot_hunk(0, pattern(HUNK as usize, 1), deflate)],
    );
    s.meta = vec![Meta {
        tag: u32::from_be_bytes(*b"CHT2"),
        flags: 1,
        data: b"track".to_vec(),
    }];
    let built = build_v5(&s);
    let n = built.bytes.len();
    let mut b = built.bytes.clone();
    b[n - 1] ^= 1; // the last metadata byte
    assert!(matches!(
        chd_err(open(&b).unwrap().verify().unwrap_err()),
        ChdError::InvalidMetadata(_)
    ));
    // A chain that points at itself is an error, not a hang.
    let mut b = built.bytes.clone();
    let meta_at = n - 16 - 5;
    b[meta_at + 8..meta_at + 16].copy_from_slice(&(meta_at as u64).to_be_bytes());
    assert!(open(&b).unwrap().verify().is_err());
}

#[test]
fn every_truncation_is_an_error_not_a_panic() {
    let files = [
        small_v5().bytes,
        build_v3_v4(&spec([ZLIB, 0, 0, 0], old_hunks()), 3).bytes,
        build_v3_v4(&spec([ZLIB, 0, 0, 0], old_hunks()), 4).bytes,
        build_v1_v2(
            &spec(
                [ZLIB, 0, 0, 0],
                old_hunks()[..2]
                    .iter()
                    .map(|h| Hunk {
                        decoded: h.decoded.clone(),
                        out: Out::Raw,
                    })
                    .collect(),
            ),
            2,
        )
        .bytes,
    ];
    for f in &files {
        let mut ok = 0;
        for len in 0..f.len() {
            if let Ok(mut c) = open(&f[..len]) {
                if c.verify().is_ok() {
                    ok += 1;
                }
            }
        }
        // A file cut inside its metadata is the only way to still verify (there is none here).
        assert_eq!(ok, 0, "a truncated CHD verified");
    }
}

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
    let mut s = spec([ZLIB, 0, 0, 0], old_hunks());
    s.meta = vec![Meta {
        tag: 1,
        flags: 1,
        data: b"abc".to_vec(),
    }];
    let files = [
        build_v5(&s).bytes,
        build_v3_v4(&s, 3).bytes,
        build_v3_v4(&s, 4).bytes,
    ];
    let mut rng = Rng(0xfeed_beef_1234_5678);
    for f in &files {
        for _ in 0..400 {
            let mut b = f.clone();
            for _ in 0..1 + rng.next() % 3 {
                let at = (rng.next() % b.len() as u64) as usize;
                b[at] ^= 1 << (rng.next() % 8);
            }
            if let Ok(mut c) = open(&b) {
                let _ = c.verify();
            }
        }
    }
}
