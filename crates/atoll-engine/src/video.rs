//! Finding the videos of a page, for downloading them.

use serde::Deserialize;

/// Prefix of the console messages the video search reports with.
pub(crate) const VIDEO_REPORT_PREFIX: &str = "\u{1}atoll-videos:";

/// The videos one frame shows.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameVideos {
    pub frame_url: String,
    pub title: String,
    pub videos: Vec<PageVideo>,
}

/// A `<video>` element of a page.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PageVideo {
    /// The address the element plays, a `blob:` address for streams put together by the page.
    pub source: String,
    /// The address of a stream manifest (HLS or DASH) near the element, if the page names one.
    pub stream: String,
    pub playing: bool,
    /// Whether it has played at all.
    pub started: bool,
    /// The visible area in CSS pixels.
    pub area: f64,
    /// Whether it is encrypted, which rules out saving it.
    pub protected: bool,
}

/// The search script, reporting every video of a frame, including those in open shadow roots.
pub(crate) fn find_videos_script() -> String {
    format!(
        "({FIND_VIDEOS})({});",
        serde_json::Value::from(VIDEO_REPORT_PREFIX)
    )
}

/// The videos of a frame, from a report of the search script, `None` for other messages.
pub(crate) fn parse_video_report(message: &str) -> Option<FrameVideos> {
    let payload = message.strip_prefix(VIDEO_REPORT_PREFIX)?;
    let (frame_url, title, videos) = serde_json::from_str(payload).ok()?;
    Some(FrameVideos {
        frame_url,
        title,
        videos,
    })
}

const FIND_VIDEOS: &str = r#"(prefix) => {
  const videos = [];
  const collect = (root) => {
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_ELEMENT);
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      if (node.tagName === "VIDEO") videos.push(node);
      if (node.shadowRoot) collect(node.shadowRoot);
    }
  };
  collect(document);
  const manifest = /^https?:\/\/[^\s"'<>]+\.(m3u8|mpd)(\?[^\s"'<>]*)?$/i;
  const streamNear = (video) => {
    for (const source of video.querySelectorAll("source")) {
      if (manifest.test(source.src)) return source.src;
    }
    let node = video;
    for (let depth = 0; node && depth < 12; depth++) {
      for (const attribute of node.attributes || []) {
        const value = attribute.value.trim();
        if (manifest.test(value)) return value;
      }
      node = node.parentNode instanceof ShadowRoot ? node.parentNode.host : node.parentNode;
    }
    return "";
  };
  const visibleArea = (video) => {
    const box = video.getBoundingClientRect();
    const width = Math.min(box.right, innerWidth) - Math.max(box.left, 0);
    const height = Math.min(box.bottom, innerHeight) - Math.max(box.top, 0);
    return Math.max(0, width) * Math.max(0, height);
  };
  const found = videos.slice(0, 50).map((video) => ({
    source: video.currentSrc || video.src || "",
    stream: streamNear(video),
    playing: !video.paused && !video.ended,
    started: video.currentTime > 0,
    area: Math.round(visibleArea(video)),
    protected: Boolean(video.mediaKeys),
  }));
  console.log(prefix + JSON.stringify([location.href, document.title, found]));
}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_reports_are_parsed() {
        let message = format!(
            r#"{VIDEO_REPORT_PREFIX}["https://clips.example/r/1", "Katze", [
              {{"source": "blob:https://clips.example/5f1", "stream": "https://v.clips.example/1/HLS.m3u8?a=1",
                "playing": true, "started": true, "area": 230400, "protected": false}}
            ]]"#
        );
        let frame = parse_video_report(&message).unwrap();
        assert_eq!(frame.frame_url, "https://clips.example/r/1");
        assert_eq!(frame.title, "Katze");
        assert_eq!(
            frame.videos,
            [PageVideo {
                source: "blob:https://clips.example/5f1".to_owned(),
                stream: "https://v.clips.example/1/HLS.m3u8?a=1".to_owned(),
                playing: true,
                started: true,
                area: 230400.0,
                protected: false,
            }]
        );
        assert_eq!(parse_video_report("[]"), None);
        assert_eq!(
            parse_video_report(&format!("{VIDEO_REPORT_PREFIX}[1]")),
            None
        );
    }

    #[test]
    fn the_script_reports_with_the_video_prefix() {
        assert!(find_videos_script().ends_with(r#")("\u0001atoll-videos:");"#));
    }
}
