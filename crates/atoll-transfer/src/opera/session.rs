//! Reading the open tabs of Opera out of its session file.
//!
//! Opera keeps them the way Chromium does: a file of length prefixed commands, one per change,
//! replayed in order to get the final state. The commands this needs are the ones that say which
//! window and place a tab has, what it last showed, whether it was closed, and which tab island
//! it belongs to. Everything else is skipped, and an unknown or broken command ends the reading
//! instead of failing the import, because the tail of such a file is often half written.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::opera::OperaError;

/// The marker at the start of a session file.
const MAGIC: &[u8] = b"SNSS";

/// Commands of Chromium's session format that say something about a tab.
const SET_TAB_WINDOW: u8 = 0;
const SET_TAB_INDEX: u8 = 2;
const UPDATE_TAB_NAVIGATION: u8 = 6;
const SET_SELECTED_NAVIGATION: u8 = 7;
const SET_PINNED_STATE: u8 = 12;
const TAB_CLOSED: u8 = 16;
const WINDOW_CLOSED: u8 = 17;
const SET_TAB_GROUP: u8 = 25;
const SET_TAB_GROUP_METADATA: u8 = 27;

/// A tab island of Opera with the addresses it holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OperaIsland {
    /// What Opera calls the island. Islands the user never named have none.
    pub name: Option<String>,
    pub urls: Vec<String>,
}

impl OperaIsland {
    /// What the island is called in Atoll. Opera lets islands stay unnamed, those are named
    /// after the site most of their tabs belong to.
    pub fn island_name(&self) -> String {
        if let Some(name) = self
            .name
            .as_ref()
            .map(|name| name.trim())
            .filter(|name| !name.is_empty())
        {
            return name.to_owned();
        }
        let mut counts: Vec<(String, usize)> = Vec::new();
        for url in &self.urls {
            let Some(site) = site_of(url) else { continue };
            match counts.iter_mut().find(|(known, _)| *known == site) {
                Some((_, count)) => *count += 1,
                None => counts.push((site, 1)),
            }
        }
        counts
            .into_iter()
            .max_by_key(|(_, count)| *count)
            .map(|(site, _)| site)
            .unwrap_or_else(|| String::from("Aus Opera"))
    }
}

#[derive(Debug, Default, Clone)]
struct Tab {
    window: Option<u32>,
    index: i32,
    group: Option<[u8; 16]>,
    selected: Option<i32>,
    pinned: bool,
    /// What the tab showed, by the place in its own back and forward list.
    pages: BTreeMap<i32, String>,
}

impl Tab {
    /// The address the tab was last on.
    fn url(&self) -> Option<&String> {
        self.selected
            .and_then(|index| self.pages.get(&index))
            .or_else(|| self.pages.values().next_back())
    }
}

/// The newest session file of an Opera profile.
pub fn newest_session(directory: &Path) -> Option<PathBuf> {
    let mut sessions: Vec<PathBuf> = std::fs::read_dir(directory.join("Sessions"))
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("Session_"))
        })
        .collect();
    sessions.sort();
    sessions.pop()
}

/// Reads the tabs of a session file, grouped the way Opera grouped them.
pub fn read_session(path: &Path) -> Result<Vec<OperaIsland>, OperaError> {
    let data = std::fs::read(path).map_err(|error| OperaError::Unreadable(error.to_string()))?;
    if data.len() < 8 || &data[..4] != MAGIC {
        return Err(OperaError::NotASession);
    }
    Ok(islands(&collect(&data[8..])))
}

