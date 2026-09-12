use std::collections::HashSet;
use std::rc::Rc;

use atoll_core::address::display_url;
use atoll_core::{Bookmark, BookmarkId, IslandId, TabId, Timestamp};
use atoll_storage::prelude::*;
use atoll_storage::{NewBookmark, StorageResult, StorageTransaction};
use atoll_transfer::{BookmarkNode, parse_bookmarks_html};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use super::Browser;
use crate::platform::dialogs::{self, FileDialogKind, FileDialogOptions, FileFilter};
use crate::ui::{BookmarkItem, BrowserState, OverlayState};
use atoll_i18n::t;

/// What the bookmark editor currently edits.
struct Editing {
    id: BookmarkId,
    is_folder: bool,
    folder_choices: Vec<Option<BookmarkId>>,
}

#[derive(Default)]
pub struct BookmarkState {
    all: Vec<Bookmark>,
    editing: Option<Editing>,
    popup_path: Vec<BookmarkId>,
}

impl Browser {
    /// Reloads bookmarks in every window after they changed.
    pub(super) fn refresh_bookmarks(&self) {
        self.for_all_windows(Browser::show_bookmarks);
    }

    /// Reloads bookmarks and updates the bar, the star and an open bookmark manager.
    pub(super) fn show_bookmarks(&self) {
        let all = self.storage.bookmarks().unwrap_or_default();
        self.bookmarks.borrow_mut().all = all;
        let Some(window) = self.window() else { return };
        let bar: Vec<BookmarkItem> = self
            .bookmark_children(None)
            .iter()
            .map(|bookmark| self.bookmark_item(bookmark, 0))
            .collect();
        window
            .global::<BrowserState>()
            .set_bookmarks_bar(ModelRc::from(Rc::new(VecModel::from(bar))));
        self.update_bookmarks_bar_visibility();
        self.update_bookmark_star();
        if window
            .global::<OverlayState>()
            .get_bookmark_manager_visible()
        {
            self.refresh_bookmark_manager();
        }
        if window.global::<OverlayState>().get_folder_popup_visible() {
            self.show_folder_popup_contents();
        }
    }

    pub(super) fn update_bookmark_star(&self) {
        let Some(window) = self.window() else { return };
        let bookmarked = self
            .current_page_url()
            .is_some_and(|url| self.bookmark_for_url(&url).is_some());
        window
            .global::<BrowserState>()
            .set_page_bookmarked(bookmarked);
    }

    /// Star in the address bar and Ctrl+D: bookmarks the page, or edits the existing bookmark.
    pub fn bookmark_page(&self) {
        let Some(url) = self.current_page_url() else {
            return;
        };
        if let Some(existing) = self.bookmark_for_url(&url) {
            self.edit_bookmark(existing.id);
            return;
        }
        let title = self
            .session
            .borrow()
            .active_tab
            .and_then(|tab| {
                self.session
                    .borrow()
                    .tab(tab)
                    .map(|tab| tab.display_title())
            })
            .unwrap_or_else(|| display_url(&url));
        match self.storage.create_bookmark(NewBookmark {
            parent_id: None,
            title: &title,
            url: Some(&url),
            index: None,
            created_at: Self::now(),
        }) {
            Ok(bookmark) => {
                self.refresh_bookmarks();
                self.edit_bookmark(bookmark.id);
            }
            Err(error) => self.report(t!("Lesezeichen speichern"), Err::<(), _>(error)),
        }
    }

    pub fn bookmark_tab(&self, tab: TabId) {
        let Some(record) = self.session.borrow().tab(tab).cloned() else {
            return;
        };
        if record.is_blank() {
            return;
        }
        let result = self.storage.create_bookmark(NewBookmark {
            parent_id: None,
            title: &record.display_title(),
            url: Some(&record.url),
            index: None,
            created_at: Self::now(),
        });
        match result {
            Ok(_) => {
                self.refresh_bookmarks();
                self.show_toast(t!("Als Lesezeichen gespeichert"));
            }
            Err(error) => self.report(t!("Lesezeichen speichern"), Err::<(), _>(error)),
        }
    }

