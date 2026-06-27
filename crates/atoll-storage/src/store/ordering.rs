use rusqlite::{Connection, params};

use crate::error::StorageResult;

#[derive(Clone, Copy)]
pub(crate) enum OrderedTable {
    Workspace,
    Island,
    Tab,
    Bookmark,
}

impl OrderedTable {
    fn update_statement(self) -> &'static str {
        match self {
            OrderedTable::Workspace => "UPDATE workspace SET position = ?1 WHERE id = ?2",
            OrderedTable::Island => "UPDATE island SET position = ?1 WHERE id = ?2",
            OrderedTable::Tab => "UPDATE tab SET position = ?1 WHERE id = ?2",
            OrderedTable::Bookmark => "UPDATE bookmark SET position = ?1 WHERE id = ?2",
        }
    }
}

/// Writes contiguous positions for `ids` in the given order.
pub(crate) fn renumber(
    connection: &Connection,
    table: OrderedTable,
    ids: &[i64],
) -> StorageResult<()> {
    let mut statement = connection.prepare_cached(table.update_statement())?;
    for (position, id) in ids.iter().enumerate() {
        statement.execute(params![position as i64, id])?;
    }
    Ok(())
}

/// Inserts `id` into `ids` at `index`, clamping the index to the valid range.
pub(crate) fn insert_at(ids: &mut Vec<i64>, id: i64, index: Option<usize>) {
    ids.retain(|existing| *existing != id);
    let index = index.unwrap_or(ids.len()).min(ids.len());
    ids.insert(index, id);
}
