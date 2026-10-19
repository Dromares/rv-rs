//! Synthetic-fixture tests of the five readers. Every keyword, element and attribute a reader
//! handles appears in a fixture under `tests/data/`, and the assertions name each one. No
//! board data is used.

use rv_core::{FileType, HeaderFileType};
use rv_dat::{read_dat, read_dat_report, DatDir, DatFile, DatHeader, DatNode};

fn data(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn read(name: &str) -> DatHeader {
    read_dat(&data(name), name).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn dir<'a>(d: &'a DatDir, name: &str) -> &'a DatDir {
    d.children
        .iter()
        .find_map(|c| match c {
            DatNode::Dir(x) if x.base.name == name => Some(x),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no dir {name:?} in {:?}", names(d)))
}

fn file<'a>(d: &'a DatDir, name: &str) -> &'a DatFile {
    d.children
        .iter()
        .find_map(|c| match c {
            DatNode::File(x) if x.base.name == name => Some(x),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no file {name:?} in {:?}", names(d)))
}

fn names(d: &DatDir) -> Vec<&str> {
    d.children.iter().map(DatNode::name).collect()
}

fn h(s: &str) -> Option<Vec<u8>> {
    Some(
        (0..s.len() / 2)
            .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap())
            .collect(),
    )
}

// ---------------------------------------------------------------------------------------
// ClrMamePro
// ---------------------------------------------------------------------------------------

#[test]
fn cmp_header_keys() {
    let out = read_dat_report(&data("cmp_full.dat"), "cmp_full.dat");
    assert!(out.ok, "{:?}", out.messages);
    let hd = out.header.unwrap();
    assert_eq!(hd.name, "Test Set");
    assert_eq!(hd.description, "Test Set Description");
    assert_eq!(hd.category, "Testing");
    assert_eq!(hd.version, "1.0");
    assert_eq!(hd.date, "2020-01-02");
    assert_eq!(hd.author, "Someone Else");
    assert_eq!(hd.email, "a@example.org");
    assert_eq!(hd.homepage, "home page");
    // `//` cuts a line even inside a quote or token (`DatFileLoader.cs:67-71`).
    assert_eq!(hd.url, "http:");
    assert_eq!(hd.comment, "a comment");
    assert_eq!(hd.header, "nes.xml");
    assert_eq!(hd.compression, "zip"); // forcezipping
    assert_eq!(hd.merge_type, "split");
    assert_eq!(hd.no_dump, "obsolete");
    assert_eq!(hd.dir, "noautodir");
    assert!(!hd.mame_xml);
    assert_eq!(hd.filename, "cmp_full.dat");
    // The one unknown header key and the trailing unknown block are reported, not fatal.
    assert!(
        out.messages.iter().any(|m| m.contains("'unknownkey'")),
        "{:?}",
        out.messages
    );
    assert!(
        out.messages.iter().any(|m| m.contains("unknownblock")),
        "{:?}",
        out.messages
    );
}

#[test]
fn cmp_forcepacking_is_compression_too() {
    let hd = read_dat(
        b"clrmamepro (\n\tname x\n\tforcepacking unzip\n)\ngame ( name g rom ( name a size 1 crc 1 ) )\n",
        "t",
    )
    .unwrap();
    assert_eq!(hd.compression, "unzip");
}

#[test]
fn cmp_dirs_and_games() {
    let hd = read("cmp_full.dat");
    let root = &hd.base_dir;
    assert_eq!(
        names(root),
        [
            "sub",
            "Game One",
            "bios res",
            "Game Two",
            "pre/path/Rebuilt"
        ]
    );
    // `dir` is a plain directory (no game); `game`, `machine`, `resource` make games.
    let sub = dir(root, "sub");
    assert!(sub.game.is_none());
    let gd = dir(sub, "Game In Dir");
    assert_eq!(gd.game.as_ref().unwrap().description, "Dir Game");
    assert_eq!(file(gd, "d.bin").size, Some(4));
    let deeper = dir(sub, "deeper");
    assert!(deeper.game.is_none());
    assert!(dir(deeper, "deepgame").game.is_some(), "machine");
    assert!(dir(root, "bios res").game.is_some(), "resource");
    assert_eq!(
        dir(root, "bios res").game.as_ref().unwrap().description,
        "A BIOS"
    );
    // The first `name` wins, a second one is ignored.
    assert_eq!(names(dir(root, "Game Two")), ["b.bin"]);
    // `rebuildto` before `name` prefixes the path (`DatCmpReader.cs:262-277`).
    assert!(dir(root, "pre/path/Rebuilt").game.is_some());
    // Everything starts as `UnSet`, in DAT order.
    assert_eq!(root.base.file_type, FileType::UNSET);
}

#[test]
fn cmp_game_fields() {
    let hd = read("cmp_full.dat");
    let g = dir(&hd.base_dir, "Game One");
    let gm = g.game.as_ref().unwrap();
    assert_eq!(gm.description, "The Description"); // text between the first pair of quotes
    assert_eq!(gm.year, "1999");
    assert_eq!(gm.manufacturer, "ACME Corp");
    assert_eq!(gm.rom_of, "parentgame");
    assert_eq!(gm.clone_of, "parentgame");
    assert_eq!(gm.sample_of, "samples");
    assert_eq!(gm.source_file, "src.c");
    assert_eq!(gm.board, "brd1");
    assert_eq!(gm.history, "some history");
    assert_eq!(gm.is_device, "yes");
    // `isbios` is not parsed from ClrMamePro text (`DatCmpReader.cs:284-398`).
    assert_eq!(gm.is_bios, "");
    // Ignored keys leave no trace.
    assert_eq!(gm.serial, "");
    assert_eq!(gm.version, "");
}

#[test]
fn cmp_roms() {
    let hd = read("cmp_full.dat");
    let g = dir(&hd.base_dir, "Game One");

    let a = file(g, "a.bin");
    assert_eq!(a.size, Some(16));
    assert_eq!(a.crc, h("abcd1234"));
    assert_eq!(a.sha1, h("0123456789abcdef0123456789abcdef01234567"));
    assert_eq!(a.md5, h("0123456789abcdef0123456789abcdef"));
    assert_eq!(a.merge, "m.bin");
    assert_eq!(a.status, "good", "flags are lower-cased into Status");
    assert!(!a.is_disk);

    let s = file(g, "with space.bin");
    assert_eq!(s.size, Some(32), "0x hex size");
    assert_eq!(s.crc, h("00000001"), "crc32 keyword, zero padded");
    assert_eq!(s.status, "");

    // `regiona` skips to the closing parenthesis (`:463-467`).
    let r = file(g, "regioned.bin");
    assert_eq!(r.size, Some(1));

    assert_eq!(file(g, "nd.bin").status, "nodump");
    assert_eq!(file(g, "bd.bin").status, "baddump");
    // `flags` only sets Status when it is still empty.
    assert_eq!(file(g, "flagged.bin").status, "baddump");

    // The other rom keywords all produce files, and `badrom` sets no status (`:370`).
    for (n, size) in [
        ("badrom.bin", 9),
        ("unused.bin", 10),
        ("znbios.bin", 11),
        ("s16snd.bin", 12),
        ("s16gfx.bin", 13),
        ("neosnd.bin", 14),
    ] {
        let f = file(g, n);
        assert_eq!(f.size, Some(size), "{n}");
        assert_eq!(f.status, "", "{n}");
    }
}

#[test]
fn cmp_disks() {
    let hd = read("cmp_full.dat");
    let g = dir(&hd.base_dir, "Game One");
    let d = file(g, "Disk1"); // `.chd` is stripped
    assert!(d.is_disk);
    assert_eq!(d.sha1, h("00112233445566778899aabbccddeeff00112233"));
    assert_eq!(d.md5, h("00112233445566778899aabbccddeeff"));
    assert_eq!(d.region, "rgn");
    assert_eq!(d.merge, "parent");
    assert_eq!(d.status, "baddump");
    assert_eq!(d.size, None);
    let n = file(g, "Disk2");
    assert!(n.is_disk);
    assert_eq!(n.status, "nodump");
}

#[test]
fn cmp_emulator_block_and_archive_are_skipped() {
    let hd = read("cmp_full.dat");
    // The `emulator ( ... )` block adds nothing to the tree.
    assert!(!names(&hd.base_dir).contains(&"MAME"));
    assert_eq!(hd.base_dir.game_count(), 6);
}

#[test]
fn raine_files_skip_to_the_emulator_block() {
    let out = read_dat_report(&data("raine.dat"), "raine.dat");
    assert!(out.ok, "{:?}", out.messages);
    let hd = out.header.unwrap();
    // `emulator ( name .. version .. )` goes through the header loader.
    assert_eq!(hd.name, "raine");
    assert_eq!(hd.version, "0.1");
    assert_eq!(hd.description, "Raine description");
    assert!(dir(&hd.base_dir, "raine_game").game.is_some());
}

#[test]
fn romvault_keyword_header() {
    let hd = read_dat(
        b"romvault (\n\tname RV\n\tdescription \"RV desc\"\n)\ngame ( name g rom ( name a size 1 crc 1 ) )\n",
        "t",
    )
    .unwrap();
    assert_eq!(hd.name, "RV");
    assert_eq!(hd.description, "RV desc");
}

#[test]
fn cmp_without_a_header_block() {
    let hd = read_dat(b"game (\n\tname g\n\trom ( name a size 1 crc 1 )\n)\n", "t").unwrap();
    assert_eq!(hd.name, "");
    assert_eq!(hd.base_dir.game_count(), 1);
}

#[test]
fn cmp_one_line_game_swallows_the_rest_of_the_line() {
    // `GnRest` takes everything to the end of the line (`DatFileLoader.cs:33-39`), so a game
    // written on one line puts `rom (...) )` into the name.
    let hd = read_dat(b"game ( name g romof p rom ( name a size 1 ) )\n\n", "t").unwrap();
    let g = dir(&hd.base_dir, "g romof p rom ( name a size 1 ) )");
    assert!(g.children.is_empty());
}

#[test]
fn cmp_one_line_file_is_not_a_dat() {
    // `EndOfStream` is true once the last *line* is read, so a one-line file stops right
    // after its first token (`DatCmpReader.cs:23-27`).
    let out = read_dat_report(b"game ( name g )", "t");
    assert!(!out.ok);
}

#[test]
fn cmp_last_line_is_not_processed_inside_a_game() {
    // The game loop also tests `EndOfStream` (`DatCmpReader.cs:282`), which is already true
    // when the last line has been read, so the last line of a game is lost.
    let hd = read_dat_report(
        b"game (\n\tname g\n\tyear 1999\n\trom ( name a size 1 crc 1 )",
        "t",
    )
    .header
    .unwrap();
    let g = dir(&hd.base_dir, "g");
    assert_eq!(g.game.as_ref().unwrap().year, "1999");
    assert!(
        g.children.is_empty(),
        "the rom on the last line was never read"
    );
}

#[test]
fn cmp_text_that_stops_inside_a_block_is_an_error_not_a_hang() {
    // 3.8.0 never returns from these (`while (Next != ")")` at end of file). A header block
    // opens each file so that the sniffer picks the text reader, and a trailing comment line
    // keeps `EndOfStream` false while the broken block is being read.
    let file = |body: &str| format!("clrmamepro (\n\tname t\n)\n{body}\n// end\n");
    for body in [
        "game (\n\tname g\n\trom (\n\t\tname a\n\t\tsize 1",
        "game (\n\tname g\n\tdisk ( name d sha1 00",
        "game (\n\tname g\n\trom ( name a regiona x y",
        "game (\n\tname g\n\t/* never closed\n\tmore",
        "game (\n\tname g\n\tarchive ( name x",
        "dir (\n\tname d",
        "emulator (\n\tname x",
    ] {
        let out = read_dat_report(file(body).as_bytes(), "t");
        assert!(!out.ok, "{body:?}");
        assert!(
            out.messages
                .iter()
                .any(|m| m.contains("unexpected end of file")),
            "{body:?}: {:?}",
            out.messages
        );
    }
    let out = read_dat_report(b"clrmamepro (\n\tname x\n", "t");
    assert!(!out.ok);
    assert!(out
        .messages
        .iter()
        .any(|m| m.contains("unexpected end of file")));
    let out = read_dat_report(
        b"RAINE (680x0 Arcade Emulation)\nno emulator keyword\n",
        "t",
    );
    assert!(!out.ok);
    // A game cut off inside its own block is added with what was read (the game loop stops at
    // the end of the stream, `DatCmpReader.cs:282`).
    let out = read_dat_report(b"game (\n\tname g\n\tyear 1999\n", "t");
    assert!(out.ok);
    assert_eq!(out.header.unwrap().base_dir.game_count(), 1);
}

#[test]
fn cmp_missing_parenthesis_and_name_fail() {
    for body in [
        "game name g",
        "game (\n\tyear 1999\n)",
        "game (\n\tname g\n\trom name a\n)",
        "game (\n\tname g\n\trom ( size 1 )\n)",
        "game (\n\tname g\n\tdisk name d\n)",
        "game (\n\tname g\n\tdisk ( size 1 )\n)",
        "game (\n\tname g\n\tarchive name d\n)",
        "dir (\n\tyear 1\n)",
        "dir name x",
        "emulator name x",
    ] {
        let text = format!("clrmamepro (\n\tname t\n)\n{body}\n// end\n");
        let out = read_dat_report(text.as_bytes(), "t");
        assert!(!out.ok, "{body:?}");
    }
    assert!(!read_dat_report(b"clrmamepro name x\n// end\n", "t").ok);
}

// ---------------------------------------------------------------------------------------
// XML
// ---------------------------------------------------------------------------------------

#[test]
fn xml_header_fields() {
    let hd = read("xml_full.xml");
    assert_eq!(hd.id, "45");
    assert_eq!(hd.name, "Test XML");
    assert_eq!(hd.r#type, "standard");
    assert_eq!(hd.root_dir, "Root Dir");
    assert_eq!(hd.description, "Test XML Description");
    assert_eq!(hd.subset, "sub");
    assert_eq!(hd.category, "Cat");
    assert_eq!(hd.version, "2.0");
    assert_eq!(hd.date, "2021-03-04");
    assert_eq!(hd.author, "A & B");
    assert_eq!(hd.email, "x@example.org");
    assert_eq!(hd.homepage, "Home");
    assert_eq!(hd.url, "https://example.org");
    assert_eq!(hd.comment, "A <comment> here", "CDATA");
    // `<romvault>` wins over `<clrmamepro>`; the values are lower-cased except `header`.
    assert_eq!(hd.header, "No-Intro_NES.xml");
    assert_eq!(hd.compression, "zip");
    assert_eq!(hd.merge_type, "split");
    assert_eq!(hd.dir, "noautodir");
    assert!(hd.not_zipped);
    assert!(!hd.mame_xml);
}

#[test]
fn xml_clrmamepro_header_element_when_there_is_no_romvault() {
    let hd = read_dat(
        b"<?xml version=\"1.0\"?><datafile><header><name>n</name><clrmamepro header=\"snes.xml\" forcepacking=\"UNZIP\" forcemerging=\"FULL\" dir=\"NoGame\"/></header></datafile>",
        "t",
    )
    .unwrap();
    assert_eq!(hd.header, "snes.xml");
    assert_eq!(hd.compression, "unzip");
    assert_eq!(hd.merge_type, "full");
    assert_eq!(hd.dir, "nogame");
    assert!(!hd.not_zipped);
}

#[test]
fn xml_children_are_grouped_by_element_not_document_order() {
    // dir, game, machine, rom, disk (`DatXMLReader.cs:27-70`).
    let hd = read("xml_full.xml");
    assert_eq!(
        names(&hd.base_dir),
        [
            "outer",
            "",
            "Full Game",
            "Dir Type",
            "Seven Type",
            "Odd Type",
            "Emu Game",
            "Tea Game",
            "EmuArc Game",
            "A Machine",
            "loose.bin",
            "loose"
        ]
    );
}

#[test]
fn xml_dirs_nest() {
    let hd = read("xml_full.xml");
    let outer = dir(&hd.base_dir, "outer");
    assert!(outer.game.is_none());
    let inner = dir(outer, "inner");
    assert!(inner.game.is_none());
    assert_eq!(
        dir(inner, "Nested Game").game.as_ref().unwrap().description,
        "Nested"
    );
    assert!(dir(outer, "Dir Machine").game.is_some());
}

#[test]
fn xml_game_attributes_and_children() {
    let hd = read("xml_full.xml");
    let d = dir(&hd.base_dir, "Full Game");
    let g = d.game.as_ref().unwrap();
    assert_eq!(g.id, "0001");
    assert_eq!(g.rom_of, "parent");
    assert_eq!(g.clone_of, "parent");
    assert_eq!(g.clone_of_id, "0002");
    assert_eq!(g.sample_of, "samp");
    assert_eq!(g.source_file, "src.c");
    assert_eq!(g.is_bios, "yes");
    assert_eq!(g.is_device, "no");
    assert_eq!(g.board, "brd");
    assert_eq!(g.runnable, "no");
    assert_eq!(g.description, "Full & complete");
    assert_eq!(g.year, "1998");
    assert_eq!(g.manufacturer, "ACME");
    assert_eq!(g.serial, "SLUS-1");
    assert_eq!(g.version, "1.1");
    assert_eq!(g.game_id, "G1");
    assert_eq!(
        g.category,
        ["Games", "Extra"],
        "empty <category> is skipped"
    );
    assert_eq!(
        g.device_ref,
        ["dev1", "dev2"],
        "blank device_ref names are skipped"
    );
    assert!(!g.is_emu_arc);
    // `type="Zip"` preselects the container type (lower-cased before the comparison).
    assert_eq!(d.base.file_type, FileType::ZIP);
}

#[test]
fn xml_type_attribute() {
    let hd = read("xml_full.xml");
    assert_eq!(dir(&hd.base_dir, "Dir Type").base.file_type, FileType::DIR);
    assert_eq!(
        dir(&hd.base_dir, "Seven Type").base.file_type,
        FileType::SEVEN_ZIP
    );
    assert_eq!(
        dir(&hd.base_dir, "Odd Type").base.file_type,
        FileType::UNSET
    );
}

#[test]
fn xml_rom_attributes() {
    let hd = read("xml_full.xml");
    let g = dir(&hd.base_dir, "Full Game");
    let a = file(g, "a.bin");
    assert_eq!(a.size, Some(16));
    assert_eq!(a.crc, h("abcd1234"));
    assert_eq!(a.sha1, h("0123456789abcdef0123456789abcdef01234567"));
    assert_eq!(
        a.sha256,
        h("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
    );
    assert_eq!(a.md5, h("0123456789abcdef0123456789abcdef"));
    assert_eq!(a.merge, "m.bin");
    assert_eq!(a.status, "verified");
    assert_eq!(a.region, "usa");
    assert_eq!(a.mia, "yes");
    // The XML reader never reads a rom's `date` (`DatXMLReader.cs:361`).
    assert_eq!(a.base.date_modified, None);
    let b = file(g, "b.bin");
    assert_eq!(b.size, Some(32));
    assert_eq!(b.status, "nodump");
    let c = file(g, "c.bin");
    assert_eq!((c.size, &c.crc, &c.sha1), (None, &None, &None));
}

#[test]
fn xml_disk_attributes() {
    let hd = read("xml_full.xml");
    let g = dir(&hd.base_dir, "Full Game");
    let d = file(g, "Disk");
    assert!(d.is_disk);
    assert_eq!(d.sha1, h("00112233445566778899aabbccddeeff00112233"));
    assert_eq!(
        d.sha256,
        h("00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff")
    );
    assert_eq!(d.md5, h("00112233445566778899aabbccddeeff"));
    assert_eq!(d.merge, "Parent");
    assert_eq!(d.status, "baddump");
    assert_eq!(d.mia, "yes");
    assert_eq!(d.size, None);
    // top-level disk
    let l = file(&hd.base_dir, "loose");
    assert!(l.is_disk && l.sha1 == h("1111111111111111111111111111111111111111"));
}

#[test]
fn xml_emuarc_overrides() {
    let hd = read("xml_full.xml");
    let g = dir(&hd.base_dir, "Emu Game").game.as_ref().unwrap();
    assert!(g.is_emu_arc);
    assert_eq!(g.id, "T1", "titleid replaces the id attribute");
    assert_eq!(g.publisher, "Pub");
    assert_eq!(g.developer, "Dev");
    assert_eq!(g.year, "2001", "year from the block replaces the game's");
    assert_eq!(g.genre, "Genre");
    assert_eq!(g.sub_genre, "Sub");
    assert_eq!(g.ratings, "E");
    assert_eq!(g.score, "9");
    assert_eq!(g.players, "2");
    assert_eq!(g.enabled, "yes");
    assert_eq!(g.crc, "deadbeef");
    assert_eq!(g.clone_of, "EmuParent");
    assert_eq!(g.related_to, "Other");
    assert_eq!(g.source, "Src");
    assert_eq!(g.description, "Emu");
    // `<tea>` and `<EmuArc>` are accepted too.
    assert_eq!(
        dir(&hd.base_dir, "Tea Game").game.as_ref().unwrap().id,
        "TEA"
    );
    assert!(
        dir(&hd.base_dir, "EmuArc Game")
            .game
            .as_ref()
            .unwrap()
            .is_emu_arc
    );
}

#[test]
fn xml_toplevel_machine_and_rom() {
    let hd = read("xml_full.xml");
    assert!(dir(&hd.base_dir, "A Machine").game.is_some());
    assert_eq!(file(&hd.base_dir, "loose.bin").size, Some(5));
}

#[test]
fn xml_default_namespace_hides_elements_like_xpath() {
    // `SelectNodes("game")` does not match elements in a namespace (`XmlDocument` semantics).
    let hd = read_dat(
        b"<?xml version=\"1.0\"?><datafile xmlns=\"urn:x\"><header><name>n</name></header><game name=\"g\"/></datafile>",
        "t",
    );
    // The header is in the namespace too, so the document is not seen as a `<datafile>`.
    assert!(hd.is_err());
}

#[test]
fn xml_encodings() {
    // UTF-16 with a byte order mark.
    let text = "<?xml version=\"1.0\"?><datafile><header><name>Caf\u{e9}</name></header><game name=\"g\"><rom name=\"a\" size=\"1\" crc=\"1\"/></game></datafile>";
    let mut utf16 = vec![0xFF, 0xFE];
    for u in text.encode_utf16() {
        utf16.extend(u.to_le_bytes());
    }
    assert_eq!(read_dat(&utf16, "t").unwrap().name, "Café");
    // UTF-8 with a byte order mark.
    let mut utf8 = vec![0xEF, 0xBB, 0xBF];
    utf8.extend(text.as_bytes());
    assert_eq!(read_dat(&utf8, "t").unwrap().name, "Café");
    // A declared single-byte encoding.
    let latin = b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><datafile><header><name>Caf\xe9</name></header></datafile>";
    assert_eq!(read_dat(latin, "t").unwrap().name, "Café");
}

#[test]
fn xml_text_is_lenient_about_characters_and_doctype() {
    // `CheckCharacters = false`: a character reference to a control character is accepted.
    let hd = read_dat(
        b"<?xml version=\"1.0\"?><datafile><header><name>a&#1;b</name></header></datafile>",
        "t",
    )
    .unwrap();
    assert_eq!(hd.name, "a\u{1}b");
    // An internal DOCTYPE subset with entities is skipped, not expanded.
    let hd = read_dat(
        b"<?xml version=\"1.0\"?>\n<!DOCTYPE datafile [ <!ENTITY e \"x\"> <!ELEMENT datafile ANY> ]>\n<datafile><header><name>n</name></header></datafile>",
        "t",
    )
    .unwrap();
    assert_eq!(hd.name, "n");
}

#[test]
fn xml_errors_are_errors() {
    let decl = "<?xml version=\"1.0\"?>";
    for (label, body, message) in [
        (
            "truncated",
            "<datafile><header><name>x</name></header><game name=\"g\">",
            "Error Occured Reading Dat",
        ),
        (
            "mismatched",
            "<datafile><header></head></datafile>",
            "Error Occured Reading Dat",
        ),
        (
            "undefined entity",
            "<datafile><header><name>&nope;</name></header></datafile>",
            "Error Occured Reading Dat",
        ),
        (
            "two roots",
            "<datafile><header/></datafile><datafile><header/></datafile>",
            "Error Occured Reading Dat",
        ),
        (
            "text before root",
            "stray<datafile><header/></datafile>",
            "Error Occured Reading Dat",
        ),
        (
            "cut inside a tag",
            "<datafile><header><name>x</name></header><game na",
            "Error Occured Reading Dat",
        ),
        ("no header", "<datafile></datafile>", ""),
        ("wrong root", "<something/>", ""),
    ] {
        let text = format!("{decl}{body}");
        let out = read_dat_report(text.as_bytes(), "t");
        assert!(!out.ok, "{label}");
        // "no header" and "wrong root" fail silently in 3.8.0 (the reader returns false
        // without a message), so only the parse errors carry text.
        assert_eq!(
            out.messages.is_empty(),
            message.is_empty(),
            "{label}: {:?}",
            out.messages
        );
        assert!(out.messages.iter().all(|m| m.contains(message)), "{label}");
        assert!(read_dat(text.as_bytes(), "t").is_err(), "{label}");
    }
}

// ---------------------------------------------------------------------------------------
// MAME XML
// ---------------------------------------------------------------------------------------

#[test]
fn mame_header_and_machines() {
    let hd = read("mame_full.xml");
    assert!(hd.mame_xml);
    assert_eq!(hd.name, "0.250 (mame0250)");
    assert_eq!(hd.description, "0.250 (mame0250)");
    assert_eq!(hd.version, "0.250");
    assert_eq!(hd.date, "2022-01-01");
    // dir, game, machine order (`DatXMLReader.cs:87-118`).
    assert_eq!(
        names(&hd.base_dir),
        ["subdir", "legacygame", "parent", "clone"]
    );
}

#[test]
fn mame_machine_fields() {
    let hd = read("mame_full.xml");
    let p = dir(&hd.base_dir, "parent");
    let g = p.game.as_ref().unwrap();
    assert_eq!(g.is_bios, "yes");
    assert_eq!(g.is_device, "yes");
    assert_eq!(g.runnable, "no");
    assert_eq!(g.source_file, "p.cpp");
    assert_eq!(g.description, "Parent & Co");
    assert_eq!(g.year, "1990");
    assert_eq!(g.manufacturer, "Maker");
    assert_eq!(g.device_ref, ["devA"]);
    let r1 = file(p, "p1.bin");
    assert_eq!(r1.size, Some(256));
    assert_eq!(r1.crc, h("01234567"));
    assert_eq!(r1.sha1, h("0123456789abcdef0123456789abcdef01234567"));
    assert_eq!(r1.merge, "m.bin");
    assert_eq!(r1.status, "good");
    let r2 = file(p, "p2.bin");
    assert_eq!(
        (r2.status.as_str(), r2.region.as_str()),
        ("baddump", "maincpu")
    );
    let d = file(p, "hd");
    assert!(d.is_disk);
    assert_eq!((d.status.as_str(), d.merge.as_str()), ("nodump", "hd"));
    let c = dir(&hd.base_dir, "clone").game.as_ref().unwrap();
    assert_eq!(
        (c.clone_of.as_str(), c.rom_of.as_str()),
        ("parent", "parent")
    );
    // `<dir>` inside `<mame>`
    assert!(dir(dir(&hd.base_dir, "subdir"), "dirmachine")
        .game
        .is_some());
}

#[test]
fn mess_root_is_read_as_mame() {
    let hd = read_dat(
        b"<?xml version=\"1.0\"?><mess build=\"mess build\" version=\"1\" date=\"d\"><machine name=\"m\"><rom name=\"a\" size=\"1\" crc=\"1\"/></machine></mess>",
        "t",
    )
    .unwrap();
    assert!(hd.mame_xml);
    assert_eq!(hd.name, "mess build");
    assert_eq!(hd.base_dir.game_count(), 1);
}

// ---------------------------------------------------------------------------------------
// Software list
// ---------------------------------------------------------------------------------------

#[test]
fn softlist_header_and_software() {
    let hd = read("softlist_full.xml");
    assert_eq!(hd.name, "test_list");
    assert_eq!(hd.description, "Test software list");
    // The software with no roms is dropped (`DatMessXmlReader.cs:124-127`).
    assert_eq!(names(&hd.base_dir), ["game1", "game2"]);
    let g1 = dir(&hd.base_dir, "game1");
    let g = g1.game.as_ref().unwrap();
    assert_eq!(g.description, "Game 1");
    assert_eq!(g.year, "1985");
    assert_eq!(g.manufacturer, "Pub", "publisher");
    assert_eq!(g.clone_of, "game0");
    assert_eq!(g.rom_of, "game0");
}

#[test]
fn softlist_roms_and_continuations() {
    let hd = read("softlist_full.xml");
    let g1 = dir(&hd.base_dir, "game1");
    let a = file(g1, "a.bin");
    assert_eq!(a.crc, h("aabbccdd"));
    assert_eq!(a.sha1, h("0123456789abcdef0123456789abcdef01234567"));
    assert_eq!(a.status, "baddump");
    // `loadflag="continue"` and `"ignore"` add to the previous named rom (`:151-160`); a
    // `fill` rom with no name is dropped.
    assert_eq!(a.size, Some(32768));
    assert_eq!(file(g1, "b.bin").size, Some(16384 + 1024 + 2048));
    // A rom with no size is a rom with no size.
    assert_eq!(file(g1, "c.bin").size, None);
    // `indexContinue` restarts per `<part>`.
    assert_eq!(file(g1, "d.bin").size, Some(3));
    // Disks come after the roms, `.chd` stripped, SHA1 and status only.
    let d = file(g1, "game1");
    assert!(d.is_disk);
    assert_eq!((d.status.as_str(), d.md5.clone()), ("nodump", None));
    assert_eq!(d.sha1, h("00112233445566778899aabbccddeeff00112233"));
    assert_eq!(names(g1), ["a.bin", "b.bin", "c.bin", "d.bin", "game1"]);
}

#[test]
fn softlist_continue_before_any_rom_aborts_the_read() {
    // `parentDir[-1]` throws in 3.8.0 (`DatMessXmlReader.cs:153`).
    let out = read_dat_report(
        b"<?xml version=\"1.0\"?><softwarelist name=\"n\" description=\"d\"><software name=\"s\"><part><dataarea><rom loadflag=\"continue\" size=\"1\"/></dataarea></part></software></softwarelist>",
        "t",
    );
    assert!(!out.ok);
    let hd = out.header.unwrap();
    assert_eq!(hd.name, "n", "the partly read header is still returned");
}

// ---------------------------------------------------------------------------------------
// DOSCenter
// ---------------------------------------------------------------------------------------

#[test]
fn dos_header() {
    let hd = read("dos_full.dat");
    // `name:` may be glued to its value, and `description:` etc. take the rest of the line.
    assert_eq!(hd.name, "Test DOS Set");
    assert_eq!(hd.description, "A DOS description");
    assert_eq!(hd.version, "1.2");
    assert_eq!(hd.date, "2020/05/06");
    assert_eq!(hd.author, "Author Name");
    assert_eq!(hd.homepage, "http:"); // `//` again
    assert_eq!(hd.comment, "Comment here");
    assert_eq!(hd.compression, "TDC");
}

#[test]
fn dos_glued_name() {
    let hd = read_dat(b"DOSCenter (\n\tname:Glued Name\n)\ngame ( name \"g.zip\" file ( name a size 1 crc 1 ) )\n", "t");
    // The header block ends at its `)`. A game on one line is a CMP-style problem, so read
    // it properly.
    let hd2 = read_dat(
        b"DOSCenter (\n\tname:Glued Name\n)\ngame (\n\tname \"g.zip\"\n\tfile ( name a size 1 crc 1 )\n)\n",
        "t",
    )
    .unwrap();
    assert_eq!(hd2.name, "Glued Name");
    let _ = hd;
}

#[test]
fn dos_games_and_files() {
    let hd = read("dos_full.dat");
    assert_eq!(names(&hd.base_dir), ["First Game", "ab", "Plain"]);
    let g = dir(&hd.base_dir, "First Game");
    assert!(g.game.is_some());
    let f = file(g, "FIRST.EXE");
    assert_eq!(f.size, Some(1234));
    assert_eq!(f.crc, h("0a0b0c0d"));
    assert_eq!(f.sha1, h("0123456789abcdef0123456789abcdef01234567"));
    // 1996/12/24 23:32:00 is the TorrentZip date, packed as MS-DOS date and time
    // (`StructuredZip.cs:414`).
    assert_eq!(f.base.date_modified, Some(563_657_728));
    // Names may hold spaces (`GnNameToSize`) and `rom` is a synonym of `file`.
    let s = file(g, "sub dir/with space.dat");
    assert_eq!(s.size, Some(5));
    assert!(s.base.date_modified.is_some());
    assert_eq!(file(g, "nodate.bin").base.date_modified, None);
    // `.zip` is only removed from names longer than 4 characters.
    assert!(dir(&hd.base_dir, "ab").game.is_some());
}

#[test]
fn dos_dates() {
    let hd = read("dos_full.dat");
    let g = dir(&hd.base_dir, "ab");
    // Before 1980 and after 2107 the value is .NET ticks, not the packed form.
    assert_eq!(
        file(g, "x").base.date_modified,
        Some(621_355_968_000_000_000)
    );
    assert!(file(g, "y").base.date_modified.unwrap() > 0xffff_ffff);
    assert!(file(g, "z").base.date_modified.unwrap() > 0xffff_ffff);
    // A date that is not `yyyy/mm/dd hh:mm:ss` is null, and its second token is then an
    // unknown keyword.
    assert_eq!(file(g, "bad").base.date_modified, None);
}

#[test]
fn dos_unknown_keys_are_reported_and_skipped() {
    let out = read_dat_report(&data("dos_full.dat"), "dos_full.dat");
    assert!(out.ok, "{:?}", out.messages);
    assert!(out.messages.iter().any(|m| m.contains("'unknownkey'")));
    // The unknown key's value is read as the next keyword too (`DatDOSReader.cs:226-229`).
    assert!(out.messages.iter().any(|m| m.contains("'value'")));
}

#[test]
fn dos_file_without_size_aborts_the_read() {
    // ` size ` missing: `Substring(0, -1)` throws in 3.8.0 (`DatDOSReader.cs:202`,
    // `DatFileLoader.cs:48-55`).
    let out = read_dat_report(
        b"DOSCenter (\n\tname: n\n)\ngame (\n\tname \"g.zip\"\n\tfile ( name nosize.bin )\n)\n",
        "t",
    );
    assert!(!out.ok);
    assert_eq!(out.header.unwrap().name, "n");
}

// ---------------------------------------------------------------------------------------
// RomCenter
// ---------------------------------------------------------------------------------------

fn check_romcenter(hd: &DatHeader) {
    assert_eq!(hd.author, "An Author");
    assert_eq!(hd.email, "a@example.org");
    assert_eq!(
        hd.url, "http://url",
        "`url` is read after `homepage`, both go to URL"
    );
    assert_eq!(hd.version, "1.0");
    assert_eq!(hd.date, "2020-01-01");
    assert_eq!(hd.comment, "A comment");
    assert_eq!(hd.split, "1");
    assert_eq!(hd.merge_type, "0");
    assert_eq!(hd.name, "Emu");
    assert_eq!(hd.description, "Emu 1.0");
    assert_eq!(hd.category, "Arcade");

    let root = &hd.base_dir;
    // The second `[GAMES]` section adds to the same game; each `[DISKS]` row makes a new
    // game directory with the same name, because the search uses `FileType.Dir` while games
    // are `UnSet` (`DatROMCenterReader.cs:323`, `DatDir.cs:239`).
    assert_eq!(names(root), ["parent", "child", "res", "parent"]);
    let parent = dir(root, "parent");
    assert_eq!(names(parent), ["p1.bin", "p2.bin", "p3.bin"]);
    let g = parent.game.as_ref().unwrap();
    assert_eq!(g.description, "Parent Desc");
    assert_eq!(
        g.clone_of, "",
        "a game whose parent is itself has no cloneof"
    );
    let p1 = file(parent, "p1.bin");
    assert_eq!(p1.crc, h("deadbeef"));
    assert_eq!(p1.size, Some(1024));
    assert_eq!(p1.merge, "");
    let child = dir(root, "child");
    assert_eq!(child.game.as_ref().unwrap().clone_of, "parent");
    assert_eq!(child.game.as_ref().unwrap().description, "Child Desc");
    assert_eq!(file(child, "c1.bin").merge, "p1.bin");
    // `[RESOURCES]` is read like `[GAMES]`; a short CRC is zero padded.
    assert_eq!(file(dir(root, "res"), "r.bin").crc, h("000000ab"));
    // The disk row: SHA1 comes from the "crc" column, `.chd` is stripped.
    let disk_game = root
        .children
        .iter()
        .filter_map(|c| match c {
            DatNode::Dir(d) if d.base.name == "parent" => Some(d),
            _ => None,
        })
        .nth(1)
        .unwrap();
    let d = file(disk_game, "hd");
    assert!(d.is_disk);
    assert_eq!(d.sha1, h("0123456789abcdef0123456789abcdef01234567"));
    assert_eq!(d.merge, "hdparent");
    assert_eq!(d.size, None);
}

#[test]
fn romcenter_latin1_separator_bytes() {
    // Real RomCenter files hold the byte 0xAC, which is invalid UTF-8 and decodes to U+FFFD.
    check_romcenter(&read("romcenter_full.dat"));
}

#[test]
fn romcenter_utf8_separator() {
    check_romcenter(&read("romcenter_utf8.dat"));
}

#[test]
fn romcenter_rejects_unknown_keys_and_sections() {
    for bad in [
        &b"[CREDITS]\nauthor=a\nnosuchkey=1\n[GAMES]\n"[..],
        b"[CREDITS]\nthis line has no equals sign\n[GAMES]\n",
        b"[CREDITS]\nauthor=a\n[WHATEVER]\nx\n",
        b"[CREDITS]\nauthor=a\n[DAT]\nbogus=1\n[GAMES]\n",
        b"[CREDITS]\nauthor=a\n[EMULATOR]\nbogus=1\n[GAMES]\n",
    ] {
        let out = read_dat_report(bad, "t");
        assert!(!out.ok, "{:?}", String::from_utf8_lossy(bad));
        // The header read so far is returned, as in 3.8.0.
        assert!(out.header.is_some());
    }
}

#[test]
fn romcenter_short_row_is_an_exception() {
    // Fewer than ten fields: `IndexOutOfRangeException` (`DatROMCenterReader.cs:238-246`).
    let out = read_dat_report(
        "[CREDITS]\nauthor=a\n[GAMES]\n¬a¬b¬c\n[DISKS]\n".as_bytes(),
        "t",
    );
    assert!(!out.ok);
    assert_eq!(out.header.unwrap().author, "a");
}

#[test]
fn romcenter_trailing_blank_lines_are_an_exception() {
    // `Gn()` returns a blank line at the end and `Substring(0, 1)` throws on it (`:213-217`).
    let out = read_dat_report(b"[CREDITS]\nauthor=a\n\n\n", "t");
    assert!(!out.ok);
}

// ---------------------------------------------------------------------------------------
// Sniffing and containers
// ---------------------------------------------------------------------------------------

#[test]
fn sniffing_order() {
    // First non-blank line decides (`DatReader.cs:46-110`).
    assert!(read_dat(
        b"\n\n  \n<?xml version=\"1.0\"?><datafile><header/></datafile>",
        "t"
    )
    .is_ok());
    // A ClrMamePro DAT whose first line is a comment containing "xml" goes to the XML reader
    // and fails there.
    assert!(read_dat(b"// made with xml tools\nclrmamepro ( name x )\n", "t").is_err());
    // A comment line with none of the keywords is "Invalid DAT File".
    let out = read_dat_report(b"// just a comment\nclrmamepro ( name x )\n", "t");
    assert!(!out.ok);
    assert!(out.header.is_none());
    assert!(out.messages.iter().any(|m| m.contains("Invalid DAT File")));
    // keywords: romvault, game, machine all pick the text reader.
    for first in [
        "romvault (",
        "game (",
        "machine (",
        "ClrMame (",
        "CLRMAMEPRO (",
    ] {
        let text = format!("{first}\n\tname x\n)\n");
        let out = read_dat_report(text.as_bytes(), "t");
        assert!(out.header.is_some(), "{first}");
    }
    assert!(read_dat_report(b"DOSCenter (\n)\n", "t").ok);
    assert!(read_dat_report(b"[CREDITS]\nauthor=x\n", "t").ok);
}

#[test]
fn empty_and_blank_files() {
    let out = read_dat_report(b"", "t");
    assert!(!out.ok && out.header.is_none());
    let out = read_dat_report(b"   \n\n\t\n", "t");
    assert!(!out.ok && out.header.is_none());
    assert!(read_dat(b"", "t").is_err());
}

#[test]
fn datz_is_gzip_of_a_dat() {
    use std::io::Write;
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(&data("cmp_full.dat")).unwrap();
    let gz = enc.finish().unwrap();
    let dir = std::env::temp_dir().join(format!("rv-dat-datz-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Test.DATZ"); // the extension test ignores case
    std::fs::write(&path, &gz).unwrap();
    let out = rv_dat::read_dat_file(&path);
    assert!(out.ok, "{:?}", out.messages);
    assert_eq!(out.header.unwrap().name, "Test Set");
    // Not gzip at all.
    std::fs::write(&path, b"not gzip").unwrap();
    assert!(!rv_dat::read_dat_file(&path).ok);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn datz_inflate_is_capped() {
    use std::io::Write;
    let gzip = |bytes: &[u8]| {
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(bytes).unwrap();
        enc.finish().unwrap()
    };
    let dat = data("cmp_full.dat");
    let dir = std::env::temp_dir().join(format!("rv-dat-datz-cap-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Cap.datz");

    // Exactly at the cap is fine; one byte over is not.
    std::fs::write(&path, gzip(&dat)).unwrap();
    let at = rv_dat::read_dat_file_limited(&path, dat.len() as u64);
    assert!(at.ok, "{:?}", at.messages);
    let over = rv_dat::read_dat_file_limited(&path, dat.len() as u64 - 1);
    assert!(!over.ok && over.header.is_none());
    assert!(
        over.messages[0].contains("exceeds the"),
        "{:?}",
        over.messages
    );

    // A bomb: 64 MiB of zeros is ~64 KiB of gzip, and must stop at the cap, not inflate.
    let bomb = gzip(&vec![0u8; 64 << 20]);
    assert!(bomb.len() < 1 << 20);
    std::fs::write(&path, &bomb).unwrap();
    let out = rv_dat::read_dat_file_limited(&path, 1 << 20);
    assert!(!out.ok && out.header.is_none());
    assert!(
        out.messages[0].contains("exceeds the"),
        "{:?}",
        out.messages
    );

    // A second gzip member counts toward the same cap.
    let mut two = gzip(&dat);
    two.extend(gzip(&dat));
    std::fs::write(&path, &two).unwrap();
    assert!(!rv_dat::read_dat_file_limited(&path, dat.len() as u64 + 1).ok);

    // The default cap leaves ordinary files alone.
    std::fs::write(&path, gzip(&dat)).unwrap();
    assert!(rv_dat::read_dat_file(&path).ok);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn missing_file_is_an_error() {
    let out = rv_dat::read_dat_file(std::path::Path::new("/nonexistent/none.dat"));
    assert!(!out.ok && out.header.is_none());
}

#[test]
fn header_file_type_defaults_to_nothing() {
    let hd = read("xml_full.xml");
    let g = dir(&hd.base_dir, "Full Game");
    assert_eq!(file(g, "a.bin").header_file_type, HeaderFileType::NOTHING);
}

// ---------------------------------------------------------------------------------------
// Robustness
// ---------------------------------------------------------------------------------------

#[test]
fn mutated_input_never_panics_or_hangs() {
    // A deterministic xorshift mutator over every fixture: truncations, flipped, inserted,
    // deleted and duplicated bytes. Any panic fails the test; a hang times the run out.
    let mut seed = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let names = [
        "cmp_full.dat",
        "xml_full.xml",
        "mame_full.xml",
        "softlist_full.xml",
        "dos_full.dat",
        "romcenter_full.dat",
        "romcenter_utf8.dat",
        "raine.dat",
    ];
    let mut outcomes = [0usize; 2];
    for name in names {
        let original = data(name);
        for _ in 0..1500 {
            let mut b = original.clone();
            let len = b.len().max(1);
            let at = (next() as usize) % len;
            match next() % 5 {
                0 => b.truncate(at),
                1 => b[at] ^= 1 << (next() % 8),
                2 => b.insert(at, next() as u8),
                3 => {
                    let end = (at + (next() as usize) % 40).min(b.len());
                    b.drain(at..end);
                }
                _ => {
                    let end = (at + (next() as usize) % 200).min(b.len());
                    let chunk = b[at..end].to_vec();
                    b.splice(at..at, chunk);
                }
            }
            let out = read_dat_report(&b, "fuzz");
            outcomes[usize::from(out.ok)] += 1;
            if let Some(h) = out.header {
                // Whatever was read can be written again.
                let _ = rv_dat::write_xml(&h, false);
                let _ = rv_dat::write_mame_xml(&h);
                let _ = rv_dat::write_dos(&h);
            }
        }
    }
    // Both kinds of outcome occur, so the mutator is exercising real parse failures.
    assert!(outcomes[0] > 100 && outcomes[1] > 100, "{outcomes:?}");
}

#[test]
fn one_huge_line_is_read_in_linear_time() {
    // 2M tokens on one line took 113 s when the loader re-sliced the rest of the line per
    // token. 1M tokens needed ~20 s then; linear it is ~0.3 s even in a debug build.
    let text = format!("clrmamepro (\n{}", "( ".repeat(1_000_000));
    let start = std::time::Instant::now();
    let out = read_dat_report(text.as_bytes(), "t");
    assert!(!out.ok);
    assert!(
        start.elapsed() < std::time::Duration::from_secs(5),
        "took {:?}",
        start.elapsed()
    );
}

#[test]
fn deep_nesting_is_an_error_not_a_stack_overflow() {
    let mut cmp = String::from("clrmamepro (\n\tname t\n)\n");
    for i in 0..5000 {
        cmp.push_str(&format!("dir (\n\tname d{i}\n"));
    }
    let out = read_dat_report(cmp.as_bytes(), "t");
    assert!(!out.ok);
    assert!(
        out.messages.iter().any(|m| m.contains("nested more than")),
        "{:?}",
        out.messages
    );

    let mut xml = String::from("<?xml version=\"1.0\"?><datafile><header/>");
    xml.push_str(&"<dir name=\"d\">".repeat(100_000));
    let out = read_dat_report(xml.as_bytes(), "t");
    assert!(!out.ok);
    assert!(
        out.messages.iter().any(|m| m.contains("nested more than")),
        "{:?}",
        out.messages
    );
    // A sensible depth is fine.
    let mut ok = String::from("<?xml version=\"1.0\"?><datafile><header/>");
    ok.push_str(&"<dir name=\"d\">".repeat(100));
    ok.push_str(&"</dir>".repeat(100));
    ok.push_str("</datafile>");
    assert!(read_dat(ok.as_bytes(), "t").is_ok());
}
