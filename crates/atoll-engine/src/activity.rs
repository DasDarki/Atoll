//! Whether a page is busy: it plays media or holds input the user has not sent yet. Busy pages
//! should not be unloaded to save memory.

use std::collections::HashMap;

/// Prefix of the console messages the activity observer reports with.
pub(crate) const PAGE_REPORT_PREFIX: &str = "\u{1}atoll-page:";

/// The most frames of a page that are tracked, further ones are ignored.
const MAX_FRAMES: usize = 64;

/// What the frames of one page reported.
#[derive(Debug, Default)]
pub(crate) struct PageActivity {
    frames: HashMap<String, FrameActivity>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct FrameActivity {
    media: bool,
    input: bool,
}

impl PageActivity {
    pub(crate) fn is_busy(&self) -> bool {
        self.frames.values().any(|frame| frame.media || frame.input)
    }

    /// Applies a report of a frame. Returns the new busy state if it changed.
    pub(crate) fn update(&mut self, frame: String, media: bool, input: bool) -> Option<bool> {
        let was_busy = self.is_busy();
        if self.frames.len() < MAX_FRAMES || self.frames.contains_key(&frame) {
            self.frames.insert(frame, FrameActivity { media, input });
        }
        let busy = self.is_busy();
        (busy != was_busy).then_some(busy)
    }

    /// Forgets every frame when the page navigates. Returns whether the page was busy.
    pub(crate) fn reset(&mut self) -> bool {
        let was_busy = self.is_busy();
        self.frames.clear();
        was_busy
    }
}

/// Frame, media and input state from a report of the observer, `None` for other messages.
pub(crate) fn parse_page_report(message: &str) -> Option<(String, bool, bool)> {
    let payload = message.strip_prefix(PAGE_REPORT_PREFIX)?;
    serde_json::from_str(payload).ok()
}

/// The observer script, run in every frame of a page.
pub(crate) fn page_observer_script() -> String {
    PAGE_OBSERVER.replace(
        "__PREFIX__",
        &serde_json::Value::from(PAGE_REPORT_PREFIX).to_string(),
    )
}

/// Reports whether audible media plays and whether form fields differ from their initial values.
const PAGE_OBSERVER: &str = r#"(() => {
  const prefix = __PREFIX__;
  const frame = Math.random().toString(36).slice(2);
  let media = false;
  let input = false;
  let edited = false;
  const report = () => console.log(prefix + JSON.stringify([frame, media, input]));
  const audible = (element) => !element.paused && !element.ended && !element.muted && element.volume > 0;
  const updateMedia = () => {
    const playing = [...document.querySelectorAll("video, audio")].some(audible);
    if (playing !== media) {
      media = playing;
      report();
    }
  };
  const fields = "textarea, input:not([type=hidden]):not([type=search]):not([type=submit])"
    + ":not([type=button]):not([type=reset]):not([type=file]):not([type=image])";
  const changed = (field) => field.type === "checkbox" || field.type === "radio"
    ? field.checked !== field.defaultChecked
    : field.value !== field.defaultValue;
  const updateInput = (event) => {
    if (event.target && event.target.isContentEditable) edited = true;
    const now = edited || [...document.querySelectorAll(fields)].some(changed);
    if (now !== input) {
      input = now;
      report();
    }
  };
  for (const type of ["playing", "pause", "ended", "volumechange", "emptied"]) {
    document.addEventListener(type, updateMedia, true);
  }
  document.addEventListener("input", updateInput, true);
  document.addEventListener("change", updateInput, true);
  document.addEventListener("submit", () => {
    edited = false;
    if (input) {
      input = false;
      report();
    }
  }, true);
})();
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_is_busy_while_any_frame_is() {
        let mut activity = PageActivity::default();
        assert!(!activity.is_busy());
        assert_eq!(activity.update("main".to_owned(), false, true), Some(true));
        assert_eq!(activity.update("video".to_owned(), true, false), None);
        assert_eq!(activity.update("main".to_owned(), false, false), None);
        assert_eq!(
            activity.update("video".to_owned(), false, false),
            Some(false)
        );
    }

    #[test]
    fn navigating_forgets_every_frame() {
        let mut activity = PageActivity::default();
        activity.update("main".to_owned(), true, false);
        assert!(activity.reset());
        assert!(!activity.is_busy());
        assert!(!activity.reset());
    }

    #[test]
    fn frames_beyond_the_limit_are_ignored() {
        let mut activity = PageActivity::default();
        for frame in 0..MAX_FRAMES {
            activity.update(frame.to_string(), false, false);
        }
        assert_eq!(activity.update("extra".to_owned(), true, false), None);
        assert!(!activity.is_busy());
    }

    #[test]
    fn page_reports_are_parsed() {
        let message = format!(r#"{PAGE_REPORT_PREFIX}["k3j", true, false]"#);
        assert_eq!(
            parse_page_report(&message),
            Some(("k3j".to_owned(), true, false))
        );
        assert_eq!(parse_page_report("hello"), None);
        assert_eq!(
            parse_page_report(&format!("{PAGE_REPORT_PREFIX}[1, 2]")),
            None
        );
        assert!(page_observer_script().contains(r#"const prefix = "\u0001atoll-page:";"#));
    }
}
