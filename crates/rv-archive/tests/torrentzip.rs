//! The TorrentZip writer and the structure checks, on synthetic data. The byte-exactness against
//! real ROMVault output is `tests/fixtures.rs`; here are the layout, the rules and the negative
//! controls (a valid zip that is not TorrentZip must be seen as such).

mod common;

use std::io::Cursor;

use common::*;
use rv_archive::zip::{
    ticks_to_dos, TorrentZipWriter, ZipArchive, ZipStructure, TRRNTZIP_DATE_TIME_TICKS,
    TRRNTZIP_DOS_DATE_TIME,
};
use rv_archive::{ArchiveError, AtomicFile, StructureError};

fn write_tz(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut w = TorrentZipWriter::new(Cursor::new(Vec::new()), ZipStructure::ZipTrrnt).unwrap();
    for (name, data) in entries {
        w.add_bytes(name, data, None).unwrap();
    }
    w.finish().unwrap().into_inner()
}

fn open(z: Vec<u8>) -> ZipArchive<Cursor<Vec<u8>>> {
    ZipArchive::open(Cursor::new(z)).unwrap()
}

#[test]
fn single_entry_layout_is_byte_exact() {
    let data = b"hello hello hello hello";
    let z = write_tz(&[("a.txt", data)]);
    let crc = crc32fast::hash(data);
    let payload = hex_to_bytes("cb48cdc9c957c8402701"); // deflate level 9, same as stock zlib here

    let mut want = Vec::new();
    // Local header (`ZipFileData.cs:309-352`, `:354-393`).
    want.extend(b"PK\x03\x04");
    want.extend([20, 0]); // version needed
    want.extend([2, 0]); // flags: "maximum compression"
    want.extend([8, 0]); // deflate
    want.extend([0x00, 0xBC]); // time 23:32:00
    want.extend([0x98, 0x21]); // date 1996-12-24
    want.extend(crc.to_le_bytes());
    want.extend((payload.len() as u32).to_le_bytes());
    want.extend((data.len() as u32).to_le_bytes());
    want.extend([5, 0, 0, 0]);
    want.extend(b"a.txt");
    want.extend(&payload);
    // Central directory (`ZipFileData.cs:175-218`).
    let cd_start = want.len();
    let mut cd = Vec::new();
    cd.extend(b"PK\x01\x02");
    cd.extend([0, 0]); // version made by is 0
    cd.extend([20, 0]);
    cd.extend([2, 0]);
    cd.extend([8, 0]);
    cd.extend([0x00, 0xBC, 0x98, 0x21]);
    cd.extend(crc.to_le_bytes());
    cd.extend((payload.len() as u32).to_le_bytes());
    cd.extend((data.len() as u32).to_le_bytes());
    cd.extend([5, 0]); // name length
    cd.extend([0; 2 + 2 + 2 + 2 + 4]); // extra, comment, disk, internal, external attributes
    cd.extend([0; 4]); // local header offset
    cd.extend(b"a.txt");
    want.extend(&cd);
    let comment = format!("TORRENTZIPPED-{:08X}", crc32fast::hash(&cd));
    want.extend(b"PK\x05\x06");
    want.extend([0; 4]);
    want.extend([1, 0, 1, 0]);
    want.extend((cd.len() as u32).to_le_bytes());
    want.extend((cd_start as u32).to_le_bytes());
    want.extend((comment.len() as u16).to_le_bytes());
    want.extend(comment.as_bytes());
    assert_eq!(z, want);
}

fn hex_to_bytes(h: &str) -> Vec<u8> {
    (0..h.len() / 2)
        .map(|i| u8::from_str_radix(&h[2 * i..2 * i + 2], 16).unwrap())
        .collect()
}

