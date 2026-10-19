//! Extra-field parsing (`ZipExtraFieldRead.cs:9-610`, `ExtraFieldRead`).

use super::time::{ticks_from_ntfs, ticks_from_unix};
use crate::error::{ArchiveError, Result};

/// What the extra fields of one header changed in the entry being read.
#[derive(Default)]
pub(crate) struct ExtraFields {
    pub is_zip64: bool,
    /// Set by every field except zip64 (`ZipExtraFieldRead.cs`, each `case` but `0x0001`).
    pub extra_data_found: bool,
    pub modified: Option<i64>,
    pub accessed: Option<i64>,
    pub created: Option<i64>,
    /// The Info-ZIP unicode path (`0x7075`), if its CRC matched the raw name.
    pub unicode_name: Option<String>,
}

/// The size/offset fields a zip64 extra field may replace.
pub(crate) struct Zip64Fields<'a> {
    pub uncompressed_size: &'a mut u64,
    pub compressed_size: &'a mut u64,
    pub local_offset: &'a mut u64,
}

struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Cursor<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8]> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&e| e <= self.data.len())
            .ok_or_else(|| ArchiveError::corrupt("extra field", "a block runs past the field"))?;
        let s = &self.data[self.pos..end];
        self.pos = end;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn i16(&mut self) -> Result<i16> {
        Ok(i16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn i64(&mut self) -> Result<i64> {
        Ok(i64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
}

/// `ExtraFieldRead`. `central` selects the central-directory variant (zip64 may replace the
/// local-header offset there, `ZipExtraFieldRead.cs:69-89`).
pub(crate) fn read_extra_fields(
    extra: &[u8],
    central: bool,
    raw_name: &[u8],
    sizes: Zip64Fields<'_>,
) -> Result<ExtraFields> {
    let mut out = ExtraFields::default();
    let mut block_pos = 0usize;
    // The +4 tolerates extra data that ends in a partial header (`ZipExtraFieldRead.cs:23`).
    while block_pos + 4 <= extra.len() {
        let ty = u16::from_le_bytes([extra[block_pos], extra[block_pos + 1]]);
        let block_len = u16::from_le_bytes([extra[block_pos + 2], extra[block_pos + 3]]) as usize;
        block_pos += 4;
        let mut c = Cursor {
            data: extra,
            pos: block_pos,
        };
        match ty {
            0x0001 => {
                out.is_zip64 = true;
                if *sizes.uncompressed_size == 0xffff_ffff {
                    *sizes.uncompressed_size = c.u64()?;
                }
                if *sizes.compressed_size == 0xffff_ffff {
                    *sizes.compressed_size = c.u64()?;
                }
                if central && *sizes.local_offset == 0xffff_ffff {
                    *sizes.local_offset = c.u64()?;
                }
            }
            0x000a => {
                out.extra_data_found = true;
                c.take(4)?; // reserved
                let tag1 = c.i16()?;
                let size1 = c.i16()?;
                if tag1 == 0x0001 && size1 == 24 {
                    out.modified = Some(ticks_from_ntfs(c.i64()?));
                    out.accessed = Some(ticks_from_ntfs(c.i64()?));
                    out.created = Some(ticks_from_ntfs(c.i64()?));
                }
            }
            0x5455 => {
                out.extra_data_found = true;
                let flags = c.u8()?;
                if flags & 1 != 0 {
                    out.modified = Some(ticks_from_unix(c.i32()?));
                }
                if !central {
                    if flags & 2 != 0 {
                        out.accessed = Some(ticks_from_unix(c.i32()?));
                    }
                    if flags & 4 != 0 {
                        out.created = Some(ticks_from_unix(c.i32()?));
                    }
                }
            }
            0x5855 => {
                out.extra_data_found = true;
                out.accessed = Some(ticks_from_unix(c.i32()?));
                out.modified = Some(ticks_from_unix(c.i32()?));
            }
            0x7075 => {
                out.extra_data_found = true;
                c.u8()?; // version
                let name_crc = c.u32()?;
                if name_crc == crc32fast::hash(raw_name) {
                    let len = block_len.saturating_sub(5);
                    out.unicode_name = Some(String::from_utf8_lossy(c.take(len)?).into_owned());
                }
            }
            // 0x0007, 0x4453, 0x4B46, 0x7855, 0x7875, 0xE57A and every unknown tag: noted only.
            _ => out.extra_data_found = true,
        }
        block_pos += block_len;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(extra: &[u8], central: bool) -> (ExtraFields, u64, u64, u64) {
        let (mut u, mut cz, mut off) = (0xffff_ffffu64, 0xffff_ffffu64, 0xffff_ffffu64);
        let f = read_extra_fields(
            extra,
            central,
            b"name",
            Zip64Fields {
                uncompressed_size: &mut u,
                compressed_size: &mut cz,
                local_offset: &mut off,
            },
        )
        .unwrap();
        (f, u, cz, off)
    }

    #[test]
    fn zip64_replaces_placeholders_in_order() {
        let mut e = vec![1, 0, 24, 0];
        e.extend(5_000_000_000u64.to_le_bytes());
        e.extend(4_900_000_000u64.to_le_bytes());
        e.extend(6_000_000_000u64.to_le_bytes());
        let (f, u, c, o) = run(&e, true);
        assert!(f.is_zip64 && !f.extra_data_found);
        assert_eq!((u, c, o), (5_000_000_000, 4_900_000_000, 6_000_000_000));
        // The local variant has no offset.
        let (_, _, _, o) = run(&e[..20], false);
        assert_eq!(o, 0xffff_ffff);
    }

    #[test]
    fn other_fields_set_extra_data_found() {
        let (f, ..) = run(&[0x34, 0x12, 0, 0], false);
        assert!(f.extra_data_found);
        // A trailing partial header is ignored.
        let (f, ..) = run(&[1, 2, 3], false);
        assert!(!f.extra_data_found);
    }

    #[test]
    fn unicode_path_needs_a_matching_crc() {
        let mut e = vec![0x75, 0x70, 7, 0, 1];
        e.extend(crc32fast::hash(b"name").to_le_bytes());
        e.extend(b"ab");
        let (f, ..) = run(&e, true);
        assert_eq!(f.unicode_name.as_deref(), Some("ab"));
        e[5] ^= 1;
        let (f, ..) = run(&e, true);
        assert_eq!(f.unicode_name, None);
    }

    #[test]
    fn short_block_is_an_error_not_a_panic() {
        let (mut u, mut c, mut o) = (0xffff_ffffu64, 0, 0);
        let r = read_extra_fields(
            &[1, 0, 8, 0, 1, 2],
            true,
            b"",
            Zip64Fields {
                uncompressed_size: &mut u,
                compressed_size: &mut c,
                local_offset: &mut o,
            },
        );
        assert!(r.is_err());
    }
}
