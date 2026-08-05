use std::os::raw::{c_int, c_ulong};

use cef::sys::cef_cursor_type_t;
use cef::{
    Browser, CefString, CefStringList, CursorInfo, CursorType, DisplayHandler,
    DownloadImageCallback, Frame, Image, ImplBinaryValue, ImplBrowser, ImplBrowserHost,
    ImplDisplayHandler, ImplDownloadImageCallback, ImplFrame, ImplImage, LogSeverity,
    WrapDisplayHandler, WrapDownloadImageCallback, rc::Rc as _, wrap_display_handler,
    wrap_download_image_callback,
};

use super::load::emit_activity;
use super::{list, text};
use crate::activity;
use crate::autofill;
use crate::clipboard;
use crate::events::{CursorKind, EngineEvent};
use crate::filter;
use crate::passkeys;
use crate::registry::{self, Runtime};
use crate::view::{ViewId, ViewState};

const FAVICON_SIZE: f32 = 16.0;

wrap_display_handler! {
    pub(crate) struct ViewDisplayHandler {
        view: ViewId,
    }

    impl DisplayHandler {
        fn on_address_change(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, url: Option<&CefString>) {
            if frame.is_some_and(|frame| frame.is_main() != 0) {
                registry::emit(EngineEvent::AddressChanged {
                    view: self.view,
                    url: text(url),
                });
            }
        }

        fn on_title_change(&self, _browser: Option<&mut Browser>, title: Option<&CefString>) {
            registry::emit(EngineEvent::TitleChanged {
                view: self.view,
                title: text(title),
            });
        }

        fn on_favicon_urlchange(&self, browser: Option<&mut Browser>, icon_urls: Option<&mut CefStringList>) {
            let Some(icon_url) = list(icon_urls).into_iter().next() else { return };
            let Some(browser) = browser else { return };
            let Some(host) = browser.host() else { return };
            let page_url = browser
                .main_frame()
                .map(|frame| CefString::from(&frame.url()).to_string())
                .unwrap_or_default();
            let scale = registry::with_view(self.view, |_, view| view.geometry.get().scale)
                .unwrap_or(1.0)
                .clamp(1.0, 2.0);
            let mut callback = FaviconCallback::new(self.view, page_url, scale);
            host.download_image(
                Some(&icon_url.as_str().into()),
                1,
                (FAVICON_SIZE * scale) as u32,
                0,
                Some(&mut callback),
            );
        }

        fn on_fullscreen_mode_change(&self, _browser: Option<&mut Browser>, fullscreen: c_int) {
            registry::emit(EngineEvent::FullscreenChanged {
                view: self.view,
                fullscreen: fullscreen != 0,
            });
        }

        fn on_tooltip(&self, _browser: Option<&mut Browser>, text: Option<&mut CefString>) -> c_int {
            registry::emit(EngineEvent::Tooltip {
                view: self.view,
                text: text.map(|text| text.to_string()).unwrap_or_default(),
            });
            1
        }

        fn on_status_message(&self, _browser: Option<&mut Browser>, value: Option<&CefString>) {
            registry::emit(EngineEvent::StatusMessage {
                view: self.view,
                text: text(value),
            });
        }

        fn on_loading_progress_change(&self, _browser: Option<&mut Browser>, progress: f64) {
            registry::emit(EngineEvent::LoadProgress {
                view: self.view,
                progress,
            });
        }

        fn on_console_message(
            &self,
            _browser: Option<&mut Browser>,
            _level: LogSeverity,
            message: Option<&CefString>,
            _source: Option<&CefString>,
            _line: c_int,
        ) -> c_int {
            let message = text(message);
            if let Some((frame, report)) = activity::parse_page_report(&message) {
                registry::with_view(self.view, |runtime, view| {
                    let change = view.activity.borrow_mut().update(frame, report);
                    emit_activity(runtime, view.id, change);
                });
                return 1;
            }
            if let Some(fields) = autofill::parse_fill_report(&message) {
                registry::emit(EngineEvent::LoginFilled {
                    view: self.view,
                    fields,
                });
                return 1;
            }
            if message == passkeys::PASSKEY_REPORT {
                registry::emit(EngineEvent::PasskeyRefused(self.view));
                return 1;
            }
            if let Some((classes, ids)) = filter::parse_element_report(&message) {
                registry::with_view(self.view, |runtime, view| {
                    apply_element_rules(runtime, view, &classes, &ids);
                });
                return 1;
            }
            let handled = registry::with_view(self.view, |runtime, view| {
                view.clipboard_helper.get() && clipboard::handle_message(runtime, view, &message)
            });
            i32::from(handled.unwrap_or(false))
        }

        fn on_cursor_change(
            &self,
            _browser: Option<&mut Browser>,
            _cursor: c_ulong,
            type_: CursorType,
            _custom_cursor_info: Option<&CursorInfo>,
        ) -> c_int {
            registry::emit(EngineEvent::CursorChanged {
                view: self.view,
                cursor: cursor_kind(*type_.as_ref()),
            });
            1
        }
    }
}

