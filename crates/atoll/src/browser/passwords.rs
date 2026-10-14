use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::time::{Duration, Instant};

use atoll_core::TabId;
use atoll_core::address::display_host;
use atoll_engine::{FilledFields, LoginFill, ViewId};
use atoll_onepassword::{Credentials, Login, OnePassword, OpError, logins_for, matches};
use slint::{ComponentHandle, ModelRc, Timer, TimerMode, VecModel};

use super::Browser;
use crate::platform::tasks::run_in_background;
use crate::ui::{OverlayState, PasswordItem, SettingsState};
use atoll_i18n::t;

/// How long the list of logins is reused before 1Password is asked again.
const LOGIN_CACHE: Duration = Duration::from_secs(5 * 60);

/// How long the frames of a page get to answer a fill.
const FILL_TIMEOUT: Duration = Duration::from_secs(3);

/// How long a copied secret stays in the clipboard before Atoll takes it back out, the way
/// 1Password does it itself. Only what is still there is cleared.
const CLIPBOARD_TIME: Duration = Duration::from_secs(90);

/// What a copy button of a login copies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Username,
    Password,
    Code,
}

impl Part {
    fn of(what: i32) -> Option<Self> {
        match what {
            0 => Some(Part::Username),
            1 => Some(Part::Password),
            2 => Some(Part::Code),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Part::Username => t!("Benutzername"),
            Part::Password => t!("Passwort"),
            Part::Code => t!("Einmalcode"),
        }
    }

    /// Whether what is copied is a secret that should not linger in the clipboard.
    fn is_secret(self) -> bool {
        self != Part::Username
    }

    fn value(self, credentials: &Credentials) -> Option<String> {
        match self {
            Part::Username => Some(credentials.username.clone()),
            Part::Password => Some(credentials.password.clone()),
            Part::Code => credentials.one_time_code.clone(),
        }
    }
}

/// 1Password, shared by all windows.
pub struct PasswordVault {
    cli: OnePassword,
    logins: RefCell<Option<(Instant, Rc<Vec<Login>>)>>,
    /// The pages that were filled by themselves already, so it happens once each.
    filled: RefCell<HashSet<(TabId, String)>>,
    /// The timer that takes a copied secret back out of the clipboard.
    clipboard: RefCell<Option<Timer>>,
}

impl PasswordVault {
    /// Uses `ATOLL_OP` as the 1Password CLI if set, `op` from the search path otherwise.
    pub fn from_environment() -> Self {
        let cli = std::env::var_os("ATOLL_OP")
            .map(OnePassword::new)
            .unwrap_or_default();
        Self {
            cli,
            logins: RefCell::default(),
            filled: RefCell::default(),
            clipboard: RefCell::default(),
        }
    }

    fn cached_logins(&self) -> Option<Rc<Vec<Login>>> {
        self.logins
            .borrow()
            .as_ref()
            .filter(|(fetched, _)| fetched.elapsed() < LOGIN_CACHE)
            .map(|(_, logins)| logins.clone())
    }

    fn remember_logins(&self, logins: Vec<Login>) -> Rc<Vec<Login>> {
        let logins = Rc::new(logins);
        *self.logins.borrow_mut() = Some((Instant::now(), logins.clone()));
        logins
    }
}

/// A fill that not every frame has answered yet.
pub struct PendingFill {
    view: ViewId,
    unanswered: usize,
    filled: FilledFields,
    code_copied: bool,
    /// A fill nobody asked for says nothing when it found no fields.
    quiet: bool,
    _timeout: Timer,
}

/// Why a login is being fetched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reason {
    /// The user chose it in the list; with `other_site` they agreed to use it elsewhere.
    Chosen { other_site: bool },
    /// Atoll fills it by itself because the site has exactly this one login.
    ByItself,
}

/// The login popup of one window.
#[derive(Default)]
pub struct PasswordPopup {
    page_url: String,
    all: Rc<Vec<Login>>,
    shown: Vec<Login>,
    query: String,
}

