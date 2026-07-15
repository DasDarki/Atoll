use std::collections::VecDeque;
use std::rc::Rc;

use atoll_core::address::display_host;
use atoll_engine::{
    ContextMenuRequest, FileDialogMode, FileDialogRequest, JsDialogKind, JsDialogRequest,
    MenuEntry, MenuEntryKind, RequestId, ViewId,
};
use slint::{ComponentHandle, ModelRc, VecModel};

use super::Browser;
use crate::platform::dialogs::{self, FileDialogKind, FileDialogOptions, FileFilter};
use crate::ui::{DialogData, DialogKind, MenuItemData, OverlayState};

type DialogResult = Box<dyn FnOnce(&Browser, bool, String)>;

struct Dialog {
    data: DialogData,
    on_close: DialogResult,
    origin: Option<ViewId>,
}

/// Dialogs waiting to be shown. Only one is visible at a time.
#[derive(Default)]
pub struct DialogQueue {
    current: Option<Dialog>,
    waiting: VecDeque<Dialog>,
}

impl Browser {
    /// Asks for a line of text. `on_accept` only runs if the user confirms a non-empty value.
    pub(super) fn prompt_text(
        &self,
        title: &str,
        message: &str,
        initial: &str,
        accept_label: &str,
        on_accept: impl FnOnce(String) + 'static,
    ) {
        self.enqueue_dialog(Dialog {
            data: DialogData {
                kind: DialogKind::Prompt,
                title: title.into(),
                message: message.into(),
                input: initial.into(),
                accept_label: accept_label.into(),
                cancel_label: "Abbrechen".into(),
                destructive: false,
            },
            on_close: Box::new(move |_, accepted, text| {
                let text = text.trim().to_owned();
                if accepted && !text.is_empty() {
                    on_accept(text);
                }
            }),
            origin: None,
        });
    }

    pub(super) fn confirm(
        &self,
        title: &str,
        message: &str,
        accept_label: &str,
        on_accept: impl FnOnce() + 'static,
    ) {
        self.enqueue_dialog(Dialog {
            data: DialogData {
                kind: DialogKind::Confirm,
                title: title.into(),
                message: message.into(),
                input: "".into(),
                accept_label: accept_label.into(),
                cancel_label: "Abbrechen".into(),
                destructive: true,
            },
            on_close: Box::new(move |_, accepted, _| {
                if accepted {
                    on_accept();
                }
            }),
            origin: None,
        });
    }

    pub(super) fn show_js_dialog(&self, view: ViewId, request: JsDialogRequest) {
        let origin = display_host(&request.origin).unwrap_or_else(|| "Diese Seite".to_owned());
        let (kind, title, accept, cancel) = match request.kind {
            JsDialogKind::Alert => (DialogKind::Alert, format!("{origin} meldet"), "OK", ""),
            JsDialogKind::Confirm => (
                DialogKind::Confirm,
                format!("{origin} fragt"),
                "OK",
                "Abbrechen",
            ),
            JsDialogKind::Prompt => (
                DialogKind::Prompt,
                format!("{origin} fragt"),
                "OK",
                "Abbrechen",
            ),
            JsDialogKind::BeforeUnload { is_reload } => (
                DialogKind::Confirm,
                if is_reload {
                    "Seite neu laden?"
                } else {
                    "Seite verlassen?"
                }
                .to_owned(),
                if is_reload { "Neu laden" } else { "Verlassen" },
                "Bleiben",
            ),
        };
        let message = match request.kind {
            JsDialogKind::BeforeUnload { .. } => {
                "Vorgenommene Änderungen werden möglicherweise nicht gespeichert.".to_owned()
            }
            _ => request.message.clone(),
        };
        let id = request.id;
        self.enqueue_dialog(Dialog {
            data: DialogData {
                kind,
                title: title.into(),
                message: message.into(),
                input: request.default_text.into(),
                accept_label: accept.into(),
                cancel_label: cancel.into(),
                destructive: false,
            },
            on_close: Box::new(move |browser, accepted, text| {
                if let Some(engine) = browser.engine.borrow().as_ref() {
                    engine.resolve_js_dialog(id, accepted, Some(&text));
                }
            }),
            origin: Some(view),
        });
    }

