//! `rv datvault ...` and the online half of `rv mia update`.
//!
//! The DatVault key and the other online settings come from the rewrite's own file
//! (`--online-config`, default `~/.config/rv-rs/online.conf`). The key is never printed: what
//! reaches the terminal is the library's error text, which has the key taken out.

use std::io::Write;
use std::path::PathBuf;

use rv_core::progress::{CancelFlag, ProgressEvent};
use rv_online::config::{self, OnlineConfig};
use rv_online::datvault::{DatUpdate, FetchOptions, Pair, Vault};
use rv_online::mia::MiaCacheStatus;
use rv_online::{Client, Error};

use crate::args::Args;
use crate::exit;
use crate::guard::{self, lexical_absolute};

/// The online settings and where they came from.
pub struct Online {
    pub cfg: OnlineConfig,
    pub path: PathBuf,
}

pub fn load(args: &Args, err: &mut dyn Write) -> Result<Online, i32> {
    let path = lexical_absolute(
        args.online_config
            .as_deref()
            .unwrap_or(&config::default_path()),
    );
    // Before the file is read: it may name paths, and it may itself be in the live library.
    if let Err(e) = guard::check(
        &[("--online-config".to_owned(), path.clone())],
        args.allow_live_library,
    ) {
        let _ = writeln!(err, "rv: {e}");
        return Err(exit::REFUSED);
    }
    match OnlineConfig::load(&path) {
        Ok(cfg) => Ok(Online { cfg, path }),
        Err(e) => {
            let _ = writeln!(err, "rv: {e}");
            Err(exit::FAILED)
        }
    }
}

impl Online {
    /// The paths the settings name, for the live library check.
    pub fn paths(&self) -> Vec<(String, PathBuf)> {
        let mut v = vec![("mia_cache".to_owned(), self.cfg.mia_cache_path())];
        if let Some(e) = &self.cfg.rom_vault_exe {
            v.push(("rom_vault_exe".to_owned(), e.clone()));
        }
        v
    }

    pub fn client(&self, err: &mut dyn Write) -> Result<Client, i32> {
        match self.cfg.endpoint() {
            Ok(ep) => Ok(Client::new(ep)),
            Err(e) => {
                let _ = writeln!(err, "rv: {e}");
                Err(exit::FAILED)
            }
        }
    }

    pub fn require_key(&self, err: &mut dyn Write) -> Result<(), i32> {
        if self.cfg.has_key() {
            return Ok(());
        }
        let _ = writeln!(
            err,
            "rv: {}. Put it in {} as dat_vault_key=...",
            MiaCacheStatus::DatVaultKeyNotFound.describe(),
            self.path.display()
        );
        Err(exit::FAILED)
    }
}

/// The group a pair belongs to.
fn group_of(p: &Pair) -> String {
    p.remote
        .as_ref()
        .map(|r| r.group.clone())
        .or(p.local.as_ref().map(|l| l.info.group.clone()))
        .unwrap_or_default()
}

fn fail(err: &mut dyn Write, e: &Error) -> i32 {
    let _ = writeln!(err, "rv: {e}");
    exit::FAILED
}

fn progress(verbose: bool) -> impl Fn(ProgressEvent) {
    move |e| {
        let mut w = std::io::stderr().lock();
        match e {
            ProgressEvent::Text(t) => {
                let _ = writeln!(w, "{t}");
            }
            ProgressEvent::Error { file, message } => {
                let _ = writeln!(w, "  {file}: {message}");
            }
            ProgressEvent::Item(t) if verbose => {
                let _ = writeln!(w, "  {t}");
            }
            _ => {}
        }
    }
}

pub fn datvault(args: &Args, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let online = match load(args, err) {
        Ok(o) => o,
        Err(c) => return c,
    };
    let mut paths = vec![];
    if let Some(d) = &args.dat_root {
        paths.push(("--dat-root".to_owned(), lexical_absolute(d)));
    }
    if let Some(d) = &args.tosort {
        paths.push(("--tosort".to_owned(), lexical_absolute(d)));
    }
    if let Err(e) = guard::check(&paths, args.allow_live_library) {
        let _ = writeln!(err, "rv: {e}");
        return exit::REFUSED;
    }
    if let Err(c) = online.require_key(err) {
        return c;
    }
    let client = match online.client(err) {
        Ok(c) => c,
        Err(c) => return c,
    };
    let cancel = CancelFlag::new();
    {
        let c = cancel.clone();
        let _ = ctrlc::set_handler(move || c.cancel());
    }
    let vault = match Vault::new(&client, &online.cfg.dat_vault_key) {
        Ok(v) => v,
        Err(e) => return fail(err, &e),
    };
    match args.sub.as_deref() {
        Some("check") => match vault.check_key() {
            Ok(true) => {
                let _ = writeln!(out, "The DatVault key is valid.");
                exit::OK
            }
            Ok(false) => {
                let _ = writeln!(out, "The DatVault key is not valid.");
                exit::FAILED
            }
            Err(e) => fail(err, &e),
        },
        Some("list") => list(args, &vault, out, err),
        Some("fetch") => fetch(args, &online, &vault, out, err, &cancel),
        _ => exit::USAGE,
    }
}

