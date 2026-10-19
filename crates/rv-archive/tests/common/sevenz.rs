//! A small 7z writer for tests: one folder with one coder, any number of files and empty files,
//! a plain or LZMA-packed header, and the RomVault footer. Enough to make archives p7zip cannot
//! (zstd, RomVault structures) and broken ones.

#![allow(dead_code)]

use std::io::Write;

pub struct Coder {
    pub method: Vec<u8>,
    pub props: Vec<u8>,
}

pub fn number(out: &mut Vec<u8>, mut v: u64) {
    // 7z variable-length integer: the leading ones of the first byte count the extra bytes.
    let mut first = 0u8;
    let mut mask = 0x80u8;
    let mut i = 0;
    while i < 8 {
        if v < (1u64 << (7 * (i + 1))) {
            first |= (v >> (8 * i)) as u8;
            break;
        }
        first |= mask;
        mask >>= 1;
        i += 1;
    }
    out.push(first);
    for _ in 0..i {
        out.push(v as u8);
        v >>= 8;
    }
}

fn utf16z(name: &str) -> Vec<u8> {
    let mut v: Vec<u8> = name.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    v.extend([0, 0]);
    v
}

pub struct File {
    pub name: String,
    pub data: Vec<u8>,
    pub is_dir: bool,
}

pub fn file(name: &str, data: &[u8]) -> File {
    File {
        name: name.to_string(),
        data: data.to_vec(),
        is_dir: false,
    }
}

pub fn dir(name: &str) -> File {
    File {
        name: name.to_string(),
        data: Vec::new(),
        is_dir: true,
    }
}

/// Raw LZMA1 with the properties 7z stores (`lc=3 lp=0 pb=2`), and those 5 property bytes.
pub fn lzma1(data: &[u8], dict: u32) -> (Vec<u8>, Vec<u8>) {
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
    let packed = e.finish().unwrap();
    let mut props = vec![(2 * 5) * 9 + 3];
    props.extend(dict.to_le_bytes());
    (packed, props)
}

fn coder_bytes(c: &Coder, out: &mut Vec<u8>) {
    let mut flags = c.method.len() as u8;
    if !c.props.is_empty() {
        flags |= 0x20;
    }
    out.push(flags);
    out.extend(&c.method);
    if !c.props.is_empty() {
        number(out, c.props.len() as u64);
        out.extend(&c.props);
    }
}

/// The header listing `files` stored in one solid folder (`packed_len` bytes at pack position
/// `pack_pos`).
pub fn plain_header(files: &[File], coder: &Coder, pack_pos: u64, packed_len: u64) -> Vec<u8> {
    let with_data: Vec<&File> = files
        .iter()
        .filter(|f| !f.is_dir && !f.data.is_empty())
        .collect();
    let total: u64 = with_data.iter().map(|f| f.data.len() as u64).sum();
    let mut h = vec![0x01]; // kHeader
    if !with_data.is_empty() {
        h.push(0x04); // kMainStreamsInfo
        h.push(0x06); // kPackInfo
        number(&mut h, pack_pos);
        number(&mut h, 1);
        h.push(0x09);
        number(&mut h, packed_len);
        h.push(0x00);
        h.push(0x07); // kUnPackInfo
        h.push(0x0b);
        number(&mut h, 1);
        h.push(0); // not external
        number(&mut h, 1); // one coder
        coder_bytes(coder, &mut h);
        h.push(0x0c);
        number(&mut h, total);
        h.push(0x00);
        h.push(0x08); // kSubStreamsInfo
        h.push(0x0d);
        number(&mut h, with_data.len() as u64);
        h.push(0x09);
        for f in &with_data[..with_data.len() - 1] {
            number(&mut h, f.data.len() as u64);
        }
        h.push(0x0a); // kCRC, all defined
        h.push(1);
        for f in &with_data {
            h.extend(crc32fast::hash(&f.data).to_le_bytes());
        }
        h.push(0x00);
        h.push(0x00);
    }
    h.push(0x05); // kFilesInfo
    number(&mut h, files.len() as u64);
    let empty: Vec<bool> = files
        .iter()
        .map(|f| f.is_dir || f.data.is_empty())
        .collect();
    if empty.iter().any(|&e| e) {
        h.push(0x0e);
        let bits = bool_vector(&empty);
        number(&mut h, bits.len() as u64);
        h.extend(bits);
        let empty_files: Vec<bool> = files
            .iter()
            .filter(|f| f.is_dir || f.data.is_empty())
            .map(|f| !f.is_dir)
            .collect();
        if empty_files.iter().any(|&e| e) {
            h.push(0x0f);
            let bits = bool_vector(&empty_files);
            number(&mut h, bits.len() as u64);
            h.extend(bits);
        }
    }
    h.push(0x11); // kName
    let mut names = vec![0u8];
    for f in files {
        names.extend(utf16z(f.name.trim_end_matches('/')));
    }
    number(&mut h, names.len() as u64);
    h.extend(names);
    h.push(0x00);
    h.push(0x00);
    h
}

