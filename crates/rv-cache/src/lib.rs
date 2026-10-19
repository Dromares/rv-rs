//! `rv-cache`: reads and writes ROMVault's `RomVault3_<n>.Cache` v2/v3 files byte for byte.
//!
//! The grammar and every field are specified in `docs/cache-format.md` of the decompile
//! notes (not part of this repository); comments cite the RVWorld 3.8.0 source lines
//! through that document. What this crate guarantees:
//!
//! * any file [`CacheFile::from_bytes`] accepts is written back by [`CacheFile::to_bytes`]
//!   byte for byte; anything that could not be (a count flag with a zero count, a bool
//!   byte other than 0/1, a non-shortest string length, bytes after the end marker) is an
//!   error instead;
//! * corrupt or truncated data is an error value, never a panic or a process exit;
//! * [`load`] falls back to `<cache>Backup` and reports that it did;
//! * [`save`] writes a temp file, `fsync`s it, keeps the old file as `<cache>Backup` and
//!   renames the temp file over the cache, so a failure part-way leaves the old cache.
//!
//! Reading returns what is on disk. ROMVault's load-time repairs are in [`fixups`].
//!
//! [`load_database`] and [`save_database`] are `DB.Read` and `DB.Write` for the working copy:
//! they apply those repairs (optionally), link every node to its DAT, compute every `RepStatus`
//! the way `DB.Read` does, and on the way out stamp the DAT positions back before the crash-safe
//! [`save`].

mod database;
mod error;
pub mod fixups;
mod flags;
mod read;
mod store;
mod write;

use std::io::Write;

use rv_core::RvFile;

pub use database::{
    into_database, load_database, save_database, to_cache_file, DatabaseError, DatabaseOptions,
    SaveError,
};
pub use error::CacheError;
pub use store::{backup_path, load, load_file, save, tmp_path, LoadError, LoadSource, Loaded};

/// The trailing `u64` that ends every cache (`DB.cs:24`): bytes `A7 DD 00 A6 15 00 00 00`.
pub const END_MARKER: u64 = 0x0015_A600_DDA7;

/// The version ROMVault 3.8 writes (`DBVersion.Version`, `DB.cs:18`).
pub const CURRENT_VERSION: i32 = 3;

/// Deepest tree accepted. Real caches are about 6 levels deep (the board's are); this stops a
/// corrupt file from overflowing the stack.
pub const MAX_DEPTH: usize = 128;

/// One cache file: the on-disk version and the root of the tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheFile {
    /// 2 or 3 (cache section 4). Version 2 archive nodes have no `zip_dat_struct`.
    pub version: i32,
    pub root: RvFile,
}

impl CacheFile {
    /// A version 3 cache holding `root`.
    pub fn new(root: RvFile) -> CacheFile {
        CacheFile {
            version: CURRENT_VERSION,
            root,
        }
    }

    /// The cache ROMVault creates on a first run (`DB.OpenDefaultDB`, `DB.cs:31-55`).
    pub fn default_db() -> CacheFile {
        CacheFile::new(RvFile::default_root())
    }

    /// Parse the bytes of a cache file.
    pub fn from_bytes(bytes: &[u8]) -> Result<CacheFile, CacheError> {
        read::parse(bytes)
    }

    /// Serialize to the bytes of a cache file.
    pub fn to_bytes(&self) -> Result<Vec<u8>, CacheError> {
        write::serialize(self)
    }

    /// Serialize into `w`.
    pub fn write_to<W: Write>(&self, w: &mut W) -> Result<(), CacheError> {
        w.write_all(&self.to_bytes()?)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
