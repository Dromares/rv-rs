//! The engine half of "Check for updates" and the install behind its Install button, against a
//! mock of the Releases API on 127.0.0.1 (the window itself is not needed to test them).
#![cfg(all(target_os = "linux", target_arch = "x86_64"))]

use std::fs;
use std::path::PathBuf;

use rv_gtk::engine::{self, UpdateTarget};
use rv_update::mock::{Mock, MockRelease, MockState};
use rv_update::Check;

const ASSET: &str = "rv-gtk-linux-x86_64";
const NEW: &[u8] = b"#!/bin/sh\necho new\n";
const APPIMAGE_ASSET: &str = "rv-gtk-x86_64.AppImage";
const NEW_APPIMAGE: &[u8] = b"#!/bin/sh\necho new appimage\n";
const OLD_APPIMAGE: &[u8] = b"#!/bin/sh\necho old appimage\n";

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

// ---- rv-gtk run from an AppImage -------------------------------------------------------------

/// A release that has both files (0.2.2 and later).
fn mock_with_appimage(tag: &str) -> Mock {
    Mock::start(MockState {
        releases: vec![
            MockRelease::new(tag, &[(ASSET, NEW), (APPIMAGE_ASSET, NEW_APPIMAGE)]).with_sums(),
        ],
        ..MockState::default()
    })
}

fn available(w: &Work, m: &Mock) -> rv_update::Update {
    let Ok(Check::Available(up)) = engine::check_for_updates(&w.conf(m, "")) else {
        panic!("expected an update")
    };
    up
}

fn downloads(m: &Mock) -> Vec<String> {
    m.requests()
        .into_iter()
        .filter(|p| p.starts_with("/download/"))
        .collect()
}

#[test]
fn the_target_is_the_appimage_file_when_appimage_names_one_else_the_binary() {
    let w = Work::new("target");
    let image = w.0.join(APPIMAGE_ASSET);
    fs::write(&image, OLD_APPIMAGE).unwrap();
    let exe = || Ok(PathBuf::from("/tmp/.mount_rv-gtkAbCd/usr/bin/rv-gtk"));
    // `APPIMAGE` names an existing file: that file, not the squashfs path of current_exe().
    assert_eq!(
        engine::update_target_from(
            rv_update::appimage::path_from(Some(image.clone().into())),
            exe()
        ),
        Ok(UpdateTarget::AppImage(image))
    );
    // Unset, empty, or naming nothing: the binary, as before.
    for var in [
        None,
        Some("".into()),
        Some(w.0.join("gone").into_os_string()),
    ] {
        assert_eq!(
            engine::update_target_from(rv_update::appimage::path_from(var), exe()),
            Ok(UpdateTarget::Binary(PathBuf::from(
                "/tmp/.mount_rv-gtkAbCd/usr/bin/rv-gtk"
            )))
        );
    }
}

#[test]
fn an_appimage_is_updated_from_the_appimage_asset_and_the_bare_binary_is_not_fetched() {
    let w = Work::new("ai-install");
    let m = mock_with_appimage("v99.0.0");
    let up = available(&w, &m);
    let image = w.0.join(APPIMAGE_ASSET);
    fs::write(&image, OLD_APPIMAGE).unwrap();
    let q = engine::update_question_for(&up, &UpdateTarget::AppImage(image.clone()));
    assert!(q.can_install);
    for needle in [
        APPIMAGE_ASSET,
        "SHA256SUMS",
        "rv-gtk-x86_64.AppImage.rv-backup",
    ] {
        assert!(
            q.detail.contains(needle),
            "{needle} missing from: {}",
            q.detail
        );
    }
    let done = engine::install_update_for(&up, &UpdateTarget::AppImage(image.clone())).unwrap();
    assert_eq!(done.target, image);
    assert_eq!(fs::read(&image).unwrap(), NEW_APPIMAGE);
    assert_eq!(fs::read(done.backup.unwrap()).unwrap(), OLD_APPIMAGE);
    let got = downloads(&m);
    assert!(got.iter().any(|p| p.ends_with(APPIMAGE_ASSET)), "{got:?}");
    assert!(got.iter().any(|p| p.ends_with("SHA256SUMS")), "{got:?}");
    assert!(!got.iter().any(|p| p.ends_with(ASSET)), "{got:?}");
}

