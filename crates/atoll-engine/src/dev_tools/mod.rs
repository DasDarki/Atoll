mod protocol;
mod server;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::io;
use std::net::{Ipv4Addr, TcpListener};
use std::os::raw::c_int;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};

use cef::{
    Browser, BrowserHost, DevToolsMessageObserver, ImplBrowserHost, ImplDevToolsMessageObserver,
    Registration, WrapDevToolsMessageObserver, rc::Rc as _, wrap_dev_tools_message_observer,
};
use serde_json::{Value, json};

use crate::error::EngineError;
use crate::pump;
use crate::registry;
use crate::view::{BrowserView, ViewId};
use protocol::FIRST_OWN_ID;
use server::FrontendEvent;

/// The developer tools of one view: the bundled DevTools frontend running in its own view,
/// connected to the protocol of the inspected page.
///
/// Chromium only shows DevTools in native windows, so the frontend is loaded as an ordinary page
/// instead. It talks to a WebSocket served by Atoll on the loopback interface under a random
/// path. Its messages go to a child session of the inspected page that exists only while the
/// frontend is connected, so detaching it undoes everything the frontend changed, exactly like
/// closing DevTools in Chrome.
pub struct DevToolsSession {
    view: BrowserView,
    bridge: Rc<DevToolsBridge>,
}

impl DevToolsSession {
    pub(crate) fn open(
        inspected: &BrowserView,
        create_view: impl FnOnce(&str) -> Result<BrowserView, EngineError>,
    ) -> Result<Self, EngineError> {
        let host = inspected.host().ok_or(EngineError::BrowserCreation)?;
        let (bridge, url) =
            DevToolsBridge::start(inspected.id(), &host).map_err(EngineError::DevTools)?;
        let view = create_view(&url)?;
        let bridge = Rc::new(bridge);
        if let Some(runtime) = registry::runtime() {
            if let Some(state) = runtime.view(view.id()) {
                state.dev_tools_frontend.set(true);
            }
            runtime.add_dev_tools_bridge(view.id(), bridge.clone());
        }
        Ok(Self { view, bridge })
    }

    /// The view showing the frontend.
    pub fn view(&self) -> &BrowserView {
        &self.view
    }

    /// Selects the element at a position of the inspected page, in view coordinates.
    pub fn inspect(&self, x: i32, y: i32) {
        self.bridge.inspect(x, y);
    }
}

impl Drop for DevToolsSession {
    fn drop(&mut self) {
        if let Some(runtime) = registry::runtime() {
            runtime.remove_dev_tools_bridge(self.view.id());
        }
    }
}

/// Requests Atoll sends on the root session of the inspected browser.
#[derive(Debug, Clone, Copy)]
enum OwnRequest {
    TargetInfo,
    Attach,
    Detach,
    NodeAt,
}

/// State shared by the bridge and the observer, both used on the UI thread only.
struct Channel {
    to_frontend: Sender<String>,
    requests: RefCell<HashMap<i64, OwnRequest>>,
    next_id: Cell<i64>,
    target: RefCell<Option<String>>,
    session: RefCell<Option<String>>,
    nested: RefCell<HashSet<String>>,
    attaching: Cell<bool>,
}

impl Channel {
    fn send_request(&self, host: &BrowserHost, request: OwnRequest, method: &str, params: Value) {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        self.requests.borrow_mut().insert(id, request);
        host.send_dev_tools_message(Some(protocol::request(id, method, params).as_bytes()));
    }

    /// Routes a message of the inspected browser: messages of the frontend's sessions go to the
    /// frontend, answers to Atoll's own requests are handled, the rest is dropped.
    fn receive(&self, message: &str) {
        if let Some((session, stripped)) = protocol::split_session(message) {
            if self.session.borrow().as_deref() == Some(session) {
                if let Some((attached, nested)) = protocol::nested_session(&stripped) {
                    let mut sessions = self.nested.borrow_mut();
                    if attached {
                        sessions.insert(nested);
                    } else {
                        sessions.remove(&nested);
                    }
                }
                let _ = self.to_frontend.send(stripped);
            } else if self.nested.borrow().contains(session) {
                let _ = self.to_frontend.send(message.to_owned());
            }
            return;
        }
        let Some(id) = protocol::response_id(message) else {
            return;
        };
        let Some(request) = self.requests.borrow_mut().remove(&id) else {
            return;
        };
        let result = protocol::result(message);
        match request {
            OwnRequest::TargetInfo => {
                let target = result
                    .as_ref()
                    .and_then(|result| result.pointer("/targetInfo/targetId"))
                    .and_then(Value::as_str);
                if target.is_none() {
                    tracing::warn!("the inspected page has no DevTools target");
                }
                *self.target.borrow_mut() = target.map(str::to_owned);
                pump::schedule(0);
            }
            OwnRequest::Attach => {
                self.attaching.set(false);
                let session = result
                    .as_ref()
                    .and_then(|result| result.get("sessionId"))
                    .and_then(Value::as_str);
                if session.is_none() {
                    tracing::warn!("DevTools could not attach to the inspected page");
                }
                *self.session.borrow_mut() = session.map(str::to_owned);
                pump::schedule(0);
            }
            OwnRequest::Detach => {}
            OwnRequest::NodeAt => {
                let node = result
                    .as_ref()
                    .and_then(|result| result.get("backendNodeId"))
                    .and_then(Value::as_i64);
                if let Some(node) = node {
                    let _ = self.to_frontend.send(protocol::inspect_node_event(node));
                }
            }
        }
    }
}