impl Browser {
    /// Ctrl+Shift+L, the key in the address bar and the palette: offers the 1Password logins of
    /// the current site.
    pub fn show_passwords(&self) {
        let Some(url) = self
            .current_page_url()
            .filter(|url| url.starts_with("https://") || url.starts_with("http://"))
        else {
            self.show_toast(t!("1Password füllt nur Anmeldungen auf Webseiten aus"));
            return;
        };
        let Some(window) = self.window() else { return };
        self.dismiss_suggestions();
        let overlay = window.global::<OverlayState>();
        overlay.set_passwords_host(display_host(&url).unwrap_or_default().into());
        overlay.set_passwords_query("".into());
        *self.passwords.borrow_mut() = PasswordPopup {
            page_url: url,
            ..PasswordPopup::default()
        };
        overlay.set_passwords_visible(true);
        match self.vault.cached_logins() {
            Some(logins) => self.show_logins(logins),
            None => self.load_logins(),
        }
    }

    pub fn close_passwords(&self) {
        *self.passwords.borrow_mut() = PasswordPopup::default();
        if let Some(window) = self.window() {
            window.global::<OverlayState>().set_passwords_visible(false);
        }
        self.focus_content();
    }

    pub fn password_query_changed(&self, query: &str) {
        self.passwords.borrow_mut().query = query.trim().to_lowercase();
        self.refresh_password_list();
    }

    /// Fills the login at `index` of the list into the page, after asking if it belongs to
    /// another site.
    pub fn choose_password(&self, index: i32) {
        let (login, page_url) = {
            let popup = self.passwords.borrow();
            let Some(login) = usize::try_from(index)
                .ok()
                .and_then(|index| popup.shown.get(index).cloned())
            else {
                return;
            };
            (login, popup.page_url.clone())
        };
        if matches(&login, &page_url) {
            self.fetch_credentials(login, Reason::Chosen { other_site: false });
            return;
        }
        let saved_for = login
            .urls
            .iter()
            .filter_map(|url| display_host(url))
            .next()
            .unwrap_or_else(|| t!("keine Adresse").to_owned());
        let host = display_host(&page_url).unwrap_or_default();
        let this = self.this();
        self.confirm(
            t!("Login einer anderen Seite verwenden?"),
            &t!(
                "„{}“ ist für {} gespeichert, nicht für {}. Füge die Zugangsdaten nur ein, wenn du der Seite vertraust.",
                login.title,
                saved_for,
                host
            ),
            t!("Trotzdem einfügen"),
            move || {
                if let Some(browser) = this.upgrade() {
                    browser.fetch_credentials(login, Reason::Chosen { other_site: true });
                }
            },
        );
    }

    /// A copy button of the list: puts one part of the login into the clipboard.
    pub fn copy_password_field(&self, index: i32, what: i32) {
        let Some(part) = Part::of(what) else { return };
        let login = {
            let popup = self.passwords.borrow();
            usize::try_from(index)
                .ok()
                .and_then(|index| popup.shown.get(index).cloned())
        };
        let Some(login) = login else { return };
        self.set_password_status(&t!("{} wird geholt…", part.name()), true);
        let cli = self.vault.cli.clone();
        let this = self.this();
        let id = login.id.clone();
        run_in_background(
            "onepassword",
            move || cli.credentials(&id),
            move |result| {
                if let Some(browser) = this.upgrade() {
                    browser.copy_credentials(part, &login, result);
                }
            },
        );
    }

    fn copy_credentials(&self, part: Part, login: &Login, result: Result<Credentials, OpError>) {
        let credentials = match result {
            Ok(credentials) => credentials,
            Err(error) => {
                tracing::warn!(%error, "1Password credentials could not be read");
                self.set_password_status(&error_text(&error), false);
                return;
            }
        };
        let Some(value) = part.value(&credentials).filter(|value| !value.is_empty()) else {
            self.set_password_status(&t!("„{}“ hat keinen {}", login.title, part.name()), false);
            return;
        };
        if !self.clipboard.set_text(&value) {
            self.set_password_status(t!("Zwischenablage nicht verfügbar"), false);
            return;
        }
        self.close_passwords();
        if part.is_secret() {
            self.clear_clipboard_later(value);
            self.show_toast(&t!(
                "{} kopiert, in anderthalb Minuten wieder entfernt",
                part.name()
            ));
        } else {
            self.show_toast(&t!("{} kopiert", part.name()));
        }
    }

