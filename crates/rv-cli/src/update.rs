//! `rv update check` and `rv update install`.
//!
//! Neither reads a library, so neither needs `--cache` or `--root`. `install` says what it is
//! about to do and waits for a yes before it downloads anything, and the replacing itself is
//! [`rv_update::install`]: a hash-checked temporary file, a backup of the old program, a rename.
//! If the program's directory is not writable by this user, the release page is printed and
//! nothing more is attempted (no `sudo`, no `pkexec`).

use std::io::{BufRead, Write};
use std::path::PathBuf;

use rv_update::config::{self, UpdateConfig};
use rv_update::install::backup_path;
use rv_update::{check, current_version, install, Check, Error, Product, Step, Update};

use crate::args::Args;
use crate::exit;
use crate::guard::{self, lexical_absolute};

fn load_config(args: &Args, err: &mut dyn Write) -> Result<UpdateConfig, i32> {
    let path = lexical_absolute(
        args.update_config
            .as_deref()
            .unwrap_or(&config::default_path()),
    );
    let mut paths = vec![("--update-config".to_owned(), path.clone())];
    if let Some(t) = &args.target {
        paths.push(("--target".to_owned(), lexical_absolute(t)));
    }
    if let Err(e) = guard::check(&paths, args.allow_live_library) {
        let _ = writeln!(err, "rv: {e}");
        return Err(exit::REFUSED);
    }
    Ok(UpdateConfig::load(&path))
}

pub fn update(
    args: &Args,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let cfg = match load_config(args, err) {
        Ok(c) => c,
        Err(code) => return code,
    };
    let current = current_version();
    let found = match check(&cfg, &current) {
        Ok(f) => f,
        Err(e) => {
            let _ = writeln!(err, "rv: cannot check for updates: {e}");
            return exit::FAILED;
        }
    };
    let up = match found {
        Check::NothingPublished => {
            let _ = writeln!(
                out,
                "rv {current}: no release is published (or none that counts), so there is nothing to update to."
            );
            return exit::OK;
        }
        Check::UpToDate { latest, .. } => {
            let _ = writeln!(
                out,
                "rv {current} is up to date (the newest release is {latest})."
            );
            return exit::OK;
        }
        Check::Available(u) => u,
    };
    let _ = writeln!(
        out,
        "rv {current}: version {} is available: {}",
        up.version, up.page
    );
    if args.sub.as_deref() == Some("check") {
        let _ = writeln!(out, "Run `rv update install` to install it.");
        return exit::OK;
    }
    install_cmd(args, &up, input, out, err)
}

fn install_cmd(
    args: &Args,
    up: &Update,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let Some(asset) = Product::Rv.asset_name() else {
        let _ = writeln!(
            err,
            "rv: there is no build of the release for this system. Download it from {}",
            up.page
        );
        return exit::FAILED;
    };
    let target: PathBuf = match &args.target {
        Some(t) => lexical_absolute(t),
        None => match std::env::current_exe() {
            Ok(p) => p,
            Err(e) => {
                let _ = writeln!(err, "rv: cannot tell where this program is: {e}");
                return exit::FAILED;
            }
        },
    };
    let _ = writeln!(out, "This will:");
    let _ = writeln!(
        out,
        "  - download {asset} and SHA256SUMS from release {}",
        up.tag
    );
    let _ = writeln!(
        out,
        "  - refuse to go on unless the file matches SHA256SUMS"
    );
    let _ = writeln!(out, "  - replace {}", target.display());
    let _ = writeln!(
        out,
        "  - keep the old program as {}",
        backup_path(&target).display()
    );
    if !args.yes {
        let _ = write!(out, "Install version {}? [y/N] ", up.version);
        let _ = out.flush();
        let mut answer = String::new();
        match input.read_line(&mut answer) {
            Ok(0) | Err(_) => {
                let _ = writeln!(
                    err,
                    "\nrv: no answer to the question, so nothing was installed (use --yes to skip it)"
                );
                return exit::FAILED;
            }
            Ok(_) => {}
        }
        if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            let _ = writeln!(out, "Not installed.");
            return exit::OK;
        }
    }
    report(install(up, &asset, &target, &|_: Step| {}), up, out, err)
}

fn report(
    result: rv_update::Result<rv_update::Installed>,
    up: &Update,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    match result {
        Ok(done) => {
            let _ = writeln!(
                out,
                "Updated {} to {}.",
                done.target.display(),
                done.version
            );
            if let Some(b) = done.backup {
                let _ = writeln!(out, "The old program is kept as {}.", b.display());
            }
            exit::OK
        }
        Err(Error::NotWritable { dir, why }) => {
            let _ = writeln!(
                err,
                "rv: you cannot write in {} ({why}), so rv did not install the update and does not ask for more rights.\n    Version {} is at {}\n    Download it from there, check it against SHA256SUMS and copy it into place yourself.",
                dir.display(),
                up.version,
                up.page
            );
            exit::FAILED
        }
        Err(e) => {
            let _ = writeln!(err, "rv: the update was not installed: {e}");
            exit::FAILED
        }
    }
}
