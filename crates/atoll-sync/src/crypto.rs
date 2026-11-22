//! End-to-end encryption: the server only ever holds what it cannot read.
//!
//! Every account has one random data key. Records are sealed with it using XChaCha20-Poly1305,
//! with the kind and id of the record as associated data, so the server cannot move a sealed
//! record to another address unnoticed. The data key itself reaches the server only wrapped in a
//! key derived from the sync passphrase with Argon2id. Both match libsodium
//! (`crypto_aead_xchacha20poly1305_ietf_*` and `crypto_pwhash` with Argon2id 1.3), which is what
//! the Android app uses.

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use data_encoding::BASE32_NOPAD;
use serde::{Deserialize, Serialize};

use crate::error::SyncError;
use crate::id::SyncId;
use crate::model::{Kind, Record};

/// The format of everything sealed here, the first byte of each sealed block.
const FORMAT: u8 = 1;
const NONCE_LENGTH: usize = 24;
const SALT_LENGTH: usize = 16;
const KEY_LENGTH: usize = 32;
const WRAP_CONTEXT: &[u8] = b"atoll/1/key";

/// The key all records of an account are sealed with.
#[derive(Clone, PartialEq, Eq)]
pub struct DataKey([u8; KEY_LENGTH]);

impl std::fmt::Debug for DataKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("DataKey(..)")
    }
}

impl Drop for DataKey {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

impl DataKey {
    /// A new random key, made once when sync is set up for an account.
    pub fn generate() -> Self {
        let mut bytes = [0u8; KEY_LENGTH];
        random(&mut bytes);
        Self(bytes)
    }

    pub fn from_bytes(bytes: [u8; KEY_LENGTH]) -> Self {
        Self(bytes)
    }

    /// The raw key, for keeping it in the keyring of the device.
    pub fn as_bytes(&self) -> &[u8; KEY_LENGTH] {
        &self.0
    }

    /// The key written for people: 52 letters and digits in groups of four. Whoever holds it can
    /// read the data, it is the way back when the passphrase is forgotten.
    pub fn recovery_key(&self) -> String {
        let text = BASE32_NOPAD.encode(&self.0);
        text.as_bytes()
            .chunks(4)
            .map(|group| std::str::from_utf8(group).unwrap_or_default())
            .collect::<Vec<_>>()
            .join("-")
    }

    /// Reads a recovery key as [`DataKey::recovery_key`] writes it. Case, spaces and dashes do
    /// not matter.
    pub fn from_recovery_key(text: &str) -> Result<Self, SyncError> {
        let cleaned: String = text
            .chars()
            .filter(|character| character.is_ascii_alphanumeric())
            .map(|character| character.to_ascii_uppercase())
            .collect();
        let bytes = BASE32_NOPAD
            .decode(cleaned.as_bytes())
            .map_err(|_| SyncError::InvalidRecoveryKey)?;
        let bytes: [u8; KEY_LENGTH] = bytes
            .try_into()
            .map_err(|_| SyncError::InvalidRecoveryKey)?;
        Ok(Self(bytes))
    }

    /// Seals `plaintext` as the content of the record `kind`/`id`.
    pub fn seal(&self, kind: Kind, id: SyncId, plaintext: &[u8]) -> Vec<u8> {
        seal_with(&self.0, &record_context(kind, id), plaintext)
    }

    /// Opens what [`DataKey::seal`] sealed for the same record.
    pub fn open(&self, kind: Kind, id: SyncId, sealed: &[u8]) -> Result<Vec<u8>, SyncError> {
        open_with(&self.0, &record_context(kind, id), sealed)
    }

    pub fn seal_record(&self, id: SyncId, record: &Record) -> Vec<u8> {
        let plaintext = serde_json::to_vec(record).expect("a record can always be written");
        self.seal(record.kind(), id, &plaintext)
    }

    /// Opens a sealed record and makes sure it is of the kind it was stored as.
    pub fn open_record(&self, kind: Kind, id: SyncId, sealed: &[u8]) -> Result<Record, SyncError> {
        let record: Record = serde_json::from_slice(&self.open(kind, id, sealed)?)?;
        if record.kind() != kind {
            return Err(SyncError::Corrupt);
        }
        Ok(record)
    }
}

/// How hard turning a passphrase into a key is. The default, 64 MiB and three passes, is
/// libsodium's interactive memory limit with one pass more, still quick on a phone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hardness {
    pub memory_kib: u32,
    pub passes: u32,
}

