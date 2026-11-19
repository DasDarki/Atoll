//! The archive: tabs taken out of their workspace without being thrown away.
//!
//! Every archived tab remembers the island it came from and that island the workspace it lived
//! in, each as a copy of what was known about it when the tab left: its id, its name, its colour
//! and its place. The archive is laid out the way the sidebar was. A tab that comes back goes
//! into its island if that still exists, and otherwise into one made alike, which the other
//! tabs of the same island then follow. Workspaces are treated the same way.
//!
//! The place kept is the one a tab had when it left, after those that left before it. Bringing
//! tabs back in the reverse order puts each one where it was, which restores the order exactly,
//! and the archive works its order out the same way.
//!
//! The database itself cuts the link once an island or a workspace is deleted, so an id SQLite
//! hands out again can never draw a tab into a stranger's island.

use atoll_core::{FaviconId, IslandColor, IslandId, Tab, TabId, Timestamp, WorkspaceId};
use rusqlite::{OptionalExtension, params};

use super::islands::{IslandStore, NewIsland};
use super::ordering::{OrderedTable, insert_at, renumber};
use super::rows::{self, TAB_COLUMNS};
use super::tabs::{TabStore, tab_ids};
use super::workspaces::WorkspaceStore;
use crate::connection::Connected;
use crate::error::{StorageError, StorageResult};

/// A workspace as the archive remembers it, with the islands archived from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchivedWorkspace {
    pub id: i64,
    /// The workspace the copy was taken from, `None` once it is gone.
    pub workspace_id: Option<WorkspaceId>,
    pub name: String,
    pub position: i64,
    pub islands: Vec<ArchivedIsland>,
}

/// An island as the archive remembers it, with the tabs archived from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchivedIsland {
    pub id: i64,
    /// The island the copy was taken from, `None` once it is gone.
    pub island_id: Option<IslandId>,
    pub name: String,
    pub color: IslandColor,
    pub position: i64,
    pub tabs: Vec<ArchivedTab>,
}

/// A tab in the archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchivedTab {
    pub id: i64,
    pub url: String,
    pub title: Option<String>,
    pub favicon_id: Option<FaviconId>,
    pub position: i64,
    pub pinned: bool,
    pub last_used_at: Option<Timestamp>,
    pub archived_at: Timestamp,
}

impl ArchivedTab {
    /// What the archive shows for the tab: its title, or its address while it has none.
    pub fn display_title(&self) -> &str {
        self.title
            .as_deref()
            .filter(|title| !title.trim().is_empty())
            .unwrap_or(&self.url)
    }
}

/// Why a tab went to the archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveReason {
    /// Someone archived it: the tab itself, its island or a selection.
    Asked,
    /// It went on its own, not visited for longer than the settings allow.
    Unused,
}