/// Relays protocol messages between the frontend connection and the inspected browser.
pub(crate) struct DevToolsBridge {
    inspected: ViewId,
    events: Receiver<FrontendEvent>,
    channel: Rc<Channel>,
    stop: Arc<AtomicBool>,
    _registration: Registration,
    connected: Cell<bool>,
    pending: RefCell<VecDeque<String>>,
    frontend_ready: Cell<bool>,
    pending_inspect: Cell<Option<(i32, i32)>>,
}

impl DevToolsBridge {
    fn start(inspected: ViewId, host: &BrowserHost) -> io::Result<(Self, String)> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let path = format!("/devtools/page/{}", random_token()?);
        let url =
            format!("devtools://devtools/bundled/devtools_app.html?ws=127.0.0.1:{port}{path}");

        let (event_sender, events) = mpsc::channel();
        let (to_frontend, for_frontend) = mpsc::channel();
        let channel = Rc::new(Channel {
            to_frontend,
            requests: RefCell::default(),
            next_id: Cell::new(FIRST_OWN_ID),
            target: RefCell::default(),
            session: RefCell::default(),
            nested: RefCell::default(),
            attaching: Cell::new(false),
        });
        let mut observer = FrontendForwarder::new(channel.clone());
        let registration = host
            .add_dev_tools_message_observer(Some(&mut observer))
            .ok_or_else(|| io::Error::other("DevTools observer could not be registered"))?;
        channel.send_request(
            host,
            OwnRequest::TargetInfo,
            "Target.getTargetInfo",
            json!({}),
        );

        let stop = Arc::new(AtomicBool::new(false));
        let server_stop = stop.clone();
        std::thread::Builder::new()
            .name("atoll-devtools".into())
            .spawn(move || {
                server::serve(&listener, &path, &event_sender, &for_frontend, &server_stop)
            })?;

        let bridge = Self {
            inspected,
            events,
            channel,
            stop,
            _registration: registration,
            connected: Cell::new(false),
            pending: RefCell::default(),
            frontend_ready: Cell::new(false),
            pending_inspect: Cell::new(None),
        };
        Ok((bridge, url))
    }

    fn host(&self) -> Option<BrowserHost> {
        registry::runtime()
            .and_then(|runtime| runtime.view(self.inspected))
            .and_then(|view| view.host())
    }

    /// Delivers everything the frontend sent since the last call. Runs on the UI thread.
    pub(crate) fn flush(&self) {
        let Some(host) = self.host() else { return };
        while let Ok(event) = self.events.try_recv() {
            match event {
                FrontendEvent::Connected => {
                    self.detach(&host);
                    self.connected.set(true);
                }
                FrontendEvent::Message(message) => {
                    if message.contains("\"method\":\"Overlay.enable\"") {
                        self.frontend_ready.set(true);
                    }
                    self.pending.borrow_mut().push_back(message);
                }
                FrontendEvent::Disconnected => {
                    self.connected.set(false);
                    self.detach(&host);
                }
            }
        }
        self.attach(&host);
        let session = self.channel.session.borrow().clone();
        let Some(session) = session else { return };
        for message in self.pending.borrow_mut().drain(..) {
            host.send_dev_tools_message(Some(
                protocol::with_session(&message, &session).as_bytes(),
            ));
        }
        if self.frontend_ready.get()
            && let Some((x, y)) = self.pending_inspect.take()
        {
            self.request_node_at(&host, x, y);
        }
    }

    /// Gives a connected frontend its own session once the target of the page is known.
    fn attach(&self, host: &BrowserHost) {
        if !self.connected.get()
            || self.channel.attaching.get()
            || self.channel.session.borrow().is_some()
        {
            return;
        }
        let Some(target) = self.channel.target.borrow().clone() else {
            return;
        };
        self.channel.attaching.set(true);
        self.channel.send_request(
            host,
            OwnRequest::Attach,
            "Target.attachToTarget",
            json!({ "targetId": target, "flatten": true }),
        );
    }

    /// Ends the session of the frontend, which resets everything it enabled or overrode.
    fn detach(&self, host: &BrowserHost) {
        self.pending.borrow_mut().clear();
        self.frontend_ready.set(false);
        self.channel.nested.borrow_mut().clear();
        let session = self.channel.session.borrow_mut().take();
        if let Some(session) = session {
            self.channel.send_request(
                host,
                OwnRequest::Detach,
                "Target.detachFromTarget",
                json!({ "sessionId": session }),
            );
        }
    }

    fn inspect(&self, x: i32, y: i32) {
        self.pending_inspect.set(Some((x, y)));
        self.flush();
    }

    /// Asks for the node at a position in view coordinates. The protocol expects CSS pixels,
    /// which differ from view coordinates by the zoom factor of the page.
    fn request_node_at(&self, host: &BrowserHost, x: i32, y: i32) {
        let zoom = 1.2_f64.powf(host.zoom_level());
        self.channel.send_request(
            host,
            OwnRequest::NodeAt,
            "DOM.getNodeForLocation",
            json!({
                "x": (f64::from(x) / zoom).round() as i64,
                "y": (f64::from(y) / zoom).round() as i64,
                "ignorePointerEventsNone": true,
            }),
        );
    }
}

