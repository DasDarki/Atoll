use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::rc::{Rc, Weak};
use std::time::Duration;

use atoll_plugin::{
    HostError, HttpRequest, LogLevel, PendingId, PendingResult, PluginHost, PluginId, TabInfo,
};
use atoll_storage::Storage;
use atoll_storage::prelude::*;
use serde_json::json;
use slint::{Timer, TimerMode};

use super::Plugins;
use crate::browser::Browser;
use crate::browser::windows::WindowManager;
use crate::platform::tasks::run_in_background;

/// How many log lines of one extension the settings page keeps.
const KEPT_LOGS: usize = 200;

/// How long an extension may wait for an answer of a server.
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// The largest answer an extension is handed.
const MAX_RESPONSE: u64 = 8 * 1024 * 1024;

/// The browser as the extensions see it. Everything reaches the window that has the focus,
/// because extensions belong to Atoll as a whole and not to one window.
pub(super) struct Bridge {
    storage: Rc<Storage>,
    manager: RefCell<Weak<WindowManager>>,
    plugins: RefCell<Weak<Plugins>>,
    logs: RefCell<BTreeMap<PluginId, VecDeque<String>>>,
    timers: RefCell<Vec<Timer>>,
}

impl Bridge {
    pub(super) fn new(storage: Rc<Storage>) -> Self {
        Self {
            storage,
            manager: RefCell::default(),
            plugins: RefCell::default(),
            logs: RefCell::default(),
            timers: RefCell::default(),
        }
    }

    pub(super) fn attach(&self, manager: Weak<WindowManager>, plugins: Weak<Plugins>) {
        *self.manager.borrow_mut() = manager;
        *self.plugins.borrow_mut() = plugins;
    }

    /// What the extension logged, newest last.
    pub(super) fn logs(&self, plugin: &PluginId) -> Vec<String> {
        self.logs
            .borrow()
            .get(plugin)
            .map(|lines| lines.iter().cloned().collect())
            .unwrap_or_default()
    }

    fn browser(&self) -> Option<Rc<Browser>> {
        self.manager.borrow().upgrade()?.focused_browser()
    }

    fn note(&self, plugin: &PluginId, line: String) {
        let mut logs = self.logs.borrow_mut();
        let lines = logs.entry(plugin.clone()).or_default();
        if lines.len() >= KEPT_LOGS {
            lines.pop_front();
        }
        lines.push_back(line);
    }
}

impl PluginHost for Bridge {
    fn log(&self, plugin: &PluginId, level: LogLevel, message: &str) {
        let marker = match level {
            LogLevel::Info => "",
            LogLevel::Warn => "Warnung: ",
            LogLevel::Error => "Fehler: ",
        };
        tracing::debug!(plugin = %plugin, %message, "extension log");
        self.note(plugin, format!("{marker}{message}"));
    }

    fn toast(&self, _plugin: &PluginId, message: &str) {
        if let Some(browser) = self.browser() {
            browser.show_toast(message);
        }
    }

    fn tabs(&self, _plugin: &PluginId) -> Vec<TabInfo> {
        self.browser()
            .map(|browser| browser.plugin_tabs())
            .unwrap_or_default()
    }

    fn open_tab(&self, _plugin: &PluginId, url: &str, background: bool) -> Result<i64, HostError> {
        let browser = self
            .browser()
            .ok_or_else(|| HostError::Refused("kein Fenster offen".to_owned()))?;
        browser
            .plugin_open_tab(url, background)
            .ok_or_else(|| HostError::Refused(format!("{url} konnte nicht geöffnet werden")))
    }

    fn close_tab(&self, _plugin: &PluginId, tab: i64) -> Result<(), HostError> {
        let browser = self
            .browser()
            .ok_or_else(|| HostError::Refused("kein Fenster offen".to_owned()))?;
        browser.plugin_close_tab(tab)
    }

    fn activate_tab(&self, _plugin: &PluginId, tab: i64) -> Result<(), HostError> {
        let browser = self
            .browser()
            .ok_or_else(|| HostError::Refused("kein Fenster offen".to_owned()))?;
        browser.plugin_activate_tab(tab)
    }

