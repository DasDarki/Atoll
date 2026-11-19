//! The archive: tabs and islands taken out of the workspaces without being thrown away.
//!
//! Islands go there when the user says so or follows a suggestion, single tabs on their own once
//! they have not been visited for as long as the settings allow. The archive is laid out like
//! the sidebar and brings everything back where it came from, see
//! [`atoll_storage::ArchiveStore`].

use std::collections::BTreeSet;
use std::rc::Rc;
use std::time::Duration;

use atoll_core::address::display_url;
use atoll_core::search::Matcher;
use atoll_core::{Island, IslandId, TabId, Timestamp};
use atoll_storage::prelude::*;
use atoll_storage::{ArchiveReason, ArchivedIsland, ArchivedTab, ArchivedWorkspace};
use slint::{Color, ComponentHandle, ModelRc, VecModel};

use super::Browser;
use crate::bridge::colors::island_color;
use crate::ui::{ArchiveItem, ArchiveRow, ArchiveRowKind, OverlayState};
use atoll_i18n::t;

const DAY: Duration = Duration::from_secs(86_400);

impl Browser {
    pub fn open_archive(&self) {
        if let Some(window) = self.window() {
            window.global::<OverlayState>().set_archive_query("".into());
        }
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

    pub fn archive_query_edited(&self, query: &str) {
        if let Some(window) = self.window() {
            window
                .global::<OverlayState>()
                .set_archive_query(query.into());
        }
        self.refresh_archive();
    }

    pub(super) fn refresh_archive(&self) {
        let Some(window) = self.window() else { return };
        let now = Self::now();
        let overlay = window.global::<OverlayState>();
        let suggestions: Vec<ArchiveItem> = self
            .archive_suggestions()
            .into_iter()
            .map(|island| self.suggestion_item(&island, now))
            .collect();
        overlay.set_archive_suggestions(ModelRc::from(Rc::new(VecModel::from(suggestions))));
        overlay.set_archive_suggestion_weeks(
            self.preferences.borrow().archive_suggestion_weeks as i32,
        );

        let archive = self.storage.archive().unwrap_or_default();
        let matcher = Matcher::new(overlay.get_archive_query().as_str());
        let rows = self.archive_rows(&archive, &matcher, now);
        overlay.set_archive_tab_count(
            archive
                .iter()
                .flat_map(|workspace| &workspace.islands)
                .map(|island| island.tabs.len() as i32)
                .sum(),
        );
        overlay.set_archive_rows(ModelRc::from(Rc::new(VecModel::from(rows))));
    }

    fn suggestion_item(&self, island: &Island, now: Timestamp) -> ArchiveItem {
        let workspaces = self.storage.workspaces().unwrap_or_default();
        ArchiveItem {
            id: island.id.0 as i32,
            name: island.name.as_str().into(),
            workspace: workspaces
                .iter()
                .find(|workspace| workspace.id == island.workspace_id)
                .map(|workspace| workspace.name.as_str())
                .unwrap_or_default()
                .into(),
            tab_count: self
                .storage
                .tabs_in_island(island.id)
                .map_or(0, |tabs| tabs.len()) as i32,
            archived: island
                .last_used_at
                .map(|at| relative_age(now, at))
                .unwrap_or_default()
                .into(),
            color: island_color(island.color),
        }
    }

    /// The lines of the archive as the sidebar would show them. While searching, a workspace or
    /// an island whose name matches shows all of its tabs, otherwise only the tabs that match.
    fn archive_rows(
        &self,
        archive: &[ArchivedWorkspace],
        matcher: &Matcher,
        now: Timestamp,
    ) -> Vec<ArchiveRow> {
        let mut favicons = self.favicons.borrow_mut();
        let mut rows = Vec::new();
        for workspace in archive {
            let workspace_matches =
                !matcher.is_empty() && matcher.score(&[&workspace.name]).is_some();
            let mut workspace_rows = Vec::new();
            for island in &workspace.islands {
                let island_matches = workspace_matches
                    || (!matcher.is_empty() && matcher.score(&[&island.name]).is_some());
                let tabs: Vec<&ArchivedTab> = island
                    .tabs
                    .iter()
                    .filter(|tab| {
                        matcher.is_empty()
                            || island_matches
                            || matcher
                                .score(&[tab.display_title(), &display_url(&tab.url)])
                                .is_some()
                    })
                    .collect();
                if tabs.is_empty() {
                    continue;
                }
                workspace_rows.push(island_row(island));
                for tab in tabs {
                    let icon = favicons.image_for_url(&self.storage, &tab.url);
                    workspace_rows.push(ArchiveRow {
                        kind: ArchiveRowKind::Tab,
                        id: tab.id as i32,
                        name: tab.display_title().into(),
                        detail: t!(
                            "{} · archiviert {}",
                            display_url(&tab.url),
                            relative_age(now, tab.archived_at)
                        )
                        .into(),
                        color: Color::default(),
                        has_icon: icon.is_some(),
                        icon: icon.unwrap_or_default(),
                    });
                }
            }
            if workspace_rows.is_empty() {
                continue;
            }
            rows.push(workspace_row(workspace));
            rows.extend(workspace_rows);
        }
        rows
    }

    /// Brings a tab, an island or a workspace back from the archive.
    pub fn restore_from_archive(&self, kind: ArchiveRowKind, id: i64) {
        let now = Self::now();
        let restored = self.storage.transaction(|transaction| match kind {
            ArchiveRowKind::Tab => transaction
                .restore_archived_tab(id, now)
                .map(|tab| vec![tab]),
            ArchiveRowKind::Island => transaction.restore_archived_island(id, now),
            ArchiveRowKind::Workspace => transaction.restore_archived_workspace(id, now),
        });
        let restored = match restored {
            Ok(restored) => restored,
            Err(error) => {
                self.report(t!("Wiederherstellen"), Err(error));
                return;
            }
        };
        self.reload_session();
        self.refresh_sidebar();
        self.refresh_archive();
        self.show_toast(&self.restored_message(&restored));
    }

    /// Says what came back and where it went.
    fn restored_message(&self, restored: &[TabId]) -> String {
        let island = restored
            .first()
            .and_then(|tab| self.storage.tab(*tab).ok())
            .and_then(|tab| self.storage.island(tab.island_id).ok());
        match (restored.len(), island) {
            (1, Some(island)) => t!("Tab nach „{}“ zurückgeholt", island.name),
            (count, _) => t!("{} Tabs zurückgeholt", count),
        }
    }

    /// Brings an archived tab back and shows it, for the palette.
    pub(super) fn restore_archived_tab_and_show(&self, id: i64) {
        let restored = self
            .storage
            .transaction(|transaction| transaction.restore_archived_tab(id, Self::now()));
        match restored {
            Ok(tab) => {
                self.reload_session();
                self.reveal_tab(tab);
                self.show_toast(t!("Tab aus dem Archiv geholt"));
            }
            Err(error) => self.report(t!("Wiederherstellen"), Err(error)),
        }
    }

    /// Throws a tab, an island or a workspace out of the archive. A single tab goes at once, a
    /// group only after asking.
    pub fn delete_from_archive(&self, kind: ArchiveRowKind, id: i64) {
        if kind == ArchiveRowKind::Tab {
            let result = self.storage.delete_archived_tab(id);
            self.report(t!("Löschen"), result);
            self.refresh_archive();
            return;
        }
        let archive = self.storage.archive().unwrap_or_default();
        let (name, count) =
            match kind {
                ArchiveRowKind::Workspace => archive
                    .iter()
                    .find(|workspace| workspace.id == id)
                    .map(|workspace| {
                        (
                            workspace.name.clone(),
                            workspace
                                .islands
                                .iter()
                                .map(|island| island.tabs.len())
                                .sum(),
                        )
                    }),
                _ => archive
                    .iter()
                    .flat_map(|workspace| &workspace.islands)
                    .find(|island| island.id == id)
                    .map(|island| (island.name.clone(), island.tabs.len())),
            }
            .unwrap_or_default();
        let this = self.this();
        self.confirm(
            t!("Aus dem Archiv löschen?"),
            &t!(
                "„{}“ und {} archivierte Tabs werden endgültig gelöscht.",
                name,
                count
            ),
            t!("Löschen"),
            move || {
                let Some(browser) = this.upgrade() else {
                    return;
                };
                let result = match kind {
                    ArchiveRowKind::Workspace => browser.storage.delete_archived_workspace(id),
                    _ => browser.storage.delete_archived_island(id),
                };
                browser.report(t!("Löschen"), result);
                browser.refresh_archive();
            },
        );
    }

    /// The tab menu: archives `tab`, or every picked tab when it is one of them. The shown tab
    /// may go too, the nearest one that stays is shown instead.
    pub fn archive_tab_from_sidebar(&self, tab: TabId) {
        let tabs: Vec<TabId> = if self.is_picked(tab) {
            self.picked_tabs()
        } else {
            vec![tab]
        };
        let (tabs, islands, next, was_active) = {
            let session = self.session.borrow();
            let tabs: Vec<TabId> = tabs
                .into_iter()
                .filter(|tab| session.tab(*tab).is_some_and(|record| !record.private))
                .collect();
            let islands: BTreeSet<IslandId> = tabs
                .iter()
                .filter_map(|tab| session.tab(*tab).map(|record| record.island_id))
                .collect();
            let was_active = session
                .active_tab
                .is_some_and(|active| tabs.contains(&active));
            let next = session
                .active_tab
                .filter(|_| was_active)
                .and_then(|active| {
                    nearest_staying(&session.navigation_order(Some(active)), active, &tabs)
                });
            (tabs, islands, next, was_active)
        };
        if tabs.is_empty() {
            self.show_toast(t!("Private Tabs kommen nicht ins Archiv"));
            return;
        }
        for tab in &tabs {
            self.forget_page_text(*tab);
            self.forget_autofilled(*tab);
        }
        self.forget_tabs(tabs.iter().copied());
        self.forget_picks_quietly();
        let result = self.take_into_archive(&tabs, &islands, Self::now(), ArchiveReason::Asked);
        if let Err(error) = result {
            self.report(t!("Archivieren"), Err(error));
            self.reload_session();
            self.refresh_sidebar();
            return;
        }
        for tab in &tabs {
            self.plugin_tab_closed(*tab);
        }
        self.reload_session();
        match (was_active, next) {
            (true, Some(next)) => self.activate_tab(next),
            (true, None) => self.show_empty_workspace(),
            _ => self.refresh_sidebar(),
        }
        self.refresh_archive();
        self.show_toast(&match tabs.len() {
            1 => t!("Tab archiviert").to_owned(),
            count => t!("{} Tabs archiviert", count),
        });
    }

    /// Moves `tabs` into the archive in one go and takes the islands they leave empty along.
    fn take_into_archive(
        &self,
        tabs: &[TabId],
        islands: &BTreeSet<IslandId>,
        now: Timestamp,
        reason: ArchiveReason,
    ) -> atoll_storage::StorageResult<()> {
        self.storage.transaction(|transaction| {
            transaction.archive_tabs(tabs, now, reason)?;
            for island in islands {
                if transaction.tabs_in_island(*island)?.is_empty() {
                    transaction.delete_island(*island)?;
                }
            }
            Ok(())
        })
    }

    /// Moves the tabs not visited for longer than the settings allow into the archive. Tabs
    /// that are shown, the last shown tab of every workspace, tabs with a loaded page and those
    /// the archive never takes on its own stay. An island left empty goes along with its tabs,
    /// the first of them to come back brings it back. Answers with how many went.
    fn archive_unused_tabs(&self, shown: &[TabId]) -> usize {
        let days = self.preferences.borrow().archive_tabs_after_days;
        let now = Self::now();
        let Some(cutoff) = unused_cutoff(now, days) else {
            return 0;
        };
        let remembered: Vec<TabId> = self
            .storage
            .workspaces()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|workspace| {
                self.storage
                    .setting_i64(&Self::active_workspace_key(workspace.id))
                    .ok()
                    .flatten()
                    .map(TabId)
            })
            .collect();
        let due: Vec<(TabId, IslandId)> = {
            let lifecycle = self.lifecycle.borrow();
            self.storage
                .tabs_unused_since(cutoff)
                .unwrap_or_default()
                .into_iter()
                .filter(|tab| {
                    !shown.contains(&tab.id)
                        && !remembered.contains(&tab.id)
                        && !lifecycle.has_engine_instance(tab.id)
                })
                .map(|tab| (tab.id, tab.island_id))
                .collect()
        };
        if due.is_empty() {
            return 0;
        }
        let tabs: Vec<TabId> = due.iter().map(|(tab, _)| *tab).collect();
        let islands: BTreeSet<IslandId> = due.iter().map(|(_, island)| *island).collect();
        self.forget_tabs(tabs.iter().copied());
        let result = self.take_into_archive(&tabs, &islands, now, ArchiveReason::Unused);
        tracing::info!(count = tabs.len(), days, "unused tabs went to the archive");
        let archived = if result.is_ok() { tabs.len() } else { 0 };
        self.report(t!("Archivieren"), result);
        self.reload_session();
        self.for_all_windows(Browser::refresh_sidebar);
        archived
    }

