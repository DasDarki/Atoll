//! Putting a picture from the clipboard into a page.
//!
//! Chromium's own clipboard holds what Atoll puts there, and Atoll only puts text there, so
//! Ctrl+V with a picture in the clipboard would do nothing at all. Instead the picture is handed
//! to the page the way a paste hands it over: a `paste` event carrying the file, dispatched at
//! whatever has the cursor. Pages that take pictures that way - message boxes, editors, upload
//! fields - take this one. A page that does not is told nothing, so Atoll says it instead.

use std::cell::RefCell;
use std::collections::HashSet;

use atoll_engine::{ScriptId, ScriptWorld, ViewId};
use atoll_i18n::t;

use super::Browser;
use crate::platform::clipboard::ClipboardImage;

/// The largest picture that is handed over. Beyond this the script would carry more than a page
/// should be asked to swallow in one go.
const MOST: usize = 12 * 1024 * 1024;

/// The pastes of a picture that are still waiting for an answer from their page.
pub type PendingPastes = RefCell<HashSet<(ViewId, ScriptId)>>;

impl Browser {
    /// Hands the picture in the clipboard to the page. Answers whether it was tried at all.
    pub(super) fn paste_image_into_page(&self) -> bool {
        if self
            .clipboard
            .text()
            .is_some_and(|text| !text.trim().is_empty())
        {
            // Text in the clipboard means text is meant, and that goes its own way.
            return false;
        }
        let Some(image) = self.clipboard.image() else {
            return false;
        };
        let Some(png) = png_of(&image) else {
            return false;
        };
        if png.len() > MOST {
            self.show_toast(t!(
                "Das Bild in der Zwischenablage ist zu groß zum Einfügen"
            ));
            return true;
        }
        let code = paste_script(&base64(&png));
        let mut asked = None;
        self.with_live_view(|view| {
            asked = view
                .evaluate(&code, ScriptWorld::Page)
                .map(|script| (view.id(), script));
        });
        let Some(key) = asked else { return false };
        self.pasting.borrow_mut().insert(key);
        tracing::info!(bytes = png.len(), "handing a picture to the page");
        true
    }

    /// The page answered whether it took the picture. Returns whether this was such an answer.
    pub(super) fn image_pasted(
        &self,
        view: ViewId,
        script: ScriptId,
        result: &Result<serde_json::Value, String>,
    ) -> bool {
        if !self.pasting.borrow_mut().remove(&(view, script)) {
            return false;
        }
        let taken = result
            .as_ref()
            .is_ok_and(|value| value.as_bool() == Some(true));
        if !taken {
            self.show_toast(t!("Hier lässt sich kein Bild einfügen"));
        }
        true
    }
}

/// The picture as PNG bytes, which is what pages are given.
fn png_of(image: &ClipboardImage) -> Option<Vec<u8>> {
    let pixels = image::RgbaImage::from_raw(image.width, image.height, image.rgba.clone())?;
    let mut png = Vec::new();
    pixels
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .ok()?;
    Some(png)
}

/// The script that hands the picture over. It answers whether anything took it: either the page
/// handled the paste itself, or the picture went into a field that holds formatted text.
fn paste_script(data: &str) -> String {
    let arguments = serde_json::json!([data]);
    format!("({PASTE_FUNCTION})(...{arguments});")
}

const PASTE_FUNCTION: &str = r#"(data) => {
  const binary = atob(data);
  const bytes = new Uint8Array(binary.length);
  for (let at = 0; at < binary.length; at += 1) bytes[at] = binary.charCodeAt(at);
  const file = new File([bytes], "bild.png", { type: "image/png" });
  const transfer = new DataTransfer();
  transfer.items.add(file);
  const target = document.activeElement && document.activeElement !== document.body
    ? document.activeElement
    : (document.querySelector("[contenteditable=true]") || document.body);
  const event = new ClipboardEvent("paste", {
    clipboardData: transfer, bubbles: true, cancelable: true, composed: true,
  });
  const went = target.dispatchEvent(event);
  if (!went) return true;
  // Nobody took it, so a field that holds formatted text gets the picture itself.
  const editable = target.closest && target.closest("[contenteditable=true]");
  if (editable) {
    const reader = new FileReader();
    reader.onload = () => document.execCommand("insertImage", false, reader.result);
    reader.readAsDataURL(file);
    return true;
  }
  return false;
}"#;

/// The bytes written the way a page reads them back with `atob`.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for group in bytes.chunks(3) {
        let [first, second, third] = [
            group[0] as u32,
            group.get(1).copied().unwrap_or_default() as u32,
            group.get(2).copied().unwrap_or_default() as u32,
        ];
        let number = (first << 16) | (second << 8) | third;
        let letters = [
            ALPHABET[(number >> 18) as usize & 63],
            ALPHABET[(number >> 12) as usize & 63],
            ALPHABET[(number >> 6) as usize & 63],
            ALPHABET[number as usize & 63],
        ];
        for (at, letter) in letters.into_iter().enumerate() {
            out.push(if at > group.len() {
                '='
            } else {
                letter as char
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_are_written_the_way_a_page_reads_them() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(&[0, 255, 128]), "AP+A");
    }

    #[test]
    fn the_picture_is_passed_as_an_argument() {
        let script = paste_script("Zm9v");
        assert!(script.starts_with("((data) =>"));
        assert!(script.ends_with(r#")(...["Zm9v"]);"#));
    }
}
