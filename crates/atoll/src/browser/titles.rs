//! What a page that was opened but not looked at yet is called, and what its icon is.
//!
//! A tab opened with the middle mouse button stays a record: nothing is loaded, no engine runs
//! for it, and the sidebar can only show its address. That address says little, so Atoll asks the
//! site for the beginning of the page, reads the title out of its head and shows that instead.
//!
//! Links are often not the page itself but something that sends you on: a search result, a short
//! address, a click counter. Atoll follows that, both the redirects of the server and the ones
//! written into the head of a page, and the tab then points where the link actually leads. Where
//! that is another site, the address of the tab is written anew, because otherwise the name of
//! the page and the icon beside it would belong to two different sites.
//!
//! It is the smallest request that answers the question: the first few kilobytes, never the whole
//! page, no pictures, no scripts, and the connection is dropped once the head is through. It goes
//! the same way as everything else Atoll fetches, so a VPN stays in front of it, and private tabs
//! are left out entirely - what they do belongs to them.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::time::Duration;

use atoll_adblock::site_of;
use atoll_core::TabId;
use atoll_core::address::host_of;
use atoll_storage::prelude::*;
use ureq::ResponseExt;

use super::Browser;
use crate::platform::tasks::run_in_background;

/// How long the site has to answer.
const TIMEOUT: Duration = Duration::from_secs(8);

/// How much of a page is read. A head that has not named the page by then is no head of ours.
const ENOUGH: u64 = 128 * 1024;

/// How much of an icon is read. Anything larger is not a small picture next to a tab.
const ENOUGH_ICON: u64 = 512 * 1024;

/// How wide and high a fetched icon is kept, which is twice what the sidebar shows it at.
const ICON_SIZE: u32 = 32;

/// How often a page may send Atoll on with a line in its head. The redirects of the servers
/// themselves are followed by the agent and do not count against this.
const HOPS: usize = 3;

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
            move || page_of(&asked),
            move |page| {
                let Some(browser) = this.upgrade() else {
                    return;
                };
                browser.titles.borrow_mut().running -= 1;
                if let Some(page) = page {
                    browser.page_answered(tab, &url, page);
                }
                browser.ask_about_titles();
            },
        );
        true
    }

    /// The site answered. The answer is only taken if the tab is still there, still shows that
    /// address and still has no title of its own: a page that was opened meanwhile knows better.
    fn page_answered(&self, tab: TabId, asked: &str, page: Page) {
        // A link that leads to another site makes the tab point there, so that what the tab is
        // called and the icon beside it belong to the same place.
        let moved = site_of(asked) != site_of(&page.url);
        {
            let mut session = self.session.borrow_mut();
            let Some(record) = session.tab_mut(tab) else {
                return;
            };
            if record.url != asked
                || record
                    .title
                    .as_deref()
                    .is_some_and(|known| !known.is_empty())
            {
                return;
            }
            record.title = Some(page.title.clone());
            if moved {
                record.url = page.url.clone();
            }
        }
        let stored = self
            .storage
            .update_tab_title(tab, Some(&page.title))
            .and_then(|()| {
                if moved {
                    self.storage.update_tab_address(tab, &page.url)
                } else {
                    Ok(())
                }
            });
        if let Err(error) = stored {
            tracing::warn!(%error, "what a page is called could not be stored");
        }
        tracing::debug!(
            tab = tab.0,
            title = page.title,
            url = page.url,
            "a page said what it is called"
        );
        self.refresh_sidebar();
        self.for_other_windows(Browser::refresh_sidebar);
        self.fetch_icon(&page);
    }

    /// Fetches the icon a page names, for a site that has none here yet. The sidebar shows the
    /// icon of a host, so one page of a site gives every tab of it its icon.
    fn fetch_icon(&self, page: &Page) {
        let Some(address) = page.icon.clone() else {
            return;
        };
        let Some(host) = host_of(&page.url) else {
            return;
        };
        if self.favicons.borrow().knows_host(&host) {
            return;
        }
        let this = self.this();
        run_in_background(
            "icon",
            move || icon_of(&address),
            move |png| {
                let Some(browser) = this.upgrade() else {
                    return;
                };
                if let Some(png) = png {
                    browser.icon_arrived(&host, &png);
                }
            },
        );
    }

    /// The icon of a site arrived, and every tab of that site shows it from now on.
    fn icon_arrived(&self, host: &str, png: &[u8]) {
        match self.storage.upsert_favicon(host, png, Self::now()) {
            Ok(favicon) => {
                self.favicons
                    .borrow_mut()
                    .insert(favicon, host.to_owned(), png);
                tracing::debug!(host, "a site gave its icon");
                self.refresh_sidebar();
            }
            Err(error) => tracing::warn!(%error, "the icon of a site could not be stored"),
        }
    }
}

/// What a page said about itself.
struct Page {
    /// Where the question ended up, which is where the tab points afterwards.
    url: String,
    /// What the page is called.
    title: String,
    /// The address of the icon the page names, if it names one.
    icon: Option<String>,
}

