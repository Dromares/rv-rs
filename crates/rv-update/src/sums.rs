//! `SHA256SUMS`, the format `sha256sum` writes: `<64 hex digits>  <name>` (a `*` before the name
//! marks binary mode and means the same).

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

use crate::error::{Error, Result};

pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

pub fn sha256_hex(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

/// The hash `text` names for `name`. `Ok(None)` when no line names it; an error when the text is
/// not a `SHA256SUMS` or names `name` twice with different hashes.
pub fn find(text: &str, name: &str) -> Result<Option<String>> {
    let mut seen: BTreeMap<&str, String> = BTreeMap::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let bad = |why: &str| Error::SumsBad(format!("line {}: {why}", i + 1));
        let (digest, rest) = line.split_once(' ').ok_or_else(|| bad("no file name"))?;
        if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(bad("the first field is not 64 hex digits"));
        }
        let file = rest
            .strip_prefix(' ')
            .or_else(|| rest.strip_prefix('*'))
            .ok_or_else(|| bad("the digest must be followed by two spaces or ` *`"))?;
        let file = file.strip_prefix("./").unwrap_or(file);
        let digest = digest.to_ascii_lowercase();
        if let Some(prev) = seen.insert(file, digest.clone()) {
            if prev != digest {
                return Err(bad("the same file has two different hashes"));
            }
        }
    }
    Ok(seen.remove(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    #[test]
    fn finds_by_exact_name() {
        let t = format!("{A}  rv-linux-x86_64\n{B} *rv-gtk-linux-x86_64\n\n# note\n");
        assert_eq!(find(&t, "rv-linux-x86_64").unwrap().as_deref(), Some(A));
        assert_eq!(find(&t, "rv-gtk-linux-x86_64").unwrap().as_deref(), Some(B));
        assert_eq!(find(&t, "rv").unwrap(), None);
        assert_eq!(find("", "rv").unwrap(), None);
    }

    #[test]
    fn upper_case_digests_and_crlf_are_read() {
        let t = format!("{}  rv\r\n", A.to_uppercase());
        assert_eq!(find(&t, "rv").unwrap().as_deref(), Some(A));
    }

    #[test]
    fn odd_lines_are_errors() {
        for bad in [
            "abc  rv\n".to_owned(),
            format!("{A} rv\n"),
            format!("{A}\n"),
            format!("{A}  rv\n{B}  rv\n"),
        ] {
            assert!(
                matches!(find(&bad, "rv"), Err(Error::SumsBad(_))),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn hashes() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
