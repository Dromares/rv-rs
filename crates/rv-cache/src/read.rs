//! Parsing: `RvFile(BinaryReader, ...)` (`rvFile.cs:513-689`) without its post-load
//! fix-ups, which live in [`crate::fixups`] so that reading stays lossless.

use rv_core::{
    DatFlags, DatStatus, FileStatus, FileType, GotStatus, HeaderFileType, MetaEntry, MiaStatus,
    RvDat, RvFile, RvGame, RvTreeRow, ToSortDirType, TreeSelect, ZipStructure,
};

use crate::error::CacheError;
use crate::flags;
use crate::{CacheFile, END_MARKER, MAX_DEPTH};

/// Smallest possible serialized child: type, flags, two empty strings, timestamp,
/// dat/got status, file status.
const MIN_CHILD_BYTES: usize = 1 + 4 + 1 + 1 + 8 + 1 + 1 + 4;
/// Smallest possible serialized `Dat`: timestamp, flags, entry count.
const MIN_DAT_BYTES: usize = 8 + 1 + 1;

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], CacheError> {
        if self.remaining() < n {
            return Err(CacheError::Truncated { offset: self.pos });
        }
        let slice = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], CacheError> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }

    fn u8(&mut self) -> Result<u8, CacheError> {
        Ok(self.array::<1>()?[0])
    }

    fn u32(&mut self) -> Result<u32, CacheError> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    fn i32(&mut self) -> Result<i32, CacheError> {
        Ok(i32::from_le_bytes(self.array()?))
    }

    fn u64(&mut self) -> Result<u64, CacheError> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    fn i64(&mut self) -> Result<i64, CacheError> {
        Ok(i64::from_le_bytes(self.array()?))
    }

    /// `BinaryReader.ReadBoolean`, but only 0 and 1 (anything else would not write back).
    fn bool(&mut self) -> Result<bool, CacheError> {
        let offset = self.pos;
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(CacheError::NonCanonical {
                offset,
                what: "bool byte is not 0 or 1",
            }),
        }
    }

    /// `BinaryReader.ReadString`: a 7-bit varint byte length (at most 5 bytes, the
    /// shortest encoding), then UTF-8 (BCL, cache section 2).
    fn string(&mut self) -> Result<String, CacheError> {
        let start = self.pos;
        let mut len: u32 = 0;
        let mut bytes_used = 0u32;
        loop {
            let b = self.u8()?;
            if bytes_used == 4 && b > 0x07 {
                // More than 31 bits: negative or overflowing in .NET.
                return Err(CacheError::BadCount {
                    offset: start,
                    count: -1,
                });
            }
            len |= u32::from(b & 0x7f) << (7 * bytes_used);
            bytes_used += 1;
            if b & 0x80 == 0 {
                if bytes_used > 1 && b == 0 {
                    return Err(CacheError::NonCanonical {
                        offset: start,
                        what: "string length is not the shortest varint",
                    });
                }
                break;
            }
        }
        let len = len as usize;
        let text_at = self.pos;
        let bytes = self.take(len)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| CacheError::InvalidUtf8 { offset: text_at })
    }

    /// `ReadByteArray`: `u8` length then the bytes (`ByteUtils.cs:16-28`).
    fn byte_array(&mut self) -> Result<Vec<u8>, CacheError> {
        let len = usize::from(self.u8()?);
        Ok(self.take(len)?.to_vec())
    }

    /// A positive `i32` count whose elements take at least `min_bytes` each, bounded by the
    /// bytes that are left so that a corrupt count cannot trigger a huge allocation.
    fn count(&mut self, min_bytes: usize) -> Result<usize, CacheError> {
        let offset = self.pos;
        let count = self.i32()?;
        if count < 0 || (count as usize).saturating_mul(min_bytes) > self.remaining() {
            return Err(CacheError::BadCount {
                offset,
                count: i64::from(count),
            });
        }
        if count == 0 {
            return Err(CacheError::NonCanonical {
                offset,
                what: "count flag set with a count of zero",
            });
        }
        Ok(count as usize)
    }

    fn meta_entries(&mut self) -> Result<Vec<MetaEntry>, CacheError> {
        let count = usize::from(self.u8()?);
        let mut entries = Vec::with_capacity(count.min(self.remaining()));
        for _ in 0..count {
            let id = self.u8()?;
            let value = self.string()?;
            entries.push(MetaEntry { id, value });
        }
        Ok(entries)
    }

    /// `RvDat(BinaryReader)` (`RvDat.cs:66-77`).
    fn dat(&mut self) -> Result<RvDat, CacheError> {
        Ok(RvDat {
            time_stamp: self.i64()?,
            dat_flags: DatFlags(self.u8()?),
            entries: self.meta_entries()?,
        })
    }

    fn node(&mut self, version: i32, is_root: bool, depth: usize) -> Result<RvFile, CacheError> {
        if depth > MAX_DEPTH {
            return Err(CacheError::TooDeep);
        }
        // The root has no type byte (rvFile.cs:515-518).
        let file_type = if is_root {
            FileType::DIR
        } else {
            FileType(self.u8()?)
        };
        let mut node = RvFile::new(file_type, "");

        let fl = self.u32()?; // rvFile.cs:528
        node.unknown_flag_bits = fl & flags::UNKNOWN_MASK;
        node.name = self.string()?; // :532
        node.file_name = self.string()?; // :533
        node.file_mod_time_stamp = self.i64()?; // :534
        if fl & flags::HAS_DAT != 0 {
            node.dat_index = Some(self.i32()?); // :537-549
        }
        node.dat_status = DatStatus(self.u8()?); // :552
        node.got_status = GotStatus(self.u8()?); // :561
        if file_type.is_compressed_dir() {
            if version >= 3 {
                node.zip_dat_struct = Some(self.u8()?); // :583-586
            }
            node.zip_struct = Some(ZipStructure(self.u8()?)); // :588
        }
        if fl & flags::HAS_TREE != 0 {
            // RvTreeRow.cs:36-37
            node.tree = Some(RvTreeRow {
                expanded: self.bool()?,
                checked: TreeSelect(self.u8()?),
            });
        }
        if fl & flags::HAS_GAME != 0 {
            node.game = Some(RvGame {
                entries: self.meta_entries()?,
            }); // :606-610
        }
        if fl & flags::HAS_DIR_DAT != 0 {
            let count = self.count(MIN_DAT_BYTES)?; // :612
            node.dir_dats.reserve(count);
            for _ in 0..count {
                node.dir_dats.push(self.dat()?); // :615-633
            }
        }
        if fl & flags::HAS_CHILDREN != 0 {
            let count = self.count(MIN_CHILD_BYTES)?; // :640
            node.children.reserve(count);
            for _ in 0..count {
                node.children.push(self.node(version, false, depth + 1)?); // :647-654
            }
        }
        if fl & flags::SIZE != 0 {
            node.size = Some(self.u64()?); // :660
        }
        if fl & flags::CRC != 0 {
            node.crc = Some(self.byte_array()?);
        }
        if fl & flags::SHA1 != 0 {
            node.sha1 = Some(self.byte_array()?);
        }
        if fl & flags::MD5 != 0 {
            node.md5 = Some(self.byte_array()?);
        }
        if fl & flags::HEADER_FILE_TYPE != 0 {
            node.header_file_type = Some(HeaderFileType(self.u8()?)); // :665
        }
        if fl & flags::ALT_SIZE != 0 {
            node.alt_size = Some(self.u64()?);
        }
        if fl & flags::ALT_CRC != 0 {
            node.alt_crc = Some(self.byte_array()?);
        }
        if fl & flags::ALT_SHA1 != 0 {
            node.alt_sha1 = Some(self.byte_array()?);
        }
        if fl & flags::ALT_MD5 != 0 {
            node.alt_md5 = Some(self.byte_array()?);
        }
        if fl & flags::MERGE != 0 {
            node.merge = Some(self.string()?); // :670
        }
        if fl & flags::STATUS != 0 {
            node.status = Some(self.string()?); // :671
        }
        if fl & flags::ZIP_FILE_INDEX != 0 {
            node.zip_file_index = Some(self.i32()?); // :672
        }
        if fl & flags::ZIP_FILE_HEADER != 0 {
            node.zip_file_header_position = Some(self.u64()?); // :673
        }
        if fl & flags::CHD_VERSION != 0 {
            node.chd_version = Some(self.u32()?); // :674 (ReadInt32 cast to uint)
        }
        if fl & flags::TO_SORT_STATUS != 0 {
            node.to_sort_type = Some(ToSortDirType(self.u8()?)); // :676
        }
        if fl & flags::MIA_STATUS != 0 {
            node.mia_status = Some(MiaStatus(self.u8()?)); // :678-679
        }
        node.file_status = FileStatus(self.u32()?); // :681
        Ok(node)
    }
}

/// Parse a whole cache file: version, root node, end marker, nothing after.
pub(crate) fn parse(buf: &[u8]) -> Result<CacheFile, CacheError> {
    let mut r = Reader { buf, pos: 0 };
    let version = r.i32()?; // DB.cs:146
    if version != 2 && version != 3 {
        return Err(CacheError::UnsupportedVersion(version));
    }
    let root = r.node(version, true, 0)?;
    let marker = r.u64()?; // DB.cs:182-186
    if marker != END_MARKER {
        return Err(CacheError::BadEndMarker { found: marker });
    }
    if r.remaining() != 0 {
        return Err(CacheError::TrailingData { offset: r.pos });
    }
    Ok(CacheFile { version, root })
}
