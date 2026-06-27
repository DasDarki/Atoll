CREATE TABLE history (
    id              INTEGER PRIMARY KEY,
    url             TEXT    NOT NULL UNIQUE,
    title           TEXT,
    visit_count     INTEGER NOT NULL DEFAULT 0,
    last_visited_at INTEGER NOT NULL
);

CREATE INDEX history_by_last_visit ON history(last_visited_at DESC);

INSERT INTO history (url, title, visit_count, last_visited_at)
SELECT url, MAX(title), COUNT(*), MAX(visited_at) FROM tab_history GROUP BY url;

CREATE TABLE bookmark (
    id         INTEGER PRIMARY KEY,
    parent_id  INTEGER REFERENCES bookmark(id) ON DELETE CASCADE,
    title      TEXT    NOT NULL,
    url        TEXT,
    position   INTEGER NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE INDEX bookmark_by_parent ON bookmark(parent_id, position);
CREATE INDEX bookmark_by_url ON bookmark(url);
