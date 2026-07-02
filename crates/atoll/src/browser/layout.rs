use atoll_storage::prelude::*;
use slint::ComponentHandle;

use super::Browser;
use crate::ui::{BrowserState, SidebarState};

const SIDEBAR_VISIBLE_KEY: &str = "sidebar_visible";
const BOOKMARKS_BAR_VISIBLE_KEY: &str = "bookmarks_bar_visible";

impl Browser {
    /// Shows the sidebar and the bookmarks bar as they were left.
    pub(super) fn restore_layout(&self) {
        let Some(window) = self.window() else { return };
        window
            .global::<SidebarState>()
            .set_visible(self.stored_flag(SIDEBAR_VISIBLE_KEY, true));
        window
            .global::<BrowserState>()
            .set_bookmarks_bar_visible(self.stored_flag(BOOKMARKS_BAR_VISIBLE_KEY, true));
    }

    pub fn toggle_sidebar(&self) {
        let Some(window) = self.window() else { return };
        let state = window.global::<SidebarState>();
        let visible = !state.get_visible();
        state.set_visible(visible);
        state.set_hovering(false);
        self.store_flag(SIDEBAR_VISIBLE_KEY, visible);
    }

    pub fn toggle_bookmarks_bar(&self) {
        let Some(window) = self.window() else { return };
        let state = window.global::<BrowserState>();
        let visible = !state.get_bookmarks_bar_visible();
        state.set_bookmarks_bar_visible(visible);
        self.store_flag(BOOKMARKS_BAR_VISIBLE_KEY, visible);
    }

    fn stored_flag(&self, key: &str, default: bool) -> bool {
        self.storage
            .setting(key)
            .ok()
            .flatten()
            .map_or(default, |value| value == "true")
    }

    fn store_flag(&self, key: &str, value: bool) {
        if let Err(error) = self
            .storage
            .set_setting(key, if value { "true" } else { "false" })
        {
            tracing::warn!(%error, key, "layout setting could not be stored");
        }
    }
}
