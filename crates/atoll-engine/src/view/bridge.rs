//! Connects [`crate::page_bridge`] to a browser: it registers the observer for Chromium's
//! protocol, sends what the bridge asks for and turns what comes back into engine events.

use std::os::raw::c_int;
use std::sync::OnceLock;

use cef::{
    Browser, DevToolsMessageObserver, ImplBrowserHost, ImplDevToolsMessageObserver,
    WrapDevToolsMessageObserver, rc::Rc as _, wrap_dev_tools_message_observer,
};
use serde_json::Value;

use crate::events::EngineEvent;
use crate::page_bridge::{self, Incoming, Outgoing, ReplyId, ScriptId, ScriptWorld};
use crate::registry::{self, Runtime};
use crate::view::ViewId;
use crate::view::state::ViewState;

wrap_dev_tools_message_observer! {
    pub(crate) struct PageBridgeObserver {
        view: ViewId,
    }

    impl DevToolsMessageObserver {
        fn on_dev_tools_message(&self, _browser: Option<&mut Browser>, message: Option<&[u8]>) -> c_int {
            let Some(text) = message.and_then(|bytes| std::str::from_utf8(bytes).ok()) else {
                return 0;
            };
            tracing::debug!(message = &text[..text.len().min(160)], "BRIDGERECV");
            receive(self.view, text);
            0
        }
    }
}

/// The name of the callback pages reach Atoll through. It is different in every run, so a page
/// cannot rely on it or set it up itself before Atoll does.
fn binding_name() -> &'static str {
    static NAME: OnceLock<String> = OnceLock::new();
    NAME.get_or_init(|| {
        let mut bytes = [0_u8; 8];
        getrandom::fill(&mut bytes).ok();
        format!("atollSend{}", u64::from_le_bytes(bytes))
    })
}

/// Sends what the bridge asked for. A browser that is still being created keeps its messages
/// until [`browser_ready`] says it can take them.
fn send(state: &ViewState, outgoing: Outgoing) {
    let Some(host) = state.host() else {
        tracing::debug!(count = outgoing.messages.len(), "BRIDGEQUEUE");
        state.bridge.borrow_mut().queue(outgoing.messages);
        return;
    };
    listen(state, &host);
    let queued = state.bridge.borrow_mut().take_queued();
    for message in queued.into_iter().chain(outgoing.messages) {
        let sent = host.send_dev_tools_message(Some(message.as_bytes()));
        tracing::debug!(
            sent,
            message = &message[..message.len().min(120)],
            "BRIDGESEND"
        );
    }
}

/// Registers the observer for this browser, once.
fn listen(state: &ViewState, host: &cef::BrowserHost) {
    if state.bridge_registration.borrow().is_some() {
        return;
    }
    let mut observer = PageBridgeObserver::new(state.id);
    *state.bridge_registration.borrow_mut() =
        host.add_dev_tools_message_observer(Some(&mut observer));
}

/// The browser of `state` exists now, so everything that was kept can go out.
pub(crate) fn browser_ready(state: &ViewState) {
    if !state.bridge.borrow().is_started() {
        return;
    }
    send(
        state,
        Outgoing {
            messages: Vec::new(),
            request: None,
        },
    );
}

/// Sets the page side up, once per browser.
fn ensure_started(state: &ViewState) {
    if state.bridge.borrow().is_started() {
        return;
    }
    let outgoing = page_bridge::start(&mut state.bridge.borrow_mut(), binding_name());
    send(state, outgoing);
}

/// Runs `code` in the page and answers with [`EngineEvent::ScriptResult`].
pub(crate) fn evaluate(state: &ViewState, code: &str, world: ScriptWorld) -> Option<ScriptId> {
    ensure_started(state);
    let outgoing = page_bridge::evaluate(&mut state.bridge.borrow_mut(), code, world);
    let request = outgoing.request;
    send(state, outgoing);
    request
}

/// Adds a script that runs in every page of this browser before the page's own scripts.
pub(crate) fn add_document_script(state: &ViewState, name: &str, source: &str, world: ScriptWorld) {
    ensure_started(state);
    let outgoing =
        page_bridge::add_document_script(&mut state.bridge.borrow_mut(), name, source, world);
    send(state, outgoing);
}

pub(crate) fn remove_document_script(state: &ViewState, name: &str) {
    let outgoing = page_bridge::remove_document_script(&mut state.bridge.borrow_mut(), name);
    send(state, outgoing);
}

/// Answers a message a page sent with `window.atoll.send`.
pub(crate) fn answer_page(state: &ViewState, reply: ReplyId, answer: Result<Value, String>) {
    let outgoing = page_bridge::answer_page(&mut state.bridge.borrow_mut(), reply, answer);
    send(state, outgoing);
}

/// Reads one protocol message and tells the application what it means.
fn receive(view: ViewId, text: &str) {
    registry::with_view(view, |runtime, state| {
        let incoming = page_bridge::receive(&mut state.bridge.borrow_mut(), text);
        handle(runtime, state, incoming);
    });
}

fn handle(runtime: &Runtime, state: &ViewState, incoming: Incoming) {
    match incoming {
        Incoming::Ignored => {}
        Incoming::Send(messages) => send(
            state,
            Outgoing {
                messages,
                request: None,
            },
        ),
        Incoming::Script { request, result } => runtime.emit(EngineEvent::ScriptResult {
            view: state.id,
            request,
            result,
        }),
        Incoming::Message {
            frame_url,
            channel,
            payload,
            reply,
        } => {
            runtime.emit(EngineEvent::PageMessage {
                view: state.id,
                frame_url,
                channel,
                payload,
                reply,
            });
        }
    }
}
