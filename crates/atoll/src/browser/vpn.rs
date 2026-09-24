//! The VPN of the user, and what Atoll does while it is not there.
//!
//! Atoll runs no VPN service. The user names the proxy of the service they already pay for, and
//! from then on every request of the browser takes that way: the pages through the engine, and
//! what Atoll fetches itself through [`atoll_net`]. While the way out does not stand, nothing
//! goes at all - that is the killswitch, and it is not a setting, because a fallback to the
//! ordinary connection is exactly the leak a VPN is meant to rule out.

use atoll_adblock::Scope;
use atoll_engine::ProxyRoute;
use atoll_i18n::t;
use atoll_vpn::{Kind, Status, Upstream};
use slint::ComponentHandle;

use super::Browser;
use super::preferences::PreferenceKey;
use crate::ui::{BrowserState, SettingsState};

/// What the user interface shows about the VPN.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Showing {
    Off = 0,
    Connecting = 1,
    Up = 2,
    Down = 3,
}

impl Showing {
    fn of(status: &Status) -> Self {
        match status {
            Status::Off => Showing::Off,
            Status::Connecting => Showing::Connecting,
            Status::Up(_) => Showing::Up,
            Status::Down(_) => Showing::Down,
        }
    }
}

impl Browser {
    /// The proxy as the user wrote it down in the settings.
    fn upstream(&self) -> Upstream {
        let preferences = self.preferences.borrow();
        Upstream {
            kind: Kind::from_key(&preferences.vpn_kind).unwrap_or_default(),
            host: preferences.vpn_host.trim().to_owned(),
            port: u16::try_from(preferences.vpn_port).unwrap_or_default(),
            user: preferences.vpn_user.clone(),
            password: preferences.vpn_password.clone(),
        }
    }

    /// Starts carrying everything through the VPN, or stops doing so.
    pub(super) fn set_vpn_enabled(&self, enabled: bool) {
        if enabled && !self.upstream().is_complete() {
            self.show_toast(t!(
                "Trag zuerst den Proxy deines VPN-Dienstes ein, mit Adresse und Port"
            ));
            self.show_vpn_settings();
            return;
        }
        self.update_preference(PreferenceKey::VpnEnabled, |preferences| {
            preferences.vpn_enabled = enabled;
        });
        if enabled {
            self.vpn.start(self.upstream());
        } else {
            self.vpn.stop();
        }
        self.apply_vpn_route();
        self.for_all_windows(Browser::show_vpn_settings);
        self.show_toast(if enabled {
            t!("Das VPN wird verbunden. Bis es steht, lädt Atoll nichts.")
        } else {
            t!("Das VPN ist aus, Atoll geht wieder direkt ins Netz")
        });
    }

    /// At startup: takes the VPN up again if the user left it on, before a page is loaded.
    pub(super) fn resume_vpn(&self) {
        if !self.preferences.borrow().vpn_enabled {
            return;
        }
        let upstream = self.upstream();
        if !upstream.is_complete() {
            tracing::warn!("the VPN is switched on but no proxy is written down");
        }
        self.vpn.start(upstream);
        self.apply_vpn_route();
    }

    /// The proxy was changed in the settings, so it is tried again right away.
    pub(super) fn vpn_settings_changed(&self) {
        if !self.preferences.borrow().vpn_enabled {
            return;
        }
        self.vpn.start(self.upstream());
        self.apply_vpn_route();
        self.for_all_windows(Browser::show_vpn_settings);
    }

    /// Tells the engine and everything else Atoll fetches where the requests go now.
    pub(super) fn apply_vpn_route(&self) {
        let port = self.vpn.is_wanted().then(|| self.vpn.port()).flatten();
        atoll_net::route_through(port);
        let route = port.map(|port| ProxyRoute {
            port,
            bypass: bypass_list(&self.adblock.policy.bypassed_sites()),
        });
        if let Some(engine) = self.engine.borrow().as_ref()
            && let Err(error) = engine.set_proxy(route)
        {
            tracing::warn!(error, "the requests could not be sent through the VPN");
            self.show_toast(&t!(
                "Der Browser konnte nicht aufs VPN umgestellt werden: {}",
                error
            ));
        }
    }