#[test]
fn writer_output_is_recognised_as_torrentzip() {
    let z = write_tz(&[
        ("a.bin", &sample_text()),
        ("B.txt", b"b"),
        ("caf\u{e9}.txt", b"cp437 name"),
        ("empty.txt", b""),
        ("zdir/", b""),
        ("\u{3042}.txt", b"utf-8 name"),
    ]);
    let mut zip = open(z);
    assert_eq!(zip.structure(), ZipStructure::ZipTrrnt);
    assert_eq!(zip.header_structure(), ZipStructure::ZipTrrnt);
    let crc = zip.central_directory_crc().unwrap();
    assert_eq!(zip.comment(), format!("TORRENTZIPPED-{crc:08X}"));
    let names: Vec<_> = zip.entries().iter().map(|e| e.name.clone()).collect();
    assert_eq!(
        names,
        [
            "a.bin",
            "B.txt",
            "caf\u{e9}.txt",
            "empty.txt",
            "zdir/",
            "\u{3042}.txt"
        ]
    );
    for e in zip.entries() {
        assert_eq!(e.method, 8, "{}", e.name);
        assert_eq!(e.version_made_by, 0);
        assert_eq!(e.dos_date_time, TRRNTZIP_DOS_DATE_TIME);
        assert!(e.status.is_empty());
        assert!(!e.extra_data_found);
    }
    assert_eq!(zip.entries()[2].flags, 2, "cafe is code page 437");
    assert_eq!(
        zip.entries()[5].flags,
        0x802,
        "hiragana needs the UTF-8 flag"
    );
    for i in [3usize, 4] {
        assert_eq!(zip.entries()[i].compressed_size, 2);
        let mut raw = Vec::new();
        std::io::Read::read_to_end(&mut zip.open_raw(i).unwrap(), &mut raw).unwrap();
        assert_eq!(raw, [3, 0]);
    }
    assert!(zip.entries()[4].is_directory);
    assert_eq!(zip.read_entry(0).unwrap(), sample_text());
    assert_eq!(zip.read_entry(5).unwrap(), b"utf-8 name");
}

#[test]
fn rewriting_a_torrentzip_gives_the_same_bytes() {
    let z = write_tz(&[("a", &sample_text()), ("b/", b""), ("c", b"x")]);
    let mut zip = open(z.clone());
    let mut w = TorrentZipWriter::new(Cursor::new(Vec::new()), ZipStructure::ZipTrrnt).unwrap();
    for i in 0..zip.len() {
        let (name, size) = {
            let e = &zip.entries()[i];
            (e.name.clone(), e.uncompressed_size)
        };
        let mut r = zip.open_entry(i).unwrap();
        w.add_file(&name, size, &mut r, None, None).unwrap();
    }
    assert_eq!(w.finish().unwrap().into_inner(), z);
}

#[test]
fn many_entries_use_the_zip64_end_records() {
    let names: Vec<String> = (0..65_536).map(|i| format!("{i:06}")).collect();
    let mut w = TorrentZipWriter::new(Cursor::new(Vec::new()), ZipStructure::ZipTrrnt).unwrap();
    for n in &names {
        w.add_bytes(n, b"", None).unwrap();
    }
    let z = w.finish().unwrap().into_inner();
    let zip = open(z);
    assert!(zip.is_zip64());
    assert_eq!(zip.len(), 65_536);
    assert_eq!(zip.structure(), ZipStructure::ZipTrrnt);
}

#[test]
fn tdc_and_dtd_keep_the_callers_timestamp() {
    for (s, id) in [
        (ZipStructure::ZipTDC, "TDC-"),
        (ZipStructure::ZipDTD, "DTD-"),
    ] {
        let mut w = TorrentZipWriter::new(Cursor::new(Vec::new()), s).unwrap();
        w.add_bytes("a", b"x", Some(630_873_792_000_000_000))
            .unwrap(); // 2000-02-29 in ticks
        w.add_bytes("b", b"y", Some(0x2198_BC00)).unwrap(); // already packed
        let zip = open(w.finish().unwrap().into_inner());
        assert_eq!(zip.structure(), s);
        assert!(zip.comment().starts_with(id));
        assert_eq!(
            zip.entries()[0].dos_date_time,
            ((29 | (2 << 5) | (20 << 9)) << 16) as i64
        );
        assert_eq!(zip.entries()[1].dos_date_time, 0x2198_BC00);
    }
}

#[test]
fn the_tick_and_packed_forms_of_the_torrentzip_date_agree() {
    assert_eq!(
        ticks_to_dos(TRRNTZIP_DATE_TIME_TICKS),
        ticks_to_dos(TRRNTZIP_DOS_DATE_TIME)
    );
}

// ---- writer refusals -------------------------------------------------------------------------

fn writer() -> TorrentZipWriter<Cursor<Vec<u8>>> {
    TorrentZipWriter::new(Cursor::new(Vec::new()), ZipStructure::ZipTrrnt).unwrap()
}

fn structure_err(r: Result<rv_archive::zip::AddedEntry, ArchiveError>) -> StructureError {
    match r.unwrap_err() {
        ArchiveError::Structure(e) => e,
        e => panic!("{e}"),
    }
}

