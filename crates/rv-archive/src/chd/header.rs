//! CHD header and hunk map, versions 1 to 5 (`CHD/CHDSharpLib/CHDHeaders.cs`, `CHDCommon.cs`).
//!
//! All integers are big-endian. Widths are 64-bit where the C# multiplies 32-bit values (the v1/v2
//! `totalbytes` and the v5 uncompressed map offsets overflow there for CHDs of 4 GiB and more,
//! `docs/archives-and-rvio.md` section 8.5); that is the one deliberate difference.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};

use super::huffman::{BitStream, HuffmanDecoder};
use crate::error::{ArchiveError, ChdError, Result};

pub(super) const TAG: &[u8; 8] = b"MComprHD";
/// More hunks than any real CHD has (a 4 KiB hunk size on 250 GB); bounds the map allocation.
const MAX_HUNKS: u32 = 1 << 26;
/// Header lengths by version (`CHD.cs:171`).
const HEADER_LENGTHS: [u32; 6] = [0, 76, 80, 120, 108, 124];

/// `chd_codec` (`CHDCommon.cs:41-55`): the four-character code as a number.
pub mod codec {
    pub const NONE: u32 = 0;
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
    /// Stands for a v3/v4 compression number ROMVault cannot map (`CHD_CODEC_ERROR`).
    pub const ERROR: u32 = 0x0eee_eeee;
}

/// A codec's four-character name for messages.
pub fn codec_name(c: u32) -> String {
    let b = c.to_be_bytes();
    if b.iter().all(|x| x.is_ascii_graphic()) {
        String::from_utf8_lossy(&b).into_owned()
    } else if c == codec::NONE {
        "none".to_string()
    } else {
        format!("{c:#010x}")
    }
}

/// `compression_type` (`CHDCommon.cs:62-113`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompType {
    /// Codec slot 0 to 3.
    Codec(u8),
    None,
    /// Same as another hunk of this file; `MapEntry::offset` is its index.
    SelfRef,
    Parent,
    /// v3/v4 only: eight bytes of `offset`, repeated.
    Mini,
    /// A map entry that is not valid.
    Error,
}

/// The pseudo-types of the compressed v5 map (`CHDCommon.cs:73-113`).
mod ct {
    pub const NONE: u32 = 4;
    pub const SELF: u32 = 5;
    pub const PARENT: u32 = 6;
    pub const RLE_SMALL: u32 = 7;
    pub const RLE_LARGE: u32 = 8;
    pub const SELF_0: u32 = 9;
    pub const SELF_1: u32 = 10;
    pub const PARENT_SELF: u32 = 11;
    pub const PARENT_0: u32 = 12;
    pub const PARENT_1: u32 = 13;
}

/// One hunk's map entry (`mapentry`, `CHD.cs:30-50`).
#[derive(Clone, Debug)]
pub struct MapEntry {
    pub comp: CompType,
    /// Length of the stored bytes.
    pub length: u32,
    /// File offset of the stored bytes, or the index of the source hunk for `SelfRef`.
    pub offset: u64,
    /// CRC32 of the decoded hunk (v3).
    pub crc32: Option<u32>,
    /// CRC-16/CCITT of the decoded hunk (v5).
    pub crc16: Option<u16>,
}

/// A parsed header and its hunk map.
#[derive(Clone, Debug)]
pub struct ChdHeader {
    pub version: u32,
    /// The codec of each slot (v1/v2 are always zlib; v3/v4 have one slot).
    pub compressors: Vec<u32>,
    pub total_bytes: u64,
    pub hunk_bytes: u32,
    pub total_hunks: u32,
    pub unit_bytes: u32,
    pub meta_offset: u64,
    /// MD5 of the raw data (v1 to v3).
    pub md5: Option<[u8; 16]>,
    /// SHA-1 of the raw data (v3 and later).
    pub raw_sha1: Option<[u8; 20]>,
    /// SHA-1 of the raw data and the checksummed metadata (v4 and later).
    pub sha1: Option<[u8; 20]>,
    pub parent_md5: Option<[u8; 16]>,
    pub parent_sha1: Option<[u8; 20]>,
    pub map: Vec<MapEntry>,
}

