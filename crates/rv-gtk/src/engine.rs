//! The one place `rv-gtk` calls the engine. Every function here is a call into the library
//! crates (`rv-cache`, `rv-dat`, `rv-scan`) plus the path guard of `rv-core`; there is no
//! scanning, hashing or database logic of its own. The windows never call the libraries
//! directly, so this file is what a reviewer reads to see what the interface can make the
//! engine do.
//!
//! Find Fixes and Fix are not here: they are `rv-fix` (Phase 5b), which is not merged yet. The
//! window shows both buttons switched off with a note (see [`FIX_PENDING`]).

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rv_cache::{
    load_database, save_database, DatabaseError, DatabaseOptions, LoadSource, SaveError,
};
use rv_core::guard;
use rv_core::progress::{CancelFlag, ProgressSink};
use rv_core::{Database, ScanLevel, Settings};
use rv_dat::update::{update_dats, UpdateError, UpdateOptions, UpdateReport};
use rv_scan::db::{scan_files, ScanError, ScanReport, ScanRequest, ScanTarget};

/// Why Find Fixes and Fix are switched off.
pub const FIX_PENDING: &str =
    "Needs the rv-fix library (Phase 5b, DAN-1791), which is not merged yet";

/// The name of the settings file when none is given.
pub const DEFAULT_CONFIG: &str = "RomVault3cfg.xml";