    fn stored(&self, plugin: &PluginId, key: &str) -> Option<String> {
        self.storage.plugin_value(plugin, key).ok().flatten()
    }

    fn store(&self, plugin: &PluginId, key: &str, value: Option<&str>) {
        if let Err(error) = self.storage.set_plugin_value(plugin, key, value) {
            tracing::warn!(%error, plugin = %plugin, "extension data could not be stored");
        }
    }

    fn stored_keys(&self, plugin: &PluginId) -> Vec<String> {
        self.storage.plugin_keys(plugin).unwrap_or_default()
    }

    fn commands_changed(&self, _plugin: &PluginId) {
        if let Some(manager) = self.manager.borrow().upgrade() {
            for browser in manager.browsers() {
                browser.refresh_plugin_shortcuts();
            }
        }
    }

    fn fetch(&self, plugin: &PluginId, pending: PendingId, request: HttpRequest) {
        let plugins = self.plugins.borrow().clone();
        let plugin = plugin.clone();
        run_in_background(
            "plugin-http",
            move || fetch(&request),
            move |result| {
                if let Some(plugins) = plugins.upgrade() {
                    tracing::debug!(plugin = %plugin, ok = result.is_ok(), "extension request finished");
                    plugins.resume(pending, result);
                }
            },
        );
    }

    fn sleep(&self, _plugin: &PluginId, pending: PendingId, milliseconds: u64) {
        let plugins = self.plugins.borrow().clone();
        let timer = Timer::default();
        timer.start(
            TimerMode::SingleShot,
            Duration::from_millis(milliseconds),
            move || {
                if let Some(plugins) = plugins.upgrade() {
                    plugins.resume(pending, Ok(json!(null)));
                }
            },
        );
        self.timers.borrow_mut().push(timer);
        self.timers.borrow_mut().retain(Timer::running);
    }

    fn report(&self, plugin: &PluginId, message: &str) {
        self.note(plugin, format!("Fehler: {message}"));
        if let Some(browser) = self.browser() {
            browser.show_toast(&format!("Erweiterung {plugin}: {message}"));
        }
    }

    fn disabled(&self, plugin: &PluginId, reason: &str) {
        self.note(plugin, format!("abgeschaltet: {reason}"));
        if let Some(plugins) = self.plugins.borrow().upgrade() {
            plugins.remember_disabled(plugin);
        }
        if let Some(browser) = self.browser() {
            browser.show_toast(&format!("Erweiterung {plugin} abgeschaltet: {reason}"));
        }
    }
}

/// Runs one request of an extension, off the main thread. Methods with a body and methods
/// without one are two different builders in `ureq`, so they take two paths here.
fn fetch(request: &HttpRequest) -> PendingResult {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(HTTP_TIMEOUT))
        .user_agent(concat!("Atoll/", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let url = request.url.as_str();
    let sent = match request.method.as_str() {
        method @ ("POST" | "PUT" | "PATCH") => {
            let mut call = match method {
                "POST" => agent.post(url),
                "PUT" => agent.put(url),
                _ => agent.patch(url),
            };
            for (name, value) in &request.headers {
                call = call.header(name, value);
            }
            call.send(request.body.clone().unwrap_or_default())
        }
        method => {
            let mut call = match method {
                "DELETE" => agent.delete(url),
                "HEAD" => agent.head(url),
                _ => agent.get(url),
            };
            for (name, value) in &request.headers {
                call = call.header(name, value);
            }
            call.call()
        }
    };
    let mut response = sent.map_err(|error| error.to_string())?;
    let status = response.status().as_u16();
    let headers: BTreeMap<String, String> = response
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_owned(),
                value.to_str().unwrap_or_default().to_owned(),
            )
        })
        .collect();
    let body = response
        .body_mut()
        .with_config()
        .limit(MAX_RESPONSE)
        .read_to_string()
        .map_err(|error| error.to_string())?;
    Ok(json!({ "status": status, "headers": headers, "body": body }))
}
