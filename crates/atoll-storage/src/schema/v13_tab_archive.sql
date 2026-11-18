CREATE TABLE archived_workspace (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    workspace_id INTEGER UNIQUE,
    name         TEXT    NOT NULL,
    position     INTEGER NOT NULL
);

CREATE TABLE archived_island (
    id                    INTEGER PRIMARY KEY AUTOINCREMENT,
    archived_workspace_id INTEGER NOT NULL REFERENCES archived_workspace(id) ON DELETE CASCADE,
    island_id             INTEGER UNIQUE,
    name                  TEXT    NOT NULL,
    color                 INTEGER NOT NULL DEFAULT 0,
    position              INTEGER NOT NULL
);

CREATE INDEX archived_island_by_workspace ON archived_island(archived_workspace_id, position);

CREATE TABLE archived_tab (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    archived_island_id INTEGER NOT NULL REFERENCES archived_island(id) ON DELETE CASCADE,
    url                TEXT    NOT NULL,
    title              TEXT,
    favicon_id         INTEGER REFERENCES favicon(id) ON DELETE SET NULL,
    position           INTEGER NOT NULL,
    pinned             INTEGER NOT NULL DEFAULT 0,
    scroll_y           INTEGER,
    last_used_at       INTEGER,
    created_at         INTEGER NOT NULL,
    archived_at        INTEGER NOT NULL
);

CREATE INDEX archived_tab_by_island ON archived_tab(archived_island_id, position);

CREATE TABLE archived_tab_history (
    id              INTEGER PRIMARY KEY,
    archived_tab_id INTEGER NOT NULL REFERENCES archived_tab(id) ON DELETE CASCADE,
    url             TEXT    NOT NULL,
    title           TEXT,
    visited_at      INTEGER NOT NULL
);

CREATE INDEX archived_tab_history_by_tab ON archived_tab_history(archived_tab_id, id);

CREATE TRIGGER island_gone_from_archive AFTER DELETE ON island
BEGIN
    UPDATE archived_island SET island_id = NULL WHERE island_id = OLD.id;
END;

CREATE TRIGGER workspace_gone_from_archive AFTER DELETE ON workspace
BEGIN
    UPDATE archived_workspace SET workspace_id = NULL WHERE workspace_id = OLD.id;
END;

INSERT INTO archived_workspace (workspace_id, name, position)
SELECT workspace.id, workspace.name, workspace.position
FROM workspace
WHERE workspace.id IN (SELECT workspace_id FROM island WHERE archived_at IS NOT NULL);

INSERT INTO archived_island (archived_workspace_id, island_id, name, color, position)
SELECT archived_workspace.id, island.id, island.name, island.color, island.position
FROM island
JOIN archived_workspace ON archived_workspace.workspace_id = island.workspace_id
WHERE island.archived_at IS NOT NULL;

INSERT INTO archived_tab (
    id, archived_island_id, url, title, favicon_id, position, pinned, scroll_y,
    last_used_at, created_at, archived_at
)
SELECT tab.id, archived_island.id, tab.url, tab.title, tab.favicon_id, tab.position, tab.pinned,
       tab.scroll_y, tab.last_used_at, tab.created_at, island.archived_at
FROM tab
JOIN island ON island.id = tab.island_id
JOIN archived_island ON archived_island.island_id = island.id
WHERE island.archived_at IS NOT NULL AND tab.private = 0;

INSERT INTO archived_tab_history (archived_tab_id, url, title, visited_at)
SELECT tab_id, url, title, visited_at
FROM tab_history
WHERE tab_id IN (SELECT id FROM archived_tab)
ORDER BY id;

DELETE FROM island WHERE archived_at IS NOT NULL;

DELETE FROM archived_island
WHERE id NOT IN (SELECT archived_island_id FROM archived_tab);

DELETE FROM archived_workspace
WHERE id NOT IN (SELECT archived_workspace_id FROM archived_island);

ALTER TABLE island DROP COLUMN archived_at;