    /// Saves every tab of an island into a new bookmark folder named after it.
    pub fn bookmark_island(&self, island: IslandId) {
        let Some(entry) = self.session.borrow().island(island).cloned() else {
            return;
        };
        let now = Self::now();
        let result = self.storage.transaction(|transaction| {
            let folder = transaction.create_bookmark(NewBookmark {
                parent_id: None,
                title: &entry.island.name,
                url: None,
                index: None,
                created_at: now,
            })?;
            for tab in entry.tabs.iter().filter(|tab| !tab.is_blank()) {
                transaction.create_bookmark(NewBookmark {
                    parent_id: Some(folder.id),
                    title: &tab.display_title(),
                    url: Some(&tab.url),
                    index: None,
                    created_at: now,
                })?;
            }
            Ok(())
        });
        match result {
            Ok(()) => {
                self.refresh_bookmarks();
                self.show_toast(&t!(
                    "Ordner „{}“ in der Lesezeichenleiste angelegt",
                    entry.island.name
                ));
            }
            Err(error) => self.report(t!("Lesezeichen speichern"), Err::<(), _>(error)),
        }
    }

    pub fn open_bookmark(&self, id: BookmarkId, new_tab: bool) {
        let Some(url) = self.bookmark(id).and_then(|bookmark| bookmark.url) else {
            return;
        };
        self.close_folder_popup();
        if new_tab {
            let active = self.session.borrow().active_tab;
            let _ = self.open_in_new_tab(active, &url, false);
        } else {
            self.navigate(&url);
        }
    }

    pub fn open_bookmark_folder(&self, id: BookmarkId, x: f32, y: f32) {
        let Some(window) = self.window() else { return };
        self.bookmarks.borrow_mut().popup_path = vec![id];
        let overlay = window.global::<OverlayState>();
        overlay.set_folder_popup_x(x);
        overlay.set_folder_popup_y(y);
        self.show_folder_popup_contents();
        overlay.set_folder_popup_visible(true);
    }

    pub fn folder_popup_activate(&self, id: BookmarkId, new_tab: bool) {
        let Some(bookmark) = self.bookmark(id) else {
            return;
        };
        if bookmark.is_folder() {
            self.bookmarks.borrow_mut().popup_path.push(id);
            self.show_folder_popup_contents();
        } else {
            self.open_bookmark(id, new_tab);
        }
    }

    pub fn folder_popup_up(&self) {
        let empty = {
            let mut state = self.bookmarks.borrow_mut();
            state.popup_path.pop();
            state.popup_path.is_empty()
        };
        if empty {
            self.close_folder_popup();
        } else {
            self.show_folder_popup_contents();
        }
    }

    pub fn close_folder_popup(&self) {
        self.bookmarks.borrow_mut().popup_path.clear();
        if let Some(window) = self.window() {
            window
                .global::<OverlayState>()
                .set_folder_popup_visible(false);
        }
    }

    fn show_folder_popup_contents(&self) {
        let Some(window) = self.window() else { return };
        let (folder, nested) = {
            let state = self.bookmarks.borrow();
            (state.popup_path.last().copied(), state.popup_path.len() > 1)
        };
        let Some(folder) = folder else { return };
        let title = self
            .bookmark(folder)
            .map(|bookmark| bookmark.title)
            .unwrap_or_default();
        let items: Vec<BookmarkItem> = self
            .bookmark_children(Some(folder))
            .iter()
            .map(|bookmark| self.bookmark_item(bookmark, 0))
            .collect();
        let overlay = window.global::<OverlayState>();
        overlay.set_folder_popup_title(title.into());
        overlay.set_folder_popup_nested(nested);
        overlay.set_folder_popup_items(ModelRc::from(Rc::new(VecModel::from(items))));
    }

    /// Context menu entries of the bookmarks bar and the manager.
    pub fn bookmark_action(&self, id: BookmarkId, action: &str) {
        match action {
            "open" => self.open_bookmark(id, false),
            "open-all" => {
                let urls: Vec<String> = self
                    .bookmark_children(Some(id))
                    .into_iter()
                    .filter_map(|bookmark| bookmark.url)
                    .collect();
                let active = self.session.borrow().active_tab;
                for url in urls {
                    let _ = self.open_in_new_tab(active, &url, false);
                }
            }
            "edit" => self.edit_bookmark(id),
            "new-folder" => self.create_bookmark_folder(Some(id), |_, _| {}),
            "delete" => self.delete_bookmark(id),
            "manage" => self.open_bookmark_manager(),
            "up" | "down" => self.shift_bookmark(id, if action == "up" { -1 } else { 1 }),
            other => tracing::warn!(action = other, "unknown bookmark action"),
        }
    }

