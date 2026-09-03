use std::time::Duration;

use atoll_engine::{FrameVideos, ViewId};
use slint::{Timer, TimerMode};

use crate::browser::Browser;
use atoll_i18n::t;

/// How long the frames of a page get to report their videos.
const SEARCH_TIMEOUT: Duration = Duration::from_millis(1500);

/// A search for the videos of a page that not every frame has answered yet.
pub struct VideoSearch {
    view: ViewId,
    page_url: String,
    unanswered: usize,
    frames: Vec<FrameVideos>,
    _timeout: Timer,
}

/// How the video of a page can be saved.
#[derive(Debug, Clone, PartialEq, Eq)]
enum VideoChoice {
    NoVideo,
    Protected,
    /// A plain file the engine downloads like any other.
    File(String),
    /// A stream for yt-dlp.
    Stream {
        url: String,
        name: Option<String>,
        referer: String,
    },
}

impl Browser {
    /// Ctrl+Shift+S, the button in the address bar and the palette: downloads the video that
    /// plays, or the largest one of the page.
    pub fn download_video(&self) {
        let Some(page_url) = self
            .current_page_url()
            .filter(|url| url.starts_with("https://") || url.starts_with("http://"))
        else {
            self.show_toast(t!("Videos lassen sich nur von Webseiten herunterladen"));
            return;
        };
        let mut asked = None;
        self.with_live_view(|view| asked = Some((view.id(), view.find_videos())));
        let Some((view, frames)) = asked.filter(|(_, frames)| *frames > 0) else {
            self.show_toast(t!("Auf dieser Seite wurde kein Video gefunden"));
            return;
        };
        let this = self.this();
        let timeout = Timer::default();
        timeout.start(TimerMode::SingleShot, SEARCH_TIMEOUT, move || {
            if let Some(browser) = this.upgrade() {
                browser.video_search_timed_out();
            }
        });
        *self.video_search.borrow_mut() = Some(VideoSearch {
            view,
            page_url,
            unanswered: frames,
            frames: Vec::new(),
            _timeout: timeout,
        });
    }

    /// A frame reported its videos. The search ends once every frame has answered.
    pub(in crate::browser) fn videos_found(&self, view: ViewId, frame: FrameVideos) {
        let finished = {
            let mut search = self.video_search.borrow_mut();
            let Some(pending) = search.as_mut().filter(|pending| pending.view == view) else {
                return;
            };
            pending.frames.push(frame);
            pending.unanswered = pending.unanswered.saturating_sub(1);
            if pending.unanswered == 0 {
                search.take()
            } else {
                None
            }
        };
        if let Some(search) = finished {
            self.finish_video_search(search);
        }
    }

    fn video_search_timed_out(&self) {
        let search = self.video_search.borrow_mut().take();
        if let Some(search) = search {
            self.finish_video_search(search);
        }
    }

    fn finish_video_search(&self, search: VideoSearch) {
        if self.current_page_url().as_deref() != Some(search.page_url.as_str()) {
            return;
        }
        match choose_video(&search.frames, &search.page_url) {
            VideoChoice::NoVideo => {
                self.show_toast(t!("Auf dieser Seite wurde kein Video gefunden"))
            }
            VideoChoice::Protected => self.show_toast(t!(
                "Dieses Video ist kopiergeschützt und lässt sich nicht speichern"
            )),
            VideoChoice::File(url) => {
                tracing::info!(url, "downloading a video file");
                self.with_live_view(|view| view.start_download(&url));
            }
            VideoChoice::Stream { url, name, referer } => {
                self.download_stream(&url, name.as_deref(), Some(&referer));
            }
        }
    }
}

/// Picks the video that plays, else one that has played, else the largest visible one, and
/// decides how to save it.
fn choose_video(frames: &[FrameVideos], page_url: &str) -> VideoChoice {
    let best = frames
        .iter()
        .flat_map(|frame| frame.videos.iter().map(move |video| (frame, video)))
        .filter(|(_, video)| !video.source.is_empty() || !video.stream.is_empty())
        .max_by_key(|(_, video)| (video.playing, video.started, video.area.round() as i64));
    let Some((frame, video)) = best else {
        return VideoChoice::NoVideo;
    };
    if video.protected {
        return VideoChoice::Protected;
    }
    let web_source = is_web(&video.source);
    if web_source && !is_manifest(&video.source) {
        return VideoChoice::File(video.source.clone());
    }
    let (url, name) = if !video.stream.is_empty() {
        (video.stream.clone(), title(frame))
    } else if web_source {
        (video.source.clone(), title(frame))
    } else if frame.frame_url != page_url && is_web(&frame.frame_url) {
        (frame.frame_url.clone(), None)
    } else {
        (page_url.to_owned(), None)
    };
    VideoChoice::Stream {
        url,
        name,
        referer: frame.frame_url.clone(),
    }
}

