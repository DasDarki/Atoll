//! Picking several tabs in the sidebar, so they can be moved together.
//!
//! A plain click shows a tab as it always did. Holding Ctrl adds one to the picking or takes it
//! out again, holding Shift takes everything between the last one and this, counted the way the
//! sidebar shows it, across island borders. What is picked is dragged as one: inside the sidebar
//! it lands together where the line stands, and let go outside the window it takes a window of
//! its own, all of it in one island.

use atoll_core::TabId;

use super::Browser;

/// The tabs picked in one window.
#[derive(Debug, Default, Clone)]
pub struct Picks {
    tabs: Vec<TabId>,
    /// The tab a range is counted from, which is the last one clicked.
    anchor: Option<TabId>,
}

impl Browser {
    /// A click on a tab row, with the keys that were held.
    pub fn tab_clicked(&self, tab: TabId, control: bool, shift: bool) {
        if control {
            self.toggle_pick(tab);
            return;
        }
        if shift {
            self.pick_up_to(tab);
            return;
        }
        self.forget_picks();
        self.activate_tab(tab);
    }

    pub(super) fn is_picked(&self, tab: TabId) -> bool {
        self.picks.borrow().tabs.contains(&tab)
    }

    /// The picked tabs in the order the sidebar shows them.
    pub(super) fn picked_tabs(&self) -> Vec<TabId> {
        let picked = self.picks.borrow();
        let session = self.session.borrow();
        session
            .tabs()
            .map(|tab| tab.id)
            .filter(|tab| picked.tabs.contains(tab))
            .collect()
    }

    /// Lets go of the picks without touching the rows, for the moment a drag begins: the list
    /// under the pointer must not be rebuilt while it is being dragged.
    pub(super) fn forget_picks_quietly(&self) {
        *self.picks.borrow_mut() = Picks::default();
    }

    /// Nothing is picked any more.
    pub(super) fn forget_picks(&self) {
        let had = !self.picks.borrow().tabs.is_empty();
        *self.picks.borrow_mut() = Picks::default();
        if had {
            self.refresh_sidebar();
        }
    }

    fn toggle_pick(&self, tab: TabId) {
        {
            let mut picks = self.picks.borrow_mut();
            match picks.tabs.iter().position(|picked| *picked == tab) {
                Some(at) => {
                    picks.tabs.remove(at);
                }
                None => picks.tabs.push(tab),
            }
            picks.anchor = Some(tab);
        }
        // Picking a single tab means nothing, so the one that is shown counts as picked too.
        let alone = {
            let picks = self.picks.borrow();
            picks.tabs.len() == 1 && picks.tabs.first().copied() == Some(tab)
        };
        if alone && let Some(active) = self.session.borrow().active_tab.filter(|a| *a != tab) {
            self.picks.borrow_mut().tabs.insert(0, active);
        }
        self.refresh_sidebar();
    }

    /// Picks everything between the tab that was clicked last and `tab`.
    fn pick_up_to(&self, tab: TabId) {
        let anchor = self
            .picks
            .borrow()
            .anchor
            .or_else(|| self.session.borrow().active_tab);
        let Some(anchor) = anchor else {
            self.toggle_pick(tab);
            return;
        };
        let order: Vec<TabId> = self.session.borrow().tabs().map(|tab| tab.id).collect();
        let (Some(from), Some(to)) = (
            order.iter().position(|one| *one == anchor),
            order.iter().position(|one| *one == tab),
        ) else {
            return;
        };
        let (first, last) = if from <= to { (from, to) } else { (to, from) };
        {
            let mut picks = self.picks.borrow_mut();
            picks.tabs = order[first..=last].to_vec();
            picks.anchor = Some(anchor);
        }
        self.refresh_sidebar();
    }

    /// Tabs that are gone cannot stay picked.
    pub(super) fn prune_picks(&self) {
        let session = self.session.borrow();
        let mut picks = self.picks.borrow_mut();
        picks.tabs.retain(|tab| session.contains_tab(*tab));
        if picks.anchor.is_some_and(|tab| !session.contains_tab(tab)) {
            picks.anchor = None;
        }
    }

    /// Whether `tab` is drawn as picked: only while more than one is, a single one says nothing.
    pub(super) fn shows_as_picked(&self, tab: TabId) -> bool {
        let picks = self.picks.borrow();
        picks.tabs.len() > 1 && picks.tabs.contains(&tab)
    }
}
