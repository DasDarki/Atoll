//! The local side of Atoll Sync: turning rows into the records of the protocol and records
//! back into rows.
//!
//! Rows keep their own ids and integer positions. Their sync id names them on every device, and
//! for workspaces, islands and tabs an order key says where they stand (see
//! [`atoll_sync::order`]). Keys are given when something is sent: whatever already stands in the
//! right order keeps its key, so a moved tab gets a new key and its neighbours do not. Records
//! that arrive are written with the triggers silenced, sorted in by their keys, and a record
//! whose parent has not arrived yet waits until it has.

use std::collections::{BTreeSet, HashMap, HashSet};

use atoll_core::{TabId, WorkspaceId};
use atoll_sync::model::{
    ArchivedIsland as WireArchivedIsland, ArchivedTab as WireArchivedTab,
    ArchivedWorkspace as WireArchivedWorkspace, Island as WireIsland, Tab as WireTab,
    Workspace as WireWorkspace,
};
use atoll_sync::{Kind, Record, SyncId, order};
use rusqlite::{OptionalExtension, params};

use super::sync::SyncStore;
use crate::connection::Connected;
use crate::error::StorageResult;

/// A record as it comes from the server: written anew, or removed.
#[derive(Debug, Clone, PartialEq)]
pub struct Incoming {
    pub kind: Kind,
    pub id: SyncId,
    pub record: Option<Record>,
}

/// What taking records over changed, for the windows to follow.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Applied {
    /// Records written or removed.
    pub changed: usize,
    /// Tabs whose address changed, so a loaded page can follow.
    pub moved_tabs: Vec<TabId>,
    /// Workspaces that are gone.
    pub removed_workspaces: Vec<WorkspaceId>,
    /// Records whose parent never arrived, put somewhere instead.
    pub orphans: usize,
}

/// The tables of the kinds, in the order parents come before their children.
const ORDER: [Kind; 6] = [
    Kind::Workspace,
    Kind::Island,
    Kind::Tab,
    Kind::ArchivedWorkspace,
    Kind::ArchivedIsland,
    Kind::ArchivedTab,
];

fn table(kind: Kind) -> &'static str {
    match kind {
        Kind::Workspace => "workspace",
        Kind::Island => "island",
        Kind::Tab => "tab",
        Kind::ArchivedWorkspace => "archived_workspace",
        Kind::ArchivedIsland => "archived_island",
        Kind::ArchivedTab => "archived_tab",
    }
}

fn rank(kind: Kind) -> usize {
    ORDER.iter().position(|known| *known == kind).unwrap_or(0)
}

pub trait ReplicaStore: Connected {
    /// The record `kind`/`id` as it stands here, `None` when it does not exist (or is private).
    /// Call [`ReplicaStore::give_order_keys`] first, so ordered records carry a valid key.
    fn sync_record(&self, kind: Kind, id: SyncId) -> StorageResult<Option<Record>> {
        let local = self.local_id(kind, id)?;
        match local {
            Some(local) => self.record_of(kind, local),
            None => Ok(None),
        }
    }

    /// Every record that exists here, parents first.
    fn sync_records(&self) -> StorageResult<Vec<(Kind, SyncId, Record)>> {
        let mut records = Vec::new();
        for kind in ORDER {
            let filter = if kind == Kind::Tab {
                " WHERE private = 0"
            } else {
                ""
            };
            let rows: Vec<(i64, String)> = self
                .connection()
                .prepare(&format!(
                    "SELECT id, sync_id FROM {}{filter} ORDER BY id",
                    table(kind)
                ))?
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<Result<_, _>>()?;
            for (local, sync) in rows {
                let Ok(id) = sync.parse::<SyncId>() else {
                    continue;
                };
                if let Some(record) = self.record_of(kind, local)? {
                    records.push((kind, id, record));
                }
            }
        }
        Ok(records)
    }

    /// Gives every workspace, island and tab an order key that fits where it stands, changing as
    /// few keys as possible. Answers with the records whose key changed: they have to be sent
    /// too, or the other devices would not learn their new place.
    fn give_order_keys(&self) -> StorageResult<Vec<(Kind, SyncId)>> {
        let mut changed = Vec::new();
        changed.extend(fix_keys(
            self,
            Kind::Workspace,
            "SELECT id, sync_id, sync_order FROM workspace ORDER BY position, id",
            None,
        )?);
        let workspaces: Vec<i64> = ids(self, "SELECT id FROM workspace", None)?;
        for workspace in workspaces {
            changed.extend(fix_keys(
                self,
                Kind::Island,
                "SELECT id, sync_id, sync_order FROM island WHERE workspace_id = ?1 ORDER BY position, id",
                Some(workspace),
            )?);
        }
        let islands: Vec<i64> = ids(self, "SELECT id FROM island", None)?;
        for island in islands {
            changed.extend(fix_keys(
                self,
                Kind::Tab,
                "SELECT id, sync_id, sync_order FROM tab WHERE island_id = ?1 AND private = 0 ORDER BY position, id",
                Some(island),
            )?);
        }
        Ok(changed)
    }

