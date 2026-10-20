//! The real HTTPS path of `rv update`, against a server whose certificate the client does not
//! trust (DAN-1861).
//!
//! Published 0.1.0 and 0.2.0 panicked here ("No CryptoProvider for Rustls") before any
//! handshake. Mock-only tests could never see it, because the mock is plain `http://`. The
//! contract: a request to an untrusted `https://` server is an `Err`, never a panic, and the
//! server did see a TLS handshake start.
//!
//! This file must not set `SSL_CERT_FILE`: the trusted case is `tls_trusted.rs`, a process of
//! its own.

mod common;

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use common::tls_server;
use rv_update::release::Asset;
use rv_update::{check, install, Error, Update, UpdateConfig, Version};

fn wait_failed(s: &tls_server::TlsServer) {
    let t = Instant::now();
    while s.failed.load(Ordering::SeqCst) == 0 && t.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn check_against_an_untrusted_https_server_is_an_error_not_a_panic() {
    let server = tls_server::start(|_, _, _, _| (404, Vec::new()));
    let cfg = UpdateConfig {
        api_base: Some(server.base()),
        ..UpdateConfig::default()
    };
    let current = Version::parse("0.1.99").unwrap();
    let err = check(&cfg, &current).expect_err("an untrusted certificate must not pass");
    assert!(matches!(err, Error::Network(_)), "{err:?}");
    // The handshake got as far as the certificate: the crypto provider is in place.
    wait_failed(&server);
    assert_eq!(server.handshakes.load(Ordering::SeqCst), 0);
    assert!(server.failed.load(Ordering::SeqCst) >= 1);
}

#[test]
fn install_against_an_untrusted_https_server_is_an_error_not_a_panic() {
    let server = tls_server::start(|_, _, _, _| (404, Vec::new()));
    let base = server.base();
    let update = Update {
        version: Version::parse("0.2.0").unwrap(),
        tag: "v0.2.0".into(),
        page: String::new(),
        assets: vec![
            Asset {
                name: "rv-linux-x86_64".into(),
                url: format!("{base}/download/v0.2.0/rv-linux-x86_64"),
                size: 0,
            },
            Asset {
                name: "SHA256SUMS".into(),
                url: format!("{base}/download/v0.2.0/SHA256SUMS"),
                size: 0,
            },
        ],
    };
    let dir = std::env::temp_dir().join(format!("rv-update-tls-untrusted-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let target = dir.join("rv");
    std::fs::write(&target, b"old").unwrap();
    let err = install(&update, "rv-linux-x86_64", &target, &|_| {})
        .expect_err("an untrusted certificate must not pass");
    assert!(matches!(err, Error::Network(_)), "{err:?}");
    assert_eq!(
        std::fs::read(&target).unwrap(),
        b"old",
        "nothing was installed"
    );
    wait_failed(&server);
    assert!(server.failed.load(Ordering::SeqCst) >= 1);
    let _ = std::fs::remove_dir_all(&dir);
}
