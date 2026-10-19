//! The TorrentZip writer: byte-exact local headers, deflate data, central directory, zip64 and
//! the `TORRENTZIPPED-XXXXXXXX` comment (`ZipFileData.cs:309-393,547-634`,
//! `ZipOpenWrite.cs:64-96`, `StructuredZip.cs:69-179`).
//!
//! The writer never touches the destination by name: it writes into whatever `Read + Write +
//! Seek` it is given, so the caller decides the temp file and does the fsync and rename
//! ([`crate::AtomicFile`]). Nothing here can delete or replace an existing file.

use std::io::{Read, Seek, SeekFrom, Write};

use super::structure::ZipStructure;
use super::time::ticks_to_dos;
use super::{method, TRRNTZIP_DOS_DATE_TIME};
use super::{
    CENTRAL_DIRECTORY_HEADER_SIGNATURE, END_OF_CENTRAL_DIR_SIGNATURE, LOCAL_FILE_HEADER_SIGNATURE,
    ZIP64_END_OF_CENTRAL_DIR_LOCATOR_SIGNATURE, ZIP64_END_OF_CENTRAL_DIR_SIGNATURE,
};
use crate::cp437;
use crate::deflate::Deflater;
use crate::error::{ArchiveError, Result, StructureError};
use crate::sort::trrntzip_compare_case_units;
use std::cmp::Ordering;

/// What [`TorrentZipWriter::add_file`] reports for one entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddedEntry {
    pub crc32: u32,
    pub uncompressed_size: u64,
    pub compressed_size: u64,
    pub local_header_offset: u64,
}

struct WrittenEntry {
    name: String,
    raw_name: Vec<u8>,
    flags: u16,
    dos_time: u16,
    dos_date: u16,
    crc32: u32,
    compressed_size: u64,
    uncompressed_size: u64,
    local_header_offset: u64,
    is_directory: bool,
}

/// The values `ZipExtraFieldWrite.Zip64` produces (`ZipExtraFieldWrite.cs:17-79`): the extra
/// field bytes, the header values, and whether zip64 is needed.
pub(super) struct Zip64Out {
    pub(super) extra: Vec<u8>,
    pub(super) header_uncompressed: u32,
    pub(super) header_compressed: u32,
    pub(super) header_offset: u32,
    pub(super) is_zip64: bool,
}

pub(super) fn zip64_fields(
    uncompressed: u64,
    compressed: u64,
    offset: u64,
    central: bool,
) -> Zip64Out {
    let mut data = Vec::new();
    let (hu, hc, ho);
    if !central {
        if uncompressed >= 0xffff_ffff || compressed >= 0xffff_ffff {
            data.extend(uncompressed.to_le_bytes());
            data.extend(compressed.to_le_bytes());
            hu = 0xffff_ffff;
            hc = 0xffff_ffff;
        } else {
            hu = uncompressed as u32;
            hc = compressed as u32;
        }
        ho = 0;
    } else {
        if uncompressed >= 0xffff_ffff {
            hu = 0xffff_ffff;
            data.extend(uncompressed.to_le_bytes());
        } else {
            hu = uncompressed as u32;
        }
        if compressed >= 0xffff_ffff {
            hc = 0xffff_ffff;
            data.extend(compressed.to_le_bytes());
        } else {
            hc = compressed as u32;
        }
        if offset >= 0xffff_ffff {
            ho = 0xffff_ffff;
            data.extend(offset.to_le_bytes());
        } else {
            ho = offset as u32;
        }
    }
    if data.is_empty() {
        return Zip64Out {
            extra: Vec::new(),
            header_uncompressed: hu,
            header_compressed: hc,
            header_offset: ho,
            is_zip64: false,
        };
    }
    let mut extra = Vec::with_capacity(4 + data.len());
    extra.extend(1u16.to_le_bytes());
    extra.extend((data.len() as u16).to_le_bytes());
    extra.extend(data);
    Zip64Out {
        extra,
        header_uncompressed: hu,
        header_compressed: hc,
        header_offset: ho,
        is_zip64: true,
    }
}

/// `GetVersionNeededToExtract` (`ZipFileData.cs:167-173`), deflate only.
fn version_needed(is_zip64: bool) -> u16 {
    if is_zip64 {
        45
    } else {
        20
    }
}