    /// Takes records of the server over. Call it inside a transaction: the triggers are silenced
    /// for the time it runs, and nothing else may see that.
    fn apply_sync(&self, incoming: Vec<Incoming>) -> StorageResult<Applied> {
        self.set_sync_applying(true)?;
        let result = apply(self, incoming);
        self.set_sync_applying(false)?;
        result
    }

    /// Makes this device hold exactly `records`: what is missing comes, what the server does not
    /// have goes. Private tabs stay. Call it inside a transaction, like [`ReplicaStore::apply_sync`].
    fn mirror_sync(&self, records: Vec<(Kind, SyncId, Record)>) -> StorageResult<Applied> {
        let wanted: HashSet<(Kind, SyncId)> =
            records.iter().map(|(kind, id, _)| (*kind, *id)).collect();
        let mut incoming: Vec<Incoming> = Vec::new();
        for kind in ORDER {
            let filter = if kind == Kind::Tab {
                " WHERE private = 0"
            } else {
                ""
            };
            let present: Vec<String> = self
                .connection()
                .prepare(&format!("SELECT sync_id FROM {}{filter}", table(kind)))?
                .query_map([], |row| row.get(0))?
                .collect::<Result<_, _>>()?;
            for sync in present {
                if let Ok(id) = sync.parse::<SyncId>()
                    && !wanted.contains(&(kind, id))
                {
                    incoming.push(Incoming {
                        kind,
                        id,
                        record: None,
                    });
                }
            }
        }
        incoming.extend(records.into_iter().map(|(kind, id, record)| Incoming {
            kind,
            id,
            record: Some(record),
        }));
        self.apply_sync(incoming)
    }

    /// Gives the local workspace the sync id of one the server already has, so the two become one
    /// when this device joins with data of its own.
    fn adopt_workspace(&self, local: WorkspaceId, id: SyncId) -> StorageResult<()> {
        self.set_sync_applying(true)?;
        let result = self
            .connection()
            .execute(
                "UPDATE workspace SET sync_id = ?1 WHERE id = ?2",
                params![id.to_string(), local.0],
            )
            .map(|_| ());
        self.set_sync_applying(false)?;
        Ok(result?)
    }

    /// Forgets every change not sent yet, when this device mirrors the server instead.
    fn clear_sync_outbox(&self) -> StorageResult<()> {
        self.connection().execute("DELETE FROM sync_outbox", [])?;
        Ok(())
    }

    fn local_id(&self, kind: Kind, id: SyncId) -> StorageResult<Option<i64>> {
        Ok(self
            .connection()
            .prepare_cached(&format!(
                "SELECT id FROM {} WHERE sync_id = ?1",
                table(kind)
            ))?
            .query_row([id.to_string()], |row| row.get(0))
            .optional()?)
    }

