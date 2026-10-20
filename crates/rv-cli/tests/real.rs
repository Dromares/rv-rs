//! The real-data dry run: copies of the board's cache, DATs and sample ROMs, run through
//! `rv scan`, `rv find-fixes` and `rv fix --dry-run`. `#[ignore]`d: it needs the fixtures.
//!
//! Run with `RV_FIXTURES_DIR=<dir> cargo test -p rv-cli --test real -- --include-ignored`. It fails
//! (never passes) when the variable or a fixture file is missing. The fixtures are only read;
//! everything is done on copies under `RV_WORK_DIR` (default: the temp directory), which are
//! removed afterwards. No path under `/mnt/HHD16TB/` is used and `--allow-live-library` is never
//! passed.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use rv_scan::{HashKinds, HashSet};

fn fixtures() -> PathBuf {
    let dir = std::env::var_os("RV_FIXTURES_DIR")
        .expect("RV_FIXTURES_DIR must name the fixtures directory");
    let dir = PathBuf::from(dir);
    for f in [
        "RomVault3_3.Cache",
        "DatRoot",
        "RomRoot-sample/MANIFEST.sha256",
        "RomVault3cfg.xml",
    ] {
        assert!(
            dir.join(f).exists(),
            "{f} is missing from {}",
            dir.display()
        );
    }
    dir
}

fn sha256_file(p: &Path) -> String {
    use std::io::Read;
    let mut h = HashSet::new(HashKinds::ALL);
    let mut f = fs::File::open(p).unwrap();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).unwrap();
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    rv_scan::hash::to_hex(&h.finish().sha256.unwrap())
}

fn manifest(root: &Path) -> BTreeMap<String, String> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, String>) {
        for e in fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(root, &p, out);
            } else {
                out.insert(
                    p.strip_prefix(root).unwrap().to_string_lossy().into_owned(),
                    sha256_file(&p),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap().flatten() {
        let (src, dst) = (e.path(), to.join(e.file_name()));
        if src.is_dir() {
            copy_tree(&src, &dst);
        } else {
            fs::copy(&src, &dst).unwrap();
        }
    }
}

/// The lines of a recorded `sha256sum` manifest: `hash  path`.
fn recorded(path: &Path) -> BTreeMap<String, String> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .filter_map(|l| l.split_once("  "))
        .map(|(h, p)| (p.trim_start_matches("./").to_owned(), h.to_owned()))
        .collect()
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

#[test]
#[ignore = "needs RV_FIXTURES_DIR"]
fn find_fixes_and_a_dry_run_on_copies_of_the_real_data_change_nothing() {
    let fx = fixtures();
    let base = std::env::var_os("RV_WORK_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let work = base.join(format!("rv-cli-real-{}", std::process::id()));
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work).unwrap();

    // Copies, remapped: the sample ROMs are the library, `ToSort` is next to it.
    copy_tree(&fx.join("DatRoot"), &work.join("DatRoot"));
    copy_tree(&fx.join("RomRoot-sample"), &work.join("RomRoot"));
    fs::remove_file(work.join("RomRoot/MANIFEST.sha256")).unwrap();
    fs::copy(fx.join("RomVault3_3.Cache"), work.join("RomVault3_3.Cache")).unwrap();

    let rv = |args: &[&str]| -> Output {
        let o = Command::new(env!("CARGO_BIN_EXE_rv"))
            .arg(args[0])
            .arg("--cache")
            .arg(work.join("RomVault3_3.Cache"))
            .arg("--root")
            .arg(work.join("RomRoot"))
            .arg("--tosort")
            .arg(work.join("ToSort"))
            .arg("--dat-root")
            .arg(work.join("DatRoot"))
            .arg("--config")
            .arg(fx.join("RomVault3cfg.xml"))
            .args(&args[1..])
            .output()
            .unwrap();
        assert!(o.status.success(), "rv {args:?}: {}", text(&o));
        o
    };

    // The cache knows the whole library, the copy holds a sample: a scan tells it so.
    rv(&["scan"]);
    let before_tree = manifest(&work.join("RomRoot"));
    let before_dats = manifest(&work.join("DatRoot"));
    let before_cache = sha256_file(&work.join("RomVault3_3.Cache"));

    let found = text(&rv(&["find-fixes"]));
    eprintln!("{found}");
    let dry = text(&rv(&["fix", "--dry-run"]));
    eprintln!("{dry}");
    assert!(dry.contains("Dry run"), "{dry}");
    assert!(dry.contains("wanted files nothing can make"), "{dry}");

    // Nothing changed: not the library copy, not the DATs, not the cache; and the originals
    // are what their recorded manifests say they are.
    assert_eq!(manifest(&work.join("RomRoot")), before_tree);
    assert_eq!(manifest(&work.join("DatRoot")), before_dats);
    assert_eq!(sha256_file(&work.join("RomVault3_3.Cache")), before_cache);
    assert!(
        !work.join("ToSort").exists(),
        "a dry run made the ToSort directory"
    );

    let mut want = recorded(&fx.join("RomRoot-sample/MANIFEST.sha256"));
    want.remove("MANIFEST.sha256");
    let have: BTreeMap<_, _> = manifest(&fx.join("RomRoot-sample"))
        .into_iter()
        .filter(|(k, _)| k != "MANIFEST.sha256")
        .collect();
    assert_eq!(have, want, "the fixture ROM tree was touched");
    let _ = fs::remove_dir_all(&work);
}
