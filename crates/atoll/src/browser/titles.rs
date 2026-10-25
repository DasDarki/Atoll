//! The title of a page that was opened but not looked at yet.
//!
//! A tab opened with the middle mouse button stays a record: nothing is loaded, no engine runs
//! for it, and the sidebar can only show its address. That address says little, so Atoll asks the
//! site for the beginning of the page, reads the title out of its head and shows that instead.
//!
//! It is the smallest request that answers the question: the first few kilobytes, never the whole
//! page, no pictures, no scripts, and the connection is dropped once the head is through. It goes
//! the same way as everything else Atoll fetches, so a VPN stays in front of it, and private tabs
//! are left out entirely - what they do belongs to them.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::time::Duration;

use atoll_core::TabId;
use atoll_storage::prelude::*;

use super::Browser;
use crate::platform::tasks::run_in_background;

/// How long the site has to answer.
const TIMEOUT: Duration = Duration::from_secs(8);

/// How much of a page is read. A head that has not named the page by then is no head of ours.
const ENOUGH: u64 = 128 * 1024;

/// How long a title may be. A page that writes an essay into its title is cut short.
const LONGEST: usize = 300;

/// How many sites are asked at the same time. Opening a whole folder of bookmarks must not turn
/// into a burst of connections.
const AT_ONCE: usize = 3;

/// The tabs waiting to be asked about, and how many questions are out.
#[derive(Default)]
pub struct Titles {
    waiting: VecDeque<TabId>,
    running: usize,
}

/// Where a window keeps them.
pub type TitleQueue = RefCell<Titles>;

impl Browser {
    /// Lines `tab` up to be asked about, for a tab nobody has looked at yet.
    pub(super) fn fetch_title(&self, tab: TabId) {
        self.titles.borrow_mut().waiting.push_back(tab);
        self.ask_about_titles();
    }

    /// Starts as many questions as may be out at once.
    fn ask_about_titles(&self) {
        loop {
            let next = {
                let mut titles = self.titles.borrow_mut();
                if titles.running >= AT_ONCE {
                    return;
                }
                let Some(next) = titles.waiting.pop_front() else {
                    return;
                };
                titles.running += 1;
                next
            };
            if !self.ask_about_title(next) {
                self.titles.borrow_mut().running -= 1;
                continue;
            }
            return;
        }
    }

    /// Asks the site of `tab` what the page is called. Answers whether a question went out.
    fn ask_about_title(&self, tab: TabId) -> bool {
        let (url, known) = {
            let session = self.session.borrow();
            let Some(record) = session.tab(tab).filter(|record| !record.private) else {
                return false;
            };
            (record.url.clone(), record.title.clone())
        };
        if known.is_some_and(|title| !title.trim().is_empty()) {
            return false;
        }
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return false;
        }
        let asked = url.clone();
        let this = self.this();
        run_in_background(
            "title",
            move || title_of(&asked),
            move |title| {
                let Some(browser) = this.upgrade() else {
                    return;
                };
                browser.titles.borrow_mut().running -= 1;
                if let Some(title) = title {
                    browser.title_arrived(tab, &url, &title);
                }
                browser.ask_about_titles();
            },
        );
        true
    }

    /// The site answered. The title is only taken if the tab is still there, still shows that
    /// address and still has no title of its own: a page that was opened meanwhile knows better.
    fn title_arrived(&self, tab: TabId, url: &str, title: &str) {
        {
            let mut session = self.session.borrow_mut();
            let Some(record) = session.tab_mut(tab) else {
                return;
            };
            if record.url != url
                || record
                    .title
                    .as_deref()
                    .is_some_and(|known| !known.is_empty())
            {
                return;
            }
            record.title = Some(title.to_owned());
        }
        if let Err(error) = self.storage.update_tab_title(tab, Some(title)) {
            tracing::warn!(%error, "the title of a page could not be stored");
        }
        tracing::debug!(tab = tab.0, title, "a page said what it is called");
        self.refresh_sidebar();
        self.for_other_windows(Browser::refresh_sidebar);
    }
}

/// Fetches the beginning of the page at `url` and reads its title. Runs away from the window.
fn title_of(url: &str) -> Option<String> {
    let mut answer = atoll_net::agent(TIMEOUT)
        .get(url)
        // Only the page itself is wanted, and only its beginning.
        .header("Accept", "text/html,application/xhtml+xml")
        .header("Range", "bytes=0-131071")
        .call()
        .ok()?;
    let kind = answer
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !kind.is_empty() && !kind.contains("html") && !kind.contains("xml") {
        return None;
    }
    let body = answer
        .body_mut()
        .with_config()
        .limit(ENOUGH)
        .read_to_vec()
        .ok()?;
    title_in(&String::from_utf8_lossy(&body))
}