    fn record_of(&self, kind: Kind, local: i64) -> StorageResult<Option<Record>> {
        let connection = self.connection();
        let sync_of = |table: &str, id: Option<i64>| -> StorageResult<Option<SyncId>> {
            let Some(id) = id else { return Ok(None) };
            let text: Option<String> = connection
                .prepare_cached(&format!("SELECT sync_id FROM {table} WHERE id = ?1"))?
                .query_row([id], |row| row.get(0))
                .optional()?;
            Ok(text.and_then(|text| text.parse().ok()))
        };
        let record = match kind {
            Kind::Workspace => connection
                .query_row(
                    "SELECT name, sync_order FROM workspace WHERE id = ?1",
                    [local],
                    |row| {
                        Ok(WireWorkspace {
                            name: row.get(0)?,
                            order: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                        })
                    },
                )
                .optional()?
                .map(Record::Workspace),
            Kind::Island => {
                let row = connection
                    .query_row(
                        "SELECT workspace_id, name, color, collapsed, sync_order, last_used_at FROM island WHERE id = ?1",
                        [local],
                        |row| {
                            Ok((
                                row.get::<_, i64>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, i64>(2)?,
                                row.get::<_, bool>(3)?,
                                row.get::<_, Option<String>>(4)?,
                                row.get::<_, Option<i64>>(5)?,
                            ))
                        },
                    )
                    .optional()?;
                match row {
                    Some((workspace, name, color, collapsed, key, last_used_at)) => {
                        sync_of("workspace", Some(workspace))?.map(|workspace| {
                            Record::Island(WireIsland {
                                workspace,
                                name,
                                color,
                                collapsed,
                                order: key.unwrap_or_default(),
                                last_used_at,
                            })
                        })
                    }
                    None => None,
                }
            }
            Kind::Tab => {
                let row = connection
                    .query_row(
                        "SELECT island_id, url, title, pinned, sync_order, last_used_at, created_at FROM tab WHERE id = ?1 AND private = 0",
                        [local],
                        |row| {
                            Ok((
                                row.get::<_, i64>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, Option<String>>(2)?,
                                row.get::<_, bool>(3)?,
                                row.get::<_, Option<String>>(4)?,
                                row.get::<_, Option<i64>>(5)?,
                                row.get::<_, i64>(6)?,
                            ))
                        },
                    )
                    .optional()?;
                match row {
                    Some((island, url, title, pinned, key, last_used_at, created_at)) => {
                        sync_of("island", Some(island))?.map(|island| {
                            Record::Tab(WireTab {
                                island,
                                url,
                                title,
                                pinned,
                                order: key.unwrap_or_default(),
                                last_used_at,
                                created_at,
                            })
                        })
                    }
                    None => None,
                }
            }
            Kind::ArchivedWorkspace => {
                let row = connection
                    .query_row(
                        "SELECT workspace_id, name, position FROM archived_workspace WHERE id = ?1",
                        [local],
                        |row| {
                            Ok((
                                row.get::<_, Option<i64>>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, i64>(2)?,
                            ))
                        },
                    )
                    .optional()?;
                match row {
                    Some((workspace, name, position)) => {
                        Some(Record::ArchivedWorkspace(WireArchivedWorkspace {
                            workspace: sync_of("workspace", workspace)?,
                            name,
                            position,
                        }))
                    }
                    None => None,
                }
            }
            Kind::ArchivedIsland => {
                let row = connection
                    .query_row(
                        "SELECT archived_workspace_id, island_id, name, color, position FROM archived_island WHERE id = ?1",
                        [local],
                        |row| {
                            Ok((
                                row.get::<_, i64>(0)?,
                                row.get::<_, Option<i64>>(1)?,
                                row.get::<_, String>(2)?,
                                row.get::<_, i64>(3)?,
                                row.get::<_, i64>(4)?,
                            ))
                        },
                    )
                    .optional()?;
                match row {
                    Some((archived_workspace, island, name, color, position)) => {
                        match sync_of("archived_workspace", Some(archived_workspace))? {
                            Some(archived_workspace) => {
                                Some(Record::ArchivedIsland(WireArchivedIsland {
                                    archived_workspace,
                                    island: sync_of("island", island)?,
                                    name,
                                    color,
                                    position,
                                }))
                            }
                            None => None,
                        }
                    }
                    None => None,
                }
            }
            Kind::ArchivedTab => {
                type Row = (
                    i64,
                    String,
                    Option<String>,
                    bool,
                    i64,
                    Option<i64>,
                    i64,
                    i64,
                    bool,
                );
                let row: Option<Row> = connection
                    .query_row(
                        "SELECT archived_island_id, url, title, pinned, position, last_used_at, created_at, archived_at, unused FROM archived_tab WHERE id = ?1",
                        [local],
                        |row| {
                            Ok((
                                row.get(0)?,
                                row.get(1)?,
                                row.get(2)?,
                                row.get(3)?,
                                row.get(4)?,
                                row.get(5)?,
                                row.get(6)?,
                                row.get(7)?,
                                row.get(8)?,
                            ))
                        },
                    )
                    .optional()?;
                match row {
                    Some((
                        owner,
                        url,
                        title,
                        pinned,
                        position,
                        last_used_at,
                        created_at,
                        archived_at,
                        unused,
                    )) => sync_of("archived_island", Some(owner))?.map(|archived_island| {
                        Record::ArchivedTab(WireArchivedTab {
                            archived_island,
                            url,
                            title,
                            pinned,
                            position,
                            last_used_at,
                            created_at,
                            archived_at,
                            unused,
                        })
                    }),
                    None => None,
                }
            }
        };
        Ok(record)
    }
}

