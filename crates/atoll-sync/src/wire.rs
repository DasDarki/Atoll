//! The messages between the devices and the sync server, as JSON. Sealed content travels as
//! base64, the server stores it as it comes and never opens it.
//!
//! Every request names the device it comes from in the header [`DEVICE_HEADER`], so the server
//! can list the devices and a device can pass over its own changes when it reads them back.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::crypto::{Hardness, KeyEnvelope};
use crate::id::SyncId;
use crate::model::Kind;

/// The header that carries the id of the device a request comes from.
pub const DEVICE_HEADER: &str = "Atoll-Device";

/// `GET /v1/config`, the one thing a device asks without being signed in: where to sign in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerConfig {
    /// The Onyx issuer, the base of its OAuth endpoints.
    pub issuer: String,
    /// The client id of Atoll in Onyx.
    pub client_id: String,
}

/// A change a device sends: a record written anew, or removed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    pub kind: Kind,
    pub id: SyncId,
    /// The sealed record, `None` when the record was removed.
    #[serde(with = "optional_base64", default)]
    pub data: Option<Vec<u8>>,
}

/// `POST /v1/changes`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Push {
    pub changes: Vec<Change>,
}

/// The answer to a push: the revision the last of its changes got.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pushed {
    pub revision: i64,
}

/// A change as the server keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stored {
    pub revision: i64,
    pub kind: Kind,
    pub id: SyncId,
    #[serde(with = "optional_base64", default)]
    pub data: Option<Vec<u8>>,
    /// The device the change came from.
    pub device: SyncId,
}

/// `GET /v1/changes?since=<revision>`: the next changes after `since`, oldest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Changes {
    pub changes: Vec<Stored>,
    /// Where to continue: the revision of the last change in the page.
    pub cursor: i64,
    /// Whether more changes follow after this page.
    pub more: bool,
}

/// `GET /v1/snapshot`: every record that exists, for setting a device up or mirroring.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub records: Vec<Stored>,
    /// The revision the snapshot stands at, where reading changes continues.
    pub cursor: i64,
}

/// The wrapped data key as the server keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredKey {
    #[serde(flatten)]
    pub envelope: WireEnvelope,
    /// Counts the times the key was wrapped anew, for replacing it without a race.
    pub version: i64,
}

/// `PUT /v1/key`: sets the wrapped key. Without `replaces` only when there is none yet,
/// otherwise only when the stored key still has that version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PutKey {
    #[serde(flatten)]
    pub envelope: WireEnvelope,
    #[serde(default)]
    pub replaces: Option<i64>,
}

/// [`KeyEnvelope`] as it travels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireEnvelope {
    #[serde(with = "base64")]
    pub salt: Vec<u8>,
    pub memory_kib: u32,
    pub passes: u32,
    #[serde(with = "base64")]
    pub wrapped: Vec<u8>,
}

impl From<&KeyEnvelope> for WireEnvelope {
    fn from(envelope: &KeyEnvelope) -> Self {
        Self {
            salt: envelope.salt.to_vec(),
            memory_kib: envelope.hardness.memory_kib,
            passes: envelope.hardness.passes,
            wrapped: envelope.wrapped.clone(),
        }
    }
}

impl TryFrom<&WireEnvelope> for KeyEnvelope {
    type Error = crate::error::SyncError;

    fn try_from(wire: &WireEnvelope) -> Result<Self, Self::Error> {
        Ok(Self {
            salt: wire
                .salt
                .as_slice()
                .try_into()
                .map_err(|_| crate::error::SyncError::Corrupt)?,
            hardness: Hardness {
                memory_kib: wire.memory_kib,
                passes: wire.passes,
            },
            wrapped: wire.wrapped.clone(),
        })
    }
}

/// `PUT /v1/devices/self`: how the device wants to be listed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceName {
    pub name: String,
    pub platform: Platform,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Desktop,
    Android,
}

/// `GET /v1/devices`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    pub id: SyncId,
    pub name: String,
    pub platform: Option<Platform>,
    pub last_seen_at: i64,
    /// Whether this is the device that asks.
    pub current: bool,
}

/// What `GET /v1/events` sends whenever something changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub revision: i64,
}

/// The body of every error answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub error: ErrorCode,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The token is missing, expired or not for Atoll.
    Unauthorized,
    /// The changes since that revision are gone, the device has to read the snapshot.
    ResyncRequired,
    /// No wrapped key is stored yet.
    NoKey,
    /// The key was set or replaced by another device in the meantime.
    KeyConflict,
    /// The request breaks the protocol.
    BadRequest,
    TooLarge,
    RateLimited,
    Internal,
}

mod base64 {
    use super::*;

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&data_encoding::BASE64.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(deserializer)?;
        data_encoding::BASE64
            .decode(text.as_bytes())
            .map_err(serde::de::Error::custom)
    }
}

mod optional_base64 {
    use super::*;

    pub fn serialize<S: Serializer>(
        bytes: &Option<Vec<u8>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match bytes {
            Some(bytes) => serializer.serialize_some(&data_encoding::BASE64.encode(bytes)),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Vec<u8>>, D::Error> {
        Option::<String>::deserialize(deserializer)?
            .map(|text| {
                data_encoding::BASE64
                    .decode(text.as_bytes())
                    .map_err(serde::de::Error::custom)
            })
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::DataKey;

    #[test]
    fn a_change_travels_as_json_with_base64_content() {
        let id: SyncId = "01890a5d-ac96-4b4e-8f3c-7a1b2c3d4e5f".parse().unwrap();
        let change = Change {
            kind: Kind::Tab,
            id,
            data: Some(vec![1, 2, 3]),
        };
        let json = serde_json::to_string(&change).unwrap();
        assert_eq!(
            json,
            r#"{"kind":"tab","id":"01890a5d-ac96-4b4e-8f3c-7a1b2c3d4e5f","data":"AQID"}"#
        );
        assert_eq!(serde_json::from_str::<Change>(&json).unwrap(), change);
        let removed: Change =
            serde_json::from_str(r#"{"kind":"tab","id":"01890a5d-ac96-4b4e-8f3c-7a1b2c3d4e5f"}"#)
                .unwrap();
        assert_eq!(removed.data, None);
    }

    #[test]
    fn the_key_envelope_survives_the_way_to_the_server_and_back() {
        let key = DataKey::generate();
        let envelope = KeyEnvelope::wrap(
            &key,
            "geheim",
            Hardness {
                memory_kib: 64,
                passes: 1,
            },
        )
        .unwrap();
        let put = PutKey {
            envelope: WireEnvelope::from(&envelope),
            replaces: None,
        };
        let json = serde_json::to_string(&put).unwrap();
        let back: PutKey = serde_json::from_str(&json).unwrap();
        let envelope = KeyEnvelope::try_from(&back.envelope).unwrap();
        assert_eq!(envelope.unwrap("geheim").unwrap(), key);
    }

    #[test]
    fn errors_name_their_code() {
        let body = ErrorBody {
            error: ErrorCode::ResyncRequired,
            message: "read the snapshot".to_owned(),
        };
        assert_eq!(
            serde_json::to_string(&body).unwrap(),
            r#"{"error":"resync_required","message":"read the snapshot"}"#
        );
    }
}
