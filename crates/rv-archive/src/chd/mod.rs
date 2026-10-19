//! CHD (MAME compressed hard disk) reading and verification, the check ROMVault runs on a CHD
//! (`CHD/CHDSharpLib/CHD.cs:72-171`): read the header and map, optionally decode every hunk,
//! recompute the raw MD5/SHA-1, and check the metadata hash.
//!
//! Spec: `docs/archives-and-rvio.md` section 8 and `docs/codecs.md` sections 12-14 of the
//! decompile notes (not part of this repository). Read only; nothing here writes a CHD.
//!
//! **Not supported** (typed [`ChdError`]): child CHDs (`RequiresParent`), `avhu` hunks
//! (LaserDisc video/audio, `UnsupportedCodec`), CHD versions above 5.

mod cdrom;
mod codecs;
mod flac;
mod header;
mod huffman;

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};

use md5::{Digest as _, Md5};
use sha1::Sha1;

pub use codecs::is_supported as is_codec_supported;
pub use header::{codec, codec_name, read_header, ChdHeader, CompType, MapEntry};

use crate::error::{ArchiveError, ChdError, Result};

/// Decoded hunks kept for `SelfRef` entries, at most this many bytes (`CHD.cs:164` keeps 512 MiB).
const CACHE_BUDGET: usize = 256 << 20;
/// The metadata chain is followed at most this far, so a loop cannot run forever.
const MAX_METADATA_ENTRIES: usize = 1 << 20;

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn all_zero(b: &[u8]) -> bool {
    b.iter().all(|&x| x == 0)
}

/// What a full verification found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifyReport {
    /// SHA-1 of the decoded data, equal to the header's raw SHA-1 (all CHDs from v3).
    pub raw_sha1: [u8; 20],
    /// MD5 of the decoded data when the header has one (v1 to v3).
    pub md5: Option<[u8; 16]>,
    /// SHA-1 over the raw SHA-1 and the checksummed metadata, equal to the header's `sha1` when
    /// the header has one (v4, v5).
    pub combined_sha1: Option<[u8; 20]>,
    pub hunks: u32,
    pub bytes: u64,
}

/// An open CHD.
pub struct Chd<R> {
    reader: R,
    header: ChdHeader,
}

impl<R: Read + Seek> Chd<R> {
    /// Parse the header and the hunk map (`CHD.CheckFile`, up to the parent check).
    pub fn open(mut reader: R) -> Result<Chd<R>> {
        let header = read_header(&mut reader)?;
        Ok(Chd { reader, header })
    }

    pub fn header(&self) -> &ChdHeader {
        &self.header
    }

    pub fn into_inner(self) -> R {
        self.reader
    }

    /// The first codec this CHD uses that this crate cannot decode, if any.
    pub fn unsupported_codec(&self) -> Option<u32> {
        let used = self.header.map.iter().filter_map(|m| match m.comp {
            CompType::Codec(n) => self.header.compressors.get(n as usize).copied(),
            _ => None,
        });
        used.into_iter().find(|&c| !is_codec_supported(c))
    }

    /// Decode hunk `index` into `out` (`hunk_bytes` long) and check its CRC.
    pub fn read_hunk(&mut self, index: usize, out: &mut [u8]) -> Result<()> {
        let mut cache = HunkCache::default();
        self.decode_hunk(index, out, &mut cache)
    }

