//! Torrents in the download list.
//!
//! A torrent only ever runs while a way out stands that hides where the user is: either Atoll's
//! own VPN, through whose bridge the peers are reached, or a VPN of the system, to whose device
//! every socket of the torrent is tied. Without one of the two nothing starts, and when the way
//! goes away the session goes with it - that is the same killswitch the rest of the browser has,
//! applied to the one kind of traffic that talks to strangers all day.

use std::path::PathBuf;
use std::time::Duration;

use atoll_core::{Download, DownloadId, DownloadKind, DownloadStatus};
use atoll_storage::prelude::*;
use atoll_storage::{DownloadProgress, NewDownload};
use atoll_torrent::{Route, Source, State, TorrentId, Torrents};

use super::{Control, Transfer};
use crate::browser::Browser;
use atoll_i18n::t;

/// The name the settings use for each way out.
pub(in crate::browser) const THROUGH_VPN: &str = "vpn";
pub(in crate::browser) const THROUGH_DEVICE: &str = "device";

/// What the settings do with a torrent file that was downloaded.
const ASK: &str = "ask";
const LOAD: &str = "load";

/// What the torrents of this run are doing, next to the download list.
#[derive(Default)]
pub struct TorrentSession {
    /// The session, while a way out stands.
    running: Option<Torrents>,
    /// Everything Atoll is loading, with where it came from, so a way that comes back can take
    /// it on again without the user doing anything.
    entries: Vec<Entry>,
    /// What was asked for while no way out stood.
    waiting: Vec<(Source, String)>,
}

/// One torrent, as the browser keeps it.
struct Entry {
    /// Its number in the running session, as long as one carries it.
    torrent: Option<TorrentId>,
    row: DownloadId,
    source: Source,
}

/// What a torrent can do right now.
enum Way {
    /// A way out stands.
    Ready(Route),
    /// One is set up but not there at the moment, so the torrent waits for it.
    Waiting(String),
    /// None is set up. Only the user can decide to go without one.
    Nothing(String),
}

impl Browser {
    /// The way out torrents are to take, and what is in the way if there is none.
    fn torrent_way(&self) -> Way {
        let preferences = self.preferences.borrow();
        let chosen = preferences.torrent_route.clone();
        let device = preferences.torrent_device.clone();
        drop(preferences);
        if chosen == THROUGH_DEVICE {
            if device.trim().is_empty() {
                return Way::Nothing(
                    t!("Wähle zuerst das Netzwerkgerät deines System-VPN").to_owned(),
                );
            }
            if !atoll_torrent::devices()
                .iter()
                .any(|known| known.name == device && known.up)
            {
                return Way::Waiting(t!(
                    "Das Gerät {} ist nicht da, Torrents bleiben stehen",
                    device
                ));
            }
            return Way::Ready(Route::Device(device));
        }
        if !self.vpn.is_wanted() {
            return Way::Nothing(
                t!("Torrents laufen nur durch ein VPN. Schalte es ein oder wähle dein System-VPN.")
                    .to_owned(),
            );
        }
        match self
            .vpn
            .status()
            .port()
            .filter(|_| self.vpn.status().is_up())
        {
            Some(port) => Way::Ready(Route::Bridge(port)),
            None => Way::Waiting(t!("Das VPN steht noch nicht, Torrents warten darauf").to_owned()),
        }
    }

    /// Takes a magnet address or the bytes of a torrent file and loads it.
    pub(in crate::browser) fn add_torrent(&self, source: Source, name: String) {
        match self.torrent_way() {
            Way::Ready(route) => self.start_torrent(source, name, route),
            Way::Waiting(trouble) => {
                self.torrents.borrow_mut().waiting.push((source, name));
                self.show_toast(&trouble);
            }
            // Nothing hides the user, so only they can decide to be seen.
            Way::Nothing(trouble) => self.ask_about_loading_openly(source, name, trouble),
        }
    }

