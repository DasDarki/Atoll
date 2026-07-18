use std::io::{self, ErrorKind};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::Duration;

use tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tungstenite::http::StatusCode;
use tungstenite::{Message, WebSocket};

use crate::pump;

/// How often an idle connection checks for messages from the page.
const POLL_INTERVAL: Duration = Duration::from_millis(8);
/// How often the server checks for a new connection or for being stopped.
const ACCEPT_INTERVAL: Duration = Duration::from_millis(50);
/// The origin of the bundled frontend. Connections from anywhere else are refused.
const FRONTEND_ORIGIN: &str = "devtools://devtools";

/// What happened on the frontend connection.
pub(super) enum FrontendEvent {
    Connected,
    Message(String),
    Disconnected,
}

/// Accepts frontend connections one at a time until `stop` is set.
pub(super) fn serve(
    listener: &TcpListener,
    path: &str,
    events: &Sender<FrontendEvent>,
    for_frontend: &Receiver<String>,
    stop: &AtomicBool,
) {
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                if let Err(error) = relay(stream, path, events, for_frontend, stop) {
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
    events: &Sender<FrontendEvent>,
    for_frontend: &Receiver<String>,
    stop: &AtomicBool,
) -> Result<(), tungstenite::Error> {
    stream.set_nonblocking(false)?;
    let mut socket = accept(stream, path)?;
    socket.get_ref().set_read_timeout(Some(POLL_INTERVAL))?;
    while for_frontend.try_recv().is_ok() {}
    notify(events, FrontendEvent::Connected);
    let result = exchange(&mut socket, events, for_frontend, stop);
    notify(events, FrontendEvent::Disconnected);
    result
}

fn exchange(
    socket: &mut WebSocket<TcpStream>,
    events: &Sender<FrontendEvent>,
    for_frontend: &Receiver<String>,
    stop: &AtomicBool,
) -> Result<(), tungstenite::Error> {
    loop {
        if stop.load(Ordering::Relaxed) {
            let _ = socket.close(None);
            return Ok(());
        }
        match socket.read() {
            Ok(Message::Text(text)) => notify(events, FrontendEvent::Message(text.to_string())),
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

/// Hands an event to the UI thread and wakes its message loop.
fn notify(events: &Sender<FrontendEvent>, event: FrontendEvent) {
    if events.send(event).is_ok() {
        pump::schedule(0);
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