    fn decode_hunk(&mut self, index: usize, out: &mut [u8], cache: &mut HunkCache) -> Result<()> {
        // Follow self references to the hunk that holds the data (`ReadBlock`, `:275-283`).
        let mut idx = index;
        for _ in 0..=self.header.map.len() {
            let e =
                self.header.map.get(idx).ok_or_else(|| {
                    ChdError::InvalidData(format!("hunk {idx} is not in the map"))
                })?;
            if e.comp != CompType::SelfRef {
                break;
            }
            idx = usize::try_from(e.offset).unwrap_or(usize::MAX);
            if idx == index {
                return Err(ChdError::InvalidData(format!("hunk {index} refers to itself")).into());
            }
        }
        if index != idx {
            if let Some(h) = cache.take(idx) {
                out.copy_from_slice(&h);
                return Ok(());
            }
        }
        let e = self
            .header
            .map
            .get(idx)
            .ok_or_else(|| ChdError::InvalidData(format!("hunk {idx} is not in the map")))?
            .clone();
        match e.comp {
            CompType::Codec(n) => {
                let c = *self.header.compressors.get(n as usize).ok_or_else(|| {
                    ChdError::InvalidData(format!("hunk {idx} uses codec slot {n}"))
                })?;
                let input = self.read_stored(&e)?;
                codecs::decode(c, &input, out)?;
            }
            CompType::None => {
                if e.length as usize != out.len() {
                    return Err(ChdError::InvalidData(format!(
                        "uncompressed hunk {idx} is {} bytes, not {}",
                        e.length,
                        out.len()
                    ))
                    .into());
                }
                let input = self.read_stored(&e)?;
                out.copy_from_slice(&input);
            }
            CompType::Mini => {
                let bytes = e.offset.to_be_bytes();
                for (i, b) in out.iter_mut().enumerate() {
                    *b = bytes[i % 8];
                }
            }
            CompType::Parent => return Err(ChdError::RequiresParent.into()),
            CompType::SelfRef | CompType::Error => {
                return Err(ChdError::Decompression(format!(
                    "hunk {idx} has a map entry that cannot be read"
                ))
                .into())
            }
        }
        if let Some(want) = e.crc32 {
            let got = crc32fast::hash(out);
            if got != want {
                return Err(ChdError::Decompression(format!(
                    "hunk {idx}: CRC-32 is {got:08x}, the map says {want:08x}"
                ))
                .into());
            }
        }
        if let Some(want) = e.crc16 {
            let got = header::crc16(out);
            if got != want {
                return Err(ChdError::Decompression(format!(
                    "hunk {idx}: CRC-16 is {got:04x}, the map says {want:04x}"
                ))
                .into());
            }
        }
        if cache.wants(idx) {
            cache.put(idx, out.to_vec());
        }
        Ok(())
    }

    fn read_stored(&mut self, e: &MapEntry) -> Result<Vec<u8>> {
        // A lying length cannot allocate more than the file holds.
        self.reader.seek(SeekFrom::Start(e.offset))?;
        let mut buf = Vec::new();
        (&mut self.reader)
            .take(e.length as u64)
            .read_to_end(&mut buf)?;
        if buf.len() != e.length as usize {
            return Err(ArchiveError::Truncated {
                what: "CHD hunk data",
            });
        }
        Ok(buf)
    }

    /// Decode every hunk and check the hashes: the raw MD5/SHA-1 against the header, then the
    /// metadata hash (`CHD.DecompressData`, `CHD.cs:196-266`, and `CHDMetaData.ReadMetaData`).
    pub fn verify(&mut self) -> Result<VerifyReport> {
        if self.header.requires_parent() {
            return Err(ChdError::RequiresParent.into());
        }
        if self.header.total_bytes != self.header.total_hunks as u64 * self.header.hunk_bytes as u64
            && self
                .header
                .total_bytes
                .div_ceil(self.header.hunk_bytes.max(1) as u64)
                != self.header.total_hunks as u64
        {
            return Err(ChdError::InvalidData(
                "the hunk count does not cover the data".to_string(),
            )
            .into());
        }
        let hunk = self.header.hunk_bytes as usize;
        let mut buf = vec![0u8; hunk];
        let mut sha = Sha1::new();
        let mut md5 = self.header.md5.map(|_| Md5::new());
        let mut cache = HunkCache::new(&self.header.map, hunk);
        let mut left = self.header.total_bytes;
        for i in 0..self.header.total_hunks as usize {
            self.decode_hunk(i, &mut buf, &mut cache)?;
            let n = left.min(hunk as u64) as usize;
            sha.update(&buf[..n]);
            if let Some(m) = md5.as_mut() {
                m.update(&buf[..n]);
            }
            left -= n as u64;
        }
        let raw_sha1: [u8; 20] = sha.finalize().into();
        let md5_out: Option<[u8; 16]> = md5.map(|m| m.finalize().into());

        if let (Some(want), Some(got)) = (self.header.md5, md5_out) {
            if !all_zero(&want) && want != got {
                return Err(ChdError::VerifyFailed {
                    what: "MD5",
                    expected: hex(&want),
                    actual: hex(&got),
                }
                .into());
            }
        }
        if let Some(want) = self.header.raw_sha1 {
            if !all_zero(&want) && want != raw_sha1 {
                return Err(ChdError::VerifyFailed {
                    what: "raw SHA-1",
                    expected: hex(&want),
                    actual: hex(&raw_sha1),
                }
                .into());
            }
        }
        let combined_sha1 = self.check_metadata()?;
        Ok(VerifyReport {
            raw_sha1,
            md5: md5_out,
            combined_sha1,
            hunks: self.header.total_hunks,
            bytes: self.header.total_bytes,
        })
    }

