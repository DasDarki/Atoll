//! Keeping the workspaces, islands, tabs and the archive of every device in step through the
//! Atoll sync server, end-to-end encrypted. This crate holds what the devices and the server
//! share: the ids, the order keys, the records, the encryption and the messages.

pub mod crypto;
pub mod error;
pub mod id;
pub mod model;
pub mod order;
pub mod wire;

pub use crypto::{DataKey, Hardness, KeyEnvelope};
pub use error::SyncError;
pub use id::SyncId;
pub use model::{Kind, Record};
