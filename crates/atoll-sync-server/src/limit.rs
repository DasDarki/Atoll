use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How many requests an account may make per window.
const REQUESTS_PER_WINDOW: u32 = 600;
const WINDOW: Duration = Duration::from_secs(60);

/// A brake for accounts that ask too often, counted per minute.
#[derive(Default)]
pub struct Limiter {
    counts: Mutex<HashMap<String, (Instant, u32)>>,
}

impl Limiter {
    /// Whether `account` may make another request now.
    pub fn allow(&self, account: &str) -> bool {
        let Ok(mut counts) = self.counts.lock() else {
            return true;
        };
        let now = Instant::now();
        if counts.len() > 10_000 {
            counts.retain(|_, (start, _)| now.duration_since(*start) < WINDOW);
        }
        let entry = counts.entry(account.to_owned()).or_insert((now, 0));
        if now.duration_since(entry.0) >= WINDOW {
            *entry = (now, 0);
        }
        entry.1 += 1;
        entry.1 <= REQUESTS_PER_WINDOW
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_account_is_slowed_down_after_its_share_and_others_are_not() {
        let limiter = Limiter::default();
        for _ in 0..REQUESTS_PER_WINDOW {
            assert!(limiter.allow("a"));
        }
        assert!(!limiter.allow("a"));
        assert!(limiter.allow("b"));
    }
}
