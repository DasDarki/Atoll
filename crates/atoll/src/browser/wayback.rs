//! The archive as a way out when a page is gone.
//!
//! When a page cannot be reached, or answers that it is not there, Atoll can ask the Internet
//! Archive whether it kept a copy. That asking sends the address to archive.org, so it only
//! happens when the user allowed it, and never for a private tab.

use atoll_core::TabId;
use atoll_i18n::t;

use super::Browser;
use super::preferences::ArchiveFallback;
use crate::platform::tasks::run_in_background;

/// Where Atoll asks whether a copy exists. The archive answers with a redirect to the copy that
/// lies nearest in time, or says that it has none.
const NEAREST: &str = "https://web.archive.org/web/2/";

/// How long the question may take.
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// A copy the archive holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub url: String,
    /// The day it was taken, as `20240503` reads in the answer.
    pub stamp: String,
}

impl Snapshot {
    /// The day of the copy, as the user reads it.
    pub fn day(&self) -> String {
        if self.stamp.len() < 8 {
            return self.stamp.clone();
        }
        format!(
            "{}.{}.{}",
            &self.stamp[6..8],
            &self.stamp[4..6],
            &self.stamp[0..4]
        )
    }
}

/// Why Atoll looks for a copy, which decides what it says and whether it may act on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reason {
    /// Nothing answered at the address.
    Unreachable,
    /// The page answered that it is not there.
    Missing,
    /// The user asked for the copy themselves.
    Asked,
}

impl Reason {
    /// What the question is called above the copy that was found.
    fn title(self) -> &'static str {
        match self {
            Reason::Unreachable => t!("Seite nicht erreichbar"),
            Reason::Missing => t!("Seite nicht gefunden"),
            Reason::Asked => t!("Fassung im Archiv"),
        }
    }
}

impl Browser {
    /// A page could not be reached. Offers the archive if that is wanted.
    pub(super) fn page_unreachable(&self, tab: TabId, url: &str) {
        if self.preferences.borrow().archive_fallback == ArchiveFallback::Off {
            return;
        }
        self.look_for_snapshot(tab, url, Reason::Unreachable);
    }

    /// A page answered that it is not there.
    pub(super) fn page_missing(&self, tab: TabId, url: &str) {
        let preferences = self.preferences.borrow();
        if preferences.archive_fallback == ArchiveFallback::Off || !preferences.archive_on_missing {
            return;
        }
        drop(preferences);
        self.look_for_snapshot(tab, url, Reason::Missing);
    }

    /// Looks for a copy because the user asked for one.
    pub fn open_archived_page(&self) {
        let Some(tab) = self.session.borrow().active_tab else {
            return;
        };
        let url = self
            .session
            .borrow()
            .tab(tab)
            .map(|record| record.url.clone())
            .unwrap_or_default();
        if url.trim().is_empty() {
            return;
        }
        self.look_for_snapshot(tab, &url, Reason::Asked);
    }

    /// Asks the archive and does what the settings say with the answer.
    fn look_for_snapshot(&self, tab: TabId, url: &str, reason: Reason) {
        let asked_for = reason == Reason::Asked;
        if self.is_private(tab) {
            if asked_for {
                self.show_toast(t!(
                    "In einem privaten Tab fragt Atoll das Archiv nicht, die Adresse bliebe sonst dort"
                ));
            }
            return;
        }
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return;
        }
        if self.vpn_holds_everything() {
            if asked_for {
                self.show_toast(t!(
                    "Solange das VPN nicht steht, fragt Atoll das Archiv nicht"
                ));
            }
            return;
        }
        let wanted = url.to_owned();
        let this = self.this();
        if asked_for {
            self.show_toast(t!("Das Archiv wird gefragt …"));
        }
        run_in_background(
            "wayback",
            move || snapshot_of(&wanted),
            move |found| {
                let Some(browser) = this.upgrade() else {
                    return;
                };
                match found {
                    Some(snapshot) => browser.offer_snapshot(tab, snapshot, reason),
                    None if asked_for => {
                        browser.show_toast(t!("Das Archiv hat keine Fassung dieser Seite"));
                    }
                    None => {}
                }
            },
        );
    }

    /// Shows the copy, or asks first, depending on what the user wants.
    fn offer_snapshot(&self, tab: TabId, snapshot: Snapshot, reason: Reason) {
        let automatic = reason != Reason::Asked
            && self.preferences.borrow().archive_fallback == ArchiveFallback::Automatic;
        if automatic {
            self.load_in_tab(tab, &snapshot.url);
            self.show_toast(&t!("Archivfassung vom {} geladen", snapshot.day()));
            return;
        }
        let this = self.this();
        let target = snapshot.url.clone();
        self.offer(
            reason.title(),
            &t!(
                "Das Internet Archive hat eine Fassung vom {}. Soll Atoll sie öffnen?",
                snapshot.day()
            ),
            t!("Archivfassung öffnen"),
            move || {
                if let Some(browser) = this.upgrade() {
                    browser.load_in_tab(tab, &target);
                }
            },
        );
    }
}

