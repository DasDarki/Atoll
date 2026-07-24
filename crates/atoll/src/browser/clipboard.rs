use std::rc::Rc;
use std::time::Duration;

use atoll_engine::{ClipboardHelper, KeyInput, KeyInputKind, RequestId};
use slint::{Timer, TimerMode};

use super::Browser;

const READ_DELAY: Duration = Duration::from_millis(150);
const WRITE_TIMEOUT: Duration = Duration::from_millis(400);

const MENU_CUT: i32 = 112;
const MENU_COPY: i32 = 113;
const MENU_PASTE: i32 = 114;
const MENU_PASTE_MATCH_STYLE: i32 = 115;

const VK_C: i32 = 0x43;
const VK_V: i32 = 0x56;
const VK_X: i32 = 0x58;
const VK_INSERT: i32 = 0x2D;
const VK_DELETE: i32 = 0x2E;

/// Keeps Chromium's clipboard and the system clipboard in step, see [`ClipboardHelper`].
#[derive(Default)]
pub struct ClipboardSync {
    helper: Option<ClipboardHelper>,
    known: Option<String>,
    pending: Option<PendingPaste>,
    read_timer: Timer,
    timeout: Timer,
}

/// A paste that waits until the system clipboard text arrived in Chromium.
struct PendingPaste {
    text: String,
    keys: Vec<KeyInput>,
    menu: Option<(RequestId, i32)>,
}

/// What a key press means for the clipboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ClipboardKey {
    None,
    Copy,
    Paste,
}

impl ClipboardKey {
    pub(super) fn of(keys: &[KeyInput]) -> Self {
        let Some(key) = keys
            .first()
            .filter(|key| key.kind == KeyInputKind::RawKeyDown)
        else {
            return ClipboardKey::None;
        };
        let modifiers = key.modifiers;
        let control = modifiers.control && !modifiers.alt;
        match key.windows_key_code {
            VK_V if control => ClipboardKey::Paste,
            VK_INSERT if modifiers.shift => ClipboardKey::Paste,
            VK_C | VK_X if control => ClipboardKey::Copy,
            VK_INSERT if control => ClipboardKey::Copy,
            VK_DELETE if modifiers.shift => ClipboardKey::Copy,
            _ => ClipboardKey::None,
        }
    }
}

impl Browser {
    fn clipboard_bridge_enabled() -> bool {
        cfg!(target_os = "linux")
    }

    pub(super) fn start_clipboard_bridge(&self) {
        if !Self::clipboard_bridge_enabled() {
            return;
        }
        let helper = self
            .engine
            .borrow()
            .as_ref()
            .map(|engine| engine.create_clipboard_helper());
        match helper {
            Some(Ok(helper)) => self.clipboard_sync.borrow_mut().helper = Some(helper),
            Some(Err(error)) => tracing::warn!(%error, "clipboard bridge unavailable"),
            None => {}
        }
    }

    pub(super) fn stop_clipboard_bridge(&self) {
        let mut sync = self.clipboard_sync.borrow_mut();
        sync.read_timer.stop();
        sync.timeout.stop();
        sync.pending = None;
        sync.helper = None;
    }

    /// Delivers keys to the page, holding them back while a paste waits for the clipboard.
    pub(super) fn deliver_keys(&self, keys: Vec<KeyInput>) {
        let kind = ClipboardKey::of(&keys);
        {
            let mut sync = self.clipboard_sync.borrow_mut();
            if let Some(pending) = sync.pending.as_mut() {
                pending.keys.extend(keys);
                return;
            }
        }
        if kind == ClipboardKey::Paste && self.start_clipboard_write(None) {
            if let Some(pending) = self.clipboard_sync.borrow_mut().pending.as_mut() {
                pending.keys = keys;
            }
            return;
        }
        self.with_input_view(|view| {
            for key in &keys {
                view.key(*key);
            }
        });
        if kind == ClipboardKey::Copy {
            self.schedule_clipboard_read();
        }
    }

    /// Resolves an edit command of the page's context menu, syncing the clipboard around it.
    /// Returns `false` if the command is resolved later.
    pub(super) fn resolve_menu_command(&self, request: RequestId, command: i32) -> bool {
        match command {
            MENU_PASTE | MENU_PASTE_MATCH_STYLE => {
                !self.start_clipboard_write(Some((request, command)))
            }
            MENU_CUT | MENU_COPY => {
                self.schedule_clipboard_read();
                true
            }
            _ => true,
        }
    }

