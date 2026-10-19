//! Real-data tests: the board's own DATs (`DatRoot/`) and its `RomVault3_3.Cache`. These need
//! files that are never in the repository, so they are `#[ignore]`d. Run them with
//!
//! ```sh
//! RV_FIXTURES_DIR=/path/to/copies cargo test -p rv-dat --test fixtures -- --ignored --nocapture
//! ```
//!
//! The directory must hold `DatRoot/` (a copy of the board's DAT root, 14 files) and
//! `RomVault3_3.Cache`. With the variable unset, or a file missing, these tests fail rather
//! than pass.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use rv_cache::CacheFile;
use rv_core::{dat_data, game_data, DatStatus, FileStatus, RvFile};
use rv_dat::{read_dat, read_dat_file, write_mame_xml, write_xml, DatDir, DatGame, DatNode};

fn fixtures_dir() -> PathBuf {
    match std::env::var_os("RV_FIXTURES_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => panic!("RV_FIXTURES_DIR is not set; these tests need the board's DATs and cache"),
    }
}

fn walk(p: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(p).unwrap_or_else(|e| panic!("{}: {e}", p.display())) {
        let e = e.unwrap().path();
        if e.is_dir() {
            walk(&e, out)
        } else {
            out.push(e)
        }
    }
}

fn dat_files() -> Vec<PathBuf> {
    let mut files = vec![];
    walk(&fixtures_dir().join("DatRoot"), &mut files);
    files.sort();
    assert_eq!(
        files.len(),
        14,
        "DatRoot should hold the board's 14 DATs: {files:?}"
    );
    files
}

/// Independent counts of `//game` + `//machine` and `//rom` per DAT, from `xmllint --xpath
/// 'count(...)'` and Python `xml.etree` (`python3 -I`), which agree with each other and with
/// the board's first rough regex count. Keyed by the start of the file name.
const EXPECTED: &[(&str, usize, usize)] = &[
    ("Nintendo - Nintendo 3DS (Decrypted)", 2136, 2136),
    ("Nintendo - Nintendo DS (Decrypted)", 7554, 7554),
    ("Nintendo - Game Boy (", 1922, 1922),
    ("Nintendo - Game Boy Advance", 3429, 3429),
    ("Nintendo - Game Boy Color", 1941, 1941),
    ("Nintendo - GameCube", 1974, 1974),
    ("Sega - Mega Drive - Genesis", 2671, 2672),
    ("Sega - 32X", 213, 213),
    ("Sega - Mega CD & Sega CD", 542, 8358),
    ("Nintendo - Nintendo 64 (BigEndian)", 1127, 1128),
    (
        "Nintendo - Nintendo Entertainment System (Headered)",
        3512,
        3513,
    ),
    ("Sony - PlayStation - Datfile", 10700, 59424),
    ("Sony - PlayStation 2", 11740, 14935),
    ("Nintendo - Super Nintendo Entertainment System", 3999, 4000),
];

fn expected_for(path: &Path) -> (usize, usize) {
    let name = path.file_name().unwrap().to_string_lossy();
    let hits: Vec<_> = EXPECTED
        .iter()
        .filter(|(p, _, _)| name.starts_with(p))
        .collect();
    assert_eq!(hits.len(), 1, "{name}: {hits:?}");
    (hits[0].1, hits[0].2)
}

#[test]
#[ignore = "needs RV_FIXTURES_DIR"]
fn all_14_real_dats_read_with_the_independent_counts() {
    println!(
        "| DAT | games (rv-dat / independent) | roms (rv-dat / independent) | disks | messages |"
    );
    println!("|---|---|---|---|---|");
    for f in dat_files() {
        let out = read_dat_file(&f);
        let name = f.file_name().unwrap().to_string_lossy().into_owned();
        assert!(out.ok, "{name}: {:?}", out.messages);
        assert!(out.messages.is_empty(), "{name}: {:?}", out.messages);
        let h = out.header.expect("header");
        let (games, roms) = expected_for(&f);
        println!(
            "| {name} | {} / {games} | {} / {roms} | {} | {} |",
            h.base_dir.game_count(),
            h.base_dir.rom_count(),
            h.base_dir.disk_count(),
            out.messages.len()
        );
        assert_eq!(h.base_dir.game_count(), games, "{name}: games");
        assert_eq!(h.base_dir.rom_count(), roms, "{name}: roms");
        assert_eq!(h.base_dir.disk_count(), 0, "{name}: no disks in these DATs");
    }
}

