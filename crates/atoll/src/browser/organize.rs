use atoll_core::{IslandColor, IslandId, TabId, WorkspaceId};
use atoll_storage::NewIsland;
use atoll_storage::prelude::*;

use super::Browser;
use super::session::Session;

impl Browser {
    /// Shows `workspace` in this window, or brings the window to the front that already shows it.
    pub fn select_workspace(&self, workspace: WorkspaceId) {
        if self.session.borrow().workspace == workspace {
            return;
        }
        if let Some(manager) = self.manager()
            && let Some(other) = manager.window_for_workspace(workspace)
        {
            manager.focus_window(other.id);
            return;
        }
        let session = match Session::load(&self.storage, workspace) {
            Ok(session) => session,
            Err(error) => {
                self.report("Workspace wechseln", Err::<(), _>(error));
                return;
            }
        };
        if self.is_engine_running() {
            let commands = self.lifecycle.borrow_mut().deactivate(self.id);
            self.apply(commands);
        }
        if let Err(error) = self.storage.set_window_workspace(self.id, workspace) {
            tracing::warn!(%error, "workspace of the window could not be stored");
        }
        *self.session.borrow_mut() = session;
        self.show_remembered_tab();
        self.for_other_windows(Browser::follow_changes);
    }

    /// Activates the tab that was last used in the current workspace.
    pub(super) fn show_remembered_tab(&self) {
        let remembered = {
            let session = self.session.borrow();
            self.storage
                .setting_i64(&Self::active_workspace_key(session.workspace))
                .ok()
                .flatten()
                .map(TabId)
                .filter(|tab| session.contains_tab(*tab))
                .or_else(|| session.tabs().next().map(|tab| tab.id))
        };
        match remembered {
            Some(tab) => self.activate_tab(tab),
            None => self.show_empty_workspace(),
        }
    }

    pub fn create_workspace(&self) {
        let this = self.this();
        self.prompt_text(
            "Neuer Workspace",
            "Wie soll der Workspace heißen?",
            "",
            "Anlegen",
            move |name| {
                let Some(browser) = this.upgrade() else {
                    return;
                };
                match browser.storage.create_workspace(&name) {
                    Ok(workspace) => browser.select_workspace(workspace.id),
                    Err(error) => browser.report("Workspace anlegen", Err::<(), _>(error)),
                }
            },
        );
    }

    pub fn workspace_action(&self, workspace: WorkspaceId, action: &str) {
        match action {
            "open-window" => self.open_workspace_in_window(workspace),
            "rename" => self.rename_workspace(workspace),
            "delete" => self.delete_workspace(workspace),
            "move-left" => self.shift_workspace(workspace, -1),
            "move-right" => self.shift_workspace(workspace, 1),
            _ => tracing::warn!(action, "unknown workspace action"),
        }
    }

    fn rename_workspace(&self, workspace: WorkspaceId) {
        let current = self
            .session
            .borrow()
            .workspaces
            .iter()
            .find(|candidate| candidate.id == workspace)
            .map(|candidate| candidate.name.clone())
            .unwrap_or_default();
        let this = self.this();
        self.prompt_text(
            "Workspace umbenennen",
            "",
            &current,
            "Umbenennen",
            move |name| {
                let Some(browser) = this.upgrade() else {
                    return;
                };
                let result = browser.storage.rename_workspace(workspace, &name);
                browser.report("Umbenennen", result);
                browser.reload_session();
                browser.refresh_sidebar();
            },
        );
    }

