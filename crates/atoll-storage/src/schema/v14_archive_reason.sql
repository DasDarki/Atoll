ALTER TABLE archived_tab ADD COLUMN unused INTEGER NOT NULL DEFAULT 0;

UPDATE archived_tab SET unused = 1
WHERE archived_at IN (
    SELECT archived_at FROM archived_tab
    GROUP BY archived_at
    HAVING COUNT(DISTINCT archived_island_id) > 1
);
