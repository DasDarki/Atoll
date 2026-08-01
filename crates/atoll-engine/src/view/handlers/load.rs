use std::os::raw::c_int;

use std::sync::atomic::Ordering;

use cef::{
    Browser, CefString, Frame, ImplFrame, ImplLoadHandler, LoadHandler, TransitionType,
    WrapLoadHandler, rc::Rc as _, wrap_load_handler,
};

use crate::activity;
use crate::events::EngineEvent;
use crate::filter;
use crate::registry;
use crate::view::ViewId;
use crate::view::state::NavigationState;

wrap_load_handler! {
    pub(crate) struct ViewLoadHandler {
        view: ViewId,
    }

    impl LoadHandler {
        fn on_loading_state_change(
            &self,
            _browser: Option<&mut Browser>,
            is_loading: c_int,
            can_go_back: c_int,
            can_go_forward: c_int,
        ) {
            let state = NavigationState {
                loading: is_loading != 0,
                can_go_back: can_go_back != 0,
                can_go_forward: can_go_forward != 0,
            };
            registry::with_view(self.view, |runtime, view| {
                view.navigation.set(state);
                runtime.emit(EngineEvent::LoadingStateChanged {
                    view: view.id,
                    loading: state.loading,
                    can_go_back: state.can_go_back,
                    can_go_forward: state.can_go_forward,
                });
            });
        }

        fn on_load_start(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, _transition_type: TransitionType) {
            let Some(frame) = frame else { return };
            let main = frame.is_main() != 0;
            registry::with_view(self.view, |runtime, view| {
                if main {
                    view.blocked_requests.store(0, Ordering::Relaxed);
                    if view.activity.borrow_mut().reset() {
                        runtime.emit(EngineEvent::PageBusy { view: view.id, busy: false });
                    }
                }
                if view.dev_tools_frontend.get() || view.clipboard_helper.get() {
                    return;
                }
                let url = CefString::from(&frame.url()).to_string();
                if !["http://", "https://", "file://"].iter().any(|scheme| url.starts_with(scheme)) {
                    return;
                }
                let mut script = activity::page_observer_script();
                if let Some(content_filter) = runtime.content_filter.as_ref() {
                    let mut rules = content_filter.document_rules(&url);
                    rules.observe_elements &= main;
                    if let Some(filtering) = filter::document_script(&rules) {
                        script.push_str(&filtering);
                    }
                }
                frame.execute_java_script(Some(&script.as_str().into()), Some(&url.as_str().into()), 0);
            });
        }

        fn on_load_end(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, http_status_code: c_int) {
            if frame.is_some_and(|frame| frame.is_main() != 0) {
                registry::emit(EngineEvent::MainFrameLoaded {
                    view: self.view,
                    http_status: http_status_code,
                });
            }
        }
    }
}
