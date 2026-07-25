use std::os::raw::c_int;

use cef::{
    Browser, CefString, Frame, ImplRequestHandler, Request, RequestHandler, ResourceRequestHandler,
    TerminationStatus, WindowOpenDisposition, WrapRequestHandler, rc::Rc as _,
    wrap_request_handler,
};

use super::life_span::{disposition_of, is_new_tab_disposition};
use super::text;
use crate::events::EngineEvent;
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

        fn on_open_urlfrom_tab(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            target_url: Option<&CefString>,
            target_disposition: WindowOpenDisposition,
            _user_gesture: c_int,
        ) -> c_int {
            if !is_new_tab_disposition(target_disposition) {
                return 0;
            }
            registry::emit(EngineEvent::OpenUrl {
                view: self.view,
                url: text(target_url),
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
