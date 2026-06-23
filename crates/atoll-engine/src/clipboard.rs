use cef::{ImplBrowserHost, KeyEvent};

use crate::events::EngineEvent;
use crate::input::{KeyInput, KeyInputKind, Modifiers};
use crate::registry::Runtime;
use crate::view::{BrowserView, ViewState};

const MESSAGE_PREFIX: &str = "\u{1}atoll-clipboard:";

const HELPER_PAGE: &str = r#"<!doctype html><textarea id="area"></textarea><script>
const area = document.getElementById("area");
let pending = "";
window.atollArm = (mode, text) => {
  pending = text || "";
  area.value = mode === "write" ? "x" : "";
  area.focus();
  area.select();
  console.log("\u0001atoll-clipboard:armed-" + mode);
};
document.addEventListener("copy", (event) => {
  event.clipboardData.setData("text/plain", pending);
  event.preventDefault();
  console.log("\u0001atoll-clipboard:written");
});
document.addEventListener("paste", (event) => {
  const text = event.clipboardData.getData("text/plain");
  event.preventDefault();
  console.log("\u0001atoll-clipboard:text:" + text);
});
</script>"#;

const VK_CONTROL: i32 = 0x11;
const VK_C: i32 = 0x43;
const VK_V: i32 = 0x56;
const XKB_CONTROL_L: i32 = 37;
const XKB_C: i32 = 54;
const XKB_V: i32 = 55;

/// Bridges Chromium's clipboard and the system clipboard.
///
/// Off-screen browsers never own keyboard focus on Wayland, so Chromium's clipboard stays
/// separate from the system clipboard. A hidden helper page moves text between both: it answers
/// a synthetic Ctrl+C with a `copy` event carrying the text to write, and reads Chromium's
/// clipboard through the `paste` event of a synthetic Ctrl+V. Results arrive as
/// [`EngineEvent::ClipboardWritten`] and [`EngineEvent::ClipboardText`].
pub struct ClipboardHelper {
    view: BrowserView,
}

impl ClipboardHelper {
    pub(crate) fn new(view: BrowserView) -> Self {
        Self { view }
    }

    pub(crate) fn page_url() -> String {
        format!(
            "data:text/html;charset=utf-8,{}",
            percent_encode(HELPER_PAGE)
        )
    }

    /// Reads Chromium's clipboard, answered with [`EngineEvent::ClipboardText`].
    pub fn request_read(&self) {
        self.view.set_focus(true);
        self.view
            .execute_script("window.atollArm && window.atollArm('read');");
    }

    /// Puts `text` into Chromium's clipboard, answered with [`EngineEvent::ClipboardWritten`].
    pub fn request_write(&self, text: &str) {
        self.view.set_focus(true);
        self.view.execute_script(&format!(
            "window.atollArm && window.atollArm('write', {});",
            js_string(text)
        ));
    }
}

/// Handles a console message of the helper page. Returns `false` if it was not meant for Atoll.
pub(crate) fn handle_message(runtime: &Runtime, view: &ViewState, message: &str) -> bool {
    let Some(message) = message.strip_prefix(MESSAGE_PREFIX) else {
        return false;
    };
    match message {
        "armed-read" => send_shortcut(view, VK_V, XKB_V),
        "armed-write" => send_shortcut(view, VK_C, XKB_C),
        "written" => runtime.emit(EngineEvent::ClipboardWritten),
        _ => {
            if let Some(text) = message.strip_prefix("text:") {
                runtime.emit(EngineEvent::ClipboardText(text.to_owned()));
            }
        }
    }
    true
}

fn send_shortcut(view: &ViewState, windows_key_code: i32, native_key_code: i32) {
    let Some(host) = view.host() else { return };
    let control = Modifiers {
        control: true,
        ..Modifiers::default()
    };
    let character = (windows_key_code - 0x40) as u16;
    let unmodified = u16::from((windows_key_code as u8).to_ascii_lowercase());
    let key = |kind, code, native, character, unmodified_character| -> KeyEvent {
        KeyInput {
            kind,
            windows_key_code: code,
            native_key_code: native,
            character,
            unmodified_character,
            modifiers: control,
            is_system_key: false,
        }
        .to_cef()
    };
    let events = [
        key(KeyInputKind::RawKeyDown, VK_CONTROL, XKB_CONTROL_L, 0, 0),
        key(
            KeyInputKind::RawKeyDown,
            windows_key_code,
            native_key_code,
            character,
            unmodified,
        ),
        key(
            KeyInputKind::Char,
            windows_key_code,
            native_key_code,
            character,
            unmodified,
        ),
        key(
            KeyInputKind::KeyUp,
            windows_key_code,
            native_key_code,
            character,
            unmodified,
        ),
        key(KeyInputKind::KeyUp, VK_CONTROL, XKB_CONTROL_L, 0, 0),
    ];
    for event in &events {
        host.send_key_event(Some(event));
    }
}

/// Encodes `text` as a JavaScript string literal.
fn js_string(text: &str) -> String {
    let mut literal = String::with_capacity(text.len() + 2);
    literal.push('"');
    for character in text.chars() {
        match character {
            '"' => literal.push_str("\\\""),
            '\\' => literal.push_str("\\\\"),
            '\n' => literal.push_str("\\n"),
            '\r' => literal.push_str("\\r"),
            '\t' => literal.push_str("\\t"),
            character if character.is_control() || matches!(character, '\u{2028}' | '\u{2029}') => {
                literal.push_str(&format!("\\u{{{:x}}}", character as u32));
            }
            character => literal.push(character),
        }
    }
    literal.push('"');
    literal
}

fn percent_encode(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len() * 3);
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn javascript_literals_escape_quotes_and_line_breaks() {
        assert_eq!(js_string("a\"b\\c\nd"), "\"a\\\"b\\\\c\\nd\"");
        assert_eq!(js_string("\u{2028}"), "\"\\u{2028}\"");
    }

    #[test]
    fn helper_page_is_a_valid_data_url() {
        let url = ClipboardHelper::page_url();
        assert!(url.starts_with("data:text/html;charset=utf-8,%3C%21doctype"));
        assert!(!url.contains(' '));
        assert!(!url.contains('#'));
    }
}
