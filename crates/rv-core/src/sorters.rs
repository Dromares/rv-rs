//! The name orderings that the database, the DATs and the scanner share
//! (`libraries/SortMethods/Sorters.cs`, `RomVaultCore/RVSorters.cs`).
//!
//! Every comparison is over UTF-16 code units, like .NET's ordinal comparison, so the order of
//! names outside the basic plane matches a cache written by ROMVault. The case folding of
//! [`directory_name_compare`] is Rust's Unicode lower-casing, where .NET's `ToLower()` follows
//! the current culture; they agree for every name that is not in a Turkic locale.

use std::cmp::Ordering;

use crate::enums::FileType;

fn sign(o: Ordering) -> i32 {
    match o {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}

/// `string.Compare(a, b, StringComparison.Ordinal)` reduced to its sign.
pub fn ordinal_compare(a: &str, b: &str) -> i32 {
    sign(a.encode_utf16().cmp(b.encode_utf16()))
}

/// `Sorters.StringCompare`.
pub fn string_compare(a: &str, b: &str) -> i32 {
    ordinal_compare(a, b)
}

/// `Sorters.TrrntZipStringCompare`: compares after lower-casing ASCII `A`-`Z` only.
pub fn trrntzip_string_compare(a: &str, b: &str) -> i32 {
    let fold = |c: u16| if (65..=90).contains(&c) { c + 0x20 } else { c };
    let mut x = a.encode_utf16().map(fold);
    let mut y = b.encode_utf16().map(fold);
    loop {
        match (x.next(), y.next()) {
            (None, None) => return 0,
            (None, Some(_)) => return -1,
            (Some(_), None) => return 1,
            (Some(c1), Some(c2)) => {
                if c1 < c2 {
                    return -1;
                }
                if c1 > c2 {
                    return 1;
                }
            }
        }
    }
}

/// `Sorters.TrrntZipStringCompareCase`: the above, then ordinal.
pub fn trrntzip_string_compare_case(a: &str, b: &str) -> i32 {
    match trrntzip_string_compare(a, b) {
        0 => ordinal_compare(a, b),
        r => r,
    }
}

/// `Sorters.DirectoryNameCompare`: ordinal compare of the lower-cased names.
pub fn directory_name_compare(a: &str, b: &str) -> i32 {
    if a.is_ascii() && b.is_ascii() {
        // The common case, without allocating.
        let x = a.bytes().map(|c| c.to_ascii_lowercase());
        let y = b.bytes().map(|c| c.to_ascii_lowercase());
        return sign(x.cmp(y));
    }
    ordinal_compare(&a.to_lowercase(), &b.to_lowercase())
}

/// `Sorters.DirectoryNameCompareCase`: the above, then ordinal.
pub fn directory_name_compare_case(a: &str, b: &str) -> i32 {
    match directory_name_compare(a, b) {
        0 => ordinal_compare(a, b),
        r => r,
    }
}

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

/// `Sorters.Trrnt7ZipStringCompare`: extension, then name, then path, all ordinal.
pub fn trrnt_7zip_string_compare(a: &str, b: &str) -> i32 {
    let (path1, name1, ext1) = split_filename(a);
    let (path2, name2, ext2) = split_filename(b);
    match ordinal_compare(ext1, ext2) {
        0 => {}
        r => return r,
    }
    match ordinal_compare(name1, name2) {
        0 => {}
        r => return r,
    }
    ordinal_compare(path1, path2)
}

/// `RVSorters.CompareName(FileType, name, FileType, name)`: the order of the children of a
/// database directory, and of the scanned and DAT entries that are merged against them.
///
/// A mix of `FileZip` or `FileSevenZip` with another type is a program error in ROMVault (it
/// shows a dialog and compares anyway); here it just compares.
pub fn compare_name(f1: FileType, name1: &str, f2: FileType, name2: &str) -> i32 {
    if f1 == FileType::FILE_ZIP || f2 == FileType::FILE_ZIP {
        return trrntzip_string_compare_case(name1, name2);
    }
    if f1 == FileType::FILE_SEVEN_ZIP || f2 == FileType::FILE_SEVEN_ZIP {
        return trrnt_7zip_string_compare(name1, name2);
    }
    match directory_name_compare(name1, name2) {
        0 => sign(f1.0.cmp(&f2.0)),
        r => r,
    }
}

/// `RVSorters.CompareDatName`: `CurrentCultureIgnoreCase`, here a simple case fold. Both
/// separators count as the same one, because a cache written on Windows stores `\`.
pub fn compare_dat_name(a: &str, b: &str) -> i32 {
    let norm = |s: &str| s.replace('\\', "/");
    directory_name_compare(&norm(a), &norm(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trrntzip_folds_ascii_only_then_ordinal() {
        assert_eq!(trrntzip_string_compare("A", "a"), 0);
        assert_eq!(trrntzip_string_compare_case("A", "a"), -1);
        assert_eq!(trrntzip_string_compare("b", "A"), 1);
        // `_` (0x5f) sorts before the lower-case letters once ASCII is folded to lower case.
        assert_eq!(trrntzip_string_compare("a_", "aB"), -1);
        assert_eq!(trrntzip_string_compare("ab", "abc"), -1);
        // Non-ASCII is not folded.
        assert_eq!(trrntzip_string_compare("É", "é"), -1);
    }

    #[test]
    fn directory_compare_is_case_blind_and_utf16_ordinal() {
        assert_eq!(directory_name_compare("ABC", "abc"), 0);
        assert_eq!(directory_name_compare_case("ABC", "abc"), -1);
        // U+FF5E (BMP) sorts after U+1F600 (a surrogate pair, 0xD83D) in UTF-16 order, the
        // opposite of UTF-8 byte order.
        assert_eq!(ordinal_compare("\u{ff5e}", "\u{1f600}"), 1);
    }

    #[test]
    fn sevenzip_compares_extension_then_name_then_path() {
        assert_eq!(trrnt_7zip_string_compare("b.aaa", "a.bbb"), -1);
        assert_eq!(trrnt_7zip_string_compare("x/a.txt", "a.txt"), 1);
        assert_eq!(trrnt_7zip_string_compare("a", "a"), 0);
    }

    #[test]
    fn compare_name_breaks_ties_by_file_type() {
        assert_eq!(compare_name(FileType::DIR, "A", FileType::FILE, "a"), -1);
        assert_eq!(compare_name(FileType::FILE, "A", FileType::DIR, "a"), 1);
        assert_eq!(
            compare_name(FileType::FILE_ZIP, "B", FileType::FILE_ZIP, "a"),
            1
        );
        assert_eq!(
            compare_name(
                FileType::FILE_SEVEN_ZIP,
                "a.z",
                FileType::FILE_SEVEN_ZIP,
                "a.y"
            ),
            1
        );
    }

    #[test]
    fn dat_names_treat_both_separators_alike() {
        assert_eq!(compare_dat_name("DatRoot\\A\\x.dat", "datroot/a/X.DAT"), 0);
    }
}

/// `BinarySearch.ListSearch` (`Libraries/StorageList/BinarySearch.cs:55-99`). `cmp(item)`
/// compares the key with `item` (negative: the key sorts first). Returns the comparison result
/// of the last probe (`0` = found) and the index: the first of a run of equal items when found,
/// else the insertion point. The same probe order as ROMVault, so a list that is not perfectly
/// sorted gives the same answer.
pub fn list_search<T>(list: &[T], cmp: impl Fn(&T) -> i32) -> (i32, usize) {
    let mut bottom = 0usize;
    let mut top = list.len();
    let mut mid = 0usize;
    let mut res = -1;
    while bottom < top && res != 0 {
        mid = (bottom + top) / 2;
        res = cmp(&list[mid]);
        if res < 0 {
            top = mid;
        } else if res > 0 {
            bottom = mid + 1;
        }
    }
    let mut index = mid;
    if res == 0 {
        while index > 0 && cmp(&list[index - 1]) == 0 {
            index -= 1;
        }
    } else if res > 0 {
        index += 1;
    }
    (res, index)
}

#[cfg(test)]
mod list_search_tests {
    use super::*;

    #[test]
    fn finds_the_first_of_a_run_and_the_insertion_point() {
        let v = [1, 3, 3, 3, 5];
        let find = |k: i32| list_search(&v, |x| (k.cmp(x)) as i32);
        assert_eq!(find(3), (0, 1));
        assert_eq!(find(1), (0, 0));
        assert_eq!(find(0), (-1, 0));
        assert_eq!(find(4).1, 4, "between 3 and 5");
        assert_eq!(find(6).1, 5);
        assert_eq!(list_search(&[] as &[i32], |_| 0), (-1, 0));
    }
}
