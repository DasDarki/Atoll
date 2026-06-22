use atoll_core::{Island, IslandId, Tab, TabId, Workspace, WorkspaceId};
use atoll_storage::prelude::*;
use atoll_storage::{Storage, StorageResult};

/// An island of the active workspace together with its tabs in display order.
#[derive(Debug, Clone)]
pub struct IslandTabs {
    pub island: Island,
    pub tabs: Vec<Tab>,
}

/// The in-memory copy of the workspace shown in the sidebar.
///
/// Structural changes are written to the database first and the session is reloaded afterwards,
/// the database stays the single source of truth.
#[derive(Debug, Clone)]
pub struct Session {
    pub workspaces: Vec<Workspace>,
    pub workspace: WorkspaceId,
    pub islands: Vec<IslandTabs>,
    pub active_tab: Option<TabId>,
}

impl Session {
    pub fn load(storage: &Storage, workspace: WorkspaceId) -> StorageResult<Self> {
        let workspaces = storage.workspaces()?;
        let mut islands: Vec<IslandTabs> = storage
            .islands(workspace)?
            .into_iter()
            .map(|island| IslandTabs {
                island,
                tabs: Vec::new(),
            })
            .collect();
        for tab in storage.tabs_in_workspace(workspace)? {
            if let Some(entry) = islands
                .iter_mut()
                .find(|entry| entry.island.id == tab.island_id)
            {
                entry.tabs.push(tab);
            }
        }
        Ok(Self {
            workspaces,
            workspace,
            islands,
            active_tab: None,
        })
    }

    pub fn tabs(&self) -> impl Iterator<Item = &Tab> {
        self.islands.iter().flat_map(|entry| entry.tabs.iter())
    }

    pub fn tab(&self, id: TabId) -> Option<&Tab> {
        self.tabs().find(|tab| tab.id == id)
    }

    pub fn tab_mut(&mut self, id: TabId) -> Option<&mut Tab> {
        self.islands
            .iter_mut()
            .flat_map(|entry| entry.tabs.iter_mut())
            .find(|tab| tab.id == id)
    }

    pub fn island(&self, id: IslandId) -> Option<&IslandTabs> {
        self.islands.iter().find(|entry| entry.island.id == id)
    }

    pub fn island_index(&self, id: IslandId) -> Option<usize> {
        self.islands.iter().position(|entry| entry.island.id == id)
    }

    pub fn contains_tab(&self, id: TabId) -> bool {
        self.tab(id).is_some()
    }

    /// The tab to show after `closing` disappears: the next one in its island, then the previous
    /// one, then the first tab of the workspace.
    pub fn successor_of(&self, closing: TabId) -> Option<TabId> {
        for entry in &self.islands {
            if let Some(index) = entry.tabs.iter().position(|tab| tab.id == closing)
                && let Some(next) = entry.tabs.get(index + 1).or_else(|| {
                    index
                        .checked_sub(1)
                        .and_then(|previous| entry.tabs.get(previous))
                })
            {
                return Some(next.id);
            }
        }
        self.tabs().map(|tab| tab.id).find(|id| *id != closing)
    }

    /// Tabs in sidebar order, skipping collapsed islands unless they contain `current`.
    pub fn navigation_order(&self, current: Option<TabId>) -> Vec<TabId> {
        self.islands
            .iter()
            .filter(|entry| {
                !entry.island.collapsed || entry.tabs.iter().any(|tab| Some(tab.id) == current)
            })
            .flat_map(|entry| entry.tabs.iter().map(|tab| tab.id))
            .collect()
    }
}
