use atoll_core::{IslandId, TabId};
use atoll_storage::prelude::*;
use atoll_storage::{NewIsland, NewTab};

use super::Browser;

const CLOSED_TAB_MEMORY: usize = 25;

/// A closed tab that can be restored with Ctrl+Shift+T.
pub struct ClosedTab {
    island: IslandId,
    index: usize,
    url: String,
    title: Option<String>,
}

impl Browser {
    /// Shows `tab`, waking its engine instance if it was sleeping.
    pub fn activate_tab(&self, tab: TabId) {
        let (blank, workspace) = {
            let mut session = self.session.borrow_mut();
            let Some(record) = session.tab(tab) else {
                return;
            };
            let blank = record.is_blank();
            session.active_tab = Some(tab);
            (blank, session.workspace)
        };
        let key = Self::active_workspace_key(workspace);
        if let Err(error) = self
            .storage
            .touch_tab(tab, Self::now())
            .and_then(|()| self.storage.set_setting(&key, &tab.0.to_string()))
        {
            tracing::warn!(%error, "tab usage could not be stored");
        }

        if self.is_engine_running() {
            let commands = if blank {
                self.lifecycle.borrow_mut().deactivate(self.id)
            } else {
                self.lifecycle.borrow_mut().activate(self.id, tab)
            };
            self.apply(commands);
        }
        self.close_find();
        self.sync_page();
        self.refresh_sidebar();
        self.focus_content();
    }

    /// Creates a tab in `island` at `index` and optionally shows it.
    pub(super) fn create_tab(
        &self,
        island: IslandId,
        index: Option<usize>,
        url: &str,
        title: Option<&str>,
        activate: bool,
    ) -> Option<TabId> {
        let created = self.storage.create_tab(NewTab {
            island_id: island,
            url,
            title,
            index,
            created_at: Self::now(),
        });
        match created {
            Ok(tab) => {
                self.reload_session();
                if activate {
                    self.activate_tab(tab.id);
                } else {
                    self.refresh_sidebar();
                }
                Some(tab.id)
            }
            Err(error) => {
                self.report("Tab anlegen", Err::<(), _>(error));
                None
            }
        }
    }

    /// Opens a new tab after the active one, or at the end of the first island.
    pub fn new_tab(&self) {
        let target = self.insertion_point_after_active();
        let island = match target {
            Some((island, _)) => island,
            None => match self.ensure_island() {
                Some(island) => island,
                None => return,
            },
        };
        self.open_new_tab(island, target.map(|(_, index)| index));
    }

    pub fn new_tab_in_island(&self, island: IslandId) {
        self.open_new_tab(island, None);
    }

    /// Creates a tab showing the start page, focusing the address bar so typing starts a search.
    fn open_new_tab(&self, island: IslandId, index: Option<usize>) {
        let address = self.new_tab_address();
        if address.is_some() {
            self.address_focus_requested.set(true);
        }
        let url = address.unwrap_or_default();
        if self.create_tab(island, index, &url, None, true).is_none() {
            self.address_focus_requested.set(false);
        }
    }

    /// Opens `url` in a new tab next to `source`.
    pub(super) fn open_in_new_tab(&self, source: Option<TabId>, url: &str, activate: bool) {
        let target = source
            .and_then(|tab| self.insertion_point_after(tab))
            .or_else(|| self.insertion_point_after_active());
        let island = match target {
            Some((island, _)) => island,
            None => match self.ensure_island() {
                Some(island) => island,
                None => return,
            },
        };
        self.create_tab(island, target.map(|(_, index)| index), url, None, activate);
    }

    pub fn close_tab(&self, tab: TabId) {
        let (successor, was_active, closed) = {
            let session = self.session.borrow();
            let Some(record) = session.tab(tab) else {
                return;
            };
            let index = session
                .island(record.island_id)
                .and_then(|entry| entry.tabs.iter().position(|candidate| candidate.id == tab))
                .unwrap_or(0);
            let closed = (!record.is_blank()).then(|| ClosedTab {
                island: record.island_id,
                index,
                url: record.url.clone(),
                title: record.title.clone(),
            });
            (
                session.successor_of(tab),
                session.active_tab == Some(tab),
                closed,
            )
        };

        if let Some(closed) = closed {
            let mut memory = self.closed_tabs.borrow_mut();
            memory.push(closed);
            if memory.len() > CLOSED_TAB_MEMORY {
                memory.remove(0);
            }
        }

        self.forget_tabs([tab]);
        if let Err(error) = self.storage.delete_tab(tab) {
            self.report("Tab schließen", Err::<(), _>(error));
            return;
        }
        self.reload_session();
        if was_active {
            match successor {
                Some(next) => self.activate_tab(next),
                None => self.show_empty_workspace(),
            }
        } else {
            self.refresh_sidebar();
        }
    }

