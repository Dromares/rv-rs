//! The commands.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rv_cache::{
    load_database, save_database, save_database_stepped, DatabaseOptions, LoadSource, SaveStep,
};
use rv_core::progress::{CancelFlag, ProgressEvent};
use rv_core::{Database, DirMapping, ScanLevel, Settings};
use rv_dat::update::{update_dats, UpdateOptions};
use rv_fix::{fixdat, perform_fixes, plan, Failpoints, FixOptions};
use rv_scan::db::{scan_files, ScanRequest, ScanTarget};

use crate::args::Args;
use crate::exit;
use crate::guard::{self, lexical_absolute};

struct Ctx<'a> {
    args: &'a Args,
    settings: Settings,
    cache: PathBuf,
    cancel: CancelFlag,
    out: &'a mut dyn Write,
    err: &'a mut dyn Write,
}

/// The settings for this run: the config file if there is one, with the directories on the
/// command line put in place of whatever the config says for them.
fn settings_for(args: &Args) -> Result<Settings, String> {
    let mut s = match &args.config {
        Some(p) => {
            let text = std::fs::read_to_string(p)
                .map_err(|e| format!("cannot read {}: {e}", p.display()))?;
            Settings::from_xml(&text).map_err(|e| e.to_string())?
        }
        None => Settings::default(),
    };
    let root = lexical_absolute(args.root.as_deref().expect("parse checked --root"));
    let tosort = match &args.tosort {
        Some(t) => lexical_absolute(t),
        None => root.with_file_name("ToSort"),
    };
    set_mapping(&mut s, "RomVault", &root);
    set_mapping(&mut s, "ToSort", &tosort);
    if let Some(d) = &args.dat_root {
        s.dat_root = lexical_absolute(d).to_string_lossy().into_owned();
    }
    Ok(s)
}

fn set_mapping(s: &mut Settings, key: &str, path: &Path) {
    let p = path.to_string_lossy().into_owned();
    match s.dir_mappings.iter_mut().find(|m| m.dir_key == key) {
        Some(m) => m.dir_path = p,
        None => s.dir_mappings.push(DirMapping {
            dir_key: key.to_owned(),
            dir_path: p,
        }),
    }
}

/// Every path this run is configured with, for the live library check.
fn configured_paths(args: &Args, s: &Settings) -> Vec<(String, PathBuf)> {
    let mut v: Vec<(String, PathBuf)> = Vec::new();
    let mut add = |label: &str, p: &Path| v.push((label.to_owned(), p.to_path_buf()));
    if let Some(p) = &args.cache {
        add("--cache", p);
    }
    if let Some(p) = &args.root {
        add("--root", p);
    }
    if let Some(p) = &args.tosort {
        add("--tosort", p);
    }
    if let Some(p) = &args.dat_root {
        add("--dat-root", p);
    }
    if let Some(p) = &args.config {
        add("--config", p);
    }
    if let Some(p) = &args.out {
        add("--out", p);
    }
    add("DatRoot", Path::new(&s.dat_root));
    add("CacheFile", Path::new(&s.cache_file));
    for m in &s.dir_mappings {
        if !m.dir_path.trim().is_empty() {
            v.push((
                format!("directory mapping {}", m.dir_key),
                PathBuf::from(&m.dir_path),
            ));
        }
    }
    v
}

pub fn execute(args: &Args, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    if args.command == "datvault" {
        return crate::online::datvault(args, out, err);
    }
    // Before anything is read: the paths, the live library.
    let settings = match settings_for(args) {
        Ok(s) => s,
        Err(e) => {
            let _ = writeln!(err, "rv: {e}");
            return exit::FAILED;
        }
    };
    if let Err(e) = guard::check(&configured_paths(args, &settings), args.allow_live_library) {
        let _ = writeln!(err, "rv: {e}");
        return exit::REFUSED;
    }

    let cancel = CancelFlag::new();
    {
        let c = cancel.clone();
        // A second Ctrl-C in a program that is not listening is not an error worth stopping for.
        let _ = ctrlc::set_handler(move || c.cancel());
    }
    let mut cx = Ctx {
        args,
        settings,
        cache: lexical_absolute(args.cache.as_deref().expect("parse checked --cache")),
        cancel,
        out,
        err,
    };
    let code = match args.command.as_str() {
        "update-dats" => update_dats_cmd(&mut cx),
        "scan" => scan_cmd(&mut cx),
        "find-fixes" => find_fixes_cmd(&mut cx),
        "fix" => fix_cmd(&mut cx),
        "report" => report_cmd(&mut cx),
        "status" => status_cmd(&mut cx),
        "mia" => mia_cmd(&mut cx),
        other => {
            let _ = writeln!(cx.err, "rv: unknown command {other}");
            exit::USAGE
        }
    };
    if code == exit::OK && cx.cancel.is_cancelled() {
        return exit::CANCELLED;
    }
    code
}