#[test]
fn an_appimage_update_keeps_the_mode_adds_execute_and_leaves_no_temporary_file() {
    use std::os::unix::fs::PermissionsExt;
    let w = Work::new("ai-mode");
    let m = mock_with_appimage("v99.0.0");
    let up = available(&w, &m);
    let dir = w.0.join("apps");
    fs::create_dir_all(&dir).unwrap();
    let image = dir.join(APPIMAGE_ASSET);
    fs::write(&image, OLD_APPIMAGE).unwrap();
    fs::set_permissions(&image, fs::Permissions::from_mode(0o640)).unwrap();
    engine::install_update_for(&up, &UpdateTarget::AppImage(image.clone())).unwrap();
    assert_eq!(
        fs::metadata(&image).unwrap().permissions().mode() & 0o777,
        0o751
    );
    let mut names: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [APPIMAGE_ASSET, "rv-gtk-x86_64.AppImage.rv-backup"],
        "no .rv-update.tmp is left"
    );
}

#[test]
fn a_release_without_an_appimage_is_reported_and_never_put_over_the_appimage() {
    // 0.2.1 and earlier: rv-gtk-linux-x86_64 and SHA256SUMS only.
    let w = Work::new("ai-none");
    let m = mock("v99.0.0", true);
    let up = available(&w, &m);
    let image = w.0.join(APPIMAGE_ASSET);
    fs::write(&image, OLD_APPIMAGE).unwrap();
    let target = UpdateTarget::AppImage(image.clone());

    let q = engine::update_question_for(&up, &target);
    assert!(!q.can_install, "no Install button without an AppImage");
    assert!(q.message.contains("not as an AppImage"), "{}", q.message);
    assert!(
        q.detail.contains("no AppImage in release v99.0.0"),
        "{}",
        q.detail
    );
    assert!(q.detail.contains("/release-page/v99.0.0"), "{}", q.detail);

    // Even called anyway, the install refuses, says so with the page, and touches nothing.
    let e = engine::install_update_for(&up, &target).unwrap_err();
    assert!(e.contains("no AppImage in release v99.0.0"), "{e}");
    assert!(e.contains("/release-page/v99.0.0"), "{e}");
    assert_eq!(fs::read(&image).unwrap(), OLD_APPIMAGE);
    assert!(!rv_update::install::backup_path(&image).exists());
    assert!(
        downloads(&m).iter().all(|p| !p.ends_with(ASSET)),
        "{:?}",
        downloads(&m)
    );
}

#[test]
fn an_appimage_in_a_read_only_directory_installs_nothing_and_names_the_release_page() {
    use std::os::unix::fs::PermissionsExt;
    let w = Work::new("ai-readonly");
    let m = mock_with_appimage("v99.0.0");
    let up = available(&w, &m);
    let dir = w.0.join("apps");
    fs::create_dir_all(&dir).unwrap();
    let image = dir.join(APPIMAGE_ASSET);
    fs::write(&image, OLD_APPIMAGE).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
    if fs::File::create(dir.join("probe")).is_ok() {
        eprintln!("skipped: this user can write into a 0555 directory (root?)");
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }
    let e = engine::install_update_for(&up, &UpdateTarget::AppImage(image.clone())).unwrap_err();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(e.contains("You cannot write in"), "{e}");
    assert!(e.contains("/release-page/v99.0.0"), "{e}");
    assert_eq!(fs::read(&image).unwrap(), OLD_APPIMAGE);
    assert!(
        downloads(&m).is_empty(),
        "stopped at the probe: {:?}",
        downloads(&m)
    );
}

#[test]
fn a_plain_binary_still_takes_the_binary_asset_when_the_release_has_an_appimage_too() {
    let w = Work::new("bin-with-ai");
    let m = mock_with_appimage("v99.0.0");
    let up = available(&w, &m);
    let target = w.0.join("rv-gtk");
    fs::write(&target, b"old").unwrap();
    let done = engine::install_update_for(&up, &UpdateTarget::Binary(target.clone())).unwrap();
    assert_eq!(fs::read(&target).unwrap(), NEW);
    assert_eq!(fs::read(done.backup.unwrap()).unwrap(), b"old");
    let got = downloads(&m);
    assert!(got.iter().any(|p| p.ends_with(ASSET)), "{got:?}");
    assert!(!got.iter().any(|p| p.ends_with(APPIMAGE_ASSET)), "{got:?}");
    // Its question is the one it always was.
    let q = engine::update_question_for(&up, &UpdateTarget::Binary(target.clone()));
    assert_eq!((q.message, q.detail), engine::update_question(&up, &target));
    assert!(q.can_install);
}
