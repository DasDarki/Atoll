use rusqlite::Connection;

use crate::error::{StorageError, StorageResult};

const MIGRATIONS: &[&str] = &[
    include_str!("v1_initial.sql"),
    include_str!("v2_bookmarks_and_history.sql"),
    include_str!("v3_downloads.sql"),
    include_str!("v4_windows.sql"),
    include_str!("v5_adblock.sql"),
    include_str!("v6_island_usage.sql"),
    include_str!("v7_download_kind.sql"),
];

pub(crate) fn migrate(connection: &Connection) -> StorageResult<()> {
    let supported = MIGRATIONS.len() as i64;
    let current: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if current > supported {
        return Err(StorageError::UnsupportedSchema {
            found: current,
            supported,
        });
    }

    for (index, migration) in MIGRATIONS.iter().enumerate().skip(current as usize) {
        let transaction = connection.unchecked_transaction()?;
        transaction.execute_batch(migration)?;
        transaction.pragma_update(None, "user_version", index as i64 + 1)?;
        transaction.commit()?;
    }
    Ok(())
}
