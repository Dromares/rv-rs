//! The one place `rv-gtk` calls the engine. Every function here is a call into the library
//! crates (`rv-cache`, `rv-dat`, `rv-scan`) plus the path guard of `rv-core`; there is no
//! scanning, hashing or database logic of its own. The windows never call the libraries
//! directly, so this file is what a reviewer reads to see what the interface can make the
//! engine do.
//!
//! Find Fixes and Fix are [`Task::FindFixes`] and [`Task::Fix`]: they call `rv_fix::find_fixes`
//! and `rv_fix::perform_fixes` and nothing else. Every file is written, synced and renamed by
//! `rv-fix`; this crate has no file-writing code for them.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rv_cache::{
    load_database, save_database, DatabaseError, DatabaseOptions, LoadSource, SaveError,
};
use rv_core::guard::{self, PROTECTED_PREFIXES};
use rv_core::progress::{CancelFlag, ProgressSink};
use rv_core::{Database, ScanLevel, Settings};
use rv_dat::update::{update_dats, UpdateError, UpdateOptions, UpdateReport};
use rv_fix::plan::{self, Action, FixPlan};
use rv_fix::{find_fixes, perform_fixes, FindReport, FixOptions, FixReport};
use rv_online::config::OnlineConfig;
use rv_online::datvault::{FetchOptions, Vault};
use rv_online::mia::{self, ApplyOptions, Prepared};
use rv_online::salt::configured_source;
use rv_online::Client;
use rv_scan::db::{scan_files, ScanError, ScanReport, ScanRequest, ScanTarget};
use rv_update::{Installed, Product, Update, UpdateConfig};

use crate::prefs::Prefs;

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
            EngineError::Protected(v) => write!(
                f,
                "{}\nChange the paths in Settings, or to use them on purpose tick {}.",
                protected_list(v),
                safety_switch_location()
            ),
            EngineError::Update(e) => write!(f, "{e}"),
            EngineError::Scan(e) => write!(f, "{e}"),
            EngineError::Online(e) => write!(f, "{e}"),
            EngineError::Write(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for EngineError {}

/// The caption of the switch that lifts the path guard. The prefixes come from
/// [`PROTECTED_PREFIXES`], so no message types them out again.
pub fn safety_switch_label() -> String {
    format!("Allow paths under {}", PROTECTED_PREFIXES.join(", "))
}

/// Where the switch is, as every refusal says it.
pub fn safety_switch_location() -> String {
    format!(
        "Settings \u{2192} General \u{2192} Safety \u{2192} \"{}\"",
        safety_switch_label()
    )
}

/// The refusal without the way out: the heading and one line per setting.
pub fn protected_list(violations: &[guard::Violation]) -> String {
    let mut text = "refusing to work in a protected directory:".to_owned();
    for violation in violations {
        text.push_str(&format!("\n  {violation}"));
    }
    text
}

/// What the settings dialog's OK does with its result: refuse a protected path unless the guard
/// is lifted, write the settings file, then the preferences. A refusal writes nothing.
pub fn commit_settings(
    config_path: &Path,
    prefs_path: &Path,
    settings: &Settings,
    prefs: &Prefs,
) -> Result<(), EngineError> {
    check_paths(settings, prefs.allow_protected_paths)?;
    write_settings(config_path, settings)?;
    if let Err(e) = prefs.save(prefs_path) {
        eprintln!("rv-gtk: preferences not saved: {e}");
    }
    Ok(())
}

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
    /// "Find Fixes": give every file its fix status and work out the plan. Nothing on disk
    /// changes and the cache is not written. `review` is the Fix button's first step: the window
    /// puts the plan in front of the user before anything is fixed.
    FindFixes { review: bool },
    /// "Fix Files", after the user confirmed a plan: Find Fixes again on the database as it is
    /// now, then `rv_fix::perform_fixes`.
    Fix,
}

impl Task {
    /// Does the run leave the cache saved? Find Fixes only sets statuses in memory; the window
    /// keeps its own "unsaved tick boxes" mark after it.
    pub fn saves_cache(&self) -> bool {
        !matches!(self, Task::FindFixes { .. })
    }