impl ChdHeader {
    /// True when the header names a parent, so hunks may live in another file
    /// (`CHD.cs:125-129`: a non-zero parent MD5 or SHA-1).
    pub fn requires_parent(&self) -> bool {
        self.parent_md5.is_some_and(|m| m.iter().any(|&b| b != 0))
            || self.parent_sha1.is_some_and(|m| m.iter().any(|&b| b != 0))
    }

    /// `chdSHA1 = chd.sha1 ?? chd.rawsha1` (`CHD.cs:121`): the SHA-1 DATs list for a CHD.
    pub fn dat_sha1(&self) -> Option<[u8; 20]> {
        self.sha1.or(self.raw_sha1)
    }
}

fn invalid(detail: impl Into<String>) -> ArchiveError {
    ChdError::InvalidData(detail.into()).into()
}

struct Rd<'a, R> {
    r: &'a mut R,
}

impl<R: Read> Rd<'_, R> {
    fn bytes<const N: usize>(&mut self) -> Result<[u8; N]> {
        let mut b = [0u8; N];
        self.r.read_exact(&mut b).map_err(|e| {
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                ArchiveError::Truncated {
                    what: "CHD header or map",
                }
            } else {
                ArchiveError::Io(e)
            }
        })?;
        Ok(b)
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.bytes::<1>()?[0])
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(self.bytes()?))
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.bytes()?))
    }
    fn u48(&mut self) -> Result<u64> {
        let b = self.bytes::<6>()?;
        Ok(b.iter().fold(0u64, |a, &x| (a << 8) | x as u64))
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_be_bytes(self.bytes()?))
    }
}

/// Read the 16-byte prefix `MComprHD`, length, version (`CHD.CheckHeader`, `CHD.cs:173-195`) and
/// then the version's header and map. `file_len` bounds the map size.
pub fn read_header<R: Read + Seek>(r: &mut R) -> Result<ChdHeader> {
    r.seek(SeekFrom::Start(0))?;
    let mut rd = Rd { r };
    let tag = rd
        .bytes::<8>()
        .map_err(|_| ChdError::InvalidFile("the file is shorter than a CHD header"))?;
    if &tag != TAG {
        return Err(ChdError::InvalidFile("the MComprHD tag is missing").into());
    }
    let length = rd.u32()?;
    let version = rd.u32()?;
    if version == 0 || version > 5 {
        // ROMVault indexes its length table with the version and throws for versions above 5.
        return Err(ChdError::UnsupportedVersion(version).into());
    }
    if HEADER_LENGTHS[version as usize] != length {
        return Err(ChdError::InvalidFile("the header length does not match its version").into());
    }
    match version {
        1 | 2 => read_v1_v2(&mut rd, version),
        3 => read_v3(&mut rd),
        4 => read_v4(&mut rd),
        _ => read_v5(&mut rd),
    }
}

/// `ReadHeaderV1` and `ReadHeaderV2` (`CHDHeaders.cs:10-112`).
fn read_v1_v2<R: Read>(rd: &mut Rd<'_, R>, version: u32) -> Result<ChdHeader> {
    const SECTOR: u64 = 512;
    let _flags = rd.u32()?;
    let _compression = rd.u32()?;
    let mut blocksize = rd.u32()? as u64; // v1: hunk size in sectors; v2: unused
    let total_hunks = rd.u32()?;
    let (cylinders, heads, sectors) = (rd.u32()? as u64, rd.u32()? as u64, rd.u32()? as u64);
    let md5 = rd.bytes::<16>()?;
    let parent_md5 = rd.bytes::<16>()?;
    if version == 1 {
        blocksize *= SECTOR;
    } else {
        blocksize = rd.u32()? as u64; // v2 adds the hunk size in bytes
    }
    let hunk_bytes = u32::try_from(blocksize).map_err(|_| invalid("the hunk size is too large"))?;
    let total_bytes = cylinders * heads * sectors * SECTOR;
    let mut map = Vec::with_capacity((total_hunks as usize).min(1 << 20));
    // Equal 64-bit entries are the same hunk: the later ones refer to the first.
    let mut seen: HashMap<u64, usize> = HashMap::new();
    for i in 0..total_hunks as usize {
        let raw = rd.u64()?;
        if let Some(&first) = seen.get(&raw) {
            map.push(MapEntry {
                comp: CompType::SelfRef,
                length: 0,
                offset: first as u64,
                crc32: None,
                crc16: None,
            });
            continue;
        }
        seen.insert(raw, i);
        let length = (raw >> 44) as u32;
        map.push(MapEntry {
            comp: if length as u64 == blocksize {
                CompType::None
            } else {
                CompType::Codec(0)
            },
            length,
            offset: raw & 0xfff_ffff_ffff,
            crc32: None,
            crc16: None,
        });
    }
    Ok(ChdHeader {
        version,
        compressors: vec![codec::ZLIB],
        total_bytes,
        hunk_bytes,
        total_hunks,
        unit_bytes: 0,
        meta_offset: 0,
        md5: Some(md5),
        raw_sha1: None,
        sha1: None,
        parent_md5: Some(parent_md5),
        parent_sha1: None,
        map,
    })
}

