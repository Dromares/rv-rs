//! Zip reading: end of central directory (with zip64), central directory, local headers, and
//! streaming entry data with a CRC check (`ZipOpenRead.cs`, `ZipEndOfCentralDir.cs`,
//! `ZipFileData.cs`).

use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::ops::BitOr;

use super::codec::{io_to_decode, method_name, open_decoder, romvault_reads_method};
use super::extra::{read_extra_fields, Zip64Fields};
use super::structure::{
    validate_directories, validate_file_headers, validate_file_order, ZipStructure, TRRNTZERO,
    ZSTD_END,
};
use super::time::combine_dos;
use super::{
    CENTRAL_DIRECTORY_HEADER_SIGNATURE, END_OF_CENTRAL_DIR_SIGNATURE, LOCAL_FILE_HEADER_SIGNATURE,
    ZIP64_END_OF_CENTRAL_DIR_LOCATOR_SIGNATURE, ZIP64_END_OF_CENTRAL_DIR_SIGNATURE,
};
use crate::cp437;
use crate::error::{ArchiveError, Result};

/// Soft findings from comparing a central header with its local header (`LocalFileStatus`,
/// `ZipFileData.cs:15-24`; the values match the C# flags).
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct LocalFileStatus(u32);

impl LocalFileStatus {
    pub const NOTHING: LocalFileStatus = LocalFileStatus(0);
    /// The general purpose flag words differ.
    pub const HEADERS_MISMATCH: LocalFileStatus = LocalFileStatus(0x04);
    /// The names differ, ignoring `/` against `\` and case.
    pub const FILENAME_MISMATCH: LocalFileStatus = LocalFileStatus(0x10);
    /// A directory entry with a non-zero size.
    pub const DIRECTORY_LENGTH_ERROR: LocalFileStatus = LocalFileStatus(0x20);
    /// The DOS date or time differs.
    pub const DATE_TIME_MISMATCH: LocalFileStatus = LocalFileStatus(0x40);

    pub fn bits(self) -> u32 {
        self.0
    }
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
    pub fn contains(self, other: LocalFileStatus) -> bool {
        self.0 & other.0 == other.0
    }
    pub fn intersects(self, other: LocalFileStatus) -> bool {
        self.0 & other.0 != 0
    }
}

impl BitOr for LocalFileStatus {
    type Output = LocalFileStatus;
    fn bitor(self, rhs: LocalFileStatus) -> LocalFileStatus {
        LocalFileStatus(self.0 | rhs.0)
    }
}

/// One central directory entry, with what the local header added.
#[derive(Clone, Debug)]
pub struct ZipEntry {
    /// The name, decoded as CP437 or UTF-8 by general purpose bit 11.
    pub name: String,
    /// The name bytes as stored.
    pub raw_name: Vec<u8>,
    pub is_directory: bool,
    pub method: u16,
    pub flags: u16,
    pub version_made_by: u16,
    pub version_needed: u16,
    pub crc32: u32,
    pub compressed_size: u64,
    pub uncompressed_size: u64,
    /// `(date << 16) | time`, sign-extended like the C# `int` arithmetic (`CompressUtils.cs:183`).
    pub dos_date_time: i64,
    /// Times from the NTFS, `UT` or `UX` extra fields, in .NET ticks.
    pub modified_ticks: Option<i64>,
    pub accessed_ticks: Option<i64>,
    pub created_ticks: Option<i64>,
    /// Offset of the local header in the file (the prefix offset already added).
    pub local_header_offset: u64,
    /// Offset of the first data byte.
    pub data_offset: u64,
    pub is_zip64: bool,
    /// Any extra field other than zip64 was present (`ExtraDataFound`).
    pub extra_data_found: bool,
    pub status: LocalFileStatus,
}

impl ZipEntry {
    /// `FileHeader.LastModified` (`FileHeader.cs:14`): the extra-field time, else the DOS time.
    pub fn last_modified(&self) -> i64 {
        self.modified_ticks.unwrap_or(self.dos_date_time)
    }

    /// The local header offset, only when the entry has no data descriptor (`ZipFileData.cs:75`).
    pub fn local_head(&self) -> Option<u64> {
        (self.flags & 8 == 0).then_some(self.local_header_offset)
    }
}