pub trait ArchiveStore: Connected {
    /// The whole archive, laid out like the sidebar: workspaces, their islands and their tabs,
    /// each in the order they had.
    fn archive(&self) -> StorageResult<Vec<ArchivedWorkspace>> {
        let mut workspaces: Vec<ArchivedWorkspace> = self
            .connection()
            .prepare_cached(
                "SELECT id, workspace_id, name, position FROM archived_workspace
                 ORDER BY position, id",
            )?
            .query_map([], |row| {
                Ok(ArchivedWorkspace {
                    id: row.get(0)?,
                    workspace_id: row.get::<_, Option<i64>>(1)?.map(WorkspaceId),
                    name: row.get(2)?,
                    position: row.get(3)?,
                    islands: Vec::new(),
                })
            })?
            .collect::<Result<_, _>>()?;
        let islands: Vec<(i64, ArchivedIsland)> = self
            .connection()
            .prepare_cached(
                "SELECT archived_workspace_id, id, island_id, name, color, position
                 FROM archived_island ORDER BY position, id",
            )?
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    ArchivedIsland {
                        id: row.get(1)?,
                        island_id: row.get::<_, Option<i64>>(2)?.map(IslandId),
                        name: row.get(3)?,
                        color: IslandColor::from_index(row.get(4)?),
                        position: row.get(5)?,
                        tabs: Vec::new(),
                    },
                ))
            })?
            .collect::<Result<_, _>>()?;
        let tabs: Vec<(i64, ArchivedTab)> = self
            .connection()
            .prepare_cached(
                "SELECT archived_island_id, id, url, title, favicon_id, position, pinned,
                        last_used_at, archived_at
                 FROM archived_tab ORDER BY position, id",
            )?
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    ArchivedTab {
                        id: row.get(1)?,
                        url: row.get(2)?,
                        title: row.get(3)?,
                        favicon_id: row.get::<_, Option<i64>>(4)?.map(FaviconId),
                        position: row.get(5)?,
                        pinned: row.get(6)?,
                        last_used_at: row.get::<_, Option<i64>>(7)?.map(Timestamp),
                        archived_at: Timestamp(row.get(8)?),
                    },
                ))
            })?
            .collect::<Result<_, _>>()?;
        for (workspace, mut island) in islands {
            let mut own: Vec<ArchivedTab> = tabs
                .iter()
                .filter(|(owner, _)| *owner == island.id)
                .map(|(_, tab)| tab.clone())
                .collect();
            own.sort_by_key(|tab| tab.id);
            island.tabs = order_before_leaving(own, |tab| tab.position);
            if let Some(entry) = workspaces.iter_mut().find(|entry| entry.id == workspace) {
                entry.islands.push(island);
            }
        }
        for workspace in &mut workspaces {
            let mut islands = std::mem::take(&mut workspace.islands);
            islands.sort_by_key(|island| island.tabs.iter().map(|tab| tab.id).max());
            workspace.islands = order_before_leaving(islands, |island| island.position);
        }
        Ok(workspaces)
    }

    /// Tabs last visited before `before` that may go to the archive on their own: private and
    /// pinned tabs stay where they are. A tab never visited counts from when it was opened.
    fn tabs_unused_since(&self, before: Timestamp) -> StorageResult<Vec<Tab>> {
        let mut statement = self.connection().prepare_cached(&format!(
            "SELECT {TAB_COLUMNS} FROM tab
             WHERE tab.private = 0 AND tab.pinned = 0
               AND COALESCE(tab.last_used_at, tab.created_at) < ?1
             ORDER BY tab.id"
        ))?;
        let tabs = statement
            .query_map([before.0], rows::tab)?
            .collect::<Result<_, _>>()?;
        Ok(tabs)
    }

    /// Moves `tabs` into the archive one after the other, each together with what is known about
    /// its island and workspace and why it goes.
    fn archive_tabs(
        &self,
        tabs: &[TabId],
        at: Timestamp,
        reason: ArchiveReason,
    ) -> StorageResult<()> {
        for tab in tabs {
            let record = self.tab(*tab)?;
            if !record.private {
                archive_one(self, &record, at, reason)?;
            }
        }
        Ok(())
    }

    /// Moves every tab of `island` into the archive and removes the island, which the first of
    /// its tabs to come back brings back again.
    fn archive_island(&self, island: IslandId, at: Timestamp) -> StorageResult<()> {
        let tabs: Vec<TabId> = self
            .tabs_in_island(island)?
            .iter()
            .map(|tab| tab.id)
            .collect();
        self.archive_tabs(&tabs, at, ArchiveReason::Asked)?;
        self.delete_island(island)
    }

    /// Brings an archived tab back where it came from and answers with the tab it is again.
    /// It counts as used at `at`, or it would be on its way back into the archive at once.
    fn restore_archived_tab(&self, id: i64, at: Timestamp) -> StorageResult<TabId> {
        restore_one(self, id, at, LastUse::Now)
    }

    /// Brings back the tabs that went to the archive on their own and were last visited at or
    /// after `since`, all of them when it is `None`, the last to leave first. They keep the
    /// visit they had, so they leave again exactly when that is long enough ago.
    fn restore_unused_tabs_visited_since(
        &self,
        since: Option<Timestamp>,
        at: Timestamp,
    ) -> StorageResult<Vec<TabId>> {
        let tabs: Vec<i64> = self
            .connection()
            .prepare_cached(
                "SELECT id FROM archived_tab
                 WHERE unused = 1
                   AND (?1 IS NULL OR COALESCE(last_used_at, created_at) >= ?1)
                 ORDER BY id DESC",
            )?
            .query_map([since.map(|since| since.0)], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        tabs.into_iter()
            .map(|tab| restore_one(self, tab, at, LastUse::AsBefore))
            .collect()
    }

    /// Brings every tab of an archived island back, the last to leave first.
    fn restore_archived_island(&self, id: i64, at: Timestamp) -> StorageResult<Vec<TabId>> {
        let tabs: Vec<i64> = self
            .connection()
            .prepare_cached(
                "SELECT id FROM archived_tab WHERE archived_island_id = ?1 ORDER BY id DESC",
            )?
            .query_map([id], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        tabs.into_iter()
            .map(|tab| self.restore_archived_tab(tab, at))
            .collect()
    }

    /// Brings everything archived from a workspace back, the island that left last first.
    fn restore_archived_workspace(&self, id: i64, at: Timestamp) -> StorageResult<Vec<TabId>> {
        let islands: Vec<i64> = self
            .connection()
            .prepare_cached(
                "SELECT archived_island.id FROM archived_island
                 JOIN archived_tab ON archived_tab.archived_island_id = archived_island.id
                 WHERE archived_island.archived_workspace_id = ?1
                 GROUP BY archived_island.id
                 ORDER BY MAX(archived_tab.id) DESC",
            )?
            .query_map([id], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        let mut restored = Vec::new();
        for island in islands {
            restored.extend(self.restore_archived_island(island, at)?);
        }
        Ok(restored)
    }

    fn delete_archived_tab(&self, id: i64) -> StorageResult<()> {
        self.connection()
            .prepare_cached("DELETE FROM archived_tab WHERE id = ?1")?
            .execute([id])?;
        forget_empty_copies(self)
    }

    fn delete_archived_island(&self, id: i64) -> StorageResult<()> {
        self.connection()
            .prepare_cached("DELETE FROM archived_island WHERE id = ?1")?
            .execute([id])?;
        forget_empty_copies(self)
    }

    fn delete_archived_workspace(&self, id: i64) -> StorageResult<()> {
        self.connection()
            .prepare_cached("DELETE FROM archived_workspace WHERE id = ?1")?
            .execute([id])?;
        Ok(())
    }

    /// How many tabs the archive holds.
    fn archived_tab_count(&self) -> StorageResult<usize> {
        let count: i64 = self
            .connection()
            .prepare_cached("SELECT COUNT(*) FROM archived_tab")?
            .query_row([], |row| row.get(0))?;
        Ok(count as usize)
    }
}

impl<T: Connected + ?Sized> ArchiveStore for T {}

/// What a tab brought back counts as its last visit.
#[derive(Clone, Copy)]
enum LastUse {
    /// Now: it was asked for.
    Now,
    /// The visit it had before it left.
    AsBefore,
}

/// Brings one archived tab back where it came from, see [`ArchiveStore::restore_archived_tab`].
fn restore_one<S: Connected + ?Sized>(
    store: &S,
    id: i64,
    at: Timestamp,
    last_use: LastUse,
) -> StorageResult<TabId> {
    let row = store
        .connection()
        .prepare_cached(
            "SELECT archived_island_id, url, title, favicon_id, position, pinned, scroll_y,
                    created_at, last_used_at
             FROM archived_tab WHERE id = ?1",
        )?
        .query_row([id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, bool>(5)?,
                row.get::<_, Option<i32>>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, Option<i64>>(8)?,
            ))
        })
        .optional()?
        .ok_or(StorageError::NotFound {
            entity: "archived tab",
            id,
        })?;
    let (archived_island, url, title, favicon, position, pinned, scroll_y, created_at, used) = row;
    let used = match last_use {
        LastUse::Now => Some(at.0),
        LastUse::AsBefore => used,
    };
    let island = island_for(store, archived_island, at)?;
    store
        .connection()
        .prepare_cached(
            "INSERT INTO tab (island_id, url, title, favicon_id, position, pinned, scroll_y,
                              last_used_at, created_at)
             VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6, ?7, ?8)",
        )?
        .execute(params![
            island.0, url, title, favicon, pinned, scroll_y, used, created_at
        ])?;
    let tab = TabId(store.connection().last_insert_rowid());
    let mut ids = tab_ids(store, island)?;
    insert_at(&mut ids, tab.0, usize::try_from(position).ok());
    renumber(store.connection(), OrderedTable::Tab, &ids)?;
    store
        .connection()
        .prepare_cached(
            "INSERT INTO tab_history (tab_id, url, title, visited_at)
             SELECT ?1, url, title, visited_at FROM archived_tab_history
             WHERE archived_tab_id = ?2 ORDER BY id",
        )?
        .execute(params![tab.0, id])?;
    if let LastUse::Now = last_use {
        store.touch_tab(tab, at)?;
    }
    store.delete_archived_tab(id)?;
    Ok(tab)
}

