UPDATE island
SET last_used_at = CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
WHERE last_used_at IS NULL;