    /// Takes `secret` out of the clipboard after a while, if nothing else was copied since.
    fn clear_clipboard_later(&self, secret: String) {
        let this = self.this();
        let timer = Timer::default();
        timer.start(TimerMode::SingleShot, CLIPBOARD_TIME, move || {
            let Some(browser) = this.upgrade() else {
                return;
            };
            if browser.clipboard.text().as_deref() == Some(secret.as_str()) {
                browser.clipboard.set_text("");
            }
            browser.vault.clipboard.borrow_mut().take();
        });
        *self.vault.clipboard.borrow_mut() = Some(timer);
    }

    /// A page shows a login form: fills it by itself when exactly one login is saved for the
    /// site and the user asked for that.
    pub(super) fn autofill_login(&self, tab: TabId) {
        if !self.preferences.borrow().password_autofill {
            return;
        }
        let Some(url) = self
            .current_page_url()
            .filter(|url| url.starts_with("https://") || url.starts_with("http://"))
        else {
            return;
        };
        if self.session.borrow().active_tab != Some(tab) || self.is_private(tab) {
            return;
        }
        if !self.vault.filled.borrow_mut().insert((tab, url.clone())) {
            return;
        }
        match self.vault.cached_logins() {
            Some(logins) => self.autofill_from(logins, &url),
            None => {
                let cli = self.vault.cli.clone();
                let this = self.this();
                run_in_background(
                    "onepassword",
                    move || cli.logins(),
                    move |result| {
                        let Some(browser) = this.upgrade() else {
                            return;
                        };
                        match result {
                            Ok(logins) => {
                                let logins = browser.vault.remember_logins(logins);
                                browser.autofill_from(logins, &url);
                            }
                            Err(error) => {
                                tracing::debug!(%error, "1Password did not answer for autofill");
                            }
                        }
                    },
                );
            }
        }
    }

    /// Fills when the site has exactly one login, so nothing is guessed.
    fn autofill_from(&self, logins: Rc<Vec<Login>>, url: &str) {
        if self.current_page_url().as_deref() != Some(url) {
            return;
        }
        let matching = logins_for(&logins, url);
        let [login] = matching.as_slice() else {
            return;
        };
        let login = (*login).clone();
        tracing::info!(title = %login.title, "filling a login by itself");
        self.fetch_credentials(login, Reason::ByItself);
    }

    /// The page changed, so it may be filled again.
    pub(super) fn forget_autofilled(&self, tab: TabId) {
        self.vault
            .filled
            .borrow_mut()
            .retain(|(filled, _)| *filled != tab);
    }

    fn load_logins(&self) {
        self.set_password_status(
            t!("1Password wird gefragt, eventuell musst du die App entsperren…"),
            true,
        );
        let cli = self.vault.cli.clone();
        let this = self.this();
        run_in_background(
            "onepassword",
            move || cli.logins(),
            move |result| {
                if let Some(browser) = this.upgrade() {
                    browser.logins_loaded(result);
                }
            },
        );
    }

    fn logins_loaded(&self, result: Result<Vec<Login>, OpError>) {
        match result {
            Ok(logins) => {
                let logins = self.vault.remember_logins(logins);
                if self.passwords_visible() {
                    self.show_logins(logins);
                }
            }
            Err(error) => {
                tracing::warn!(%error, "1Password logins could not be read");
                self.set_password_status(&error_text(&error), false);
            }
        }
    }

    fn show_logins(&self, logins: Rc<Vec<Login>>) {
        self.passwords.borrow_mut().all = logins;
        self.refresh_password_list();
    }

