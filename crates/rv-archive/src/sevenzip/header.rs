//! 7z header parsing: signature header, (encoded) header, streams and files info
//! (`Structure/{SignatureHeader,Header,StreamsInfo,Folder,Coder,FileInfo,PackedStreamInfo}.cs`,
//! `Util.cs`). The property ids are numbered from 0 (`Util.cs:7-47`).

use crate::error::{ArchiveError, Result};

pub(super) const SIGNATURE: [u8; 6] = [0x37, 0x7a, 0xbc, 0xaf, 0x27, 0x1c];

pub(super) const K_END: u8 = 0x00;
pub(super) const K_HEADER: u8 = 0x01;
pub(super) const K_ARCHIVE_PROPERTIES: u8 = 0x02;
pub(super) const K_ADDITIONAL_STREAMS_INFO: u8 = 0x03;
pub(super) const K_MAIN_STREAMS_INFO: u8 = 0x04;
pub(super) const K_FILES_INFO: u8 = 0x05;
pub(super) const K_PACK_INFO: u8 = 0x06;
pub(super) const K_UNPACK_INFO: u8 = 0x07;
pub(super) const K_SUBSTREAMS_INFO: u8 = 0x08;
pub(super) const K_SIZE: u8 = 0x09;
pub(super) const K_CRC: u8 = 0x0a;
pub(super) const K_FOLDER: u8 = 0x0b;
pub(super) const K_CODERS_UNPACK_SIZE: u8 = 0x0c;
pub(super) const K_NUM_UNPACK_STREAM: u8 = 0x0d;
pub(super) const K_EMPTY_STREAM: u8 = 0x0e;
pub(super) const K_EMPTY_FILE: u8 = 0x0f;
pub(super) const K_ANTI: u8 = 0x10;
pub(super) const K_NAME: u8 = 0x11;
pub(super) const K_CREATION_TIME: u8 = 0x12;
pub(super) const K_LAST_ACCESS_TIME: u8 = 0x13;
pub(super) const K_LAST_WRITE_TIME: u8 = 0x14;
pub(super) const K_WIN_ATTRIBUTES: u8 = 0x15;
pub(super) const K_ENCODED_HEADER: u8 = 0x17;

/// `Coder` (`Structure/Coder.cs`): the method id, the stream counts and the properties.
#[derive(Clone, Debug)]
pub struct Coder {
    pub method: Vec<u8>,
    pub num_in_streams: u64,
    pub num_out_streams: u64,
    pub properties: Vec<u8>,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct BindPair {
    pub in_index: u64,
    pub out_index: u64,
}

/// A folder: one independently decodable unit (`Structure/Folder.cs`).
#[derive(Clone, Debug)]
pub struct Folder {
    pub coders: Vec<Coder>,
    pub(super) bind_pairs: Vec<BindPair>,
    /// In-stream indices fed by packed streams, in pack order.
    pub(super) packed_streams: Vec<u64>,
    /// One size per out stream, over all coders.
    pub unpack_sizes: Vec<u64>,
    pub unpack_crc: Option<u32>,
    /// Index of this folder's first packed stream in the archive's pack list.
    pub(super) first_pack_stream: usize,
    /// The sizes and CRCs of the files stored in this folder, in order.
    pub substreams: Vec<SubStream>,
}

#[derive(Clone, Copy, Debug)]
pub struct SubStream {
    pub size: u64,
    pub crc: Option<u32>,
}

impl Folder {
    /// The out stream that is not bound to any coder's input: the folder's result
    /// (`Folder.GetUnpackSize`, `Folder.cs:92-120`).
    pub(super) fn main_out_stream(&self) -> Option<u64> {
        let total_out: u64 = self.coders.iter().map(|c| c.num_out_streams).sum();
        (0..total_out).find(|j| !self.bind_pairs.iter().any(|b| b.out_index == *j))
    }

