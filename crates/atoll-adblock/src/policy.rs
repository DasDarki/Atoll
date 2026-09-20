//! Who is protected from what.
//!
//! Atoll protects a page in five ways, and some sites only work when one of them is off: a shop
//! whose payment opens a popup, a news site that notices the blocker. The user therefore decides
//! on three levels - everywhere, for one site, or for one single address - and the narrower level
//! wins. A level that says nothing passes the question on, so switching one thing off for one
//! address leaves everything else as it was.

use std::collections::HashMap;
use std::sync::{PoisonError, RwLock};

use crate::blocker::site_of;

/// One of the things Atoll does for a page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Protection {
    /// The one above the others: with this off, the page gets no protection at all.
    Active,
    /// Blocks advertising and trackers and hides what the filter lists know.
    Banners,
    /// Stops windows a page opens by itself.
    Popups,
    /// Stops a page from sending the user to another site without being asked.
    Redirects,
    /// Stops a page from opening a tab on another site by itself.
    NewTabs,
}

impl Protection {
    pub const ALL: [Protection; 5] = [
        Protection::Active,
        Protection::Banners,
        Protection::Popups,
        Protection::Redirects,
        Protection::NewTabs,
    ];

    /// The name the protection carries in the database and in the user interface.
    pub fn key(self) -> &'static str {
        match self {
            Protection::Active => "active",
            Protection::Banners => "banners",
            Protection::Popups => "popups",
            Protection::Redirects => "redirects",
            Protection::NewTabs => "new-tabs",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Protection::ALL
            .into_iter()
            .find(|protection| protection.key() == key)
    }

    /// What Atoll does where nobody said anything. Sending the user away and opening tabs is
    /// what many ordinary sites do as well, so those two are only stopped where it is asked for.
    pub fn by_default(self) -> bool {
        !matches!(self, Protection::Redirects | Protection::NewTabs)
    }
}

/// What one rule says, as far as it says anything.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Protections {
    pub active: Option<bool>,
    pub banners: Option<bool>,
    pub popups: Option<bool>,
    pub redirects: Option<bool>,
    pub new_tabs: Option<bool>,
}

impl Protections {
    pub fn get(self, protection: Protection) -> Option<bool> {
        match protection {
            Protection::Active => self.active,
            Protection::Banners => self.banners,
            Protection::Popups => self.popups,
            Protection::Redirects => self.redirects,
            Protection::NewTabs => self.new_tabs,
        }
    }

    pub fn set(&mut self, protection: Protection, value: Option<bool>) {
        let slot = match protection {
            Protection::Active => &mut self.active,
            Protection::Banners => &mut self.banners,
            Protection::Popups => &mut self.popups,
            Protection::Redirects => &mut self.redirects,
            Protection::NewTabs => &mut self.new_tabs,
        };
        *slot = value;
    }

    pub fn with(mut self, protection: Protection, value: Option<bool>) -> Self {
        self.set(protection, value);
        self
    }

    /// Whether the rule decides nothing and may as well be thrown away.
    pub fn says_nothing(self) -> bool {
        Protection::ALL
            .into_iter()
            .all(|protection| self.get(protection).is_none())
    }

    /// Lays `self` over `under`: what this rule says counts, the rest stays as it was.
    fn over(self, under: Protections) -> Protections {
        let mut result = under;
        for protection in Protection::ALL {
            if let Some(value) = self.get(protection) {
                result.set(protection, Some(value));
            }
        }
        result
    }
}

/// What a page actually gets, with every question answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Effective {
    pub active: bool,
    pub banners: bool,
    pub popups: bool,
    pub redirects: bool,
    pub new_tabs: bool,
}

impl Effective {
    pub fn get(self, protection: Protection) -> bool {
        match protection {
            Protection::Active => self.active,
            Protection::Banners => self.banners,
            Protection::Popups => self.popups,
            Protection::Redirects => self.redirects,
            Protection::NewTabs => self.new_tabs,
        }
    }

    /// Whether the protection is in force, which the one above the others can take away.
    pub fn does(self, protection: Protection) -> bool {
        self.active && self.get(protection)
    }
}

/// Where a rule applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Scope {
    /// Everywhere, unless a narrower rule says otherwise.
    Global,
    /// Every page of one site, see [`site_of`].
    Site,
    /// One address, see [`page_of`].
    Page,
}

impl Scope {
    pub const ALL: [Scope; 3] = [Scope::Global, Scope::Site, Scope::Page];

