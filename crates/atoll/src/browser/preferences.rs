use std::path::PathBuf;

use atoll_storage::Storage;
use atoll_storage::prelude::*;

const DEFAULT_FROZEN_LIMIT: usize = 6;
const DEFAULT_DEV_TOOLS_SIZE: u32 = 320;
const DEFAULT_ARCHIVE_SUGGESTION_WEEKS: u32 = 4;
pub const MAX_FROZEN_LIMIT: usize = 20;
pub const MAX_ARCHIVE_SUGGESTION_WEEKS: u32 = 52;
const DEFAULT_MEMORY_LIMIT_GB: u32 = 4;
pub const MAX_MEMORY_LIMIT_GB: u32 = 64;

/// A search provider offered in the settings.
pub struct SearchEngine {
    pub name: &'static str,
    pub url: &'static str,
}

pub const SEARCH_ENGINES: &[SearchEngine] = &[
    SearchEngine {
        name: "Google",
        url: "https://www.google.com/search?q={query}",
    },
    SearchEngine {
        name: "DuckDuckGo",
        url: "https://duckduckgo.com/?q={query}",
    },
    SearchEngine {
        name: "Bing",
        url: "https://www.bing.com/search?q={query}",
    },
    SearchEngine {
        name: "Startpage",
        url: "https://www.startpage.com/do/search?q={query}",
    },
    SearchEngine {
        name: "Ecosia",
        url: "https://www.ecosia.org/search?q={query}",
    },
    SearchEngine {
        name: "Brave Search",
        url: "https://search.brave.com/search?q={query}",
    },
];

/// Every preference the user can change, stored in the settings table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preferences {
    /// Search address with a `{query}` placeholder.
    pub search_url: String,
    /// How many recently used tabs keep their engine instance in the background.
    pub frozen_limit: usize,
    /// The page opened by the home button, empty for the Atoll start page.
    pub start_page: String,
    /// Whether new tabs load the start page instead of showing the Atoll start page.
    pub new_tab_start_page: bool,
    pub home_button: bool,
    pub sidebar_visible: bool,
    pub bookmarks_bar_visible: bool,
    /// Hides the bookmarks bar while it has no entries.
    pub bookmarks_bar_auto_hide: bool,
    /// Where downloads are saved, `None` for the system download directory.
    pub download_directory: Option<PathBuf>,
    /// Asks for a location before every download.
    pub download_ask: bool,
    /// Opens the download list when a download starts.
    pub download_panel: bool,
    /// Docks the developer tools to the right of the page instead of below it.
    pub dev_tools_docked_right: bool,
    /// Height, or width when docked right, of the developer tools in logical pixels.
    pub dev_tools_size: u32,
    /// Blocks ads and trackers.
    pub adblock_enabled: bool,
    /// Islands unused for this many weeks are suggested for the archive, `0` suggests none.
    pub archive_suggestion_weeks: u32,
    /// Puts background tabs to sleep while the pages use more memory than the budget.
    pub memory_limit_enabled: bool,
    /// The memory budget of all pages in binary gigabytes.
    pub memory_limit_gb: u32,
}

/// A single preference, used to store exactly what changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreferenceKey {
    SearchUrl,
    FrozenLimit,
    StartPage,
    NewTabStartPage,
    HomeButton,
    SidebarVisible,
    BookmarksBarVisible,
    BookmarksBarAutoHide,
    DownloadDirectory,
    DownloadAsk,
    DownloadPanel,
    DevToolsDockedRight,
    DevToolsSize,
    AdblockEnabled,
    ArchiveSuggestionWeeks,
    MemoryLimitEnabled,
    MemoryLimitGb,
}