/// Hides the reported elements of the top level document that the content filter wants hidden.
fn apply_element_rules(runtime: &Runtime, view: &ViewState, classes: &[String], ids: &[String]) {
    let Some(content_filter) = runtime.content_filter.as_ref() else {
        return;
    };
    let Some(frame) = view.browser().and_then(|browser| browser.main_frame()) else {
        return;
    };
    let url = CefString::from(&frame.url()).to_string();
    let css = content_filter.element_rules(&url, classes, ids);
    if !css.is_empty() {
        let script = filter::stylesheet_script(&css);
        frame.execute_java_script(Some(&script.as_str().into()), Some(&url.as_str().into()), 0);
    }
}

wrap_download_image_callback! {
    struct FaviconCallback {
        view: ViewId,
        page_url: String,
        scale: f32,
    }

    impl DownloadImageCallback {
        fn on_download_image_finished(&self, _image_url: Option<&CefString>, _http_status_code: c_int, image: Option<&mut Image>) {
            let Some(image) = image.filter(|image| image.is_empty() == 0) else { return };
            let (mut width, mut height) = (0, 0);
            let Some(png) = image.as_png(self.scale, 1, Some(&mut width), Some(&mut height)) else {
                return;
            };
            let mut bytes = vec![0; png.size()];
            png.data(Some(&mut bytes), 0);
            registry::emit(EngineEvent::FaviconReady {
                view: self.view,
                page_url: self.page_url.clone(),
                png: bytes,
            });
        }
    }
}

fn cursor_kind(cursor: cef_cursor_type_t) -> CursorKind {
    use cef_cursor_type_t as Cursor;
    match cursor {
        Cursor::CT_HAND => CursorKind::Pointer,
        Cursor::CT_IBEAM | Cursor::CT_VERTICALTEXT => CursorKind::Text,
        Cursor::CT_WAIT => CursorKind::Wait,
        Cursor::CT_PROGRESS => CursorKind::Progress,
        Cursor::CT_HELP => CursorKind::Help,
        Cursor::CT_CROSS | Cursor::CT_CELL => CursorKind::Crosshair,
        Cursor::CT_MOVE | Cursor::CT_MIDDLEPANNING => CursorKind::Move,
        Cursor::CT_NOTALLOWED => CursorKind::NotAllowed,
        Cursor::CT_NODROP | Cursor::CT_DND_NONE => CursorKind::NoDrop,
        Cursor::CT_GRAB => CursorKind::Grab,
        Cursor::CT_GRABBING => CursorKind::Grabbing,
        Cursor::CT_COPY | Cursor::CT_DND_COPY => CursorKind::Copy,
        Cursor::CT_ALIAS | Cursor::CT_DND_LINK => CursorKind::Alias,
        Cursor::CT_EASTRESIZE | Cursor::CT_WESTRESIZE | Cursor::CT_EASTWESTRESIZE => {
            CursorKind::ResizeEastWest
        }
        Cursor::CT_NORTHRESIZE | Cursor::CT_SOUTHRESIZE | Cursor::CT_NORTHSOUTHRESIZE => {
            CursorKind::ResizeNorthSouth
        }
        Cursor::CT_NORTHEASTRESIZE
        | Cursor::CT_SOUTHWESTRESIZE
        | Cursor::CT_NORTHEASTSOUTHWESTRESIZE => CursorKind::ResizeNorthEastSouthWest,
        Cursor::CT_NORTHWESTRESIZE
        | Cursor::CT_SOUTHEASTRESIZE
        | Cursor::CT_NORTHWESTSOUTHEASTRESIZE => CursorKind::ResizeNorthWestSouthEast,
        Cursor::CT_COLUMNRESIZE => CursorKind::ResizeColumn,
        Cursor::CT_ROWRESIZE => CursorKind::ResizeRow,
        Cursor::CT_NONE => CursorKind::None,
        _ => CursorKind::Default,
    }
}