    fn edit_bookmark(&self, id: BookmarkId) {
        let Some(window) = self.window() else { return };
        let Some(bookmark) = self.bookmark(id) else {
            return;
        };
        let (names, choices) = self.folder_choices(&bookmark);
        let selected = choices
            .iter()
            .position(|choice| *choice == bookmark.parent_id)
            .unwrap_or(0);

        let overlay = window.global::<OverlayState>();
        overlay.set_bookmark_title(bookmark.title.as_str().into());
        overlay.set_bookmark_url(bookmark.url.clone().unwrap_or_default().into());
        overlay.set_bookmark_editor_folder(bookmark.is_folder());
        overlay.set_bookmark_folders(ModelRc::from(Rc::new(VecModel::from(names))));
        overlay.set_bookmark_folder_index(selected as i32);
        overlay.set_bookmark_editor_visible(true);
        self.bookmarks.borrow_mut().editing = Some(Editing {
            id,
            is_folder: bookmark.is_folder(),
            folder_choices: choices,
        });
    }

    /// Every folder `bookmark` can be moved to, as display names and their ids.
    fn folder_choices(&self, bookmark: &Bookmark) -> (Vec<SharedString>, Vec<Option<BookmarkId>>) {
        let excluded = if bookmark.is_folder() {
            self.descendants(bookmark.id)
        } else {
            HashSet::new()
        };
        let mut names = vec![SharedString::from(t!("Lesezeichenleiste"))];
        let mut choices = vec![None];
        self.collect_folders(None, 1, &excluded, &mut names, &mut choices);
        (names, choices)
    }

    /// Bookmark editor: creates a folder inside the selected one and moves the bookmark there.
    pub fn bookmark_editor_new_folder(&self) {
        let Some(window) = self.window() else { return };
        let parent = {
            let state = self.bookmarks.borrow();
            let Some(editing) = state.editing.as_ref() else {
                return;
            };
            usize::try_from(window.global::<OverlayState>().get_bookmark_folder_index())
                .ok()
                .and_then(|index| editing.folder_choices.get(index).copied())
                .flatten()
        };
        self.create_bookmark_folder(parent, |browser, folder| {
            browser.select_editor_folder(folder)
        });
    }

    fn select_editor_folder(&self, folder: BookmarkId) {
        let Some(window) = self.window() else { return };
        let edited = self
            .bookmarks
            .borrow()
            .editing
            .as_ref()
            .and_then(|editing| self.bookmark(editing.id));
        let Some(edited) = edited else { return };
        let (names, choices) = self.folder_choices(&edited);
        let selected = choices
            .iter()
            .position(|choice| *choice == Some(folder))
            .unwrap_or(0);
        let overlay = window.global::<OverlayState>();
        overlay.set_bookmark_folders(ModelRc::from(Rc::new(VecModel::from(names))));
        overlay.set_bookmark_folder_index(selected as i32);
        if let Some(editing) = self.bookmarks.borrow_mut().editing.as_mut() {
            editing.folder_choices = choices;
        }
    }

    /// Bookmarks bar: moves `dragged` next to or into `target`.
    pub fn bookmark_dropped(&self, dragged: BookmarkId, target: BookmarkId, placement: i32) {
        let placement = match placement {
            0 => DropPlacement::Before,
            1 => DropPlacement::After,
            2 => DropPlacement::Into,
            _ => return,
        };
        let Some(target) = self.bookmark(target) else {
            return;
        };
        let siblings = self.bookmark_children(target.parent_id);
        let Some((parent, index)) = drop_destination(dragged, &target, &siblings, placement) else {
            return;
        };
        let result = self.storage.move_bookmark(dragged, parent, index);
        self.report(t!("Lesezeichen verschieben"), result);
        self.refresh_bookmarks();
    }

