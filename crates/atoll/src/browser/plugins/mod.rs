//! Extensions: loading them, asking the user about their permissions and connecting them to the
//! window. The Luau side lives in `atoll-plugin`, this is the browser side of its [`PluginHost`].

mod bridge;
mod install;
mod panel;

pub use panel::OpenPanel;

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::Duration;

use atoll_core::Timestamp;
use atoll_engine::{ReplyId, ScriptId, ViewId};
use atoll_plugin::{
    Command, ContentScript, Event, HostError, Manifest, PageMessage, Panel, PendingId,
    PendingResult, Permission, PluginHost, PluginId, PluginInfo, PluginRuntime, RuleAction,
    RuleSet, ScriptWorld, Source, TabInfo,
};
use atoll_storage::prelude::*;
use atoll_storage::{PluginRecord, Storage};
use serde_json::Value;
use slint::{ComponentHandle, ModelRc, VecModel};

use self::bridge::Bridge;
use super::Browser;
use crate::browser::windows::WindowManager;
use crate::platform::opener;
use crate::pump::MessagePump;
use crate::ui::{PluginItem, SettingsState};
use atoll_i18n::t;

/// The extensions of this Atoll, shared by every window.
pub struct Plugins {
    runtime: RefCell<PluginRuntime>,
    bridge: Rc<Bridge>,
    storage: Rc<Storage>,
    directory: PathBuf,
    /// Extensions that asked the user and are waiting for the answer.
    asking: RefCell<BTreeSet<PluginId>>,
    /// The styles and scripts that go into every page, by the name the browser knows them by.
    scripts: RefCell<BTreeMap<String, ContentScript>>,
    /// Scripts that are running in a page and the extension call that waits for the value.
    running: RefCell<HashMap<(ViewId, ScriptId), PendingId>>,
    /// Messages of pages that wait for an answer.
    replies: RefCell<HashMap<u64, (ViewId, ReplyId)>>,
    /// The engine instances that show a panel, and whose panel they show.
    panels: RefCell<HashMap<ViewId, (PluginId, String)>>,
    /// What extensions want done with requests, read on the network thread.
    rules: Arc<RuleSet>,
    next_reply: Cell<u64>,
    manager: RefCell<Weak<WindowManager>>,
}

impl Plugins {
    pub fn new(storage: Rc<Storage>, directory: PathBuf) -> Rc<Self> {
        let bridge = Rc::new(Bridge::new(storage.clone()));
        let mut runtime = PluginRuntime::new(bridge.clone());
        let schedule = MessagePump::scheduler();
        runtime.set_waker(Arc::new(move || schedule(Duration::ZERO)));
        Rc::new(Self {
            runtime: RefCell::new(runtime),
            bridge,
            storage,
            directory,
            asking: RefCell::default(),
            scripts: RefCell::default(),
            running: RefCell::default(),
            replies: RefCell::default(),
            panels: RefCell::default(),
            rules: Arc::default(),
            next_reply: Cell::default(),
            manager: RefCell::default(),
        })
    }

    /// Hands the extensions the window manager, once it exists.
    pub fn attach(self: &Rc<Self>, manager: Weak<WindowManager>) {
        *self.manager.borrow_mut() = manager.clone();
        self.bridge.attach(manager, Rc::downgrade(self));
    }

    fn manager(&self) -> Option<Rc<WindowManager>> {
        self.manager.borrow().upgrade()
    }

    /// The folder extensions are installed in.
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    pub fn dispatch(&self, event: &Event) {
        self.runtime.borrow_mut().dispatch(event);
    }

    /// Looks at what the workers of the extensions have to say. The workers wake the browser
    /// when they have something, so this only has to run when the browser is awake anyway.
    pub fn poll_workers(&self) {
        self.runtime.borrow_mut().poll_workers();
    }

    pub fn resume(&self, pending: PendingId, result: PendingResult) {
        self.runtime.borrow_mut().resume(pending, result);
    }

    pub fn run_command(&self, plugin: &PluginId, command: &str) {
        self.runtime.borrow_mut().run_command(plugin, command);
    }