impl PreferenceKey {
    fn storage_key(self) -> &'static str {
        match self {
            PreferenceKey::SearchUrl => "search_url",
            PreferenceKey::FrozenLimit => "frozen_limit",
            PreferenceKey::StartPage => "start_page",
            PreferenceKey::NewTabStartPage => "new_tab_start_page",
            PreferenceKey::HomeButton => "home_button",
            PreferenceKey::SidebarVisible => "sidebar_visible",
            PreferenceKey::BookmarksBarVisible => "bookmarks_bar_visible",
            PreferenceKey::BookmarksBarAutoHide => "bookmarks_bar_auto_hide",
            PreferenceKey::DownloadDirectory => "download_directory",
            PreferenceKey::DownloadAsk => "download_ask",
            PreferenceKey::DownloadPanel => "download_panel",
            PreferenceKey::DevToolsDockedRight => "dev_tools_docked_right",
            PreferenceKey::DevToolsSize => "dev_tools_size",
            PreferenceKey::AdblockEnabled => "adblock_enabled",
            PreferenceKey::ArchiveSuggestionWeeks => "archive_suggestion_weeks",
            PreferenceKey::MemoryLimitEnabled => "memory_limit_enabled",
            PreferenceKey::MemoryLimitGb => "memory_limit_gb",
        }
    }
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            search_url: SEARCH_ENGINES[0].url.to_owned(),
            frozen_limit: DEFAULT_FROZEN_LIMIT,
            start_page: String::new(),
            new_tab_start_page: false,
            home_button: false,
            sidebar_visible: true,
            bookmarks_bar_visible: true,
            bookmarks_bar_auto_hide: false,
            download_directory: None,
            download_ask: false,
            download_panel: true,
            dev_tools_docked_right: false,
            dev_tools_size: DEFAULT_DEV_TOOLS_SIZE,
            adblock_enabled: true,
            archive_suggestion_weeks: DEFAULT_ARCHIVE_SUGGESTION_WEEKS,
            memory_limit_enabled: true,
            memory_limit_gb: DEFAULT_MEMORY_LIMIT_GB,
        }
    }
}

impl Preferences {
    pub fn load(storage: &Storage) -> Self {
        let defaults = Self::default();
        let text = |key: PreferenceKey| storage.setting(key.storage_key()).ok().flatten();
        let flag =
            |key: PreferenceKey, default: bool| text(key).map_or(default, |value| value == "true");
        Self {
            search_url: text(PreferenceKey::SearchUrl)
                .filter(|url| url.contains("{query}"))
                .unwrap_or(defaults.search_url),
            frozen_limit: text(PreferenceKey::FrozenLimit)
                .and_then(|limit| limit.parse::<usize>().ok())
                .map_or(defaults.frozen_limit, |limit| limit.min(MAX_FROZEN_LIMIT)),
            start_page: text(PreferenceKey::StartPage).unwrap_or_default(),
            new_tab_start_page: flag(PreferenceKey::NewTabStartPage, defaults.new_tab_start_page),
            home_button: flag(PreferenceKey::HomeButton, defaults.home_button),
            sidebar_visible: flag(PreferenceKey::SidebarVisible, defaults.sidebar_visible),
            bookmarks_bar_visible: flag(
                PreferenceKey::BookmarksBarVisible,
                defaults.bookmarks_bar_visible,
            ),
            bookmarks_bar_auto_hide: flag(
                PreferenceKey::BookmarksBarAutoHide,
                defaults.bookmarks_bar_auto_hide,
            ),
            download_directory: text(PreferenceKey::DownloadDirectory)
                .filter(|path| !path.is_empty())
                .map(PathBuf::from),
            download_ask: flag(PreferenceKey::DownloadAsk, defaults.download_ask),
            download_panel: flag(PreferenceKey::DownloadPanel, defaults.download_panel),
            dev_tools_docked_right: flag(
                PreferenceKey::DevToolsDockedRight,
                defaults.dev_tools_docked_right,
            ),
            dev_tools_size: text(PreferenceKey::DevToolsSize)
                .and_then(|size| size.parse().ok())
                .unwrap_or(defaults.dev_tools_size),
            adblock_enabled: flag(PreferenceKey::AdblockEnabled, defaults.adblock_enabled),
            archive_suggestion_weeks: text(PreferenceKey::ArchiveSuggestionWeeks)
                .and_then(|weeks| weeks.parse::<u32>().ok())
                .map_or(defaults.archive_suggestion_weeks, |weeks| {
                    weeks.min(MAX_ARCHIVE_SUGGESTION_WEEKS)
                }),
            memory_limit_enabled: flag(
                PreferenceKey::MemoryLimitEnabled,
                defaults.memory_limit_enabled,
            ),
            memory_limit_gb: text(PreferenceKey::MemoryLimitGb)
                .and_then(|gigabytes| gigabytes.parse::<u32>().ok())
                .map_or(defaults.memory_limit_gb, |gigabytes| {
                    gigabytes.clamp(1, MAX_MEMORY_LIMIT_GB)
                }),
        }
    }