impl<T: Connected + ?Sized> ReplicaStore for T {}

fn ids<S: Connected + ?Sized>(
    store: &S,
    sql: &str,
    parent: Option<i64>,
) -> StorageResult<Vec<i64>> {
    let mut statement = store.connection().prepare(sql)?;
    let rows = match parent {
        Some(parent) => statement
            .query_map([parent], |row| row.get(0))?
            .collect::<Result<_, _>>()?,
        None => statement
            .query_map([], |row| row.get(0))?
            .collect::<Result<_, _>>()?,
    };
    Ok(rows)
}

/// Makes the keys of one container ascend in the order of its rows. The longest run of keys that
/// already ascends stays, the others get keys between their neighbours.
fn fix_keys<S: Connected + ?Sized>(
    store: &S,
    kind: Kind,
    sql: &str,
    parent: Option<i64>,
) -> StorageResult<Vec<(Kind, SyncId)>> {
    let mut statement = store.connection().prepare(sql)?;
    let map = |row: &rusqlite::Row<'_>| -> rusqlite::Result<(i64, String, Option<String>)> {
        Ok((row.get(0)?, row.get(1)?, row.get(2)?))
    };
    let rows: Vec<(i64, String, Option<String>)> = match parent {
        Some(parent) => statement
            .query_map([parent], map)?
            .collect::<Result<_, _>>()?,
        None => statement.query_map([], map)?.collect::<Result<_, _>>()?,
    };
    let keys: Vec<Option<String>> = rows
        .iter()
        .map(|(_, _, key)| key.clone().filter(|key| !key.is_empty()))
        .collect();
    let fixed = order_keys(&keys);
    let mut changed = Vec::new();
    for ((local, sync, before), after) in rows.iter().zip(&fixed) {
        if before.as_deref() != Some(after.as_str()) {
            store.connection().execute(
                &format!("UPDATE {} SET sync_order = ?1 WHERE id = ?2", table(kind)),
                params![after, local],
            )?;
            if let Ok(id) = sync.parse() {
                changed.push((kind, id));
            }
        }
    }
    Ok(changed)
}

/// Keys for items in the order given, keeping as many of the existing ones as can stay.
pub(crate) fn order_keys(keys: &[Option<String>]) -> Vec<String> {
    let keep = longest_ascending(keys);
    let mut result: Vec<String> = Vec::with_capacity(keys.len());
    for index in 0..keys.len() {
        if keep.contains(&index) {
            result.push(keys[index].clone().unwrap_or_default());
            continue;
        }
        let lower = result.last().map(String::as_str);
        let upper = (index + 1..keys.len())
            .find(|later| keep.contains(later))
            .and_then(|later| keys[later].as_deref());
        result.push(order::between(lower, upper));
    }
    result
}

/// The indexes of the longest run of keys that ascend strictly, in order.
fn longest_ascending(keys: &[Option<String>]) -> BTreeSet<usize> {
    let present: Vec<(usize, &str)> = keys
        .iter()
        .enumerate()
        .filter_map(|(index, key)| key.as_deref().map(|key| (index, key)))
        .collect();
    let mut length = vec![1usize; present.len()];
    let mut previous = vec![usize::MAX; present.len()];
    for current in 0..present.len() {
        for earlier in 0..current {
            if present[earlier].1 < present[current].1 && length[earlier] + 1 > length[current] {
                length[current] = length[earlier] + 1;
                previous[current] = earlier;
            }
        }
    }
    let mut kept = BTreeSet::new();
    let Some(mut at) = (0..present.len()).max_by_key(|index| (length[*index], usize::MAX - *index))
    else {
        return kept;
    };
    loop {
        kept.insert(present[at].0);
        if previous[at] == usize::MAX {
            break;
        }
        at = previous[at];
    }
    kept
}

/// Where a container of ordered rows lives, to sort it after records came in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Container {
    Workspaces,
    Islands(i64),
    Tabs(i64),
}

