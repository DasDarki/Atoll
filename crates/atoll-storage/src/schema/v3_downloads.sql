CREATE TABLE download (
    id INTEGER PRIMARY KEY,
    url TEXT NOT NULL,
    path TEXT,
    file_name TEXT NOT NULL,
    received_bytes INTEGER NOT NULL DEFAULT 0,
    total_bytes INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL,
    started_at INTEGER NOT NULL,
    finished_at INTEGER
);

CREATE INDEX download_by_start ON download (started_at DESC);
