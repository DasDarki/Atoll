use atoll_core::address::display_host;
use atoll_core::{IslandId, TabId, WorkspaceId};
use atoll_storage::NewIsland;
use atoll_storage::prelude::*;
use slint::ComponentHandle;

use super::Browser;
use atoll_i18n::t;

/// Where a window opened by dropping a tab lands relative to the pointer, so the pointer ends up
/// over its sidebar.
const DROP_OFFSET: (f32, f32) = (140.0, 90.0);

impl Browser {
    /// A tab row was dragged and released at `x`, `y` in window coordinates. Inside the window it
    /// lands where the line stood, outside it moves to the window under the pointer or into a new
    /// one.
    pub fn tab_drag_released(&self, tab: TabId, x: f32, y: f32) {
        let Some(window) = self.window() else { return };
        let handle = window.window();
        let scale = handle.scale_factor();
        let size = handle.size().to_logical(scale);
        if (0.0..size.width).contains(&x) && (0.0..size.height).contains(&y) {
            self.tab_drag_dropped();
            return;
        }
        self.forget_tab_drag();
        let origin = handle.position().to_logical(scale);
        let position = (
            (origin.x + x - DROP_OFFSET.0).round() as i32,
            (origin.y + y - DROP_OFFSET.1).round() as i32,
        );
        if let Some(manager) = self.manager() {
            manager.tab_dropped_outside(self.id, tab, position);
        }
    }

    /// Moves `tab` into a new window with a workspace of its own. The page keeps running.
    pub fn move_tab_to_new_window(&self, tab: TabId, position: Option<(i32, i32)>) {
        let Some(manager) = self.manager() else {
            return;
        };
        let (record, island, departure) = {
            let session = self.session.borrow();
            let Some(record) = session.tab(tab).cloned() else {
                return;
            };
            let Some(island) = session
                .island(record.island_id)
                .map(|entry| entry.island.clone())
            else {
                return;
            };
            let departure = Departure::of(&session, [tab]);
            (record, island, departure)
        };
        let name = display_host(&record.url)
            .or_else(|| record.title.clone())
            .unwrap_or_else(|| island.name.clone());
        let created = self.storage.transaction(|transaction| {
            let workspace = transaction.create_workspace(&name)?;
            let target = transaction.create_island(NewIsland {
                workspace_id: workspace.id,
                name: &island.name,
                color: island.color,
                index: None,
                created_at: Self::now(),
            })?;
            transaction.move_tab(tab, target.id, 0)?;
            transaction.set_setting(
                &Self::active_workspace_key(workspace.id),
                &tab.0.to_string(),
            )?;
            Ok(workspace.id)
        });
        match created {
            Ok(workspace) => {
                self.reload_session_here();
                manager.open_window_for(workspace, Some(self), position);
                self.depart(departure);
            }
            Err(error) => self.report(t!("In neues Fenster verschieben"), Err::<(), _>(error)),
        }
    }

    /// Moves `tab` into the window `target` and shows it there. The page keeps running.
    pub(super) fn move_tab_to_window(&self, tab: TabId, target: &Browser) {
        if target.id == self.id {
            return;
        }
        let departure = {
            let session = self.session.borrow();
            if !session.contains_tab(tab) {
                return;
            }
            Departure::of(&session, [tab])
        };
        let destination = target
            .insertion_point_after_active()
            .or_else(|| target.ensure_island().map(|island| (island, usize::MAX)));
        let Some((island, index)) = destination else {
            return;
        };
        if let Err(error) = self.storage.move_tab(tab, island, index) {
            self.report(t!("Tab verschieben"), Err::<(), _>(error));
            return;
        }
        self.reload_session_here();
        target.reload_session_here();
        target.activate_tab(tab);
        if let Some(manager) = self.manager() {
            manager.focus_window(target.id);
        }
        self.depart(departure);
    }

    /// Tab menu "Move to window": moves `tab` to the window showing `workspace`.
    pub fn move_tab_to_workspace(&self, tab: TabId, workspace: WorkspaceId) {
        let target = self
            .manager()
            .and_then(|manager| manager.window_for_workspace(workspace));
        if let Some(target) = target {
            self.move_tab_to_window(tab, &target);
        }
    }

