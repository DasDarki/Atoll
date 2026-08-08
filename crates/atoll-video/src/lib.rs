//! Downloading videos from streaming sites through the command line tool `yt-dlp`, which knows
//! how to put together streams that pages play in pieces.

mod download;
mod output;

pub use download::{Canceller, VideoDownload, VideoError, VideoEvent, VideoRequest, YtDlp};
pub use output::Progress;
