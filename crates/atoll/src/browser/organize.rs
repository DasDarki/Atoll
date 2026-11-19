use atoll_core::{IslandColor, IslandId, TabId, WorkspaceId};
use atoll_storage::NewIsland;
use atoll_storage::prelude::*;

use super::Browser;
use super::session::Session;
use atoll_i18n::t;

impl Browser {
    /// Shows `workspace` in this window, or brings the window to the front that already shows it.
    pub fn select_workspace(&self, workspace: WorkspaceId) {
        if self.session.borrow().workspace == workspace {
            return;
        }
        if let Some(manager) = self.manager()
            && let Some(other) = manager.window_for_workspace(workspace)
        {
            manager.focus_window(other.id);
            return;
        }
        let session = match Session::load(&self.storage, workspace) {
            Ok(session) => session,
            Err(error) => {
                self.report(t!("Workspace wechseln"), Err::<(), _>(error));
                return;
            }
        };
        if self.is_engine_running() {
            let commands = self.lifecycle.borrow_mut().deactivate(self.id);
            self.apply(commands);
        }
        if let Err(error) = self.storage.set_window_workspace(self.id, workspace) {
            tracing::warn!(%error, "workspace of the window could not be stored");
        }
        *self.session.borrow_mut() = session;
        self.show_remembered_tab();
        self.for_other_windows(Browser::follow_changes);
    }

    /// Activates the tab that was last used in the current workspace.
    pub(super) fn show_remembered_tab(&self) {
        let remembered = {
            let session = self.session.borrow();
            self.storage
                .setting_i64(&Self::active_workspace_key(session.workspace))
                .ok()
                .flatten()
                .map(TabId)
                .filter(|tab| session.contains_tab(*tab))
                .or_else(|| session.tabs().next().map(|tab| tab.id))
        };
        match remembered {
            Some(tab) => self.activate_tab(tab),
            None => self.show_empty_workspace(),
        }
    }

    pub fn create_workspace(&self) {
        let this = self.this();
        self.prompt_text(
            t!("Neuer Workspace"),
            t!("Wie soll der Workspace heißen?"),
            "",
            t!("Anlegen"),
            move |name| {
                let Some(browser) = this.upgrade() else {
                    return;
                };
                match browser.storage.create_workspace(&name) {
                    Ok(workspace) => browser.select_workspace(workspace.id),
                    Err(error) => browser.report(t!("Workspace anlegen"), Err::<(), _>(error)),
                }
            },
        );
    }

    pub fn workspace_action(&self, workspace: WorkspaceId, action: &str) {
        match action {
            "open-window" => self.open_workspace_in_window(workspace),
            "rename" => self.rename_workspace(workspace),
            "delete" => self.delete_workspace(workspace),
            "move-left" => self.shift_workspace(workspace, -1),
            "move-right" => self.shift_workspace(workspace, 1),
            _ => tracing::warn!(action, "unknown workspace action"),
        }
    }

    fn rename_workspace(&self, workspace: WorkspaceId) {
        let current = self
            .session
            .borrow()
            .workspaces
            .iter()
            .find(|candidate| candidate.id == workspace)
            .map(|candidate| candidate.name.clone())
            .unwrap_or_default();
        let this = self.this();
        self.prompt_text(
            t!("Workspace umbenennen"),
            "",
            &current,
            t!("Umbenennen"),
            move |name| {
                let Some(browser) = this.upgrade() else {
                    return;
                };
                let result = browser.storage.rename_workspace(workspace, &name);
                browser.report(t!("Umbenennen"), result);
                browser.reload_session();
                browser.refresh_sidebar();
            },
        );
    }