    fn refresh_password_list(&self) {
        let Some(window) = self.window() else { return };
        let (items, status) = {
            let mut popup = self.passwords.borrow_mut();
            let shown: Vec<Login> = if popup.query.is_empty() {
                logins_for(&popup.all, &popup.page_url)
                    .into_iter()
                    .cloned()
                    .collect()
            } else {
                popup
                    .all
                    .iter()
                    .filter(|login| mentions(login, &popup.query))
                    .take(50)
                    .cloned()
                    .collect()
            };
            let items: Vec<PasswordItem> = shown
                .iter()
                .map(|login| PasswordItem {
                    title: login.title.as_str().into(),
                    username: login.username.as_str().into(),
                    detail: login_detail(login).into(),
                    matches: matches(login, &popup.page_url),
                })
                .collect();
            let host = display_host(&popup.page_url).unwrap_or_default();
            let status = match (shown.is_empty(), popup.query.is_empty()) {
                (false, _) => String::new(),
                (true, true) => {
                    t!(
                        "Für {} ist kein Login gespeichert. Suche in allen Logins:",
                        host
                    )
                }
                (true, false) => t!("Kein Login gefunden").to_owned(),
            };
            popup.shown = shown;
            (items, status)
        };
        let overlay = window.global::<OverlayState>();
        overlay.set_passwords_items(ModelRc::from(Rc::new(VecModel::from(items))));
        overlay.set_passwords_status(status.into());
        overlay.set_passwords_busy(false);
    }

    fn fetch_credentials(&self, login: Login, reason: Reason) {
        if reason != Reason::ByItself {
            self.set_password_status(t!("Zugangsdaten werden geholt…"), true);
        }
        let cli = self.vault.cli.clone();
        let this = self.this();
        let id = login.id.clone();
        run_in_background(
            "onepassword",
            move || cli.credentials(&id),
            move |result| {
                if let Some(browser) = this.upgrade() {
                    browser.credentials_loaded(&login, reason, result);
                }
            },
        );
    }

    fn credentials_loaded(
        &self,
        login: &Login,
        reason: Reason,
        result: Result<Credentials, OpError>,
    ) {
        let by_itself = reason == Reason::ByItself;
        let other_site = reason == Reason::Chosen { other_site: true };
        let credentials = match result {
            Ok(credentials) => credentials,
            Err(error) => {
                if by_itself {
                    tracing::debug!(%error, "1Password did not answer for a fill by itself");
                    return;
                }
                tracing::warn!(%error, "1Password credentials could not be read");
                self.set_password_status(&error_text(&error), false);
                return;
            }
        };
        let page_url = if by_itself {
            self.current_page_url().unwrap_or_default()
        } else {
            self.passwords.borrow().page_url.clone()
        };
        if self.current_page_url().as_deref() != Some(&page_url)
            || (!by_itself && !self.passwords_visible())
        {
            self.close_passwords();
            if !by_itself {
                self.show_toast(t!("Die Seite hat sich geändert, es wurde nichts eingefügt"));
            }
            return;
        }
        let page_host = display_host(&page_url);
        let fill = LoginFill {
            username: &credentials.username,
            password: &credentials.password,
            one_time_code: credentials.one_time_code.as_deref(),
            // What the user typed themselves stays, a fill nobody asked for only adds.
            only_empty: by_itself,
        };
        let mut asked = None;
        self.with_live_view(|view| {
            let frames = view.fill_login(fill, |url| {
                matches(login, url) || (other_site && display_host(url) == page_host)
            });
            asked = Some((view.id(), frames));
        });
        self.close_passwords();
        let Some((view, frames)) = asked.filter(|(_, frames)| *frames > 0) else {
            if !by_itself {
                self.show_toast(t!(
                    "Das Anmeldefeld gehört zu einer anderen Seite, es wurde nichts eingefügt"
                ));
            }
            return;
        };
        tracing::info!(frames, "filling a login from 1Password");
        let code_copied = credentials
            .one_time_code
            .as_deref()
            .is_some_and(|code| self.clipboard.set_text(code));
        if code_copied {
            self.clear_clipboard_later(credentials.one_time_code.clone().unwrap_or_default());
        }
        let this = self.this();
        let timeout = Timer::default();
        timeout.start(TimerMode::SingleShot, FILL_TIMEOUT, move || {
            if let Some(browser) = this.upgrade() {
                browser.fill_timed_out();
            }
        });
        *self.pending_fill.borrow_mut() = Some(PendingFill {
            view,
            unanswered: frames,
            filled: FilledFields::default(),
            code_copied,
            quiet: by_itself,
            _timeout: timeout,
        });
    }

