//! Extensions for Atoll, written in Luau.
//!
//! An extension is a folder with a `plugin.toml` and Luau files. It gets a Luau state of its own
//! with a memory limit and a time budget, and reaches the browser only through the functions its
//! manifest asks for. The browser side is a [`PluginHost`], which keeps this crate free of the
//! user interface and the browser engine.

mod api;
mod content;
mod event;
mod host;
mod manifest;
mod pattern;
mod runtime;

pub use api::Command;
pub use content::{ContentScript, RunAt, ScriptWorld};
pub use event::Event;
pub use host::{
    HostError, HttpRequest, LogLevel, PendingId, PendingResult, PluginHost, PluginId, TabInfo,
};
pub use manifest::{
    API_VERSION, MANIFEST_NAME, Manifest, ManifestError, Panel, PanelLocation, Permission,
};
pub use pattern::{PatternError, UrlPattern, matches_glob};
pub use runtime::{PageMessage, PluginError, PluginInfo, PluginRuntime, installed};