    pub fn key(self) -> &'static str {
        match self {
            Scope::Global => "global",
            Scope::Site => "site",
            Scope::Page => "page",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Scope::ALL.into_iter().find(|scope| scope.key() == key)
    }

    /// What a rule of this scope is written down under for `url`, `None` where it cannot apply.
    pub fn pattern_of(self, url: &str) -> Option<String> {
        match self {
            Scope::Global => Some(String::new()),
            Scope::Site => site_of(url),
            Scope::Page => page_of(url),
        }
    }
}

/// What the rules say, asked for every page and every window a page wants to open. Safe to share
/// between threads, because the network thread asks it too.
#[derive(Debug, Default)]
pub struct Policy {
    rules: RwLock<Rules>,
}

#[derive(Debug, Default)]
struct Rules {
    global: Protections,
    sites: HashMap<String, Protections>,
    pages: HashMap<String, Protections>,
}

impl Policy {
    pub fn new() -> Self {
        Self::default()
    }

    /// Writes down what `pattern` is to get. A rule that says nothing is removed.
    pub fn set(&self, scope: Scope, pattern: &str, protections: Protections) {
        let Ok(mut rules) = self.rules.write() else {
            return;
        };
        let table = match scope {
            Scope::Global => {
                rules.global = protections;
                return;
            }
            Scope::Site => &mut rules.sites,
            Scope::Page => &mut rules.pages,
        };
        if protections.says_nothing() {
            table.remove(pattern);
        } else {
            table.insert(pattern.to_owned(), protections);
        }
    }

    /// What the rule of `scope` says about `url` itself, without the levels above it.
    pub fn protections(&self, scope: Scope, url: &str) -> Protections {
        let Ok(rules) = self.rules.read() else {
            return Protections::default();
        };
        match scope {
            Scope::Global => rules.global,
            Scope::Site => site_of(url)
                .and_then(|site| rules.sites.get(&site).copied())
                .unwrap_or_default(),
            Scope::Page => page_of(url)
                .and_then(|page| rules.pages.get(&page).copied())
                .unwrap_or_default(),
        }
    }

    /// What the page at `url` gets, after all three levels had their say.
    pub fn effective(&self, url: &str) -> Effective {
        self.settled(url, None)
    }

    /// What the page at `url` would get if the rule of `scope` said nothing, which is what a
    /// switch falls back to when it is left alone.
    pub fn inherited(&self, scope: Scope, url: &str) -> Effective {
        self.settled(url, Some(scope))
    }

    /// Lays the levels over each other, leaving out `without` and everything narrower.
    fn settled(&self, url: &str, without: Option<Scope>) -> Effective {
        let counts = |level: Scope| without.is_none_or(|narrowest| level < narrowest);
        let mut settled = Protections::default();
        if let Ok(rules) = self.rules.read() {
            if counts(Scope::Global) {
                settled = rules.global;
            }
            if counts(Scope::Site)
                && let Some(site) = site_of(url)
                && let Some(rule) = rules.sites.get(&site)
            {
                settled = rule.over(settled);
            }
            if counts(Scope::Page)
                && let Some(page) = page_of(url)
                && let Some(rule) = rules.pages.get(&page)
            {
                settled = rule.over(settled);
            }
        }
        Effective {
            active: settled
                .active
                .unwrap_or_else(|| Protection::Active.by_default()),
            banners: settled
                .banners
                .unwrap_or_else(|| Protection::Banners.by_default()),
            popups: settled
                .popups
                .unwrap_or_else(|| Protection::Popups.by_default()),
            redirects: settled
                .redirects
                .unwrap_or_else(|| Protection::Redirects.by_default()),
            new_tabs: settled
                .new_tabs
                .unwrap_or_else(|| Protection::NewTabs.by_default()),
        }
    }

    /// The sites and addresses that have rules of their own, for the settings.
    pub fn exceptions(&self) -> Vec<(Scope, String)> {
        let rules = self.rules.read().unwrap_or_else(PoisonError::into_inner);
        let mut found: Vec<(Scope, String)> = rules
            .sites
            .keys()
            .map(|site| (Scope::Site, site.clone()))
            .chain(rules.pages.keys().map(|page| (Scope::Page, page.clone())))
            .collect();
        found.sort();
        found
    }

    /// Throws away every rule but the one that counts everywhere.
    pub fn forget_exceptions(&self) {
        if let Ok(mut rules) = self.rules.write() {
            rules.sites.clear();
            rules.pages.clear();
        }
    }
}