/// Progress goes to the real standard error, a line at a time.
fn progress(verbose: bool) -> impl Fn(ProgressEvent) {
    move |e| {
        let mut w = std::io::stderr().lock();
        match e {
            ProgressEvent::Text(t) => {
                let _ = writeln!(w, "{t}");
            }
            ProgressEvent::Item(t) if verbose => {
                let _ = writeln!(w, "  {t}");
            }
            ProgressEvent::Error { file, message } => {
                let _ = writeln!(w, "  {file}: {message}");
            }
            _ => {}
        }
    }
}

fn load(cx: &mut Ctx<'_>) -> Result<Database, i32> {
    match load_database(&cx.cache, &cx.settings, DatabaseOptions::default()) {
        Ok((db, source)) => {
            if !cx.cache.exists() && !rv_cache::backup_path(&cx.cache).exists() {
                let _ = writeln!(
                    cx.err,
                    "no cache at {}: starting with an empty one",
                    cx.cache.display()
                );
            }
            if let LoadSource::Backup { primary_error } = source {
                let _ = writeln!(
                    cx.err,
                    "the cache {} could not be used ({primary_error}); using its backup",
                    cx.cache.display()
                );
            }
            Ok(db)
        }
        Err(e) => {
            let _ = writeln!(cx.err, "rv: cannot load {}: {e}", cx.cache.display());
            Err(exit::FAILED)
        }
    }
}

fn save(cx: &mut Ctx<'_>, db: &mut Database) -> Result<(), i32> {
    save_database(&cx.cache, db).map_err(|e| {
        let _ = writeln!(cx.err, "rv: cannot save {}: {e}", cx.cache.display());
        exit::FAILED
    })
}

fn update_dats_cmd(cx: &mut Ctx<'_>) -> i32 {
    let mut db = match load(cx) {
        Ok(d) => d,
        Err(c) => return c,
    };
    let sink = progress(cx.args.verbose);
    let r = update_dats(
        &mut db,
        &cx.settings,
        &UpdateOptions::default(),
        &sink,
        &cx.cancel,
    );
    let r = match r {
        Ok(r) => r,
        Err(e) => {
            let _ = writeln!(cx.err, "rv: update-dats failed: {e}");
            return exit::FAILED;
        }
    };
    let _ = writeln!(
        cx.out,
        "{} DATs found: {} added, {} removed, {} unchanged, {} failed{}",
        r.dats_found,
        r.dats_added.len(),
        r.dats_removed.len(),
        r.dats_unchanged.len(),
        r.dats_failed.len(),
        if r.cancelled { " (cancelled)" } else { "" }
    );
    for (name, why) in &r.dats_failed {
        let _ = writeln!(cx.out, "  failed: {name}: {why}");
    }
    if let Err(c) = save(cx, &mut db) {
        return c;
    }
    exit::OK
}

fn scan_cmd(cx: &mut Ctx<'_>) -> i32 {
    let mut db = match load(cx) {
        Ok(d) => d,
        Err(c) => return c,
    };
    let level = match cx.args.level {
        Some(1) => ScanLevel::Level1,
        Some(3) => ScanLevel::Level3,
        _ => ScanLevel::Level2,
    };
    let req = ScanRequest {
        level,
        target: ScanTarget::Selected,
    };
    let sink = progress(cx.args.verbose);
    let r = scan_files(&mut db, &cx.settings, &req, &sink, &cx.cancel);
    let r = match r {
        Ok(r) => r,
        Err(e) => {
            let _ = writeln!(cx.err, "rv: scan failed: {e}");
            return exit::FAILED;
        }
    };
    let _ = writeln!(
        cx.out,
        "scanned {} directories: {} archives read, {} files hashed, {} problems{}{}",
        r.dirs_scanned,
        r.archives_scanned,
        r.files_hashed,
        r.errors.len(),
        if r.cancelled { " (cancelled)" } else { "" },
        if r.aborted {
            " (aborted: a file changed under the scan)"
        } else {
            ""
        }
    );
    for m in &r.roots_missing {
        let _ = writeln!(cx.out, "  no directory on disk for {m}");
    }
    if let Err(c) = save(cx, &mut db) {
        return c;
    }
    if r.aborted {
        exit::FAILED
    } else {
        exit::OK
    }
}

