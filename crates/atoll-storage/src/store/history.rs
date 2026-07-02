use atoll_core::{TabId, Timestamp, VisitedPage};
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
                "INSERT INTO history (url, title, visit_count, last_visited_at) VALUES (?1, ?2, 1, ?3)
                 ON CONFLICT(url) DO UPDATE SET
                     visit_count = visit_count + 1,
                     last_visited_at = excluded.last_visited_at,
                     title = COALESCE(excluded.title, title)",
            )?
            .execute(params![url, title, at.0])?;
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
        self.connection()
            .prepare_cached("UPDATE history SET title = ?1 WHERE url = ?2")?
            .execute(params![title, url])?;
        Ok(())
    }

    /// The most recently visited pages, newest first.
    fn visited_pages(&self, limit: usize) -> StorageResult<Vec<VisitedPage>> {
        let mut statement = self.connection().prepare_cached(
            "SELECT url, title, visit_count, last_visited_at FROM history
             ORDER BY last_visited_at DESC LIMIT ?1",
        )?;
        let pages = statement
            .query_map([limit as i64], visited_page)?
            .collect::<Result<_, _>>()?;
        Ok(pages)
    }

    /// Pages whose address or title contains `text`, most visited first.
    fn visited_pages_containing(
        &self,
        text: &str,
        limit: usize,
    ) -> StorageResult<Vec<VisitedPage>> {
        let pattern = format!("%{}%", escape_like(text));
        let mut statement = self.connection().prepare_cached(
            "SELECT url, title, visit_count, last_visited_at FROM history
             WHERE url LIKE ?1 ESCAPE '\\' OR title LIKE ?1 ESCAPE '\\'
             ORDER BY visit_count DESC, last_visited_at DESC LIMIT ?2",
        )?;
        let pages = statement
            .query_map(params![pattern, limit as i64], visited_page)?
            .collect::<Result<_, _>>()?;
        Ok(pages)
    }

    /// Forgets all visited pages, including the per-tab logs.
    fn clear_history(&self) -> StorageResult<()> {
        self.connection()
            .execute_batch("DELETE FROM history; DELETE FROM tab_history;")?;
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

fn visited_page(row: &rusqlite::Row<'_>) -> rusqlite::Result<VisitedPage> {
    Ok(VisitedPage {
        url: row.get(0)?,
        title: row.get(1)?,
        visit_count: row.get(2)?,
        last_visited_at: Timestamp(row.get(3)?),
    })
}

fn escape_like(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        if matches!(character, '%' | '_' | '\\') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}
