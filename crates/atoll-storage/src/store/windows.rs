use atoll_core::{WindowId, WindowPlacement, WorkspaceId};
use rusqlite::{Row, params};

use crate::connection::Connected;
use crate::error::StorageResult;

const COLUMNS: &str = "id, workspace_id, x, y, width, height, maximized";

/// The windows that were open when Atoll last closed.
pub trait WindowStore: Connected {
    /// Every remembered window in the order they were opened.
    fn windows(&self) -> StorageResult<Vec<WindowPlacement>> {
        let mut statement = self.connection().prepare_cached(&format!(
            "SELECT {COLUMNS} FROM window ORDER BY position, id"
        ))?;
        let windows = statement
            .query_map([], placement)?
            .collect::<Result<_, _>>()?;
        Ok(windows)
    }

    /// Remembers a new window showing `workspace`.
    fn create_window(&self, workspace: WorkspaceId) -> StorageResult<WindowPlacement> {
        self.connection()
            .prepare_cached(
                "INSERT INTO window (workspace_id, position)
                 VALUES (?1, (SELECT COALESCE(MAX(position) + 1, 0) FROM window))",
            )?
            .execute([workspace.0])?;
        Ok(WindowPlacement {
            id: WindowId(self.connection().last_insert_rowid()),
            workspace_id: workspace,
            position: None,
            size: None,
            maximized: false,
        })
    }

    fn update_window_placement(&self, placement: &WindowPlacement) -> StorageResult<()> {
        let (x, y) = placement.position.unzip();
        let (width, height) = placement.size.unzip();
        self.connection()
            .prepare_cached(
                "UPDATE window SET x = ?1, y = ?2, width = ?3, height = ?4, maximized = ?5
                 WHERE id = ?6",
            )?
            .execute(params![
                x,
                y,
                width,
                height,
                placement.maximized,
                placement.id.0
            ])?;
        Ok(())
    }

    fn set_window_workspace(&self, window: WindowId, workspace: WorkspaceId) -> StorageResult<()> {
        self.connection()
            .prepare_cached("UPDATE window SET workspace_id = ?1 WHERE id = ?2")?
            .execute(params![workspace.0, window.0])?;
        Ok(())
    }

    fn delete_window(&self, window: WindowId) -> StorageResult<()> {
        self.connection()
            .prepare_cached("DELETE FROM window WHERE id = ?1")?
            .execute([window.0])?;
        Ok(())
    }
}

impl<T: Connected + ?Sized> WindowStore for T {}

fn placement(row: &Row<'_>) -> rusqlite::Result<WindowPlacement> {
    let x: Option<i32> = row.get(2)?;
    let y: Option<i32> = row.get(3)?;
    let width: Option<u32> = row.get(4)?;
    let height: Option<u32> = row.get(5)?;
    Ok(WindowPlacement {
        id: WindowId(row.get(0)?),
        workspace_id: WorkspaceId(row.get(1)?),
        position: x.zip(y),
        size: width.zip(height),
        maximized: row.get(6)?,
    })
}
