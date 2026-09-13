//! Private tabs: tabs that keep nothing and use nothing.
//!
//! A private tab sits in the sidebar like any other, in an island, with its place and its colour.
//! What it shows, however, never reaches the disk: the database only holds the bare row that says
//! a private tab is there, the address and the title live here in memory, and the row goes away
//! with the tab. Its page runs in a session of its own without a cache folder, so cookies and
//! storage are gone when the last private tab closes, and nothing of the ordinary session is
//! visible to it.

use std::collections::HashMap;

use atoll_core::TabId;
use atoll_engine::Session;
use atoll_i18n::t;
use atoll_storage::NewTab;
use atoll_storage::prelude::*;

use super::Browser;
use super::session::Session as BrowserSession;

/// What the private tabs of this run show, by tab.
#[derive(Debug, Default)]
pub struct PrivatePages {
    pages: HashMap<TabId, Page>,
}

#[derive(Debug, Default, Clone)]
struct Page {
    url: String,
    title: Option<String>,
}

impl PrivatePages {
    fn page(&mut self, tab: TabId) -> &mut Page {
        self.pages.entry(tab).or_default()
    }

    pub fn forget(&mut self, tab: TabId) {
        self.pages.remove(&tab);
    }

    /// Fills in what the database does not hold, after the session was read.
    pub fn apply(&self, session: &mut BrowserSession) {
        for entry in &mut session.islands {
            for tab in &mut entry.tabs {
                if !tab.private {
                    continue;
                }
                if let Some(page) = self.pages.get(&tab.id) {
                    tab.url = page.url.clone();
                    tab.title = page.title.clone();
                }
            }
        }
    }
}

impl Browser {
    /// Opens a private tab next to the active one.
    pub fn new_private_tab(&self) {
        let target = self.insertion_point_after_active();
        let island = match target {
            Some((island, _)) => Some(island),
            None => self.ensure_island(),
        };
        let Some(island) = island else { return };
        let created = self.storage.create_tab(NewTab {
            island_id: island,
            url: "",
            title: None,
            index: target.map(|(_, index)| index),
            created_at: Self::now(),
            private: true,
        });
        match created {
            Ok(tab) => {
                self.address_focus_requested.set(true);
                self.reload_session();
                self.activate_tab(tab.id);
                self.show_toast(t!(
                    "Privater Tab: nichts davon wird gespeichert, und er weiß nichts von deinen Anmeldungen"
                ));
            }
            Err(error) => self.report(t!("Tab anlegen"), Err::<(), _>(error)),
        }
    }

    /// Whether the tab keeps nothing, which every write has to ask before it stores something.
    pub(super) fn is_private(&self, tab: TabId) -> bool {
        self.session
            .borrow()
            .tab(tab)
            .is_some_and(|record| record.private)
    }

    /// The session a tab's pages run in.
    pub(super) fn session_of(&self, tab: TabId) -> Session {
        if self.is_private(tab) {
            Session::Private
        } else {
            Session::Normal
        }
    }

    /// Remembers what a private tab shows, in memory only.
    pub(super) fn remember_private_page(&self, tab: TabId, url: Option<&str>, title: Option<&str>) {
        let mut pages = self.private.borrow_mut();
        let page = pages.page(tab);
        if let Some(url) = url {
            page.url = url.to_owned();
        }
        if let Some(title) = title {
            page.title = Some(title.to_owned());
        }
    }

    pub(super) fn forget_private_page(&self, tab: TabId) {
        self.private.borrow_mut().forget(tab);
    }

    /// Fills the private tabs of the session with what they show.
    pub(super) fn apply_private_pages(&self, session: &mut BrowserSession) {
        self.private.borrow().apply(session);
    }
}