    /// Size of the folder's result.
    pub fn unpack_size(&self) -> u64 {
        self.main_out_stream()
            .and_then(|j| self.unpack_sizes.get(j as usize).copied())
            .unwrap_or(0)
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct StreamsInfo {
    pub pack_pos: u64,
    pub pack_sizes: Vec<u64>,
    pub folders: Vec<Folder>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct FilesInfo {
    pub names: Vec<String>,
    pub empty_stream: Option<Vec<bool>>,
    pub empty_file: Option<Vec<bool>>,
    pub anti: Option<Vec<bool>>,
    pub creation: Option<Vec<Option<u64>>>,
    pub access: Option<Vec<Option<u64>>>,
    pub write: Option<Vec<Option<u64>>>,
    pub attributes: Option<Vec<Option<u32>>>,
}

#[derive(Debug, Default)]
pub(super) struct Header {
    pub streams: StreamsInfo,
    pub files: FilesInfo,
}

/// What the (possibly encoded) header turned out to be.
pub(super) enum Parsed {
    Header(Header),
    /// `kEncodedHeader`: the streams info describing where the real header is packed.
    Encoded(StreamsInfo),
}

pub(super) struct ByteReader<'a> {
    buf: &'a [u8],
    pos: usize,
}

fn trunc() -> ArchiveError {
    ArchiveError::Truncated { what: "7z header" }
}

fn bad(detail: impl Into<String>) -> ArchiveError {
    ArchiveError::corrupt("7z header", detail)
}

impl<'a> ByteReader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        ByteReader { buf, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if n > self.remaining() {
            return Err(trunc());
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    /// `ReadEncodedUInt64` (`Util.cs:84-108`): the high bits of the first byte say how many
    /// extra little-endian bytes follow.
    pub fn number(&mut self) -> Result<u64> {
        let first = self.byte()?;
        let mut mask = 0x80u8;
        let mut value = 0u64;
        for i in 0..8 {
            if first & mask == 0 {
                let high = (first & (mask - 1)) as u64;
                return Ok(value + (high << (8 * i)));
            }
            value |= (self.byte()? as u64) << (8 * i);
            mask >>= 1;
        }
        Ok(value)
    }

    /// A count that must be addressable by what is left of the header, so a corrupt value cannot
    /// make us allocate gigabytes. `per_item_bits` is the least each item takes.
    fn count(&mut self, per_item_bits: usize) -> Result<usize> {
        let n = self.number()?;
        let max = (self.remaining() as u64).saturating_mul(8) / per_item_bits.max(1) as u64;
        if n > max {
            return Err(bad(format!("a count of {n} cannot fit in the header")));
        }
        Ok(n as usize)
    }

    /// `ReadBoolFlags` (`Util.cs:220`): MSB first.
    fn bools(&mut self, n: usize) -> Result<Vec<bool>> {
        let mut out = Vec::with_capacity(n);
        let (mut b, mut mask) = (0u8, 0u8);
        for _ in 0..n {
            if mask == 0 {
                b = self.byte()?;
                mask = 0x80;
            }
            out.push(b & mask != 0);
            mask >>= 1;
        }
        Ok(out)
    }

    /// `ReadBoolFlagsDefaultTrue` (`Util.cs:204-216`).
    fn bools_default_true(&mut self, n: usize) -> Result<Vec<bool>> {
        if self.byte()? == 0 {
            self.bools(n)
        } else {
            Ok(vec![true; n])
        }
    }

    /// `UnPackCRCs` (`Util.cs:177-190`).
    fn digests(&mut self, n: usize) -> Result<Vec<Option<u32>>> {
        let defined = self.bools_default_true(n)?;
        defined
            .into_iter()
            .map(|d| if d { self.u32().map(Some) } else { Ok(None) })
            .collect()
    }
}

/// `SignatureHeader.Read` (`Structure/SignatureHeader.cs:39-69`): returns `(next_header_offset,
/// next_header_size, next_header_crc)`.
pub(super) fn parse_signature_header(b: &[u8; 32]) -> Result<(u64, u64, u32)> {
    if b[..6] != SIGNATURE {
        return Err(ArchiveError::BadSignature {
            what: "7z signature header",
        });
    }
    let start_crc = u32::from_le_bytes(b[8..12].try_into().unwrap());
    if crc32fast::hash(&b[12..32]) != start_crc {
        return Err(bad("the start header CRC does not match"));
    }
    Ok((
        u64::from_le_bytes(b[12..20].try_into().unwrap()),
        u64::from_le_bytes(b[20..28].try_into().unwrap()),
        u32::from_le_bytes(b[28..32].try_into().unwrap()),
    ))
}

/// `Header.ReadHeaderOrPackedHeader` (`Structure/Header.cs:48-84`).
pub(super) fn parse_header(buf: &[u8]) -> Result<Parsed> {
    let mut r = ByteReader::new(buf);
    match r.byte()? {
        K_HEADER => Ok(Parsed::Header(read_header(&mut r)?)),
        K_ENCODED_HEADER => Ok(Parsed::Encoded(read_streams_info(&mut r)?)),
        other => Err(bad(format!("unknown header type {other:#04x}"))),
    }
}

fn read_header(r: &mut ByteReader<'_>) -> Result<Header> {
    let mut h = Header::default();
    loop {
        match r.byte()? {
            K_END => return Ok(h),
            K_ARCHIVE_PROPERTIES => skip_archive_properties(r)?,
            K_ADDITIONAL_STREAMS_INFO => {
                return Err(ArchiveError::Unsupported {
                    what: "7z additional streams",
                })
            }
            K_MAIN_STREAMS_INFO => h.streams = read_streams_info(r)?,
            K_FILES_INFO => h.files = read_files_info(r)?,
            other => {
                return Err(bad(format!(
                    "unexpected property {other:#04x} in the header"
                )))
            }
        }
    }
}

fn skip_archive_properties(r: &mut ByteReader<'_>) -> Result<()> {
    loop {
        if r.byte()? == K_END {
            return Ok(());
        }
        let n = r.number()?;
        r.take(usize::try_from(n).map_err(|_| trunc())?)?;
    }
}

/// `StreamsInfo.Read` (`Structure/StreamsInfo.cs`).
fn read_streams_info(r: &mut ByteReader<'_>) -> Result<StreamsInfo> {
    let mut s = StreamsInfo::default();
    let mut have_substreams = false;
    loop {
        match r.byte()? {
            K_END => break,
            K_PACK_INFO => read_pack_info(r, &mut s)?,
            K_UNPACK_INFO => read_unpack_info(r, &mut s)?,
            K_SUBSTREAMS_INFO => {
                read_substreams_info(r, &mut s.folders)?;
                have_substreams = true;
            }
            other => {
                return Err(bad(format!(
                    "unexpected property {other:#04x} in streams info"
                )))
            }
        }
    }
    if !have_substreams {
        // One file per folder, the size of the folder (`7zFormat.txt`: SubStreamsInfo is
        // optional).
        for f in &mut s.folders {
            f.substreams = vec![SubStream {
                size: f.unpack_size(),
                crc: f.unpack_crc,
            }];
        }
    }
    Ok(s)
}

/// `PackedStreamInfo.Read` (`Structure/PackedStreamInfo.cs`).
fn read_pack_info(r: &mut ByteReader<'_>, s: &mut StreamsInfo) -> Result<()> {
    s.pack_pos = r.number()?;
    let n = r.count(8)?;
    loop {
        match r.byte()? {
            K_END => break,
            K_SIZE => {
                s.pack_sizes = (0..n).map(|_| r.number()).collect::<Result<_>>()?;
            }
            K_CRC => {
                r.digests(n)?;
            }
            other => {
                return Err(bad(format!(
                    "unexpected property {other:#04x} in pack info"
                )))
            }
        }
    }
    if s.pack_sizes.len() != n {
        return Err(bad("pack info has no sizes"));
    }
    Ok(())
}

/// `Folder.ReadUnPackInfo` (`Structure/Folder.cs:109-170`).
fn read_unpack_info(r: &mut ByteReader<'_>, s: &mut StreamsInfo) -> Result<()> {
    loop {
        match r.byte()? {
            K_END => return Ok(()),
            K_FOLDER => {
                let n = r.count(16)?;
                if r.byte()? != 0 {
                    return Err(ArchiveError::Unsupported {
                        what: "external 7z header data",
                    });
                }
                s.folders = Vec::with_capacity(n);
                let mut first_pack = 0usize;
                for _ in 0..n {
                    let mut f = read_folder(r)?;
                    f.first_pack_stream = first_pack;
                    first_pack += f.packed_streams.len();
                    s.folders.push(f);
                }
            }
            K_CODERS_UNPACK_SIZE => {
                for f in &mut s.folders {
                    let total_out: u64 = f.coders.iter().map(|c| c.num_out_streams).sum();
                    f.unpack_sizes = (0..total_out).map(|_| r.number()).collect::<Result<_>>()?;
                }
            }
            K_CRC => {
                let crcs = r.digests(s.folders.len())?;
                for (f, c) in s.folders.iter_mut().zip(crcs) {
                    f.unpack_crc = c;
                }
            }
            other => {
                return Err(bad(format!(
                    "unexpected property {other:#04x} in unpack info"
                )))
            }
        }
    }
}

/// `Folder.ReadFolder` (`Structure/Folder.cs:31-90`).
fn read_folder(r: &mut ByteReader<'_>) -> Result<Folder> {
    let n_coders = r.count(8)?;
    if n_coders == 0 || n_coders > 32 {
        return Err(bad(format!("a folder with {n_coders} coders")));
    }
    let mut coders = Vec::with_capacity(n_coders);
    for _ in 0..n_coders {
        let flags = r.byte()?;
        if flags & 0x80 != 0 {
            return Err(ArchiveError::Unsupported {
                what: "7z alternative coder methods",
            });
        }
        let method = r.take((flags & 0x0f) as usize)?.to_vec();
        let (num_in, num_out) = if flags & 0x10 != 0 {
            (r.number()?, r.number()?)
        } else {
            (1, 1)
        };
        if num_in > 32 || num_out > 32 {
            return Err(bad("a coder with too many streams"));
        }
        let properties = if flags & 0x20 != 0 {
            let n = r.count(8)?;
            r.take(n)?.to_vec()
        } else {
            Vec::new()
        };
        coders.push(Coder {
            method,
            num_in_streams: num_in,
            num_out_streams: num_out,
            properties,
        });
    }
    let total_in: u64 = coders.iter().map(|c| c.num_in_streams).sum();
    let total_out: u64 = coders.iter().map(|c| c.num_out_streams).sum();
    if total_out == 0 {
        return Err(bad("a folder without output streams"));
    }
    let n_bind = total_out - 1;
    if n_bind > total_in {
        return Err(bad("a folder with more bind pairs than input streams"));
    }
    let mut bind_pairs = Vec::new();
    for _ in 0..n_bind {
        let in_index = r.number()?;
        let out_index = r.number()?;
        if in_index >= total_in || out_index >= total_out {
            return Err(bad("a bind pair points outside the folder"));
        }
        bind_pairs.push(BindPair {
            in_index,
            out_index,
        });
    }
    let n_packed = total_in - n_bind;
    let packed_streams = if n_packed == 1 {
        let free = (0..total_in).find(|i| !bind_pairs.iter().any(|b| b.in_index == *i));
        vec![free.ok_or_else(|| bad("a folder with no free input stream"))?]
    } else {
        (0..n_packed)
            .map(|_| {
                let i = r.number()?;
                if i >= total_in {
                    Err(bad("a packed stream index is outside the folder"))
                } else {
                    Ok(i)
                }
            })
            .collect::<Result<_>>()?
    };
    Ok(Folder {
        coders,
        bind_pairs,
        packed_streams,
        unpack_sizes: Vec::new(),
        unpack_crc: None,
        first_pack_stream: 0,
        substreams: Vec::new(),
    })
}

/// `Folder.ReadSubStreamsInfo` (`Structure/Folder.cs:196-300`).
fn read_substreams_info(r: &mut ByteReader<'_>, folders: &mut [Folder]) -> Result<()> {
    let mut counts: Vec<usize> = vec![1; folders.len()];
    let mut ty = r.byte()?;
    if ty == K_NUM_UNPACK_STREAM {
        for c in counts.iter_mut() {
            *c = r.count(1)?;
        }
        ty = r.byte()?;
    }
    let mut sizes: Vec<Vec<u64>> = Vec::with_capacity(folders.len());
    for (f, &n) in folders.iter().zip(&counts) {
        let mut v = Vec::with_capacity(n);
        if n > 0 {
            let total = f.unpack_size();
            let mut sum = 0u64;
            if ty == K_SIZE {
                for _ in 0..n - 1 {
                    let s = r.number()?;
                    sum = sum
                        .checked_add(s)
                        .filter(|&x| x <= total)
                        .ok_or_else(|| bad("file sizes are larger than their folder"))?;
                    v.push(s);
                }
            } else if n > 1 {
                return Err(bad("a folder with several streams and no sizes"));
            }
            v.push(total - sum);
        }
        sizes.push(v);
    }
    if ty == K_SIZE {
        ty = r.byte()?;
    }
    // Digests: only for streams whose CRC is not already the folder CRC.
    let mut crcs: Vec<Vec<Option<u32>>> = folders
        .iter()
        .zip(&counts)
        .map(|(f, &n)| {
            if n == 1 && f.unpack_crc.is_some() {
                vec![f.unpack_crc]
            } else {
                vec![None; n]
            }
        })
        .collect();
    loop {
        match ty {
            K_END => break,
            K_CRC => {
                let need: usize = folders
                    .iter()
                    .zip(&counts)
                    .filter(|(f, &n)| !(n == 1 && f.unpack_crc.is_some()))
                    .map(|(_, &n)| n)
                    .sum();
                let mut digests = r.digests(need)?.into_iter();
                for ((f, &n), c) in folders.iter().zip(&counts).zip(crcs.iter_mut()) {
                    if n == 1 && f.unpack_crc.is_some() {
                        continue;
                    }
                    for slot in c.iter_mut() {
                        *slot = digests.next().flatten();
                    }
                }
            }
            other => {
                return Err(bad(format!(
                    "unexpected property {other:#04x} in substreams info"
                )))
            }
        }
        ty = r.byte()?;
    }
    for ((f, sz), c) in folders.iter_mut().zip(sizes).zip(crcs) {
        f.substreams = sz
            .into_iter()
            .zip(c)
            .map(|(size, crc)| SubStream { size, crc })
            .collect();
    }
    Ok(())
}

/// `FileInfo.Read` (`Structure/FileInfo.cs`).
fn read_files_info(r: &mut ByteReader<'_>) -> Result<FilesInfo> {
    let n = r.count(1)?;
    let mut f = FilesInfo::default();
    loop {
        let ty = r.byte()?;
        if ty == K_END {
            break;
        }
        let size = usize::try_from(r.number()?).map_err(|_| trunc())?;
        let data = r.take(size)?;
        let mut p = ByteReader::new(data);
        match ty {
            K_EMPTY_STREAM => f.empty_stream = Some(p.bools(n)?),
            K_EMPTY_FILE => {
                let empties = f
                    .empty_stream
                    .as_ref()
                    .map_or(0, |v| v.iter().filter(|&&b| b).count());
                f.empty_file = Some(p.bools(empties)?);
            }
            K_ANTI => {
                let empties = f
                    .empty_stream
                    .as_ref()
                    .map_or(0, |v| v.iter().filter(|&&b| b).count());
                f.anti = Some(p.bools(empties)?);
            }
            K_NAME => {
                if p.byte()? != 0 {
                    return Err(ArchiveError::Unsupported {
                        what: "external 7z names",
                    });
                }
                f.names = read_names(&mut p, n)?;
            }
            K_CREATION_TIME => f.creation = Some(read_times(&mut p, n)?),
            K_LAST_ACCESS_TIME => f.access = Some(read_times(&mut p, n)?),
            K_LAST_WRITE_TIME => f.write = Some(read_times(&mut p, n)?),
            K_WIN_ATTRIBUTES => {
                let defined = p.bools_default_true(n)?;
                if p.byte()? != 0 {
                    return Err(ArchiveError::Unsupported {
                        what: "external 7z attributes",
                    });
                }
                f.attributes = Some(
                    defined
                        .into_iter()
                        .map(|d| if d { p.u32().map(Some) } else { Ok(None) })
                        .collect::<Result<_>>()?,
                );
            }
            // kComment, kStartPos, kDummy and anything unknown are skipped by size
            // (`FileInfo.cs:68-70,82-84`).
            _ => {}
        }
    }
    if f.names.len() != n {
        return Err(bad("the number of names differs from the number of files"));
    }
    Ok(f)
}

/// `ReadName` over all files (`Util.cs:134-157`): UTF-16LE, NUL terminated.
fn read_names(p: &mut ByteReader<'_>, n: usize) -> Result<Vec<String>> {
    let mut names = Vec::with_capacity(n);
    for _ in 0..n {
        let mut units = Vec::new();
        loop {
            let b = p.take(2)?;
            let u = u16::from_le_bytes([b[0], b[1]]);
            if u == 0 {
                break;
            }
            units.push(u);
        }
        names.push(String::from_utf16_lossy(&units));
    }
    Ok(names)
}

fn read_times(p: &mut ByteReader<'_>, n: usize) -> Result<Vec<Option<u64>>> {
    let defined = p.bools_default_true(n)?;
    if p.byte()? != 0 {
        return Err(ArchiveError::Unsupported {
            what: "external 7z times",
        });
    }
    defined
        .into_iter()
        .map(|d| if d { p.u64().map(Some) } else { Ok(None) })
        .collect()
}
