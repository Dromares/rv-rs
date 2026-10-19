//! A CHD writer for tests: builds v1 to v5 files from decoded hunks and per-hunk encoders. It
//! writes the simplest map that is valid (a flat 4-bit Huffman code for the v5 map), which is
//! enough to exercise the reader's paths; real compressed maps are in `tests/fixtures.rs`.

#![allow(dead_code)]

use std::io::Write;

use md5::Md5;
use sha1::{Digest, Sha1};

pub const ZLIB: u32 = 0x7A6C_6962;
pub const ZSTD: u32 = 0x7A73_7464;
pub const LZMA: u32 = 0x6C7A_6D61;
pub const HUFF: u32 = 0x6875_6666;
pub const FLAC: u32 = 0x666C_6163;
pub const CD_ZLIB: u32 = 0x6364_7A6C;
pub const CD_ZSTD: u32 = 0x6364_7A73;
pub const CD_LZMA: u32 = 0x6364_6C7A;
pub const CD_FLAC: u32 = 0x6364_666C;
pub const AVHU: u32 = 0x6176_6875;

pub fn deflate(data: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::new(6));
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

pub fn zstd(data: &[u8]) -> Vec<u8> {
    ruzstd::encoding::compress_to_vec(data, ruzstd::encoding::CompressionLevel::Fastest)
}

