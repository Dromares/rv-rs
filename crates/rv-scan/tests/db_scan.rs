//! The directory scan on small trees in a temp directory: a DAT is loaded into a database, files
//! are put on disk, and the scan has to give every item the status ROMVault gives it.

use std::fs;
use std::io::Cursor;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use rv_archive::zip::{TorrentZipWriter, ZipStructure};
use rv_core::progress::{CancelFlag, NoProgress, ProgressEvent};
use rv_core::{
    DatStatus, Database, FileStatus, FileType, GotStatus, HeaderFileType, RepStatus, RvFile,
    ScanLevel, Settings,
};
use rv_scan::db::{scan_files, ScanReport, ScanRequest, ScanTarget};
use rv_scan::{HashKinds, HashSet};

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Tmp {
        let dir = std::env::temp_dir().join(format!("rv-scan-db-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Tmp(dir)
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.0.join(rel)
    }

    fn settings(&self) -> Settings {
        let mut s = Settings::default();
        s.dat_root = self.path("DatRoot").to_string_lossy().into_owned();
        s.dir_mappings[0].dir_path = self.path("RomRoot").to_string_lossy().into_owned();
        s
    }

    fn write(&self, rel: &str, bytes: &[u8]) -> PathBuf {
        let p = self.path(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, bytes).unwrap();
        p
    }

    fn set_mtime(&self, rel: &str, secs: u64) {
        let f = fs::OpenOptions::new()
            .write(true)
            .open(self.path(rel))
            .unwrap();
        f.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
            .unwrap();
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Hashes {
    size: u64,
    crc: String,
    sha1: String,
    md5: String,
}

fn hashes(data: &[u8]) -> Hashes {
    let mut h = HashSet::new(HashKinds::ROMVAULT);
    h.update(data);
    let d = h.finish();
    Hashes {
        size: data.len() as u64,
        crc: rv_scan::hash::to_hex(&d.crc.unwrap()),
        sha1: rv_scan::hash::to_hex(&d.sha1.unwrap()),
        md5: rv_scan::hash::to_hex(&d.md5.unwrap()),
    }
}

fn zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut w = TorrentZipWriter::new(Cursor::new(Vec::new()), ZipStructure::ZipTrrnt).unwrap();
    for (name, data) in entries {
        w.add_bytes(name, data, None).unwrap();
    }
    w.finish().unwrap().into_inner()
}

/// A DAT with the games `(name, [(rom, data)])`, hashes written as CRC only.
fn dat_text(header: &str, games: &[(&str, &[(&str, &[u8])])]) -> String {
    let mut s = format!("clrmamepro (\n\tname \"T\"\n{header})\n");
    for (name, roms) in games {
        s.push_str(&format!("game (\n\tname \"{name}\"\n"));
        for (r, data) in *roms {
            let h = hashes(data);
            s.push_str(&format!(
                "\trom ( name \"{r}\" size {} crc {} )\n",
                h.size, h.crc
            ));
        }
        s.push_str(")\n");
    }
    s
}

fn load(t: &Tmp, settings: &Settings, dat: &str) -> Database {
    t.write("DatRoot/Console/NES/Roms/nes.dat", dat.as_bytes());
    let mut db = Database::default_db();
    db.report_status_reset(settings);
    rv_dat::update::update_dats(
        &mut db,
        settings,
        &Default::default(),
        &NoProgress,
        &CancelFlag::new(),
    )
    .unwrap();
    db
}

const ROMS: &str = "RomVault/Console/NES/Roms";

fn node<'a>(db: &'a Database, path: &str) -> &'a RvFile {
    let mut cur = &db.root;
    for part in path.split('/') {
        cur = cur
            .children
            .iter()
            .find(|c| c.name == part)
            .unwrap_or_else(|| {
                panic!(
                    "{part} not found in {:?} (path {path})",
                    cur.children.iter().map(|c| &c.name).collect::<Vec<_>>()
                )
            });
    }
    cur
}

fn scan(db: &mut Database, s: &Settings, level: ScanLevel) -> ScanReport {
    scan_dir(db, s, level, ROMS)
}

fn scan_dir(db: &mut Database, s: &Settings, level: ScanLevel, dir: &str) -> ScanReport {
    scan_files(
        db,
        s,
        &ScanRequest {
            level,
            target: ScanTarget::Dir(dir.to_owned()),
        },
        &NoProgress,
        &CancelFlag::new(),
    )
    .unwrap()
}

fn statuses(n: &RvFile) -> Vec<(String, RepStatus)> {
    n.children
        .iter()
        .map(|c| (c.name.clone(), c.rep_status))
        .collect()
}

