use std::path::PathBuf;

use crate::view::ViewId;

/// Identifies a pending request that must be answered through the [`crate::Engine`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RequestId(pub(crate) u64);

/// Notifications produced by the engine, drained with [`crate::Engine::drain_events`].
#[derive(Debug, Clone)]
pub enum EngineEvent {
    /// A new frame is available in the view's surface.
    FrameReady(ViewId),
    /// A page widget such as a `<select>` dropdown was shown, moved, painted or hidden.
    PopupWidgetChanged(ViewId),
    AddressChanged {
        view: ViewId,
        url: String,
    },
    TitleChanged {
        view: ViewId,
        title: String,
    },
    FaviconReady {
        view: ViewId,
        page_url: String,
        png: Vec<u8>,
    },
    LoadingStateChanged {
        view: ViewId,
        loading: bool,
        can_go_back: bool,
        can_go_forward: bool,
    },
    LoadProgress {
        view: ViewId,
        progress: f64,
    },
    MainFrameLoaded {
        view: ViewId,
        http_status: i32,
    },
    CursorChanged {
        view: ViewId,
        cursor: CursorKind,
    },
    StatusMessage {
        view: ViewId,
        text: String,
    },
    Tooltip {
        view: ViewId,
        text: String,
    },
    FullscreenChanged {
        view: ViewId,
        fullscreen: bool,
    },
    /// The page wants to open an address in another tab.
    OpenUrl {
        view: ViewId,
        url: String,
        disposition: OpenDisposition,
    },
    /// The page opened a scripted popup that keeps its connection to the opener. The new view
    /// already exists and can be taken over with [`crate::Engine::adopt_view`].
    PopupOpened {
        view: ViewId,
        opener: ViewId,
        url: String,
    },
    ContextMenu {
        view: ViewId,
        request: ContextMenuRequest,
    },
    FileDialog {
        view: ViewId,
        request: FileDialogRequest,
    },
    JsDialog {
        view: ViewId,
        request: JsDialogRequest,
    },
    /// A dialog shown for the view is no longer needed, for example after a navigation.
    JsDialogDismissed {
        view: ViewId,
    },
    Download(DownloadState),
    FindResult {
        view: ViewId,
        matches: i32,
        active_match: i32,
        final_update: bool,
    },
    /// Text the user wants to copy through a context menu entry of Atoll.
    CopyText(String),
    /// The content of Chromium's clipboard, read by the [`crate::ClipboardHelper`].
    ClipboardText(String),
    /// Text was written into Chromium's clipboard by the [`crate::ClipboardHelper`].
    ClipboardWritten,
    /// The page asked to close itself, for example via `window.close()`.
    CloseRequested(ViewId),
    Closed(ViewId),
    RenderProcessGone(ViewId),
    /// Shared texture import failed, the view should be recreated with CPU rendering.
    AcceleratedPaintFailed(ViewId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenDisposition {
    ForegroundTab,
    BackgroundTab,
    NewWindow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorKind {
    #[default]
    Default,
    Pointer,
    Text,
    Wait,
    Progress,
    Help,
    Crosshair,
    Move,
    NotAllowed,
    NoDrop,
    Grab,
    Grabbing,
    Copy,
    Alias,
    ResizeEastWest,
    ResizeNorthSouth,
    ResizeNorthEastSouthWest,
    ResizeNorthWestSouthEast,
    ResizeColumn,
    ResizeRow,
    None,
}

#[derive(Debug, Clone)]
pub struct ContextMenuRequest {
    pub id: RequestId,
    /// Position in view coordinates.
    pub x: i32,
    pub y: i32,
    pub entries: Vec<MenuEntry>,
}

#[derive(Debug, Clone)]
pub struct MenuEntry {
    pub command_id: i32,
    pub label: String,
    pub enabled: bool,
    pub kind: MenuEntryKind,
}

#[derive(Debug, Clone)]
pub enum MenuEntryKind {
    Command,
    Check { checked: bool },
    Separator,
    Submenu(Vec<MenuEntry>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileDialogMode {
    Open,
    OpenMultiple,
    OpenFolder,
    Save,
}

#[derive(Debug, Clone)]
pub struct FileDialogRequest {
    pub id: RequestId,
    pub mode: FileDialogMode,
    pub title: String,
    pub default_path: Option<PathBuf>,
    /// Accepted types as given by the page, e.g. `image/*` or `.png`.
    pub accept_types: Vec<String>,
    /// File extensions derived from `accept_types`, without the leading dot.
    pub accept_extensions: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsDialogKind {
    Alert,
    Confirm,
    Prompt,
    BeforeUnload { is_reload: bool },
}

#[derive(Debug, Clone)]
pub struct JsDialogRequest {
    pub id: RequestId,
    pub kind: JsDialogKind,
    pub origin: String,
    pub message: String,
    pub default_text: String,
}

#[derive(Debug, Clone)]
pub struct DownloadState {
    /// Identifies the download for [`crate::Engine::cancel_download`] and friends.
    pub id: u32,
    pub view: Option<ViewId>,
    pub url: String,
    pub file_name: String,
    pub path: Option<PathBuf>,
    pub received_bytes: i64,
    /// The expected size, zero if unknown.
    pub total_bytes: i64,
    /// Bytes per second.
    pub speed: i64,
    pub in_progress: bool,
    pub paused: bool,
    pub complete: bool,
    pub canceled: bool,
    /// The download failed, for example because the connection was lost.
    pub interrupted: bool,
}
