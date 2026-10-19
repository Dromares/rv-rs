//! Write round trips on the synthetic fixtures: `read(write(m)) == project(m)` for every
//! writer ROMVault has. The projections in `common.rs` list what each format cannot hold.

mod common;

use rv_dat::{read_dat, read_dat_report, write_dos, write_mame_xml, write_xml, DatHeader};

fn data(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn read(name: &str) -> DatHeader {
    read_dat(&data(name), name).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn xml_round_trip(m: &DatHeader, new_style: bool) -> DatHeader {
    let text = write_xml(m, new_style);
    read_dat(text.as_bytes(), &m.filename)
        .unwrap_or_else(|e| panic!("re-read failed: {e}\n{}", &text[..text.len().min(3000)]))
}

#[test]
fn xml_writer_round_trips_every_reader_output() {
    // Models from every reader go through the XML writer.
    for name in [
        "xml_full.xml",
        "cmp_full.dat",
        "mame_full.xml",
        "softlist_full.xml",
        "dos_full.dat",
        "romcenter_full.dat",
        "raine.dat",
    ] {
        let m = read(name);
        let back = xml_round_trip(&m, false);
        if let Some(d) = common::diff(&back, &common::xml_header(&m)) {
            panic!("{name}: {d}");
        }
        // A second trip is stable.
        let again = xml_round_trip(&back, false);
        assert_eq!(again, back, "{name}: second trip");
    }
}

#[test]
fn xml_writer_output_is_stable_text() {
    let m = read("xml_full.xml");
    let once = write_xml(&m, false);
    let twice = write_xml(&xml_round_trip(&m, false), false);
    assert_eq!(once, twice, "write(read(write(m))) == write(m)");
}

#[test]
fn xml_writer_layout() {
    let m = read("xml_full.xml");
    let text = write_xml(&m, false);
    assert!(text.starts_with("<?xml version=\"1.0\"?>\n<datafile>\n\t<header>\n\t\t<id>45</id>\n"));
    assert!(text.contains("\t\t<romvault header=\"No-Intro_NES.xml\" forcepacking=\"zip\"/>\n"));
    assert!(text.ends_with("\t</dir>\n</datafile>\n") || text.ends_with("</datafile>\n"));
    // Defaults are not written, non-defaults are.
    assert!(text.contains("isbios=\"yes\""));
    assert!(!text.contains("isdevice=\"no\""));
    assert!(text.contains("runnable=\"no\""));
    assert!(text.contains("mia=\"yes\""));
    assert!(text.contains("status=\"nodump\""));
    assert!(!text.contains("status=\"good\""));
    // Hashes are lower-case hex.
    assert!(text.contains("crc=\"abcd1234\""));
}

#[test]
fn xml_writer_escapes_and_stays_well_formed() {
    use rv_dat::{DatDir, DatFile, DatGame, DatNode};
    let mut h = DatHeader {
        name: "A & B <\"q\"> 'a'".into(),
        description: "line1\nline2\ttab\u{1}ctl".into(),
        ..DatHeader::default()
    };
    let mut g = DatDir::new("game \"q\" & <x>\nnl", rv_core::FileType::UNSET);
    g.game = Some(Box::new(DatGame {
        description: "d\r\n".replace('\r', ""),
        device_ref: vec!["dev & <1>".into()],
        ..DatGame::default()
    }));
    g.children
        .push(DatNode::File(DatFile::new("f \"1\" & <2>.bin")));
    h.base_dir.children.push(DatNode::Dir(g));
    let text = write_xml(&h, false);
    // 3.8.0 wrote `&#0A;` for a line feed, which is not a character reference.
    assert!(text.contains("&#x0A;"));
    assert!(!text.contains("&#0A;"));
    let back = read_dat(text.as_bytes(), "").expect("well-formed");
    assert_eq!(back.name, h.name);
    assert_eq!(back.description, h.description);
    let rg = match &back.base_dir.children[0] {
        rv_dat::DatNode::Dir(d) => d,
        _ => panic!(),
    };
    assert_eq!(rg.base.name, "game \"q\" & <x>\nnl");
    assert_eq!(rg.game.as_ref().unwrap().device_ref, ["dev & <1>"]);
}

#[test]
fn xml_new_style_is_well_formed_but_not_read_back_by_the_reader() {
    // `newStyle` (RVDatFile) is never used by the application. Its `<set>` and `<file>`
    // elements are not among the ones `DatXmlReader.ReadDat` selects (`dir`, `game`,
    // `machine`, `rom`, `disk`), so 3.8.0 cannot read what its own new-style writer writes.
    let m = read("xml_full.xml");
    let text = write_xml(&m, true);
    assert!(
        text.contains("<RVDatFile>") && text.contains("<set name=") && text.contains("<file name=")
    );
    let out = read_dat_report(text.as_bytes(), "t");
    assert!(out.ok, "the header is read: {:?}", out.messages);
    let hd = out.header.unwrap();
    assert_eq!(hd.name, m.name);
    // Only plain `<dir>` elements come back; the games (`<set>`) do not.
    assert_eq!(hd.base_dir.game_count(), 0);
    assert!(m.base_dir.game_count() > 0);
}

#[test]
fn mame_writer_round_trips() {
    for name in [
        "mame_full.xml",
        "xml_full.xml",
        "cmp_full.dat",
        "softlist_full.xml",
    ] {
        let m = read(name);
        let text = write_mame_xml(&m);
        let back = read_dat(text.as_bytes(), &m.filename)
            .unwrap_or_else(|e| panic!("{name}: {e}\n{}", &text[..text.len().min(3000)]));
        if let Some(d) = common::diff(&back, &common::mame_header(&m)) {
            panic!("{name}: {d}");
        }
        // Stable on the second trip.
        assert_eq!(write_mame_xml(&back), write_mame_xml(&back));
        let again = read_dat(write_mame_xml(&back).as_bytes(), &m.filename).unwrap();
        assert_eq!(again, back, "{name}: second trip");
    }
}

#[test]
fn mame_writer_layout() {
    let m = read("mame_full.xml");
    let text = write_mame_xml(&m);
    assert!(text.starts_with("<?xml version=\"1.0\"?>\n<!DOCTYPE mame ["));
    assert!(text.contains(
        "]>\n\n\n<mame build=\"0.250 (mame0250)\" version=\"0.250\" date=\"2022-01-01\">\n"
    ));
    assert!(text
        .contains("\n<machine name=\"parent\" isbios=\"yes\" isdevice=\"yes\" runnable=\"no\">\n"));
    assert!(text.contains("\t<rom name=\"p1.bin\" merge=\"m.bin\" size=\"256\" crc=\"01234567\" sha1=\"0123456789abcdef0123456789abcdef01234567\"/>\n"));
    assert!(text.contains("\t<device_ref name=\"devA\"/>\n"));
    assert!(text.ends_with("</mame>\n"));
}

#[test]
fn dos_writer_round_trips() {
    let m = read("dos_full.dat");
    let text = write_dos(&m);
    let back = read_dat(text.as_bytes(), &m.filename).unwrap_or_else(|e| panic!("{e}\n{text}"));
    if let Some(d) = common::diff(&back, &common::dos_header(&m)) {
        panic!("{d}");
    }
    let again = read_dat(write_dos(&back).as_bytes(), &m.filename).unwrap();
    assert_eq!(again, back);
}

#[test]
fn dos_writer_layout() {
    let m = read("dos_full.dat");
    let text = write_dos(&m);
    assert!(
        text.starts_with("DOSCenter (\n\tname: Test DOS Set\n\tdescription: A DOS description\n")
    );
    assert!(text.contains("\n\n"));
    assert!(text.contains("game (\n\tname \"First Game.zip\"\n"));
    assert!(text.contains(
        "\tfile ( name FIRST.EXE size 1234 date 1996/12/24 23:32:00 crc 0a0b0c0d sha1 0123456789abcdef0123456789abcdef01234567 )\n"
    ));
    assert!(text.contains("\tfile ( name nodate.bin size 7 crc 00000007 )\n"));
}

#[test]
fn dos_writer_gaps_are_real() {
    use rv_dat::{DatDir, DatFile, DatGame, DatNode};
    // md5 and sha256 are written, and the reader has no keyword for them.
    let mut h = DatHeader::default();
    let mut g = DatDir::new("g", rv_core::FileType::UNSET);
    g.game = Some(Box::<DatGame>::default());
    let mut f = DatFile::new("a.bin");
    f.size = Some(1);
    f.md5 = Some(vec![0; 16]);
    g.children.push(DatNode::File(f));
    h.base_dir.children.push(DatNode::Dir(g));
    let text = write_dos(&h);
    assert!(text.contains(" md5 "));
    let out = read_dat_report(text.as_bytes(), "t");
    assert!(
        out.messages.iter().any(|m| m.contains("'md5'")),
        "{:?}",
        out.messages
    );
    // A game outside the first level... directories that are not games are skipped.
    let mut h = DatHeader::default();
    let mut d = DatDir::new("plain", rv_core::FileType::UNSET);
    let mut g = DatDir::new("g", rv_core::FileType::UNSET);
    g.game = Some(Box::default());
    d.children.push(DatNode::Dir(g));
    h.base_dir.children.push(DatNode::Dir(d));
    assert!(!write_dos(&h).contains("game ("));
}

#[test]
fn formats_without_a_romvault_writer() {
    // Documented gaps: ROMVault 3.8.0 has no writer for ClrMamePro text, RomCenter or software
    // lists, and `DatJSONWriter.cs` is inside a `/* ... */` block with no reader at all. There
    // is nothing to round trip, so the readers are covered by `readers.rs` alone.
    // This test only keeps the list in the test report.
    let gaps = [
        "clrmamepro text writer",
        "romcenter writer",
        "softwarelist writer",
        "json reader and writer",
    ];
    assert_eq!(gaps.len(), 4);
}
