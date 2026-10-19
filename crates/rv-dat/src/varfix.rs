//! `DAT/DATReader/Utils/VarFix.cs`.

/// `VarFix.ULong(string)` (`VarFix.cs:19-48`): decimal or `0x` hex, `-` and empty are null.
///
/// `UInt64.TryParse` accepts surrounding white space and a sign (`-0` parses, other negatives
/// do not).
pub fn ulong(n: &str) -> Option<u64> {
    if n.is_empty() || n == "-" {
        return None;
    }
    if n.len() >= 2 && n.as_bytes()[..2].eq_ignore_ascii_case(b"0x") {
        // `Convert.ToUInt64(s, 16)`: hex digits only, overflow throws and becomes null. It also
        // skips one more `0x` of its own, so `0x0x1f` is 31 (`0x0x` is empty and stays null).
        let digits = &n[2..];
        let digits = match digits.as_bytes() {
            [b'0', b'x' | b'X', ..] => &digits[2..],
            _ => digits,
        };
        if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        return u64::from_str_radix(digits, 16).ok();
    }
    let t = n.trim_matches(|c: char| matches!(c, ' ' | '\t' | '\n' | '\u{b}' | '\u{c}' | '\r'));
    let (neg, digits) = match t.as_bytes().first() {
        Some(b'+') => (false, &t[1..]),
        Some(b'-') => (true, &t[1..]),
        _ => (false, t),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let v: u64 = digits.parse().ok()?;
    if neg && v != 0 {
        return None;
    }
    Some(v)
}

/// `VarFix.CleanMD5SHA1` (`VarFix.cs:83-138`): `None` for empty, `-` or a non-hex string.
///
/// Quirks kept: the text is left-padded with zeros up to `length` hex digits, and an odd
/// number of digits above `length` drops the last one (`retL = Length / 2`).
pub fn clean_hash(checksum: &str, length: usize) -> Option<Vec<u8>> {
    if checksum.is_empty() {
        return None;
    }
    let mut c = checksum.to_lowercase().trim().to_owned();
    if c.len() >= 2 && c.starts_with("0x") {
        c.drain(..2);
    }
    if c.is_empty() || c == "-" {
        return None;
    }
    if c.len() < length {
        c = format!("{}{}", "0".repeat(length - c.len()), c);
    }
    let digits: Vec<char> = c.chars().collect();
    let mut out = Vec::with_capacity(digits.len() / 2);
    for pair in digits.as_chunks::<2>().0 {
        let hi = pair[0].to_digit(16)?;
        let lo = pair[1].to_digit(16)?;
        out.push((hi * 16 + lo) as u8);
    }
    Some(out)
}

/// `VarFix.CleanCHD` (`VarFix.cs:54-65`): strip a trailing `.chd`, any case.
pub fn clean_chd(n: &str) -> String {
    let b = n.as_bytes();
    if b.len() >= 4 && b[b.len() - 4..].eq_ignore_ascii_case(b".chd") {
        n[..n.len() - 4].to_owned()
    } else {
        n.to_owned()
    }
}

/// `VarFix.ToLower(string)` (`VarFix.cs:164-172`).
pub fn to_lower(s: &str) -> String {
    s.to_lowercase()
}

/// `string.IsNullOrWhiteSpace`.
pub fn is_blank(s: &str) -> bool {
    s.trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ulong_cases() {
        assert_eq!(ulong("123"), Some(123));
        assert_eq!(ulong(" 12 "), Some(12));
        assert_eq!(ulong("+7"), Some(7));
        assert_eq!(ulong("-0"), Some(0));
        assert_eq!(ulong("-5"), None);
        assert_eq!(ulong("0x1F"), Some(31));
        assert_eq!(ulong("0X1f"), Some(31));
        assert_eq!(ulong("0x"), None);
        assert_eq!(ulong("0xZZ"), None);
        // `Convert.ToUInt64` drops a second `0x` after the one `VarFix` strips
        assert_eq!(ulong("0x0x1f"), Some(31));
        assert_eq!(ulong("0X0X1F"), Some(31));
        assert_eq!(ulong("0x0x"), None);
        assert_eq!(ulong("0x0x0x1f"), None);
        assert_eq!(ulong("-"), None);
        assert_eq!(ulong(""), None);
        assert_eq!(ulong("12a"), None);
        // a multi-byte character where the `0x` test looks must not panic
        assert_eq!(ulong("\u{20ac}1"), None);
        assert_eq!(ulong("\u{e9}"), None);
        assert_eq!(ulong("18446744073709551615"), Some(u64::MAX));
        assert_eq!(ulong("18446744073709551616"), None);
    }

    #[test]
    fn clean_hash_cases() {
        assert_eq!(clean_hash("", 8), None);
        assert_eq!(clean_hash("-", 8), None);
        assert_eq!(clean_hash("0x", 8), None);
        assert_eq!(
            clean_hash("ABCD1234", 8),
            Some(vec![0xab, 0xcd, 0x12, 0x34])
        );
        assert_eq!(
            clean_hash(" 0xabcd1234 ", 8),
            Some(vec![0xab, 0xcd, 0x12, 0x34])
        );
        // left-padded to the target length (`VarFix.cs:120-123`)
        assert_eq!(clean_hash("1234", 8), Some(vec![0, 0, 0x12, 0x34]));
        assert_eq!(clean_hash("0", 8), Some(vec![0, 0, 0, 0]));
        // odd digit count above the target drops the last digit (`retL = Length / 2`)
        assert_eq!(
            clean_hash("123456789", 8),
            Some(vec![0x12, 0x34, 0x56, 0x78])
        );
        assert_eq!(clean_hash("zz", 2), None);
    }

    #[test]
    fn clean_chd_cases() {
        assert_eq!(clean_chd("disk.chd"), "disk");
        assert_eq!(clean_chd("disk.CHD"), "disk");
        assert_eq!(clean_chd("disk"), "disk");
        assert_eq!(clean_chd(".chd"), "");
        assert_eq!(clean_chd("chd"), "chd");
    }
}