/// The address a rule for one single page is written down under: the address without the part
/// after `#`, which only moves within the page.
pub fn page_of(url: &str) -> Option<String> {
    let mut parsed = url::Url::parse(url).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    parsed.set_fragment(None);
    Some(parsed.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHOP: &str = "https://shop.example.org/kasse?schritt=2";

    fn policy() -> Policy {
        Policy::new()
    }

    #[test]
    fn without_a_rule_a_page_gets_what_atoll_does_anyway() {
        let effective = policy().effective(SHOP);
        assert_eq!(
            effective,
            Effective {
                active: true,
                banners: true,
                popups: true,
                redirects: false,
                new_tabs: false,
            },
            "sending the user away happens on ordinary sites too, so it is only stopped on request"
        );
    }

    #[test]
    fn the_narrower_rule_wins_and_the_rest_stays() {
        let policy = policy();
        policy.set(
            Scope::Global,
            "",
            Protections::default().with(Protection::Redirects, Some(true)),
        );
        policy.set(
            Scope::Site,
            "shop.example.org",
            Protections::default().with(Protection::Popups, Some(false)),
        );
        policy.set(
            Scope::Page,
            &page_of(SHOP).unwrap(),
            Protections::default().with(Protection::Redirects, Some(false)),
        );
        let effective = policy.effective(SHOP);
        assert!(!effective.popups, "the site may open its windows");
        assert!(
            !effective.redirects,
            "the address says more than everywhere"
        );
        assert!(effective.banners, "nobody said anything about advertising");
        assert!(
            policy
                .effective("https://shop.example.org/anderes")
                .redirects,
            "another page of the site keeps what counts everywhere"
        );
    }

    #[test]
    fn the_one_above_the_others_takes_everything_away() {
        let policy = policy();
        policy.set(
            Scope::Site,
            "shop.example.org",
            Protections::default().with(Protection::Active, Some(false)),
        );
        let effective = policy.effective(SHOP);
        assert!(effective.banners, "the switch itself stays as it was");
        assert!(
            !effective.does(Protection::Banners) && !effective.does(Protection::Popups),
            "but nothing of it is in force"
        );
    }

    #[test]
    fn what_a_level_falls_back_to_is_what_the_wider_levels_say() {
        let policy = policy();
        policy.set(
            Scope::Global,
            "",
            Protections::default().with(Protection::Popups, Some(false)),
        );
        policy.set(
            Scope::Site,
            "shop.example.org",
            Protections::default().with(Protection::Popups, Some(true)),
        );
        assert!(policy.effective(SHOP).popups);
        assert!(
            policy.inherited(Scope::Page, SHOP).popups,
            "an address without a rule of its own follows the site"
        );
        assert!(
            !policy.inherited(Scope::Site, SHOP).popups,
            "the site without a rule of its own follows what counts everywhere"
        );
        assert!(
            policy.inherited(Scope::Global, SHOP).popups,
            "and that falls back to what Atoll does anyway"
        );
    }

    #[test]
    fn a_rule_that_says_nothing_is_not_kept() {
        let policy = policy();
        policy.set(
            Scope::Site,
            "shop.example.org",
            Protections::default().with(Protection::Popups, Some(false)),
        );
        assert_eq!(
            policy.exceptions(),
            vec![(Scope::Site, "shop.example.org".into())]
        );
        policy.set(Scope::Site, "shop.example.org", Protections::default());
        assert!(policy.exceptions().is_empty());
    }

    #[test]
    fn an_address_is_remembered_without_what_only_moves_within_the_page() {
        assert_eq!(
            page_of("https://example.org/a?b=c#teil"),
            Some("https://example.org/a?b=c".to_owned())
        );
        assert_eq!(page_of("about:blank"), None);
        assert_eq!(page_of("not an address"), None);
    }

    #[test]
    fn pages_that_belong_to_nothing_still_get_what_counts_everywhere() {
        let policy = policy();
        policy.set(
            Scope::Global,
            "",
            Protections::default().with(Protection::Banners, Some(false)),
        );
        assert!(!policy.effective("about:blank").banners);
    }

    #[test]
    fn every_protection_is_known_by_its_name() {
        for protection in Protection::ALL {
            assert_eq!(Protection::from_key(protection.key()), Some(protection));
        }
        for scope in Scope::ALL {
            assert_eq!(Scope::from_key(scope.key()), Some(scope));
        }
        assert_eq!(Protection::from_key("wind"), None);
    }
}
