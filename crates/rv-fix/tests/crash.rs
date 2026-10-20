//! Fault injection: the fix is stopped dead at each step boundary (the process aborts, as a kill
//! or a power cut would leave it), and afterwards nothing may be lost.
//!
//! The child process is this test binary run again with `RV_CRASH_CHILD_DIR` set: it opens the
//! library its parent built and fixes it with `RV_FIX_FAILPOINT` naming the step to die at.
//!
//! "Nothing lost" is checked on content, not names: every byte string that was in the library
//! (the entries of a valid zip, the whole of anything else) is still in it somewhere afterwards,
//! and nothing new is: a half-written file at a final name would be new content.

#![cfg_attr(feature = "mutation-3-8-0-order", allow(dead_code))]

mod common;

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::process::Command;

use common::*;
use rv_cache::{save_database_stepped, SaveStep};
use rv_core::progress::{CancelFlag, NoProgress};
use rv_core::{DatRule, Database, FileType, ScanLevel};
use rv_fix::{find_fixes, perform_fixes, Failpoints, FixOptions};

const A: &[u8] = b"rom a contents rom a contents";
const B: &[u8] = b"second rom is a bit longer";
const C: &[u8] = b"third";
const J: &[u8] = b"junk junk junk";
const X: &[u8] = b"xxxx xxxx xxxx";

/// The steps a child can die at: the fixer's, then the cache save's.
const STEPS: &[&str] = &[
    "after_temp_write",
    "after_fsync",
    "before_rename",
    "after_rename",
    "before_source_delete",
    "before_cache_save",
    "db_tmp_written",
    "db_tmp_synced",
    "db_backed_up",
    "db_renamed",
];

type Scenario = fn(&mut Lib);

/// A zip is rebuilt: Beta.zip is made from Wrong.zip, and the junk leaves for ToSort.
fn archives(l: &mut Lib) {
    l.dat(
        "nes.dat",
        &[
            ("Alpha", &[("a.nes", A), ("b.nes", B)]),
            ("Beta", &[("c.nes", C)]),
        ],
    );
    l.update_dats();
    l.zip(
        "RomRoot/Console/NES/Roms/Alpha.zip",
        &[("a.nes", A), ("b.nes", B), ("a2.nes", A)],
    );
    l.zip(
        "RomRoot/Console/NES/Roms/Wrong.zip",
        &[("c.nes", C), ("j.bin", J)],
    );
    l.zip("RomRoot/Console/NES/Roms/Stranger.zip", &[("x.bin", X)]);
    let bad = b"not a zip, just bytes named like one";
    l.write("RomRoot/Console/NES/Roms/Gamma.zip", bad);
}

/// Loose files: one is moved into place, one is copied, one goes to ToSort, one is deleted.
fn loose(l: &mut Lib) {
    let mut rule = DatRule::default();
    rule.dir_key = "RomVault/Console/NES/Loose".to_owned();
    rule.compression = FileType::FILE_ONLY;
    l.settings.dat_rules.push(rule);
    l.write(
        "DatRoot/Console/NES/Loose/loose.dat",
        dat_text(&[
            ("G1", &[("a.bin", A), ("b.bin", B)]),
            ("G2", &[("b.bin", B)]),
        ])
        .as_bytes(),
    );
    l.update_dats();
    l.write("RomRoot/Console/NES/Loose/G1/a.bin", A);
    l.write("RomRoot/Console/NES/Loose/G1/bee.bin", B);
    l.write("RomRoot/Console/NES/Loose/G1/junk.bin", J);
    l.write("RomRoot/Console/NES/Loose/G1/again.bin", A);
}

const SCENARIOS: &[(&str, Scenario)] = &[("archives", archives), ("loose", loose)];

/// The loose-file scenario needs the rule in the settings of the child too.
fn open_child(dir: &Path, scenario: &str) -> Lib {
    let mut l = Lib::at(dir);
    if scenario == "loose" {
        let mut rule = DatRule::default();
        rule.dir_key = "RomVault/Console/NES/Loose".to_owned();
        rule.compression = FileType::FILE_ONLY;
        l.settings.dat_rules.push(rule);
    }
    l
}

/// Not a test of its own: the child half of the others. Does nothing unless a parent started it.
#[test]
fn child_run() {
    let Ok(dir) = std::env::var("RV_CRASH_CHILD_DIR") else {
        return;
    };
    let scenario = std::env::var("RV_CRASH_SCENARIO").unwrap();
    let mut l = open_child(Path::new(&dir), &scenario);
    let ff = find_fixes(&mut l.db, &l.settings, &NoProgress, &CancelFlag::new());
    let fail = Failpoints::from_env();
    let opts = FixOptions {
        failpoints: fail.clone(),
        save_every: None,
    };
    let cache = l.cache.clone();
    let mut saver = |db: &mut Database| {
        save_database_stepped(&cache, db, &mut |s| {
            let name = match s {
                SaveStep::TmpWritten => "db_tmp_written",
                SaveStep::TmpSynced => "db_tmp_synced",
                SaveStep::BackedUp => "db_backed_up",
                SaveStep::Renamed => "db_renamed",
            };
            let _ = fail.hit(name);
        })
        .map_err(|e| e.to_string())
    };
    let r = perform_fixes(
        &mut l.db,
        &l.settings,
        ff.groups,
        &opts,
        &NoProgress,
        &CancelFlag::new(),
        &mut saver,
    );
    assert!(r.error.is_none(), "{:?}", r.error);
}

