//! The structured-archive kinds and the rules that recognise them
//! (`Compress/StructuredZip/StructuredArchive.cs`, `StructuredZip.cs:181-399`).

use std::cmp::Ordering;

use super::method;
use super::read::{LocalFileStatus, ZipEntry};
use super::TRRNTZIP_DATE_TIME_TICKS;
use super::TRRNTZIP_DOS_DATE_TIME;
use crate::cp437;
use crate::sort::trrntzip_compare;

/// `ZipStructure` (`StructuredArchive.cs:3-16`). The numbers match the C# enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum ZipStructure {
    /// No structure.
    None = 0,
    /// Original TorrentZip: deflate, fixed 1996-12-24 timestamp.
    ZipTrrnt = 1,
    /// Total DOS Collection: deflate, any timestamp.
    ZipTDC = 2,
    /// Date Time Deflate.
    ZipDTD = 3,
    /// The original t7z format.
    SevenZipTrrnt = 4,
    /// zstd, zero timestamps (`RVZSTD-`).
    ZipZSTD = 5,
    /// Date Time zstd.
    ZipDTZ = 6,
    SevenZipSLZMA = 8,
    SevenZipNLZMA = 9,
    SevenZipSZSTD = 10,
    SevenZipNZSTD = 11,
}

/// How a structure constrains the entries' timestamps (`zipDateType`, `StructuredArchive.cs:18-24`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DateType {
    Undefined,
    None,
    TrrntZip,
    DateTime,
}

/// The method value that means "no valid method" (`(ZipCompression)ushort.MaxValue`).
pub(crate) const NO_VALID_METHOD: u16 = u16::MAX;

impl ZipStructure {
    /// Every value, in enum order (the order `ValidateStructure` tries them in).
    pub const ALL: [ZipStructure; 11] = [
        ZipStructure::None,
        ZipStructure::ZipTrrnt,
        ZipStructure::ZipTDC,
        ZipStructure::ZipDTD,
        ZipStructure::SevenZipTrrnt,
        ZipStructure::ZipZSTD,
        ZipStructure::ZipDTZ,
        ZipStructure::SevenZipSLZMA,
        ZipStructure::SevenZipNLZMA,
        ZipStructure::SevenZipSZSTD,
        ZipStructure::SevenZipNZSTD,
    ];

    /// `StructuredArchive.GetCompressionType` (`StructuredArchive.cs:29-53`).
    pub fn compression_type(self, uncompressed_size: u64) -> u16 {
        match self {
            ZipStructure::None => method::STORED,
            ZipStructure::ZipTrrnt | ZipStructure::ZipTDC | ZipStructure::ZipDTD => {
                method::DEFLATED
            }
            ZipStructure::SevenZipTrrnt => NO_VALID_METHOD,
            ZipStructure::ZipZSTD | ZipStructure::ZipDTZ => {
                if uncompressed_size == 0 {
                    method::STORED
                } else {
                    method::ZSTANDARD
                }
            }
            ZipStructure::SevenZipSLZMA | ZipStructure::SevenZipNLZMA => method::LZMA,
            ZipStructure::SevenZipSZSTD | ZipStructure::SevenZipNZSTD => method::ZSTANDARD,
        }
    }

    /// `StructuredArchive.IsSolid` (`StructuredArchive.cs:55-67`).
    pub fn is_solid(self) -> bool {
        !matches!(
            self,
            ZipStructure::SevenZipNLZMA | ZipStructure::SevenZipNZSTD
        )
    }