    pub fn close_active_tab(&self) {
        let active = self.session.borrow().active_tab;
        if let Some(tab) = active {
            self.close_tab(tab);
        }
    }

    pub fn reopen_closed_tab(&self) {
        let Some(closed) = self.closed_tabs.borrow_mut().pop() else {
            return;
        };
        let island = if self.session.borrow().island(closed.island).is_some() {
            Some(closed.island)
        } else {
            self.ensure_island()
        };
        if let Some(island) = island {
            self.create_tab(
                island,
                Some(closed.index),
                &closed.url,
                closed.title.as_deref(),
                true,
            );
        }
    }

    /// Activates the tab `offset` steps away in sidebar order, wrapping around.
    pub fn cycle_tabs(&self, offset: isize) {
        let (order, current) = {
            let session = self.session.borrow();
            (
                session.navigation_order(session.active_tab),
                session.active_tab,
            )
        };
        if order.is_empty() {
            return;
        }
        let position = current
            .and_then(|tab| order.iter().position(|candidate| *candidate == tab))
            .unwrap_or(0) as isize;
        let next = (position + offset).rem_euclid(order.len() as isize) as usize;
        self.activate_tab(order[next]);
    }

    pub fn sleep_tab(&self, tab: TabId) {
        if self.session.borrow().active_tab == Some(tab) {
            return;
        }
        let commands = self.lifecycle.borrow_mut().sleep(tab);
        self.apply(commands);
        self.refresh_sidebar();
    }

    pub fn toggle_pin(&self, tab: TabId) {
        let Some(pinned) = self.session.borrow().tab(tab).map(|record| !record.pinned) else {
            return;
        };
        if let Err(error) = self.storage.set_tab_pinned(tab, pinned) {
            self.report("Anpinnen", Err::<(), _>(error));
            return;
        }
        let commands = self.lifecycle.borrow_mut().set_protected(tab, pinned);
        self.apply(commands);
        self.reload_session();
        self.refresh_sidebar();
    }

    pub fn duplicate_tab(&self, tab: TabId) {
        let Some((url, title)) = self
            .session
            .borrow()
            .tab(tab)
            .map(|record| (record.url.clone(), record.title.clone()))
        else {
            return;
        };
        let Some((island, index)) = self.insertion_point_after(tab) else {
            return;
        };
        self.create_tab(island, Some(index), &url, title.as_deref(), true);
    }

    pub fn copy_tab_address(&self, tab: TabId) {
        let url = self
            .session
            .borrow()
            .tab(tab)
            .map(|record| record.url.clone());
        if let Some(url) = url.filter(|url| !url.is_empty()) {
            self.copy_text(&url);
        }
    }

    pub fn reload_tab(&self, tab: TabId) {
        let reloaded = self
            .views
            .borrow()
            .get(tab)
            .map(|entry| entry.view.reload(false))
            .is_some();
        if !reloaded {
            self.activate_tab(tab);
        }
    }

    pub fn move_tab_to_island(&self, tab: TabId, island: IslandId) {
        let result = self.storage.move_tab(tab, island, usize::MAX);
        self.report("Tab verschieben", result);
        self.reload_session();
        self.refresh_sidebar();
    }

    pub fn move_tab_to_new_island(&self, tab: TabId) {
        let Some(source) = self
            .session
            .borrow()
            .tab(tab)
            .map(|record| record.island_id)
        else {
            return;
        };
        let this = self.this();
        self.prompt_text(
            "Neue Insel",
            "Name der neuen Insel",
            "Neue Insel",
            "Anlegen",
            move |name| {
                let Some(browser) = this.upgrade() else {
                    return;
                };
                let index = browser
                    .session
                    .borrow()
                    .island_index(source)
                    .map(|index| index + 1);
                let workspace = browser.session.borrow().workspace;
                let color = browser.next_island_color();
                let result = browser.storage.transaction(|transaction| {
                    let island = transaction.create_island(NewIsland {
                        workspace_id: workspace,
                        name: &name,
                        color,
                        index,
                        created_at: Self::now(),
                    })?;
                    transaction.move_tab(tab, island.id, 0)
                });
                browser.report("Insel anlegen", result);
                browser.reload_session();
                browser.refresh_sidebar();
            },
        );
    }

