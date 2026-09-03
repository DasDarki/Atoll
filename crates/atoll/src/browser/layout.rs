use slint::ComponentHandle;

use super::Browser;
use super::preferences::PreferenceKey;
use crate::ui::{BrowserState, SidebarState};
use atoll_i18n::t;

impl Browser {
    /// Shows the sidebar, the bookmarks bar and the home button as configured.
    pub(super) fn restore_layout(&self) {
        let Some(window) = self.window() else { return };
        let preferences = self.preferences.borrow();
        window
            .global::<SidebarState>()
            .set_visible(preferences.sidebar_visible);
        window
            .global::<BrowserState>()
            .set_home_button_visible(preferences.home_button);
        drop(preferences);
        self.update_bookmarks_bar_visibility();
    }

    /// Follows preferences another window changed.
    pub(super) fn apply_shared_preferences(&self) {
        self.restore_layout();
        self.restore_dev_tools_layout();
    }

    pub fn toggle_sidebar(&self) {
        let visible = !self.preferences.borrow().sidebar_visible;
        self.set_sidebar_visible(visible);
    }

    pub(super) fn set_sidebar_visible(&self, visible: bool) {
        self.update_preference(PreferenceKey::SidebarVisible, |preferences| {
            preferences.sidebar_visible = visible;
        });
        if let Some(window) = self.window() {
            let state = window.global::<SidebarState>();
            state.set_visible(visible);
            state.set_hovering(false);
        }
    }

    pub fn toggle_bookmarks_bar(&self) {
        let visible = !self.preferences.borrow().bookmarks_bar_visible;
        self.set_bookmarks_bar_visible(visible);
        if visible && !self.is_bookmarks_bar_shown() {
            self.show_toast(t!(
                "Die Lesezeichenleiste erscheint, sobald sie Lesezeichen enthält"
            ));
        }
    }

    pub(super) fn set_bookmarks_bar_visible(&self, visible: bool) {
        self.update_preference(PreferenceKey::BookmarksBarVisible, |preferences| {
            preferences.bookmarks_bar_visible = visible;
        });
        self.update_bookmarks_bar_visibility();
    }

    pub(super) fn set_bookmarks_bar_auto_hide(&self, auto_hide: bool) {
        self.update_preference(PreferenceKey::BookmarksBarAutoHide, |preferences| {
            preferences.bookmarks_bar_auto_hide = auto_hide;
        });
        self.update_bookmarks_bar_visibility();
    }

    /// Shows the bookmarks bar if it is enabled and, with auto hide, not empty.
    pub(super) fn update_bookmarks_bar_visibility(&self) {
        if let Some(window) = self.window() {
            window
                .global::<BrowserState>()
                .set_bookmarks_bar_visible(self.is_bookmarks_bar_shown());
        }
    }

    fn is_bookmarks_bar_shown(&self) -> bool {
        let preferences = self.preferences.borrow();
        preferences.bookmarks_bar_visible
            && (!preferences.bookmarks_bar_auto_hide || self.has_bar_bookmarks())
    }
}