    /// Asks whether this one torrent may go out the ordinary way. Nothing is remembered: the
    /// next torrent asks again, because only the user knows what it is.
    fn ask_about_loading_openly(&self, source: Source, name: String, trouble: String) {
        let this = self.this();
        self.confirm(
            t!("Kein VPN für Torrents"),
            &t!(
                "{} Lädst du {} trotzdem, sehen der Schwarm und dein Anbieter, dass du es lädst, und von wo.",
                trouble,
                name
            ),
            t!("Ohne Schutz laden"),
            move || {
                if let Some(browser) = this.upgrade() {
                    browser.start_torrent(source, name, Route::Direct);
                }
            },
        );
    }

    /// Starts one torrent on a way that is there.
    fn start_torrent(&self, source: Source, name: String, route: Route) {
        self.keep_torrent_session(&route);
        let directory = self.download_directory();
        let started = {
            let session = self.torrents.borrow();
            session
                .running
                .as_ref()
                .map(|torrents| torrents.add(source.clone(), directory))
        };
        let Some(id) = started else {
            self.torrents.borrow_mut().waiting.push((source, name));
            return;
        };
        self.note_torrent(id, &name, &source);
        self.show_toast(&if route.hides_the_user() {
            t!("{} wird geladen", name)
        } else {
            t!("{} wird ohne Schutz geladen", name)
        });
        self.open_downloads();
    }

    /// Writes the torrent into the download list, so it is there like everything else that loads.
    fn note_torrent(&self, id: TorrentId, name: &str, source: &Source) {
        let url = match source {
            Source::Link(link) => link.clone(),
            Source::File(_) => name.to_owned(),
        };
        let now = Self::now();
        let created = self.storage.create_download(NewDownload {
            url: &url,
            path: None,
            file_name: name,
            total_bytes: 0,
            started_at: now,
            kind: DownloadKind::Torrent,
        });
        let row = match created {
            Ok(row) => row,
            Err(error) => {
                self.report(t!("Torrent anlegen"), Err::<(), _>(error));
                return;
            }
        };
        self.torrents.borrow_mut().entries.push(Entry {
            torrent: Some(id),
            row,
            source: source.clone(),
        });
        let mut downloads = self.downloads.borrow_mut();
        downloads.entries.insert(
            0,
            Download {
                id: row,
                url,
                path: None,
                file_name: name.to_owned(),
                received_bytes: 0,
                total_bytes: 0,
                status: DownloadStatus::Running,
                started_at: now,
                finished_at: None,
                kind: DownloadKind::Torrent,
            },
        );
        downloads.transfers.insert(
            row,
            Transfer {
                control: Control::Torrent(id),
                speed: 0,
                paused: false,
            },
        );
    }

    /// Builds the session for `route`, unless one is already running that way.
    fn keep_torrent_session(&self, route: &Route) {
        let standing = self
            .torrents
            .borrow()
            .running
            .as_ref()
            .is_some_and(|torrents| torrents.route() == route);
        if standing {
            return;
        }
        let upload = self.preferences.borrow().torrent_upload;
        let torrents = Torrents::start(
            route.clone(),
            self.download_directory(),
            self.paths.torrents.clone(),
            upload,
        );
        self.torrents.borrow_mut().running = Some(torrents);
        tracing::info!(?route, "the torrents have a way out");
    }

    /// Looks after the torrents, called whenever the browser is awake anyway.
    pub(in crate::browser) fn watch_torrents(&self) {
        let has_work = {
            let session = self.torrents.borrow();
            session.running.is_some() || !session.waiting.is_empty()
        };
        if !has_work {
            return;
        }
        match self.torrent_way() {
            Way::Ready(route) => {
                let changed = self
                    .torrents
                    .borrow()
                    .running
                    .as_ref()
                    .is_some_and(|torrents| *torrents.route() != route);
                if changed {
                    // The way out changed under them, so they travel the new one from now on.
                    self.rebuild_torrent_session();
                    return;
                }
                self.keep_torrent_session(&route);
                self.take_on_what_waited();
                self.show_torrent_progress();
            }
            // A way that is only away for a moment: nothing runs meanwhile, and what was
            // loading is taken on again once it is back.
            Way::Waiting(trouble) | Way::Nothing(trouble) => self.hold_torrents(&trouble),
        }
    }

