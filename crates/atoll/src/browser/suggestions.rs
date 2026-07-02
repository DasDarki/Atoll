use std::collections::HashSet;

use atoll_core::address::{display_url, normalize};
use atoll_core::search::Matcher;
use atoll_core::{Bookmark, TabId, Timestamp, VisitedPage};
use atoll_storage::TabEntry;
use atoll_storage::prelude::*;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use super::Browser;
use super::navigation::{Resolved, resolve};
use crate::ui::{BrowserState, Completion, SuggestionItem, SuggestionKind};

const MAX_SUGGESTIONS: usize = 8;
const HISTORY_CANDIDATES: usize = 300;

/// What choosing a suggestion does.
#[derive(Debug, Clone)]
enum Target {
    Load(String),
    SwitchToTab(TabId),
}

/// The state of the address bar while the user is typing.
#[derive(Default)]
pub struct SuggestionState {
    targets: Vec<Target>,
    displays: Vec<String>,
    typed: String,
    selected: i32,
    tabs: Option<Vec<TabEntry>>,
    bookmarks: Option<Vec<Bookmark>>,
}

struct Candidate {
    item: SuggestionItem,
    target: Target,
    url: String,
    display: String,
    score: i64,
}

impl Browser {
    /// Recomputes the suggestions for the text typed into the address bar (`source` 0) or the new
    /// tab page (`source` 1) and returns the inline completion.
    pub fn address_edited(&self, source: i32, text: &str) -> Completion {
        let deleting = {
            let mut state = self.suggestions.borrow_mut();
            let deleting = state.typed.len() > text.len() && state.typed.starts_with(text);
            state.typed = text.to_owned();
            state.selected = 0;
            deleting
        };
        let unchanged = Completion {
            text: text.into(),
            selection_start: text.len() as i32,
            selection_end: text.len() as i32,
        };
        let query = text.trim();
        if query.is_empty() {
            self.dismiss_suggestions();
            return unchanged;
        }

        self.load_suggestion_sources();
        let pages = self
            .storage
            .visited_pages_containing(
                query.split_whitespace().next().unwrap_or(query),
                HISTORY_CANDIDATES,
            )
            .unwrap_or_default();
        let completion = (!deleting && query == text && !text.contains(char::is_whitespace))
            .then(|| {
                inline_completion(
                    text,
                    &pages,
                    self.suggestions
                        .borrow()
                        .bookmarks
                        .as_deref()
                        .unwrap_or_default(),
                )
            })
            .flatten();

        let primary_text = completion.as_deref().unwrap_or(query);
        let mut candidates = vec![self.primary_candidate(primary_text, query)];
        candidates.extend(self.matching_candidates(query, &pages));

        let mut seen = HashSet::new();
        let mut targets = Vec::new();
        let mut displays = Vec::new();
        let mut items = Vec::new();
        for candidate in candidates {
            let key = normalize(&candidate.url);
            if !seen.insert(key) || items.len() >= MAX_SUGGESTIONS {
                continue;
            }
            targets.push(candidate.target);
            displays.push(candidate.display);
            items.push(candidate.item);
        }
        {
            let mut state = self.suggestions.borrow_mut();
            state.targets = targets;
            state.displays = displays;
        }
        if let Some(window) = self.window() {
            let browser_state = window.global::<BrowserState>();
            browser_state.set_suggestion_source(source);
            browser_state.set_suggestion_index(0);
            browser_state.set_suggestions(ModelRc::from(std::rc::Rc::new(VecModel::from(items))));
            browser_state.set_suggestions_visible(true);
        }

        match completion {
            Some(completed) => Completion {
                selection_start: text.len() as i32,
                selection_end: completed.len() as i32,
                text: completed.into(),
            },
            None => unchanged,
        }
    }

    /// Moves the highlighted suggestion and returns the text to show in the input.
    pub fn suggestion_move(&self, delta: i32) -> SharedString {
        let (selected, text) = {
            let mut state = self.suggestions.borrow_mut();
            let last = (state.targets.len() as i32 - 1).max(0);
            state.selected = (state.selected + delta).clamp(0, last);
            let text = usize::try_from(state.selected)
                .ok()
                .and_then(|index| state.displays.get(index).cloned())
                .unwrap_or_else(|| state.typed.clone());
            (state.selected, text)
        };
        if let Some(window) = self.window() {
            window
                .global::<BrowserState>()
                .set_suggestion_index(selected);
        }
        text.into()
    }

