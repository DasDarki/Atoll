use crate::address;
use crate::ids::{FaviconId, IslandId, TabId};
use crate::time::Timestamp;

/// A tab record. It only becomes an engine instance while it is being looked at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tab {
    pub id: TabId,
    pub island_id: IslandId,
    pub url: String,
    pub title: Option<String>,
    pub favicon_id: Option<FaviconId>,
    pub position: i64,
    pub pinned: bool,
    pub scroll_y: Option<i32>,
    pub last_used_at: Option<Timestamp>,
}

impl Tab {
    pub fn is_blank(&self) -> bool {
        self.url.trim().is_empty()
    }

    /// The text shown for the tab: its title, or the host or URL while no title is known yet.
    pub fn display_title(&self) -> String {
        if let Some(title) = self
            .title
            .as_deref()
            .map(str::trim)
            .filter(|title| !title.is_empty())
        {
            return title.to_owned();
        }
        if self.is_blank() {
            return String::from("Neuer Tab");
        }
        address::display_host(&self.url).unwrap_or_else(|| self.url.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(url: &str, title: Option<&str>) -> Tab {
        Tab {
            id: TabId(1),
            island_id: IslandId(1),
            url: url.to_owned(),
            title: title.map(str::to_owned),
            favicon_id: None,
            position: 0,
            pinned: false,
            scroll_y: None,
            last_used_at: None,
        }
    }

    #[test]
    fn display_title_prefers_title() {
        assert_eq!(
            tab("https://example.com", Some("Example")).display_title(),
            "Example"
        );
    }

    #[test]
    fn display_title_falls_back_to_host() {
        assert_eq!(
            tab("https://www.example.com/a", None).display_title(),
            "example.com"
        );
        assert_eq!(
            tab("https://example.com/a", Some("  ")).display_title(),
            "example.com"
        );
    }

    #[test]
    fn display_title_of_blank_tab() {
        assert_eq!(tab("", None).display_title(), "Neuer Tab");
    }
}