    /// Writes the value of `key` to the settings table.
    pub fn store(&self, storage: &Storage, key: PreferenceKey) {
        let flag = |value: bool| if value { "true" } else { "false" }.to_owned();
        let value = match key {
            PreferenceKey::SearchUrl => self.search_url.clone(),
            PreferenceKey::FrozenLimit => self.frozen_limit.to_string(),
            PreferenceKey::StartPage => self.start_page.clone(),
            PreferenceKey::NewTabStartPage => flag(self.new_tab_start_page),
            PreferenceKey::HomeButton => flag(self.home_button),
            PreferenceKey::SidebarVisible => flag(self.sidebar_visible),
            PreferenceKey::BookmarksBarVisible => flag(self.bookmarks_bar_visible),
            PreferenceKey::BookmarksBarAutoHide => flag(self.bookmarks_bar_auto_hide),
            PreferenceKey::DownloadDirectory => self
                .download_directory
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_default(),
            PreferenceKey::DownloadAsk => flag(self.download_ask),
            PreferenceKey::DownloadPanel => flag(self.download_panel),
            PreferenceKey::DevToolsDockedRight => flag(self.dev_tools_docked_right),
            PreferenceKey::DevToolsSize => self.dev_tools_size.to_string(),
            PreferenceKey::AdblockEnabled => flag(self.adblock_enabled),
            PreferenceKey::ArchiveSuggestionWeeks => self.archive_suggestion_weeks.to_string(),
            PreferenceKey::MemoryLimitEnabled => flag(self.memory_limit_enabled),
            PreferenceKey::MemoryLimitGb => self.memory_limit_gb.to_string(),
        };
        if let Err(error) = storage.set_setting(key.storage_key(), &value) {
            tracing::warn!(%error, key = key.storage_key(), "preference could not be stored");
        }
    }

    /// The index of the configured search engine in [`SEARCH_ENGINES`], or its length for a
    /// custom address.
    pub fn search_engine_index(&self) -> usize {
        SEARCH_ENGINES
            .iter()
            .position(|engine| engine.url == self.search_url)
            .unwrap_or(SEARCH_ENGINES.len())
    }

    pub fn has_start_page(&self) -> bool {
        !self.start_page.trim().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferences_survive_a_round_trip() {
        let storage = Storage::open_in_memory().unwrap();
        let preferences = Preferences {
            search_url: SEARCH_ENGINES[1].url.to_owned(),
            frozen_limit: 3,
            start_page: "https://example.org/".to_owned(),
            new_tab_start_page: true,
            home_button: true,
            sidebar_visible: false,
            bookmarks_bar_visible: false,
            bookmarks_bar_auto_hide: true,
            download_directory: Some(PathBuf::from("/tmp/downloads")),
            download_ask: true,
            download_panel: false,
            dev_tools_docked_right: true,
            dev_tools_size: 480,
            adblock_enabled: false,
            archive_suggestion_weeks: 9,
            memory_limit_enabled: false,
            memory_limit_gb: 12,
        };
        for key in [
            PreferenceKey::SearchUrl,
            PreferenceKey::FrozenLimit,
            PreferenceKey::StartPage,
            PreferenceKey::NewTabStartPage,
            PreferenceKey::HomeButton,
            PreferenceKey::SidebarVisible,
            PreferenceKey::BookmarksBarVisible,
            PreferenceKey::BookmarksBarAutoHide,
            PreferenceKey::DownloadDirectory,
            PreferenceKey::DownloadAsk,
            PreferenceKey::DownloadPanel,
            PreferenceKey::DevToolsDockedRight,
            PreferenceKey::DevToolsSize,
            PreferenceKey::AdblockEnabled,
            PreferenceKey::ArchiveSuggestionWeeks,
            PreferenceKey::MemoryLimitEnabled,
            PreferenceKey::MemoryLimitGb,
        ] {
            preferences.store(&storage, key);
        }
        assert_eq!(Preferences::load(&storage), preferences);
        assert_eq!(preferences.search_engine_index(), 1);
    }

    #[test]
    fn invalid_values_fall_back_to_defaults() {
        let storage = Storage::open_in_memory().unwrap();
        storage
            .set_setting("search_url", "https://example.org")
            .unwrap();
        storage.set_setting("frozen_limit", "many").unwrap();
        let preferences = Preferences::load(&storage);
        assert_eq!(preferences, Preferences::default());
        assert_eq!(preferences.search_engine_index(), 0);
    }
}
