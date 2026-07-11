use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use atoll_core::address::display_host;
use atoll_core::{Download, DownloadId, DownloadStatus};
use atoll_engine::DownloadState;
use atoll_storage::prelude::*;
use atoll_storage::{DownloadProgress, NewDownload};
use slint::{ComponentHandle, ModelRc, VecModel};

use super::Browser;
use crate::platform::opener;
use crate::ui::{BrowserState, DownloadItem, DownloadItemStatus, OverlayState};

const LISTED_DOWNLOADS: usize = 50;

/// What only the engine knows about a running download.
struct Transfer {
    engine_id: u32,
    speed: i64,
    paused: bool,
}

/// The download list: stored entries plus the live state of running downloads.
#[derive(Default)]
pub struct Downloads {
    entries: Vec<Download>,
    loaded: bool,
    transfers: HashMap<DownloadId, Transfer>,
    by_engine_id: HashMap<u32, DownloadId>,
    started_this_session: bool,
}

impl Browser {
    /// Marks downloads that were running when Atoll last closed as interrupted.
    pub(super) fn restore_downloads(&self) {
        if let Err(error) = self.storage.interrupt_running_downloads(Self::now()) {
            tracing::warn!(%error, "download list could not be updated");
        }
    }

    /// The folder downloads are saved to unless the user picks another one.
    pub(super) fn download_directory(&self) -> PathBuf {
        self.preferences
            .borrow()
            .download_directory
            .clone()
            .unwrap_or_else(|| self.paths.downloads.clone())
    }

    /// Hands the download preferences to the engine.
    pub(super) fn apply_download_options(&self) {
        let ask = self.preferences.borrow().download_ask;
        if let Some(engine) = self.engine.borrow().as_ref() {
            engine.set_download_options(self.download_directory(), ask);
        }
    }