    /// A frame answered the fill with the fields it filled. The first frame that filled
    /// something, or the last one to answer, ends the fill.
    pub(super) fn login_filled(&self, view: ViewId, fields: FilledFields) {
        let finished = {
            let mut pending = self.pending_fill.borrow_mut();
            let Some(fill) = pending.as_mut().filter(|fill| fill.view == view) else {
                return;
            };
            fill.unanswered = fill.unanswered.saturating_sub(1);
            fill.filled.username |= fields.username;
            fill.filled.password |= fields.password;
            fill.filled.one_time_code |= fields.one_time_code;
            if fill.filled.any() || fill.unanswered == 0 {
                pending.take()
            } else {
                None
            }
        };
        if let Some(fill) = finished {
            tracing::info!(filled = ?fill.filled, "login filled");
            if !fill.quiet || fill.filled.any() {
                self.show_toast(&fill_message(fill.filled, fill.code_copied));
            }
        }
    }

    fn fill_timed_out(&self) {
        let Some(fill) = self.pending_fill.borrow_mut().take() else {
            return;
        };
        tracing::info!(unanswered = fill.unanswered, "no frame filled the login");
        if !fill.quiet || fill.filled.any() {
            self.show_toast(&fill_message(fill.filled, fill.code_copied));
        }
    }

    /// Settings: shows whether the 1Password CLI can be used.
    pub(super) fn show_password_settings(&self) {
        let Some(window) = self.window() else { return };
        let state = window.global::<SettingsState>();
        state.set_password_autofill(self.preferences.borrow().password_autofill);
        state.set_onepassword_status(t!("Die 1Password-CLI wird gesucht…").into());
        let cli = self.vault.cli.clone();
        let this = self.this();
        run_in_background(
            "onepassword",
            move || cli.version(),
            move |result| {
                if let Some(browser) = this.upgrade()
                    && let Some(window) = browser.window()
                {
                    let status = match result {
                        Ok(version) => format!(
                            "1Password-CLI {version} gefunden. Strg+Umschalt+L oder der Schlüssel \
                             in der Adressleiste füllt Anmeldungen aus, entsperrt wird in der \
                             1Password-App."
                        ),
                        Err(error) => error_text(&error),
                    };
                    window
                        .global::<SettingsState>()
                        .set_onepassword_status(status.into());
                }
            },
        );
    }

    fn set_password_status(&self, status: &str, busy: bool) {
        if let Some(window) = self.window() {
            let overlay = window.global::<OverlayState>();
            overlay.set_passwords_status(status.into());
            overlay.set_passwords_busy(busy);
        }
    }

    fn passwords_visible(&self) -> bool {
        self.window()
            .is_some_and(|window| window.global::<OverlayState>().get_passwords_visible())
    }
}

/// Whether `login` contains `query`, which is lower case, in its title, username or addresses.
fn mentions(login: &Login, query: &str) -> bool {
    login.title.to_lowercase().contains(query)
        || login.username.to_lowercase().contains(query)
        || login
            .urls
            .iter()
            .any(|url| url.to_lowercase().contains(query))
}

/// Vault and site of a login, like `Privat · github.com`.
fn login_detail(login: &Login) -> String {
    let site = login.urls.iter().find_map(|url| display_host(url));
    match (login.vault.is_empty(), site) {
        (false, Some(site)) => t!("{} · {}", login.vault, site),
        (false, None) => login.vault.clone(),
        (true, Some(site)) => site,
        (true, None) => String::new(),
    }
}

