//! Where the refresh token waits between runs: in the keyring of the desktop (Secret Service),
//! and only where there is none in a file that only the user can read. Each Atoll profile has an
//! entry of its own, so a second profile never takes over the sign-in of the first.

use std::path::{Path, PathBuf};

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
            use_keyring: std::env::var_os("ATOLL_SYNC_NO_KEYRING").is_none(),
        }
    }

    pub fn load(&self) -> Option<Saved> {
        if let Some(entry) = self.entry()
            && let Ok(text) = entry.get_password()
            && let Ok(saved) = serde_json::from_str(&text)
        {
            return Some(saved);
        }
        let text = std::fs::read_to_string(&self.file).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Keeps `saved`, in the keyring when there is one. Fails only when neither worked.
    pub fn save(&self, saved: &Saved) -> std::io::Result<()> {
        let text = serde_json::to_string(saved).expect("the session can always be written");
        if let Some(entry) = self.entry() {
            match entry.set_password(&text) {
                Ok(()) => {
                    let _ = std::fs::remove_file(&self.file);
                    return Ok(());
                }
                Err(error) => {
                    tracing::warn!(%error, "the keyring refused the sign-in, a file keeps it")
                }
            }
        }
        write_private(&self.file, &text)
    }

    pub fn clear(&self) {
        if let Some(entry) = self.entry()
            && let Err(error) = entry.delete_credential()
            && !matches!(error, keyring_core::Error::NoEntry)
        {
            tracing::warn!(%error, "the sign-in could not be removed from the keyring");
        }
        let _ = std::fs::remove_file(&self.file);
    }

    #[cfg(target_os = "linux")]
    fn entry(&self) -> Option<keyring_core::Entry> {
        if !self.use_keyring {
            return None;
        }
        let store: std::sync::Arc<keyring_core::CredentialStore> =
            zbus_secret_service_keyring_store::Store::new()
                .map_err(|error| tracing::debug!(%error, "no keyring"))
                .ok()?;
        keyring_core::Entry::new_in_store(&store, SERVICE, &self.user).ok()
    }

    #[cfg(not(target_os = "linux"))]
    fn entry(&self) -> Option<keyring_core::Entry> {
        None
    }
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
