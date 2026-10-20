//! The one place `rv-gtk` calls the engine. Every function here is a call into the library
//! crates (`rv-cache`, `rv-dat`, `rv-scan`) plus the path guard of `rv-core`; there is no
//! scanning, hashing or database logic of its own. The windows never call the libraries
//! directly, so this file is what a reviewer reads to see what the interface can make the
//! engine do.
//!
//! Find Fixes and Fix are not here: the window is not wired to `rv-fix` yet. It shows both
//! buttons switched off with a note (see [`FIX_PENDING`]).

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
use rv_online::config::OnlineConfig;
use rv_online::datvault::{FetchOptions, Vault};
use rv_online::mia::{self, ApplyOptions, Prepared};
use rv_online::salt::configured_source;
use rv_online::Client;
use rv_scan::db::{scan_files, ScanError, ScanReport, ScanRequest, ScanTarget};
use rv_update::{Installed, Product, Update, UpdateConfig};

/// Why Find Fixes and Fix are switched off.
pub const FIX_PENDING: &str =
    "Off until this window is wired to rv-fix. `rv find-fixes` and `rv fix` work from the command line";

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
    /// DatVault or MIA: the words of the online library, which never contain the key.
    Online(String),
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
            EngineError::Online(e) => write!(f, "{e}"),
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
    /// "Update DATs from DatVault": the DATs that are here and have a newer version, then the
    /// normal DAT update. The settings are read from the file when the button is pressed.
    DatVaultUpdate { online: OnlineConfig },
    /// "Update MIA": fetch the list and mark the files; `force` also when it has not changed.
    UpdateMia { force: bool, online: OnlineConfig },
}