    fn delete_workspace(&self, workspace: WorkspaceId) {
        let (name, count) = {
            let session = self.session.borrow();
            if session.workspaces.len() <= 1 {
                return;
            }
            let name = session
                .workspaces
                .iter()
                .find(|candidate| candidate.id == workspace)
                .map(|candidate| candidate.name.clone())
                .unwrap_or_default();
            let count = self
                .storage
                .all_tabs_in_workspace(workspace)
                .map(|tabs| tabs.len())
                .unwrap_or(0);
            (name, count)
        };
        let this = self.this();
        self.confirm(
            "Workspace löschen?",
            &format!("„{name}“ wird mit allen Inseln und {count} Tabs endgültig gelöscht."),
            "Löschen",
            move || {
                let Some(browser) = this.upgrade() else {
                    return;
                };
                let tabs = browser
                    .storage
                    .all_tabs_in_workspace(workspace)
                    .map(|tabs| tabs.into_iter().map(|tab| tab.id).collect::<Vec<_>>())
                    .unwrap_or_default();
                let manager = browser.manager();
                let shown_elsewhere = manager
                    .as_ref()
                    .and_then(|manager| manager.window_for_workspace(workspace))
                    .filter(|other| other.id != browser.id);
                let shown_here = browser.session.borrow().workspace == workspace;
                let next = shown_here
                    .then(|| browser.free_workspace(workspace))
                    .flatten();
                if let Some(next) = next
                    && let Err(error) = browser.storage.set_window_workspace(browser.id, next)
                {
                    tracing::warn!(%error, "workspace of the window could not be stored");
                }
                browser.forget_tabs(tabs);
                if let Err(error) = browser.storage.delete_workspace(workspace) {
                    browser.report("Workspace löschen", Err::<(), _>(error));
                    return;
                }
                if let (Some(manager), Some(other)) = (&manager, shown_elsewhere) {
                    manager.close_window(other.id);
                }
                if !shown_here {
                    browser.reload_session();
                    browser.refresh_sidebar();
                    return;
                }
                match (next, manager) {
                    (Some(next), _) => {
                        {
                            let mut session = browser.session.borrow_mut();
                            session.workspace = next;
                            session.active_tab = None;
                        }
                        browser.reload_session();
                        browser.show_remembered_tab();
                    }
                    (None, Some(manager)) => manager.close_window(browser.id),
                    (None, None) => {}
                }
            },
        );
    }

    /// A workspace other than `except` that no other window shows.
    fn free_workspace(&self, except: WorkspaceId) -> Option<WorkspaceId> {
        let manager = self.manager();
        self.storage
            .workspaces()
            .ok()?
            .into_iter()
            .map(|workspace| workspace.id)
            .find(|workspace| {
                *workspace != except
                    && manager
                        .as_ref()
                        .is_none_or(|manager| !manager.is_open_elsewhere(*workspace, self.id))
            })
    }

    fn shift_workspace(&self, workspace: WorkspaceId, offset: isize) {
        let index = self
            .session
            .borrow()
            .workspaces
            .iter()
            .position(|candidate| candidate.id == workspace);
        let Some(index) = index else { return };
        let target = (index as isize + offset).max(0) as usize;
        let result = self.storage.move_workspace(workspace, target);
        self.report("Workspace verschieben", result);
        self.reload_session();
        self.refresh_sidebar();
    }

    pub fn create_island(&self) {
        let this = self.this();
        self.prompt_text(
            "Neue Insel",
            "Wie soll die Insel heißen?",
            "",
            "Anlegen",
            move |name| {
                let Some(browser) = this.upgrade() else {
                    return;
                };
                let workspace = browser.session.borrow().workspace;
                let color = browser.next_island_color();
                let created = browser.storage.create_island(NewIsland {
                    workspace_id: workspace,
                    name: &name,
                    color,
                    index: None,
                    created_at: Self::now(),
                });
                match created {
                    Ok(island) => {
                        browser.reload_session();
                        browser.new_tab_in_island(island.id);
                    }
                    Err(error) => browser.report("Insel anlegen", Err::<(), _>(error)),
                }
            },
        );
    }

    pub fn toggle_island(&self, island: IslandId) {
        let Some(collapsed) = self
            .session
            .borrow()
            .island(island)
            .map(|entry| !entry.island.collapsed)
        else {
            return;
        };
        let result = self.storage.set_island_collapsed(island, collapsed);
        self.report("Insel einklappen", result);
        self.reload_session();
        self.refresh_sidebar();
    }

    pub fn set_island_color(&self, island: IslandId, color: usize) {
        let color = IslandColor::from_index(color as i64);
        let result = self.storage.set_island_color(island, color);
        self.report("Farbe ändern", result);
        self.reload_session();
        self.refresh_sidebar();
    }

    pub fn island_action(&self, island: IslandId, action: &str) {
        match action {
            "new-tab" => self.new_tab_in_island(island),
            "rename" => self.rename_island(island),
            "sleep" => self.sleep_island(island),
            "export" => self.open_export(Some(island)),
            "move-up" => self.shift_island(island, -1),
            "move-down" => self.shift_island(island, 1),
            "archive" => self.archive_island(island),
            "delete" => self.delete_island(island),
            _ => tracing::warn!(action, "unknown island action"),
        }
    }

