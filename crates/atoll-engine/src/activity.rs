//! What a page is doing: whether it is busy, playing media or holding input the user has not
//! sent yet, so it should not be unloaded to save memory, and whether it shows a login form.

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

/// What one frame reported.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct FrameActivity {
    pub(crate) media: bool,
    pub(crate) input: bool,
    pub(crate) login: bool,
}

/// How a report changed the page, `None` where nothing changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ActivityChange {
    pub(crate) busy: Option<bool>,
    pub(crate) login_form: Option<bool>,
}

impl PageActivity {
    pub(crate) fn is_busy(&self) -> bool {
        self.frames.values().any(|frame| frame.media || frame.input)
    }

    pub(crate) fn has_login_form(&self) -> bool {
        self.frames.values().any(|frame| frame.login)
    }

    /// Applies the report of a frame.
    pub(crate) fn update(&mut self, frame: String, activity: FrameActivity) -> ActivityChange {
        let before = (self.is_busy(), self.has_login_form());
        if self.frames.len() < MAX_FRAMES || self.frames.contains_key(&frame) {
            self.frames.insert(frame, activity);
        }
        self.change_since(before)
    }

    /// Forgets every frame when the page navigates.
    pub(crate) fn reset(&mut self) -> ActivityChange {
        let before = (self.is_busy(), self.has_login_form());
        self.frames.clear();
        self.change_since(before)
    }

    fn change_since(&self, (was_busy, had_login_form): (bool, bool)) -> ActivityChange {
        let busy = self.is_busy();
        let login_form = self.has_login_form();
        ActivityChange {
            busy: (busy != was_busy).then_some(busy),
            login_form: (login_form != had_login_form).then_some(login_form),
        }
    }
}

/// The frame and its activity from a report of the observer, `None` for other messages.
pub(crate) fn parse_page_report(message: &str) -> Option<(String, FrameActivity)> {
    let payload = message.strip_prefix(PAGE_REPORT_PREFIX)?;
    let (frame, media, input, login) = serde_json::from_str(payload).ok()?;
    Some((
        frame,
        FrameActivity {
            media,
            input,
            login,
        },
    ))
}

/// The observer script, run in every frame of a page.
pub(crate) fn page_observer_script() -> String {
    PAGE_OBSERVER.replace(
        "__PREFIX__",
        &serde_json::Value::from(PAGE_REPORT_PREFIX).to_string(),
    )
}

/// Reports whether audible media plays, whether form fields differ from their initial values and
/// whether the frame shows a password field.
const PAGE_OBSERVER: &str = r#"(() => {
  const prefix = __PREFIX__;
  const frame = Math.random().toString(36).slice(2);
  let media = false;
  let input = false;
  let edited = false;
  let login = false;
  const report = () => console.log(prefix + JSON.stringify([frame, media, input, login]));
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
  let loginCheck = 0;
  const updateLogin = () => {
    loginCheck = 0;
    const present = document.querySelector("input[type=password]") !== null;
    if (present !== login) {
      login = present;
      report();
    }
  };
  const scheduleLogin = () => {
    if (!loginCheck) loginCheck = setTimeout(updateLogin, 400);
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
  document.addEventListener("DOMContentLoaded", updateLogin);
  document.addEventListener("focusin", updateLogin, true);
  new MutationObserver(scheduleLogin).observe(document, { childList: true, subtree: true });
})();
"#;

#[cfg(test)]
mod tests {
    use super::*;

    const IDLE: FrameActivity = FrameActivity {
        media: false,
        input: false,
        login: false,
    };

    fn busy(busy: bool) -> ActivityChange {
        ActivityChange {
            busy: Some(busy),
            login_form: None,
        }
    }

    #[test]
    fn a_page_is_busy_while_any_frame_is() {
        let mut activity = PageActivity::default();
        assert!(!activity.is_busy());
        let typing = FrameActivity {
            input: true,
            ..IDLE
        };
        let playing = FrameActivity {
            media: true,
            ..IDLE
        };
        assert_eq!(activity.update("main".to_owned(), typing), busy(true));
        assert_eq!(
            activity.update("video".to_owned(), playing),
            ActivityChange::default()
        );
        assert_eq!(
            activity.update("main".to_owned(), IDLE),
            ActivityChange::default()
        );
        assert_eq!(activity.update("video".to_owned(), IDLE), busy(false));
    }

    #[test]
    fn login_forms_are_tracked_separately() {
        let mut activity = PageActivity::default();
        let login = FrameActivity {
            login: true,
            ..IDLE
        };
        assert_eq!(
            activity.update("sso".to_owned(), login),
            ActivityChange {
                busy: None,
                login_form: Some(true),
            }
        );
        assert!(activity.has_login_form());
        assert!(!activity.is_busy());
    }

    #[test]
    fn navigating_forgets_every_frame() {
        let mut activity = PageActivity::default();
        let both = FrameActivity {
            media: true,
            login: true,
            ..IDLE
        };
        activity.update("main".to_owned(), both);
        assert_eq!(
            activity.reset(),
            ActivityChange {
                busy: Some(false),
                login_form: Some(false),
            }
        );
        assert!(!activity.is_busy());
        assert_eq!(activity.reset(), ActivityChange::default());
    }

    #[test]
    fn frames_beyond_the_limit_are_ignored() {
        let mut activity = PageActivity::default();
        for frame in 0..MAX_FRAMES {
            activity.update(frame.to_string(), IDLE);
        }
        let playing = FrameActivity {
            media: true,
            ..IDLE
        };
        assert_eq!(
            activity.update("extra".to_owned(), playing),
            ActivityChange::default()
        );
        assert!(!activity.is_busy());
    }

    #[test]
    fn page_reports_are_parsed() {
        let message = format!(r#"{PAGE_REPORT_PREFIX}["k3j", true, false, true]"#);
        assert_eq!(
            parse_page_report(&message),
            Some((
                "k3j".to_owned(),
                FrameActivity {
                    media: true,
                    input: false,
                    login: true,
                }
            ))
        );
        assert_eq!(parse_page_report("hello"), None);
        assert_eq!(
            parse_page_report(&format!("{PAGE_REPORT_PREFIX}[1, 2]")),
            None
        );
        assert!(page_observer_script().contains(r#"const prefix = "\u0001atoll-page:";"#));
    }
}
