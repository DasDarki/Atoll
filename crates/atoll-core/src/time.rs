use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Milliseconds since the Unix epoch, the unit used for every persisted point in time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Timestamp(pub i64);

impl Timestamp {
    pub fn now() -> Self {
        let elapsed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO);
        Self(i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
    }

    pub fn millis(self) -> i64 {
        self.0
    }

    pub fn elapsed_since(self, earlier: Timestamp) -> Duration {
        Duration::from_millis(u64::try_from(self.0 - earlier.0).unwrap_or(0))
    }
}

/// The calendar date (year, month, day) in UTC of a point in time given in seconds since the Unix
/// epoch.
pub fn calendar_date(unix_seconds: i64) -> (i64, u32, u32) {
    let days = unix_seconds.div_euclid(86_400);
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_index + 2) / 5 + 1) as u32;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    } as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::calendar_date;

    #[test]
    fn calendar_dates_are_computed_in_utc() {
        assert_eq!(calendar_date(0), (1970, 1, 1));
        assert_eq!(calendar_date(951_782_400), (2000, 2, 29));
        assert_eq!(calendar_date(1_758_153_600), (2025, 9, 18));
        assert_eq!(calendar_date(-86_400), (1969, 12, 31));
    }
}
