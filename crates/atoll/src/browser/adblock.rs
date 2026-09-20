use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use atoll_adblock::{
    Blocker, COOKIE_LISTS, FILTER_LISTS, ListState, ListStatus, Policy, Protection, Protections,
    Scope, Updater, site_of,
};
use atoll_engine::{ContentFilter, DocumentRules, Opening, ResourceKind, ResourceRequest, Verdict};
use atoll_storage::AdblockRule;
use atoll_storage::prelude::*;
use slint::ComponentHandle;

use super::Browser;
use super::preferences::PreferenceKey;
use crate::ui::{OverlayState, SettingsState};
use atoll_i18n::t;

thread_local! {
    static STATUS_LISTENER: RefCell<Option<Box<dyn Fn()>>> = const { RefCell::new(None) };
}

/// The ad blocker all windows share, together with what it does about cookie banners.
pub struct Adblock {
    /// What the user decided, everywhere, per site and per address.
    pub(super) policy: Arc<Policy>,
    blocker: Arc<Blocker>,
    /// A blocker of its own for the list that hides cookie banners, so hiding them does not
    /// depend on whether advertising is blocked.
    cookies: Arc<Blocker>,
    /// Whether the cookie banner list is used, which is a setting of its own.
    hide_cookie_banners: Arc<AtomicBool>,
    /// Whether a banner Atoll recognises is answered with a refusal.
    answer_banners: Arc<AtomicBool>,
    updater: RefCell<Option<Updater>>,
    banner_updater: RefCell<Option<Updater>>,
    directory: PathBuf,
}

impl Adblock {
    pub fn new(
        rules: Vec<AdblockRule>,
        directory: PathBuf,
        hide_banners: bool,
        answer_banners: bool,
    ) -> Self {
        let policy = Arc::new(Policy::new());
        for rule in &rules {
            let (Some(scope), protections) = (Scope::from_key(&rule.scope), protections_of(rule))
            else {
                continue;
            };
            policy.set(scope, &rule.pattern, protections);
        }
        let hide_cookie_banners = Arc::new(AtomicBool::new(hide_banners));
        let blocker = {
            let policy = policy.clone();
            Blocker::new(move |url| policy.effective(url).does(Protection::Banners))
        };
        let cookies = {
            let policy = policy.clone();
            let hide = hide_cookie_banners.clone();
            Blocker::new(move |url| hide.load(Ordering::Relaxed) && policy.effective(url).active)
        };
        Self {
            policy,
            blocker: Arc::new(blocker),
            cookies: Arc::new(cookies),
            hide_cookie_banners,
            answer_banners: Arc::new(AtomicBool::new(answer_banners)),
            updater: RefCell::new(None),
            banner_updater: RefCell::new(None),
            directory,
        }
    }

    /// The directory of the cookie banner list, which is kept apart from the blocking lists.
    fn banner_directory(&self) -> PathBuf {
        self.directory.join("cookies")
    }

    /// Starts loading and updating the cookie banner list, unless that already happened.
    fn start_banner_lists(&self) {
        if self.banner_updater.borrow().is_some() {
            return;
        }
        let updater = Updater::start_lists(
            self.cookies.clone(),
            self.banner_directory(),
            COOKIE_LISTS,
            |_| {
                let _ = slint::invoke_from_event_loop(|| {
                    STATUS_LISTENER.with_borrow(|listener| {
                        if let Some(listener) = listener {
                            listener();
                        }
                    });
                });
            },
        );
        *self.banner_updater.borrow_mut() = Some(updater);
    }

    fn banner_status(&self) -> Option<ListStatus> {
        self.banner_updater.borrow().as_ref().map(Updater::status)
    }

    /// Starts loading and updating the filter lists and returns the filter for the engine.
    /// `on_status` runs on the main thread whenever the state of the lists changes.
    pub fn start(&self, on_status: impl Fn() + 'static) -> Arc<dyn ContentFilter> {
        STATUS_LISTENER.with_borrow_mut(|listener| *listener = Some(Box::new(on_status)));
        if self.updater.borrow().is_none() {
            let updater = Updater::start(self.blocker.clone(), self.directory.clone(), |_| {
                let _ = slint::invoke_from_event_loop(|| {
                    STATUS_LISTENER.with_borrow(|listener| {
                        if let Some(listener) = listener {
                            listener();
                        }
                    });
                });
            });
            *self.updater.borrow_mut() = Some(updater);
        }
        if self.hide_cookie_banners.load(Ordering::Relaxed) {
            self.start_banner_lists();
        }
        Arc::new(AdblockFilter {
            policy: self.policy.clone(),
            blocker: self.blocker.clone(),
            cookies: self.cookies.clone(),
            answer_banners: self.answer_banners.clone(),
        })
    }

