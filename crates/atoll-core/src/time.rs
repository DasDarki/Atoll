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
