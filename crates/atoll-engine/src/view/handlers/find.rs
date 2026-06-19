use std::os::raw::c_int;

use cef::{
    Browser, FindHandler, ImplFindHandler, Rect, WrapFindHandler, rc::Rc as _, wrap_find_handler,
};

use crate::events::EngineEvent;
use crate::registry;
use crate::view::ViewId;

wrap_find_handler! {
    pub(crate) struct ViewFindHandler {
        view: ViewId,
    }

    impl FindHandler {
        fn on_find_result(
            &self,
            _browser: Option<&mut Browser>,
            _identifier: c_int,
            count: c_int,
            _selection_rect: Option<&Rect>,
            active_match_ordinal: c_int,
            final_update: c_int,
        ) {
            registry::emit(EngineEvent::FindResult {
                view: self.view,
                matches: count,
                active_match: active_match_ordinal,
                final_update: final_update != 0,
            });
        }
    }
}
