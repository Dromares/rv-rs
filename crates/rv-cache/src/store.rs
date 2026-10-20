//! Loading with backup fallback and crash-safe saving (`DB.Read`/`DB.Write`,
//! `DB.cs:57-191`, with the fixes this rewrite promises: fsync, no exit on corruption,
//! no window without a live file).

use std::ffi::OsString;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::error::CacheError;
use crate::CacheFile;

/// `<cache>_tmp`, the file a save is written to first (`DB.cs:59`).
pub fn tmp_path(path: &Path) -> PathBuf {
    suffixed(path, "_tmp")
}

/// `<cache>Backup`, with no separator (`DB.cs:96`).
pub fn backup_path(path: &Path) -> PathBuf {
    suffixed(path, "Backup")
}

fn suffixed(path: &Path, suffix: &str) -> PathBuf {
    let mut name: OsString = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Which file a [`Loaded`] cache came from.
#[derive(Debug)]
pub enum LoadSource {
    /// The cache file itself.
    Primary,
    /// The cache file could not be used, so `<cache>Backup` was loaded instead.
    Backup { primary_error: CacheError },
}

/// A successfully loaded cache.
#[derive(Debug)]
pub struct Loaded {
    pub cache: CacheFile,
    pub source: LoadSource,
}

/// Neither the cache nor its backup could be loaded.
#[derive(Debug)]
pub struct LoadError {
    pub primary: CacheError,
    pub backup: CacheError,
}

impl LoadError {
    /// True when neither file exists, i.e. a first run: the caller should start from
    /// [`rv_core::RvFile::default_root`] (`DB.OpenDefaultDB`, `DB.cs:31-55`).
    pub fn is_not_found(&self) -> bool {
        self.primary.is_not_found() && self.backup.is_not_found()
    }
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "cache unusable ({}) and its backup is unusable ({})",
            self.primary, self.backup
        )
    }
}

impl std::error::Error for LoadError {}

/// Read and parse one cache file.
pub fn load_file(path: &Path) -> Result<CacheFile, CacheError> {
    CacheFile::from_bytes(&fs::read(path)?)
}

/// Load `path`; if it is missing, corrupt or truncated, load `<path>Backup` and say so
/// in [`Loaded::source`]. Never exits the process and never panics on bad data.
pub fn load(path: &Path) -> Result<Loaded, LoadError> {
    match load_file(path) {
        Ok(cache) => Ok(Loaded {
            cache,
            source: LoadSource::Primary,
        }),
        Err(primary) => match load_file(&backup_path(path)) {
            Ok(cache) => Ok(Loaded {
                cache,
                source: LoadSource::Backup {
                    primary_error: primary,
                },
            }),
            Err(backup) => Err(LoadError { primary, backup }),
        },
    }
}

/// Save `cache` to `path`:
///
/// 1. serialize (a model error stops here, before any file is touched);
/// 2. write `<path>_tmp` and `fsync` it;
/// 3. keep the old file as `<path>Backup` (a hard link, or a copy where the file system
///    has none), so `path` itself is never absent;
/// 4. rename the temp file over `path` and `fsync` the directory.
///
/// A failure before step 4 leaves `path` as it was.
pub fn save(path: &Path, cache: &CacheFile) -> Result<(), CacheError> {
    save_with(path, cache, |file| file)
}

/// The points of a [`save_stepped`] where a caller may look, and stop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveStep {
    /// The temp file is written (not yet synced).
    TmpWritten,
    /// The temp file is on disk.
    TmpSynced,
    /// The old cache is kept as `<path>Backup`; `path` still holds the old cache.
    BackedUp,
    /// The temp file has been renamed over `path`.
    Renamed,
}

/// [`save`] that reports each [`SaveStep`] to `step`, for the fixer's fault-injection tests: a
/// kill at any of them must leave a cache that loads.
pub fn save_stepped(
    path: &Path,
    cache: &CacheFile,
    step: &mut dyn FnMut(SaveStep),
) -> Result<(), CacheError> {
    save_inner(path, cache, |file| file, step)
}

/// [`save`] with the temp-file writer wrapped, so tests can fail a write part-way.
pub(crate) fn save_with<W: Write>(
    path: &Path,
    cache: &CacheFile,
    wrap: impl FnOnce(File) -> W,
) -> Result<(), CacheError> {
    save_inner(path, cache, wrap, &mut |_| {})
}

fn save_inner<W: Write>(
    path: &Path,
    cache: &CacheFile,
    wrap: impl FnOnce(File) -> W,
    step: &mut dyn FnMut(SaveStep),
) -> Result<(), CacheError> {
    let bytes = cache.to_bytes()?;
    let tmp = tmp_path(path);
    let result = write_tmp(&tmp, &bytes, wrap, step).and_then(|()| install(path, &tmp, step));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result?;
    sync_parent(path);
    Ok(())
}

fn write_tmp<W: Write>(
    tmp: &Path,
    bytes: &[u8],
    wrap: impl FnOnce(File) -> W,
    step: &mut dyn FnMut(SaveStep),
) -> Result<(), CacheError> {
    match fs::remove_file(tmp) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let file = OpenOptions::new().write(true).create_new(true).open(tmp)?;
    let mut w = wrap(file);
    w.write_all(bytes)?;
    w.flush()?;
    drop(w);
    step(SaveStep::TmpWritten);
    // Reopen to fsync: `wrap` consumed the handle, and syncing needs only the inode.
    File::open(tmp)?.sync_all()?;
    step(SaveStep::TmpSynced);
    Ok(())
}

fn install(path: &Path, tmp: &Path, step: &mut dyn FnMut(SaveStep)) -> Result<(), CacheError> {
    if path.exists() {
        let backup = backup_path(path);
        match fs::remove_file(&backup) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        if fs::hard_link(path, &backup).is_err() {
            fs::copy(path, &backup)?;
            File::open(&backup)?.sync_all()?;
        }
    }
    step(SaveStep::BackedUp);
    fs::rename(tmp, path)?;
    step(SaveStep::Renamed);
    Ok(())
}

/// `fsync` the directory so the rename survives a crash. Best effort: some file systems
/// refuse to open or sync a directory, and the data itself is already synced.
fn sync_parent(path: &Path) {
    let dir = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    if let Ok(d) = File::open(dir) {
        let _ = d.sync_all();
    }
}