impl Default for Hardness {
    fn default() -> Self {
        Self {
            memory_kib: 64 * 1024,
            passes: 3,
        }
    }
}

/// The data key wrapped by the sync passphrase, the only form in which it reaches the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyEnvelope {
    pub salt: [u8; SALT_LENGTH],
    pub hardness: Hardness,
    /// The sealed data key, in the same format as a sealed record.
    pub wrapped: Vec<u8>,
}

impl KeyEnvelope {
    /// Wraps `key` with `passphrase`, with a fresh salt.
    pub fn wrap(key: &DataKey, passphrase: &str, hardness: Hardness) -> Result<Self, SyncError> {
        let mut salt = [0u8; SALT_LENGTH];
        random(&mut salt);
        let wrapping = derive(passphrase, &salt, hardness)?;
        Ok(Self {
            salt,
            hardness,
            wrapped: seal_with(&wrapping, WRAP_CONTEXT, key.as_bytes()),
        })
    }

    /// The data key, if `passphrase` is the one it was wrapped with.
    pub fn unwrap(&self, passphrase: &str) -> Result<DataKey, SyncError> {
        let wrapping = derive(passphrase, &self.salt, self.hardness)?;
        let bytes =
            open_with(&wrapping, WRAP_CONTEXT, &self.wrapped).map_err(|error| match error {
                SyncError::Corrupt => SyncError::WrongKey,
                other => other,
            })?;
        let bytes: [u8; KEY_LENGTH] = bytes.try_into().map_err(|_| SyncError::Corrupt)?;
        Ok(DataKey(bytes))
    }
}

fn record_context(kind: Kind, id: SyncId) -> Vec<u8> {
    format!("atoll/{FORMAT}/{}/{id}", kind.as_str()).into_bytes()
}

/// Turns a passphrase into a wrapping key, like libsodium's `crypto_pwhash` with Argon2id 1.3.
fn derive(
    passphrase: &str,
    salt: &[u8; SALT_LENGTH],
    hardness: Hardness,
) -> Result<[u8; KEY_LENGTH], SyncError> {
    let params = Params::new(hardness.memory_kib, hardness.passes, 1, Some(KEY_LENGTH))
        .map_err(|error| SyncError::Derivation(error.to_string()))?;
    let mut key = [0u8; KEY_LENGTH];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .map_err(|error| SyncError::Derivation(error.to_string()))?;
    Ok(key)
}

fn seal_with(key: &[u8; KEY_LENGTH], context: &[u8], plaintext: &[u8]) -> Vec<u8> {
    let mut nonce = [0u8; NONCE_LENGTH];
    random(&mut nonce);
    seal_with_nonce(key, &nonce, context, plaintext)
}

fn seal_with_nonce(
    key: &[u8; KEY_LENGTH],
    nonce: &[u8; NONCE_LENGTH],
    context: &[u8],
    plaintext: &[u8],
) -> Vec<u8> {
    let cipher = XChaCha20Poly1305::new(key.into());
    let sealed = cipher
        .encrypt(
            XNonce::from_slice(nonce),
            Payload {
                msg: plaintext,
                aad: context,
            },
        )
        .expect("sealing never fails for data of this size");
    let mut block = Vec::with_capacity(1 + NONCE_LENGTH + sealed.len());
    block.push(FORMAT);
    block.extend_from_slice(nonce);
    block.extend_from_slice(&sealed);
    block
}

fn open_with(key: &[u8; KEY_LENGTH], context: &[u8], block: &[u8]) -> Result<Vec<u8>, SyncError> {
    let (&format, rest) = block.split_first().ok_or(SyncError::Corrupt)?;
    if format != FORMAT {
        return Err(SyncError::UnknownFormat(format));
    }
    if rest.len() < NONCE_LENGTH {
        return Err(SyncError::Corrupt);
    }
    let (nonce, sealed) = rest.split_at(NONCE_LENGTH);
    XChaCha20Poly1305::new(key.into())
        .decrypt(
            XNonce::from_slice(nonce),
            Payload {
                msg: sealed,
                aad: context,
            },
        )
        .map_err(|_| SyncError::Corrupt)
}