    pub(super) fn download_updated(&self, state: &DownloadState) {
        self.ensure_downloads_loaded();
        let finished = state.complete || state.canceled || state.interrupted;
        let known = self.downloads.borrow().by_engine_id.get(&state.id).copied();
        let id = match known {
            Some(id) => id,
            None if finished => return,
            None => match self.add_download(state) {
                Some(id) => id,
                None => return,
            },
        };
        let status = if state.complete {
            DownloadStatus::Complete
        } else if state.canceled {
            DownloadStatus::Canceled
        } else if state.interrupted {
            DownloadStatus::Interrupted
        } else {
            DownloadStatus::Running
        };
        let path = state
            .path
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned());
        let now = Self::now();
        let (entry, path_changed, was_running) = {
            let mut downloads = self.downloads.borrow_mut();
            if finished {
                downloads.transfers.remove(&id);
                downloads.by_engine_id.remove(&state.id);
            } else {
                downloads.transfers.insert(
                    id,
                    Transfer {
                        engine_id: state.id,
                        speed: state.speed,
                        paused: state.paused,
                    },
                );
            }
            let Some(entry) = downloads.entries.iter_mut().find(|entry| entry.id == id) else {
                return;
            };
            let path_changed = path.is_some() && entry.path != path;
            let was_running = entry.status == DownloadStatus::Running;
            if path.is_some() {
                entry.path = path.clone();
            }
            if !state.file_name.is_empty() {
                entry.file_name = state.file_name.clone();
            }
            if !finished || state.received_bytes > 0 {
                entry.received_bytes = state.received_bytes;
            }
            if state.total_bytes > 0 {
                entry.total_bytes = state.total_bytes;
            }
            entry.status = status;
            if finished {
                entry.finished_at = Some(now);
            }
            (entry.clone(), path_changed, was_running)
        };
        if finished || path_changed {
            let result = self.storage.update_download(
                id,
                DownloadProgress {
                    path: entry.path.as_deref(),
                    file_name: &entry.file_name,
                    received_bytes: entry.received_bytes,
                    total_bytes: entry.total_bytes,
                    status,
                    finished_at: entry.finished_at,
                },
            );
            if let Err(error) = result {
                tracing::warn!(%error, "download could not be stored");
            }
        }
        if finished && was_running {
            match status {
                DownloadStatus::Complete => {
                    self.show_toast(&format!("Download abgeschlossen: {}", entry.file_name))
                }
                DownloadStatus::Interrupted => {
                    self.show_toast(&format!("Download fehlgeschlagen: {}", entry.file_name))
                }
                DownloadStatus::Running | DownloadStatus::Canceled => {}
            }
        }
        self.refresh_downloads();
    }

    fn add_download(&self, state: &DownloadState) -> Option<DownloadId> {
        let path = state
            .path
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned());
        let now = Self::now();
        let created = self.storage.create_download(NewDownload {
            url: &state.url,
            path: path.as_deref(),
            file_name: &state.file_name,
            total_bytes: state.total_bytes,
            started_at: now,
        });
        let id = match created {
            Ok(id) => id,
            Err(error) => {
                self.report("Download speichern", Err::<(), _>(error));
                return None;
            }
        };
        {
            let mut downloads = self.downloads.borrow_mut();
            downloads.entries.insert(
                0,
                Download {
                    id,
                    url: state.url.clone(),
                    path,
                    file_name: state.file_name.clone(),
                    received_bytes: state.received_bytes,
                    total_bytes: state.total_bytes,
                    status: DownloadStatus::Running,
                    started_at: now,
                    finished_at: None,
                },
            );
            downloads.by_engine_id.insert(state.id, id);
            downloads.started_this_session = true;
        }
        if self.preferences.borrow().download_panel {
            self.open_downloads();
        } else {
            self.show_toast(&format!("Download gestartet: {}", state.file_name));
        }
        Some(id)
    }

    fn ensure_downloads_loaded(&self) {
        let mut downloads = self.downloads.borrow_mut();
        if !downloads.loaded {
            downloads.entries = self.storage.downloads(LISTED_DOWNLOADS).unwrap_or_default();
            downloads.loaded = true;
        }
    }

    pub fn open_downloads(&self) {
        let Some(window) = self.window() else { return };
        self.ensure_downloads_loaded();
        self.dismiss_suggestions();
        window.global::<OverlayState>().set_downloads_visible(true);
        self.refresh_downloads();
    }

    pub fn close_downloads(&self) {
        if let Some(window) = self.window() {
            window.global::<OverlayState>().set_downloads_visible(false);
        }
        self.focus_content();
    }

    /// Updates the toolbar button and, if it is open, the download list.
    fn refresh_downloads(&self) {
        let Some(window) = self.window() else { return };
        let downloads = self.downloads.borrow();
        let running: Vec<&Download> = downloads
            .entries
            .iter()
            .filter(|entry| entry.status == DownloadStatus::Running)
            .collect();
        let received: i64 = running.iter().map(|entry| entry.received_bytes).sum();
        let total: i64 = running.iter().map(|entry| entry.total_bytes).sum();
        let size_known = running.iter().all(|entry| entry.total_bytes > 0);
        let progress = if size_known && total > 0 {
            (received as f32 / total as f32).clamp(0.0, 1.0)
        } else {
            -1.0
        };
        let state = window.global::<BrowserState>();
        state.set_downloads_running(!running.is_empty());
        state.set_downloads_progress(progress);
        state.set_downloads_button_visible(downloads.started_this_session || !running.is_empty());

        let overlay = window.global::<OverlayState>();
        if !overlay.get_downloads_visible() {
            return;
        }
        let items: Vec<DownloadItem> = downloads
            .entries
            .iter()
            .map(|entry| download_item(entry, downloads.transfers.get(&entry.id)))
            .collect();
        overlay.set_download_items(ModelRc::from(Rc::new(VecModel::from(items))));
        overlay.set_downloads_clearable(
            downloads
                .entries
                .iter()
                .any(|entry| entry.status != DownloadStatus::Running),
        );
    }

    /// Buttons of an entry in the download list.
    pub fn download_action(&self, id: DownloadId, action: &str) {
        let (entry, engine_id) = {
            let downloads = self.downloads.borrow();
            let Some(entry) = downloads.entries.iter().find(|entry| entry.id == id) else {
                return;
            };
            let engine_id = downloads
                .transfers
                .get(&id)
                .map(|transfer| transfer.engine_id);
            (entry.clone(), engine_id)
        };
        match action {
            "open" => match entry.path.as_deref().map(Path::new) {
                Some(path) if path.exists() => {
                    if let Err(error) = opener::open(path) {
                        self.show_toast(&format!("Datei kann nicht geöffnet werden: {error}"));
                    }
                }
                _ => self.show_toast("Die Datei wurde verschoben oder gelöscht"),
            },
            "show" => {
                if let Some(path) = entry.path {
                    opener::show_in_folder(PathBuf::from(path));
                }
            }
            "cancel" | "pause" | "resume" => {
                if let (Some(engine_id), Some(engine)) = (engine_id, self.engine.borrow().as_ref())
                {
                    match action {
                        "cancel" => engine.cancel_download(engine_id),
                        "pause" => engine.pause_download(engine_id),
                        _ => engine.resume_download(engine_id),
                    }
                }
            }
            "retry" => self.retry_download(&entry.url),
            "remove" => {
                let result = self.storage.remove_download(id);
                self.report("Download entfernen", result);
                self.downloads
                    .borrow_mut()
                    .entries
                    .retain(|candidate| candidate.id != id);
                self.refresh_downloads();
            }
            other => tracing::warn!(action = other, "unknown download action"),
        }
    }

    pub fn clear_downloads(&self) {
        let result = self.storage.clear_finished_downloads();
        self.report("Downloads leeren", result);
        self.downloads
            .borrow_mut()
            .entries
            .retain(|entry| entry.status == DownloadStatus::Running);
        self.refresh_downloads();
    }

    pub fn open_download_folder(&self) {
        let directory = self.download_directory();
        let result = std::fs::create_dir_all(&directory).and_then(|()| opener::open(&directory));
        if let Err(error) = result {
            self.show_toast(&format!("Ordner kann nicht geöffnet werden: {error}"));
        }
    }

    fn retry_download(&self, url: &str) {
        let mut started = false;
        self.with_live_view(|view| {
            view.start_download(url);
            started = true;
        });
        if !started {
            self.open_address(url);
        }
    }
}

