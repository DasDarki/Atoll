use atoll_core::{TabId, Timestamp};
use rusqlite::params;

use crate::connection::Connected;
use crate::error::StorageResult;

const ENTRIES_PER_TAB: i64 = 100;

/// One navigation recorded for a tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    pub url: String,
    pub title: Option<String>,
    pub visited_at: Timestamp,
}

/// The per-tab navigation log.
///
/// The engine forgets back/forward history when a tab goes to sleep. This log survives that and
/// acts as a replacement "back" list.
pub trait HistoryStore: Connected {
    /// Appends a visit unless it repeats the most recent entry of the tab.
    fn record_visit(
        &self,
        tab: TabId,
        url: &str,
        title: Option<&str>,
        at: Timestamp,
    ) -> StorageResult<()> {
        if self
            .tab_history(tab, 1)?
            .first()
            .is_some_and(|latest| latest.url == url)
        {
            return Ok(());
        }
        self.connection()
            .prepare_cached(
                "INSERT INTO tab_history (tab_id, url, title, visited_at) VALUES (?1, ?2, ?3, ?4)",
            )?
            .execute(params![tab.0, url, title, at.0])?;
        self.connection()
            .prepare_cached(
                "DELETE FROM tab_history WHERE tab_id = ?1 AND id NOT IN
                 (SELECT id FROM tab_history WHERE tab_id = ?1 ORDER BY id DESC LIMIT ?2)",
            )?
            .execute(params![tab.0, ENTRIES_PER_TAB])?;
        Ok(())
    }

    /// Updates the title of the most recent entry if it belongs to `url`.
    fn update_visit_title(&self, tab: TabId, url: &str, title: &str) -> StorageResult<()> {
        self.connection()
            .prepare_cached(
                "UPDATE tab_history SET title = ?1 WHERE id =
                 (SELECT id FROM tab_history WHERE tab_id = ?2 ORDER BY id DESC LIMIT 1) AND url = ?3",
            )?
            .execute(params![title, tab.0, url])?;
        Ok(())
    }

    /// The most recent entries of a tab, newest first.
    fn tab_history(&self, tab: TabId, limit: usize) -> StorageResult<Vec<HistoryEntry>> {
        let mut statement = self.connection().prepare_cached(
            "SELECT url, title, visited_at FROM tab_history WHERE tab_id = ?1 ORDER BY id DESC LIMIT ?2",
        )?;
        let entries = statement
            .query_map(params![tab.0, limit as i64], |row| {
                Ok(HistoryEntry {
                    url: row.get(0)?,
                    title: row.get(1)?,
                    visited_at: Timestamp(row.get(2)?),
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(entries)
    }

    /// Steps back in the replacement history: drops the entry for `current_url` and returns the
    /// address before it, if there is one.
    fn step_back(&self, tab: TabId, current_url: &str) -> StorageResult<Option<String>> {
        let entries = self.tab_history(tab, 2)?;
        let Some(latest) = entries.first() else {
            return Ok(None);
        };
        if latest.url != current_url {
            return Ok(Some(latest.url.clone()));
        }
        let Some(previous) = entries.get(1) else {
            return Ok(None);
        };
        self.connection()
            .prepare_cached(
                "DELETE FROM tab_history WHERE id = (SELECT id FROM tab_history WHERE tab_id = ?1 ORDER BY id DESC LIMIT 1)",
            )?
            .execute([tab.0])?;
        Ok(Some(previous.url.clone()))
    }
}

impl<T: Connected + ?Sized> HistoryStore for T {}
