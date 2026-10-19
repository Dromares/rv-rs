//! What the two fixes change in a `.Cache`: nothing about the format, only values that a new
//! scan would store. A scanned SNES file now gets the `SNES` header type and the alt fields
//! that 3.8.0 only ever stored for NES, FDS, ... files; this test proves `rv-cache` carries
//! such a node byte for byte.

use rv_cache::CacheFile;
use rv_core::{FileStatus, FileType, GotStatus, HeaderFileType, RvFile};
use rv_scan::{scan, ScanOptions};

fn snes_rom() -> Vec<u8> {
    let mut data = vec![0u8; 512];
    data[22..30].copy_from_slice(&[0xAA, 0xBB, 0x04, 0, 0, 0, 0, 0]);
    data.extend((0..4096u32).map(|i| (i % 253) as u8));
    data
}

/// The node `FileScanning.NewFileFound` would add for this scan (`RvFileCode.cs:112-139`).
fn node_for(name: &str, data: &[u8]) -> RvFile {
    let s = scan(&mut &data[..], ScanOptions::default()).unwrap();
    let alt = s.alt.as_ref().expect("alt hashes");
    let mut f = RvFile::new(FileType::FILE, name);
    f.got_status = GotStatus::GOT;
    f.size = Some(s.size);
    f.crc = s.digests.crc.map(|d| d.to_vec());
    f.sha1 = s.digests.sha1.map(|d| d.to_vec());
    f.md5 = s.digests.md5.map(|d| d.to_vec());
    f.header_file_type = Some(s.header_type);
    f.alt_size = Some(alt.size);
    f.alt_crc = alt.digests.crc.map(|d| d.to_vec());
    f.alt_sha1 = alt.digests.sha1.map(|d| d.to_vec());
    f.alt_md5 = alt.digests.md5.map(|d| d.to_vec());
    f.file_status = FileStatus::HEADER_FILE_TYPE_FROM_HEADER
        .with(FileStatus::ALT_SIZE_FROM_HEADER)
        .with(FileStatus::ALT_SIZE_VERIFIED)
        .with(FileStatus::SIZE_VERIFIED)
        .with(FileStatus::CRC_VERIFIED)
        .with(FileStatus::SHA1_VERIFIED)
        .with(FileStatus::MD5_VERIFIED)
        .with(FileStatus::ALT_CRC_VERIFIED)
        .with(FileStatus::ALT_SHA1_VERIFIED)
        .with(FileStatus::ALT_MD5_VERIFIED);
    f
}

#[test]
fn snes_and_pce_nodes_round_trip_through_rv_cache() {
    let mut root = RvFile::default_root();
    let snes = node_for("game.smc", &snes_rom());
    assert_eq!(snes.header_file_type, Some(HeaderFileType::SNES));
    let mut pce_data = vec![0u8; 512];
    pce_data[..11].copy_from_slice(&[0x40, 0, 0, 0, 0, 0, 0, 0, 0xAA, 0xBB, 0x02]);
    pce_data.extend(vec![7u8; 2048]);
    let pce = node_for("game.pce", &pce_data);
    assert_eq!(pce.header_file_type, Some(HeaderFileType::PCE));
    root.children[0].children = vec![snes, pce];

    let cache = CacheFile::new(root);
    let bytes = cache.to_bytes().expect("serialize");
    let back = CacheFile::from_bytes(&bytes).expect("parse");
    assert_eq!(back, cache);
    assert_eq!(back.to_bytes().unwrap(), bytes, "byte-identical rewrite");
}
