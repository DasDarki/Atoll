use std::collections::HashSet;

use crate::ids::TabId;

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
/// At most one tab is `Live`. Tabs that were live before become `Frozen` and are kept in
/// most-recently-used order until the frozen limit or the memory budget pushes the least recently
/// used ones back to `Sleeping`. Protected tabs are never unloaded automatically.
#[derive(Debug, Clone)]
pub struct TabLifecycle {
    live: Option<TabId>,
    frozen: Vec<TabId>,
    frozen_limit: usize,
    protected: HashSet<TabId>,
}

impl TabLifecycle {
    pub fn new(frozen_limit: usize) -> Self {
        Self {
            live: None,
            frozen: Vec::new(),
            frozen_limit,
            protected: HashSet::new(),
        }
    }

    pub fn live(&self) -> Option<TabId> {
        self.live
    }

    /// Frozen tabs, most recently used first.
    pub fn frozen(&self) -> &[TabId] {
        &self.frozen
    }

    pub fn frozen_limit(&self) -> usize {
        self.frozen_limit
    }

    pub fn state_of(&self, tab: TabId, archived: bool) -> TabState {
        if self.live == Some(tab) {
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
        self.live == Some(tab) || self.frozen.contains(&tab)
    }

    /// Makes `tab` the live tab. The previous live tab is frozen.
    pub fn activate(&mut self, tab: TabId) -> Vec<LifecycleCommand> {
        if self.live == Some(tab) {
            return Vec::new();
        }

        let mut commands = self.freeze_live();
        if let Some(index) = self.frozen.iter().position(|frozen| *frozen == tab) {
            self.frozen.remove(index);
        } else {
            commands.push(LifecycleCommand::Wake(tab));
        }
        commands.push(LifecycleCommand::Show(tab));
        self.live = Some(tab);
        commands.extend(self.enforce_frozen_limit());
        commands
    }

    /// Freezes the live tab without activating another one, for example while a blank tab is shown.
    pub fn deactivate(&mut self) -> Vec<LifecycleCommand> {
        let mut commands = self.freeze_live();
        commands.extend(self.enforce_frozen_limit());
        commands
    }

    /// Puts `tab` to sleep, destroying its engine instance if it has one.
    pub fn sleep(&mut self, tab: TabId) -> Vec<LifecycleCommand> {
        if self.live == Some(tab) {
            self.live = None;
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

    pub fn set_frozen_limit(&mut self, limit: usize) -> Vec<LifecycleCommand> {
        self.frozen_limit = limit;
        self.enforce_frozen_limit()
    }

    /// Unloads the least recently used unprotected frozen tab, used when the memory budget is
    /// exceeded. Returns `None` if nothing can be demoted.
    pub fn demote_least_recent(&mut self) -> Option<LifecycleCommand> {
        let index = self
            .frozen
            .iter()
            .rposition(|tab| !self.protected.contains(tab))?;
        let tab = self.frozen.remove(index);
        Some(LifecycleCommand::Unload(tab))
    }

    fn freeze_live(&mut self) -> Vec<LifecycleCommand> {
        match self.live.take() {
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
            .filter(|tab| !self.protected.contains(tab))
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
