use std::rc::Rc;

use atoll_core::{IslandId, Timestamp};
use atoll_storage::prelude::*;
use slint::{ComponentHandle, ModelRc, VecModel};

use super::Browser;
use crate::bridge::colors::island_color;
use crate::ui::{ArchiveItem, OverlayState};

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

    fn refresh_archive(&self) {
        let Some(window) = self.window() else { return };
        let now = Self::now();
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
        let result = self.storage.restore_island(island);
        self.report("Wiederherstellen", result);
        self.refresh_archive();
        self.reload_session();
        self.refresh_sidebar();
        self.show_toast("Insel wiederhergestellt");
    }

    pub fn delete_archived_island(&self, island: IslandId) {
        let name = self
            .storage
            .island(island)
            .map(|record| record.name)
            .unwrap_or_default();
        let this = self.this();
        self.confirm(
            "Insel endgültig löschen?",
            &format!("„{name}“ und alle enthaltenen Tabs werden gelöscht."),
            "Löschen",
            move || {
                let Some(browser) = this.upgrade() else {
                    return;
                };
                let result = browser.storage.delete_island(island);
                browser.report("Löschen", result);
                browser.refresh_archive();
            },
        );
    }
}

fn relative_age(now: Timestamp, then: Timestamp) -> String {
    let days = now.elapsed_since(then).as_secs() / 86_400;
    match days {
        0 => "heute".to_owned(),
        1 => "gestern".to_owned(),
        2..=13 => format!("vor {days} Tagen"),
        14..=59 => format!("vor {} Wochen", days / 7),
        _ => format!("vor {} Monaten", days / 30),
    }
}
