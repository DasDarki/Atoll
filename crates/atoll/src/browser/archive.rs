use std::rc::Rc;

use std::time::Duration;

use atoll_core::{Island, IslandId, Timestamp};
use atoll_storage::prelude::*;
use slint::{ComponentHandle, ModelRc, VecModel};

use super::Browser;
use crate::bridge::colors::island_color;
use crate::ui::{ArchiveItem, OverlayState};
use atoll_i18n::t;

impl Browser {
    pub fn open_archive(&self) {
        self.refresh_archive();
        if let Some(window) = self.window() {
            window.global::<OverlayState>().set_archive_visible(true);
        }
    }

    pub fn close_archive(&self) {
        if let Some(window) = self.window() {
            window.global::<OverlayState>().set_archive_visible(false);
        }
        self.focus_content();
    }

    pub(super) fn refresh_archive(&self) {
        let Some(window) = self.window() else { return };
        let now = Self::now();
        let workspaces = self.storage.workspaces().unwrap_or_default();
        let suggestions: Vec<ArchiveItem> = self
            .archive_suggestions()
            .into_iter()
            .map(|island| {
                let tab_count = self
                    .storage
                    .tabs_in_island(island.id)
                    .map_or(0, |tabs| tabs.len());
                ArchiveItem {
                    id: island.id.0 as i32,
                    name: island.name.as_str().into(),
                    workspace: workspaces
                        .iter()
                        .find(|workspace| workspace.id == island.workspace_id)
                        .map(|workspace| workspace.name.as_str())
                        .unwrap_or_default()
                        .into(),
                    tab_count: tab_count as i32,
                    archived: island
                        .last_used_at
                        .map(|at| relative_age(now, at))
                        .unwrap_or_default()
                        .into(),
                    color: island_color(island.color),
                }
            })
            .collect();
        let overlay = window.global::<OverlayState>();
        overlay.set_archive_suggestions(ModelRc::from(Rc::new(VecModel::from(suggestions))));
        overlay.set_archive_suggestion_weeks(
            self.preferences.borrow().archive_suggestion_weeks as i32,
        );
        let items: Vec<ArchiveItem> = self
            .storage
            .archived_islands()
            .unwrap_or_default()
            .into_iter()
            .map(|archived| ArchiveItem {
                id: archived.island.id.0 as i32,
                name: archived.island.name.as_str().into(),
                workspace: archived.workspace_name.into(),
                tab_count: archived.tab_count as i32,
                archived: archived
                    .island
                    .archived_at
                    .map(|at| relative_age(now, at))
                    .unwrap_or_default()
                    .into(),
                color: island_color(archived.island.color),
            })
            .collect();
        window
            .global::<OverlayState>()
            .set_archive_items(ModelRc::from(Rc::new(VecModel::from(items))));
    }

    pub fn restore_island(&self, island: IslandId) {
        let result = self
            .storage
            .restore_island(island)
            .and_then(|()| self.storage.touch_island(island, Self::now()));
        self.report(t!("Wiederherstellen"), result);
        self.refresh_archive();
        self.reload_session();
        self.refresh_sidebar();
        self.show_toast(t!("Insel wiederhergestellt"));
    }

    pub fn delete_archived_island(&self, island: IslandId) {
        let name = self
            .storage
            .island(island)
            .map(|record| record.name)
            .unwrap_or_default();
        let this = self.this();
        self.confirm(
            t!("Insel endgültig löschen?"),
            &t!("„{}“ und alle enthaltenen Tabs werden gelöscht.", name),
            t!("Löschen"),
            move || {
                let Some(browser) = this.upgrade() else {
                    return;
                };
                let result = browser.storage.delete_island(island);
                browser.report(t!("Löschen"), result);
                browser.refresh_archive();
            },
        );
    }
}

impl Browser {
    /// The time before which an island counts as unused, `None` if nothing is suggested.
    pub(super) fn archive_cutoff(&self) -> Option<Timestamp> {
        archive_cutoff(
            Self::now(),
            self.preferences.borrow().archive_suggestion_weeks,
        )
    }

