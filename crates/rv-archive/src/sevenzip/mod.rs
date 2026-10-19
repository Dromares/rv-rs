//! 7z: list and extract, with every file's CRC32 checked, and recognise the RomVault footer.
//!
//! Spec: `docs/archives-and-rvio.md` section 3 of the decompile notes (not part of this
//! repository); comments cite RVWorld 3.8.0 `Compress/SevenZip/*` by file and line. Read only:
//! the writer is Phase 5+ (and needs a byte-exact LZMA encoder, `docs/codecs.md` section 4.5).
//!
//! Gaps against ROMVault's reader: BCJ2, ARM64 and AES folders and filters behind a non-LZMA coder
//! are [`ArchiveError::UnsupportedCompression`] (see `decode.rs`), and the legacy
//! `torrent7z_0.9beta` detection (`Structured7Zip.Istorrent7Z`) is not ported.

mod decode;
mod filters;
mod header;

use std::io::{self, Read, Seek, SeekFrom};

pub use header::{Coder, Folder, SubStream};

use header::{parse_header, parse_signature_header, Header, Parsed, StreamsInfo};

use crate::checked::{io_to_decode, CheckedReader};
use crate::error::{ArchiveError, Result};
use crate::sort::{ordinal_compare, trrnt7zip_compare};
use crate::zip::ZipStructure;

/// 100 ns ticks between 0001-01-01 and the FILETIME epoch 1601-01-01.
const FILE_TIME_TO_UTC_TIME: i64 = 504_911_232_000_000_000;

/// The largest header this reader will unpack (the real ones are kilobytes).
const MAX_HEADER_BYTES: u64 = 1 << 30;

/// One file or directory of a 7z (`SevenZipLocalFile`, `SevenZipRead.cs:103-159`).
#[derive(Clone, Debug)]
pub struct SevenZipEntry {
    /// With a trailing `/` for directories.
    pub name: String,
    pub is_directory: bool,
    /// An "anti" item (a deletion marker in update archives).
    pub is_anti: bool,
    pub size: u64,
    /// `None` for entries without data and files whose CRC is not stored.
    pub crc32: Option<u32>,
    /// The folder holding the data, `None` for entries without a stream.
    pub folder: Option<usize>,
    /// Where the data starts in the folder's decoded stream.
    pub offset_in_folder: u64,
    pub modified_ticks: Option<i64>,
    pub accessed_ticks: Option<i64>,
    pub created_ticks: Option<i64>,
    pub attributes: Option<u32>,
}

/// A summary of one folder, for listings and tests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderInfo {
    /// Coder names in header order, e.g. `["LZMA2"]` or `["BCJ", "LZMA"]`.
    pub coders: Vec<String>,
    pub packed_size: u64,
    pub unpacked_size: u64,
    pub files: usize,
}

/// An open 7z.
pub struct SevenZipArchive<R> {
    reader: R,
    file_len: u64,
    entries: Vec<SevenZipEntry>,
    streams: StreamsInfo,
    /// Absolute offset of every packed stream.
    pack_offsets: Vec<u64>,
    next_header_crc: u32,
    next_header_location: u64,
    next_header_size: u64,
    structure: ZipStructure,
    header_structure: ZipStructure,
}