impl Task {
    pub fn title(&self) -> String {
        match self {
            Task::UpdateDats { all: false } => "Updating DATs".to_owned(),
            Task::UpdateDats { all: true } => "Updating all DATs".to_owned(),
            Task::DatVaultUpdate { .. } => "Updating DATs from DatVault".to_owned(),
            Task::UpdateMia { .. } => "Updating MIA".to_owned(),
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
    DatVault {
        downloaded: usize,
        removed: usize,
        failed: Vec<(String, String)>,
        cancelled: bool,
        /// The DAT update that follows a download.
        updated: Option<UpdateReport>,
    },
    Mia {
        release: String,
        /// `(files checked, files on the list)`; none when the list had not changed.
        applied: Option<(usize, usize)>,
    },
}

impl Outcome {
    pub fn cancelled(&self) -> bool {
        match self {
            Outcome::Updated(r) => r.cancelled,
            Outcome::Scanned(r) => r.cancelled,
            Outcome::DatVault { cancelled, .. } => *cancelled,
            Outcome::Mia { .. } => false,
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
            Outcome::DatVault {
                downloaded,
                removed,
                failed,
                cancelled,
                updated,
            } => format!(
                "DatVault: {downloaded} DATs downloaded, {removed} removed, {} failed{}{}",
                failed.len(),
                updated
                    .as_ref()
                    .map(|r| format!(
                        "; {} DATs added, {} removed in the database",
                        r.dats_added.len(),
                        r.dats_removed.len()
                    ))
                    .unwrap_or_default(),
                if *cancelled { " (cancelled)" } else { "" }
            ),
            Outcome::Mia { release, applied } => match applied {
                Some((files, found)) => {
                    format!("MIA release {release}: {files} files checked, {found} on the list")
                }
                None => format!("MIA: nothing changed, release {release}"),
            },
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
        Task::DatVaultUpdate { online } => datvault_update(db, settings, online, progress, cancel)?,
        Task::UpdateMia { force, online } => mia_update(db, online, *force, progress)?,
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

/// "Check key": the words for the status line or the settings dialog. A network problem is
/// not "invalid", and nothing here holds the key.
pub fn check_key(online: &OnlineConfig) -> String {
    if !online.has_key() {
        return "Enter a DatVault key first.".to_owned();
    }
    let result = client_for(online).and_then(|client| {
        Vault::new(&client, &online.dat_vault_key)
            .and_then(|v| v.check_key())
            .map_err(online_err)
    });
    match result {
        Ok(true) => "The DatVault key is valid.".to_owned(),
        Ok(false) => "The DatVault key is not valid.".to_owned(),
        Err(e) => format!("The key could not be checked: {e}"),
    }
}

/// Look for a newer release (the "Check for updates" menu item and the check at start). Says
/// nothing about installing: that is [`install_update`], after the user confirmed.
pub fn check_for_updates(update_conf: &Path) -> Result<rv_update::Check, String> {
    let cfg = UpdateConfig::load(update_conf);
    rv_update::check(&cfg, &rv_update::current_version()).map_err(|e| e.to_string())
}

/// Does the update setting ask for a check when the window opens?
pub fn check_on_startup(update_conf: &Path) -> bool {
    UpdateConfig::load(update_conf).check_on_startup
}

/// The words of the question put to the user before anything is downloaded.
pub fn update_question(up: &Update, target: &Path) -> (String, String) {
    (
        format!("Version {} is available", up.version),
        format!(
            "You have {}. If you go on, rv-gtk downloads {} and SHA256SUMS from release {}, \
             refuses to continue unless the file matches SHA256SUMS, and replaces {}. The old \
             program is kept as {}.\n\nRelease page: {}",
            rv_update::current_version(),
            Product::RvGtk
                .asset_name()
                .unwrap_or_else(|| "the file for this system".to_owned()),
            up.tag,
            target.display(),
            rv_update::install::backup_path(target).display(),
            up.page
        ),
    )
}

/// Download, check and install the release over `target` (the running `rv-gtk`). Only called
/// after the user said yes.
pub fn install_update(up: &Update, target: &Path) -> Result<Installed, String> {
    let Some(asset) = Product::RvGtk.asset_name() else {
        return Err(format!(
            "There is no build of the release for this system. Download it from {}",
            up.page
        ));
    };
    rv_update::install(up, &asset, target, &|_| {}).map_err(|e| match e {
        rv_update::Error::NotWritable { dir, why } => format!(
            "You cannot write in {} ({why}), so the update was not installed, and rv-gtk does not ask for more rights. Version {} is at {}: download it, check it against SHA256SUMS and copy it into place yourself.",
            dir.display(),
            up.version,
            up.page
        ),
        other => format!("The update was not installed: {other}"),
    })
}

fn online_err(e: rv_online::Error) -> EngineError {
    EngineError::Online(e.to_string())
}

fn client_for(online: &OnlineConfig) -> Result<Client, EngineError> {
    Ok(Client::new(online.endpoint().map_err(online_err)?))
}

/// The DATs of the groups that are already in the DAT root, newer versions only (nothing new is
/// added and nothing is removed: those are choices the command line offers), then the DAT update
/// when anything was downloaded.
fn datvault_update(
    db: &mut Database,
    settings: &Settings,
    online: &OnlineConfig,
    progress: &dyn ProgressSink,
    cancel: &CancelFlag,
) -> Result<Outcome, EngineError> {
    let client = client_for(online)?;
    let vault = Vault::new(&client, &online.dat_vault_key).map_err(online_err)?;
    let root = PathBuf::from(&settings.dat_root);
    let mut plan = vault.plan(&root, &[], progress).map_err(online_err)?;
    let cues_dir = settings
        .dir_mappings
        .iter()
        .find(|m| m.dir_key == "ToSort" && !m.dir_path.trim().is_empty())
        .map(|m| PathBuf::from(&m.dir_path).join("cues"));
    let opts = FetchOptions {
        cues_dir,
        delete_old_cue_files: online.delete_old_cue_files,
        remove_discontinued: false,
    };
    let fetched = vault.fetch(&root, &mut plan, &[], &opts, progress, cancel);
    let updated = if fetched.downloaded.is_empty() || cancel.is_cancelled() {
        None
    } else {
        Some(
            update_dats(db, settings, &UpdateOptions::default(), progress, cancel)
                .map_err(EngineError::Update)?,
        )
    };
    Ok(Outcome::DatVault {
        downloaded: fetched.downloaded.len(),
        removed: fetched.removed.len(),
        failed: fetched.failed,
        cancelled: fetched.cancelled,
        updated,
    })
}

fn mia_update(
    db: &mut Database,
    online: &OnlineConfig,
    force: bool,
    progress: &dyn ProgressSink,
) -> Result<Outcome, EngineError> {
    let client = client_for(online)?;
    progress.report(rv_core::progress::ProgressEvent::Text(
        "Fetching MIA Cache".into(),
    ));
    match mia::prepare(&client, online, &configured_source(), force).map_err(online_err)? {
        Prepared::Unchanged { release } => Ok(Outcome::Mia {
            release,
            applied: None,
        }),
        Prepared::Ready { list, release, .. } => {
            progress.report(rv_core::progress::ProgressEvent::Text(
                "Updating MIA".into(),
            ));
            let a = mia::apply_mia(
                &mut db.root,
                &list,
                &ApplyOptions {
                    mia_days: online.mia_days,
                    force_update: force,
                    now_ticks: mia::now_ticks(),
                },
            );
            Ok(Outcome::Mia {
                release,
                applied: Some((a.files, a.found)),
            })
        }
    }
}
