//! Round trips of real ROMVault caches. These need the board's data, which is never in
//! the repository, so they are `#[ignore]`d. Run them with:
//!
//! ```sh
//! RV_FIXTURES_DIR=/path/to/copies cargo test -p rv-cache --test fixtures -- --ignored --nocapture
//! ```
//!
//! The directory must hold copies of `RomVault3_2.Cache`, `RomVault3_2.CacheBackup`,
//! `RomVault3_3.Cache` and `RomVault3_3.CacheBackup`. With the variable unset these tests
//! fail rather than pass, so an ignored run never looks green by accident.

use std::fs;
use std::path::PathBuf;

use rv_cache::{CacheError, CacheFile};

fn fixtures_dir() -> PathBuf {
    match std::env::var_os("RV_FIXTURES_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => panic!("RV_FIXTURES_DIR is not set; these tests need real cache files"),
    }
}

fn read_fixture(name: &str) -> Vec<u8> {
    let path = fixtures_dir().join(name);
    fs::read(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

fn round_trip(name: &str, want_version: i32) {
    let original = read_fixture(name);
    let cache = CacheFile::from_bytes(&original).unwrap_or_else(|e| panic!("{name}: {e}"));
    assert_eq!(cache.version, want_version, "{name}: version");
    eprintln!(
        "{name}: {} bytes, version {}, {} nodes, root has {} children",
        original.len(),
        cache.version,
        cache.root.node_count(),
        cache.root.children.len()
    );
    let written = cache.to_bytes().expect("serialize");
    assert_eq!(written.len(), original.len(), "{name}: length differs");
    if let Some(at) = written.iter().zip(&original).position(|(a, b)| a != b) {
        panic!("{name}: first differing byte at offset {at}");
    }
    assert_eq!(written, original, "{name}: bytes differ");
}

#[test]
#[ignore = "needs RV_FIXTURES_DIR"]
fn v3_cache_round_trips_byte_identical() {
    round_trip("RomVault3_3.Cache", 3);
}

#[test]
#[ignore = "needs RV_FIXTURES_DIR"]
fn v3_cache_backup_round_trips_byte_identical() {
    round_trip("RomVault3_3.CacheBackup", 3);
}

#[test]
#[ignore = "needs RV_FIXTURES_DIR"]
fn v2_cache_round_trips_byte_identical() {
    round_trip("RomVault3_2.Cache", 2);
}

#[test]
#[ignore = "needs RV_FIXTURES_DIR"]
fn v2_cache_backup_round_trips_byte_identical() {
    round_trip("RomVault3_2.CacheBackup", 2);
}

#[test]
#[ignore = "needs RV_FIXTURES_DIR"]
fn truncated_real_cache_is_an_error_not_a_panic() {
    let original = read_fixture("RomVault3_3.Cache");
    let truncated = &original[..1024 * 1024];
    let err = CacheFile::from_bytes(truncated).expect_err("1 MiB prefix must be rejected");
    assert!(
        matches!(
            err,
            CacheError::Truncated { .. } | CacheError::BadCount { .. }
        ),
        "{err}"
    );
}

#[test]
#[ignore = "needs RV_FIXTURES_DIR"]
fn fixups_on_real_caches_keep_a_valid_file() {
    for name in ["RomVault3_2.Cache", "RomVault3_3.Cache"] {
        let mut cache = CacheFile::from_bytes(&read_fixture(name)).expect("parse");
        rv_cache::fixups::apply_load_fixups(&mut cache);
        let again = CacheFile::from_bytes(&cache.to_bytes().expect("serialize")).expect("reparse");
        assert_eq!(
            again, cache,
            "{name}: fix-ups then write then read changed the tree"
        );
        assert_eq!(cache.version, 3);
    }
}
