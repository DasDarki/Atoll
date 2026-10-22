use atoll_core::{IslandId, TabId};
use atoll_storage::prelude::*;
use atoll_storage::{NewIsland, NewTab};

use super::Browser;
use atoll_i18n::t;

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
        let already_active = self.session.borrow().active_tab == Some(tab);
        // The page being left is finished, which is the moment to read what it shows.
        if let Some(leaving) = self.session.borrow().active_tab.filter(|_| !already_active) {
            self.capture_page_text(leaving);
        }
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
        if !already_active {
            self.plugin_tab_activated(tab);
        }
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
        self.create_tab_in(island, index, url, title, activate, false)
    }

    /// Creates a tab, privately when `private` is set. A private tab writes its address nowhere,
    /// so it goes into the database empty and is remembered in memory.
    pub(super) fn create_tab_in(
        &self,
        island: IslandId,
        index: Option<usize>,
        url: &str,
        title: Option<&str>,
        activate: bool,
        private: bool,
    ) -> Option<TabId> {
        let created = self.storage.create_tab(NewTab {
            island_id: island,
            url: if private { "" } else { url },
            title: if private { None } else { title },
            index,
            created_at: Self::now(),
            private,
        });
        match created {
            Ok(tab) => {
                if private {
                    self.remember_private_page(tab.id, Some(url), title);
                }
                self.reload_session();
                if activate {
                    self.activate_tab(tab.id);
                } else {
                    self.refresh_sidebar();
                }
                self.plugin_tab_opened(tab.id);
                Some(tab.id)
            }
            Err(error) => {
                self.report(t!("Tab anlegen"), Err::<(), _>(error));
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
    pub(super) fn open_in_new_tab(
        &self,
        source: Option<TabId>,
        url: &str,
        activate: bool,
    ) -> Option<TabId> {
        let target = source
            .and_then(|tab| self.insertion_point_after(tab))
            .or_else(|| self.insertion_point_after_active());
        let island = match target {
            Some((island, _)) => island,
            None => self.ensure_island()?,
        };
        let private = source.is_some_and(|tab| self.is_private(tab));
        self.create_tab_in(
            island,
            target.map(|(_, index)| index),
            url,
            None,
            activate,
            private,
        )
    }

    pub fn close_tab(&self, tab: TabId) {
        self.forget_page_text(tab);
        self.forget_autofilled(tab);
        let (successor, was_active, closed) = {
            let session = self.session.borrow();
            let Some(record) = session.tab(tab) else {
                return;
            };
            let index = session
                .island(record.island_id)
                .and_then(|entry| entry.tabs.iter().position(|candidate| candidate.id == tab))
                .unwrap_or(0);
            // A private tab leaves nothing behind, not even a way back.
            let closed = (!record.is_blank() && !record.private).then(|| ClosedTab {
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
        self.forget_private_page(tab);
        if let Err(error) = self.storage.delete_tab(tab) {
            self.report(t!("Tab schließen"), Err::<(), _>(error));
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
        self.plugin_tab_closed(tab);
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
            self.report(t!("Anpinnen"), Err::<(), _>(error));
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
        self.create_tab_in(
            island,
            Some(index),
            &url,
            title.as_deref(),
            true,
            self.is_private(tab),
        );
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
        self.move_tab_to(tab, island, usize::MAX);
    }

    /// Moves `tab` to `index` inside `island`, which may be the one it is already in.
    pub(super) fn move_tab_to(&self, tab: TabId, island: IslandId, index: usize) {
        self.move_tabs_to(&[tab], island, index);
    }

    /// Moves `tabs` to `index` inside `island`, where `index` counts the island's tabs without
    /// the ones that are moving. They end up next to each other in the order they are given.
    pub(super) fn move_tabs_to(&self, tabs: &[TabId], island: IslandId, index: usize) {
        let here: Vec<TabId> = self
            .session
            .borrow()
            .island(island)
            .map(|entry| entry.tabs.iter().map(|tab| tab.id).collect())
            .unwrap_or_default();
        let order = order_after_move(&here, tabs, index);
        let moved = self.storage.transaction(|transaction| {
            // The island is written out from the front: every single move shifts what comes
            // after it, and tabs that travel together must not be scattered on the way.
            for (place, tab) in order.iter().enumerate() {
                transaction.move_tab(*tab, island, place)?;
            }
            Ok(())
        });
        self.report(t!("Tab verschieben"), moved);
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
            t!("Neue Insel"),
            t!("Name der neuen Insel"),
            t!("Neue Insel"),
            t!("Anlegen"),
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
                browser.report(t!("Insel anlegen"), result);
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
            name: t!("Tabs"),
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
                self.report(t!("Insel anlegen"), Err::<(), _>(error));
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
            self.report(t!("Wiederherstellen"), result);
            self.show_toast(&t!("Insel „{}“ aus dem Archiv geholt", island.name));
        }
        if island.collapsed {
            let result = self.storage.set_island_collapsed(island.id, false);
            self.report(t!("Insel aufklappen"), result);
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
                self.apply_private_pages(&mut session);
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
        self.refresh_open_elsewhere();
        if active_before.is_some() && active_now.is_none() {
            self.show_remembered_tab();
        } else {
            self.refresh_sidebar();
        }
    }
}

/// How an island's tabs lie after `moving` landed at `index`, which counts the tabs that stay.
/// Tabs that come from another island are simply inserted there.
fn order_after_move(here: &[TabId], moving: &[TabId], index: usize) -> Vec<TabId> {
    let staying: Vec<TabId> = here
        .iter()
        .copied()
        .filter(|tab| !moving.contains(tab))
        .collect();
    let at = index.min(staying.len());
    let mut order = Vec::with_capacity(staying.len() + moving.len());
    order.extend_from_slice(&staying[..at]);
    order.extend_from_slice(moving);
    order.extend_from_slice(&staying[at..]);
    order
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tabs(ids: &[i64]) -> Vec<TabId> {
        ids.iter().map(|id| TabId(*id)).collect()
    }

    #[test]
    fn what_travels_together_lands_together() {
        let here = tabs(&[1, 2, 3]);
        assert_eq!(
            order_after_move(&here, &tabs(&[1, 2]), 1),
            tabs(&[3, 1, 2]),
            "behind the only tab that stays"
        );
        assert_eq!(
            order_after_move(&here, &tabs(&[1, 3]), 0),
            tabs(&[1, 3, 2]),
            "in front of it"
        );
    }

    #[test]
    fn tabs_from_elsewhere_are_put_in_place() {
        let here = tabs(&[1, 2]);
        assert_eq!(
            order_after_move(&here, &tabs(&[7, 8]), 1),
            tabs(&[1, 7, 8, 2])
        );
        assert_eq!(
            order_after_move(&here, &tabs(&[7]), 99),
            tabs(&[1, 2, 7]),
            "a place beyond the end is the end"
        );
    }

    #[test]
    fn a_single_tab_moves_as_before() {
        let here = tabs(&[1, 2, 3]);
        assert_eq!(order_after_move(&here, &tabs(&[1]), 2), tabs(&[2, 3, 1]));
        assert_eq!(order_after_move(&here, &tabs(&[3]), 0), tabs(&[3, 1, 2]));
    }
}
