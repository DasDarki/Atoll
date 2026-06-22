use std::collections::HashMap;

use atoll_core::TabId;
use atoll_engine::{BrowserView, CursorKind, ViewId};

/// Runtime information about a tab that currently owns an engine instance.
pub struct TabView {
    pub view: BrowserView,
    pub pending_scroll: Option<i32>,
    pub progress: f64,
    pub crashed: bool,
    pub cursor: CursorKind,
    pub status: String,
    /// Set while back navigation walks the replacement history instead of the engine history.
    pub replacement_history: bool,
    pub expected_address: Option<String>,
}

impl TabView {
    pub fn new(view: BrowserView, pending_scroll: Option<i32>) -> Self {
        Self {
            view,
            pending_scroll,
            progress: 0.0,
            crashed: false,
            cursor: CursorKind::Default,
            status: String::new(),
            replacement_history: false,
            expected_address: None,
        }
    }
}

/// Maps tabs to their engine views and back.
#[derive(Default)]
pub struct ViewRegistry {
    views: HashMap<TabId, TabView>,
    tabs: HashMap<ViewId, TabId>,
}

impl ViewRegistry {
    pub fn insert(&mut self, tab: TabId, view: TabView) {
        self.tabs.insert(view.view.id(), tab);
        if let Some(previous) = self.views.insert(tab, view) {
            self.tabs.remove(&previous.view.id());
        }
    }

    pub fn remove(&mut self, tab: TabId) -> Option<TabView> {
        let view = self.views.remove(&tab)?;
        self.tabs.remove(&view.view.id());
        Some(view)
    }

    pub fn get(&self, tab: TabId) -> Option<&TabView> {
        self.views.get(&tab)
    }

    pub fn get_mut(&mut self, tab: TabId) -> Option<&mut TabView> {
        self.views.get_mut(&tab)
    }

    pub fn contains(&self, tab: TabId) -> bool {
        self.views.contains_key(&tab)
    }

    pub fn tab_of(&self, view: ViewId) -> Option<TabId> {
        self.tabs.get(&view).copied()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&TabId, &TabView)> {
        self.views.iter()
    }

    pub fn drain(&mut self) -> Vec<(TabId, TabView)> {
        self.tabs.clear();
        self.views.drain().collect()
    }
}
