//! The clock on the wall. The rest of Atoll counts in milliseconds since 1970, the history shows
//! days and times, and those only make sense in the time zone of the user.

/// A moment as the local clock and calendar show it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalTime {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    /// Sunday is 0, as the C library has it.
    pub weekday: u32,
}

impl LocalTime {
    /// Whether both moments fall on the same day.
    pub fn same_day(self, other: LocalTime) -> bool {
        (self.year, self.month, self.day) == (other.year, other.month, other.day)
    }
}

/// Breaks a moment in seconds since 1970 into the local date and time.
pub fn local_time(unix_seconds: i64) -> LocalTime {
    let seconds = unix_seconds as libc::time_t;
    let mut parts: libc::tm = unsafe { std::mem::zeroed() };
    let filled = unsafe { libc::localtime_r(&seconds, &mut parts) };
    if filled.is_null() {
        let (year, month, day) = atoll_core::time::calendar_date(unix_seconds);
        return LocalTime {
            year,
            month,
            day,
            hour: 0,
            minute: 0,
            weekday: 0,
        };
    }
    LocalTime {
        year: i64::from(parts.tm_year) + 1900,
        month: parts.tm_mon as u32 + 1,
        day: parts.tm_mday as u32,
        hour: parts.tm_hour as u32,
        minute: parts.tm_min as u32,
        weekday: parts.tm_wday as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_moment_becomes_a_date_and_a_time() {
        // SAFETY: the tests of this module are the only ones that touch the time zone.
        unsafe { std::env::set_var("TZ", "UTC") };
        let time = local_time(1_758_196_500);
        assert_eq!((time.year, time.month, time.day), (2025, 9, 18));
        assert_eq!((time.hour, time.minute), (11, 55));
        assert!(time.same_day(local_time(1_758_196_500 + 60)));
        assert!(!time.same_day(local_time(1_758_196_500 + 86_400)));
    }
}
