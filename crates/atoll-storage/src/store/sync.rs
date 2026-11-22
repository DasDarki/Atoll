use rusqlite::params;

use crate::connection::Connected;
use crate::error::StorageResult;

/// A record that changed since it was last sent to the sync server, as the triggers of the
/// database wrote it down. Several changes of one record leave one entry, the newest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxEntry {
    /// The kind of the record as the sync protocol names it, for example `tab`.
    pub kind: String,
    pub sync_id: String,
    pub removed: bool,
    pub changed_at: i64,
}

pub trait SyncStore: Connected {
    /// Everything that changed and was not sent yet, oldest first.
    fn sync_outbox(&self) -> StorageResult<Vec<OutboxEntry>> {
        let mut statement = self.connection().prepare_cached(
            "SELECT kind, sync_id, removed, changed_at FROM sync_outbox ORDER BY changed_at, kind, sync_id",
        )?;
        let entries = statement
            .query_map([], |row| {
                Ok(OutboxEntry {
                    kind: row.get(0)?,
                    sync_id: row.get(1)?,
                    removed: row.get(2)?,
                    changed_at: row.get(3)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(entries)
    }

    /// Takes sent entries off the list. An entry that changed again since it was read stays,
    /// so a change made while sending is not lost.
    fn forget_outbox(&self, entries: &[OutboxEntry]) -> StorageResult<()> {
        let mut statement = self.connection().prepare_cached(
            "DELETE FROM sync_outbox WHERE kind = ?1 AND sync_id = ?2 AND changed_at = ?3 AND removed = ?4",
        )?;
        for entry in entries {
            statement.execute(params![
                entry.kind,
                entry.sync_id,
                entry.changed_at,
                entry.removed
            ])?;
        }
        Ok(())
    }

    /// Whether changes are being taken over from the server. While they are, the triggers write
    /// nothing down, or every change would travel back to where it came from.
    fn set_sync_applying(&self, applying: bool) -> StorageResult<()> {
        self.connection()
            .prepare_cached("UPDATE sync_control SET applying = ?1 WHERE id = 1")?
            .execute([applying])?;
        Ok(())
    }
}

impl<T: Connected + ?Sized> SyncStore for T {}
