//! Semantic versions (semver.org 2.0): `MAJOR.MINOR.PATCH[-pre.release]`, with the build
//! metadata (`+...`) read and ignored, as the spec says it does not take part in precedence.
//! A leading `v` (the tag form) is accepted.

use std::cmp::Ordering;
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Ident {
    Num(u64),
    Text(String),
}

impl Ord for Ident {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Ident::Num(a), Ident::Num(b)) => a.cmp(b),
            // Numeric identifiers always have lower precedence than alphanumeric ones.
            (Ident::Num(_), Ident::Text(_)) => Ordering::Less,
            (Ident::Text(_), Ident::Num(_)) => Ordering::Greater,
            (Ident::Text(a), Ident::Text(b)) => a.cmp(b),
        }
    }
}

impl PartialOrd for Ident {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pre: Vec<Ident>,
}

impl Version {
    /// `1.2.3`, `v1.2.3`, `1.2.3-rc.1`, `1.2.3+build`. Anything else is an error.
    pub fn parse(text: &str) -> Result<Version, String> {
        let bad = |why: &str| format!("not a version ({why}): {text:?}");
        let t = text.trim();
        let t = t.strip_prefix('v').unwrap_or(t);
        let t = t.split_once('+').map_or(t, |(core, _build)| core);
        let (core, pre) = match t.split_once('-') {
            Some((c, p)) => (c, Some(p)),
            None => (t, None),
        };
        let nums: Vec<&str> = core.split('.').collect();
        if nums.len() != 3 {
            return Err(bad("it needs MAJOR.MINOR.PATCH"));
        }
        let num = |s: &str| -> Result<u64, String> {
            if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
                return Err(bad("a part is not a number"));
            }
            if s.len() > 1 && s.starts_with('0') {
                return Err(bad("a number has a leading zero"));
            }
            s.parse().map_err(|_| bad("a number is too big"))
        };
        let mut idents = Vec::new();
        if let Some(p) = pre {
            for part in p.split('.') {
                if part.is_empty() || !part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                {
                    return Err(bad("a pre-release part is empty or has odd characters"));
                }
                if part.bytes().all(|b| b.is_ascii_digit()) {
                    if part.len() > 1 && part.starts_with('0') {
                        return Err(bad("a pre-release number has a leading zero"));
                    }
                    idents.push(Ident::Num(
                        part.parse().map_err(|_| bad("a number is too big"))?,
                    ));
                } else {
                    idents.push(Ident::Text(part.to_owned()));
                }
            }
        }
        Ok(Version {
            major: num(nums[0])?,
            minor: num(nums[1])?,
            patch: num(nums[2])?,
            pre: idents,
        })
    }

    pub fn is_prerelease(&self) -> bool {
        !self.pre.is_empty()
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                // A pre-release is lower than the release it leads up to.
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                // A longer list wins when all the shared parts are equal (Vec's order).
                (false, false) => self.pre.cmp(&other.pre),
            })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        for (i, p) in self.pre.iter().enumerate() {
            f.write_str(if i == 0 { "-" } else { "." })?;
            match p {
                Ident::Num(n) => write!(f, "{n}")?,
                Ident::Text(t) => f.write_str(t)?,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn parses_the_tag_and_plain_forms() {
        assert_eq!(v("v1.2.3"), v("1.2.3"));
        assert_eq!(v("1.2.3+abc"), v("1.2.3"));
        assert_eq!(v("1.2.3-rc.1").to_string(), "1.2.3-rc.1");
        assert!(v("1.2.3-rc.1").is_prerelease());
        assert!(!v("1.2.3").is_prerelease());
    }

    #[test]
    fn refuses_what_is_not_a_version() {
        for bad in [
            "",
            "1",
            "1.2",
            "1.2.3.4",
            "a.b.c",
            "01.2.3",
            "1.2.-3",
            "1.2.3-",
            "1.2.3-a..b",
            "1.2.3-01",
            "1.2.3-a_b",
            "99999999999999999999.0.0",
            "latest",
        ] {
            assert!(Version::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn orders_like_semver_dot_org() {
        // The chain from the spec, section 11.
        let chain = [
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0-alpha.beta",
            "1.0.0-beta",
            "1.0.0-beta.2",
            "1.0.0-beta.11",
            "1.0.0-rc.1",
            "1.0.0",
            "1.0.1",
            "1.1.0",
            "2.0.0",
        ];
        for w in chain.windows(2) {
            assert!(v(w[0]) < v(w[1]), "{} < {}", w[0], w[1]);
        }
        assert_eq!(v("1.0.0+a").cmp(&v("1.0.0+b")), Ordering::Equal);
        assert!(v("0.0.0") < v("0.0.1-test.1"));
        assert!(v("0.10.0") > v("0.9.0"));
    }
}
