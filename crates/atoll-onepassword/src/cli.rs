use std::io;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use thiserror::Error;

use crate::items::{Credentials, Login, parse_credentials, parse_logins};

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum OpError {
    #[error("the 1Password CLI is not installed")]
    Missing,
    #[error("the 1Password CLI is not connected to the 1Password app")]
    SignedOut,
    #[error("unlocking 1Password was cancelled")]
    Cancelled,
    #[error("the 1Password CLI failed: {0}")]
    Failed(String),
    #[error("the 1Password CLI answered in an unexpected format: {0}")]
    Invalid(String),
}

/// The 1Password command line tool. Every call may wait until the user unlocks 1Password in
/// its app, so call it off the user interface thread.
#[derive(Debug, Clone)]
pub struct OnePassword {
    program: PathBuf,
}

impl OnePassword {
    /// Uses the `op` program at `program`, usually just `op` from the search path.
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
        }
    }

    /// The version of the CLI, which also tells whether it is installed.
    pub fn version(&self) -> Result<String, OpError> {
        self.run(&["--version"])
            .map(|output| output.trim().to_owned())
    }

    /// Every login of every vault the account can see, without secrets.
    pub fn logins(&self) -> Result<Vec<Login>, OpError> {
        let output = self.run(&["item", "list", "--categories", "Login", "--format", "json"])?;
        parse_logins(&output).map_err(|error| OpError::Invalid(error.to_string()))
    }

    /// Username, password and the current one-time password of the login `id`.
    pub fn credentials(&self, id: &str) -> Result<Credentials, OpError> {
        let output = self.run(&["item", "get", id, "--format", "json", "--reveal"])?;
        parse_credentials(&output).map_err(|error| OpError::Invalid(error.to_string()))
    }

    fn run(&self, arguments: &[&str]) -> Result<String, OpError> {
        let output = Command::new(&self.program)
            .args(arguments)
            .stdin(Stdio::null())
            .output()
            .map_err(|error| match error.kind() {
                io::ErrorKind::NotFound => OpError::Missing,
                _ => OpError::Failed(error.to_string()),
            })?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            Err(classify(&String::from_utf8_lossy(&output.stderr)))
        }
    }
}

impl Default for OnePassword {
    fn default() -> Self {
        Self::new("op")
    }
}

/// Maps an error message of the CLI to what the user can do about it.
fn classify(message: &str) -> OpError {
    let lower = message.to_lowercase();
    if ["dismissed", "cancelled", "canceled", "denied"]
        .iter()
        .any(|word| lower.contains(word))
    {
        OpError::Cancelled
    } else if [
        "not currently signed in",
        "not signed in",
        "no accounts configured",
        "sign in",
        "signin",
        "connecting to desktop app",
    ]
    .iter()
    .any(|phrase| lower.contains(phrase))
    {
        OpError::SignedOut
    } else {
        let line = message
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("unknown error");
        OpError::Failed(line.trim_start_matches("[ERROR]").trim().to_owned())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    use super::*;

    /// A stand-in for `op` that answers from `script`, a shell `case` over the arguments.
    fn fake_op(name: &str, script: &str) -> (OnePassword, PathBuf) {
        let directory =
            std::env::temp_dir().join(format!("atoll-op-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let program = directory.join("op");
        fs::write(&program, format!("#!/bin/sh\n{script}\n")).unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        (OnePassword::new(&program), directory)
    }

    fn remove(directory: &Path) {
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn logins_and_credentials_are_read_from_the_cli() {
        let (op, directory) = fake_op(
            "read",
            r#"case "$1 $2" in
  "item list") echo '[{"id":"a1","title":"Forum","additional_information":"me","urls":[{"href":"https://forum.example"}]}]' ;;
  "item get") [ "$3" = "a1" ] && [ "$6" = "--reveal" ] && echo '{"fields":[{"purpose":"USERNAME","value":"me"},{"purpose":"PASSWORD","value":"pw"}]}' ;;
  *) echo "2.30.0" ;;
esac"#,
        );
        assert_eq!(op.version().unwrap(), "2.30.0");
        let logins = op.logins().unwrap();
        assert_eq!(logins.len(), 1);
        assert_eq!(logins[0].urls, ["https://forum.example"]);
        let credentials = op.credentials("a1").unwrap();
        assert_eq!(credentials.username, "me");
        assert_eq!(credentials.password, "pw");
        assert_eq!(credentials.one_time_code, None);
        remove(&directory);
    }

    #[test]
    fn a_missing_cli_is_reported() {
        let op = OnePassword::new("/nonexistent/atoll/op");
        assert_eq!(op.logins(), Err(OpError::Missing));
    }

    #[test]
    fn failures_are_classified() {
        let (op, directory) = fake_op(
            "fail",
            "echo '[ERROR] 2026/09/19 authorization prompt dismissed, please try again' >&2\nexit 1",
        );
        assert_eq!(op.logins(), Err(OpError::Cancelled));
        remove(&directory);

        assert_eq!(
            classify("[ERROR] account is not signed in"),
            OpError::SignedOut
        );
        assert_eq!(
            classify("[ERROR] no accounts configured for use with 1Password CLI"),
            OpError::SignedOut
        );
        assert_eq!(
            classify("\n[ERROR] \"x\" isn't an item\n"),
            OpError::Failed("\"x\" isn't an item".to_owned())
        );
    }

    #[test]
    fn unexpected_output_is_invalid() {
        let (op, directory) = fake_op("invalid", "echo 'Hallo'");
        assert!(matches!(op.logins(), Err(OpError::Invalid(_))));
        remove(&directory);
    }
}