#[test]
#[ignore = "needs RV_FIXTURES_DIR"]
fn positive_control_the_nes_headered_dat() {
    let f = dat_files()
        .into_iter()
        .find(|f| f.to_string_lossy().contains("(Headered)"))
        .expect("NES (Headered) DAT");
    let h = read_dat_file(&f).header.unwrap();
    assert_eq!(
        h.name,
        "Nintendo - Nintendo Entertainment System (Headered)"
    );
    assert_eq!(h.id, "45");
    assert_eq!(h.base_dir.game_count(), 3512);
    assert_eq!(h.base_dir.rom_count(), 3513);
    // The first game, by value (checked against the file by eye: `10-Yard Fight (Japan)`
    // sorts after `'89 Dennou Kyuusei Uranai (Japan)` in the cache but not in DAT order).
    let first = h
        .base_dir
        .children
        .iter()
        .find_map(|c| match c {
            DatNode::Dir(d) if d.game.is_some() && d.base.name == "10-Yard Fight (Japan)" => {
                Some(d)
            }
            _ => None,
        })
        .expect("10-Yard Fight (Japan)");
    let rom = match &first.children[0] {
        DatNode::File(f) => f,
        _ => panic!(),
    };
    assert_eq!(rom.base.name, "10-Yard Fight (Japan).nes");
    assert_eq!(rom.size, Some(24592));
    assert_eq!(rom.crc.as_deref().map(hex), Some("44aa3eeb".into()));
    assert_eq!(
        rom.sha1.as_deref().map(hex),
        Some("2a91760c828b26adc765ba9b0322fb770089e391".into())
    );
    assert_eq!(
        rom.md5.as_deref().map(hex),
        Some("1378270f6cf90095b7fea1e6648503a4".into())
    );
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[test]
#[ignore = "needs RV_FIXTURES_DIR"]
fn negative_control_a_truncated_ps_dat_is_an_error_not_a_panic() {
    let f = dat_files()
        .into_iter()
        .find(|f| f.to_string_lossy().contains("PlayStation - Datfile"))
        .expect("PS DAT");
    let bytes = std::fs::read(&f).unwrap();
    // Cut in the middle of an element: inside the 5000th `<rom ... />` tag.
    let needle = b"<rom name=\"";
    let mut pos = 0;
    let mut cut = 0;
    for _ in 0..5000 {
        pos += bytes[pos..]
            .windows(needle.len())
            .position(|w| w == needle)
            .unwrap()
            + 1;
        cut = pos + 5; // `<rom ` -> after `<rom `, inside the tag
    }
    assert!(
        read_dat(&bytes[..cut + 6], "ps").is_err(),
        "cut inside a tag"
    );
    // And at many other offsets, including every kind of boundary.
    let step = bytes.len() / 23;
    for end in (1..bytes.len()).step_by(step.max(1)) {
        let r = read_dat(&bytes[..end], "ps");
        assert!(
            r.is_err(),
            "offset {end} of {} read as a complete DAT",
            bytes.len()
        );
    }
    // The whole file is fine.
    assert!(read_dat(&bytes, "ps").is_ok());
}

// ---------------------------------------------------------------------------------------
// Cross-check against the cache ROMVault wrote
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Rom {
    name: String,
    size: Option<u64>,
    crc: Option<Vec<u8>>,
    sha1: Option<Vec<u8>>,
    md5: Option<Vec<u8>>,
    status: String,
}

fn find_dat_dir<'a>(n: &'a RvFile, want: &str) -> Option<&'a RvFile> {
    if n.dir_dats
        .iter()
        .any(|d| d.get(dat_data::DAT_ROOT_FULL_NAME) == Some(want))
    {
        return Some(n);
    }
    n.children.iter().find_map(|c| find_dat_dir(c, want))
}