    fn rename_island(&self, island: IslandId) {
        let current = self
            .session
            .borrow()
            .island(island)
            .map(|entry| entry.island.name.clone())
            .unwrap_or_default();
        let this = self.this();
        self.prompt_text(
            "Insel umbenennen",
            "",
            &current,
            "Umbenennen",
            move |name| {
                let Some(browser) = this.upgrade() else {
                    return;
                };
                let result = browser.storage.rename_island(island, &name);
                browser.report("Umbenennen", result);
                browser.reload_session();
                browser.refresh_sidebar();
            },
        );
    }

    fn sleep_island(&self, island: IslandId) {
        let tabs = self.tabs_of_island(island);
        let active = self.session.borrow().active_tab;
        let mut commands = Vec::new();
        {
            let mut lifecycle = self.lifecycle.borrow_mut();
            for tab in tabs.into_iter().filter(|tab| Some(*tab) != active) {
                commands.extend(lifecycle.sleep(tab));
            }
        }
        self.apply(commands);
        self.refresh_sidebar();
    }

    fn shift_island(&self, island: IslandId, offset: isize) {
        let (workspace, index) = {
            let session = self.session.borrow();
            (session.workspace, session.island_index(island))
        };
        let Some(index) = index else { return };
        let target = (index as isize + offset).max(0) as usize;
        let result = self.storage.move_island(island, workspace, target);
        self.report("Insel verschieben", result);
        self.reload_session();
        self.refresh_sidebar();
    }

    pub fn move_island_to_workspace(&self, island: IslandId, workspace: WorkspaceId) {
        self.leave_island_if_active(island);
        let result = self.storage.move_island(island, workspace, usize::MAX);
        self.report("Insel verschieben", result);
        self.reload_session();
        self.refresh_after_structure_change();
    }

    fn archive_island(&self, island: IslandId) {
        self.archive_island_quietly(island);
        self.show_toast("Insel archiviert. Wiederherstellen über das Archiv.");
    }

    /// Archives `island` of this window's workspace.
    pub(super) fn archive_island_quietly(&self, island: IslandId) {
        self.leave_island_if_active(island);
        self.forget_tabs(self.tabs_of_island(island));
        let result = self.storage.archive_island(island, Self::now());
        self.report("Archivieren", result);
        self.reload_session();
        self.refresh_after_structure_change();
    }

    fn delete_island(&self, island: IslandId) {
        let (name, count) = self
            .session
            .borrow()
            .island(island)
            .map(|entry| (entry.island.name.clone(), entry.tabs.len()))
            .unwrap_or_default();
        let this = self.this();
        self.confirm(
            "Insel löschen?",
            &format!("„{name}“ wird mit {count} Tabs endgültig gelöscht."),
            "Löschen",
            move || {
                let Some(browser) = this.upgrade() else {
                    return;
                };
                browser.leave_island_if_active(island);
                browser.forget_tabs(browser.tabs_of_island(island));
                let result = browser.storage.delete_island(island);
                browser.report("Insel löschen", result);
                browser.reload_session();
                browser.refresh_after_structure_change();
            },
        );
    }

    /// Switches away from the active tab if it belongs to `island`, which is about to disappear.
    fn leave_island_if_active(&self, island: IslandId) {
        let next = {
            let session = self.session.borrow();
            let Some(active) = session.active_tab else {
                return;
            };
            if session.tab(active).map(|tab| tab.island_id) != Some(island) {
                return;
            }
            session
                .tabs()
                .find(|tab| tab.island_id != island)
                .map(|tab| tab.id)
        };
        match next {
            Some(tab) => self.activate_tab(tab),
            None => self.show_empty_workspace(),
        }
    }

    fn refresh_after_structure_change(&self) {
        let active_missing = {
            let session = self.session.borrow();
            session.active_tab.is_none() && session.tabs().next().is_some()
        };
        if active_missing {
            self.show_remembered_tab();
        } else {
            self.sync_page();
            self.refresh_sidebar();
        }
    }

    pub(super) fn tabs_of_island(&self, island: IslandId) -> Vec<TabId> {
        self.session
            .borrow()
            .island(island)
            .map(|entry| entry.tabs.iter().map(|tab| tab.id).collect())
            .unwrap_or_default()
    }
}
