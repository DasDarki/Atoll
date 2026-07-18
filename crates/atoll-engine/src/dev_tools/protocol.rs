use serde_json::{Value, json};

/// Protocol message ids from this value on belong to Atoll instead of the frontend.
pub(super) const FIRST_OWN_ID: i64 = 1 << 30;

/// Chromium appends the session of a flattened child session as the last field of a message.
const SESSION_FIELD: &str = ",\"sessionId\":\"";

/// Splits a message from the browser into the session it belongs to and the message without the
/// session field. Returns `None` for messages of the root session.
pub(super) fn split_session(message: &str) -> Option<(&str, String)> {
    let body = message.strip_suffix("\"}")?;
    let start = body.rfind(SESSION_FIELD)?;
    let session = &body[start + SESSION_FIELD.len()..];
    if session.is_empty() || session.contains('"') {
        return None;
    }
    Some((session, format!("{}}}", &message[..start])))
}

/// Addresses a message of the frontend to `session`, unless it names a session itself.
pub(super) fn with_session(message: &str, session: &str) -> String {
    match serde_json::from_str::<Value>(message) {
        Ok(Value::Object(mut fields)) => {
            if !fields.contains_key("sessionId") {
                fields.insert("sessionId".to_owned(), Value::from(session));
            }
            Value::Object(fields).to_string()
        }
        _ => message.to_owned(),
    }
}

pub(super) fn request(id: i64, method: &str, params: Value) -> String {
    json!({ "id": id, "method": method, "params": params }).to_string()
}

/// The id of a response, read without parsing the whole message.
pub(super) fn response_id(message: &str) -> Option<i64> {
    message
        .strip_prefix("{\"id\":")?
        .split(|character: char| !character.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

/// The result of a response, `None` for errors.
pub(super) fn result(message: &str) -> Option<Value> {
    let mut value: Value = serde_json::from_str(message).ok()?;
    value.get_mut("result").map(Value::take)
}

/// A child session the frontend attached or detached, announced in one of its own events.
pub(super) fn nested_session(message: &str) -> Option<(bool, String)> {
    let attached = if message.starts_with("{\"method\":\"Target.attachedToTarget\"") {
        true
    } else if message.starts_with("{\"method\":\"Target.detachedFromTarget\"") {
        false
    } else {
        return None;
    };
    let value: Value = serde_json::from_str(message).ok()?;
    let session = value.pointer("/params/sessionId")?.as_str()?.to_owned();
    Some((attached, session))
}

/// The event that makes the frontend reveal a node in the Elements panel.
pub(super) fn inspect_node_event(backend_node_id: i64) -> String {
    json!({
        "method": "Overlay.inspectNodeRequested",
        "params": { "backendNodeId": backend_node_id },
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_session_messages_are_split_off() {
        let (session, message) =
            split_session(r#"{"id":3,"result":{"a":"b"},"sessionId":"AB12"}"#).unwrap();
        assert_eq!(session, "AB12");
        assert_eq!(message, r#"{"id":3,"result":{"a":"b"}}"#);
        let (_, event) =
            split_session(r#"{"method":"Page.loadEventFired","params":{},"sessionId":"S"}"#)
                .unwrap();
        assert_eq!(event, r#"{"method":"Page.loadEventFired","params":{}}"#);
    }

    #[test]
    fn root_messages_have_no_session() {
        assert_eq!(split_session(r#"{"id":3,"result":{}}"#), None);
        assert_eq!(
            split_session(r#"{"method":"Target.detachedFromTarget","params":{"sessionId":"S"}}"#),
            None
        );
        assert_eq!(split_session(r#"{"method":"Inspector.detached"}"#), None);
    }

    #[test]
    fn frontend_messages_get_the_session_unless_they_name_one() {
        let addressed: Value =
            serde_json::from_str(&with_session(r#"{"id":1,"method":"DOM.enable"}"#, "S")).unwrap();
        assert_eq!(addressed["sessionId"], "S");
        let nested: Value = serde_json::from_str(&with_session(
            r#"{"id":2,"method":"Runtime.enable","sessionId":"N"}"#,
            "S",
        ))
        .unwrap();
        assert_eq!(nested["sessionId"], "N");
    }

    #[test]
    fn responses_are_recognized_by_their_id() {
        assert_eq!(
            response_id(r#"{"id":1073741824,"result":{}}"#),
            Some(FIRST_OWN_ID)
        );
        assert_eq!(response_id(r#"{"method":"Page.loadEventFired"}"#), None);
        assert_eq!(
            result(r#"{"id":1,"result":{"backendNodeId":42}}"#).unwrap()["backendNodeId"],
            42
        );
        assert_eq!(result(r#"{"id":1,"error":{"code":-32000}}"#), None);
    }

    #[test]
    fn nested_sessions_are_tracked_from_target_events() {
        assert_eq!(
            nested_session(
                r#"{"method":"Target.attachedToTarget","params":{"sessionId":"N","targetInfo":{}}}"#
            ),
            Some((true, "N".to_owned()))
        );
        assert_eq!(
            nested_session(r#"{"method":"Target.detachedFromTarget","params":{"sessionId":"N"}}"#),
            Some((false, "N".to_owned()))
        );
        assert_eq!(nested_session(r#"{"method":"Page.loadEventFired"}"#), None);
    }
}
