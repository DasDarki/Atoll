use std::path::PathBuf;

use crate::autofill::FilledFields;
use crate::page_bridge::{ReplyId, ScriptId};
use crate::video::FrameVideos;
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
    /// The user chose "inspect" in the context menu of the page at a position in view
    /// coordinates.
    InspectElement {
        view: ViewId,
        x: i32,
        y: i32,
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
    /// The page started or stopped being busy: it plays audible media or holds input the user
    /// has not sent yet. Busy pages should not be unloaded to save memory.
    PageBusy {
        view: ViewId,
        busy: bool,
    },
    /// A password field appeared on the page or the last one disappeared.
    LoginFormChanged {
        view: ViewId,
        present: bool,
    },
    /// A frame answered [`crate::BrowserView::fill_login`] with the fields it filled.
    LoginFilled {
        view: ViewId,
        fields: FilledFields,
    },
    /// The page asked for a passkey or security key, which was refused.
    PasskeyRefused(ViewId),
    /// A video of a noticeable size appeared on the page or the last one disappeared.
    VideoChanged {
        view: ViewId,
        present: bool,
    },
    /// A script of [`crate::BrowserView::evaluate`] finished.
    ScriptResult {
        view: ViewId,
        request: ScriptId,
        result: Result<serde_json::Value, String>,
    },
    /// A page called `window.atoll.send`. It waits for [`crate::BrowserView::answer_page`].
    PageMessage {
        view: ViewId,
        frame_url: String,
        channel: String,
        payload: serde_json::Value,
        reply: ReplyId,
    },
    /// A frame answered [`crate::BrowserView::find_videos`].
    VideosFound {
        view: ViewId,
        frame: FrameVideos,
    },
}

impl EngineEvent {
    /// The view whose tab the event concerns: the opener for popups, `None` for events of the
    /// whole engine.
    pub fn view(&self) -> Option<ViewId> {
        match self {
            EngineEvent::FrameReady(view)
            | EngineEvent::PopupWidgetChanged(view)
            | EngineEvent::CloseRequested(view)
            | EngineEvent::Closed(view)
            | EngineEvent::RenderProcessGone(view)
            | EngineEvent::AcceleratedPaintFailed(view)
            | EngineEvent::PasskeyRefused(view) => Some(*view),
            EngineEvent::AddressChanged { view, .. }
            | EngineEvent::TitleChanged { view, .. }
            | EngineEvent::FaviconReady { view, .. }
            | EngineEvent::LoadingStateChanged { view, .. }
            | EngineEvent::LoadProgress { view, .. }
            | EngineEvent::MainFrameLoaded { view, .. }
            | EngineEvent::CursorChanged { view, .. }
            | EngineEvent::StatusMessage { view, .. }
            | EngineEvent::Tooltip { view, .. }
            | EngineEvent::FullscreenChanged { view, .. }
            | EngineEvent::OpenUrl { view, .. }
            | EngineEvent::ContextMenu { view, .. }
            | EngineEvent::FileDialog { view, .. }
            | EngineEvent::JsDialog { view, .. }
            | EngineEvent::JsDialogDismissed { view }
            | EngineEvent::FindResult { view, .. }
            | EngineEvent::InspectElement { view, .. }
            | EngineEvent::PageBusy { view, .. }
            | EngineEvent::LoginFormChanged { view, .. }
            | EngineEvent::LoginFilled { view, .. }
            | EngineEvent::VideoChanged { view, .. }
            | EngineEvent::VideosFound { view, .. }
            | EngineEvent::ScriptResult { view, .. }
            | EngineEvent::PageMessage { view, .. } => Some(*view),
            EngineEvent::PopupOpened { opener, .. } => Some(*opener),
            EngineEvent::Download(_)
            | EngineEvent::CopyText(_)
            | EngineEvent::ClipboardText(_)
            | EngineEvent::ClipboardWritten => None,
        }
    }
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