    fn status(&self) -> Option<ListStatus> {
        self.updater.borrow().as_ref().map(Updater::status)
    }
}

/// Connects the ad blocker to the engine.
struct AdblockFilter {
    policy: Arc<Policy>,
    blocker: Arc<Blocker>,
    cookies: Arc<Blocker>,
    answer_banners: Arc<AtomicBool>,
}

impl AdblockFilter {
    /// Whether the cookie banner of the page at `url` is answered. A page without protection is
    /// left to answer for itself.
    fn answers_banners(&self, url: &str) -> bool {
        self.answer_banners.load(Ordering::Relaxed) && self.policy.effective(url).active
    }
}

impl ContentFilter for AdblockFilter {
    fn verdict(&self, request: &ResourceRequest<'_>) -> Verdict {
        let kind = request_type(request.kind);
        let blocked = [&self.blocker, &self.cookies]
            .iter()
            .any(|blocker| blocker.blocks(request.url, request.source_url, request.page_url, kind));
        if blocked {
            Verdict::Block
        } else {
            Verdict::Allow
        }
    }

    fn document_rules(&self, url: &str) -> DocumentRules {
        let cosmetics = self.blocker.cosmetics(url);
        let banners = self.cookies.cosmetics(url);
        let mut script = cosmetics.script;
        script.push_str(&banners.script);
        if self.answers_banners(url) {
            script.push_str(atoll_consent::answer_script());
        }
        DocumentRules {
            stylesheet: cosmetics.css + &banners.css,
            script,
            observe_elements: cosmetics.generic || banners.generic,
        }
    }

    fn element_rules(&self, url: &str, classes: &[String], ids: &[String]) -> String {
        self.blocker.element_css(url, classes, ids) + &self.cookies.element_css(url, classes, ids)
    }

    fn allows(
        &self,
        opening: Opening,
        page_url: &str,
        target_url: &str,
        user_gesture: bool,
    ) -> bool {
        let protection = match opening {
            Opening::Popup => Protection::Popups,
            Opening::NewTab => Protection::NewTabs,
            Opening::Navigation => Protection::Redirects,
        };
        if !self.policy.effective(page_url).does(protection) {
            return true;
        }
        // A window a page opens is stopped whether or not a click set it off: the click usually
        // meant something else, and the engine stops the ones without a click by itself anyway.
        if opening == Opening::Popup {
            return false;
        }
        user_gesture || !leads_elsewhere(page_url, target_url)
    }
}

/// Whether the two addresses belong to different sites. A page that opens something of its own
/// is never in the way, the protections are about being sent somewhere else.
fn leads_elsewhere(page_url: &str, target_url: &str) -> bool {
    match (site_of(page_url), site_of(target_url)) {
        (Some(page), Some(target)) => page != target,
        (_, None) => false,
        (None, Some(_)) => false,
    }
}

/// What a stored rule says, in the words of the policy.
fn protections_of(rule: &AdblockRule) -> Protections {
    Protections {
        active: rule.active,
        banners: rule.banners,
        popups: rule.popups,
        redirects: rule.redirects,
        new_tabs: rule.new_tabs,
    }
}

/// The same the other way round, ready to be stored.
fn rule_of(scope: Scope, pattern: &str, protections: Protections) -> AdblockRule {
    AdblockRule {
        scope: scope.key().to_owned(),
        pattern: pattern.to_owned(),
        active: protections.active,
        banners: protections.banners,
        popups: protections.popups,
        redirects: protections.redirects,
        new_tabs: protections.new_tabs,
    }
}

