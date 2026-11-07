//! Days as the add-on counts them.
//!
//! The add-on's day is `floor((unix_time - SCAN_DAY_0) / 86400)`, where
//! `SCAN_DAY_0` is the local midnight of 2020-01-01 (`Prices.lua`). Across DST
//! changes this differs from the local calendar date, so always use these
//! functions, never the calendar.

use chrono::{Local, TimeZone};

pub const SECONDS_PER_DAY: i64 = 86_400;

/// Unix time of the local midnight of 2020-01-01, as Lua's
/// `time({year=2020, month=1, day=1, hour=0})` computes it.
pub fn scan_day_0() -> i64 {
    Local
        .with_ymd_and_hms(2020, 1, 1, 0, 0, 0)
        .earliest()
        .expect("local midnight of 2020-01-01 exists")
        .timestamp()
}

/// The add-on day of a unix time.
pub fn day_of(unix_time: i64, day_0: i64) -> i64 {
    (unix_time - day_0).div_euclid(SECONDS_PER_DAY)
}

/// The unix time at which a day starts.
pub fn day_start(day: i64, day_0: i64) -> i64 {
    day * SECONDS_PER_DAY + day_0
}

/// The first day the add-on keeps: it keeps days `> today - retention_days`.
pub fn first_kept_day(today: i64, retention_days: i64) -> i64 {
    today - retention_days + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2020-01-01T00:00:00Z, as if the local time zone were UTC.
    const DAY_0_UTC: i64 = 1_577_836_800;

    #[test]
    fn day_of_counts_from_day_0() {
        assert_eq!(day_of(DAY_0_UTC, DAY_0_UTC), 0);
        assert_eq!(day_of(DAY_0_UTC + SECONDS_PER_DAY - 1, DAY_0_UTC), 0);
        assert_eq!(day_of(DAY_0_UTC + SECONDS_PER_DAY, DAY_0_UTC), 1);
        assert_eq!(day_of(DAY_0_UTC - 1, DAY_0_UTC), -1);
    }

    #[test]
    fn day_start_is_inverse_of_day_of() {
        for day in [0, 1, 2501, 2502] {
            let start = day_start(day, DAY_0_UTC);
            assert_eq!(day_of(start, DAY_0_UTC), day);
            assert_eq!(day_of(start - 1, DAY_0_UTC), day - 1);
        }
    }

    #[test]
    fn retention_keeps_today_and_previous_days() {
        // 90 days: today and the 89 days before it.
        assert_eq!(first_kept_day(2500, 90), 2411);
        assert_eq!(first_kept_day(2500, 1), 2500);
    }
}