    fn delete_workspace(&self, workspace: WorkspaceId) {
        let (name, count) = {
            let session = self.session.borrow();
            if session.workspaces.len() <= 1 {
                return;
            }
            let name = session
                .workspaces
                .iter()
                .find(|candidate| candidate.id == workspace)
                .map(|candidate| candidate.name.clone())
                .unwrap_or_default();
            let count = self
                .storage
                .tabs_in_workspace(workspace)
                .map(|tabs| tabs.len())
                .unwrap_or(0);
            (name, count)
        };
        let this = self.this();
        self.confirm(
            t!("Workspace löschen?"),
            &t!(
                "„{}“ wird mit allen Inseln und {} Tabs endgültig gelöscht.",
                name,
                count
            ),
            t!("Löschen"),
            move || {
                let Some(browser) = this.upgrade() else {
                    return;
                };
                let tabs = browser
                    .storage
                    .tabs_in_workspace(workspace)
                    .map(|tabs| tabs.into_iter().map(|tab| tab.id).collect::<Vec<_>>())
                    .unwrap_or_default();
                let manager = browser.manager();
                let shown_elsewhere = manager
                    .as_ref()
                    .and_then(|manager| manager.window_for_workspace(workspace))
                    .filter(|other| other.id != browser.id);
                let shown_here = browser.session.borrow().workspace == workspace;
                let next = shown_here
                    .then(|| browser.free_workspace(workspace))
                    .flatten();
                if let Some(next) = next
                    && let Err(error) = browser.storage.set_window_workspace(browser.id, next)
                {
                    tracing::warn!(%error, "workspace of the window could not be stored");
                }
                browser.forget_tabs(tabs);
                if let Err(error) = browser.storage.delete_workspace(workspace) {
                    browser.report(t!("Workspace löschen"), Err::<(), _>(error));
                    return;
                }
                if let (Some(manager), Some(other)) = (&manager, shown_elsewhere) {
                    manager.close_window(other.id);
                }
                if !shown_here {
                    browser.reload_session();
                    browser.refresh_sidebar();
                    return;
                }
                match (next, manager) {
                    (Some(next), _) => {
                        {
                            let mut session = browser.session.borrow_mut();
                            session.workspace = next;
                            session.active_tab = None;
                        }
                        browser.reload_session();
                        browser.show_remembered_tab();
                    }
                    (None, Some(manager)) => manager.close_window(browser.id),
                    (None, None) => {}
                }
            },
        );
    }

    /// A workspace other than `except` that no other window shows.
    fn free_workspace(&self, except: WorkspaceId) -> Option<WorkspaceId> {
        let manager = self.manager();
        self.storage
            .workspaces()
            .ok()?
            .into_iter()
            .map(|workspace| workspace.id)
            .find(|workspace| {
                *workspace != except
                    && manager
                        .as_ref()
                        .is_none_or(|manager| !manager.is_open_elsewhere(*workspace, self.id))
            })
    }

    fn shift_workspace(&self, workspace: WorkspaceId, offset: isize) {
        let index = self
            .session
            .borrow()
            .workspaces
            .iter()
            .position(|candidate| candidate.id == workspace);
        let Some(index) = index else { return };
        let target = (index as isize + offset).max(0) as usize;
        let result = self.storage.move_workspace(workspace, target);
        self.report(t!("Workspace verschieben"), result);
        self.reload_session();
        self.refresh_sidebar();
    }

    pub fn create_island(&self) {
        let this = self.this();
        self.prompt_text_or_nothing(
            t!("Neue Insel"),
            t!("Wie soll die Insel heißen? Ohne Namen wird durchnummeriert."),
            t!("Anlegen"),
            move |name| {
                let Some(browser) = this.upgrade() else {
                    return;
                };
                let workspace = browser.session.borrow().workspace;
                let name = if name.is_empty() {
                    let names = browser.storage.island_names(workspace).unwrap_or_default();
                    next_number(&names)
                } else {
                    name
                };
                let color = browser.next_island_color();
                let created = browser.storage.create_island(NewIsland {
                    workspace_id: workspace,
                    name: &name,
                    color,
                    index: None,
                    created_at: Self::now(),
                });
                match created {
                    Ok(island) => {
                        browser.reload_session();
                        browser.new_tab_in_island(island.id);
                    }
                    Err(error) => browser.report(t!("Insel anlegen"), Err::<(), _>(error)),
                }
            },
        );
    }

