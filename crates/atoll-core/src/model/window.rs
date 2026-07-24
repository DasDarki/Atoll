use crate::ids::{WindowId, WorkspaceId};

/// A window as it is remembered between sessions. Each window shows one workspace, and a
/// workspace is shown in at most one window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowPlacement {
    pub id: WindowId,
    pub workspace_id: WorkspaceId,
    /// Position in logical pixels, unknown on platforms that do not reveal it.
    pub position: Option<(i32, i32)>,
    /// Size in logical pixels, unknown until the window was closed once.
    pub size: Option<(u32, u32)>,
    pub maximized: bool,
}