fn from_dat(n: &RvFile, primary: FileStatus, alt: FileStatus) -> bool {
    n.file_status.contains(primary) || n.file_status.contains(alt)
}

/// The values the DAT gave a cache node. A node that matched a scanned file by its
/// header-stripped hashes holds the DAT values in the `Alt*` fields (`SetAsAltFile`,
/// `RvFileCode.cs:239-268`); the `*FromDAT` flags say which fields are DAT values.
fn cache_rom(n: &RvFile, prefix: &str) -> Rom {
    let pick = |p: FileStatus, a: FileStatus, pv: &Option<Vec<u8>>, av: &Option<Vec<u8>>| {
        if n.file_status.contains(p) {
            pv.clone()
        } else if n.file_status.contains(a) {
            av.clone()
        } else {
            None
        }
    };
    let size = if n.file_status.contains(FileStatus::SIZE_FROM_DAT) {
        n.size
    } else if n.file_status.contains(FileStatus::ALT_SIZE_FROM_DAT) {
        n.alt_size
    } else {
        None
    };
    let _ = from_dat;
    Rom {
        name: format!("{prefix}{}", n.name),
        size,
        crc: pick(
            FileStatus::CRC_FROM_DAT,
            FileStatus::ALT_CRC_FROM_DAT,
            &n.crc,
            &n.alt_crc,
        ),
        sha1: pick(
            FileStatus::SHA1_FROM_DAT,
            FileStatus::ALT_SHA1_FROM_DAT,
            &n.sha1,
            &n.alt_sha1,
        ),
        md5: pick(
            FileStatus::MD5_FROM_DAT,
            FileStatus::ALT_MD5_FROM_DAT,
            &n.md5,
            &n.alt_md5,
        ),
        status: n.status.clone().unwrap_or_default(),
    }
}

#[derive(Default)]
struct CacheSide<'a> {
    /// game path (name without `.zip`) -> node
    games: BTreeMap<String, &'a RvFile>,
    /// Nodes under the DAT's directory that no DAT supplied (`DatStatus.NotInDat`): files the
    /// scanner found. Counted by (file type, depth class) so they can be reported.
    scanner_extras_games: usize,
    scanner_extras_other: usize,
}

fn cache_side<'a>(dn: &'a RvFile) -> CacheSide<'a> {
    fn go<'a>(n: &'a RvFile, prefix: &str, out: &mut CacheSide<'a>) {
        for c in &n.children {
            if c.dat_status == DatStatus::NOT_IN_DAT {
                if c.game.is_some() || c.file_type.0 == 2 {
                    out.scanner_extras_games += 1;
                } else {
                    out.scanner_extras_other += 1;
                }
                continue;
            }
            if c.game.is_some() {
                let mut name = c.name.clone();
                if c.file_type.0 == 2 {
                    name = name.strip_suffix(".zip").unwrap_or(&name).to_owned();
                }
                out.games.insert(format!("{prefix}{name}"), c);
            } else if c.file_type.0 == 1 {
                go(c, &format!("{prefix}{}/", c.name), out);
            }
        }
    }
    let mut out = CacheSide::default();
    go(dn, "", &mut out);
    out
}

fn cache_roms(game: &RvFile, out: &mut Vec<Rom>, extras: &mut usize) {
    fn go(n: &RvFile, prefix: &str, out: &mut Vec<Rom>, extras: &mut usize) {
        for c in &n.children {
            if c.dat_status == DatStatus::NOT_IN_DAT {
                *extras += 1;
                continue;
            }
            match c.file_type.0 {
                1 => go(c, &format!("{prefix}{}/", c.name), out, extras),
                4..=6 => out.push(cache_rom(c, prefix)),
                _ => {}
            }
        }
    }
    go(game, "", out, extras);
}