    pub fn save_bookmark_editor(&self) {
        let Some(window) = self.window() else { return };
        let Some(editing) = self.bookmarks.borrow_mut().editing.take() else {
            return;
        };
        let overlay = window.global::<OverlayState>();
        overlay.set_bookmark_editor_visible(false);
        let title = overlay.get_bookmark_title().trim().to_owned();
        let url = overlay.get_bookmark_url().trim().to_owned();
        let parent = usize::try_from(overlay.get_bookmark_folder_index())
            .ok()
            .and_then(|index| editing.folder_choices.get(index).copied())
            .flatten();
        let url = if editing.is_folder {
            None
        } else {
            match atoll_transfer::validate_address(&url) {
                Ok(url) => Some(url),
                Err(reason) => {
                    self.show_toast(&t!("Adresse nicht gespeichert: {}", reason));
                    None
                }
            }
        };
        let current = self.bookmark(editing.id);
        let url = url.or_else(|| current.as_ref().and_then(|bookmark| bookmark.url.clone()));
        let title = if title.is_empty() {
            current
                .as_ref()
                .map(|bookmark| bookmark.title.clone())
                .unwrap_or_default()
        } else {
            title
        };
        let moved = current
            .as_ref()
            .is_some_and(|bookmark| bookmark.parent_id != parent);
        let result = self.storage.transaction(|transaction| {
            transaction.update_bookmark(editing.id, &title, url.as_deref())?;
            if moved {
                transaction.move_bookmark(editing.id, parent, usize::MAX)?;
            }
            Ok(())
        });
        self.report(t!("Lesezeichen speichern"), result);
        self.refresh_bookmarks();
        self.focus_content();
    }

    pub fn remove_bookmark_in_editor(&self) {
        let Some(editing) = self.bookmarks.borrow_mut().editing.take() else {
            return;
        };
        if let Some(window) = self.window() {
            window
                .global::<OverlayState>()
                .set_bookmark_editor_visible(false);
        }
        let result = self.storage.delete_bookmark(editing.id);
        self.report(t!("Lesezeichen entfernen"), result);
        self.refresh_bookmarks();
        self.focus_content();
    }

    pub fn cancel_bookmark_editor(&self) {
        self.bookmarks.borrow_mut().editing = None;
        if let Some(window) = self.window() {
            window
                .global::<OverlayState>()
                .set_bookmark_editor_visible(false);
        }
        self.focus_content();
    }

    fn delete_bookmark(&self, id: BookmarkId) {
        let Some(bookmark) = self.bookmark(id) else {
            return;
        };
        if !bookmark.is_folder() {
            let result = self.storage.delete_bookmark(id);
            self.report(t!("Lesezeichen löschen"), result);
            self.refresh_bookmarks();
            return;
        }
        let count = self.descendants(id).len() - 1;
        let this = self.this();
        self.confirm(
            t!("Ordner löschen?"),
            &t!(
                "„{}“ wird mit {} enthaltenen Einträgen gelöscht.",
                bookmark.title,
                count
            ),
            t!("Löschen"),
            move || {
                if let Some(browser) = this.upgrade() {
                    let result = browser.storage.delete_bookmark(id);
                    browser.report(t!("Ordner löschen"), result);
                    browser.refresh_bookmarks();
                }
            },
        );
    }

    fn shift_bookmark(&self, id: BookmarkId, offset: isize) {
        let Some(bookmark) = self.bookmark(id) else {
            return;
        };
        let siblings = self.bookmark_children(bookmark.parent_id);
        let Some(index) = siblings.iter().position(|sibling| sibling.id == id) else {
            return;
        };
        let target = (index as isize + offset).clamp(0, siblings.len() as isize - 1) as usize;
        let result = self.storage.move_bookmark(id, bookmark.parent_id, target);
        self.report(t!("Lesezeichen verschieben"), result);
        self.refresh_bookmarks();
    }

    pub fn open_bookmark_manager(&self) {
        let Some(window) = self.window() else { return };
        self.close_folder_popup();
        window
            .global::<OverlayState>()
            .set_bookmark_manager_visible(true);
        self.refresh_bookmark_manager();
    }

    pub fn close_bookmark_manager(&self) {
        if let Some(window) = self.window() {
            window
                .global::<OverlayState>()
                .set_bookmark_manager_visible(false);
        }
        self.focus_content();
    }

    fn refresh_bookmark_manager(&self) {
        let Some(window) = self.window() else { return };
        let mut rows = Vec::new();
        self.collect_rows(None, 0, &mut rows);
        window
            .global::<OverlayState>()
            .set_bookmark_rows(ModelRc::from(Rc::new(VecModel::from(rows))));
    }

    pub fn new_bookmark_folder(&self) {
        self.create_bookmark_folder(None, |_, _| {});
    }