    /// The regular look: archives what is due and says so in the window in front.
    pub(super) fn archive_unused_tabs_and_tell(&self, shown: &[TabId]) {
        let count = self.archive_unused_tabs(shown);
        if count == 0 {
            return;
        }
        let toast = match count {
            1 => t!("1 Tab ins Archiv gelegt, lange nicht besucht").to_owned(),
            count => t!("{} Tabs ins Archiv gelegt, lange nicht besucht", count),
        };
        match self.manager().and_then(|manager| manager.focused_browser()) {
            Some(focused) => focused.show_toast(&toast),
            None => self.show_toast(&toast),
        }
    }

    /// Brings the archive in line with the settings: every tab that went on its own but would
    /// not be due under the number of days set now comes back, all of them while archiving on
    /// its own is off, and what is due now goes at once instead of at the next look. Tabs
    /// archived by hand are left where they are.
    pub fn sync_archive_with_settings(&self) {
        let days = self.preferences.borrow().archive_tabs_after_days;
        let now = Self::now();
        let since = unused_cutoff(now, days);
        let restored = self
            .storage
            .transaction(|transaction| transaction.restore_unused_tabs_visited_since(since, now));
        let restored = match restored {
            Ok(restored) => restored.len(),
            Err(error) => {
                self.report(t!("Wiederherstellen"), Err(error));
                return;
            }
        };
        if restored > 0 {
            self.reload_session();
            self.for_all_windows(Browser::refresh_sidebar);
        }
        let shown = self
            .manager()
            .map(|manager| manager.shown_tabs())
            .unwrap_or_default();
        let archived = self.archive_unused_tabs(&shown);
        tracing::info!(restored, archived, days, "archive synced with the settings");
        self.refresh_archive();
        self.show_toast(&match (restored, archived) {
            (0, 0) => t!("Das Archiv passt schon zu den Einstellungen").to_owned(),
            (restored, 0) => t!("{} aus dem Archiv zurückgeholt", tab_count(restored)),
            (0, archived) => t!("{} ins Archiv gelegt", tab_count(archived)),
            (restored, archived) => t!(
                "{} zurückgeholt, {} ins Archiv gelegt",
                tab_count(restored),
                tab_count(archived)
            ),
        });
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
        let result = self
            .storage
            .transaction(|transaction| transaction.archive_island(island, Self::now()));
        self.report(t!("Archivieren"), result);
        self.reload_session();
        self.refresh_sidebar();
    }
}