/// What can go wrong before or while an operation runs.
#[derive(Debug)]
pub enum EngineError {
    /// The settings file exists and cannot be read or parsed.
    Settings(String),
    /// The cache cannot be read.
    Cache(DatabaseError),
    /// The cache cannot be written.
    Save(SaveError),
    /// Settings that point into a fenced-off directory (see [`rv_core::guard`]).
    Protected(Vec<guard::Violation>),
    Update(UpdateError),
    Scan(ScanError),
    /// The settings file cannot be written.
    Write(String),
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EngineError::Settings(e) => write!(f, "{e}"),
            EngineError::Cache(e) => write!(f, "cache: {e}"),
            EngineError::Save(e) => write!(f, "saving the cache: {e}"),
            EngineError::Protected(v) => {
                write!(f, "refusing to work in a protected directory:")?;
                for violation in v {
                    write!(f, "\n  {violation}")?;
                }
                Ok(())
            }
            EngineError::Update(e) => write!(f, "{e}"),
            EngineError::Scan(e) => write!(f, "{e}"),
            EngineError::Write(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for EngineError {}

/// Read the settings file; a missing file gives ROMVault's first-run defaults. Relative paths in
/// the result stay relative: they are taken from the directory the process runs in, like
/// ROMVault's.
pub fn read_settings(path: &Path) -> Result<Settings, EngineError> {
    match std::fs::read_to_string(path) {
        Ok(text) => Settings::from_xml(&text).map_err(|e| EngineError::Settings(e.to_string())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
        Err(e) => Err(EngineError::Settings(format!("{}: {e}", path.display()))),
    }
}

/// Write the settings file, keeping the parts of an existing file that the settings do not
/// model. Goes through a temp file and a rename, so a failure leaves the old file.
pub fn write_settings(path: &Path, settings: &Settings) -> Result<(), EngineError> {
    let old = std::fs::read_to_string(path).ok();
    let text = settings.to_xml(old.as_deref());
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let werr = |e: std::io::Error| EngineError::Write(format!("{}: {e}", path.display()));
    std::fs::write(&tmp, text).map_err(werr)?;
    std::fs::rename(&tmp, path).map_err(werr)
}

/// The settings that a window can show or change: where the files are.
pub fn check_paths(settings: &Settings, allow_protected: bool) -> Result<(), EngineError> {
    if allow_protected {
        return Ok(());
    }
    let base = std::env::current_dir().unwrap_or_default();
    let found = guard::check_settings(settings, &base);
    if found.is_empty() {
        Ok(())
    } else {
        Err(EngineError::Protected(found))
    }
}

/// The cache file path for these settings.
pub fn cache_path(settings: &Settings) -> PathBuf {
    PathBuf::from(&settings.cache_file)
}

/// A database that was loaded, with what the load did.
pub struct Opened {
    pub db: Database,
    pub source: LoadSource,
    pub took: Duration,
    /// There was no cache file, so this is a new empty database (ROMVault's first run).
    pub created: bool,
}

impl Opened {
    /// Words for the window when the cache file itself could not be used and its backup was
    /// loaded instead. The next save would replace the unusable file with the backup's state,
    /// so the user is told before that happens.
    pub fn fallback_notice(&self) -> Option<String> {
        match &self.source {
            LoadSource::Primary => None,
            LoadSource::Backup { primary_error } => Some(format!(
                "The cache file could not be used ({primary_error}), so its backup was loaded \
                 instead. The next scan or update will overwrite the cache file with this state."
            )),
        }
    }
}

/// `DB.Read`: the cache of `settings`. Refuses a protected path first.
pub fn open_cache(settings: &Settings, allow_protected: bool) -> Result<Opened, EngineError> {
    check_paths(settings, allow_protected)?;
    let start = Instant::now();
    let created = matches!(cache_path(settings).try_exists(), Ok(false));
    let (db, source) = load_database(&cache_path(settings), settings, DatabaseOptions::default())
        .map_err(EngineError::Cache)?;
    Ok(Opened {
        db,
        source,
        took: start.elapsed(),
        created,
    })
}

/// `DB.Write`.
pub fn save_cache(
    settings: &Settings,
    db: &mut Database,
    allow_protected: bool,
) -> Result<(), EngineError> {
    check_paths(settings, allow_protected)?;
    save_database(&cache_path(settings), db).map_err(EngineError::Save)
}

/// What the user asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Task {
    /// "Update DATs", with `all` set for the "Update All DATs" entry that reads every DAT again.
    UpdateDats { all: bool },
    /// "Scan ROMs" at a level, on the ticked rows or on one row.
    Scan {
        level: ScanLevel,
        target: ScanTarget,
    },
}

impl Task {
    pub fn title(&self) -> String {
        match self {
            Task::UpdateDats { all: false } => "Updating DATs".to_owned(),
            Task::UpdateDats { all: true } => "Updating all DATs".to_owned(),
            Task::Scan { level, target } => {
                let what = match level {
                    ScanLevel::Level1 => "Scanning (headers only)",
                    ScanLevel::Level2 => "Scanning",
                    ScanLevel::Level3 => "Scanning (full re-scan)",
                };
                match target {
                    ScanTarget::Selected => what.to_owned(),
                    ScanTarget::Dir(dir) => format!("{what}: {dir}"),
                }
            }
        }
    }
}

/// What a task did, for the status line.
#[derive(Clone, Debug)]
pub enum Outcome {
    Updated(UpdateReport),
    Scanned(ScanReport),
}

impl Outcome {
    pub fn cancelled(&self) -> bool {
        match self {
            Outcome::Updated(r) => r.cancelled,
            Outcome::Scanned(r) => r.cancelled,
        }
    }

    pub fn summary(&self) -> String {
        match self {
            Outcome::Updated(r) => format!(
                "{} DATs found, {} added, {} removed, {} unchanged, {} failed{}",
                r.dats_found,
                r.dats_added.len(),
                r.dats_removed.len(),
                r.dats_unchanged.len(),
                r.dats_failed.len(),
                if r.cancelled { " (cancelled)" } else { "" }
            ),
            Outcome::Scanned(r) => format!(
                "{} directories, {} archives read, {} files hashed, {} problems{}{}",
                r.dirs_scanned,
                r.archives_scanned,
                r.files_hashed,
                r.errors.len(),
                if r.aborted {
                    " (stopped: a file changed under the scan)"
                } else {
                    ""
                },
                if r.cancelled { " (cancelled)" } else { "" }
            ),
        }
    }
}

/// Run a task on `db`, then save the cache (the library functions never write it). The check of
/// the paths is repeated here, so no caller can start an operation in a protected directory.
pub fn run_task(
    db: &mut Database,
    settings: &Settings,
    task: &Task,
    allow_protected: bool,
    progress: &dyn ProgressSink,
    cancel: &CancelFlag,
) -> Result<Outcome, EngineError> {
    check_paths(settings, allow_protected)?;
    let outcome = match task {
        Task::UpdateDats { all } => {
            let options = UpdateOptions {
                invalidate: all.then(|| "DatRoot".to_owned()),
            };
            Outcome::Updated(
                update_dats(db, settings, &options, progress, cancel)
                    .map_err(EngineError::Update)?,
            )
        }
        Task::Scan { level, target } => {
            let request = ScanRequest {
                level: *level,
                target: target.clone(),
            };
            Outcome::Scanned(
                scan_files(db, settings, &request, progress, cancel).map_err(EngineError::Scan)?,
            )
        }
    };
    save_cache(settings, db, allow_protected)?;
    Ok(outcome)
}
