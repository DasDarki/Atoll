use atoll_core::TabId;
use atoll_engine::ZoomCommand;
use atoll_storage::prelude::*;
use slint::ComponentHandle;

use super::Browser;
use crate::ui::{AppWindow, BrowserState};

impl Browser {
    /// Loads what the user typed into the address bar or the new tab page.
    pub fn navigate(&self, input: &str) {
        let Some(url) = resolve_input(input, &self.preferences.search_url) else {
            return;
        };
        let active = self.session.borrow().active_tab;
        let Some(tab) = active else {
            self.open_in_new_tab(None, &url, true);
            return;
        };

        let blank = self
            .session
            .borrow()
            .tab(tab)
            .is_some_and(|record| record.is_blank());
        let loaded = !blank
            && self
                .views
                .borrow_mut()
                .get_mut(tab)
                .map(|entry| {
                    entry.replacement_history = false;
                    entry.crashed = false;
                    entry.view.load_url(&url);
                })
                .is_some();
        if !loaded {
            if let Err(error) = self.storage.update_tab_address(tab, &url) {
                self.report("Navigation", Err::<(), _>(error));
                return;
            }
            if let Some(record) = self.session.borrow_mut().tab_mut(tab) {
                record.url = url.clone();
                record.title = None;
            }
            self.activate_tab(tab);
        }
        self.focus_content();
    }

    pub fn go_back(&self) {
        let Some(tab) = self.session.borrow().active_tab else {
            return;
        };
        let use_engine = self
            .views
            .borrow()
            .get(tab)
            .is_some_and(|entry| entry.view.can_go_back() && !entry.replacement_history);
        if use_engine {
            if let Some(entry) = self.views.borrow().get(tab) {
                entry.view.go_back();
            }
            return;
        }
        let current = self.current_address(tab);
        match self.storage.step_back(tab, &current) {
            Ok(Some(previous)) => {
                if let Some(entry) = self.views.borrow_mut().get_mut(tab) {
                    entry.replacement_history = true;
                    entry.expected_address = Some(previous.clone());
                    entry.view.load_url(&previous);
                }
            }
            Ok(None) => {}
            Err(error) => tracing::warn!(%error, "replacement history unavailable"),
        }
    }

    pub fn go_forward(&self) {
        self.with_live_view(|view| view.go_forward());
    }

    pub fn reload(&self, ignore_cache: bool) {
        let Some(tab) = self.session.borrow().active_tab else {
            return;
        };
        let reloaded = self
            .views
            .borrow_mut()
            .get_mut(tab)
            .map(|entry| {
                entry.crashed = false;
                entry.view.reload(ignore_cache);
            })
            .is_some();
        if reloaded {
            self.sync_page();
        } else {
            self.activate_tab(tab);
        }
    }

    pub fn stop(&self) {
        self.with_live_view(|view| view.stop());
    }

    pub fn zoom(&self, command: ZoomCommand) {
        self.with_live_view(|view| view.zoom(command));
    }

    pub fn show_dev_tools(&self) {
        self.with_live_view(|view| view.show_dev_tools(None));
    }

    pub fn toggle_fullscreen(&self) {
        let Some(window) = self.window() else { return };
        let state = window.global::<BrowserState>();
        let fullscreen = !state.get_fullscreen();
        if !fullscreen {
            self.with_live_view(|view| view.exit_fullscreen());
        }
        state.set_fullscreen(fullscreen);
    }

    pub(super) fn set_page_fullscreen(&self, fullscreen: bool) {
        if let Some(window) = self.window() {
            window.global::<BrowserState>().set_fullscreen(fullscreen);
        }
    }

    pub fn open_find(&self) {
        let Some(window) = self.window() else { return };
        if !window.global::<BrowserState>().get_has_page() {
            return;
        }
        window.global::<BrowserState>().set_find_visible(true);
        window.invoke_focus_find();
    }

    pub fn find(&self, text: &str, forward: bool) {
        let find_next = {
            let mut previous = self.find_text.borrow_mut();
            let same = *previous == text;
            *previous = text.to_owned();
            same
        };
        if text.is_empty() {
            self.with_live_view(|view| view.stop_finding());
            if let Some(window) = self.window() {
                window.global::<BrowserState>().set_find_result("".into());
            }
            return;
        }
        self.with_live_view(|view| view.find(text, forward, find_next));
    }

    pub fn close_find(&self) {
        let Some(window) = self.window() else { return };
        let state = window.global::<BrowserState>();
        if !state.get_find_visible() {
            return;
        }
        state.set_find_visible(false);
        state.set_find_result("".into());
        self.find_text.borrow_mut().clear();
        self.with_live_view(|view| view.stop_finding());
        self.focus_content();
    }