/// An open zip.
pub struct ZipArchive<R> {
    reader: R,
    entries: Vec<ZipEntry>,
    comment: Vec<u8>,
    is_zip64: bool,
    prefix_offset: u64,
    extra_data_on_end_of_file: bool,
    central_dir_start: u64,
    central_dir_size: u64,
    structure: ZipStructure,
    header_structure: ZipStructure,
}

struct Eocd {
    count: u64,
    cd_size: u64,
    cd_start: u64,
    comment: Vec<u8>,
    extra_data_on_end: bool,
}

fn rd_u16(r: &mut impl Read) -> io::Result<u16> {
    let mut b = [0; 2];
    r.read_exact(&mut b)?;
    Ok(u16::from_le_bytes(b))
}
fn rd_u32(r: &mut impl Read) -> io::Result<u32> {
    let mut b = [0; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}
fn rd_u64(r: &mut impl Read) -> io::Result<u64> {
    let mut b = [0; 8];
    r.read_exact(&mut b)?;
    Ok(u64::from_le_bytes(b))
}
fn rd_vec(r: &mut impl Read, n: usize) -> io::Result<Vec<u8>> {
    let mut v = vec![0; n];
    r.read_exact(&mut v)?;
    Ok(v)
}

fn truncated(what: &'static str) -> impl Fn(io::Error) -> ArchiveError {
    move |e| {
        if e.kind() == io::ErrorKind::UnexpectedEof {
            ArchiveError::Truncated { what }
        } else {
            ArchiveError::Io(e)
        }
    }
}

/// `FindEndOfCentralDirSignature` (`ZipEndOfCentralDir.cs:12-54`): the last `PK\5\6` in the final
/// `0xffff` bytes (or the whole file if it is shorter).
fn find_eocd<R: Read + Seek>(r: &mut R, file_len: u64) -> Result<u64> {
    let window = file_len.min(0xffff);
    let start = file_len - window;
    r.seek(SeekFrom::Start(start))?;
    let mut buf = vec![0; window as usize];
    r.read_exact(&mut buf)?;
    let sig = END_OF_CENTRAL_DIR_SIGNATURE.to_le_bytes();
    (0..buf.len().saturating_sub(3))
        .rev()
        .find(|&i| buf[i..i + 4] == sig)
        .map(|i| start + i as u64)
        .ok_or(ArchiveError::BadSignature {
            what: "end of central directory",
        })
}

/// `EndOfCentralDirRead` (`ZipEndOfCentralDir.cs:57-97`), positioned at the signature.
fn read_eocd<R: Read + Seek>(r: &mut R, file_len: u64) -> Result<Eocd> {
    let t = truncated("end of central directory");
    if rd_u32(r).map_err(&t)? != END_OF_CENTRAL_DIR_SIGNATURE {
        return Err(ArchiveError::BadSignature {
            what: "end of central directory",
        });
    }
    let bad = |detail: &str| ArchiveError::corrupt("end of central directory", detail);
    if rd_u16(r).map_err(&t)? != 0 || rd_u16(r).map_err(&t)? != 0 {
        return Err(bad("multi-disk archives are not supported"));
    }
    let count = rd_u16(r).map_err(&t)?;
    if rd_u16(r).map_err(&t)? != count {
        return Err(bad("entry counts differ"));
    }
    let cd_size = rd_u32(r).map_err(&t)?;
    let cd_start = rd_u32(r).map_err(&t)?;
    let comment_len = rd_u16(r).map_err(&t)?;
    // ROMVault's `ReadBytes` takes whatever is there, so a zip cut inside its comment still opens
    // (`ZipEndOfCentralDir.cs:91`). That is a damaged file, so here it is an error.
    let comment = rd_vec(r, comment_len as usize).map_err(&t)?;
    let pos = r.stream_position()?;
    Ok(Eocd {
        count: count as u64,
        cd_size: cd_size as u64,
        cd_start: cd_start as u64,
        comment,
        extra_data_on_end: pos != file_len,
    })
}

/// `Zip64EndOfCentralDirectoryLocatorRead` (`ZipEndOfCentralDir.cs:185-209`): the absolute offset
/// of the zip64 record.
fn read_zip64_locator<R: Read>(r: &mut R) -> Option<u64> {
    if rd_u32(r).ok()? != ZIP64_END_OF_CENTRAL_DIR_LOCATOR_SIGNATURE || rd_u32(r).ok()? != 0 {
        return None;
    }
    let off = rd_u64(r).ok()?;
    (rd_u32(r).ok()? <= 1).then_some(off)
}

/// `Zip64EndOfCentralDirRead` (`ZipEndOfCentralDir.cs:116-165`): `(count, cd_size, cd_start)`.
fn read_zip64_eocd<R: Read>(r: &mut R) -> Option<(u64, u64, u64)> {
    if rd_u32(r).ok()? != ZIP64_END_OF_CENTRAL_DIR_SIGNATURE || rd_u64(r).ok()? != 44 {
        return None;
    }
    rd_u16(r).ok()?; // version made by
    if rd_u16(r).ok()? != 45 || rd_u32(r).ok()? != 0 || rd_u32(r).ok()? != 0 {
        return None;
    }
    let count = rd_u64(r).ok()? as u32 as u64;
    if rd_u64(r).ok()? != count {
        return None;
    }
    Some((count, rd_u64(r).ok()?, rd_u64(r).ok()?))
}

/// A header's name: UTF-8 by bit 11, else CP437 (`ZipFileData.cs:130-132`).
fn decode_name(raw: &[u8], flags: u16) -> String {
    if flags & (1 << 11) == 0 {
        cp437::decode(raw)
    } else {
        String::from_utf8_lossy(raw).into_owned()
    }
}

/// `CompareStringSlashToLower` (`CompressUtils.cs:37-52`).
fn names_match(a: &str, b: &str) -> bool {
    let norm = |s: &str| -> Vec<char> {
        s.chars()
            .map(|c| {
                let c = if c == '/' { '\\' } else { c };
                let mut l = c.to_lowercase();
                match (l.next(), l.next()) {
                    (Some(x), None) => x,
                    _ => c,
                }
            })
            .collect()
    };
    norm(a) == norm(b)
}

/// The fixed part of a header plus its variable parts, as read from the file.
struct RawHeader {
    flags: u16,
    method: u16,
    dos_date_time: i64,
    crc32: u32,
    compressed_size: u64,
    uncompressed_size: u64,
    name: String,
    raw_name: Vec<u8>,
    version_made_by: u16,
    version_needed: u16,
    local_offset: u64,
    extra: super::extra::ExtraFields,
    /// Where the header ended (local headers only: the first data byte).
    end: u64,
}

/// `ZipFileData.CentralDirectoryRead` (`ZipFileData.cs:92-165`).
fn read_central_header<R: Read>(r: &mut R, prefix: u64) -> Result<RawHeader> {
    let t = truncated("central directory");
    if rd_u32(r).map_err(&t)? != CENTRAL_DIRECTORY_HEADER_SIGNATURE {
        return Err(ArchiveError::BadSignature {
            what: "central directory entry",
        });
    }
    let version_made_by = rd_u16(r).map_err(&t)?;
    let version_needed = rd_u16(r).map_err(&t)?;
    let flags = rd_u16(r).map_err(&t)?;
    let method = rd_u16(r).map_err(&t)?;
    let time = rd_u16(r).map_err(&t)?;
    let date = rd_u16(r).map_err(&t)?;
    let crc32 = rd_u32(r).map_err(&t)?;
    let mut compressed_size = rd_u32(r).map_err(&t)? as u64;
    let mut uncompressed_size = rd_u32(r).map_err(&t)? as u64;
    let name_len = rd_u16(r).map_err(&t)? as usize;
    let extra_len = rd_u16(r).map_err(&t)? as usize;
    let comment_len = rd_u16(r).map_err(&t)? as usize;
    rd_u16(r).map_err(&t)?; // disk number start
    rd_u16(r).map_err(&t)?; // internal attributes
    rd_u32(r).map_err(&t)?; // external attributes
    let mut local_offset = rd_u32(r).map_err(&t)? as u64;
    let raw_name = rd_vec(r, name_len).map_err(&t)?;
    let mut name = decode_name(&raw_name, flags);
    let extra_bytes = rd_vec(r, extra_len).map_err(&t)?;
    let extra = read_extra_fields(
        &extra_bytes,
        true,
        &raw_name,
        Zip64Fields {
            uncompressed_size: &mut uncompressed_size,
            compressed_size: &mut compressed_size,
            local_offset: &mut local_offset,
        },
    )?;
    if let Some(n) = &extra.unicode_name {
        name = n.clone();
    }
    let local_offset = local_offset.checked_add(prefix).ok_or_else(|| {
        ArchiveError::corrupt("central directory", "local header offset overflows")
    })?;
    rd_vec(r, comment_len).map_err(&t)?; // file comment, unused
    Ok(RawHeader {
        flags,
        method,
        dos_date_time: combine_dos(date, time),
        crc32,
        compressed_size,
        uncompressed_size,
        name,
        raw_name,
        version_made_by,
        version_needed,
        local_offset,
        extra,
        end: 0,
    })
}

/// `ZipFileData.LocalFileHeaderRead` (`ZipFileData.cs:221-307`). With general purpose bit 3 the
/// CRC and sizes come from the central header; the data descriptor is never read.
fn read_local_header<R: Read + Seek>(r: &mut R, central: &RawHeader) -> Result<RawHeader> {
    r.seek(SeekFrom::Start(central.local_offset))?;
    let t = truncated("local file header");
    if rd_u32(r).map_err(&t)? != LOCAL_FILE_HEADER_SIGNATURE {
        return Err(ArchiveError::BadSignature {
            what: "local file header",
        });
    }
    let version_needed = rd_u16(r).map_err(&t)?;
    let flags = rd_u16(r).map_err(&t)?;
    let method = rd_u16(r).map_err(&t)?;
    let time = rd_u16(r).map_err(&t)?;
    let date = rd_u16(r).map_err(&t)?;
    let mut crc32 = rd_u32(r).map_err(&t)?;
    let mut compressed_size = rd_u32(r).map_err(&t)? as u64;
    let mut uncompressed_size = rd_u32(r).map_err(&t)? as u64;
    let name_len = rd_u16(r).map_err(&t)? as usize;
    let extra_len = rd_u16(r).map_err(&t)? as usize;
    let raw_name = rd_vec(r, name_len).map_err(&t)?;
    let mut name = decode_name(&raw_name, flags);
    let extra_bytes = rd_vec(r, extra_len).map_err(&t)?;
    let mut ignored_offset = 0;
    let extra = read_extra_fields(
        &extra_bytes,
        false,
        &raw_name,
        Zip64Fields {
            uncompressed_size: &mut uncompressed_size,
            compressed_size: &mut compressed_size,
            local_offset: &mut ignored_offset,
        },
    )?;
    if let Some(n) = &extra.unicode_name {
        name = n.clone();
    }
    let end = r.stream_position()?;
    if flags & 8 == 8 {
        crc32 = central.crc32;
        compressed_size = central.compressed_size;
        uncompressed_size = central.uncompressed_size;
    }
    Ok(RawHeader {
        flags,
        method,
        dos_date_time: combine_dos(date, time),
        crc32,
        compressed_size,
        uncompressed_size,
        name,
        raw_name,
        version_made_by: 0,
        version_needed,
        local_offset: central.local_offset,
        extra,
        end,
    })
}

impl<R: Read + Seek> ZipArchive<R> {
    /// Read the headers of the zip in `reader`. Every central entry's local header is read and
    /// compared too, as ROMVault does at open.
    pub fn open(mut reader: R) -> Result<ZipArchive<R>> {
        let file_len = reader.seek(SeekFrom::End(0))?;
        let eocd_pos = find_eocd(&mut reader, file_len)?;
        reader.seek(SeekFrom::Start(eocd_pos))?;
        let eocd = read_eocd(&mut reader, file_len)?;

        let zip64_required =
            eocd.cd_start == 0xffff_ffff || eocd.cd_size == 0xffff_ffff || eocd.count == 0xffff;
        let (mut count, mut cd_size, mut cd_start) = (eocd.count, eocd.cd_size, eocd.cd_start);
        let mut end_of_cd = eocd_pos;
        let mut is_zip64 = false;
        if eocd_pos >= 20 {
            reader.seek(SeekFrom::Start(eocd_pos - 20))?;
            if let Some(rec_pos) = read_zip64_locator(&mut reader) {
                if reader.seek(SeekFrom::Start(rec_pos)).is_ok() {
                    if let Some(z) = read_zip64_eocd(&mut reader) {
                        (count, cd_size, cd_start) = z;
                        end_of_cd = rec_pos;
                        is_zip64 = true;
                    }
                }
            }
        }
        if zip64_required && !is_zip64 {
            return Err(ArchiveError::Zip64Required);
        }

        // Anything in front of the zip (an SFX stub) shifts every offset (`ZipOpenRead.cs:119`).
        let prefix_offset = end_of_cd
            .checked_sub(cd_size)
            .and_then(|v| v.checked_sub(cd_start))
            .ok_or_else(|| {
                ArchiveError::corrupt(
                    "end of central directory",
                    "the central directory is larger than the file",
                )
            })?;
        let central_dir_start = cd_start + prefix_offset;
        if count > file_len / 46 + 1 {
            return Err(ArchiveError::corrupt(
                "end of central directory",
                format!("{count} entries cannot fit in {file_len} bytes"),
            ));
        }

        reader.seek(SeekFrom::Start(central_dir_start))?;
        let mut centrals = Vec::with_capacity(count as usize);
        {
            let mut br = BufReader::with_capacity(1 << 16, &mut reader);
            for _ in 0..count {
                centrals.push(read_central_header(&mut br, prefix_offset)?);
            }
        }
        let mut entries = Vec::with_capacity(centrals.len());
        for c in &centrals {
            let local = read_local_header(&mut reader, c)?;
            entries.push(validate_headers(c, &local)?);
        }

        let mut zip = ZipArchive {
            reader,
            entries,
            comment: eocd.comment,
            is_zip64,
            prefix_offset,
            extra_data_on_end_of_file: eocd.extra_data_on_end,
            central_dir_start,
            central_dir_size: cd_size,
            structure: ZipStructure::None,
            header_structure: ZipStructure::None,
        };
        zip.detect_structure()?;
        if zip.extra_data_on_end_of_file || zip.prefix_offset != 0 {
            zip.structure = ZipStructure::None;
        }
        Ok(zip)
    }

    pub fn entries(&self) -> &[ZipEntry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The archive comment as stored.
    pub fn comment_bytes(&self) -> &[u8] {
        &self.comment
    }

    /// The archive comment decoded as CP437 (`Zip.FileComment`, `Zip.cs:47-50`).
    pub fn comment(&self) -> String {
        cp437::decode(&self.comment)
    }

    pub fn is_zip64(&self) -> bool {
        self.is_zip64
    }

    /// Bytes in front of the zip (a self-extractor stub); non-zero disqualifies every structure.
    pub fn prefix_offset(&self) -> u64 {
        self.prefix_offset
    }

    /// Bytes after the end of central directory record and its comment.
    pub fn has_trailing_data(&self) -> bool {
        self.extra_data_on_end_of_file
    }

    /// The structure the files satisfy: set only when the comment announces it *and* every file
    /// rule holds (`StructuredZip.ZipStruct`, `StructuredZip.cs:216-235`).
    pub fn structure(&self) -> ZipStructure {
        self.structure
    }

    /// The structure the comment announces, whether or not the files satisfy it
    /// (`StructuredZip.HeaderZipStruct`).
    pub fn header_structure(&self) -> ZipStructure {
        self.header_structure
    }

    /// Hex CRC32 of the central directory bytes, 8 upper-case digits (`Zip.GetCRC`,
    /// `ZipOpenRead.cs:260-272`): what a TorrentZip comment ends with.
    pub fn central_directory_crc(&mut self) -> Result<u32> {
        self.reader.seek(SeekFrom::Start(self.central_dir_start))?;
        let mut hasher = crc32fast::Hasher::new();
        let mut left = self.central_dir_size;
        let mut buf = [0u8; 1 << 16];
        while left > 0 {
            let n = left.min(buf.len() as u64) as usize;
            self.reader
                .read_exact(&mut buf[..n])
                .map_err(truncated("central directory"))?;
            hasher.update(&buf[..n]);
            left -= n as u64;
        }
        Ok(hasher.finalize())
    }

    /// `StructuredZip.ValidateStructure` (`StructuredZip.cs:181-195`).
    fn detect_structure(&mut self) -> Result<()> {
        let crc = self.central_directory_crc()?;
        let comment = self.comment();
        for s in ZipStructure::ALL {
            let id = s.comment_id();
            if id.is_empty() || comment != format!("{id}{crc:08X}") {
                continue;
            }
            self.header_structure = s;
            if self.validate_files_structure(s)? {
                self.structure = s;
            }
            return Ok(());
        }
        Ok(())
    }

    /// `validateFilesStructure` (`StructuredZip.cs:216-235`).
    fn validate_files_structure(&mut self, s: ZipStructure) -> Result<bool> {
        Ok(validate_file_headers(s, &self.entries)
            && validate_file_order(&self.entries)
            && validate_directories(s, &self.entries)
            && self.validate_compression_stream(s)?)
    }

    /// `ValidateCompressionStream` (`StructuredZip.cs:326-399`): the shape of the compressed
    /// bytes of empty entries (TorrentZip, TDC) and the zstd trailer (RVZSTD).
    fn validate_compression_stream(&mut self, s: ZipStructure) -> Result<bool> {
        match s {
            ZipStructure::ZipZSTD => {
                for i in 0..self.entries.len() {
                    let e = &self.entries[i];
                    if e.uncompressed_size == 0 {
                        if e.compressed_size != 0 {
                            return Ok(false);
                        }
                        continue;
                    }
                    if e.compressed_size < 3 {
                        return Ok(false);
                    }
                    let (off, len) = (e.data_offset, e.compressed_size);
                    let mut tail = [0u8; 3];
                    self.reader.seek(SeekFrom::Start(off + len - 3))?;
                    self.reader
                        .read_exact(&mut tail)
                        .map_err(truncated("compressed data"))?;
                    if tail != ZSTD_END {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            ZipStructure::ZipTrrnt | ZipStructure::ZipTDC => {
                for i in 0..self.entries.len() {
                    let e = &self.entries[i];
                    if e.uncompressed_size == 0 {
                        if e.compressed_size != 2 {
                            return Ok(false);
                        }
                        let mut b = [0u8; 2];
                        self.reader.seek(SeekFrom::Start(e.data_offset))?;
                        self.reader
                            .read_exact(&mut b)
                            .map_err(truncated("compressed data"))?;
                        if b != TRRNTZERO {
                            return Ok(false);
                        }
                    }
                }
                Ok(true)
            }
            _ => Ok(true),
        }
    }

    /// The entry's compressed bytes as stored (`ZipFileOpenReadStream(raw: true)`).
    pub fn open_raw(&mut self, index: usize) -> Result<impl Read + '_> {
        let e = self
            .entries
            .get(index)
            .ok_or(ArchiveError::NoSuchEntry(index))?;
        let (off, len) = (e.data_offset, e.compressed_size);
        self.reader.seek(SeekFrom::Start(off))?;
        Ok((&mut self.reader).take(len))
    }

    /// The entry's uncompressed bytes. The reader checks the length and CRC32 when the last byte
    /// is read and fails then if they differ from the headers.
    pub fn open_entry(&mut self, index: usize) -> Result<EntryReader<'_>> {
        let e = self
            .entries
            .get(index)
            .ok_or(ArchiveError::NoSuchEntry(index))?
            .clone();
        self.reader.seek(SeekFrom::Start(e.data_offset))?;
        let raw = (&mut self.reader).take(e.compressed_size);
        let inner = open_decoder(e.method, raw, &e.name)?;
        Ok(EntryReader {
            inner,
            name: e.name,
            expected_crc: e.crc32,
            expected_len: e.uncompressed_size,
            produced: 0,
            hasher: crc32fast::Hasher::new(),
            done: false,
        })
    }

    /// Read a whole entry into memory, checked.
    pub fn read_entry(&mut self, index: usize) -> Result<Vec<u8>> {
        let size = self
            .entries
            .get(index)
            .ok_or(ArchiveError::NoSuchEntry(index))?
            .uncompressed_size;
        let mut out = Vec::with_capacity(size.min(1 << 28) as usize);
        let mut r = self.open_entry(index)?;
        r.read_to_end(&mut out).map_err(into_archive_error)?;
        Ok(out)
    }

    /// Stream an entry to `sink`, checking length and CRC32. Returns the byte count.
    pub fn extract_entry(&mut self, index: usize, sink: &mut impl io::Write) -> Result<u64> {
        let mut r = self.open_entry(index)?;
        io::copy(&mut r, sink).map_err(into_archive_error)
    }

    /// Decode an entry and check it, discarding the data.
    pub fn verify_entry(&mut self, index: usize) -> Result<u64> {
        self.extract_entry(index, &mut io::sink())
    }

    /// Give back the underlying reader.
    pub fn into_inner(self) -> R {
        self.reader
    }
}

/// `ValidateFileHeaders` (`ZipOpenRead.cs:172-258`).
fn validate_headers(c: &RawHeader, l: &RawHeader) -> Result<ZipEntry> {
    let bad = |what: &str| {
        ArchiveError::corrupt(
            "local file header",
            format!("{what} differs from the central directory for {:?}", c.name),
        )
    };
    if c.method != l.method {
        return Err(bad("method"));
    }
    if c.crc32 != l.crc32 {
        return Err(bad("CRC"));
    }
    if c.compressed_size != l.compressed_size {
        return Err(bad("compressed size"));
    }
    if c.uncompressed_size != l.uncompressed_size {
        return Err(bad("uncompressed size"));
    }
    let is_directory = c.name.ends_with('/') || c.name.ends_with('\\');
    let mut status = LocalFileStatus::NOTHING;
    if c.flags != l.flags {
        status = status | LocalFileStatus::HEADERS_MISMATCH;
    }
    if c.dos_date_time != l.dos_date_time {
        status = status | LocalFileStatus::DATE_TIME_MISMATCH;
    }
    if !names_match(&c.name, &l.name) {
        status = status | LocalFileStatus::FILENAME_MISMATCH;
    }
    if is_directory && c.uncompressed_size != 0 {
        status = status | LocalFileStatus::DIRECTORY_LENGTH_ERROR;
    }
    if !romvault_reads_method(c.method) {
        return Err(ArchiveError::UnsupportedCompression {
            method: method_name(c.method),
        });
    }
    Ok(ZipEntry {
        name: c.name.clone(),
        raw_name: c.raw_name.clone(),
        is_directory,
        method: c.method,
        flags: c.flags,
        version_made_by: c.version_made_by,
        version_needed: c.version_needed,
        crc32: c.crc32,
        compressed_size: c.compressed_size,
        uncompressed_size: c.uncompressed_size,
        dos_date_time: c.dos_date_time,
        modified_ticks: c.extra.modified,
        accessed_ticks: c.extra.accessed,
        created_ticks: c.extra.created,
        local_header_offset: c.local_offset,
        data_offset: l.end,
        is_zip64: c.extra.is_zip64,
        extra_data_found: c.extra.extra_data_found,
        status,
    })
}

/// A decoded entry stream that checks its own length and CRC32.
pub struct EntryReader<'a> {
    inner: Box<dyn Read + 'a>,
    name: String,
    expected_crc: u32,
    expected_len: u64,
    produced: u64,
    hasher: crc32fast::Hasher,
    done: bool,
}

impl EntryReader<'_> {
    fn fail(e: ArchiveError) -> io::Error {
        io::Error::new(io::ErrorKind::InvalidData, e)
    }
}

impl Read for EntryReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.done || buf.is_empty() {
            return Ok(0);
        }
        let remaining = self.expected_len - self.produced;
        let want = buf.len().min(remaining.min(usize::MAX as u64) as usize);
        let n = if want == 0 {
            0
        } else {
            self.inner
                .read(&mut buf[..want])
                .map_err(|e| Self::fail(io_to_decode(&self.name, e)))?
        };
        if n == 0 && remaining != 0 {
            self.done = true;
            return Err(Self::fail(ArchiveError::SizeMismatch {
                entry: self.name.clone(),
                expected: self.expected_len,
                actual: self.produced,
            }));
        }
        self.hasher.update(&buf[..n]);
        self.produced += n as u64;
        if self.produced == self.expected_len {
            self.done = true;
            let actual = self.hasher.clone().finalize();
            if actual != self.expected_crc {
                return Err(Self::fail(ArchiveError::CrcMismatch {
                    entry: self.name.clone(),
                    expected: self.expected_crc,
                    actual,
                }));
            }
        }
        Ok(n)
    }
}

/// Recover the [`ArchiveError`] an [`EntryReader`] wrapped in an `io::Error`.
pub fn into_archive_error(e: io::Error) -> ArchiveError {
    if e.get_ref().is_some_and(|inner| inner.is::<ArchiveError>()) {
        if let Some(inner) = e.into_inner() {
            if let Ok(a) = inner.downcast::<ArchiveError>() {
                return *a;
            }
        }
        unreachable!("checked above")
    }
    ArchiveError::from(e)
}