    /// Every command of every loaded extension, for the palette.
    pub fn commands(&self) -> Vec<(PluginId, Command)> {
        self.runtime.borrow().commands()
    }

    /// The extension a key combination belongs to, if any.
    pub fn command_for_keys(&self, keys: &str) -> Option<(PluginId, String)> {
        self.commands().into_iter().find_map(|(plugin, command)| {
            (command.shortcut.as_deref() == Some(keys)).then_some((plugin, command.id))
        })
    }

    /// The rules for requests, for the filter chain of the engine.
    pub fn rules(&self) -> Arc<RuleSet> {
        self.rules.clone()
    }

    /// Tells extensions about the requests their rules caught, in one go.
    pub fn report_rule_hits(&self) {
        let hits = self.rules.take_hits();
        for hit in hits {
            let action = match hit.action {
                RuleAction::Block => "block".to_owned(),
                RuleAction::Redirect(target) => target,
            };
            self.runtime.borrow_mut().dispatch_to(
                &hit.plugin,
                &Event::RequestCaught {
                    rule: hit.rule,
                    url: hit.url,
                    kind: hit.kind,
                    action,
                },
            );
        }
    }

    /// Everything extensions want in pages, for a view that is new or was reloaded.
    pub fn content_scripts(&self) -> Vec<ContentScript> {
        self.scripts.borrow().values().cloned().collect()
    }

    /// Remembers which extension call waits for a script in a page.
    fn expect_script(&self, view: ViewId, script: ScriptId, pending: PendingId) {
        self.running.borrow_mut().insert((view, script), pending);
    }

    /// A script in a page finished, so the extension that waits for it can go on.
    pub fn script_finished(&self, view: ViewId, script: ScriptId, result: Result<Value, String>) {
        let pending = self.running.borrow_mut().remove(&(view, script));
        if let Some(pending) = pending {
            self.resume(pending, result);
        }
    }

    /// Hands a message of a page to the extensions and remembers where the answer goes. A panel
    /// only ever talks to the extension it belongs to.
    pub fn page_message(&self, view: ViewId, message: EnginePageMessage) {
        let token = self.next_reply.get() + 1;
        self.next_reply.set(token);
        self.replies
            .borrow_mut()
            .insert(token, (view, message.reply));
        let target = self
            .panels
            .borrow()
            .get(&view)
            .map(|(plugin, _)| plugin.clone());
        self.runtime.borrow_mut().dispatch_message(PageMessage {
            frame_url: message.frame_url,
            channel: message.channel,
            payload: message.payload,
            reply: token,
            target,
        });
    }

    fn remember_panel(&self, view: ViewId, plugin: &PluginId, panel: &str) {
        self.panels
            .borrow_mut()
            .insert(view, (plugin.clone(), panel.to_owned()));
    }

    fn forget_panel(&self, view: ViewId) {
        self.panels.borrow_mut().remove(&view);
    }

    /// Where an extension is installed, for the files it brings along. This reads the folders
    /// instead of asking the runtime, because extensions call it while their own code runs.
    fn folder_of(&self, plugin: &PluginId) -> Option<PathBuf> {
        atoll_plugin::installed(&self.directory)
            .into_iter()
            .find(|folder| {
                PluginRuntime::read_manifest(folder)
                    .map(|manifest| manifest.id == *plugin)
                    .unwrap_or(false)
            })
    }

    /// The panels of every loaded extension, for the palette. Not for use while extension code
    /// runs, because it asks the runtime.
    pub fn panels(&self) -> Vec<(PluginId, Panel)> {
        self.loaded()
            .into_iter()
            .flat_map(|info| {
                let id = info.id.clone();
                info.panels
                    .into_iter()
                    .map(move |panel| (id.clone(), panel))
            })
            .collect()
    }