fn download_item(entry: &Download, transfer: Option<&Transfer>) -> DownloadItem {
    let status = item_status(entry, transfer);
    let progress = if entry.total_bytes > 0 {
        (entry.received_bytes as f32 / entry.total_bytes as f32).clamp(0.0, 1.0)
    } else {
        -1.0
    };
    DownloadItem {
        id: entry.id.0 as i32,
        file_name: entry.file_name.as_str().into(),
        detail: download_detail(entry, status, transfer.map_or(0, |transfer| transfer.speed))
            .into(),
        progress,
        status,
    }
}

fn item_status(entry: &Download, transfer: Option<&Transfer>) -> DownloadItemStatus {
    match entry.status {
        DownloadStatus::Running if transfer.is_some_and(|transfer| transfer.paused) => {
            DownloadItemStatus::Paused
        }
        DownloadStatus::Running => DownloadItemStatus::Running,
        DownloadStatus::Complete
            if entry
                .path
                .as_deref()
                .is_some_and(|path| !Path::new(path).exists()) =>
        {
            DownloadItemStatus::Missing
        }
        DownloadStatus::Complete => DownloadItemStatus::Complete,
        DownloadStatus::Canceled => DownloadItemStatus::Canceled,
        DownloadStatus::Interrupted => DownloadItemStatus::Failed,
    }
}

