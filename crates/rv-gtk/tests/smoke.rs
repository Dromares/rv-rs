//! The real window, headless: a synthetic collection is built, `rv-gtk --smoke` is started on it
//! under whatever display the environment gives (CI: `xvfb-run`), and what the window reports
//! having on screen is checked.
//!
//! Without `RV_GTK_SMOKE=1` the test is skipped and says why, because most machines that run
//! `cargo test` have no display to give it. In CI the variable is set, so a window that cannot
//! open fails the build.

use std::fs;
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use rv_core::progress::{CancelFlag, ProgressEvent};
use rv_core::{FileType, ScanLevel, Settings};
use rv_gtk::engine::{self, Task};
use rv_scan::db::ScanTarget;

const DAT: &str = "clrmamepro (\n\tname \"T\"\n)\n\
game (\n\tname \"Alpha\"\n\
\trom ( name \"a.rom\" size 4 crc ed82cd11 )\n\
\trom ( name \"b.rom\" size 4 crc 08337bb5 )\n)\n\
game (\n\tname \"Beta\"\n\
\trom ( name \"c.rom\" size 4 crc 08337bb5 )\n)\n";

fn main() {
    if std::env::var_os("RV_GTK_SMOKE").is_none_or(|v| v != "1") {
        println!(
            "smoke: skipped, it needs a display (set RV_GTK_SMOKE=1; CI runs it under xvfb-run)"
        );
        return;
    }
    let dir = std::env::temp_dir().join(format!("rv-gtk-smoke-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let result = run(&dir);
    let _ = fs::remove_dir_all(&dir);
    if let Err(e) = result {
        eprintln!("smoke: FAILED: {e}");
        std::process::exit(1);
    }
    println!("smoke: ok");
}

fn run(dir: &std::path::Path) -> Result<(), String> {
    let write = |rel: &str, bytes: &[u8]| {
        let p = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, bytes).unwrap();
    };
    write("DatRoot/Console/NES/Roms/nes.dat", DAT.as_bytes());
    write("RomRoot/Console/NES/Roms/Alpha/a.rom", b"abcd");

    let mut s = Settings::default();
    s.dat_root = dir.join("DatRoot").to_string_lossy().into_owned();
    s.cache_file = dir.join("smoke.Cache").to_string_lossy().into_owned();
    s.dir_mappings[0].dir_path = dir.join("RomRoot").to_string_lossy().into_owned();
    s.dat_rules[0].compression = FileType::FILE_ONLY;
    let cfg = dir.join("RomVault3cfg.xml");
    engine::write_settings(&cfg, &s).map_err(|e| e.to_string())?;

    // Build the cache the way the window does: update the DATs, scan, save.
    let mut db = engine::open_cache(&s, false).map_err(|e| e.to_string())?.db;
    let nothing = |_: ProgressEvent| {};
    engine::run_task(
        &mut db,
        &s,
        &Task::UpdateDats { all: false },
        false,
        &nothing,
        &CancelFlag::new(),
    )
    .map_err(|e| e.to_string())?;
    engine::run_task(
        &mut db,
        &s,
        &Task::Scan {
            level: ScanLevel::Level2,
            target: ScanTarget::Dir("RomVault/Console/NES/Roms".into()),
        },
        false,
        &nothing,
        &CancelFlag::new(),
    )
    .map_err(|e| e.to_string())?;

    let mut child = Command::new(env!("CARGO_BIN_EXE_rv-gtk"))
        .args(["--smoke", "--config"])
        .arg(&cfg)
        .arg("--prefs")
        .arg(dir.join("prefs.conf"))
        .args(["--select", "RomVault/Console/NES/Roms", "--game", "Alpha"])
        .env("GSK_RENDERER", "cairo")
        .env("NO_AT_BRIDGE", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot start rv-gtk: {e}"))?;
    let start = Instant::now();
    let status = loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(st) => break st,
            None if start.elapsed() > Duration::from_secs(60) => {
                let _ = child.kill();
                return Err("the window did not report and quit within 60 s".to_owned());
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    };
    let (mut out, mut err) = (String::new(), String::new());
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut out)
        .unwrap();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut err)
        .unwrap();
    if !status.success() {
        return Err(format!("rv-gtk exited with {status}\n{err}"));
    }
    // The tree has RomVault, Console, NES, Roms and ToSort; Alpha is selected with its two ROMs,
    // and Beta is in the list too.
    let line = out
        .lines()
        .find(|l| l.starts_with("smoke:"))
        .ok_or_else(|| format!("no report in the output:\n{out}\n{err}"))?;
    for needle in [
        "tree_rows=5",
        "selected=Some(\"RomVault/Console/NES/Roms\")",
        "games=2",
        "roms=2",
    ] {
        if !line.contains(needle) {
            return Err(format!("{needle} is not in: {line}\n{err}"));
        }
    }
    // The cache was not rewritten by a smoke run.
    if !err.contains("first frame with the tree") {
        return Err(format!("the tree was never painted:\n{err}"));
    }
    println!("{line}");
    Ok(())
}
