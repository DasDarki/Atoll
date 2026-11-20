//! Hiding Atoll at a keystroke: every window turns black and everything falls silent, while
//! underneath the pages go on as they were, so nothing is lost. The same keys bring it back.

use slint::ComponentHandle;
use slint::winit_030::winit::event::{ElementState, KeyEvent};
use slint::winit_030::winit::keyboard::ModifiersState;

use super::Browser;
use super::preferences::PreferenceKey;
use crate::input::shortcuts;
use crate::ui::{BrowserState, SettingsState};
use atoll_i18n::t;

impl Browser {
    /// Whether `event` is the key combination set for hiding.
    pub(super) fn is_hide_shortcut(&self, event: &KeyEvent, modifiers: ModifiersState) -> bool {
        event.state == ElementState::Pressed
            && !event.repeat
            && shortcuts::describe(event, modifiers).is_some_and(|keys| {
                keys.eq_ignore_ascii_case(&self.preferences.borrow().hide_shortcut)
            })
    }

    pub(super) fn is_hidden(&self) -> bool {
        self.hidden.get()
    }

    /// Hides every window behind black and silences every page, or brings it all back. A tab
    /// the user muted stays muted when the sound returns.
    pub fn toggle_hidden(&self) {
        let hidden = !self.hidden.get();
        self.hidden.set(hidden);
        tracing::debug!(hidden, "atoll hides");
        let browsers = self
            .manager()
            .map(|manager| manager.browsers())
            .unwrap_or_default();
        for (tab, entry) in self.views.borrow().iter() {
            let muted = hidden
                || self.is_muted(*tab)
                || browsers.iter().any(|browser| browser.is_muted(*tab));
            entry.view.set_audio_muted(muted);
        }
        self.show_hidden();
        for browser in &browsers {
            if browser.id != self.id {
                browser.show_hidden();
            }
        }
    }

    /// Shows this window as hidden or not, whatever the shared state says.
    pub(super) fn show_hidden(&self) {
        let hidden = self.hidden.get();
        if hidden {
            self.cancel_context_menu();
        }
        self.with_live_panel(|view| view.set_audio_muted(hidden));
        if let Some(window) = self.window() {
            window.global::<BrowserState>().set_hidden(hidden);
        }
        if !hidden {
            self.focus_content();
        }
    }

    /// The settings: takes the keys typed for hiding once they name a combination.
    pub(super) fn hide_shortcut_edited(&self, text: &str) {
        let Some(keys) = shortcuts::normalize(text) else {
            return;
        };
        self.update_preference(PreferenceKey::HideShortcut, |preferences| {
            preferences.hide_shortcut = keys.clone();
        });
        if let Some(window) = self.window() {
            window
                .global::<SettingsState>()
                .set_hide_shortcut_active(keys.as_str().into());
        }
    }

    /// The palette entry for hiding, named with the keys that do the same.
    pub(super) fn hide_command_title(&self) -> String {
        t!(
            "Atoll verbergen ({})",
            self.preferences.borrow().hide_shortcut
        )
    }
}