/// Every byte string in the library under `dirs`: entries of valid zips, whole other files.
fn atoms(root: &Path) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for sub in ["RomRoot", "ToSort"] {
        let base = root.join(sub);
        for (rel, _) in manifest(&base) {
            let p = base.join(&rel);
            if rel.ends_with(".rvtmp") {
                // The temp file of a write that was stopped: not at a final name.
                continue;
            }
            let bytes = fs::read(&p).unwrap();
            match rv_archive::zip::ZipArchive::open(std::io::Cursor::new(bytes.clone())) {
                Ok(mut z) => {
                    for i in 0..z.len() {
                        let data = z.read_entry(i).unwrap_or_else(|e| {
                            panic!("{sub}/{rel}: entry {i} of a zip at a final name is bad: {e}")
                        });
                        out.insert(sha256(&data));
                    }
                }
                Err(_) => {
                    out.insert(sha256(&bytes));
                }
            }
        }
    }
    out
}

/// Atoms and temp files, minus the empty file, which a fix may create.
fn check_no_loss(pre: &BTreeSet<String>, root: &Path) -> Result<(), String> {
    let post = atoms(root);
    let lost: Vec<_> = pre.difference(&post).collect();
    let invented: Vec<_> = post
        .difference(pre)
        .filter(|a| **a != sha256(b""))
        .collect();
    if !lost.is_empty() || !invented.is_empty() {
        return Err(format!(
            "lost: {lost:?}; invented (half written?): {invented:?}"
        ));
    }
    Ok(())
}

/// Stop the fix at `step`. Returns false if the scenario never reaches it.
fn crash_at(name: &str, build: Scenario, step: &str) -> bool {
    let mut l = Lib::new(&format!("crash-{name}-{step}"));
    build(&mut l);
    l.scan(ScanLevel::Level2);
    l.save();
    let pre = atoms(&l.dir);

    let out = Command::new(std::env::current_exe().unwrap())
        .args(["child_run", "--exact", "--nocapture", "--test-threads=1"])
        .env("RV_CRASH_CHILD_DIR", &l.dir)
        .env("RV_CRASH_SCENARIO", name)
        .env("RV_FIX_FAILPOINT", step)
        .output()
        .unwrap();
    let reached = !out.status.success();
    if !reached {
        // The step was never reached: the child must have finished the whole fix.
        assert!(
            String::from_utf8_lossy(&out.stdout).contains("1 passed"),
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    // 1. Nothing is lost, and nothing half written is at a final name.
    if let Err(e) = check_no_loss(&pre, &l.dir) {
        panic!("{name} dying at {step}: {e}");
    }

    // 2. The cache loads (or its backup does), and a scan reconciles it with the disk.
    let mut after = open_child(&l.dir, name);
    after.scan(ScanLevel::Level3);

    // 3. Running the fix again finishes the job, with the same end state as one that was never
    //    interrupted, and the tree agrees with the disk.
    let r = after.fix();
    assert!(
        r.error.is_none(),
        "{name} at {step}, second fix: {:?}",
        r.error
    );
    check_no_loss(&pre, &l.dir).unwrap_or_else(|e| panic!("{name} at {step}, after recovery: {e}"));
    after.assert_rescan_agrees();
    for sub in ["RomRoot", "ToSort"] {
        for k in manifest(&l.dir.join(sub)).keys() {
            assert!(
                !k.ends_with(".rvtmp"),
                "{name} at {step}: {sub}/{k} was left behind"
            );
        }
    }

    let mut reference = Lib::new(&format!("crash-ref-{name}-{step}"));
    build(&mut reference);
    reference.scan(ScanLevel::Level2);
    let r = reference.fix();
    assert!(r.error.is_none(), "{:?}", r.error);
    assert_eq!(
        atoms(&l.dir),
        atoms(&reference.dir),
        "{name} at {step}: the recovered library differs from an uninterrupted one"
    );
    let _ = fs::remove_dir_all(&l.dir);
    let _ = fs::remove_dir_all(&reference.dir);
    reached
}

#[cfg(not(feature = "mutation-3-8-0-order"))]
#[test]
fn killed_at_every_step_nothing_is_lost_and_the_fix_can_be_finished() {
    let mut reached: BTreeSet<&str> = BTreeSet::new();
    for (name, build) in SCENARIOS {
        for step in STEPS {
            if crash_at(name, *build, step) {
                reached.insert(step);
            }
        }
    }
    let never: Vec<_> = STEPS.iter().filter(|s| !reached.contains(**s)).collect();
    assert!(never.is_empty(), "no scenario reaches {never:?}");
}

/// With the feature on, the order of 3.8.0 is back (the old archive is deleted, then the new one
/// renamed in), and the same crash test must notice the loss.
#[cfg(feature = "mutation-3-8-0-order")]
#[test]
fn mutation_deleting_before_renaming_is_caught() {
    let r = std::panic::catch_unwind(|| crash_at("archives", archives, "before_rename"));
    let err = r.expect_err("the crash test did not notice that an archive was deleted first");
    let msg = err.downcast_ref::<String>().cloned().unwrap_or_default();
    assert!(msg.contains("lost"), "{msg}");
}