/// One central directory record (`ZipFileData.CentralDirectoryWrite`, `ZipFileData.cs:175-218`).
fn central_record(e: &WrittenEntry, out: &mut Vec<u8>) {
    let z = zip64_fields(
        e.uncompressed_size,
        e.compressed_size,
        e.local_header_offset,
        true,
    );
    out.extend(CENTRAL_DIRECTORY_HEADER_SIGNATURE.to_le_bytes());
    out.extend(0u16.to_le_bytes()); // version made by
    out.extend(version_needed(z.is_zip64).to_le_bytes());
    out.extend(e.flags.to_le_bytes());
    out.extend(method::DEFLATED.to_le_bytes());
    out.extend(e.dos_time.to_le_bytes());
    out.extend(e.dos_date.to_le_bytes());
    out.extend(e.crc32.to_le_bytes());
    out.extend(z.header_compressed.to_le_bytes());
    out.extend(z.header_uncompressed.to_le_bytes());
    out.extend((e.raw_name.len() as u16).to_le_bytes());
    out.extend((z.extra.len() as u16).to_le_bytes());
    out.extend(0u16.to_le_bytes()); // file comment length
    out.extend(0u16.to_le_bytes()); // disk number start
    out.extend(0u16.to_le_bytes()); // internal attributes
    out.extend(0u32.to_le_bytes()); // external attributes
    out.extend(z.header_offset.to_le_bytes());
    out.extend(&e.raw_name);
    out.extend(z.extra);
}

/// A TorrentZip (or TDC/DTD) writer over a seekable stream.
pub struct TorrentZipWriter<W: Read + Write + Seek> {
    w: W,
    structure: ZipStructure,
    entries: Vec<WrittenEntry>,
}

impl<W: Read + Write + Seek> TorrentZipWriter<W> {
    /// Start a zip of the given structure. The deflate-based structures are supported (TorrentZip,
    /// TDC and DTD); the zstd and 7z ones are not written by this crate yet.
    pub fn new(w: W, structure: ZipStructure) -> Result<TorrentZipWriter<W>> {
        match structure {
            ZipStructure::ZipTrrnt | ZipStructure::ZipTDC | ZipStructure::ZipDTD => {}
            _ => {
                return Err(StructureError::UnsupportedStructure(structure.name()).into());
            }
        }
        Ok(TorrentZipWriter {
            w,
            structure,
            entries: Vec::new(),
        })
    }

    /// `StructuredZip.ZipFileOpenWriteStream`'s checks (`StructuredZip.cs:135-179`).
    fn check_name(&self, name: &str, size: u64) -> Result<()> {
        if name.contains('\\') {
            return Err(StructureError::BackslashInName(name.to_string()).into());
        }
        if name.ends_with('/') && size != 0 {
            return Err(StructureError::DirectoryWithData(name.to_string()).into());
        }
        if let Some(last) = self.entries.last() {
            let (l, n): (Vec<u16>, Vec<u16>) = (
                last.name.encode_utf16().collect(),
                name.encode_utf16().collect(),
            );
            if trrntzip_compare_case_units(&l, &n) == Ordering::Greater {
                return Err(StructureError::OutOfOrder {
                    previous: last.name.clone(),
                    name: name.to_string(),
                }
                .into());
            }
            if self.structure == ZipStructure::ZipTrrnt
                && last.is_directory
                && n.len() > l.len()
                && trrntzip_compare_case_units(&l, &n[..l.len()]) == Ordering::Equal
            {
                return Err(StructureError::DirectoryNotEmpty {
                    directory: last.name.clone(),
                    name: name.to_string(),
                }
                .into());
            }
        }
        Ok(())
    }

