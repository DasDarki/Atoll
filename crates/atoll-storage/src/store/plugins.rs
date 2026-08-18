use atoll_core::Timestamp;
use rusqlite::{Row, params};

use crate::connection::Connected;
use crate::error::StorageResult;

/// What Atoll remembers about an installed extension between starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginRecord {
    pub id: String,
    pub version: String,
    pub enabled: bool,
    /// The permissions the user granted, by their names.
    pub permissions: Vec<String>,
    /// Where it came from: `folder`, a zip file or a git address.
    pub source: String,
    pub installed_at: Timestamp,
}

const COLUMNS: &str = "id, version, enabled, permissions, source, installed_at";

/// Installed extensions and the data they keep.
pub trait PluginStore: Connected {
    fn plugins(&self) -> StorageResult<Vec<PluginRecord>> {
        let mut statement = self
            .connection()
            .prepare_cached(&format!("SELECT {COLUMNS} FROM plugin ORDER BY id"))?;
        let plugins = statement.query_map([], plugin)?.collect::<Result<_, _>>()?;
        Ok(plugins)
    }

    fn plugin(&self, id: &str) -> StorageResult<Option<PluginRecord>> {
        let mut statement = self
            .connection()
            .prepare_cached(&format!("SELECT {COLUMNS} FROM plugin WHERE id = ?1"))?;
        let mut found = statement.query_map([id], plugin)?;
        found.next().transpose().map_err(Into::into)
    }

    /// Adds the extension or updates what is known about it, keeping its data.
    fn save_plugin(&self, record: &PluginRecord) -> StorageResult<()> {
        self.connection()
            .prepare_cached(
                "INSERT INTO plugin (id, version, enabled, permissions, source, installed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(id) DO UPDATE SET version = ?2, enabled = ?3, permissions = ?4,
                 source = ?5",
            )?
            .execute(params![
                record.id,
                record.version,
                i64::from(record.enabled),
                record.permissions.join(","),
                record.source,
                record.installed_at.0
            ])?;
        Ok(())
    }

    fn set_plugin_enabled(&self, id: &str, enabled: bool) -> StorageResult<()> {
        self.connection()
            .prepare_cached("UPDATE plugin SET enabled = ?2 WHERE id = ?1")?
            .execute(params![id, i64::from(enabled)])?;
        Ok(())
    }

    /// Forgets the extension together with everything it stored.
    fn remove_plugin(&self, id: &str) -> StorageResult<()> {
        self.connection()
            .prepare_cached("DELETE FROM plugin WHERE id = ?1")?
            .execute([id])?;
        self.clear_plugin_data(id)
    }

    fn plugin_value(&self, id: &str, key: &str) -> StorageResult<Option<String>> {
        let mut statement = self
            .connection()
            .prepare_cached("SELECT value FROM plugin_data WHERE plugin_id = ?1 AND key = ?2")?;
        let mut found = statement.query_map(params![id, key], |row| row.get(0))?;
        found.next().transpose().map_err(Into::into)
    }

    fn set_plugin_value(&self, id: &str, key: &str, value: Option<&str>) -> StorageResult<()> {
        match value {
            Some(value) => self
                .connection()
                .prepare_cached(
                    "INSERT INTO plugin_data (plugin_id, key, value) VALUES (?1, ?2, ?3)
                     ON CONFLICT(plugin_id, key) DO UPDATE SET value = ?3",
                )?
                .execute(params![id, key, value])?,
            None => self
                .connection()
                .prepare_cached("DELETE FROM plugin_data WHERE plugin_id = ?1 AND key = ?2")?
                .execute(params![id, key])?,
        };
        Ok(())
    }

    fn plugin_keys(&self, id: &str) -> StorageResult<Vec<String>> {
        let mut statement = self
            .connection()
            .prepare_cached("SELECT key FROM plugin_data WHERE plugin_id = ?1 ORDER BY key")?;
        let keys = statement
            .query_map([id], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        Ok(keys)
    }

    fn clear_plugin_data(&self, id: &str) -> StorageResult<()> {
        self.connection()
            .prepare_cached("DELETE FROM plugin_data WHERE plugin_id = ?1")?
            .execute([id])?;
        Ok(())
    }
}

impl<T: Connected + ?Sized> PluginStore for T {}

fn plugin(row: &Row<'_>) -> rusqlite::Result<PluginRecord> {
    let permissions: String = row.get(3)?;
    Ok(PluginRecord {
        id: row.get(0)?,
        version: row.get(1)?,
        enabled: row.get::<_, i64>(2)? != 0,
        permissions: permissions
            .split(',')
            .filter(|permission| !permission.is_empty())
            .map(str::to_owned)
            .collect(),
        source: row.get(4)?,
        installed_at: Timestamp(row.get(5)?),
    })
}
