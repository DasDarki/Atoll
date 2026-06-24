use std::rc::Rc;

use atoll_core::{IslandColor, IslandId, WorkspaceId};
use atoll_storage::NewIsland;
use atoll_storage::prelude::*;
use atoll_transfer::{
    DuplicatePolicy, ExistingIsland, ExistingTab, ExportGroup, ExportOptions, ImportOptions,
    ImportPlan, ImportTarget, IslandDestination, IslandNamePolicy, PlannedEntry, SkipReason,
    export, parse, plan_import,
};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use super::Browser;
use crate::platform::dialogs::{self, FileDialogKind, FileDialogOptions, FileFilter};
use crate::ui::{ExportIslandItem, OverlayState};

const SCOPE_ISLAND: i32 = 0;
const SCOPE_WORKSPACE: i32 = 1;
const SCOPE_ALL: i32 = 2;
const SCOPE_SELECTION: i32 = 3;

/// State of the import and export dialogs that is not kept in the user interface.
#[derive(Default)]
pub struct TransferState {
    export_island: Option<IslandId>,
}

impl Browser {
    pub fn open_import(&self) {
        let Some(window) = self.window() else { return };
        let state = window.global::<OverlayState>();
        state.set_import_text("".into());
        state.set_import_target(0);
        state.set_import_workspace_name("Import".into());
        state.set_import_duplicates(1);
        state.set_import_islands(0);
        state.set_import_fallback("Importiert".into());
        state.set_import_visible(true);
        self.import_text_edited("");
    }

    pub fn import_text_edited(&self, text: &str) {
        let Some(window) = self.window() else { return };
        let state = window.global::<OverlayState>();
        let list = parse(text);
        let urls = list.url_count();
        let summary = if text.trim().is_empty() {
            "Füge eine URL-Liste ein oder wähle eine Datei.".to_owned()
        } else if urls == 0 {
            format!(
                "Keine gültigen Adressen gefunden · {} ungültige Zeilen",
                list.invalid.len()
            )
        } else {
            let islands = list.groups.len();
            let mut summary = format!(
                "{urls} Adressen in {islands} {}",
                plural(islands, "Insel", "Inseln")
            );
            if state.get_import_target() == 0 {
                let plan = plan_import(
                    &list,
                    &self.import_target(self.session.borrow().workspace),
                    &self.import_options(&state),
                );
                let skipped = plan.skipped.len();
                let moved = plan.moved_count();
                if skipped > 0 {
                    summary.push_str(&format!(" · {skipped} übersprungen"));
                }
                if moved > 0 {
                    summary.push_str(&format!(" · {moved} verschoben"));
                }
            }
            if !list.invalid.is_empty() {
                let invalid = list.invalid.len();
                summary.push_str(&format!(
                    " · {invalid} {}",
                    plural(invalid, "ungültige Zeile", "ungültige Zeilen")
                ));
            }
            summary
        };
        state.set_import_summary(summary.into());
        state.set_import_ready(urls > 0 || list.groups.iter().any(|group| group.name.is_some()));
    }

    pub fn import_choose_file(&self) {
        let this = self.this();
        dialogs::show(
            FileDialogOptions {
                kind: FileDialogKind::Open,
                title: "URL-Liste öffnen".into(),
                directory: None,
                file_name: None,
                filters: vec![
                    FileFilter {
                        name: "Textdateien".into(),
                        extensions: vec!["txt".into(), "text".into(), "list".into()],
                    },
                    FileFilter {
                        name: "Alle Dateien".into(),
                        extensions: vec!["*".into()],
                    },
                ],
            },
            move |paths| {
                let Some(browser) = this.upgrade() else {
                    return;
                };
                let Some(path) = paths.and_then(|paths| paths.into_iter().next()) else {
                    return;
                };
                match std::fs::read(&path) {
                    Ok(bytes) => browser.set_import_text(&String::from_utf8_lossy(&bytes)),
                    Err(error) => {
                        browser.show_toast(&format!("Datei konnte nicht gelesen werden: {error}"))
                    }
                }
            },
        );
    }

    pub fn import_paste(&self) {
        match self.clipboard.text() {
            Some(text) => self.set_import_text(&text),
            None => self.show_toast("Die Zwischenablage enthält keinen Text"),
        }
    }

