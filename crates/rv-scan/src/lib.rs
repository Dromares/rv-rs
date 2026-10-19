//! `rv-scan`: hashes a byte stream and detects copier headers, as ROMVault's file scanner
//! does for loose files (`FileScanner/FileScan.cs` `CheckSumRead`, `FileScanner/FileHeaders.cs`,
//! `RomVaultCore/Scanner/Compare.cs`).
//!
//! The hashing works on any [`std::io::Read`]. [`db`] is the directory scan that uses it: it walks
//! the files on disk against the database tree, opens zips, 7z archives and CHDs through
//! `rv-archive`, and decides what each item's `GotStatus` is.
//!
//! # Scanning a database
//!
//! [`db::scan_files`] walks the directories of a [`rv_core::Database`] against the files on
//! disk. It keeps no state, reports progress to a sink, looks at a cancel flag between items, and
//! writes nothing; the caller saves.
//!
//! ```no_run
//! use rv_cache::{load_database, save_database, DatabaseOptions};
//! use rv_core::progress::{CancelFlag, ProgressEvent};
//! use rv_core::{ScanLevel, Settings};
//! use rv_scan::db::{scan_files, ScanRequest, ScanTarget};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let settings = Settings::from_xml(&std::fs::read_to_string("RomVault3cfg.xml")?)?;
//! let cache = std::path::Path::new("RomVault3_3.Cache");
//! let (mut db, _) = load_database(cache, &settings, DatabaseOptions::default())?;
//!
//! let request = ScanRequest {
//!     level: ScanLevel::Level2,                              // the usual "Scan"
//!     target: ScanTarget::Dir("RomVault/Console/NES/Roms".into()), // or ScanTarget::Selected
//! };
//! let progress = |e: ProgressEvent| eprintln!("{e:?}");
//! let report = scan_files(&mut db, &settings, &request, &progress, &CancelFlag::new())?;
//! eprintln!("{} archives read, {} files hashed", report.archives_scanned, report.files_hashed);
//!
//! save_database(cache, &mut db)?;
//! # Ok(()) }
//! ```
//!
//! Two defects of ROMVault 3.8.0 are fixed here on purpose (README section 10):
//!
//! * [`compare::compare_alt_hash`] compares MD5 the right way round
//!   (`Scanner/Compare.cs:239` returns `false` when the MD5 values are *equal*);
//! * SNES and PCE copier headers can be detected (`FileScan.cs:252-254` probes at most 512
//!   bytes while `FileHeaders.cs:119` needs more than 512 for those two detectors, so the
//!   3.8.0 scanner never sees them). [`MAX_HEADER_PROBE`] is 513.

pub mod compare;
pub mod db;
pub mod hash;
mod scan;

/// Copier-header detection. The table lives in `rv-core`, because the DAT merge needs it too.
pub use rv_core::header;

pub use hash::{Digests, HashKinds, HashSet};
pub use scan::{scan, AltDigests, ScanOptions, ScanResult};

/// Number of bytes read up front for header detection.
///
/// 3.8.0 reads `min(512, size)` (`FileScan.cs:252-254`). A detector only fires when the probe
/// is *longer* than its header (`FileHeaders.cs:119`, `headerSize <= HeaderLength` skips), so
/// the SNES and PCE detectors (512-byte headers, `FileHeaders.cs:41,47-48`) need 513.
pub const MAX_HEADER_PROBE: usize = 513;
