use std::os::raw::c_int;

use cef::{
    Browser, CefString, Frame, ImplBrowser, ImplFrame, ImplRequest, ImplRequestHandler, Request,
    RequestHandler, ResourceRequestHandler, TerminationStatus, WindowOpenDisposition,
    WrapRequestHandler, rc::Rc as _, wrap_request_handler,
};

use super::life_span::{disposition_of, is_new_tab_disposition};
use super::text;
use crate::events::EngineEvent;
use crate::filter::Opening;
use crate::registry;
use crate::view::ViewId;

wrap_request_handler! {
    pub(crate) struct ViewRequestHandler {
        view: ViewId,
        resources: Option<ResourceRequestHandler>,
    }

    impl RequestHandler {
        fn resource_request_handler(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _request: Option<&mut Request>,
            _is_navigation: c_int,
            _is_download: c_int,
            _request_initiator: Option<&CefString>,
            _disable_default_handling: Option<&mut c_int>,
        ) -> Option<ResourceRequestHandler> {
            self.resources.clone()
        }

        fn on_before_browse(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            request: Option<&mut Request>,
            user_gesture: c_int,
            _is_redirect: c_int,
        ) -> c_int {
            let Some(frame) = frame else { return 0 };
            if frame.is_main() == 0 {
                return 0;
            }
            let asked_for = registry::with_view(self.view, |_, view| view.asked_for.replace(false));
            if asked_for != Some(false) {
                return 0;
            }
            let Some(request) = request else { return 0 };
            let target = CefString::from(&request.url()).to_string();
            let page = browser
                .and_then(|browser| browser.main_frame())
                .map(|frame| CefString::from(&frame.url()).to_string());
            let allowed = super::allowed(
                self.view,
                Opening::Navigation,
                page.as_deref(),
                &target,
                user_gesture,
            );
            i32::from(!allowed)
        }

        fn on_open_urlfrom_tab(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            target_url: Option<&CefString>,
            target_disposition: WindowOpenDisposition,
            user_gesture: c_int,
        ) -> c_int {
            if !is_new_tab_disposition(target_disposition) {
                return 0;
            }
            let url = text(target_url);
            let page = frame.map(|frame| CefString::from(&frame.url()).to_string());
            if !super::allowed(self.view, Opening::NewTab, page.as_deref(), &url, user_gesture) {
                return 1;
            }
            registry::emit(EngineEvent::OpenUrl {
                view: self.view,
                url,
                disposition: disposition_of(target_disposition),
            });
            1
        }

        fn on_render_process_terminated(
            &self,
            _browser: Option<&mut Browser>,
            status: TerminationStatus,
            _error_code: c_int,
            _error_string: Option<&CefString>,
        ) {
            tracing::warn!(status = ?status, "render process terminated");
            registry::emit(EngineEvent::RenderProcessGone(self.view));
        }
    }
}