    /// Starts what was asked for while nothing could carry it.
    fn take_on_what_waited(&self) {
        let waiting: Vec<(Source, String)> =
            std::mem::take(&mut self.torrents.borrow_mut().waiting);
        for (source, name) in waiting {
            self.add_torrent(source, name);
        }
        // What was loading before the way broke off goes on now.
        let resting: Vec<(usize, Source)> = self
            .torrents
            .borrow()
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.torrent.is_none())
            .map(|(at, entry)| (at, entry.source.clone()))
            .collect();
        if resting.is_empty() {
            return;
        }
        let directory = self.download_directory();
        for (at, source) in resting {
            let started = {
                let session = self.torrents.borrow();
                session
                    .running
                    .as_ref()
                    .map(|torrents| torrents.add(source, directory.clone()))
            };
            let Some(id) = started else { continue };
            let mut session = self.torrents.borrow_mut();
            let Some(entry) = session.entries.get_mut(at) else {
                continue;
            };
            entry.torrent = Some(id);
            let row = entry.row;
            drop(session);
            let mut downloads = self.downloads.borrow_mut();
            downloads.transfers.insert(
                row,
                Transfer {
                    control: Control::Torrent(id),
                    speed: 0,
                    paused: false,
                },
            );
            if let Some(entry) = downloads.entries.iter_mut().find(|entry| entry.id == row) {
                entry.status = DownloadStatus::Running;
                entry.finished_at = None;
            }
        }
        self.refresh_download_list();
    }

    /// Holds everything while no way out stands. The torrents are kept, only their session is
    /// given up, so nothing of them can travel unprotected.
    fn hold_torrents(&self, trouble: &str) {
        let unprotected = self
            .torrents
            .borrow()
            .running
            .as_ref()
            .is_some_and(|torrents| !torrents.route().hides_the_user());
        if unprotected {
            // The user said this one may go out openly, so nothing is taken from them.
            self.show_torrent_progress();
            return;
        }
        let stopped = self.torrents.borrow_mut().running.take().is_some();
        if stopped {
            self.stop_torrent_rows();
            self.show_toast(trouble);
        }
    }

    /// Takes what the torrents report into the download list.
    fn show_torrent_progress(&self) {
        let (states, trouble) = {
            let session = self.torrents.borrow();
            let Some(torrents) = session.running.as_ref() else {
                return;
            };
            (torrents.states(), torrents.take_trouble())
        };
        if let Some(trouble) = trouble {
            self.show_toast(&trouble);
        }
        let mut changed = false;
        for state in states {
            let Some(id) = self.row_of(state.id) else {
                continue;
            };
            changed |= self.apply_torrent_state(id, &state);
        }
        if changed {
            self.refresh_download_list();
        }
    }

    fn row_of(&self, torrent: TorrentId) -> Option<DownloadId> {
        self.torrents
            .borrow()
            .entries
            .iter()
            .find(|entry| entry.torrent == Some(torrent))
            .map(|entry| entry.row)
    }

    /// Writes what one torrent says into its row. Answers whether the list has to be redrawn.
    fn apply_torrent_state(&self, id: DownloadId, state: &State) -> bool {
        let status = if state.error.is_some() {
            DownloadStatus::Interrupted
        } else if state.finished {
            DownloadStatus::Complete
        } else {
            DownloadStatus::Running
        };
        let path = state
            .folder
            .as_ref()
            .map(|folder| folder.join(&state.name).display().to_string());
        let (finished_now, entry) = {
            let mut downloads = self.downloads.borrow_mut();
            if state.finished || state.error.is_some() {
                downloads.transfers.remove(&id);
            } else if let Some(transfer) = downloads.transfers.get_mut(&id) {
                transfer.speed = state.speed as i64;
                transfer.paused = state.paused;
            }
            let Some(entry) = downloads.entries.iter_mut().find(|entry| entry.id == id) else {
                return false;
            };
            let was_running = entry.status == DownloadStatus::Running;
            if !state.name.is_empty() {
                entry.file_name = state.name.clone();
            }
            entry.received_bytes = state.done_bytes as i64;
            entry.total_bytes = state.total_bytes as i64;
            entry.path = path;
            entry.status = status;
            if status != DownloadStatus::Running && was_running {
                entry.finished_at = Some(Self::now());
            }
            (
                status != DownloadStatus::Running && was_running,
                entry.clone(),
            )
        };
        if finished_now {
            let stored = self.storage.update_download(
                id,
                DownloadProgress {
                    path: entry.path.as_deref(),
                    file_name: &entry.file_name,
                    received_bytes: entry.received_bytes,
                    total_bytes: entry.total_bytes,
                    status: entry.status,
                    finished_at: entry.finished_at,
                },
            );
            self.report(t!("Torrent merken"), stored);
            if entry.status == DownloadStatus::Complete {
                self.show_toast(&t!("{} ist fertig", entry.file_name));
            }
        }
        true
    }

    /// Builds the session again after something it was built from changed, and hands it what
    /// was loading: the files are there, so the torrents go on where they stopped.
    fn rebuild_torrent_session(&self) {
        {
            let mut session = self.torrents.borrow_mut();
            session.running = None;
            for entry in &mut session.entries {
                entry.torrent = None;
            }
        }
        {
            let mut downloads = self.downloads.borrow_mut();
            let rows: Vec<DownloadId> = self
                .torrents
                .borrow()
                .entries
                .iter()
                .map(|entry| entry.row)
                .collect();
            for row in rows {
                downloads.transfers.remove(&row);
            }
        }
        if let Way::Ready(route) = self.torrent_way() {
            self.keep_torrent_session(&route);
            self.take_on_what_waited();
        }
        self.for_all_windows(Browser::show_torrent_settings);
        self.refresh_download_list();
    }

    /// The way out went away: every torrent row says so, and nothing keeps loading. What they
    /// are stays known, so they go on once a way is back.
    fn stop_torrent_rows(&self) {
        let rows: Vec<DownloadId> = {
            let mut session = self.torrents.borrow_mut();
            for entry in &mut session.entries {
                entry.torrent = None;
            }
            session.entries.iter().map(|entry| entry.row).collect()
        };
        let mut downloads = self.downloads.borrow_mut();
        for row in rows {
            downloads.transfers.remove(&row);
            if let Some(entry) = downloads.entries.iter_mut().find(|entry| entry.id == row)
                && entry.status == DownloadStatus::Running
            {
                entry.status = DownloadStatus::Interrupted;
            }
        }
        drop(downloads);
        self.refresh_download_list();
    }

    /// What the download list does with a torrent row.
    pub(super) fn torrent_action(&self, torrent: TorrentId, action: &str) {
        let session = self.torrents.borrow();
        let Some(torrents) = session.running.as_ref() else {
            return;
        };
        match action {
            "pause" => torrents.pause(torrent),
            "resume" => torrents.resume(torrent),
            "cancel" | "remove" => torrents.forget(torrent, false),
            _ => {}
        }
    }

    /// A magnet address or the address of a torrent file, from the address bar, a link or the
    /// palette. A torrent file is fetched the same way the torrent itself will travel.
    pub fn open_torrent(&self, address: &str) {
        let address = address.trim().to_owned();
        if is_magnet(&address) {
            let name = magnet_name(&address);
            self.add_torrent(Source::Link(address), name);
            return;
        }
        let name = file_name_of(&address);
        self.show_toast(&t!("{} wird geholt", name));
        let this = self.this();
        let wanted = address.clone();
        crate::platform::tasks::run_in_background(
            "torrent-file",
            move || fetch_torrent_file(&wanted),
            move |fetched| {
                let Some(browser) = this.upgrade() else {
                    return;
                };
                match fetched {
                    Ok(bytes) => browser.add_torrent(Source::File(bytes), name),
                    Err(trouble) => browser.show_toast(&trouble),
                }
            },
        );
    }

    /// A file was downloaded. If it is a torrent file, what happens now is the user's choice:
    /// ask, load it at once, or leave it lying there like any other file.
    pub(super) fn torrent_file_downloaded(&self, path: Option<&str>, file_name: &str) {
        if !file_name.to_ascii_lowercase().ends_with(".torrent") {
            return;
        }
        let Some(path) = path.map(PathBuf::from).filter(|path| path.exists()) else {
            return;
        };
        let name = file_name
            .strip_suffix(".torrent")
            .unwrap_or(file_name)
            .to_owned();
        match self.preferences.borrow().torrent_files.as_str() {
            LOAD => self.load_torrent_file(path, name),
            ASK => {
                let this = self.this();
                self.offer(
                    t!("Torrent gefunden"),
                    &t!(
                        "{} ist eine Torrent-Datei. Soll Atoll laden, was darin steht?",
                        name
                    ),
                    t!("Torrent laden"),
                    move || {
                        if let Some(browser) = this.upgrade() {
                            browser.load_torrent_file(path, name);
                        }
                    },
                );
            }
            _ => {}
        }
    }

    /// Reads a torrent file that lies on the disk and takes it on.
    pub(in crate::browser) fn load_torrent_file(&self, path: PathBuf, name: String) {
        match std::fs::read(&path) {
            Ok(bytes) => self.add_torrent(Source::File(bytes), name),
            Err(error) => self.show_toast(&t!("Die Torrent-Datei kam nicht an: {}", error)),
        }
    }

    /// Fills the torrent part of the settings page.
    pub(in crate::browser) fn show_torrent_settings(&self) {
        use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

        let Some(window) = self.window() else { return };
        let state = window.global::<crate::ui::SettingsState>();
        let preferences = self.preferences.borrow();
        state.set_torrent_route(i32::from(preferences.torrent_route == THROUGH_DEVICE));
        state.set_torrent_upload(preferences.torrent_upload);
        state.set_torrent_files(match preferences.torrent_files.as_str() {
            LOAD => 1,
            ASK => 0,
            _ => 2,
        });
        let chosen = preferences.torrent_device.clone();
        drop(preferences);
        let devices = atoll_torrent::devices();
        let names: Vec<SharedString> = std::iter::once(SharedString::from(t!("Gerät wählen")))
            .chain(devices.iter().map(|device| {
                SharedString::from(if device.up {
                    device.name.clone()
                } else {
                    t!("{} (nicht da)", device.name)
                })
            }))
            .collect();
        state.set_torrent_devices(ModelRc::from(std::rc::Rc::new(VecModel::from(names))));
        state.set_torrent_device(
            devices
                .iter()
                .position(|device| device.name == chosen)
                .map_or(0, |at| at as i32 + 1),
        );
        state.set_torrent_status(
            match self.torrent_way() {
                Way::Ready(Route::Bridge(_)) => t!(
                    "Läuft durch Atolls VPN. Ohne DHT und ohne UDP-Tracker, darum oft wenige Peers."
                )
                .to_owned(),
                Way::Ready(Route::Device(device)) => {
                    t!("Läuft über {}. Alles von BitTorrent ist möglich.", device)
                }
                Way::Ready(Route::Direct) => {
                    t!("Läuft ohne Schutz, weil du es für diesen Torrent erlaubt hast.").to_owned()
                }
                Way::Waiting(trouble) | Way::Nothing(trouble) => trouble,
            }
            .into(),
        );
    }

    /// Settings: picks the device of the system VPN.
    pub fn choose_torrent_device(&self, index: i32) {
        let devices = atoll_torrent::devices();
        let chosen = usize::try_from(index)
            .ok()
            .and_then(|index| index.checked_sub(1))
            .and_then(|index| devices.get(index))
            .map(|device| device.name.clone())
            .unwrap_or_default();
        self.update_preference(
            crate::browser::preferences::PreferenceKey::TorrentDevice,
            |preferences| preferences.torrent_device = chosen,
        );
        self.rebuild_torrent_session();
    }

    /// Settings: chooses which way out the torrents take.
    pub(in crate::browser) fn set_torrent_route(&self, through_device: bool) {
        let route = if through_device {
            THROUGH_DEVICE
        } else {
            THROUGH_VPN
        };
        self.update_preference(
            crate::browser::preferences::PreferenceKey::TorrentRoute,
            |preferences| preferences.torrent_route = route.to_owned(),
        );
        self.rebuild_torrent_session();
    }

    /// Settings: what happens with a torrent file that was downloaded.
    pub(in crate::browser) fn set_torrent_files(&self, choice: i32) {
        let choice = match choice {
            1 => LOAD,
            2 => "keep",
            _ => ASK,
        };
        self.update_preference(
            crate::browser::preferences::PreferenceKey::TorrentFiles,
            |preferences| preferences.torrent_files = choice.to_owned(),
        );
        self.for_all_windows(Browser::show_torrent_settings);
    }

    /// Settings: switches giving pieces back to the swarm on or off.
    pub(in crate::browser) fn set_torrent_upload(&self, upload: bool) {
        self.update_preference(
            crate::browser::preferences::PreferenceKey::TorrentUpload,
            |preferences| {
                preferences.torrent_upload = upload;
            },
        );
        // The session only reads this when it is built, so it is built again.
        self.rebuild_torrent_session();
    }
}