    /// `StructuredArchive.GetZipCommentId` (`StructuredArchive.cs:69-86`); empty for the kinds that
    /// have no zip comment.
    pub fn comment_id(self) -> &'static str {
        match self {
            ZipStructure::ZipTrrnt => "TORRENTZIPPED-",
            ZipStructure::ZipTDC => "TDC-",
            ZipStructure::ZipZSTD => "RVZSTD-",
            ZipStructure::ZipDTD => "DTD-",
            ZipStructure::ZipDTZ => "DTZ-",
            _ => "",
        }
    }

    /// `StructuredArchive.GetZipStructureName` (`StructuredArchive.cs:88-119`).
    pub fn name(self) -> &'static str {
        match self {
            ZipStructure::None => "Unstructured",
            ZipStructure::ZipTrrnt => "TrrntZip",
            ZipStructure::ZipTDC => "TDC-Zip",
            ZipStructure::ZipDTD => "DTD-Zip",
            ZipStructure::SevenZipTrrnt => "T7Z",
            ZipStructure::ZipZSTD => "ZSTD-Zip",
            ZipStructure::ZipDTZ => "DTZ-Zip",
            ZipStructure::SevenZipSLZMA => "7Z-Solid-LZMA",
            ZipStructure::SevenZipNLZMA => "7Z-NonSolid-LZMA",
            ZipStructure::SevenZipSZSTD => "7Z-Solid-zSTD",
            ZipStructure::SevenZipNZSTD => "7Z-NonSolid-zSTD",
        }
    }

    /// `StructuredArchive.GetZipDateTimeType` (`StructuredArchive.cs:121-138`).
    pub(crate) fn date_type(self) -> DateType {
        match self {
            ZipStructure::ZipTrrnt => DateType::TrrntZip,
            ZipStructure::ZipTDC | ZipStructure::ZipDTD | ZipStructure::ZipDTZ => {
                DateType::DateTime
            }
            ZipStructure::ZipZSTD => DateType::None,
            _ => DateType::Undefined,
        }
    }

    /// The zip comment the writer stores: the id plus the upper-case hex CRC32 of the central
    /// directory bytes (`WriteComments`, `StructuredZip.cs:403-409`).
    pub fn comment(self, central_dir_crc: u32) -> String {
        let id = self.comment_id();
        if id.is_empty() {
            String::new()
        } else {
            format!("{id}{central_dir_crc:08X}")
        }
    }
}

/// `StructuredZip.IsTzipDate` (`StructuredZip.cs:416-422`).
pub fn is_tzip_date(ticks: i64) -> bool {
    if ticks <= 0xffff_ffff {
        ticks == TRRNTZIP_DOS_DATE_TIME
    } else {
        ticks == TRRNTZIP_DATE_TIME_TICKS
    }
}

/// `ValidateFileHeader` (`StructuredZip.cs:237-281`) over every entry.
pub(crate) fn validate_file_headers(structure: ZipStructure, entries: &[ZipEntry]) -> bool {
    for e in entries {
        if e.status.intersects(
            LocalFileStatus::HEADERS_MISMATCH
                | LocalFileStatus::FILENAME_MISMATCH
                | LocalFileStatus::DIRECTORY_LENGTH_ERROR
                | LocalFileStatus::DATE_TIME_MISMATCH,
        ) {
            return false;
        }
        if e.extra_data_found {
            return false;
        }
        if e.method != structure.compression_type(e.uncompressed_size) {
            return false;
        }
        if e.name.contains('\\') {
            return false;
        }
        if cp437::is_codepage_437(&e.name) != (e.flags & (1 << 11) == 0) {
            return false;
        }
        match structure.date_type() {
            DateType::DateTime => {}
            DateType::None => {
                if e.last_modified() != 0 {
                    return false;
                }
            }
            DateType::TrrntZip => {
                if !is_tzip_date(e.last_modified()) {
                    return false;
                }
            }
            DateType::Undefined => return false,
        }
    }
    true
}

/// `ValidateFileOrder` (`StructuredZip.cs:283-291`): strictly increasing, no ASCII-case ties.
pub(crate) fn validate_file_order(entries: &[ZipEntry]) -> bool {
    entries
        .windows(2)
        .all(|w| trrntzip_compare(&w[0].name, &w[1].name) == Ordering::Less)
}

/// `ValidateDirectories` (`StructuredZip.cs:293-319`): a directory entry directly followed by a
/// name inside it makes the archive invalid (TorrentZip and RVZSTD only).
pub(crate) fn validate_directories(structure: ZipStructure, entries: &[ZipEntry]) -> bool {
    if !matches!(structure, ZipStructure::ZipTrrnt | ZipStructure::ZipZSTD) {
        return true;
    }
    for w in entries.windows(2) {
        let dir: Vec<u16> = w[0].name.encode_utf16().collect();
        if !dir.is_empty() && *dir.last().unwrap() != b'/' as u16 {
            continue;
        }
        let next: Vec<u16> = w[1].name.encode_utf16().collect();
        if next.len() <= dir.len() {
            continue;
        }
        let a = dir.iter().map(|&c| fold(c));
        let b = next[..dir.len()].iter().map(|&c| fold(c));
        if a.eq(b) {
            return false;
        }
    }
    true
}

fn fold(c: u16) -> u16 {
    if (65..=90).contains(&c) {
        c + 0x20
    } else {
        c
    }
}

/// The two-byte deflate stream of an empty entry (`trrntzero`, `StructuredZip.cs:321`).
pub(crate) const TRRNTZERO: [u8; 2] = [0x03, 0x00];
/// The last three bytes of every non-empty RVZSTD entry (`zstdEnd`, `StructuredZip.cs:322`).
pub(crate) const ZSTD_END: [u8; 3] = [0x01, 0x00, 0x00];
