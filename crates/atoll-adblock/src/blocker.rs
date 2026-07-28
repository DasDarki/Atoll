use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use adblock::Engine;
use adblock::request::Request;

use crate::cosmetic;

/// How many pages the element exceptions are remembered for.
const REMEMBERED_PAGES: usize = 16;

/// What hides elements on a page.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Cosmetics {
    /// Complete CSS rules for the page.
    pub css: String,
    /// Script the filter lists inject into the page.
    pub script: String,
    /// Whether generic rules apply, which depend on the class names and ids on the page, see
    /// [`Blocker::element_css`].
    pub generic: bool,
}

/// What a page excludes from the generic rules.
struct PageExceptions {
    page_url: String,
    generic_hidden: bool,
    exceptions: HashSet<String>,
}

/// Checks requests and pages against the filter lists. Safe to share between threads.
pub struct Blocker {
    engine: RwLock<Option<Arc<Engine>>>,
    enabled: AtomicBool,
    allowed_sites: RwLock<HashSet<String>>,
    pages: Mutex<VecDeque<PageExceptions>>,
}

impl Blocker {
    /// A blocker without filter lists yet, see [`crate::Updater`]. `allowed_sites` are sites the
    /// user switched filtering off for.
    pub fn new(enabled: bool, allowed_sites: impl IntoIterator<Item = String>) -> Self {
        Self {
            engine: RwLock::new(None),
            enabled: AtomicBool::new(enabled),
            allowed_sites: RwLock::new(allowed_sites.into_iter().collect()),
            pages: Mutex::default(),
        }
    }

