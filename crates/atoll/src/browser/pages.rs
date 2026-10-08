//! Searching every page of the workspace, including the ones that are only sleeping.
//!
//! A tab knows its address and its title at all times, and that is what the search looks at by
//! default: it costs nothing and answers while the user types. Looking inside the pages is the
//! other half, and for that Atoll has to have read them: what a page shows is taken when the user
//! leaves it - the moment where it is finished, unlike at its first load, where a page that
//! builds itself has nothing to show yet.
//!
//! Only the text is kept, never the markup, and only of the open tabs: one row per tab, replaced
//! when the page is left and thrown away with the tab. The index of the database does the
//! searching, so a search costs the matches and not the pages, and nothing is held in memory.
//! Private tabs are left out of all of it, like everywhere else.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use atoll_core::search::Matcher;
use atoll_core::{IslandId, TabId};
use atoll_engine::{ScriptId, ScriptWorld, ViewId};
use atoll_i18n::t;
use atoll_storage::prelude::*;
use slint::{ComponentHandle, ModelRc, VecModel};

use super::Browser;
use super::preferences::PreferenceKey;
use crate::ui::{OverlayState, PageHit};

/// How many pages the list shows.
const SHOWN: usize = 40;

/// How long a page gets after it finished loading before Atoll reads it. A page that builds
/// itself has nothing to show at the moment it is loaded, which is why nothing is read then.
const SETTLES_IN: Duration = Duration::from_secs(5);

/// What a page shows, as the browser asks the page itself. `innerText` is what is rendered, so
/// what is hidden, and everything that is markup, stays out by itself.
const READ_THE_PAGE: &str = r#"(() => {
  const body = document.body;
  if (!body) {
    return "";
  }
  const text = body.innerText || "";
  return text.replace(/[\s ]+/g, " ").trim().slice(0, 65536);
})()"#;

/// The search over the pages of the workspace.
#[derive(Default)]
pub struct PageSearch {
    pub(super) visible: bool,
    text: String,
    /// Whether the text of the pages is searched as well.
    in_pages: bool,
    hits: Vec<Hit>,
    index: usize,
    /// The captures Atoll is waiting for, so their answers can be told apart.
    asked: HashMap<(ViewId, ScriptId), TabId>,
    /// Pages that finished loading and are given a moment to fill themselves.
    settling: HashMap<TabId, Instant>,
}

/// One page the search found.
#[derive(Debug, Clone)]
struct Hit {
    tab: TabId,
    island: IslandId,
    title: String,
    url: String,
    snippet: String,
}

impl Browser {
    /// Ctrl+Shift+F and the palette: opens the search over the pages of this workspace.
    pub fn open_page_search(&self) {
        let Some(window) = self.window() else { return };
        self.dismiss_suggestions();
        {
            let mut search = self.pages.borrow_mut();
            search.visible = true;
            search.text.clear();
            search.index = 0;
            search.in_pages = self.preferences.borrow().search_in_pages;
        }
        // The page in front of the user is the one most likely to be searched, so it is read
        // now; everything else was read when it was left.
        if let Some(tab) = self.session.borrow().active_tab {
            self.capture_page_text(tab);
        }
        let state = window.global::<OverlayState>();
        state.set_page_search_text("".into());
        state.set_page_search_in_pages(self.pages.borrow().in_pages);
        state.set_page_search_visible(true);
        self.find_pages("");
    }

    pub fn close_page_search(&self) {
        self.pages.borrow_mut().visible = false;
        if let Some(window) = self.window() {
            window
                .global::<OverlayState>()
                .set_page_search_visible(false);
        }
        self.focus_content();
    }

    /// The user typed something.
    pub fn page_search_edited(&self, text: &str) {
        self.pages.borrow_mut().text = text.to_owned();
        self.find_pages(text);
    }

    /// The user switched looking inside the pages on or off.
    pub fn page_search_in_pages(&self, inside: bool) {
        self.pages.borrow_mut().in_pages = inside;
        self.update_preference(PreferenceKey::SearchInPages, |preferences| {
            preferences.search_in_pages = inside;
        });
        let text = self.pages.borrow().text.clone();
        self.find_pages(&text);
    }