/// The tab shown when `active` goes away with `leaving`: the next one that stays, or the one
/// before when nothing stays after it.
fn nearest_staying(order: &[TabId], active: TabId, leaving: &[TabId]) -> Option<TabId> {
    let at = order.iter().position(|tab| *tab == active)?;
    let stays = |tab: &&TabId| !leaving.contains(tab);
    order[at..]
        .iter()
        .find(stays)
        .or_else(|| order[..at].iter().rev().find(stays))
        .copied()
}

fn workspace_row(workspace: &ArchivedWorkspace) -> ArchiveRow {
    let count: usize = workspace
        .islands
        .iter()
        .map(|island| island.tabs.len())
        .sum();
    ArchiveRow {
        kind: ArchiveRowKind::Workspace,
        id: workspace.id as i32,
        name: workspace.name.as_str().into(),
        detail: tab_count(count).into(),
        color: Color::default(),
        has_icon: false,
        icon: Default::default(),
    }
}

fn island_row(island: &ArchivedIsland) -> ArchiveRow {
    ArchiveRow {
        kind: ArchiveRowKind::Island,
        id: island.id as i32,
        name: island.name.as_str().into(),
        detail: tab_count(island.tabs.len()).into(),
        color: island_color(island.color),
        has_icon: false,
        icon: Default::default(),
    }
}

