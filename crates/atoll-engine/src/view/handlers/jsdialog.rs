use std::os::raw::c_int;

use cef::sys::cef_jsdialog_type_t;
use cef::{
    Browser, CefString, ImplJsdialogHandler, JsdialogCallback, JsdialogHandler, JsdialogType,
    WrapJsdialogHandler, rc::Rc as _, wrap_jsdialog_handler,
};

use super::text;
use crate::events::{EngineEvent, JsDialogKind, JsDialogRequest};
use crate::registry::{self, PendingCallback};
use crate::view::ViewId;

wrap_jsdialog_handler! {
    pub(crate) struct ViewJsDialogHandler {
        view: ViewId,
    }

    impl JsdialogHandler {
        fn on_jsdialog(
            &self,
            _browser: Option<&mut Browser>,
            origin_url: Option<&CefString>,
            dialog_type: JsdialogType,
            message_text: Option<&CefString>,
            default_prompt_text: Option<&CefString>,
            callback: Option<&mut JsdialogCallback>,
            _suppress_message: Option<&mut c_int>,
        ) -> c_int {
            let kind = match *dialog_type.as_ref() {
                cef_jsdialog_type_t::JSDIALOGTYPE_CONFIRM => JsDialogKind::Confirm,
                cef_jsdialog_type_t::JSDIALOGTYPE_PROMPT => JsDialogKind::Prompt,
                _ => JsDialogKind::Alert,
            };
            self.request(kind, text(origin_url), text(message_text), text(default_prompt_text), callback)
        }

        fn on_before_unload_dialog(
            &self,
            _browser: Option<&mut Browser>,
            message_text: Option<&CefString>,
            is_reload: c_int,
            callback: Option<&mut JsdialogCallback>,
        ) -> c_int {
            let kind = JsDialogKind::BeforeUnload {
                is_reload: is_reload != 0,
            };
            self.request(kind, String::new(), text(message_text), String::new(), callback)
        }

        fn on_reset_dialog_state(&self, _browser: Option<&mut Browser>) {
            if let Some(runtime) = registry::runtime() {
                runtime.drop_js_dialogs(self.view);
                runtime.emit(EngineEvent::JsDialogDismissed { view: self.view });
            }
        }
    }
}

impl ViewJsDialogHandler {
    fn request(
        &self,
        kind: JsDialogKind,
        origin: String,
        message: String,
        default_text: String,
        callback: Option<&mut JsdialogCallback>,
    ) -> c_int {
        let Some(callback) = callback else { return 0 };
        let Some(runtime) = registry::runtime() else {
            return 0;
        };
        let id = runtime.add_pending(PendingCallback::JsDialog {
            view: self.view,
            callback: callback.clone(),
        });
        runtime.emit(EngineEvent::JsDialog {
            view: self.view,
            request: JsDialogRequest {
                id,
                kind,
                origin,
                message,
                default_text,
            },
        });
        1
    }
}