    fn set_import_text(&self, text: &str) {
        if let Some(window) = self.window() {
            window.global::<OverlayState>().set_import_text(text.into());
        }
        self.import_text_edited(text);
    }

    pub fn import_cancel(&self) {
        if let Some(window) = self.window() {
            window.global::<OverlayState>().set_import_visible(false);
        }
        self.focus_content();
    }

    pub fn import_confirm(&self) {
        let Some(window) = self.window() else { return };
        let state = window.global::<OverlayState>();
        let list = parse(&state.get_import_text());
        let options = self.import_options(&state);

        let workspace = if state.get_import_target() == 1 {
            let name = state.get_import_workspace_name().trim().to_owned();
            let name = if name.is_empty() {
                "Import".to_owned()
            } else {
                name
            };
            match self.storage.create_workspace(&name) {
                Ok(workspace) => workspace.id,
                Err(error) => return self.report("Import", Err::<(), _>(error)),
            }
        } else {
            self.session.borrow().workspace
        };

        let plan = plan_import(&list, &self.import_target(workspace), &options);
        let applied = self.apply_import(workspace, &plan);
        state.set_import_visible(false);
        if let Err(error) = applied {
            return self.report("Import", Err::<(), _>(error));
        }

        if workspace != self.session.borrow().workspace {
            self.select_workspace(workspace);
        } else {
            self.reload_session();
            self.refresh_sidebar();
            if self.session.borrow().active_tab.is_none() {
                self.show_remembered_tab();
            }
        }
        self.show_import_report(&plan);
    }

    fn import_options(&self, state: &OverlayState<'_>) -> ImportOptions {
        let fallback = state.get_import_fallback().trim().to_owned();
        ImportOptions {
            duplicates: match state.get_import_duplicates() {
                0 => DuplicatePolicy::Keep,
                2 => DuplicatePolicy::Merge,
                _ => DuplicatePolicy::Skip,
            },
            island_names: if state.get_import_islands() == 1 {
                IslandNamePolicy::CreateNew
            } else {
                IslandNamePolicy::Merge
            },
            fallback_island: if fallback.is_empty() {
                "Importiert".to_owned()
            } else {
                fallback
            },
        }
    }

    fn import_target(&self, workspace: WorkspaceId) -> ImportTarget {
        let islands = self
            .storage
            .islands(workspace)
            .unwrap_or_default()
            .into_iter()
            .map(|island| ExistingIsland {
                id: island.id,
                name: island.name,
            })
            .collect();
        let tabs = self
            .storage
            .all_tabs_in_workspace(workspace)
            .unwrap_or_default()
            .into_iter()
            .filter(|tab| !tab.is_blank())
            .map(|tab| ExistingTab {
                id: tab.id,
                url: tab.url,
            })
            .collect();
        ImportTarget { islands, tabs }
    }

    fn apply_import(
        &self,
        workspace: WorkspaceId,
        plan: &ImportPlan,
    ) -> atoll_storage::StorageResult<()> {
        let existing_islands = self.storage.islands(workspace)?.len();
        let now = Self::now();
        self.storage.transaction(|transaction| {
            let mut created = 0;
            for planned in &plan.islands {
                let island = match planned.destination {
                    IslandDestination::Existing(id) => id,
                    IslandDestination::New => {
                        let island = transaction.create_island(NewIsland {
                            workspace_id: workspace,
                            name: &planned.name,
                            color: IslandColor::for_position(existing_islands + created),
                            index: None,
                        })?;
                        created += 1;
                        island.id
                    }
                };
                for entry in &planned.entries {
                    match entry {
                        PlannedEntry::Create { url } => {
                            transaction.create_tab(atoll_storage::NewTab {
                                island_id: island,
                                url,
                                title: None,
                                index: None,
                                created_at: now,
                            })?;
                        }
                        PlannedEntry::Move { tab, .. } => {
                            transaction.move_tab(*tab, island, usize::MAX)?
                        }
                    }
                }
            }
            Ok(())
        })
    }

