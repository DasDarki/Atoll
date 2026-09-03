use std::path::Path;
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use atoll_core::{Download, DownloadId, DownloadKind, DownloadStatus};
use atoll_storage::prelude::*;
use atoll_storage::{DownloadProgress, NewDownload};
use atoll_video::{VideoError, VideoEvent, VideoRequest};

use super::{Control, Transfer};
use crate::browser::Browser;
use crate::browser::windows::WindowManager;
use crate::platform::tasks::run_with_updates;
use atoll_i18n::t;

/// How often the download list shows the progress of a video.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);

/// The name of a video download until yt-dlp has named the file. It is stored as it is, so that
/// it stays the same when the language changes, and is translated where the list shows it.
pub(super) const UNNAMED: &str = "Video wird vorbereitet…";

/// The name of a download as the list shows it.
pub(super) fn display_name(file_name: &str) -> &str {
    if file_name == UNNAMED {
        atoll_i18n::translate(UNNAMED)
    } else {
        file_name
    }
}

impl Browser {
    /// Downloads the video at `url`, a page, player or stream address, with yt-dlp into the
    /// download folder. `name` is the file name without extension, `None` to use the video's
    /// title.
    pub(in crate::browser) fn download_stream(
        &self,
        url: &str,
        name: Option<&str>,
        referer: Option<&str>,
    ) {
        self.ensure_downloads_loaded();
        let directory = self.download_directory();
        if let Err(error) = std::fs::create_dir_all(&directory) {
            self.show_toast(&t!("Download-Ordner nicht verfügbar: {}", error));
            return;
        }
        let file_name = name.map_or_else(|| UNNAMED.to_owned(), |name| format!("{name}.mp4"));
        let now = Self::now();
        let created = self.storage.create_download(NewDownload {
            url,
            path: None,
            file_name: &file_name,
            total_bytes: 0,
            started_at: now,
            kind: DownloadKind::Video,
        });
        let id = match created {
            Ok(id) => id,
            Err(error) => {
                self.report(t!("Download speichern"), Err::<(), _>(error));
                return;
            }
        };
        let download = self.yt_dlp.download(VideoRequest {
            url: url.to_owned(),
            directory,
            name: name.map(str::to_owned),
            referer: referer.map(str::to_owned),
        });
        {
            let mut downloads = self.downloads.borrow_mut();
            downloads.entries.insert(
                0,
                Download {
                    id,
                    url: url.to_owned(),
                    path: None,
                    file_name,
                    received_bytes: 0,
                    total_bytes: 0,
                    status: DownloadStatus::Running,
                    started_at: now,
                    finished_at: None,
                    kind: DownloadKind::Video,
                },
            );
            downloads.transfers.insert(
                id,
                Transfer {
                    control: Control::Video(download.canceller()),
                    speed: 0,
                    paused: false,
                },
            );
            downloads.started_this_session = true;
        }
        tracing::info!(url, "downloading a video with yt-dlp");
        if self.preferences.borrow().download_panel {
            self.open_downloads();
        } else {
            self.show_toast(t!("Video wird heruntergeladen"));
        }
        self.refresh_downloads();

        let for_updates = self.manager.clone();
        let for_result = self.manager.clone();
        run_with_updates(
            "yt-dlp",
            move |send| {
                let mut last_progress: Option<Instant> = None;
                download.run(|event| {
                    let due = match event {
                        VideoEvent::Named(_) => true,
                        VideoEvent::Progress(_) => {
                            last_progress.is_none_or(|at| at.elapsed() >= PROGRESS_INTERVAL)
                        }
                    };
                    if due {
                        if matches!(event, VideoEvent::Progress(_)) {
                            last_progress = Some(Instant::now());
                        }
                        send(event);
                    }
                })
            },
            move |event| {
                if let Some(browser) = any_browser(&for_updates) {
                    browser.stream_updated(id, event);
                }
            },
            move |result| {
                if let Some(browser) = any_browser(&for_result) {
                    browser.stream_finished(id, result);
                }
            },
        );
    }

    /// Starts a video download of the list again, under the name it had.
    pub(super) fn retry_stream(&self, entry: &Download) {
        let name = (entry.file_name != UNNAMED)
            .then(|| Path::new(&entry.file_name).file_stem())
            .flatten()
            .map(|stem| stem.to_string_lossy().into_owned());
        self.download_stream(&entry.url, name.as_deref(), None);
    }

