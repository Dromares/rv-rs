//! Base64 (RFC 4648, standard alphabet, padded) for the `byte[]` values of the SOAP messages.

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Decode; white space is ignored (XML text may be wrapped). `None` for anything else that is
/// not base64.
pub fn decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let mut acc = 0u32;
    let mut bits = 0u32;
    let mut pad = 0usize;
    for c in text.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => {
                pad += 1;
                continue;
            }
            b' ' | b'\t' | b'\r' | b'\n' => continue,
            _ => return None,
        };
        if pad > 0 {
            return None;
        }
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    // What is left must be zero bits, and the padding must account for them.
    if acc != 0 || pad > 2 || (bits != 0 && pad != (bits as usize) / 2) {
        return None;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    // RFC 4648 section 10.
    const VECTORS: &[(&str, &str)] = &[
        ("", ""),
        ("f", "Zg=="),
        ("fo", "Zm8="),
        ("foo", "Zm9v"),
        ("foob", "Zm9vYg=="),
        ("fooba", "Zm9vYmE="),
        ("foobar", "Zm9vYmFy"),
    ];

    #[test]
    fn rfc_4648_vectors() {
        for (plain, enc) in VECTORS {
            assert_eq!(encode(plain.as_bytes()), *enc);
            assert_eq!(decode(enc).unwrap(), plain.as_bytes());
        }
    }

    #[test]
    fn white_space_is_ignored_and_junk_is_not() {
        assert_eq!(decode("Zm9v\nYmFy ").unwrap(), b"foobar");
        assert!(decode("Zm9*").is_none());
        assert!(decode("Zg=").is_none());
        assert!(decode("Z").is_none());
        assert!(decode("Zg==Zg==").is_none());
    }

    #[test]
    fn every_byte_value_round_trips() {
        let all: Vec<u8> = (0..=255).collect();
        assert_eq!(decode(&encode(&all)).unwrap(), all);
    }
}