/// `compTypeConv` (`CHDCommon.cs:8-18`).
fn comp_type_conv(ct: u32) -> u32 {
    match ct {
        1 | 2 => codec::ZLIB,
        3 => codec::AVHU,
        _ => codec::ERROR,
    }
}

/// `ConvMapFlagstoCompressionType` (`CHDCommon.cs:20-34`).
fn conv_map_flags(flags: u8) -> CompType {
    match flags & 0x0f {
        1 => CompType::Codec(0),
        2 => CompType::None,
        3 => CompType::Mini,
        4 => CompType::SelfRef,
        5 => CompType::Parent,
        _ => CompType::Error,
    }
}

/// `ReadHeaderV3` (`CHDHeaders.cs:114-147`).
fn read_v3<R: Read>(rd: &mut Rd<'_, R>) -> Result<ChdHeader> {
    let _flags = rd.u32()?;
    let compression = comp_type_conv(rd.u32()?);
    let total_hunks = rd.u32()?;
    let total_bytes = rd.u64()?;
    let meta_offset = rd.u64()?;
    let md5 = rd.bytes::<16>()?;
    let parent_md5 = rd.bytes::<16>()?;
    let hunk_bytes = rd.u32()?;
    let raw_sha1 = rd.bytes::<20>()?;
    let parent_sha1 = rd.bytes::<20>()?;
    let mut map = Vec::with_capacity((total_hunks as usize).min(1 << 20));
    for _ in 0..total_hunks {
        let offset = rd.u64()?;
        let crc = rd.u32()?;
        // The 24-bit length is stored as bytes 1, 0, 2 of an unusual order (`:135-139`).
        let (b0, b1, b2) = (rd.u8()? as u32, rd.u8()? as u32, rd.u8()? as u32);
        let length = (b0 << 8) | b1 | (b2 << 16);
        let flags = rd.u8()?;
        map.push(MapEntry {
            comp: conv_map_flags(flags),
            length,
            offset,
            crc32: (flags & 0x10 == 0).then_some(crc),
            crc16: None,
        });
    }
    Ok(ChdHeader {
        version: 3,
        compressors: vec![compression],
        total_bytes,
        hunk_bytes,
        total_hunks,
        unit_bytes: 0,
        meta_offset,
        md5: Some(md5),
        raw_sha1: Some(raw_sha1),
        sha1: None,
        parent_md5: Some(parent_md5),
        parent_sha1: Some(parent_sha1),
        map,
    })
}

/// `ReadHeaderV4` (`CHDHeaders.cs:149-180`). The map CRCs are not kept (`:177`).
fn read_v4<R: Read>(rd: &mut Rd<'_, R>) -> Result<ChdHeader> {
    let _flags = rd.u32()?;
    let compression = comp_type_conv(rd.u32()?);
    let total_hunks = rd.u32()?;
    let total_bytes = rd.u64()?;
    let meta_offset = rd.u64()?;
    let hunk_bytes = rd.u32()?;
    let sha1 = rd.bytes::<20>()?;
    let parent_sha1 = rd.bytes::<20>()?;
    let raw_sha1 = rd.bytes::<20>()?;
    let mut map = Vec::with_capacity((total_hunks as usize).min(1 << 20));
    for _ in 0..total_hunks {
        let offset = rd.u64()?;
        let _crc = rd.u32()?;
        let length = rd.u16()? as u32 | ((rd.u8()? as u32) << 16);
        let flags = rd.u8()?;
        map.push(MapEntry {
            comp: conv_map_flags(flags),
            length,
            offset,
            crc32: None,
            crc16: None,
        });
    }
    Ok(ChdHeader {
        version: 4,
        compressors: vec![compression],
        total_bytes,
        hunk_bytes,
        total_hunks,
        unit_bytes: 0,
        meta_offset,
        md5: None,
        raw_sha1: Some(raw_sha1),
        sha1: Some(sha1),
        parent_md5: None,
        parent_sha1: Some(parent_sha1),
        map,
    })
}