    /// Shows the new tab page when the workspace has no tab to show.
    pub(super) fn show_empty_workspace(&self) {
        if self.is_engine_running() {
            let commands = self.lifecycle.borrow_mut().deactivate(self.id);
            self.apply(commands);
        }
        self.session.borrow_mut().active_tab = None;
        self.sync_page();
        self.refresh_sidebar();
        self.focus_content();
    }

    /// The first island of the workspace, created if the workspace has none.
    pub(super) fn ensure_island(&self) -> Option<IslandId> {
        if let Some(island) = self
            .session
            .borrow()
            .islands
            .first()
            .map(|entry| entry.island.id)
        {
            return Some(island);
        }
        let workspace = self.session.borrow().workspace;
        let created = self.storage.create_island(NewIsland {
            workspace_id: workspace,
            name: "Tabs",
            color: self.next_island_color(),
            index: None,
            created_at: Self::now(),
        });
        match created {
            Ok(island) => {
                self.reload_session();
                Some(island.id)
            }
            Err(error) => {
                self.report("Insel anlegen", Err::<(), _>(error));
                None
            }
        }
    }

    pub(super) fn insertion_point_after_active(&self) -> Option<(IslandId, usize)> {
        let active = self.session.borrow().active_tab?;
        self.insertion_point_after(active)
    }

    fn insertion_point_after(&self, tab: TabId) -> Option<(IslandId, usize)> {
        let session = self.session.borrow();
        let island = session.tab(tab)?.island_id;
        let index = session
            .island(island)?
            .tabs
            .iter()
            .position(|candidate| candidate.id == tab)?;
        Some((island, index + 1))
    }

    /// Shows `tab` wherever it lives: its island comes back from the archive and is expanded, and
    /// the workspace is switched if necessary. A tab of a workspace that another window shows is
    /// revealed there.
    pub(super) fn reveal_tab(&self, tab: TabId) {
        let Ok(record) = self.storage.tab(tab) else {
            return;
        };
        let Ok(island) = self.storage.island(record.island_id) else {
            return;
        };
        if island.workspace_id != self.session.borrow().workspace
            && let Some(manager) = self.manager()
            && let Some(other) = manager.window_for_workspace(island.workspace_id)
        {
            manager.focus_window(other.id);
            other.reveal_tab(tab);
            return;
        }
        if island.is_archived() {
            let result = self.storage.restore_island(island.id);
            self.report("Wiederherstellen", result);
            self.show_toast(&format!("Insel „{}“ aus dem Archiv geholt", island.name));
        }
        if island.collapsed {
            let result = self.storage.set_island_collapsed(island.id, false);
            self.report("Insel aufklappen", result);
        }
        if island.workspace_id != self.session.borrow().workspace {
            let key = Self::active_workspace_key(island.workspace_id);
            if let Err(error) = self.storage.set_setting(&key, &tab.0.to_string()) {
                tracing::warn!(%error, "active tab could not be stored");
            }
            self.select_workspace(island.workspace_id);
        } else {
            self.reload_session();
            self.activate_tab(tab);
        }
    }

    /// Reloads the workspace from the database, keeping the active tab if it still exists. The
    /// other windows follow, a change here may have moved tabs or islands into their workspaces.
    pub(super) fn reload_session(&self) {
        self.reload_session_here();
        self.for_other_windows(Browser::follow_changes);
    }

    pub(super) fn reload_session_here(&self) {
        let (workspace, active) = {
            let session = self.session.borrow();
            (session.workspace, session.active_tab)
        };
        match super::session::Session::load(&self.storage, workspace) {
            Ok(mut session) => {
                session.active_tab = active.filter(|tab| session.contains_tab(*tab));
                *self.session.borrow_mut() = session;
            }
            Err(error) => tracing::error!(%error, "session could not be reloaded"),
        }
    }

    /// Picks up changes another window made: shows another tab if the active one moved away.
    pub(super) fn follow_changes(&self) {
        let active_before = self.session.borrow().active_tab;
        self.reload_session_here();
        let active_now = self.session.borrow().active_tab;
        if active_before.is_some() && active_now.is_none() {
            self.show_remembered_tab();
        } else {
            self.refresh_sidebar();
        }
    }
}
