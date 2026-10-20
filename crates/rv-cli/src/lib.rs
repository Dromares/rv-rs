//! `rv`: every feature of the engine from the command line.
//!
//! * `update-dats`, `scan`, `fix` write the cache (and `scan`/`fix` the library); `find-fixes`,
//!   `fix --dry-run`, `report` (it writes only into `--out`) and `status` never write the cache or
//!   anything under `--root`.
//! * `datvault` and `mia update` talk to the RomVault service, over `https://` unless the online
//!   settings (`~/.config/rv-rs/online.conf`) say `allow_plain_http=1`. The DatVault key is read
//!   from those settings and is never printed.
//! * `update check` and `update install` look at GitHub Releases; nothing is replaced unless you
//!   say yes (or pass `--yes`), and the download must match the release's `SHA256SUMS`.
//! * Every path is given. None has a default that points at the library, and a run refuses to
//!   start when a path is inside `/mnt/HHD16TB/` unless `--allow-live-library` is given
//!   ([`guard`]).
//! * Ctrl-C asks the running operation to stop at its next safe point; the cache is saved before
//!   the program exits.

pub mod args;
mod commands;
pub mod guard;
mod online;
mod update;

use std::io::{BufRead, Write};

use args::{parse, Parsed, USAGE};

/// Exit codes.
pub mod exit {
    pub const OK: i32 = 0;
    /// The operation failed, or stopped on an error. The cache was saved if there was one to save.
    pub const FAILED: i32 = 1;
    pub const USAGE: i32 = 2;
    /// A path is inside the live library.
    pub const REFUSED: i32 = 3;
    /// Cancelled with Ctrl-C.
    pub const CANCELLED: i32 = 130;
}

/// Run the program. `argv` is the command line without the program name.
pub fn run(argv: &[String], out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    run_with_input(argv, &mut std::io::stdin().lock(), out, err)
}

/// [`run`] with the place answers come from (`rv update install` asks before it replaces the
/// program).
pub fn run_with_input(
    argv: &[String],
    input: &mut dyn BufRead,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let args = match parse(argv) {
        Ok(Parsed::Help) => {
            let _ = write!(out, "{USAGE}");
            return exit::OK;
        }
        Ok(Parsed::Version) => {
            let _ = writeln!(out, "rv {}", rv_update::current_version());
            return exit::OK;
        }
        Ok(Parsed::Run(a)) => a,
        Err(e) => {
            let _ = writeln!(err, "rv: {e}\n\n{USAGE}");
            return exit::USAGE;
        }
    };
    if args.command == "update" {
        return update::update(&args, input, out, err);
    }
    commands::execute(&args, out, err)
}
