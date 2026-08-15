use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use atoll_plugin::{
    Event, HostError, HttpRequest, LogLevel, PendingId, Permission, PluginHost, PluginId,
    PluginRuntime, TabInfo,
};
use serde_json::json;

/// A browser that only writes down what it was asked to do.
#[derive(Default)]
struct FakeBrowser {
    calls: RefCell<Vec<String>>,
    reports: RefCell<Vec<String>>,
    storage: RefCell<BTreeMap<String, String>>,
    tabs: RefCell<Vec<TabInfo>>,
    sleeping: RefCell<Vec<PendingId>>,
    fetched: RefCell<Vec<(PendingId, HttpRequest)>>,
    disabled: RefCell<Vec<String>>,
}

impl FakeBrowser {
    fn calls(&self) -> Vec<String> {
        self.calls.borrow().clone()
    }

    fn reports(&self) -> Vec<String> {
        self.reports.borrow().clone()
    }

    fn note(&self, call: impl Into<String>) {
        self.calls.borrow_mut().push(call.into());
    }
}

impl PluginHost for FakeBrowser {
    fn log(&self, _plugin: &PluginId, level: LogLevel, message: &str) {
        self.note(format!("log {level:?} {message}"));
    }

    fn toast(&self, _plugin: &PluginId, message: &str) {
        self.note(format!("toast {message}"));
    }

    fn tabs(&self, _plugin: &PluginId) -> Vec<TabInfo> {
        self.note("tabs");
        self.tabs.borrow().clone()
    }

    fn open_tab(&self, _plugin: &PluginId, url: &str, background: bool) -> Result<i64, HostError> {
        self.note(format!("open {url} background={background}"));
        Ok(7)
    }

    fn close_tab(&self, _plugin: &PluginId, tab: i64) -> Result<(), HostError> {
        self.note(format!("close {tab}"));
        Err(HostError::UnknownTab(tab))
    }

    fn activate_tab(&self, _plugin: &PluginId, tab: i64) -> Result<(), HostError> {
        self.note(format!("activate {tab}"));
        Ok(())
    }

    fn stored(&self, _plugin: &PluginId, key: &str) -> Option<String> {
        self.storage.borrow().get(key).cloned()
    }

    fn store(&self, _plugin: &PluginId, key: &str, value: Option<&str>) {
        let mut storage = self.storage.borrow_mut();
        match value {
            Some(value) => storage.insert(key.to_owned(), value.to_owned()),
            None => storage.remove(key),
        };
    }

    fn stored_keys(&self, _plugin: &PluginId) -> Vec<String> {
        self.storage.borrow().keys().cloned().collect()
    }

    fn commands_changed(&self, _plugin: &PluginId) {
        self.note("commands changed");
    }

    fn fetch(&self, _plugin: &PluginId, pending: PendingId, request: HttpRequest) {
        self.fetched.borrow_mut().push((pending, request));
    }

    fn sleep(&self, _plugin: &PluginId, pending: PendingId, milliseconds: u64) {
        self.note(format!("sleep {milliseconds}"));
        self.sleeping.borrow_mut().push(pending);
    }

    fn report(&self, _plugin: &PluginId, message: &str) {
        self.reports.borrow_mut().push(message.to_owned());
    }

    fn disabled(&self, _plugin: &PluginId, reason: &str) {
        self.disabled.borrow_mut().push(reason.to_owned());
    }
}

/// An extension folder that is removed when the test ends.
struct Extension {
    directory: PathBuf,
}

