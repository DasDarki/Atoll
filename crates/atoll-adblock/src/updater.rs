use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use crate::blocker::Blocker;
use crate::download::{self, DownloadError};
use crate::lists::{FILTER_LISTS, FilterList, ListDirectory, build_engine};

/// Lists older than this are downloaded again. EasyList expires after four days.
const MAX_AGE: Duration = Duration::from_secs(4 * 24 * 60 * 60);

/// How often the age of the lists is checked, and a failed download retried.
const CHECK_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// What the filter lists are doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListState {
    /// Nothing is blocked yet, the lists are being read or downloaded for the first time.
    Loading,
    Ready,
    /// Newer lists are downloaded, the current ones stay in use meanwhile.
    Updating,
    /// Downloading `list` failed. Lists downloaded before stay in use.
    Failed {
        list: &'static str,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListStatus {
    pub state: ListState,
    /// When the lists in use were downloaded.
    pub downloaded_at: Option<SystemTime>,
}

type Fetch = dyn Fn(&FilterList) -> Result<String, DownloadError> + Send;
type Notify = dyn Fn(&ListStatus) + Send;

#[derive(Default)]
struct Signals {
    update: bool,
    stop: bool,
}

struct Shared {
    status: Mutex<ListStatus>,
    signals: Mutex<Signals>,
    wake: Condvar,
}

/// Keeps the filter lists of a [`Blocker`] current on a background thread: the prepared engine
/// is loaded at once, and lists older than four days are downloaded again.
pub struct Updater {
    shared: Arc<Shared>,
}

impl Updater {
    /// Starts updating the lists kept in `directory`. The prepared engine is loaded before this
    /// returns, it takes a few milliseconds, so blocking works from the first page on. `notify`
    /// is called whenever the status changes, from the background thread except for the first
    /// call.
    pub fn start(
        blocker: Arc<Blocker>,
        directory: PathBuf,
        notify: impl Fn(&ListStatus) + Send + 'static,
    ) -> Self {
        Self::start_lists(blocker, directory, FILTER_LISTS, notify)
    }

    /// The same for a set of lists of its own, which keeps its lists and its prepared engine in
    /// `directory` and feeds its own blocker, see [`crate::COOKIE_LISTS`].
    pub fn start_lists(
        blocker: Arc<Blocker>,
        directory: PathBuf,
        lists: &'static [FilterList],
        notify: impl Fn(&ListStatus) + Send + 'static,
    ) -> Self {
        Self::start_with(
            blocker,
            directory,
            lists,
            Box::new(download::fetch),
            Box::new(notify),
        )
    }

    fn start_with(
        blocker: Arc<Blocker>,
        directory: PathBuf,
        lists: &'static [FilterList],
        fetch: Box<Fetch>,
        notify: Box<Notify>,
    ) -> Self {
        let shared = Arc::new(Shared {
            status: Mutex::new(ListStatus {
                state: ListState::Loading,
                downloaded_at: None,
            }),
            signals: Mutex::default(),
            wake: Condvar::new(),
        });
        let worker = Worker {
            shared: shared.clone(),
            blocker,
            directory: ListDirectory::new(directory),
            lists,
            fetch,
            notify,
        };
        worker.load_prepared();
        let spawned = thread::Builder::new()
            .name("adblock-lists".into())
            .spawn(move || worker.run());
        if let Err(error) = spawned {
            tracing::error!(%error, "filter list updater could not be started");
        }
        Self { shared }
    }

    /// Downloads the lists now, whatever their age.
    pub fn update_now(&self) {
        if let Ok(mut signals) = self.shared.signals.lock() {
            signals.update = true;
        }
        self.shared.wake.notify_all();
    }

    pub fn status(&self) -> ListStatus {
        self.shared
            .status
            .lock()
            .map(|status| *status)
            .unwrap_or(ListStatus {
                state: ListState::Loading,
                downloaded_at: None,
            })
    }
}

