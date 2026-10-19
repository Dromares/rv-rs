//! The cache as a [`Database`]: what `DB.Read` leaves in memory and what `DB.Write` saves
//! (`RomVaultCore/RvDB/DB.cs:57-191`).
//!
//! Reading a cache file is lossless ([`crate::CacheFile`]); turning it into a database applies
//! ROMVault's load-time repairs ([`crate::fixups`], optional), links every node to its DAT, and
//! recomputes `RepStatus` for every node, the way `DB.Read` does (cache section 6, step 9).
//! Saving stamps the DAT positions back into the tree and goes through the crash-safe
//! [`crate::save`].

use std::path::Path;

use rv_core::{Database, DbError, Settings};

use crate::fixups::apply_load_fixups;
use crate::store::{load, save, LoadError, LoadSource};
use crate::{CacheError, CacheFile};

/// How to turn a cache into a database.
#[derive(Clone, Copy, Debug)]
pub struct DatabaseOptions {
    /// Apply ROMVault's load-time repairs ([`apply_load_fixups`]); on by default. Off, the
    /// tree is exactly what is on disk, which is what a byte-identical round trip needs.
    pub apply_load_fixups: bool,
}

impl Default for DatabaseOptions {
    fn default() -> Self {
        DatabaseOptions {
            apply_load_fixups: true,
        }
    }
}

/// A cache that cannot become a database.
#[derive(Debug)]
pub enum DatabaseError {
    Load(LoadError),
    Link(DbError),
}

impl std::fmt::Display for DatabaseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DatabaseError::Load(e) => write!(f, "{e}"),
            DatabaseError::Link(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for DatabaseError {}

/// A [`CacheFile`] as a database, with every `RepStatus` computed.
pub fn into_database(
    mut cache: CacheFile,
    settings: &Settings,
    options: DatabaseOptions,
) -> Result<Database, DbError> {
    if options.apply_load_fixups {
        apply_load_fixups(&mut cache);
    }
    let mut db = Database::from_root(cache.root)?;
    db.report_status_reset(settings);
    Ok(db)
}

/// The database as a cache file: DAT positions stamped, the tree copied. Fails when a node points
/// at a DAT that is not in the list of its nearest ancestor with DATs.
pub fn to_cache_file(db: &mut Database) -> Result<CacheFile, DbError> {
    db.stamp_dat_indexes()?;
    Ok(CacheFile::new(db.root.clone()))
}

/// `DB.Read`: load `path` (falling back to `<path>Backup`) into a database. When neither file
/// exists the result is the default database and `LoadSource::Primary`; a corrupt pair is an
/// error.
pub fn load_database(
    path: &Path,
    settings: &Settings,
    options: DatabaseOptions,
) -> Result<(Database, LoadSource), DatabaseError> {
    match load(path) {
        Ok(loaded) => {
            let db = into_database(loaded.cache, settings, options).map_err(DatabaseError::Link)?;
            Ok((db, loaded.source))
        }
        Err(e) if e.is_not_found() => {
            let mut db = Database::default_db();
            db.report_status_reset(settings);
            Ok((db, LoadSource::Primary))
        }
        Err(e) => Err(DatabaseError::Load(e)),
    }
}

/// `DB.Write`: save the database to `path`, keeping the old file as `<path>Backup`.
pub fn save_database(path: &Path, db: &mut Database) -> Result<(), SaveError> {
    let cache = to_cache_file(db).map_err(SaveError::Link)?;
    save(path, &cache).map_err(SaveError::Cache)
}

/// A database that could not be saved.
#[derive(Debug)]
pub enum SaveError {
    Link(DbError),
    Cache(CacheError),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveError::Link(e) => write!(f, "{e}"),
            SaveError::Cache(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for SaveError {}