impl Drop for DevToolsBridge {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(host) = self.host() {
            self.detach(&host);
        }
    }
}

wrap_dev_tools_message_observer! {
    struct FrontendForwarder {
        channel: Rc<Channel>,
    }

    impl DevToolsMessageObserver {
        fn on_dev_tools_message(&self, _browser: Option<&mut Browser>, message: Option<&[u8]>) -> c_int {
            if let Some(text) = message.and_then(|message| std::str::from_utf8(message).ok()) {
                self.channel.receive(text);
            }
            1
        }
    }
}

fn random_token() -> io::Result<String> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(io::Error::other)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel() -> (Channel, Receiver<String>) {
        let (to_frontend, received) = mpsc::channel();
        let channel = Channel {
            to_frontend,
            requests: RefCell::default(),
            next_id: Cell::new(FIRST_OWN_ID),
            target: RefCell::default(),
            session: RefCell::new(Some("OWN".to_owned())),
            nested: RefCell::default(),
            attaching: Cell::new(false),
        };
        (channel, received)
    }

    #[test]
    fn only_the_frontend_sessions_reach_the_frontend() {
        let (channel, received) = channel();
        channel.receive(r#"{"id":1,"result":{},"sessionId":"OWN"}"#);
        channel.receive(r#"{"id":1,"result":{}}"#);
        channel.receive(r#"{"method":"Page.loadEventFired","params":{},"sessionId":"OTHER"}"#);
        channel.receive(
            r#"{"method":"Target.attachedToTarget","params":{"sessionId":"CHILD"},"sessionId":"OWN"}"#,
        );
        channel.receive(r#"{"method":"Runtime.enable","params":{},"sessionId":"CHILD"}"#);
        let messages: Vec<String> = received.try_iter().collect();
        assert_eq!(
            messages,
            [
                r#"{"id":1,"result":{}}"#,
                r#"{"method":"Target.attachedToTarget","params":{"sessionId":"CHILD"}}"#,
                r#"{"method":"Runtime.enable","params":{},"sessionId":"CHILD"}"#,
            ]
        );
    }

    #[test]
    fn answers_to_own_requests_update_the_state() {
        let (channel, received) = channel();
        channel.session.borrow_mut().take();
        channel.requests.borrow_mut().extend([
            (FIRST_OWN_ID, OwnRequest::TargetInfo),
            (FIRST_OWN_ID + 1, OwnRequest::NodeAt),
        ]);
        channel.receive(&format!(
            r#"{{"id":{FIRST_OWN_ID},"result":{{"targetInfo":{{"targetId":"T1"}}}}}}"#
        ));
        channel.receive(&format!(
            r#"{{"id":{},"result":{{"backendNodeId":7}}}}"#,
            FIRST_OWN_ID + 1
        ));
        assert_eq!(channel.target.borrow().as_deref(), Some("T1"));
        let event: Value = serde_json::from_str(&received.try_recv().unwrap()).unwrap();
        assert_eq!(event["method"], "Overlay.inspectNodeRequested");
        assert_eq!(event["params"]["backendNodeId"], 7);
    }

    #[test]
    fn tokens_are_long_and_random() {
        let first = random_token().unwrap();
        assert_eq!(first.len(), 32);
        assert_ne!(first, random_token().unwrap());
    }
}
