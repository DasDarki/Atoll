CREATE TABLE plugin (
    id TEXT PRIMARY KEY,
    version TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    permissions TEXT NOT NULL DEFAULT '',
    source TEXT NOT NULL DEFAULT 'folder',
    installed_at INTEGER NOT NULL
);

CREATE TABLE plugin_data (
    plugin_id TEXT NOT NULL,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    PRIMARY KEY (plugin_id, key)
) WITHOUT ROWID;
