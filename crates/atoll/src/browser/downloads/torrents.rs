//! Torrents in the download list.
//!
//! A torrent only ever runs while a way out stands that hides where the user is: either Atoll's
//! own VPN, through whose bridge the peers are reached, or a VPN of the system, to whose device
//! every socket of the torrent is tied. Without one of the two nothing starts, and when the way
//! goes away the session goes with it - that is the same killswitch the rest of the browser has,
//! applied to the one kind of traffic that talks to strangers all day.

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

/// What the torrents of this run are doing, next to the download list.
#[derive(Default)]
pub struct TorrentSession {
    /// The session, while a way out stands.
    running: Option<Torrents>,
    /// Which download row belongs to which torrent.
    rows: Vec<(TorrentId, DownloadId)>,
    /// What was asked for while no way out stood.
    waiting: Vec<(Source, String)>,
}

impl Browser {
    /// The way out torrents are to take, as far as it stands right now.
    fn torrent_route(&self) -> Result<Route, String> {
        let preferences = self.preferences.borrow();
        let chosen = preferences.torrent_route.clone();
        let device = preferences.torrent_device.clone();
        drop(preferences);
        if chosen == THROUGH_DEVICE {
            if device.trim().is_empty() {
                return Err(t!("Wähle zuerst das Netzwerkgerät deines System-VPN").to_owned());
            }
            if !atoll_torrent::devices()
                .iter()
                .any(|known| known.name == device && known.up)
            {
                return Err(t!(
                    "Das Gerät {} ist nicht da, Torrents bleiben stehen",
                    device
                ));
            }
            return Ok(Route::Device(device));
        }
        if !self.vpn.is_wanted() {
            return Err(t!(
                "Torrents laufen nur durch ein VPN. Schalte es ein oder wähle dein System-VPN."
            )
            .to_owned());
        }
        match self
            .vpn
            .status()
            .port()
            .filter(|_| self.vpn.status().is_up())
        {
            Some(port) => Ok(Route::Bridge(port)),
            None => Err(t!("Das VPN steht noch nicht, Torrents warten darauf").to_owned()),
        }
    }

    /// Takes a magnet address or the bytes of a torrent file and loads it.
    pub(in crate::browser) fn add_torrent(&self, source: Source, name: String) {
        match self.torrent_route() {
            Ok(route) => {
                self.keep_torrent_session(&route);
                let directory = self.download_directory();
                let started = {
                    let session = self.torrents.borrow_mut();
                    session
                        .running
                        .as_ref()
                        .map(|torrents| torrents.add(source.clone(), directory))
                };
                match started {
                    Some(id) => {
                        self.note_torrent(id, &name, &source);
                        self.show_toast(&t!("{} wird geladen", name));
                        self.open_downloads();
                    }
                    None => self.torrents.borrow_mut().waiting.push((source, name)),
                }
            }
            Err(trouble) => {
                self.torrents.borrow_mut().waiting.push((source, name));
                self.show_toast(&trouble);
            }
        }
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
        self.torrents.borrow_mut().rows.push((id, row));
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
        match self.torrent_route() {
            Ok(route) => {
                self.keep_torrent_session(&route);
                let waiting: Vec<(Source, String)> =
                    std::mem::take(&mut self.torrents.borrow_mut().waiting);
                for (source, name) in waiting {
                    self.add_torrent(source, name);
                }
                self.show_torrent_progress();
            }
            Err(trouble) => {
                let stopped = self.torrents.borrow_mut().running.take().is_some();
                if stopped {
                    self.stop_torrent_rows();
                    self.show_toast(&trouble);
                }
            }
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
            .rows
            .iter()
            .find(|(known, _)| *known == torrent)
            .map(|(_, row)| *row)
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

    /// Builds the session again after something it was built from changed. What was running
    /// stops, because the new session knows nothing of it.
    fn rebuild_torrent_session(&self) {
        let had_torrents = self.torrents.borrow().running.is_some();
        self.torrents.borrow_mut().running = None;
        if had_torrents {
            self.stop_torrent_rows();
        }
        self.watch_torrents();
        self.for_all_windows(Browser::show_torrent_settings);
    }

    /// The way out went away: every torrent row says so, and nothing keeps loading.
    fn stop_torrent_rows(&self) {
        let rows: Vec<DownloadId> = std::mem::take(&mut self.torrents.borrow_mut().rows)
            .into_iter()
            .map(|(_, row)| row)
            .collect();
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

    /// Fills the torrent part of the settings page.
    pub(in crate::browser) fn show_torrent_settings(&self) {
        use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

        let Some(window) = self.window() else { return };
        let state = window.global::<crate::ui::SettingsState>();
        let preferences = self.preferences.borrow();
        state.set_torrent_route(i32::from(preferences.torrent_route == THROUGH_DEVICE));
        state.set_torrent_upload(preferences.torrent_upload);
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
            match self.torrent_route() {
                Ok(Route::Bridge(_)) => t!(
                    "Läuft durch Atolls VPN. Ohne DHT und ohne UDP-Tracker, darum oft wenige Peers."
                )
                .to_owned(),
                Ok(Route::Device(device)) => {
                    t!("Läuft über {}. Alles von BitTorrent ist möglich.", device)
                }
                Err(trouble) => trouble,
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
