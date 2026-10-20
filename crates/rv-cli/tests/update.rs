//! `rv update check` / `rv update install` against a mock of the Releases API on 127.0.0.1.
//! The programs replaced are small shell scripts; no test touches the running `rv`, and none
//! reaches any host but the mock.
#![cfg(all(target_os = "linux", target_arch = "x86_64"))]

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use rv_update::mock::{Mock, MockRelease, MockState};

const ASSET: &str = "rv-linux-x86_64";
const OLD: &[u8] = b"#!/bin/sh\necho old\n";
const NEW: &[u8] = b"#!/bin/sh\necho new\n";

struct Work(PathBuf);

impl Work {
    fn new(tag: &str) -> Work {
        let d = std::env::temp_dir().join(format!("rv-cli-update-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        Work(d)
    }
    fn p(&self, rel: &str) -> PathBuf {
        self.0.join(rel)
    }
    fn program(&self) -> PathBuf {
        let p = self.p("bin/rv");
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, OLD).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        p
    }
    fn config(&self, m: &Mock, extra: &str) -> PathBuf {
        let c = self.p("update.conf");
        fs::write(&c, format!("api_base={}\n{extra}", m.base())).unwrap();
        c
    }
    /// Run `rv` with `stdin` as the answers.
    fn rv(&self, args: &[&str], stdin: &str) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_rv"))
            .args(args)
            .env("HOME", &self.0)
            .env_remove("XDG_CONFIG_HOME")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }
}