fn dat_games<'a>(d: &'a DatDir, prefix: &str, out: &mut BTreeMap<String, &'a DatDir>) {
    for c in &d.children {
        if let DatNode::Dir(x) = c {
            if x.game.is_some() {
                let key = format!("{prefix}{}", x.base.name);
                assert!(out.insert(key.clone(), x).is_none(), "duplicate game {key}");
            } else {
                dat_games(x, &format!("{prefix}{}/", x.base.name), out);
            }
        }
    }
}

fn dat_roms(g: &DatDir) -> Vec<Rom> {
    g.children
        .iter()
        .filter_map(|c| match c {
            DatNode::File(f) => Some(Rom {
                name: f.base.name.clone(),
                size: f.size,
                crc: f.crc.clone(),
                sha1: f.sha1.clone(),
                md5: f.md5.clone(),
                status: f.status.clone(),
            }),
            _ => None,
        })
        .collect()
}

#[derive(Default, Debug)]
struct Tally {
    games_dat: usize,
    games_cache: usize,
    only_dat: Vec<String>,
    only_cache: Vec<String>,
    rom_set_diffs: Vec<String>,
    header_diffs: Vec<String>,
    /// Explained differences, by cause.
    cloneof_from_cloneofid: usize,
    game_id_not_in_cache: usize,
    dat_games_with_game_id: usize,
    description_literal_in_cache: usize,
    /// Unexplained game metadata differences.
    meta_diffs: Vec<String>,
    extras_games: usize,
    extras_files: usize,
    extras_other: usize,
}

fn walk_all_ids(n: &RvFile, c: &mut BTreeMap<u8, usize>) {
    if let Some(g) = &n.game {
        for e in &g.entries {
            *c.entry(e.id).or_default() += 1;
        }
    }
    for ch in &n.children {
        walk_all_ids(ch, c);
    }
}

