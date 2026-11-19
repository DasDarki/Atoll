use atoll_core::{Island, IslandColor, IslandId, Timestamp, WorkspaceId};
use rusqlite::{OptionalExtension, params};

use super::ordering::{OrderedTable, insert_at, renumber};
use super::rows::{self, ISLAND_COLUMNS};
use crate::connection::Connected;
use crate::error::{StorageError, StorageResult};

/// Parameters for [`IslandStore::create_island`].
#[derive(Debug, Clone)]
pub struct NewIsland<'a> {
    pub workspace_id: WorkspaceId,
    pub name: &'a str,
    pub color: IslandColor,
    /// Position among the workspace's active islands, appended when `None`.
    pub index: Option<usize>,
    /// Counts as the first use, archiving is only suggested for islands unused for a while.
    pub created_at: Timestamp,
}

pub trait IslandStore: Connected {
    /// The islands of a workspace in display order.
    fn islands(&self, workspace: WorkspaceId) -> StorageResult<Vec<Island>> {
        let mut statement = self.connection().prepare_cached(&format!(
            "SELECT {ISLAND_COLUMNS} FROM island
             WHERE workspace_id = ?1
             ORDER BY position, id"
        ))?;
        let islands = statement
            .query_map([workspace.0], rows::island)?
            .collect::<Result<_, _>>()?;
        Ok(islands)
    }

    /// The names of every island of a workspace.
    fn island_names(&self, workspace: WorkspaceId) -> StorageResult<Vec<String>> {
        let mut statement = self
            .connection()
            .prepare_cached("SELECT name FROM island WHERE workspace_id = ?1")?;
        let names = statement
            .query_map([workspace.0], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        Ok(names)
    }

    fn island(&self, id: IslandId) -> StorageResult<Island> {
        self.connection()
            .prepare_cached(&format!(
                "SELECT {ISLAND_COLUMNS} FROM island WHERE id = ?1"
            ))?
            .query_row([id.0], rows::island)
            .optional()?
            .ok_or(StorageError::NotFound {
                entity: "island",
                id: id.0,
            })
    }

    /// Islands whose last use lies before `before`, candidates for the archive suggestion.
    fn islands_unused_since(&self, before: Timestamp) -> StorageResult<Vec<Island>> {
        let mut statement = self.connection().prepare_cached(&format!(
            "SELECT {ISLAND_COLUMNS} FROM island
             WHERE COALESCE(last_used_at, 0) < ?1
             ORDER BY last_used_at"
        ))?;
        let islands = statement
            .query_map([before.0], rows::island)?
            .collect::<Result<_, _>>()?;
        Ok(islands)
    }

    fn create_island(&self, new: NewIsland<'_>) -> StorageResult<Island> {
        self.connection()
            .prepare_cached(
                "INSERT INTO island (workspace_id, name, position, color, last_used_at)
                 VALUES (?1, ?2, 0, ?3, ?4)",
            )?
            .execute(params![
                new.workspace_id.0,
                new.name,
                new.color.index(),
                new.created_at.0
            ])?;
        let id = IslandId(self.connection().last_insert_rowid());
        place_island(self, id, new.workspace_id, new.index)?;
        self.island(id)
    }

    fn rename_island(&self, id: IslandId, name: &str) -> StorageResult<()> {
        self.connection()
            .prepare_cached("UPDATE island SET name = ?1 WHERE id = ?2")?
            .execute(params![name, id.0])?;
        Ok(())
    }

    fn set_island_collapsed(&self, id: IslandId, collapsed: bool) -> StorageResult<()> {
        self.connection()
            .prepare_cached("UPDATE island SET collapsed = ?1 WHERE id = ?2")?
            .execute(params![collapsed, id.0])?;
        Ok(())
    }

    fn set_island_color(&self, id: IslandId, color: IslandColor) -> StorageResult<()> {
        self.connection()
            .prepare_cached("UPDATE island SET color = ?1 WHERE id = ?2")?
            .execute(params![color.index(), id.0])?;
        Ok(())
    }

    fn touch_island(&self, id: IslandId, at: Timestamp) -> StorageResult<()> {
        self.connection()
            .prepare_cached("UPDATE island SET last_used_at = ?1 WHERE id = ?2")?
            .execute(params![at.0, id.0])?;
        Ok(())
    }

    fn delete_island(&self, id: IslandId) -> StorageResult<()> {
        let island = self.island(id)?;
        self.connection()
            .prepare_cached("DELETE FROM island WHERE id = ?1")?
            .execute([id.0])?;
        let ids = island_ids(self, island.workspace_id)?;
        renumber(self.connection(), OrderedTable::Island, &ids)
    }

    /// Puts the islands of a workspace in the order of `ids`, which names all of them.
    fn order_islands(&self, ids: &[IslandId]) -> StorageResult<()> {
        let ids: Vec<i64> = ids.iter().map(|id| id.0).collect();
        renumber(self.connection(), OrderedTable::Island, &ids)
    }

    /// Moves an island to `index` within `workspace`, which may differ from its current one.
    fn move_island(&self, id: IslandId, workspace: WorkspaceId, index: usize) -> StorageResult<()> {
        let island = self.island(id)?;
        self.connection()
            .prepare_cached("UPDATE island SET workspace_id = ?1 WHERE id = ?2")?
            .execute(params![workspace.0, id.0])?;
        if island.workspace_id != workspace {
            let ids = island_ids(self, island.workspace_id)?;
            renumber(self.connection(), OrderedTable::Island, &ids)?;
        }
        place_island(self, id, workspace, Some(index))
    }
}

impl<T: Connected + ?Sized> IslandStore for T {}

pub(crate) fn island_ids<S: Connected + ?Sized>(
    store: &S,
    workspace: WorkspaceId,
) -> StorageResult<Vec<i64>> {
    let mut statement = store
        .connection()
        .prepare_cached("SELECT id FROM island WHERE workspace_id = ?1 ORDER BY position, id")?;
    let ids = statement
        .query_map([workspace.0], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    Ok(ids)
}

pub(crate) fn place_island<S: Connected + ?Sized>(
    store: &S,
    id: IslandId,
    workspace: WorkspaceId,
    index: Option<usize>,
) -> StorageResult<()> {
    let mut ids = island_ids(store, workspace)?;
    insert_at(&mut ids, id.0, index);
    renumber(store.connection(), OrderedTable::Island, &ids)
}
