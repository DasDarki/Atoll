mod connection;
mod error;
mod schema;
mod store;

pub use connection::{Connected, Storage, StorageTransaction};
pub use error::{StorageError, StorageResult};
pub use store::bookmarks::{BookmarkStore, NewBookmark};
pub use store::downloads::{DownloadProgress, DownloadStore, NewDownload};
pub use store::favicons::FaviconStore;
pub use store::history::{HistoryEntry, HistoryStore};
pub use store::islands::{ArchivedIsland, IslandStore, NewIsland};
pub use store::settings::SettingsStore;
pub use store::tabs::{NewTab, TabEntry, TabStore};
pub use store::windows::WindowStore;
pub use store::workspaces::WorkspaceStore;

/// Brings every store trait into scope.
pub mod prelude {
    pub use crate::{
        BookmarkStore, Connected, DownloadStore, FaviconStore, HistoryStore, IslandStore,
        SettingsStore, TabStore, WindowStore, WorkspaceStore,
    };
}