/// Moves one tab into the archive, keeping the place it had in `record`.
fn archive_one<S: Connected + ?Sized>(
    store: &S,
    record: &Tab,
    at: Timestamp,
    reason: ArchiveReason,
) -> StorageResult<()> {
    let island = store.island(record.island_id)?;
    let workspace = store.workspace(island.workspace_id)?;
    let archived_workspace = remember_workspace(store, &workspace)?;
    let archived_island = remember_island(store, archived_workspace, &island)?;
    let created_at: i64 = store
        .connection()
        .prepare_cached("SELECT created_at FROM tab WHERE id = ?1")?
        .query_row([record.id.0], |row| row.get(0))?;
    store
        .connection()
        .prepare_cached(
            "INSERT INTO archived_tab (
                archived_island_id, url, title, favicon_id, position, pinned, scroll_y,
                last_used_at, created_at, archived_at, unused
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        )?
        .execute(params![
            archived_island,
            record.url,
            record.title,
            record.favicon_id.map(|favicon| favicon.0),
            record.position,
            record.pinned,
            record.scroll_y,
            record.last_used_at.map(|used| used.0),
            created_at,
            at.0,
            reason == ArchiveReason::Unused
        ])?;
    let archived_tab = store.connection().last_insert_rowid();
    store
        .connection()
        .prepare_cached(
            "INSERT INTO archived_tab_history (archived_tab_id, url, title, visited_at)
             SELECT ?1, url, title, visited_at FROM tab_history WHERE tab_id = ?2 ORDER BY id",
        )?
        .execute(params![archived_tab, record.id.0])?;
    store.delete_tab(record.id)
}