fn cross_check(cache: &CacheFile, dat_path: &Path, datroot: &Path) -> Tally {
    let rel = dat_path
        .strip_prefix(datroot)
        .unwrap()
        .to_string_lossy()
        .replace('/', "\\");
    let want = format!("DatRoot\\{rel}");
    let h = read_dat_file(dat_path).header.expect("header");
    let dn =
        find_dat_dir(&cache.root, &want).unwrap_or_else(|| panic!("no cache directory for {want}"));
    let mut t = Tally::default();

    // The cache's RvDat holds the DAT's header and its mtime.
    let rd = dn
        .dir_dats
        .iter()
        .find(|d| d.get(dat_data::DAT_ROOT_FULL_NAME) == Some(want.as_str()))
        .unwrap();
    for (n, id, v) in [
        ("id", dat_data::ID, &h.id),
        ("name", dat_data::DAT_NAME, &h.name),
        ("rootdir", dat_data::ROOT_DIR, &h.root_dir),
        ("description", dat_data::DESCRIPTION, &h.description),
        ("category", dat_data::CATEGORY, &h.category),
        ("version", dat_data::VERSION, &h.version),
        ("date", dat_data::DATE, &h.date),
        ("author", dat_data::AUTHOR, &h.author),
        ("email", dat_data::EMAIL, &h.email),
        ("homepage", dat_data::HOME_PAGE, &h.homepage),
        ("url", dat_data::URL, &h.url),
        ("dir", dat_data::DIR_SETUP, &h.dir),
        ("header", dat_data::HEADER, &h.header),
        ("compression", dat_data::COMPRESSION, &h.compression),
    ] {
        let cv = rd.get(id).unwrap_or("");
        if cv != v {
            t.header_diffs.push(format!("{n}: dat {v:?} cache {cv:?}"));
        }
    }

    let side = cache_side(dn);
    t.extras_games = side.scanner_extras_games;
    t.extras_other = side.scanner_extras_other;
    let mut dg = BTreeMap::new();
    dat_games(&h.base_dir, "", &mut dg);
    t.games_dat = dg.len();
    t.games_cache = side.games.len();
    let dk: BTreeSet<_> = dg.keys().cloned().collect();
    let ck: BTreeSet<_> = side.games.keys().cloned().collect();
    t.only_dat = dk.difference(&ck).cloned().collect();
    t.only_cache = ck.difference(&dk).cloned().collect();

    // DatSetMatchIDs (`DatID.cs:25-55`): the first game with an id names that id; a game with a
    // `cloneofid` that is known takes that game's name as its `cloneof`.
    let mut by_id: BTreeMap<&str, &str> = BTreeMap::new();
    for (k, g) in &dg {
        let id = g.game.as_ref().unwrap().id.as_str();
        if !id.is_empty() {
            by_id.entry(id).or_insert(k.rsplit('/').next().unwrap());
        }
    }

    for k in dk.intersection(&ck) {
        let mut d = dat_roms(dg[k]);
        let mut c = vec![];
        cache_roms(side.games[k], &mut c, &mut t.extras_files);
        d.sort();
        c.sort();
        if d != c {
            t.rom_set_diffs.push(format!("{k}: dat {d:?} cache {c:?}"));
        }

        let g: &DatGame = dg[k].game.as_ref().unwrap();
        t.dat_games_with_game_id += usize::from(!g.game_id.trim().is_empty());
        let rg = side.games[k].game.as_ref().expect("cache game");
        let short = k.rsplit('/').next().unwrap();
        let cloneof_want = match by_id.get(g.clone_of_id.as_str()) {
            Some(n) if !by_id.is_empty() && !g.clone_of_id.is_empty() => {
                if *n != g.clone_of {
                    t.cloneof_from_cloneofid += 1;
                }
                n.to_string()
            }
            _ => g.clone_of.clone(),
        };
        let category = g.category.join(" | ");
        for (n, id, want) in [
            // `ClearDescription` stores `¤` for a description equal to the file name without
            // its extension (`DatClearDescription.cs:8-25`).
            (
                "description",
                game_data::DESCRIPTION,
                if g.description == short {
                    "¤".to_owned()
                } else {
                    g.description.clone()
                },
            ),
            ("romof", game_data::ROM_OF, g.rom_of.clone()),
            ("isbios", game_data::IS_BIOS, g.is_bios.clone()),
            ("sourcefile", game_data::SOURCE_FILE, g.source_file.clone()),
            ("cloneof", game_data::CLONE_OF, cloneof_want),
            ("sampleof", game_data::SAMPLE_OF, g.sample_of.clone()),
            ("board", game_data::BOARD, g.board.clone()),
            ("year", game_data::YEAR, g.year.clone()),
            (
                "manufacturer",
                game_data::MANUFACTURER,
                g.manufacturer.clone(),
            ),
            ("id", game_data::ID, g.id.clone()),
            ("category", game_data::CATEGORY, category),
            ("cloneofid", game_data::CLONE_OF_ID, g.clone_of_id.clone()),
            ("serial", game_data::SERIAL, g.serial.clone()),
            ("version", game_data::VERSION, g.version.clone()),
            ("game_id", game_data::GAME_ID, g.game_id.clone()),
        ] {
            // `RvGame` drops empty and white-space values (`RvGame.cs:70-109`).
            let want = if want.trim().is_empty() {
                String::new()
            } else {
                want
            };
            let have = rg.get(id).unwrap_or("").to_owned();
            if have == want {
                continue;
            }
            match n {
                // The cache holds the description itself where 3.8 would hold `¤`.
                "description" if want == "¤" && have == short => {
                    t.description_literal_in_cache += 1
                }
                // The caches hold no game_id at all (checked over the whole tree).
                "game_id" if have.is_empty() => t.game_id_not_in_cache += 1,
                _ => t
                    .meta_diffs
                    .push(format!("{k}: {n}: dat {want:?} cache {have:?}")),
            }
        }
    }
    t
}

