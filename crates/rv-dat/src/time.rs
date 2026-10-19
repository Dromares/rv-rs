//! Date handling of the DOSCenter format (`DatDOSReader.cs:239-271`,
//! `Compress/CompressUtils.cs:153-178`).

use crate::error::{Abort, R};

/// .NET ticks (100 ns) per second.
const TICKS_PER_SECOND: i64 = 10_000_000;

/// Days from 0001-01-01 to the given proleptic Gregorian date.
fn days_from_civil_0001(y: i64, m: i64, d: i64) -> i64 {
    // Howard Hinnant's algorithm, days since 1970-01-01, then shifted to 0001-01-01.
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let since_1970 = era * 146_097 + doe - 719_468;
    since_1970 + 719_162
}

fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        _ => 28,
    }
}

/// `new DateTime(year, month, day, hour, minute, second).Ticks`; throws for impossible values.
fn date_time_ticks(y: i64, mo: i64, d: i64, h: i64, mi: i64, s: i64) -> R<i64> {
    if !(1..=9999).contains(&y)
        || !(1..=12).contains(&mo)
        || d < 1
        || d > days_in_month(y, mo)
        || !(0..24).contains(&h)
        || !(0..60).contains(&mi)
        || !(0..60).contains(&s)
    {
        return Err(Abort(
            "Year, Month, and Day parameters describe an un-representable DateTime".into(),
        ));
    }
    let days = days_from_civil_0001(y, mo, d);
    Ok(((days * 24 + h) * 60 + mi) * 60 * TICKS_PER_SECOND + s * TICKS_PER_SECOND)
}

fn parse_int(s: &str) -> Option<i64> {
    // `int.TryParse`: optional sign, digits, surrounding white space.
    let t = s.trim();
    let digits = t.strip_prefix(['+', '-']).unwrap_or(t);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    t.parse::<i64>().ok().filter(|v| i32::try_from(*v).is_ok())
}

/// `DatDOSReader.StringDateTimeToTicks`: `yyyy/mm/dd hh:mm:ss` into the packed MS-DOS value
/// `(date << 16) | time`; years outside 1980..=2107 give .NET ticks instead.
pub(crate) fn string_date_time_to_ticks(s: &str) -> R<Option<i64>> {
    let b: Vec<char> = s.chars().collect();
    if b.len() != 19 {
        return Ok(None);
    }
    if b[4] != '/' || b[7] != '/' || b[10] != ' ' || b[13] != ':' || b[16] != ':' {
        return Ok(None);
    }
    let part = |from: usize, len: usize| -> Option<i64> {
        parse_int(&b[from..from + len].iter().collect::<String>())
    };
    let (Some(year), Some(month), Some(day), Some(hours), Some(minutes), Some(seconds)) = (
        part(0, 4),
        part(5, 2),
        part(8, 2),
        part(11, 2),
        part(14, 2),
        part(17, 2),
    ) else {
        return Ok(None);
    };
    // `(seconds % 1) == 1` in the source is never true (`:265`).
    if !(0..=0x7f).contains(&(year - 1980)) {
        return date_time_ticks(year, month, day, hours, minutes, seconds).map(Some);
    }
    let dos_date = (day & 0x1f) | ((month & 0x0f) << 5) | (((year - 1980) & 0x7f) << 9);
    let dos_time = ((seconds >> 1) & 0x1f) | ((minutes & 0x3f) << 5) | ((hours & 0x1f) << 11);
    Ok(Some((dos_date << 16) | dos_time))
}

/// `CompressUtils.zipDateTimeToString` (`CompressUtils.cs:153-181`): `yyyy/MM/dd HH:mm:ss`, or
/// `""` for null, 0 or `long.MinValue`.
pub(crate) fn zip_date_time_to_string(v: Option<i64>) -> String {
    let Some(v) = v else {
        return String::new();
    };
    if v == 0 || v == i64::MIN {
        return String::new();
    }
    if v > 0xffff_ffff {
        // .NET ticks
        if !(0..=3_155_378_975_999_999_999).contains(&v) {
            return String::new();
        }
        let secs = v / TICKS_PER_SECOND;
        let days = secs / 86_400;
        let rem = secs % 86_400;
        let (y, m, d) = civil_from_days_0001(days);
        return format!(
            "{y:04}/{m:02}/{d:02} {:02}:{:02}:{:02}",
            rem / 3600,
            rem % 3600 / 60,
            rem % 60
        );
    }
    let dos_date = (v >> 16) & 0xffff;
    let dos_time = v & 0xffff;
    let second = (dos_time & 0x1f) << 1;
    let minute = (dos_time >> 5) & 0x3f;
    let hour = (dos_time >> 11) & 0x1f;
    let day = dos_date & 0x1f;
    let month = (dos_date >> 5) & 0x0f;
    let year = ((dos_date >> 9) & 0x7f) + 1980;
    format!("{year:04}/{month:02}/{day:02} {hour:02}:{minute:02}:{second:02}")
}

fn civil_from_days_0001(days: i64) -> (i64, i64, i64) {
    let z = days - 719_162 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dos_packing_round_trip() {
        // The TorrentZip date, `StructuredZip.cs:414`: 1996/12/24 23:32:00.
        let v = string_date_time_to_ticks("1996/12/24 23:32:00")
            .unwrap()
            .unwrap();
        assert_eq!(v, 563_657_728);
        assert_eq!(zip_date_time_to_string(Some(v)), "1996/12/24 23:32:00");
        let v = string_date_time_to_ticks("2020/02/29 12:34:56")
            .unwrap()
            .unwrap();
        assert_eq!(zip_date_time_to_string(Some(v)), "2020/02/29 12:34:56");
    }

    #[test]
    fn out_of_dos_range_gives_ticks() {
        // `StructuredZip.cs:413`: TrrntzipDateTime = 629870671200000000 is 1996-12-24 23:32:00.
        assert_eq!(
            string_date_time_to_ticks("1970/01/01 00:00:00").unwrap(),
            Some(621_355_968_000_000_000)
        );
        assert_eq!(
            zip_date_time_to_string(Some(621_355_968_000_000_000)),
            "1970/01/01 00:00:00"
        );
        assert_eq!(
            string_date_time_to_ticks("2108/01/01 00:00:00").unwrap(),
            Some(date_time_ticks(2108, 1, 1, 0, 0, 0).unwrap())
        );
        assert!(string_date_time_to_ticks("1970/13/01 00:00:00").is_err());
    }

    #[test]
    fn bad_shapes_are_none() {
        assert_eq!(
            string_date_time_to_ticks("1996-12-24 23:32:00").unwrap(),
            None
        );
        assert_eq!(string_date_time_to_ticks("1996/12/24").unwrap(), None);
        assert_eq!(
            string_date_time_to_ticks("abcd/12/24 23:32:00").unwrap(),
            None
        );
    }

    #[test]
    fn empty_cases() {
        assert_eq!(zip_date_time_to_string(None), "");
        assert_eq!(zip_date_time_to_string(Some(0)), "");
        assert_eq!(zip_date_time_to_string(Some(i64::MIN)), "");
    }
}