    /// `CHDMetaData.ReadMetaData` (`CHDMetaData.cs:15-80`).
    fn check_metadata(&mut self) -> Result<Option<[u8; 20]>> {
        let mut hashes: Vec<[u8; 24]> = Vec::new();
        let mut next = self.header.meta_offset;
        let mut seen = std::collections::HashSet::new();
        while next != 0 {
            if !seen.insert(next) || seen.len() > MAX_METADATA_ENTRIES {
                return Err(
                    ChdError::InvalidMetadata("the metadata chain loops".to_string()).into(),
                );
            }
            self.reader.seek(SeekFrom::Start(next))?;
            let mut head = [0u8; 16];
            self.reader
                .read_exact(&mut head)
                .map_err(|_| ArchiveError::Truncated {
                    what: "CHD metadata",
                })?;
            let tag = u32::from_be_bytes(head[0..4].try_into().unwrap());
            let len_flags = u32::from_be_bytes(head[4..8].try_into().unwrap());
            next = u64::from_be_bytes(head[8..16].try_into().unwrap());
            let flags = len_flags >> 24;
            let len = (len_flags & 0x00ff_ffff) as u64;
            let mut data = Vec::new();
            (&mut self.reader).take(len).read_to_end(&mut data)?;
            if data.len() as u64 != len {
                return Err(ArchiveError::Truncated {
                    what: "CHD metadata",
                });
            }
            // CHD_MDFLAGS_CHECKSUM: this entry is covered by the combined SHA-1.
            if flags & 0x01 != 0 {
                let mut h = [0u8; 24];
                h[..4].copy_from_slice(&tag.to_be_bytes());
                h[4..].copy_from_slice(&Sha1::digest(&data));
                hashes.push(h);
            }
        }
        let Some(want) = self.header.sha1 else {
            return Ok(None);
        };
        hashes.sort();
        let mut total = Sha1::new();
        total.update(self.header.raw_sha1.unwrap_or([0; 20]));
        for h in &hashes {
            total.update(h);
        }
        let got: [u8; 20] = total.finalize().into();
        if !all_zero(&want) && want != got {
            return Err(ChdError::InvalidMetadata(format!(
                "combined SHA-1 is {}, the header says {}",
                hex(&got),
                hex(&want)
            ))
            .into());
        }
        Ok(Some(got))
    }
}

/// Decoded hunks that later `SelfRef` entries will copy, so each is decoded once (ROMVault's
/// `KeepMostRepeatedBlocks`).
#[derive(Default)]
struct HunkCache {
    uses: HashMap<usize, u32>,
    held: HashMap<usize, Vec<u8>>,
    held_bytes: usize,
}

impl HunkCache {
    fn new(map: &[MapEntry], hunk: usize) -> HunkCache {
        let mut uses: HashMap<usize, u32> = HashMap::new();
        for (i, e) in map.iter().enumerate() {
            if e.comp != CompType::SelfRef {
                continue;
            }
            // Resolve the chain to the hunk that holds the data.
            let mut idx = e.offset as usize;
            for _ in 0..map.len() {
                match map.get(idx) {
                    Some(t) if t.comp == CompType::SelfRef && idx != i => idx = t.offset as usize,
                    _ => break,
                }
            }
            *uses.entry(idx).or_default() += 1;
        }
        let _ = hunk;
        HunkCache {
            uses,
            held: HashMap::new(),
            held_bytes: 0,
        }
    }

    fn wants(&self, idx: usize) -> bool {
        self.uses.get(&idx).is_some_and(|&n| n > 0) && !self.held.contains_key(&idx)
    }

    fn put(&mut self, idx: usize, data: Vec<u8>) {
        if self.held_bytes + data.len() <= CACHE_BUDGET {
            self.held_bytes += data.len();
            self.held.insert(idx, data);
        }
    }

    fn take(&mut self, idx: usize) -> Option<Vec<u8>> {
        let left = self.uses.get_mut(&idx)?;
        *left = left.saturating_sub(1);
        let data = if *left == 0 {
            let d = self.held.remove(&idx)?;
            self.held_bytes -= d.len();
            d
        } else {
            self.held.get(&idx)?.clone()
        };
        Some(data)
    }
}
