use atoll_core::{FaviconId, Island, IslandId, Tab, TabId, Timestamp, WorkspaceId};
use rusqlite::{OptionalExtension, params};

use super::ordering::{OrderedTable, insert_at, renumber};
use super::rows::{self, ISLAND_COLUMNS, TAB_COLUMNS};
use crate::connection::Connected;
use crate::error::{StorageError, StorageResult};

/// Parameters for [`TabStore::create_tab`].
#[derive(Debug, Clone)]
pub struct NewTab<'a> {
    pub island_id: IslandId,
    pub url: &'a str,
    pub title: Option<&'a str>,
    /// Position inside the island, appended when `None`.
    pub index: Option<usize>,
    pub created_at: Timestamp,
    /// A private tab keeps nothing on disk: only this row says that it is there, its address and
    /// its title stay in memory and the row goes away with the tab.
    pub private: bool,
}

/// A tab together with where it lives, for searches across all workspaces.
#[derive(Debug, Clone)]
pub struct TabEntry {
    pub tab: Tab,
    pub island: Island,
    pub workspace_name: String,
}

pub trait TabStore: Connected {
    /// Every tab of every workspace including archived islands, most recently used first.
    fn all_tab_entries(&self) -> StorageResult<Vec<TabEntry>> {
        let mut statement = self.connection().prepare_cached(&format!(
            "SELECT {TAB_COLUMNS}, {ISLAND_COLUMNS}, workspace.name
             FROM tab
             JOIN island ON island.id = tab.island_id
             JOIN workspace ON workspace.id = island.workspace_id
             ORDER BY tab.last_used_at DESC NULLS LAST, tab.id DESC"
        ))?;
        let entries = statement
            .query_map([], |row| {
                Ok(TabEntry {
                    tab: rows::tab(row)?,
                    island: rows::island_at(row, 10)?,
                    workspace_name: row.get(18)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(entries)
    }

    /// Tabs of all active islands in a workspace, ordered by island and then by tab position.
    fn tabs_in_workspace(&self, workspace: WorkspaceId) -> StorageResult<Vec<Tab>> {
        let mut statement = self.connection().prepare_cached(&format!(
            "SELECT {TAB_COLUMNS} FROM tab JOIN island ON island.id = tab.island_id
             WHERE island.workspace_id = ?1 AND island.archived_at IS NULL
             ORDER BY island.position, island.id, tab.position, tab.id"
        ))?;
        let tabs = statement
            .query_map([workspace.0], rows::tab)?
            .collect::<Result<_, _>>()?;
        Ok(tabs)
    }

    fn tabs_in_island(&self, island: IslandId) -> StorageResult<Vec<Tab>> {
        let mut statement = self.connection().prepare_cached(&format!(
            "SELECT {TAB_COLUMNS} FROM tab WHERE tab.island_id = ?1 ORDER BY tab.position, tab.id"
        ))?;
        let tabs = statement
            .query_map([island.0], rows::tab)?
            .collect::<Result<_, _>>()?;
        Ok(tabs)
    }

    /// Every tab of a workspace including archived islands, used for duplicate detection.
    fn all_tabs_in_workspace(&self, workspace: WorkspaceId) -> StorageResult<Vec<Tab>> {
        let mut statement = self.connection().prepare_cached(&format!(
            "SELECT {TAB_COLUMNS} FROM tab JOIN island ON island.id = tab.island_id
             WHERE island.workspace_id = ?1
             ORDER BY island.position, island.id, tab.position, tab.id"
        ))?;
        let tabs = statement
            .query_map([workspace.0], rows::tab)?
            .collect::<Result<_, _>>()?;
        Ok(tabs)
    }

    fn tab(&self, id: TabId) -> StorageResult<Tab> {
        self.connection()
            .prepare_cached(&format!("SELECT {TAB_COLUMNS} FROM tab WHERE tab.id = ?1"))?
            .query_row([id.0], rows::tab)
            .optional()?
            .ok_or(StorageError::NotFound {
                entity: "tab",
                id: id.0,
            })
    }

    /// Throws away what is left of private tabs. They keep nothing, so there is nothing to
    /// restore, and a crash must not leave them behind either.
    fn clear_private_tabs(&self) -> StorageResult<usize> {
        let removed = self
            .connection()
            .prepare_cached("DELETE FROM tab WHERE private = 1")?
            .execute([])?;
        Ok(removed)
    }

    fn create_tab(&self, new: NewTab<'_>) -> StorageResult<Tab> {
        let position = match new.index {
            Some(_) => 0,
            None => self
                .connection()
                .prepare_cached(
                    "SELECT COALESCE(MAX(position) + 1, 0) FROM tab WHERE island_id = ?1",
                )?
                .query_row([new.island_id.0], |row| row.get(0))?,
        };
        self.connection()
            .prepare_cached(
                "INSERT INTO tab (island_id, url, title, position, created_at, private)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?
            .execute(params![
                new.island_id.0,
                new.url,
                new.title,
                position,
                new.created_at.0,
                i64::from(new.private)
            ])?;
        let id = TabId(self.connection().last_insert_rowid());
        if new.index.is_some() {
            place_tab(self, id, new.island_id, new.index)?;
        }
        self.tab(id)
    }

    fn update_tab_address(&self, id: TabId, url: &str) -> StorageResult<()> {
        self.connection()
            .prepare_cached("UPDATE tab SET url = ?1 WHERE id = ?2")?
            .execute(params![url, id.0])?;
        Ok(())
    }

    fn update_tab_title(&self, id: TabId, title: Option<&str>) -> StorageResult<()> {
        self.connection()
            .prepare_cached("UPDATE tab SET title = ?1 WHERE id = ?2")?
            .execute(params![title, id.0])?;
        Ok(())
    }

    fn set_tab_favicon(&self, id: TabId, favicon: Option<FaviconId>) -> StorageResult<()> {
        self.connection()
            .prepare_cached("UPDATE tab SET favicon_id = ?1 WHERE id = ?2")?
            .execute(params![favicon.map(|favicon| favicon.0), id.0])?;
        Ok(())
    }

    fn set_tab_pinned(&self, id: TabId, pinned: bool) -> StorageResult<()> {
        self.connection()
            .prepare_cached("UPDATE tab SET pinned = ?1 WHERE id = ?2")?
            .execute(params![pinned, id.0])?;
        Ok(())
    }

    fn set_tab_scroll(&self, id: TabId, scroll_y: Option<i32>) -> StorageResult<()> {
        self.connection()
            .prepare_cached("UPDATE tab SET scroll_y = ?1 WHERE id = ?2")?
            .execute(params![scroll_y, id.0])?;
        Ok(())
    }

    /// Marks the tab and its island as used at `at`.
    fn touch_tab(&self, id: TabId, at: Timestamp) -> StorageResult<()> {
        self.connection()
            .prepare_cached("UPDATE tab SET last_used_at = ?1 WHERE id = ?2")?
            .execute(params![at.0, id.0])?;
        self.connection()
            .prepare_cached(
                "UPDATE island SET last_used_at = ?1 WHERE id = (SELECT island_id FROM tab WHERE id = ?2)",
            )?
            .execute(params![at.0, id.0])?;
        Ok(())
    }

    fn delete_tab(&self, id: TabId) -> StorageResult<()> {
        let tab = self.tab(id)?;
        self.connection()
            .prepare_cached("DELETE FROM tab WHERE id = ?1")?
            .execute([id.0])?;
        let ids = tab_ids(self, tab.island_id)?;
        renumber(self.connection(), OrderedTable::Tab, &ids)
    }

    /// Moves a tab to `index` inside `island`, which may differ from its current island.
    fn move_tab(&self, id: TabId, island: IslandId, index: usize) -> StorageResult<()> {
        let tab = self.tab(id)?;
        self.connection()
            .prepare_cached("UPDATE tab SET island_id = ?1 WHERE id = ?2")?
            .execute(params![island.0, id.0])?;
        if tab.island_id != island {
            let ids = tab_ids(self, tab.island_id)?;
            renumber(self.connection(), OrderedTable::Tab, &ids)?;
        }
        place_tab(self, id, island, Some(index))
    }
}

impl<T: Connected + ?Sized> TabStore for T {}

fn tab_ids<S: Connected + ?Sized>(store: &S, island: IslandId) -> StorageResult<Vec<i64>> {
    let mut statement = store
        .connection()
        .prepare_cached("SELECT id FROM tab WHERE island_id = ?1 ORDER BY position, id")?;
    let ids = statement
        .query_map([island.0], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    Ok(ids)
}

fn place_tab<S: Connected + ?Sized>(
    store: &S,
    id: TabId,
    island: IslandId,
    index: Option<usize>,
) -> StorageResult<()> {
    let mut ids = tab_ids(store, island)?;
    insert_at(&mut ids, id.0, index);
    renumber(store.connection(), OrderedTable::Tab, &ids)
}