#[test]
#[ignore = "needs RV_FIXTURES_DIR"]
fn cross_check_dat_model_against_the_real_cache() {
    let dir = fixtures_dir();
    let cache_path = dir.join("RomVault3_3.Cache");
    let cache = CacheFile::from_bytes(
        &std::fs::read(&cache_path).unwrap_or_else(|e| panic!("{}: {e}", cache_path.display())),
    )
    .expect("cache parses");

    // The caches store no `game_id` for any game: ROMVault 3.7 wrote them (plan comment) and
    // `GameId` is a 3.8 field. That explains every `game_id` difference below.
    let mut ids = BTreeMap::new();
    walk_all_ids(&cache.root, &mut ids);
    println!("game metadata ids present anywhere in the cache: {ids:?}");
    assert!(!ids.contains_key(&game_data::GAME_ID), "{ids:?}");

    println!("| DAT | games dat/cache | only-dat | only-cache | roms differ | scanner-only nodes (games / files in games / other) | cloneof via cloneofid | game_id not in cache | literal description | unexplained |");
    println!("|---|---|---|---|---|---|---|---|---|---|");
    let datroot = dir.join("DatRoot");
    let mut total_unexplained = 0;
    let mut gc_literal = 0;
    for f in dat_files() {
        let t = cross_check(&cache, &f, &datroot);
        let unexplained = t.only_dat.len()
            + t.only_cache.len()
            + t.rom_set_diffs.len()
            + t.header_diffs.len()
            + t.meta_diffs.len();
        println!(
            "| {} | {}/{} | {} | {} | {} | {} / {} / {} | {} | {} | {} | {} |",
            f.file_name().unwrap().to_string_lossy(),
            t.games_dat,
            t.games_cache,
            t.only_dat.len(),
            t.only_cache.len(),
            t.rom_set_diffs.len(),
            t.extras_games,
            t.extras_files,
            t.extras_other,
            t.cloneof_from_cloneofid,
            t.game_id_not_in_cache,
            t.description_literal_in_cache,
            unexplained
        );
        for line in t
            .only_dat
            .iter()
            .chain(&t.only_cache)
            .chain(&t.rom_set_diffs)
            .chain(&t.header_diffs)
            .chain(&t.meta_diffs)
            .take(10)
        {
            println!("    UNEXPLAINED {line}");
        }
        // Every `game_id` the DAT has is missing from the cache, and nothing else is.
        assert_eq!(t.game_id_not_in_cache, t.dat_games_with_game_id, "{f:?}");
        total_unexplained += unexplained;
        if t.description_literal_in_cache > 0 {
            gc_literal += 1;
            assert!(
                f.to_string_lossy().contains("GameCube"),
                "literal descriptions only expected for the GC DAT: {f:?}"
            );
            assert_eq!(t.description_literal_in_cache, t.games_dat, "all GC games");
        }
    }
    assert_eq!(gc_literal, 1);
    assert_eq!(total_unexplained, 0);
}

// ---------------------------------------------------------------------------------------
// Write round trip on the real DATs
// ---------------------------------------------------------------------------------------

#[test]
#[ignore = "needs RV_FIXTURES_DIR"]
fn real_dats_round_trip_through_the_xml_writer() {
    for f in dat_files() {
        let name = f.file_name().unwrap().to_string_lossy().into_owned();
        let h = read_dat_file(&f).header.unwrap();
        let text = write_xml(&h, false);
        let back = read_dat(text.as_bytes(), &h.filename).unwrap_or_else(|e| panic!("{name}: {e}"));
        if let Some(d) = common::diff(&back, &common::xml_header(&h)) {
            panic!("{name}: {d}");
        }
        // For these DATs the projection loses almost nothing; say how much, per DAT.
        let kept = back.base_dir.rom_count();
        assert_eq!(kept, h.base_dir.rom_count(), "{name}: every rom survives");
        assert_eq!(
            back.base_dir.game_count(),
            h.base_dir.game_count(),
            "{name}"
        );

        // And through the MAME writer.
        let text = write_mame_xml(&h);
        let back =
            read_dat(text.as_bytes(), &h.filename).unwrap_or_else(|e| panic!("{name}: mame {e}"));
        if let Some(d) = common::diff(&back, &common::mame_header(&h)) {
            panic!("{name}: mame {d}");
        }
        println!(
            "{name}: XML and MAME writer round trips equal ({} games, {} roms)",
            kept,
            back.base_dir.rom_count()
        );
    }
}