#[test]
fn the_writer_enforces_the_structure_rules() {
    let mut w = writer();
    assert!(matches!(
        structure_err(w.add_bytes("a\\b", b"x", None)),
        StructureError::BackslashInName(_)
    ));
    assert!(matches!(
        structure_err(w.add_bytes("dir/", b"x", None)),
        StructureError::DirectoryWithData(_)
    ));
    w.add_bytes("b", b"x", None).unwrap();
    assert!(matches!(
        structure_err(w.add_bytes("a", b"x", None)),
        StructureError::OutOfOrder { .. }
    ));
    // Equal ignoring ASCII case: the writer's tie-break is ordinal, so "B" (0x42) after "b" is out
    // of order but "b" after "B" is not.
    assert!(matches!(
        structure_err(w.add_bytes("B", b"x", None)),
        StructureError::OutOfOrder { .. }
    ));
    let mut w = writer();
    w.add_bytes("B", b"x", None).unwrap();
    w.add_bytes("b", b"x", None).unwrap();
    // The verifier does not accept the tie.
    let zip = open(w.finish().unwrap().into_inner());
    assert_eq!(zip.header_structure(), ZipStructure::ZipTrrnt);
    assert_eq!(zip.structure(), ZipStructure::None);
}

#[test]
fn a_directory_entry_must_stay_empty() {
    let mut w = writer();
    w.add_bytes("dir/", b"", None).unwrap();
    assert!(matches!(
        structure_err(w.add_bytes("dir/file", b"x", None)),
        StructureError::DirectoryNotEmpty { .. }
    ));
    // A sibling after the directory is fine.
    w.add_bytes("dir2", b"x", None).unwrap();
}

#[test]
fn declared_size_and_crc_are_checked() {
    let mut w = writer();
    let mut short: &[u8] = b"abc";
    assert!(matches!(
        structure_err(w.add_file("a", 10, &mut short, None, None)),
        StructureError::DataMismatch(_)
    ));
    let mut long: &[u8] = b"abcdef";
    assert!(matches!(
        structure_err(w.add_file("b", 3, &mut long, None, None)),
        StructureError::DataMismatch(_)
    ));
    let mut data: &[u8] = b"abc";
    assert!(matches!(
        structure_err(w.add_file("c", 3, &mut data, None, Some(1))),
        StructureError::DataMismatch(_)
    ));
}

#[test]
fn structures_without_a_writer_are_refused() {
    for s in [
        ZipStructure::None,
        ZipStructure::ZipZSTD,
        ZipStructure::SevenZipSLZMA,
    ] {
        assert!(matches!(
            TorrentZipWriter::new(Cursor::new(Vec::new()), s)
                .err()
                .unwrap(),
            ArchiveError::Structure(StructureError::UnsupportedStructure(_))
        ));
    }
}

// ---- negative controls: valid zips that are not TorrentZip -----------------------------------

fn structure_of(entries: &[RawEntry], comment: Comment) -> (ZipStructure, ZipStructure) {
    let zip = open(build(entries, comment));
    (zip.header_structure(), zip.structure())
}

#[test]
fn positive_control_the_builder_makes_a_torrentzip() {
    let e = [tz_entry("a", b"data"), tz_entry("b", b"")];
    assert_eq!(
        structure_of(&e, Comment::TorrentZip),
        (ZipStructure::ZipTrrnt, ZipStructure::ZipTrrnt)
    );
}

#[test]
fn a_foreign_comment_is_not_torrentzip() {
    let e = [tz_entry("a", b"data")];
    for c in [
        Comment::None,
        Comment::Text("made by something else"),
        Comment::TorrentZipWrongCrc,
        Comment::TorrentZipLower,
        Comment::Text("TORRENTZIPPED-"),
    ] {
        assert_eq!(
            structure_of(&e, c),
            (ZipStructure::None, ZipStructure::None)
        );
    }
}

