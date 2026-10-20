//! One HTTP POST. The client is written against [`Transport`] so the tests can put a guard in
//! front of the real one; [`UreqTransport`] is the real one.

use std::time::Duration;

use crate::endpoint::host_of;
use crate::error::{Error, Result};

/// The binding timeouts of the exe, which differ per call site (`online-protocol.md` 1.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timeouts {
    pub connect: Duration,
    pub send: Duration,
    pub receive: Duration,
}

impl Timeouts {
    /// WCF defaults: one minute each (startup, update check).
    pub const DEFAULT: Timeouts = Timeouts {
        connect: Duration::from_secs(60),
        send: Duration::from_secs(60),
        receive: Duration::from_secs(60),
    };
    /// `Settings.RVOpenTimeout/RVSendTimeout/RVReceiveTimeout`: 120, 120, 180 seconds.
    pub const DATVAULT: Timeouts = Timeouts {
        connect: Duration::from_secs(120),
        send: Duration::from_secs(120),
        receive: Duration::from_secs(180),
    };
    /// The MIA fetch and the found upload: 20 minutes.
    pub const MIA: Timeouts = Timeouts {
        connect: Duration::from_secs(60),
        send: Duration::from_secs(20 * 60),
        receive: Duration::from_secs(20 * 60),
    };
}

/// The biggest reply read (the exe allows 2 GiB; the largest real one is a MIA list of some MB).
pub const MAX_REPLY: u64 = 512 * 1024 * 1024;

pub struct HttpRequest<'a> {
    pub url: &'a str,
    pub soap_action: &'a str,
    pub body: &'a [u8],
    pub timeouts: Timeouts,
}

pub struct HttpReply {
    pub status: u16,
    pub body: Vec<u8>,
}

pub trait Transport: Send + Sync {
    fn post(&self, req: &HttpRequest<'_>) -> Result<HttpReply>;
}

/// The real transport: `ureq` with rustls (the platform's certificate store) for `https://`.
/// It never follows a redirect (a redirect from https to http would carry the key to clear
/// text) and ignores the proxy environment for loopback addresses.
#[derive(Debug, Default)]
pub struct UreqTransport;

fn is_loopback(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

impl Transport for UreqTransport {
    fn post(&self, req: &HttpRequest<'_>) -> Result<HttpReply> {
        let t = req.timeouts;
        let mut cfg = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .max_redirects_will_error(false)
            .timeout_connect(Some(t.connect))
            .timeout_send_request(Some(t.send))
            .timeout_send_body(Some(t.send))
            .timeout_recv_response(Some(t.receive))
            .timeout_recv_body(Some(t.receive))
            .tls_config(
                ureq::tls::TlsConfig::builder()
                    .provider(ureq::tls::TlsProvider::Rustls)
                    .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                    .build(),
            );
        if is_loopback(host_of(req.url)) {
            cfg = cfg.proxy(None);
        }
        let agent = ureq::Agent::new_with_config(cfg.build());
        let mut reply = agent
            .post(req.url)
            .header("Content-Type", "text/xml; charset=utf-8")
            .header("SOAPAction", format!("\"{}\"", req.soap_action))
            .send(req.body)
            .map_err(|e| Error::Network(e.to_string()))?;
        let status = reply.status().as_u16();
        let body = reply
            .body_mut()
            .with_config()
            .limit(MAX_REPLY)
            .read_to_vec()
            .map_err(|e| Error::Network(e.to_string()))?;
        Ok(HttpReply { status, body })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_hosts() {
        for h in ["127.0.0.1", "127.9.9.9", "::1", "localhost", "LOCALHOST"] {
            assert!(is_loopback(h), "{h}");
        }
        for h in ["10.0.0.1", "example.invalid", "", "0.0.0.0"] {
            assert!(!is_loopback(h), "{h}");
        }
    }

    #[test]
    fn the_timeouts_are_the_exes() {
        assert_eq!(Timeouts::DATVAULT.receive, Duration::from_secs(180));
        assert_eq!(Timeouts::MIA.send, Duration::from_secs(1200));
    }
}