fn random(bytes: &mut [u8]) {
    getrandom::fill(bytes).expect("the system has no source of randomness");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Workspace;

    fn hex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(&text[index..index + 2], 16).unwrap())
            .collect()
    }

    fn quick() -> Hardness {
        Hardness {
            memory_kib: 64,
            passes: 1,
        }
    }

    #[test]
    fn sealing_matches_libsodium() {
        let key: [u8; 32] = core::array::from_fn(|index| index as u8);
        let nonce: [u8; 24] = core::array::from_fn(|index| 0x40 + index as u8);
        let context = b"atoll/1/tab/01890a5d-ac96-4b4e-8f3c-7a1b2c3d4e5f";
        let plaintext = br#"{"kind":"tab","data":{"url":"https://example.org/"}}"#;
        let block = seal_with_nonce(&key, &nonce, context, plaintext);
        let expected = hex(
            "af1b6e19be845b2cad80e6dc8db047f6f3cecce6292271ef185ddf7f2b6c57e4668a38417af7fc7192c2982924b5a896b44900957fc05c541b7750781ded306338d3fd5a",
        );
        assert_eq!(block[0], FORMAT);
        assert_eq!(&block[1..25], &nonce);
        assert_eq!(
            &block[25..],
            expected.as_slice(),
            "crypto_aead_xchacha20poly1305_ietf_encrypt"
        );
        assert_eq!(open_with(&key, context, &block).unwrap(), plaintext);
    }

    #[test]
    fn deriving_matches_libsodium() {
        let salt: [u8; 16] = core::array::from_fn(|index| 0x10 + index as u8);
        let key = derive("korrekt pferd batterie", &salt, Hardness::default()).unwrap();
        assert_eq!(
            key.to_vec(),
            hex("04cbe78a359a6d39be1f2a1f1b9612ae465d43aaf731e8b6693fcd02b8737124"),
            "crypto_pwhash with Argon2id 1.3, opslimit 3, memlimit 64 MiB"
        );
    }

    #[test]
    fn a_record_opens_only_where_it_was_sealed() {
        let key = DataKey::generate();
        let id = SyncId::random();
        let record = Record::Workspace(Workspace {
            name: "Arbeit".to_owned(),
            order: "V".to_owned(),
        });
        let sealed = key.seal_record(id, &record);
        assert_eq!(
            key.open_record(Kind::Workspace, id, &sealed).unwrap(),
            record
        );
        assert!(
            key.open_record(Kind::Workspace, SyncId::random(), &sealed)
                .is_err(),
            "moved to another id"
        );
        assert!(
            key.open_record(Kind::Island, id, &sealed).is_err(),
            "moved to another kind"
        );
        assert!(
            DataKey::generate()
                .open_record(Kind::Workspace, id, &sealed)
                .is_err(),
            "another account"
        );
        let mut changed = sealed.clone();
        *changed.last_mut().unwrap() ^= 1;
        assert!(
            key.open_record(Kind::Workspace, id, &changed).is_err(),
            "changed on the way"
        );
        assert_ne!(
            key.seal_record(id, &record),
            sealed,
            "every seal has its own nonce"
        );
    }

    #[test]
    fn the_passphrase_unwraps_the_data_key_and_nothing_else_does() {
        let key = DataKey::generate();
        let envelope = KeyEnvelope::wrap(&key, "korrekt pferd batterie", quick()).unwrap();
        assert_eq!(envelope.unwrap("korrekt pferd batterie").unwrap(), key);
        assert!(matches!(
            envelope.unwrap("Korrekt pferd batterie"),
            Err(SyncError::WrongKey)
        ));
        let again = KeyEnvelope::wrap(&key, "korrekt pferd batterie", quick()).unwrap();
        assert_ne!(again.salt, envelope.salt);
    }

    #[test]
    fn the_recovery_key_brings_the_data_key_back() {
        let key = DataKey::generate();
        let written = key.recovery_key();
        assert_eq!(written.len(), 52 + 12);
        assert_eq!(DataKey::from_recovery_key(&written).unwrap(), key);
        let typed = written.to_lowercase().replace('-', " ");
        assert_eq!(DataKey::from_recovery_key(&typed).unwrap(), key);
        assert!(DataKey::from_recovery_key(&written[..40]).is_err());
        assert!(DataKey::from_recovery_key("").is_err());
    }

    #[test]
    fn a_newer_format_is_named_instead_of_read() {
        let key = DataKey::generate();
        let mut sealed = key.seal(Kind::Tab, SyncId::random(), b"x");
        sealed[0] = 9;
        assert!(matches!(
            key.open(Kind::Tab, SyncId::random(), &sealed),
            Err(SyncError::UnknownFormat(9))
        ));
    }
}
