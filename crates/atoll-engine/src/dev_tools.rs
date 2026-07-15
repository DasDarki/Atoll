use std::cell::Cell;
use std::io::{self, ErrorKind};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::os::raw::c_int;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::Duration;

use cef::{
    Browser, BrowserHost, DevToolsMessageObserver, ImplBrowserHost, ImplDevToolsMessageObserver,
    Registration, WrapDevToolsMessageObserver, rc::Rc as _, wrap_dev_tools_message_observer,
};
use serde_json::{Value, json};
use tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tungstenite::http::StatusCode;
use tungstenite::{Message, WebSocket};

use crate::error::EngineError;
use crate::pump;
use crate::registry;
use crate::view::{BrowserView, ViewId};

/// How often an idle connection checks for messages from the page.
const POLL_INTERVAL: Duration = Duration::from_millis(8);
/// How often the server checks for a new connection or for being stopped.
const ACCEPT_INTERVAL: Duration = Duration::from_millis(50);
/// The origin of the bundled frontend. Connections from anywhere else are refused.
const FRONTEND_ORIGIN: &str = "devtools://devtools";
/// Protocol message ids from this value on belong to Atoll instead of the frontend.
const FIRST_OWN_ID: i64 = 1 << 30;

/// The developer tools of one view: the bundled DevTools frontend running in its own view,
/// connected to the protocol of the inspected page.
///
/// Chromium only shows DevTools in native windows, so the frontend is loaded as an ordinary page
/// instead. It talks to a WebSocket served by Atoll on the loopback interface under a random
/// path, and every message is relayed to the inspected browser and back.
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

/// Relays protocol messages between the frontend connection and the inspected browser.
pub(crate) struct DevToolsBridge {
    inspected: ViewId,
    from_frontend: Receiver<String>,
    stop: Arc<AtomicBool>,
    _registration: Registration,
    frontend_ready: Cell<bool>,
    pending_inspect: Cell<Option<(i32, i32)>>,
    next_id: Cell<i64>,
}

impl DevToolsBridge {
    fn start(inspected: ViewId, host: &BrowserHost) -> io::Result<(Self, String)> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let path = format!("/devtools/page/{}", random_token()?);
        let url =
            format!("devtools://devtools/bundled/devtools_app.html?ws=127.0.0.1:{port}{path}");

        let (to_page, from_frontend) = mpsc::channel();
        let (to_frontend, for_frontend) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let mut observer = FrontendForwarder::new(to_frontend);
        let registration = host
            .add_dev_tools_message_observer(Some(&mut observer))
            .ok_or_else(|| io::Error::other("DevTools observer could not be registered"))?;
        let server_stop = stop.clone();
        std::thread::Builder::new()
            .name("atoll-devtools".into())
            .spawn(move || serve(listener, &path, &to_page, &for_frontend, &server_stop))?;

        let bridge = Self {
            inspected,
            from_frontend,
            stop,
            _registration: registration,
            frontend_ready: Cell::new(false),
            pending_inspect: Cell::new(None),
            next_id: Cell::new(FIRST_OWN_ID),
        };
        Ok((bridge, url))
    }

    /// Delivers everything the frontend sent since the last call. Runs on the UI thread.
    pub(crate) fn flush(&self) {
        let host = registry::runtime()
            .and_then(|runtime| runtime.view(self.inspected))
            .and_then(|view| view.host());
        let Some(host) = host else { return };
        while let Ok(message) = self.from_frontend.try_recv() {
            host.send_dev_tools_message(Some(message.as_bytes()));
            if !self.frontend_ready.get() && message.contains("\"method\":\"Overlay.enable\"") {
                self.frontend_ready.set(true);
            }
        }
        if self.frontend_ready.get()
            && let Some((x, y)) = self.pending_inspect.take()
        {
            self.request_node_at(&host, x, y);
        }
    }

    fn inspect(&self, x: i32, y: i32) {
        self.pending_inspect.set(Some((x, y)));
        self.flush();
    }

    /// Asks for the node at a position in view coordinates. The protocol expects CSS pixels,
    /// which differ from view coordinates by the zoom factor of the page.
    fn request_node_at(&self, host: &BrowserHost, x: i32, y: i32) {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        let zoom = 1.2_f64.powf(host.zoom_level());
        let message = json!({
            "id": id,
            "method": "DOM.getNodeForLocation",
            "params": {
                "x": (f64::from(x) / zoom).round() as i64,
                "y": (f64::from(y) / zoom).round() as i64,
                "ignorePointerEventsNone": true,
            },
        })
        .to_string();
        host.send_dev_tools_message(Some(message.as_bytes()));
    }
}

