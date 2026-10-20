//! The `rv` binary on a synthetic library: DATs, then files that are missing, misnamed, in the
//! wrong archive, unneeded, in ToSort, corrupt and correct, and the whole pipeline
//! `update-dats -> scan -> find-fixes -> fix`.
//!
//! No test here passes `--allow-live-library`, and none names a path under `/mnt/HHD16TB/` except
//! the refusal tests, which give it as text only.

use std::collections::BTreeMap;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use rv_archive::zip::{TorrentZipWriter, ZipArchive, ZipStructure};
use rv_scan::{HashKinds, HashSet};

const A: &[u8] = b"rom a contents rom a contents";
const B: &[u8] = b"second rom is a bit longer";
const C: &[u8] = b"third";
const D: &[u8] = b"fourth rom data data data";
const E: &[u8] = b"fifth rom is a different one";
const F: &[u8] = b"sixth";
const G: &[u8] = b"seventh rom lives in ToSort";
const H: &[u8] = b"eighth rom";
const J: &[u8] = b"junk junk junk";

struct Work(PathBuf);

impl Work {
    fn new(tag: &str) -> Work {
        let base = std::env::var_os("RV_WORK_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let dir = base.join(format!("rv-cli-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Work(dir)
    }
    fn p(&self, rel: &str) -> PathBuf {
        self.0.join(rel)
    }
    fn write(&self, rel: &str, bytes: &[u8]) {
        let p = self.p(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, bytes).unwrap();
    }
    fn zip(&self, rel: &str, entries: &[(&str, &[u8])]) {
        self.write(rel, &zip_bytes(entries));
    }
    fn rv(&self, args: &[&str]) -> Output {
        let mut c = Command::new(env!("CARGO_BIN_EXE_rv"));
        c.arg(args[0]);
        c.args([
            "--cache",
            self.p("RomVault3_3.Cache").to_str().unwrap(),
            "--root",
            self.p("RomRoot").to_str().unwrap(),
            "--tosort",
            self.p("ToSort").to_str().unwrap(),
            "--dat-root",
            self.p("DatRoot").to_str().unwrap(),
        ]);
        c.args(&args[1..]);
        c.output().unwrap()
    }
}

impl Drop for Work {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut e = entries.to_vec();
    e.sort_by(|a, b| rv_archive::sort::trrntzip_compare_case(a.0, b.0));
    let mut w = TorrentZipWriter::new(Cursor::new(Vec::new()), ZipStructure::ZipTrrnt).unwrap();
    for (n, d) in e {
        w.add_bytes(n, d, None).unwrap();
    }
    w.finish().unwrap().into_inner()
}

fn crc(data: &[u8]) -> String {
    let mut h = HashSet::new(HashKinds::ROMVAULT);
    h.update(data);
    rv_scan::hash::to_hex(&h.finish().crc.unwrap())
}

type Game<'a> = (&'a str, &'a [(&'a str, &'a [u8])]);

fn dat(games: &[Game<'_>]) -> String {
    let mut s = "clrmamepro (\n\tname \"Synthetic\"\n)\n".to_owned();
    for (name, roms) in games {
        s.push_str(&format!("game (\n\tname \"{name}\"\n"));
        for (r, d) in *roms {
            s.push_str(&format!(
                "\trom ( name \"{r}\" size {} crc {} )\n",
                d.len(),
                crc(d)
            ));
        }
        s.push_str(")\n");
    }
    s
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn manifest(root: &Path) -> BTreeMap<String, String> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, String>) {
        let Ok(rd) = fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(root, &p, out);
            } else {
                let mut h = HashSet::new(HashKinds::ALL);
                h.update(&fs::read(&p).unwrap());
                out.insert(
                    p.strip_prefix(root).unwrap().to_string_lossy().into_owned(),
                    rv_scan::hash::to_hex(&h.finish().sha256.unwrap()),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

/// The library of the ticket: one of everything that can be wrong.
fn library(w: &Work) {
    w.write(
        "DatRoot/Console/NES/Roms/nes.dat",
        dat(&[
            ("Alpha", &[("a.nes", A), ("b.nes", B)]), // correct
            ("Beta", &[("c.nes", C)]),                // missing, its rom is in Wrong.zip
            ("Gamma", &[("d.nes", D), ("e.nes", E)]), // e.nes is misnamed inside
            ("Delta", &[("f.nes", F)]),               // the file is corrupt
            ("Epsilon", &[("g.nes", G)]),             // its rom is in ToSort
            ("Zeta", &[("h.nes", H)]),                // right, plus an unneeded entry
        ])
        .as_bytes(),
    );
    w.zip(
        "RomRoot/Console/NES/Roms/Alpha.zip",
        &[("a.nes", A), ("b.nes", B)],
    );
    w.zip(
        "RomRoot/Console/NES/Roms/Wrong.zip",
        &[("c.nes", C), ("j.bin", J)],
    );
    w.zip(
        "RomRoot/Console/NES/Roms/Gamma.zip",
        &[("d.nes", D), ("e_misnamed.nes", E)],
    );
    w.write(
        "RomRoot/Console/NES/Roms/Delta.zip",
        b"this is not a zip file at all",
    );
    w.zip(
        "RomRoot/Console/NES/Roms/Zeta.zip",
        &[("h.nes", H), ("extra.bin", b"extra bytes here")],
    );
    w.zip(
        "RomRoot/Console/NES/Roms/Stranger.zip",
        &[("x.bin", b"nobody wants this")],
    );
    w.zip("ToSort/Console/NES/Roms/Epsilon_src.zip", &[("g.nes", G)]);
}

#[test]
fn the_whole_pipeline_leaves_everything_correct_or_missing() {
    let w = Work::new("pipeline");
    library(&w);

    let o = w.rv(&["update-dats"]);
    assert!(o.status.success(), "{}", text(&o));
    let o = w.rv(&["scan"]);
    assert!(o.status.success(), "{}", text(&o));

    // Find Fixes only says what it would do, and changes nothing on disk or in the cache.
    let before = manifest(&w.0);
    let o = w.rv(&["find-fixes"]);
    assert!(o.status.success(), "{}", text(&o));
    let said = text(&o);
    assert!(said.contains("make archive entry"), "{said}");
    assert_eq!(manifest(&w.0), before, "find-fixes wrote something");

    let o = w.rv(&["fix"]);
    assert!(o.status.success(), "{}", text(&o));
    let said = text(&o);
    assert!(said.contains("Fix done"), "{said}");

    // A second Find Fixes has nothing left to do except the one game nobody has a copy of.
    let o = w.rv(&["find-fixes"]);
    let again = text(&o);
    assert!(again.contains("nothing to do"), "{again}");
    assert!(
        again.contains("1  wanted files nothing can make"),
        "{again}"
    );

    // After a full scan the cache says what is on disk, and every library zip is a TorrentZip
    // that rv-archive reads and verifies.
    let o = w.rv(&["scan", "--level", "3"]);
    assert!(o.status.success(), "{}", text(&o));
    let o = w.rv(&["status"]);
    let counts = text(&o);
    assert!(counts.contains("Correct"), "{counts}");
    for bad in [
        "CanBeFixed",
        "MoveToSort",
        "NeededForFix",
        "Unknown",
        "Corrupt ",
    ] {
        assert!(
            !counts
                .lines()
                .any(|l| l.trim_end().ends_with(bad.trim_end()) && !l.contains("DirCorrupt")),
            "{bad} is still there:\n{counts}"
        );
    }
    for name in ["Alpha", "Beta", "Gamma", "Epsilon", "Zeta"] {
        let path = w.p(&format!("RomRoot/Console/NES/Roms/{name}.zip"));
        let mut z = ZipArchive::open(Cursor::new(fs::read(&path).unwrap()))
            .unwrap_or_else(|e| panic!("{name}.zip: {e}"));
        assert_eq!(z.structure(), ZipStructure::ZipTrrnt, "{name}.zip");
        for i in 0..z.len() {
            z.verify_entry(i).unwrap();
        }
    }
    // What was wrong is kept, not lost.
    assert!(w.p("ToSort/Console/NES/Roms/Stranger.zip").exists());
    assert!(w.p("ToSort/Console/NES/Roms/Wrong.zip").exists());
    assert!(w.p("ToSort/Console/NES/Roms/Zeta.zip").exists());
    assert!(w.p("ToSort/Corrupt/Delta.zip").exists());
    assert!(!w.p("RomRoot/Console/NES/Roms/Delta.zip").exists());
    assert_eq!(
        fs::read(w.p("ToSort/Corrupt/Delta.zip")).unwrap(),
        b"this is not a zip file at all"
    );
}

#[test]
fn a_dry_run_writes_nothing_at_all() {
    let w = Work::new("dry");
    library(&w);
    assert!(w.rv(&["update-dats"]).status.success());
    assert!(w.rv(&["scan"]).status.success());
    let before = manifest(&w.0);
    let o = w.rv(&["fix", "--dry-run"]);
    assert!(o.status.success(), "{}", text(&o));
    let said = text(&o);
    assert!(said.contains("Dry run"), "{said}");
    assert!(said.contains("rebuild archive"), "{said}");
    assert_eq!(
        manifest(&w.0),
        before,
        "the tree and the cache are exactly as they were"
    );
}

#[test]
fn report_writes_fixdats_into_its_own_directory_only() {
    let w = Work::new("report");
    library(&w);
    assert!(w.rv(&["update-dats"]).status.success());
    assert!(w.rv(&["scan"]).status.success());
    let before = manifest(&w.p("RomRoot"));
    let cache_before = fs::read(w.p("RomVault3_3.Cache")).unwrap();
    let o = w.rv(&["report", "--out", w.p("fixdats").to_str().unwrap()]);
    assert!(o.status.success(), "{}", text(&o));
    assert_eq!(manifest(&w.p("RomRoot")), before);
    assert_eq!(fs::read(w.p("RomVault3_3.Cache")).unwrap(), cache_before);
    let files: Vec<_> = fs::read_dir(w.p("fixdats")).unwrap().flatten().collect();
    assert_eq!(files.len(), 1);
    let t = fs::read_to_string(files[0].path()).unwrap();
    assert!(
        t.contains("FixDat_") && t.contains("Delta") && t.contains("f.nes"),
        "{t}"
    );
    // Beta can be fixed from Wrong.zip, so by default it is not listed.
    assert!(!t.contains("c.nes"), "{t}");
}

// ---- the live library is refused -------------------------------------------------------------

fn refused(args: &[&str]) -> Output {
    let o = Command::new(env!("CARGO_BIN_EXE_rv"))
        .args(args)
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(3), "{}", text(&o));
    assert!(text(&o).contains("Refusing to start"), "{}", text(&o));
    o
}

#[test]
fn a_path_inside_the_live_library_is_refused_before_anything_is_read() {
    // Text only: these paths are never looked at.
    refused(&[
        "status",
        "--cache",
        "/mnt/HHD16TB/RomVault3_3.Cache",
        "--root",
        "/tmp/x",
    ]);
    refused(&[
        "scan",
        "--cache",
        "/tmp/x.Cache",
        "--root",
        "/mnt/HHD16TB/RomVault/RomRoot",
    ]);
    refused(&[
        "fix",
        "--dry-run",
        "--cache",
        "/tmp/x.Cache",
        "--root",
        "/tmp/r",
        "--tosort",
        "/mnt/HHD16TB/ToSort",
    ]);
    refused(&[
        "update-dats",
        "--cache",
        "/tmp/x.Cache",
        "--root",
        "/tmp/r",
        "--dat-root",
        "/mnt/HHD16TB/DatRoot",
    ]);
    refused(&[
        "report",
        "--cache",
        "/tmp/x.Cache",
        "--root",
        "/tmp/r",
        "--out",
        "/mnt/HHD16TB/fixdats",
    ]);
    // Dressed up with `..`.
    refused(&[
        "status",
        "--cache",
        "/tmp/../mnt/HHD16TB/x.Cache",
        "--root",
        "/tmp/r",
    ]);
}

#[test]
fn a_config_that_maps_into_the_live_library_is_refused() {
    let w = Work::new("cfg");
    w.write(
        "bad.xml",
        br#"<?xml version="1.0"?><Settings><DatRoot>DatRoot</DatRoot><CacheFile>c.Cache</CacheFile>
<DirMappings><DirMapping><DirKey>RomVault/Arcade</DirKey><DirPath>/mnt/HHD16TB/Arcade</DirPath></DirMapping></DirMappings>
</Settings>"#,
    );
    let o = Command::new(env!("CARGO_BIN_EXE_rv"))
        .args(["status", "--cache"])
        .arg(w.p("c.Cache"))
        .arg("--root")
        .arg(w.p("RomRoot"))
        .arg("--config")
        .arg(w.p("bad.xml"))
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(3), "{}", text(&o));
    assert!(text(&o).contains("RomVault/Arcade"), "{}", text(&o));
}

#[test]
fn nothing_has_a_default_path_and_bad_usage_is_not_a_run() {
    for args in [
        vec!["scan"],
        vec!["scan", "--cache", "/tmp/c"],
        vec!["scan", "--root", "/tmp/r"],
        vec!["update-dats", "--cache", "/tmp/c", "--root", "/tmp/r"],
        vec!["report", "--cache", "/tmp/c", "--root", "/tmp/r"],
        vec!["frobnicate"],
        vec!["scan", "--dry-run", "--cache", "/tmp/c", "--root", "/tmp/r"],
    ] {
        let o = Command::new(env!("CARGO_BIN_EXE_rv"))
            .args(&args)
            .output()
            .unwrap();
        assert_eq!(o.status.code(), Some(2), "{args:?}: {}", text(&o));
    }
    let o = Command::new(env!("CARGO_BIN_EXE_rv"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(o.status.success());
    assert!(text(&o).contains("nothing has a default"));
}
