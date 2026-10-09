//! Noticing that the page in front of the user is already open in another tab.
//!
//! Everything a tab shows is written to the database as it navigates, so the other tabs that show
//! the same page are one query away, across every workspace and every window. The answer is kept
//! until the address changes or the tabs do, because the toolbar is filled far more often than
//! anything about the tabs changes.

use atoll_core::TabId;
use atoll_core::address::without_fragment;
use atoll_i18n::t;
use atoll_storage::prelude::*;
use slint::ComponentHandle;

use super::Browser;
use crate::ui::BrowserState;

/// The other tabs showing the page of the active tab.
#[derive(Default)]
pub struct OpenElsewhere {
    /// The address the answer belongs to, and the tab that asked.
    asked_for: Option<(TabId, String)>,
    /// The other tabs, in the order they were opened.
    others: Vec<TabId>,
    /// What the pill in the address bar says, empty when there is no other tab.
    label: String,
    /// Set when the tabs changed, so the next look asks the database again.
    stale: bool,
}

impl Browser {
    /// Tabs were opened, closed or moved, so the answer may be a different one now.
    pub(super) fn tabs_may_have_changed(&self) {
        self.elsewhere.borrow_mut().stale = true;
    }

    /// Looks again and fills the pill, for the windows that did not navigate themselves: what
    /// another window opened or closed decides what this one shows.
    pub(super) fn refresh_open_elsewhere(&self) {
        self.tabs_may_have_changed();
        self.update_open_elsewhere();
    }

    /// Fills the pill in the address bar that says the page is open elsewhere.
    pub(super) fn update_open_elsewhere(&self) {
        let Some(window) = self.window() else { return };
        let label = self.look_for_the_same_page();
        window.global::<BrowserState>().set_also_open(label.into());
    }

    /// The label of the pill, which is empty when this page is open only here. A private tab
    /// keeps to itself and never compares its page with another tab.
    fn look_for_the_same_page(&self) -> String {
        let here = {
            let session = self.session.borrow();
            let tab = session.active_tab.and_then(|id| session.tab(id));
            match tab {
                Some(tab) if !tab.private && !tab.is_blank() => Some(tab.id),
                _ => None,
            }
        };
        let Some(here) = here else {
            self.elsewhere.replace(OpenElsewhere::default());
            return String::new();
        };
        let Some(address) = self.current_page_url() else {
            self.elsewhere.replace(OpenElsewhere::default());
            return String::new();
        };
        let page = without_fragment(&address);
        {
            let known = self.elsewhere.borrow();
            if !known.stale && known.asked_for.as_ref() == Some(&(here, page.clone())) {
                return known.label.clone();
            }
        }
        let found = self.storage.tabs_showing(&page, here).unwrap_or_default();
        let here_workspace = self.session.borrow().workspace;
        let label = match found.as_slice() {
            [] => String::new(),
            [only] if only.island.workspace_id != here_workspace => {
                t!("Schon offen in „{}“", only.workspace_name)
            }
            [_] => t!("Schon offen").to_owned(),
            several => t!("Schon {}x offen", several.len()),
        };
        self.elsewhere.replace(OpenElsewhere {
            asked_for: Some((here, page)),
            others: found.iter().map(|entry| entry.tab.id).collect(),
            label: label.clone(),
            stale: false,
        });
        label
    }

    /// The pill was clicked: shows the next tab that holds the same page. The order is the one
    /// the tabs were opened in and it starts after this tab, so clicking again and again walks
    /// through all of them and comes back here.
    pub fn show_other_tab(&self) {
        // The other tab may have navigated away in the meantime, so this asks anew.
        self.tabs_may_have_changed();
        self.look_for_the_same_page();
        let here = self.session.borrow().active_tab;
        let next = {
            let elsewhere = self.elsewhere.borrow();
            let after = here.map_or(0, |tab| tab.0);
            elsewhere
                .others
                .iter()
                .find(|other| other.0 > after)
                .or_else(|| elsewhere.others.first())
                .copied()
        };
        if let Some(next) = next {
            self.reveal_tab(next);
        }
    }
}
