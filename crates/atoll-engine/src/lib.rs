//! The Chromium Embedded Framework integration of Atoll.
//!
//! Browsers are rendered off-screen. Every frame ends up in a `wgpu` texture owned by the
//! [`BrowserView`], ready to be composited by the user interface. All input is forwarded
//! explicitly through the view.

mod app;
mod clipboard;
mod config;
mod engine;
mod error;
mod events;
mod input;
mod process;
mod pump;
mod registry;
mod render;
mod view;

pub use clipboard::ClipboardHelper;
pub use config::EngineConfig;
pub use engine::Engine;
pub use error::EngineError;
pub use events::{
    ContextMenuRequest, CursorKind, DownloadState, EngineEvent, FileDialogMode, FileDialogRequest,
    JsDialogKind, JsDialogRequest, MenuEntry, MenuEntryKind, OpenDisposition, RequestId,
};
pub use input::{EditCommand, KeyInput, KeyInputKind, Modifiers, MouseButton, ZoomCommand};
pub use process::run_subprocess;
pub use render::{GpuContext, GpuError, SurfaceFrame, prune_unusable_vulkan_drivers};
pub use view::{BrowserView, CertificateInfo, SecurityInfo, ViewGeometry, ViewId, ViewRect};
