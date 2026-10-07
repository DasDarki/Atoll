//! Talking to pages: running scripts and getting their result, putting scripts and styles into
//! every page before its own scripts run, and letting page scripts call back.
//!
//! This goes through Chromium's own protocol, which the browser process can speak without an open
//! developer tools window. Message ids live in a range of their own so they can never collide with
//! the ids of the developer tools front end (see [`crate::dev_tools`]).

use std::collections::HashMap;

use serde_json::{Value, json};

/// Protocol message ids from this value on belong to the page bridge. Chromium only accepts ids
/// that fit into 32 bits, so the bridge sits below the ids of the developer tools front end
/// (`1 << 30`, see [`crate::dev_tools`]) and far above the ids that front end counts up from one.
const FIRST_BRIDGE_ID: i64 = 1 << 29;

/// The name of the world extension scripts run in. It is invisible to the page.
const ISOLATED_WORLD: &str = "atoll";

/// Which JavaScript world a script runs in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ScriptWorld {
    /// A world of its own: it sees the page's document, but not its scripts, and the page cannot
    /// see it either.
    #[default]
    Isolated,
    /// The world the scripts of the page itself run in.
    Page,
}

impl ScriptWorld {
    fn world_name(self) -> Option<&'static str> {
        match self {
            ScriptWorld::Isolated => Some(ISOLATED_WORLD),
            ScriptWorld::Page => None,
        }
    }
}

/// Identifies a script Atoll asked a page to run, answered with [`crate::EngineEvent::ScriptResult`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ScriptId(pub u64);

/// Identifies a message of a page that waits for an answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ReplyId(pub u64);

/// What Atoll asked the browser and what to do with the answer.
enum Pending {
    /// A script whose value the caller waits for.
    Script(ScriptId),
    /// A world that was created for a frame, and the script that waits for it.
    World {
        frame: String,
        script: Option<(ScriptId, String)>,
    },
    /// The frames of a page that was already loaded when the bridge started.
    Frames,
    /// Something whose answer only matters if it failed.
    Quiet,
}

/// What one page message the bridge is waiting to answer needs.
struct Reply {
    context: i64,
    call: i64,
}

/// The state of one browser: what it was asked, which worlds exist and who waits for an answer.
#[derive(Default)]
pub(crate) struct PageBridge {
    started: bool,
    next_message: i64,
    next_request: u64,
    pending: HashMap<i64, Pending>,
    /// The execution contexts of the isolated world, by frame.
    worlds: HashMap<String, i64>,
    /// Which frame an execution context belongs to.
    frames: HashMap<i64, String>,
    /// The frame of the page itself, as the protocol names it.
    main: Option<String>,
    /// What every frame currently shows. Chromium names frames differently than CEF does, so the
    /// addresses are taken from the protocol itself.
    urls: HashMap<String, String>,
    /// The scripts that run in every new page, by the name the caller gave them.
    scripts: HashMap<String, String>,
    replies: HashMap<u64, Reply>,
    /// Messages for a browser that is not ready yet.
    queued: Vec<String>,
}

impl PageBridge {
    /// Whether the browser was already told about the bridge.
    pub(crate) fn is_started(&self) -> bool {
        self.started
    }

    /// Keeps messages until the browser exists.
    pub(crate) fn queue(&mut self, messages: Vec<String>) {
        self.queued.extend(messages);
    }

    /// Takes what was kept while the browser was not ready.
    pub(crate) fn take_queued(&mut self) -> Vec<String> {
        std::mem::take(&mut self.queued)
    }

    fn message_id(&mut self) -> i64 {
        self.next_message += 1;
        FIRST_BRIDGE_ID + self.next_message
    }

    fn request_id(&mut self) -> u64 {
        self.next_request += 1;
        self.next_request
    }
}

/// What the bridge wants the browser to be told, built without touching the browser itself so it
/// stays testable.
pub(crate) struct Outgoing {
    pub(crate) messages: Vec<String>,
    pub(crate) request: Option<ScriptId>,
}

impl Outgoing {
    fn new(messages: Vec<String>) -> Self {
        Self {
            messages,
            request: None,
        }
    }
}

