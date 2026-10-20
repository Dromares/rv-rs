//! Every online client over real HTTPS, against a server whose certificate the client does not
//! trust (DAN-1861).
//!
//! Published 0.1.0 and 0.2.0 panicked on the first `https://` request ("No CryptoProvider for
//! Rustls"); the tests only ever used a plain-HTTP mock. The contract: an untrusted server is an
//! `Err` (or the documented failure status), never a panic, and the server did see a TLS
//! handshake start. The SOAP client, DatVault and MIA all share one transport, and each is
//! driven here by its own entry point.
//!
//! This file must not set `SSL_CERT_FILE`: the trusted case is `tls_trusted.rs`.

mod common;

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use common::tls_server::{self, TlsServer};
use rv_online::datvault::Vault;
use rv_online::mia::{self, MiaCacheStatus};
use rv_online::salt::Salt;
use rv_online::{Client, Endpoint, Error};

fn server() -> TlsServer {
    tls_server::start(|_, _, _, _| (404, Vec::new()))
}

fn client(s: &TlsServer) -> Client {
    Client::new(Endpoint::new(&format!("{}/RVService.svc", s.base()), false).unwrap())
}

/// The server counted a failed handshake: the client got as far as the certificate.
fn saw_refused_handshake(s: &TlsServer) {
    let t = Instant::now();
    while s.failed.load(Ordering::SeqCst) == 0 && t.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(s.handshakes.load(Ordering::SeqCst), 0);
    assert!(s.failed.load(Ordering::SeqCst) >= 1);
}

#[test]
fn soap_client_against_an_untrusted_https_server_is_an_error_not_a_panic() {
    let s = server();
    let err = client(&s).check_dv_user("key").expect_err("must not pass");
    assert!(matches!(err, Error::Network(_)), "{err:?}");
    saw_refused_handshake(&s);
}

#[test]
fn datvault_against_an_untrusted_https_server_is_an_error_not_a_panic() {
    let s = server();
    let c = client(&s);
    let v = Vault::new(&c, "key").unwrap();
    assert!(matches!(v.check_key(), Err(Error::Network(_))));
    assert!(matches!(v.groups(), Err(Error::Network(_))));
    saw_refused_handshake(&s);
}

#[test]
fn mia_against_an_untrusted_https_server_fails_without_a_panic() {
    let s = server();
    let c = client(&s);
    let dir = std::env::temp_dir().join(format!("rv-online-tls-untrusted-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // No cache and no way to reach the server: "expired", as for any network error.
    let r = mia::fetch(
        &c,
        "key",
        &Salt::from_bytes(b"salt"),
        &dir.join("MIA.Cache"),
    );
    assert_eq!(r.status, MiaCacheStatus::Expired);
    assert!(r.list.is_none());
    saw_refused_handshake(&s);
    let _ = std::fs::remove_dir_all(&dir);
}