    /// Islands of all workspaces unused for longer than configured, least recently used first.
    fn archive_suggestions(&self) -> Vec<Island> {
        self.archive_cutoff()
            .and_then(|cutoff| self.storage.islands_unused_since(cutoff).ok())
            .unwrap_or_default()
    }

    pub(super) fn has_archive_suggestions(&self) -> bool {
        !self.archive_suggestions().is_empty()
    }

    /// Archive suggestion: keeps `island` and suggests it again only after the next period.
    pub fn keep_island(&self, island: IslandId) {
        let result = self.storage.touch_island(island, Self::now());
        self.report(t!("Behalten"), result);
        self.reload_session();
        self.refresh_sidebar();
        self.refresh_archive();
    }

    /// Archive suggestion: archives `island`, whatever workspace it belongs to.
    pub fn archive_suggested_island(&self, island: IslandId) {
        self.archive_island_anywhere(island);
        self.refresh_archive();
        self.show_toast(t!("Insel archiviert"));
    }

    /// Archive suggestion: archives every suggested island.
    pub fn archive_all_suggested(&self) {
        let islands = self.archive_suggestions();
        for island in &islands {
            self.archive_island_anywhere(island.id);
        }
        self.refresh_archive();
        self.show_toast(&match islands.len() {
            1 => t!("1 Insel archiviert").to_owned(),
            count => t!("{} Inseln archiviert", count),
        });
    }

    /// Archives `island` in the window showing its workspace, or directly if none does.
    fn archive_island_anywhere(&self, island: IslandId) {
        let Ok(record) = self.storage.island(island) else {
            return;
        };
        let owner = self
            .manager()
            .and_then(|manager| manager.window_for_workspace(record.workspace_id));
        if let Some(owner) = owner {
            owner.archive_island_quietly(island);
            return;
        }
        let tabs = self
            .storage
            .tabs_in_island(island)
            .map(|tabs| tabs.into_iter().map(|tab| tab.id).collect::<Vec<_>>())
            .unwrap_or_default();
        self.forget_tabs(tabs);
        let result = self.storage.archive_island(island, Self::now());
        self.report(t!("Archivieren"), result);
        self.reload_session();
        self.refresh_sidebar();
    }
}

/// Islands last used before the returned time are suggested for the archive.
fn archive_cutoff(now: Timestamp, weeks: u32) -> Option<Timestamp> {
    (weeks > 0).then(|| {
        let period = Duration::from_secs(u64::from(weeks) * 7 * 86_400);
        Timestamp(now.0 - period.as_millis() as i64)
    })
}

fn relative_age(now: Timestamp, then: Timestamp) -> String {
    let days = now.elapsed_since(then).as_secs() / 86_400;
    match days {
        0 => "heute".to_owned(),
        1 => "gestern".to_owned(),
        2..=13 => t!("vor {} Tagen", days),
        14..=59 => t!("vor {} Wochen", days / 7),
        _ => t!("vor {} Monaten", days / 30),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 86_400_000;

    #[test]
    fn suggestions_start_after_the_configured_weeks() {
        let now = Timestamp(100 * DAY);
        assert_eq!(archive_cutoff(now, 0), None);
        assert_eq!(archive_cutoff(now, 4), Some(Timestamp(72 * DAY)));
    }

    #[test]
    fn ages_are_rounded_to_days_weeks_and_months() {
        let now = Timestamp(400 * DAY);
        assert_eq!(relative_age(now, Timestamp(400 * DAY - 1000)), "heute");
        assert_eq!(relative_age(now, Timestamp(399 * DAY)), "gestern");
        assert_eq!(relative_age(now, Timestamp(390 * DAY)), "vor 10 Tagen");
        assert_eq!(relative_age(now, Timestamp(365 * DAY)), "vor 5 Wochen");
        assert_eq!(relative_age(now, Timestamp(300 * DAY)), "vor 3 Monaten");
    }
}