/// `ReadHeaderV5` (`CHDHeaders.cs:183-211`).
fn read_v5<R: Read + Seek>(rd: &mut Rd<'_, R>) -> Result<ChdHeader> {
    let mut compressors = Vec::with_capacity(4);
    for _ in 0..4 {
        compressors.push(rd.u32()?);
    }
    let total_bytes = rd.u64()?;
    let map_offset = rd.u64()?;
    let meta_offset = rd.u64()?;
    let hunk_bytes = rd.u32()?;
    let unit_bytes = rd.u32()?;
    let raw_sha1 = rd.bytes::<20>()?;
    let sha1 = rd.bytes::<20>()?;
    let parent_sha1 = rd.bytes::<20>()?;
    if hunk_bytes == 0 {
        return Err(invalid("the hunk size is zero"));
    }
    let total_hunks = u32::try_from(total_bytes.div_ceil(hunk_bytes as u64))
        .ok()
        .filter(|&n| n <= MAX_HUNKS)
        .ok_or_else(|| invalid("too many hunks"))?;
    let map = if compressors[0] != codec::NONE {
        compressed_v5_map(rd, map_offset, total_hunks, hunk_bytes, unit_bytes)?
    } else {
        uncompressed_v5_map(rd, map_offset, total_hunks, hunk_bytes)?
    };
    Ok(ChdHeader {
        version: 5,
        compressors,
        total_bytes,
        hunk_bytes,
        total_hunks,
        unit_bytes,
        meta_offset,
        md5: None,
        raw_sha1: Some(raw_sha1),
        sha1: Some(sha1),
        parent_md5: None,
        parent_sha1: Some(parent_sha1),
        map,
    })
}

/// `uncompressed_v5_map` (`CHDHeaders.cs:214-227`): one `u32` hunk number per hunk.
fn uncompressed_v5_map<R: Read + Seek>(
    rd: &mut Rd<'_, R>,
    map_offset: u64,
    total_hunks: u32,
    hunk_bytes: u32,
) -> Result<Vec<MapEntry>> {
    rd.r.seek(SeekFrom::Start(map_offset))?;
    let mut map = Vec::with_capacity((total_hunks as usize).min(1 << 20));
    for _ in 0..total_hunks {
        map.push(MapEntry {
            comp: CompType::None,
            length: hunk_bytes,
            offset: rd.u32()? as u64 * hunk_bytes as u64,
            crc32: None,
            crc16: None,
        });
    }
    Ok(map)
}

