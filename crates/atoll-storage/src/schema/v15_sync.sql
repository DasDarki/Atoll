ALTER TABLE workspace ADD COLUMN sync_id TEXT;
ALTER TABLE workspace ADD COLUMN sync_order TEXT;
ALTER TABLE island ADD COLUMN sync_id TEXT;
ALTER TABLE island ADD COLUMN sync_order TEXT;
ALTER TABLE tab ADD COLUMN sync_id TEXT;
ALTER TABLE tab ADD COLUMN sync_order TEXT;
ALTER TABLE archived_workspace ADD COLUMN sync_id TEXT;
ALTER TABLE archived_island ADD COLUMN sync_id TEXT;
ALTER TABLE archived_tab ADD COLUMN sync_id TEXT;

UPDATE workspace SET sync_id = lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' || substr(lower(hex(randomblob(2))), 2) || '-' || substr('89ab', 1 + abs(random() % 4), 1) || substr(lower(hex(randomblob(2))), 2) || '-' || lower(hex(randomblob(6)));
CREATE UNIQUE INDEX workspace_by_sync_id ON workspace(sync_id);
UPDATE island SET sync_id = lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' || substr(lower(hex(randomblob(2))), 2) || '-' || substr('89ab', 1 + abs(random() % 4), 1) || substr(lower(hex(randomblob(2))), 2) || '-' || lower(hex(randomblob(6)));
CREATE UNIQUE INDEX island_by_sync_id ON island(sync_id);
UPDATE tab SET sync_id = lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' || substr(lower(hex(randomblob(2))), 2) || '-' || substr('89ab', 1 + abs(random() % 4), 1) || substr(lower(hex(randomblob(2))), 2) || '-' || lower(hex(randomblob(6)));
CREATE UNIQUE INDEX tab_by_sync_id ON tab(sync_id);
UPDATE archived_workspace SET sync_id = lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' || substr(lower(hex(randomblob(2))), 2) || '-' || substr('89ab', 1 + abs(random() % 4), 1) || substr(lower(hex(randomblob(2))), 2) || '-' || lower(hex(randomblob(6)));
CREATE UNIQUE INDEX archived_workspace_by_sync_id ON archived_workspace(sync_id);
UPDATE archived_island SET sync_id = lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' || substr(lower(hex(randomblob(2))), 2) || '-' || substr('89ab', 1 + abs(random() % 4), 1) || substr(lower(hex(randomblob(2))), 2) || '-' || lower(hex(randomblob(6)));
CREATE UNIQUE INDEX archived_island_by_sync_id ON archived_island(sync_id);
UPDATE archived_tab SET sync_id = lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' || substr(lower(hex(randomblob(2))), 2) || '-' || substr('89ab', 1 + abs(random() % 4), 1) || substr(lower(hex(randomblob(2))), 2) || '-' || lower(hex(randomblob(6)));
CREATE UNIQUE INDEX archived_tab_by_sync_id ON archived_tab(sync_id);

CREATE TABLE sync_control (
    id       INTEGER PRIMARY KEY CHECK (id = 1),
    applying INTEGER NOT NULL DEFAULT 0
);

INSERT INTO sync_control (id) VALUES (1);

CREATE TABLE sync_outbox (
    kind       TEXT    NOT NULL,
    sync_id    TEXT    NOT NULL,
    removed    INTEGER NOT NULL,
    changed_at INTEGER NOT NULL,
    PRIMARY KEY (kind, sync_id)
);

CREATE TRIGGER workspace_sync_insert AFTER INSERT ON workspace
BEGIN
    UPDATE workspace SET sync_id = lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' || substr(lower(hex(randomblob(2))), 2) || '-' || substr('89ab', 1 + abs(random() % 4), 1) || substr(lower(hex(randomblob(2))), 2) || '-' || lower(hex(randomblob(6))) WHERE id = NEW.id AND sync_id IS NULL;
    INSERT OR REPLACE INTO sync_outbox (kind, sync_id, removed, changed_at)
    SELECT 'workspace', sync_id, 0, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER) FROM workspace
    WHERE id = NEW.id AND (SELECT applying FROM sync_control) = 0;
END;

CREATE TRIGGER workspace_sync_update AFTER UPDATE OF name, position ON workspace
WHEN NEW.sync_id IS NOT NULL AND (OLD.name IS NOT NEW.name OR OLD.position IS NOT NEW.position)
BEGIN
    INSERT OR REPLACE INTO sync_outbox (kind, sync_id, removed, changed_at)
    SELECT 'workspace', NEW.sync_id, 0, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
    WHERE (SELECT applying FROM sync_control) = 0;
END;