fn apply<S: Connected + ?Sized>(store: &S, incoming: Vec<Incoming>) -> StorageResult<Applied> {
    let mut applied = Applied::default();
    let mut touched: BTreeSet<Container> = BTreeSet::new();
    let (mut removals, mut writes): (Vec<Incoming>, Vec<Incoming>) = incoming
        .into_iter()
        .partition(|change| change.record.is_none());
    removals.sort_by_key(|change| std::cmp::Reverse(rank(change.kind)));
    writes.sort_by_key(|change| rank(change.kind));

    for removal in removals {
        if remove(store, removal.kind, removal.id, &mut applied, &mut touched)? {
            applied.changed += 1;
        }
    }

    let mut waiting = writes;
    loop {
        let before = waiting.len();
        let mut still = Vec::new();
        for write in waiting {
            match write_record(store, &write, &mut applied, &mut touched)? {
                true => applied.changed += 1,
                false => still.push(write),
            }
        }
        waiting = still;
        if waiting.is_empty() || waiting.len() == before {
            break;
        }
    }
    for orphan in waiting {
        if place_orphan(store, &orphan, &mut applied, &mut touched)? {
            applied.changed += 1;
            applied.orphans += 1;
        }
    }

    for container in touched {
        sort_container(store, container)?;
    }
    Ok(applied)
}

/// Removes one record. Private tabs in an island that goes move to another island first.
fn remove<S: Connected + ?Sized>(
    store: &S,
    kind: Kind,
    id: SyncId,
    applied: &mut Applied,
    touched: &mut BTreeSet<Container>,
) -> StorageResult<bool> {
    let Some(local) = store.local_id(kind, id)? else {
        return Ok(false);
    };
    let connection = store.connection();
    match kind {
        Kind::Workspace => {
            let islands = ids(
                store,
                "SELECT id FROM island WHERE workspace_id = ?1",
                Some(local),
            )?;
            rescue_private_tabs(store, &islands)?;
            applied.removed_workspaces.push(WorkspaceId(local));
            touched.insert(Container::Workspaces);
        }
        Kind::Island => {
            rescue_private_tabs(store, &[local])?;
            let workspace: i64 = connection.query_row(
                "SELECT workspace_id FROM island WHERE id = ?1",
                [local],
                |row| row.get(0),
            )?;
            touched.insert(Container::Islands(workspace));
        }
        Kind::Tab => {
            let island: i64 = connection.query_row(
                "SELECT island_id FROM tab WHERE id = ?1",
                [local],
                |row| row.get(0),
            )?;
            touched.insert(Container::Tabs(island));
        }
        _ => {}
    }
    connection.execute(
        &format!("DELETE FROM {} WHERE id = ?1", table(kind)),
        [local],
    )?;
    Ok(true)
}

/// Moves the private tabs of islands that are about to go into an island that stays, made if
/// there is none, because private tabs never leave the device and must not vanish with it.
fn rescue_private_tabs<S: Connected + ?Sized>(store: &S, islands: &[i64]) -> StorageResult<()> {
    let connection = store.connection();
    let mut private: Vec<i64> = Vec::new();
    for island in islands {
        private.extend(ids(
            store,
            "SELECT id FROM tab WHERE island_id = ?1 AND private = 1",
            Some(*island),
        )?);
    }
    if private.is_empty() {
        return Ok(());
    }
    let doomed: HashSet<i64> = islands.iter().copied().collect();
    let target = ids(store, "SELECT id FROM island ORDER BY id", None)?
        .into_iter()
        .find(|island| !doomed.contains(island));
    let target = match target {
        Some(target) => target,
        None => {
            let doomed_workspaces: HashSet<i64> = islands
                .iter()
                .filter_map(|island| {
                    connection
                        .query_row(
                            "SELECT workspace_id FROM island WHERE id = ?1",
                            [island],
                            |row| row.get(0),
                        )
                        .ok()
                })
                .collect();
            let workspace = ids(
                store,
                "SELECT id FROM workspace ORDER BY position, id",
                None,
            )?
            .into_iter()
            .find(|workspace| !doomed_workspaces.contains(workspace))
            .or_else(|| doomed_workspaces.iter().copied().next());
            let Some(workspace) = workspace else {
                return Ok(());
            };
            connection.execute(
                "INSERT INTO island (workspace_id, name, position) VALUES (?1, 'Privat', (SELECT COALESCE(MAX(position) + 1, 0) FROM island WHERE workspace_id = ?1))",
                [workspace],
            )?;
            connection.last_insert_rowid()
        }
    };
    for tab in private {
        connection.execute(
            "UPDATE tab SET island_id = ?1, position = (SELECT COALESCE(MAX(position) + 1, 0) FROM tab WHERE island_id = ?1) WHERE id = ?2",
            params![target, tab],
        )?;
    }
    Ok(())
}

