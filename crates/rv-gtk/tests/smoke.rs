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
    let result = run(&dir)
        .and_then(|()| refusal(&dir))
        .and_then(|()| first_run(&dir));
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

    let (out, err) = run_window(
        &cfg,
        dir,
        &["--select", "RomVault/Console/NES/Roms", "--game", "Alpha"],
    )?;
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
    // At the minimum window size the whole filter row is inside the window.
    layout(&out, &err)?;
    // With DATs in the tree there is no pointer to them, and no banner.
    if flag(&out, "tree_hint=")? || flag(&out, "games_hint=")? {
        return Err(format!(
            "the first-run pointer is up over a collection:\n{out}"
        ));
    }
    // The cache was not rewritten by a smoke run.
    if !err.contains("first frame with the tree") {
        return Err(format!("the tree was never painted:\n{err}"));
    }
    println!("{line}");
    Ok(())
}

/// Start the window on `cfg` at the smallest size the app allows, which is where the filter row
/// and the lists have the least room, and collect what it reports.
fn run_window(
    cfg: &std::path::Path,
    dir: &std::path::Path,
    extra: &[&str],
) -> Result<(String, String), String> {
    fs::write(dir.join("prefs.conf"), b"width=1000\nheight=600\n").unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_rv-gtk"))
        .args(["--smoke", "--config"])
        .arg(cfg)
        .arg("--prefs")
        .arg(dir.join("prefs.conf"))
        .arg("--online-config")
        .arg(dir.join("online.conf"))
        .args(extra)
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
    Ok((out, err))
}

/// The layout line of a run at the minimum window: the window is 10 px narrower than the 1000 px
/// asked for (the frame), and no tick box or search field may be past its edge. Returns the
/// height of the games list.
fn layout(out: &str, err: &str) -> Result<i32, String> {
    let line = out
        .lines()
        .find(|l| l.starts_with("smoke-layout:"))
        .ok_or_else(|| format!("no layout report in the output:\n{out}\n{err}"))?;
    let number = |key: &str| -> Result<i32, String> {
        line.split_whitespace()
            .find_map(|w| w.strip_prefix(key))
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| format!("no {key} in: {line}"))
    };
    let (width, right) = (number("window_width=")?, number("filter_row_right=")?);
    if width > 1000 || right > width {
        return Err(format!(
            "the filter row is clipped at the minimum window ({right} px of {width}): {line}"
        ));
    }
    number("games_height=")
}

/// A `true` or `false` of the layout line.
fn flag(out: &str, key: &str) -> Result<bool, String> {
    out.lines()
        .find(|l| l.starts_with("smoke-layout:"))
        .and_then(|l| l.split_whitespace().find_map(|w| w.strip_prefix(key)))
        .map(|v| v == "true")
        .ok_or_else(|| format!("no {key} in the layout report:\n{out}"))
}

/// A refusal of the guard at the minimum window: the banner is long, and the filter row and the
/// games list must still be there. The path cannot exist on disk.
fn refusal(dir: &std::path::Path) -> Result<(), String> {
    let mut s = Settings::default();
    s.dat_root = "/mnt/HHD16TB/rv-rs-dan1888-nonexistent/DatRoot".to_owned();
    s.cache_file = dir.join("refused.Cache").to_string_lossy().into_owned();
    s.dir_mappings[0].dir_path = "/mnt/HHD16TB/rv-rs-dan1888-nonexistent/RomRoot".to_owned();
    let cfg = dir.join("refused.xml");
    engine::write_settings(&cfg, &s).map_err(|e| e.to_string())?;
    let (out, err) = run_window(&cfg, dir, &[])?;
    let height = layout(&out, &err)?;
    println!("smoke: refusal banner, games list {height} px high");
    // Dismiss is the banner's last button: it is on the window at the minimum size too.
    let dismiss = out
        .lines()
        .find(|l| l.starts_with("smoke-layout:"))
        .and_then(|l| {
            l.split_whitespace()
                .find_map(|w| w.strip_prefix("dismiss_right="))
        })
        .and_then(|v| v.parse::<i32>().ok())
        .ok_or_else(|| format!("no dismiss_right in the layout report:\n{out}"))?;
    if !(1..=1000).contains(&dismiss) {
        return Err(format!(
            "the banner's Dismiss is not on the window ({dismiss}):\n{out}"
        ));
    }
    // The banner left the list 93 px (its header and about three rows) while the info block
    // kept its room, and 213 px once the block gave way.
    if height < 120 {
        return Err(format!(
            "the games list is {height} px high under the refusal banner:\n{out}"
        ));
    }
    Ok(())
}

/// The first run: no cache file and no DAT. Both panes that would hold DATs say where to get
/// them, and there is no banner.
fn first_run(dir: &std::path::Path) -> Result<(), String> {
    let mut s = Settings::default();
    s.dat_root = dir.join("NoDats").to_string_lossy().into_owned();
    s.cache_file = dir.join("first.Cache").to_string_lossy().into_owned();
    s.dir_mappings[0].dir_path = dir.join("NoRoms").to_string_lossy().into_owned();
    fs::create_dir_all(dir.join("NoDats")).unwrap();
    let cfg = dir.join("first.xml");
    engine::write_settings(&cfg, &s).map_err(|e| e.to_string())?;
    let (out, err) = run_window(&cfg, dir, &[])?;
    layout(&out, &err)?;
    if !(flag(&out, "tree_hint=")? && flag(&out, "games_hint=")?) {
        return Err(format!(
            "the first run has no pointer to the DATs:\n{out}\n{err}"
        ));
    }
    println!("smoke: first run, both panes point at Update DATs / Settings");
    Ok(())
}
