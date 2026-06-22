use atoll_storage::Storage;
use atoll_storage::prelude::*;

const DEFAULT_SEARCH_URL: &str = "https://www.google.com/search?q={query}";
const DEFAULT_FROZEN_LIMIT: usize = 6;

/// User preferences, stored in the settings table.
#[derive(Debug, Clone)]
pub struct Preferences {
    /// Search address with a `{query}` placeholder.
    pub search_url: String,
    /// How many recently used tabs keep their engine instance in the background.
    pub frozen_limit: usize,
}

impl Preferences {
    pub fn load(storage: &Storage) -> Self {
        let search_url = storage
            .setting("search_url")
            .ok()
            .flatten()
            .filter(|url| url.contains("{query}"))
            .unwrap_or_else(|| DEFAULT_SEARCH_URL.to_owned());
        let frozen_limit = storage
            .setting_i64("frozen_limit")
            .ok()
            .flatten()
            .and_then(|limit| usize::try_from(limit).ok())
            .unwrap_or(DEFAULT_FROZEN_LIMIT);
        Self {
            search_url,
            frozen_limit,
        }
    }
}
