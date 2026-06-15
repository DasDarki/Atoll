use atoll_core::{FaviconId, Timestamp};
use rusqlite::{OptionalExtension, params, params_from_iter};

use crate::connection::Connected;
use crate::error::StorageResult;

pub trait FaviconStore: Connected {
    /// Stores the favicon for `host`, replacing an older one, and returns its id.
    fn upsert_favicon(&self, host: &str, data: &[u8], at: Timestamp) -> StorageResult<FaviconId> {
        let id = self
            .connection()
            .prepare_cached(
                "INSERT INTO favicon (host, data, updated_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(host) DO UPDATE SET data = excluded.data, updated_at = excluded.updated_at
                 RETURNING id",
            )?
            .query_row(params![host, data, at.0], |row| row.get(0))?;
        Ok(FaviconId(id))
    }

    fn favicon(&self, id: FaviconId) -> StorageResult<Option<Vec<u8>>> {
        Ok(self
            .connection()
            .prepare_cached("SELECT data FROM favicon WHERE id = ?1")?
            .query_row([id.0], |row| row.get(0))
            .optional()?)
    }

    /// The id of every stored favicon together with its host.
    fn favicon_hosts(&self) -> StorageResult<Vec<(FaviconId, String)>> {
        let mut statement = self
            .connection()
            .prepare_cached("SELECT id, host FROM favicon")?;
        let hosts = statement
            .query_map([], |row| Ok((FaviconId(row.get(0)?), row.get(1)?)))?
            .collect::<Result<_, _>>()?;
        Ok(hosts)
    }

    fn favicons(&self, ids: &[FaviconId]) -> StorageResult<Vec<(FaviconId, Vec<u8>)>> {
        let mut favicons = Vec::with_capacity(ids.len());
        for chunk in ids.chunks(500) {
            let placeholders = vec!["?"; chunk.len()].join(", ");
            let mut statement = self.connection().prepare(&format!(
                "SELECT id, data FROM favicon WHERE id IN ({placeholders})"
            ))?;
            let rows = statement
                .query_map(params_from_iter(chunk.iter().map(|id| id.0)), |row| {
                    Ok((FaviconId(row.get(0)?), row.get(1)?))
                })?;
            for row in rows {
                favicons.push(row?);
            }
        }
        Ok(favicons)
    }
}

impl<T: Connected + ?Sized> FaviconStore for T {}
