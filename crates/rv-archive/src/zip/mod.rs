//! Zip: a reader for the methods a permitted crate covers (Shrink, Reduce and Implode are typed errors), and the byte-exact TorrentZip writer.
//!
//! Spec: `docs/archives-and-rvio.md` section 2 and `docs/codecs.md` of the decompile notes (not
//! part of this repository). Comments cite RVWorld 3.8.0 `Compress/ZipFile/*` and
//! `Compress/StructuredZip/*` by file and line.

mod codec;
mod extra;
mod read;
mod structure;
#[cfg(test)]
mod tests;
mod time;
mod write;

pub(crate) use codec::raw_lzma1;
pub use read::{EntryReader, LocalFileStatus, ZipArchive, ZipEntry};
pub use structure::ZipStructure;
pub use time::{dos_to_ticks, ticks_to_dos};
pub use write::{AddedEntry, TorrentZipWriter};

/// The PKWARE method numbers (`ZipEnums.cs:48-77`).
pub mod method {
    pub const STORED: u16 = 0;
    pub const SHRUNK: u16 = 1;
    pub const REDUCED1: u16 = 2;
    pub const REDUCED2: u16 = 3;
    pub const REDUCED3: u16 = 4;
    pub const REDUCED4: u16 = 5;
    pub const IMPLODED: u16 = 6;
    pub const DEFLATED: u16 = 8;
    pub const DEFLATE64: u16 = 9;
    pub const BZIP2: u16 = 12;
    pub const LZMA: u16 = 14;
    pub const ZSTANDARD_DEPRECATED: u16 = 20;
    pub const ZSTANDARD: u16 = 93;
    pub const PPMD: u16 = 98;
}

/// DOS date/time of every entry in a TorrentZip, packed as `(date << 16) | time`: 1996-12-24
/// 23:32:00 (`StructuredZip.cs:413-414`, `TrrntzipDosDateTime`).
pub const TRRNTZIP_DOS_DATE_TIME: i64 = 563_657_728;

/// The same instant in .NET ticks (`StructuredZip.cs:413`, `TrrntzipDateTime`).
pub const TRRNTZIP_DATE_TIME_TICKS: i64 = 629_870_671_200_000_000;

pub(crate) const LOCAL_FILE_HEADER_SIGNATURE: u32 = 0x0403_4b50;
pub(crate) const CENTRAL_DIRECTORY_HEADER_SIGNATURE: u32 = 0x0201_4b50;
pub(crate) const END_OF_CENTRAL_DIR_SIGNATURE: u32 = 0x0605_4b50;
pub(crate) const ZIP64_END_OF_CENTRAL_DIR_SIGNATURE: u32 = 0x0606_4b50;
pub(crate) const ZIP64_END_OF_CENTRAL_DIR_LOCATOR_SIGNATURE: u32 = 0x0706_4b50;