/// The title out of the head of a page, as it should be shown.
fn title_in(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<title")?;
    let after = html[start..].find('>')? + start + 1;
    let end = lower[after..].find("</title>")? + after;
    let title = unescape(&html[after..end]);
    // Whatever a page puts between its words, the sidebar shows single spaces.
    let title = title
        .split(|character: char| character.is_whitespace() || character == '\u{a0}')
        .filter(|piece| !piece.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if title.is_empty() {
        return None;
    }
    Some(shortened(&title))
}
/// Turns the escapes a title carries back into their characters. What is not known stays as it
/// stands, which is better than losing it.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let Some(end) = rest[..rest.len().min(12)].find(';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        match character_of(&rest[1..end]) {
            Some(character) => out.push(character),
            None => out.push_str(&rest[..=end]),
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

/// The character an escape stands for, written without the `&` and the `;`.
fn character_of(entity: &str) -> Option<char> {
    if let Some(character) = numeric(entity) {
        return Some(character);
    }
    if let Some(at) = LATIN1.iter().position(|name| *name == entity) {
        return char::from_u32(0xa0 + at as u32);
    }
    match entity {
        "amp" | "AMP" => Some('&'),
        "lt" | "LT" => Some('<'),
        "gt" | "GT" => Some('>'),
        "quot" | "QUOT" => Some('"'),
        "apos" => Some('\''),
        "ndash" => Some('\u{2013}'),
        "mdash" => Some('\u{2014}'),
        "hellip" => Some('\u{2026}'),
        "lsquo" => Some('\u{2018}'),
        "rsquo" => Some('\u{2019}'),
        "ldquo" => Some('\u{201c}'),
        "rdquo" => Some('\u{201d}'),
        "bdquo" => Some('\u{201e}'),
        "bull" => Some('\u{2022}'),
        "dagger" => Some('\u{2020}'),
        "trade" => Some('\u{2122}'),
        "euro" => Some('\u{20ac}'),
        "permil" => Some('\u{2030}'),
        "larr" => Some('\u{2190}'),
        "rarr" => Some('\u{2192}'),
        _ => None,
    }
}

/// The named escapes of the Latin-1 characters, in the order of the characters themselves: the
/// first stands for U+00A0 and every next one for the character after it.
const LATIN1: [&str; 96] = [
    "nbsp", "iexcl", "cent", "pound", "curren", "yen", "brvbar", "sect", "uml", "copy", "ordf",
    "laquo", "not", "shy", "reg", "macr", "deg", "plusmn", "sup2", "sup3", "acute", "micro",
    "para", "middot", "cedil", "sup1", "ordm", "raquo", "frac14", "frac12", "frac34", "iquest",
    "Agrave", "Aacute", "Acirc", "Atilde", "Auml", "Aring", "AElig", "Ccedil", "Egrave", "Eacute",
    "Ecirc", "Euml", "Igrave", "Iacute", "Icirc", "Iuml", "ETH", "Ntilde", "Ograve", "Oacute",
    "Ocirc", "Otilde", "Ouml", "times", "Oslash", "Ugrave", "Uacute", "Ucirc", "Uuml", "Yacute",
    "THORN", "szlig", "agrave", "aacute", "acirc", "atilde", "auml", "aring", "aelig", "ccedil",
    "egrave", "eacute", "ecirc", "euml", "igrave", "iacute", "icirc", "iuml", "eth", "ntilde",
    "ograve", "oacute", "ocirc", "otilde", "ouml", "divide", "oslash", "ugrave", "uacute", "ucirc",
    "uuml", "yacute", "thorn", "yuml",
];

/// A numbered escape such as `&#252;` or `&#x00fc;`.
fn numeric(entity: &str) -> Option<char> {
    let digits = entity.strip_prefix('#')?;
    let number = match digits.strip_prefix(['x', 'X']) {
        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
        None => digits.parse().ok()?,
    };
    char::from_u32(number)
}
/// Cuts a title that goes on and on, at a character boundary.
fn shortened(title: &str) -> String {
    match title.char_indices().nth(LONGEST) {
        Some((end, _)) => format!("{}…", title[..end].trim_end()),
        None => title.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_title_comes_out_of_the_head() {
        let html = "<!doctype html><html><head><meta charset=utf-8>\n  <title>Das Forum</title>\
                    </head><body><title>nicht dieser</title></body>";
        assert_eq!(title_in(html), Some("Das Forum".to_owned()));
    }

    #[test]
    fn a_title_with_attributes_and_line_breaks_is_read() {
        let html = "<TITLE lang=\"de\">Zeile eins\n   Zeile zwei</TITLE>";
        assert_eq!(title_in(html), Some("Zeile eins Zeile zwei".to_owned()));
    }

    #[test]
    fn a_page_without_a_title_says_nothing() {
        assert_eq!(title_in("<html><head></head><body>nichts</body>"), None);
        assert_eq!(title_in("<title>   </title>"), None);
        assert_eq!(title_in("kein html"), None);
        assert_eq!(title_in("<title>offen"), None);
    }

    #[test]
    fn escapes_turn_back_into_characters() {
        assert_eq!(
            title_in("<title>Caf&eacute; &amp; Bar &#8211; G&#xe4;ste</title>"),
            Some("Café & Bar – Gäste".to_owned())
        );
        assert_eq!(unescape("a &amp;&lt;b&gt; &quot;c&quot;"), "a &<b> \"c\"");
        assert_eq!(unescape("100 & 200"), "100 & 200");
    }

    #[test]
    fn the_letters_of_latin_one_are_known_by_name() {
        assert_eq!(unescape("Sch&ouml;ne Gr&uuml;&szlig;e"), "Schöne Grüße");
        assert_eq!(unescape("&Auml;&Ouml;&Uuml; &agrave; &ntilde;"), "ÄÖÜ à ñ");
        assert_eq!(unescape("&yuml;"), "ÿ", "the last one of the row");
        assert_eq!(unescape("&garnix; bleibt"), "&garnix; bleibt");
    }

    #[test]
    fn the_space_that_does_not_break_is_a_space_too() {
        assert_eq!(
            title_in("<title>Ein&nbsp;Titel&nbsp;&nbsp;mit L&uuml;cken</title>"),
            Some("Ein Titel mit Lücken".to_owned())
        );
    }

    #[test]
    fn a_long_title_is_cut_short() {
        let long = "a".repeat(400);
        let cut = title_in(&format!("<title>{long}</title>")).unwrap();
        assert_eq!(cut.chars().count(), LONGEST + 1);
        assert!(cut.ends_with('…'));
    }
}