const A: &[u8] = b"rom a contents rom a contents";
const B: &[u8] = b"second rom";

// ---- archives ------------------------------------------------------------------------------------

#[test]
fn a_zip_that_matches_its_dat_is_correct_and_gets_its_hashes() {
    let t = Tmp::new("correct");
    let s = t.settings();
    let mut db = load(
        &t,
        &s,
        &dat_text("", &[("Alpha", &[("a.nes", A), ("b.nes", B)])]),
    );
    t.write(
        "RomRoot/Console/NES/Roms/Alpha.zip",
        &zip(&[("a.nes", A), ("b.nes", B)]),
    );
    let r = scan(&mut db, &s, ScanLevel::Level2);
    assert_eq!(r.roots_scanned, [ROMS]);
    assert_eq!((r.archives_scanned, r.files_hashed), (1, 0));
    assert!(r.errors.is_empty() && !r.cancelled && !r.aborted, "{r:?}");

    let zip_node = node(&db, &format!("{ROMS}/Alpha.zip"));
    assert_eq!(zip_node.got_status, GotStatus::GOT);
    assert_eq!(zip_node.rep_status, RepStatus::DIR_CORRECT);
    assert_eq!(zip_node.zip_struct, Some(rv_core::ZipStructure::ZIP_TRRNT));
    assert_ne!(zip_node.file_mod_time_stamp, i64::MIN);
    let a = &zip_node.children[0];
    let h = hashes(A);
    assert_eq!(
        (a.name.as_str(), a.got_status, a.rep_status),
        ("a.nes", GotStatus::GOT, RepStatus::CORRECT)
    );
    assert_eq!(a.size, Some(h.size));
    // The DAT gave size and CRC; the scan filled in what the DAT lacked and verified it.
    assert_eq!(rv_scan::hash::to_hex(a.sha1.as_ref().unwrap()), h.sha1);
    assert_eq!(rv_scan::hash::to_hex(a.md5.as_ref().unwrap()), h.md5);
    let want = FileStatus::SIZE_FROM_DAT
        .with(FileStatus::CRC_FROM_DAT)
        .with(FileStatus::SIZE_VERIFIED)
        .with(FileStatus::CRC_VERIFIED)
        .with(FileStatus::SHA1_VERIFIED)
        .with(FileStatus::MD5_VERIFIED)
        .with(FileStatus::SIZE_FROM_HEADER)
        .with(FileStatus::CRC_FROM_HEADER);
    assert!(a.file_status.has(want), "{:#x}", a.file_status.0);
    assert_eq!(a.zip_file_index, Some(0));
    assert_eq!(zip_node.children[1].zip_file_index, Some(1));
    assert_eq!(zip_node.children[0].zip_file_header_position, Some(0));
    assert!(zip_node.is_deep_scanned(&s));
    let counts = node(&db, ROMS).dir_status(true);
    assert_eq!(
        (
            counts.get(RepStatus::CORRECT),
            counts.get(RepStatus::DIR_CORRECT)
        ),
        (2, 1)
    );
}

#[test]
fn games_without_files_stay_missing_and_unknown_files_are_added_as_unknown() {
    let t = Tmp::new("missing");
    let s = t.settings();
    let mut db = load(
        &t,
        &s,
        &dat_text("", &[("Alpha", &[("a.nes", A)]), ("Beta", &[("b.nes", B)])]),
    );
    t.write(
        "RomRoot/Console/NES/Roms/Alpha.zip",
        &zip(&[("a.nes", A), ("extra.txt", b"x")]),
    );
    t.write(
        "RomRoot/Console/NES/Roms/Stranger.zip",
        &zip(&[("s.nes", b"zzz")]),
    );
    t.write("RomRoot/Console/NES/Roms/notes.txt", b"hello");
    scan(&mut db, &s, ScanLevel::Level2);

    let roms = node(&db, ROMS);
    assert_eq!(
        statuses(roms),
        [
            ("Alpha.zip".into(), RepStatus::DIR_CORRECT),
            ("Beta.zip".into(), RepStatus::DIR_MISSING),
            // Files and archives sort together by lower-cased name.
            ("notes.txt".into(), RepStatus::UNKNOWN),
            ("Stranger.zip".into(), RepStatus::DIR_UNKNOWN),
        ]
    );
    // An extra entry in a zip that is in the DAT is an unknown file next to the DAT's.
    let alpha = &roms.children[0];
    assert_eq!(
        statuses(alpha),
        [
            ("a.nes".into(), RepStatus::CORRECT),
            ("extra.txt".into(), RepStatus::UNKNOWN)
        ]
    );
    assert_eq!(alpha.children[1].dat_status, DatStatus::NOT_IN_DAT);
    assert_eq!(roms.children[1].children[0].rep_status, RepStatus::MISSING);
    let notes = &roms.children[2];
    assert_eq!(notes.file_type, FileType::FILE);
    assert_eq!(notes.size, Some(5));
    assert!(notes
        .file_status
        .has(FileStatus::SIZE_VERIFIED.with(FileStatus::CRC_VERIFIED)));
    assert_eq!(
        notes.zip_file_index,
        Some(0),
        "a scanned item starts with ScannedFile.Index 0"
    );
}