/// The first messages of a browser: the domains the bridge needs, the callback for page messages
/// and the script that puts `window.atoll` into every page.
pub(crate) fn start(bridge: &mut PageBridge, binding: &str) -> Outgoing {
    if bridge.started {
        return Outgoing::new(Vec::new());
    }
    bridge.started = true;
    let mut messages = Vec::new();
    for (method, params) in [
        ("Runtime.enable", json!({})),
        ("Page.enable", json!({})),
        ("Runtime.addBinding", json!({ "name": binding })),
        (
            "Runtime.addBinding",
            json!({ "name": binding, "executionContextName": ISOLATED_WORLD }),
        ),
    ] {
        let id = bridge.message_id();
        bridge.pending.insert(id, Pending::Quiet);
        messages.push(message(id, method, params));
    }
    // A page that was already loaded sends no `Page.frameNavigated` any more, so its frames are
    // asked for once. Without this nothing could be run in a page the bridge came to late.
    let id = bridge.message_id();
    bridge.pending.insert(id, Pending::Frames);
    messages.push(message(id, "Page.getFrameTree", json!({})));

    let source = bridge_script(binding);
    for world in [ScriptWorld::Isolated, ScriptWorld::Page] {
        let id = bridge.message_id();
        bridge.pending.insert(id, Pending::Quiet);
        messages.push(message(
            id,
            "Page.addScriptToEvaluateOnNewDocument",
            script_params(&source, world),
        ));
    }
    Outgoing::new(messages)
}

/// Runs `code` in `world` of the page and answers with its value.
pub(crate) fn evaluate(bridge: &mut PageBridge, code: &str, world: ScriptWorld) -> Outgoing {
    let request = ScriptId(bridge.request_id());
    let main_frame = bridge.main.clone().unwrap_or_default();
    let main_frame = main_frame.as_str();
    let context = match world {
        ScriptWorld::Page => None,
        ScriptWorld::Isolated => match bridge.worlds.get(main_frame).copied() {
            Some(context) => Some(context),
            None => {
                // The world has to exist before anything can run in it.
                let id = bridge.message_id();
                bridge.pending.insert(
                    id,
                    Pending::World {
                        frame: main_frame.to_owned(),
                        script: Some((request, code.to_owned())),
                    },
                );
                let message = request_world(id, main_frame);
                return Outgoing {
                    messages: vec![message],
                    request: Some(request),
                };
            }
        },
    };
    let id = bridge.message_id();
    bridge.pending.insert(id, Pending::Script(request));
    Outgoing {
        messages: vec![message(
            id,
            "Runtime.evaluate",
            evaluate_params(code, context),
        )],
        request: Some(request),
    }
}

/// Adds a script that runs in every page of this browser before the page's own scripts, and
/// replaces the script of the same name.
pub(crate) fn add_document_script(
    bridge: &mut PageBridge,
    name: &str,
    source: &str,
    world: ScriptWorld,
) -> Outgoing {
    let mut messages = remove_document_script(bridge, name).messages;
    let id = bridge.message_id();
    bridge.pending.insert(id, Pending::Quiet);
    bridge.scripts.insert(name.to_owned(), String::new());
    messages.push(message(
        id,
        "Page.addScriptToEvaluateOnNewDocument",
        script_params(source, world),
    ));
    // Remember which message belongs to the name so the answer can store the identifier.
    bridge.pending.insert(
        id,
        Pending::World {
            frame: format!("script:{name}"),
            script: None,
        },
    );
    Outgoing::new(messages)
}

pub(crate) fn remove_document_script(bridge: &mut PageBridge, name: &str) -> Outgoing {
    let Some(identifier) = bridge.scripts.remove(name).filter(|id| !id.is_empty()) else {
        return Outgoing::new(Vec::new());
    };
    let id = bridge.message_id();
    bridge.pending.insert(id, Pending::Quiet);
    Outgoing::new(vec![message(
        id,
        "Page.removeScriptToEvaluateOnNewDocument",
        json!({ "identifier": identifier }),
    )])
}

/// Answers a message of a page.
pub(crate) fn answer_page(
    bridge: &mut PageBridge,
    reply: ReplyId,
    answer: Result<Value, String>,
) -> Outgoing {
    let Some(waiting) = bridge.replies.remove(&reply.0) else {
        return Outgoing::new(Vec::new());
    };
    let (ok, value) = match answer {
        Ok(value) => (true, value),
        Err(message) => (false, Value::from(message)),
    };
    let code = format!(
        "window.atoll && window.atoll.__answer({}, {}, {})",
        waiting.call, ok, value
    );
    let id = bridge.message_id();
    bridge.pending.insert(id, Pending::Quiet);
    Outgoing::new(vec![message(
        id,
        "Runtime.evaluate",
        evaluate_params(&code, Some(waiting.context)),
    )])
}

