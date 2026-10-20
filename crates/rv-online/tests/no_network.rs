//! The no-network proof (DAN-1816 acceptance 2).
//!
//! Two parts. (1) Every test builds its client from the address of a mock on `127.0.0.1`
//! (`rv_online::mock::loopback_client`, whose transport refuses any other host, or an explicit
//! `http://127.0.0.1:<port>` endpoint); this file checks the source tree for the two real
//! hostnames and lists every hit with its reason. (2) No test file may reach for the default
//! endpoint.
//!
//! The hostnames are assembled from pieces below, so that this file is not a hit itself.

use std::fs;
use std::path::{Path, PathBuf};

const HOSTS: [&str; 2] = [concat!("romvault", ".com"), concat!("datvault", ".com")];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if name == "target" || name == ".git" {
            continue;
        }
        if p.is_dir() {
            files(&p, out);
        } else if matches!(
            p.extension().and_then(|x| x.to_str()),
            Some("rs" | "md" | "toml" | "yml" | "sh" | "conf")
        ) {
            out.push(p);
        }
    }
}

/// `(file relative to the repo, line number)` of every line that names a real host.
fn hits() -> Vec<(String, usize, String)> {
    let root = repo_root();
    let mut all = Vec::new();
    files(&root, &mut all);
    let mut out = Vec::new();
    for f in all {
        let Ok(text) = fs::read_to_string(&f) else {
            continue;
        };
        for (i, line) in text.lines().enumerate() {
            let l = line.to_ascii_lowercase();
            if HOSTS.iter().any(|h| l.contains(h)) {
                let rel = f
                    .canonicalize()
                    .unwrap()
                    .strip_prefix(root.canonicalize().unwrap())
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                out.push((rel, i + 1, line.trim().to_owned()));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn the_real_hostnames_appear_only_where_they_are_explained() {
    let found = hits();
    // Each allowed hit and why it is there. Anything else fails the test.
    let allowed: [(&str, &str); 1] = [(
        "crates/rv-online/src/endpoint.rs",
        "DEFAULT_ENDPOINT, the https address in the exe; used only when no endpoint is configured, which no test does",
    )];
    for (file, line, text) in &found {
        assert!(
            allowed.iter().any(|(f, _)| f == file),
            "unexplained real hostname at {file}:{line}: {text}"
        );
    }
    assert_eq!(found.len(), allowed.len(), "{found:#?}");
    for (file, line, text) in &found {
        eprintln!("hit {file}:{line}: {text}");
    }
}

#[test]
fn no_test_file_reaches_for_the_default_endpoint() {
    let root = repo_root();
    let mut all = Vec::new();
    files(&root.join("crates"), &mut all);
    for f in all {
        let rel = f
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        // Integration tests only; unit tests sit in `src/` and are covered by the hostname scan
        // (they compare strings, they do not connect).
        if !rel.contains("/tests/") || rel.ends_with("no_network.rs") {
            continue;
        }
        let text = fs::read_to_string(&f).unwrap();
        for needle in ["default_https", "DEFAULT_ENDPOINT", "Endpoint::default"] {
            assert!(!text.contains(needle), "{rel} uses {needle}");
        }
    }
}

#[test]
fn the_default_configuration_is_https_and_nothing_here_connects_with_it() {
    let c = rv_online::config::OnlineConfig::default();
    let ep = c.endpoint().unwrap();
    assert!(!ep.is_plain_http());
    assert_eq!(ep.url(), rv_online::DEFAULT_ENDPOINT);
    // The loopback guard of the tests refuses it (nothing is sent).
    let client = rv_online::Client::with_transport(
        ep,
        Box::new(rv_online::mock::LoopbackOnly(
            rv_online::transport::UreqTransport,
        )),
    );
    let e = client.get_update_link().unwrap_err();
    assert!(e.to_string().contains("test guard"), "{e}");
}
