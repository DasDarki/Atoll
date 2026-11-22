//! What the devices keep in step: workspaces, islands and tabs, and the archive. Every record
//! names the others by their [`SyncId`], never by the row ids of one device. Times are
//! milliseconds since the Unix epoch, as everywhere in Atoll.

use serde::{Deserialize, Serialize};

use crate::id::SyncId;

/// The kind of a record, part of its address on the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Workspace,
    Island,
    Tab,
    ArchivedWorkspace,
    ArchivedIsland,
    ArchivedTab,
}

impl Kind {
    pub const ALL: [Kind; 6] = [
        Kind::Workspace,
        Kind::Island,
        Kind::Tab,
        Kind::ArchivedWorkspace,
        Kind::ArchivedIsland,
        Kind::ArchivedTab,
    ];

    /// The name the kind is written as, in the protocol and in the encryption.
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Workspace => "workspace",
            Kind::Island => "island",
            Kind::Tab => "tab",
            Kind::ArchivedWorkspace => "archived_workspace",
            Kind::ArchivedIsland => "archived_island",
            Kind::ArchivedTab => "archived_tab",
        }
    }

    pub fn parse(name: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|kind| kind.as_str() == name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    pub name: String,
    /// Where it stands among the workspaces, see [`crate::order`].
    pub order: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Island {
    pub workspace: SyncId,
    pub name: String,
    /// The index of the colour, as Atoll keeps it.
    pub color: i64,
    pub collapsed: bool,
    /// Where it stands among the islands of its workspace, see [`crate::order`].
    pub order: String,
    pub last_used_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tab {
    pub island: SyncId,
    pub url: String,
    pub title: Option<String>,
    pub pinned: bool,
    /// Where it stands among the tabs of its island, see [`crate::order`].
    pub order: String,
    pub last_used_at: Option<i64>,
    pub created_at: i64,
}

/// A workspace as the archive remembers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchivedWorkspace {
    /// The workspace the copy was taken from while it exists.
    pub workspace: Option<SyncId>,
    pub name: String,
    pub position: i64,
}

/// An island as the archive remembers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchivedIsland {
    pub archived_workspace: SyncId,
    /// The island the copy was taken from while it exists.
    pub island: Option<SyncId>,
    pub name: String,
    pub color: i64,
    pub position: i64,
}

/// A tab in the archive. Its position is the place it had when it left, as in the archive of
/// Atoll itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchivedTab {
    pub archived_island: SyncId,
    pub url: String,
    pub title: Option<String>,
    pub pinned: bool,
    pub position: i64,
    pub last_used_at: Option<i64>,
    pub created_at: i64,
    pub archived_at: i64,
    /// Whether it went on its own, not visited for longer than the settings allow.
    pub unused: bool,
}

/// The content of one record, whatever its kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum Record {
    Workspace(Workspace),
    Island(Island),
    Tab(Tab),
    ArchivedWorkspace(ArchivedWorkspace),
    ArchivedIsland(ArchivedIsland),
    ArchivedTab(ArchivedTab),
}

impl Record {
    pub fn kind(&self) -> Kind {
        match self {
            Record::Workspace(_) => Kind::Workspace,
            Record::Island(_) => Kind::Island,
            Record::Tab(_) => Kind::Tab,
            Record::ArchivedWorkspace(_) => Kind::ArchivedWorkspace,
            Record::ArchivedIsland(_) => Kind::ArchivedIsland,
            Record::ArchivedTab(_) => Kind::ArchivedTab,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_are_written_and_read_by_name() {
        for kind in Kind::ALL {
            assert_eq!(Kind::parse(kind.as_str()), Some(kind));
            assert_eq!(
                serde_json::to_string(&kind).unwrap(),
                format!("\"{}\"", kind.as_str())
            );
        }
        assert_eq!(Kind::parse("bookmark"), None);
    }

    #[test]
    fn a_record_carries_its_kind() {
        let record = Record::Workspace(Workspace {
            name: "Arbeit".to_owned(),
            order: "V".to_owned(),
        });
        let json = serde_json::to_string(&record).unwrap();
        assert_eq!(
            json,
            r#"{"kind":"workspace","data":{"name":"Arbeit","order":"V"}}"#
        );
        let back: Record = serde_json::from_str(&json).unwrap();
        assert_eq!(back, record);
        assert_eq!(back.kind(), Kind::Workspace);
    }
}