impl Drop for Updater {
    fn drop(&mut self) {
        if let Ok(mut signals) = self.shared.signals.lock() {
            signals.stop = true;
        }
        self.shared.wake.notify_all();
    }
}

struct Worker {
    shared: Arc<Shared>,
    blocker: Arc<Blocker>,
    directory: ListDirectory,
    lists: &'static [FilterList],
    fetch: Box<Fetch>,
    notify: Box<Notify>,
}

impl Worker {
    fn run(self) {
        if !self.blocker.is_ready() {
            self.rebuild_from_lists();
        }
        loop {
            let requested = self
                .shared
                .signals
                .lock()
                .map(|mut signals| std::mem::take(&mut signals.update))
                .unwrap_or(false);
            let stale = self
                .directory
                .downloaded_at(self.lists)
                .is_none_or(|at| at.elapsed().unwrap_or_default() >= MAX_AGE);
            if requested || stale {
                self.update();
            }
            if !self.wait(CHECK_INTERVAL) {
                return;
            }
        }
    }

    /// Uses the prepared engine if the lists were downloaded before.
    fn load_prepared(&self) {
        let started = Instant::now();
        let Some(downloaded_at) = self.directory.downloaded_at(self.lists) else {
            return;
        };
        if let Some(engine) = self.directory.read_engine() {
            self.blocker.replace_engine(engine);
            self.set_status(ListState::Ready, Some(downloaded_at));
            tracing::info!(elapsed = ?started.elapsed(), "filter lists loaded");
        }
    }

    /// Builds the engine from the downloaded lists if the prepared one is missing or was written
    /// by another version.
    fn rebuild_from_lists(&self) {
        let Some(downloaded_at) = self.directory.downloaded_at(self.lists) else {
            return;
        };
        let Ok(texts) = self.directory.read_lists(self.lists) else {
            return;
        };
        let engine = build_engine(texts);
        if let Err(error) = self.directory.write_engine(&engine) {
            tracing::warn!(%error, "prepared filter engine could not be stored");
        }
        self.blocker.replace_engine(engine);
        self.set_status(ListState::Ready, Some(downloaded_at));
        tracing::info!("filter lists rebuilt");
    }

    fn update(&self) {
        let previous = self.status();
        let working = if self.blocker.is_ready() {
            ListState::Updating
        } else {
            ListState::Loading
        };
        self.set_status(working, previous.downloaded_at);
        let mut texts = Vec::with_capacity(self.lists.len());
        for list in self.lists {
            match (self.fetch)(list) {
                Ok(text) => texts.push(text),
                Err(error) => {
                    tracing::warn!(%error, "filter lists could not be updated");
                    self.set_status(
                        ListState::Failed { list: error.list() },
                        previous.downloaded_at,
                    );
                    return;
                }
            }
        }
        for (list, text) in self.lists.iter().zip(&texts) {
            if let Err(error) = self.directory.write_list(list, text) {
                tracing::warn!(%error, list = list.name, "filter list could not be stored");
            }
        }
        let engine = build_engine(texts);
        if let Err(error) = self.directory.write_engine(&engine) {
            tracing::warn!(%error, "prepared filter engine could not be stored");
        }
        self.blocker.replace_engine(engine);
        let downloaded_at = self
            .directory
            .downloaded_at(self.lists)
            .or_else(|| Some(SystemTime::now()));
        self.set_status(ListState::Ready, downloaded_at);
        tracing::info!("filter lists updated");
    }

    /// Waits for `timeout` or a signal. Returns `false` once the updater is dropped.
    fn wait(&self, timeout: Duration) -> bool {
        let Ok(signals) = self.shared.signals.lock() else {
            return false;
        };
        let Ok((signals, _)) = self
            .shared
            .wake
            .wait_timeout_while(signals, timeout, |signals| !signals.update && !signals.stop)
        else {
            return false;
        };
        !signals.stop
    }

    fn status(&self) -> ListStatus {
        self.shared
            .status
            .lock()
            .map(|status| *status)
            .unwrap_or(ListStatus {
                state: ListState::Loading,
                downloaded_at: None,
            })
    }