/// Find Fixes on the loaded database; returns the groups and prints nothing.
fn find(cx: &mut Ctx<'_>, db: &mut Database) -> rv_fix::FindFixes {
    let sink = progress(cx.args.verbose);
    rv_fix::find_fixes(db, &cx.settings, &sink, &cx.cancel)
}

fn print_plan(cx: &mut Ctx<'_>, db: &Database, report: &rv_fix::FindReport, header: &str) {
    let p = plan::plan(&db.root, &cx.settings);
    let _ = writeln!(
        cx.out,
        "{} files on disk in {} groups, {} files wanted",
        report.files_got, report.groups, report.files_missing
    );
    let _ = writeln!(cx.out, "{header}");
    if p.counts.is_empty() {
        let _ = writeln!(cx.out, "  nothing to do");
    }
    for (a, n) in &p.counts {
        let _ = writeln!(cx.out, "  {n:>8}  {}", a.name());
    }
    let _ = writeln!(
        cx.out,
        "  {:>8}  wanted files nothing can make",
        p.still_missing
    );
    if cx.args.verbose {
        for (a, path) in &p.items {
            let _ = writeln!(cx.out, "    {}: {path}", a.name());
        }
    }
}

fn find_fixes_cmd(cx: &mut Ctx<'_>) -> i32 {
    let mut db = match load(cx) {
        Ok(d) => d,
        Err(c) => return c,
    };
    let ff = find(cx, &mut db);
    for w in &ff.report.warnings {
        let _ = writeln!(cx.err, "warning: {w}");
    }
    print_plan(
        cx,
        &db,
        &ff.report,
        "Find Fixes: the fix would do (nothing was changed):",
    );
    exit::OK
}

fn fix_cmd(cx: &mut Ctx<'_>) -> i32 {
    let mut db = match load(cx) {
        Ok(d) => d,
        Err(c) => return c,
    };
    let ff = find(cx, &mut db);
    for w in &ff.report.warnings {
        let _ = writeln!(cx.err, "warning: {w}");
    }
    if cx.args.dry_run {
        print_plan(
            cx,
            &db,
            &ff.report,
            "Dry run: the fix would do (nothing was written):",
        );
        return exit::OK;
    }

    let fail = Failpoints::from_env();
    let opts = FixOptions {
        failpoints: fail.clone(),
        save_every: cx
            .settings
            .cache_save_timer_enabled
            .then(|| Duration::from_secs(60 * cx.settings.cache_save_time_period.max(1) as u64)),
    };
    let cache = cx.cache.clone();
    let mut saver = |db: &mut Database| {
        save_database_stepped(&cache, db, &mut |s| {
            let _ = fail.hit(match s {
                SaveStep::TmpWritten => "db_tmp_written",
                SaveStep::TmpSynced => "db_tmp_synced",
                SaveStep::BackedUp => "db_backed_up",
                SaveStep::Renamed => "db_renamed",
            });
        })
        .map_err(|e| e.to_string())
    };
    let sink = progress(cx.args.verbose);
    let r = perform_fixes(
        &mut db,
        &cx.settings,
        ff.groups,
        &opts,
        &sink,
        &cx.cancel,
        &mut saver,
    );

    let mut by_action: BTreeMap<&str, usize> = BTreeMap::new();
    for row in &r.rows {
        *by_action.entry(row.action).or_default() += 1;
        if cx.args.verbose {
            let _ = writeln!(cx.out, "  {}: {} {}", row.action, row.target, row.detail);
        }
    }
    let _ = writeln!(cx.out, "Fix done: {} missing files made", r.fixed);
    for (a, n) in by_action {
        let _ = writeln!(cx.out, "  {n:>8}  {a}");
    }
    for (what, why) in &r.skipped {
        let _ = writeln!(cx.out, "  left alone: {what}: {why}");
    }
    let mut code = exit::OK;
    if let Some(e) = &r.error {
        let _ = writeln!(cx.err, "rv: the fix stopped: {e}");
        code = exit::FAILED;
    }
    if let Some(e) = &r.save_error {
        let _ = writeln!(cx.err, "rv: the cache could not be saved: {e}");
        code = exit::FAILED;
    }
    if r.cancelled {
        let _ = writeln!(cx.err, "rv: cancelled; the cache was saved");
        if code == exit::OK {
            code = exit::CANCELLED;
        }
    }
    code
}