    fn stream_updated(&self, id: DownloadId, event: VideoEvent) {
        let named = {
            let mut downloads = self.downloads.borrow_mut();
            let speed = match &event {
                VideoEvent::Progress(progress) => progress.speed,
                VideoEvent::Named(_) => 0,
            };
            if let Some(transfer) = downloads.transfers.get_mut(&id)
                && speed > 0
            {
                transfer.speed = speed;
            }
            let Some(entry) = downloads.entries.iter_mut().find(|entry| entry.id == id) else {
                return;
            };
            match event {
                VideoEvent::Named(path) => {
                    entry.file_name = file_name(&path, &entry.file_name);
                    entry.path = Some(path.to_string_lossy().into_owned());
                    Some(entry.clone())
                }
                VideoEvent::Progress(progress) => {
                    entry.received_bytes = progress.received_bytes;
                    entry.total_bytes = progress.total_bytes;
                    None
                }
            }
        };
        if let Some(entry) = named {
            self.store_download(&entry);
        }
        self.refresh_downloads();
    }

    fn stream_finished(&self, id: DownloadId, result: Result<std::path::PathBuf, VideoError>) {
        if result == Err(VideoError::Missing) {
            let removed = self.storage.remove_download(id);
            self.report(t!("Download entfernen"), removed);
            {
                let mut downloads = self.downloads.borrow_mut();
                downloads.transfers.remove(&id);
                downloads.entries.retain(|entry| entry.id != id);
            }
            self.refresh_downloads();
            self.show_toast(t!(
                "Für gestreamte Videos fehlt yt-dlp, installiere das Paket yt-dlp"
            ));
            return;
        }
        let entry = {
            let mut downloads = self.downloads.borrow_mut();
            downloads.transfers.remove(&id);
            let Some(entry) = downloads.entries.iter_mut().find(|entry| entry.id == id) else {
                return;
            };
            entry.finished_at = Some(Self::now());
            match &result {
                Ok(path) => {
                    let size = std::fs::metadata(path).map_or(0, |metadata| metadata.len() as i64);
                    entry.file_name = file_name(path, &entry.file_name);
                    entry.path = Some(path.to_string_lossy().into_owned());
                    entry.received_bytes = size;
                    entry.total_bytes = size;
                    entry.status = DownloadStatus::Complete;
                }
                Err(VideoError::Cancelled) => entry.status = DownloadStatus::Canceled,
                Err(_) => entry.status = DownloadStatus::Interrupted,
            }
            entry.clone()
        };
        self.store_download(&entry);
        self.refresh_downloads();
        match result {
            Ok(_) => self.show_toast(&t!("Video gespeichert: {}", entry.file_name)),
            Err(VideoError::Failed(message)) => {
                tracing::warn!(%message, "video download failed");
                self.show_toast(&t!(
                    "Video-Download fehlgeschlagen: {}",
                    shorten(&message, 48)
                ));
            }
            Err(VideoError::Cancelled | VideoError::Missing) => {}
        }
    }

    fn store_download(&self, entry: &Download) {
        let result = self.storage.update_download(
            entry.id,
            DownloadProgress {
                path: entry.path.as_deref(),
                file_name: &entry.file_name,
                received_bytes: entry.received_bytes,
                total_bytes: entry.total_bytes,
                status: entry.status,
                finished_at: entry.finished_at,
            },
        );
        if let Err(error) = result {
            tracing::warn!(%error, "download could not be stored");
        }
    }
}

/// A window to report to. The download list is shared, so any window will do, even when the
/// one that started the download was closed.
fn any_browser(manager: &Weak<WindowManager>) -> Option<Rc<Browser>> {
    manager.upgrade()?.browsers().into_iter().next()
}

fn file_name(path: &Path, fallback: &str) -> String {
    path.file_name().map_or_else(
        || fallback.to_owned(),
        |name| name.to_string_lossy().into_owned(),
    )
}

fn shorten(text: &str, limit: usize) -> String {
    match text.char_indices().nth(limit) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_messages_are_shortened() {
        assert_eq!(shorten("kurz", 10), "kurz");
        assert_eq!(shorten("ääääää", 3), "äää…");
    }

    #[test]
    fn file_names_come_from_the_path() {
        assert_eq!(
            file_name(Path::new("/downloads/Katze.mp4"), "x"),
            "Katze.mp4"
        );
        assert_eq!(file_name(Path::new("/"), "Video.mp4"), "Video.mp4");
    }
}
