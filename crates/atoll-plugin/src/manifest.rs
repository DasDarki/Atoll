//! What an extension says about itself in its `plugin.toml`.

use std::collections::BTreeSet;
use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::pattern::UrlPattern;
use atoll_i18n::t;

/// The extension interface this version of Atoll offers. An extension names the version it was
/// written for and is refused if Atoll does not offer it.
pub const API_VERSION: u32 = 1;

/// The file every extension folder has.
pub const MANIFEST_NAME: &str = "plugin.toml";

/// What an extension is allowed to do. Everything an extension wants is in its manifest, and the
/// user confirms it before the extension runs for the first time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Permission {
    /// Read the open tabs, open and close them.
    Tabs,
    /// Put styles and scripts into pages of the named sites and read their content.
    Pages,
    /// Block or redirect requests.
    Requests,
    /// Keep its own settings and data.
    Storage,
    /// Ask the named sites for data on its own.
    Network,
    /// Read and write the clipboard.
    Clipboard,
    /// See the downloads and start new ones.
    Downloads,
    /// Read and write files in its own folder.
    Files,
}

impl Permission {
    /// What the user sees when Atoll asks whether the extension may do this.
    pub fn description(self) -> &'static str {
        match self {
            Permission::Tabs => t!("die offenen Tabs sehen, Tabs öffnen und schließen"),
            Permission::Pages => {
                t!("Styles und Skripte in die genannten Seiten einfügen und deren Inhalt lesen")
            }
            Permission::Requests => t!("Anfragen blockieren und umleiten"),
            Permission::Storage => t!("eigene Einstellungen und Daten speichern"),
            Permission::Network => t!("die genannten Seiten selbst abfragen"),
            Permission::Clipboard => t!("die Zwischenablage lesen und schreiben"),
            Permission::Downloads => t!("Downloads sehen und starten"),
            Permission::Files => t!("Dateien im eigenen Ordner lesen und schreiben"),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Permission::Tabs => "tabs",
            Permission::Pages => "pages",
            Permission::Requests => "requests",
            Permission::Storage => "storage",
            Permission::Network => "network",
            Permission::Clipboard => "clipboard",
            Permission::Downloads => "downloads",
            Permission::Files => "files",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        [
            Permission::Tabs,
            Permission::Pages,
            Permission::Requests,
            Permission::Storage,
            Permission::Network,
            Permission::Clipboard,
            Permission::Downloads,
            Permission::Files,
        ]
        .into_iter()
        .find(|permission| permission.as_str() == text)
    }

    /// Whether this permission only applies to the sites named in `hosts`.
    pub fn needs_hosts(self) -> bool {
        matches!(self, Permission::Pages | Permission::Network)
    }
}

impl fmt::Display for Permission {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Where a panel of an extension is shown.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PanelLocation {
    /// Next to the page, like the developer tools.
    #[default]
    Side,
    /// Over the page, closed with Escape.
    Overlay,
}

/// A page an extension brings along and Atoll shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Panel {
    pub id: String,
    pub title: String,
    /// The HTML file inside the extension folder.
    pub page: String,
    #[serde(default)]
    pub location: PanelLocation,
}

/// A `plugin.toml`, already checked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    /// The extension interface the extension was written for.
    pub api: u32,
    #[serde(default = "default_entry")]
    pub entry: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub permissions: Vec<Permission>,
    #[serde(default)]
    pub hosts: Vec<UrlPattern>,
    #[serde(default, rename = "panel")]
    pub panels: Vec<Panel>,
}

fn default_entry() -> String {
    "main.luau".to_owned()
}

/// Why a manifest was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ManifestError {
    #[error("the manifest could not be read: {0}")]
    Unreadable(String),
    #[error(
        "the extension needs the Atoll extension interface {wanted}, this Atoll offers {offered}"
    )]
    UnknownApi { wanted: u32, offered: u32 },
    #[error(
        "\"{0}\" is not an extension name, use lower case letters, digits and hyphens (2 to 64)"
    )]
    InvalidId(String),
    #[error("{0} has to be filled in")]
    Missing(&'static str),
    #[error("\"{0}\" has to be a file inside the extension folder")]
    InvalidPath(String),
    #[error("the permission \"{0}\" needs at least one entry under \"hosts\"")]
    MissingHosts(Permission),
    #[error("two panels are named \"{0}\"")]
    DuplicatePanel(String),
}

impl Manifest {
    /// Reads and checks a `plugin.toml`.
    pub fn parse(text: &str) -> Result<Self, ManifestError> {
        let manifest: Manifest =
            toml::from_str(text).map_err(|error| ManifestError::Unreadable(clean(&error)))?;
        manifest.check()?;
        Ok(manifest)
    }

    fn check(&self) -> Result<(), ManifestError> {
        if self.api != API_VERSION {
            return Err(ManifestError::UnknownApi {
                wanted: self.api,
                offered: API_VERSION,
            });
        }
        if !is_id(&self.id) {
            return Err(ManifestError::InvalidId(self.id.clone()));
        }
        if self.name.trim().is_empty() {
            return Err(ManifestError::Missing("name"));
        }
        if self.version.trim().is_empty() {
            return Err(ManifestError::Missing("version"));
        }
        check_path(&self.entry)?;
        let mut names = BTreeSet::new();
        for panel in &self.panels {
            check_path(&panel.page)?;
            if panel.id.trim().is_empty() {
                return Err(ManifestError::Missing("the name of a panel"));
            }
            if !names.insert(&panel.id) {
                return Err(ManifestError::DuplicatePanel(panel.id.clone()));
            }
        }
        if self.hosts.is_empty()
            && let Some(permission) = self
                .permissions
                .iter()
                .copied()
                .find(|permission| permission.needs_hosts())
        {
            return Err(ManifestError::MissingHosts(permission));
        }
        Ok(())
    }

