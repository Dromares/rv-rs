//! A tiny HTTPS server on `127.0.0.1` for the TLS tests: a real rustls handshake with a test
//! certificate, so the client under test goes through its whole TLS path.
//!
//! The certificate and key in `tests/data/tls/` exist for these tests only: a throw-away CA
//! (`ca.pem`) and a leaf for `127.0.0.1`/`localhost` signed by it. They protect nothing.

#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

pub struct TlsServer {
    pub port: u16,
    /// TLS handshakes the server completed (the client trusted the certificate).
    pub handshakes: Arc<AtomicUsize>,
    /// Connections the server saw end in a failed handshake (the client refused the
    /// certificate, say).
    pub failed: Arc<AtomicUsize>,
}

impl TlsServer {
    pub fn base(&self) -> String {
        format!("https://127.0.0.1:{}", self.port)
    }
}

/// The CA certificate, for `SSL_CERT_FILE`.
pub fn ca_pem() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/tls/ca.pem")
}

fn data(name: &str) -> Vec<u8> {
    std::fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data/tls")
            .join(name),
    )
    .unwrap()
}

fn config() -> Arc<rustls::ServerConfig> {
    // The server builds its own provider: it must not depend on what the code under test sets.
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let cfg = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(data("leaf.der"))],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(data("leaf.key.der"))),
        )
        .unwrap();
    Arc::new(cfg)
}

/// Serve `answer(method, path, head, body)` -> `(status, body)` to every request, one request per
/// connection. The threads end with the test process.
pub fn start(
    answer: impl Fn(&str, &str, &str, &[u8]) -> (u16, Vec<u8>) + Send + Sync + 'static,
) -> TlsServer {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let cfg = config();
    let answer = Arc::new(answer);
    let handshakes = Arc::new(AtomicUsize::new(0));
    let failed = Arc::new(AtomicUsize::new(0));
    let (h, f) = (handshakes.clone(), failed.clone());
    std::thread::spawn(move || {
        for tcp in listener.incoming() {
            let Ok(mut tcp) = tcp else { continue };
            let (cfg, answer, h, f) = (cfg.clone(), answer.clone(), h.clone(), f.clone());
            std::thread::spawn(move || {
                let mut conn = rustls::ServerConnection::new(cfg).unwrap();
                while conn.is_handshaking() {
                    if conn.complete_io(&mut tcp).is_err() {
                        f.fetch_add(1, Ordering::SeqCst);
                        return;
                    }
                }
                h.fetch_add(1, Ordering::SeqCst);
                let mut tls = rustls::Stream::new(&mut conn, &mut tcp);
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                let head_end = loop {
                    if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break i + 4;
                    }
                    match tls.read(&mut chunk) {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                };
                let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
                let mut first = head.lines().next().unwrap_or("").split(' ');
                let (method, path) = (first.next().unwrap_or(""), first.next().unwrap_or(""));
                let len: usize = head
                    .lines()
                    .find_map(|l| {
                        let (k, v) = l.split_once(':')?;
                        k.eq_ignore_ascii_case("content-length")
                            .then(|| v.trim().parse().ok())?
                    })
                    .unwrap_or(0);
                while buf.len() < head_end + len {
                    match tls.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                }
                let (status, body) = answer(method, path, &head, &buf[head_end..]);
                let reply = format!(
                    "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = tls.write_all(reply.as_bytes());
                let _ = tls.write_all(&body);
                let _ = tls.flush();
                tls.conn.send_close_notify();
                let _ = tls.flush();
            });
        }
    });
    TlsServer {
        port,
        handshakes,
        failed,
    }
}