fn is_web(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

/// Whether `url` is an HLS or DASH manifest, which yt-dlp puts together.
fn is_manifest(url: &str) -> bool {
    let path = url
        .split(['?', '#'])
        .next()
        .unwrap_or_default()
        .to_lowercase();
    path.ends_with(".m3u8") || path.ends_with(".mpd")
}

/// The page title as a file name for a bare stream, which has no title of its own.
fn title(frame: &FrameVideos) -> Option<String> {
    let title = frame.title.trim();
    (!title.is_empty()).then(|| title.to_owned())
}

#[cfg(test)]
mod tests {
    use atoll_engine::PageVideo;

    use super::*;

    fn video(source: &str, playing: bool, area: f64) -> PageVideo {
        PageVideo {
            source: source.to_owned(),
            stream: String::new(),
            playing,
            started: playing,
            area,
            protected: false,
        }
    }

    fn frame(url: &str, videos: Vec<PageVideo>) -> FrameVideos {
        FrameVideos {
            frame_url: url.to_owned(),
            title: "Katze macht Sachen".to_owned(),
            videos,
        }
    }

    const PAGE: &str = "https://clips.example/r/1";

    #[test]
    fn a_playing_video_beats_a_larger_one() {
        let frames = [frame(
            PAGE,
            vec![
                video("https://cdn.example/big.mp4", false, 900_000.0),
                video("https://cdn.example/playing.mp4", true, 20_000.0),
            ],
        )];
        assert_eq!(
            choose_video(&frames, PAGE),
            VideoChoice::File("https://cdn.example/playing.mp4".to_owned())
        );
    }

    #[test]
    fn without_playback_the_largest_video_wins() {
        let frames = [
            frame(
                PAGE,
                vec![video("https://cdn.example/small.mp4", false, 100.0)],
            ),
            frame(
                "https://player.example/embed/7",
                vec![video("https://cdn.example/large.webm", false, 5000.0)],
            ),
        ];
        assert_eq!(
            choose_video(&frames, PAGE),
            VideoChoice::File("https://cdn.example/large.webm".to_owned())
        );
    }

    #[test]
    fn streams_use_the_manifest_named_near_the_video() {
        let mut stream = video("blob:https://clips.example/5f1", true, 1000.0);
        stream.stream = "https://v.clips.example/1/HLSPlaylist.m3u8?a=1".to_owned();
        assert_eq!(
            choose_video(&[frame(PAGE, vec![stream])], PAGE),
            VideoChoice::Stream {
                url: "https://v.clips.example/1/HLSPlaylist.m3u8?a=1".to_owned(),
                name: Some("Katze macht Sachen".to_owned()),
                referer: PAGE.to_owned(),
            }
        );
    }

    #[test]
    fn a_manifest_source_is_a_stream() {
        let frames = [frame(
            PAGE,
            vec![video("https://cdn.example/live/index.M3U8?t=2", true, 10.0)],
        )];
        assert!(matches!(
            choose_video(&frames, PAGE),
            VideoChoice::Stream { url, .. } if url == "https://cdn.example/live/index.M3U8?t=2"
        ));
    }

    #[test]
    fn blob_videos_fall_back_to_their_frame_or_page() {
        let embedded = [frame(
            "https://www.youtube-nocookie.com/embed/abc",
            vec![video("blob:https://www.youtube-nocookie.com/1", true, 10.0)],
        )];
        assert_eq!(
            choose_video(&embedded, PAGE),
            VideoChoice::Stream {
                url: "https://www.youtube-nocookie.com/embed/abc".to_owned(),
                name: None,
                referer: "https://www.youtube-nocookie.com/embed/abc".to_owned(),
            }
        );
        let own = [frame(
            PAGE,
            vec![video("blob:https://clips.example/2", true, 10.0)],
        )];
        assert!(matches!(
            choose_video(&own, PAGE),
            VideoChoice::Stream { url, name: None, .. } if url == PAGE
        ));
    }

    #[test]
    fn protected_and_missing_videos_are_reported() {
        let mut locked = video("blob:https://films.example/1", true, 10.0);
        locked.protected = true;
        assert_eq!(
            choose_video(&[frame(PAGE, vec![locked])], PAGE),
            VideoChoice::Protected
        );
        assert_eq!(
            choose_video(&[frame(PAGE, vec![video("", false, 10.0)])], PAGE),
            VideoChoice::NoVideo
        );
        assert_eq!(choose_video(&[], PAGE), VideoChoice::NoVideo);
    }
}