    pub fn title(&self) -> String {
        match self {
            Task::UpdateDats { all: false } => "Updating DATs".to_owned(),
            Task::UpdateDats { all: true } => "Updating all DATs".to_owned(),
            Task::DatVaultUpdate { .. } => "Updating DATs from DatVault".to_owned(),
            Task::UpdateMia { .. } => "Updating MIA".to_owned(),
            Task::FindFixes { .. } => "Finding fixes".to_owned(),
            Task::Fix => "Fixing files".to_owned(),
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
#[derive(Debug)]
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
    FoundFixes {
        report: FindReport,
        plan: FixPlan,
        /// The Fix button asked for this run, to show the plan before fixing.
        review: bool,
    },
    Fixed(FixReport),
}

impl Outcome {
    pub fn cancelled(&self) -> bool {
        match self {
            Outcome::Updated(r) => r.cancelled,
            Outcome::Scanned(r) => r.cancelled,
            Outcome::DatVault { cancelled, .. } => *cancelled,
            Outcome::Mia { .. } => false,
            Outcome::FoundFixes { report, .. } => report.cancelled,
            Outcome::Fixed(r) => r.cancelled,
        }
    }

    /// What went wrong in a run that otherwise finished: the error that stopped a fix, or a
    /// cache that could not be saved. The window shows it where the user cannot miss it.
    pub fn problem(&self) -> Option<String> {
        let Outcome::Fixed(r) = self else { return None };
        let mut parts = Vec::new();
        if let Some(e) = &r.error {
            parts.push(format!("The fix stopped: {e}"));
        }
        if let Some(e) = &r.save_error {
            parts.push(format!("The cache could not be saved: {e}"));
        }
        (!parts.is_empty()).then(|| parts.join("\n"))
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
            Outcome::FoundFixes { report, plan, .. } => {
                if report.cancelled {
                    "Find Fixes (cancelled)".to_owned()
                } else {
                    format!(
                        "Find Fixes: {}; {} nothing can make",
                        PlanSummary::of(plan).one_line(),
                        noun(plan.still_missing, "wanted file")
                    )
                }
            }
            Outcome::Fixed(r) => format!(
                "Fix: {} made, {} moved, {} deleted, {} left alone{}{}",
                noun(r.fixed as usize, "missing file"),
                r.count(rv_fix::action::TO_SORT)
                    + r.count(rv_fix::action::TO_CORRUPT)
                    + r.count(rv_fix::action::ARCHIVE_TO_SORT)
                    + r.count(rv_fix::action::ARCHIVE_TO_CORRUPT),
                r.count(rv_fix::action::DELETE),
                r.skipped.len(),
                if r.error.is_some() { " (stopped)" } else { "" },
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

/// Run a task on `db`, then save the cache (the library functions never write it; Find Fixes
/// changes nothing on disk and Fix saves through `rv-fix`). The check of the paths is repeated
/// here, so no caller can start an operation in a protected directory, Fix included.
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
        Task::FindFixes { review } => {
            let found = find_fixes(db, settings, progress, cancel);
            Outcome::FoundFixes {
                plan: plan::plan(&db.root, settings),
                report: found.report,
                review: *review,
            }
        }
        Task::Fix => return fix(db, settings, allow_protected, progress, cancel),
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
    if !matches!(task, Task::FindFixes { .. }) {
        save_cache(settings, db, allow_protected)?;
    }
    Ok(outcome)
}

/// Find Fixes, then Fix. The files are written by [`perform_fixes`] (temp file, fsync, rename),
/// which also saves the cache through the closure below, between items and at the end, whatever
/// happened. Cancel is a normal end.
fn fix(
    db: &mut Database,
    settings: &Settings,
    allow_protected: bool,
    progress: &dyn ProgressSink,
    cancel: &CancelFlag,
) -> Result<Outcome, EngineError> {
    let found = find_fixes(db, settings, progress, cancel);
    if found.report.cancelled {
        return Ok(Outcome::Fixed(FixReport {
            cancelled: true,
            ..FixReport::default()
        }));
    }
    let options = FixOptions {
        save_every: settings
            .cache_save_timer_enabled
            .then(|| Duration::from_secs(60 * settings.cache_save_time_period.max(1) as u64)),
        ..FixOptions::default()
    };
    let mut saver =
        |db: &mut Database| save_cache(settings, db, allow_protected).map_err(|e| e.to_string());
    Ok(Outcome::Fixed(perform_fixes(
        db,
        settings,
        found.groups,
        &options,
        progress,
        cancel,
        &mut saver,
    )))
}

/// What a plan will do, in the three words the confirmation uses (written, moved, deleted) and
/// the rest. Counts files, archive entries and archives as the plan lists them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PlanSummary {
    /// Missing files made: loose files and entries of rebuilt archives.
    pub written: usize,
    /// Archives written whole again.
    pub rebuilt: usize,
    /// Files, entries and archives that go to ToSort or ToSort/Corrupt.
    pub moved: usize,
    /// Files and archive entries removed, each after a copy is proven.
    pub deleted: usize,
    /// Names that change case only.
    pub renamed: usize,
    /// Archives Fix cannot rewrite or has to wait for.
    pub left: usize,
}

impl PlanSummary {
    pub fn of(plan: &FixPlan) -> PlanSummary {
        let n = |a| plan.count(a);
        PlanSummary {
            written: n(Action::MakeFile) + n(Action::MakeMember),
            rebuilt: n(Action::RebuildArchive),
            moved: n(Action::MoveFileToSort)
                + n(Action::MoveFileToCorrupt)
                + n(Action::MoveArchiveToSort)
                + n(Action::MoveArchiveToCorrupt)
                + n(Action::MoveMemberToSort)
                + n(Action::MoveMemberToCorrupt),
            deleted: n(Action::DeleteFile) + n(Action::DropMember),
            renamed: n(Action::RenameFileCase) + n(Action::RenameArchiveCase),
            left: n(Action::Unsupported) + n(Action::DeferredArchive),
        }
    }

    /// Nothing to write, move, delete, rebuild or rename.
    pub fn is_empty(&self) -> bool {
        self.written + self.rebuilt + self.moved + self.deleted + self.renamed == 0
    }

    pub fn one_line(&self) -> String {
        if self.is_empty() {
            return "nothing to fix".to_owned();
        }
        format!(
            "{} to write, {} to move, {} to delete",
            noun(self.written, "file"),
            self.moved,
            self.deleted
        )
    }

    /// The words of the confirmation dialog.
    pub fn question(&self, still_missing: usize) -> (String, String) {
        let mut detail = format!(
            "Fix will write {} (a file with the right content is copied or moved into place), \
             move {} to ToSort (or ToSort/Corrupt) and delete {}. A file is deleted only after \
             its replacement is written, synced and checked.\n",
            noun(self.written, "file"),
            noun(self.moved, "file or archive"),
            noun(self.deleted, "file or archive entry"),
        );
        let mut more = Vec::new();
        if self.rebuilt > 0 {
            more.push(format!(
                "{} written again as TorrentZips",
                noun(self.rebuilt, "archive")
            ));
        }
        if self.renamed > 0 {
            more.push(format!("{} change case", noun(self.renamed, "name")));
        }
        if self.left > 0 {
            more.push(format!(
                "{} left as they are (Fix cannot rewrite them, or they wait for a file)",
                noun(self.left, "archive")
            ));
        }
        more.push(format!(
            "{} nothing can make",
            noun(still_missing, "wanted file")
        ));
        for line in more {
            detail.push_str(&format!("\n{line}"));
        }
        ("Fix the ticked directories?".to_owned(), detail)
    }
}

/// "1 file", "2 files": `one` is the singular.
fn noun(n: usize, one: &str) -> String {
    match (n, one) {
        (1, _) => format!("1 {one}"),
        (_, "file or archive entry") => format!("{n} files or archive entries"),
        (_, "file or archive") => format!("{n} files or archives"),
        _ => format!("{n} {one}s"),
    }
}

/// The dry-run view: the counts, then every item of the plan (the first `limit`), as text. Read
/// from the plan only; nothing is opened or written.
pub fn plan_text(plan: &FixPlan, limit: usize) -> String {
    let mut out = String::from("Dry run: the fix would do this (nothing is written):\n\n");
    if plan.counts.is_empty() {
        out.push_str("  nothing to do\n");
    }
    for (a, n) in &plan.counts {
        out.push_str(&format!("  {n:>8}  {}\n", a.name()));
    }
    out.push_str(&format!(
        "  {:>8}  wanted {} nothing can make\n\n",
        plan.still_missing,
        if plan.still_missing == 1 {
            "file"
        } else {
            "files"
        }
    ));
    for (a, path) in plan.items.iter().take(limit) {
        out.push_str(&format!("{}: {path}\n", a.name()));
    }
    if plan.items.len() > limit {
        out.push_str(&format!(
            "... and {} more (`rv fix --dry-run --verbose` lists all of them)\n",
            plan.items.len() - limit
        ));
    }
    out
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
    install_asset(up, &asset, target)
}

fn install_asset(up: &Update, asset: &str, target: &Path) -> Result<Installed, String> {
    rv_update::install(up, asset, target, &|_| {}).map_err(|e| match e {
        rv_update::Error::NotWritable { dir, why } => format!(
            "You cannot write in {} ({why}), so the update was not installed, and rv-gtk does not ask for more rights. Version {} is at {}: download it, check it against SHA256SUMS and copy it into place yourself.",
            dir.display(),
            up.version,
            up.page
        ),
        rv_update::Error::NoAsset { .. } if asset == rv_update::appimage::ASSET => {
            no_appimage(up)
        }
        other => format!("The update was not installed: {other}"),
    })
}

/// What "Check for updates" replaces: the running binary, or the AppImage it runs from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateTarget {
    /// A plain `rv-gtk` binary: `current_exe()`, replaced by `rv-gtk-linux-x86_64`.
    Binary(PathBuf),
    /// The AppImage file named by `$APPIMAGE` (`current_exe()` is inside its read-only mount),
    /// replaced by `rv-gtk-x86_64.AppImage`, never by the bare binary.
    AppImage(PathBuf),
}

impl UpdateTarget {
    pub fn path(&self) -> &Path {
        match self {
            UpdateTarget::Binary(p) | UpdateTarget::AppImage(p) => p,
        }
    }
}

/// The file the running `rv-gtk` would replace (`$APPIMAGE` when it names an existing file).
pub fn update_target() -> Result<UpdateTarget, String> {
    update_target_from(rv_update::appimage::running(), std::env::current_exe())
}

/// [`update_target`] with the environment and `current_exe()` given.
pub fn update_target_from(
    appimage: Option<PathBuf>,
    exe: std::io::Result<PathBuf>,
) -> Result<UpdateTarget, String> {
    match appimage {
        Some(p) => Ok(UpdateTarget::AppImage(p)),
        None => exe
            .map(UpdateTarget::Binary)
            .map_err(|e| format!("Cannot tell where rv-gtk is: {e}")),
    }
}

/// The question put to the user for an [`UpdateTarget`].
pub struct UpdateQuestion {
    pub message: String,
    pub detail: String,
    /// False when there is nothing to install: the dialog then only informs.
    pub can_install: bool,
}

fn no_appimage(up: &Update) -> String {
    format!(
        "There is no AppImage in release {}, so nothing was installed: the plain rv-gtk file is never put over an AppImage. Release page: {}",
        up.tag, up.page
    )
}

/// [`update_question`] for either kind of target. An AppImage whose new release has no AppImage
/// is told so, with the release page, and is not offered an install.
pub fn update_question_for(up: &Update, target: &UpdateTarget) -> UpdateQuestion {
    match target {
        UpdateTarget::Binary(p) => {
            let (message, detail) = update_question(up, p);
            UpdateQuestion {
                message,
                detail,
                can_install: true,
            }
        }
        UpdateTarget::AppImage(_) if up.asset(rv_update::appimage::ASSET).is_none() => {
            UpdateQuestion {
                message: format!(
                    "Version {} is available, but not as an AppImage",
                    up.version
                ),
                detail: no_appimage(up),
                can_install: false,
            }
        }
        UpdateTarget::AppImage(p) => UpdateQuestion {
            message: format!("Version {} is available", up.version),
            detail: format!(
                "You have {}. If you go on, rv-gtk downloads {} and SHA256SUMS from release {}, \
                 refuses to continue unless the file matches SHA256SUMS, and replaces the \
                 AppImage {}. The old AppImage is kept as {}.\n\nRelease page: {}",
                rv_update::current_version(),
                rv_update::appimage::ASSET,
                up.tag,
                p.display(),
                rv_update::install::backup_path(p).display(),
                up.page
            ),
            can_install: true,
        },
    }
}

/// [`install_update`] for either kind of target.
pub fn install_update_for(up: &Update, target: &UpdateTarget) -> Result<Installed, String> {
    match target {
        UpdateTarget::Binary(p) => install_update(up, p),
        UpdateTarget::AppImage(p) => match Product::RvGtk.appimage_asset_name() {
            Some(asset) => install_asset(up, asset, p),
            None => Err(no_appimage(up)),
        },
    }
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
