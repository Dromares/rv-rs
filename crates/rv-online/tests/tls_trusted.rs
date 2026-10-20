//! The SOAP client, DatVault and MIA over a real TLS handshake with a test CA the client is
//! told to trust (DAN-1861). The server is the emulated service of `rv_online::mock`, behind
//! rustls instead of plain TCP.
//!
//! `SSL_CERT_FILE` replaces the system roots for the platform verifier, so this file is a
//! process of its own and has one test: the variable is set before any request, from the main
//! test thread, and nothing else here reads the environment.

mod common;

use std::sync::atomic::Ordering;

use common::tls_server;
use rv_online::datvault::Vault;
use rv_online::mia::testing::blob;
use rv_online::mia::{self, MiaCacheStatus, MiaRecord};
use rv_online::mock::{self, MockRequest};
use rv_online::salt::Salt;
use rv_online::{Client, Endpoint, Group};

const KEY: &str = "tls-test-key";

#[test]
fn the_online_clients_over_a_trusted_https_server() {
    // Nothing else in this process reads the environment while it is set.
    std::env::set_var("SSL_CERT_FILE", tls_server::ca_pem());

    let salt = Salt::from_bytes(b"test salt");
    let record = MiaRecord {
        size: Some(101),
        crc: Some([1, 0, 0, 1]),
        ..MiaRecord::default()
    };
    let st = mock::state(KEY);
    {
        let mut s = st.lock().unwrap();
        s.groups = vec![Group {
            basepath: "a".into(),
            display_name: "A".into(),
            group: "g".into(),
            url: "u".into(),
        }];
        s.mia_version = Some(700);
        s.mia_blob = Some(blob(700, &[record], KEY, &salt));
    }
    let server = tls_server::start(move |_, path, head, body| {
        let action = head
            .lines()
            .find_map(|l| {
                let (k, v) = l.split_once(':')?;
                k.eq_ignore_ascii_case("soapaction")
                    .then(|| v.trim().trim_matches('"').to_owned())
            })
            .unwrap_or_default();
        let reply = mock::answer(
            &st,
            &MockRequest {
                path: path.to_owned(),
                soap_action: action,
                body: String::from_utf8_lossy(body).into_owned(),
            },
        );
        (reply.status, reply.body)
    });
    let client =
        Client::new(Endpoint::new(&format!("{}/RVService.svc", server.base()), false).unwrap());

    // The SOAP client.
    assert_eq!(client.check_dv_user(KEY).unwrap().to_lowercase(), "ok");
    // DatVault.
    let vault = Vault::new(&client, KEY).unwrap();
    assert!(vault.check_key().unwrap());
    assert_eq!(vault.groups().unwrap().len(), 1);
    // MIA: the version check and the list, through the cache.
    let dir = std::env::temp_dir().join(format!("rv-online-tls-trusted-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let r = mia::fetch(&client, KEY, &salt, &dir.join("MIA.Cache"));
    assert_eq!(r.status, MiaCacheStatus::NewFetched);
    assert_eq!(r.list.unwrap().records().len(), 1);

    assert!(server.handshakes.load(Ordering::SeqCst) >= 5);
    assert_eq!(server.failed.load(Ordering::SeqCst), 0);
    let _ = std::fs::remove_dir_all(&dir);
}