/// Replays the commands of a session into the tabs they describe.
fn collect(mut rest: &[u8]) -> Tabs {
    let mut tabs: BTreeMap<u32, Tab> = BTreeMap::new();
    let mut names: BTreeMap<[u8; 16], String> = BTreeMap::new();
    let mut order: Vec<[u8; 16]> = Vec::new();
    let mut closed_windows: Vec<u32> = Vec::new();
    while rest.len() >= 3 {
        let size = u16::from_le_bytes([rest[0], rest[1]]) as usize;
        if size == 0 || rest.len() < size + 2 {
            break;
        }
        let command = rest[2];
        let payload = &rest[3..size + 2];
        rest = &rest[size + 2..];
        match command {
            SET_TAB_WINDOW => {
                if let (Some(window), Some(tab)) = (number(payload, 0), number(payload, 4)) {
                    tabs.entry(tab).or_default().window = Some(window);
                }
            }
            SET_TAB_INDEX => {
                if let (Some(tab), Some(index)) = (number(payload, 0), number(payload, 4)) {
                    tabs.entry(tab).or_default().index = index as i32;
                }
            }
            SET_SELECTED_NAVIGATION => {
                if let (Some(tab), Some(index)) = (number(payload, 0), number(payload, 4)) {
                    tabs.entry(tab).or_default().selected = Some(index as i32);
                }
            }
            SET_PINNED_STATE => {
                if let (Some(tab), Some(pinned)) = (number(payload, 0), number(payload, 4)) {
                    tabs.entry(tab).or_default().pinned = pinned != 0;
                }
            }
            TAB_CLOSED => {
                if let Some(tab) = number(payload, 0) {
                    tabs.remove(&tab);
                }
            }
            WINDOW_CLOSED => {
                if let Some(window) = number(payload, 0) {
                    closed_windows.push(window);
                }
            }
            SET_TAB_GROUP => {
                if let (Some(tab), Some(token)) = (number(payload, 0), token(payload, 8)) {
                    let has_group = number(payload, 24).unwrap_or(1) != 0;
                    tabs.entry(tab).or_default().group = has_group.then_some(token);
                }
            }
            SET_TAB_GROUP_METADATA => {
                if let Some(token) = token(payload, 4)
                    && let Some(name) = text(payload, 20)
                    && !name.trim().is_empty()
                {
                    names.insert(token, name);
                }
                if let Some(token) = token(payload, 4)
                    && !order.contains(&token)
                {
                    order.push(token);
                }
            }
            UPDATE_TAB_NAVIGATION => {
                if let Some((tab, index, url)) = navigation(payload) {
                    tabs.entry(tab).or_default().pages.insert(index, url);
                }
            }
            _ => {}
        }
    }
    tabs.retain(|_, tab| {
        tab.window
            .is_none_or(|window| !closed_windows.contains(&window))
    });
    Tabs { tabs, names, order }
}

struct Tabs {
    tabs: BTreeMap<u32, Tab>,
    names: BTreeMap<[u8; 16], String>,
    /// The islands in the order Opera wrote them down.
    order: Vec<[u8; 16]>,
}

/// Sorts the tabs into their islands, keeping the order of the windows and of the islands.
fn islands(collected: &Tabs) -> Vec<OperaIsland> {
    let mut sorted: Vec<&Tab> = collected.tabs.values().collect();
    sorted.sort_by_key(|tab| (tab.window.unwrap_or(u32::MAX), tab.index, !tab.pinned));
    let mut loose = OperaIsland::default();
    let mut grouped: BTreeMap<[u8; 16], OperaIsland> = BTreeMap::new();
    for tab in sorted {
        let Some(url) = tab.url().filter(|url| importable(url)) else {
            continue;
        };
        match tab.group {
            Some(token) => {
                let island = grouped.entry(token).or_insert_with(|| OperaIsland {
                    name: collected.names.get(&token).cloned(),
                    urls: Vec::new(),
                });
                island.urls.push(url.clone());
            }
            None => loose.urls.push(url.clone()),
        }
    }
    let mut islands = Vec::new();
    for token in &collected.order {
        if let Some(island) = grouped.remove(token) {
            islands.push(island);
        }
    }
    islands.extend(grouped.into_values());
    if !loose.urls.is_empty() {
        islands.push(loose);
    }
    islands
}