    /// Closes dialogs of `view` that the page no longer waits for.
    pub(super) fn dismiss_js_dialogs(&self, view: ViewId) {
        let current_matches = {
            let mut queue = self.dialogs.borrow_mut();
            queue.waiting.retain(|dialog| dialog.origin != Some(view));
            queue
                .current
                .as_ref()
                .is_some_and(|dialog| dialog.origin == Some(view))
        };
        if current_matches {
            self.dialogs.borrow_mut().current = None;
            self.show_next_dialog();
        }
    }

    fn enqueue_dialog(&self, dialog: Dialog) {
        let idle = {
            let mut queue = self.dialogs.borrow_mut();
            queue.waiting.push_back(dialog);
            queue.current.is_none()
        };
        if idle {
            self.show_next_dialog();
        }
    }

    fn show_next_dialog(&self) {
        let Some(window) = self.window() else { return };
        let data = {
            let mut queue = self.dialogs.borrow_mut();
            queue.current = queue.waiting.pop_front();
            queue.current.as_ref().map(|dialog| dialog.data.clone())
        };
        let state = window.global::<OverlayState>();
        match data {
            Some(data) => state.set_dialog(data),
            None => {
                state.set_dialog(DialogData::default());
                self.focus_content();
            }
        }
    }

    /// Called by the user interface when the visible dialog was answered.
    pub fn dialog_closed(&self, accepted: bool, text: String) {
        let Some(dialog) = self.dialogs.borrow_mut().current.take() else {
            return;
        };
        if let Some(window) = self.window() {
            window
                .global::<OverlayState>()
                .set_dialog(DialogData::default());
        }
        (dialog.on_close)(self, accepted, text);
        let idle = self.dialogs.borrow().current.is_none();
        if idle {
            self.show_next_dialog();
        }
    }

    pub(super) fn show_context_menu(&self, view: ViewId, request: ContextMenuRequest) {
        let Some(window) = self.window() else { return };
        let origin = if self.is_live_view(view) {
            Some((window.get_content_x(), window.get_content_y()))
        } else if self.is_live_dev_tools(view) {
            Some((window.get_dev_tools_x(), window.get_dev_tools_y()))
        } else {
            None
        };
        let Some((origin_x, origin_y)) = origin else {
            if let Some(engine) = self.engine.borrow().as_ref() {
                engine.resolve_context_menu(request.id, None);
            }
            return;
        };
        self.cancel_context_menu();
        let mut items = Vec::new();
        flatten_menu(&request.entries, false, &mut items);
        tidy_separators(&mut items);
        if items.is_empty() {
            if let Some(engine) = self.engine.borrow().as_ref() {
                engine.resolve_context_menu(request.id, None);
            }
            return;
        }
        let state = window.global::<OverlayState>();
        state.set_menu_items(ModelRc::from(Rc::new(VecModel::from(items))));
        state.set_menu_x(origin_x + request.x as f32);
        state.set_menu_y(origin_y + request.y as f32);
        state.set_menu_visible(true);
        self.context_menu.set(Some((view, request.id)));
    }

    pub fn context_menu_closed(&self, command: Option<i32>) {
        let Some((_, request)) = self.context_menu.take() else {
            return;
        };
        if let Some(window) = self.window() {
            window.global::<OverlayState>().set_menu_visible(false);
        }
        let resolve_now = command.is_none_or(|command| self.resolve_menu_command(request, command));
        if resolve_now {
            self.resolve_menu(request, command);
        }
        self.focus_content();
    }

    pub(super) fn cancel_context_menu(&self) {
        if let Some((_, request)) = self.context_menu.take() {
            if let Some(window) = self.window() {
                window.global::<OverlayState>().set_menu_visible(false);
            }
            self.resolve_menu(request, None);
        }
    }

    fn resolve_menu(&self, request: RequestId, command: Option<i32>) {
        if let Some(engine) = self.engine.borrow().as_ref() {
            engine.resolve_context_menu(request, command);
        }
    }