    /// Opens a panel from the palette.
    pub fn show_panel(&self, plugin: &PluginId, panel: &str) {
        let Some(folder) = self.folder_of(plugin) else {
            return;
        };
        let Some(found) = PluginRuntime::read_manifest(&folder)
            .ok()
            .and_then(|manifest| {
                manifest
                    .panels
                    .into_iter()
                    .find(|candidate| candidate.id == panel)
            })
            .map(|panel| (plugin.clone(), panel))
        else {
            return;
        };
        if let Some(manager) = self.manager()
            && let Some(browser) = manager.focused_browser()
            && let Err(error) = browser.open_panel(plugin, &folder, &found.1)
        {
            browser.show_toast(&t!("Panel konnte nicht geöffnet werden: {}", error));
        }
    }

    pub fn loaded(&self) -> Vec<PluginInfo> {
        self.runtime.borrow().loaded()
    }

    pub fn logs(&self, plugin: &PluginId) -> Vec<String> {
        self.bridge.logs(plugin)
    }

    /// What is installed, whether it is loaded or not.
    pub fn installed(&self) -> Vec<InstalledPlugin> {
        let records = self.storage.plugins().unwrap_or_default();
        let loaded = self.loaded();
        atoll_plugin::installed(&self.directory)
            .into_iter()
            .filter_map(|folder| {
                let manifest = PluginRuntime::read_manifest(&folder).ok()?;
                let record = records
                    .iter()
                    .find(|record| record.id == manifest.id)
                    .cloned();
                let info = loaded.iter().find(|info| info.id == manifest.id).cloned();
                Some(InstalledPlugin {
                    manifest,
                    folder,
                    enabled: record.as_ref().is_some_and(|record| record.enabled),
                    known: record.is_some(),
                    source: record
                        .as_ref()
                        .map_or(Source::Folder, |record| Source::parse(&record.source)),
                    info,
                })
            })
            .collect()
    }

    /// Removes the extension, its data and its folder.
    fn remove(&self, id: &PluginId, folder: &Path) {
        self.runtime.borrow_mut().unload(id);
        if let Err(error) = self.storage.remove_plugin(id) {
            tracing::warn!(%error, plugin = %id, "the extension could not be removed");
        }
        if let Err(error) = std::fs::remove_dir_all(folder) {
            tracing::warn!(%error, plugin = %id, "the extension folder could not be removed");
        }
    }

    fn remember_script(&self, script: ContentScript) {
        self.scripts
            .borrow_mut()
            .insert(script.name.clone(), script);
    }

    fn forget_script(&self, name: &str) {
        self.scripts.borrow_mut().remove(name);
    }

    /// Sends the answer of an extension back into the page that asked.
    fn answer_page(&self, reply: u64, answer: PendingResult) {
        let Some((view, waiting)) = self.replies.borrow_mut().remove(&reply) else {
            return;
        };
        if let Some(manager) = self.manager() {
            for browser in manager.browsers() {
                if browser.plugin_answer_page(view, waiting, &answer) {
                    return;
                }
            }
        }
    }

    /// Switches an extension on or off and remembers it.
    fn set_enabled(&self, id: &PluginId, enabled: bool) {
        if let Err(error) = self.storage.set_plugin_enabled(id, enabled) {
            tracing::warn!(%error, plugin = %id, "the extension state could not be stored");
        }
        if !enabled {
            self.runtime.borrow_mut().unload(id);
        }
    }

    /// An extension broke its limits and was switched off by the runtime.
    fn remember_disabled(&self, id: &PluginId) {
        self.set_enabled(id, false);
    }

    /// Loads the extension in `folder` with the permissions the user granted.
    fn start(&self, folder: &Path, manifest: &Manifest, granted: &[Permission]) {
        let result = self.runtime.borrow_mut().load(folder, granted);
        match result {
            Ok(info) => {
                tracing::info!(plugin = %info.id, version = %info.version, "extension loaded")
            }
            Err(error) => {
                tracing::warn!(plugin = %manifest.id, %error, "extension could not be loaded");
                self.bridge.report(&manifest.id, &error.to_string());
            }
        }
    }

    /// Writes down that the user allowed the extension to do what its manifest asks for.
    fn grant(&self, manifest: &Manifest, source: &str) {
        let record = PluginRecord {
            id: manifest.id.clone(),
            version: manifest.version.clone(),
            enabled: true,
            permissions: manifest
                .listed_permissions()
                .iter()
                .map(|permission| permission.as_str().to_owned())
                .collect(),
            source: source.to_owned(),
            installed_at: Timestamp::now(),
        };
        if let Err(error) = self.storage.save_plugin(&record) {
            tracing::warn!(%error, plugin = %manifest.id, "the extension could not be stored");
        }
    }
}