/// `compressed_v5_map` (`CHDHeaders.cs:229-367`): a Huffman-coded list of hunk types, then the
/// lengths, offsets and CRC-16s, all bit-packed.
fn compressed_v5_map<R: Read + Seek>(
    rd: &mut Rd<'_, R>,
    map_offset: u64,
    total_hunks: u32,
    hunk_bytes: u32,
    unit_bytes: u32,
) -> Result<Vec<MapEntry>> {
    rd.r.seek(SeekFrom::Start(map_offset))?;
    let map_bytes = rd.u32()?;
    let first_offset = rd.u48()?;
    let map_crc = rd.u16()?;
    let length_bits = rd.u8()? as u32;
    let self_bits = rd.u8()? as u32;
    let parent_bits = rd.u8()? as u32;
    rd.u8()?; // unused
    if length_bits > 24 || self_bits > 32 || parent_bits > 32 {
        return Err(invalid("a bit width in the map header is out of range"));
    }
    // Read the packed map through `take` so a lying size cannot allocate more than the file has.
    let mut packed = Vec::new();
    rd.r.take(map_bytes as u64).read_to_end(&mut packed)?;
    if packed.len() != map_bytes as usize {
        return Err(ArchiveError::Truncated { what: "CHD map" });
    }
    let mut bs = BitStream::new(&packed);

    let mut decoder = HuffmanDecoder::new(16, 8);
    decoder
        .import_tree_rle(&mut bs)
        .map_err(|e| ChdError::Decompression(format!("map tree: {e:?}")))?;

    // The hunk types, with run lengths.
    let mut types = Vec::with_capacity((total_hunks as usize).min(1 << 20));
    let (mut repcount, mut lastcomp) = (0u32, 0u32);
    for _ in 0..total_hunks {
        if repcount > 0 {
            types.push(lastcomp);
            repcount -= 1;
            continue;
        }
        let val = decoder.decode_one(&mut bs);
        if val == ct::RLE_SMALL {
            types.push(lastcomp);
            repcount = 2 + decoder.decode_one(&mut bs);
        } else if val == ct::RLE_LARGE {
            types.push(lastcomp);
            repcount = 2 + 16 + (decoder.decode_one(&mut bs) << 4);
            repcount += decoder.decode_one(&mut bs);
        } else {
            types.push(val);
            lastcomp = val;
        }
    }

    // Then each hunk's own data.
    let mut map = Vec::with_capacity(types.len());
    let (mut last_self, mut last_parent, mut cur) = (0u64, 0u64, first_offset);
    for (i, &ty) in types.iter().enumerate() {
        let mut offset = cur;
        let mut length = 0u32;
        let mut crc16 = None;
        let comp;
        match ty {
            0..=3 => {
                length = bs.read(length_bits);
                cur += length as u64;
                crc16 = Some(bs.read(16) as u16);
                comp = CompType::Codec(ty as u8);
            }
            ct::NONE => {
                length = hunk_bytes;
                cur += length as u64;
                crc16 = Some(bs.read(16) as u16);
                comp = CompType::None;
            }
            ct::SELF => {
                offset = bs.read(self_bits) as u64;
                last_self = offset;
                comp = CompType::SelfRef;
            }
            ct::SELF_1 | ct::SELF_0 => {
                if ty == ct::SELF_1 {
                    last_self += 1;
                }
                offset = last_self;
                comp = CompType::SelfRef;
            }
            ct::PARENT_SELF => {
                if unit_bytes == 0 {
                    return Err(invalid("the unit size is zero"));
                }
                offset = (i as u64 * hunk_bytes as u64) / unit_bytes as u64;
                last_parent = offset;
                comp = CompType::Parent;
            }
            ct::PARENT => {
                offset = bs.read(parent_bits) as u64;
                last_parent = offset;
                comp = CompType::Parent;
            }
            ct::PARENT_1 | ct::PARENT_0 => {
                if ty == ct::PARENT_1 {
                    if unit_bytes == 0 {
                        return Err(invalid("the unit size is zero"));
                    }
                    last_parent += (hunk_bytes / unit_bytes) as u64;
                }
                offset = last_parent;
                comp = CompType::Parent;
            }
            _ => return Err(invalid(format!("hunk {i} has the map type {ty}"))),
        }
        map.push(MapEntry {
            comp,
            length,
            offset,
            crc32: None,
            crc16,
        });
    }
    if bs.overflow() {
        return Err(ChdError::Decompression("the map is cut short".to_string()).into());
    }

    // The CRC-16 over the rebuilt 12-byte-per-hunk raw map must match the header's.
    let mut raw = Vec::with_capacity(map.len() * 12);
    for m in &map {
        let ty = match m.comp {
            CompType::Codec(n) => n,
            CompType::None => ct::NONE as u8,
            CompType::SelfRef => ct::SELF as u8,
            CompType::Parent => ct::PARENT as u8,
            CompType::Mini => 100,
            CompType::Error => 101,
        };
        raw.push(ty);
        raw.extend(&m.length.to_be_bytes()[1..]);
        raw.extend(&m.offset.to_be_bytes()[2..]);
        raw.extend(m.crc16.unwrap_or(0).to_be_bytes());
    }
    if crc16(&raw) != map_crc {
        return Err(ChdError::Decompression("the map CRC-16 does not match".to_string()).into());
    }
    Ok(map)
}

/// CRC-16/CCITT-FALSE: polynomial 0x1021, initial value 0xffff (`Utils/CRC16.cs`).
pub(super) fn crc16(data: &[u8]) -> u16 {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc16_is_ccitt_false() {
        assert_eq!(crc16(b"123456789"), 0x29b1);
        assert_eq!(crc16(b""), 0xffff);
    }

    #[test]
    fn codec_names() {
        assert_eq!(codec_name(codec::CD_LZMA), "cdlz");
        assert_eq!(codec_name(codec::NONE), "none");
    }
}