    /// Asks for a name, creates a folder in `parent` and passes its id to `created`.
    fn create_bookmark_folder(
        &self,
        parent: Option<BookmarkId>,
        created: impl FnOnce(&Browser, BookmarkId) + 'static,
    ) {
        let message = match parent.and_then(|parent| self.bookmark(parent)) {
            Some(folder) => t!("Der Ordner wird in „{}“ angelegt.", folder.title),
            None => t!("Der Ordner erscheint in der Lesezeichenleiste.").to_owned(),
        };
        let this = self.this();
        self.prompt_text(
            t!("Neuer Ordner"),
            &message,
            "",
            t!("Anlegen"),
            move |name| {
                let Some(browser) = this.upgrade() else {
                    return;
                };
                let result = browser.storage.create_bookmark(NewBookmark {
                    parent_id: parent,
                    title: &name,
                    url: None,
                    index: None,
                    created_at: Self::now(),
                });
                match result {
                    Ok(folder) => {
                        browser.refresh_bookmarks();
                        created(&browser, folder.id);
                    }
                    Err(error) => browser.report(t!("Ordner anlegen"), Err::<(), _>(error)),
                }
            },
        );
    }

    pub fn import_bookmarks(&self) {
        let this = self.this();
        dialogs::show(
            FileDialogOptions {
                kind: FileDialogKind::Open,
                title: t!("Lesezeichen importieren").into(),
                directory: None,
                file_name: None,
                filters: vec![FileFilter {
                    name: t!("Lesezeichen (HTML)").into(),
                    extensions: vec!["html".into(), "htm".into()],
                }],
            },
            move |paths| {
                let Some(browser) = this.upgrade() else {
                    return;
                };
                let Some(path) = paths.and_then(|paths| paths.into_iter().next()) else {
                    return;
                };
                match std::fs::read(&path) {
                    Ok(bytes) => browser.import_bookmark_html(&String::from_utf8_lossy(&bytes)),
                    Err(error) => {
                        browser.show_toast(&t!("Datei konnte nicht gelesen werden: {}", error))
                    }
                }
            },
        );
    }

    fn import_bookmark_html(&self, html: &str) {
        let nodes = parse_bookmarks_html(html);
        if nodes.is_empty() {
            self.show_toast(t!("Die Datei enthält keine Lesezeichen"));
            return;
        }
        let imported = self.import_bookmark_nodes(&nodes);
        if imported > 0 {
            self.show_toast(&t!("{} Lesezeichen importiert", imported));
        }
    }

    /// Puts a bookmark tree of another browser in, skipping what is already there. Answers with
    /// how many were added.
    pub(super) fn import_bookmark_nodes(&self, nodes: &[BookmarkNode]) -> usize {
        let mut known: HashSet<String> = self
            .bookmarks
            .borrow()
            .all
            .iter()
            .filter_map(|bookmark| bookmark.url.clone())
            .collect();
        let now = Self::now();
        let result = self.storage.transaction(|transaction| {
            let mut imported = 0;
            import_nodes(transaction, None, nodes, &mut known, now, &mut imported)?;
            Ok(imported)
        });
        match result {
            Ok(imported) => {
                self.refresh_bookmarks();
                imported
            }
            Err(error) => {
                self.report(t!("Lesezeichen importieren"), Err::<(), _>(error));
                0
            }
        }
    }

    fn bookmark(&self, id: BookmarkId) -> Option<Bookmark> {
        self.bookmarks
            .borrow()
            .all
            .iter()
            .find(|bookmark| bookmark.id == id)
            .cloned()
    }

    fn bookmark_for_url(&self, url: &str) -> Option<Bookmark> {
        self.bookmarks
            .borrow()
            .all
            .iter()
            .find(|bookmark| bookmark.url.as_deref() == Some(url))
            .cloned()
    }

    fn bookmark_children(&self, parent: Option<BookmarkId>) -> Vec<Bookmark> {
        let mut children: Vec<Bookmark> = self
            .bookmarks
            .borrow()
            .all
            .iter()
            .filter(|bookmark| bookmark.parent_id == parent)
            .cloned()
            .collect();
        children.sort_by_key(|bookmark| bookmark.position);
        children
    }

    fn descendants(&self, id: BookmarkId) -> HashSet<BookmarkId> {
        let mut found = HashSet::from([id]);
        let mut pending = vec![id];
        while let Some(current) = pending.pop() {
            for child in self.bookmark_children(Some(current)) {
                if found.insert(child.id) {
                    pending.push(child.id);
                }
            }
        }
        found
    }

