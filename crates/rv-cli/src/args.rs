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
    datvault check       is the DatVault key in the online settings valid?
    datvault list        the DatVault groups (--group: the DATs of a group, and how they compare
                         with the ones under --dat-root)
    datvault fetch       download DATs into --dat-root (--group <g>, or --updates for the groups
                         you already have)
    mia update           fetch the MIA list and mark the files in the cache (needs the path of
                         your own ROMVault38.exe in the online settings)

PATHS (all are given; nothing has a default that points at your library)
    --cache <file>       the ROMVault cache file (read; written by update-dats, scan, fix)
    --root <dir>         the directory the `RomVault` part of the tree lives in
    --tosort <dir>       the ToSort directory (default: `ToSort` next to --root)
    --dat-root <dir>     the DAT directory (required by update-dats)
    --config <file>      a RomVault3cfg.xml for DAT rules and extra directory mappings
    --online-config <f>  the online settings (default: ~/.config/rv-rs/online.conf): the DatVault
                         key, the ROMVault exe the MIA salt is read from, allow_plain_http, ...
    --out <dir>          where `report` writes (required by report)

OPTIONS
    --level <1|2|3>      scan level: 1 quick, 2 normal (default), 3 full
    --dry-run            fix: write nothing at all
    --fixable            report: also list files that fix would make
    --fixes              status: run Find Fixes first and count the fix statuses
    --verbose            print every action
    --group <name>       datvault list/fetch: a DatVault group (repeatable)
    --updates            datvault fetch: update the groups that have DATs under --dat-root
    --remove-discontinued datvault fetch: take DATs the service no longer lists out (the DAT
                         becomes <name>.old, its seed is deleted)
    --force              mia update: apply the list even if it has not changed
    --allow-live-library allow paths under /mnt/HHD16TB (refused otherwise)
    -h, --help           this text
";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Args {
    pub command: String,
    /// `check`, `list`, `fetch` or `update`: the second word of `datvault` and `mia`.
    pub sub: Option<String>,
    pub online_config: Option<PathBuf>,
    pub groups: Vec<String>,
    pub updates: bool,
    pub remove_discontinued: bool,
    pub force: bool,
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
    Run(Box<Args>),
    Help,
}

const COMMANDS: &[&str] = &[
    "update-dats",
    "scan",
    "find-fixes",
    "fix",
    "report",
    "status",
    "datvault",
    "mia",
];

fn subcommands(command: &str) -> &'static [&'static str] {
    match command {
        "datvault" => &["check", "list", "fetch"],
        "mia" => &["update"],
        _ => &[],
    }
}

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
            "--online-config" => a.online_config = Some(value("--online-config")?),
            "--group" => {
                let g = value("--group")?;
                a.groups.push(g.to_string_lossy().into_owned());
            }
            "--updates" => a.updates = true,
            "--remove-discontinued" => a.remove_discontinued = true,
            "--force" => a.force = true,
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
                    if a.sub.is_none() && subcommands(&a.command).contains(&s) {
                        a.sub = Some(s.to_owned());
                        continue;
                    }
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
    if !subcommands(&a.command).is_empty() && a.sub.is_none() {
        return Err(format!(
            "{} needs one of: {}",
            a.command,
            subcommands(&a.command).join(", ")
        ));
    }
    let online = a.command == "datvault" || a.command == "mia";
    if a.command != "datvault" {
        need("--cache <file>", &a.cache)?;
        need("--root <dir>", &a.root)?;
    }
    if a.command == "datvault" && a.sub.as_deref() == Some("fetch") {
        need("--dat-root <dir>", &a.dat_root)?;
        if a.groups.is_empty() && !a.updates {
            return Err("datvault fetch needs --group <name> or --updates".to_owned());
        }
    }
    if (!a.groups.is_empty() || a.updates || a.remove_discontinued) && a.command != "datvault" {
        return Err("--group, --updates and --remove-discontinued are for datvault".to_owned());
    }
    if a.updates && a.sub.as_deref() != Some("fetch") {
        return Err("--updates is for datvault fetch".to_owned());
    }
    if a.remove_discontinued && a.sub.as_deref() != Some("fetch") {
        return Err("--remove-discontinued is for datvault fetch".to_owned());
    }
    if a.force && a.command != "mia" {
        return Err("--force is for mia update".to_owned());
    }
    if a.online_config.is_some() && !online {
        return Err("--online-config is for datvault and mia".to_owned());
    }
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
    Ok(Parsed::Run(Box::new(a)))
}

#[cfg(test)]
mod online_tests {
    use super::*;

    fn p(a: &[&str]) -> Result<Args, String> {
        let argv: Vec<String> = a.iter().map(|s| (*s).to_owned()).collect();
        match parse(&argv)? {
            Parsed::Run(a) => Ok(*a),
            Parsed::Help => Err("help".to_owned()),
        }
    }

    #[test]
    fn datvault_and_mia_words() {
        let a = p(&["datvault", "check"]).unwrap();
        assert_eq!(
            (a.command.as_str(), a.sub.as_deref()),
            ("datvault", Some("check"))
        );
        let a = p(&["datvault", "list", "--group", "a", "--group", "b"]).unwrap();
        assert_eq!(a.groups, ["a", "b"]);
        let a = p(&["mia", "update", "--cache", "c", "--root", "r", "--force"]).unwrap();
        assert!(a.force);
        assert_eq!(a.sub.as_deref(), Some("update"));
    }

    #[test]
    fn what_each_needs() {
        assert!(p(&["datvault"]).unwrap_err().contains("needs one of"));
        assert!(p(&["datvault", "fetch", "--group", "g"])
            .unwrap_err()
            .contains("--dat-root"));
        assert!(p(&["datvault", "fetch", "--dat-root", "d"])
            .unwrap_err()
            .contains("--group"));
        assert!(p(&["datvault", "fetch", "--dat-root", "d", "--updates"]).is_ok());
        assert!(p(&["mia", "update"]).unwrap_err().contains("--cache"));
        assert!(p(&["mia"]).unwrap_err().contains("update"));
        assert!(p(&["datvault", "bogus"]).is_err());
    }

    #[test]
    fn options_that_belong_elsewhere_are_refused() {
        let base = ["--cache", "c", "--root", "r"];
        let with = |cmd: &[&str], extra: &[&str]| -> Vec<String> {
            cmd.iter()
                .chain(base.iter())
                .chain(extra.iter())
                .map(|s| (*s).to_owned())
                .collect()
        };
        for extra in [
            &["--group", "g"][..],
            &["--updates"],
            &["--remove-discontinued"],
            &["--force"],
            &["--online-config", "x"],
        ] {
            assert!(parse(&with(&["scan"], extra)).is_err(), "{extra:?}");
        }
        assert!(p(&["datvault", "list", "--force"]).is_err());
        assert!(p(&["datvault", "list", "--updates"]).is_err());
        assert!(p(&["datvault", "check", "--online-config", "x"]).is_ok());
    }
}
