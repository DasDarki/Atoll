use rusqlite::{OptionalExtension, params};

use crate::connection::Connected;
use crate::error::StorageResult;

/// A simple key-value store for preferences and session state.
pub trait SettingsStore: Connected {
    fn setting(&self, key: &str) -> StorageResult<Option<String>> {
        Ok(self
            .connection()
            .prepare_cached("SELECT value FROM setting WHERE key = ?1")?
            .query_row([key], |row| row.get(0))
            .optional()?)
    }

    fn setting_i64(&self, key: &str) -> StorageResult<Option<i64>> {
        Ok(self.setting(key)?.and_then(|value| value.parse().ok()))
    }

    fn set_setting(&self, key: &str, value: &str) -> StorageResult<()> {
        self.connection()
            .prepare_cached(
                "INSERT INTO setting (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            )?
            .execute(params![key, value])?;
        Ok(())
    }

    fn remove_setting(&self, key: &str) -> StorageResult<()> {
        self.connection()
            .prepare_cached("DELETE FROM setting WHERE key = ?1")?
            .execute([key])?;
        Ok(())
    }
}

impl<T: Connected + ?Sized> SettingsStore for T {}