fn error_text(error: &OpError) -> String {
    match error {
        OpError::Missing => "Die 1Password-CLI ist nicht installiert. Installiere das Paket \
                             1password-cli und aktiviere in der 1Password-App unter Einstellungen \
                             › Entwickler „Mit 1Password CLI verbinden“."
            .to_owned(),
        OpError::SignedOut => {
            "Die 1Password-CLI ist nicht mit der App verbunden. Aktiviere in der \
                               1Password-App unter Einstellungen › Entwickler „Mit 1Password CLI \
                               verbinden“."
                .to_owned()
        }
        OpError::Cancelled => t!("Entsperren abgebrochen").to_owned(),
        OpError::Failed(message) | OpError::Invalid(message) => {
            t!("1Password meldet: {}", message)
        }
    }
}

/// What the toast says after a fill, for example `Benutzername und Passwort eingefügt`.
fn fill_message(filled: FilledFields, code_copied: bool) -> String {
    let names: Vec<&str> = [
        (filled.username, t!("Benutzername")),
        (filled.password, t!("Passwort")),
        (filled.one_time_code, t!("Einmalcode")),
    ]
    .into_iter()
    .filter_map(|(filled, name)| filled.then_some(name))
    .collect();
    let mut message = match names.as_slice() {
        [] => t!("Keine Anmeldefelder gefunden").to_owned(),
        [name] => t!("{} eingefügt", name),
        [first @ .., last] => t!("{} und {} eingefügt", first.join(", "), last),
    };
    if code_copied && !filled.one_time_code {
        message.push_str(t!(", Einmalcode in der Zwischenablage"));
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    fn login(title: &str, username: &str, vault: &str, urls: &[&str]) -> Login {
        Login {
            id: title.to_lowercase(),
            title: title.to_owned(),
            username: username.to_owned(),
            vault: vault.to_owned(),
            urls: urls.iter().map(|url| (*url).to_owned()).collect(),
        }
    }

    #[test]
    fn searching_looks_at_title_username_and_addresses() {
        let forum = login(
            "Forum",
            "kim@example.org",
            "Privat",
            &["https://forum.example"],
        );
        assert!(mentions(&forum, "forum"));
        assert!(mentions(&forum, "kim@"));
        assert!(mentions(&forum, "forum.example"));
        assert!(!mentions(&forum, "bank"));
    }

    #[test]
    fn details_name_vault_and_site() {
        assert_eq!(
            login_detail(&login("A", "", "Privat", &["https://www.github.com/login"])),
            "Privat · github.com"
        );
        assert_eq!(login_detail(&login("A", "", "Arbeit", &[])), "Arbeit");
        assert_eq!(login_detail(&login("A", "", "", &[])), "");
    }

    #[test]
    fn fill_messages_name_the_filled_fields() {
        let fields = |username, password, one_time_code| FilledFields {
            username,
            password,
            one_time_code,
        };
        assert_eq!(
            fill_message(fields(true, true, false), false),
            "Benutzername und Passwort eingefügt"
        );
        assert_eq!(
            fill_message(fields(false, true, false), true),
            "Passwort eingefügt, Einmalcode in der Zwischenablage"
        );
        assert_eq!(
            fill_message(fields(true, false, true), true),
            "Benutzername und Einmalcode eingefügt"
        );
        assert_eq!(
            fill_message(fields(true, true, true), false),
            "Benutzername, Passwort und Einmalcode eingefügt"
        );
        assert_eq!(
            fill_message(FilledFields::default(), false),
            "Keine Anmeldefelder gefunden"
        );
    }

    #[test]
    fn errors_explain_what_to_do() {
        assert!(error_text(&OpError::Missing).contains("1password-cli"));
        assert!(error_text(&OpError::SignedOut).contains("Entwickler"));
        assert_eq!(
            error_text(&OpError::Failed("item not found".to_owned())),
            "1Password meldet: item not found"
        );
    }
}
