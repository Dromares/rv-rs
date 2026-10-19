//! The name comparators that decide TorrentZip order (`libraries/SortMethods/Sorters.cs`).
//! All compare UTF-16 code units, like the .NET `char` loops, so names outside the BMP order the
//! same way as in ROMVault.

use std::cmp::Ordering;

fn units(s: &str) -> impl Iterator<Item = u16> + '_ {
    s.encode_utf16()
}

/// `Sorters.StringCompare` / `string.Compare(..., Ordinal)`: UTF-16 code unit order.
pub fn ordinal_compare(a: &str, b: &str) -> Ordering {
    units(a).cmp(units(b))
}

fn fold(c: u16) -> u16 {
    if (65..=90).contains(&c) {
        c + 0x20
    } else {
        c
    }
}

/// `Sorters.TrrntZipStringCompare` (`Sorters.cs:37-62`): ASCII `A-Z` folded to lower case, then
/// code unit order; a proper prefix sorts first.
pub fn trrntzip_compare(a: &str, b: &str) -> Ordering {
    trrntzip_compare_units(
        &a.encode_utf16().collect::<Vec<_>>(),
        &b.encode_utf16().collect::<Vec<_>>(),
    )
}

/// [`trrntzip_compare`] on UTF-16 code units (the C# compares `Substring` prefixes, which can
/// split a surrogate pair).
pub fn trrntzip_compare_units(a: &[u16], b: &[u16]) -> Ordering {
    a.iter().map(|&c| fold(c)).cmp(b.iter().map(|&c| fold(c)))
}

/// `Sorters.TrrntZipStringCompareCase` (`Sorters.cs:63-68`): the same, with an ordinal
/// tie-break. This is what the writer checks; the verifier uses [`trrntzip_compare`].
pub fn trrntzip_compare_case(a: &str, b: &str) -> Ordering {
    trrntzip_compare_case_units(
        &a.encode_utf16().collect::<Vec<_>>(),
        &b.encode_utf16().collect::<Vec<_>>(),
    )
}

/// [`trrntzip_compare_case`] on UTF-16 code units.
pub fn trrntzip_compare_case_units(a: &[u16], b: &[u16]) -> Ordering {
    trrntzip_compare_units(a, b).then_with(|| a.cmp(b))
}

/// `Sorters.Trrnt7ZipStringCompare` (`Sorters.cs:86-135`): extension, then name, then path, each
/// ordinal.
pub fn trrnt7zip_compare(a: &str, b: &str) -> Ordering {
    let (path1, name1, ext1) = split_filename(a);
    let (path2, name2, ext2) = split_filename(b);
    ordinal_compare(ext1, ext2)
        .then_with(|| ordinal_compare(name1, name2))
        .then_with(|| ordinal_compare(path1, path2))
}

/// `splitFilename` (`Sorters.cs:108-135`): split at the last `/`, then the last `.` of the leaf.
fn split_filename(filename: &str) -> (&str, &str, &str) {
    let (path, name) = match filename.rfind('/') {
        Some(i) => (&filename[..i], &filename[i + 1..]),
        None => ("", filename),
    };
    match name.rfind('.') {
        Some(i) => (path, &name[..i], &name[i + 1..]),
        None => (path, name, ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trrntzip_folds_ascii_case_only() {
        assert_eq!(trrntzip_compare("ABC", "abc"), Ordering::Equal);
        assert_eq!(trrntzip_compare("a", "B"), Ordering::Less);
        assert_eq!(trrntzip_compare("a_", "aa"), Ordering::Less);
        assert_eq!(trrntzip_compare("é", "E"), Ordering::Greater);
    }

    #[test]
    fn prefix_sorts_first() {
        assert_eq!(trrntzip_compare("dir", "dir/"), Ordering::Less);
        assert_eq!(trrntzip_compare("", "a"), Ordering::Less);
    }

    #[test]
    fn case_variant_breaks_ties_ordinally() {
        assert_eq!(trrntzip_compare_case("ABC", "abc"), Ordering::Less);
        assert_eq!(trrntzip_compare_case("abc", "ABC"), Ordering::Greater);
        assert_eq!(trrntzip_compare_case("abc", "abc"), Ordering::Equal);
    }

    #[test]
    fn utf16_order_not_utf8_order() {
        // U+FF5E (BMP, UTF-8 EF BD 9E) vs U+1F600 (UTF-16 D83D DE00, UTF-8 F0 9F 98 80):
        // UTF-8 bytes put the astral char last, UTF-16 units put it first.
        assert_eq!(ordinal_compare("\u{ff5e}", "\u{1f600}"), Ordering::Greater);
    }

    #[test]
    fn seven_zip_order_is_ext_name_path() {
        assert_eq!(trrnt7zip_compare("b.a", "a.b"), Ordering::Less);
        assert_eq!(trrnt7zip_compare("x/a.txt", "y/a.txt"), Ordering::Less);
        assert_eq!(trrnt7zip_compare("a.txt", "a.txt"), Ordering::Equal);
    }
}
