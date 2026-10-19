//! `rv-archive`: zip (with a byte-exact TorrentZip writer), 7z and CHD.
//!
//! The formats are specified in `docs/archives-and-rvio.md` and `docs/codecs.md` of the decompile
//! notes (not part of this repository); comments cite the RVWorld 3.8.0 source lines through
//! those documents. What this crate guarantees:
//!
//! * corrupt or truncated input is an [`ArchiveError`], never a panic;
//! * entry data is read as a stream and its CRC32 and length are checked against the headers;
//! * a method or codec that is not decoded is [`ArchiveError::UnsupportedCompression`], like
//!   `ZipUnsupportedCompression` in ROMVault;
//! * writers write into the stream they are given and never touch an existing file by name.
//!   [`AtomicFile`] is the temp-file, `fsync`, rename step; the caller wires it up (the fixer does
//!   that in Phase 5b).
//!
//! `rv-archive` does not depend on `rv-dat` or `rv-scan`.

mod atomic;
pub mod chd;
mod checked;
pub mod cp437;
pub mod deflate;
mod error;
pub mod sevenzip;
pub mod sort;
pub mod zip;

pub use atomic::AtomicFile;
pub use checked::{into_archive_error, CheckedReader};
pub use error::{ArchiveError, ChdError, Result, StructureError};
