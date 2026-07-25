use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use cef::{
    Browser, Callback, CefString, Frame, ImplBrowser, ImplFrame, ImplRequest,
    ImplResourceRequestHandler, Request, ResourceRequestHandler, ReturnValue,
    WrapResourceRequestHandler, rc::Rc as _, wrap_resource_request_handler,
};

use crate::filter::{ContentFilter, ResourceKind, ResourceRequest};

wrap_resource_request_handler! {
    pub(crate) struct FilteringResourceHandler {
        filter: Arc<dyn ContentFilter>,
        blocked: Arc<AtomicU32>,
    }

    impl ResourceRequestHandler {
        fn on_before_resource_load(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            request: Option<&mut Request>,
            _callback: Option<&mut Callback>,
        ) -> ReturnValue {
            let Some(request) = request else {
                return ReturnValue::CONTINUE;
            };
            let kind = ResourceKind::of(*request.resource_type().as_ref());
            if kind == ResourceKind::Document {
                return ReturnValue::CONTINUE;
            }
            let url = CefString::from(&request.url()).to_string();
            if !is_web_address(&url) {
                return ReturnValue::CONTINUE;
            }
            let source_url = frame
                .map(|frame| CefString::from(&frame.url()).to_string())
                .unwrap_or_default();
            let page_url = browser
                .and_then(|browser| browser.main_frame())
                .map(|frame| CefString::from(&frame.url()).to_string())
                .filter(|url| !url.is_empty())
                .unwrap_or_else(|| source_url.clone());
            let blocked = self.filter.blocks(&ResourceRequest {
                url: &url,
                source_url: &source_url,
                page_url: &page_url,
                kind,
            });
            if blocked {
                self.blocked.fetch_add(1, Ordering::Relaxed);
                ReturnValue::CANCEL
            } else {
                ReturnValue::CONTINUE
            }
        }
    }
}

fn is_web_address(url: &str) -> bool {
    ["http://", "https://", "ws://", "wss://"]
        .iter()
        .any(|scheme| url.starts_with(scheme))
}
