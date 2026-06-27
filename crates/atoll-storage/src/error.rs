use thiserror::Error;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("could not create the database directory: {0}")]
    Directory(#[from] std::io::Error),
    #[error("a bookmark folder cannot be moved into itself")]
    CyclicBookmark,
    #[error("{entity} {id} does not exist")]
    NotFound { entity: &'static str, id: i64 },
    #[error(
        "the database was written by a newer version of Atoll (schema {found}, supported {supported})"
    )]
    UnsupportedSchema { found: i64, supported: i64 },
}

pub type StorageResult<T> = Result<T, StorageError>;