#[test]
fn right_comment_wrong_files_is_not_torrentzip() {
    let h = (ZipStructure::ZipTrrnt, ZipStructure::None);
    // Members out of order.
    assert_eq!(
        structure_of(
            &[tz_entry("b", b"1"), tz_entry("a", b"2")],
            Comment::TorrentZip
        ),
        h
    );
    // Names equal ignoring ASCII case.
    assert_eq!(
        structure_of(
            &[tz_entry("A", b"1"), tz_entry("a", b"2")],
            Comment::TorrentZip
        ),
        h
    );
    // Not deflated.
    let mut stored = tz_entry("a", b"data");
    stored.method = 0;
    stored.payload = b"data".to_vec();
    assert_eq!(structure_of(&[stored], Comment::TorrentZip), h);
    // The wrong date.
    let mut dated = tz_entry("a", b"data");
    dated.dos_date = 0x2199;
    assert_eq!(structure_of(&[dated], Comment::TorrentZip), h);
    // Local and central general purpose flag words that differ. (The verifier does not require
    // the value 2 itself, only that bit 11 matches the name and the two words agree,
    // `StructuredZip.cs:237-281`.)
    let mut z = build(&[tz_entry("a", b"data")], Comment::TorrentZip);
    z[6] = 0;
    let zip = open(z);
    assert_eq!((zip.header_structure(), zip.structure()), h);
    let mut zero_flags = tz_entry("a", b"data");
    zero_flags.flags = 0;
    assert_eq!(structure_of(&[zero_flags], Comment::TorrentZip), (h.0, h.0));
    // An extra field.
    let mut extra = tz_entry("a", b"data");
    extra.extra = vec![0x55, 0x54, 5, 0, 1, 0, 0, 0, 0];
    assert_eq!(structure_of(&[extra], Comment::TorrentZip), h);
    // A backslash.
    assert_eq!(
        structure_of(&[tz_entry("a\\b", b"1")], Comment::TorrentZip),
        h
    );
    // A directory entry followed by a name inside it.
    assert_eq!(
        structure_of(
            &[tz_entry("d/", b""), tz_entry("d/f", b"1")],
            Comment::TorrentZip
        ),
        h
    );
    // The UTF-8 flag on a code page 437 name.
    let mut f = tz_entry("a", b"data");
    f.flags = 0x802;
    assert_eq!(structure_of(&[f], Comment::TorrentZip), h);
    // An empty file stored as a deflate stream other than `03 00`.
    let mut empty = tz_entry("a", b"");
    empty.payload = vec![0x01, 0x00, 0x00, 0xff, 0xff];
    assert_eq!(structure_of(&[empty], Comment::TorrentZip), h);
}

#[test]
fn a_prefix_or_trailing_data_is_not_torrentzip() {
    let z = write_tz(&[("a", b"x")]);
    let mut with_prefix = b"stub".to_vec();
    with_prefix.extend(&z);
    let zip = open(with_prefix);
    assert_eq!(zip.header_structure(), ZipStructure::ZipTrrnt);
    assert_eq!(zip.structure(), ZipStructure::None);
    let mut with_tail = z;
    with_tail.extend(b"tail");
    let zip = open(with_tail);
    assert_eq!(zip.structure(), ZipStructure::None);
}

#[test]
fn truncated_torrentzips_are_errors() {
    let z = write_tz(&[("a", &sample_text()), ("b", b"")]);
    for len in 0..z.len() {
        assert!(
            ZipArchive::open(Cursor::new(z[..len].to_vec())).is_err(),
            "{len}"
        );
    }
}

// ---- the fixer bug is not carried over -------------------------------------------------------

fn tmp_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("rv-archive-tz-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn a_failed_write_leaves_the_original_zip_alone() {
    let d = tmp_dir("fail");
    let target = d.join("game.zip");
    let original = write_tz(&[("a", b"original")]);
    std::fs::write(&target, &original).unwrap();

    // The new zip fails part-way (a name out of order): nothing may change.
    let mut w = TorrentZipWriter::new(AtomicFile::create(&target).unwrap(), ZipStructure::ZipTrrnt)
        .unwrap();
    w.add_bytes("b", b"new", None).unwrap();
    assert!(w.add_bytes("a", b"new", None).is_err());
    drop(w);
    assert_eq!(std::fs::read(&target).unwrap(), original);
    assert!(
        !d.join("__game.zip.rvtmp").exists(),
        "temp file is cleaned up"
    );

    // A good write replaces it only at commit.
    let mut w = TorrentZipWriter::new(AtomicFile::create(&target).unwrap(), ZipStructure::ZipTrrnt)
        .unwrap();
    w.add_bytes("a", b"replacement", None).unwrap();
    let f = w.finish().unwrap();
    assert_eq!(
        std::fs::read(&target).unwrap(),
        original,
        "still the old file before commit"
    );
    f.commit().unwrap();
    let mut zip = ZipArchive::open(std::fs::File::open(&target).unwrap()).unwrap();
    assert_eq!(zip.read_entry(0).unwrap(), b"replacement");
    assert_eq!(zip.structure(), ZipStructure::ZipTrrnt);
    std::fs::remove_dir_all(&d).unwrap();
}
