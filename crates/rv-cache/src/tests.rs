//! Tests on synthetic caches, which need no board data and run in hosted CI.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use rv_core::{
    dat_data, game_data, DatFlags, DatStatus, FileStatus, FileType, GotStatus, HeaderFileType,
    MetaEntry, MiaStatus, RvDat, RvFile, RvGame, RvTreeRow, ToSortDirType, TreeSelect,
    ZipStructure,
};

use crate::fixups::apply_load_fixups;
use crate::store::{save_with, tmp_path};
use crate::{backup_path, load, save, CacheError, CacheFile, LoadSource, END_MARKER, MAX_DEPTH};

/* ------------------------------------------------------------------ helpers */

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> TempDir {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "rv-cache-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn entry(id: u8, value: &str) -> MetaEntry {
    MetaEntry {
        id,
        value: value.to_owned(),
    }
}

fn hex(s: &str) -> Vec<u8> {
    let digits: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    digits
        .chunks(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

/// A small tree that uses every field type once. `version` 2 drops the `zip_dat_struct`
/// bytes that only version 3 stores.
fn kitchen_sink(version: i32) -> CacheFile {
    let zip_dat_struct = |raw: u8| (version >= 3).then_some(raw);

    let mut root = RvFile::new(FileType::DIR, "");
    root.dat_status = DatStatus::IN_DAT_COLLECT;
    root.tree = Some(RvTreeRow::default());

    let mut rom_vault = RvFile::new(FileType::DIR, "RomVault");
    rom_vault.dat_status = DatStatus::IN_DAT_COLLECT;
    rom_vault.tree = Some(RvTreeRow {
        expanded: false,
        checked: TreeSelect::LOCKED,
    });
    rom_vault.file_mod_time_stamp = 638_000_000_000_000_000;
    rom_vault.dir_dats = vec![
        RvDat::new(
            638_100_000_000_000_000,
            DatFlags::AUTO_ADDED_DIRECTORY.with(DatFlags::SINGLE_ARCHIVE),
            vec![
                entry(dat_data::DAT_NAME, "Nintendo - Game Boy.dat"),
                entry(dat_data::DAT_ROOT_FULL_NAME, "Nintendo\\Game Boy.dat"),
                entry(dat_data::SUB_DIR_TYPE, "2"),
            ],
        ),
        RvDat::new(i64::MAX, DatFlags::default(), Vec::new()),
    ];

    let mut games = RvFile::new(FileType::DIR, "Games \u{e9}\u{65e5}\u{672c}");
    games.file_name = "Games \u{c9}\u{65e5}\u{672c}".to_owned();
    games.dat_index = Some(0);
    games.dat_status = DatStatus::IN_DAT_COLLECT;
    games.tree = Some(RvTreeRow::default());

    let mut zip = RvFile::new(FileType::ZIP, "mario.zip");
    zip.dat_index = Some(1);
    zip.dat_status = DatStatus::IN_DAT_COLLECT;
    zip.got_status = GotStatus::GOT;
    zip.zip_dat_struct = zip_dat_struct(0x81);
    zip.zip_struct = Some(ZipStructure::ZIP_TRRNT);
    zip.game = Some(RvGame {
        entries: vec![
            entry(game_data::DESCRIPTION, "Super Mario"),
            entry(game_data::ID, "7"),
            entry(game_data::CLONE_OF_ID, "x"),
        ],
    });

    let mut rom = RvFile::new(FileType::FILE_ZIP, "mario.gb");
    rom.dat_status = DatStatus::IN_DAT_COLLECT;
    rom.got_status = GotStatus::GOT;
    rom.size = Some(32_768);
    rom.crc = Some(vec![0xde, 0xad, 0xbe, 0xef]);
    rom.sha1 = Some((0..20).collect());
    rom.md5 = Some((100..116).collect());
    rom.header_file_type = Some(HeaderFileType::NES.with(HeaderFileType::REQUIRED));
    rom.alt_size = Some(32_752);
    rom.alt_crc = Some(vec![1, 2, 3, 4]);
    rom.alt_sha1 = Some(vec![0xaa; 20]);
    rom.alt_md5 = Some(vec![0xbb; 16]);
    rom.merge = Some("parent.gb".to_owned());
    rom.status = Some("baddump".to_owned());
    rom.zip_file_index = Some(0);
    rom.zip_file_header_position = Some(u64::MAX);
    rom.chd_version = Some(5);
    rom.to_sort_type = Some(ToSortDirType::FILE_ONLY);
    rom.mia_status = Some(MiaStatus::NEW.with(MiaStatus::MIA_USER));
    rom.file_status = FileStatus::SIZE_FROM_HEADER
        .with(FileStatus::CRC_VERIFIED)
        .with(FileStatus::LEGACY_TO_SORT_CACHE);
    zip.children.push(rom);

    let mut seven = RvFile::new(FileType::SEVEN_ZIP, "zelda.7z");
    seven.dat_status = DatStatus::NOT_IN_DAT;
    seven.zip_dat_struct = zip_dat_struct(ZipStructure::SEVEN_ZIP_SLZMA.0);
    seven.zip_struct = Some(ZipStructure::SEVEN_ZIP_SLZMA);

    let mut loose = RvFile::new(FileType::FILE, &"n".repeat(300));
    loose.got_status = GotStatus::CORRUPT;
    // Flag set with an empty string, and an index of -1: both must survive.
    loose.merge = Some(String::new());
    loose.zip_file_index = Some(-1);
    loose.unknown_flag_bits = 0x8000_0000;

    let mut ghost = RvFile::new(FileType::UNSET, "");
    ghost.got_status = GotStatus::FILE_LOCKED;

    rom_vault.children = vec![games, zip, seven, loose, ghost];

    let mut to_sort = RvFile::new(FileType::DIR, "ToSort");
    to_sort.dat_status = DatStatus::IN_TO_SORT;
    to_sort.tree = Some(RvTreeRow::default());
    to_sort.to_sort_type = Some(ToSortDirType::PRIMARY.with(ToSortDirType::CACHE));

    root.children = vec![rom_vault, to_sort];
    CacheFile { version, root }
}

/* --------------------------------------------------------------- round trips */

#[test]
fn kitchen_sink_v3_round_trips() {
    let cache = kitchen_sink(3);
    let bytes = cache.to_bytes().unwrap();
    let parsed = CacheFile::from_bytes(&bytes).unwrap();
    assert_eq!(parsed, cache);
    assert_eq!(parsed.to_bytes().unwrap(), bytes);
}

#[test]
fn kitchen_sink_v2_round_trips() {
    let cache = kitchen_sink(2);
    let bytes = cache.to_bytes().unwrap();
    let parsed = CacheFile::from_bytes(&bytes).unwrap();
    assert_eq!(parsed, cache);
    assert_eq!(parsed.to_bytes().unwrap(), bytes);
    // Version 2 has one byte less per archive node (two archives here).
    assert_eq!(kitchen_sink(3).to_bytes().unwrap().len(), bytes.len() + 2);
}

#[test]
fn default_db_matches_the_hand_derived_bytes_of_cache_format_7_1() {
    // docs/cache-format.md section 7.1, derived from DB.cs:31-55 and rvFile.cs:405-509.
    let expected = hex("03000000
         00400400 00 00 0000000000000080 00 00 0101 02000000
         01 00400000 08526f6d5661756c74 00 0000000000000080 0000 0101 00000000
         01 00400800 06546f536f7274 00 0000000000000080 0400 0101 03 00000000
         00000000
         a7dd00a615000000");
    assert_eq!(expected.len(), 99);
    assert_eq!(CacheFile::default_db().to_bytes().unwrap(), expected);
    assert_eq!(
        CacheFile::from_bytes(&expected).unwrap(),
        CacheFile::default_db()
    );
}

#[test]
fn leaf_node_matches_the_hand_derived_bytes_of_cache_format_7_2() {
    let mut leaf = RvFile::new(FileType::FILE_ZIP, "a.rom");
    leaf.dat_status = DatStatus::NOT_IN_DAT;
    leaf.got_status = GotStatus::GOT;
    leaf.size = Some(4);
    leaf.crc = Some(vec![0xde, 0xad, 0xbe, 0xef]);
    leaf.zip_file_index = Some(0);
    leaf.file_status = FileStatus::SIZE_FROM_HEADER.with(FileStatus::CRC_FROM_HEADER);
    let mut cache = CacheFile::default_db();
    cache.root.children[0].children.push(leaf);
    let bytes = cache.to_bytes().unwrap();
    let snippet = hex("05 03080000 05612e726f6d 00 0000000000000080 0301
         0400000000000000 04deadbeef 00000000 000c0000");
    assert!(bytes
        .windows(snippet.len())
        .any(|w| w == snippet.as_slice()));
}

#[test]
fn long_names_use_multi_byte_length_prefixes() {
    for len in [127usize, 128, 300, 16_383, 16_384, 70_000] {
        let mut cache = CacheFile::default_db();
        cache.root.children[0].name = "x".repeat(len);
        let bytes = cache.to_bytes().unwrap();
        assert_eq!(CacheFile::from_bytes(&bytes).unwrap(), cache, "len {len}");
    }
}

/* ------------------------------------------------------------ corrupt input */

#[test]
fn every_truncation_of_a_valid_cache_is_an_error() {
    for version in [2, 3] {
        let bytes = kitchen_sink(version).to_bytes().unwrap();
        for cut in 0..bytes.len() {
            assert!(
                CacheFile::from_bytes(&bytes[..cut]).is_err(),
                "v{version} cut at {cut}"
            );
        }
    }
}

#[test]
fn every_single_bit_flip_either_errors_or_rewrites_to_the_mutated_bytes() {
    for version in [2, 3] {
        let bytes = kitchen_sink(version).to_bytes().unwrap();
        for at in 0..bytes.len() {
            for bit in 0..8 {
                let mut mutated = bytes.clone();
                mutated[at] ^= 1 << bit;
                if let Ok(cache) = CacheFile::from_bytes(&mutated) {
                    assert_eq!(
                        cache.to_bytes().unwrap(),
                        mutated,
                        "v{version} byte {at} bit {bit} parsed but does not write back"
                    );
                }
            }
        }
    }
}

#[test]
fn bad_version_marker_and_trailing_bytes_are_rejected() {
    let good = CacheFile::default_db().to_bytes().unwrap();

    for version in [-1i32, 0, 1, 4, 99] {
        let mut b = good.clone();
        b[..4].copy_from_slice(&version.to_le_bytes());
        assert!(matches!(
            CacheFile::from_bytes(&b),
            Err(CacheError::UnsupportedVersion(v)) if v == version
        ));
    }

    let mut b = good.clone();
    let n = b.len();
    b[n - 1] = 0x01;
    assert!(matches!(
        CacheFile::from_bytes(&b),
        Err(CacheError::BadEndMarker { .. })
    ));

    let mut b = good;
    b.push(0);
    assert!(matches!(
        CacheFile::from_bytes(&b),
        Err(CacheError::TrailingData { .. })
    ));
}

#[test]
fn huge_counts_fail_without_allocating() {
    let mut bytes = CacheFile::default_db().to_bytes().unwrap();
    // The root's child count is the i32 at offset 22 (cache format 7.1).
    bytes[22..26].copy_from_slice(&i32::MAX.to_le_bytes());
    assert!(matches!(
        CacheFile::from_bytes(&bytes),
        Err(CacheError::BadCount { .. })
    ));
    bytes[22..26].copy_from_slice(&(-5i32).to_le_bytes());
    assert!(matches!(
        CacheFile::from_bytes(&bytes),
        Err(CacheError::BadCount { .. })
    ));
}

/// `depth` nested `Dir` nodes (the root plus `depth` descendants, each with one child
/// but the last), as raw bytes. Built without recursion so a huge depth is cheap.
fn nested_dirs(depth: usize) -> Vec<u8> {
    let mut out = 3i32.to_le_bytes().to_vec();
    for level in 0..=depth {
        if level > 0 {
            out.push(1); // the child is a Dir
        }
        let has_child = level < depth;
        let flags = if has_child { 0x4_0000u32 } else { 0 }; // HasChildren
        out.extend_from_slice(&flags.to_le_bytes());
        out.extend_from_slice(&[0, 0]); // empty name and file name
        out.extend_from_slice(&i64::MIN.to_le_bytes());
        out.extend_from_slice(&[0, 0]); // dat and got status
        if has_child {
            out.extend_from_slice(&1i32.to_le_bytes());
        }
    }
    // Every node ends with its file status, innermost first.
    for _ in 0..=depth {
        out.extend_from_slice(&0u32.to_le_bytes());
    }
    out.extend_from_slice(&END_MARKER.to_le_bytes());
    out
}

#[test]
fn deeply_nested_trees_are_rejected_not_a_stack_overflow() {
    let shallow = CacheFile::from_bytes(&nested_dirs(10)).unwrap();
    assert_eq!(shallow.root.node_count(), 11);
    assert_eq!(shallow.to_bytes().unwrap(), nested_dirs(10));
    assert!(CacheFile::from_bytes(&nested_dirs(MAX_DEPTH)).is_ok());
    assert!(matches!(
        CacheFile::from_bytes(&nested_dirs(MAX_DEPTH + 1)),
        Err(CacheError::TooDeep)
    ));
    assert!(matches!(
        CacheFile::from_bytes(&nested_dirs(100_000)),
        Err(CacheError::TooDeep)
    ));
    let mut cache = CacheFile::default_db();
    let mut node = RvFile::new(FileType::DIR, "leaf");
    for _ in 0..(MAX_DEPTH + 10) {
        let mut parent = RvFile::new(FileType::DIR, "d");
        parent.children.push(node);
        node = parent;
    }
    cache.root.children[0].children.push(node);
    assert!(matches!(cache.to_bytes(), Err(CacheError::TooDeep)));
}

#[test]
fn invalid_models_are_errors_on_write() {
    let mut c = CacheFile::default_db();
    c.root.children[0].zip_struct = Some(ZipStructure::ZIP_TRRNT);
    assert!(matches!(c.to_bytes(), Err(CacheError::InvalidModel(_))));

    let mut c = kitchen_sink(3);
    c.root.children[0].children[1].zip_dat_struct = None;
    assert!(matches!(c.to_bytes(), Err(CacheError::InvalidModel(_))));

    let mut c = kitchen_sink(2);
    c.root.children[0].children[1].zip_dat_struct = Some(0);
    assert!(matches!(c.to_bytes(), Err(CacheError::InvalidModel(_))));

    let mut c = CacheFile::default_db();
    c.root.children[0].crc = Some(vec![0; 256]);
    assert!(matches!(c.to_bytes(), Err(CacheError::InvalidModel(_))));

    let mut c = CacheFile::default_db();
    c.root.children[0].game = Some(RvGame {
        entries: vec![entry(1, "x"); 256],
    });
    assert!(matches!(c.to_bytes(), Err(CacheError::InvalidModel(_))));

    let mut c = CacheFile::default_db();
    c.version = 4;
    assert!(matches!(
        c.to_bytes(),
        Err(CacheError::UnsupportedVersion(4))
    ));

    let mut c = CacheFile::default_db();
    c.root.children[0].unknown_flag_bits = 1;
    assert!(matches!(c.to_bytes(), Err(CacheError::InvalidModel(_))));
}

/* ------------------------------------------------------------------ fix-ups */

#[test]
fn fixups_migrate_a_v2_cache_like_rvfile_cs() {
    let mut cache = kitchen_sink(2);
    // In-dat 7z with the pre-2024 struct, a retired InDatMIA node, and legacy sort bits.
    let rv = &mut cache.root.children[0];
    rv.children[2].dat_status = DatStatus::IN_DAT_COLLECT;
    rv.children[2].zip_struct = Some(ZipStructure::ZIP_TRRNT);
    rv.children[3].dat_status = DatStatus::RETIRED_IN_DAT_MIA;
    cache.root.children[1].to_sort_type = None;
    cache.root.children[1].file_status = FileStatus::LEGACY_TO_SORT_PRIMARY;
    cache.root.children[0].file_status = FileStatus::LEGACY_TO_SORT_CACHE;

    apply_load_fixups(&mut cache);
    assert_eq!(cache.version, 3);

    let rv = &cache.root.children[0];
    // v2 zip in dat: (ZipTrrnt, fix); 7z in dat: (SevenZipSLZMA, fix).
    assert_eq!(rv.children[1].zip_dat_struct, Some(0x81));
    assert_eq!(rv.children[2].zip_dat_struct, Some(0x88));
    assert_eq!(
        rv.children[2].zip_struct,
        Some(ZipStructure::SEVEN_ZIP_SLZMA)
    );
    // Retired InDatMIA becomes InDatCollect + MIAFromDat.
    assert_eq!(rv.children[3].dat_status, DatStatus::IN_DAT_COLLECT);
    assert_eq!(rv.children[3].mia_status, Some(MiaStatus::MIA_FROM_DAT));
    // Everything under a root child after the first is ToSort; the first keeps its own.
    assert_eq!(cache.root.children[1].dat_status, DatStatus::IN_TO_SORT);
    assert_eq!(rv.children[0].dat_status, DatStatus::IN_DAT_COLLECT);
    // Header repair: HeaderFileType plus Alt hashes sets the "from header" bit.
    assert!(rv.children[1].children[0]
        .file_status
        .contains(FileStatus::HEADER_FILE_TYPE_FROM_HEADER));
    // Legacy bits 30/31 moved into the ToSort type and cleared.
    let to_sort = &cache.root.children[1];
    assert_eq!(to_sort.to_sort_type, Some(ToSortDirType::PRIMARY));
    assert_eq!(to_sort.file_status, FileStatus::default());
    assert_eq!(rv.to_sort_type, Some(ToSortDirType::CACHE));

    // What fix-ups produce is a valid v3 cache.
    let bytes = cache.to_bytes().unwrap();
    assert_eq!(CacheFile::from_bytes(&bytes).unwrap(), cache);
}

#[test]
fn fixups_push_to_sort_down_the_subtree() {
    let mut cache = CacheFile::default_db();
    let mut sub = RvFile::new(FileType::DIR, "sub");
    sub.dat_status = DatStatus::IN_DAT_COLLECT;
    let mut leaf = RvFile::new(FileType::FILE, "f");
    leaf.dat_status = DatStatus::NOT_IN_DAT;
    sub.children.push(leaf);
    cache.root.children[1].children.push(sub);
    apply_load_fixups(&mut cache);
    let sub = &cache.root.children[1].children[0];
    assert_eq!(sub.dat_status, DatStatus::IN_TO_SORT);
    assert_eq!(sub.children[0].dat_status, DatStatus::IN_TO_SORT);
}

#[test]
fn fixups_leave_a_stored_mia_status_in_place() {
    let mut cache = CacheFile::default_db();
    let mut f = RvFile::new(FileType::FILE, "f");
    f.dat_status = DatStatus::RETIRED_IN_DAT_MIA;
    f.mia_status = Some(MiaStatus::MIA_USER);
    cache.root.children[0].children.push(f);
    apply_load_fixups(&mut cache);
    assert_eq!(
        cache.root.children[0].children[0].mia_status,
        Some(MiaStatus::MIA_USER)
    );
}

/* -------------------------------------------------- load with backup fallback */

fn write_cache(path: &Path, cache: &CacheFile) -> Vec<u8> {
    let bytes = cache.to_bytes().unwrap();
    fs::write(path, &bytes).unwrap();
    bytes
}

#[test]
fn load_uses_the_primary_when_it_is_valid() {
    let dir = TempDir::new();
    let path = dir.path("RomVault3_3.Cache");
    write_cache(&path, &kitchen_sink(3));
    write_cache(&backup_path(&path), &CacheFile::default_db());
    let loaded = load(&path).unwrap();
    assert!(matches!(loaded.source, LoadSource::Primary));
    assert_eq!(loaded.cache, kitchen_sink(3));
}

#[test]
fn truncated_primary_falls_back_to_the_backup_and_says_so() {
    let dir = TempDir::new();
    let path = dir.path("RomVault3_3.Cache");
    let bytes = write_cache(&path, &kitchen_sink(3));
    fs::write(&path, &bytes[..bytes.len() / 2]).unwrap();
    write_cache(&backup_path(&path), &CacheFile::default_db());

    let loaded = load(&path).unwrap();
    match loaded.source {
        LoadSource::Backup { primary_error } => {
            assert!(
                matches!(primary_error, CacheError::Truncated { .. }),
                "{primary_error}"
            )
        }
        LoadSource::Primary => panic!("loaded the corrupt primary"),
    }
    assert_eq!(loaded.cache, CacheFile::default_db());
}

#[test]
fn corrupt_primary_with_a_bad_end_marker_falls_back() {
    let dir = TempDir::new();
    let path = dir.path("RomVault3_3.Cache");
    let mut bytes = write_cache(&path, &kitchen_sink(3));
    let n = bytes.len();
    bytes[n - 3] ^= 0xff;
    fs::write(&path, &bytes).unwrap();
    write_cache(&backup_path(&path), &kitchen_sink(2));
    let loaded = load(&path).unwrap();
    assert!(matches!(loaded.source, LoadSource::Backup { .. }));
    assert_eq!(loaded.cache.version, 2);
}

#[test]
fn missing_primary_loads_the_backup() {
    let dir = TempDir::new();
    let path = dir.path("RomVault3_3.Cache");
    write_cache(&backup_path(&path), &CacheFile::default_db());
    match load(&path).unwrap().source {
        LoadSource::Backup { primary_error } => assert!(primary_error.is_not_found()),
        LoadSource::Primary => panic!("no primary exists"),
    }
}

#[test]
fn both_corrupt_gives_an_error_value() {
    let dir = TempDir::new();
    let path = dir.path("RomVault3_3.Cache");
    fs::write(&path, b"\x03\x00").unwrap();
    fs::write(backup_path(&path), vec![0xffu8; 64]).unwrap();
    let err = load(&path).unwrap_err();
    assert!(!err.is_not_found());
    assert!(matches!(err.primary, CacheError::Truncated { .. }));
    assert!(matches!(err.backup, CacheError::UnsupportedVersion(_)));
    assert!(err.to_string().contains("backup"));
}

#[test]
fn neither_file_existing_is_reported_as_not_found() {
    let dir = TempDir::new();
    assert!(load(&dir.path("RomVault3_3.Cache"))
        .unwrap_err()
        .is_not_found());
}

/* ------------------------------------------------------------------- saving */

struct FailAfter {
    inner: fs::File,
    left: usize,
}

impl Write for FailAfter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.left == 0 {
            return Err(io::Error::other("injected write failure"));
        }
        let n = buf.len().min(self.left);
        let written = self.inner.write(&buf[..n])?;
        self.left -= written;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[test]
fn save_creates_a_new_cache_without_a_backup() {
    let dir = TempDir::new();
    let path = dir.path("RomVault3_3.Cache");
    save(&path, &kitchen_sink(3)).unwrap();
    assert_eq!(
        fs::read(&path).unwrap(),
        kitchen_sink(3).to_bytes().unwrap()
    );
    assert!(!backup_path(&path).exists());
    assert!(!tmp_path(&path).exists());
}

#[test]
fn save_keeps_the_previous_cache_as_backup() {
    let dir = TempDir::new();
    let path = dir.path("RomVault3_3.Cache");
    let first = write_cache(&path, &CacheFile::default_db());
    fs::write(backup_path(&path), b"older backup").unwrap();

    save(&path, &kitchen_sink(3)).unwrap();

    assert_eq!(
        fs::read(&path).unwrap(),
        kitchen_sink(3).to_bytes().unwrap()
    );
    assert_eq!(fs::read(backup_path(&path)).unwrap(), first);
    assert!(!tmp_path(&path).exists());
    assert_eq!(
        backup_path(&path).file_name().unwrap(),
        "RomVault3_3.CacheBackup"
    );
}

#[test]
fn a_stale_temp_file_does_not_block_a_save() {
    let dir = TempDir::new();
    let path = dir.path("RomVault3_3.Cache");
    fs::write(tmp_path(&path), b"left over from a crash").unwrap();
    save(&path, &kitchen_sink(3)).unwrap();
    assert_eq!(
        fs::read(&path).unwrap(),
        kitchen_sink(3).to_bytes().unwrap()
    );
    assert!(!tmp_path(&path).exists());
}

#[test]
fn a_failure_mid_write_leaves_the_original_and_its_backup_intact() {
    let dir = TempDir::new();
    let path = dir.path("RomVault3_3.Cache");
    let original = write_cache(&path, &CacheFile::default_db());
    fs::write(backup_path(&path), b"previous backup").unwrap();
    let new_len = kitchen_sink(3).to_bytes().unwrap().len();

    for fail_at in [0, 1, new_len / 2, new_len - 1] {
        let err = save_with(&path, &kitchen_sink(3), |file| FailAfter {
            inner: file,
            left: fail_at,
        })
        .unwrap_err();
        assert!(matches!(err, CacheError::Io(_)), "{err}");
        assert_eq!(
            fs::read(&path).unwrap(),
            original,
            "original changed (fail_at {fail_at})"
        );
        assert_eq!(fs::read(backup_path(&path)).unwrap(), b"previous backup");
        assert!(
            !tmp_path(&path).exists(),
            "temp file left behind (fail_at {fail_at})"
        );
    }
    // The cache is still loadable and still the original one.
    assert_eq!(load(&path).unwrap().cache, CacheFile::default_db());
}

#[test]
fn a_failed_first_save_creates_no_cache() {
    let dir = TempDir::new();
    let path = dir.path("RomVault3_3.Cache");
    save_with(&path, &kitchen_sink(3), |file| FailAfter {
        inner: file,
        left: 10,
    })
    .unwrap_err();
    assert!(!path.exists());
    assert!(!tmp_path(&path).exists());
}

#[test]
fn a_failure_while_installing_leaves_the_original_intact() {
    let dir = TempDir::new();
    let path = dir.path("RomVault3_3.Cache");
    let original = write_cache(&path, &CacheFile::default_db());
    // A non-empty directory where the backup should go makes the install step fail
    // after the temp file has been fully written and synced.
    let backup = backup_path(&path);
    fs::create_dir(&backup).unwrap();
    fs::write(backup.join("x"), b"x").unwrap();

    save(&path, &kitchen_sink(3)).unwrap_err();
    assert_eq!(fs::read(&path).unwrap(), original);
    assert!(!tmp_path(&path).exists());
}

#[test]
fn an_invalid_model_is_rejected_before_any_file_is_touched() {
    let dir = TempDir::new();
    let path = dir.path("RomVault3_3.Cache");
    let original = write_cache(&path, &CacheFile::default_db());
    let mut bad = CacheFile::default_db();
    bad.version = 9;
    assert!(matches!(
        save(&path, &bad),
        Err(CacheError::UnsupportedVersion(9))
    ));
    assert_eq!(fs::read(&path).unwrap(), original);
    assert!(!tmp_path(&path).exists());
    assert!(!backup_path(&path).exists());
}
