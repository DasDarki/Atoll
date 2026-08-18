//! Extensions: loading them, asking the user about their permissions and connecting them to the
//! window. The Luau side lives in `atoll-plugin`, this is the browser side of its [`PluginHost`].

mod bridge;

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};

use atoll_core::Timestamp;
use atoll_plugin::{
    Command, Event, HostError, Manifest, PendingId, PendingResult, Permission, PluginHost,
    PluginId, PluginInfo, PluginRuntime, TabInfo,
};
use atoll_storage::prelude::*;
use atoll_storage::{PluginRecord, Storage};
use slint::{ComponentHandle, ModelRc, VecModel};

use self::bridge::Bridge;
use super::Browser;
use crate::browser::windows::WindowManager;
use crate::platform::opener;
use crate::ui::{PluginItem, SettingsState};

/// The extensions of this Atoll, shared by every window.
pub struct Plugins {
    runtime: RefCell<PluginRuntime>,
    bridge: Rc<Bridge>,
    storage: Rc<Storage>,
    directory: PathBuf,
    /// Extensions that asked the user and are waiting for the answer.
    asking: RefCell<BTreeSet<PluginId>>,
}

impl Plugins {
    pub fn new(storage: Rc<Storage>, directory: PathBuf) -> Rc<Self> {
        let bridge = Rc::new(Bridge::new(storage.clone()));
        Rc::new(Self {
            runtime: RefCell::new(PluginRuntime::new(bridge.clone())),
            bridge,
            storage,
            directory,
            asking: RefCell::default(),
        })
    }

    /// Hands the extensions the window manager, once it exists.
    pub fn attach(self: &Rc<Self>, manager: Weak<WindowManager>) {
        self.bridge.attach(manager, Rc::downgrade(self));
    }

    /// The folder extensions are installed in.
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    pub fn dispatch(&self, event: &Event) {
        self.runtime.borrow_mut().dispatch(event);
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

/// An extension folder together with what Atoll knows about it.
pub struct InstalledPlugin {
    pub manifest: Manifest,
    pub folder: PathBuf,
    pub enabled: bool,
    /// Whether the user has already been asked about it.
    pub known: bool,
    /// What the runtime knows, as long as it is loaded.
    pub info: Option<PluginInfo>,
}

impl Browser {
    /// Hands an event to every extension that may see it.
    pub(super) fn notify_plugins(&self, event: Event) {
        self.plugins.dispatch(&event);
    }

    /// A page finished loading, which extensions of that site want to know.
    pub(super) fn page_loaded(&self, view: atoll_engine::ViewId) {
        let Some(tab) = self.tab_of(view) else { return };
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
        let record = session.tab(tab)?;
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
        if !self
            .plugins
            .asking
            .borrow_mut()
            .insert(installed.manifest.id.clone())
        {
            return;
        }
        let this = self.this();
        let manifest = installed.manifest;
        let folder = installed.folder;
        let message = permission_request(&manifest);
        self.confirm(
            &format!("Erweiterung „{}“ zulassen?", manifest.name),
            &message,
            "Zulassen",
            move || {
                if let Some(browser) = this.upgrade() {
                    browser.plugins.asking.borrow_mut().remove(&manifest.id);
                    browser.plugins.grant(&manifest, "folder");
                    let granted = manifest.listed_permissions();
                    browser.plugins.start(&folder, &manifest, &granted);
                    browser.refresh_plugin_shortcuts();
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

    /// The tabs of this window as extensions see them.
    pub(super) fn plugin_tabs(&self) -> Vec<TabInfo> {
        let session = self.session.borrow();
        session
            .tabs()
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
        message.push_str("\n\nDiese Erweiterung möchte keine besonderen Rechte.");
        return message;
    }
    message.push_str("\n\nSie möchte:");
    for permission in permissions {
        message.push_str(&format!("\n• {}", permission.description()));
    }
    if !manifest.hosts.is_empty() {
        let hosts: Vec<&str> = manifest
            .hosts
            .iter()
            .map(atoll_plugin::UrlPattern::as_str)
            .collect();
        message.push_str(&format!("\n\nSeiten: {}", hosts.join(", ")));
    }
    message.push_str("\n\nLade Erweiterungen nur aus Quellen, denen du vertraust.");
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
                self.show_toast(&format!("„{}“ neu geladen", installed.manifest.name));
            }
            "log" => {
                let log = self.plugins.logs(&id);
                let text = if log.is_empty() {
                    "Diese Erweiterung hat noch nichts gemeldet.".to_owned()
                } else {
                    log.join("\n")
                };
                self.inform(
                    &format!("Protokoll von „{}“", installed.manifest.name),
                    &text,
                );
            }
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
            &format!("„{name}“ entfernen?"),
            "Der Ordner der Erweiterung und alles, was sie gespeichert hat, werden gelöscht.",
            "Entfernen",
            move || {
                if let Some(browser) = this.upgrade() {
                    browser.plugins.remove(&id, &folder);
                    browser.show_plugin_settings();
                    browser.show_toast(&format!("„{name}“ entfernt"));
                }
            },
        );
    }

    pub fn open_plugin_folder(&self) {
        let directory = self.plugins.directory().to_path_buf();
        let result = std::fs::create_dir_all(&directory).and_then(|()| opener::open(&directory));
        if let Err(error) = result {
            self.show_toast(&format!("Ordner kann nicht geöffnet werden: {error}"));
        }
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
            None => format!("Läuft, {} Befehle", info.commands.len()),
        },
        (None, true, _) => "Nicht geladen".to_owned(),
        (None, false, true) => "Ausgeschaltet".to_owned(),
        (None, false, false) => "Noch nicht zugelassen".to_owned(),
    };
    PluginItem {
        id: manifest.id.as_str().into(),
        name: manifest.name.as_str().into(),
        version: manifest.version.as_str().into(),
        detail: manifest.description.as_str().into(),
        permissions: if permissions.is_empty() {
            "Keine besonderen Rechte".into()
        } else {
            format!("Darf: {}", permissions.join(", ")).into()
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