/// Writes one record. Answers `false` when its parent is not here yet.
fn write_record<S: Connected + ?Sized>(
    store: &S,
    change: &Incoming,
    applied: &mut Applied,
    touched: &mut BTreeSet<Container>,
) -> StorageResult<bool> {
    let Some(record) = &change.record else {
        return Ok(false);
    };
    let connection = store.connection();
    let id = change.id.to_string();
    let existing = store.local_id(change.kind, change.id)?;
    let parent = |kind: Kind, id: Option<SyncId>| -> StorageResult<Option<i64>> {
        match id {
            Some(id) => store.local_id(kind, id),
            None => Ok(None),
        }
    };
    match record {
        Record::Workspace(workspace) => {
            match existing {
                Some(local) => {
                    connection.execute(
                        "UPDATE workspace SET name = ?1, sync_order = ?2 WHERE id = ?3",
                        params![workspace.name, workspace.order, local],
                    )?;
                }
                None => {
                    connection.execute(
                        "INSERT INTO workspace (name, position, sync_id, sync_order)
                         VALUES (?1, (SELECT COALESCE(MAX(position) + 1, 0) FROM workspace), ?2, ?3)",
                        params![workspace.name, id, workspace.order],
                    )?;
                }
            }
            touched.insert(Container::Workspaces);
        }
        Record::Island(island) => {
            let Some(workspace) = parent(Kind::Workspace, Some(island.workspace))? else {
                return Ok(false);
            };
            match existing {
                Some(local) => {
                    let before: i64 = connection.query_row(
                        "SELECT workspace_id FROM island WHERE id = ?1",
                        [local],
                        |row| row.get(0),
                    )?;
                    touched.insert(Container::Islands(before));
                    connection.execute(
                        "UPDATE island SET workspace_id = ?1, name = ?2, color = ?3, collapsed = ?4, sync_order = ?5, last_used_at = ?6 WHERE id = ?7",
                        params![workspace, island.name, island.color, island.collapsed, island.order, island.last_used_at, local],
                    )?;
                }
                None => {
                    connection.execute(
                        "INSERT INTO island (workspace_id, name, color, collapsed, sync_order, last_used_at, position, sync_id)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, (SELECT COALESCE(MAX(position) + 1, 0) FROM island WHERE workspace_id = ?1), ?7)",
                        params![workspace, island.name, island.color, island.collapsed, island.order, island.last_used_at, id],
                    )?;
                }
            }
            touched.insert(Container::Islands(workspace));
        }
        Record::Tab(tab) => {
            let Some(island) = parent(Kind::Island, Some(tab.island))? else {
                return Ok(false);
            };
            match existing {
                Some(local) => {
                    let (before, url): (i64, String) = connection.query_row(
                        "SELECT island_id, url FROM tab WHERE id = ?1",
                        [local],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )?;
                    touched.insert(Container::Tabs(before));
                    if url != tab.url {
                        applied.moved_tabs.push(TabId(local));
                    }
                    connection.execute(
                        "UPDATE tab SET island_id = ?1, url = ?2, title = ?3, pinned = ?4, sync_order = ?5, last_used_at = ?6, created_at = ?7 WHERE id = ?8",
                        params![island, tab.url, tab.title, tab.pinned, tab.order, tab.last_used_at, tab.created_at, local],
                    )?;
                }
                None => {
                    connection.execute(
                        "INSERT INTO tab (island_id, url, title, pinned, sync_order, last_used_at, created_at, position, sync_id)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, (SELECT COALESCE(MAX(position) + 1, 0) FROM tab WHERE island_id = ?1), ?8)",
                        params![island, tab.url, tab.title, tab.pinned, tab.order, tab.last_used_at, tab.created_at, id],
                    )?;
                }
            }
            touched.insert(Container::Tabs(island));
        }
        Record::ArchivedWorkspace(archived) => {
            let workspace = parent(Kind::Workspace, archived.workspace)?;
            let taken = workspace.is_some_and(|workspace| {
                connection
                    .query_row(
                        "SELECT 1 FROM archived_workspace WHERE workspace_id = ?1 AND (sync_id IS NOT ?2)",
                        params![workspace, id],
                        |_| Ok(()),
                    )
                    .optional()
                    .ok()
                    .flatten()
                    .is_some()
            });
            let workspace = workspace.filter(|_| !taken);
            match existing {
                Some(local) => connection.execute(
                    "UPDATE archived_workspace SET workspace_id = ?1, name = ?2, position = ?3 WHERE id = ?4",
                    params![workspace, archived.name, archived.position, local],
                )?,
                None => connection.execute(
                    "INSERT INTO archived_workspace (workspace_id, name, position, sync_id) VALUES (?1, ?2, ?3, ?4)",
                    params![workspace, archived.name, archived.position, id],
                )?,
            };
        }
        Record::ArchivedIsland(archived) => {
            let Some(owner) = parent(Kind::ArchivedWorkspace, Some(archived.archived_workspace))?
            else {
                return Ok(false);
            };
            let island = parent(Kind::Island, archived.island)?;
            let taken = island.is_some_and(|island| {
                connection
                    .query_row(
                        "SELECT 1 FROM archived_island WHERE island_id = ?1 AND (sync_id IS NOT ?2)",
                        params![island, id],
                        |_| Ok(()),
                    )
                    .optional()
                    .ok()
                    .flatten()
                    .is_some()
            });
            let island = island.filter(|_| !taken);
            match existing {
                Some(local) => connection.execute(
                    "UPDATE archived_island SET archived_workspace_id = ?1, island_id = ?2, name = ?3, color = ?4, position = ?5 WHERE id = ?6",
                    params![owner, island, archived.name, archived.color, archived.position, local],
                )?,
                None => connection.execute(
                    "INSERT INTO archived_island (archived_workspace_id, island_id, name, color, position, sync_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![owner, island, archived.name, archived.color, archived.position, id],
                )?,
            };
        }
        Record::ArchivedTab(archived) => {
            let Some(owner) = parent(Kind::ArchivedIsland, Some(archived.archived_island))? else {
                return Ok(false);
            };
            match existing {
                Some(local) => connection.execute(
                    "UPDATE archived_tab SET archived_island_id = ?1, url = ?2, title = ?3, pinned = ?4, position = ?5,
                         last_used_at = ?6, created_at = ?7, archived_at = ?8, unused = ?9 WHERE id = ?10",
                    params![owner, archived.url, archived.title, archived.pinned, archived.position,
                        archived.last_used_at, archived.created_at, archived.archived_at, archived.unused, local],
                )?,
                None => connection.execute(
                    "INSERT INTO archived_tab (archived_island_id, url, title, pinned, position, last_used_at, created_at, archived_at, unused, sync_id)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                    params![owner, archived.url, archived.title, archived.pinned, archived.position,
                        archived.last_used_at, archived.created_at, archived.archived_at, archived.unused, id],
                )?,
            };
        }
    }
    Ok(true)
}