    fn set_status(&self, state: ListState, downloaded_at: Option<SystemTime>) {
        let status = ListStatus {
            state,
            downloaded_at,
        };
        if let Ok(mut current) = self.shared.status.lock() {
            *current = status;
        }
        (self.notify)(&status);
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    const LISTS: &[FilterList] = &[FILTER_LISTS[0], FILTER_LISTS[1]];
    const LIST: &str = "[Adblock Plus 2.0]\n! Title: Test\n||ads.example.net^\n";

    fn scratch(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("atoll-updater-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        path
    }

    fn wait_for(updater: &Updater, state: ListState) {
        let started = Instant::now();
        while updater.status().state != state {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "still {:?}",
                updater.status()
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn blocks_ads(blocker: &Blocker) -> bool {
        blocker.blocks(
            "https://ads.example.net/ad.js",
            "https://example.org/",
            "https://example.org/",
            "script",
        )
    }

    fn start(path: &Path, fetch: Box<Fetch>) -> (Updater, Arc<Blocker>) {
        let blocker = Arc::new(Blocker::new(true, []));
        let updater = Updater::start_with(
            blocker.clone(),
            path.to_path_buf(),
            LISTS,
            fetch,
            Box::new(|_| {}),
        );
        (updater, blocker)
    }

    #[test]
    fn lists_are_downloaded_once_and_then_loaded_from_disk() {
        let path = scratch("download");
        let downloads = Arc::new(AtomicUsize::new(0));
        let counter = downloads.clone();
        let (updater, blocker) = start(
            &path,
            Box::new(move |_| {
                counter.fetch_add(1, Ordering::SeqCst);
                Ok(LIST.to_owned())
            }),
        );
        wait_for(&updater, ListState::Ready);
        assert!(blocks_ads(&blocker));
        assert_eq!(downloads.load(Ordering::SeqCst), LISTS.len());
        assert!(updater.status().downloaded_at.is_some());
        drop(updater);

        let (restarted, blocker) = start(
            &path,
            Box::new(|list| Err(DownloadError::NotAList { list: list.name })),
        );
        wait_for(&restarted, ListState::Ready);
        assert!(blocks_ads(&blocker));
        drop(restarted);
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn a_missing_prepared_engine_is_rebuilt_from_the_lists() {
        let path = scratch("rebuild");
        let directory = ListDirectory::new(path.clone());
        for list in LISTS {
            directory.write_list(list, LIST).unwrap();
        }
        let (updater, blocker) = start(
            &path,
            Box::new(|list| Err(DownloadError::NotAList { list: list.name })),
        );
        wait_for(&updater, ListState::Ready);
        assert!(blocks_ads(&blocker));
        assert!(directory.read_engine().is_some());
        drop(updater);
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn failed_downloads_name_the_list_and_keep_blocking_off() {
        let path = scratch("failed");
        let (updater, blocker) = start(
            &path,
            Box::new(|list| Err(DownloadError::NotAList { list: list.name })),
        );
        wait_for(
            &updater,
            ListState::Failed {
                list: LISTS[0].name,
            },
        );
        assert!(!blocker.is_ready());
        assert!(!blocks_ads(&blocker));
        drop(updater);
        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn an_update_can_be_requested_at_any_time() {
        let path = scratch("requested");
        let downloads = Arc::new(AtomicUsize::new(0));
        let counter = downloads.clone();
        let (updater, _) = start(
            &path,
            Box::new(move |_| {
                counter.fetch_add(1, Ordering::SeqCst);
                Ok(LIST.to_owned())
            }),
        );
        wait_for(&updater, ListState::Ready);
        updater.update_now();
        let started = Instant::now();
        while downloads.load(Ordering::SeqCst) < 2 * LISTS.len() {
            assert!(started.elapsed() < Duration::from_secs(10));
            thread::sleep(Duration::from_millis(10));
        }
        wait_for(&updater, ListState::Ready);
        drop(updater);
        fs::remove_dir_all(path).unwrap();
    }
}