/// What arrived from the browser and what it means for Atoll.
pub(crate) enum Incoming {
    /// Nothing that concerns the bridge.
    Ignored,
    /// More has to be sent, for example the script that waited for its world.
    Send(Vec<String>),
    /// A script finished.
    Script {
        request: ScriptId,
        result: Result<Value, String>,
    },
    /// A page called `window.atoll.send`.
    Message {
        frame_url: String,
        channel: String,
        payload: Value,
        reply: ReplyId,
    },
}

/// Reads one protocol message. Messages of the developer tools front end are ignored.
pub(crate) fn receive(bridge: &mut PageBridge, message: &str) -> Incoming {
    let Ok(value) = serde_json::from_str::<Value>(message) else {
        return Incoming::Ignored;
    };
    if value.get("sessionId").is_some() {
        return Incoming::Ignored;
    }
    if let Some(id) = value.get("id").and_then(Value::as_i64) {
        return answered(bridge, id, &value);
    }
    match value.get("method").and_then(Value::as_str) {
        Some("Runtime.executionContextCreated") => {
            context_created(bridge, &value);
            Incoming::Ignored
        }
        Some("Runtime.executionContextDestroyed") => {
            if let Some(context) = value
                .pointer("/params/executionContextId")
                .and_then(Value::as_i64)
            {
                forget_context(bridge, context);
            }
            Incoming::Ignored
        }
        Some("Runtime.executionContextsCleared") => {
            bridge.worlds.clear();
            bridge.frames.clear();
            Incoming::Ignored
        }
        Some("Runtime.bindingCalled") => binding_called(bridge, &value),
        Some("Page.frameNavigated") => {
            if let Some(frame) = value.pointer("/params/frame")
                && let (Some(id), Some(url)) = (
                    frame.get("id").and_then(Value::as_str),
                    frame.get("url").and_then(Value::as_str),
                )
            {
                if frame.get("parentId").is_none() {
                    bridge.main = Some(id.to_owned());
                }
                bridge.urls.insert(id.to_owned(), url.to_owned());
            }
            Incoming::Ignored
        }
        Some("Page.navigatedWithinDocument") => {
            if let (Some(id), Some(url)) = (
                value.pointer("/params/frameId").and_then(Value::as_str),
                value.pointer("/params/url").and_then(Value::as_str),
            ) {
                bridge.urls.insert(id.to_owned(), url.to_owned());
            }
            Incoming::Ignored
        }
        Some("Page.frameDetached") => {
            if let Some(id) = value.pointer("/params/frameId").and_then(Value::as_str) {
                bridge.urls.remove(id);
            }
            Incoming::Ignored
        }
        _ => Incoming::Ignored,
    }
}

/// An answer to something Atoll asked.
fn answered(bridge: &mut PageBridge, id: i64, value: &Value) -> Incoming {
    if id < FIRST_BRIDGE_ID {
        return Incoming::Ignored;
    }
    let Some(pending) = bridge.pending.remove(&id) else {
        return Incoming::Ignored;
    };
    let error = value
        .pointer("/error/message")
        .and_then(Value::as_str)
        .map(str::to_owned);
    match pending {
        Pending::Quiet => {
            if let Some(error) = error {
                tracing::debug!(%error, "the page bridge was refused");
            }
            Incoming::Ignored
        }
        Pending::Frames => {
            if let Some(frame) = value.pointer("/result/frameTree/frame")
                && let Some(id) = frame.get("id").and_then(Value::as_str)
            {
                bridge.main = Some(id.to_owned());
                if let Some(url) = frame.get("url").and_then(Value::as_str) {
                    bridge.urls.insert(id.to_owned(), url.to_owned());
                }
            }
            Incoming::Ignored
        }
        Pending::Script(request) => Incoming::Script {
            request,
            result: script_result(value, error),
        },
        Pending::World { frame, script } => {
            if let Some(name) = frame.strip_prefix("script:") {
                if let Some(identifier) =
                    value.pointer("/result/identifier").and_then(Value::as_str)
                    && bridge.scripts.contains_key(name)
                {
                    bridge
                        .scripts
                        .insert(name.to_owned(), identifier.to_owned());
                }
                return Incoming::Ignored;
            }
            let context = value
                .pointer("/result/executionContextId")
                .and_then(Value::as_i64);
            if let Some(context) = context {
                bridge.worlds.insert(frame.clone(), context);
                bridge.frames.insert(context, frame);
            }
            match (script, context) {
                (Some((request, code)), Some(context)) => {
                    let id = bridge.message_id();
                    bridge.pending.insert(id, Pending::Script(request));
                    Incoming::Send(vec![message(
                        id,
                        "Runtime.evaluate",
                        evaluate_params(&code, Some(context)),
                    )])
                }
                (Some((request, _)), None) => Incoming::Script {
                    request,
                    result: Err(error.unwrap_or_else(|| "die Seite antwortet nicht".to_owned())),
                },
                (None, _) => Incoming::Ignored,
            }
        }
    }
}