    /// Whether filter lists are loaded.
    pub fn is_ready(&self) -> bool {
        self.engine().is_some()
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    /// Whether filtering is switched off for `site`, see [`site_of`].
    pub fn is_site_allowed(&self, site: &str) -> bool {
        self.allowed_sites
            .read()
            .is_ok_and(|sites| sites.contains(site))
    }

    pub fn set_site_allowed(&self, site: &str, allowed: bool) {
        if let Ok(mut sites) = self.allowed_sites.write() {
            if allowed {
                sites.insert(site.to_owned());
            } else {
                sites.remove(site);
            }
        }
    }

    /// Whether requests and elements of the page at `page_url` are filtered.
    pub fn filters_page(&self, page_url: &str) -> bool {
        self.is_enabled() && !site_of(page_url).is_some_and(|site| self.is_site_allowed(&site))
    }

    /// Whether the request for `url` from the document at `source_url` on the page at `page_url`
    /// is blocked. `request_type` is a type of the filter syntax such as `script` or `image`.
    pub fn blocks(&self, url: &str, source_url: &str, page_url: &str, request_type: &str) -> bool {
        if !self.filters_page(page_url) {
            return false;
        }
        let Some(engine) = self.engine() else {
            return false;
        };
        let source_url = if source_url.is_empty() {
            page_url
        } else {
            source_url
        };
        Request::new(url, source_url, request_type, "get")
            .is_ok_and(|request| engine.check_network_request(&request).should_block())
    }

    /// The rules hiding elements on the page at `page_url`.
    pub fn cosmetics(&self, page_url: &str) -> Cosmetics {
        if !self.filters_page(page_url) {
            return Cosmetics::default();
        }
        let Some(engine) = self.engine() else {
            return Cosmetics::default();
        };
        let resources = engine.url_cosmetic_resources(page_url);
        let mut css = cosmetic::hide_rules(&resources.hide_selectors);
        css.push_str(&cosmetic::procedural_rules(&resources.procedural_actions));
        let generic = !resources.generichide;
        self.remember_page(page_url, resources.generichide, resources.exceptions);
        Cosmetics {
            css,
            script: resources.injected_script,
            generic,
        }
    }

    /// CSS hiding the elements with the given class names and ids on the page at `page_url`
    /// that generic rules want hidden.
    pub fn element_css(&self, page_url: &str, classes: &[String], ids: &[String]) -> String {
        if !self.filters_page(page_url) {
            return String::new();
        }
        let Some(engine) = self.engine() else {
            return String::new();
        };
        let selectors = self.with_page(page_url, |page| {
            if page.generic_hidden {
                Vec::new()
            } else {
                engine.hidden_class_id_selectors(classes, ids, &page.exceptions)
            }
        });
        let selectors = selectors.unwrap_or_else(|| {
            let resources = engine.url_cosmetic_resources(page_url);
            let selectors = if resources.generichide {
                Vec::new()
            } else {
                engine.hidden_class_id_selectors(classes, ids, &resources.exceptions)
            };
            self.remember_page(page_url, resources.generichide, resources.exceptions);
            selectors
        });
        cosmetic::hide_rules(&selectors)
    }

    pub(crate) fn replace_engine(&self, engine: Engine) {
        if let Ok(mut slot) = self.engine.write() {
            *slot = Some(Arc::new(engine));
        }
        if let Ok(mut pages) = self.pages.lock() {
            pages.clear();
        }
    }

    fn engine(&self) -> Option<Arc<Engine>> {
        self.engine.read().ok().and_then(|engine| engine.clone())
    }

    fn remember_page(&self, page_url: &str, generic_hidden: bool, exceptions: HashSet<String>) {
        if let Ok(mut pages) = self.pages.lock() {
            pages.retain(|known| known.page_url != page_url);
            if pages.len() == REMEMBERED_PAGES {
                pages.pop_front();
            }
            pages.push_back(PageExceptions {
                page_url: page_url.to_owned(),
                generic_hidden,
                exceptions,
            });
        }
    }

    /// Runs `action` with the remembered exceptions of `page_url`, `None` if they are unknown.
    fn with_page<R>(&self, page_url: &str, action: impl FnOnce(&PageExceptions) -> R) -> Option<R> {
        let pages = self.pages.lock().ok()?;
        pages
            .iter()
            .find(|page| page.page_url == page_url)
            .map(action)
    }
}

/// The site `url` belongs to, as used for switching filtering off: its host without `www.`.
pub fn site_of(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    let host = parsed.host_str()?.to_ascii_lowercase();
    Some(host.strip_prefix("www.").map(str::to_owned).unwrap_or(host))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lists::build_engine;

    const LIST: &str = "[Adblock Plus 2.0]
! Title: Test
||ads.example.net^
||tracker.example.com^$third-party
@@||ads.example.net/allowed.js
##.ad-banner
example.org##.local-ad
news.example##.box:style(margin: 0)
@@||quiet.example^$generichide
";

    fn blocker() -> Blocker {
        let blocker = Blocker::new(true, ["allowed.example".to_owned()]);
        blocker.replace_engine(build_engine(vec![LIST.to_owned()]));
        blocker
    }

    fn blocks(blocker: &Blocker, url: &str, page: &str) -> bool {
        blocker.blocks(url, page, page, "script")
    }

    #[test]
    fn nothing_is_blocked_before_the_lists_are_loaded() {
        let blocker = Blocker::new(true, []);
        assert!(!blocker.is_ready());
        assert!(!blocks(
            &blocker,
            "https://ads.example.net/ad.js",
            "https://example.org/"
        ));
        assert_eq!(
            blocker.cosmetics("https://example.org/"),
            Cosmetics::default()
        );
    }

    #[test]
    fn requests_follow_the_rules_and_their_exceptions() {
        let blocker = blocker();
        assert!(blocker.is_ready());
        assert!(blocks(
            &blocker,
            "https://ads.example.net/ad.js",
            "https://example.org/"
        ));
        assert!(!blocks(
            &blocker,
            "https://ads.example.net/allowed.js",
            "https://example.org/"
        ));
        assert!(!blocks(
            &blocker,
            "https://example.org/app.js",
            "https://example.org/"
        ));
        assert!(blocks(
            &blocker,
            "https://tracker.example.com/t.js",
            "https://example.org/"
        ));
        assert!(!blocks(
            &blocker,
            "https://tracker.example.com/t.js",
            "https://tracker.example.com/"
        ));
    }

    #[test]
    fn allowed_sites_and_the_switch_turn_blocking_off() {
        let blocker = blocker();
        assert!(!blocks(
            &blocker,
            "https://ads.example.net/ad.js",
            "https://www.allowed.example/"
        ));
        assert!(!blocker.filters_page("https://allowed.example/page"));

        blocker.set_site_allowed("example.org", true);
        assert!(!blocks(
            &blocker,
            "https://ads.example.net/ad.js",
            "https://example.org/"
        ));
        blocker.set_site_allowed("example.org", false);
        assert!(blocks(
            &blocker,
            "https://ads.example.net/ad.js",
            "https://example.org/"
        ));

        blocker.set_enabled(false);
        assert!(!blocks(
            &blocker,
            "https://ads.example.net/ad.js",
            "https://example.org/"
        ));
        assert_eq!(
            blocker.element_css("https://example.org/", &["ad-banner".to_owned()], &[]),
            ""
        );
    }

    #[test]
    fn pages_get_their_specific_and_generic_rules() {
        let blocker = blocker();
        let cosmetics = blocker.cosmetics("https://example.org/");
        assert!(
            cosmetics
                .css
                .contains(".local-ad { display: none !important; }")
        );
        assert!(cosmetics.generic);
        assert!(
            !blocker
                .cosmetics("https://other.example/")
                .css
                .contains(".local-ad")
        );
        assert!(
            blocker
                .cosmetics("https://news.example/")
                .css
                .contains(".box { margin: 0 }")
        );

        let css = blocker.element_css(
            "https://example.org/",
            &["ad-banner".to_owned(), "content".to_owned()],
            &[],
        );
        assert_eq!(css, ".ad-banner { display: none !important; }\n");
        assert_eq!(
            blocker.element_css("https://unseen.example/", &["ad-banner".to_owned()], &[]),
            ".ad-banner { display: none !important; }\n"
        );
    }

    #[test]
    fn generichide_pages_get_no_generic_rules() {
        let blocker = blocker();
        assert!(!blocker.cosmetics("https://quiet.example/").generic);
        assert_eq!(
            blocker.element_css("https://quiet.example/", &["ad-banner".to_owned()], &[]),
            ""
        );
    }

    #[test]
    fn sites_are_hosts_without_www() {
        assert_eq!(
            site_of("https://www.Example.org/path"),
            Some("example.org".to_owned())
        );
        assert_eq!(
            site_of("http://sub.example.org:8080/"),
            Some("sub.example.org".to_owned())
        );
        assert_eq!(site_of("data:text/html,hi"), None);
        assert_eq!(site_of("not a url"), None);
    }
}
