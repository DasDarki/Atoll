use atoll_core::{Workspace, WorkspaceId};
use rusqlite::{OptionalExtension, params};

use super::ordering::{OrderedTable, insert_at, renumber};
use super::rows::{self, WORKSPACE_COLUMNS};
use crate::connection::Connected;
use crate::error::{StorageError, StorageResult};

pub trait WorkspaceStore: Connected {
    fn workspaces(&self) -> StorageResult<Vec<Workspace>> {
        let mut statement = self.connection().prepare_cached(&format!(
            "SELECT {WORKSPACE_COLUMNS} FROM workspace ORDER BY position, id"
        ))?;
        let workspaces = statement
            .query_map([], rows::workspace)?
            .collect::<Result<_, _>>()?;
        Ok(workspaces)
    }

    fn workspace(&self, id: WorkspaceId) -> StorageResult<Workspace> {
        self.connection()
            .prepare_cached(&format!(
                "SELECT {WORKSPACE_COLUMNS} FROM workspace WHERE id = ?1"
            ))?
            .query_row([id.0], rows::workspace)
            .optional()?
            .ok_or(StorageError::NotFound {
                entity: "workspace",
                id: id.0,
            })
    }

    fn create_workspace(&self, name: &str) -> StorageResult<Workspace> {
        let position: i64 = self.connection().query_row(
            "SELECT COALESCE(MAX(position) + 1, 0) FROM workspace",
            [],
            |row| row.get(0),
        )?;
        self.connection()
            .prepare_cached("INSERT INTO workspace (name, position) VALUES (?1, ?2)")?
            .execute(params![name, position])?;
        Ok(Workspace {
            id: WorkspaceId(self.connection().last_insert_rowid()),
            name: name.to_owned(),
            position,
        })
    }

    fn rename_workspace(&self, id: WorkspaceId, name: &str) -> StorageResult<()> {
        self.connection()
            .prepare_cached("UPDATE workspace SET name = ?1 WHERE id = ?2")?
            .execute(params![name, id.0])?;
        Ok(())
    }

    fn delete_workspace(&self, id: WorkspaceId) -> StorageResult<()> {
        self.connection()
            .prepare_cached("DELETE FROM workspace WHERE id = ?1")?
            .execute([id.0])?;
        Ok(())
    }

    fn move_workspace(&self, id: WorkspaceId, index: usize) -> StorageResult<()> {
        let mut ids: Vec<i64> = self
            .workspaces()?
            .into_iter()
            .map(|workspace| workspace.id.0)
            .collect();
        insert_at(&mut ids, id.0, Some(index));
        renumber(self.connection(), OrderedTable::Workspace, &ids)
    }
}

impl<T: Connected + ?Sized> WorkspaceStore for T {}