    /// Looks after the VPN, called whenever the browser is awake anyway.
    pub(super) fn watch_vpn(&self) {
        let status = self.vpn.status();
        if self.vpn_status.borrow().as_ref() == Some(&status) {
            return;
        }
        let first = self.vpn_status.borrow().is_none();
        *self.vpn_status.borrow_mut() = Some(status.clone());
        self.sync_vpn_state(&status);
        if first {
            return;
        }
        match &status {
            Status::Up(_) => {
                self.apply_vpn_route();
                self.show_toast(t!("Das VPN steht"));
            }
            Status::Down(reason) => {
                self.show_toast(&t!(
                    "Das VPN ist unterbrochen: {}. Atoll lädt nichts.",
                    reason
                ));
            }
            Status::Off | Status::Connecting => {}
        }
    }

    /// Fills the badge in the address bar.
    fn sync_vpn_state(&self, status: &Status) {
        self.for_all_windows(|browser| {
            if let Some(window) = browser.window() {
                window
                    .global::<BrowserState>()
                    .set_vpn_state(Showing::of(status) as i32);
            }
            browser.show_vpn_settings();
        });
    }

    /// Fills the VPN part of the settings page.
    pub(super) fn show_vpn_settings(&self) {
        let Some(window) = self.window() else { return };
        let state = window.global::<SettingsState>();
        let preferences = self.preferences.borrow();
        state.set_vpn_enabled(preferences.vpn_enabled);
        state.set_vpn_kind(
            match Kind::from_key(&preferences.vpn_kind).unwrap_or_default() {
                Kind::Socks5 => 0,
                Kind::Http => 1,
                Kind::Https => 2,
            },
        );
        state.set_vpn_host(preferences.vpn_host.as_str().into());
        state.set_vpn_port(preferences.vpn_port as i32);
        state.set_vpn_user(preferences.vpn_user.as_str().into());
        state.set_vpn_password(preferences.vpn_password.as_str().into());
        drop(preferences);
        let status = self.vpn.status();
        window
            .global::<BrowserState>()
            .set_vpn_state(Showing::of(&status) as i32);
        state.set_vpn_status(
            match &status {
                Status::Off => t!("Aus. Atoll geht direkt ins Netz.").to_owned(),
                Status::Connecting => t!("Wird verbunden. Bis dahin lädt Atoll nichts.").to_owned(),
                Status::Up(_) => {
                    t!("Verbunden. Alles geht durch das VPN, auch Downloads und Erweiterungen.")
                        .to_owned()
                }
                Status::Down(reason) => t!("Unterbrochen: {}. Atoll lädt nichts.", reason),
            }
            .into(),
        );
        let kept_out = self.adblock.policy.bypassed_sites();
        state.set_vpn_exceptions(kept_out.join(", ").into());
    }

    /// Whether a page may be loaded at all right now, which is what the killswitch decides.
    pub(super) fn vpn_holds_everything(&self) -> bool {
        self.vpn.is_wanted() && !self.vpn.status().is_up()
    }

    /// Settings: lets every site take the way through the VPN again.
    pub fn clear_vpn_exceptions(&self) {
        for site in self.adblock.policy.bypassed_sites() {
            self.forget_protections(Scope::Site, &format!("https://{site}/"));
        }
        self.apply_vpn_route();
        self.for_all_windows(Browser::show_vpn_settings);
    }
}

/// The hosts Chromium lets out directly: the site itself and everything below it.
fn bypass_list(sites: &[String]) -> Vec<String> {
    let mut bypass = vec![
        "localhost".to_owned(),
        "127.0.0.1".to_owned(),
        "[::1]".to_owned(),
    ];
    for site in sites {
        bypass.push(site.clone());
        bypass.push(format!("*.{site}"));
    }
    bypass
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_let_out_directly_always_holds_this_computer() {
        let bypass = bypass_list(&[]);
        assert!(bypass.contains(&"localhost".to_owned()));
        assert!(
            bypass.contains(&"127.0.0.1".to_owned()),
            "the developer tools and the clipboard helper talk to this computer"
        );
    }

    #[test]
    fn a_site_kept_out_takes_everything_below_it_along() {
        let bypass = bypass_list(&["intern.example".to_owned()]);
        assert!(bypass.contains(&"intern.example".to_owned()));
        assert!(
            bypass.contains(&"*.intern.example".to_owned()),
            "a page of the site under another name is the same site to the user"
        );
    }

    #[test]
    fn the_state_shown_follows_the_state_of_the_way_out() {
        assert_eq!(Showing::of(&Status::Off), Showing::Off);
        assert_eq!(Showing::of(&Status::Connecting), Showing::Connecting);
        assert_eq!(Showing::of(&Status::Up(1080)), Showing::Up);
        assert_eq!(
            Showing::of(&Status::Down("nothing answers".to_owned())),
            Showing::Down
        );
    }
}