fn tab_count(count: usize) -> String {
    match count {
        1 => t!("1 Tab").to_owned(),
        count => t!("{} Tabs", count),
    }
}

/// Islands last used before the returned time are suggested for the archive.
fn archive_cutoff(now: Timestamp, weeks: u32) -> Option<Timestamp> {
    (weeks > 0).then(|| Timestamp(now.0 - (DAY * 7 * weeks).as_millis() as i64))
}

/// Tabs last visited before the returned time go to the archive on their own.
fn unused_cutoff(now: Timestamp, days: u32) -> Option<Timestamp> {
    (days > 0).then(|| Timestamp(now.0 - (DAY * days).as_millis() as i64))
}

fn relative_age(now: Timestamp, then: Timestamp) -> String {
    let days = now.elapsed_since(then).as_secs() / 86_400;
    match days {
        0 => t!("heute").to_owned(),
        1 => t!("gestern").to_owned(),
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
    fn tabs_go_after_the_configured_days_and_never_while_it_is_off() {
        let now = Timestamp(100 * DAY);
        assert_eq!(unused_cutoff(now, 0), None);
        assert_eq!(unused_cutoff(now, 30), Some(Timestamp(70 * DAY)));
    }

    #[test]
    fn the_shown_tab_hands_over_to_the_nearest_one_that_stays() {
        let order = [TabId(1), TabId(2), TabId(3), TabId(4)];
        assert_eq!(
            nearest_staying(&order, TabId(2), &[TabId(2), TabId(3)]),
            Some(TabId(4))
        );
        assert_eq!(
            nearest_staying(&order, TabId(3), &[TabId(3), TabId(4)]),
            Some(TabId(2)),
            "nothing stays after it, so the one before"
        );
        assert_eq!(nearest_staying(&order, TabId(1), &order), None);
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