impl Drop for Work {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn run_script(p: &std::path::Path) -> String {
    for _ in 0..50 {
        match Command::new(p).output() {
            Ok(o) => return text(&o.stdout).trim().to_owned(),
            Err(e) if e.raw_os_error() == Some(26) => {
                std::thread::sleep(std::time::Duration::from_millis(20))
            }
            Err(e) => panic!("{e}"),
        }
    }
    panic!("busy");
}

fn mock(tag: &str) -> Mock {
    Mock::start(MockState {
        releases: vec![MockRelease::new(tag, &[(ASSET, NEW)]).with_sums()],
        ..MockState::default()
    })
}

#[test]
fn version_flag() {
    let w = Work::new("version");
    let o = w.rv(&["--version"], "");
    assert!(o.status.success());
    assert_eq!(
        text(&o.stdout).trim(),
        format!("rv {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn check_says_what_it_found_and_changes_nothing() {
    let w = Work::new("check");
    let target = w.program();
    let m = mock("v99.0.0");
    let c = w.config(&m, "");
    let o = w.rv(
        &["update", "check", "--update-config", c.to_str().unwrap()],
        "",
    );
    assert!(o.status.success(), "{}", text(&o.stderr));
    let out = text(&o.stdout);
    assert!(out.contains("version 99.0.0 is available"), "{out}");
    assert!(out.contains("/release-page/v99.0.0"), "{out}");
    assert_eq!(fs::read(&target).unwrap(), OLD);
    assert_eq!(m.requests(), ["/repos/Dromares/rv-rs/releases/latest"]);
}

#[test]
fn check_when_current_and_when_nothing_is_published() {
    let w = Work::new("current");
    let c_args = |m: &Mock| {
        let c = w.config(m, "");
        vec![
            "update".to_owned(),
            "check".into(),
            "--update-config".into(),
            c.to_string_lossy().into_owned(),
        ]
    };
    let m = mock(&format!("v{}", env!("CARGO_PKG_VERSION")));
    let a = c_args(&m);
    let o = w.rv(&a.iter().map(String::as_str).collect::<Vec<_>>(), "");
    assert!(o.status.success());
    assert!(
        text(&o.stdout).contains("is up to date"),
        "{}",
        text(&o.stdout)
    );
    let m = Mock::start(MockState::default());
    let a = c_args(&m);
    let o = w.rv(&a.iter().map(String::as_str).collect::<Vec<_>>(), "");
    assert!(o.status.success(), "{}", text(&o.stderr));
    assert!(
        text(&o.stdout).contains("no release is published"),
        "{}",
        text(&o.stdout)
    );
}

#[test]
fn install_asks_and_a_no_changes_nothing() {
    let w = Work::new("no");
    let target = w.program();
    let m = mock("v99.0.0");
    let c = w.config(&m, "");
    let args = [
        "update",
        "install",
        "--update-config",
        c.to_str().unwrap(),
        "--target",
        target.to_str().unwrap(),
    ];
    for answer in ["n\n", "\n", "maybe\n"] {
        let o = w.rv(&args, answer);
        assert!(o.status.success(), "{}", text(&o.stderr));
        assert!(
            text(&o.stdout).contains("Not installed."),
            "{}",
            text(&o.stdout)
        );
        assert_eq!(fs::read(&target).unwrap(), OLD);
    }
    // Nothing was downloaded either: only the version check went out.
    assert_eq!(m.requests().len(), 3, "{:?}", m.requests());
    assert!(m.requests().iter().all(|p| !p.starts_with("/download")));
}

#[test]
fn install_without_an_answer_and_without_yes_installs_nothing() {
    let w = Work::new("eof");
    let target = w.program();
    let m = mock("v99.0.0");
    let c = w.config(&m, "");
    let o = w.rv(
        &[
            "update",
            "install",
            "--update-config",
            c.to_str().unwrap(),
            "--target",
            target.to_str().unwrap(),
        ],
        "",
    );
    assert_eq!(o.status.code(), Some(1));
    assert!(text(&o.stderr).contains("--yes"), "{}", text(&o.stderr));
    assert_eq!(fs::read(&target).unwrap(), OLD);
}

#[test]
fn a_yes_installs_and_keeps_a_backup() {
    let w = Work::new("yes");
    let target = w.program();
    let m = mock("v99.0.0");
    let c = w.config(&m, "");
    let o = w.rv(
        &[
            "update",
            "install",
            "--update-config",
            c.to_str().unwrap(),
            "--target",
            target.to_str().unwrap(),
        ],
        "y\n",
    );
    assert!(o.status.success(), "{}", text(&o.stderr));
    assert_eq!(run_script(&target), "new");
    assert_eq!(fs::read(w.p("bin/rv.rv-backup")).unwrap(), OLD);
    assert!(text(&o.stdout).contains("Updated"), "{}", text(&o.stdout));
}

#[test]
fn dash_dash_yes_does_not_ask() {
    let w = Work::new("flag");
    let target = w.program();
    let m = mock("v99.0.0");
    let c = w.config(&m, "");
    let o = w.rv(
        &[
            "update",
            "install",
            "--yes",
            "--update-config",
            c.to_str().unwrap(),
            "--target",
            target.to_str().unwrap(),
        ],
        "",
    );
    assert!(o.status.success(), "{}", text(&o.stderr));
    assert!(!text(&o.stdout).contains("[y/N]"));
    assert_eq!(run_script(&target), "new");
}

#[test]
fn a_bad_download_is_refused_and_the_old_program_stays() {
    let w = Work::new("bad");
    let target = w.program();
    let mut flipped = NEW.to_vec();
    flipped[3] ^= 1;
    let m = Mock::start(MockState {
        releases: vec![MockRelease::new("v99.0.0", &[(ASSET, NEW)])
            .with_sums()
            .replace(ASSET, &flipped)],
        ..MockState::default()
    });
    let c = w.config(&m, "");
    let o = w.rv(
        &[
            "update",
            "install",
            "--yes",
            "--update-config",
            c.to_str().unwrap(),
            "--target",
            target.to_str().unwrap(),
        ],
        "",
    );
    assert_eq!(o.status.code(), Some(1));
    assert!(
        text(&o.stderr).contains("does not match SHA256SUMS"),
        "{}",
        text(&o.stderr)
    );
    assert_eq!(fs::read(&target).unwrap(), OLD);
    assert_eq!(run_script(&target), "old");
}

#[test]
fn a_directory_that_is_not_writable_gets_the_release_page_and_no_escalation() {
    use std::os::unix::fs::PermissionsExt;
    let w = Work::new("ro");
    let target = w.program();
    let dir = target.parent().unwrap().to_owned();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
    if fs::File::create(dir.join("probe")).is_ok() {
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        eprintln!("skipped: this user can write into a 0555 directory (root?)");
        return;
    }
    let m = mock("v99.0.0");
    let c = w.config(&m, "");
    let o = w.rv(
        &[
            "update",
            "install",
            "--yes",
            "--update-config",
            c.to_str().unwrap(),
            "--target",
            target.to_str().unwrap(),
        ],
        "",
    );
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(o.status.code(), Some(1));
    let err = text(&o.stderr);
    assert!(err.contains("does not ask for more rights"), "{err}");
    assert!(err.contains("/release-page/v99.0.0"), "{err}");
    assert_eq!(fs::read(&target).unwrap(), OLD);
}

#[test]
fn the_live_library_guard_covers_the_target() {
    let w = Work::new("guard");
    let m = mock("v99.0.0");
    let c = w.config(&m, "");
    let o = w.rv(
        &[
            "update",
            "install",
            "--yes",
            "--update-config",
            c.to_str().unwrap(),
            "--target",
            "/mnt/HHD16TB/x/rv",
        ],
        "",
    );
    assert_eq!(o.status.code(), Some(3), "{}", text(&o.stderr));
    assert!(m.requests().is_empty());
}
