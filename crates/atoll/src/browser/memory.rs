use std::cell::{Cell, RefCell};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

use slint::ComponentHandle;

use super::Browser;
use super::downloads::format_bytes;
use super::preferences::{MAX_MEMORY_LIMIT_GB, PreferenceKey};
use crate::platform::memory::renderer_memory;
use crate::ui::SettingsState;
use atoll_i18n::t;

/// How often the memory of the pages is measured.
const INTERVAL: Duration = Duration::from_secs(30);

/// How long after a tab was put to sleep the memory is measured again. Its renderer process
/// needs a moment to exit.
const RECHECK: Duration = Duration::from_secs(3);

type Listener = Box<dyn Fn(u64)>;

thread_local! {
    static LISTENER: RefCell<Option<Listener>> = const { RefCell::new(None) };
}

#[derive(Default)]
struct Signals {
    soon: bool,
    stop: bool,
}

/// Measures the memory the pages use on a background thread.
#[derive(Default)]
pub struct MemoryWatcher {
    signals: Arc<(Mutex<Signals>, Condvar)>,
    started: Cell<bool>,
    usage: Cell<Option<u64>>,
}

impl MemoryWatcher {
    /// Starts measuring. `on_measured` runs on the main thread with every result in bytes.
    pub fn start(&self, on_measured: impl Fn(u64) + 'static) {
        if self.started.replace(true) {
            return;
        }
        LISTENER.with_borrow_mut(|listener| *listener = Some(Box::new(on_measured)));
        let signals = self.signals.clone();
        let spawned = thread::Builder::new()
            .name("memory-watcher".into())
            .spawn(move || watch(&signals));
        if let Err(error) = spawned {
            tracing::error!(%error, "memory watcher could not be started");
        }
    }

    /// Measures again shortly, after a tab was put to sleep or the budget changed.
    pub fn measure_soon(&self) {
        let (signals, wake) = &*self.signals;
        if let Ok(mut signals) = signals.lock() {
            signals.soon = true;
        }
        wake.notify_all();
    }

    pub fn usage(&self) -> Option<u64> {
        self.usage.get()
    }

    pub(super) fn set_usage(&self, bytes: u64) {
        self.usage.set(Some(bytes));
    }
}

impl Drop for MemoryWatcher {
    fn drop(&mut self) {
        let (signals, wake) = &*self.signals;
        if let Ok(mut signals) = signals.lock() {
            signals.stop = true;
        }
        wake.notify_all();
    }
}

fn watch(signals: &(Mutex<Signals>, Condvar)) {
    let (state, wake) = signals;
    loop {
        let Some(bytes) = renderer_memory() else {
            return;
        };
        let _ = slint::invoke_from_event_loop(move || {
            LISTENER.with_borrow(|listener| {
                if let Some(listener) = listener {
                    listener(bytes);
                }
            });
        });
        let Ok(guard) = state.lock() else { return };
        let Ok((mut guard, _)) =
            wake.wait_timeout_while(guard, INTERVAL, |signals| !signals.soon && !signals.stop)
        else {
            return;
        };
        if guard.stop {
            return;
        }
        if std::mem::take(&mut guard.soon) {
            drop(guard);
            thread::sleep(RECHECK);
        }
    }
}

impl Browser {
    /// Fills the memory part of the settings page.
    pub(super) fn show_memory_settings(&self) {
        let Some(window) = self.window() else { return };
        let state = window.global::<SettingsState>();
        let preferences = self.preferences.borrow();
        state.set_memory_limit_enabled(preferences.memory_limit_enabled);
        state.set_memory_limit_gb(preferences.memory_limit_gb as i32);
        state.set_max_memory_limit_gb(MAX_MEMORY_LIMIT_GB as i32);
        state.set_memory_usage(
            self.memory
                .usage()
                .map(|bytes| {
                    t!(
                        "Geöffnete Seiten belegen gerade {}.",
                        format_bytes(bytes as i64)
                    )
                })
                .unwrap_or_default()
                .into(),
        );
    }

    pub(super) fn set_memory_limit(&self, enabled: bool, gigabytes: u32) {
        let gigabytes = gigabytes.clamp(1, MAX_MEMORY_LIMIT_GB);
        self.update_preference(PreferenceKey::MemoryLimitEnabled, |preferences| {
            preferences.memory_limit_enabled = enabled;
        });
        self.update_preference(PreferenceKey::MemoryLimitGb, |preferences| {
            preferences.memory_limit_gb = gigabytes;
        });
        self.memory.measure_soon();
    }

    /// Updates the settings page after a measurement.
    pub(super) fn memory_measured(&self) {
        if self
            .window()
            .is_some_and(|window| window.global::<SettingsState>().get_visible())
        {
            self.show_memory_settings();
        }
    }
}

/// Whether `usage` bytes exceed a budget of `gigabytes`.
pub(super) fn exceeds_budget(usage: u64, gigabytes: u32) -> bool {
    usage > u64::from(gigabytes) << 30
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_budget_is_counted_in_binary_gigabytes() {
        assert!(!exceeds_budget(4 << 30, 4));
        assert!(exceeds_budget((4 << 30) + 1, 4));
        assert!(!exceeds_budget(0, 1));
    }
}
