use crate::ids::WorkspaceId;

/// A complete browsing context such as "Work" or "Private". The sidebar always shows exactly one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: String,
    pub position: i64,
}