fn report_cmd(cx: &mut Ctx<'_>) -> i32 {
    let mut db = match load(cx) {
        Ok(d) => d,
        Err(c) => return c,
    };
    // Which missing files can be fixed is what Find Fixes says.
    let _ff = find(cx, &mut db);
    let out_dir = lexical_absolute(cx.args.out.as_deref().expect("parse checked --out"));
    match fixdat::write_fix_dats(&db.root, !cx.args.fixable, &fixdat::today(), &out_dir) {
        Ok(files) => {
            for f in &files {
                let _ = writeln!(cx.out, "{}", f.display());
            }
            let _ = writeln!(
                cx.out,
                "{} FixDat files written to {}",
                files.len(),
                out_dir.display()
            );
            exit::OK
        }
        Err(e) => {
            let _ = writeln!(cx.err, "rv: cannot write the FixDats: {e}");
            exit::FAILED
        }
    }
}

fn status_cmd(cx: &mut Ctx<'_>) -> i32 {
    let mut db = match load(cx) {
        Ok(d) => d,
        Err(c) => return c,
    };
    if cx.args.fixes {
        let _ff = find(cx, &mut db);
    }
    let ds = db.root.dir_status(cx.settings.show_new_mia);
    let mut any = false;
    for (st, n) in ds.non_zero() {
        any = true;
        let _ = writeln!(cx.out, "{n:>10}  {}", st.name());
    }
    if !any {
        let _ = writeln!(cx.out, "(the cache is empty)");
    }
    exit::OK
}

fn mia_cmd(cx: &mut Ctx<'_>) -> i32 {
    use rv_online::mia::{self, ApplyOptions, MiaCacheStatus};
    use rv_online::salt::{configured_source, read_salt};

    let online = match crate::online::load(cx.args, cx.err) {
        Ok(o) => o,
        Err(c) => return c,
    };
    if let Err(e) = guard::check(&online.paths(), cx.args.allow_live_library) {
        let _ = writeln!(cx.err, "rv: {e}");
        return exit::REFUSED;
    }
    // The exe checks the key first, then talks to the service.
    if let Err(c) = online.require_key(cx.err) {
        return c;
    }
    let Some(exe) = online.cfg.rom_vault_exe.clone() else {
        let _ = writeln!(
            cx.err,
            "rv: the MIA list is encrypted with a value that is read from your own ROMVault 3.8.0 exe. \
             Put its path in {} as rom_vault_exe=...",
            online.path.display()
        );
        return exit::FAILED;
    };
    let salt = match read_salt(&exe, &configured_source()) {
        Ok(s) => s,
        Err(e) => {
            let _ = writeln!(cx.err, "rv: {e}");
            return exit::FAILED;
        }
    };
    let client = match online.client(cx.err) {
        Ok(c) => c,
        Err(c) => return c,
    };
    let sink = progress(cx.args.verbose);
    sink(ProgressEvent::Text("Fetching MIA Cache".into()));
    let fetched = mia::fetch(
        &client,
        &online.cfg.dat_vault_key,
        &salt,
        &online.cfg.mia_cache_path(),
    );
    let Some(list) = fetched.list else {
        let _ = writeln!(cx.err, "rv: {}", fetched.status.describe());
        return exit::FAILED;
    };
    if fetched.status == MiaCacheStatus::Failed {
        let _ = writeln!(
            cx.err,
            "warning: the list was fetched but could not be cached at {}",
            online.cfg.mia_cache_path().display()
        );
    }
    let release = mia::ticks_to_date(list.miaver);
    if fetched.status == MiaCacheStatus::NoChange && !cx.args.force {
        let _ = writeln!(cx.out, "Nothing changed, MIA release {release}");
        return exit::OK;
    }
    let mut db = match load(cx) {
        Ok(d) => d,
        Err(c) => return c,
    };
    sink(ProgressEvent::Text("Updating MIA".into()));
    let applied = mia::apply_mia(
        &mut db.root,
        &list,
        &ApplyOptions {
            mia_days: online.cfg.mia_days,
            force_update: cx.args.force,
            now_ticks: mia::now_ticks(),
        },
    );
    if let Err(c) = save(cx, &mut db) {
        return c;
    }
    let _ = writeln!(
        cx.out,
        "Update complete, MIA release {release}: {} files checked, {} on the list",
        applied.files, applied.found
    );
    exit::OK
}
