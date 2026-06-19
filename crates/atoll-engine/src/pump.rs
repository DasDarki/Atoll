use std::sync::OnceLock;
use std::time::Duration;

type Scheduler = Box<dyn Fn(Duration) + Send + Sync>;

static SCHEDULER: OnceLock<Scheduler> = OnceLock::new();

/// Upper bound for the delay between two message loop iterations while the engine is idle.
pub(crate) const MAX_DELAY: Duration = Duration::from_millis(1000 / 30);

pub(crate) fn install(scheduler: Scheduler) {
    let _ = SCHEDULER.set(scheduler);
}

/// Called by the engine from arbitrary threads to request message loop work.
pub(crate) fn schedule(delay_ms: i64) {
    let delay = Duration::from_millis(u64::try_from(delay_ms).unwrap_or(0)).min(MAX_DELAY);
    if let Some(scheduler) = SCHEDULER.get() {
        scheduler(delay);
    }
}
