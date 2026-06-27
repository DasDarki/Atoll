use atoll_core::{Bookmark, BookmarkId, Timestamp};
use rusqlite::{OptionalExtension, Row, params};

use super::ordering::{OrderedTable, insert_at, renumber};
use crate::connection::Connected;
use crate::error::{StorageError, StorageResult};

const COLUMNS: &str = "id, parent_id, title, url, position";

/// Parameters for [`BookmarkStore::create_bookmark`]. A missing `url` creates a folder.
#[derive(Debug, Clone)]
pub struct NewBookmark<'a> {
    pub parent_id: Option<BookmarkId>,
    pub title: &'a str,
    pub url: Option<&'a str>,
    /// Position among the siblings, appended when `None`.
    pub index: Option<usize>,
    pub created_at: Timestamp,
}

pub trait BookmarkStore: Connected {
    /// Every bookmark and folder, siblings in display order.
    fn bookmarks(&self) -> StorageResult<Vec<Bookmark>> {
        let mut statement = self.connection().prepare_cached(&format!(
            "SELECT {COLUMNS} FROM bookmark ORDER BY parent_id IS NOT NULL, parent_id, position, id"
        ))?;
        let bookmarks = statement
            .query_map([], bookmark)?
            .collect::<Result<_, _>>()?;
        Ok(bookmarks)
    }

    /// The direct children of `parent`, or the bookmarks bar for `None`.
    fn bookmark_children(&self, parent: Option<BookmarkId>) -> StorageResult<Vec<Bookmark>> {
        let mut statement = self.connection().prepare_cached(&format!(
            "SELECT {COLUMNS} FROM bookmark WHERE parent_id IS ?1 ORDER BY position, id"
        ))?;
        let bookmarks = statement
            .query_map([parent.map(|id| id.0)], bookmark)?
            .collect::<Result<_, _>>()?;
        Ok(bookmarks)
    }

    fn bookmark(&self, id: BookmarkId) -> StorageResult<Bookmark> {
        self.connection()
            .prepare_cached(&format!("SELECT {COLUMNS} FROM bookmark WHERE id = ?1"))?
            .query_row([id.0], bookmark)
            .optional()?
            .ok_or(StorageError::NotFound {
                entity: "bookmark",
                id: id.0,
            })
    }

    /// The first bookmark pointing to `url`, if any.
    fn bookmark_by_url(&self, url: &str) -> StorageResult<Option<Bookmark>> {
        Ok(self
            .connection()
            .prepare_cached(&format!(
                "SELECT {COLUMNS} FROM bookmark WHERE url = ?1 ORDER BY id LIMIT 1"
            ))?
            .query_row([url], bookmark)
            .optional()?)
    }

    fn create_bookmark(&self, new: NewBookmark<'_>) -> StorageResult<Bookmark> {
        self.connection()
            .prepare_cached(
                "INSERT INTO bookmark (parent_id, title, url, position, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?
            .execute(params![
                new.parent_id.map(|id| id.0),
                new.title,
                new.url,
                i64::MAX,
                new.created_at.0
            ])?;
        let id = BookmarkId(self.connection().last_insert_rowid());
        place(self, id, new.parent_id, new.index)?;
        self.bookmark(id)
    }

    fn update_bookmark(&self, id: BookmarkId, title: &str, url: Option<&str>) -> StorageResult<()> {
        self.connection()
            .prepare_cached("UPDATE bookmark SET title = ?1, url = ?2 WHERE id = ?3")?
            .execute(params![title, url, id.0])?;
        Ok(())
    }

    /// Moves a bookmark or folder to `index` inside `parent`.
    fn move_bookmark(
        &self,
        id: BookmarkId,
        parent: Option<BookmarkId>,
        index: usize,
    ) -> StorageResult<()> {
        let current = self.bookmark(id)?;
        let mut ancestor = parent;
        while let Some(candidate) = ancestor {
            if candidate == id {
                return Err(StorageError::CyclicBookmark);
            }
            ancestor = self.bookmark(candidate)?.parent_id;
        }
        self.connection()
            .prepare_cached("UPDATE bookmark SET parent_id = ?1 WHERE id = ?2")?
            .execute(params![parent.map(|parent| parent.0), id.0])?;
        if current.parent_id != parent {
            let ids = sibling_ids(self, current.parent_id)?;
            renumber(self.connection(), OrderedTable::Bookmark, &ids)?;
        }
        place(self, id, parent, Some(index))
    }

    /// Deletes a bookmark, or a folder together with everything inside it.
    fn delete_bookmark(&self, id: BookmarkId) -> StorageResult<()> {
        let current = self.bookmark(id)?;
        self.connection()
            .prepare_cached("DELETE FROM bookmark WHERE id = ?1")?
            .execute([id.0])?;
        let ids = sibling_ids(self, current.parent_id)?;
        renumber(self.connection(), OrderedTable::Bookmark, &ids)
    }
}

impl<T: Connected + ?Sized> BookmarkStore for T {}

fn bookmark(row: &Row<'_>) -> rusqlite::Result<Bookmark> {
    Ok(Bookmark {
        id: BookmarkId(row.get(0)?),
        parent_id: row.get::<_, Option<i64>>(1)?.map(BookmarkId),
        title: row.get(2)?,
        url: row.get(3)?,
        position: row.get(4)?,
    })
}

fn sibling_ids<S: Connected + ?Sized>(
    store: &S,
    parent: Option<BookmarkId>,
) -> StorageResult<Vec<i64>> {
    let mut statement = store
        .connection()
        .prepare_cached("SELECT id FROM bookmark WHERE parent_id IS ?1 ORDER BY position, id")?;
    let ids = statement
        .query_map([parent.map(|id| id.0)], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    Ok(ids)
}

fn place<S: Connected + ?Sized>(
    store: &S,
    id: BookmarkId,
    parent: Option<BookmarkId>,
    index: Option<usize>,
) -> StorageResult<()> {
    let mut ids = sibling_ids(store, parent)?;
    insert_at(&mut ids, id.0, index);
    renumber(store.connection(), OrderedTable::Bookmark, &ids)
}
