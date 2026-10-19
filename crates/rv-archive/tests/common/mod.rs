//! Helpers shared by the integration tests: a byte-level zip builder that can make archives the
//! TorrentZip writer refuses to (wrong order, wrong comment, extra fields...).

#![allow(dead_code)]

use std::io::Write;

pub fn sample_text() -> Vec<u8> {
    (0..200)
        .map(|i| format!("line {i}: the quick brown fox jumps over the lazy dog\n"))
        .collect::<String>()
        .into_bytes()
}

pub fn deflate(data: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::new(9));
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

pub struct RawEntry {
    pub name: Vec<u8>,
    pub flags: u16,
    pub method: u16,
    pub dos_time: u16,
    pub dos_date: u16,
    pub crc32: u32,
    pub payload: Vec<u8>,
    pub uncompressed_size: u32,
    pub extra: Vec<u8>,
}

/// An entry that looks like a TorrentZip one (deflate, flag 2, the TorrentZip date), with real
/// data.
pub fn tz_entry(name: &str, data: &[u8]) -> RawEntry {
    let payload = if data.is_empty() {
        vec![3, 0]
    } else {
        deflate(data)
    };
    RawEntry {
        name: name.as_bytes().to_vec(),
        flags: 2,
        method: 8,
        dos_time: 0xBC00,
        dos_date: 0x2198,
        crc32: crc32fast::hash(data),
        payload,
        uncompressed_size: data.len() as u32,
        extra: Vec::new(),
    }
}

pub enum Comment {
    /// `TORRENTZIPPED-` plus the real CRC32 of the central directory.
    TorrentZip,
    /// The same with a lower-case hex CRC.
    TorrentZipLower,
    /// `TORRENTZIPPED-` plus a CRC that is wrong.
    TorrentZipWrongCrc,
    Text(&'static str),
    None,
}

pub fn build(entries: &[RawEntry], comment: Comment) -> Vec<u8> {
    let mut out = Vec::new();
    let mut offsets = Vec::new();
    for e in entries {
        offsets.push(out.len() as u32);
        out.extend(0x0403_4b50u32.to_le_bytes());
        out.extend(20u16.to_le_bytes());
        out.extend(e.flags.to_le_bytes());
        out.extend(e.method.to_le_bytes());
        out.extend(e.dos_time.to_le_bytes());
        out.extend(e.dos_date.to_le_bytes());
        out.extend(e.crc32.to_le_bytes());
        out.extend((e.payload.len() as u32).to_le_bytes());
        out.extend(e.uncompressed_size.to_le_bytes());
        out.extend((e.name.len() as u16).to_le_bytes());
        out.extend((e.extra.len() as u16).to_le_bytes());
        out.extend(&e.name);
        out.extend(&e.extra);
        out.extend(&e.payload);
    }
    let cd_start = out.len() as u32;
    let mut cd = Vec::new();
    for (e, off) in entries.iter().zip(&offsets) {
        cd.extend(0x0201_4b50u32.to_le_bytes());
        cd.extend(0u16.to_le_bytes());
        cd.extend(20u16.to_le_bytes());
        cd.extend(e.flags.to_le_bytes());
        cd.extend(e.method.to_le_bytes());
        cd.extend(e.dos_time.to_le_bytes());
        cd.extend(e.dos_date.to_le_bytes());
        cd.extend(e.crc32.to_le_bytes());
        cd.extend((e.payload.len() as u32).to_le_bytes());
        cd.extend(e.uncompressed_size.to_le_bytes());
        cd.extend((e.name.len() as u16).to_le_bytes());
        cd.extend((e.extra.len() as u16).to_le_bytes());
        cd.extend([0u8; 10]); // comment length, disk, internal and external attributes
        cd.extend(off.to_le_bytes());
        cd.extend(&e.name);
        cd.extend(&e.extra);
    }
    let crc = crc32fast::hash(&cd);
    let comment = match comment {
        Comment::TorrentZip => format!("TORRENTZIPPED-{crc:08X}"),
        Comment::TorrentZipLower => format!("TORRENTZIPPED-{crc:08x}"),
        Comment::TorrentZipWrongCrc => format!("TORRENTZIPPED-{:08X}", crc ^ 1),
        Comment::Text(t) => t.to_string(),
        Comment::None => String::new(),
    };
    out.extend(&cd);
    out.extend(0x0605_4b50u32.to_le_bytes());
    out.extend([0, 0, 0, 0]);
    out.extend((entries.len() as u16).to_le_bytes());
    out.extend((entries.len() as u16).to_le_bytes());
    out.extend((cd.len() as u32).to_le_bytes());
    out.extend(cd_start.to_le_bytes());
    out.extend((comment.len() as u16).to_le_bytes());
    out.extend(comment.as_bytes());
    out
}

pub mod chd;
pub mod flac;

pub mod sevenz;