    pub(super) fn show_file_dialog(&self, request: FileDialogRequest) {
        let kind = match request.mode {
            FileDialogMode::Open => FileDialogKind::Open,
            FileDialogMode::OpenMultiple => FileDialogKind::OpenMultiple,
            FileDialogMode::OpenFolder => FileDialogKind::OpenFolder,
            FileDialogMode::Save => FileDialogKind::Save,
        };
        let extensions = accepted_extensions(&request);
        let filters = if extensions.is_empty() {
            Vec::new()
        } else {
            vec![FileFilter {
                name: "Erlaubte Dateien".into(),
                extensions,
            }]
        };
        let (directory, file_name) = match (&request.default_path, request.mode) {
            (Some(path), FileDialogMode::Save) => (
                path.parent().map(|parent| parent.to_path_buf()),
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned()),
            ),
            (Some(path), _) if path.is_dir() => (Some(path.clone()), None),
            _ => (None, None),
        };
        let title = if request.title.is_empty() {
            match request.mode {
                FileDialogMode::Save => "Speichern unter".to_owned(),
                FileDialogMode::OpenFolder => "Ordner auswählen".to_owned(),
                _ => "Datei auswählen".to_owned(),
            }
        } else {
            request.title.clone()
        };
        let this = self.this();
        let id = request.id;
        dialogs::show(
            FileDialogOptions {
                kind,
                title,
                directory,
                file_name,
                filters,
            },
            move |paths| {
                if let Some(browser) = this.upgrade()
                    && let Some(engine) = browser.engine.borrow().as_ref()
                {
                    engine.resolve_file_dialog(id, paths);
                }
            },
        );
    }

    pub fn show_toast(&self, message: &str) {
        if let Some(window) = self.window() {
            let state = window.global::<OverlayState>();
            state.set_toast(message.into());
            state.set_toast_visible(false);
            state.set_toast_visible(true);
        }
    }

    pub fn hide_toast(&self) {
        if let Some(window) = self.window() {
            window.global::<OverlayState>().set_toast_visible(false);
        }
    }

    pub(super) fn copy_text(&self, text: &str) {
        if self.clipboard.set_text(text) {
            self.show_toast("In die Zwischenablage kopiert");
        } else {
            self.show_toast("Zwischenablage nicht verfügbar");
        }
    }
}

fn flatten_menu(entries: &[MenuEntry], indent: bool, items: &mut Vec<MenuItemData>) {
    for entry in entries {
        match &entry.kind {
            MenuEntryKind::Separator => items.push(MenuItemData {
                separator: true,
                ..MenuItemData::default()
            }),
            MenuEntryKind::Submenu(children) => {
                items.push(MenuItemData {
                    id: entry.command_id,
                    label: entry.label.as_str().into(),
                    enabled: false,
                    indent,
                    ..MenuItemData::default()
                });
                flatten_menu(children, true, items);
            }
            MenuEntryKind::Check { checked } => items.push(MenuItemData {
                id: entry.command_id,
                label: entry.label.as_str().into(),
                enabled: entry.enabled,
                checkable: true,
                checked: *checked,
                indent,
                ..MenuItemData::default()
            }),
            MenuEntryKind::Command => items.push(MenuItemData {
                id: entry.command_id,
                label: entry.label.as_str().into(),
                enabled: entry.enabled,
                indent,
                ..MenuItemData::default()
            }),
        }
    }
}

fn tidy_separators(items: &mut Vec<MenuItemData>) {
    let mut tidy: Vec<MenuItemData> = Vec::with_capacity(items.len());
    for item in items.drain(..) {
        let redundant = item.separator && tidy.last().is_none_or(|previous| previous.separator);
        if !redundant {
            tidy.push(item);
        }
    }
    while tidy.last().is_some_and(|item| item.separator) {
        tidy.pop();
    }
    *items = tidy;
}

/// File extensions for the dialog filter, derived from the page's `accept` attribute.
fn accepted_extensions(request: &FileDialogRequest) -> Vec<String> {
    let mut extensions = request.accept_extensions.clone();
    for accept in &request.accept_types {
        let accept = accept.trim().to_ascii_lowercase();
        if let Some(extension) = accept.strip_prefix('.') {
            extensions.push(extension.to_owned());
            continue;
        }
        let known: &[&str] = match accept.as_str() {
            "image/*" => &[
                "png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "avif", "ico", "heic",
            ],
            "video/*" => &["mp4", "webm", "mkv", "mov", "avi", "m4v"],
            "audio/*" => &["mp3", "wav", "ogg", "flac", "m4a", "opus", "aac"],
            "text/*" => &["txt", "csv", "md", "html", "htm", "json", "xml"],
            "application/pdf" => &["pdf"],
            "image/png" => &["png"],
            "image/jpeg" => &["jpg", "jpeg"],
            "image/gif" => &["gif"],
            "image/webp" => &["webp"],
            _ => &[],
        };
        extensions.extend(known.iter().map(|extension| (*extension).to_owned()));
    }
    extensions.sort();
    extensions.dedup();
    extensions
}