    fn show_import_report(&self, plan: &ImportPlan) {
        let created = plan.created_count();
        let moved = plan.moved_count();
        let islands = plan.new_island_count();
        let mut summary = format!(
            "{created} {} importiert, {islands} neue {}.",
            plural(created, "Tab", "Tabs"),
            plural(islands, "Insel", "Inseln")
        );
        if moved > 0 {
            summary.push_str(&format!(" {moved} vorhandene Tabs wurden verschoben."));
        }
        if plan.invalid.is_empty() && plan.skipped.is_empty() {
            self.show_toast(&summary);
            return;
        }

        let mut lines: Vec<SharedString> = Vec::new();
        for invalid in &plan.invalid {
            lines.push(
                format!(
                    "Zeile {}: {} ({})",
                    invalid.line_number, invalid.content, invalid.reason
                )
                .into(),
            );
        }
        for skipped in &plan.skipped {
            let reason = match skipped.reason {
                SkipReason::AlreadyPresent => "bereits vorhanden",
                SkipReason::RepeatedInList => "doppelt in der Liste",
            };
            lines.push(format!("Übersprungen, {reason}: {}", skipped.url).into());
        }
        let invalid = plan.invalid.len();
        let skipped = plan.skipped.len();
        summary.push_str(&format!(
            " {invalid} {}, {skipped} {}:",
            plural(invalid, "ungültige Zeile", "ungültige Zeilen"),
            plural(skipped, "übersprungene Adresse", "übersprungene Adressen")
        ));

        let Some(window) = self.window() else { return };
        let state = window.global::<OverlayState>();
        state.set_report_summary(summary.into());
        state.set_report_lines(ModelRc::from(Rc::new(VecModel::from(lines))));
        state.set_report_visible(true);
    }

    pub fn close_report(&self) {
        if let Some(window) = self.window() {
            window.global::<OverlayState>().set_report_visible(false);
        }
        self.focus_content();
    }

    /// Opens the export dialog. `island` preselects the "current island" scope.
    pub fn open_export(&self, island: Option<IslandId>) {
        let Some(window) = self.window() else { return };
        let active_island = {
            let session = self.session.borrow();
            session
                .active_tab
                .and_then(|tab| session.tab(tab))
                .map(|tab| tab.island_id)
        };
        self.transfer.borrow_mut().export_island = island.or(active_island);

        let workspaces = self.storage.workspaces().unwrap_or_default();
        let current = self.session.borrow().workspace;
        let mut items = Vec::new();
        for workspace in &workspaces {
            for island in self.storage.islands(workspace.id).unwrap_or_default() {
                items.push(ExportIslandItem {
                    id: island.id.0 as i32,
                    name: island.name.into(),
                    workspace: workspace.name.as_str().into(),
                    archived: false,
                    selected: workspace.id == current,
                });
            }
        }
        for archived in self.storage.archived_islands().unwrap_or_default() {
            items.push(ExportIslandItem {
                id: archived.island.id.0 as i32,
                name: archived.island.name.into(),
                workspace: archived.workspace_name.into(),
                archived: true,
                selected: false,
            });
        }

        let state = window.global::<OverlayState>();
        state.set_export_islands(ModelRc::from(Rc::new(VecModel::from(items))));
        state.set_export_scope(if island.is_some() {
            SCOPE_ISLAND
        } else {
            SCOPE_WORKSPACE
        });
        state.set_export_markers(true);
        state.set_export_archived(false);
        state.set_export_destination(0);
        state.set_export_visible(true);
        self.export_changed();
    }

    pub fn export_toggle_island(&self, island: i32) {
        let Some(window) = self.window() else { return };
        let model = window.global::<OverlayState>().get_export_islands();
        for row in 0..model.row_count() {
            if let Some(mut item) = model.row_data(row)
                && item.id == island
            {
                item.selected = !item.selected;
                model.set_row_data(row, item);
            }
        }
        self.export_changed();
    }

    pub fn export_changed(&self) {
        let Some(window) = self.window() else { return };
        let groups = self.export_groups();
        let urls: usize = groups
            .iter()
            .map(|group| group.urls.iter().filter(|url| !url.is_empty()).count())
            .sum();
        let summary = format!(
            "{urls} {} in {} {}",
            plural(urls, "Adresse", "Adressen"),
            groups.len(),
            plural(groups.len(), "Insel", "Inseln")
        );
        window
            .global::<OverlayState>()
            .set_export_summary(summary.into());
    }

    pub fn export_cancel(&self) {
        if let Some(window) = self.window() {
            window.global::<OverlayState>().set_export_visible(false);
        }
        self.focus_content();
    }