/// A message a page sent, the way the engine hands it over.
pub struct EnginePageMessage {
    pub frame_url: String,
    pub channel: String,
    pub payload: Value,
    pub reply: ReplyId,
}

/// An extension folder together with what Atoll knows about it.
pub struct InstalledPlugin {
    pub manifest: Manifest,
    pub folder: PathBuf,
    pub enabled: bool,
    /// Whether the user has already been asked about it.
    pub known: bool,
    /// Where it came from, for updating it later.
    pub source: Source,
    /// What the runtime knows, as long as it is loaded.
    pub info: Option<PluginInfo>,
}

impl Browser {
    /// Hands an event to every extension that may see it.
    pub(super) fn notify_plugins(&self, event: Event) {
        self.plugins.dispatch(&event);
    }

    /// A page finished loading, which extensions of that site want to know. Private tabs stay
    /// out of this, extensions never hear about them.
    pub(super) fn page_loaded(&self, view: ViewId) {
        let Some(tab) = self.tab_of(view) else { return };
        if self.is_private(tab) {
            return;
        }
        let session = self.session.borrow();
        let Some(record) = session.tab(tab) else {
            return;
        };
        if record.is_blank() {
            return;
        }
        let event = Event::PageLoaded {
            tab: record.id.0,
            url: record.url.clone(),
            title: record.title.clone().unwrap_or_default(),
        };
        drop(session);
        self.notify_plugins(event);
    }

    /// The tab as extensions see it, for the events about it.
    fn plugin_tab(&self, tab: atoll_core::TabId) -> Option<TabInfo> {
        let session = self.session.borrow();
        let record = session.tab(tab).filter(|record| !record.private)?;
        Some(TabInfo {
            id: record.id.0,
            url: record.url.clone(),
            title: record.title.clone().unwrap_or_default(),
            active: session.active_tab == Some(record.id),
        })
    }

    pub(super) fn plugin_tab_opened(&self, tab: atoll_core::TabId) {
        if let Some(info) = self.plugin_tab(tab) {
            self.notify_plugins(Event::TabOpened(info));
        }
    }

    pub(super) fn plugin_tab_activated(&self, tab: atoll_core::TabId) {
        if let Some(info) = self.plugin_tab(tab) {
            self.notify_plugins(Event::TabActivated(info));
        }
    }

    pub(super) fn plugin_tab_closed(&self, tab: atoll_core::TabId) {
        self.notify_plugins(Event::TabClosed { tab: tab.0 });
    }

    /// Loads every installed extension. New ones are only loaded once the user allowed what they
    /// ask for.
    pub(super) fn load_plugins(&self) {
        for installed in self.plugins.installed() {
            if installed.known {
                if installed.enabled {
                    let granted = self.granted_permissions(&installed.manifest);
                    self.plugins
                        .start(&installed.folder, &installed.manifest, &granted);
                }
                continue;
            }
            self.ask_about_plugin(installed);
        }
    }

    /// The permissions the user granted, limited to what the manifest asks for today.
    fn granted_permissions(&self, manifest: &Manifest) -> Vec<Permission> {
        let stored = self
            .storage
            .plugin(&manifest.id)
            .ok()
            .flatten()
            .map(|record| record.permissions)
            .unwrap_or_default();
        manifest
            .listed_permissions()
            .into_iter()
            .filter(|permission| stored.iter().any(|name| name == permission.as_str()))
            .collect()
    }

    /// Asks whether a new extension, or one that wants more than before, may run.
    fn ask_about_plugin(&self, installed: InstalledPlugin) {
        self.ask_about(installed.manifest, installed.folder, installed.source);
    }

