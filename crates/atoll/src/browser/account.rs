//! The account of Atoll Sync: signing in with Onyx in a tab, staying signed in between runs and
//! signing out. The sign-in is the same for every window.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use atoll_core::TabId;
use atoll_sync_client::{Auth, Login, LoginError, Profile, TokenError, TokenStore, tokens};
use slint::{ComponentHandle, Timer};

use super::Browser;
use super::preferences::PreferenceKey;
use crate::platform::tasks::{run_in_background, run_with_updates};
use crate::ui::SettingsState;
use atoll_i18n::t;

/// How long the sign-in page may stay open before Atoll stops waiting for it.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// How long the page saying "signed in" stays before its tab closes.
const LOGIN_TAB_LINGER: Duration = Duration::from_millis(1500);

/// Where the account stands, shared by every window.
#[derive(Default)]
pub enum AccountState {
    #[default]
    SignedOut,
    /// The sign-in of the last run is being checked.
    Checking,
    SigningIn {
        cancel: Arc<AtomicBool>,
        /// The tab showing the sign-in page.
        tab: Option<TabId>,
    },
    SignedIn {
        auth: Arc<Auth>,
        /// The name of the account, `None` while Onyx could not be asked.
        name: Option<String>,
    },
}

/// Why a sign-in did not finish.
enum SignInFailure {
    Cancelled,
    Failed(String),
}

impl Browser {
    pub fn account_action(&self, action: &str) {
        match action {
            "sign-in" => self.sign_in(),
            "cancel" => self.cancel_sign_in(),
            "sign-out" => self.sign_out(),
            other => tracing::warn!(action = other, "unknown account action"),
        }
    }

    /// The sign-in of this profile, wherever it waits.
    pub(super) fn token_store(&self) -> TokenStore {
        let data = self
            .paths
            .database
            .parent()
            .unwrap_or(std::path::Path::new("."));
        TokenStore::new(data)
    }

    /// Picks the sign-in of the last run back up and checks whether Onyx still accepts it. Asked
    /// once at the start, not per window.
    pub(super) fn resume_account(&self) {
        let Some(auth) = Auth::resume(self.token_store()) else {
            return;
        };
        let auth = Arc::new(auth);
        *self.account.borrow_mut() = AccountState::Checking;
        self.show_account_everywhere();
        let this = self.this();
        let worker = auth.clone();
        run_in_background(
            "account",
            move || worker.profile(),
            move |answer| {
                let Some(browser) = this.upgrade() else {
                    return;
                };
                let state = match answer {
                    Ok(profile) => AccountState::SignedIn {
                        auth,
                        name: name_of(&profile),
                    },
                    Err(TokenError::SignedOut(reason)) => {
                        tracing::info!(reason, "the sign-in of the last run was ended");
                        browser.show_toast(t!(
                            "Atoll Sync: Die Anmeldung wurde beendet. Melde dich in den Einstellungen neu an."
                        ));
                        AccountState::SignedOut
                    }
                    Err(TokenError::Unavailable(reason)) => {
                        tracing::info!(reason, "Onyx could not be asked, the sign-in stays");
                        AccountState::SignedIn { auth, name: None }
                    }
                };
                let signed_in = matches!(state, AccountState::SignedIn { .. });
                *browser.account.borrow_mut() = state;
                browser.show_account_everywhere();
                if signed_in {
                    browser.sync_signed_in();
                }
            },
        );
    }

    /// The sign-in, while there is one.
    pub(super) fn signed_in_auth(&self) -> Option<Arc<Auth>> {
        match &*self.account.borrow() {
            AccountState::SignedIn { auth, .. } => Some(auth.clone()),
            _ => None,
        }
    }

    /// Onyx no longer accepts the sign-in: it was ended elsewhere.
    pub(super) fn signed_out_by_server(&self) {
        if self.signed_in_auth().is_none() {
            return;
        }
        *self.account.borrow_mut() = AccountState::SignedOut;
        self.sync_signed_out();
        self.show_account_everywhere();
        self.show_toast(t!(
            "Atoll Sync: Die Anmeldung wurde beendet. Melde dich in den Einstellungen neu an."
        ));
    }

