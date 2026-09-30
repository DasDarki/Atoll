//! The VPN of the user, and what Atoll does while it is not there.
//!
//! Atoll runs no VPN service. The user names the proxy of the service they already pay for, and
//! from then on every request of the browser takes that way: the pages through the engine, and
//! what Atoll fetches itself through [`atoll_net`]. While the way out does not stand, nothing
//! goes at all - that is the killswitch, and it is not a setting, because a fallback to the
//! ordinary connection is exactly the leak a VPN is meant to rule out.

use std::path::PathBuf;

use atoll_adblock::Scope;
use atoll_engine::ProxyRoute;
use atoll_i18n::t;
use atoll_vpn::{Kind, Status, TunnelConfig, Upstream, Way};
use slint::ComponentHandle;

use super::Browser;
use super::preferences::PreferenceKey;
use crate::platform::dialogs::{self, FileDialogKind, FileDialogOptions, FileFilter};
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
    /// The way out as the user wrote it down in the settings, or what is wrong with it.
    fn way(&self) -> Result<Way, String> {
        let preferences = self.preferences.borrow();
        if preferences.vpn_kind != TUNNEL {
            return Ok(Way::Proxy(Upstream {
                kind: Kind::from_key(&preferences.vpn_kind).unwrap_or_default(),
                host: preferences.vpn_host.trim().to_owned(),
                port: u16::try_from(preferences.vpn_port).unwrap_or_default(),
                user: preferences.vpn_user.clone(),
                password: preferences.vpn_password.clone(),
            }));
        }
        let path = preferences
            .vpn_config_path
            .clone()
            .ok_or_else(|| t!("Noch keine WireGuard-Konfiguration gewählt").to_owned())?;
        drop(preferences);
        let text = std::fs::read_to_string(&path)
            .map_err(|error| t!("{} ist nicht lesbar: {}", path.display(), error))?;
        TunnelConfig::parse(&text).map(|config| Way::Tunnel(Box::new(config)))
    }

    /// Settings: picks the configuration file the service handed out.
    pub fn choose_vpn_config(&self) {
        let this = self.this();
        dialogs::show(
            FileDialogOptions {
                kind: FileDialogKind::Open,
                title: t!("WireGuard-Konfiguration wählen").into(),
                directory: None,
                file_name: None,
                filters: vec![FileFilter {
                    name: t!("WireGuard-Konfiguration").to_owned(),
                    extensions: vec!["conf".to_owned()],
                }],
            },
            move |paths| {
                let Some(browser) = this.upgrade() else {
                    return;
                };
                if let Some(path) = paths.and_then(|paths| paths.into_iter().next()) {
                    browser.set_vpn_config(Some(path));
                }
            },
        );
    }

    /// Remembers the chosen configuration and tries it at once.
    pub(super) fn set_vpn_config(&self, path: Option<PathBuf>) {
        self.update_preference(PreferenceKey::VpnConfigPath, |preferences| {
            preferences.vpn_config_path = path;
        });
        self.vpn_settings_changed();
        self.for_all_windows(Browser::show_vpn_settings);
    }

    /// Starts carrying everything through the VPN, or stops doing so.
    pub(super) fn set_vpn_enabled(&self, enabled: bool) {
        let way = self.way();
        if enabled && let Err(trouble) = &way {
            self.show_toast(trouble);
            self.show_vpn_settings();
            return;
        }
        if enabled && !way.as_ref().is_ok_and(Way::is_complete) {
            self.show_toast(t!(
                "Trag zuerst den Proxy deines VPN-Dienstes ein, mit Adresse und Port"
            ));
            self.show_vpn_settings();
            return;
        }
        self.update_preference(PreferenceKey::VpnEnabled, |preferences| {
            preferences.vpn_enabled = enabled;
        });
        match (enabled, way) {
            (true, Ok(way)) => self.vpn.start(way),
            _ => self.vpn.stop(),
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
        match self.way() {
            Ok(way) => self.vpn.start(way),
            Err(trouble) => {
                // The killswitch holds anyway: the bridge refuses while nothing carries.
                tracing::warn!(trouble, "the VPN is switched on but cannot be built");
                self.vpn.start(Way::default());
            }
        }
        self.apply_vpn_route();
    }

    /// The way out was changed in the settings, so it is tried again right away.
    pub(super) fn vpn_settings_changed(&self) {
        if !self.preferences.borrow().vpn_enabled {
            return;
        }
        match self.way() {
            Ok(way) => self.vpn.start(way),
            Err(trouble) => self.show_toast(&trouble),
        }
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
        state.set_vpn_kind(match preferences.vpn_kind.as_str() {
            "http" => 1,
            "https" => 2,
            TUNNEL => 3,
            _ => 0,
        });
        state.set_vpn_config(
            preferences
                .vpn_config_path
                .as_ref()
                .map(|path| {
                    path.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.display().to_string())
                })
                .unwrap_or_default()
                .into(),
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

/// The name the settings use for the tunnel Atoll builds itself.
pub(super) const TUNNEL: &str = "wireguard";

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