    pub fn suggestion_activate(&self, index: i32) {
        let target = usize::try_from(index)
            .ok()
            .and_then(|index| self.suggestions.borrow().targets.get(index).cloned());
        self.dismiss_suggestions();
        match target {
            Some(Target::Load(url)) => self.navigate(&url),
            Some(Target::SwitchToTab(tab)) => self.reveal_tab(tab),
            None => {}
        }
    }

    /// Handles Enter in the address bar or on the new tab page.
    pub fn address_submitted(&self, text: &str) {
        let selected = {
            let state = self.suggestions.borrow();
            usize::try_from(state.selected)
                .ok()
                .filter(|index| *index < state.targets.len())
        };
        match selected {
            Some(index) => self.suggestion_activate(index as i32),
            None => {
                self.dismiss_suggestions();
                self.navigate(text);
            }
        }
    }

    pub fn dismiss_suggestions(&self) {
        *self.suggestions.borrow_mut() = SuggestionState::default();
        if let Some(window) = self.window() {
            let state = window.global::<BrowserState>();
            state.set_suggestions_visible(false);
            state.set_suggestion_index(-1);
        }
    }

    fn load_suggestion_sources(&self) {
        let mut state = self.suggestions.borrow_mut();
        if state.tabs.is_none() {
            state.tabs = Some(self.storage.all_tab_entries().unwrap_or_default());
        }
        if state.bookmarks.is_none() {
            state.bookmarks = Some(self.storage.bookmarks().unwrap_or_default());
        }
    }

    fn primary_candidate(&self, text: &str, query: &str) -> Candidate {
        let (item, url) = match resolve(text, &self.preferences.search_url) {
            Some(Resolved::Search(url)) => (
                SuggestionItem {
                    kind: SuggestionKind::Search,
                    title: format!("„{query}“ suchen").into(),
                    ..SuggestionItem::default()
                },
                url,
            ),
            Some(Resolved::Address(url)) => (
                SuggestionItem {
                    kind: SuggestionKind::Navigate,
                    title: display_url(&url).into(),
                    ..SuggestionItem::default()
                },
                url,
            ),
            None => (SuggestionItem::default(), String::new()),
        };
        Candidate {
            item,
            display: text.to_owned(),
            url: url.clone(),
            target: Target::Load(url),
            score: i64::MAX,
        }
    }

    fn matching_candidates(&self, query: &str, pages: &[VisitedPage]) -> Vec<Candidate> {
        let matcher = Matcher::new(query);
        let active = self.session.borrow().active_tab;
        let current_workspace = self.session.borrow().workspace;
        let state = self.suggestions.borrow();
        let mut favicons = self.favicons.borrow_mut();

        let mut tabs: Vec<Candidate> = state
            .tabs
            .as_deref()
            .unwrap_or_default()
            .iter()
            .filter(|entry| Some(entry.tab.id) != active && !entry.tab.is_blank())
            .filter_map(|entry| {
                let title = entry.tab.display_title();
                let score = matcher.score(&[&title, &display_url(&entry.tab.url)])?
                    + if entry.island.workspace_id == current_workspace {
                        10
                    } else {
                        0
                    };
                let icon = favicons.image_for_tab(&self.storage, &entry.tab);
                Some(Candidate {
                    item: SuggestionItem {
                        kind: SuggestionKind::Tab,
                        title: title.into(),
                        url: display_url(&entry.tab.url).into(),
                        detail: "Zu Tab wechseln".into(),
                        has_icon: icon.is_some(),
                        icon: icon.unwrap_or_default(),
                    },
                    display: display_url(&entry.tab.url),
                    url: entry.tab.url.clone(),
                    target: Target::SwitchToTab(entry.tab.id),
                    score,
                })
            })
            .collect();
        tabs.sort_by_key(|candidate| std::cmp::Reverse(candidate.score));
        tabs.truncate(2);

        let mut bookmarks: Vec<Candidate> = state
            .bookmarks
            .as_deref()
            .unwrap_or_default()
            .iter()
            .filter_map(|bookmark| {
                let url = bookmark.url.as_deref()?;
                let score = matcher.score(&[&bookmark.title, &display_url(url)])?;
                let icon = favicons.image_for_url(&self.storage, url);
                Some(Candidate {
                    item: SuggestionItem {
                        kind: SuggestionKind::Bookmark,
                        title: bookmark.title.as_str().into(),
                        url: display_url(url).into(),
                        detail: "Lesezeichen".into(),
                        has_icon: icon.is_some(),
                        icon: icon.unwrap_or_default(),
                    },
                    display: display_url(url),
                    url: url.to_owned(),
                    target: Target::Load(url.to_owned()),
                    score,
                })
            })
            .collect();
        bookmarks.sort_by_key(|candidate| std::cmp::Reverse(candidate.score));
        bookmarks.truncate(3);

        let now = Timestamp::now();
        let mut history: Vec<Candidate> = pages
            .iter()
            .filter_map(|page| {
                let title = page.title.clone().unwrap_or_else(|| display_url(&page.url));
                let score =
                    matcher.score(&[&title, &display_url(&page.url)])? + frecency(page, now);
                let icon = favicons.image_for_url(&self.storage, &page.url);
                Some(Candidate {
                    item: SuggestionItem {
                        kind: SuggestionKind::History,
                        title: title.into(),
                        url: display_url(&page.url).into(),
                        detail: "".into(),
                        has_icon: icon.is_some(),
                        icon: icon.unwrap_or_default(),
                    },
                    display: display_url(&page.url),
                    url: page.url.clone(),
                    target: Target::Load(page.url.clone()),
                    score,
                })
            })
            .collect();
        history.sort_by_key(|candidate| std::cmp::Reverse(candidate.score));
        history.truncate(MAX_SUGGESTIONS);

        tabs.into_iter().chain(bookmarks).chain(history).collect()
    }
}