    pub fn export_confirm(&self) {
        let Some(window) = self.window() else { return };
        let state = window.global::<OverlayState>();
        let groups = self.export_groups();
        let text = export(
            &groups,
            ExportOptions {
                island_markers: state.get_export_markers(),
            },
        );
        let count = text
            .lines()
            .filter(|line| !line.is_empty() && !line.starts_with("---"))
            .count();
        state.set_export_visible(false);

        if state.get_export_destination() == 0 {
            if self.clipboard.set_text(&text) {
                self.show_toast(&format!(
                    "{count} {} in die Zwischenablage kopiert",
                    plural(count, "Adresse", "Adressen")
                ));
            } else {
                self.show_toast("Zwischenablage nicht verfügbar");
            }
            return;
        }

        let file_name = match (state.get_export_scope(), groups.first()) {
            (SCOPE_ISLAND, Some(group)) => format!("{}.txt", sanitize_file_stem(&group.name)),
            _ => "atoll-tabs.txt".to_owned(),
        };
        let this = self.this();
        dialogs::show(
            FileDialogOptions {
                kind: FileDialogKind::Save,
                title: "URL-Liste speichern".into(),
                directory: None,
                file_name: Some(file_name),
                filters: vec![FileFilter {
                    name: "Textdateien".into(),
                    extensions: vec!["txt".into()],
                }],
            },
            move |paths| {
                let Some(browser) = this.upgrade() else {
                    return;
                };
                let Some(path) = paths.and_then(|paths| paths.into_iter().next()) else {
                    return;
                };
                match std::fs::write(&path, text.as_bytes()) {
                    Ok(()) => browser.show_toast(&format!(
                        "{count} Adressen gespeichert in {}",
                        path.display()
                    )),
                    Err(error) => browser.show_toast(&format!("Speichern fehlgeschlagen: {error}")),
                }
            },
        );
    }

    fn export_groups(&self) -> Vec<ExportGroup> {
        let Some(window) = self.window() else {
            return Vec::new();
        };
        let state = window.global::<OverlayState>();
        let include_archived = state.get_export_archived();
        let islands: Vec<IslandId> = match state.get_export_scope() {
            SCOPE_ISLAND => self.transfer.borrow().export_island.into_iter().collect(),
            SCOPE_WORKSPACE => {
                self.workspace_islands(self.session.borrow().workspace, include_archived)
            }
            SCOPE_ALL => self
                .storage
                .workspaces()
                .unwrap_or_default()
                .into_iter()
                .flat_map(|workspace| self.workspace_islands(workspace.id, include_archived))
                .collect(),
            SCOPE_SELECTION => {
                let model = state.get_export_islands();
                (0..model.row_count())
                    .filter_map(|row| model.row_data(row))
                    .filter(|item| item.selected && (include_archived || !item.archived))
                    .map(|item| IslandId(i64::from(item.id)))
                    .collect()
            }
            _ => Vec::new(),
        };
        islands
            .into_iter()
            .filter_map(|island| {
                let record = self.storage.island(island).ok()?;
                let urls = self
                    .storage
                    .tabs_in_island(island)
                    .ok()?
                    .into_iter()
                    .map(|tab| tab.url)
                    .filter(|url| !url.is_empty())
                    .collect();
                Some(ExportGroup {
                    name: record.name,
                    urls,
                })
            })
            .collect()
    }

    fn workspace_islands(&self, workspace: WorkspaceId, include_archived: bool) -> Vec<IslandId> {
        let mut islands: Vec<IslandId> = self
            .storage
            .islands(workspace)
            .unwrap_or_default()
            .into_iter()
            .map(|island| island.id)
            .collect();
        if include_archived {
            islands.extend(
                self.storage
                    .archived_islands()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|archived| archived.island.workspace_id == workspace)
                    .map(|archived| archived.island.id),
            );
        }
        islands
    }
}

fn plural<'a>(count: usize, one: &'a str, many: &'a str) -> &'a str {
    if count == 1 { one } else { many }
}

fn sanitize_file_stem(name: &str) -> String {
    let stem: String = name
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '-'
            }
        })
        .collect();
    let stem = stem.trim_matches('-');
    if stem.is_empty() {
        "insel".to_owned()
    } else {
        stem.to_owned()
    }
}
