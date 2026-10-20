//! Where the service is. The default is the `https://` address the 3.8.0 exe carries
//! (`RVServiceClient::GetEndpointAddress`, `online-protocol.md` section 1.3). The exe itself
//! always talks plain `http://`; this client does that only when `allow_plain_http` is on,
//! because the DatVault key travels as an argument of every call.

use crate::error::{Error, Result};

/// The address in the exe's `EndpointConfiguration`.
pub const DEFAULT_ENDPOINT: &str = "https://services.romvault.com/RVService.svc";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    url: String,
    plain: bool,
}

impl Endpoint {
    /// The default `https://` endpoint.
    pub fn default_https() -> Endpoint {
        Endpoint {
            url: DEFAULT_ENDPOINT.to_owned(),
            plain: false,
        }
    }

    /// `url` must be `https://`, or `http://` when `allow_plain_http`. No other scheme, and no
    /// user name or password in the address.
    pub fn new(url: &str, allow_plain_http: bool) -> Result<Endpoint> {
        let url = url.trim();
        if url.chars().any(|c| c.is_control() || c.is_whitespace()) {
            return Err(Error::BadUrl(
                "it has white space or control characters".into(),
            ));
        }
        let lower = url.to_ascii_lowercase();
        let (plain, rest) = if lower.starts_with("https://") {
            (false, &url["https://".len()..])
        } else if lower.starts_with("http://") {
            (true, &url["http://".len()..])
        } else {
            return Err(Error::BadUrl(
                "only http:// and https:// are supported".into(),
            ));
        };
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        if authority.is_empty() {
            return Err(Error::BadUrl("there is no host".into()));
        }
        if authority.contains('@') {
            return Err(Error::BadUrl(
                "a user name or password in the address".into(),
            ));
        }
        if plain && !allow_plain_http {
            return Err(Error::PlainHttpRefused(url.to_owned()));
        }
        Ok(Endpoint {
            url: url.to_owned(),
            plain,
        })
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn is_plain_http(&self) -> bool {
        self.plain
    }

    /// The host without a port, or the address inside `[...]` for IPv6.
    pub fn host(&self) -> &str {
        host_of(&self.url)
    }
}

/// The host part of an `http(s)://` URL.
pub(crate) fn host_of(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if let Some(v6) = authority.strip_prefix('[') {
        return v6.split(']').next().unwrap_or("");
    }
    authority.split(':').next().unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_is_always_accepted() {
        let e = Endpoint::new("https://example.invalid/RVService.svc", false).unwrap();
        assert!(!e.is_plain_http());
        assert_eq!(e.host(), "example.invalid");
        assert_eq!(Endpoint::default_https().url(), DEFAULT_ENDPOINT);
        assert!(!Endpoint::default_https().is_plain_http());
    }

    #[test]
    fn plain_http_is_refused_unless_allowed() {
        let e = Endpoint::new("http://127.0.0.1:8080/x", false).unwrap_err();
        assert!(matches!(e, Error::PlainHttpRefused(_)));
        let e = Endpoint::new("HTTP://127.0.0.1:8080/x", false).unwrap_err();
        assert!(matches!(e, Error::PlainHttpRefused(_)));
        let ok = Endpoint::new("http://127.0.0.1:8080/x", true).unwrap();
        assert!(ok.is_plain_http());
        assert_eq!(ok.host(), "127.0.0.1");
    }

    #[test]
    fn other_schemes_and_odd_addresses_are_refused() {
        for bad in [
            "ftp://h/x",
            "file:///etc/passwd",
            "h/x",
            "",
            "https://",
            "https:///x",
            "https://user:pw@h/x",
            "https://h/ x",
            "https://h/a\nb",
        ] {
            assert!(
                matches!(Endpoint::new(bad, true), Err(Error::BadUrl(_))),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn host_of_handles_ports_and_ipv6() {
        assert_eq!(host_of("http://[::1]:80/x"), "::1");
        assert_eq!(host_of("https://h.example:443/x?y=z"), "h.example");
    }
}
