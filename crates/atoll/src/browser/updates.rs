//! Looking whether a newer Atoll was released.
//!
//! Atoll asks GitHub for the newest release, once a day and never twice in one run. It only
//! looks: where Atoll was installed by a package manager, that manager does the updating, and
//! a browser that replaces its own files behind the manager's back would only cause trouble.
//! The request takes the same way as everything else Atoll fetches, so a VPN stays in front
//! of it, and it can be switched off entirely.

use std::time::Duration;

use atoll_i18n::t;
use atoll_storage::prelude::*;
use slint::ComponentHandle;

use super::Browser;
use crate::platform::tasks::run_in_background;
use crate::ui::SettingsState;

/// The release Atoll asks about.
const RELEASES: &str = "https://api.github.com/repos/DasDarki/Atoll/releases/latest";

/// The page a user is sent to when there is something newer.
const RELEASE_PAGE: &str = "https://github.com/DasDarki/Atoll/releases/latest";

/// How long an answer counts before Atoll asks again.
const ASK_AGAIN_AFTER: i64 = 24 * 60 * 60 * 1000;

/// How long the request may take.
const TIMEOUT: Duration = Duration::from_secs(10);

/// How much of the answer is read, which is far more than a release description needs.
const MAX_ANSWER: u64 = 512 * 1024;

/// When Atoll last asked, so it does not ask at every start.
const ASKED_AT: &str = "update_checked_at";

/// The newest version Atoll heard of, so a found update is announced once and not at every start.
const HEARD_OF: &str = "update_latest_seen";

/// The version this build carries.
pub fn running_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// What is known about newer versions in this window.
#[derive(Default)]
pub struct Updates {
    asking: bool,
    /// The newest version GitHub named, once it answered.
    newest: Option<String>,
}

/// What an answer from GitHub says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub page: String,
}

impl Browser {
    /// At the start: asks about new versions when it is time and the user wants it.
    pub(super) fn check_for_updates_in_time(&self) {
        if !self.preferences.borrow().update_check {
            return;
        }
        let asked_at = self.storage.setting_i64(ASKED_AT).ok().flatten();
        let now = Self::now().0;
        if asked_at.is_some_and(|at| now.saturating_sub(at) < ASK_AGAIN_AFTER) {
            self.show_known_update();
            return;
        }
        self.ask_about_updates(false);
    }

    /// The button in the settings: asks right now and says what came back either way.
    pub fn check_for_updates_now(&self) {
        self.ask_about_updates(true);
    }

    fn ask_about_updates(&self, asked_by_hand: bool) {
        if self.updates.borrow().asking {
            return;
        }
        self.updates.borrow_mut().asking = true;
        if asked_by_hand {
            self.set_update_status(t!("GitHub wird gefragt…").to_owned());
        }
        let this = self.this();
        run_in_background("updates", newest_release, move |result| {
            if let Some(browser) = this.upgrade() {
                browser.update_answered(asked_by_hand, result);
            }
        });
    }

    fn update_answered(&self, asked_by_hand: bool, result: Result<Release, String>) {
        self.updates.borrow_mut().asking = false;
        let release = match result {
            Ok(release) => release,
            Err(error) => {
                tracing::debug!(%error, "the newest version could not be asked for");
                if asked_by_hand {
                    self.set_update_status(t!("GitHub antwortet nicht: {}", error));
                }
                return;
            }
        };
        let now = Self::now();
        if let Err(error) = self.storage.set_setting(ASKED_AT, &now.0.to_string()) {
            tracing::warn!(%error, "the time of the update check could not be stored");
        }
        let newer = is_newer(&release.version, running_version());
        let known = self.storage.setting(HEARD_OF).ok().flatten();
        let heard_of_it = known.as_deref() == Some(release.version.as_str());
        self.updates.borrow_mut().newest = newer.then(|| release.version.clone());
        if newer {
            let stored = self.storage.set_setting(HEARD_OF, &release.version);
            if let Err(error) = stored {
                tracing::warn!(%error, "the newest version could not be stored");
            }
        }
        self.show_update_status(newer.then_some(release.version.as_str()));
        if newer && (asked_by_hand || !heard_of_it) {
            self.show_toast(&update_message(&release.version, installed_by_a_manager()));
        } else if asked_by_hand {
            self.show_toast(&t!("Atoll {} ist die neueste Version", running_version()));
        }
    }

    /// Shows what the last check found, without asking again.
    fn show_known_update(&self) {
        let known = self.storage.setting(HEARD_OF).ok().flatten();
        let newer = known.filter(|version| is_newer(version, running_version()));
        self.updates.borrow_mut().newest = newer.clone();
        self.show_update_status(newer.as_deref());
    }

    /// Fills the line in the settings that says what is known about versions.
    pub(super) fn show_update_settings(&self) {
        let Some(window) = self.window() else { return };
        let state = window.global::<SettingsState>();
        state.set_update_check(self.preferences.borrow().update_check);
        state.set_version(running_version().into());
        let newest = self.updates.borrow().newest.clone();
        self.show_update_status(newest.as_deref());
    }