/// Raw LZMA1 as CHD stores it: lc=3 lp=0 pb=2 and a dictionary of `dict` bytes, no header.
pub fn lzma(data: &[u8], dict: u32) -> Vec<u8> {
    let mut opts = liblzma::stream::LzmaOptions::new_preset(6).unwrap();
    opts.dict_size(dict)
        .literal_context_bits(3)
        .literal_position_bits(0)
        .position_bits(2);
    let mut filters = liblzma::stream::Filters::new();
    filters.lzma1(&opts);
    let stream = liblzma::stream::Stream::new_raw_encoder(&filters).unwrap();
    let mut e = liblzma::write::XzEncoder::new_stream(Vec::new(), stream);
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

/// MSB-first bit writer.
pub struct Bits {
    pub bytes: Vec<u8>,
    acc: u64,
    n: u32,
}

impl Bits {
    pub fn new() -> Self {
        Bits {
            bytes: Vec::new(),
            acc: 0,
            n: 0,
        }
    }
    pub fn put(&mut self, v: u64, bits: u32) {
        for i in (0..bits).rev() {
            self.acc = (self.acc << 1) | ((v >> i) & 1);
            self.n += 1;
            if self.n == 8 {
                self.bytes.push(self.acc as u8);
                self.acc = 0;
                self.n = 0;
            }
        }
    }
    pub fn finish(mut self) -> Vec<u8> {
        while self.n != 0 {
            self.put(0, 1);
        }
        self.bytes
    }
}

pub fn crc16(data: &[u8]) -> u16 {
    let mut crc = 0xffffu16;
    for &b in data {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// How a hunk is stored.
pub enum Out {
    /// Codec slot and the encoded bytes.
    Slot(u8, Vec<u8>),
    /// Uncompressed, `hunk_bytes` long.
    Raw,
    /// Same as the hunk with this index.
    SelfRef(usize),
    /// v3/v4: eight bytes repeated.
    Mini(u64),
}

pub struct Hunk {
    /// The hunk as decoded, `hunk_bytes` long (the last may be padding past `total_bytes`).
    pub decoded: Vec<u8>,
    pub out: Out,
}

pub struct Meta {
    pub tag: u32,
    pub flags: u8,
    pub data: Vec<u8>,
}

pub struct Spec {
    pub compressors: [u32; 4],
    pub hunk_bytes: u32,
    pub unit_bytes: u32,
    pub total_bytes: u64,
    pub hunks: Vec<Hunk>,
    pub meta: Vec<Meta>,
    pub parent_sha1: [u8; 20],
}

pub struct Built {
    pub bytes: Vec<u8>,
    pub raw_sha1: [u8; 20],
    pub combined_sha1: [u8; 20],
    pub md5: [u8; 16],
}

fn raw_data(spec: &Spec) -> Vec<u8> {
    let mut all: Vec<u8> = spec
        .hunks
        .iter()
        .flat_map(|h| h.decoded.iter().copied())
        .collect();
    all.truncate(spec.total_bytes as usize);
    all
}

fn hashes(spec: &Spec) -> ([u8; 20], [u8; 20], [u8; 16]) {
    let raw = raw_data(spec);
    let raw_sha1: [u8; 20] = Sha1::digest(&raw).into();
    let md5: [u8; 16] = Md5::digest(&raw).into();
    let mut metas: Vec<[u8; 24]> = spec
        .meta
        .iter()
        .filter(|m| m.flags & 1 != 0)
        .map(|m| {
            let mut h = [0u8; 24];
            h[..4].copy_from_slice(&m.tag.to_be_bytes());
            h[4..].copy_from_slice(&Sha1::digest(&m.data));
            h
        })
        .collect();
    metas.sort();
    let mut total = Sha1::new();
    total.update(raw_sha1);
    for m in &metas {
        total.update(m);
    }
    (raw_sha1, total.finalize().into(), md5)
}

/// The metadata chain, written at `at`; returns the bytes and the offset of the first entry.
fn metadata(spec: &Spec, at: u64) -> (Vec<u8>, u64) {
    let mut out = Vec::new();
    let mut pos = at;
    for (i, m) in spec.meta.iter().enumerate() {
        let len = m.data.len() as u32;
        let next = if i + 1 < spec.meta.len() {
            pos + 16 + len as u64
        } else {
            0
        };
        out.extend(m.tag.to_be_bytes());
        out.extend((((m.flags as u32) << 24) | len).to_be_bytes());
        out.extend(next.to_be_bytes());
        out.extend(&m.data);
        pos += 16 + len as u64;
    }
    (out, if spec.meta.is_empty() { 0 } else { at })
}

fn header_prefix(len: u32, version: u32) -> Vec<u8> {
    let mut h = b"MComprHD".to_vec();
    h.extend(len.to_be_bytes());
    h.extend(version.to_be_bytes());
    h
}

/// A version 5 CHD. With `compressors[0] == 0` the map is the plain `u32` list and hunks sit at
/// multiples of the hunk size; otherwise the map is Huffman packed.
pub fn build_v5(spec: &Spec) -> Built {
    let (raw_sha1, combined, md5) = hashes(spec);
    let hb = spec.hunk_bytes as u64;
    let uncompressed_map = spec.compressors[0] == 0;
    let mut file = vec![0u8; 124];
    let mut offsets = Vec::new(); // (type, length, offset, crc16)
    if uncompressed_map {
        // Hunks at multiples of the hunk size, starting at 1 * hunk size.
        file.resize(hb as usize, 0);
        for (index, h) in (1u64..).zip(spec.hunks.iter()) {
            offsets.push((4u32, hb as u32, index * hb, 0u16));
            file.extend(&h.decoded);
        }
    } else {
        for h in &spec.hunks {
            let crc = crc16(&h.decoded);
            match &h.out {
                Out::Slot(s, data) => {
                    offsets.push((*s as u32, data.len() as u32, file.len() as u64, crc));
                    file.extend(data);
                }
                Out::Raw => {
                    offsets.push((4, hb as u32, file.len() as u64, crc));
                    file.extend(&h.decoded);
                }
                Out::SelfRef(t) => {
                    offsets.push((5, 0, *t as u64, 0));
                }
                Out::Mini(_) => panic!("v5 has no mini hunks"),
            }
        }
    }
    let map_offset = file.len() as u64;
    if uncompressed_map {
        for (_, _, off, _) in &offsets {
            file.extend(((off / hb) as u32).to_be_bytes());
        }
    } else {
        let first = offsets.iter().find(|o| o.0 <= 4).map_or(124, |o| o.2);
        let max_len = offsets.iter().map(|o| o.1).max().unwrap_or(0);
        let length_bits = (32 - max_len.leading_zeros()).max(1);
        let self_bits = (32 - (spec.hunks.len() as u32).leading_zeros()).max(1);
        // A flat code: 16 symbols of 4 bits, so the symbol is its own code.
        let mut b = Bits::new();
        for _ in 0..16 {
            b.put(4, 4);
        }
        for o in &offsets {
            b.put(o.0 as u64, 4);
        }
        for o in &offsets {
            match o.0 {
                0..=3 => {
                    b.put(o.1 as u64, length_bits);
                    b.put(o.3 as u64, 16);
                }
                4 => b.put(o.3 as u64, 16),
                5 => b.put(o.2, self_bits),
                _ => unreachable!(),
            }
        }
        let packed = b.finish();
        let mut raw_map = Vec::new();
        for o in &offsets {
            raw_map.push(o.0 as u8);
            raw_map.extend(&o.1.to_be_bytes()[1..]);
            raw_map.extend(&o.2.to_be_bytes()[2..]);
            raw_map.extend(o.3.to_be_bytes());
        }
        file.extend((packed.len() as u32).to_be_bytes());
        file.extend(&first.to_be_bytes()[2..]);
        file.extend(crc16(&raw_map).to_be_bytes());
        file.extend([length_bits as u8, self_bits as u8, 0, 0]);
        file.extend(&packed);
    }
    let (meta, meta_offset) = metadata(spec, file.len() as u64);
    file.extend(meta);

    let mut h = header_prefix(124, 5);
    for c in spec.compressors {
        h.extend(c.to_be_bytes());
    }
    h.extend(spec.total_bytes.to_be_bytes());
    h.extend(map_offset.to_be_bytes());
    h.extend(meta_offset.to_be_bytes());
    h.extend(spec.hunk_bytes.to_be_bytes());
    h.extend(spec.unit_bytes.to_be_bytes());
    h.extend(raw_sha1);
    h.extend(combined);
    h.extend(spec.parent_sha1);
    assert_eq!(h.len(), 124);
    file[..124].copy_from_slice(&h);
    Built {
        bytes: file,
        raw_sha1,
        combined_sha1: combined,
        md5,
    }
}

/// A version 3 or 4 CHD: header, flat map, hunk data, metadata. Codec slot 0 is zlib.
pub fn build_v3_v4(spec: &Spec, version: u32) -> Built {
    assert!(version == 3 || version == 4);
    let (raw_sha1, combined, md5) = hashes(spec);
    let header_len = if version == 3 { 120 } else { 108 };
    let map_len = spec.hunks.len() * 16;
    let mut data: Vec<u8> = Vec::new();
    let mut map = Vec::new();
    for h in &spec.hunks {
        let (offset, len, flags) = match &h.out {
            Out::Slot(_, d) => {
                let o = (header_len + map_len + data.len()) as u64;
                data.extend(d);
                (o, d.len() as u32, 1u8)
            }
            Out::Raw => {
                let o = (header_len + map_len + data.len()) as u64;
                data.extend(&h.decoded);
                (o, h.decoded.len() as u32, 2)
            }
            Out::Mini(v) => (*v, 0, 3),
            Out::SelfRef(t) => (*t as u64, 0, 4),
        };
        map.extend(offset.to_be_bytes());
        map.extend(crc32fast::hash(&h.decoded).to_be_bytes());
        if version == 3 {
            // The 24-bit length is stored as bytes 1, 0, 2.
            map.extend([(len >> 8) as u8, len as u8, (len >> 16) as u8]);
        } else {
            map.extend((len as u16).to_be_bytes());
            map.push((len >> 16) as u8);
        }
        map.push(flags);
    }
    let meta_at = (header_len + map_len + data.len()) as u64;
    let (meta, meta_offset) = metadata(spec, meta_at);
    let mut h = header_prefix(header_len as u32, version);
    h.extend(0u32.to_be_bytes()); // flags
    h.extend(1u32.to_be_bytes()); // compression: zlib
    h.extend((spec.hunks.len() as u32).to_be_bytes());
    h.extend(spec.total_bytes.to_be_bytes());
    h.extend(meta_offset.to_be_bytes());
    if version == 3 {
        h.extend(md5);
        h.extend([0u8; 16]); // parent MD5
        h.extend(spec.hunk_bytes.to_be_bytes());
        h.extend(raw_sha1);
        h.extend(spec.parent_sha1);
    } else {
        h.extend(spec.hunk_bytes.to_be_bytes());
        h.extend(combined);
        h.extend(spec.parent_sha1);
        h.extend(raw_sha1);
    }
    assert_eq!(h.len(), header_len);
    let mut file = h;
    file.extend(map);
    file.extend(data);
    file.extend(meta);
    Built {
        bytes: file,
        raw_sha1,
        combined_sha1: combined,
        md5,
    }
}

/// A version 1 or 2 (hard disk) CHD: 512-byte sectors, zlib hunks, 64-bit map entries.
pub fn build_v1_v2(spec: &Spec, version: u32) -> Built {
    assert!(version == 1 || version == 2);
    let (raw_sha1, combined, md5) = hashes(spec);
    let header_len = if version == 1 { 76 } else { 80 };
    let map_len = spec.hunks.len() * 8;
    let sectors_per_hunk = spec.hunk_bytes / 512;
    let total_sectors = spec.total_bytes / 512;
    let mut data: Vec<u8> = Vec::new();
    let mut entries: Vec<u64> = Vec::new();
    for h in &spec.hunks {
        let entry = match &h.out {
            Out::Slot(_, d) => {
                let o = (header_len + map_len + data.len()) as u64;
                data.extend(d);
                ((d.len() as u64) << 44) | o
            }
            Out::Raw => {
                let o = (header_len + map_len + data.len()) as u64;
                data.extend(&h.decoded);
                ((h.decoded.len() as u64) << 44) | o
            }
            // An equal 64-bit entry is how v1 and v2 say "same as that hunk".
            Out::SelfRef(t) => entries[*t],
            Out::Mini(_) => panic!("not in v1/v2"),
        };
        entries.push(entry);
    }
    let map: Vec<u8> = entries.iter().flat_map(|e| e.to_be_bytes()).collect();
    let mut hd = header_prefix(header_len as u32, version);
    hd.extend(0u32.to_be_bytes()); // flags
    hd.extend(1u32.to_be_bytes()); // compression
    hd.extend(sectors_per_hunk.to_be_bytes()); // v1: hunk size in sectors; v2: unused
    hd.extend((spec.hunks.len() as u32).to_be_bytes());
    hd.extend((total_sectors as u32).to_be_bytes()); // cylinders
    hd.extend(1u32.to_be_bytes()); // heads
    hd.extend(1u32.to_be_bytes()); // sectors
    hd.extend(md5);
    hd.extend([0u8; 16]);
    if version == 2 {
        hd.extend(spec.hunk_bytes.to_be_bytes());
    }
    assert_eq!(hd.len(), header_len);
    let mut file = hd;
    file.extend(map);
    file.extend(data);
    Built {
        bytes: file,
        raw_sha1,
        combined_sha1: combined,
        md5,
    }
}

/// `n` deterministic, compressible bytes.
pub fn pattern(n: usize, seed: u32) -> Vec<u8> {
    (0..n)
        .map(|i| (((i as u32 / 7) ^ seed).wrapping_mul(31) % 251) as u8)
        .collect()
}
