//! Logins from 1Password through its command line tool `op`. The tool asks the 1Password app to
//! unlock, for example with a fingerprint, so Atoll never sees the master password.

mod cli;
mod items;
mod matching;

pub use cli::{OnePassword, OpError};
pub use items::{Credentials, Login};
pub use matching::{logins_for, matches};