/// What the settings say about the list that hides cookie banners.
fn cookie_banner_status(status: Option<ListStatus>) -> String {
    match status.map(|status| status.state) {
        None => t!("Die Liste wird geladen, sobald das Verstecken eingeschaltet ist").to_owned(),
        Some(ListState::Loading) => t!("Die Liste wird geladen…").to_owned(),
        Some(ListState::Failed { .. }) => {
            t!("Die Liste konnte nicht geladen werden, Atoll versucht es weiter").to_owned()
        }
        Some(ListState::Ready | ListState::Updating) => {
            t!("Versteckt die Banner, die die EasyList-Cookie-Liste kennt").to_owned()
        }
    }
}

/// The request type of the filter syntax.
fn request_type(kind: ResourceKind) -> &'static str {
    match kind {
        ResourceKind::Document => "document",
        ResourceKind::Subframe => "sub_frame",
        ResourceKind::Stylesheet => "stylesheet",
        ResourceKind::Script | ResourceKind::Worker => "script",
        ResourceKind::Image => "image",
        ResourceKind::Font => "font",
        ResourceKind::Media => "media",
        ResourceKind::Object => "object",
        ResourceKind::Xhr => "xmlhttprequest",
        ResourceKind::Ping => "ping",
        ResourceKind::Other => "other",
    }
}

/// What the site information popup says about the ad blocker on a page.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SiteAdblock {
    active: bool,
    switchable: bool,
    detail: String,
}

impl Browser {
    /// What the page at `url` is protected from, after all three levels had their say.
    pub(super) fn protections_on(&self, url: &str) -> atoll_adblock::Effective {
        self.adblock.policy.effective(url)
    }

    /// Writes down what one level is to do about the page at `url`. A switch that says the same
    /// as the wider levels is left to them, so no rule is kept that decides nothing.
    pub(super) fn set_protection(
        &self,
        scope: Scope,
        url: &str,
        protection: Protection,
        value: bool,
    ) {
        let Some(pattern) = scope.pattern_of(url) else {
            return;
        };
        let inherited = self.adblock.policy.inherited(scope, url).get(protection);
        let mut protections = self.adblock.policy.protections(scope, url);
        protections.set(protection, (value != inherited).then_some(value));
        self.adblock.policy.set(scope, &pattern, protections);
        let stored = self
            .storage
            .set_adblock_rule(&rule_of(scope, &pattern, protections), Self::now());
        self.report(t!("Schutz ändern"), stored);
        self.for_all_windows(Browser::protections_changed);
    }

    /// Lets one level decide nothing again, so the page follows the wider levels.
    pub(super) fn forget_protections(&self, scope: Scope, url: &str) {
        let Some(pattern) = scope.pattern_of(url) else {
            return;
        };
        self.adblock
            .policy
            .set(scope, &pattern, Protections::default());
        let stored = self.storage.set_adblock_rule(
            &rule_of(scope, &pattern, Protections::default()),
            Self::now(),
        );
        self.report(t!("Schutz ändern"), stored);
        self.for_all_windows(Browser::protections_changed);
    }

    /// Settings, palette: blocks advertising everywhere, or stops doing so.
    pub(super) fn set_adblock_enabled(&self, enabled: bool) {
        self.set_protection(Scope::Global, "", Protection::Banners, enabled);
        self.show_toast(if enabled {
            t!("Werbeblocker eingeschaltet, wirkt ab dem nächsten Laden einer Seite")
        } else {
            t!("Werbeblocker ausgeschaltet")
        });
    }

    /// Site information and palette: switches the protection off for the current site, or on
    /// again, and reloads the page. The reload bypasses the cache, the page would otherwise keep
    /// resources it loaded before.
    pub fn toggle_site_adblock(&self) {
        let Some(url) = self.current_page_url() else {
            return;
        };
        let Some(site) = site_of(&url) else { return };
        let active = !self.adblock.policy.effective(&url).active;
        self.set_protection(Scope::Site, &url, Protection::Active, active);
        self.reload(true);
        if self
            .window()
            .is_some_and(|window| window.global::<OverlayState>().get_site_info_visible())
        {
            self.show_site_info();
        }
        self.show_toast(&if active {
            t!("Schutz für {} eingeschaltet", site)
        } else {
            t!("Schutz für {} ausgeschaltet", site)
        });
    }

