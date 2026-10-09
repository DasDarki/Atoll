use std::rc::Rc;

use atoll_core::{IslandColor, TabState, WorkspaceId};
use slint::{ComponentHandle, ModelRc, VecModel};

use super::Browser;
use crate::bridge::colors::island_color;
use crate::ui::{
    AppWindow, IslandTarget, RowKind, SidebarRow, SidebarState, TabLoadState, WorkspaceItem,
};

/// The models behind the sidebar. They are attached once and refilled on every change, which
/// keeps the scroll position of the list.
pub(super) struct SidebarModels {
    workspaces: Rc<VecModel<WorkspaceItem>>,
    rows: Rc<VecModel<SidebarRow>>,
    targets: Rc<VecModel<IslandTarget>>,
    windows: Rc<VecModel<WorkspaceItem>>,
}

impl SidebarModels {
    pub(super) fn attach(window: &AppWindow) -> Self {
        let models = Self {
            workspaces: Rc::new(VecModel::default()),
            rows: Rc::new(VecModel::default()),
            targets: Rc::new(VecModel::default()),
            windows: Rc::new(VecModel::default()),
        };
        let state = window.global::<SidebarState>();
        state.set_workspaces(ModelRc::from(models.workspaces.clone()));
        state.set_rows(ModelRc::from(models.rows.clone()));
        state.set_island_targets(ModelRc::from(models.targets.clone()));
        state.set_window_targets(ModelRc::from(models.windows.clone()));
        models
    }
}

impl Browser {
    /// Rebuilds the workspace chips, the island and tab rows and the move targets.
    pub(super) fn refresh_sidebar(&self) {
        let shown_elsewhere = self.workspaces_shown_elsewhere();
        let cutoff = self.archive_cutoff();
        let suggestions = cutoff.is_some() && self.has_archive_suggestions();
        let session = self.session.borrow();
        let lifecycle = self.lifecycle.borrow();
        let views = self.views.borrow();
        let mut favicons = self.favicons.borrow_mut();
        favicons.prepare(&self.storage, session.tabs());

        let workspaces: Vec<WorkspaceItem> = session
            .workspaces
            .iter()
            .map(|workspace| WorkspaceItem {
                id: workspace.id.0 as i32,
                name: workspace.name.as_str().into(),
                active: workspace.id == session.workspace,
                open_elsewhere: shown_elsewhere.contains(&workspace.id),
            })
            .collect();
        let windows: Vec<WorkspaceItem> = workspaces
            .iter()
            .filter(|workspace| workspace.open_elsewhere)
            .cloned()
            .collect();

        let mut rows = Vec::with_capacity(session.islands.len() + session.tabs().count());
        let mut targets = Vec::with_capacity(session.islands.len());
        for entry in &session.islands {
            let color = island_color(entry.island.color);
            targets.push(IslandTarget {
                id: entry.island.id.0 as i32,
                name: entry.island.name.as_str().into(),
                color,
            });
            rows.push(SidebarRow {
                kind: RowKind::Island,
                island_id: entry.island.id.0 as i32,
                title: entry.island.name.as_str().into(),
                color,
                collapsed: entry.island.collapsed,
                count: entry.tabs.len() as i32,
                idle: cutoff.is_some_and(|cutoff| {
                    entry.island.last_used_at.is_none_or(|used| used < cutoff)
                }),
                ..SidebarRow::default()
            });
            if entry.island.collapsed {
                continue;
            }
            for tab in &entry.tabs {
                let state = lifecycle.state_of(tab.id, false);
                let icon = favicons.image(tab);
                let loading = views
                    .get(tab.id)
                    .is_some_and(|entry| entry.view.is_loading());
                rows.push(SidebarRow {
                    kind: RowKind::Tab,
                    private: tab.private,
                    through_tor: tab.private && self.is_tor(tab.id),
                    island_id: entry.island.id.0 as i32,
                    tab_id: tab.id.0 as i32,
                    title: tab.display_title().into(),
                    url: tab.url.as_str().into(),
                    has_icon: icon.is_some(),
                    icon: icon.unwrap_or_default(),
                    color,
                    state: load_state(state),
                    active: session.active_tab == Some(tab.id),
                    pinned: tab.pinned,
                    loading,
                    ..SidebarRow::default()
                });
            }
        }

        self.sidebar.workspaces.set_vec(workspaces);
        self.sidebar.rows.set_vec(rows);
        self.sidebar.targets.set_vec(targets);
        self.sidebar.windows.set_vec(windows);
        if let Some(window) = self.window() {
            window
                .global::<SidebarState>()
                .set_archive_suggestion(suggestions);
        }
        drop(favicons);
        drop(views);
        drop(lifecycle);
        drop(session);
        // Whatever moved the tabs may have added or taken away a tab showing this page.
        self.refresh_open_elsewhere();
    }

    /// The workspaces other windows show.
    pub(super) fn workspaces_shown_elsewhere(&self) -> Vec<WorkspaceId> {
        let mut shown = Vec::new();
        self.for_other_windows(|other| shown.push(other.session.borrow().workspace));
        shown
    }

    pub(super) fn next_island_color(&self) -> IslandColor {
        IslandColor::for_position(self.session.borrow().islands.len())
    }
}

fn load_state(state: TabState) -> TabLoadState {
    match state {
        TabState::Archived => TabLoadState::Archived,
        TabState::Sleeping => TabLoadState::Sleeping,
        TabState::Frozen => TabLoadState::Frozen,
        TabState::Live => TabLoadState::Live,
    }
}
