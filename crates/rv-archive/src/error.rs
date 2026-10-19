//! Errors for the archive readers and writers. Corrupt or truncated input is always an error
//! value, never a panic (the C# code reports through the `ZipReturn` enum, `ZipEnums.cs:3-38`,
//! plus a global `Error` object, `Error.cs:3-6`).

use std::fmt;
use std::io;

/// What went wrong with one archive.
#[derive(Debug)]
pub enum ArchiveError {
    /// The file could not be read or written.
    Io(io::Error),
    /// The data ended inside a structure (`what`).
    Truncated { what: &'static str },
    /// A required signature or magic number is wrong or missing (`what`).
    BadSignature { what: &'static str },
    /// The structure is readable but inconsistent (`ZipCentralDirError`, `ZipLocalFileHeaderError`
    /// and friends).
    Corrupt { what: &'static str, detail: String },
    /// The archive needs zip64 fields that are missing (`Zip64EndOfCentralDirError`).
    Zip64Required,
    /// A compression method or 7z coder this crate does not decode (`ZipUnsupportedCompression`,
    /// `ZipEnums.cs:3-38`). ROMVault returns the same status instead of failing the whole scan.
    UnsupportedCompression { method: String },
    /// A feature that is read but deliberately not supported (encryption, external 7z data...).
    Unsupported { what: &'static str },
    /// The decoded data does not match the CRC in the headers (`ZipCRCDecodeError`).
    CrcMismatch {
        entry: String,
        expected: u32,
        actual: u32,
    },
    /// The decoded data has the wrong length (`ZipDecodeError`).
    SizeMismatch {
        entry: String,
        expected: u64,
        actual: u64,
    },
    /// The decompressor reported a damaged stream.
    Decode { entry: String, detail: String },
    /// No entry with this index.
    NoSuchEntry(usize),
    /// A write that would break the structure being written (`ZipTrrntzipIncorrect*`).
    Structure(StructureError),
}

/// Why a structured (TorrentZip-style) writer refused an entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StructureError {
    /// The name contains a backslash (`ZipTrrntZipIncorrectDirectorySeparator`).
    BackslashInName(String),
    /// The name sorts before the previous one (`ZipTrrntzipIncorrectFileOrder`).
    OutOfOrder { previous: String, name: String },
    /// A directory entry is followed by a name inside it (`ZipTrrntzipIncorrectDirectoryAddedToZip`).
    DirectoryNotEmpty { directory: String, name: String },
    /// The structure asks for a codec this writer does not have.
    UnsupportedStructure(&'static str),
    /// The caller's data did not match the declared size or CRC.
    DataMismatch(String),
    /// A directory entry (name ends with `/`) with data.
    DirectoryWithData(String),
    /// More entries or bytes than the format can address.
    TooLarge(&'static str),
}

impl ArchiveError {
    pub(crate) fn corrupt(what: &'static str, detail: impl Into<String>) -> ArchiveError {
        ArchiveError::Corrupt {
            what,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for ArchiveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ArchiveError::Io(e) => write!(f, "I/O error: {e}"),
            ArchiveError::Truncated { what } => write!(f, "archive is truncated in the {what}"),
            ArchiveError::BadSignature { what } => {
                write!(f, "bad or missing signature of the {what}")
            }
            ArchiveError::Corrupt { what, detail } => write!(f, "corrupt {what}: {detail}"),
            ArchiveError::Zip64Required => {
                write!(f, "the zip needs zip64 fields that are not there")
            }
            ArchiveError::UnsupportedCompression { method } => {
                write!(f, "unsupported compression: {method}")
            }
            ArchiveError::Unsupported { what } => write!(f, "unsupported: {what}"),
            ArchiveError::CrcMismatch {
                entry,
                expected,
                actual,
            } => write!(
                f,
                "{entry}: CRC32 is {actual:08X}, the header says {expected:08X}"
            ),
            ArchiveError::SizeMismatch {
                entry,
                expected,
                actual,
            } => write!(f, "{entry}: {actual} bytes, the header says {expected}"),
            ArchiveError::Decode { entry, detail } => write!(f, "{entry}: cannot decode: {detail}"),
            ArchiveError::NoSuchEntry(i) => write!(f, "no entry {i}"),
            ArchiveError::Structure(e) => write!(f, "{e}"),
        }
    }
}

impl fmt::Display for StructureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StructureError::BackslashInName(n) => write!(f, "{n:?} contains a backslash"),
            StructureError::OutOfOrder { previous, name } => {
                write!(f, "{name:?} must not come after {previous:?}")
            }
            StructureError::DirectoryNotEmpty { directory, name } => {
                write!(
                    f,
                    "directory entry {directory:?} is not empty ({name:?} is inside it)"
                )
            }
            StructureError::UnsupportedStructure(s) => write!(f, "cannot write {s}"),
            StructureError::DataMismatch(s) => write!(f, "{s}"),
            StructureError::DirectoryWithData(n) => write!(f, "directory entry {n:?} has data"),
            StructureError::TooLarge(s) => write!(f, "too large: {s}"),
        }
    }
}

impl std::error::Error for ArchiveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ArchiveError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for ArchiveError {
    fn from(e: io::Error) -> ArchiveError {
        // A short read in the middle of a structure is "truncated", not a generic I/O failure.
        if e.kind() == io::ErrorKind::UnexpectedEof {
            ArchiveError::Truncated { what: "data" }
        } else {
            ArchiveError::Io(e)
        }
    }
}

impl From<StructureError> for ArchiveError {
    fn from(e: StructureError) -> ArchiveError {
        ArchiveError::Structure(e)
    }
}

pub type Result<T> = std::result::Result<T, ArchiveError>;
