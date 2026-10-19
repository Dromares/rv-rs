//! .NET ticks, the unit of every time stamp in the cache.

use std::fs;
use std::time::UNIX_EPOCH;

/// Ticks (100 ns units) between 0001-01-01 and 1970-01-01 (`DateTime.UnixEpoch.Ticks`).
pub const UNIX_EPOCH_TICKS: i64 = 621_355_968_000_000_000;

/// A file's last-write time as .NET UTC ticks (`FileInfo.LastWriteTimeUtc.Ticks`, `RVIO.cs:74`).
/// `0` when it cannot be read, like ROMVault's catch.
pub fn ticks_of(meta: &fs::Metadata) -> i64 {
    let Ok(modified) = meta.modified() else {
        return 0;
    };
    match modified.duration_since(UNIX_EPOCH) {
        Ok(d) => {
            UNIX_EPOCH_TICKS + (d.as_secs() as i64) * 10_000_000 + i64::from(d.subsec_nanos() / 100)
        }
        Err(e) => {
            let d = e.duration();
            UNIX_EPOCH_TICKS - (d.as_secs() as i64) * 10_000_000 - i64::from(d.subsec_nanos() / 100)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_known_time_in_ticks() {
        // 2024-09-01 03:52:00 UTC, the modification time of the board's NES DAT; the cache
        // stores 638607595200000000 for it.
        let secs = 1_725_162_720i64;
        assert_eq!(
            UNIX_EPOCH_TICKS + secs * 10_000_000,
            638_607_595_200_000_000
        );
    }

    #[test]
    fn a_file_written_now_is_after_2024() {
        let dir = std::env::temp_dir();
        let meta = fs::metadata(&dir).expect("temp dir");
        assert!(ticks_of(&meta) > 638_000_000_000_000_000);
    }
}
