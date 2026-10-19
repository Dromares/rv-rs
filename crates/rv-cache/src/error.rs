//! Errors for reading, writing and loading a cache. Corrupt input is always an error
//! value, never a panic and never a process exit (the C# code exits with code 0,
//! `ReportError.cs:81-105`).

use std::fmt;
use std::io;

/// What went wrong with one cache file.
#[derive(Debug)]
pub enum CacheError {
    /// The file could not be read or written.
    Io(io::Error),
    /// The data ended in the middle of a field, at this byte offset.
    Truncated { offset: usize },
    /// The leading version is not 2 or 3. The C# reader treats versions above 3 as 3
    /// (`DB.cs:148-162`); this reader does not guess at a layout it has never seen.
    UnsupportedVersion(i32),
    /// The 8 bytes after the root node are not `0x15A600DDA7` (`DB.cs:182-186`).
    BadEndMarker { found: u64 },
    /// Bytes follow the end marker, at this offset.
    TrailingData { offset: usize },
    /// A string is not valid UTF-8.
    InvalidUtf8 { offset: usize },
    /// A negative or impossibly large count or length.
    BadCount { offset: usize, count: i64 },
    /// The bytes are readable but rewriting them would not give the same bytes (for
    /// example a count flag set with a count of zero). Rejected so that every file this
    /// crate accepts writes back byte for byte.
    NonCanonical { offset: usize, what: &'static str },
    /// The tree is nested deeper than [`crate::MAX_DEPTH`].
    TooDeep,
    /// The in-memory model cannot be written as a valid cache.
    InvalidModel(String),
}

impl CacheError {
    /// True when the file simply does not exist.
    pub fn is_not_found(&self) -> bool {
        matches!(self, CacheError::Io(e) if e.kind() == io::ErrorKind::NotFound)
    }
}

impl fmt::Display for CacheError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CacheError::Io(e) => write!(f, "I/O error: {e}"),
            CacheError::Truncated { offset } => write!(f, "cache is truncated at byte {offset}"),
            CacheError::UnsupportedVersion(v) => write!(f, "unsupported cache version {v}"),
            CacheError::BadEndMarker { found } => {
                write!(f, "bad cache end marker {found:#x}, expected 0x15a600dda7")
            }
            CacheError::TrailingData { offset } => {
                write!(f, "unexpected data after the end marker at byte {offset}")
            }
            CacheError::InvalidUtf8 { offset } => {
                write!(f, "string at byte {offset} is not valid UTF-8")
            }
            CacheError::BadCount { offset, count } => {
                write!(f, "bad count or length {count} at byte {offset}")
            }
            CacheError::NonCanonical { offset, what } => {
                write!(f, "non-canonical encoding at byte {offset}: {what}")
            }
            CacheError::TooDeep => write!(f, "cache tree is nested too deeply"),
            CacheError::InvalidModel(why) => write!(f, "cannot write cache: {why}"),
        }
    }
}

impl std::error::Error for CacheError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            CacheError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for CacheError {
    fn from(e: io::Error) -> Self {
        CacheError::Io(e)
    }
}