/// Puts a record whose parent never came somewhere it can live: an island into the first
/// workspace, a tab into an island named after the workspace's first one. Archive records
/// without their copy are dropped, they would not show anywhere.
fn place_orphan<S: Connected + ?Sized>(
    store: &S,
    change: &Incoming,
    applied: &mut Applied,
    touched: &mut BTreeSet<Container>,
) -> StorageResult<bool> {
    let connection = store.connection();
    let Some(record) = &change.record else {
        return Ok(false);
    };
    let first_workspace = || -> StorageResult<i64> {
        if let Some(workspace) = ids(
            store,
            "SELECT id FROM workspace ORDER BY position, id LIMIT 1",
            None,
        )?
        .first()
        {
            return Ok(*workspace);
        }
        connection.execute(
            "INSERT INTO workspace (name, position) VALUES ('Allgemein', 0)",
            [],
        )?;
        Ok(connection.last_insert_rowid())
    };
    match record {
        Record::Island(island) => {
            let workspace = first_workspace()?;
            let sync: Option<String> = connection.query_row(
                "SELECT sync_id FROM workspace WHERE id = ?1",
                [workspace],
                |row| row.get(0),
            )?;
            let Some(sync) = sync.and_then(|sync| sync.parse().ok()) else {
                return Ok(false);
            };
            let mut moved = island.clone();
            moved.workspace = sync;
            write_record(
                store,
                &Incoming {
                    kind: change.kind,
                    id: change.id,
                    record: Some(Record::Island(moved)),
                },
                applied,
                touched,
            )
        }
        Record::Tab(tab) => {
            let workspace = first_workspace()?;
            let island = match ids(
                store,
                "SELECT id FROM island WHERE workspace_id = ?1 ORDER BY position, id LIMIT 1",
                Some(workspace),
            )?
            .first()
            {
                Some(island) => *island,
                None => {
                    connection.execute(
                        "INSERT INTO island (workspace_id, name, position) VALUES (?1, 'Tabs', 0)",
                        [workspace],
                    )?;
                    connection.last_insert_rowid()
                }
            };
            let sync: Option<String> = connection.query_row(
                "SELECT sync_id FROM island WHERE id = ?1",
                [island],
                |row| row.get(0),
            )?;
            let Some(sync) = sync.and_then(|sync| sync.parse().ok()) else {
                return Ok(false);
            };
            let mut moved = tab.clone();
            moved.island = sync;
            write_record(
                store,
                &Incoming {
                    kind: change.kind,
                    id: change.id,
                    record: Some(Record::Tab(moved)),
                },
                applied,
                touched,
            )
        }
        _ => Ok(false),
    }
}