/// Asks the Internet Archive whether it has a copy of `url`.
fn snapshot_of(url: &str) -> Option<Snapshot> {
    let answer = atoll_net::agent(TIMEOUT)
        .get(format!("{NEAREST}{}", in_a_path(url)))
        .config()
        .max_redirects(0)
        .build()
        .call()
        .ok()?;
    if !answer.status().is_redirection() {
        return None;
    }
    let location = answer.headers().get("location")?.to_str().ok()?;
    snapshot_at(location)
}

/// Reads the copy out of the address the archive points at.
fn snapshot_at(location: &str) -> Option<Snapshot> {
    let rest = location.split_once("/web/")?.1;
    let stamp: String = rest
        .split('/')
        .next()?
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    if stamp.len() < 8 {
        return None;
    }
    Some(Snapshot {
        url: location.to_owned(),
        stamp,
    })
}

/// Puts an address where it can stand inside another address. Everything a path may hold stays as
/// it is, so the archive sees the address it knows; the rest, a fragment above all, is written out.
fn in_a_path(url: &str) -> String {
    const ALLOWED: &[u8] = b"-._~:/?[]@!$&'()*+,;=%";
    let mut written = String::with_capacity(url.len());
    for byte in url.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' => written.push(byte as char),
            other if ALLOWED.contains(&other) => written.push(other as char),
            other => written.push_str(&format!("%{other:02X}")),
        }
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_day_of_a_copy_is_read_from_its_stamp() {
        let snapshot = Snapshot {
            url: "https://web.archive.org/web/20240503120000/https://example.org/".to_owned(),
            stamp: "20240503120000".to_owned(),
        };
        assert_eq!(snapshot.day(), "03.05.2024");
        assert_eq!(
            Snapshot {
                url: String::new(),
                stamp: "2024".to_owned(),
            }
            .day(),
            "2024",
            "a stamp that says less is shown as it is"
        );
    }

    #[test]
    fn the_copy_is_read_out_of_what_the_archive_points_at() {
        let found = snapshot_at(
            "https://web.archive.org/web/20260904185555/https://www.geocities.com/heartland",
        );
        assert_eq!(
            found,
            Some(Snapshot {
                url:
                    "https://web.archive.org/web/20260904185555/https://www.geocities.com/heartland"
                        .to_owned(),
                stamp: "20260904185555".to_owned(),
            })
        );
        assert_eq!(
            snapshot_at("https://web.archive.org/web/20240503id_/https://example.org/")
                .map(|snapshot| snapshot.stamp),
            Some("20240503".to_owned()),
            "a stamp that carries a note still reads as a day"
        );
        assert_eq!(
            snapshot_at("https://web.archive.org/web/2/https://example.org/"),
            None,
            "without a day there is nothing to offer"
        );
        assert_eq!(snapshot_at("https://example.org/"), None);
    }

    #[test]
    fn addresses_survive_the_way_into_the_question() {
        assert_eq!(
            in_a_path("https://example.org/a b?c=d&e=f"),
            "https://example.org/a%20b?c=d&e=f",
            "what a path may hold stays as it is, so the archive recognises the address"
        );
        assert_eq!(
            in_a_path("https://example.org/x#y"),
            "https://example.org/x%23y",
            "a fragment would otherwise cut the address off"
        );
        assert_eq!(
            in_a_path("https://example.org/%C3%A4"),
            "https://example.org/%C3%A4",
            "an address that is already written out is not written out twice"
        );
    }
}
