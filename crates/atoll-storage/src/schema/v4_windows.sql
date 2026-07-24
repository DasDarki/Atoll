CREATE TABLE window (
    id INTEGER PRIMARY KEY,
    workspace_id INTEGER NOT NULL UNIQUE REFERENCES workspace(id) ON DELETE CASCADE,
    x INTEGER,
    y INTEGER,
    width INTEGER,
    height INTEGER,
    maximized INTEGER NOT NULL DEFAULT 0,
    position INTEGER NOT NULL
);