    /// Settings: lets every site and every address follow the settings that count everywhere.
    pub fn clear_adblock_exceptions(&self) {
        self.adblock.policy.forget_exceptions();
        let cleared = self.storage.clear_adblock_exceptions();
        self.report(t!("Ausnahmen entfernen"), cleared);
        self.for_all_windows(Browser::protections_changed);
    }

    /// Refreshes everything that shows what a page is protected from.
    pub(super) fn protections_changed(&self) {
        self.show_adblock_settings();
        self.sync_shield();
        if self
            .window()
            .is_some_and(|window| window.global::<OverlayState>().get_site_info_visible())
            && let Some(url) = self.current_page_url()
        {
            self.show_site_adblock(&url);
        }
    }

    /// Settings: hides the cookie banners of the sites the list knows.
    pub(super) fn set_hide_cookie_banners(&self, hide: bool) {
        self.update_preference(PreferenceKey::CookieBannersHide, |preferences| {
            preferences.cookie_banners_hide = hide;
        });
        self.adblock
            .hide_cookie_banners
            .store(hide, Ordering::Relaxed);
        if hide {
            self.adblock.start_banner_lists();
        }
        self.for_all_windows(Browser::show_adblock_settings);
        self.show_toast(if hide {
            t!("Cookie-Banner werden versteckt, ab dem nächsten Laden einer Seite")
        } else {
            t!("Cookie-Banner werden wieder gezeigt")
        });
    }

    /// Settings: answers the banners Atoll recognises with a refusal.
    pub(super) fn set_answer_cookie_banners(&self, answer: bool) {
        self.update_preference(PreferenceKey::CookieBannersAnswer, |preferences| {
            preferences.cookie_banners_answer = answer;
        });
        self.adblock.answer_banners.store(answer, Ordering::Relaxed);
        self.for_all_windows(Browser::show_adblock_settings);
        self.show_toast(if answer {
            t!("Cookie-Banner werden abgelehnt, ab dem nächsten Laden einer Seite")
        } else {
            t!("Cookie-Banner werden nicht mehr beantwortet")
        });
    }

    /// Settings and palette: downloads the filter lists now.
    pub fn update_filter_lists(&self) {
        if let Some(updater) = self.adblock.updater.borrow().as_ref() {
            updater.update_now();
        }
        self.show_toast(t!("Filterlisten werden aktualisiert"));
    }

    /// Fills the ad blocker part of the settings page.
    pub(super) fn show_adblock_settings(&self) {
        let Some(window) = self.window() else { return };
        let state = window.global::<SettingsState>();
        let status = self.adblock.status();
        state.set_adblock_enabled(self.adblock.policy.effective("").banners);
        state.set_adblock_status(
            status
                .map(|status| list_status_text(&status, SystemTime::now()))
                .unwrap_or_else(|| t!("Filterlisten werden geladen…").to_owned())
                .into(),
        );
        state.set_adblock_updating(status.is_some_and(|status| {
            matches!(status.state, ListState::Loading | ListState::Updating)
        }));
        let exceptions: Vec<String> = self
            .adblock
            .policy
            .exceptions()
            .into_iter()
            .map(|(_, pattern)| pattern)
            .collect();
        state.set_adblock_exceptions(exceptions.join(", ").into());
        let preferences = self.preferences.borrow();
        state.set_cookie_banners_hide(preferences.cookie_banners_hide);
        state.set_cookie_banners_answer(preferences.cookie_banners_answer);
        drop(preferences);
        state.set_cookie_banners_status(cookie_banner_status(self.adblock.banner_status()).into());
    }

    /// Refreshes what shows the state of the filter lists.
    pub(super) fn adblock_status_changed(&self) {
        let Some(window) = self.window() else { return };
        if window.global::<SettingsState>().get_visible() {
            self.show_adblock_settings();
        }
        if window.global::<OverlayState>().get_site_info_visible()
            && let Some(url) = self.current_page_url()
        {
            self.show_site_adblock(&url);
        }
    }

