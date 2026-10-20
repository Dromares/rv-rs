//! The GETs of this crate, on the HTTP and TLS stack `rv-online` uses (`ureq` 3 with rustls and
//! ring, the platform's certificate store): no second client.
//!
//! Every request is anonymous: no token or cookie is ever sent, so a redirect to another host
//! has nothing to leak. Redirects are followed by hand (GitHub sends release files to a CDN),
//! to `https://` only, or to the loopback while the request itself is on the loopback.

use std::io::Read;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use crate::error::{Error, Result};

const MAX_REDIRECTS: usize = 5;

/// The host of an `http(s)://` URL, without port, user or brackets.
fn host_of(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if let Some(v6) = authority.strip_prefix('[') {
        return v6.split(']').next().unwrap_or("");
    }
    authority.split(':').next().unwrap_or("")
}

fn is_loopback(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// `https://`, or `http://` when the host is the loopback. No user name in the address, no
/// white space or control characters.
pub(crate) fn check_url(url: &str) -> Result<()> {
    let bad = |why: &str| Err(Error::BadUrl(format!("{why}: {url}")));
    if url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return bad("white space or control characters");
    }
    let lower = url.to_ascii_lowercase();
    let rest = if let Some(r) = lower.strip_prefix("https://") {
        r
    } else if let Some(r) = lower.strip_prefix("http://") {
        if !is_loopback(host_of(url)) {
            return bad("plain http is for the loopback only");
        }
        r
    } else {
        return bad("only https:// is supported");
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() {
        return bad("there is no host");
    }
    if authority.contains('@') {
        return bad("a user name or password in the address");
    }
    Ok(())
}

/// The crypto provider of every agent in this crate. `ureq` is built without a default
/// provider (`rustls-no-provider`), and without one the first `https://` request panics
/// ("No CryptoProvider for Rustls"). It is set on each agent, not as the process default, so it
/// holds for any caller of this crate and does not touch the rest of the process. One shared
/// `Arc`: `ureq` keys its TLS config on the provider's identity.
fn crypto_provider() -> Arc<rustls::crypto::CryptoProvider> {
    static PROVIDER: OnceLock<Arc<rustls::crypto::CryptoProvider>> = OnceLock::new();
    PROVIDER
        .get_or_init(|| Arc::new(rustls::crypto::ring::default_provider()))
        .clone()
}

fn agent(loopback: bool) -> ureq::Agent {
    let mut cfg = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .max_redirects(0)
        .max_redirects_will_error(false)
        .timeout_connect(Some(Duration::from_secs(30)))
        .timeout_recv_response(Some(Duration::from_secs(60)))
        .timeout_recv_body(Some(Duration::from_secs(300)))
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .provider(ureq::tls::TlsProvider::Rustls)
                .unversioned_rustls_crypto_provider(crypto_provider())
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .build(),
        );
    if loopback {
        cfg = cfg.proxy(None);
    }
    ureq::Agent::new_with_config(cfg.build())
}

/// The URL a `Location` header points at (absolute, or a path on the same host).
fn resolve(current: &str, location: &str) -> String {
    if location.contains("://") {
        return location.to_owned();
    }
    let (scheme, rest) = current.split_once("://").unwrap_or(("https", current));
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if location.starts_with('/') {
        format!("{scheme}://{authority}{location}")
    } else {
        let dir = rest.rsplit_once('/').map_or("", |(d, _)| d);
        format!("{scheme}://{dir}/{location}")
    }
}

/// GET `url` and hand the status and a reader over the body to `f`. The body is limited to
/// `limit` bytes: reading past it is an error.
pub(crate) fn get<T>(
    url: &str,
    accept: &str,
    limit: u64,
    f: impl FnOnce(u16, &mut dyn Read) -> Result<T>,
) -> Result<T> {
    let mut url = url.to_owned();
    for _ in 0..=MAX_REDIRECTS {
        check_url(&url)?;
        let loopback = is_loopback(host_of(&url));
        let mut resp = agent(loopback)
            .get(&url)
            .header("Accept", accept)
            .header("User-Agent", concat!("rv-rs/", env!("CARGO_PKG_VERSION")))
            .header("X-GitHub-Api-Version", "2022-11-28")
            .call()
            .map_err(|e| Error::Network(e.to_string()))?;
        let status = resp.status().as_u16();
        if matches!(status, 301 | 302 | 303 | 307 | 308) {
            let Some(loc) = resp
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
            else {
                return Err(Error::BadReply(format!("{url} redirects to nowhere")));
            };
            let next = resolve(&url, &loc);
            // Plain http is only allowed to stay on the loopback; check_url above sees to the rest.
            if next.to_ascii_lowercase().starts_with("http://") && !loopback {
                return Err(Error::BadUrl(format!(
                    "{url} redirects from https to plain http: {next}"
                )));
            }
            url = next;
            continue;
        }
        let mut reader = resp.body_mut().with_config().limit(limit).reader();
        return f(status, &mut reader).map_err(|e| match e {
            Error::Network(m) if m.contains("body") && m.contains("limit") => Error::TooBig {
                url: url.clone(),
                limit,
            },
            other => other,
        });
    }
    Err(Error::BadReply(format!(
        "more than {MAX_REDIRECTS} redirects from {url}"
    )))
}

/// GET a small body (JSON, `SHA256SUMS`) into memory. `None` for a 404.
pub(crate) fn get_small(url: &str, accept: &str, limit: u64) -> Result<Option<Vec<u8>>> {
    let shown = url.to_owned();
    get(url, accept, limit, |status, body| match status {
        200 => {
            let mut buf = Vec::new();
            body.take(limit + 1)
                .read_to_end(&mut buf)
                .map_err(|e| Error::Network(e.to_string()))?;
            if buf.len() as u64 > limit {
                return Err(Error::TooBig { url: shown, limit });
            }
            Ok(Some(buf))
        }
        404 => Ok(None),
        code => Err(Error::Status { url: shown, code }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        for ok in [
            "https://api.github.com/repos/x/y",
            "http://127.0.0.1:8080/a",
            "http://[::1]:80/a",
            "http://localhost/a",
        ] {
            assert!(check_url(ok).is_ok(), "{ok}");
        }
        for bad in [
            "http://example.invalid/a",
            "http://127.0.0.1.example.invalid/a",
            "ftp://h/a",
            "https://u:p@h/a",
            "https:///a",
            "https://h/a b",
            "",
        ] {
            assert!(check_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn redirect_targets() {
        assert_eq!(resolve("http://h:1/a/b", "/c"), "http://h:1/c");
        assert_eq!(resolve("http://h:1/a/b", "c"), "http://h:1/a/c");
        assert_eq!(resolve("http://h:1/a", "https://x/y"), "https://x/y");
    }
}