    /// Checks Chromium's clipboard shortly after something may have been copied into it.
    pub(super) fn schedule_clipboard_read(&self) {
        if !Self::clipboard_bridge_enabled() {
            return;
        }
        let this = self.this();
        self.clipboard_sync.borrow().read_timer.start(
            TimerMode::SingleShot,
            READ_DELAY,
            move || {
                if let Some(browser) = this.upgrade() {
                    browser.read_clipboard();
                }
            },
        );
    }

    fn read_clipboard(&self) {
        let sync = self.clipboard_sync.borrow();
        if sync.pending.is_some() {
            return;
        }
        if let Some(helper) = &sync.helper {
            helper.request_read();
        }
    }

    fn start_clipboard_write(&self, menu: Option<(RequestId, i32)>) -> bool {
        if !Self::clipboard_bridge_enabled() {
            return false;
        }
        let Some(text) = self.clipboard.text() else {
            return false;
        };
        let mut sync = self.clipboard_sync.borrow_mut();
        if sync.known.as_deref() == Some(text.as_str()) {
            return false;
        }
        let Some(helper) = &sync.helper else {
            return false;
        };
        helper.request_write(&text);
        sync.pending = Some(PendingPaste {
            text,
            keys: Vec::new(),
            menu,
        });
        let this = self.this();
        let shared = Rc::downgrade(&self.clipboard_sync);
        sync.timeout
            .start(TimerMode::SingleShot, WRITE_TIMEOUT, move || {
                tracing::warn!("clipboard write timed out");
                match this.upgrade() {
                    Some(browser) => browser.finish_clipboard_write(false),
                    None => {
                        if let Some(sync) = shared.upgrade() {
                            sync.borrow_mut().pending = None;
                        }
                    }
                }
            });
        true
    }

    pub(super) fn clipboard_written(&self) {
        self.finish_clipboard_write(true);
    }

    fn finish_clipboard_write(&self, written: bool) {
        let pending = {
            let mut sync = self.clipboard_sync.borrow_mut();
            sync.timeout.stop();
            let pending = sync.pending.take();
            if written && let Some(pending) = &pending {
                sync.known = Some(pending.text.clone());
            }
            pending
        };
        let Some(pending) = pending else { return };
        self.with_input_view(|view| {
            view.set_focus(true);
            for key in &pending.keys {
                view.key(*key);
            }
        });
        if let Some((request, command)) = pending.menu
            && let Some(engine) = self.engine.borrow().as_ref()
        {
            engine.resolve_context_menu(request, Some(command));
        }
    }

    pub(super) fn clipboard_text_read(&self, text: String) {
        self.with_input_view(|view| view.set_focus(true));
        if text.is_empty() {
            return;
        }
        {
            let mut sync = self.clipboard_sync.borrow_mut();
            if sync.known.as_deref() == Some(text.as_str()) {
                return;
            }
            sync.known = Some(text.clone());
        }
        if !self.clipboard.set_text(&text) {
            tracing::warn!("system clipboard could not be updated");
        }
    }
}

#[cfg(test)]
mod tests {
    use atoll_engine::Modifiers;

    use super::*;

    fn key(code: i32, modifiers: Modifiers) -> Vec<KeyInput> {
        vec![KeyInput {
            kind: KeyInputKind::RawKeyDown,
            windows_key_code: code,
            native_key_code: 0,
            character: 0,
            unmodified_character: 0,
            modifiers,
            is_system_key: false,
        }]
    }

    #[test]
    fn clipboard_shortcuts_are_recognised() {
        let control = Modifiers {
            control: true,
            ..Modifiers::default()
        };
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        assert_eq!(ClipboardKey::of(&key(VK_V, control)), ClipboardKey::Paste);
        assert_eq!(
            ClipboardKey::of(&key(VK_INSERT, shift)),
            ClipboardKey::Paste
        );
        assert_eq!(ClipboardKey::of(&key(VK_C, control)), ClipboardKey::Copy);
        assert_eq!(ClipboardKey::of(&key(VK_X, control)), ClipboardKey::Copy);
        assert_eq!(
            ClipboardKey::of(&key(VK_V, Modifiers::default())),
            ClipboardKey::None
        );
    }
}