    /// Fills the ad blocker row of the site information popup for the page at `url`.
    pub(super) fn show_site_adblock(&self, url: &str) {
        let Some(window) = self.window() else { return };
        let overlay = window.global::<OverlayState>();
        let site = is_web_address(url).then(|| site_of(url)).flatten();
        overlay.set_site_info_adblock_available(site.is_some());
        let Some(site) = site else { return };
        let mut blocked = 0;
        self.with_live_view(|view| blocked = view.blocked_requests());
        let summary = site_adblock(
            &self.adblock.policy,
            &self.adblock.blocker,
            url,
            &site,
            blocked,
            self.adblock.status(),
        );
        overlay.set_site_info_adblock_active(summary.active);
        overlay.set_site_info_adblock_switchable(summary.switchable);
        overlay.set_site_info_adblock_detail(summary.detail.into());
    }
}

fn is_web_address(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

fn site_adblock(
    policy: &Policy,
    blocker: &Blocker,
    url: &str,
    site: &str,
    blocked: u32,
    status: Option<ListStatus>,
) -> SiteAdblock {
    let effective = policy.effective(url);
    if !effective.active {
        return SiteAdblock {
            active: false,
            switchable: true,
            detail: t!("Für {} ausgeschaltet", site),
        };
    }
    if !effective.banners {
        return SiteAdblock {
            active: false,
            switchable: true,
            detail: t!("Werbung wird hier nicht blockiert").to_owned(),
        };
    }
    let detail = if !blocker.is_ready() {
        match status.map(|status| status.state) {
            Some(ListState::Failed { list }) => t!("{} konnte nicht geladen werden", list),
            _ => t!("Filterlisten werden geladen…").to_owned(),
        }
    } else {
        match blocked {
            0 => t!("Auf dieser Seite wurde nichts blockiert").to_owned(),
            1 => t!("1 Anfrage blockiert").to_owned(),
            count => t!("{} Anfragen blockiert", count),
        }
    };
    SiteAdblock {
        active: true,
        switchable: true,
        detail,
    }
}

/// The state of the filter lists for the settings page.
fn list_status_text(status: &ListStatus, now: SystemTime) -> String {
    let age = status
        .downloaded_at
        .map(|at| relative_age(now.duration_since(at).unwrap_or_default()));
    let names = list_names();
    match (status.state, age) {
        (ListState::Loading, _) => t!("Filterlisten werden geladen…").to_owned(),
        (ListState::Updating, _) => t!("Filterlisten werden aktualisiert…").to_owned(),
        (ListState::Ready, Some(age)) => t!("{}, aktualisiert {}", names, age),
        (ListState::Ready, None) => names,
        (ListState::Failed { list }, Some(age)) => {
            t!(
                "{} konnte nicht geladen werden. Es gelten die Listen von {}.",
                list,
                age
            )
        }
        (ListState::Failed { list }, None) => {
            t!(
                "{} konnte nicht geladen werden. Neuer Versuch in einer Stunde.",
                list
            )
        }
    }
}

/// "EasyList, EasyPrivacy und EasyList Germany"
fn list_names() -> String {
    let names: Vec<&str> = FILTER_LISTS.iter().map(|list| list.name).collect();
    match names.split_last() {
        Some((last, [])) => (*last).to_owned(),
        Some((last, rest)) => t!("{} und {}", rest.join(", "), last),
        None => String::new(),
    }
}

fn relative_age(age: Duration) -> String {
    let minutes = age.as_secs() / 60;
    match minutes {
        0 => "gerade eben".to_owned(),
        1 => t!("vor einer Minute").to_owned(),
        2..60 => t!("vor {} Minuten", minutes),
        60..120 => t!("vor einer Stunde").to_owned(),
        120..1440 => t!("vor {} Stunden", minutes / 60),
        1440..2880 => "gestern".to_owned(),
        _ => t!("vor {} Tagen", minutes / 1440),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(state: ListState, age: Option<u64>, now: SystemTime) -> ListStatus {
        ListStatus {
            state,
            downloaded_at: age.map(|seconds| now - Duration::from_secs(seconds)),
        }
    }

    #[test]
    fn list_status_names_the_lists_and_their_age() {
        let now = SystemTime::now();
        assert_eq!(
            list_status_text(&status(ListState::Ready, Some(3 * 3600), now), now),
            "EasyList, EasyPrivacy und EasyList Germany, aktualisiert vor 3 Stunden"
        );
        assert_eq!(
            list_status_text(&status(ListState::Updating, Some(10), now), now),
            "Filterlisten werden aktualisiert…"
        );
        assert_eq!(
            list_status_text(
                &status(
                    ListState::Failed {
                        list: "EasyPrivacy"
                    },
                    Some(5 * 86_400),
                    now
                ),
                now
            ),
            "EasyPrivacy konnte nicht geladen werden. Es gelten die Listen von vor 5 Tagen."
        );
        assert_eq!(
            list_status_text(
                &status(ListState::Failed { list: "EasyList" }, None, now),
                now
            ),
            "EasyList konnte nicht geladen werden. Neuer Versuch in einer Stunde."
        );
    }

    #[test]
    fn ages_read_naturally() {
        assert_eq!(relative_age(Duration::from_secs(20)), "gerade eben");
        assert_eq!(relative_age(Duration::from_secs(90)), "vor einer Minute");
        assert_eq!(relative_age(Duration::from_secs(45 * 60)), "vor 45 Minuten");
        assert_eq!(
            relative_age(Duration::from_secs(61 * 60)),
            "vor einer Stunde"
        );
        assert_eq!(relative_age(Duration::from_secs(30 * 3600)), "gestern");
        assert_eq!(relative_age(Duration::from_secs(4 * 86_400)), "vor 4 Tagen");
    }

    #[test]
    fn the_site_row_explains_why_nothing_is_blocked() {
        let policy = Policy::new();
        let blocker = Blocker::new(|_| true);
        let quiet = "https://quiet.example/page";
        policy.set(
            Scope::Site,
            "quiet.example",
            Protections::default().with(Protection::Active, Some(false)),
        );
        assert_eq!(
            site_adblock(&policy, &blocker, quiet, "quiet.example", 0, None),
            SiteAdblock {
                active: false,
                switchable: true,
                detail: "Für quiet.example ausgeschaltet".to_owned(),
            }
        );
        let page = "https://example.org/";
        assert_eq!(
            site_adblock(&policy, &blocker, page, "example.org", 0, None).detail,
            "Filterlisten werden geladen…"
        );
        let failed = ListStatus {
            state: ListState::Failed { list: "EasyList" },
            downloaded_at: None,
        };
        assert_eq!(
            site_adblock(&policy, &blocker, page, "example.org", 0, Some(failed)).detail,
            "EasyList konnte nicht geladen werden"
        );
        policy.set(
            Scope::Global,
            "",
            Protections::default().with(Protection::Banners, Some(false)),
        );
        assert_eq!(
            site_adblock(&policy, &blocker, page, "example.org", 3, None).detail,
            "Werbung wird hier nicht blockiert"
        );
    }

    #[test]
    fn a_page_only_opens_what_it_is_allowed_to() {
        let policy = Arc::new(Policy::new());
        let filter = AdblockFilter {
            policy: policy.clone(),
            blocker: Arc::new(Blocker::new(|_| true)),
            cookies: Arc::new(Blocker::new(|_| false)),
            answer_banners: Arc::new(AtomicBool::new(false)),
        };
        let page = "https://example.org/artikel";
        let elsewhere = "https://werbung.example/ziel";
        assert!(
            !filter.allows(Opening::Popup, page, elsewhere, false),
            "a window a page opens by itself is stopped from the start"
        );
        assert!(
            !filter.allows(Opening::Popup, page, elsewhere, true),
            "a click usually meant something else than a window"
        );
        assert!(
            filter.allows(Opening::Navigation, page, elsewhere, false),
            "being sent away is only stopped where that is switched on"
        );
        policy.set(
            Scope::Global,
            "",
            Protections::default()
                .with(Protection::Popups, Some(false))
                .with(Protection::Redirects, Some(true)),
        );
        assert!(filter.allows(Opening::Popup, page, elsewhere, false));
        assert!(!filter.allows(Opening::Navigation, page, elsewhere, false));
        assert!(
            filter.allows(Opening::Navigation, page, elsewhere, true),
            "where the user clicked, they are taken along"
        );
        assert!(
            filter.allows(Opening::Navigation, page, "https://example.org/mehr", false),
            "a page may move within itself"
        );
        policy.set(
            Scope::Site,
            "example.org",
            Protections::default().with(Protection::Active, Some(false)),
        );
        assert!(
            filter.allows(Opening::Navigation, page, elsewhere, false),
            "a page without protection is left alone"
        );
    }
}
