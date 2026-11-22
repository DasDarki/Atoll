#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("{0:?} is no sync id")]
    InvalidId(String),
    #[error("the passphrase or recovery key does not fit")]
    WrongKey,
    #[error("the recovery key is not complete or has a typo")]
    InvalidRecoveryKey,
    #[error("the data could not be read: it was changed or belongs elsewhere")]
    Corrupt,
    #[error("the data comes from a newer Atoll (format {0})")]
    UnknownFormat(u8),
    #[error("the passphrase could not be turned into a key: {0}")]
    Derivation(String),
    #[error("the content is no valid record: {0}")]
    Record(#[from] serde_json::Error),
}
