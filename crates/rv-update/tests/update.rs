//! The update flow against a mock of the Releases API on 127.0.0.1 (DAN-1823).
//!
//! The "programs" are small shell scripts that print their name, so the tests can run what the
//! install left behind.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use rv_update::install::{backup_path, install, temp_path, Step};
use rv_update::mock::{Mock, MockRelease, MockState};
use rv_update::{check, Check, Error, Update, UpdateConfig, Version};

const ASSET: &str = "rv-linux-x86_64";
const OLD: &[u8] = b"#!/bin/sh\necho old\n";
const NEW: &[u8] = b"#!/bin/sh\necho new\n";

struct Dir(PathBuf);

impl Dir {
    fn new(tag: &str) -> Dir {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!(
            "rv-update-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        Dir(d)
    }

    /// An installed "program" with the old content.
    fn program(&self) -> PathBuf {
        let p = self.0.join("rv");
        fs::write(&p, OLD).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        }
        p
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        // Put back the rights some tests take away, or the clean-up fails.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
        }
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn run(p: &Path) -> String {
    // A freshly written script can be "text file busy" for a moment when another test thread
    // forks at the same instant; try again.
    for _ in 0..50 {
        match Command::new(p).output() {
            Ok(o) => return String::from_utf8_lossy(&o.stdout).trim().to_owned(),
            Err(e) if e.raw_os_error() == Some(26) => {
                std::thread::sleep(std::time::Duration::from_millis(20))
            }
            Err(e) => panic!("cannot run {}: {e}", p.display()),
        }
    }
    panic!("{} stayed busy", p.display());
}

fn release(tag: &str) -> MockRelease {
    MockRelease::new(tag, &[(ASSET, NEW)]).with_sums()
}

fn cfg(mock: &Mock, prereleases: bool) -> UpdateConfig {
    UpdateConfig {
        include_prereleases: prereleases,
        api_base: Some(mock.base()),
        ..UpdateConfig::default()
    }
}

fn v(s: &str) -> Version {
    Version::parse(s).unwrap()
}

fn serve(releases: Vec<MockRelease>) -> Mock {
    Mock::start(MockState {
        releases,
        ..MockState::default()
    })
}

fn available(mock: &Mock, current: &str) -> Update {
    match check(&cfg(mock, false), &v(current)).unwrap() {
        Check::Available(u) => u,
        other => panic!("expected an update, got {other:?}"),
    }
}

fn nothing(_: Step) {}

/// After a refusal: the program is exactly the old one, still runs, and no temporary is left.
fn assert_untouched(target: &Path) {
    assert_eq!(fs::read(target).unwrap(), OLD);
    assert_eq!(run(target), "old");
    assert!(!temp_path(target).exists(), "the temporary file is left");
}

// ---- the version check -------------------------------------------------------------------

#[test]
fn a_newer_release_is_offered() {
    let m = serve(vec![release("v0.2.0")]);
    let u = available(&m, "0.1.0");
    assert_eq!(u.version, v("0.2.0"));
    assert_eq!(u.tag, "v0.2.0");
    assert!(u.page.ends_with("/release-page/v0.2.0"));
    assert!(u.asset(ASSET).is_some() && u.asset("SHA256SUMS").is_some());
}

#[test]
fn the_same_version_is_up_to_date() {
    let m = serve(vec![release("v0.2.0")]);
    assert_eq!(
        check(&cfg(&m, false), &v("0.2.0")).unwrap(),
        Check::UpToDate {
            current: v("0.2.0"),
            latest: v("0.2.0")
        }
    );
}

#[test]
fn an_older_release_is_up_to_date_too() {
    let m = serve(vec![release("v0.2.0")]);
    assert_eq!(
        check(&cfg(&m, false), &v("0.3.0")).unwrap(),
        Check::UpToDate {
            current: v("0.3.0"),
            latest: v("0.2.0")
        }
    );
}

#[test]
fn prereleases_are_ignored_unless_the_setting_is_on() {
    let m = serve(vec![release("v0.3.0-rc.1"), release("v0.2.0")]);
    // Off: the stable 0.2.0 is the newest that counts, and it is newer than 0.1.0 ...
    assert_eq!(available(&m, "0.1.0").version, v("0.2.0"));
    // ... and not newer than 0.2.0, though 0.3.0-rc.1 exists.
    assert!(matches!(
        check(&cfg(&m, false), &v("0.2.0")).unwrap(),
        Check::UpToDate { .. }
    ));
    // On: the release candidate wins.
    match check(&cfg(&m, true), &v("0.2.0")).unwrap() {
        Check::Available(u) => assert_eq!(u.version, v("0.3.0-rc.1")),
        other => panic!("{other:?}"),
    }
}

#[test]
fn only_a_prerelease_published_is_nothing_for_the_default_setting() {
    let m = serve(vec![release("v0.3.0-rc.1")]);
    assert_eq!(
        check(&cfg(&m, false), &v("0.1.0")).unwrap(),
        Check::NothingPublished
    );
    assert!(matches!(
        check(&cfg(&m, true), &v("0.1.0")).unwrap(),
        Check::Available(_)
    ));
}

#[test]
fn a_flagged_prerelease_with_a_plain_version_is_ignored_too() {
    let mut r = release("v0.3.0");
    r.prerelease = true;
    let m = serve(vec![r]);
    assert_eq!(
        check(&cfg(&m, false), &v("0.1.0")).unwrap(),
        Check::NothingPublished
    );
}

#[test]
fn no_releases_at_all_is_a_normal_answer() {
    let m = serve(vec![]);
    for pre in [false, true] {
        assert_eq!(
            check(&cfg(&m, pre), &v("0.1.0")).unwrap(),
            Check::NothingPublished,
            "prereleases {pre}"
        );
    }
}

#[test]
fn the_check_is_anonymous_and_touches_only_the_api() {
    let m = serve(vec![release("v0.2.0")]);
    available(&m, "0.1.0");
    assert_eq!(m.requests(), ["/repos/Dromares/rv-rs/releases/latest"]);
}

// ---- the install --------------------------------------------------------------------------

#[test]
fn a_good_install_replaces_the_program_and_keeps_the_old_one() {
    let m = serve(vec![release("v0.2.0")]);
    let u = available(&m, "0.1.0");
    let d = Dir::new("good");
    let target = d.program();
    let done = install(&u, ASSET, &target, &nothing).unwrap();
    assert_eq!(done.version, v("0.2.0"));
    assert_eq!(fs::read(&target).unwrap(), NEW);
    assert_eq!(run(&target), "new");
    let backup = done.backup.unwrap();
    assert_eq!(backup, backup_path(&target));
    assert_eq!(fs::read(&backup).unwrap(), OLD);
    assert!(!temp_path(&target).exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }
}

#[test]
fn downloads_that_redirect_are_followed() {
    let m = Mock::start(MockState {
        releases: vec![release("v0.2.0")],
        redirect_downloads: true,
        ..MockState::default()
    });
    let u = available(&m, "0.1.0");
    let d = Dir::new("redirect");
    let target = d.program();
    install(&u, ASSET, &target, &nothing).unwrap();
    assert_eq!(run(&target), "new");
    assert!(m.requests().iter().any(|p| p.starts_with("/cdn/")));
}

#[test]
fn no_file_for_this_system_installs_nothing() {
    let m = serve(vec![release("v0.2.0")]);
    let u = available(&m, "0.1.0");
    let d = Dir::new("noasset");
    let target = d.program();
    let e = install(&u, "rv-linux-aarch64", &target, &nothing).unwrap_err();
    assert!(matches!(e, Error::NoAsset { .. }), "{e}");
    assert_untouched(&target);
}

#[test]
fn a_release_without_sha256sums_is_refused() {
    let m = serve(vec![MockRelease::new("v0.2.0", &[(ASSET, NEW)])]);
    let u = available(&m, "0.1.0");
    let d = Dir::new("nosums");
    let target = d.program();
    let e = install(&u, ASSET, &target, &nothing).unwrap_err();
    assert!(matches!(e, Error::SumsMissing { .. }), "{e}");
    assert_untouched(&target);
    assert!(!backup_path(&target).exists());
    // It did not even download the program.
    assert!(m.requests().iter().all(|p| !p.ends_with(ASSET)));
}

#[test]
fn sha256sums_without_a_line_for_the_file_is_refused() {
    let mut r = MockRelease::new("v0.2.0", &[(ASSET, NEW)]);
    r.files.push((
        "SHA256SUMS".into(),
        format!("{}  rv-gtk-linux-x86_64\n", "a".repeat(64)).into_bytes(),
    ));
    let m = serve(vec![r]);
    let u = available(&m, "0.1.0");
    let d = Dir::new("noline");
    let target = d.program();
    let e = install(&u, ASSET, &target, &nothing).unwrap_err();
    assert!(matches!(e, Error::SumsNoEntry { .. }), "{e}");
    assert_untouched(&target);
}

#[test]
fn an_unreadable_sha256sums_is_refused() {
    let r = release("v0.2.0").replace("SHA256SUMS", b"this is not a checksum file\n");
    let m = serve(vec![r]);
    let u = available(&m, "0.1.0");
    let d = Dir::new("badsums");
    let target = d.program();
    let e = install(&u, ASSET, &target, &nothing).unwrap_err();
    assert!(matches!(e, Error::SumsBad(_)), "{e}");
    assert_untouched(&target);
}

#[test]
fn a_hash_mismatch_is_refused() {
    // The sums are those of the real file; the server hands out different bytes of the same
    // length (one byte flipped).
    let mut flipped = NEW.to_vec();
    flipped[3] ^= 1;
    let r = release("v0.2.0").replace(ASSET, &flipped);
    let m = serve(vec![r]);
    let u = available(&m, "0.1.0");
    let d = Dir::new("mismatch");
    let target = d.program();
    // `replace` changed the file but not the sums: they still name NEW.
    let e = install(&u, ASSET, &target, &nothing).unwrap_err();
    assert!(matches!(e, Error::HashMismatch { .. }), "{e}");
    assert_untouched(&target);
    assert!(!backup_path(&target).exists());
}

#[test]
fn a_truncated_download_is_refused() {
    let m = Mock::start(MockState {
        releases: vec![release("v0.2.0")],
        truncate_assets_to: Some(7),
        ..MockState::default()
    });
    let u = available(&m, "0.1.0");
    let d = Dir::new("truncated");
    let target = d.program();
    let e = install(&u, ASSET, &target, &nothing).unwrap_err();
    assert!(
        matches!(
            e,
            Error::Network(_) | Error::SizeMismatch { .. } | Error::HashMismatch { .. }
        ),
        "{e}"
    );
    assert_untouched(&target);
}

#[test]
fn a_directory_without_write_access_is_reported_not_escalated() {
    use std::os::unix::fs::PermissionsExt;
    let m = serve(vec![release("v0.2.0")]);
    let u = available(&m, "0.1.0");
    let d = Dir::new("readonly");
    let target = d.program();
    fs::set_permissions(&d.0, fs::Permissions::from_mode(0o555)).unwrap();
    if fs::File::create(d.0.join("probe")).is_ok() {
        // Running as root: the directory is writable whatever its mode says.
        eprintln!("skipped: this user can write into a 0555 directory (root?)");
        return;
    }
    let e = install(&u, ASSET, &target, &nothing).unwrap_err();
    assert!(matches!(e, Error::NotWritable { .. }), "{e}");
    assert_eq!(fs::read(&target).unwrap(), OLD);
    // Nothing was fetched: it stopped at the probe.
    assert!(m.requests().iter().all(|p| !p.starts_with("/download")));
}

// ---- failures in the middle ---------------------------------------------------------------

#[test]
fn a_failed_rename_leaves_the_old_program_running() {
    let m = serve(vec![release("v0.2.0")]);
    let u = available(&m, "0.1.0");
    let d = Dir::new("rename-fails");
    let target = d.program();
    let tmp = temp_path(&target);
    // Take the temporary file away after the backup: the rename has nothing to move.
    let hook = |s: Step| {
        if s == Step::BackedUp {
            fs::remove_file(&tmp).unwrap();
        }
    };
    let e = install(&u, ASSET, &target, &hook).unwrap_err();
    assert!(e.to_string().contains("old program is untouched"), "{e}");
    assert_untouched(&target);
}

#[test]
fn a_damaged_target_is_restored_from_the_backup() {
    let m = serve(vec![release("v0.2.0")]);
    let u = available(&m, "0.1.0");
    let d = Dir::new("restore");
    let target = d.program();
    let tmp = temp_path(&target);
    // The rename fails and the target is not the old program any more.
    let hook = |s: Step| {
        if s == Step::BackedUp {
            fs::remove_file(&tmp).unwrap();
            fs::remove_file(&target).unwrap();
            fs::write(&target, b"garbage").unwrap();
        }
    };
    let e = install(&u, ASSET, &target, &hook).unwrap_err();
    assert!(e.to_string().contains("restored from the backup"), "{e}");
    assert_untouched(&target);
}

/// Child half of `killed_between_the_steps`: install, and die right after `$RV_UPDATE_DIE_AFTER`.
#[test]
fn child_install() {
    let (Ok(base), Ok(target), Ok(after)) = (
        std::env::var("RV_UPDATE_CHILD_API"),
        std::env::var("RV_UPDATE_CHILD_TARGET"),
        std::env::var("RV_UPDATE_DIE_AFTER"),
    ) else {
        return; // not the child: nothing to do
    };
    let cfg = UpdateConfig {
        api_base: Some(base),
        ..UpdateConfig::default()
    };
    let Check::Available(u) = check(&cfg, &v("0.1.0")).unwrap() else {
        std::process::exit(90)
    };
    let hook = |s: Step| {
        if format!("{s:?}") == after {
            std::process::abort();
        }
    };
    let _ = install(&u, ASSET, Path::new(&target), &hook);
    std::process::exit(91); // got to the end without dying
}

#[test]
fn killed_between_the_steps_the_old_program_still_runs() {
    use std::os::unix::process::ExitStatusExt;
    let m = serve(vec![release("v0.2.0")]);
    for after in ["Probed", "Downloaded", "Verified", "BackedUp"] {
        let d = Dir::new(&format!("kill-{after}"));
        let target = d.program();
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "child_install",
                "--exact",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("RV_UPDATE_CHILD_API", m.base())
            .env("RV_UPDATE_CHILD_TARGET", &target)
            .env("RV_UPDATE_DIE_AFTER", after)
            .status()
            .unwrap();
        assert_eq!(status.signal(), Some(6), "killed after {after}: {status:?}");
        // The old program is whole and runs. (A leftover temporary file is allowed: it is not
        // the program.)
        assert_eq!(fs::read(&target).unwrap(), OLD, "after {after}");
        assert_eq!(run(&target), "old", "after {after}");
        if after == "BackedUp" {
            assert_eq!(fs::read(backup_path(&target)).unwrap(), OLD);
        }
        // And the next attempt starts from there and succeeds.
        let u = available(&m, "0.1.0");
        install(&u, ASSET, &target, &nothing).unwrap();
        assert_eq!(run(&target), "new", "retry after {after}");
        assert!(!temp_path(&target).exists());
    }
}

#[test]
fn killed_right_after_the_swap_the_new_program_is_there() {
    use std::os::unix::process::ExitStatusExt;
    let m = serve(vec![release("v0.2.0")]);
    let d = Dir::new("kill-Renamed");
    let target = d.program();
    let status = Command::new(std::env::current_exe().unwrap())
        .args([
            "child_install",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("RV_UPDATE_CHILD_API", m.base())
        .env("RV_UPDATE_CHILD_TARGET", &target)
        .env("RV_UPDATE_DIE_AFTER", "Renamed")
        .status()
        .unwrap();
    assert_eq!(status.signal(), Some(6));
    assert_eq!(run(&target), "new");
    assert_eq!(fs::read(backup_path(&target)).unwrap(), OLD);
}