    /// Moves `island` with all its tabs into a new window with a workspace of its own.
    pub fn move_island_to_new_window(&self, island: IslandId) {
        let Some(manager) = self.manager() else {
            return;
        };
        let (name, tabs, departure) = {
            let session = self.session.borrow();
            let Some(entry) = session.island(island) else {
                return;
            };
            let tabs: Vec<TabId> = entry.tabs.iter().map(|tab| tab.id).collect();
            let departure = Departure::of(&session, tabs.iter().copied());
            (entry.island.name.clone(), tabs, departure)
        };
        let shown = departure
            .active
            .filter(|active| tabs.contains(active))
            .or_else(|| tabs.first().copied());
        let created = self.storage.transaction(|transaction| {
            let workspace = transaction.create_workspace(&name)?;
            transaction.move_island(island, workspace.id, 0)?;
            if let Some(tab) = shown {
                transaction.set_setting(
                    &Self::active_workspace_key(workspace.id),
                    &tab.0.to_string(),
                )?;
            }
            Ok(workspace.id)
        });
        match created {
            Ok(workspace) => {
                self.reload_session_here();
                manager.open_window_for(workspace, Some(self), None);
                self.depart(departure);
            }
            Err(error) => self.report(t!("In neues Fenster verschieben"), Err::<(), _>(error)),
        }
    }

    /// Workspace menu: opens `workspace` in a window of its own, or shows the window it is in.
    pub(super) fn open_workspace_in_window(&self, workspace: WorkspaceId) {
        if let Some(manager) = self.manager() {
            manager.open_workspace_window(workspace);
        }
    }

    /// Shows another tab once the active one left this window.
    fn depart(&self, departure: Departure) {
        if departure.leaves_active {
            match departure.successor {
                Some(next) => self.activate_tab(next),
                None => self.show_empty_workspace(),
            }
        } else {
            self.sync_page();
            self.refresh_sidebar();
        }
    }
}

/// What a window shows after some of its tabs moved to another window.
struct Departure {
    active: Option<TabId>,
    leaves_active: bool,
    successor: Option<TabId>,
}

impl Departure {
    fn of(session: &super::session::Session, leaving: impl IntoIterator<Item = TabId>) -> Self {
        let leaving: Vec<TabId> = leaving.into_iter().collect();
        let active = session.active_tab;
        let leaves_active = active.is_some_and(|tab| leaving.contains(&tab));
        let successor = active.filter(|_| leaves_active).and_then(|active| {
            session
                .successor_of(active)
                .filter(|next| !leaving.contains(next))
                .or_else(|| {
                    session
                        .tabs()
                        .map(|tab| tab.id)
                        .find(|tab| !leaving.contains(tab))
                })
        });
        Self {
            active,
            leaves_active,
            successor,
        }
    }
}

#[cfg(test)]
mod tests {
    use atoll_core::{IslandColor, Timestamp};
    use atoll_storage::{NewTab, Storage};

    use super::*;
    use crate::browser::session::Session;

    /// A workspace with the islands "A" (tabs a1, a2, a3) and "B" (tab b1), the tab ids in that
    /// order.
    fn session(active: usize) -> (Session, Vec<TabId>) {
        let storage = Storage::open_in_memory().unwrap();
        let workspace = storage.create_workspace("Test").unwrap().id;
        let mut tabs = Vec::new();
        for (name, count) in [("A", 3), ("B", 1)] {
            let island = storage
                .create_island(NewIsland {
                    workspace_id: workspace,
                    name,
                    color: IslandColor::for_position(0),
                    index: None,
                    created_at: Timestamp(0),
                })
                .unwrap();
            for index in 0..count {
                let url = format!("https://example.org/{name}{index}");
                let tab = storage
                    .create_tab(NewTab {
                        island_id: island.id,
                        url: &url,
                        title: None,
                        index: None,
                        created_at: Timestamp(0),
                        private: false,
                    })
                    .unwrap();
                tabs.push(tab.id);
            }
        }
        let mut session = Session::load(&storage, workspace).unwrap();
        session.active_tab = Some(tabs[active]);
        (session, tabs)
    }

    #[test]
    fn a_leaving_background_tab_keeps_the_active_one() {
        let (session, tabs) = session(0);
        let departure = Departure::of(&session, [tabs[1]]);
        assert!(!departure.leaves_active);
        assert_eq!(departure.successor, None);
    }

    #[test]
    fn the_next_tab_of_the_island_follows_the_active_one() {
        let (session, tabs) = session(1);
        let departure = Departure::of(&session, [tabs[1]]);
        assert!(departure.leaves_active);
        assert_eq!(departure.successor, Some(tabs[2]));
    }

    #[test]
    fn a_leaving_island_is_followed_by_a_tab_outside_of_it() {
        let (session, tabs) = session(1);
        let departure = Departure::of(&session, tabs[..3].iter().copied());
        assert_eq!(departure.active, Some(tabs[1]));
        assert_eq!(departure.successor, Some(tabs[3]));
    }

    #[test]
    fn nothing_follows_when_every_tab_leaves() {
        let (session, tabs) = session(3);
        let departure = Departure::of(&session, tabs.iter().copied());
        assert!(departure.leaves_active);
        assert_eq!(departure.successor, None);
    }
}