/// The site an address belongs to, which is its host without any subdomains. It only names an
/// island, so the simple rule of the last two parts is good enough.
fn site_of(url: &str) -> Option<String> {
    let host = atoll_core::address::host_of(url)?;
    let parts: Vec<&str> = host.split('.').collect();
    if parts.len() <= 2 {
        return Some(host);
    }
    Some(parts[parts.len() - 2..].join("."))
}

/// Whether the address is one Atoll can open. Opera's own pages are left behind.
fn importable(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// One navigation of a tab: which tab, which place in its list, and the address.
fn navigation(payload: &[u8]) -> Option<(u32, i32, String)> {
    let tab = number(payload, 4)?;
    let index = number(payload, 8)? as i32;
    let length = number(payload, 12)? as usize;
    let start = 16usize;
    let url = payload.get(start..start.checked_add(length)?)?;
    Some((tab, index, String::from_utf8(url.to_vec()).ok()?))
}

fn number(payload: &[u8], at: usize) -> Option<u32> {
    let bytes = payload.get(at..at + 4)?;
    Some(u32::from_le_bytes(bytes.try_into().ok()?))
}

fn token(payload: &[u8], at: usize) -> Option<[u8; 16]> {
    payload.get(at..at + 16)?.try_into().ok()
}

/// A string of two byte characters, as Chromium writes titles and island names.
fn text(payload: &[u8], at: usize) -> Option<String> {
    let length = number(payload, at)? as usize;
    let start = at + 4;
    let bytes = payload.get(start..start.checked_add(length.checked_mul(2)?)?)?;
    let characters: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    String::from_utf16(&characters).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a session file the way Chromium writes one.
    #[derive(Default)]
    struct Session {
        bytes: Vec<u8>,
    }

    impl Session {
        fn new() -> Self {
            let mut bytes = MAGIC.to_vec();
            bytes.extend(3i32.to_le_bytes());
            Self { bytes }
        }

        fn command(&mut self, id: u8, payload: &[u8]) -> &mut Self {
            let size = (payload.len() + 1) as u16;
            self.bytes.extend(size.to_le_bytes());
            self.bytes.push(id);
            self.bytes.extend(payload);
            self
        }

        fn tab(&mut self, tab: u32, window: u32, index: u32, url: &str) -> &mut Self {
            let mut navigation = Vec::new();
            navigation.extend(0u32.to_le_bytes());
            navigation.extend(tab.to_le_bytes());
            navigation.extend(0u32.to_le_bytes());
            navigation.extend((url.len() as u32).to_le_bytes());
            navigation.extend(url.as_bytes());
            while !navigation.len().is_multiple_of(4) {
                navigation.push(0);
            }
            self.command(
                SET_TAB_WINDOW,
                &[window.to_le_bytes(), tab.to_le_bytes()].concat(),
            );
            self.command(
                SET_TAB_INDEX,
                &[tab.to_le_bytes(), index.to_le_bytes()].concat(),
            );
            self.command(UPDATE_TAB_NAVIGATION, &navigation);
            self
        }

        fn group(&mut self, tab: u32, token: [u8; 16]) -> &mut Self {
            let mut payload = tab.to_le_bytes().to_vec();
            payload.extend(0u32.to_le_bytes());
            payload.extend(token);
            payload.extend(1u32.to_le_bytes());
            payload.extend(0u32.to_le_bytes());
            self.command(SET_TAB_GROUP, &payload)
        }

        fn name(&mut self, token: [u8; 16], name: &str) -> &mut Self {
            let mut payload = 0u32.to_le_bytes().to_vec();
            payload.extend(token);
            let characters: Vec<u8> = name
                .encode_utf16()
                .flat_map(|character| character.to_le_bytes())
                .collect();
            payload.extend((name.encode_utf16().count() as u32).to_le_bytes());
            payload.extend(&characters);
            while !payload.len().is_multiple_of(4) {
                payload.push(0);
            }
            self.command(SET_TAB_GROUP_METADATA, &payload)
        }
    }

    fn token(number: u8) -> [u8; 16] {
        [number; 16]
    }

    #[test]
    fn tabs_keep_their_island_and_their_order() {
        let mut session = Session::new();
        session.name(token(1), "Arbeit");
        session.tab(10, 1, 0, "https://a.example/");
        session.group(10, token(1));
        session.tab(11, 1, 1, "https://b.example/");
        session.tab(12, 1, 2, "https://c.example/");
        session.group(12, token(1));
        let data = session.bytes;

        let islands = islands(&collect(&data[8..]));
        assert_eq!(islands.len(), 2);
        assert_eq!(islands[0].name.as_deref(), Some("Arbeit"));
        assert_eq!(
            islands[0].urls,
            vec!["https://a.example/", "https://c.example/"]
        );
        assert_eq!(
            islands[1].name, None,
            "loose tabs land in an island of their own"
        );
        assert_eq!(islands[1].urls, vec!["https://b.example/"]);
    }

    #[test]
    fn an_island_without_a_name_is_named_after_its_site() {
        let island = OperaIsland {
            name: None,
            urls: vec![
                "https://www.reddit.com/r/rust".to_owned(),
                "https://old.reddit.com/r/rust".to_owned(),
                "https://example.org/".to_owned(),
            ],
        };
        assert_eq!(island.island_name(), "reddit.com");
        assert_eq!(
            OperaIsland {
                name: Some("  Arbeit ".to_owned()),
                urls: Vec::new(),
            }
            .island_name(),
            "Arbeit"
        );
        assert_eq!(
            OperaIsland::default().island_name(),
            "Aus Opera",
            "an island without addresses still needs a name"
        );
    }

    #[test]
    fn closed_tabs_and_pages_of_opera_itself_stay_behind() {
        let mut session = Session::new();
        session.tab(10, 1, 0, "https://a.example/");
        session.tab(11, 1, 1, "https://b.example/");
        session.tab(12, 1, 2, "opera://startpage");
        session.command(
            TAB_CLOSED,
            &[11u32.to_le_bytes(), 0u32.to_le_bytes()].concat(),
        );
        let data = session.bytes;

        let islands = islands(&collect(&data[8..]));
        assert_eq!(islands.len(), 1);
        assert_eq!(islands[0].urls, vec!["https://a.example/"]);
    }

    #[test]
    fn the_last_page_of_a_tab_is_the_one_that_counts() {
        let mut session = Session::new();
        session.tab(10, 1, 0, "https://old.example/");
        let mut navigation = Vec::new();
        navigation.extend(0u32.to_le_bytes());
        navigation.extend(10u32.to_le_bytes());
        navigation.extend(1u32.to_le_bytes());
        navigation.extend(("https://new.example/".len() as u32).to_le_bytes());
        navigation.extend(b"https://new.example/");
        navigation.extend([0, 0, 0, 0]);
        session.command(UPDATE_TAB_NAVIGATION, &navigation);
        session.command(
            SET_SELECTED_NAVIGATION,
            &[10u32.to_le_bytes(), 1u32.to_le_bytes()].concat(),
        );
        let data = session.bytes;

        let islands = islands(&collect(&data[8..]));
        assert_eq!(islands[0].urls, vec!["https://new.example/"]);
    }

    #[test]
    fn a_file_that_breaks_off_keeps_what_was_read() {
        let mut session = Session::new();
        session.tab(10, 1, 0, "https://a.example/");
        let mut data = session.bytes;
        data.extend([200, 0, 6, 1, 2, 3]);

        let islands = islands(&collect(&data[8..]));
        assert_eq!(islands[0].urls, vec!["https://a.example/"]);
    }
}
