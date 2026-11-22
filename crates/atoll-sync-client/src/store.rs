//! Where the refresh token waits between runs: in the keyring of the desktop (Secret Service),
//! and only where there is none in a file that only the user can read. Each Atoll profile has an
//! entry of its own, so a second profile never takes over the sign-in of the first. The key the
//! records are sealed with waits the same way, in an entry of its own.

use std::path::{Path, PathBuf};

use atoll_sync::DataKey;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const SERVICE: &str = "Atoll";

/// What survives a restart: the refresh token and where it is valid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Saved {
    pub server: String,
    pub issuer: String,
    pub client_id: String,
    pub refresh: String,
}

pub struct TokenStore {
    /// The name of the keyring entry of this profile.
    user: String,
    /// The file used when there is no keyring.
    file: PathBuf,
    /// The keyring entry and the file of the data key.
    key_user: String,
    key_file: PathBuf,
    use_keyring: bool,
}

impl TokenStore {
    /// The store of the profile in `data_dir`. `ATOLL_SYNC_NO_KEYRING` keeps it to the file, for
    /// tests that must never touch the keyring of the real desktop.
    pub fn new(data_dir: &Path) -> Self {
        let digest = Sha256::digest(data_dir.to_string_lossy().as_bytes());
        let profile: String = digest
            .iter()
            .take(8)
            .map(|byte| format!("{byte:02x}"))
            .collect();
        Self {
            user: format!("sync-{profile}"),
            file: data_dir.join("sync").join("session.json"),
            key_user: format!("sync-key-{profile}"),
            key_file: data_dir.join("sync").join("key"),
            use_keyring: std::env::var_os("ATOLL_SYNC_NO_KEYRING").is_none(),
        }
    }

    pub fn load(&self) -> Option<Saved> {
        serde_json::from_str(&self.read(&self.user, &self.file)?).ok()
    }

    /// Keeps `saved`, in the keyring when there is one. Fails only when neither worked.
    pub fn save(&self, saved: &Saved) -> std::io::Result<()> {
        let text = serde_json::to_string(saved).expect("the session can always be written");
        self.write(&self.user, &self.file, &text)
    }

    pub fn clear(&self) {
        self.remove(&self.user, &self.file);
    }

    /// The data key unlocked on this device, `None` until it was.
    pub fn load_key(&self) -> Option<DataKey> {
        let text = self.read(&self.key_user, &self.key_file)?;
        let bytes = hex_bytes(text.trim())?;
        Some(DataKey::from_bytes(bytes))
    }

    /// Keeps the data key, so the passphrase is asked for only once per device.
    pub fn save_key(&self, key: &DataKey) -> std::io::Result<()> {
        let text: String = key
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        self.write(&self.key_user, &self.key_file, &text)
    }

    pub fn clear_key(&self) {
        self.remove(&self.key_user, &self.key_file);
    }

    fn read(&self, user: &str, file: &Path) -> Option<String> {
        if let Some(entry) = self.entry(user)
            && let Ok(text) = entry.get_password()
        {
            return Some(text);
        }
        std::fs::read_to_string(file).ok()
    }

    fn write(&self, user: &str, file: &Path, text: &str) -> std::io::Result<()> {
        if let Some(entry) = self.entry(user) {
            match entry.set_password(text) {
                Ok(()) => {
                    let _ = std::fs::remove_file(file);
                    return Ok(());
                }
                Err(error) => {
                    tracing::warn!(%error, "the keyring refused the entry, a file keeps it")
                }
            }
        }
        write_private(file, text)
    }

    fn remove(&self, user: &str, file: &Path) {
        if let Some(entry) = self.entry(user)
            && let Err(error) = entry.delete_credential()
            && !matches!(error, keyring_core::Error::NoEntry)
        {
            tracing::warn!(%error, "the entry could not be removed from the keyring");
        }
        let _ = std::fs::remove_file(file);
    }

    #[cfg(target_os = "linux")]
    fn entry(&self, user: &str) -> Option<keyring_core::Entry> {
        if !self.use_keyring {
            return None;
        }
        let store: std::sync::Arc<keyring_core::CredentialStore> =
            zbus_secret_service_keyring_store::Store::new()
                .map_err(|error| tracing::debug!(%error, "no keyring"))
                .ok()?;
        keyring_core::Entry::new_in_store(&store, SERVICE, user).ok()
    }

    #[cfg(not(target_os = "linux"))]
    fn entry(&self, _user: &str) -> Option<keyring_core::Entry> {
        None
    }
}

fn hex_bytes(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 {
        return None;
    }
    let mut bytes = [0; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(text.get(index * 2..index * 2 + 2)?, 16).ok()?;
    }
    Some(bytes)
}

/// Writes a file only the user can read, in a folder only the user can enter.
fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    #[cfg(unix)]
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

    if let Some(parent) = path.parent() {
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        builder.mode(0o700);
        builder.create(parent)?;
    }
    let partial = path.with_extension("partial");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&partial)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    std::fs::rename(&partial, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directory(name: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("atoll-token-store-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        directory
    }

    fn file_store(directory: &Path) -> TokenStore {
        TokenStore {
            use_keyring: false,
            ..TokenStore::new(directory)
        }
    }

    fn saved(refresh: &str) -> Saved {
        Saved {
            server: "https://atoll.example".to_owned(),
            issuer: "https://auth.example".to_owned(),
            client_id: "atoll".to_owned(),
            refresh: refresh.to_owned(),
        }
    }

    #[test]
    fn without_a_keyring_the_sign_in_waits_in_a_private_file() {
        let directory = directory("file");
        let store = file_store(&directory);
        assert_eq!(store.load(), None);
        store.save(&saved("eins")).unwrap();
        store.save(&saved("zwei")).unwrap();
        assert_eq!(store.load(), Some(saved("zwei")));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&store.file).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
            let folder = std::fs::metadata(store.file.parent().unwrap())
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(folder & 0o777, 0o700);
        }
        store.clear();
        assert_eq!(store.load(), None);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn the_data_key_waits_beside_the_sign_in() {
        let directory = directory("key");
        let store = file_store(&directory);
        assert!(store.load_key().is_none());
        let key = DataKey::generate();
        store.save_key(&key).unwrap();
        store.save(&saved("eins")).unwrap();
        assert_eq!(store.load_key().map(|loaded| loaded.id()), Some(key.id()));
        store.clear();
        assert!(
            store.load_key().is_some(),
            "signing out is not forgetting the key"
        );
        store.clear_key();
        assert!(store.load_key().is_none());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn every_profile_has_an_entry_of_its_own() {
        let first = TokenStore::new(Path::new("/home/someone/.local/share/atoll"));
        let second = TokenStore::new(Path::new("/tmp/test-profile"));
        assert_ne!(first.user, second.user);
        assert_eq!(
            first.user,
            TokenStore::new(Path::new("/home/someone/.local/share/atoll")).user
        );
    }
}