/// Sorts a container by the order keys and writes the positions anew. Rows without a key, made
/// here and not sent yet, keep the place they had among the others.
fn sort_container<S: Connected + ?Sized>(store: &S, container: Container) -> StorageResult<()> {
    let (select, update, parent) = match container {
        Container::Workspaces => (
            "SELECT id, sync_order FROM workspace ORDER BY position, id",
            "UPDATE workspace SET position = ?1 WHERE id = ?2",
            None,
        ),
        Container::Islands(workspace) => (
            "SELECT id, sync_order FROM island WHERE workspace_id = ?1 ORDER BY position, id",
            "UPDATE island SET position = ?1 WHERE id = ?2",
            Some(workspace),
        ),
        Container::Tabs(island) => (
            "SELECT id, sync_order FROM tab WHERE island_id = ?1 ORDER BY position, id",
            "UPDATE tab SET position = ?1 WHERE id = ?2",
            Some(island),
        ),
    };
    let mut statement = store.connection().prepare(select)?;
    let map = |row: &rusqlite::Row<'_>| -> rusqlite::Result<(i64, Option<String>)> {
        Ok((row.get(0)?, row.get(1)?))
    };
    let rows: Vec<(i64, Option<String>)> = match parent {
        Some(parent) => statement
            .query_map([parent], map)?
            .collect::<Result<_, _>>()?,
        None => statement.query_map([], map)?.collect::<Result<_, _>>()?,
    };
    let order = sorted_by_keys(&rows);
    let mut update = store.connection().prepare(update)?;
    for (position, id) in order.iter().enumerate() {
        update.execute(params![position as i64, id])?;
    }
    Ok(())
}

/// The ids in the order of their keys, rows without a key staying where they stood.
fn sorted_by_keys(rows: &[(i64, Option<String>)]) -> Vec<i64> {
    let mut keyed: Vec<(&str, i64)> = rows
        .iter()
        .filter_map(|(id, key)| {
            key.as_deref()
                .filter(|key| !key.is_empty())
                .map(|key| (key, *id))
        })
        .collect();
    keyed.sort();
    let mut keyed = keyed.into_iter().map(|(_, id)| id);
    let unkeyed: HashMap<usize, i64> = rows
        .iter()
        .enumerate()
        .filter(|(_, (_, key))| key.as_deref().is_none_or(str::is_empty))
        .map(|(index, (id, _))| (index, *id))
        .collect();
    (0..rows.len())
        .filter_map(|index| unkeyed.get(&index).copied().or_else(|| keyed.next()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(values: &[Option<&str>]) -> Vec<Option<String>> {
        values
            .iter()
            .map(|value| value.map(str::to_owned))
            .collect()
    }

    #[test]
    fn keys_in_order_stay_as_they_are() {
        let given = keys(&[Some("A"), Some("V"), Some("k")]);
        assert_eq!(order_keys(&given), ["A", "V", "k"]);
    }

    #[test]
    fn a_moved_item_gets_a_key_and_its_neighbours_keep_theirs() {
        let moved_to_front = keys(&[Some("k"), Some("A"), Some("V")]);
        let fixed = order_keys(&moved_to_front);
        assert_eq!(&fixed[1..], ["A", "V"]);
        assert!(fixed[0].as_str() < "A");
    }

    #[test]
    fn items_without_keys_get_keys_between_their_neighbours() {
        let fixed = order_keys(&keys(&[None, Some("V"), None, None, Some("k"), None]));
        assert!(fixed.windows(2).all(|pair| pair[0] < pair[1]), "{fixed:?}");
        assert_eq!(fixed[1], "V");
        assert_eq!(fixed[4], "k");
        assert_eq!(order_keys(&keys(&[None, None])).len(), 2);
    }

    #[test]
    fn rows_are_sorted_by_key_and_new_ones_keep_their_place() {
        let rows = vec![
            (1, Some("k".to_owned())),
            (2, None),
            (3, Some("A".to_owned())),
        ];
        assert_eq!(sorted_by_keys(&rows), [3, 2, 1]);
    }
}
