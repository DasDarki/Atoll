use std::os::raw::c_int;

use cef::sys::cef_permission_request_types_t;
use cef::{
    Browser, CefString, ImplPermissionHandler, ImplPermissionPromptCallback, PermissionHandler,
    PermissionPromptCallback, PermissionRequestResult, WrapPermissionHandler, rc::Rc as _,
    wrap_permission_handler,
};

use super::text;

/// The origin of the bundled DevTools frontend, see [`crate::DevToolsSession`].
const DEV_TOOLS_ORIGIN: &str = "devtools://devtools";

const NETWORK_PERMISSIONS: u32 =
    cef_permission_request_types_t::CEF_PERMISSION_TYPE_LOOPBACK_NETWORK as u32
        | cef_permission_request_types_t::CEF_PERMISSION_TYPE_LOCAL_NETWORK as u32;

wrap_permission_handler! {
    pub(crate) struct ViewPermissionHandler {}

    impl PermissionHandler {
        fn on_show_permission_prompt(
            &self,
            _browser: Option<&mut Browser>,
            _prompt_id: u64,
            requesting_origin: Option<&CefString>,
            requested_permissions: u32,
            callback: Option<&mut PermissionPromptCallback>,
        ) -> c_int {
            let Some(callback) = callback else { return 0 };
            if !is_dev_tools_connection(&text(requesting_origin), requested_permissions) {
                return 0;
            }
            callback.cont(PermissionRequestResult::ACCEPT);
            1
        }
    }
}

/// The DevTools frontend may reach the loopback interface, where its protocol connection is
/// served. Every other prompt keeps the default answer, which is to deny.
fn is_dev_tools_connection(origin: &str, permissions: u32) -> bool {
    origin.trim_end_matches('/') == DEV_TOOLS_ORIGIN
        && permissions != 0
        && permissions & !NETWORK_PERMISSIONS == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_frontend_may_reach_the_local_network() {
        let loopback = cef_permission_request_types_t::CEF_PERMISSION_TYPE_LOOPBACK_NETWORK as u32;
        let camera = cef_permission_request_types_t::CEF_PERMISSION_TYPE_CAMERA_STREAM as u32;
        assert!(is_dev_tools_connection("devtools://devtools/", loopback));
        assert!(!is_dev_tools_connection("https://example.org/", loopback));
        assert!(!is_dev_tools_connection(
            "devtools://devtools",
            loopback | camera
        ));
        assert!(!is_dev_tools_connection("devtools://devtools", 0));
    }
}
