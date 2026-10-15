use atoll_storage::prelude::*;

use super::Browser;

impl Browser {
    /// Shows the stored session. Only the database is read, no engine instance is created yet.
    pub(super) fn show_session(&self) {
        // Pages of tabs that are not there any more have nothing to be searched for.
        self.forget_closed_page_texts();
        self.restore_layout();
        self.restore_dev_tools_layout();
        self.show_bookmarks();
        self.show_download_state();
        self.show_remembered_tab();
    }

    /// Wakes the active tab once the engine runs.
    pub(super) fn engine_started(&self) {
        self.protect_pinned_tabs();
        let active = self.session.borrow().active_tab;
        if let Some(tab) = active {
            self.activate_tab(tab);
        }
        // Once a run, and only the window that starts the engine does it.
        static ASKED: std::sync::Once = std::sync::Once::new();
        ASKED.call_once(|| self.check_for_updates_in_time());
    }

    /// Pinned tabs keep their engine instance once they have one.
    pub(super) fn protect_pinned_tabs(&self) {
        let pinned: Vec<_> = self
            .session
            .borrow()
            .tabs()
            .filter(|tab| tab.pinned)
            .map(|tab| tab.id)
            .collect();
        let mut lifecycle = self.lifecycle.borrow_mut();
        for tab in pinned {
            lifecycle.set_protected(tab, true);
        }
    }

    /// Unloads every tab of the window before it closes, nobody sees them any more.
    pub(super) fn release_tabs(&self) {
        self.cancel_context_menu();
        let tabs: Vec<_> = self.session.borrow().tabs().map(|tab| tab.id).collect();
        let mut commands = Vec::new();
        {
            let mut lifecycle = self.lifecycle.borrow_mut();
            for tab in tabs {
                commands.extend(lifecycle.sleep(tab));
            }
        }
        self.apply(commands);
    }

    /// Deletes the workspace of a closing window if it holds nothing worth keeping and is not the
    /// last one.
    pub(super) fn remove_workspace_if_empty(&self) {
        let workspace = self.session.borrow().workspace;
        let Ok(workspaces) = self.storage.workspaces() else {
            return;
        };
        if workspaces.len() <= 1 || !workspaces.iter().any(|entry| entry.id == workspace) {
            return;
        }
        let empty = self
            .storage
            .all_tabs_in_workspace(workspace)
            .is_ok_and(|tabs| tabs.iter().all(|tab| tab.is_blank()));
        if empty && let Err(error) = self.storage.delete_workspace(workspace) {
            tracing::warn!(%error, "empty workspace could not be deleted");
        }
    }
}