    /// Compress `data` (exactly `uncompressed_size` bytes) into a new entry. `mod_time` is the
    /// entry's timestamp for TDC and DTD (packed DOS or .NET ticks, see
    /// [`ticks_to_dos`]); TorrentZip entries always get 1996-12-24 23:32:00. If `expected_crc` is
    /// given it must match the data.
    pub fn add_file<Rd: Read>(
        &mut self,
        name: &str,
        uncompressed_size: u64,
        data: &mut Rd,
        mod_time: Option<i64>,
        expected_crc: Option<u32>,
    ) -> Result<AddedEntry> {
        self.check_name(name, uncompressed_size)?;
        let mod_time = if self.structure == ZipStructure::ZipTrrnt {
            TRRNTZIP_DOS_DATE_TIME
        } else {
            mod_time.unwrap_or(0)
        };
        let (dos_date, dos_time) = ticks_to_dos(mod_time);
        let (raw_name, flags) = match cp437::encode(name) {
            Some(b) => (b, 2u16),
            None => (name.as_bytes().to_vec(), 2u16 | (1 << 11)),
        };
        if raw_name.len() > u16::MAX as usize {
            return Err(StructureError::TooLarge("file name").into());
        }

        // Local header with placeholder CRC and sizes (`LocalFileHeaderWrite`).
        let header_offset = self.w.stream_position()?;
        let pre = zip64_fields(uncompressed_size, 0, header_offset, false);
        self.w
            .write_all(&LOCAL_FILE_HEADER_SIGNATURE.to_le_bytes())?;
        self.w
            .write_all(&version_needed(pre.is_zip64).to_le_bytes())?;
        self.w.write_all(&flags.to_le_bytes())?;
        self.w.write_all(&method::DEFLATED.to_le_bytes())?;
        self.w.write_all(&dos_time.to_le_bytes())?;
        self.w.write_all(&dos_date.to_le_bytes())?;
        self.w.write_all(&[0xff; 12])?;
        self.w.write_all(&(raw_name.len() as u16).to_le_bytes())?;
        self.w.write_all(&(pre.extra.len() as u16).to_le_bytes())?;
        self.w.write_all(&raw_name)?;
        let extra_location = self.w.stream_position()?;
        self.w.write_all(&pre.extra)?;
        let data_location = self.w.stream_position()?;

        // Data. An empty file is the two bytes `03 00` and the deflater is never run
        // (`LocalFileAddZeroLengthFile`, `ZipFileData.cs:630-634`).
        let mut hasher = crc32fast::Hasher::new();
        let mut total = 0u64;
        let mut buf = vec![0u8; 1 << 20];
        let mut deflater = Deflater::new();
        let mut out = Vec::new();
        let mut limited = data.take(uncompressed_size.saturating_add(1));
        loop {
            let n = limited.read(&mut buf)?;
            if n == 0 {
                break;
            }
            total += n as u64;
            if total > uncompressed_size {
                break;
            }
            hasher.update(&buf[..n]);
            deflater.write(&buf[..n], &mut out);
            self.w.write_all(&out)?;
            out.clear();
        }
        if total != uncompressed_size {
            return Err(StructureError::DataMismatch(format!(
                "{name}: the data is {}{} bytes, {uncompressed_size} were declared",
                if total > uncompressed_size {
                    "more than "
                } else {
                    ""
                },
                total.min(uncompressed_size),
            ))
            .into());
        }
        if total > 0 {
            deflater.finish(&mut out);
            self.w.write_all(&out)?;
        }
        let crc32 = hasher.finalize();
        if let Some(want) = expected_crc {
            if want != crc32 {
                return Err(StructureError::DataMismatch(format!(
                    "{name}: CRC32 is {crc32:08X}, expected {want:08X}"
                ))
                .into());
            }
        }
        let mut end = self.w.stream_position()?;
        let mut compressed_size = end - data_location;
        if compressed_size == 0 && uncompressed_size == 0 {
            self.w.write_all(&[0x03, 0x00])?;
            end += 2;
            compressed_size = 2;
        }

        // Back-patch CRC and sizes (`LocalFileHeaderPostWrite`, `ZipFileData.cs:354-393`).
        let post = zip64_fields(uncompressed_size, compressed_size, header_offset, false);
        let mut end_now = end;
        if post.is_zip64 != pre.is_zip64 {
            // The compressed size crossed 4 GiB although the uncompressed size did not: make room
            // for the zip64 extra field by moving the data up (`FixFileForZip64`, `:595-628`).
            self.w.seek(SeekFrom::Start(header_offset + 4))?;
            self.w.write_all(&45u16.to_le_bytes())?;
            self.w.seek(SeekFrom::Start(header_offset + 28))?;
            self.w.write_all(&(post.extra.len() as u16).to_le_bytes())?;
            let expand_by = (post.extra.len() - pre.extra.len()) as u64;
            expand_file(&mut self.w, extra_location, end, expand_by)?;
            end_now = end + expand_by;
        }
        self.w.seek(SeekFrom::Start(header_offset + 14))?;
        self.w.write_all(&crc32.to_le_bytes())?;
        self.w.write_all(&post.header_compressed.to_le_bytes())?;
        self.w.write_all(&post.header_uncompressed.to_le_bytes())?;
        if !post.extra.is_empty() {
            self.w.seek(SeekFrom::Start(extra_location))?;
            self.w.write_all(&post.extra)?;
        }
        self.w.seek(SeekFrom::Start(end_now))?;

        self.entries.push(WrittenEntry {
            name: name.to_string(),
            raw_name,
            flags,
            dos_time,
            dos_date,
            crc32,
            compressed_size,
            uncompressed_size,
            local_header_offset: header_offset,
            is_directory: name.ends_with('/'),
        });
        Ok(AddedEntry {
            crc32,
            uncompressed_size,
            compressed_size,
            local_header_offset: header_offset,
        })
    }

    /// [`add_file`](Self::add_file) for data in memory.
    pub fn add_bytes(
        &mut self,
        name: &str,
        data: &[u8],
        mod_time: Option<i64>,
    ) -> Result<AddedEntry> {
        let mut r = data;
        self.add_file(name, data.len() as u64, &mut r, mod_time, None)
    }