impl<R: Read + Seek> SevenZipArchive<R> {
    /// Read the headers (`SevenZ.ZipFileReadHeaders`, `SevenZipRead.cs:69-100`).
    pub fn open(mut reader: R) -> Result<SevenZipArchive<R>> {
        let file_len = reader.seek(SeekFrom::End(0))?;
        reader.seek(SeekFrom::Start(0))?;
        let mut sig = [0u8; 32];
        reader.read_exact(&mut sig).map_err(|e| {
            if e.kind() == io::ErrorKind::UnexpectedEof {
                ArchiveError::BadSignature {
                    what: "7z signature header",
                }
            } else {
                ArchiveError::Io(e)
            }
        })?;
        let (offset, size, crc) = parse_signature_header(&sig)?;
        let location = 32u64
            .checked_add(offset)
            .filter(|l| l.checked_add(size).is_some_and(|e| e <= file_len))
            .ok_or(ArchiveError::Truncated {
                what: "7z header (it lies beyond the end of the file)",
            })?;

        let mut header = Header::default();
        if size != 0 {
            reader.seek(SeekFrom::Start(location))?;
            let mut buf = vec![0u8; size as usize];
            reader.read_exact(&mut buf)?;
            if crc32fast::hash(&buf) != crc {
                return Err(ArchiveError::corrupt("7z header", "the CRC does not match"));
            }
            // The header may itself be packed, more than once in principle.
            let mut depth = 0;
            loop {
                match parse_header(&buf)? {
                    Parsed::Header(h) => {
                        header = h;
                        break;
                    }
                    Parsed::Encoded(streams) => {
                        depth += 1;
                        if depth > 4 {
                            return Err(ArchiveError::corrupt(
                                "7z header",
                                "the header is packed too many times",
                            ));
                        }
                        buf = unpack_header(&mut reader, &streams, file_len)?;
                    }
                }
            }
        }

        let pack_offsets = pack_offsets(&header.streams, file_len)?;
        let entries = populate_entries(&header)?;
        let mut zip = SevenZipArchive {
            reader,
            file_len,
            entries,
            streams: header.streams,
            pack_offsets,
            next_header_crc: crc,
            next_header_location: location,
            next_header_size: size,
            structure: ZipStructure::None,
            header_structure: ZipStructure::None,
        };
        zip.detect_structure()?;
        Ok(zip)
    }

    pub fn entries(&self) -> &[SevenZipEntry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn folders(&self) -> &[Folder] {
        &self.streams.folders
    }

    pub fn folder_info(&self, index: usize) -> Option<FolderInfo> {
        let f = self.streams.folders.get(index)?;
        let packed = (f.first_pack_stream..f.first_pack_stream + f.packed_streams.len())
            .filter_map(|i| self.streams.pack_sizes.get(i))
            .sum();
        Some(FolderInfo {
            coders: decode::coder_names(f),
            packed_size: packed,
            unpacked_size: f.unpack_size(),
            files: f.substreams.len(),
        })
    }

    /// The RomVault structure whose footer matches the headers *and* whose file rules hold
    /// (`Structured7Zip.ZipStruct`).
    pub fn structure(&self) -> ZipStructure {
        self.structure
    }

    /// The structure the footer announces, even if the file rules fail
    /// (`Structured7Zip.HeaderZipStruct`).
    pub fn header_structure(&self) -> ZipStructure {
        self.header_structure
    }

    /// Open an entry. The folder is decoded from its start up to the entry (solid archives cannot
    /// do better), then the entry's bytes are checked against its length and CRC32.
    pub fn open_entry(&mut self, index: usize) -> Result<CheckedReader<'_>> {
        let e = self
            .entries
            .get(index)
            .ok_or(ArchiveError::NoSuchEntry(index))?
            .clone();
        let Some(folder) = e.folder else {
            return Ok(CheckedReader::new(Box::new(io::empty()), e.name, 0, None));
        };
        let mut inner = folder_reader(&mut self.reader, &self.streams, &self.pack_offsets, folder)?;
        skip(&mut inner, e.offset_in_folder, &e.name)?;
        Ok(CheckedReader::new(inner, e.name, e.size, e.crc32))
    }

    pub fn read_entry(&mut self, index: usize) -> Result<Vec<u8>> {
        let size = self
            .entries
            .get(index)
            .ok_or(ArchiveError::NoSuchEntry(index))?
            .size;
        let mut out = Vec::with_capacity(size.min(1 << 28) as usize);
        self.open_entry(index)?
            .read_to_end(&mut out)
            .map_err(crate::checked::into_archive_error)?;
        Ok(out)
    }

