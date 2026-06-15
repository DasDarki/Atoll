use std::path::Path;

use rusqlite::{Connection, OpenFlags, Transaction};

use crate::error::StorageResult;
use crate::schema;

/// Anything that can run statements against the Atoll database.
///
/// Every store trait is implemented for all `Connected` types, so the same operations are
/// available on [`Storage`] and inside a [`StorageTransaction`].
pub trait Connected {
    fn connection(&self) -> &Connection;
}

/// The SQLite database, the single source of truth for workspaces, islands and tabs.
pub struct Storage {
    connection: Connection,
}

impl Storage {
    pub fn open(path: &Path) -> StorageResult<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        Self::initialize(connection)
    }

    pub fn open_in_memory() -> StorageResult<Self> {
        Self::initialize(Connection::open_in_memory()?)
    }

    fn initialize(connection: Connection) -> StorageResult<Self> {
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "NORMAL")?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.pragma_update(None, "temp_store", "MEMORY")?;
        connection.busy_timeout(std::time::Duration::from_secs(2))?;
        connection.set_prepared_statement_cache_capacity(64);
        schema::migrate(&connection)?;
        Ok(Self { connection })
    }

    /// Runs `operation` atomically. The transaction is rolled back if it returns an error.
    pub fn transaction<T>(
        &self,
        operation: impl FnOnce(&StorageTransaction<'_>) -> StorageResult<T>,
    ) -> StorageResult<T> {
        let transaction = StorageTransaction {
            transaction: self.connection.unchecked_transaction()?,
        };
        let result = operation(&transaction)?;
        transaction.transaction.commit()?;
        Ok(result)
    }
}

impl Connected for Storage {
    fn connection(&self) -> &Connection {
        &self.connection
    }
}

/// An open transaction, see [`Storage::transaction`].
pub struct StorageTransaction<'a> {
    transaction: Transaction<'a>,
}

impl Connected for StorageTransaction<'_> {
    fn connection(&self) -> &Connection {
        &self.transaction
    }
}
