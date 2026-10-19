//! 7z reading against archives made by p7zip (`tests/data/gen_7z.py`), and by hand for the cases
//! p7zip cannot make (zstd, the RomVault footer, bad headers).

use std::io::Cursor;

use rv_archive::sevenzip::SevenZipArchive;
use rv_archive::zip::ZipStructure;
use rv_archive::ArchiveError;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR")))
        .unwrap_or_else(|e| panic!("cannot read fixture {name}: {e}"))
}

fn sample_text() -> Vec<u8> {
    (0..200)
        .map(|i| format!("line {i}: the quick brown fox jumps over the lazy dog\n"))
        .collect::<String>()
        .into_bytes()
}

/// The files `gen_7z.py` puts in every archive: name, content.
fn expected() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("a.txt", sample_text()),
        ("b.bin", (0..=255u8).cycle().take(256 * 40).collect()),
        ("dir/c.txt", vec![b'c'; 5000]),
        ("dir/sub/d.txt", b"deep file\n".to_vec()),
        ("empty.txt", Vec::new()),
    ]
}

fn open(name: &str) -> SevenZipArchive<Cursor<Vec<u8>>> {
    SevenZipArchive::open(Cursor::new(fixture(name))).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn check_contents(name: &str) {
    let mut z = open(name);
    let want = expected();
    // Directories are listed too (`emptydir/`, `dir/`, `dir/sub/`).
    let files: Vec<usize> = (0..z.len())
        .filter(|&i| !z.entries()[i].is_directory)
        .collect();
    assert_eq!(files.len(), want.len(), "{name}: file count");
    for (w_name, w_data) in &want {
        let i = z
            .entries()
            .iter()
            .position(|e| e.name == *w_name)
            .unwrap_or_else(|| panic!("{name}: {w_name} missing"));
        assert_eq!(
            z.entries()[i].size,
            w_data.len() as u64,
            "{name}: {w_name} size"
        );
        assert_eq!(z.read_entry(i).unwrap(), *w_data, "{name}: {w_name} data");
    }
    let dirs: Vec<_> = z
        .entries()
        .iter()
        .filter(|e| e.is_directory)
        .map(|e| e.name.clone())
        .collect();
    assert!(dirs.contains(&"emptydir/".to_string()), "{name}: {dirs:?}");
    // Streaming all entries once gives the same bytes, with every CRC checked.
    let mut seen = 0;
    z.for_each_entry(|e, r| {
        let mut buf = Vec::new();
        r.read_to_end(&mut buf)
            .map_err(rv_archive::into_archive_error)?;
        if let Some((_, d)) = want.iter().find(|(n, _)| *n == e.name) {
            assert_eq!(&buf, d, "{name}: {}", e.name);
            seen += 1;
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(seen, want.len());
    let total: u64 = want.iter().map(|(_, d)| d.len() as u64).sum();
    assert_eq!(z.verify_all().unwrap(), total);
}

#[test]
fn lzma2_solid() {
    check_contents("seven_lzma2_solid.7z");
    let z = open("seven_lzma2_solid.7z");
    assert_eq!(z.folder_info(0).unwrap().coders, ["LZMA2"]);
    assert_eq!(z.folders().len(), 1, "solid: one folder");
}

#[test]
fn lzma_non_solid_has_a_folder_per_file() {
    check_contents("seven_lzma_nonsolid.7z");
    let z = open("seven_lzma_nonsolid.7z");
    assert_eq!(z.folder_info(0).unwrap().coders, ["LZMA"]);
    assert_eq!(
        z.folders().len(),
        4,
        "four files with data, the empty one has none"
    );
}

#[test]
fn copy() {
    check_contents("seven_copy.7z");
    assert_eq!(
        open("seven_copy.7z").folder_info(0).unwrap().coders,
        ["Copy"]
    );
}

#[test]
fn ppmd() {
    check_contents("seven_ppmd.7z");
    assert_eq!(
        open("seven_ppmd.7z").folder_info(0).unwrap().coders,
        ["PPMd"]
    );
}

#[test]
fn bzip2() {
    check_contents("seven_bzip2.7z");
}

#[test]
fn deflate() {
    check_contents("seven_deflate.7z");
}

#[test]
fn bcj_filter_in_front_of_lzma() {
    check_contents("seven_bcj_lzma.7z");
    let z = open("seven_bcj_lzma.7z");
    let mut names = z.folder_info(0).unwrap().coders;
    names.sort();
    assert_eq!(names, ["BCJ", "LZMA"]);
}

#[test]
fn delta_filter_in_front_of_lzma2() {
    check_contents("seven_delta_lzma2.7z");
}

/// The 16 KiB file `gen_7z.py` stores behind each branch filter.
fn branches() -> Vec<u8> {
    fixture("branches.bin")
}

#[test]
fn branch_filters_and_delta_match_p7zip() {
    for (file, coders) in [
        ("seven_filter_bcj_lzma.7z", vec!["BCJ", "LZMA"]),
        ("seven_filter_bcj_lzma2.7z", vec!["BCJ", "LZMA2"]),
        ("seven_filter_arm_lzma.7z", vec!["ARM", "LZMA"]),
        ("seven_filter_armt_lzma.7z", vec!["ARMT", "LZMA"]),
        ("seven_filter_ppc_lzma.7z", vec!["PPC", "LZMA"]),
        ("seven_filter_sparc_lzma.7z", vec!["SPARC", "LZMA"]),
        ("seven_filter_ia64_lzma.7z", vec!["IA64", "LZMA"]),
        ("seven_filter_delta4_lzma2.7z", vec!["Delta", "LZMA2"]),
    ] {
        let mut z = open(file);
        let mut names = z.folder_info(0).unwrap().coders;
        names.sort();
        let mut want = coders.clone();
        want.sort();
        assert_eq!(names, want, "{file}");
        assert_eq!(z.len(), 1, "{file}");
        assert_eq!(z.read_entry(0).unwrap(), branches(), "{file}");
    }
}

#[test]
fn header_that_is_not_packed() {
    check_contents("seven_plain_header.7z");
}

#[test]
fn times_are_read_as_ticks() {
    let z = open("seven_with_times.7z");
    let e = z.entries().iter().find(|e| e.name == "a.txt").unwrap();
    // Any 2020s date: ticks since 0001-01-01 around 6.38e17 .. 6.5e17.
    for t in [e.modified_ticks, e.created_ticks, e.accessed_ticks] {
        let t = t.expect("time present");
        assert!(
            (637_000_000_000_000_000..650_000_000_000_000_000).contains(&t),
            "{t}"
        );
    }
    assert!(open("seven_lzma2_solid.7z").entries()[0]
        .modified_ticks
        .is_none());
}

#[test]
fn plain_archives_are_not_romvault_structured() {
    let z = open("seven_lzma2_solid.7z");
    assert_eq!(z.header_structure(), ZipStructure::None);
    assert_eq!(z.structure(), ZipStructure::None);
}

#[test]
fn a_flipped_packed_byte_is_a_crc_or_decode_error() {
    let mut raw = fixture("seven_copy.7z");
    raw[32 + 100] ^= 1;
    let mut z = SevenZipArchive::open(Cursor::new(raw)).unwrap();
    let err = z.verify_all().unwrap_err();
    assert!(matches!(err, ArchiveError::CrcMismatch { .. }), "{err}");
}

#[test]
fn a_damaged_header_is_an_error() {
    let raw = fixture("seven_lzma2_solid.7z");
    let mut bad_sig = raw.clone();
    bad_sig[0] = b'8';
    assert!(matches!(
        SevenZipArchive::open(Cursor::new(bad_sig)).err().unwrap(),
        ArchiveError::BadSignature { .. }
    ));
    let mut bad_start_crc = raw.clone();
    bad_start_crc[12] ^= 1;
    assert!(SevenZipArchive::open(Cursor::new(bad_start_crc)).is_err());
    let mut bad_header = raw.clone();
    let n = bad_header.len();
    bad_header[n - 5] ^= 1;
    assert!(SevenZipArchive::open(Cursor::new(bad_header)).is_err());
}

#[test]
fn every_truncation_is_an_error_not_a_panic() {
    for f in [
        "seven_lzma2_solid.7z",
        "seven_plain_header.7z",
        "seven_ppmd.7z",
    ] {
        let raw = fixture(f);
        for len in 0..raw.len() {
            assert!(
                SevenZipArchive::open(Cursor::new(raw[..len].to_vec())).is_err(),
                "{f} cut to {len} bytes opened"
            );
        }
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
    let mut rng = Rng(0x1234_5678_9abc_def1);
    for f in [
        "seven_lzma2_solid.7z",
        "seven_lzma_nonsolid.7z",
        "seven_plain_header.7z",
        "seven_ppmd.7z",
        "seven_bcj_lzma.7z",
        "seven_copy.7z",
    ] {
        let orig = fixture(f);
        for _ in 0..300 {
            let mut raw = orig.clone();
            for _ in 0..1 + rng.next() % 3 {
                let at = (rng.next() % raw.len() as u64) as usize;
                raw[at] ^= 1 << (rng.next() % 8);
            }
            if let Ok(mut z) = SevenZipArchive::open(Cursor::new(raw)) {
                let _ = z.verify_all();
            }
        }
    }
}

// ---- archives made by hand ---------------------------------------------------------------------

mod common;

use common::sevenz::{self as sz, build, build_romvault, dir, file, rvzstd, Coder, ZSTD_METHOD};

fn zstd_coder() -> Coder {
    Coder {
        method: ZSTD_METHOD.to_vec(),
        props: vec![1, 5, 19, 0, 0],
    }
}

#[test]
fn zstd_folder_with_empty_files_and_directories() {
    let a = b"first file, in a zstd stream";
    let b = b"second";
    let packed = rvzstd(&[a.as_slice(), b.as_slice()].concat());
    let files = [
        dir("d/"),
        file("d/a.txt", a),
        file("e.txt", b""),
        file("f.txt", b),
    ];
    let mut z = SevenZipArchive::open(Cursor::new(build(&files, &zstd_coder(), &packed))).unwrap();
    let names: Vec<_> = z
        .entries()
        .iter()
        .map(|e| (e.name.as_str(), e.is_directory, e.size))
        .collect();
    assert_eq!(
        names,
        [
            ("d/", true, 0),
            ("d/a.txt", false, 28),
            ("e.txt", false, 0),
            ("f.txt", false, 6)
        ]
    );
    assert_eq!(z.folder_info(0).unwrap().coders, ["ZSTD"]);
    assert_eq!(z.read_entry(1).unwrap(), a);
    assert_eq!(z.read_entry(2).unwrap(), b"");
    assert_eq!(z.read_entry(3).unwrap(), b);
    assert_eq!(z.verify_all().unwrap(), 34);
}

fn structured(digit: u8, names: &[&str]) -> Vec<u8> {
    let datas: Vec<Vec<u8>> = names
        .iter()
        .map(|n| format!("data of {n}").into_bytes())
        .collect();
    let files: Vec<_> = names.iter().zip(&datas).map(|(n, d)| file(n, d)).collect();
    let all: Vec<u8> = datas.concat();
    let (packed, props) = sz::lzma1(&all, 1 << 16);
    build_romvault(
        &files,
        &Coder {
            method: vec![3, 1, 1],
            props,
        },
        &packed,
        digit,
    )
}

#[test]
fn the_romvault_footer_is_recognised() {
    // Names in the 7z order: by extension, then name, then path.
    let z =
        SevenZipArchive::open(Cursor::new(structured(b'1', &["a.bin", "b.bin", "a.txt"]))).unwrap();
    assert_eq!(z.header_structure(), ZipStructure::SevenZipSLZMA);
    assert_eq!(z.structure(), ZipStructure::SevenZipSLZMA);
    for (digit, want) in [
        (b'2', ZipStructure::SevenZipNLZMA),
        (b'3', ZipStructure::SevenZipSZSTD),
        (b'4', ZipStructure::SevenZipNZSTD),
    ] {
        let z = SevenZipArchive::open(Cursor::new(structured(digit, &["a.bin"]))).unwrap();
        assert_eq!(z.header_structure(), want);
    }
    let mut z =
        SevenZipArchive::open(Cursor::new(structured(b'1', &["a.bin", "b.bin", "a.txt"]))).unwrap();
    assert_eq!(z.read_entry(2).unwrap(), b"data of a.txt");
}

#[test]
fn right_footer_wrong_files_is_announced_but_not_structured() {
    // Out of order, a backslash, and a directory entry that is not empty.
    for names in [
        vec!["b.bin", "a.bin"],
        vec!["a\\b.bin"],
        vec!["a.bin", "a.bin"],
    ] {
        let z = SevenZipArchive::open(Cursor::new(structured(b'1', &names))).unwrap();
        assert_eq!(
            z.header_structure(),
            ZipStructure::SevenZipSLZMA,
            "{names:?}"
        );
        assert_eq!(z.structure(), ZipStructure::None, "{names:?}");
    }
    let files = [dir("d/"), file("d/x.bin", b"x")];
    let (packed, props) = sz::lzma1(b"x", 1 << 16);
    let z = SevenZipArchive::open(Cursor::new(build_romvault(
        &files,
        &Coder {
            method: vec![3, 1, 1],
            props,
        },
        &packed,
        b'1',
    )))
    .unwrap();
    assert_eq!(z.header_structure(), ZipStructure::SevenZipSLZMA);
    assert_eq!(
        z.structure(),
        ZipStructure::None,
        "a directory entry with a file inside"
    );
}

#[test]
fn a_damaged_footer_is_not_a_romvault_archive() {
    let good = structured(b'1', &["a.bin"]);
    // The footer sits 32 bytes before the descriptor: find it and break each field in turn.
    let at = good.windows(11).position(|w| w == b"RomVault7Z0").unwrap();
    for off in [0usize, 5, 12, 16, 24] {
        let mut z = good.clone();
        z[at + off] ^= 1;
        let a = SevenZipArchive::open(Cursor::new(z)).unwrap();
        assert_eq!(
            a.header_structure(),
            ZipStructure::None,
            "footer byte {off}"
        );
    }
    let mut z = good;
    z[at + 11] = b'9';
    assert_eq!(
        SevenZipArchive::open(Cursor::new(z))
            .unwrap()
            .header_structure(),
        ZipStructure::None
    );
}

#[test]
fn rvzstd_7z_needs_the_zstd_trailer() {
    let data = b"payload";
    let good = rvzstd(data);
    let files = [file("a.bin", data)];
    let z = SevenZipArchive::open(Cursor::new(build_romvault(
        &files,
        &zstd_coder(),
        &good,
        b'3',
    )))
    .unwrap();
    assert_eq!(z.structure(), ZipStructure::SevenZipSZSTD);
    // The same frame without the empty last block does not end in `01 00 00`.
    let mut cut = good.clone();
    cut.truncate(good.len() - 3);
    let last = cut.len();
    cut[last - 1] |= 0;
    let z = SevenZipArchive::open(Cursor::new(build_romvault(
        &files,
        &zstd_coder(),
        &cut,
        b'3',
    )))
    .unwrap();
    assert_eq!(z.header_structure(), ZipStructure::SevenZipSZSTD);
    assert_eq!(z.structure(), ZipStructure::None);
}

#[test]
fn bcj2_and_aes_are_typed_unsupported_errors() {
    // BCJ2 folders have four input streams: the archive lists, the data does not decode.
    let mut z = open("seven_bcj2.7z");
    assert!(z.entries().iter().any(|e| e.name == "a.txt"));
    let i = z.entries().iter().position(|e| e.name == "a.txt").unwrap();
    assert!(matches!(
        z.read_entry(i).unwrap_err(),
        ArchiveError::UnsupportedCompression { .. }
    ));
    // AES with a plain header: listing works, extraction is refused.
    let mut z = open("seven_aes.7z");
    let i = z.entries().iter().position(|e| e.name == "a.txt").unwrap();
    assert!(matches!(
        z.read_entry(i).unwrap_err(),
        ArchiveError::UnsupportedCompression { .. }
    ));
    // AES with an encrypted header: the archive cannot even be listed.
    let err = SevenZipArchive::open(Cursor::new(fixture("seven_aes_header.7z")))
        .err()
        .unwrap();
    assert!(
        matches!(err, ArchiveError::UnsupportedCompression { .. }),
        "{err}"
    );
}
