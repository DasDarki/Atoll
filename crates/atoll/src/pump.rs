use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::{Timer, TimerMode};

/// The longest the engine may stay without message loop work.
const IDLE_INTERVAL: Duration = Duration::from_millis(1000 / 30);

type Work = Rc<dyn Fn()>;

#[derive(Default)]
struct Pump {
    timer: Timer,
    due: Cell<Option<Instant>>,
    work: RefCell<Option<Work>>,
}

thread_local! {
    static PUMP: Pump = Pump::default();
}

/// A scheduler for the engine that may be called from any thread.
pub fn scheduler() -> Box<dyn Fn(Duration) + Send + Sync> {
    Box::new(|delay| {
        let _ = slint::invoke_from_event_loop(move || schedule(delay));
    })
}

/// Starts driving the engine with `work`, which performs one message loop iteration.
pub fn start(work: impl Fn() + 'static) {
    PUMP.with(|pump| *pump.work.borrow_mut() = Some(Rc::new(work)));
    schedule(Duration::ZERO);
}

pub fn stop() {
    PUMP.with(|pump| {
        pump.timer.stop();
        pump.due.set(None);
        pump.work.borrow_mut().take();
    });
}

/// Runs the work after `delay` unless an earlier run is already due.
pub fn schedule(delay: Duration) {
    PUMP.with(|pump| {
        if pump.work.borrow().is_none() {
            return;
        }
        let due = Instant::now() + delay;
        if pump.due.get().is_some_and(|current| current <= due) {
            return;
        }
        pump.due.set(Some(due));
        pump.timer.start(TimerMode::SingleShot, delay, run);
    });
}

fn run() {
    let work = PUMP.with(|pump| {
        pump.due.set(None);
        pump.work.borrow().clone()
    });
    if let Some(work) = work {
        work();
        schedule(IDLE_INTERVAL);
    }
}
