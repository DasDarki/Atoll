use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use atoll_core::address::display_host;
use atoll_onepassword::{Credentials, Login, OnePassword, OpError, logins_for, matches};
use slint::{ComponentHandle, ModelRc, VecModel};

use super::Browser;
use crate::platform::tasks::run_in_background;
use crate::ui::{OverlayState, PasswordItem, SettingsState};

/// How long the list of logins is reused before 1Password is asked again.
const LOGIN_CACHE: Duration = Duration::from_secs(5 * 60);

/// 1Password, shared by all windows.
pub struct PasswordVault {
    cli: OnePassword,
    logins: RefCell<Option<(Instant, Rc<Vec<Login>>)>>,
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
            self.show_toast("1Password füllt nur Anmeldungen auf Webseiten aus");
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
            self.fetch_credentials(login, false);
            return;
        }
        let saved_for = login
            .urls
            .iter()
            .filter_map(|url| display_host(url))
            .next()
            .unwrap_or_else(|| "keine Adresse".to_owned());
        let host = display_host(&page_url).unwrap_or_default();
        let this = self.this();
        self.confirm(
            "Login einer anderen Seite verwenden?",
            &format!(
                "„{}“ ist für {saved_for} gespeichert, nicht für {host}. Füge die Zugangsdaten nur ein, wenn du der Seite vertraust.",
                login.title
            ),
            "Trotzdem einfügen",
            move || {
                if let Some(browser) = this.upgrade() {
                    browser.fetch_credentials(login, true);
                }
            },
        );
    }

    fn load_logins(&self) {
        self.set_password_status(
            "1Password wird gefragt, eventuell musst du die App entsperren…",
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
                    format!("Für {host} ist kein Login gespeichert. Suche in allen Logins:")
                }
                (true, false) => "Kein Login gefunden".to_owned(),
            };
            popup.shown = shown;
            (items, status)
        };
        let overlay = window.global::<OverlayState>();
        overlay.set_passwords_items(ModelRc::from(Rc::new(VecModel::from(items))));
        overlay.set_passwords_status(status.into());
        overlay.set_passwords_busy(false);
    }

    fn fetch_credentials(&self, login: Login, other_site: bool) {
        self.set_password_status("Zugangsdaten werden geholt…", true);
        let cli = self.vault.cli.clone();
        let this = self.this();
        let id = login.id.clone();
        run_in_background(
            "onepassword",
            move || cli.credentials(&id),
            move |result| {
                if let Some(browser) = this.upgrade() {
                    browser.credentials_loaded(&login, other_site, result);
                }
            },
        );
    }

    fn credentials_loaded(
        &self,
        login: &Login,
        other_site: bool,
        result: Result<Credentials, OpError>,
    ) {
        let credentials = match result {
            Ok(credentials) => credentials,
            Err(error) => {
                tracing::warn!(%error, "1Password credentials could not be read");
                self.set_password_status(&error_text(&error), false);
                return;
            }
        };
        let page_url = self.passwords.borrow().page_url.clone();
        if !self.passwords_visible() || self.current_page_url().as_deref() != Some(&page_url) {
            self.close_passwords();
            self.show_toast("Die Seite hat sich geändert, es wurde nichts eingefügt");
            return;
        }
        let script = fill_script(&credentials);
        let mut filled = false;
        self.with_live_view(|view| {
            let focused = view.focused_frame_url().unwrap_or_default();
            if other_site || matches(login, &focused) {
                view.execute_script_in_focused_frame(&script);
                filled = true;
            } else if matches(login, &page_url) {
                view.execute_script(&script);
                filled = true;
            }
        });
        self.close_passwords();
        if !filled {
            self.show_toast(
                "Das Anmeldefeld gehört zu einer anderen Seite, es wurde nichts eingefügt",
            );
            return;
        }
        if let Some(code) = &credentials.one_time_code {
            self.copy_text(code);
            self.show_toast("Einmalcode in die Zwischenablage kopiert");
        }
    }

    /// Settings: shows whether the 1Password CLI can be used.
    pub(super) fn show_password_settings(&self) {
        let Some(window) = self.window() else { return };
        window
            .global::<SettingsState>()
            .set_onepassword_status("Die 1Password-CLI wird gesucht…".into());
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
        (false, Some(site)) => format!("{} · {site}", login.vault),
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
        OpError::Cancelled => "Entsperren abgebrochen".to_owned(),
        OpError::Failed(message) | OpError::Invalid(message) => {
            format!("1Password meldet: {message}")
        }
    }
}

/// The script filling `credentials` into the visible login form of a frame.
fn fill_script(credentials: &Credentials) -> String {
    let arguments = serde_json::json!([
        credentials.username,
        credentials.password,
        credentials.one_time_code.clone().unwrap_or_default(),
    ]);
    format!("({FILL_FUNCTION})(...{arguments});")
}

/// Fills username, password or one-time code into the visible fields that look like them, the
/// way typing would, so pages built with frameworks notice the values.
const FILL_FUNCTION: &str = r#"(username, secret, code) => {
  const visible = (field) => field.getClientRects().length > 0
    && getComputedStyle(field).visibility !== "hidden" && !field.disabled && !field.readOnly;
  const fill = (field, value) => {
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set;
    field.focus();
    setter.call(field, value);
    field.dispatchEvent(new Event("input", { bubbles: true }));
    field.dispatchEvent(new Event("change", { bubbles: true }));
  };
  const hint = (field) => [field.autocomplete, field.name, field.id, field.placeholder].join(" ").toLowerCase();
  const textual = (field) => ["text", "email", "tel", ""].includes(field.type);
  const fields = [...document.querySelectorAll("input")].filter(visible);
  const password = fields.find((field) => field.type === "password");
  const oneTime = fields.find((field) => field.autocomplete === "one-time-code"
    || /otp|totp|2fa|mfa|one.?time|verification|einmal/.test(hint(field)));
  const user = fields.find((field) => field.autocomplete === "username" && textual(field))
    || (password
      ? fields.slice(0, fields.indexOf(password)).reverse().find(textual)
      : fields.find((field) => textual(field) && field !== oneTime
        && /user|login|mail|konto|account|name/.test(hint(field))));
  if (user && username) fill(user, username);
  if (password && secret) fill(password, secret);
  if (!password && oneTime && code) fill(oneTime, code);
}"#;

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
    fn credentials_are_passed_as_json_arguments() {
        let script = fill_script(&Credentials {
            username: "kim".to_owned(),
            password: "a\"b</script>\\".to_owned(),
            one_time_code: None,
        });
        assert!(script.starts_with("((username, secret, code) =>"));
        assert!(script.ends_with(r#")(...["kim","a\"b</script>\\",""]);"#));
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
