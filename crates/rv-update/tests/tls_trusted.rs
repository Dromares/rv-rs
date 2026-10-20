//! The whole update over real TLS, against a test CA the client is told to trust (DAN-1861).
//!
//! `SSL_CERT_FILE` replaces the system roots for the platform verifier, so this file is a
//! process of its own and has one test: the variable is set before any request, from the main
//! test thread, and nothing else here reads the environment.

mod common;

use std::sync::atomic::Ordering;

use common::tls_server;
use rv_update::install::install;
use rv_update::{check, Check, UpdateConfig, Version};

const ASSET: &str = "rv-linux-x86_64";
const NEW: &[u8] = b"#!/bin/sh\necho new\n";

#[test]
fn check_and_install_over_a_trusted_https_server() {
    // Nothing else in this process reads the environment while it is set.
    std::env::set_var("SSL_CERT_FILE", tls_server::ca_pem());

    let sums = format!("{}  {ASSET}\n", rv_update::sums::sha256_hex(NEW)).into_bytes();
    // The port is only known once the server runs, and the release JSON names it.
    let port = std::sync::Arc::new(std::sync::atomic::AtomicU16::new(0));
    let p = port.clone();
    let server = tls_server::start(move |method, path, _| {
        assert_eq!(method, "GET");
        let base = format!("https://127.0.0.1:{}", p.load(Ordering::SeqCst));
        match path {
            "/repos/Dromares/rv-rs/releases/latest" => (
                200,
                format!(
                    r#"{{"tag_name":"v0.2.0","html_url":"{base}/page","draft":false,"prerelease":false,"assets":[
                        {{"name":"{ASSET}","size":{},"browser_download_url":"{base}/download/v0.2.0/{ASSET}"}},
                        {{"name":"SHA256SUMS","size":{},"browser_download_url":"{base}/download/v0.2.0/SHA256SUMS"}}]}}"#,
                    NEW.len(),
                    sums.len()
                )
                .into_bytes(),
            ),
            "/download/v0.2.0/SHA256SUMS" => (200, sums.clone()),
            p if p == format!("/download/v0.2.0/{ASSET}") => (200, NEW.to_vec()),
            _ => (404, Vec::new()),
        }
    });
    port.store(server.port, Ordering::SeqCst);

    let cfg = UpdateConfig {
        api_base: Some(server.base()),
        ..UpdateConfig::default()
    };
    let Check::Available(update) = check(&cfg, &Version::parse("0.1.99").unwrap()).unwrap() else {
        panic!("0.2.0 should be available over 0.1.99");
    };
    assert_eq!(update.version, Version::parse("0.2.0").unwrap());

    let dir = std::env::temp_dir().join(format!("rv-update-tls-trusted-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let target = dir.join("rv");
    std::fs::write(&target, b"old").unwrap();
    let done = install(&update, ASSET, &target, &|_| {}).unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), NEW);
    assert_eq!(std::fs::read(done.backup.unwrap()).unwrap(), b"old");
    assert!(
        server.handshakes.load(Ordering::SeqCst) >= 3,
        "check, sums, binary"
    );
    assert_eq!(server.failed.load(Ordering::SeqCst), 0);
    let _ = std::fs::remove_dir_all(&dir);
}