CREATE TRIGGER workspace_sync_delete AFTER DELETE ON workspace
WHEN OLD.sync_id IS NOT NULL
BEGIN
    INSERT OR REPLACE INTO sync_outbox (kind, sync_id, removed, changed_at)
    SELECT 'workspace', OLD.sync_id, 1, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
    WHERE (SELECT applying FROM sync_control) = 0;
END;

CREATE TRIGGER island_sync_insert AFTER INSERT ON island
BEGIN
    UPDATE island SET sync_id = lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' || substr(lower(hex(randomblob(2))), 2) || '-' || substr('89ab', 1 + abs(random() % 4), 1) || substr(lower(hex(randomblob(2))), 2) || '-' || lower(hex(randomblob(6))) WHERE id = NEW.id AND sync_id IS NULL;
    INSERT OR REPLACE INTO sync_outbox (kind, sync_id, removed, changed_at)
    SELECT 'island', sync_id, 0, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER) FROM island
    WHERE id = NEW.id AND (SELECT applying FROM sync_control) = 0;
END;

CREATE TRIGGER island_sync_update AFTER UPDATE OF workspace_id, name, position, collapsed, color, last_used_at ON island
WHEN NEW.sync_id IS NOT NULL AND (OLD.workspace_id IS NOT NEW.workspace_id OR OLD.name IS NOT NEW.name OR OLD.position IS NOT NEW.position OR OLD.collapsed IS NOT NEW.collapsed OR OLD.color IS NOT NEW.color OR OLD.last_used_at IS NOT NEW.last_used_at)
BEGIN
    INSERT OR REPLACE INTO sync_outbox (kind, sync_id, removed, changed_at)
    SELECT 'island', NEW.sync_id, 0, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
    WHERE (SELECT applying FROM sync_control) = 0;
END;

CREATE TRIGGER island_sync_delete AFTER DELETE ON island
WHEN OLD.sync_id IS NOT NULL
BEGIN
    INSERT OR REPLACE INTO sync_outbox (kind, sync_id, removed, changed_at)
    SELECT 'island', OLD.sync_id, 1, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
    WHERE (SELECT applying FROM sync_control) = 0;
END;

CREATE TRIGGER tab_sync_insert AFTER INSERT ON tab
BEGIN
    UPDATE tab SET sync_id = lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' || substr(lower(hex(randomblob(2))), 2) || '-' || substr('89ab', 1 + abs(random() % 4), 1) || substr(lower(hex(randomblob(2))), 2) || '-' || lower(hex(randomblob(6))) WHERE id = NEW.id AND sync_id IS NULL;
    INSERT OR REPLACE INTO sync_outbox (kind, sync_id, removed, changed_at)
    SELECT 'tab', sync_id, 0, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER) FROM tab
    WHERE id = NEW.id AND private = 0 AND (SELECT applying FROM sync_control) = 0;
END;

CREATE TRIGGER tab_sync_update AFTER UPDATE OF island_id, url, title, position, pinned, last_used_at, private ON tab
WHEN NEW.sync_id IS NOT NULL AND (OLD.island_id IS NOT NEW.island_id OR OLD.url IS NOT NEW.url OR OLD.title IS NOT NEW.title OR OLD.position IS NOT NEW.position OR OLD.pinned IS NOT NEW.pinned OR OLD.last_used_at IS NOT NEW.last_used_at OR OLD.private IS NOT NEW.private)
BEGIN
    INSERT OR REPLACE INTO sync_outbox (kind, sync_id, removed, changed_at)
    SELECT 'tab', NEW.sync_id, CASE WHEN NEW.private = 0 THEN 0 ELSE 1 END, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
    WHERE (SELECT applying FROM sync_control) = 0 AND (NEW.private = 0 OR OLD.private = 0);
END;

CREATE TRIGGER tab_sync_delete AFTER DELETE ON tab
WHEN OLD.sync_id IS NOT NULL AND OLD.private = 0
BEGIN
    INSERT OR REPLACE INTO sync_outbox (kind, sync_id, removed, changed_at)
    SELECT 'tab', OLD.sync_id, 1, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
    WHERE (SELECT applying FROM sync_control) = 0;
END;

CREATE TRIGGER archived_workspace_sync_insert AFTER INSERT ON archived_workspace
BEGIN
    UPDATE archived_workspace SET sync_id = lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' || substr(lower(hex(randomblob(2))), 2) || '-' || substr('89ab', 1 + abs(random() % 4), 1) || substr(lower(hex(randomblob(2))), 2) || '-' || lower(hex(randomblob(6))) WHERE id = NEW.id AND sync_id IS NULL;
    INSERT OR REPLACE INTO sync_outbox (kind, sync_id, removed, changed_at)
    SELECT 'archived_workspace', sync_id, 0, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER) FROM archived_workspace
    WHERE id = NEW.id AND (SELECT applying FROM sync_control) = 0;
END;