    /// Moves the chosen line.
    pub fn page_search_move(&self, by: i32) {
        let Some(window) = self.window() else { return };
        let mut search = self.pages.borrow_mut();
        if search.hits.is_empty() {
            return;
        }
        let last = search.hits.len() - 1;
        search.index = match by {
            by if by < 0 => search.index.checked_sub(1).unwrap_or(last),
            _ => {
                if search.index >= last {
                    0
                } else {
                    search.index + 1
                }
            }
        };
        let index = search.index as i32;
        drop(search);
        window.global::<OverlayState>().set_page_search_index(index);
    }

    /// Shows the chosen page.
    pub fn page_search_activate(&self, index: i32) {
        let chosen = {
            let search = self.pages.borrow();
            usize::try_from(index)
                .ok()
                .or(Some(search.index))
                .and_then(|index| search.hits.get(index))
                .map(|hit| (hit.tab, hit.island))
        };
        let Some((tab, island)) = chosen else { return };
        self.close_page_search();
        // A tab inside a collapsed island would be shown but not seen.
        let collapsed = self
            .session
            .borrow()
            .island(island)
            .is_some_and(|entry| entry.island.collapsed);
        if collapsed {
            self.toggle_island(island);
        }
        self.activate_tab(tab);
    }

    /// Looks for `text` among the pages of this workspace.
    fn find_pages(&self, text: &str) {
        let Some(window) = self.window() else { return };
        let in_pages = self.pages.borrow().in_pages;
        let hits = self.pages_matching(text, in_pages);
        let items: Vec<PageHit> = hits
            .iter()
            .map(|hit| PageHit {
                title: hit.title.as_str().into(),
                url: hit.url.as_str().into(),
                snippet: hit.snippet.as_str().into(),
            })
            .collect();
        {
            let mut search = self.pages.borrow_mut();
            search.index = 0;
            search.hits = hits;
        }
        let state = window.global::<OverlayState>();
        state.set_page_search_items(ModelRc::from(std::rc::Rc::new(VecModel::from(items))));
        state.set_page_search_index(0);
        state.set_page_search_hint(
            if text.trim().is_empty() {
                t!("Tippe, um unter den Seiten dieses Arbeitsbereichs zu suchen").to_owned()
            } else if self.pages.borrow().hits.is_empty() {
                t!("Nichts gefunden").to_owned()
            } else {
                String::new()
            }
            .into(),
        );
    }

    /// The pages of this workspace that match, the best first.
    fn pages_matching(&self, text: &str, in_pages: bool) -> Vec<Hit> {
        let matcher = Matcher::new(text);
        let session = self.session.borrow();
        let mut found: Vec<(i64, Hit)> = Vec::new();
        let inside: HashMap<TabId, String> = if in_pages && !matcher.is_empty() {
            self.storage
                .search_page_text(text, SHOWN * 4)
                .unwrap_or_default()
                .into_iter()
                .map(|hit| (hit.tab, hit.snippet))
                .collect()
        } else {
            HashMap::new()
        };
        for entry in &session.islands {
            for tab in &entry.tabs {
                if tab.private {
                    continue;
                }
                let title = tab.display_title().to_string();
                let by_name = matcher.score(&[&title, &tab.url]);
                let in_text = inside.get(&tab.id);
                let (score, snippet) = match (by_name, in_text) {
                    (Some(score), _) if matcher.is_empty() => (score, String::new()),
                    // What the title says weighs more than what stands somewhere in the page.
                    (Some(score), Some(snippet)) => (score + 50, snippet.clone()),
                    (Some(score), None) => (score, String::new()),
                    (None, Some(snippet)) => (40, snippet.clone()),
                    (None, None) => continue,
                };
                found.push((
                    score,
                    Hit {
                        tab: tab.id,
                        island: entry.island.id,
                        title,
                        url: tab.url.clone(),
                        snippet,
                    },
                ));
            }
        }
        found.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
        found.truncate(SHOWN);
        found.into_iter().map(|(_, hit)| hit).collect()
    }

