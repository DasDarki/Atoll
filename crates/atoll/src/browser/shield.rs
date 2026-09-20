//! The shield in the address bar: what this page is protected from.
//!
//! Some sites only work when a protection is off, and the user needs that in two clicks, not in
//! the settings. The shield therefore shows the five switches for one of three levels - for
//! everything, for this site, or only for this address - and writes what is changed straight
//! into the rules the ad blocker and the engine ask.

use atoll_adblock::{Protection, Scope, site_of};
use atoll_i18n::t;
use slint::ComponentHandle;

use super::Browser;
use crate::ui::{BrowserState, OverlayState};

/// The level the shield is showing, as the segmented control counts them.
fn scope_at(index: i32) -> Scope {
    match index {
        0 => Scope::Global,
        2 => Scope::Page,
        _ => Scope::Site,
    }
}

fn index_of(scope: Scope) -> i32 {
    match scope {
        Scope::Global => 0,
        Scope::Site => 1,
        Scope::Page => 2,
    }
}

impl Browser {
    /// Opens the shield, on the level that already has something to say about this page.
    pub fn show_shield(&self) {
        let Some(window) = self.window() else { return };
        self.dismiss_suggestions();
        let url = self.current_page_url().unwrap_or_default();
        let scope = if Scope::Page.pattern_of(&url).is_some_and(|_| {
            !self
                .adblock
                .policy
                .protections(Scope::Page, &url)
                .says_nothing()
        }) {
            Scope::Page
        } else if site_of(&url).is_some() {
            Scope::Site
        } else {
            Scope::Global
        };
        window
            .global::<OverlayState>()
            .set_shield_scope(index_of(scope));
        self.fill_shield(&url);
        window.global::<OverlayState>().set_shield_visible(true);
    }

    pub fn close_shield(&self) {
        if let Some(window) = self.window() {
            window.global::<OverlayState>().set_shield_visible(false);
        }
        self.focus_content();
    }

    /// The user looks at another level.
    pub fn shield_scope_chosen(&self, index: i32) {
        let Some(window) = self.window() else { return };
        window.global::<OverlayState>().set_shield_scope(index);
        self.fill_shield(&self.current_page_url().unwrap_or_default());
    }

    /// The user flipped one of the switches.
    pub fn shield_toggled(&self, key: &str, value: bool) {
        let Some(window) = self.window() else { return };
        let Some(protection) = Protection::from_key(key) else {
            return;
        };
        let url = self.current_page_url().unwrap_or_default();
        let scope = scope_at(window.global::<OverlayState>().get_shield_scope());
        self.set_protection(scope, &url, protection, value);
        // What is blocked in the page itself only changes once the page is built again, the rest
        // counts for the next thing the page tries.
        if matches!(protection, Protection::Active | Protection::Banners) {
            self.reload(true);
            self.show_toast(t!("Die Seite wird neu geladen"));
        } else {
            self.show_toast(t!("Gilt ab sofort"));
        }
    }

    /// The user let this level decide nothing again.
    pub fn shield_reset(&self) {
        let Some(window) = self.window() else { return };
        let url = self.current_page_url().unwrap_or_default();
        let scope = scope_at(window.global::<OverlayState>().get_shield_scope());
        self.forget_protections(scope, &url);
        self.show_toast(match scope {
            Scope::Global => t!("Überall gilt wieder, was Atoll vorsieht"),
            Scope::Site => t!("Die Seite folgt wieder den allgemeinen Einstellungen"),
            Scope::Page => t!("Die Adresse folgt wieder den Einstellungen der Seite"),
        });
    }

    /// Fills the shield and the badge in the address bar with the state of the current page.
    pub(super) fn sync_shield(&self) {
        let url = self.current_page_url().unwrap_or_default();
        if let Some(window) = self.window() {
            let effective = self.protections_on(&url);
            window
                .global::<BrowserState>()
                .set_protection_off(!effective.active);
            if window.global::<OverlayState>().get_shield_visible() {
                self.fill_shield(&url);
            }
        }
    }

    fn fill_shield(&self, url: &str) {
        let Some(window) = self.window() else { return };
        let state = window.global::<OverlayState>();
        let scope = scope_at(state.get_shield_scope());
        let site = site_of(url);
        state.set_shield_site(site.clone().unwrap_or_default().into());
        state.set_shield_page(shortened(url).into());
        state.set_shield_site_known(site.is_some());
        state.set_shield_page_known(Scope::Page.pattern_of(url).is_some());
        let shown = if scope.pattern_of(url).is_some() {
            scope
        } else {
            Scope::Global
        };
        let settings = self.adblock.policy.protections(shown, url);
        let effective = if shown == scope {
            self.protections_on(url)
        } else {
            self.adblock.policy.effective("")
        };
        state.set_shield_scope(index_of(shown));
        state.set_shield_own(!settings.says_nothing());
        state.set_shield_active(effective.active);
        state.set_shield_banners(effective.banners);
        state.set_shield_popups(effective.popups);
        state.set_shield_redirects(effective.redirects);
        state.set_shield_new_tabs(effective.new_tabs);
        let mut blocked = 0;
        self.with_live_view(|view| blocked = view.blocked_requests());
        state.set_shield_blocked(
            match blocked {
                0 => t!("Auf dieser Seite wurde nichts blockiert").to_owned(),
                1 => t!("1 Anfrage blockiert").to_owned(),
                count => t!("{} Anfragen blockiert", count),
            }
            .into(),
        );
    }

    /// Tells the user that a page was not allowed to open something.
    pub(super) fn opening_blocked(&self, opening: atoll_engine::Opening, url: &str) {
        let site = site_of(url).unwrap_or_else(|| url.to_owned());
        self.show_toast(&match opening {
            atoll_engine::Opening::Popup => t!("Popup von {} blockiert", site),
            atoll_engine::Opening::NewTab => t!("Neuer Tab auf {} verhindert", site),
            atoll_engine::Opening::Navigation => t!("Weiterleitung auf {} verhindert", site),
        });
    }
}

/// The address as the shield shows it, without the scheme and not longer than a line.
fn shortened(url: &str) -> String {
    let short = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    let short = short.strip_suffix('/').unwrap_or(short);
    if short.chars().count() <= 48 {
        return short.to_owned();
    }
    let kept: String = short.chars().take(47).collect();
    format!("{kept}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_level_is_read_from_the_control_and_back() {
        for scope in Scope::ALL {
            assert_eq!(scope_at(index_of(scope)), scope);
        }
    }

    #[test]
    fn an_address_is_shown_without_its_scheme_and_not_endlessly() {
        assert_eq!(shortened("https://example.org/"), "example.org");
        assert_eq!(shortened("http://example.org/a/b"), "example.org/a/b");
        let long = format!("https://example.org/{}", "a".repeat(80));
        let shown = shortened(&long);
        assert_eq!(shown.chars().count(), 48);
        assert!(shown.ends_with('…'));
    }
}