fn download_detail(entry: &Download, status: DownloadItemStatus, speed: i64) -> String {
    let transferred = if entry.total_bytes > 0 {
        format!(
            "{} von {}",
            format_bytes(entry.received_bytes),
            format_bytes(entry.total_bytes)
        )
    } else {
        format_bytes(entry.received_bytes)
    };
    match status {
        DownloadItemStatus::Running => {
            let mut parts = vec![transferred];
            if speed > 0 {
                parts.push(format!("{}/s", format_bytes(speed)));
                if entry.total_bytes > entry.received_bytes {
                    parts.push(remaining_time(
                        (entry.total_bytes - entry.received_bytes) / speed,
                    ));
                }
            }
            parts.join(" · ")
        }
        DownloadItemStatus::Paused => format!("Pausiert · {transferred}"),
        DownloadItemStatus::Complete => {
            let size = format_bytes(entry.total_bytes.max(entry.received_bytes));
            match display_host(&entry.url) {
                Some(host) => format!("{size} · {host}"),
                None => size,
            }
        }
        DownloadItemStatus::Missing => "Datei wurde verschoben oder gelöscht".to_owned(),
        DownloadItemStatus::Canceled => "Abgebrochen".to_owned(),
        DownloadItemStatus::Failed => "Fehlgeschlagen".to_owned(),
    }
}

/// A byte count with a binary unit and a German decimal comma, like `12,5 MB`.
fn format_bytes(bytes: i64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    let number = if value >= 100.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    };
    format!("{} {}", number.replace('.', ","), UNITS[unit])
}

fn remaining_time(seconds: i64) -> String {
    let seconds = seconds.max(1) as u64;
    match seconds {
        ..60 => format!("noch {seconds} s"),
        60..3_600 => format!("noch {} min", seconds.div_ceil(60)),
        _ => format!("noch {} h", seconds.div_ceil(3_600)),
    }
}

#[cfg(test)]
mod tests {
    use atoll_core::Timestamp;

    use super::*;

    fn entry(status: DownloadStatus, received: i64, total: i64) -> Download {
        Download {
            id: DownloadId(1),
            url: "https://www.example.org/file.zip".to_owned(),
            path: None,
            file_name: "file.zip".to_owned(),
            received_bytes: received,
            total_bytes: total,
            status,
            started_at: Timestamp(0),
            finished_at: None,
        }
    }

    #[test]
    fn sizes_use_binary_units_and_a_decimal_comma() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1536), "1,5 KB");
        assert_eq!(format_bytes(250 * 1024 * 1024), "250 MB");
        assert_eq!(format_bytes(3 * 1024 * 1024 * 1024), "3,0 GB");
    }

    #[test]
    fn running_downloads_show_progress_speed_and_time_left() {
        let download = entry(DownloadStatus::Running, 1024 * 1024, 11 * 1024 * 1024);
        assert_eq!(
            download_detail(&download, DownloadItemStatus::Running, 1024 * 1024),
            "1,0 MB von 11,0 MB · 1,0 MB/s · noch 10 s"
        );
        assert_eq!(
            download_detail(&download, DownloadItemStatus::Paused, 0),
            "Pausiert · 1,0 MB von 11,0 MB"
        );
    }

    #[test]
    fn finished_downloads_name_size_and_source() {
        let download = entry(DownloadStatus::Complete, 2048, 2048);
        assert_eq!(
            download_detail(&download, DownloadItemStatus::Complete, 0),
            "2,0 KB · example.org"
        );
        let paused = Transfer {
            engine_id: 1,
            speed: 0,
            paused: true,
        };
        assert_eq!(
            item_status(&entry(DownloadStatus::Running, 0, 0), Some(&paused)),
            DownloadItemStatus::Paused
        );
        assert_eq!(
            item_status(&entry(DownloadStatus::Interrupted, 0, 0), None),
            DownloadItemStatus::Failed
        );
    }

    #[test]
    fn remaining_time_is_rounded_up() {
        assert_eq!(remaining_time(0), "noch 1 s");
        assert_eq!(remaining_time(61), "noch 2 min");
        assert_eq!(remaining_time(3_601), "noch 2 h");
    }
}
