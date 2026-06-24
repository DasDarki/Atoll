use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use slint::{Timer, TimerMode};

/// The longest the engine may stay without message loop work.
const IDLE_INTERVAL: Duration = Duration::from_millis(1000 / 30);

type Work = Box<dyn Fn()>;

/// Drives the engine's message loop from the user interface event loop.
///
/// The engine asks for work from arbitrary threads, the requests are forwarded to the main
/// thread and coalesced into a single timer.
pub struct MessagePump {
    timer: Timer,
    due: Cell<Option<Instant>>,
    work: Work,
}

thread_local! {
    static ACTIVE: RefCell<Weak<MessagePump>> = const { RefCell::new(Weak::new()) };
}

impl MessagePump {
    /// Starts pumping with `work`, which performs one message loop iteration. Pumping stops
    /// when the returned pump is dropped.
    pub fn start(work: impl Fn() + 'static) -> Rc<Self> {
        let pump = Rc::new(Self {
            timer: Timer::default(),
            due: Cell::new(None),
            work: Box::new(work),
        });
        ACTIVE.with_borrow_mut(|active| *active = Rc::downgrade(&pump));
        pump.schedule(Duration::ZERO);
        pump
    }

    /// A scheduler for the engine that may be called from any thread.
    pub fn scheduler() -> Box<dyn Fn(Duration) + Send + Sync> {
        Box::new(|delay| {
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(pump) = ACTIVE.with_borrow(Weak::upgrade) {
                    pump.schedule(delay);
                }
            });
        })
    }

    /// Runs the work after `delay` unless an earlier run is already due.
    fn schedule(self: &Rc<Self>, delay: Duration) {
        let due = Instant::now() + delay;
        if self.due.get().is_some_and(|current| current <= due) {
            return;
        }
        self.due.set(Some(due));
        let pump = Rc::downgrade(self);
        self.timer.start(TimerMode::SingleShot, delay, move || {
            if let Some(pump) = pump.upgrade() {
                pump.run();
            }
        });
    }

    fn run(self: &Rc<Self>) {
        self.due.set(None);
        (self.work)();
        self.schedule(IDLE_INTERVAL);
    }
}