impl Drop for DevToolsBridge {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

wrap_dev_tools_message_observer! {
    struct FrontendForwarder {
        to_frontend: Sender<String>,
    }

    impl DevToolsMessageObserver {
        fn on_dev_tools_message(&self, _browser: Option<&mut Browser>, message: Option<&[u8]>) -> c_int {
            let Some(text) = message.and_then(|message| std::str::from_utf8(message).ok()) else {
                return 0;
            };
            let forwarded = match own_response(text) {
                Some(event) => event,
                None => Some(text.to_owned()),
            };
            if let Some(forwarded) = forwarded {
                let _ = self.to_frontend.send(forwarded);
            }
            1
        }
    }
}

/// Handles the answer to a request Atoll sent itself. Returns `None` for messages that belong to
/// the frontend, otherwise the event to show the frontend instead, if any.
fn own_response(message: &str) -> Option<Option<String>> {
    let id: i64 = message
        .strip_prefix("{\"id\":")?
        .split(|character: char| !character.is_ascii_digit())
        .next()?
        .parse()
        .ok()?;
    if id < FIRST_OWN_ID {
        return None;
    }
    let value: Value = serde_json::from_str(message).ok()?;
    let node = value
        .pointer("/result/backendNodeId")
        .and_then(Value::as_i64);
    Some(node.map(|node| {
        json!({
            "method": "Overlay.inspectNodeRequested",
            "params": { "backendNodeId": node },
        })
        .to_string()
    }))
}

fn random_token() -> io::Result<String> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(io::Error::other)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn serve(
    listener: TcpListener,
    path: &str,
    to_page: &Sender<String>,
    for_frontend: &Receiver<String>,
    stop: &AtomicBool,
) {
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                if let Err(error) = relay(stream, path, to_page, for_frontend, stop) {
                    tracing::debug!(%error, "DevTools connection ended");
                }
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                std::thread::sleep(ACCEPT_INTERVAL);
            }
            Err(error) => {
                tracing::warn!(%error, "DevTools server stopped");
                return;
            }
        }
    }
}

fn relay(
    stream: TcpStream,
    path: &str,
    to_page: &Sender<String>,
    for_frontend: &Receiver<String>,
    stop: &AtomicBool,
) -> Result<(), tungstenite::Error> {
    stream.set_nonblocking(false)?;
    let mut socket = accept(stream, path)?;
    socket.get_ref().set_read_timeout(Some(POLL_INTERVAL))?;
    loop {
        if stop.load(Ordering::Relaxed) {
            let _ = socket.close(None);
            return Ok(());
        }
        match socket.read() {
            Ok(Message::Text(text)) => {
                if to_page.send(text.to_string()).is_err() {
                    return Ok(());
                }
                pump::schedule(0);
            }
            Ok(Message::Close(_)) => return Ok(()),
            Ok(_) => {}
            Err(tungstenite::Error::Io(error))
                if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => {
                return Ok(());
            }
            Err(error) => return Err(error),
        }
        loop {
            match for_frontend.try_recv() {
                Ok(message) => socket.send(Message::text(message))?,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return Ok(()),
            }
        }
    }
}

/// Completes the WebSocket handshake if the frontend asked for the right path.
#[allow(
    clippy::result_large_err,
    reason = "the signature of the handshake callback is defined by tungstenite"
)]
fn accept(stream: TcpStream, path: &str) -> Result<WebSocket<TcpStream>, tungstenite::Error> {
    let check = |request: &Request, response: Response| {
        let origin = request
            .headers()
            .get("origin")
            .and_then(|origin| origin.to_str().ok());
        if request.uri().path() == path && origin == Some(FRONTEND_ORIGIN) {
            Ok(response)
        } else {
            tracing::warn!(?origin, "refused DevTools connection");
            let mut refusal = ErrorResponse::new(None);
            *refusal.status_mut() = StatusCode::FORBIDDEN;
            Err(refusal)
        }
    };
    tungstenite::accept_hdr(stream, check).map_err(|error| match error {
        tungstenite::HandshakeError::Failure(error) => error,
        tungstenite::HandshakeError::Interrupted(_) => {
            tungstenite::Error::Io(io::Error::from(ErrorKind::WouldBlock))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontend_messages_pass_through() {
        assert_eq!(own_response(r#"{"id":12,"result":{}}"#), None);
        assert_eq!(
            own_response(r#"{"method":"Page.loadEventFired","params":{}}"#),
            None
        );
    }

    #[test]
    fn located_nodes_become_inspect_requests() {
        let answer =
            format!(r#"{{"id":{FIRST_OWN_ID},"result":{{"backendNodeId":42,"frameId":"F"}}}}"#);
        let event: Value = serde_json::from_str(&own_response(&answer).flatten().unwrap()).unwrap();
        assert_eq!(event["method"], "Overlay.inspectNodeRequested");
        assert_eq!(event["params"]["backendNodeId"], 42);

        let failure = format!(r#"{{"id":{FIRST_OWN_ID},"error":{{"code":-32000}}}}"#);
        assert_eq!(own_response(&failure), Some(None));
    }

    #[test]
    fn tokens_are_long_and_random() {
        let first = random_token().unwrap();
        assert_eq!(first.len(), 32);
        assert_ne!(first, random_token().unwrap());
    }
}
