use crate::ids::DownloadId;
use crate::time::Timestamp;

/// Whether a download is still running or how it ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadStatus {
    Running,
    Complete,
    Canceled,
    /// The download stopped without the user canceling it, for example because Atoll closed.
    Interrupted,
}

impl DownloadStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            DownloadStatus::Running => "running",
            DownloadStatus::Complete => "complete",
            DownloadStatus::Canceled => "canceled",
            DownloadStatus::Interrupted => "interrupted",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "running" => Some(DownloadStatus::Running),
            "complete" => Some(DownloadStatus::Complete),
            "canceled" => Some(DownloadStatus::Canceled),
            "interrupted" => Some(DownloadStatus::Interrupted),
            _ => None,
        }
    }
}

/// How a download is fetched.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DownloadKind {
    /// A file the browser engine downloads.
    #[default]
    File,
    /// A video put together from a stream by yt-dlp, `url` is what yt-dlp was given.
    Video,
    /// A torrent, `url` is the magnet address or where the torrent file came from.
    Torrent,
}

impl DownloadKind {
    pub fn as_str(self) -> &'static str {
        match self {
            DownloadKind::File => "file",
            DownloadKind::Video => "video",
            DownloadKind::Torrent => "torrent",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "file" => Some(DownloadKind::File),
            "video" => Some(DownloadKind::Video),
            "torrent" => Some(DownloadKind::Torrent),
            _ => None,
        }
    }
}

/// An entry of the download list. Finished downloads stay until the user removes them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Download {
    pub id: DownloadId,
    pub url: String,
    /// Where the file is saved, unknown until the location is chosen.
    pub path: Option<String>,
    pub file_name: String,
    pub received_bytes: i64,
    /// The expected size, zero if the server did not announce it.
    pub total_bytes: i64,
    pub status: DownloadStatus,
    pub started_at: Timestamp,
    pub finished_at: Option<Timestamp>,
    pub kind: DownloadKind,
}