/// Remembers `workspace` for the archive and answers with the copy. A copy taken earlier is
/// brought up to date, so the archive shows the name the workspace has now.
fn remember_workspace<S: Connected + ?Sized>(
    store: &S,
    workspace: &atoll_core::Workspace,
) -> StorageResult<i64> {
    let known: Option<i64> = store
        .connection()
        .prepare_cached("SELECT id FROM archived_workspace WHERE workspace_id = ?1")?
        .query_row([workspace.id.0], |row| row.get(0))
        .optional()?;
    if let Some(id) = known {
        store
            .connection()
            .prepare_cached("UPDATE archived_workspace SET name = ?1, position = ?2 WHERE id = ?3")?
            .execute(params![workspace.name, workspace.position, id])?;
        return Ok(id);
    }
    store
        .connection()
        .prepare_cached(
            "INSERT INTO archived_workspace (workspace_id, name, position) VALUES (?1, ?2, ?3)",
        )?
        .execute(params![workspace.id.0, workspace.name, workspace.position])?;
    Ok(store.connection().last_insert_rowid())
}

/// Remembers `island` for the archive, under the copy of its workspace, and answers with the
/// copy. An island moved to another workspace since its last tab left takes its copy along.
fn remember_island<S: Connected + ?Sized>(
    store: &S,
    archived_workspace: i64,
    island: &atoll_core::Island,
) -> StorageResult<i64> {
    let known: Option<i64> = store
        .connection()
        .prepare_cached("SELECT id FROM archived_island WHERE island_id = ?1")?
        .query_row([island.id.0], |row| row.get(0))
        .optional()?;
    if let Some(id) = known {
        store
            .connection()
            .prepare_cached(
                "UPDATE archived_island
                 SET archived_workspace_id = ?1, name = ?2, color = ?3, position = ?4
                 WHERE id = ?5",
            )?
            .execute(params![
                archived_workspace,
                island.name,
                island.color.index(),
                island.position,
                id
            ])?;
        forget_empty_copies(store)?;
        return Ok(id);
    }
    store
        .connection()
        .prepare_cached(
            "INSERT INTO archived_island (archived_workspace_id, island_id, name, color, position)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )?
        .execute(params![
            archived_workspace,
            island.id.0,
            island.name,
            island.color.index(),
            island.position
        ])?;
    Ok(store.connection().last_insert_rowid())
}

/// The island an archived tab goes back into: the one it came from while that exists,
/// otherwise one made like it, in a workspace found or made the same way. The copy is linked to
/// what was made, so the other tabs of the island follow.
fn island_for<S: Connected + ?Sized>(
    store: &S,
    archived_island: i64,
    at: Timestamp,
) -> StorageResult<IslandId> {
    let (archived_workspace, island, name, color, position) = store
        .connection()
        .prepare_cached(
            "SELECT archived_workspace_id, island_id, name, color, position
             FROM archived_island WHERE id = ?1",
        )?
        .query_row([archived_island], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })?;
    if let Some(island) = island {
        return Ok(IslandId(island));
    }
    let workspace = workspace_for(store, archived_workspace)?;
    let island = store.create_island(NewIsland {
        workspace_id: workspace,
        name: &name,
        color: IslandColor::from_index(color),
        index: usize::try_from(position).ok(),
        created_at: at,
    })?;
    store
        .connection()
        .prepare_cached("UPDATE archived_island SET island_id = ?1 WHERE id = ?2")?
        .execute(params![island.id.0, archived_island])?;
    Ok(island.id)
}

