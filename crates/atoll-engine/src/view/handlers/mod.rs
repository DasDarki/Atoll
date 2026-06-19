mod context_menu;
mod dialog;
mod display;
mod download;
mod find;
mod jsdialog;
mod life_span;
mod load;
mod render;
mod request;

pub(crate) use context_menu::ViewContextMenuHandler;
pub(crate) use dialog::ViewDialogHandler;
pub(crate) use display::ViewDisplayHandler;
pub(crate) use download::ViewDownloadHandler;
pub(crate) use find::ViewFindHandler;
pub(crate) use jsdialog::ViewJsDialogHandler;
pub(crate) use life_span::ViewLifeSpanHandler;
pub(crate) use load::ViewLoadHandler;
pub(crate) use render::ViewRenderHandler;
pub(crate) use request::ViewRequestHandler;

use cef::{CefString, CefStringList};

fn text(value: Option<&CefString>) -> String {
    value.map(ToString::to_string).unwrap_or_default()
}

/// Reads a string list handed to a callback.
///
/// Cloning a borrowed `CefStringList` yields an empty list, so the entries are read through the
/// C API directly.
fn list(value: Option<&mut CefStringList>) -> Vec<String> {
    let Some(list) = value else { return Vec::new() };
    let raw: *mut cef::sys::_cef_string_list_t = list.into();
    if raw.is_null() {
        return Vec::new();
    }
    // SAFETY: the list is valid for the duration of the callback. Every value copied out of it is
    // owned by us and released with `cef_string_utf16_clear`.
    unsafe {
        let count = cef::sys::cef_string_list_size(raw);
        (0..count)
            .filter_map(|index| {
                let mut value: cef::sys::cef_string_t = std::mem::zeroed();
                if cef::sys::cef_string_list_value(raw, index, &mut value) == 0 {
                    return None;
                }
                let text = CefString::from(std::ptr::from_ref(&value)).to_string();
                cef::sys::cef_string_utf16_clear(&mut value);
                Some(text)
            })
            .collect()
    }
}