#[test]
fn a_rom_with_the_right_name_and_wrong_contents_is_missing_and_the_file_is_unknown() {
    let t = Tmp::new("wrong");
    let s = t.settings();
    let mut db = load(&t, &s, &dat_text("", &[("Alpha", &[("a.nes", A)])]));
    t.write(
        "RomRoot/Console/NES/Roms/Alpha.zip",
        &zip(&[("a.nes", b"not the rom")]),
    );
    scan(&mut db, &s, ScanLevel::Level2);
    let alpha = node(&db, &format!("{ROMS}/Alpha.zip"));
    assert_eq!(
        alpha.rep_status,
        RepStatus::DIR_CORRECT,
        "the zip itself is the DAT's game"
    );
    assert_eq!(
        statuses(alpha),
        [
            ("a.nes".into(), RepStatus::MISSING),
            ("a.nes".into(), RepStatus::UNKNOWN)
        ]
    );
}

#[test]
fn an_entry_whose_data_does_not_match_its_header_is_corrupt_and_keeps_the_crc_it_has() {
    // A stored zip whose central directory claims a CRC the data does not have.
    let data = b"stored data";
    let wrong = hashes(b"something else");
    let crc = u32::from_str_radix(&wrong.crc, 16).unwrap();
    let mut z = Vec::new();
    let name = b"a.nes";
    z.extend(b"PK\x03\x04");
    z.extend([10, 0, 0, 0, 0, 0, 0, 0, 0x21, 0]);
    z.extend(crc.to_le_bytes());
    z.extend((data.len() as u32).to_le_bytes());
    z.extend((data.len() as u32).to_le_bytes());
    z.extend((name.len() as u16).to_le_bytes());
    z.extend([0, 0]);
    z.extend(name);
    z.extend(data);
    let cd = z.len();
    z.extend(b"PK\x01\x02");
    z.extend([10, 0, 10, 0, 0, 0, 0, 0, 0, 0, 0x21, 0]);
    z.extend(crc.to_le_bytes());
    z.extend((data.len() as u32).to_le_bytes());
    z.extend((data.len() as u32).to_le_bytes());
    z.extend((name.len() as u16).to_le_bytes());
    z.extend([0u8; 12]);
    z.extend(0u32.to_le_bytes());
    z.extend(name);
    let cd_size = z.len() - cd;
    z.extend(b"PK\x05\x06");
    z.extend([0, 0, 0, 0, 1, 0, 1, 0]);
    z.extend((cd_size as u32).to_le_bytes());
    z.extend((cd as u32).to_le_bytes());
    z.extend([0, 0]);

    let t = Tmp::new("corrupt");
    let s = t.settings();
    let mut db = load(&t, &s, &dat_text("", &[("Alpha", &[("a.nes", data)])]));
    t.write("RomRoot/Console/NES/Roms/Alpha.zip", &z);
    scan(&mut db, &s, ScanLevel::Level2);
    let alpha = node(&db, &format!("{ROMS}/Alpha.zip"));
    let e = alpha
        .children
        .iter()
        .find(|c| c.got_status == GotStatus::CORRUPT)
        .expect("a corrupt entry");
    assert_eq!(e.rep_status, RepStatus::CORRUPT);
    // The entry has the CRC of the bytes it holds, which is the DAT's, not the header's wrong one.
    assert_eq!(
        rv_scan::hash::to_hex(e.crc.as_ref().unwrap()),
        hashes(data).crc
    );
    assert_ne!(rv_scan::hash::to_hex(e.crc.as_ref().unwrap()), wrong.crc);
}