/// A bonus for pages that are visited often and recently.
pub(super) fn frecency(page: &VisitedPage, now: Timestamp) -> i64 {
    let visits = ((page.visit_count.max(1) as f64).ln() * 12.0) as i64;
    let days = now.elapsed_since(page.last_visited_at).as_secs() / 86_400;
    let recency = match days {
        0 => 20,
        1..=6 => 12,
        7..=30 => 5,
        _ => 0,
    };
    visits + recency
}

/// Completes typed text to the host of the most visited matching page.
fn inline_completion(typed: &str, pages: &[VisitedPage], bookmarks: &[Bookmark]) -> Option<String> {
    if !typed.is_ascii() {
        return None;
    }
    let lower = typed.to_ascii_lowercase();
    let mut candidates: Vec<(i64, String)> = pages
        .iter()
        .map(|page| (page.visit_count, display_url(&page.url)))
        .chain(
            bookmarks
                .iter()
                .filter_map(|bookmark| bookmark.url.as_deref())
                .map(|url| (1, display_url(url))),
        )
        .filter(|(_, display)| display.to_ascii_lowercase().starts_with(&lower))
        .collect();
    candidates.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.len().cmp(&b.1.len())));
    let (_, display) = candidates.into_iter().next()?;
    let completion = if typed.contains('/') {
        display
    } else {
        display.split('/').next().unwrap_or(&display).to_owned()
    };
    (completion.len() > typed.len()).then(|| format!("{typed}{}", &completion[typed.len()..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(url: &str, visits: i64) -> VisitedPage {
        VisitedPage {
            url: url.into(),
            title: None,
            visit_count: visits,
            last_visited_at: Timestamp(0),
        }
    }

    #[test]
    fn completes_to_the_host_of_the_most_visited_page() {
        let pages = [
            page("https://github.com/brave", 3),
            page("https://gitlab.com/", 9),
        ];
        assert_eq!(
            inline_completion("git", &pages, &[]).as_deref(),
            Some("gitlab.com")
        );
        assert_eq!(
            inline_completion("gith", &pages, &[]).as_deref(),
            Some("github.com")
        );
    }

    #[test]
    fn keeps_the_typed_casing_and_completes_paths_after_a_slash() {
        let pages = [page("https://www.github.com/brave/adblock-rust", 1)];
        assert_eq!(
            inline_completion("GitH", &pages, &[]).as_deref(),
            Some("GitHub.com")
        );
        assert_eq!(
            inline_completion("github.com/br", &pages, &[]).as_deref(),
            Some("github.com/brave/adblock-rust")
        );
    }

    #[test]
    fn no_completion_without_a_prefix_match() {
        let pages = [page("https://example.com/", 1)];
        assert_eq!(inline_completion("ample", &pages, &[]), None);
        assert_eq!(inline_completion("example.com", &pages, &[]), None);
    }
}
