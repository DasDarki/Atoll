use std::os::raw::c_int;

use cef::{
    Browser, BrowserSettings, CefString, Client, DictionaryValue, Frame, ImplFrame,
    ImplLifeSpanHandler, LifeSpanHandler, PopupFeatures, WindowInfo, WindowOpenDisposition,
    WrapLifeSpanHandler, rc::Rc as _, wrap_life_span_handler,
};

use super::text;
use crate::events::{EngineEvent, OpenDisposition};
use crate::filter::Opening;
use crate::registry;
use crate::view::bridge;
use crate::view::{ViewId, build_client};

wrap_life_span_handler! {
    pub(crate) struct ViewLifeSpanHandler {
        view: ViewId,
    }

    impl LifeSpanHandler {
        fn on_before_popup(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            popup_id: c_int,
            target_url: Option<&CefString>,
            _target_frame_name: Option<&CefString>,
            target_disposition: WindowOpenDisposition,
            user_gesture: c_int,
            _popup_features: Option<&PopupFeatures>,
            window_info: Option<&mut WindowInfo>,
            client: Option<&mut Option<Client>>,
            settings: Option<&mut BrowserSettings>,
            _extra_info: Option<&mut Option<DictionaryValue>>,
            _no_javascript_access: Option<&mut c_int>,
        ) -> c_int {
            let url = text(target_url);
            let popup = target_disposition == WindowOpenDisposition::NEW_POPUP;
            let opening = if popup { Opening::Popup } else { Opening::NewTab };
            let page = frame.map(|frame| CefString::from(&frame.url()).to_string());
            if !super::allowed(self.view, opening, page.as_deref(), &url, user_gesture) {
                return 1;
            }
            if !popup {
                registry::emit(EngineEvent::OpenUrl {
                    view: self.view,
                    url,
                    disposition: open_disposition(target_disposition),
                });
                return 1;
            }

            let (Some(window_info), Some(client), Some(settings)) = (window_info, client, settings) else {
                return 1;
            };
            let adopted = registry::with_view(self.view, |runtime, opener| {
                let popup = runtime.create_view(opener.geometry.get(), false);
                window_info.windowless_rendering_enabled = 1;
                window_info.shared_texture_enabled = i32::from(runtime.accelerated.get());
                window_info.external_begin_frame_enabled = 0;
                *client = Some(build_client(popup.id));
                settings.windowless_frame_rate = runtime.frame_rate;
                runtime.remember_popup(popup_id, popup.id);
                runtime.emit(EngineEvent::PopupOpened {
                    view: popup.id,
                    opener: opener.id,
                    url,
                });
            });
            i32::from(adopted.is_none())
        }

        fn on_before_popup_aborted(&self, _browser: Option<&mut Browser>, popup_id: c_int) {
            let Some(runtime) = registry::runtime() else { return };
            if let Some(popup) = runtime.take_popup(popup_id) {
                runtime.remove_view(popup);
                runtime.emit(EngineEvent::Closed(popup));
            }
        }

        fn on_after_created(&self, browser: Option<&mut Browser>) {
            let Some(browser) = browser else { return };
            registry::with_view(self.view, |_, view| {
                let mut slot = view.browser.borrow_mut();
                if slot.is_none() {
                    *slot = Some(browser.clone());
                }
                drop(slot);
                bridge::browser_ready(view);
            });
        }

        fn do_close(&self, _browser: Option<&mut Browser>) -> c_int {
            registry::with_view(self.view, |runtime, view| {
                if !view.closing.replace(true) {
                    runtime.emit(EngineEvent::CloseRequested(view.id));
                }
            });
            0
        }

        fn on_before_close(&self, _browser: Option<&mut Browser>) {
            let Some(runtime) = registry::runtime() else { return };
            if let Some(view) = runtime.remove_view(self.view) {
                view.browser.borrow_mut().take();
                view.closing.set(true);
            }
            runtime.emit(EngineEvent::Closed(self.view));
        }
    }
}

fn open_disposition(disposition: WindowOpenDisposition) -> OpenDisposition {
    if disposition == WindowOpenDisposition::NEW_BACKGROUND_TAB {
        OpenDisposition::BackgroundTab
    } else if disposition == WindowOpenDisposition::NEW_WINDOW {
        OpenDisposition::NewWindow
    } else {
        OpenDisposition::ForegroundTab
    }
}

pub(super) fn is_new_tab_disposition(disposition: WindowOpenDisposition) -> bool {
    [
        WindowOpenDisposition::NEW_FOREGROUND_TAB,
        WindowOpenDisposition::NEW_BACKGROUND_TAB,
        WindowOpenDisposition::NEW_WINDOW,
        WindowOpenDisposition::NEW_POPUP,
    ]
    .contains(&disposition)
}

pub(super) fn disposition_of(disposition: WindowOpenDisposition) -> OpenDisposition {
    open_disposition(disposition)
}