impl Drop for Extension {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

impl Extension {
    fn new(name: &str, manifest: &str, entry: &str) -> Self {
        let directory = std::env::temp_dir().join(format!(
            "atoll-plugin-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("plugin.toml"), manifest).unwrap();
        fs::write(directory.join("main.luau"), entry).unwrap();
        Self { directory }
    }

    fn with_file(self, name: &str, content: &str) -> Self {
        fs::write(self.directory.join(name), content).unwrap();
        self
    }

    fn path(&self) -> &Path {
        &self.directory
    }
}

fn manifest(permissions: &str, hosts: &str) -> String {
    format!(
        r#"
        id = "demo"
        name = "Demo"
        version = "1.0"
        api = 1
        permissions = [{permissions}]
        hosts = [{hosts}]
        "#
    )
}

fn runtime() -> (PluginRuntime, Rc<FakeBrowser>) {
    let browser = Rc::new(FakeBrowser::default());
    (PluginRuntime::new(browser.clone()), browser)
}

const ALL: &[Permission] = &[
    Permission::Tabs,
    Permission::Pages,
    Permission::Storage,
    Permission::Network,
    Permission::Downloads,
];

#[test]
fn an_extension_runs_and_registers_its_commands() {
    let extension = Extension::new(
        "commands",
        &manifest("\"tabs\"", ""),
        r#"
        local atoll = require("@atoll")
        atoll.log("hallo", 1)
        atoll.commands.register{
            id = "greet",
            title = "Grüßen",
            shortcut = "Ctrl+Shift+G",
            run = function() atoll.ui.toast("hallo " .. atoll.id) end,
        }
        "#,
    );
    let (mut runtime, browser) = runtime();
    let info = runtime.load(extension.path(), ALL).unwrap();
    assert_eq!(info.name, "Demo");
    assert_eq!(info.commands.len(), 1);
    assert_eq!(info.commands[0].shortcut.as_deref(), Some("Ctrl+Shift+G"));
    assert_eq!(
        runtime.commands(),
        vec![("demo".to_owned(), info.commands[0].clone())]
    );

    runtime.run_command(&"demo".to_owned(), "greet");
    assert!(browser.calls().contains(&"toast hallo demo".to_owned()));
    assert!(browser.calls().contains(&"log Info hallo 1".to_owned()));
    assert!(browser.reports().is_empty());
}

#[test]
fn events_only_reach_extensions_that_may_see_them() {
    let extension = Extension::new(
        "events",
        &manifest("\"pages\"", "\"*://*.example.org/*\""),
        r#"
        local atoll = require("@atoll")
        atoll.on("page.loaded", function(page) atoll.ui.toast("geladen " .. page.url) end)
        atoll.on("tab.opened", function(tab) atoll.ui.toast("tab " .. tab.url) end)
        "#,
    );
    let (mut runtime, browser) = runtime();
    runtime.load(extension.path(), ALL).unwrap();

    runtime.dispatch(&Event::PageLoaded {
        tab: 1,
        url: "https://www.example.org/a".to_owned(),
        title: "A".to_owned(),
    });
    runtime.dispatch(&Event::PageLoaded {
        tab: 2,
        url: "https://other.test/b".to_owned(),
        title: "B".to_owned(),
    });
    runtime.dispatch(&Event::TabOpened(TabInfo {
        id: 3,
        url: "https://www.example.org/c".to_owned(),
        title: "C".to_owned(),
        active: true,
    }));

    let calls = browser.calls();
    assert!(calls.contains(&"toast geladen https://www.example.org/a".to_owned()));
    assert!(
        !calls.iter().any(|call| call.contains("other.test")),
        "sites outside \"hosts\" stay invisible"
    );
    assert!(
        !calls.iter().any(|call| call.starts_with("toast tab")),
        "without the tabs permission there are no tab events"
    );
}

#[test]
fn waiting_for_the_browser_does_not_block_and_is_continued() {
    let extension = Extension::new(
        "await",
        &manifest("\"network\"", "\"https://api.example/*\""),
        r#"
        local atoll = require("@atoll")
        atoll.commands.register{ id = "load", run = function()
            atoll.sleep(10)
            local answer = atoll.http.get("https://api.example/v1", { headers = { Accept = "application/json" } })
            atoll.ui.toast("status " .. tostring(answer.status) .. " " .. answer.body)
        end }
        "#,
    );
    let (mut runtime, browser) = runtime();
    runtime.load(extension.path(), ALL).unwrap();

    runtime.run_command(&"demo".to_owned(), "load");
    assert_eq!(runtime.waiting(), 1, "the script waits for the sleep");
    assert!(browser.calls().contains(&"sleep 10".to_owned()));

    let pending = browser.sleeping.borrow()[0];
    runtime.resume(pending, Ok(json!(null)));
    assert_eq!(runtime.waiting(), 1, "now it waits for the answer");
    let (pending, request) = browser.fetched.borrow()[0].clone();
    assert_eq!(request.method, "GET");
    assert_eq!(
        request.headers,
        vec![("Accept".to_owned(), "application/json".to_owned())]
    );

    runtime.resume(pending, Ok(json!({ "status": 200, "body": "ok" })));
    assert_eq!(runtime.waiting(), 0);
    assert!(browser.calls().contains(&"toast status 200 ok".to_owned()));
    assert!(browser.reports().is_empty());
}

#[test]
fn an_error_from_the_browser_can_be_caught_in_the_script() {
    let extension = Extension::new(
        "failing-await",
        &manifest("\"network\"", "\"https://api.example/*\""),
        r#"
        local atoll = require("@atoll")
        atoll.commands.register{ id = "load", run = function()
            local ok, problem = pcall(function() return atoll.http.get("https://api.example/v1") end)
            atoll.ui.toast(tostring(ok) .. " " .. tostring(problem))
        end }
        "#,
    );
    let (mut runtime, browser) = runtime();
    runtime.load(extension.path(), ALL).unwrap();
    runtime.run_command(&"demo".to_owned(), "load");
    let pending = browser.fetched.borrow()[0].0;
    runtime.resume(pending, Err("kein Netz".to_owned()));
    assert!(
        browser
            .calls()
            .contains(&"toast false kein Netz".to_owned())
    );
}

#[test]
fn missing_permissions_and_sites_are_refused() {
    let extension = Extension::new(
        "refused",
        &manifest("\"network\"", "\"https://api.example/*\""),
        r#"
        local atoll = require("@atoll")
        atoll.commands.register{ id = "tabs", run = function() atoll.tabs.list() end }
        atoll.commands.register{ id = "elsewhere", run = function() atoll.http.get("https://evil.test/x") end }
        "#,
    );
    let (mut runtime, browser) = runtime();
    runtime.load(extension.path(), ALL).unwrap();

    runtime.run_command(&"demo".to_owned(), "tabs");
    runtime.run_command(&"demo".to_owned(), "elsewhere");
    let reports = browser.reports();
    assert!(
        reports[0].contains("Berechtigung \"tabs\""),
        "reported: {reports:?}"
    );
    assert!(
        reports[1].contains("https://evil.test/x"),
        "reported: {reports:?}"
    );
    assert!(browser.fetched.borrow().is_empty());
}

#[test]
fn a_script_that_runs_forever_is_stopped_and_finally_switched_off() {
    let extension = Extension::new(
        "endless",
        &manifest("", ""),
        r#"
        local atoll = require("@atoll")
        atoll.commands.register{ id = "hang", run = function() while true do end end }
        "#,
    );
    let (mut runtime, browser) = runtime();
    runtime.load(extension.path(), ALL).unwrap();

    for _ in 0..3 {
        runtime.run_command(&"demo".to_owned(), "hang");
    }
    let reports = browser.reports();
    assert_eq!(reports.len(), 3, "each attempt is reported: {reports:?}");
    assert!(reports[0].contains("zu lange"));
    assert_eq!(browser.disabled.borrow().len(), 1);
    assert_eq!(
        runtime.loaded()[0].failure.as_deref(),
        Some("das Skript lief zu lange und wurde angehalten (3 Mal)")
    );
}

#[test]
fn storage_belongs_to_the_extension() {
    let extension = Extension::new(
        "storage",
        &manifest("\"storage\"", ""),
        r#"
        local atoll = require("@atoll")
        atoll.storage.set("a", "1")
        atoll.storage.set("b", "2")
        atoll.storage.remove("b")
        atoll.ui.toast(tostring(atoll.storage.get("a")) .. " " .. table.concat(atoll.storage.keys(), ","))
        "#,
    );
    let (mut runtime, browser) = runtime();
    runtime.load(extension.path(), ALL).unwrap();
    assert!(browser.calls().contains(&"toast 1 a".to_owned()));
}

#[test]
fn modules_are_loaded_from_the_extension_folder_only() {
    let extension = Extension::new(
        "modules",
        &manifest("", ""),
        r#"
        local atoll = require("@atoll")
        local util = require("./util")
        atoll.ui.toast(util.greet("Welt"))
        local ok, problem = pcall(function() return require("../../etc/passwd") end)
        atoll.ui.toast(tostring(ok) .. " " .. tostring(problem))
        "#,
    )
    .with_file(
        "util.luau",
        "return { greet = function(name) return \"hallo \" .. name end }",
    );
    let (mut runtime, browser) = runtime();
    runtime.load(extension.path(), ALL).unwrap();
    let calls = browser.calls();
    assert!(calls.contains(&"toast hallo Welt".to_owned()));
    assert!(
        calls
            .iter()
            .any(|call| call.starts_with("toast false") && call.contains("nicht in dieser")),
        "calls: {calls:?}"
    );
}

#[test]
fn a_broken_extension_is_not_loaded() {
    let broken = Extension::new("broken", &manifest("", ""), "error(\"kaputt\")");
    let (mut runtime, _) = runtime();
    let failure = runtime.load(broken.path(), ALL).unwrap_err();
    assert!(format!("{failure}").contains("kaputt"), "{failure}");
    assert!(!runtime.is_loaded(&"demo".to_owned()));

    let without_entry = Extension::new("no-entry", &manifest("", ""), "");
    fs::remove_file(without_entry.path().join("main.luau")).unwrap();
    assert!(runtime.load(without_entry.path(), ALL).is_err());
}

#[test]
fn unloading_forgets_everything_the_extension_waited_for() {
    let extension = Extension::new(
        "unload",
        &manifest("", ""),
        r#"
        local atoll = require("@atoll")
        atoll.commands.register{ id = "wait", run = function() atoll.sleep(1000) end }
        "#,
    );
    let (mut runtime, _) = runtime();
    runtime.load(extension.path(), ALL).unwrap();
    runtime.run_command(&"demo".to_owned(), "wait");
    assert_eq!(runtime.waiting(), 1);
    runtime.unload(&"demo".to_owned());
    assert_eq!(runtime.waiting(), 0);
    assert!(runtime.commands().is_empty());
    assert!(runtime.loaded().is_empty());
}