/// Fetches the beginning of the page at `url` and reads it, following whatever sends Atoll on.
/// Runs away from the window.
fn page_of(url: &str) -> Option<Page> {
    let mut asked = url.to_owned();
    for _ in 0..HOPS {
        let (landed, body) = head_of(&asked)?;
        // A head that only says "go there instead" is not the head of the page that was meant.
        if let Some(next) = sent_on(&body, &landed).filter(|next| *next != landed) {
            asked = next;
            continue;
        }
        return Some(Page {
            title: title_in(&body)?,
            icon: icon_in(&body, &landed),
            url: landed,
        });
    }
    None
}

/// The beginning of the page at `url`: where the request ended up, and what came back.
fn head_of(url: &str) -> Option<(String, String)> {
    let mut answer = atoll_net::agent(TIMEOUT)
        .get(url)
        // Only the page itself is wanted, and only its beginning.
        .header("Accept", "text/html,application/xhtml+xml")
        .header("Range", "bytes=0-131071")
        .call()
        .ok()?;
    // The redirects of the servers are followed by the agent, so this is where it all ended.
    let landed = answer.get_uri().to_string();
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
    Some((landed, String::from_utf8_lossy(&body).into_owned()))
}

/// Where the head of a page sends Atoll on, if it does: the line that loads another address by
/// itself. A page that waits before it goes wants to be read first, so only the ones that leave
/// at once are followed.
fn sent_on(html: &str, base: &str) -> Option<String> {
    for tag in tags_in_head(html, "meta") {
        let refreshing = attribute(&tag, "http-equiv")
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("refresh"));
        if !refreshing {
            continue;
        }
        let Some(content) = attribute(&tag, "content") else {
            continue;
        };
        let Some((delay, target)) = content.split_once(';') else {
            continue;
        };
        if delay
            .trim()
            .parse::<f32>()
            .is_ok_and(|seconds| seconds > 3.0)
        {
            continue;
        }
        let mut target = target.trim();
        if target.len() >= 3 && target[..3].eq_ignore_ascii_case("url") {
            target = target[3..]
                .trim_start()
                .strip_prefix('=')
                .unwrap_or("")
                .trim();
        }
        let target = target.trim_matches(['"', '\'']);
        if let Some(address) = absolute(target, base) {
            return Some(address);
        }
    }
    None
}

/// The icon a page names in its head. A site that names none still has the one at its root,
/// which is where every browser looks.
fn icon_in(html: &str, base: &str) -> Option<String> {
    let mut best: Option<(u8, String)> = None;
    for tag in tags_in_head(html, "link") {
        let Some(rel) = attribute(&tag, "rel") else {
            continue;
        };
        let rel = rel.to_ascii_lowercase();
        let rank = if rel.split_whitespace().any(|word| word == "icon") {
            2
        } else if rel.contains("apple-touch-icon") {
            1
        } else {
            continue;
        };
        let Some(address) = attribute(&tag, "href").and_then(|href| absolute(href.trim(), base))
        else {
            continue;
        };
        if best.as_ref().is_none_or(|(known, _)| *known < rank) {
            best = Some((rank, address));
        }
    }
    if let Some((_, address)) = best {
        return Some(address);
    }
    absolute("/favicon.ico", base)
}

/// `target` as a complete address, read against the page it stands on. Only what a page is
/// fetched over counts, so nothing else can be asked for by a line in a head.
fn absolute(target: &str, base: &str) -> Option<String> {
    if target.is_empty() {
        return None;
    }
    let address = url::Url::parse(base).ok()?.join(target).ok()?;
    if !matches!(address.scheme(), "http" | "https") {
        return None;
    }
    Some(address.to_string())
}

/// The insides of every `<name …>` tag in the head of a page, without the brackets.
fn tags_in_head(html: &str, name: &str) -> Vec<String> {
    let lower = html.to_ascii_lowercase();
    let head = lower.find("</head>").unwrap_or(lower.len());
    let opening = format!("<{name}");
    let mut tags = Vec::new();
    let mut at = 0;
    while at < head
        && let Some(found) = lower[at..head].find(&opening)
    {
        let after = at + found + opening.len();
        let Some(end) = lower[after..head].find('>') else {
            break;
        };
        let end = after + end;
        // What follows the name has to end it, or this is another tag that starts the same way.
        if lower
            .as_bytes()
            .get(after)
            .is_some_and(|byte| byte.is_ascii_whitespace() || *byte == b'/' || *byte == b'>')
        {
            tags.push(html[after..end].to_owned());
        }
        at = end + 1;
    }
    tags
}

/// The value of the attribute `name` of a tag, written with quotes of either kind or without.
fn attribute(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut at = 0;
    loop {
        let start = at + lower[at..].find(name)?;
        at = start + name.len();
        // The name has to stand on its own, and what follows it has to be its value.
        let before = start
            .checked_sub(1)
            .and_then(|before| lower.as_bytes().get(before).copied());
        if before.is_some_and(|byte| !byte.is_ascii_whitespace() && byte != b'/') {
            continue;
        }
        let behind = lower[at..].trim_start();
        if !behind.starts_with('=') {
            continue;
        }
        let value = tag[at + (lower[at..].len() - behind.len()) + 1..].trim_start();
        let text = match value.as_bytes().first() {
            Some(b'"') => value[1..].split('"').next()?,
            Some(b'\'') => value[1..].split('\'').next()?,
            _ => value.split([' ', '\t', '\n', '\r']).next()?,
        };
        return Some(text.to_owned());
    }
}