/// The value of a finished script, or what went wrong.
fn script_result(value: &Value, error: Option<String>) -> Result<Value, String> {
    if let Some(error) = error {
        return Err(error);
    }
    if let Some(thrown) = value.pointer("/result/exceptionDetails") {
        let message = thrown
            .pointer("/exception/description")
            .and_then(Value::as_str)
            .or_else(|| thrown.get("text").and_then(Value::as_str))
            .unwrap_or("das Skript hat einen Fehler ausgelöst");
        return Err(message.lines().next().unwrap_or(message).to_owned());
    }
    Ok(value
        .pointer("/result/result/value")
        .cloned()
        .unwrap_or(Value::Null))
}

fn context_created(bridge: &mut PageBridge, value: &Value) {
    let Some(context) = value.pointer("/params/context") else {
        return;
    };
    let Some(id) = context.get("id").and_then(Value::as_i64) else {
        return;
    };
    let Some(frame) = context.pointer("/auxData/frameId").and_then(Value::as_str) else {
        return;
    };
    bridge.frames.insert(id, frame.to_owned());
    if context.get("name").and_then(Value::as_str) == Some(ISOLATED_WORLD) {
        bridge.worlds.insert(frame.to_owned(), id);
    }
}

fn forget_context(bridge: &mut PageBridge, context: i64) {
    if let Some(frame) = bridge.frames.remove(&context)
        && bridge.worlds.get(&frame) == Some(&context)
    {
        bridge.worlds.remove(&frame);
    }
}

/// A page called the binding, which is how `window.atoll.send` reaches Atoll.
fn binding_called(bridge: &mut PageBridge, value: &Value) -> Incoming {
    let Some(payload) = value.pointer("/params/payload").and_then(Value::as_str) else {
        return Incoming::Ignored;
    };
    let Ok(sent) = serde_json::from_str::<Value>(payload) else {
        return Incoming::Ignored;
    };
    let context = value
        .pointer("/params/executionContextId")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    let Some(channel) = sent.get("channel").and_then(Value::as_str) else {
        return Incoming::Ignored;
    };
    let call = sent.get("call").and_then(Value::as_i64).unwrap_or_default();
    let reply = ReplyId(bridge.request_id());
    bridge.replies.insert(reply.0, Reply { context, call });
    let frame_url = bridge
        .frames
        .get(&context)
        .and_then(|frame| bridge.urls.get(frame))
        .cloned()
        .unwrap_or_default();
    Incoming::Message {
        frame_url,
        channel: channel.to_owned(),
        payload: sent.get("payload").cloned().unwrap_or(Value::Null),
        reply,
    }
}

fn message(id: i64, method: &str, params: Value) -> String {
    json!({ "id": id, "method": method, "params": params }).to_string()
}

fn request_world(id: i64, frame: &str) -> String {
    message(
        id,
        "Page.createIsolatedWorld",
        json!({ "frameId": frame, "worldName": ISOLATED_WORLD, "grantUniveralAccess": false }),
    )
}

fn evaluate_params(code: &str, context: Option<i64>) -> Value {
    let mut params = json!({
        "expression": code,
        "awaitPromise": true,
        "returnByValue": true,
        "userGesture": true,
    });
    if let Some(context) = context {
        params["contextId"] = json!(context);
    }
    params
}

fn script_params(source: &str, world: ScriptWorld) -> Value {
    let mut params = json!({ "source": source, "runImmediately": true });
    if let Some(name) = world.world_name() {
        params["worldName"] = json!(name);
    }
    params
}

