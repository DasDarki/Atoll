use std::path::PathBuf;
use std::rc::Rc;

use atoll_storage::prelude::*;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use super::Browser;
use super::navigation::resolve_input;
use super::preferences::{
    ArchiveFallback, MAX_ARCHIVE_SUGGESTION_WEEKS, MAX_FROZEN_LIMIT, PreferenceKey, Preferences,
    SEARCH_ENGINES,
};
use crate::platform::dialogs::{self, FileDialogKind, FileDialogOptions};
use crate::ui::{BrowserState, SettingsState};
use atoll_i18n::t;

impl Browser {
    pub fn open_settings(&self) {
        let Some(window) = self.window() else { return };
        self.dismiss_suggestions();
        let preferences = self.preferences.borrow().clone();
        let state = window.global::<SettingsState>();
        let engines: Vec<SharedString> = SEARCH_ENGINES
            .iter()
            .map(|engine| engine.name.into())
            .chain(std::iter::once(t!("Eigene Adresse").into()))
            .collect();
        state.set_search_engines(ModelRc::from(Rc::new(VecModel::from(engines))));
        let languages: Vec<SharedString> = std::iter::once(SharedString::from(t!("Systemsprache")))
            .chain(
                atoll_i18n::Language::ALL
                    .iter()
                    .map(|language| SharedString::from(language.label())),
            )
            .collect();
        state.set_languages(ModelRc::from(Rc::new(VecModel::from(languages))));
        state.set_language(preferences.language.map_or(0, |chosen| {
            atoll_i18n::Language::ALL
                .iter()
                .position(|language| *language == chosen)
                .map_or(0, |index| index as i32 + 1)
        }));
        state.set_search_engine(preferences.search_engine_index() as i32);
        state.set_search_url(preferences.search_url.as_str().into());
        state.set_search_url_valid(true);
        state.set_start_page(preferences.start_page.as_str().into());
        state.set_new_tab_start_page(preferences.new_tab_start_page);
        state.set_home_button(preferences.home_button);
        state.set_sidebar_visible(preferences.sidebar_visible);
        state.set_bookmarks_bar_visible(preferences.bookmarks_bar_visible);
        state.set_bookmarks_bar_auto_hide(preferences.bookmarks_bar_auto_hide);
        state.set_frozen_limit(preferences.frozen_limit as i32);
        state.set_max_frozen_limit(MAX_FROZEN_LIMIT as i32);
        state.set_archive_suggestion_weeks(preferences.archive_suggestion_weeks as i32);
        state.set_max_archive_suggestion_weeks(MAX_ARCHIVE_SUGGESTION_WEEKS as i32);
        state.set_archive_fallback(match preferences.archive_fallback {
            ArchiveFallback::Off => 0,
            ArchiveFallback::Ask => 1,
            ArchiveFallback::Automatic => 2,
        });
        state.set_archive_on_missing(preferences.archive_on_missing);
        state.set_download_ask(preferences.download_ask);
        state.set_download_panel(preferences.download_panel);
        self.show_download_directory();
        self.show_adblock_settings();
        self.show_vpn_settings();
        self.show_memory_settings();
        self.show_password_settings();
        self.show_plugin_settings();
        state.set_visible(true);
    }

    pub fn close_settings(&self) {
        if let Some(window) = self.window() {
            window.global::<SettingsState>().set_visible(false);
        }
        self.focus_content();
    }