    pub fn allows(&self, permission: Permission) -> bool {
        self.permissions.contains(&permission)
    }

    /// Whether the extension asked for the site `url`.
    pub fn covers(&self, url: &str) -> bool {
        self.hosts.iter().any(|pattern| pattern.matches(url))
    }

    /// The permissions in the order they are shown, without repetitions.
    pub fn listed_permissions(&self) -> Vec<Permission> {
        let mut permissions: Vec<Permission> = self.permissions.clone();
        permissions.sort_unstable();
        permissions.dedup();
        permissions
    }
}

/// Extension names are used as folder names and database keys, so they stay simple.
fn is_id(id: &str) -> bool {
    (2..=64).contains(&id.len())
        && id.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
        && !id.starts_with('-')
        && !id.ends_with('-')
}

/// Paths in a manifest point into the extension folder and nowhere else.
fn check_path(path: &str) -> Result<(), ManifestError> {
    let relative = Path::new(path);
    let plain = relative
        .components()
        .all(|component| matches!(component, std::path::Component::Normal(_)));
    if path.is_empty() || !plain {
        return Err(ManifestError::InvalidPath(path.to_owned()));
    }
    Ok(())
}

/// The first line of a `toml` error, without the pretty frame around it.
fn clean(error: &toml::de::Error) -> String {
    error
        .message()
        .lines()
        .next()
        .unwrap_or("unknown error")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: &str = r#"
        id = "reddit-tweaks"
        name = "Reddit Tweaks"
        version = "1.0.0"
        api = 1
        description = "Räumt Reddit auf"
        permissions = ["pages", "storage"]
        hosts = ["*://*.reddit.com/*"]

        [[panel]]
        id = "sidebar"
        title = "Reddit Tweaks"
        page = "panel.html"
        location = "overlay"
    "#;

    #[test]
    fn a_full_manifest_is_read() {
        let manifest = Manifest::parse(FULL).unwrap();
        assert_eq!(manifest.id, "reddit-tweaks");
        assert_eq!(manifest.entry, "main.luau", "the entry has a default");
        assert_eq!(
            manifest.permissions,
            [Permission::Pages, Permission::Storage]
        );
        assert!(manifest.covers("https://www.reddit.com/r/rust"));
        assert!(!manifest.covers("https://example.org/"));
        assert_eq!(manifest.panels[0].location, PanelLocation::Overlay);
        assert_eq!(manifest.panels[0].page, "panel.html");
    }

    #[test]
    fn panels_default_to_the_side() {
        let manifest = Manifest::parse(
            r#"
            id = "notes"
            name = "Notizen"
            version = "0.1"
            api = 1
            [[panel]]
            id = "notes"
            title = "Notizen"
            page = "notes.html"
            "#,
        )
        .unwrap();
        assert_eq!(manifest.panels[0].location, PanelLocation::Side);
        assert!(manifest.permissions.is_empty());
    }

    #[test]
    fn an_extension_for_another_interface_is_refused() {
        let manifest = FULL.replace("api = 1", "api = 2");
        assert_eq!(
            Manifest::parse(&manifest),
            Err(ManifestError::UnknownApi {
                wanted: 2,
                offered: 1
            })
        );
    }

    #[test]
    fn names_stay_simple_and_paths_stay_inside() {
        let broken = FULL.replace("\"reddit-tweaks\"", "\"Reddit Tweaks!\"");
        assert!(matches!(
            Manifest::parse(&broken),
            Err(ManifestError::InvalidId(_))
        ));
        let escaping = FULL.replace("[[panel]]", "entry = \"../../evil.luau\"\n[[panel]]");
        assert_eq!(
            Manifest::parse(&escaping),
            Err(ManifestError::InvalidPath("../../evil.luau".to_owned()))
        );
    }

    #[test]
    fn page_and_network_permissions_need_hosts() {
        let without = FULL.replace("hosts = [\"*://*.reddit.com/*\"]", "hosts = []");
        assert_eq!(
            Manifest::parse(&without),
            Err(ManifestError::MissingHosts(Permission::Pages))
        );
    }

    #[test]
    fn two_panels_with_one_name_are_refused() {
        let twice =
            format!("{FULL}\n[[panel]]\nid = \"sidebar\"\ntitle = \"B\"\npage = \"b.html\"");
        assert_eq!(
            Manifest::parse(&twice),
            Err(ManifestError::DuplicatePanel("sidebar".to_owned()))
        );
    }

    #[test]
    fn broken_toml_and_unknown_permissions_are_explained() {
        assert!(matches!(
            Manifest::parse("id = "),
            Err(ManifestError::Unreadable(_))
        ));
        let unknown = FULL.replace("\"storage\"", "\"everything\"");
        assert!(matches!(
            Manifest::parse(&unknown),
            Err(ManifestError::Unreadable(_))
        ));
        assert_eq!(Permission::parse("tabs"), Some(Permission::Tabs));
        assert_eq!(Permission::parse("nope"), None);
    }
}