    /// Write the central directory, the comment and the end records, and hand the stream back.
    /// The stream is left positioned after the last byte and is not truncated or synced: the
    /// caller owns that (see [`crate::AtomicFile`]).
    pub fn finish(mut self) -> Result<W> {
        let cd_start = self.w.stream_position()?;
        let mut cd = Vec::new();
        for e in &self.entries {
            central_record(e, &mut cd);
        }
        let cd_crc = crc32fast::hash(&cd);
        self.w.write_all(&cd)?;
        let cd_size = cd.len() as u64;
        let count = self.entries.len() as u64;
        let comment = self.structure.comment(cd_crc);

        // `EndOfCentralDirectoryWrite` (`ZipOpenWrite.cs:81-96`).
        if cd_start >= 0xffff_ffff || cd_size >= 0xffff_ffff || count >= 0xffff {
            let rec_pos = self.w.stream_position()?;
            let mut z = Vec::with_capacity(56 + 20);
            z.extend(ZIP64_END_OF_CENTRAL_DIR_SIGNATURE.to_le_bytes());
            z.extend(44u64.to_le_bytes());
            z.extend(45u16.to_le_bytes()); // version made by
            z.extend(45u16.to_le_bytes()); // version needed
            z.extend(0u32.to_le_bytes());
            z.extend(0u32.to_le_bytes());
            z.extend(count.to_le_bytes());
            z.extend(count.to_le_bytes());
            z.extend(cd_size.to_le_bytes());
            z.extend(cd_start.to_le_bytes());
            z.extend(ZIP64_END_OF_CENTRAL_DIR_LOCATOR_SIGNATURE.to_le_bytes());
            z.extend(0u32.to_le_bytes());
            z.extend(rec_pos.to_le_bytes());
            z.extend(1u32.to_le_bytes());
            self.w.write_all(&z)?;
        }
        // The comment is ASCII for every structure written here.
        let comment = cp437::encode(&comment).ok_or(ArchiveError::Unsupported {
            what: "a zip comment that is not code page 437",
        })?;
        let mut e = Vec::with_capacity(22 + comment.len());
        e.extend(END_OF_CENTRAL_DIR_SIGNATURE.to_le_bytes());
        e.extend(0u16.to_le_bytes());
        e.extend(0u16.to_le_bytes());
        let c16 = if count >= 0xffff {
            0xffff
        } else {
            count as u16
        };
        e.extend(c16.to_le_bytes());
        e.extend(c16.to_le_bytes());
        e.extend(
            (if cd_size >= 0xffff_ffff {
                0xffff_ffff
            } else {
                cd_size as u32
            })
            .to_le_bytes(),
        );
        e.extend(
            (if cd_start >= 0xffff_ffff {
                0xffff_ffff
            } else {
                cd_start as u32
            })
            .to_le_bytes(),
        );
        e.extend((comment.len() as u16).to_le_bytes());
        e.extend(comment);
        self.w.write_all(&e)?;
        self.w.flush()?;
        Ok(self.w)
    }
}

/// `ExpandFile` (`ZipFileData.cs:613-628`): move `[offset, length)` up by `extra_bytes`, from the
/// end so the ranges may overlap.
pub(super) fn expand_file<W: Read + Write + Seek>(
    w: &mut W,
    offset: u64,
    length: u64,
    extra_bytes: u64,
) -> Result<()> {
    const BUF: u64 = 40960;
    let mut buffer = vec![0u8; BUF as usize];
    let mut pos = length;
    while pos > offset {
        let to_read = if pos >= BUF + offset {
            BUF
        } else {
            pos - offset
        };
        pos -= to_read;
        w.seek(SeekFrom::Start(pos))?;
        w.read_exact(&mut buffer[..to_read as usize])?;
        w.seek(SeekFrom::Start(pos + extra_bytes))?;
        w.write_all(&buffer[..to_read as usize])?;
    }
    Ok(())
}

/// A central directory record for a bare name and numbers, for the tests of the reader.
#[cfg(test)]
pub(super) fn central_record_for_test(
    name: &[u8],
    crc32: u32,
    compressed_size: u64,
    uncompressed_size: u64,
    local_header_offset: u64,
    out: &mut Vec<u8>,
) {
    central_record(
        &WrittenEntry {
            name: String::from_utf8_lossy(name).into_owned(),
            raw_name: name.to_vec(),
            flags: 2,
            dos_time: 0xBC00,
            dos_date: 0x2198,
            crc32,
            compressed_size,
            uncompressed_size,
            local_header_offset,
            is_directory: false,
        },
        out,
    );
}