/// Fetches a torrent file the same way the torrent itself will travel.
fn fetch_torrent_file(address: &str) -> Result<Vec<u8>, String> {
    let mut answer = atoll_net::agent(Duration::from_secs(30))
        .get(address)
        .call()
        .map_err(|error| t!("Die Torrent-Datei kam nicht an: {}", error))?;
    answer
        .body_mut()
        .with_config()
        .limit(16 * 1024 * 1024)
        .read_to_vec()
        .map_err(|error| t!("Die Torrent-Datei kam nicht an: {}", error))
}

/// Whether an address is a magnet link.
pub fn is_magnet(address: &str) -> bool {
    address.trim_start().starts_with("magnet:?")
}

/// The name a magnet address carries, for the download list.
pub(in crate::browser) fn magnet_name(address: &str) -> String {
    let after = address.split("dn=").nth(1).unwrap_or_default();
    let raw = after.split('&').next().unwrap_or_default();
    let name = percent_decoded(raw).replace('+', " ");
    if name.trim().is_empty() {
        t!("Torrent").to_owned()
    } else {
        name
    }
}

/// Reads back what an address wrote out.
fn percent_decoded(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%'
            && at + 2 < bytes.len()
            && let Ok(byte) = u8::from_str_radix(&text[at + 1..at + 3], 16)
        {
            out.push(byte);
            at += 3;
            continue;
        }
        out.push(bytes[at]);
        at += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Where a torrent file was asked for, as the download list names it.
pub(in crate::browser) fn file_name_of(url: &str) -> String {
    url.rsplit('/')
        .next()
        .map(|name| name.split('?').next().unwrap_or(name).to_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| t!("Torrent").to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_magnet_is_told_from_an_ordinary_address() {
        assert!(is_magnet("magnet:?xt=urn:btih:abc"));
        assert!(is_magnet("  magnet:?xt=urn:btih:abc"));
        assert!(!is_magnet("https://example.org/x.torrent"));
    }

    #[test]
    fn the_name_in_a_magnet_is_read_for_the_list() {
        assert_eq!(
            magnet_name("magnet:?xt=urn:btih:abc&dn=ubuntu-24.04.5.iso&tr=https%3A%2F%2Fx"),
            "ubuntu-24.04.5.iso"
        );
        assert_eq!(
            magnet_name("magnet:?xt=urn:btih:abc&dn=mein%20film.mkv"),
            "mein film.mkv"
        );
        assert_eq!(magnet_name("magnet:?xt=urn:btih:abc"), "Torrent");
    }

    #[test]
    fn a_torrent_file_is_named_after_its_address() {
        assert_eq!(
            file_name_of("https://releases.example/ubuntu.iso.torrent?x=1"),
            "ubuntu.iso.torrent"
        );
        assert_eq!(file_name_of("https://example.org/"), "Torrent");
    }
}
