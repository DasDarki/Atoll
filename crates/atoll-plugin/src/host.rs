//! What the runtime asks the browser to do. Atoll implements this, the tests implement it with a
//! stand-in, so the runtime itself needs neither a window nor a browser engine.

use serde_json::Value;

use crate::content::{ContentScript, ScriptWorld};
use crate::manifest::{Panel, Permission};

/// Identifies a loaded extension, which is the `id` of its manifest.
pub type PluginId = String;

/// Identifies work the browser is doing for an extension whose script waits for the answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PendingId(pub u64);

/// A tab as extensions see it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TabInfo {
    pub id: i64,
    pub url: String,
    pub title: String,
    pub active: bool,
}

/// What an extension wants to fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
}

/// How urgent a message of an extension is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

/// Why the browser refused something an extension asked for.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HostError {
    #[error("{0}")]
    Refused(String),
    #[error("tab {0} is not open")]
    UnknownTab(i64),
}

/// The browser, as far as extensions can reach it. Everything that takes a [`PendingId`] answers
/// later through [`crate::PluginRuntime::resume`], so the script that waits for it does not block
/// the window.
pub trait PluginHost {
    /// A message of the extension for its log.
    fn log(&self, plugin: &PluginId, level: LogLevel, message: &str);

    /// A short message for the user.
    fn toast(&self, plugin: &PluginId, message: &str);

    fn tabs(&self, plugin: &PluginId) -> Vec<TabInfo>;

    fn open_tab(&self, plugin: &PluginId, url: &str, background: bool) -> Result<i64, HostError>;

    fn close_tab(&self, plugin: &PluginId, tab: i64) -> Result<(), HostError>;

    fn activate_tab(&self, plugin: &PluginId, tab: i64) -> Result<(), HostError>;

    fn stored(&self, plugin: &PluginId, key: &str) -> Option<String>;

    fn store(&self, plugin: &PluginId, key: &str, value: Option<&str>);

    fn stored_keys(&self, plugin: &PluginId) -> Vec<String>;

    /// The list of commands of `plugin` changed, so the palette has to be rebuilt.
    fn commands_changed(&self, plugin: &PluginId);

    /// Fetches `request` and answers with `{ status, headers, body }`.
    fn fetch(&self, plugin: &PluginId, pending: PendingId, request: HttpRequest);

    /// Answers with nothing after `milliseconds`.
    fn sleep(&self, plugin: &PluginId, pending: PendingId, milliseconds: u64);

    /// The address of the page the user is looking at.
    fn current_page_url(&self, plugin: &PluginId) -> Option<String>;

    /// Runs `code` in the current page and answers with its value.
    fn evaluate(&self, plugin: &PluginId, pending: PendingId, code: String, world: ScriptWorld);

    /// Puts `script` into every page from now on, replacing the one of the same name.
    fn set_content_script(&self, plugin: &PluginId, script: ContentScript);

    fn remove_content_script(&self, plugin: &PluginId, name: &str);

    /// Answers a message a page sent with `window.atoll.send`.
    fn answer_page(&self, plugin: &PluginId, reply: u64, answer: PendingResult);

    /// Shows the panel `panel` of the extension.
    fn show_panel(&self, plugin: &PluginId, panel: &Panel) -> Result<(), HostError>;

    fn hide_panel(&self, plugin: &PluginId, panel: &str);

    /// Sends a message to the panel, where `window.atoll.on` picks it up.
    fn post_to_panel(&self, plugin: &PluginId, panel: &str, channel: &str, payload: Value);

    /// The extension ran into an error Atoll should show, for example a failed handler.
    fn report(&self, plugin: &PluginId, message: &str);

    /// The extension was switched off because it kept breaking its limits.
    fn disabled(&self, plugin: &PluginId, reason: &str);
}

/// The answer to a [`PendingId`], either a value for the waiting script or an error it can catch.
pub type PendingResult = Result<Value, String>;

/// A message for the user when an extension asks for something it may not do.
pub(crate) fn missing_permission(permission: Permission) -> String {
    format!("diese Erweiterung darf das nicht, im Manifest fehlt die Berechtigung \"{permission}\"")
}

/// A message for the user when an extension reaches for a site it did not ask for.
pub(crate) fn missing_host(url: &str) -> String {
    format!("diese Erweiterung darf nicht auf {url} zugreifen, die Adresse fehlt unter \"hosts\"")
}