fn list(args: &Args, vault: &Vault<'_>, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    if args.groups.is_empty() {
        return match vault.groups() {
            Ok(groups) => {
                for g in &groups {
                    let _ = writeln!(out, "{}/{}\t{}", g.basepath, g.group, g.display_name);
                }
                let _ = writeln!(out, "{} groups", groups.len());
                exit::OK
            }
            Err(e) => fail(err, &e),
        };
    }
    let sink = progress(args.verbose);
    // Without a DAT root there is nothing to compare with: everything listed is available.
    let plan = match &args.dat_root {
        Some(root) => vault.plan(&lexical_absolute(root), &args.groups, &sink),
        None => vault.plan_seeds(Vec::new(), Vec::new(), &args.groups, &sink),
    };
    let plan = match plan {
        Ok(p) => p,
        Err(e) => return fail(err, &e),
    };
    for w in &plan.warnings {
        let _ = writeln!(err, "warning: {w}");
    }
    for p in &plan.pairs {
        if !args.groups.contains(&group_of(p)) {
            continue;
        }
        let shown = p.remote.as_ref().or(p.local.as_ref().map(|l| &l.info));
        let (version, date, name) = shown
            .map(|i| (i.version.as_str(), i.date.as_str(), i.dat_name.as_str()))
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "{:<13} {}\t{version}\t{date}\t{name}",
            p.status.name(),
            p.key
        );
    }
    exit::OK
}

fn fetch(
    args: &Args,
    online: &Online,
    vault: &Vault<'_>,
    out: &mut dyn Write,
    err: &mut dyn Write,
    cancel: &CancelFlag,
) -> i32 {
    let root = lexical_absolute(args.dat_root.as_deref().expect("parse checked --dat-root"));
    let sink = progress(args.verbose);
    let mut plan = match vault.plan(&root, &args.groups, &sink) {
        Ok(p) => p,
        Err(e) => return fail(err, &e),
    };
    for w in &plan.warnings {
        let _ = writeln!(err, "warning: {w}");
    }
    if !args.groups.is_empty() {
        // `--group g` takes everything the service lists for g. What belongs to no named group
        // is left alone, unless `--updates` asked for the groups already here.
        for p in &mut plan.pairs {
            if args.groups.iter().any(|g| *g == group_of(p)) {
                if p.status == DatUpdate::Available {
                    p.checked = true;
                }
            } else if !(args.updates && p.local.is_some()) {
                p.checked = false;
                p.status = DatUpdate::None;
            }
        }
    }
    let groups = if plan
        .pairs
        .iter()
        .any(|p| p.checked && p.status == DatUpdate::Available)
    {
        match vault.groups() {
            Ok(g) => g,
            Err(e) => return fail(err, &e),
        }
    } else {
        Vec::new()
    };
    let opts = FetchOptions {
        cues_dir: args
            .tosort
            .as_deref()
            .map(|t| lexical_absolute(t).join("cues")),
        delete_old_cue_files: online.cfg.delete_old_cue_files,
        remove_discontinued: args.remove_discontinued,
    };
    let report = vault.fetch(&root, &mut plan, &groups, &opts, &sink, cancel);
    let _ = writeln!(
        out,
        "{} DATs downloaded, {} removed, {} failed{}",
        report.downloaded.len(),
        report.removed.len(),
        report.failed.len(),
        if report.cancelled { " (cancelled)" } else { "" }
    );
    for (what, why) in &report.failed {
        let _ = writeln!(out, "  failed: {what}: {why}");
    }
    if !report.failed.is_empty() {
        exit::FAILED
    } else if report.cancelled {
        exit::CANCELLED
    } else {
        exit::OK
    }
}