    fn collect_folders(
        &self,
        parent: Option<BookmarkId>,
        depth: usize,
        excluded: &HashSet<BookmarkId>,
        names: &mut Vec<SharedString>,
        choices: &mut Vec<Option<BookmarkId>>,
    ) {
        for folder in self
            .bookmark_children(parent)
            .into_iter()
            .filter(Bookmark::is_folder)
        {
            if excluded.contains(&folder.id) {
                continue;
            }
            names.push(format!("{}{}", "    ".repeat(depth), folder.title).into());
            choices.push(Some(folder.id));
            self.collect_folders(Some(folder.id), depth + 1, excluded, names, choices);
        }
    }

    fn collect_rows(&self, parent: Option<BookmarkId>, depth: i32, rows: &mut Vec<BookmarkItem>) {
        for bookmark in self.bookmark_children(parent) {
            rows.push(self.bookmark_item(&bookmark, depth));
            if bookmark.is_folder() {
                self.collect_rows(Some(bookmark.id), depth + 1, rows);
            }
        }
    }

    fn bookmark_item(&self, bookmark: &Bookmark, depth: i32) -> BookmarkItem {
        let icon = bookmark
            .url
            .as_deref()
            .and_then(|url| self.favicons.borrow_mut().image_for_url(&self.storage, url));
        BookmarkItem {
            id: bookmark.id.0 as i32,
            title: bookmark.title.as_str().into(),
            url: bookmark
                .url
                .as_deref()
                .map(display_url)
                .unwrap_or_default()
                .into(),
            is_folder: bookmark.is_folder(),
            has_icon: icon.is_some(),
            icon: icon.unwrap_or_default(),
            depth,
        }
    }

    pub(super) fn has_bar_bookmarks(&self) -> bool {
        self.bookmarks
            .borrow()
            .all
            .iter()
            .any(|bookmark| bookmark.parent_id.is_none())
    }

