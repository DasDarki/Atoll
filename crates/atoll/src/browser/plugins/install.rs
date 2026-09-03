//! Installing extensions from the settings: from a zip archive, from a git address, and fetching
//! what changed for those that came from git.
//!
//! The work happens on a thread of its own, because unpacking and git take their time. What comes
//! back goes through the same permission question as an extension the user put into the folder by
//! hand, unless it is already known and does not ask for more than before.

use std::path::PathBuf;

use atoll_core::Timestamp;
use atoll_plugin::{InstallError, Installed, Manifest, Source};
use atoll_storage::PluginRecord;
use atoll_storage::prelude::*;

use super::InstalledPlugin;
use crate::browser::Browser;
use crate::platform::dialogs::{self, FileDialogKind, FileDialogOptions, FileFilter};
use crate::platform::tasks::run_in_background;
use atoll_i18n::t;

impl Browser {
    /// Asks for an archive and installs what is inside it.
    pub(in crate::browser) fn install_plugin_archive(&self) {
        let this = self.this();
        dialogs::show(
            FileDialogOptions {
                kind: FileDialogKind::Open,
                title: t!("Erweiterung aus Archiv installieren").into(),
                directory: None,
                file_name: None,
                filters: vec![FileFilter {
                    name: t!("Zip-Archiv").into(),
                    extensions: vec!["zip".into()],
                }],
            },
            move |paths| {
                let Some(browser) = this.upgrade() else {
                    return;
                };
                if let Some(archive) = paths.and_then(|paths| paths.into_iter().next()) {
                    browser.install_from_archive(archive);
                }
            },
        );
    }

    fn install_from_archive(&self, archive: PathBuf) {
        let directory = self.plugins.directory().to_path_buf();
        let this = self.this();
        self.show_toast(t!("Erweiterung wird entpackt …"));
        run_in_background(
            "plugin-install",
            move || atoll_plugin::install_archive(&archive, &directory),
            move |result| {
                if let Some(browser) = this.upgrade() {
                    browser.installed_plugin(result, Source::Archive);
                }
            },
        );
    }

    /// Asks for a git address and installs what is behind it.
    pub(in crate::browser) fn install_plugin_git(&self) {
        let this = self.this();
        self.prompt_text(
            t!("Erweiterung aus Git installieren"),
            t!("Adresse des Repositorys, zum Beispiel https://example.org/erweiterung.git"),
            "",
            t!("Installieren"),
            move |url| {
                if let Some(browser) = this.upgrade() {
                    browser.install_from_git(url);
                }
            },
        );
    }

    fn install_from_git(&self, url: String) {
        let directory = self.plugins.directory().to_path_buf();
        let this = self.this();
        let source = Source::Git(url.clone());
        self.show_toast(t!("Erweiterung wird geladen …"));
        run_in_background(
            "plugin-clone",
            move || atoll_plugin::install_git(&url, &directory),
            move |result| {
                if let Some(browser) = this.upgrade() {
                    browser.installed_plugin(result, source);
                }
            },
        );
    }

    /// Fetches what changed for an extension that came from git.
    pub(super) fn update_plugin(&self, installed: InstalledPlugin) {
        let Some(url) = installed.source.git_url().map(str::to_owned) else {
            self.inform(
                &t!("„{}“ aktualisieren", installed.manifest.name),
                "Diese Erweiterung kam nicht aus Git. Installiere sie neu, um sie zu \
                 aktualisieren.",
            );
            return;
        };
        let folder = installed.folder.clone();
        let name = installed.manifest.name.clone();
        let this = self.this();
        self.show_toast(&t!("„{}“ wird aktualisiert …", name));
        run_in_background(
            "plugin-update",
            move || atoll_plugin::update_git(&folder),
            move |result| {
                let Some(browser) = this.upgrade() else {
                    return;
                };
                match result {
                    Ok(false) => browser.show_toast(&t!("„{}“ ist aktuell", name)),
                    Ok(true) => {
                        let folder = installed.folder.clone();
                        match atoll_plugin::PluginRuntime::read_manifest(&folder) {
                            Ok(manifest) => browser.installed_plugin(
                                Ok(Installed {
                                    manifest,
                                    folder,
                                    replaced: true,
                                }),
                                Source::Git(url),
                            ),
                            Err(error) => browser.plugin_install_failed(&error.to_string()),
                        }
                    }
                    Err(error) => browser.plugin_install_failed(&error.to_string()),
                }
            },
        );
    }

    /// An extension arrived: it runs right away if it is known and wants no more than before,
    /// otherwise the user is asked first.
    fn installed_plugin(&self, result: Result<Installed, InstallError>, source: Source) {
        let installed = match result {
            Ok(installed) => installed,
            Err(error) => return self.plugin_install_failed(&error.to_string()),
        };
        let name = installed.manifest.name.clone();
        match self.plugin_as_allowed(&installed.manifest) {
            Some(record) => {
                self.remember_installed(&installed.manifest, &source);
                if record.enabled {
                    let granted = self.granted_permissions(&installed.manifest);
                    self.plugins
                        .start(&installed.folder, &installed.manifest, &granted);
                }
                self.show_toast(&t!("„{}“ aktualisiert", name));
            }
            None => {
                self.show_toast(&t!("„{}“ installiert", name));
                self.ask_about(installed.manifest, installed.folder, source);
            }
        }
        self.show_plugin_settings();
    }

    /// What Atoll knows about the extension, if the user already allowed everything its new
    /// version asks for.
    fn plugin_as_allowed(&self, manifest: &Manifest) -> Option<PluginRecord> {
        let record = self.storage.plugin(&manifest.id).ok().flatten()?;
        manifest
            .listed_permissions()
            .iter()
            .all(|permission| {
                record
                    .permissions
                    .iter()
                    .any(|granted| granted == permission.as_str())
            })
            .then_some(record)
    }

    /// Writes down the new version and where it came from, leaving the permissions alone.
    fn remember_installed(&self, manifest: &Manifest, source: &Source) {
        let stored = self.storage.plugin(&manifest.id).ok().flatten();
        let record = PluginRecord {
            id: manifest.id.clone(),
            version: manifest.version.clone(),
            enabled: stored.as_ref().is_none_or(|record| record.enabled),
            permissions: stored.map(|record| record.permissions).unwrap_or_default(),
            source: source.as_str().to_owned(),
            installed_at: Timestamp::now(),
        };
        if let Err(error) = self.storage.save_plugin(&record) {
            tracing::warn!(%error, plugin = %manifest.id, "the extension could not be stored");
        }
    }

    fn plugin_install_failed(&self, error: &str) {
        self.inform(t!("Erweiterung nicht installiert"), error);
    }
}
