//! `rv-scan`: hashes a byte stream and detects copier headers, as ROMVault's file scanner
//! does for loose files (`FileScanner/FileScan.cs` `CheckSumRead`, `FileScanner/FileHeaders.cs`,
//! `RomVaultCore/Scanner/Compare.cs`).
//!
//! The crate works on any [`std::io::Read`]. Looking inside zip/7z/CHD containers is a later
//! phase and this crate does not depend on an archive crate.
//!
//! Two defects of ROMVault 3.8.0 are fixed here on purpose (README section 10):
//!
//! * [`compare::compare_alt_hash`] compares MD5 the right way round
//!   (`Scanner/Compare.cs:239` returns `false` when the MD5 values are *equal*);
//! * SNES and PCE copier headers can be detected (`FileScan.cs:252-254` probes at most 512
//!   bytes while `FileHeaders.cs:119` needs more than 512 for those two detectors, so the
//!   3.8.0 scanner never sees them). [`MAX_HEADER_PROBE`] is 513.

pub mod compare;
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
