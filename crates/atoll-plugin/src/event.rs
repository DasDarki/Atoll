//! What extensions can react to.

use serde_json::{Value, json};

use crate::host::TabInfo;
use crate::manifest::Permission;

/// Something happened in the browser that extensions may want to know about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    TabOpened(TabInfo),
    TabClosed {
        tab: i64,
    },
    TabActivated(TabInfo),
    /// A page finished loading in the tab.
    PageLoaded {
        tab: i64,
        url: String,
        title: String,
    },
    DownloadFinished {
        file_name: String,
        url: String,
        path: Option<String>,
    },
}

impl Event {
    /// The name extensions use in `atoll.on`.
    pub fn name(&self) -> &'static str {
        match self {
            Event::TabOpened(_) => "tab.opened",
            Event::TabClosed { .. } => "tab.closed",
            Event::TabActivated(_) => "tab.activated",
            Event::PageLoaded { .. } => "page.loaded",
            Event::DownloadFinished { .. } => "download.finished",
        }
    }

    /// What the handler is given.
    pub fn payload(&self) -> Value {
        match self {
            Event::TabOpened(tab) | Event::TabActivated(tab) => json!(tab),
            Event::TabClosed { tab } => json!({ "id": tab }),
            Event::PageLoaded { tab, url, title } => {
                json!({ "tab": tab, "url": url, "title": title })
            }
            Event::DownloadFinished {
                file_name,
                url,
                path,
            } => json!({ "fileName": file_name, "url": url, "path": path }),
        }
    }

    /// What an extension needs to be allowed to see it.
    pub fn permission(&self) -> Permission {
        match self {
            Event::TabOpened(_) | Event::TabClosed { .. } | Event::TabActivated(_) => {
                Permission::Tabs
            }
            Event::PageLoaded { .. } => Permission::Pages,
            Event::DownloadFinished { .. } => Permission::Downloads,
        }
    }

    /// The address the event is about, which is checked against the extension's `hosts`.
    pub fn url(&self) -> Option<&str> {
        match self {
            Event::TabOpened(tab) | Event::TabActivated(tab) => Some(&tab.url),
            Event::PageLoaded { url, .. } => Some(url),
            Event::TabClosed { .. } | Event::DownloadFinished { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_carry_their_name_permission_and_address() {
        let page = Event::PageLoaded {
            tab: 7,
            url: "https://example.org/a".to_owned(),
            title: "A".to_owned(),
        };
        assert_eq!(page.name(), "page.loaded");
        assert_eq!(page.permission(), Permission::Pages);
        assert_eq!(page.url(), Some("https://example.org/a"));
        assert_eq!(page.payload()["tab"], json!(7));

        let download = Event::DownloadFinished {
            file_name: "a.zip".to_owned(),
            url: "https://example.org/a.zip".to_owned(),
            path: None,
        };
        assert_eq!(download.permission(), Permission::Downloads);
        assert_eq!(download.url(), None, "downloads are not about a site");
        assert_eq!(download.payload()["fileName"], json!("a.zip"));
    }
}
