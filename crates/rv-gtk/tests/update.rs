//! The engine half of "Check for updates" and the install behind its Install button, against a
//! mock of the Releases API on 127.0.0.1 (the window itself is not needed to test them).
#![cfg(all(target_os = "linux", target_arch = "x86_64"))]

use std::fs;
use std::path::PathBuf;

use rv_gtk::engine;
use rv_update::mock::{Mock, MockRelease, MockState};
use rv_update::Check;

const ASSET: &str = "rv-gtk-linux-x86_64";
const NEW: &[u8] = b"#!/bin/sh\necho new\n";

struct Work(PathBuf);

impl Work {
    fn new(tag: &str) -> Work {
        let d = std::env::temp_dir().join(format!("rv-gtk-update-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        Work(d)
    }
    fn conf(&self, m: &Mock, extra: &str) -> PathBuf {
        let c = self.0.join("update.conf");
        fs::write(&c, format!("api_base={}\n{extra}", m.base())).unwrap();
        c
    }
}

impl Drop for Work {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn mock(tag: &str, sums: bool) -> Mock {
    let r = MockRelease::new(tag, &[(ASSET, NEW)]);
    Mock::start(MockState {
        releases: vec![if sums { r.with_sums() } else { r }],
        ..MockState::default()
    })
}

#[test]
fn the_check_at_start_is_off_unless_the_setting_says_so() {
    let w = Work::new("startup");
    assert!(!engine::check_on_startup(&w.0.join("missing.conf")));
    let m = mock("v99.0.0", true);
    assert!(!engine::check_on_startup(&w.conf(&m, "")));
    assert!(engine::check_on_startup(
        &w.conf(&m, "check_on_startup=1\n")
    ));
}

#[test]
fn a_newer_release_is_found_and_described() {
    let w = Work::new("found");
    let m = mock("v99.0.0", true);
    let Ok(Check::Available(up)) = engine::check_for_updates(&w.conf(&m, "")) else {
        panic!("expected an update")
    };
    let (message, detail) = engine::update_question(&up, std::path::Path::new("/opt/rv/rv-gtk"));
    assert_eq!(message, "Version 99.0.0 is available");
    for needle in [
        "SHA256SUMS",
        "/opt/rv/rv-gtk",
        "rv-gtk.rv-backup",
        "/release-page/v99.0.0",
    ] {
        assert!(detail.contains(needle), "{needle} missing from: {detail}");
    }
}

#[test]
fn nothing_published_is_not_an_error() {
    let w = Work::new("none");
    let m = Mock::start(MockState::default());
    assert!(matches!(
        engine::check_for_updates(&w.conf(&m, "")),
        Ok(Check::NothingPublished)
    ));
}

#[test]
fn the_install_replaces_the_target_and_nothing_else() {
    let w = Work::new("install");
    let m = mock("v99.0.0", true);
    let Ok(Check::Available(up)) = engine::check_for_updates(&w.conf(&m, "")) else {
        panic!()
    };
    let target = w.0.join("rv-gtk");
    fs::write(&target, b"#!/bin/sh\necho old\n").unwrap();
    let done = engine::install_update(&up, &target).unwrap();
    assert_eq!(fs::read(&target).unwrap(), NEW);
    assert_eq!(
        fs::read(done.backup.unwrap()).unwrap(),
        b"#!/bin/sh\necho old\n"
    );
}

#[test]
fn an_install_without_sha256sums_is_refused_and_says_so() {
    let w = Work::new("nosums");
    let m = mock("v99.0.0", false);
    let Ok(Check::Available(up)) = engine::check_for_updates(&w.conf(&m, "")) else {
        panic!()
    };
    let target = w.0.join("rv-gtk");
    fs::write(&target, b"old").unwrap();
    let e = engine::install_update(&up, &target).unwrap_err();
    assert!(e.contains("SHA256SUMS"), "{e}");
    assert_eq!(fs::read(&target).unwrap(), b"old");
}