    pub fn toggle_island(&self, island: IslandId) {
        let Some(collapsed) = self
            .session
            .borrow()
            .island(island)
            .map(|entry| !entry.island.collapsed)
        else {
            return;
        };
        let result = self.storage.set_island_collapsed(island, collapsed);
        self.report(t!("Insel einklappen"), result);
        self.reload_session();
        self.refresh_sidebar();
    }

    pub fn set_island_color(&self, island: IslandId, color: usize) {
        let color = IslandColor::from_index(color as i64);
        let result = self.storage.set_island_color(island, color);
        self.report(t!("Farbe ändern"), result);
        self.reload_session();
        self.refresh_sidebar();
    }

    pub fn island_action(&self, island: IslandId, action: &str) {
        match action {
            "new-tab" => self.new_tab_in_island(island),
            "rename" => self.rename_island(island),
            "sleep" => self.sleep_island(island),
            "export" => self.open_export(Some(island)),
            "move-up" => self.shift_island(island, -1),
            "move-down" => self.shift_island(island, 1),
            "move-top" => self.move_island_to(island, 0),
            "move-bottom" => self.move_island_to(island, usize::MAX),
            "archive" => self.archive_island(island),
            "delete" => self.delete_island(island),
            _ => tracing::warn!(action, "unknown island action"),
        }
    }

    fn rename_island(&self, island: IslandId) {
        let current = self
            .session
            .borrow()
            .island(island)
            .map(|entry| entry.island.name.clone())
            .unwrap_or_default();
        let this = self.this();
        self.prompt_text(
            t!("Insel umbenennen"),
            "",
            &current,
            t!("Umbenennen"),
            move |name| {
                let Some(browser) = this.upgrade() else {
                    return;
                };
                let result = browser.storage.rename_island(island, &name);
                browser.report(t!("Umbenennen"), result);
                browser.reload_session();
                browser.refresh_sidebar();
            },
        );
    }

    fn sleep_island(&self, island: IslandId) {
        let tabs = self.tabs_of_island(island);
        let active = self.session.borrow().active_tab;
        let mut commands = Vec::new();
        {
            let mut lifecycle = self.lifecycle.borrow_mut();
            for tab in tabs.into_iter().filter(|tab| Some(*tab) != active) {
                commands.extend(lifecycle.sleep(tab));
            }
        }
        self.apply(commands);
        self.refresh_sidebar();
    }

    fn shift_island(&self, island: IslandId, offset: isize) {
        let index = self.session.borrow().island_index(island);
        let Some(index) = index else { return };
        self.move_island_to(island, (index as isize + offset).max(0) as usize);
    }

    /// Puts the island at `index` among the islands of its workspace. The index counts the
    /// islands without this one, so `0` is the top and anything past the end is the bottom.
    pub(super) fn move_island_to(&self, island: IslandId, index: usize) {
        let workspace = self.session.borrow().workspace;
        let result = self.storage.move_island(island, workspace, index);
        self.report(t!("Insel verschieben"), result);
        self.reload_session();
        self.refresh_sidebar();
    }

    pub fn move_island_to_workspace(&self, island: IslandId, workspace: WorkspaceId) {
        self.leave_island_if_active(island);
        let result = self.storage.move_island(island, workspace, usize::MAX);
        self.report(t!("Insel verschieben"), result);
        self.reload_session();
        self.refresh_after_structure_change();
    }

    fn archive_island(&self, island: IslandId) {
        self.archive_island_quietly(island);
        self.show_toast(t!("Insel archiviert. Wiederherstellen über das Archiv."));
    }

    /// Archives `island` of this window's workspace.
    pub(super) fn archive_island_quietly(&self, island: IslandId) {
        self.leave_island_if_active(island);
        self.forget_tabs(self.tabs_of_island(island));
        let result = self
            .storage
            .transaction(|transaction| transaction.archive_island(island, Self::now()));
        self.report(t!("Archivieren"), result);
        self.reload_session();
        self.refresh_after_structure_change();
    }