    /// Decode every entry once, in order, giving each to `f` as a checked reader. Each folder is
    /// decoded a single time, so this is the way to read a solid archive.
    pub fn for_each_entry<F>(&mut self, f: F) -> Result<()>
    where
        F: FnMut(&SevenZipEntry, &mut dyn Read) -> Result<()>,
    {
        self.for_each_entry_with(true, f)
    }

    /// [`SevenZipArchive::for_each_entry`] that can leave the CRC32 unchecked
    /// (`check_crc` false); the length is checked either way. For a scanner that hashes the data
    /// itself and wants the bytes of an entry whose CRC is wrong.
    pub fn for_each_entry_with<F>(&mut self, check_crc: bool, mut f: F) -> Result<()>
    where
        F: FnMut(&SevenZipEntry, &mut dyn Read) -> Result<()>,
    {
        let entries = self.entries.clone();
        let mut i = 0;
        while i < entries.len() {
            let Some(folder) = entries[i].folder else {
                f(&entries[i], &mut io::empty())?;
                i += 1;
                continue;
            };
            // One decoder for the run of entries in this folder (entries without data in between
            // do not end the run).
            let mut inner =
                folder_reader(&mut self.reader, &self.streams, &self.pack_offsets, folder)?;
            while i < entries.len() && entries[i].folder.is_none_or(|fi| fi == folder) {
                let e = &entries[i];
                i += 1;
                if e.folder.is_none() {
                    f(e, &mut io::empty())?;
                    continue;
                }
                let mut checked = CheckedReader::new(
                    Box::new(inner.by_ref().take(e.size)),
                    e.name.clone(),
                    e.size,
                    e.crc32.filter(|_| check_crc),
                );
                f(e, &mut checked)?;
                // Whatever the callback left unread must still be consumed, and checked.
                io::copy(&mut checked, &mut io::sink())
                    .map_err(crate::checked::into_archive_error)?;
            }
        }
        Ok(())
    }

    /// Decode everything and check every CRC32. Returns the number of bytes decoded.
    pub fn verify_all(&mut self) -> Result<u64> {
        let mut total = 0;
        self.for_each_entry(|_, r| {
            total += io::copy(r, &mut io::sink()).map_err(crate::checked::into_archive_error)?;
            Ok(())
        })?;
        Ok(total)
    }

    pub fn into_inner(self) -> R {
        self.reader
    }

    /// `Structured7Zip.ValidateStructure` for the RomVault kinds (`Structured7Zip.cs:100-113`).
    fn detect_structure(&mut self) -> Result<()> {
        let s = self.romvault_footer()?;
        if s != ZipStructure::None {
            self.header_structure = s;
            if self.validate_files_structure(s)? {
                self.structure = s;
            }
        }
        Ok(())
    }

    /// `IsRomVault7Z` (`Structured7Zip.cs:251-298`): 32 bytes in front of the header.
    fn romvault_footer(&mut self) -> Result<ZipStructure> {
        if self.file_len < 32 || self.next_header_location < 32 + 32 {
            return Ok(ZipStructure::None);
        }
        self.reader
            .seek(SeekFrom::Start(self.next_header_location - 32))?;
        let mut b = [0u8; 32];
        if self.reader.read_exact(&mut b).is_err() || &b[..11] != b"RomVault7Z0" {
            return Ok(ZipStructure::None);
        }
        let crc = u32::from_le_bytes(b[12..16].try_into().unwrap());
        let loc = u64::from_le_bytes(b[16..24].try_into().unwrap());
        let size = u64::from_le_bytes(b[24..32].try_into().unwrap());
        if crc != self.next_header_crc
            || loc != self.next_header_location
            || size != self.next_header_size
        {
            return Ok(ZipStructure::None);
        }
        Ok(match b[11] {
            b'1' => ZipStructure::SevenZipSLZMA,
            b'2' => ZipStructure::SevenZipNLZMA,
            b'3' => ZipStructure::SevenZipSZSTD,
            b'4' => ZipStructure::SevenZipNZSTD,
            _ => ZipStructure::None,
        })
    }