    /// Asks a page what it shows, so it can be searched later. Answers arrive in
    /// [`Browser::page_text_arrived`].
    pub(super) fn capture_page_text(&self, tab: TabId) {
        if self.is_private(tab) {
            return;
        }
        let asked = {
            let views = self.views.borrow();
            let Some(entry) = views.get(tab) else {
                return;
            };
            entry
                .view
                .evaluate(READ_THE_PAGE, ScriptWorld::Isolated)
                .map(|script| (entry.view.id(), script))
        };
        if let Some(key) = asked {
            self.pages.borrow_mut().asked.insert(key, tab);
        }
    }

    /// A page answered what it shows. Returns whether this answer was one Atoll asked for.
    pub(super) fn page_text_arrived(
        &self,
        view: ViewId,
        script: ScriptId,
        result: &Result<serde_json::Value, String>,
    ) -> bool {
        let Some(tab) = self.pages.borrow_mut().asked.remove(&(view, script)) else {
            return false;
        };
        let Ok(value) = result else {
            tracing::debug!(tab = tab.0, "a page did not say what it shows");
            return true;
        };
        let Some(text) = value.as_str().filter(|text| !text.trim().is_empty()) else {
            return true;
        };
        let (url, title) = {
            let session = self.session.borrow();
            let Some(record) = session.tab(tab) else {
                return true;
            };
            (record.url.clone(), record.display_title().to_string())
        };
        if url.trim().is_empty() {
            return true;
        }
        let stored = self
            .storage
            .set_page_text(tab, &url, &title, text, Self::now());
        if let Err(error) = stored {
            tracing::warn!(%error, "the text of a page could not be kept");
        }
        true
    }

    /// Before Atoll closes: asks every page that is still open what it shows right now. A page
    /// that was never left would otherwise be remembered as it was a moment after it loaded.
    pub(super) fn capture_open_pages(&self) {
        let tabs = self.views.borrow().open_tabs();
        for tab in tabs {
            self.capture_page_text(tab);
        }
    }

    /// Whether answers are still on their way.
    pub(super) fn waits_for_page_text(&self) -> bool {
        !self.pages.borrow().asked.is_empty()
    }

    /// A page finished loading. It is read once it had a moment to build itself.
    pub(super) fn page_will_settle(&self, tab: TabId) {
        if self.is_private(tab) {
            return;
        }
        self.pages.borrow_mut().settling.insert(tab, Instant::now());
    }

    /// Reads the pages whose moment has come, called whenever the browser is awake anyway.
    pub(super) fn capture_settled_pages(&self) {
        let ready: Vec<TabId> = {
            let mut search = self.pages.borrow_mut();
            if search.settling.is_empty() {
                return;
            }
            let ready: Vec<TabId> = search
                .settling
                .iter()
                .filter(|(_, since)| since.elapsed() >= SETTLES_IN)
                .map(|(tab, _)| *tab)
                .collect();
            for tab in &ready {
                search.settling.remove(tab);
            }
            ready
        };
        for tab in ready {
            self.capture_page_text(tab);
        }
    }

    /// A tab is gone, and so is what its page said.
    pub(super) fn forget_page_text(&self, tab: TabId) {
        {
            let mut search = self.pages.borrow_mut();
            search.asked.retain(|_, known| *known != tab);
            search.settling.remove(&tab);
        }
        if let Err(error) = self.storage.forget_page_text(tab) {
            tracing::warn!(%error, "the text of a page could not be thrown away");
        }
    }

    /// At startup: what belongs to tabs that are not there any more goes.
    pub(super) fn forget_closed_page_texts(&self) {
        match self.storage.forget_closed_page_texts() {
            Ok(gone) if gone > 0 => tracing::debug!(gone, "texts of closed tabs thrown away"),
            Ok(_) => {}
            Err(error) => tracing::warn!(%error, "texts of closed tabs could not be thrown away"),
        }
    }
}