    fn delete_island(&self, island: IslandId) {
        let (name, count) = self
            .session
            .borrow()
            .island(island)
            .map(|entry| (entry.island.name.clone(), entry.tabs.len()))
            .unwrap_or_default();
        let this = self.this();
        self.confirm(
            t!("Insel löschen?"),
            &t!("„{}“ wird mit {} Tabs endgültig gelöscht.", name, count),
            t!("Löschen"),
            move || {
                let Some(browser) = this.upgrade() else {
                    return;
                };
                browser.leave_island_if_active(island);
                browser.forget_tabs(browser.tabs_of_island(island));
                let result = browser.storage.delete_island(island);
                browser.report(t!("Insel löschen"), result);
                browser.reload_session();
                browser.refresh_after_structure_change();
            },
        );
    }

    /// Switches away from the active tab if it belongs to `island`, which is about to disappear.
    fn leave_island_if_active(&self, island: IslandId) {
        let next = {
            let session = self.session.borrow();
            let Some(active) = session.active_tab else {
                return;
            };
            if session.tab(active).map(|tab| tab.island_id) != Some(island) {
                return;
            }
            session
                .tabs()
                .find(|tab| tab.island_id != island)
                .map(|tab| tab.id)
        };
        match next {
            Some(tab) => self.activate_tab(tab),
            None => self.show_empty_workspace(),
        }
    }

    fn refresh_after_structure_change(&self) {
        let active_missing = {
            let session = self.session.borrow();
            session.active_tab.is_none() && session.tabs().next().is_some()
        };
        if active_missing {
            self.show_remembered_tab();
        } else {
            self.sync_page();
            self.refresh_sidebar();
        }
    }

    /// The islands of this window's workspace with their names, in the order they stand.
    fn islands_with_names(&self) -> Vec<(IslandId, String)> {
        self.session
            .borrow()
            .islands
            .iter()
            .map(|entry| (entry.island.id, entry.island.name.clone()))
            .collect()
    }

    /// Sorts the islands named with a number from small to large, among the places they hold.
    pub(super) fn sort_number_islands(&self) {
        let islands = self.islands_with_names();
        let order = sorted_by_number(&islands);
        let moved = islands
            .iter()
            .zip(&order)
            .filter(|((before, _), after)| before != *after)
            .count();
        if moved == 0 {
            self.show_toast(t!(
                "Die Zahleninseln stehen schon in der richtigen Reihenfolge"
            ));
            return;
        }
        let result = self.storage.order_islands(&order);
        self.report(t!("Inseln sortieren"), result);
        self.reload_session();
        self.refresh_sidebar();
        self.show_toast(t!("Zahleninseln sortiert"));
    }

    /// Names the islands named with a number 1, 2, 3 and so on, in the order they stand.
    pub(super) fn renumber_number_islands(&self) {
        let names = renumbered(&self.islands_with_names());
        if names.is_empty() {
            self.show_toast(t!("Die Zahleninseln sind schon durchnummeriert"));
            return;
        }
        let result = self.storage.transaction(|transaction| {
            for (island, name) in &names {
                transaction.rename_island(*island, name)?;
            }
            Ok(())
        });
        self.report(t!("Inseln umbenennen"), result);
        self.reload_session();
        self.refresh_sidebar();
        self.show_toast(&match names.len() {
            1 => t!("1 Zahleninsel umbenannt").to_owned(),
            count => t!("{} Zahleninseln umbenannt", count),
        });
    }

    pub(super) fn tabs_of_island(&self, island: IslandId) -> Vec<TabId> {
        self.session
            .borrow()
            .island(island)
            .map(|entry| entry.tabs.iter().map(|tab| tab.id).collect())
            .unwrap_or_default()
    }
}

/// The number an island is named, if its name is nothing but a number. Spaces around it do not
/// hide it, anything written next to it does.
fn number_of(name: &str) -> Option<u64> {
    name.trim().parse().ok()
}