#[test]
fn a_file_that_is_not_a_zip_makes_a_corrupt_archive() {
    let t = Tmp::new("notzip");
    let s = t.settings();
    let mut db = load(&t, &s, &dat_text("", &[("Alpha", &[("a.nes", A)])]));
    t.write(
        "RomRoot/Console/NES/Roms/Alpha.zip",
        b"this is not a zip file at all",
    );
    let r = scan(&mut db, &s, ScanLevel::Level2);
    let alpha = node(&db, &format!("{ROMS}/Alpha.zip"));
    assert_eq!(alpha.got_status, GotStatus::CORRUPT);
    assert_eq!(alpha.rep_status, RepStatus::DIR_CORRUPT);
    assert_eq!(
        alpha.children[0].rep_status,
        RepStatus::MISSING,
        "its entries are missing"
    );
    assert_eq!(r.errors.len(), 1, "{r:?}");
}

#[test]
fn a_zip_that_cannot_be_opened_is_unscanned_not_corrupt() {
    use std::os::unix::fs::PermissionsExt;
    let t = Tmp::new("locked");
    let s = t.settings();
    let mut db = load(&t, &s, &dat_text("", &[("Alpha", &[("a.nes", A)])]));
    let p = t.write("RomRoot/Console/NES/Roms/Alpha.zip", &zip(&[("a.nes", A)]));
    fs::set_permissions(&p, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::File::open(&p).is_ok() {
        // Running as root: permissions do not apply, there is nothing to test.
        return;
    }
    scan(&mut db, &s, ScanLevel::Level2);
    let alpha = node(&db, &format!("{ROMS}/Alpha.zip"));
    assert_eq!(alpha.got_status, GotStatus::FILE_LOCKED);
    assert_eq!(alpha.rep_status, RepStatus::UN_SCANNED);
    assert_eq!(alpha.file_mod_time_stamp, i64::MIN);
    // Its entries inherit the lock rather than saying "missing".
    assert_eq!(alpha.children[0].got_status, GotStatus::FILE_LOCKED);
    fs::set_permissions(&p, fs::Permissions::from_mode(0o644)).unwrap();
}

#[test]
fn rescanning_changes_nothing_and_reads_no_archive() {
    let t = Tmp::new("stable");
    let s = t.settings();
    let mut db = load(&t, &s, &dat_text("", &[("Alpha", &[("a.nes", A)])]));
    t.write("RomRoot/Console/NES/Roms/Alpha.zip", &zip(&[("a.nes", A)]));
    t.write("RomRoot/Console/NES/Roms/loose.bin", b"loose");
    scan(&mut db, &s, ScanLevel::Level2);
    let first = rv_cache::to_cache_file(&mut db)
        .unwrap()
        .to_bytes()
        .unwrap();
    let r = scan(&mut db, &s, ScanLevel::Level2);
    assert_eq!(
        (r.archives_scanned, r.files_hashed),
        (0, 0),
        "nothing changed: {r:?}"
    );
    let second = rv_cache::to_cache_file(&mut db)
        .unwrap()
        .to_bytes()
        .unwrap();
    assert_eq!(first, second);
    // A deeper scan looks at everything again and finds the same.
    let r = scan(&mut db, &s, ScanLevel::Level3);
    assert_eq!((r.archives_scanned, r.files_hashed), (1, 1));
    assert_eq!(
        rv_cache::to_cache_file(&mut db)
            .unwrap()
            .to_bytes()
            .unwrap(),
        first
    );
}

#[test]
fn a_changed_zip_is_read_again_and_a_deleted_one_goes_missing() {
    let t = Tmp::new("changed");
    let s = t.settings();
    let mut db = load(
        &t,
        &s,
        &dat_text("", &[("Alpha", &[("a.nes", A)]), ("Beta", &[("b.nes", B)])]),
    );
    t.write("RomRoot/Console/NES/Roms/Alpha.zip", &zip(&[("a.nes", A)]));
    t.write("RomRoot/Console/NES/Roms/Beta.zip", &zip(&[("b.nes", B)]));
    scan(&mut db, &s, ScanLevel::Level2);
    // Alpha is replaced by one without its rom; Beta is deleted.
    t.write(
        "RomRoot/Console/NES/Roms/Alpha.zip",
        &zip(&[("other.nes", b"o")]),
    );
    t.set_mtime("RomRoot/Console/NES/Roms/Alpha.zip", 1_800_000_000);
    fs::remove_file(t.path("RomRoot/Console/NES/Roms/Beta.zip")).unwrap();
    let r = scan(&mut db, &s, ScanLevel::Level2);
    assert_eq!(r.archives_scanned, 1);
    let roms = node(&db, ROMS);
    assert_eq!(roms.children[1].name, "Beta.zip");
    assert_eq!(roms.children[1].got_status, GotStatus::NOT_GOT);
    assert_eq!(roms.children[1].rep_status, RepStatus::DIR_MISSING);
    assert_eq!(roms.children[1].children[0].rep_status, RepStatus::MISSING);
    assert_eq!(roms.children[1].file_mod_time_stamp, i64::MIN);
    assert_eq!(
        statuses(&roms.children[0]),
        [
            ("a.nes".into(), RepStatus::MISSING),
            ("other.nes".into(), RepStatus::UNKNOWN)
        ]
    );
}

#[test]
fn a_file_that_disappears_is_removed_when_nothing_knows_it_and_kept_missing_when_a_dat_does() {
    let t = Tmp::new("gone");
    let s = t.settings();
    let mut db = load(&t, &s, &dat_text("", &[("Alpha", &[("a.nes", A)])]));
    t.write("RomRoot/Console/NES/Roms/Alpha.zip", &zip(&[("a.nes", A)]));
    t.write(
        "RomRoot/Console/NES/Roms/Stranger.zip",
        &zip(&[("s", b"s")]),
    );
    scan(&mut db, &s, ScanLevel::Level2);
    assert_eq!(node(&db, ROMS).children.len(), 2);
    fs::remove_file(t.path("RomRoot/Console/NES/Roms/Alpha.zip")).unwrap();
    fs::remove_file(t.path("RomRoot/Console/NES/Roms/Stranger.zip")).unwrap();
    scan(&mut db, &s, ScanLevel::Level2);
    let roms = node(&db, ROMS);
    assert_eq!(
        statuses(roms),
        [("Alpha.zip".into(), RepStatus::DIR_MISSING)]
    );
}

#[test]
fn a_missing_directory_marks_everything_in_it_missing() {
    let t = Tmp::new("nodir");
    let s = t.settings();
    let mut db = load(&t, &s, &dat_text("", &[("Alpha", &[("a.nes", A)])]));
    let r = scan(&mut db, &s, ScanLevel::Level2);
    assert_eq!(r.roots_missing, [ROMS]);
    assert!(r.roots_scanned.is_empty());
    assert_eq!(
        node(&db, ROMS).children[0].rep_status,
        RepStatus::DIR_MISSING
    );
}

// ---- copier headers --------------------------------------------------------------------------------

fn nes(body: &[u8]) -> Vec<u8> {
    let mut v = vec![b'N', b'E', b'S', 0x1A, 2, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    v.extend(body);
    v
}

#[test]
fn a_headered_file_matches_a_headerless_dat_through_its_alt_hashes_md5_included() {
    // The DAT lists the MD5 of the body without the 16-byte header and nothing else, so the
    // match can only come from the alt MD5 test (ROMVault's loose-file version has it inverted).
    let body = b"nes program rom data";
    let h = hashes(body);
    let dat = format!(
        "clrmamepro (\n\tname \"T\"\n\theader \"No-Intro_NES.xml\"\n)\ngame (\n\tname \"Alpha\"\n\trom ( name \"a.nes\" size {} md5 {} )\n)\n",
        h.size, h.md5
    );
    let t = Tmp::new("header");
    let s = t.settings();
    let mut db = load(&t, &s, &dat);
    t.write(
        "RomRoot/Console/NES/Roms/Alpha.zip",
        &zip(&[("a.nes", &nes(body))]),
    );
    scan(&mut db, &s, ScanLevel::Level2);
    let alpha = node(&db, &format!("{ROMS}/Alpha.zip"));
    let a = &alpha.children[0];
    assert_eq!(a.rep_status, RepStatus::CORRECT, "{:?}", statuses(alpha));
    assert_eq!(alpha.children.len(), 1);
    // The item now holds the file's headered hashes, and the DAT's as alt hashes.
    assert_eq!(a.size, Some(h.size + 16));
    assert_eq!(a.alt_size, Some(h.size));
    assert_eq!(rv_scan::hash::to_hex(a.alt_md5.as_ref().unwrap()), h.md5);
    assert!(a
        .file_status
        .has(FileStatus::ALT_MD5_FROM_DAT.with(FileStatus::HEADER_FILE_TYPE_FROM_HEADER)));
    assert_eq!(a.header_type(), HeaderFileType::NES);
}

// ---- loose files, levels, directories ------------------------------------------------------------

fn file_only_settings(t: &Tmp) -> Settings {
    let mut s = t.settings();
    s.dat_rules[0].compression = FileType::FILE_ONLY;
    s
}

#[test]
fn loose_files_in_a_game_directory_match_by_hash_and_are_not_hashed_twice() {
    let t = Tmp::new("loose");
    let s = file_only_settings(&t);
    let mut db = load(&t, &s, &dat_text("", &[("Alpha", &[("a.nes", A)])]));
    assert_eq!(node(&db, &format!("{ROMS}/Alpha")).file_type, FileType::DIR);
    t.write("RomRoot/Console/NES/Roms/Alpha/a.nes", A);
    t.write("RomRoot/Console/NES/Roms/Alpha/b.nes", B);
    let r = scan(&mut db, &s, ScanLevel::Level2);
    assert_eq!(r.files_hashed, 2);
    let alpha = node(&db, &format!("{ROMS}/Alpha"));
    assert_eq!(
        statuses(alpha),
        [
            ("a.nes".into(), RepStatus::CORRECT),
            ("b.nes".into(), RepStatus::UNKNOWN)
        ]
    );
    assert!(alpha.children[0].is_deep_scanned(&s));
    // Level 1 matches by time stamp and size alone; level 2 only does that for deep-scanned items.
    let r = scan(&mut db, &s, ScanLevel::Level1);
    assert_eq!(r.files_hashed, 0);
    let r = scan(&mut db, &s, ScanLevel::Level2);
    assert_eq!(r.files_hashed, 0);
    // Touch a file: its time stamp changed, so it is hashed again.
    t.set_mtime("RomRoot/Console/NES/Roms/Alpha/a.nes", 1_800_000_000);
    let r = scan(&mut db, &s, ScanLevel::Level2);
    assert_eq!(r.files_hashed, 1);
    assert_eq!(
        node(&db, &format!("{ROMS}/Alpha")).children[0].rep_status,
        RepStatus::CORRECT
    );
}

#[test]
fn a_loose_file_with_the_same_stamp_and_size_but_other_contents_is_not_noticed_below_level_3() {
    let t = Tmp::new("level3");
    let s = file_only_settings(&t);
    let mut db = load(&t, &s, &dat_text("", &[("Alpha", &[("a.nes", A)])]));
    let p = t.write("RomRoot/Console/NES/Roms/Alpha/a.nes", A);
    scan(&mut db, &s, ScanLevel::Level2);
    let stamp = fs::metadata(&p).unwrap().modified().unwrap();
    let mut bad = A.to_vec();
    bad[0] ^= 0xff;
    fs::write(&p, &bad).unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(&p)
        .unwrap()
        .set_modified(stamp)
        .unwrap();
    scan(&mut db, &s, ScanLevel::Level2);
    assert_eq!(
        node(&db, &format!("{ROMS}/Alpha")).children[0].rep_status,
        RepStatus::CORRECT
    );
    scan(&mut db, &s, ScanLevel::Level3);
    let alpha = node(&db, &format!("{ROMS}/Alpha"));
    assert_eq!(
        alpha.children[0].rep_status,
        RepStatus::MISSING,
        "{:?}",
        statuses(alpha)
    );
}

#[test]
fn new_directories_are_scanned_into_the_tree_and_names_pair_with_the_exact_case_first() {
    let t = Tmp::new("dirs");
    let s = file_only_settings(&t);
    let mut db = load(&t, &s, &dat_text("", &[("Alpha", &[("a.nes", A)])]));
    t.write("RomRoot/Console/NES/Roms/Alpha/a.nes", A);
    t.write("RomRoot/Console/NES/Roms/Alpha/sub/deep/x.bin", b"x");
    t.write("RomRoot/Console/NES/Roms/Alpha/File.BIN", b"upper");
    t.write("RomRoot/Console/NES/Roms/Alpha/file.bin", b"lower");
    scan(&mut db, &s, ScanLevel::Level2);
    let alpha = node(&db, &format!("{ROMS}/Alpha"));
    let names: Vec<&str> = alpha.children.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["a.nes", "File.BIN", "file.bin", "sub"]);
    let sub = alpha.children.iter().find(|c| c.name == "sub").unwrap();
    assert_eq!(sub.file_type, FileType::DIR);
    assert_eq!(sub.rep_status, RepStatus::DIR_UNKNOWN);
    assert_eq!(sub.children[0].children[0].name, "x.bin");
    assert_eq!(sub.children[0].children[0].rep_status, RepStatus::UNKNOWN);
}

// ---- ignore lists, temp files, ToSort ---------------------------------------------------------

#[test]
fn ignore_patterns_label_or_skip_and_stale_temp_files_are_reported_not_deleted() {
    let t = Tmp::new("ignore");
    let mut s = file_only_settings(&t);
    // The patterns of the rule that covers the directory are used; the global list only where no
    // rule covers (ToSort).
    s.dat_rules[0].ignore_files = vec!["*.nfo".to_owned(), "ignore:*.bak".to_owned()];
    s.ignore_files = vec!["*.bin".to_owned()];
    s.set_regex_rules().unwrap();
    let mut db = load(&t, &s, &dat_text("", &[("Alpha", &[("a.nes", A)])]));
    t.write("RomRoot/Console/NES/Roms/Alpha/a.nes", A);
    t.write("RomRoot/Console/NES/Roms/Alpha/readme.NFO", b"info");
    t.write("RomRoot/Console/NES/Roms/Alpha/old.bak", b"backup");
    let tmp = t.write("RomRoot/Console/NES/Roms/Alpha/__RomVault.123.tmp", b"half");
    let r = scan(&mut db, &s, ScanLevel::Level2);
    let alpha = node(&db, &format!("{ROMS}/Alpha"));
    let names: Vec<(&str, RepStatus)> = alpha
        .children
        .iter()
        .map(|c| (c.name.as_str(), c.rep_status))
        .collect();
    // `ignore:` files are not in the tree at all; the other pattern labels them.
    assert_eq!(
        names,
        [
            ("a.nes", RepStatus::CORRECT),
            ("readme.NFO", RepStatus::IGNORE)
        ]
    );
    assert!(tmp.exists(), "the scanner only reads");
    assert!(r
        .errors
        .iter()
        .any(|(f, m)| f.ends_with("__RomVault.123.tmp") && m.contains("Stale")));
}

#[test]
fn files_below_a_tosort_directory_are_in_tosort() {
    let t = Tmp::new("tosort");
    let mut s = t.settings();
    s.dir_mappings.push(rv_core::DirMapping {
        dir_key: "ToSort".to_owned(),
        dir_path: t.path("ToSort").to_string_lossy().into_owned(),
    });
    let mut db = Database::default_db();
    db.report_status_reset(&s);
    t.write("ToSort/loose.bin", b"loose");
    t.write("ToSort/pack.zip", &zip(&[("inside", b"inside")]));
    t.write("ToSort/sub/deeper.bin", b"deeper");
    let r = scan_files(
        &mut db,
        &s,
        &ScanRequest {
            level: ScanLevel::Level2,
            target: ScanTarget::Dir("ToSort".to_owned()),
        },
        &NoProgress,
        &CancelFlag::new(),
    )
    .unwrap();
    assert_eq!(r.roots_scanned, ["ToSort"]);
    let to_sort = node(&db, "ToSort");
    assert_eq!(to_sort.rep_status, RepStatus::DIR_IN_TO_SORT);
    let mut all = Vec::new();
    fn walk(n: &RvFile, out: &mut Vec<(String, DatStatus, RepStatus)>) {
        for c in &n.children {
            out.push((c.name.clone(), c.dat_status, c.rep_status));
            walk(c, out);
        }
    }
    walk(to_sort, &mut all);
    assert!(!all.is_empty());
    for (name, ds, rs) in &all {
        assert_eq!(*ds, DatStatus::IN_TO_SORT, "{name}");
        assert!(
            *rs == RepStatus::IN_TO_SORT || *rs == RepStatus::DIR_IN_TO_SORT,
            "{name}: {}",
            rs.name()
        );
    }
}

// ---- API: progress, cancel, targets ---------------------------------------------------------------

#[test]
fn cancelling_stops_the_scan_between_items_and_leaves_a_consistent_tree() {
    let t = Tmp::new("cancel");
    let s = t.settings();
    let mut db = load(
        &t,
        &s,
        &dat_text("", &[("Alpha", &[("a.nes", A)]), ("Beta", &[("b.nes", B)])]),
    );
    t.write("RomRoot/Console/NES/Roms/Alpha.zip", &zip(&[("a.nes", A)]));
    t.write("RomRoot/Console/NES/Roms/Beta.zip", &zip(&[("b.nes", B)]));
    let cancel = CancelFlag::new();
    cancel.cancel();
    let r = scan_files(
        &mut db,
        &s,
        &ScanRequest {
            level: ScanLevel::Level2,
            target: ScanTarget::Dir(ROMS.to_owned()),
        },
        &NoProgress,
        &cancel,
    )
    .unwrap();
    assert!(r.cancelled);
    assert!(
        r.roots_scanned.is_empty(),
        "cancelled before the first directory"
    );
    rv_cache::to_cache_file(&mut db).unwrap();
}

#[test]
fn progress_names_the_directory_and_the_archive_being_read() {
    let t = Tmp::new("progress");
    let s = t.settings();
    let mut db = load(&t, &s, &dat_text("", &[("Alpha", &[("a.nes", A)])]));
    t.write("RomRoot/Console/NES/Roms/Alpha.zip", &zip(&[("a.nes", A)]));
    let seen = std::cell::RefCell::new(Vec::new());
    let sink = |e: ProgressEvent| seen.borrow_mut().push(e);
    scan_files(
        &mut db,
        &s,
        &ScanRequest {
            level: ScanLevel::Level2,
            target: ScanTarget::Dir(ROMS.to_owned()),
        },
        &sink,
        &CancelFlag::new(),
    )
    .unwrap();
    let seen = seen.into_inner();
    assert!(seen.iter().any(|e| matches!(e, ProgressEvent::Text(t) if t.starts_with("Scanning Dir : ") && t.ends_with("Console/NES/Roms"))));
    assert!(seen
        .iter()
        .any(|e| matches!(e, ProgressEvent::Item(i) if i.ends_with("Alpha.zip"))));
}

#[test]
fn a_target_that_is_not_in_the_tree_is_an_error_and_selected_scans_the_ticked_rows() {
    let t = Tmp::new("targets");
    let s = t.settings();
    let mut db = load(&t, &s, &dat_text("", &[("Alpha", &[("a.nes", A)])]));
    let e = scan_files(
        &mut db,
        &s,
        &ScanRequest {
            level: ScanLevel::Level2,
            target: ScanTarget::Dir("RomVault/Nope".to_owned()),
        },
        &NoProgress,
        &CancelFlag::new(),
    )
    .unwrap_err();
    assert_eq!(
        e,
        rv_scan::db::ScanError::NoSuchDir("RomVault/Nope".to_owned())
    );

    t.write("RomRoot/Console/NES/Roms/Alpha.zip", &zip(&[("a.nes", A)]));
    // Rows are ticked by default after a DAT update: `Selected` scans RomVault and everything
    // below it, and ToSort (no folder on disk here, so it is reported missing).
    let r = scan_files(
        &mut db,
        &s,
        &ScanRequest::default(),
        &NoProgress,
        &CancelFlag::new(),
    )
    .unwrap();
    assert!(r.roots_scanned.contains(&ROMS.to_owned()), "{r:?}");
    assert_eq!(
        node(&db, &format!("{ROMS}/Alpha.zip")).rep_status,
        RepStatus::DIR_CORRECT
    );
    // Unticking a row takes it out.
    let mut db2 = db.clone();
    let mut cur = &mut db2.root;
    for p in ["RomVault", "Console", "NES", "Roms"] {
        let i = cur.children.iter().position(|c| c.name == p).unwrap();
        cur = &mut cur.children[i];
    }
    cur.tree.as_mut().unwrap().checked = rv_core::TreeSelect::UN_SELECTED;
    let r = scan_files(
        &mut db2,
        &s,
        &ScanRequest::default(),
        &NoProgress,
        &CancelFlag::new(),
    )
    .unwrap();
    assert!(!r.roots_scanned.contains(&ROMS.to_owned()), "{r:?}");
}

#[test]
fn a_quick_scan_is_followed_up_by_a_real_one_for_archives_that_were_never_fully_read() {
    let t = Tmp::new("quick");
    let s = t.settings();
    let mut db = load(&t, &s, &dat_text("", &[("Alpha", &[("a.nes", A)])]));
    t.write("RomRoot/Console/NES/Roms/Alpha.zip", &zip(&[("a.nes", A)]));
    // Level 1 trusts the archive's own CRCs: nothing is decoded, so no hash but the CRC exists.
    let r = scan(&mut db, &s, ScanLevel::Level1);
    assert_eq!(r.archives_scanned, 1);
    let alpha = node(&db, &format!("{ROMS}/Alpha.zip"));
    let a = &alpha.children[0];
    assert_eq!(a.rep_status, RepStatus::CORRECT);
    assert!(a.sha1.is_none() && a.md5.is_none());
    assert!(!alpha.is_deep_scanned(&s));
    // Level 1 again: nothing changed, nothing is read.
    assert_eq!(scan(&mut db, &s, ScanLevel::Level1).archives_scanned, 0);
    // Level 2 does not take the time stamp as proof for an archive that was never fully read.
    let r = scan(&mut db, &s, ScanLevel::Level2);
    assert_eq!(r.archives_scanned, 1);
    let a = &node(&db, &format!("{ROMS}/Alpha.zip")).children[0];
    assert_eq!(rv_scan::hash::to_hex(a.sha1.as_ref().unwrap()), hashes(A).sha1);
    assert!(node(&db, &format!("{ROMS}/Alpha.zip")).is_deep_scanned(&s));
    assert_eq!(scan(&mut db, &s, ScanLevel::Level2).archives_scanned, 0);
}
