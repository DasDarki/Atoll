mod context_menu;
mod dialog;
mod display;
mod download;
mod find;
mod jsdialog;
mod life_span;
mod load;
mod permission;
mod render;
mod request;
mod resource;

pub(crate) use context_menu::ViewContextMenuHandler;
pub(crate) use dialog::ViewDialogHandler;
pub(crate) use display::ViewDisplayHandler;
pub(crate) use download::ViewDownloadHandler;
pub(crate) use find::ViewFindHandler;
pub(crate) use jsdialog::ViewJsDialogHandler;
pub(crate) use life_span::ViewLifeSpanHandler;
pub(crate) use load::ViewLoadHandler;
pub(crate) use permission::ViewPermissionHandler;
pub(crate) use render::ViewRenderHandler;
pub(crate) use request::ViewRequestHandler;
pub(crate) use resource::FilteringResourceHandler;

use cef::{CefString, CefStringList};

use crate::events::EngineEvent;
use crate::filter::Opening;
use crate::registry;
use crate::view::ViewId;

/// Asks the content filter whether the page may open this, and says so when it may not.
///
/// A page without an address of its own, such as a tab that is still empty, is left alone, and so
/// is everything the user asked for themselves.
fn allowed(
    view: ViewId,
    opening: Opening,
    page_url: Option<&str>,
    target_url: &str,
    user_gesture: std::os::raw::c_int,
) -> bool {
    let page_url = page_url.unwrap_or_default();
    if page_url.is_empty() || target_url.is_empty() {
        return true;
    }
    let allowed = registry::with_view(view, |runtime, _| {
        runtime
            .content_filter
            .as_ref()
            .is_none_or(|filter| filter.allows(opening, page_url, target_url, user_gesture != 0))
    });
    if allowed == Some(false) {
        tracing::debug!(
            ?opening,
            page_url,
            target_url,
            "a page was not allowed to open this"
        );
        registry::emit(EngineEvent::OpeningBlocked {
            view,
            opening,
            url: target_url.to_owned(),
        });
        return false;
    }
    true
}

fn text(value: Option<&CefString>) -> String {
    value.map(ToString::to_string).unwrap_or_default()
}

/// Reads a string list handed to a callback.
///
/// Cloning a borrowed `CefStringList` yields an empty list, so the entries are read through the
/// C API directly.
pub(crate) fn list(value: Option<&mut CefStringList>) -> Vec<String> {
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
