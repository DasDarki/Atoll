use std::collections::{BTreeMap, HashSet};

use crate::ids::{TabId, WindowId};

/// The runtime state of a tab.
///
/// Only `Archived` and `Sleeping` survive a restart. `Frozen` and `Live` exist while an engine
/// instance is attached to the tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TabState {
    Archived,
    Sleeping,
    Frozen,
    Live,
}

impl TabState {
    pub fn has_engine_instance(self) -> bool {
        matches!(self, TabState::Frozen | TabState::Live)
    }
}

/// An instruction for the engine layer produced by a lifecycle transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleCommand {
    /// Create an engine instance for the tab and load its address.
    Wake(TabId),
    /// Make the tab's engine instance visible and active.
    Show(TabId),
    /// Pause the tab's engine instance and hide it.
    Hide(TabId),
    /// Destroy the tab's engine instance, turning it back into a plain record.
    Unload(TabId),
}

/// The state machine deciding which tabs own an engine instance.
///
/// Every window has at most one `Live` tab. Tabs that were live before become `Frozen` and are
/// kept in most-recently-used order, shared by all windows, until the frozen limit or the memory
/// budget pushes the least recently used ones back to `Sleeping`. Protected tabs, such as pinned
/// ones, and busy tabs, which play media or hold unsaved input, are never unloaded automatically.
#[derive(Debug, Clone)]
pub struct TabLifecycle {
    live: BTreeMap<WindowId, TabId>,
    frozen: Vec<TabId>,
    frozen_limit: usize,
    protected: HashSet<TabId>,
    busy: HashSet<TabId>,
}

impl TabLifecycle {
    pub fn new(frozen_limit: usize) -> Self {
        Self {
            live: BTreeMap::new(),
            frozen: Vec::new(),
            frozen_limit,
            protected: HashSet::new(),
            busy: HashSet::new(),
        }
    }

    /// The live tab of `window`.
    pub fn live_in(&self, window: WindowId) -> Option<TabId> {
        self.live.get(&window).copied()
    }

    /// Frozen tabs, most recently used first.
    pub fn frozen(&self) -> &[TabId] {
        &self.frozen
    }

    pub fn frozen_limit(&self) -> usize {
        self.frozen_limit
    }

    pub fn state_of(&self, tab: TabId, archived: bool) -> TabState {
        if self.is_live(tab) {
            TabState::Live
        } else if self.frozen.contains(&tab) {
            TabState::Frozen
        } else if archived {
            TabState::Archived
        } else {
            TabState::Sleeping
        }
    }

    pub fn has_engine_instance(&self, tab: TabId) -> bool {
        self.is_live(tab) || self.frozen.contains(&tab)
    }

    fn is_live(&self, tab: TabId) -> bool {
        self.live.values().any(|live| *live == tab)
    }

    /// Makes `tab` the live tab of `window`. The previous live tab of that window is frozen. A tab
    /// that is live in another window moves over without being reloaded.
    pub fn activate(&mut self, window: WindowId, tab: TabId) -> Vec<LifecycleCommand> {
        if self.live_in(window) == Some(tab) {
            return Vec::new();
        }

        let mut commands = self.freeze_live(window);
        let elsewhere = self
            .live
            .iter()
            .find(|(_, live)| **live == tab)
            .map(|(other, _)| *other);
        if let Some(other) = elsewhere {
            self.live.remove(&other);
        } else if let Some(index) = self.frozen.iter().position(|frozen| *frozen == tab) {
            self.frozen.remove(index);
        } else {
            commands.push(LifecycleCommand::Wake(tab));
        }
        commands.push(LifecycleCommand::Show(tab));
        self.live.insert(window, tab);
        commands.extend(self.enforce_frozen_limit());
        commands
    }

    /// Freezes the live tab of `window` without activating another one, for example while a blank
    /// tab is shown.
    pub fn deactivate(&mut self, window: WindowId) -> Vec<LifecycleCommand> {
        let mut commands = self.freeze_live(window);
        commands.extend(self.enforce_frozen_limit());
        commands
    }

    /// Puts `tab` to sleep, destroying its engine instance if it has one.
    pub fn sleep(&mut self, tab: TabId) -> Vec<LifecycleCommand> {
        let live_in = self
            .live
            .iter()
            .find(|(_, live)| **live == tab)
            .map(|(window, _)| *window);
        if let Some(window) = live_in {
            self.live.remove(&window);
            return vec![LifecycleCommand::Unload(tab)];
        }
        if let Some(index) = self.frozen.iter().position(|frozen| *frozen == tab) {
            self.frozen.remove(index);
            return vec![LifecycleCommand::Unload(tab)];
        }
        Vec::new()
    }

    /// Removes every trace of `tab`, used when the tab is closed or archived.
    pub fn forget(&mut self, tab: TabId) -> Vec<LifecycleCommand> {
        self.protected.remove(&tab);
        self.busy.remove(&tab);
        self.sleep(tab)
    }

    /// Protected tabs are never unloaded by the frozen limit or the memory budget.
    pub fn set_protected(&mut self, tab: TabId, protected: bool) -> Vec<LifecycleCommand> {
        if protected {
            self.protected.insert(tab);
            Vec::new()
        } else {
            self.protected.remove(&tab);
            self.enforce_frozen_limit()
        }
    }

    pub fn is_protected(&self, tab: TabId) -> bool {
        self.protected.contains(&tab)
    }

    /// Busy tabs play media or hold unsaved input and are kept like protected ones while busy.
    pub fn set_busy(&mut self, tab: TabId, busy: bool) -> Vec<LifecycleCommand> {
        if busy {
            self.busy.insert(tab);
            Vec::new()
        } else if self.busy.remove(&tab) {
            self.enforce_frozen_limit()
        } else {
            Vec::new()
        }
    }

    pub fn is_busy(&self, tab: TabId) -> bool {
        self.busy.contains(&tab)
    }

    /// Whether `tab` is never unloaded automatically.
    fn is_kept(&self, tab: TabId) -> bool {
        self.protected.contains(&tab) || self.busy.contains(&tab)
    }

    pub fn set_frozen_limit(&mut self, limit: usize) -> Vec<LifecycleCommand> {
        self.frozen_limit = limit;
        self.enforce_frozen_limit()
    }

    /// Unloads the least recently used frozen tab that is neither protected nor busy, used when
    /// the memory budget is exceeded. Returns `None` if nothing can be demoted.
    pub fn demote_least_recent(&mut self) -> Option<LifecycleCommand> {
        let index = self.frozen.iter().rposition(|tab| !self.is_kept(*tab))?;
        let tab = self.frozen.remove(index);
        Some(LifecycleCommand::Unload(tab))
    }

    fn freeze_live(&mut self, window: WindowId) -> Vec<LifecycleCommand> {
        match self.live.remove(&window) {
            Some(previous) => {
                self.frozen.insert(0, previous);
                vec![LifecycleCommand::Hide(previous)]
            }
            None => Vec::new(),
        }
    }

    fn enforce_frozen_limit(&mut self) -> Vec<LifecycleCommand> {
        let mut commands = Vec::new();
        let mut unprotected = self
            .frozen
            .iter()
            .filter(|tab| !self.is_kept(**tab))
            .count();
        while unprotected > self.frozen_limit {
            match self.demote_least_recent() {
                Some(command) => commands.push(command),
                None => break,
            }
            unprotected -= 1;
        }
        commands
    }
}

#[cfg(test)]
mod tests;