    /// `validateFilesStructure` (`Structured7Zip.cs:117-222`).
    fn validate_files_structure(&mut self, s: ZipStructure) -> Result<bool> {
        let names: Vec<&str> = self.entries.iter().map(|e| e.name.as_str()).collect();
        if names.iter().any(|n| n.contains('\\')) {
            return Ok(false);
        }
        if names
            .windows(2)
            .any(|w| trrnt7zip_compare(w[0], w[1]) != std::cmp::Ordering::Less)
        {
            return Ok(false);
        }
        // No directory entry that is a prefix of the next name in ordinal order.
        let mut sorted = names.clone();
        sorted.sort_by(|a, b| ordinal_compare(a, b));
        for w in sorted.windows(2) {
            if w[0].ends_with('/') && w[1].len() > w[0].len() && w[1].starts_with(w[0]) {
                return Ok(false);
            }
        }
        if matches!(s, ZipStructure::SevenZipSZSTD | ZipStructure::SevenZipNZSTD) {
            // Every packed stream ends with `01 00 00`.
            for i in 0..self.streams.pack_sizes.len() {
                let (off, size) = (self.pack_offsets[i], self.streams.pack_sizes[i]);
                if size < 3 {
                    return Ok(false);
                }
                let mut tail = [0u8; 3];
                self.reader.seek(SeekFrom::Start(off + size - 3))?;
                self.reader.read_exact(&mut tail)?;
                if tail != [0x01, 0x00, 0x00] {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }
}

/// The decoder of one folder, reading its packed stream from `reader`.
fn folder_reader<'a, R: Read + Seek>(
    reader: &'a mut R,
    streams: &StreamsInfo,
    pack_offsets: &[u64],
    folder: usize,
) -> Result<Box<dyn Read + 'a>> {
    let f = &streams.folders[folder];
    let pack = f.first_pack_stream;
    reader.seek(SeekFrom::Start(pack_offsets[pack]))?;
    decode::open_folder(f, reader.take(streams.pack_sizes[pack]))
}

/// Read and discard `n` bytes.
fn skip(r: &mut dyn Read, n: u64, name: &str) -> Result<()> {
    let copied = io::copy(&mut r.take(n), &mut io::sink()).map_err(|e| io_to_decode(name, e))?;
    if copied != n {
        return Err(ArchiveError::SizeMismatch {
            entry: name.to_string(),
            expected: n,
            actual: copied,
        });
    }
    Ok(())
}

/// Absolute offsets of the packed streams (`PackedStreamInfo`, base offset 32).
fn pack_offsets(s: &StreamsInfo, file_len: u64) -> Result<Vec<u64>> {
    let mut out = Vec::with_capacity(s.pack_sizes.len());
    let mut pos = 32u64
        .checked_add(s.pack_pos)
        .ok_or_else(|| ArchiveError::corrupt("7z header", "pack position overflows"))?;
    for &size in &s.pack_sizes {
        out.push(pos);
        pos = pos
            .checked_add(size)
            .filter(|&p| p <= file_len)
            .ok_or(ArchiveError::Truncated {
                what: "7z packed streams",
            })?;
    }
    for f in &s.folders {
        if f.first_pack_stream + f.packed_streams.len() > s.pack_sizes.len() {
            return Err(ArchiveError::corrupt(
                "7z header",
                "a folder uses more packed streams than exist",
            ));
        }
        if f.unpack_sizes.len() as u64 != f.coders.iter().map(|c| c.num_out_streams).sum::<u64>() {
            return Err(ArchiveError::corrupt(
                "7z header",
                "a folder has no unpack sizes",
            ));
        }
    }
    Ok(out)
}

/// Decode the first folder of an encoded header (`Header.cs:61-79`).
fn unpack_header<R: Read + Seek>(
    reader: &mut R,
    s: &StreamsInfo,
    file_len: u64,
) -> Result<Vec<u8>> {
    let folder = s
        .folders
        .first()
        .ok_or_else(|| ArchiveError::corrupt("7z header", "an encoded header without a folder"))?;
    let offsets = pack_offsets(s, file_len)?;
    if offsets.is_empty() || s.pack_sizes.is_empty() {
        return Err(ArchiveError::corrupt(
            "7z header",
            "an encoded header without packed data",
        ));
    }
    let size = folder.unpack_size();
    if size > MAX_HEADER_BYTES {
        return Err(ArchiveError::corrupt(
            "7z header",
            "the packed header is implausibly large",
        ));
    }
    reader.seek(SeekFrom::Start(offsets[folder.first_pack_stream]))?;
    let packed = reader.take(s.pack_sizes[folder.first_pack_stream]);
    let mut inner = decode::open_folder(folder, packed)?;
    let mut out = Vec::with_capacity(size as usize);
    inner
        .read_to_end(&mut out)
        .map_err(|e| io_to_decode("7z header", e))?;
    if out.len() as u64 != size {
        return Err(ArchiveError::Truncated {
            what: "packed 7z header",
        });
    }
    if let Some(crc) = folder.unpack_crc {
        let actual = crc32fast::hash(&out);
        if actual != crc {
            return Err(ArchiveError::CrcMismatch {
                entry: "7z header".to_string(),
                expected: crc,
                actual,
            });
        }
    }
    Ok(out)
}

/// `PopulateLocalFiles` (`SevenZipRead.cs:103-159`), without its "every folder has files" assumption.
fn populate_entries(h: &Header) -> Result<Vec<SevenZipEntry>> {
    let files = &h.files;
    let folders = &h.streams.folders;
    let ticks = |v: &Option<Vec<Option<u64>>>, i: usize| -> Option<i64> {
        v.as_ref()
            .and_then(|v| v[i])
            .and_then(|t| i64::try_from(t).ok())
            .and_then(|t| t.checked_add(FILE_TIME_TO_UTC_TIME))
    };
    let mut out = Vec::with_capacity(files.names.len());
    let (mut folder_index, mut sub_index, mut stream_offset) = (0usize, 0usize, 0u64);
    let mut empty_index = 0usize;
    for (i, name) in files.names.iter().enumerate() {
        let empty_stream = files.empty_stream.as_ref().is_some_and(|v| v[i]);
        let mut e = SevenZipEntry {
            name: name.clone(),
            is_directory: false,
            is_anti: false,
            size: 0,
            crc32: None,
            folder: None,
            offset_in_folder: 0,
            modified_ticks: ticks(&files.write, i),
            accessed_ticks: ticks(&files.access, i),
            created_ticks: ticks(&files.creation, i),
            attributes: files.attributes.as_ref().and_then(|v| v[i]),
        };
        if !empty_stream {
            // The next substream in folder order, skipping folders that hold none.
            while folder_index < folders.len()
                && sub_index >= folders[folder_index].substreams.len()
            {
                folder_index += 1;
                sub_index = 0;
                stream_offset = 0;
            }
            let sub = folders
                .get(folder_index)
                .and_then(|f| f.substreams.get(sub_index))
                .ok_or_else(|| {
                    ArchiveError::corrupt("7z header", "more files with data than streams")
                })?;
            e.folder = Some(folder_index);
            e.offset_in_folder = stream_offset;
            e.size = sub.size;
            e.crc32 = sub.crc;
            stream_offset += sub.size;
            sub_index += 1;
        } else {
            e.is_directory = files.empty_file.as_ref().is_none_or(|v| !v[empty_index]);
            e.is_anti = files.anti.as_ref().is_some_and(|v| v[empty_index]);
            empty_index += 1;
            if e.is_directory && !e.name.ends_with('/') {
                e.name.push('/');
            }
        }
        out.push(e);
    }
    Ok(out)
}