    /// Asks the user and starts the extension if they agree.
    fn ask_about(&self, manifest: Manifest, folder: PathBuf, source: Source) {
        if !self.plugins.asking.borrow_mut().insert(manifest.id.clone()) {
            return;
        }
        let this = self.this();
        let message = permission_request(&manifest);
        self.confirm(
            &t!("Erweiterung „{}“ zulassen?", manifest.name),
            &message,
            t!("Zulassen"),
            move || {
                if let Some(browser) = this.upgrade() {
                    browser.plugins.asking.borrow_mut().remove(&manifest.id);
                    browser.plugins.grant(&manifest, source.as_str());
                    let granted = manifest.listed_permissions();
                    browser.plugins.start(&folder, &manifest, &granted);
                    browser.refresh_plugin_shortcuts();
                    browser.show_plugin_settings();
                }
            },
        );
    }

    /// Runs the command a plugin shortcut belongs to, and says whether there was one.
    pub(super) fn run_plugin_shortcut(&self, keys: &str) -> bool {
        match self.plugins.command_for_keys(keys) {
            Some((plugin, command)) => {
                self.plugins.run_command(&plugin, &command);
                true
            }
            None => false,
        }
    }

    pub(super) fn refresh_plugin_shortcuts(&self) {
        // The palette reads the commands when it opens, so nothing has to be pushed anywhere yet.
    }

    /// Runs `code` in the page of the active tab for an extension.
    pub(super) fn plugin_evaluate(
        &self,
        pending: PendingId,
        code: &str,
        world: ScriptWorld,
    ) -> bool {
        let mut started = None;
        self.with_live_view(|view| {
            started = view
                .evaluate(code, engine_world(world))
                .map(|script| (view.id(), script));
        });
        match started {
            Some((view, script)) => {
                self.plugins.expect_script(view, script, pending);
                true
            }
            None => false,
        }
    }

    /// Puts a script of an extension into every page of this window from now on.
    pub(super) fn plugin_add_script(&self, script: &ContentScript) {
        for entry in self.views.borrow().entries() {
            entry.view.add_document_script(
                &script.name,
                &script.source,
                engine_world(script.world),
            );
        }
    }

    pub(super) fn plugin_remove_script(&self, name: &str) {
        for entry in self.views.borrow().entries() {
            entry.view.remove_document_script(name);
        }
    }

    /// Answers a message of a page or of a panel, and says whether that view belongs to this
    /// window.
    pub(super) fn plugin_answer_page(
        &self,
        view: ViewId,
        reply: ReplyId,
        answer: &PendingResult,
    ) -> bool {
        if self.is_live_panel(view) {
            self.with_live_panel(|panel| panel.answer_page(reply, answer.clone()));
            return true;
        }
        let views = self.views.borrow();
        let Some(entry) = views.entries().find(|entry| entry.view.id() == view) else {
            return false;
        };
        entry.view.answer_page(reply, answer.clone());
        true
    }

    /// Gives a new engine instance the scripts extensions want in every page.
    pub(super) fn plugin_prepare_view(&self, view: &atoll_engine::BrowserView) {
        for script in self.plugins.content_scripts() {
            view.add_document_script(&script.name, &script.source, engine_world(script.world));
        }
    }

    /// The tabs of this window as extensions see them.
    pub(super) fn plugin_tabs(&self) -> Vec<TabInfo> {
        let session = self.session.borrow();
        session
            .tabs()
            .filter(|tab| !tab.private)
            .map(|tab| TabInfo {
                id: tab.id.0,
                url: tab.url.clone(),
                title: tab.title.clone().unwrap_or_default(),
                active: session.active_tab == Some(tab.id),
            })
            .collect()
    }

    pub(super) fn plugin_open_tab(&self, url: &str, background: bool) -> Option<i64> {
        if !is_web_address(url) {
            return None;
        }
        let active = self.session.borrow().active_tab;
        self.open_in_new_tab(active, url, !background)
            .map(|tab| tab.0)
    }

    pub(super) fn plugin_close_tab(&self, tab: i64) -> Result<(), HostError> {
        let id = self.plugin_tab_id(tab)?;
        self.close_tab(id);
        Ok(())
    }