    /// Applies the setting `key` after the settings page changed it.
    pub fn setting_changed(&self, key: &str) {
        let Some(window) = self.window() else { return };
        let state = window.global::<SettingsState>();
        match key {
            "start-page" => {
                let start_page = state.get_start_page().trim().to_owned();
                self.set_start_page(&start_page);
                state.set_new_tab_start_page(self.preferences.borrow().new_tab_start_page);
            }
            "new-tab-start-page" => {
                let enabled = state.get_new_tab_start_page();
                self.update_preference(PreferenceKey::NewTabStartPage, |preferences| {
                    preferences.new_tab_start_page = enabled;
                });
            }
            "home-button" => self.set_home_button(state.get_home_button()),
            "archive-fallback" => {
                let mode = match state.get_archive_fallback() {
                    0 => ArchiveFallback::Off,
                    2 => ArchiveFallback::Automatic,
                    _ => ArchiveFallback::Ask,
                };
                self.update_preference(PreferenceKey::ArchiveFallback, |preferences| {
                    preferences.archive_fallback = mode;
                });
            }
            "archive-on-missing" => {
                let enabled = state.get_archive_on_missing();
                self.update_preference(PreferenceKey::ArchiveOnMissing, |preferences| {
                    preferences.archive_on_missing = enabled;
                });
            }
            "language" => {
                let index = usize::try_from(state.get_language()).unwrap_or_default();
                self.set_language(
                    index
                        .checked_sub(1)
                        .and_then(|index| atoll_i18n::Language::ALL.get(index).copied()),
                );
            }
            "search-engine" => {
                let index = usize::try_from(state.get_search_engine()).unwrap_or_default();
                if let Some(engine) = SEARCH_ENGINES.get(index) {
                    self.set_search_url(engine.url);
                    state.set_search_url(engine.url.into());
                }
                state.set_search_url_valid(true);
            }
            "search-url" => {
                let url = state.get_search_url().trim().to_owned();
                let valid = is_valid_search_url(&url);
                state.set_search_url_valid(valid);
                if valid {
                    self.set_search_url(&url);
                }
            }
            "sidebar-visible" => self.set_sidebar_visible(state.get_sidebar_visible()),
            "bookmarks-bar-visible" => {
                self.set_bookmarks_bar_visible(state.get_bookmarks_bar_visible())
            }
            "bookmarks-bar-auto-hide" => {
                self.set_bookmarks_bar_auto_hide(state.get_bookmarks_bar_auto_hide())
            }
            "download-ask" => {
                let ask = state.get_download_ask();
                self.update_preference(PreferenceKey::DownloadAsk, |preferences| {
                    preferences.download_ask = ask;
                });
                self.apply_download_options();
            }
            "download-panel" => {
                let open = state.get_download_panel();
                self.update_preference(PreferenceKey::DownloadPanel, |preferences| {
                    preferences.download_panel = open;
                });
            }
            "adblock-enabled" => self.set_adblock_enabled(state.get_adblock_enabled()),
            "vpn-enabled" => self.set_vpn_enabled(state.get_vpn_enabled()),
            "vpn-kind" => {
                let kind = match state.get_vpn_kind() {
                    1 => "http",
                    2 => "https",
                    _ => "socks5",
                };
                self.update_preference(PreferenceKey::VpnKind, |preferences| {
                    preferences.vpn_kind = kind.to_owned();
                });
                self.vpn_settings_changed();
            }
            "vpn-host" => {
                let host = state.get_vpn_host().trim().to_owned();
                self.update_preference(PreferenceKey::VpnHost, |preferences| {
                    preferences.vpn_host = host;
                });
                self.vpn_settings_changed();
            }
            "vpn-port" => {
                let port = state.get_vpn_port().clamp(1, i32::from(u16::MAX)) as u32;
                self.update_preference(PreferenceKey::VpnPort, |preferences| {
                    preferences.vpn_port = port;
                });
                self.vpn_settings_changed();
            }
            "vpn-user" => {
                let user = state.get_vpn_user().to_string();
                self.update_preference(PreferenceKey::VpnUser, |preferences| {
                    preferences.vpn_user = user;
                });
                self.vpn_settings_changed();
            }
            "vpn-password" => {
                let password = state.get_vpn_password().to_string();
                self.update_preference(PreferenceKey::VpnPassword, |preferences| {
                    preferences.vpn_password = password;
                });
                self.vpn_settings_changed();
            }
            "cookie-banners-hide" => {
                self.set_hide_cookie_banners(state.get_cookie_banners_hide());
            }
            "cookie-banners-answer" => {
                self.set_answer_cookie_banners(state.get_cookie_banners_answer());
            }
            "memory-limit" => self.set_memory_limit(
                state.get_memory_limit_enabled(),
                u32::try_from(state.get_memory_limit_gb()).unwrap_or(1),
            ),
            "archive-suggestion-weeks" => {
                let weeks = u32::try_from(state.get_archive_suggestion_weeks())
                    .unwrap_or_default()
                    .min(MAX_ARCHIVE_SUGGESTION_WEEKS);
                self.update_preference(PreferenceKey::ArchiveSuggestionWeeks, |preferences| {
                    preferences.archive_suggestion_weeks = weeks;
                });
                self.for_all_windows(Browser::refresh_sidebar);
            }
            "frozen-limit" => {
                let limit = usize::try_from(state.get_frozen_limit()).unwrap_or_default();
                self.set_frozen_limit(limit);
            }
            other => tracing::warn!(key = other, "unknown setting"),
        }
    }

    pub fn choose_download_directory(&self) {
        let this = self.this();
        dialogs::show(
            FileDialogOptions {
                kind: FileDialogKind::OpenFolder,
                title: t!("Download-Ordner wählen").into(),
                directory: Some(self.download_directory()),
                file_name: None,
                filters: Vec::new(),
            },
            move |paths| {
                let Some(browser) = this.upgrade() else {
                    return;
                };
                if let Some(directory) = paths.and_then(|paths| paths.into_iter().next()) {
                    browser.set_download_directory(Some(directory));
                }
            },
        );
    }

    pub fn reset_download_directory(&self) {
        self.set_download_directory(None);
    }

    fn set_download_directory(&self, directory: Option<PathBuf>) {
        self.update_preference(PreferenceKey::DownloadDirectory, |preferences| {
            preferences.download_directory = directory;
        });
        self.apply_download_options();
        self.show_download_directory();
    }

    fn show_download_directory(&self) {
        if let Some(window) = self.window() {
            let state = window.global::<SettingsState>();
            state.set_download_directory(
                self.download_directory().to_string_lossy().as_ref().into(),
            );
            state.set_download_directory_custom(
                self.preferences.borrow().download_directory.is_some(),
            );
        }
    }

