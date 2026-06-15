CREATE TABLE workspace (
    id       INTEGER PRIMARY KEY,
    name     TEXT    NOT NULL,
    position INTEGER NOT NULL
);

CREATE TABLE island (
    id           INTEGER PRIMARY KEY,
    workspace_id INTEGER NOT NULL REFERENCES workspace(id) ON DELETE CASCADE,
    name         TEXT    NOT NULL,
    position     INTEGER NOT NULL,
    collapsed    INTEGER NOT NULL DEFAULT 0,
    color        INTEGER NOT NULL DEFAULT 0,
    archived_at  INTEGER,
    last_used_at INTEGER
);

CREATE INDEX island_by_workspace ON island(workspace_id, position);

CREATE TABLE favicon (
    id         INTEGER PRIMARY KEY,
    host       TEXT    NOT NULL UNIQUE,
    data       BLOB    NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE tab (
    id           INTEGER PRIMARY KEY,
    island_id    INTEGER NOT NULL REFERENCES island(id) ON DELETE CASCADE,
    url          TEXT    NOT NULL,
    title        TEXT,
    favicon_id   INTEGER REFERENCES favicon(id) ON DELETE SET NULL,
    position     INTEGER NOT NULL,
    pinned       INTEGER NOT NULL DEFAULT 0,
    scroll_y     INTEGER,
    last_used_at INTEGER,
    created_at   INTEGER NOT NULL
);

CREATE INDEX tab_by_island ON tab(island_id, position);

CREATE TABLE tab_history (
    id         INTEGER PRIMARY KEY,
    tab_id     INTEGER NOT NULL REFERENCES tab(id) ON DELETE CASCADE,
    url        TEXT    NOT NULL,
    title      TEXT,
    visited_at INTEGER NOT NULL
);

CREATE INDEX tab_history_by_tab ON tab_history(tab_id, id);

CREATE TABLE setting (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
