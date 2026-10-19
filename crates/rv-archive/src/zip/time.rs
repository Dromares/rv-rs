//! DOS date/time and .NET tick conversions (`CompressUtils.cs:130-210`).

/// .NET ticks (100 ns since 0001-01-01) of the Unix epoch (`CompressUtils.cs:132`).
pub(crate) const EPOCH_TIME_TO_UTC_TIME: i64 = 621_355_968_000_000_000;
/// .NET ticks of the NTFS epoch 1601-01-01 (`CompressUtils.cs:131`).
pub(crate) const FILE_TIME_TO_UTC_TIME: i64 = 504_911_232_000_000_000;
const TICKS_PER_SECOND: i64 = 10_000_000;
const TICKS_PER_DAY: i64 = 864_000_000_000;

/// `UtcTicksFromUnixDateTime` (`CompressUtils.cs:203`).
pub(crate) fn ticks_from_unix(secs: i32) -> i64 {
    secs as i64 * TICKS_PER_SECOND + EPOCH_TIME_TO_UTC_TIME
}

/// `UtcTicksFromNtfsDateTime` (`CompressUtils.cs:195-198`).
pub(crate) fn ticks_from_ntfs(ntfs: i64) -> i64 {
    ntfs.wrapping_add(FILE_TIME_TO_UTC_TIME)
}

/// `CombineDosDateTime` (`CompressUtils.cs:183-186`): `(date << 16) | time` in `int` arithmetic,
/// so a date of 0x8000 or more (year 2044+) sign-extends to a negative value.
pub(crate) fn combine_dos(date: u16, time: u16) -> i64 {
    (((date as i32) << 16) | time as i32) as i64
}

/// `UtcTicksToDosDateTime` (`CompressUtils.cs:138-150`): a value up to `0xffffffff` is taken to be
/// packed already, anything larger is a tick count. Returns `(date, time)`.
pub fn ticks_to_dos(ticks: i64) -> (u16, u16) {
    if ticks <= 0xffff_ffff {
        return (((ticks >> 16) & 0xffff) as u16, (ticks & 0xffff) as u16);
    }
    let days = ticks.div_euclid(TICKS_PER_DAY);
    let rem = ticks.rem_euclid(TICKS_PER_DAY);
    let secs = rem / TICKS_PER_SECOND;
    let (hour, minute, second) = (secs / 3600, (secs / 60) % 60, secs % 60);
    let (year, month, day) = civil_from_days(days);
    let date = (day & 0x1f) | ((month & 0x0f) << 5) | (((year - 1980) & 0x7f) << 9);
    let time = ((second >> 1) & 0x1f) | ((minute & 0x3f) << 5) | ((hour & 0x1f) << 11);
    (date as u16, time as u16)
}

/// The packed `(date << 16) | time` of a DOS date and time, as the C# stores it.
pub fn dos_to_ticks(date: u16, time: u16) -> i64 {
    combine_dos(date, time)
}

/// Days since 0001-01-01 to a proleptic Gregorian `(year, month, day)`.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    // Howard Hinnant's algorithm, shifted so day 0 is 0001-01-01 (days from 0000-03-01: +306).
    let z = days + 306; // days since 0000-03-01
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
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
    fn packed_dos_values_pass_through() {
        assert_eq!(ticks_to_dos(563_657_728), (0x2198, 0xBC00));
    }

    #[test]
    fn tick_counts_convert_by_fields() {
        // 1996-12-24 23:32:00, `TrrntzipDateTime`.
        assert_eq!(ticks_to_dos(629_870_671_200_000_000), (0x2198, 0xBC00));
        // 2000-02-29 12:34:56 (seconds are stored halved).
        let ticks = 630_873_792_000_000_000 + 10_000_000 * (12 * 3600 + 34 * 60 + 56);
        let (date, time) = ticks_to_dos(ticks);
        assert_eq!(date, 29 | (2 << 5) | (20 << 9));
        assert_eq!(time, 28 | (34 << 5) | (12 << 11));
    }

    #[test]
    fn year_2044_dates_sign_extend() {
        let t = combine_dos(0x8000, 0);
        assert!(t < 0);
        assert_eq!(ticks_to_dos(t), (0x8000, 0));
    }

    #[test]
    fn unix_and_ntfs_epochs() {
        assert_eq!(ticks_from_unix(0), EPOCH_TIME_TO_UTC_TIME);
        assert_eq!(ticks_from_ntfs(0), FILE_TIME_TO_UTC_TIME);
    }
}