    /// Switches the language of the whole user interface, `None` to follow the system.
    fn set_language(&self, language: Option<atoll_i18n::Language>) {
        self.update_preference(PreferenceKey::Language, |preferences| {
            preferences.language = language;
        });
        let chosen = self.preferences.borrow().language();
        atoll_i18n::set_language(chosen);
        crate::app::select_interface_language(chosen);
        self.for_all_windows(Browser::relabel);
    }

    /// Builds the texts again that Atoll puts together in Rust, after the language changed.
    pub(super) fn relabel(&self) {
        self.open_settings();
        self.refresh_sidebar();
    }

    pub(super) fn update_preference(
        &self,
        key: PreferenceKey,
        change: impl FnOnce(&mut Preferences),
    ) {
        {
            let mut preferences = self.preferences.borrow_mut();
            change(&mut preferences);
            preferences.store(&self.storage, key);
        }
        self.for_other_windows(Browser::apply_shared_preferences);
    }

    pub(super) fn set_start_page(&self, start_page: &str) {
        self.update_preference(PreferenceKey::StartPage, |preferences| {
            preferences.start_page = start_page.to_owned();
        });
        if start_page.is_empty() && self.preferences.borrow().new_tab_start_page {
            self.update_preference(PreferenceKey::NewTabStartPage, |preferences| {
                preferences.new_tab_start_page = false;
            });
        }
    }

    pub(super) fn set_home_button(&self, visible: bool) {
        self.update_preference(PreferenceKey::HomeButton, |preferences| {
            preferences.home_button = visible;
        });
        if let Some(window) = self.window() {
            window
                .global::<BrowserState>()
                .set_home_button_visible(visible);
        }
    }

    pub(super) fn set_search_url(&self, url: &str) {
        self.update_preference(PreferenceKey::SearchUrl, |preferences| {
            preferences.search_url = url.to_owned();
        });
    }

    fn set_frozen_limit(&self, limit: usize) {
        let limit = limit.min(MAX_FROZEN_LIMIT);
        self.update_preference(PreferenceKey::FrozenLimit, |preferences| {
            preferences.frozen_limit = limit;
        });
        let commands = self.lifecycle.borrow_mut().set_frozen_limit(limit);
        if self.is_engine_running() {
            self.apply(commands);
        }
        self.refresh_sidebar();
    }

    /// Settings page and palette: uses the address of the active tab as the start page.
    pub fn use_current_page_as_start_page(&self) {
        let Some(url) = self.current_page_url() else {
            return;
        };
        self.set_start_page(&url);
        if let Some(window) = self.window() {
            window
                .global::<SettingsState>()
                .set_start_page(url.as_str().into());
        }
        self.show_toast(t!("Startseite festgelegt"));
    }

    /// The address new tabs load, or `None` for the Atoll start page.
    pub(super) fn new_tab_address(&self) -> Option<String> {
        let preferences = self.preferences.borrow();
        if !preferences.new_tab_start_page {
            return None;
        }
        resolve_input(&preferences.start_page, &preferences.search_url)
    }

    /// Home button and Alt+Home: loads the start page, or opens the Atoll start page in a new tab.
    pub fn go_home(&self) {
        let start_page = self.preferences.borrow().start_page.clone();
        if start_page.trim().is_empty() {
            self.new_tab();
        } else {
            self.navigate(&start_page);
        }
    }

    pub fn clear_history(&self) {
        let this = self.this();
        self.confirm(
            t!("Verlauf löschen?"),
            t!("Der gesamte Verlauf wird gelöscht. Tabs, Inseln und Lesezeichen bleiben erhalten."),
            t!("Löschen"),
            move || {
                if let Some(browser) = this.upgrade() {
                    match browser.storage.clear_history() {
                        Ok(()) => {
                            browser.forget_shown_history();
                            browser.show_toast(t!("Verlauf gelöscht"));
                        }
                        Err(error) => browser.report(t!("Verlauf löschen"), Err::<(), _>(error)),
                    }
                }
            },
        );
    }

    pub fn clear_cookies(&self) {
        let this = self.this();
        self.confirm(
            t!("Alle Cookies löschen?"),
            t!("Du wirst auf allen Websites abgemeldet."),
            t!("Löschen"),
            move || {
                let Some(browser) = this.upgrade() else {
                    return;
                };
                let deleted = browser
                    .engine
                    .borrow()
                    .as_ref()
                    .is_some_and(|engine| engine.delete_all_cookies());
                browser.show_toast(if deleted {
                    t!("Cookies gelöscht")
                } else {
                    t!("Cookies konnten nicht gelöscht werden")
                });
            },
        );
    }
}

fn is_valid_search_url(url: &str) -> bool {
    url.contains("{query}") && (url.starts_with("https://") || url.starts_with("http://"))
}

#[cfg(test)]
mod tests {
    use super::is_valid_search_url;

    #[test]
    fn search_addresses_need_a_placeholder_and_a_web_scheme() {
        assert!(is_valid_search_url("https://example.org/?q={query}"));
        assert!(!is_valid_search_url("https://example.org/?q="));
        assert!(!is_valid_search_url("example.org/?q={query}"));
    }
}