/// The script that gives every page a `window.atoll` with `send` and `on`.
fn bridge_script(binding: &str) -> String {
    BRIDGE_SCRIPT.replace("__BINDING__", &Value::from(binding).to_string())
}

const BRIDGE_SCRIPT: &str = r#"(() => {
  const name = __BINDING__;
  const send = window[name];
  if (typeof send !== "function" || window.atoll) return;
  const waiting = new Map();
  const handlers = new Map();
  let next = 0;
  const api = {
    send(channel, payload) {
      const call = ++next;
      return new Promise((resolve, reject) => {
        waiting.set(call, { resolve, reject });
        send(JSON.stringify({ call, channel: String(channel), payload }));
      });
    },
    on(channel, handler) {
      handlers.set(String(channel), handler);
    },
    __answer(call, ok, value) {
      const entry = waiting.get(call);
      if (!entry) return;
      waiting.delete(call);
      if (ok) entry.resolve(value); else entry.reject(new Error(String(value)));
    },
    __deliver(channel, payload) {
      const handler = handlers.get(String(channel));
      if (handler) handler(payload);
    },
  };
  Object.defineProperty(window, "atoll", { value: Object.freeze(api), configurable: true });
})();
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn method_of(message: &str) -> String {
        serde_json::from_str::<Value>(message).unwrap()["method"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    fn id_of(message: &str) -> i64 {
        serde_json::from_str::<Value>(message).unwrap()["id"]
            .as_i64()
            .unwrap()
    }

    #[test]
    fn the_frames_of_a_page_that_was_already_loaded_are_learned() {
        let mut bridge = PageBridge::default();
        let out = start(&mut bridge, "atollSend1");
        let asking = out
            .messages
            .iter()
            .find(|message| method_of(message) == "Page.getFrameTree")
            .map(|message| id_of(message))
            .expect("the frames are asked for");
        assert_eq!(bridge.main, None, "nothing is known before the answer");

        let answer = json!({
            "id": asking,
            "result": { "frameTree": { "frame": { "id": "FRAME1", "url": "https://example.org/" } } }
        })
        .to_string();
        receive(&mut bridge, &answer);
        assert_eq!(bridge.main.as_deref(), Some("FRAME1"));
        assert_eq!(
            bridge.urls.get("FRAME1").map(String::as_str),
            Some("https://example.org/")
        );
    }

    #[test]
    fn the_first_messages_set_up_the_bridge() {
        let mut bridge = PageBridge::default();
        let out = start(&mut bridge, "atollSend1");
        let methods: Vec<String> = out
            .messages
            .iter()
            .map(|message| method_of(message))
            .collect();
        assert_eq!(
            methods,
            [
                "Runtime.enable",
                "Page.enable",
                "Runtime.addBinding",
                "Runtime.addBinding",
                // A page that was already loaded has to be asked for its frames, see `start`.
                "Page.getFrameTree",
                "Page.addScriptToEvaluateOnNewDocument",
                "Page.addScriptToEvaluateOnNewDocument",
            ]
        );
        assert!(out.messages[3].contains("\"executionContextName\":\"atoll\""));
        assert!(out.messages[5].contains("\"worldName\":\"atoll\""));
        assert!(
            !out.messages[6].contains("worldName"),
            "the page world has no name"
        );
        assert!(
            out.messages[0..1]
                .iter()
                .all(|message| id_of(message) > FIRST_BRIDGE_ID)
        );
        assert!(
            start(&mut bridge, "atollSend1").messages.is_empty(),
            "the bridge is only started once"
        );
    }

    #[test]
    fn a_script_waits_for_its_world_and_then_runs() {
        let mut bridge = PageBridge::default();
        start(&mut bridge, "b");
        bridge.main = Some("FRAME1".to_owned());
        let out = evaluate(&mut bridge, "document.title", ScriptWorld::Isolated);
        let request = out.request.unwrap();
        assert_eq!(method_of(&out.messages[0]), "Page.createIsolatedWorld");

        let answer =
            json!({ "id": id_of(&out.messages[0]), "result": { "executionContextId": 5 } });
        let Incoming::Send(messages) = receive(&mut bridge, &answer.to_string()) else {
            panic!("the script should run once the world exists");
        };
        assert_eq!(method_of(&messages[0]), "Runtime.evaluate");
        let evaluate_id = id_of(&messages[0]);
        assert!(messages[0].contains("\"contextId\":5"));

        let result = json!({ "id": evaluate_id, "result": { "result": { "value": "Titel" } } });
        let Incoming::Script {
            request: answered,
            result,
        } = receive(&mut bridge, &result.to_string())
        else {
            panic!("the value should come back");
        };
        assert_eq!(answered, request);
        assert_eq!(result, Ok(Value::from("Titel")));

        let again = evaluate(&mut bridge, "1 + 1", ScriptWorld::Isolated);
        assert_eq!(
            method_of(&again.messages[0]),
            "Runtime.evaluate",
            "the world is known now"
        );
    }

    #[test]
    fn a_thrown_error_reaches_the_caller() {
        let mut bridge = PageBridge {
            main: Some("F".to_owned()),
            ..PageBridge::default()
        };
        bridge.worlds.insert("F".to_owned(), 3);
        let out = evaluate(&mut bridge, "boom()", ScriptWorld::Isolated);
        let answer = json!({
            "id": id_of(&out.messages[0]),
            "result": {
                "exceptionDetails": { "exception": { "description": "ReferenceError: boom\n  at x" } }
            }
        });
        let Incoming::Script { result, .. } = receive(&mut bridge, &answer.to_string()) else {
            panic!("an error is a result too");
        };
        assert_eq!(result, Err("ReferenceError: boom".to_owned()));
    }

    #[test]
    fn pages_can_send_messages_and_get_an_answer() {
        let mut bridge = PageBridge::default();
        start(&mut bridge, "b");
        receive(
            &mut bridge,
            &json!({
                "method": "Runtime.executionContextCreated",
                "params": { "context": { "id": 7, "name": "atoll", "auxData": { "frameId": "F7" } } }
            })
            .to_string(),
        );
        receive(
            &mut bridge,
            &json!({
                "method": "Page.frameNavigated",
                "params": { "frame": { "id": "F7", "url": "https://example.org/a" } }
            })
            .to_string(),
        );
        let called = json!({
            "method": "Runtime.bindingCalled",
            "params": {
                "name": "b",
                "executionContextId": 7,
                "payload": "{\"call\":1,\"channel\":\"hallo\",\"payload\":{\"a\":1}}"
            }
        });
        let Incoming::Message {
            frame_url,
            channel,
            payload,
            reply,
        } = receive(&mut bridge, &called.to_string())
        else {
            panic!("the message should arrive");
        };
        assert_eq!(frame_url, "https://example.org/a");
        assert_eq!(channel, "hallo");
        assert_eq!(payload, json!({ "a": 1 }));

        let out = answer_page(&mut bridge, reply, Ok(json!({ "ok": true })));
        assert_eq!(method_of(&out.messages[0]), "Runtime.evaluate");
        assert!(out.messages[0].contains("__answer(1, true, {\\\"ok\\\":true})"));
        assert!(
            answer_page(&mut bridge, reply, Ok(Value::Null))
                .messages
                .is_empty(),
            "a message is only answered once"
        );
    }

    #[test]
    fn document_scripts_are_replaced_by_name() {
        let mut bridge = PageBridge::default();
        let added =
            add_document_script(&mut bridge, "demo", "console.log(1)", ScriptWorld::Isolated);
        assert_eq!(added.messages.len(), 1);
        receive(
            &mut bridge,
            &json!({ "id": id_of(&added.messages[0]), "result": { "identifier": "1.2" } })
                .to_string(),
        );
        let again =
            add_document_script(&mut bridge, "demo", "console.log(2)", ScriptWorld::Isolated);
        assert_eq!(
            method_of(&again.messages[0]),
            "Page.removeScriptToEvaluateOnNewDocument"
        );
        assert!(again.messages[0].contains("\"identifier\":\"1.2\""));
        assert_eq!(
            method_of(&again.messages[1]),
            "Page.addScriptToEvaluateOnNewDocument"
        );
    }

    #[test]
    fn messages_of_the_developer_tools_are_left_alone() {
        let mut bridge = PageBridge::default();
        assert!(matches!(
            receive(&mut bridge, r#"{"id":5,"result":{},"sessionId":"S"}"#),
            Incoming::Ignored
        ));
        assert!(matches!(
            receive(&mut bridge, r#"{"id":5,"result":{}}"#),
            Incoming::Ignored
        ));
        assert!(matches!(
            receive(&mut bridge, "kein json"),
            Incoming::Ignored
        ));
    }
}