CREATE TRIGGER archived_workspace_sync_update AFTER UPDATE OF workspace_id, name, position ON archived_workspace
WHEN NEW.sync_id IS NOT NULL AND (OLD.workspace_id IS NOT NEW.workspace_id OR OLD.name IS NOT NEW.name OR OLD.position IS NOT NEW.position)
BEGIN
    INSERT OR REPLACE INTO sync_outbox (kind, sync_id, removed, changed_at)
    SELECT 'archived_workspace', NEW.sync_id, 0, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
    WHERE (SELECT applying FROM sync_control) = 0;
END;

CREATE TRIGGER archived_workspace_sync_delete AFTER DELETE ON archived_workspace
WHEN OLD.sync_id IS NOT NULL
BEGIN
    INSERT OR REPLACE INTO sync_outbox (kind, sync_id, removed, changed_at)
    SELECT 'archived_workspace', OLD.sync_id, 1, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
    WHERE (SELECT applying FROM sync_control) = 0;
END;

CREATE TRIGGER archived_island_sync_insert AFTER INSERT ON archived_island
BEGIN
    UPDATE archived_island SET sync_id = lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' || substr(lower(hex(randomblob(2))), 2) || '-' || substr('89ab', 1 + abs(random() % 4), 1) || substr(lower(hex(randomblob(2))), 2) || '-' || lower(hex(randomblob(6))) WHERE id = NEW.id AND sync_id IS NULL;
    INSERT OR REPLACE INTO sync_outbox (kind, sync_id, removed, changed_at)
    SELECT 'archived_island', sync_id, 0, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER) FROM archived_island
    WHERE id = NEW.id AND (SELECT applying FROM sync_control) = 0;
END;

CREATE TRIGGER archived_island_sync_update AFTER UPDATE OF archived_workspace_id, island_id, name, color, position ON archived_island
WHEN NEW.sync_id IS NOT NULL AND (OLD.archived_workspace_id IS NOT NEW.archived_workspace_id OR OLD.island_id IS NOT NEW.island_id OR OLD.name IS NOT NEW.name OR OLD.color IS NOT NEW.color OR OLD.position IS NOT NEW.position)
BEGIN
    INSERT OR REPLACE INTO sync_outbox (kind, sync_id, removed, changed_at)
    SELECT 'archived_island', NEW.sync_id, 0, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
    WHERE (SELECT applying FROM sync_control) = 0;
END;

CREATE TRIGGER archived_island_sync_delete AFTER DELETE ON archived_island
WHEN OLD.sync_id IS NOT NULL
BEGIN
    INSERT OR REPLACE INTO sync_outbox (kind, sync_id, removed, changed_at)
    SELECT 'archived_island', OLD.sync_id, 1, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
    WHERE (SELECT applying FROM sync_control) = 0;
END;

CREATE TRIGGER archived_tab_sync_insert AFTER INSERT ON archived_tab
BEGIN
    UPDATE archived_tab SET sync_id = lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' || substr(lower(hex(randomblob(2))), 2) || '-' || substr('89ab', 1 + abs(random() % 4), 1) || substr(lower(hex(randomblob(2))), 2) || '-' || lower(hex(randomblob(6))) WHERE id = NEW.id AND sync_id IS NULL;
    INSERT OR REPLACE INTO sync_outbox (kind, sync_id, removed, changed_at)
    SELECT 'archived_tab', sync_id, 0, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER) FROM archived_tab
    WHERE id = NEW.id AND (SELECT applying FROM sync_control) = 0;
END;

CREATE TRIGGER archived_tab_sync_update AFTER UPDATE OF archived_island_id, url, title, position, pinned, last_used_at, created_at, archived_at, unused ON archived_tab
WHEN NEW.sync_id IS NOT NULL AND (OLD.archived_island_id IS NOT NEW.archived_island_id OR OLD.url IS NOT NEW.url OR OLD.title IS NOT NEW.title OR OLD.position IS NOT NEW.position OR OLD.pinned IS NOT NEW.pinned OR OLD.last_used_at IS NOT NEW.last_used_at OR OLD.created_at IS NOT NEW.created_at OR OLD.archived_at IS NOT NEW.archived_at OR OLD.unused IS NOT NEW.unused)
BEGIN
    INSERT OR REPLACE INTO sync_outbox (kind, sync_id, removed, changed_at)
    SELECT 'archived_tab', NEW.sync_id, 0, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
    WHERE (SELECT applying FROM sync_control) = 0;
END;

CREATE TRIGGER archived_tab_sync_delete AFTER DELETE ON archived_tab
WHEN OLD.sync_id IS NOT NULL
BEGIN
    INSERT OR REPLACE INTO sync_outbox (kind, sync_id, removed, changed_at)
    SELECT 'archived_tab', OLD.sync_id, 1, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
    WHERE (SELECT applying FROM sync_control) = 0;
END;