/// The name a new island gets when the user says none: the largest number any island of the
/// workspace carries, one higher. Names that are no number are passed over, and where there is
/// no number at all the first island is the first number.
fn next_number(names: &[String]) -> String {
    let highest = names.iter().filter_map(|name| number_of(name)).max();
    match highest {
        Some(number) => (number.saturating_add(1)).to_string(),
        None => "1".to_owned(),
    }
}

/// The order of `islands` with the numbered ones sorted from small to large among the places
/// they hold. Islands named otherwise keep their places, equal numbers their order.
fn sorted_by_number(islands: &[(IslandId, String)]) -> Vec<IslandId> {
    let mut numbered: Vec<(u64, IslandId)> = islands
        .iter()
        .filter_map(|(id, name)| number_of(name).map(|number| (number, *id)))
        .collect();
    numbered.sort_by_key(|(number, _)| *number);
    let mut numbered = numbered.into_iter().map(|(_, id)| id);
    islands
        .iter()
        .map(|(id, name)| match number_of(name) {
            Some(_) => numbered.next().unwrap_or(*id),
            None => *id,
        })
        .collect()
}

/// The new names of the numbered islands among `islands`, counted from one in the order they
/// stand. Only the islands whose name changes are named.
fn renumbered(islands: &[(IslandId, String)]) -> Vec<(IslandId, String)> {
    islands
        .iter()
        .filter(|(_, name)| number_of(name).is_some())
        .zip(1u64..)
        .filter(|((_, name), number)| number_of(name) != Some(*number))
        .map(|((id, _), number)| (*id, number.to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn islands(names: &[&str]) -> Vec<(IslandId, String)> {
        names
            .iter()
            .enumerate()
            .map(|(index, name)| (IslandId(index as i64 + 1), (*name).to_owned()))
            .collect()
    }

    #[test]
    fn numbered_islands_are_sorted_among_their_own_places() {
        let order = sorted_by_number(&islands(&["17", "Arbeit", "5", "1", "Lesen", "6"]));
        assert_eq!(
            order,
            [
                IslandId(4),
                IslandId(2),
                IslandId(3),
                IslandId(6),
                IslandId(5),
                IslandId(1)
            ],
            "1, Arbeit, 5, 6, Lesen, 17"
        );
    }

    #[test]
    fn islands_named_otherwise_and_equal_numbers_keep_their_order() {
        let order = sorted_by_number(&islands(&["Insel 7", "3", " 2 ", "3"]));
        assert_eq!(order, [IslandId(1), IslandId(3), IslandId(2), IslandId(4)]);
        assert_eq!(
            sorted_by_number(&islands(&["Arbeit", "Lesen"])),
            [IslandId(1), IslandId(2)]
        );
    }

    #[test]
    fn numbered_islands_are_counted_from_one_in_the_order_they_stand() {
        let names = renumbered(&islands(&["1", "6", "Arbeit", "5", "17"]));
        assert_eq!(
            names,
            [
                (IslandId(2), "2".to_owned()),
                (IslandId(4), "3".to_owned()),
                (IslandId(5), "4".to_owned())
            ],
            "1 stays 1, the island named otherwise is not counted"
        );
        assert!(renumbered(&islands(&["1", "2", "Lesen", "3"])).is_empty());
    }

    #[test]
    fn the_next_island_carries_the_next_number() {
        assert_eq!(next_number(&[]), "1");
        assert_eq!(next_number(&["Tabs".to_owned(), "Lesen".to_owned()]), "1");
        assert_eq!(next_number(&["1".to_owned(), "2".to_owned()]), "3");
        assert_eq!(
            next_number(&["9".to_owned(), "Arbeit".to_owned(), "11".to_owned()]),
            "12",
            "the largest one counts, not the last"
        );
        assert_eq!(
            next_number(&[" 4 ".to_owned()]),
            "5",
            "spaces around a number do not hide it"
        );
        assert_eq!(
            next_number(&["Insel 7".to_owned()]),
            "1",
            "a number with something written next to it is no number"
        );
    }
}