fn bool_vector(v: &[bool]) -> Vec<u8> {
    let mut out = vec![0u8; v.len().div_ceil(8)];
    for (i, &b) in v.iter().enumerate() {
        if b {
            out[i / 8] |= 0x80 >> (i % 8);
        }
    }
    out
}

/// `[signature header][packed][...tail]` where `tail` is the (possibly packed) header and the
/// next-header location points at `next_header`.
pub fn assemble(packed: &[u8], middle: &[u8], next_header: &[u8]) -> Vec<u8> {
    let next_offset = (packed.len() + middle.len()) as u64;
    let mut start = Vec::new();
    start.extend(next_offset.to_le_bytes());
    start.extend((next_header.len() as u64).to_le_bytes());
    start.extend(crc32fast::hash(next_header).to_le_bytes());
    let mut out = vec![0x37, 0x7a, 0xbc, 0xaf, 0x27, 0x1c, 0, 4];
    out.extend(crc32fast::hash(&start).to_le_bytes());
    out.extend(start);
    out.extend(packed);
    out.extend(middle);
    out.extend(next_header);
    out
}

/// A 7z with a plain header: `files` in one folder packed by `coder`.
pub fn build(files: &[File], coder: &Coder, packed: &[u8]) -> Vec<u8> {
    let header = plain_header(files, coder, 0, packed.len() as u64);
    assemble(packed, &[], &header)
}

/// A RomVault-style 7z: the header packed with LZMA, the 32-byte `RomVault7Z0N` footer in front
/// of the descriptor (`SevenZipWriteClose.cs:126-236`). `digit` is `b'1'` to `b'4'`.
pub fn build_romvault(files: &[File], coder: &Coder, packed: &[u8], digit: u8) -> Vec<u8> {
    let header = plain_header(files, coder, 0, packed.len() as u64);
    let (packed_header, props) = lzma1(&header, 1 << 16);
    // The descriptor: an encoded header whose streams info points at the packed header.
    let header_pos = packed.len() as u64; // relative to the end of the signature header
    let mut d = vec![0x17, 0x06];
    number(&mut d, header_pos);
    number(&mut d, 1);
    d.push(0x09);
    number(&mut d, packed_header.len() as u64);
    d.push(0x00);
    d.push(0x07);
    d.push(0x0b);
    number(&mut d, 1);
    d.push(0);
    number(&mut d, 1);
    coder_bytes(
        &Coder {
            method: vec![3, 1, 1],
            props,
        },
        &mut d,
    );
    d.push(0x0c);
    number(&mut d, header.len() as u64);
    d.push(0x0a);
    d.push(1);
    d.extend(crc32fast::hash(&header).to_le_bytes());
    d.push(0x00);
    d.push(0x00);
    // Footer: 11 bytes of text, the digit, then the descriptor's CRC, location and size.
    let descriptor_location = (32 + packed.len() + packed_header.len() + 32) as u64;
    let mut footer = b"RomVault7Z0".to_vec();
    footer.push(digit);
    footer.extend(crc32fast::hash(&d).to_le_bytes());
    footer.extend(descriptor_location.to_le_bytes());
    footer.extend((d.len() as u64).to_le_bytes());
    assert_eq!(footer.len(), 32);
    let mut middle = packed_header;
    middle.extend(footer);
    assemble(packed, &middle, &d)
}

/// A zstd frame in the shape RVZSTD writes: raw blocks, then an empty last block (`01 00 00`).
pub fn rvzstd(data: &[u8]) -> Vec<u8> {
    assert!(data.len() < 256);
    let mut f = vec![0x28, 0xb5, 0x2f, 0xfd, 0x20, data.len() as u8];
    if !data.is_empty() {
        let hdr = (data.len() as u32) << 3; // not last, raw
        f.extend(&hdr.to_le_bytes()[..3]);
        f.extend(data);
    }
    f.extend([0x01, 0x00, 0x00]);
    f
}

pub const ZSTD_METHOD: [u8; 4] = [0x04, 0xf7, 0x11, 0x01];