    /// Opens the sign-in page of Onyx in a tab and waits for it to send the browser back.
    fn sign_in(&self) {
        if !matches!(*self.account.borrow(), AccountState::SignedOut) {
            return;
        }
        let server = self.sync_server();
        if server.is_empty() {
            self.account_error(t!("Zuerst die Adresse des Sync-Servers eintragen."));
            return;
        }
        self.account_error("");
        let cancel = Arc::new(AtomicBool::new(false));
        *self.account.borrow_mut() = AccountState::SigningIn {
            cancel: cancel.clone(),
            tab: None,
        };
        self.show_account_everywhere();
        let store = self.token_store();
        let opener = self.this();
        let this = self.this();
        run_with_updates(
            "sign-in",
            move |opened: &dyn Fn(String)| -> Result<(Auth, Profile), SignInFailure> {
                let config = tokens::server_config(&server).map_err(|error| {
                    SignInFailure::Failed(t!("Der Sync-Server antwortet nicht: {}", error))
                })?;
                let login = Login::start(&config)
                    .map_err(|error| SignInFailure::Failed(error.to_string()))?;
                opened(login.authorize_url.clone());
                let authorized =
                    login
                        .wait(SIGN_IN_TIMEOUT, &cancel)
                        .map_err(|error| match error {
                            LoginError::Cancelled => SignInFailure::Cancelled,
                            LoginError::TimedOut => SignInFailure::Failed(
                                t!("Die Anmeldung hat zu lange gedauert.").to_owned(),
                            ),
                            other => SignInFailure::Failed(other.to_string()),
                        })?;
                Auth::sign_in(&server, config, &authorized, store)
                    .map_err(|error| SignInFailure::Failed(error.to_string()))
            },
            move |url| {
                let Some(browser) = opener.upgrade() else {
                    return;
                };
                browser.close_settings();
                let tab = browser.open_in_new_tab(None, &url, true);
                if let AccountState::SigningIn { tab: login_tab, .. } =
                    &mut *browser.account.borrow_mut()
                {
                    *login_tab = tab;
                }
            },
            move |result| {
                let Some(browser) = this.upgrade() else {
                    return;
                };
                let login_tab = match &*browser.account.borrow() {
                    AccountState::SigningIn { tab, .. } => *tab,
                    _ => None,
                };
                let state = match result {
                    Ok((auth, profile)) => {
                        let name = name_of(&profile);
                        browser.show_toast(&t!(
                            "Bei Atoll Sync angemeldet als {}",
                            name.clone().unwrap_or_default()
                        ));
                        AccountState::SignedIn {
                            auth: Arc::new(auth),
                            name,
                        }
                    }
                    Err(SignInFailure::Cancelled) => AccountState::SignedOut,
                    Err(SignInFailure::Failed(reason)) => {
                        tracing::warn!(reason, "signing in failed");
                        browser.account_error(&t!("Anmeldung fehlgeschlagen: {}", reason));
                        AccountState::SignedOut
                    }
                };
                let signed_in = matches!(state, AccountState::SignedIn { .. });
                *browser.account.borrow_mut() = state;
                browser.show_account_everywhere();
                if signed_in {
                    browser.sync_signed_in();
                }
                if let Some(tab) = login_tab {
                    browser.close_login_tab_soon(tab);
                }
            },
        );
    }

    fn cancel_sign_in(&self) {
        let login_tab = match &*self.account.borrow() {
            AccountState::SigningIn { cancel, tab } => {
                cancel.store(true, Ordering::SeqCst);
                *tab
            }
            _ => None,
        };
        if let Some(tab) = login_tab {
            self.close_login_tab_soon(tab);
        }
    }

    /// Ends the sign-in here and at Onyx. Onyx is told in the background, the account is signed
    /// out here at once.
    fn sign_out(&self) {
        let previous = std::mem::take(&mut *self.account.borrow_mut());
        self.sync_signed_out();
        self.show_account_everywhere();
        if let AccountState::SignedIn { auth, .. } = previous {
            run_in_background("sign-out", move || auth.sign_out(), |()| {});
            self.show_toast(t!("Von Atoll Sync abgemeldet"));
        }
    }

    /// Closes the tab of the sign-in page shortly, so the page saying "signed in" can be seen,
    /// and only while it still shows the answer of Onyx.
    fn close_login_tab_soon(&self, tab: TabId) {
        let this = self.this();
        Timer::single_shot(LOGIN_TAB_LINGER, move || {
            let Some(browser) = this.upgrade() else {
                return;
            };
            let still_login = browser.session.borrow().tab(tab).is_some_and(|record| {
                record.url.starts_with("http://127.0.0.1:")
                    || record.url.contains("/oauth/authorize")
            });
            if still_login {
                browser.close_tab(tab);
            }
        });
    }

    /// The address of the sync server, as the settings name it.
    fn sync_server(&self) -> String {
        self.preferences
            .borrow()
            .sync_server
            .trim()
            .trim_end_matches('/')
            .to_owned()
    }

    pub(super) fn sync_server_edited(&self, address: &str) {
        let address = address.trim().to_owned();
        self.update_preference(PreferenceKey::SyncServer, |preferences| {
            preferences.sync_server = address;
        });
    }

    fn account_error(&self, message: &str) {
        self.for_all_windows(|browser| {
            if let Some(window) = browser.window() {
                window
                    .global::<SettingsState>()
                    .set_account_error(message.into());
            }
        });
    }

    fn show_account_everywhere(&self) {
        self.for_all_windows(Browser::show_account);
    }

    /// Fills the account part of the settings.
    pub(super) fn show_account(&self) {
        let Some(window) = self.window() else { return };
        let state = window.global::<SettingsState>();
        let server = self.sync_server();
        let host = server.split("://").nth(1).unwrap_or(&server).to_owned();
        let (phase, title, detail) = match &*self.account.borrow() {
            AccountState::SignedOut => (
                0,
                t!("Nicht angemeldet").to_owned(),
                t!("Mit deinem Onyx-Konto gleicht Atoll Tabs, Inseln, Workspaces und das Archiv zwischen deinen Geräten ab, Ende-zu-Ende verschlüsselt.").to_owned(),
            ),
            AccountState::SigningIn { .. } => (
                1,
                t!("Anmeldung läuft").to_owned(),
                t!("Melde dich im geöffneten Tab bei Onyx an.").to_owned(),
            ),
            AccountState::Checking => (
                3,
                t!("Anmeldung wird geprüft").to_owned(),
                host.clone(),
            ),
            AccountState::SignedIn { name, .. } => (
                2,
                name.clone()
                    .unwrap_or_else(|| t!("Angemeldet").to_owned()),
                match name {
                    Some(_) => t!("Angemeldet bei {}", host),
                    None => t!("Angemeldet bei {}, Onyx ist gerade nicht erreichbar", host),
                },
            ),
        };
        state.set_account_phase(phase);
        state.set_account_title(title.into());
        state.set_account_detail(detail.into());
        state.set_sync_server(self.preferences.borrow().sync_server.as_str().into());
        self.show_sync();
    }
}

fn name_of(profile: &Profile) -> Option<String> {
    profile
        .username
        .clone()
        .filter(|name| !name.is_empty())
        .or_else(|| Some(profile.sub.clone()))
}
