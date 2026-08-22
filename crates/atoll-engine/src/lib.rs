//! The Chromium Embedded Framework integration of Atoll.
//!
//! Browsers are rendered off-screen. Every frame ends up in a `wgpu` texture owned by the
//! [`BrowserView`], ready to be composited by the user interface. All input is forwarded
//! explicitly through the view.

mod activity;
mod app;
mod autofill;
mod clipboard;
mod config;
mod dev_tools;
mod engine;
mod error;
mod events;
mod filter;
mod input;
mod page_bridge;
mod passkeys;
mod process;
mod pump;
mod registry;
mod render;
mod video;
mod view;

pub use autofill::{FilledFields, LoginFill};
pub use clipboard::ClipboardHelper;
pub use config::EngineConfig;
pub use dev_tools::DevToolsSession;
pub use engine::Engine;
pub use error::EngineError;
pub use events::{
    ContextMenuRequest, CursorKind, DownloadState, EngineEvent, FileDialogMode, FileDialogRequest,
    JsDialogKind, JsDialogRequest, MenuEntry, MenuEntryKind, OpenDisposition, RequestId,
};
pub use filter::{ContentFilter, DocumentRules, ResourceKind, ResourceRequest};
pub use input::{EditCommand, KeyInput, KeyInputKind, Modifiers, MouseButton, ZoomCommand};
pub use page_bridge::{ReplyId, ScriptId, ScriptWorld};
pub use process::run_subprocess;
pub use render::{GpuContext, GpuError, SurfaceFrame, prune_unusable_vulkan_drivers};
pub use video::{FrameVideos, PageVideo};
pub use view::{BrowserView, CertificateInfo, SecurityInfo, ViewGeometry, ViewId, ViewRect};