    /// Moves keyboard focus to the page, or to the new tab page if no page is shown.
    ///
    /// Runs on the next event loop iteration, so that overlays closed by the current event are
    /// gone and cannot take the focus back. Does nothing if another overlay opened meanwhile.
    pub(super) fn focus_content(&self) {
        let window = self.window.clone();
        slint::Timer::single_shot(std::time::Duration::ZERO, move || {
            let Some(window) = window.upgrade() else {
                return;
            };
            if window.get_modal_open() {
                return;
            }
            if window.global::<BrowserState>().get_has_page() {
                window.invoke_focus_web();
            } else {
                window.invoke_focus_new_tab();
            }
        });
    }

    pub fn focus_address(&self) {
        if let Some(window) = self.window() {
            focus_address_bar(&window);
        }
    }

    pub(super) fn has_replacement_history(&self, tab: TabId) -> bool {
        self.storage
            .tab_history(tab, 2)
            .map(|entries| entries.len() > 1)
            .unwrap_or(false)
    }

    fn current_address(&self, tab: TabId) -> String {
        self.views
            .borrow()
            .get(tab)
            .and_then(|entry| entry.view.url())
            .or_else(|| {
                self.session
                    .borrow()
                    .tab(tab)
                    .map(|record| record.url.clone())
            })
            .unwrap_or_default()
    }

    pub(super) fn with_live_view(&self, action: impl FnOnce(&atoll_engine::BrowserView)) {
        let active = self.session.borrow().active_tab;
        let views = self.views.borrow();
        if let Some(entry) = active.and_then(|tab| views.get(tab)) {
            action(&entry.view);
        }
    }
}

fn focus_address_bar(window: &AppWindow) {
    window.invoke_focus_address();
}

/// What address bar input stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Resolved {
    Address(String),
    Search(String),
}

impl Resolved {
    pub(super) fn into_url(self) -> String {
        match self {
            Resolved::Address(url) | Resolved::Search(url) => url,
        }
    }
}

/// Turns address bar input into an address: URLs are used as they are, everything else is a
/// search.
pub(super) fn resolve(input: &str, search_url: &str) -> Option<Resolved> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    if is_local_address(input) {
        return atoll_transfer::validate_address(&format!("http://{input}"))
            .ok()
            .map(Resolved::Address);
    }
    if let Ok(url) = atoll_transfer::validate_address(input) {
        let explicit_scheme =
            input.contains("://") || input.starts_with("about:") || input.starts_with("data:");
        let looks_like_host =
            !input.contains(' ') && (input.contains('.') || input.starts_with("localhost"));
        if explicit_scheme || looks_like_host {
            return Some(Resolved::Address(url));
        }
    }
    let query: String = url::form_urlencoded::byte_serialize(input.as_bytes()).collect();
    Some(Resolved::Search(search_url.replace("{query}", &query)))
}

pub(super) fn resolve_input(input: &str, search_url: &str) -> Option<String> {
    resolve(input, search_url).map(Resolved::into_url)
}

pub(super) fn is_local_address(input: &str) -> bool {
    let host = input.split(['/', ':']).next().unwrap_or_default();
    !input.contains("://") && (host == "localhost" || host.parse::<std::net::Ipv4Addr>().is_ok())
}

#[cfg(test)]
mod tests {
    use super::resolve_input;

    const SEARCH: &str = "https://search.example/?q={query}";

    #[test]
    fn addresses_are_loaded_directly() {
        assert_eq!(
            resolve_input("https://rust-lang.org", SEARCH).unwrap(),
            "https://rust-lang.org/"
        );
        assert_eq!(
            resolve_input("github.com/brave", SEARCH).unwrap(),
            "https://github.com/brave"
        );
        assert_eq!(
            resolve_input("localhost:8080", SEARCH).unwrap(),
            "http://localhost:8080/"
        );
        assert_eq!(
            resolve_input("192.168.1.1/admin", SEARCH).unwrap(),
            "http://192.168.1.1/admin"
        );
    }

    #[test]
    fn everything_else_is_searched() {
        assert_eq!(
            resolve_input("rust borrow checker", SEARCH).unwrap(),
            "https://search.example/?q=rust+borrow+checker"
        );
        assert_eq!(
            resolve_input("wetter", SEARCH).unwrap(),
            "https://search.example/?q=wetter"
        );
        assert_eq!(resolve_input("  ", SEARCH), None);
    }
}
