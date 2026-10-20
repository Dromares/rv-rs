//! A stand-in for the Releases API on 127.0.0.1 that serves the files of a directory as one
//! release, for trying `rv update` by hand (DAN-1823 does it with the assets of a draft).
//!
//!     cargo run -p rv-update --features mock --example mock_releases -- \
//!         --dir <dir with the assets> --tag v0.1.0 [--flip <asset>]
//!
//! It prints `base=http://127.0.0.1:<port>` (put that in `api_base` of `update.conf`) and serves
//! until it is killed. A release tagged `-something` is a pre-release, so it is only offered
//! when `include_prereleases=1`, as on GitHub. `--flip <asset>` serves that file with one byte
//! changed, to see the install refuse it. The directory should hold `SHA256SUMS` as the release
//! has it; without one, none is served (and the install refuses).

use std::io::Write;

use rv_update::mock::{Mock, MockRelease, MockState};

fn main() {
    let mut dir = None;
    let mut tag = None;
    let mut flip = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut value = || {
            args.next()
                .unwrap_or_else(|| usage(&format!("{a} needs a value")))
        };
        match a.as_str() {
            "--dir" => dir = Some(value()),
            "--tag" => tag = Some(value()),
            "--flip" => flip = Some(value()),
            other => usage(&format!("unknown argument {other}")),
        }
    }
    let (Some(dir), Some(tag)) = (dir, tag) else {
        usage("--dir and --tag are required")
    };
    let mut files = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap_or_else(|e| usage(&format!("{dir}: {e}"))) {
        let entry = entry.expect("directory entry");
        if !entry.path().is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let mut bytes = std::fs::read(entry.path()).expect("read an asset");
        if flip.as_deref() == Some(name.as_str()) {
            let mid = bytes.len() / 2;
            bytes[mid] ^= 1;
        }
        files.push((name, bytes));
    }
    let release = MockRelease {
        prerelease: tag.contains('-'),
        tag,
        files,
    };
    let mock = Mock::start(MockState {
        releases: vec![release],
        ..MockState::default()
    });
    println!("base={}", mock.base());
    let _ = std::io::stdout().flush();
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}

fn usage(why: &str) -> ! {
    eprintln!(
        "mock_releases: {why}\nusage: mock_releases --dir <dir> --tag <tag> [--flip <asset>]"
    );
    std::process::exit(2)
}
