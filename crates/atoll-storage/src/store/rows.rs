use atoll_core::{
    FaviconId, Island, IslandColor, IslandId, Tab, TabId, Timestamp, Workspace, WorkspaceId,
};
use rusqlite::Row;

pub(crate) const WORKSPACE_COLUMNS: &str = "id, name, position";

pub(crate) const ISLAND_COLUMNS: &str = "island.id, island.workspace_id, island.name, island.position, island.collapsed, island.color, island.archived_at, island.last_used_at";

pub(crate) const TAB_COLUMNS: &str = "tab.id, tab.island_id, tab.url, tab.title, tab.favicon_id, tab.position, tab.pinned, tab.scroll_y, tab.last_used_at";

pub(crate) fn workspace(row: &Row<'_>) -> rusqlite::Result<Workspace> {
    Ok(Workspace {
        id: WorkspaceId(row.get(0)?),
        name: row.get(1)?,
        position: row.get(2)?,
    })
}

pub(crate) fn island(row: &Row<'_>) -> rusqlite::Result<Island> {
    Ok(Island {
        id: IslandId(row.get(0)?),
        workspace_id: WorkspaceId(row.get(1)?),
        name: row.get(2)?,
        position: row.get(3)?,
        collapsed: row.get(4)?,
        color: IslandColor::from_index(row.get(5)?),
        archived_at: row.get::<_, Option<i64>>(6)?.map(Timestamp),
        last_used_at: row.get::<_, Option<i64>>(7)?.map(Timestamp),
    })
}

pub(crate) fn tab(row: &Row<'_>) -> rusqlite::Result<Tab> {
    Ok(Tab {
        id: TabId(row.get(0)?),
        island_id: IslandId(row.get(1)?),
        url: row.get(2)?,
        title: row.get(3)?,
        favicon_id: row.get::<_, Option<i64>>(4)?.map(FaviconId),
        position: row.get(5)?,
        pinned: row.get(6)?,
        scroll_y: row.get(7)?,
        last_used_at: row.get::<_, Option<i64>>(8)?.map(Timestamp),
    })
}
