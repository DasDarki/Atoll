use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use adblock::Engine;
use adblock::lists::{FilterSet, ParseOptions};

/// A filter list in the Adblock Plus syntax.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilterList {
    pub name: &'static str,
    pub url: &'static str,
    file: &'static str,
}

/// The lists Atoll blocks with: ads, trackers and ads on German sites.
pub const FILTER_LISTS: &[FilterList] = &[
    FilterList {
        name: "EasyList",
        url: "https://easylist.to/easylist/easylist.txt",
        file: "easylist.txt",
    },
    FilterList {
        name: "EasyPrivacy",
        url: "https://easylist.to/easylist/easyprivacy.txt",
        file: "easyprivacy.txt",
    },
    FilterList {
        name: "EasyList Germany",
        url: "https://easylist.to/easylistgermany/easylistgermany.txt",
        file: "easylistgermany.txt",
    },
];

/// The list that hides cookie banners. It is kept apart from the blocking lists because the user
/// switches banners and advertising separately, and each set has an engine of its own.
pub const COOKIE_LISTS: &[FilterList] = &[FilterList {
    name: "EasyList Cookie List",
    url: "https://ublockorigin.github.io/uAssetsCDN/thirdparties/easylist-cookies.txt",
    file: "easylist-cookies.txt",
}];

/// File of the prepared engine, which loads much faster than the lists.
const ENGINE_FILE: &str = "engine.bin";

/// The directory the lists and the prepared engine are kept in.
pub(crate) struct ListDirectory {
    path: PathBuf,
}

impl ListDirectory {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// When the oldest list was downloaded, `None` if a list is missing.
    pub(crate) fn downloaded_at(&self, lists: &[FilterList]) -> Option<SystemTime> {
        lists
            .iter()
            .map(|list| fs::metadata(self.list_path(list)).and_then(|metadata| metadata.modified()))
            .collect::<io::Result<Vec<_>>>()
            .ok()?
            .into_iter()
            .min()
    }

    pub(crate) fn read_lists(&self, lists: &[FilterList]) -> io::Result<Vec<String>> {
        lists
            .iter()
            .map(|list| fs::read_to_string(self.list_path(list)))
            .collect()
    }

    pub(crate) fn write_list(&self, list: &FilterList, text: &str) -> io::Result<()> {
        write_atomically(&self.path, &self.list_path(list), text.as_bytes())
    }

    /// The prepared engine, `None` if there is none or it was written by another version.
    pub(crate) fn read_engine(&self) -> Option<Engine> {
        let bytes = fs::read(self.path.join(ENGINE_FILE)).ok()?;
        let mut engine = Engine::default();
        engine.deserialize(&bytes).ok()?;
        Some(engine)
    }

    pub(crate) fn write_engine(&self, engine: &Engine) -> io::Result<()> {
        write_atomically(
            &self.path,
            &self.path.join(ENGINE_FILE),
            &engine.serialize(),
        )
    }

    fn list_path(&self, list: &FilterList) -> PathBuf {
        self.path.join(list.file)
    }
}

/// Parses the filter lists into an engine.
pub(crate) fn build_engine(texts: Vec<String>) -> Engine {
    let mut filters = FilterSet::new(false);
    for text in texts {
        filters.add_filter_list(text, ParseOptions::default());
    }
    Engine::new_with_filter_set(filters)
}

/// Whether `text` looks like a filter list rather than, say, the login page of a hotel network.
pub(crate) fn is_filter_list(text: &str) -> bool {
    text.lines()
        .take(20)
        .any(|line| line.starts_with("[Adblock Plus") || line.starts_with("! Title:"))
}

fn write_atomically(directory: &Path, path: &Path, bytes: &[u8]) -> io::Result<()> {
    fs::create_dir_all(directory)?;
    let temporary = path.with_extension("part");
    fs::write(&temporary, bytes)?;
    fs::rename(&temporary, path)
}

#[cfg(test)]
mod tests {
    use adblock::request::Request;

    use super::*;

    const LIST: &str = "[Adblock Plus 2.0]\n! Title: Test\n||ads.example.net^\n";

    fn scratch(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("atoll-adblock-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        path
    }

    fn blocks(engine: &Engine, url: &str) -> bool {
        let request = Request::new(url, "https://example.org/", "script", "get").unwrap();
        engine.check_network_request(&request).should_block()
    }

    #[test]
    fn lists_and_the_prepared_engine_survive_a_round_trip() {
        let path = scratch("round-trip");
        let directory = ListDirectory::new(path.clone());
        let list = FILTER_LISTS[0];
        assert_eq!(directory.downloaded_at(&[list]), None);
        directory.write_list(&list, LIST).unwrap();
        assert!(directory.downloaded_at(&[list]).is_some());
        assert_eq!(directory.downloaded_at(FILTER_LISTS), None);

        let engine = build_engine(directory.read_lists(&[list]).unwrap());
        assert!(blocks(&engine, "https://ads.example.net/ad.js"));
        directory.write_engine(&engine).unwrap();
        let restored = directory.read_engine().unwrap();
        assert!(blocks(&restored, "https://ads.example.net/ad.js"));
        assert!(!blocks(&restored, "https://example.org/app.js"));
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn broken_engine_files_are_ignored() {
        let path = scratch("broken");
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join(ENGINE_FILE), b"not an engine").unwrap();
        assert!(ListDirectory::new(path.clone()).read_engine().is_none());
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn only_filter_lists_are_accepted() {
        assert!(is_filter_list(LIST));
        assert!(is_filter_list("! Title: EasyPrivacy\n||tracker.example^"));
        assert!(!is_filter_list("<!doctype html><title>Anmeldung</title>"));
    }
}