    pub(super) fn plugin_activate_tab(&self, tab: i64) -> Result<(), HostError> {
        let id = self.plugin_tab_id(tab)?;
        self.activate_tab(id);
        Ok(())
    }

    fn plugin_tab_id(&self, tab: i64) -> Result<atoll_core::TabId, HostError> {
        let id = atoll_core::TabId(tab);
        self.session
            .borrow()
            .tab(id)
            .map(|tab| tab.id)
            .ok_or(HostError::UnknownTab(tab))
    }
}

fn engine_world(world: ScriptWorld) -> atoll_engine::ScriptWorld {
    match world {
        ScriptWorld::Isolated => atoll_engine::ScriptWorld::Isolated,
        ScriptWorld::Page => atoll_engine::ScriptWorld::Page,
    }
}

/// Extensions may only open ordinary web pages.
fn is_web_address(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

/// What the user is asked before an extension runs for the first time.
fn permission_request(manifest: &Manifest) -> String {
    let mut message = if manifest.description.trim().is_empty() {
        format!("„{}“ {}", manifest.name, manifest.version)
    } else {
        format!(
            "„{}“ {}: {}",
            manifest.name,
            manifest.version,
            manifest.description.trim()
        )
    };
    let permissions = manifest.listed_permissions();
    if permissions.is_empty() {
        message.push_str(t!("\n\nDiese Erweiterung möchte keine besonderen Rechte."));
        return message;
    }
    message.push_str(t!("\n\nSie möchte:"));
    for permission in permissions {
        message.push_str(&t!("\n• {}", permission.description()));
    }
    if !manifest.hosts.is_empty() {
        let hosts: Vec<&str> = manifest
            .hosts
            .iter()
            .map(atoll_plugin::UrlPattern::as_str)
            .collect();
        message.push_str(&t!("\n\nSeiten: {}", hosts.join(", ")));
    }
    message.push_str(t!(
        "\n\nLade Erweiterungen nur aus Quellen, denen du vertraust."
    ));
    message
}

impl Browser {
    /// Settings: shows what is installed and lets the user switch it on and off.
    pub(super) fn show_plugin_settings(&self) {
        let Some(window) = self.window() else { return };
        let items: Vec<PluginItem> = self
            .plugins
            .installed()
            .into_iter()
            .map(|installed| plugin_item(&installed))
            .collect();
        let state = window.global::<SettingsState>();
        state.set_plugins(ModelRc::from(Rc::new(VecModel::from(items))));
        state.set_plugin_directory(self.plugins.directory().display().to_string().into());
    }

    /// The buttons of one extension in the settings.
    pub fn plugin_action(&self, id: &str, action: &str) {
        let id = id.to_owned();
        let Some(installed) = self
            .plugins
            .installed()
            .into_iter()
            .find(|installed| installed.manifest.id == id)
        else {
            return;
        };
        match action {
            "enable" => {
                if installed.known {
                    self.plugins.set_enabled(&id, true);
                    let granted = self.granted_permissions(&installed.manifest);
                    self.plugins
                        .start(&installed.folder, &installed.manifest, &granted);
                } else {
                    self.ask_about_plugin(installed);
                }
            }
            "disable" => self.plugins.set_enabled(&id, false),
            "reload" => {
                let granted = self.granted_permissions(&installed.manifest);
                self.plugins.set_enabled(&id, true);
                self.plugins
                    .start(&installed.folder, &installed.manifest, &granted);
                self.show_toast(&t!("„{}“ neu geladen", installed.manifest.name));
            }
            "log" => {
                let log = self.plugins.logs(&id);
                let text = if log.is_empty() {
                    t!("Diese Erweiterung hat noch nichts gemeldet.").to_owned()
                } else {
                    log.join("\n")
                };
                self.inform(&t!("Protokoll von „{}“", installed.manifest.name), &text);
            }
            "update" => self.update_plugin(installed),
            "remove" => self.remove_plugin(installed),
            other => tracing::warn!(action = other, "unknown extension action"),
        }
        self.show_plugin_settings();
    }

    /// Removes an extension after asking, together with everything it stored.
    fn remove_plugin(&self, installed: InstalledPlugin) {
        let this = self.this();
        let name = installed.manifest.name.clone();
        let id = installed.manifest.id.clone();
        let folder = installed.folder.clone();
        self.confirm(
            &t!("„{}“ entfernen?", name),
            t!("Der Ordner der Erweiterung und alles, was sie gespeichert hat, werden gelöscht."),
            t!("Entfernen"),
            move || {
                if let Some(browser) = this.upgrade() {
                    browser.plugins.remove(&id, &folder);
                    browser.show_plugin_settings();
                    browser.show_toast(&t!("„{}“ entfernt", name));
                }
            },
        );
    }

    /// Settings: the two ways to install an extension.
    pub fn install_plugin(&self, kind: &str) {
        match kind {
            "archive" => self.install_plugin_archive(),
            "git" => self.install_plugin_git(),
            other => tracing::warn!(kind = other, "unknown way to install an extension"),
        }
    }

    pub fn open_plugin_folder(&self) {
        let directory = self.plugins.directory().to_path_buf();
        let result = std::fs::create_dir_all(&directory).and_then(|()| opener::open(&directory));
        if let Err(error) = result {
            self.show_toast(&t!("Ordner kann nicht geöffnet werden: {}", error));
        }
    }
}

/// Where an extension came from, as the settings show it.
fn source_name(source: &Source) -> String {
    match source {
        Source::Folder => t!("Aus dem Erweiterungsordner").to_owned(),
        Source::Archive => t!("Aus einem Archiv installiert").to_owned(),
        Source::Git(url) => t!("Aus {}", url),
    }
}

/// One line of the extension list in the settings.
fn plugin_item(installed: &InstalledPlugin) -> PluginItem {
    let manifest = &installed.manifest;
    let permissions: Vec<&str> = manifest
        .listed_permissions()
        .iter()
        .map(|permission| permission.description())
        .collect();
    let status = match (&installed.info, installed.enabled, installed.known) {
        (Some(info), _, _) => match &info.failure {
            Some(failure) => failure.clone(),
            None => t!("Läuft, {} Befehle", info.commands.len()),
        },
        (None, true, _) => t!("Nicht geladen").to_owned(),
        (None, false, true) => t!("Ausgeschaltet").to_owned(),
        (None, false, false) => t!("Noch nicht zugelassen").to_owned(),
    };
    PluginItem {
        id: manifest.id.as_str().into(),
        source: source_name(&installed.source).into(),
        from_git: installed.source.git_url().is_some(),
        name: manifest.name.as_str().into(),
        version: manifest.version.as_str().into(),
        detail: manifest.description.as_str().into(),
        permissions: if permissions.is_empty() {
            t!("Keine besonderen Rechte").into()
        } else {
            t!("Darf: {}", permissions.join(", ")).into()
        },
        status: status.as_str().into(),
        enabled: installed.enabled,
        failed: installed
            .info
            .as_ref()
            .is_some_and(|info| info.failure.is_some()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(source: &str) -> Manifest {
        Manifest::parse(source).unwrap()
    }

    #[test]
    fn the_question_lists_what_the_extension_wants() {
        let manifest = manifest(
            r#"
            id = "demo"
            name = "Demo"
            version = "1.2"
            api = 1
            description = "Räumt auf"
            permissions = ["pages", "tabs"]
            hosts = ["*://*.example.org/*"]
            "#,
        );
        let message = permission_request(&manifest);
        assert!(message.contains("„Demo“ 1.2: Räumt auf"));
        assert!(message.contains("• die offenen Tabs sehen"));
        assert!(message.contains("Seiten: *://*.example.org/*"));
    }

    #[test]
    fn an_extension_without_permissions_says_so() {
        let message = permission_request(&manifest(
            "id = \"demo\"\nname = \"Demo\"\nversion = \"1\"\napi = 1",
        ));
        assert!(message.contains("keine besonderen Rechte"));
    }

    #[test]
    fn extensions_only_open_web_pages() {
        assert!(is_web_address("https://example.org"));
        assert!(!is_web_address("file:///etc/passwd"));
        assert!(!is_web_address("javascript:alert(1)"));
    }
}
