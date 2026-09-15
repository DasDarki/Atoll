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
use atoll_tor::Status as TorStatus;

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
    /// Whether the tab goes through Tor, which only this run knows.
    through_tor: bool,
}

impl PrivatePages {
    fn page(&mut self, tab: TabId) -> &mut Page {
        self.pages.entry(tab).or_default()
    }

    pub fn forget(&mut self, tab: TabId) {
        self.pages.remove(&tab);
    }

    fn through_tor(&self, tab: TabId) -> bool {
        self.pages.get(&tab).is_some_and(|page| page.through_tor)
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
        self.open_private_tab(false);
    }

    /// Opens a private tab whose pages go through the Tor network.
    pub fn new_tor_tab(&self) {
        self.tor.start();
        self.open_private_tab(true);
    }

    fn open_private_tab(&self, through_tor: bool) {
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
                if through_tor {
                    self.private.borrow_mut().page(tab.id).through_tor = true;
                }
                self.address_focus_requested.set(true);
                self.reload_session();
                self.activate_tab(tab.id);
                let message = if through_tor {
                    t!(
                        "Tor-Tab: die Seite sieht nicht, woher du kommst. Gegen Wiedererkennung am Browser selbst hilft das nicht."
                    )
                } else {
                    t!(
                        "Privater Tab: nichts davon wird gespeichert, und er weiß nichts von deinen Anmeldungen"
                    )
                };
                self.show_toast(message);
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

    /// Whether the tab carries its pages through Tor.
    pub(super) fn is_tor(&self, tab: TabId) -> bool {
        self.private.borrow().through_tor(tab)
    }

    /// The session a tab's pages run in.
    pub(super) fn session_of(&self, tab: TabId) -> Session {
        if !self.is_private(tab) {
            return Session::Normal;
        }
        match self.tor_port(tab) {
            Some(port) => Session::Tor(port),
            None => Session::Private,
        }
    }

    /// The proxy port a Tor tab uses, if Tor is through.
    fn tor_port(&self, tab: TabId) -> Option<u16> {
        self.is_tor(tab).then(|| self.tor.status().port()).flatten()
    }

    /// Whether the tab may not load anything yet because its Tor is still on its way. Nothing of
    /// such a tab ever goes out beside Tor, so it waits instead.
    pub(super) fn waits_for_tor(&self, tab: TabId) -> bool {
        self.is_tor(tab) && self.tor_port(tab).is_none()
    }

    /// Looks after the tabs that wait for Tor, called whenever the browser is awake anyway.
    pub(super) fn watch_tor(&self) {
        let status = self.tor.status();
        if self.tor_status.borrow().as_ref() == Some(&status) {
            return;
        }
        *self.tor_status.borrow_mut() = Some(status.clone());
        match status {
            TorStatus::Ready(_) => {
                let waiting: Vec<(TabId, String)> = {
                    let session = self.session.borrow();
                    let pages = self.private.borrow();
                    session
                        .tabs()
                        .filter(|tab| tab.private && pages.through_tor(tab.id))
                        .map(|tab| (tab.id, tab.url.clone()))
                        .collect()
                };
                for (tab, url) in waiting {
                    if url.trim().is_empty() {
                        continue;
                    }
                    self.load_in_tab(tab, &url);
                }
                self.show_toast(t!("Tor ist verbunden"));
            }
            TorStatus::Failed(reason) => {
                self.show_toast(&t!("Tor konnte nicht gestartet werden: {}", reason));
                self.tor.forget_failure();
            }
            TorStatus::Idle | TorStatus::Starting => {}
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