/// The icon at `url` as PNG bytes, small enough to stand next to a tab.
fn icon_of(url: &str) -> Option<Vec<u8>> {
    let mut answer = atoll_net::agent(TIMEOUT).get(url).call().ok()?;
    let bytes = answer
        .body_mut()
        .with_config()
        .limit(ENOUGH_ICON)
        .read_to_vec()
        .ok()?;
    let reader = image::ImageReader::new(std::io::Cursor::new(&bytes))
        .with_guessed_format()
        .ok()?;
    // A picture of a size nobody needs for an icon is not unpacked at all.
    let (width, height) = reader.into_dimensions().ok()?;
    if width > 1024 || height > 1024 {
        return None;
    }
    let icon = image::load_from_memory(&bytes).ok()?;
    let icon = if icon.width() > ICON_SIZE || icon.height() > ICON_SIZE {
        icon.resize(ICON_SIZE, ICON_SIZE, image::imageops::FilterType::Lanczos3)
    } else {
        icon
    };
    let mut png = Vec::new();
    icon.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .ok()?;
    Some(png)
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
    fn a_head_that_sends_on_is_followed() {
        let notice = "<html><head><title>Weiterleitungshinweis</title>\
                      <meta http-equiv=\"refresh\" content=\"0;URL='https://example.org/artikel'\">\
                      </head><body>Weiter</body></html>";
        assert_eq!(
            sent_on(notice, "https://www.google.com/url?q=x"),
            Some("https://example.org/artikel".to_owned())
        );
    }

    #[test]
    fn a_head_that_sends_on_later_is_left_alone() {
        let waiting = "<head><meta http-equiv=refresh content=\"30; url=/weiter\"></head>";
        assert_eq!(sent_on(waiting, "https://example.org/a/b"), None);
    }

    #[test]
    fn a_relative_target_is_read_against_the_page() {
        let html = "<head><meta http-equiv='Refresh' content='0; url=../weiter.html'></head>";
        assert_eq!(
            sent_on(html, "https://example.org/a/b/c"),
            Some("https://example.org/a/weiter.html".to_owned())
        );
    }

    #[test]
    fn a_page_that_stays_says_nothing() {
        assert_eq!(
            sent_on("<head><title>Hier</title></head>", "https://a.test/"),
            None
        );
        let other = "<head><meta http-equiv=\"content-type\" content=\"text/html\"></head>";
        assert_eq!(sent_on(other, "https://a.test/"), None);
        let in_the_body =
            "<head></head><body><meta http-equiv=refresh content=\"0;url=/x\"></body>";
        assert_eq!(sent_on(in_the_body, "https://a.test/"), None);
    }

    #[test]
    fn the_icon_of_a_page_comes_out_of_its_head() {
        let html = "<head><link rel=\"apple-touch-icon\" href=\"/gross.png\">\
                    <link rel=\"shortcut icon\" href=\"bild/klein.png\"></head>";
        assert_eq!(
            icon_in(html, "https://example.org/a/b"),
            Some("https://example.org/a/bild/klein.png".to_owned()),
            "the icon of the page itself counts for more than the one for a home screen"
        );
        assert_eq!(
            icon_in(
                "<head><link rel=apple-touch-icon href=/t.png></head>",
                "https://example.org/"
            ),
            Some("https://example.org/t.png".to_owned())
        );
        assert_eq!(
            icon_in("<head></head>", "https://example.org/a/b?c=d"),
            Some("https://example.org/favicon.ico".to_owned()),
            "a site that names none still has the one at its root"
        );
    }

    #[test]
    fn an_attribute_is_read_however_it_is_written() {
        assert_eq!(attribute(" href=\"/a\"", "href"), Some("/a".to_owned()));
        assert_eq!(attribute(" href='/a b'", "href"), Some("/a b".to_owned()));
        assert_eq!(attribute(" href=/a ", "href"), Some("/a".to_owned()));
        assert_eq!(attribute(" href = \"/a\"", "href"), Some("/a".to_owned()));
        assert_eq!(
            attribute(" REL=\"icon\" HREF=\"/a\"", "href"),
            Some("/a".to_owned())
        );
        assert_eq!(attribute(" data-href=\"/a\"", "href"), None, "another name");
        assert_eq!(attribute(" href", "href"), None, "without a value");
    }

    #[test]
    fn only_the_tags_of_the_head_are_read() {
        let html = "<head><metadata x=1><meta a=1><meta b=2></head><meta c=3>";
        let tags = tags_in_head(html, "meta");
        assert_eq!(tags, vec![" a=1", " b=2"]);
    }

    #[test]
    fn a_long_title_is_cut_short() {
        let long = "a".repeat(400);
        let cut = title_in(&format!("<title>{long}</title>")).unwrap();
        assert_eq!(cut.chars().count(), LONGEST + 1);
        assert!(cut.ends_with('…'));
    }
}