/// The workspace an archived island goes back into, found or made like [`island_for`] does.
fn workspace_for<S: Connected + ?Sized>(
    store: &S,
    archived_workspace: i64,
) -> StorageResult<WorkspaceId> {
    let (workspace, name, position) = store
        .connection()
        .prepare_cached(
            "SELECT workspace_id, name, position FROM archived_workspace WHERE id = ?1",
        )?
        .query_row([archived_workspace], |row| {
            Ok((
                row.get::<_, Option<i64>>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
    if let Some(workspace) = workspace {
        return Ok(WorkspaceId(workspace));
    }
    let created = store.create_workspace(&name)?;
    store.move_workspace(created.id, usize::try_from(position).unwrap_or(usize::MAX))?;
    store
        .connection()
        .prepare_cached("UPDATE archived_workspace SET workspace_id = ?1 WHERE id = ?2")?
        .execute(params![created.id.0, archived_workspace])?;
    Ok(created.id)
}

/// Copies of islands and workspaces stay only while something archived still belongs to them.
fn forget_empty_copies<S: Connected + ?Sized>(store: &S) -> StorageResult<()> {
    store.connection().execute_batch(
        "DELETE FROM archived_island
         WHERE id NOT IN (SELECT archived_island_id FROM archived_tab);
         DELETE FROM archived_workspace
         WHERE id NOT IN (SELECT archived_workspace_id FROM archived_island);",
    )?;
    Ok(())
}

/// The order things had before they left, from `left` in the order they left and the place each
/// had at that moment: taking them back in reverse, each into its place, undoes the leaving. The
/// places count what stayed behind as well, so they are kept as numbers and never cut short.
pub(crate) fn order_before_leaving<T>(left: Vec<T>, place: impl Fn(&T) -> i64) -> Vec<T> {
    let mut placed: Vec<(i64, T)> = Vec::with_capacity(left.len());
    for item in left.into_iter().rev() {
        let at = place(&item);
        for (other, _) in &mut placed {
            if *other >= at {
                *other += 1;
            }
        }
        placed.push((at, item));
    }
    placed.sort_by_key(|(at, _)| *at);
    placed.into_iter().map(|(_, item)| item).collect()
}

#[cfg(test)]
mod tests {
    use super::order_before_leaving;

    #[test]
    fn the_order_before_leaving_is_worked_out_backwards() {
        let left = vec![("a", 0), ("c", 1), ("b", 0)];
        let order: Vec<&str> = order_before_leaving(left, |item| item.1)
            .into_iter()
            .map(|item| item.0)
            .collect();
        assert_eq!(order, ["a", "b", "c"]);
    }

    #[test]
    fn what_stayed_behind_still_counts() {
        let left = vec![("first", 1), ("second", 1)];
        let order: Vec<&str> = order_before_leaving(left, |item| item.1)
            .into_iter()
            .map(|item| item.0)
            .collect();
        assert_eq!(
            order,
            ["first", "second"],
            "a tab that stayed in front holds place 0"
        );
    }
}
