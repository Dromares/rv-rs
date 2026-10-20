//! Command line parsing. No defaults point anywhere: every path is given.

use std::path::PathBuf;

pub const USAGE: &str = "\
rv: a ROMVault library from the command line.

USAGE
    rv <command> --cache <file> --root <dir> [options]

COMMANDS
    update-dats          read the DATs under --dat-root into the cache
    scan                 look at the files on disk and update the cache
    find-fixes           say what is wrong and what would be done (changes nothing)
    fix                  fix the library (--dry-run: say what it would do, change nothing)
    report               write FixDats of what is still missing (--out <dir>)
    status               count the items by status

PATHS (all are given; nothing has a default that points at your library)
    --cache <file>       the ROMVault cache file (read; written by update-dats, scan, fix)
    --root <dir>         the directory the `RomVault` part of the tree lives in
    --tosort <dir>       the ToSort directory (default: `ToSort` next to --root)
    --dat-root <dir>     the DAT directory (required by update-dats)
    --config <file>      a RomVault3cfg.xml for DAT rules and extra directory mappings
    --out <dir>          where `report` writes (required by report)

OPTIONS
    --level <1|2|3>      scan level: 1 quick, 2 normal (default), 3 full
    --dry-run            fix: write nothing at all
    --fixable            report: also list files that fix would make
    --fixes              status: run Find Fixes first and count the fix statuses
    --verbose            print every action
    --allow-live-library allow paths under /mnt/HHD16TB (refused otherwise)
    -h, --help           this text
";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Args {
    pub command: String,
    pub cache: Option<PathBuf>,
    pub root: Option<PathBuf>,
    pub tosort: Option<PathBuf>,
    pub dat_root: Option<PathBuf>,
    pub config: Option<PathBuf>,
    pub out: Option<PathBuf>,
    pub level: Option<u8>,
    pub dry_run: bool,
    pub fixable: bool,
    pub fixes: bool,
    pub verbose: bool,
    pub allow_live_library: bool,
}

/// What parsing found: a run, or a request for the help text.
pub enum Parsed {
    Run(Args),
    Help,
}

const COMMANDS: &[&str] = &[
    "update-dats",
    "scan",
    "find-fixes",
    "fix",
    "report",
    "status",
];

pub fn parse(argv: &[String]) -> Result<Parsed, String> {
    let mut a = Args::default();
    let mut it = argv.iter();
    while let Some(arg) = it.next() {
        let mut value = |name: &str| -> Result<PathBuf, String> {
            it.next()
                .map(PathBuf::from)
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match arg.as_str() {
            "-h" | "--help" => return Ok(Parsed::Help),
            "--cache" => a.cache = Some(value("--cache")?),
            "--root" => a.root = Some(value("--root")?),
            "--tosort" => a.tosort = Some(value("--tosort")?),
            "--dat-root" => a.dat_root = Some(value("--dat-root")?),
            "--config" => a.config = Some(value("--config")?),
            "--out" => a.out = Some(value("--out")?),
            "--level" => {
                let v = value("--level")?;
                a.level = match v.to_str() {
                    Some("1") => Some(1),
                    Some("2") => Some(2),
                    Some("3") => Some(3),
                    _ => return Err("--level is 1, 2 or 3".to_owned()),
                };
            }
            "--dry-run" => a.dry_run = true,
            "--fixable" => a.fixable = true,
            "--fixes" => a.fixes = true,
            "--verbose" | "-v" => a.verbose = true,
            "--allow-live-library" => a.allow_live_library = true,
            s if s.starts_with('-') => return Err(format!("unknown option {s}")),
            s => {
                if !a.command.is_empty() {
                    return Err(format!("unexpected argument {s}"));
                }
                if !COMMANDS.contains(&s) {
                    return Err(format!("unknown command {s}"));
                }
                a.command = s.to_owned();
            }
        }
    }
    if a.command.is_empty() {
        return Ok(Parsed::Help);
    }
    let need = |name: &str, v: &Option<PathBuf>| -> Result<(), String> {
        if v.is_none() {
            Err(format!("{} needs {name}", a.command))
        } else {
            Ok(())
        }
    };
    need("--cache <file>", &a.cache)?;
    need("--root <dir>", &a.root)?;
    if a.command == "update-dats" {
        need("--dat-root <dir>", &a.dat_root)?;
    }
    if a.command == "report" {
        need("--out <dir>", &a.out)?;
    }
    if a.dry_run && a.command != "fix" {
        return Err("--dry-run is for fix".to_owned());
    }
    if a.level.is_some() && a.command != "scan" {
        return Err("--level is for scan".to_owned());
    }
    Ok(Parsed::Run(a))
}