    pub(super) fn current_page_url(&self) -> Option<String> {
        let session = self.session.borrow();
        let tab = session.active_tab.and_then(|tab| session.tab(tab))?;
        (!tab.is_blank()).then(|| tab.url.clone())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DropPlacement {
    Before,
    After,
    Into,
}

/// The parent and index a bookmark dropped onto `target` moves to. `siblings` are the children of
/// the target's parent in order.
fn drop_destination(
    dragged: BookmarkId,
    target: &Bookmark,
    siblings: &[Bookmark],
    placement: DropPlacement,
) -> Option<(Option<BookmarkId>, usize)> {
    if dragged == target.id {
        return None;
    }
    match placement {
        DropPlacement::Into => target.is_folder().then_some((Some(target.id), usize::MAX)),
        DropPlacement::Before | DropPlacement::After => {
            let position = siblings
                .iter()
                .filter(|sibling| sibling.id != dragged)
                .position(|sibling| sibling.id == target.id)?;
            let index = if placement == DropPlacement::After {
                position + 1
            } else {
                position
            };
            Some((target.parent_id, index))
        }
    }
}

fn import_nodes(
    transaction: &StorageTransaction<'_>,
    parent: Option<BookmarkId>,
    nodes: &[BookmarkNode],
    known: &mut HashSet<String>,
    now: Timestamp,
    imported: &mut usize,
) -> StorageResult<()> {
    for node in nodes {
        match node {
            BookmarkNode::Link { title, url } => {
                if !known.insert(url.clone()) {
                    continue;
                }
                transaction.create_bookmark(NewBookmark {
                    parent_id: parent,
                    title,
                    url: Some(url),
                    index: None,
                    created_at: now,
                })?;
                *imported += 1;
            }
            BookmarkNode::Folder { title, children } => {
                let existing = transaction
                    .bookmark_children(parent)?
                    .into_iter()
                    .find(|bookmark| bookmark.is_folder() && bookmark.title == *title);
                let folder = match existing {
                    Some(folder) => folder.id,
                    None => {
                        transaction
                            .create_bookmark(NewBookmark {
                                parent_id: parent,
                                title,
                                url: None,
                                index: None,
                                created_at: now,
                            })?
                            .id
                    }
                };
                import_nodes(transaction, Some(folder), children, known, now, imported)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use atoll_storage::Storage;

    use super::*;

    fn link(title: &str, url: &str) -> BookmarkNode {
        BookmarkNode::Link {
            title: title.into(),
            url: url.into(),
        }
    }

    fn folder(title: &str, children: Vec<BookmarkNode>) -> BookmarkNode {
        BookmarkNode::Folder {
            title: title.into(),
            children,
        }
    }

    fn import(storage: &Storage, nodes: &[BookmarkNode]) -> usize {
        let mut known: HashSet<String> = storage
            .bookmarks()
            .unwrap()
            .into_iter()
            .filter_map(|bookmark| bookmark.url)
            .collect();
        storage
            .transaction(|transaction| {
                let mut imported = 0;
                import_nodes(
                    transaction,
                    None,
                    nodes,
                    &mut known,
                    Timestamp(0),
                    &mut imported,
                )?;
                Ok(imported)
            })
            .unwrap()
    }

    fn titles(storage: &Storage, parent: Option<BookmarkId>) -> Vec<String> {
        storage
            .bookmark_children(parent)
            .unwrap()
            .into_iter()
            .map(|bookmark| bookmark.title)
            .collect()
    }

    fn entry(id: i64, url: Option<&str>) -> Bookmark {
        Bookmark {
            id: BookmarkId(id),
            parent_id: None,
            title: format!("{id}"),
            url: url.map(str::to_owned),
            position: id,
        }
    }

    #[test]
    fn drops_land_before_after_or_inside_the_target() {
        let siblings = [
            entry(1, Some("https://a.example/")),
            entry(2, None),
            entry(3, Some("https://c.example/")),
        ];
        let dragged = BookmarkId(1);
        assert_eq!(
            drop_destination(dragged, &siblings[2], &siblings, DropPlacement::After),
            Some((None, 2))
        );
        assert_eq!(
            drop_destination(dragged, &siblings[2], &siblings, DropPlacement::Before),
            Some((None, 1))
        );
        assert_eq!(
            drop_destination(dragged, &siblings[1], &siblings, DropPlacement::Into),
            Some((Some(BookmarkId(2)), usize::MAX))
        );
        assert_eq!(
            drop_destination(
                BookmarkId(3),
                &siblings[0],
                &siblings,
                DropPlacement::Before
            ),
            Some((None, 0))
        );
    }

    #[test]
    fn drops_onto_links_or_the_dragged_entry_are_ignored() {
        let siblings = [entry(1, Some("https://a.example/")), entry(2, None)];
        assert_eq!(
            drop_destination(BookmarkId(2), &siblings[0], &siblings, DropPlacement::Into),
            None
        );
        assert_eq!(
            drop_destination(
                BookmarkId(2),
                &siblings[1],
                &siblings,
                DropPlacement::Before
            ),
            None
        );
    }

    #[test]
    fn import_keeps_the_folder_structure() {
        let storage = Storage::open_in_memory().unwrap();
        let nodes = [
            folder(
                "Rust",
                vec![
                    link("Rust", "https://www.rust-lang.org/"),
                    folder("Crates", vec![link("docs.rs", "https://docs.rs/")]),
                ],
            ),
            link("Example", "https://example.org/"),
        ];

        assert_eq!(import(&storage, &nodes), 3);
        assert_eq!(titles(&storage, None), ["Rust", "Example"]);
        let rust = storage.bookmark_children(None).unwrap()[0].id;
        assert_eq!(titles(&storage, Some(rust)), ["Rust", "Crates"]);
    }

    #[test]
    fn import_merges_folders_and_skips_known_addresses() {
        let storage = Storage::open_in_memory().unwrap();
        import(
            &storage,
            &[folder(
                "Rust",
                vec![link("Rust", "https://www.rust-lang.org/")],
            )],
        );

        let imported = import(
            &storage,
            &[
                folder(
                    "Rust",
                    vec![
                        link("Rust again", "https://www.rust-lang.org/"),
                        link("docs.rs", "https://docs.rs/"),
                        link("docs.rs twice", "https://docs.rs/"),
                    ],
                ),
                folder("Rust", vec![link("Blog", "https://blog.rust-lang.org/")]),
            ],
        );

        assert_eq!(imported, 2);
        assert_eq!(titles(&storage, None), ["Rust"]);
        let rust = storage.bookmark_children(None).unwrap()[0].id;
        assert_eq!(titles(&storage, Some(rust)), ["Rust", "docs.rs", "Blog"]);
    }
}