    fn show_update_status(&self, newer: Option<&str>) {
        let status = match newer {
            Some(version) => update_message(version, installed_by_a_manager()),
            None => t!("Das ist die neueste Version.").to_owned(),
        };
        self.set_update_status(status);
    }

    fn set_update_status(&self, status: String) {
        self.for_all_windows(|browser| {
            if let Some(window) = browser.window() {
                window
                    .global::<SettingsState>()
                    .set_update_status(status.as_str().into());
            }
        });
    }

    /// Settings: whether Atoll looks for new versions at all.
    pub(in crate::browser) fn set_update_check(&self, on: bool) {
        self.update_preference(
            super::preferences::PreferenceKey::UpdateCheck,
            |preferences| {
                preferences.update_check = on;
            },
        );
        if on {
            self.ask_about_updates(false);
        } else {
            self.set_update_status(t!("Es wird nicht nach neuen Versionen gesehen.").to_owned());
        }
    }

    /// The settings: opens the page the new version lies on.
    pub fn open_release_page(&self) {
        self.open_address(RELEASE_PAGE);
    }
}

/// Whether Atoll lies where a package manager put it, which then does the updating.
fn installed_by_a_manager() -> bool {
    std::env::current_exe().is_ok_and(|path| {
        path.starts_with("/opt") || path.starts_with("/usr") || path.starts_with("/app")
    })
}

/// What the user is told about a version they do not have yet.
fn update_message(version: &str, by_a_manager: bool) -> String {
    if by_a_manager {
        t!(
            "Atoll {} ist da. Dein Paketsystem bringt sie, mit „pacman -Syu“.",
            version
        )
    } else {
        t!(
            "Atoll {} ist da, auf der Release-Seite liegt sie bereit.",
            version
        )
    }
}

/// Where the question goes. `ATOLL_RELEASES` points it elsewhere, which is how it is tried out.
fn releases_url() -> String {
    std::env::var("ATOLL_RELEASES").unwrap_or_else(|_| RELEASES.to_owned())
}

/// Asks GitHub for the newest release. Runs away from the user interface.
fn newest_release() -> Result<Release, String> {
    let mut answer = atoll_net::agent(TIMEOUT)
        .get(releases_url())
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|error| error.to_string())?;
    let body = answer
        .body_mut()
        .with_config()
        .limit(MAX_ANSWER)
        .read_to_string()
        .map_err(|error| error.to_string())?;
    parse_release(&body).ok_or_else(|| t!("unverständliche Antwort").to_owned())
}

/// The version and page out of GitHub's answer.
fn parse_release(body: &str) -> Option<Release> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let tag = value.get("tag_name")?.as_str()?;
    let version = tag.trim().trim_start_matches(['v', 'V']).to_owned();
    if version.is_empty() {
        return None;
    }
    let page = value
        .get("html_url")
        .and_then(|url| url.as_str())
        .unwrap_or(RELEASE_PAGE)
        .to_owned();
    Some(Release { version, page })
}

/// Whether `offered` counts higher than `running`.
fn is_newer(offered: &str, running: &str) -> bool {
    rank(offered) > rank(running)
}

/// The three numbers of a version and whether it is a finished one. A version with something
/// appended, as a release candidate has, stands below the plain one it leads to.
fn rank(version: &str) -> ([u64; 3], bool) {
    let (numbers, appended) = version.split_once(['-', '+']).unwrap_or((version, ""));
    let mut parts = [0; 3];
    for (slot, piece) in parts.iter_mut().zip(numbers.split('.')) {
        *slot = piece.trim().parse().unwrap_or(0);
    }
    (parts, appended.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_higher_version_is_newer() {
        assert!(is_newer("1.2.0", "1.1.9"));
        assert!(is_newer("1.10.0", "1.9.0"));
        assert!(is_newer("2.0.0", "1.99.99"));
        assert!(!is_newer("1.2.0", "1.2.0"));
        assert!(!is_newer("1.1.0", "1.2.0"));
    }

    #[test]
    fn a_release_candidate_stands_below_the_release() {
        assert!(!is_newer("1.2.0-rc1", "1.2.0"));
        assert!(is_newer("1.2.0", "1.2.0-rc1"));
    }

    #[test]
    fn the_answer_of_github_is_read() {
        let body = r#"{"tag_name":"v1.4.2","html_url":"https://example.org/r/1.4.2","x":1}"#;
        assert_eq!(
            parse_release(body),
            Some(Release {
                version: "1.4.2".to_owned(),
                page: "https://example.org/r/1.4.2".to_owned(),
            })
        );
        assert_eq!(parse_release("{}"), None);
        assert_eq!(parse_release("kein json"), None);
        assert_eq!(parse_release(r#"{"tag_name":"  "}"#), None);
    }

    #[test]
    fn the_message_says_who_does_the_updating() {
        assert!(update_message("1.2.0", true).contains("pacman"));
        assert!(update_message("1.2.0", false).contains("Release"));
    }
}
