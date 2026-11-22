//! The desktop side of Atoll Sync: signing in with Onyx, staying signed in, and later talking to
//! the sync server. Nothing here knows the user interface, the app calls it from worker threads.

pub mod api;
pub mod auth;
pub mod engine;
pub mod login;
pub mod store;
pub mod tokens;

pub use api::{Api, ApiError, Tokens};
pub use auth::Auth;
pub use engine::{Choice, Command, Engine, Mode, Report, Setup};
pub use login::{Authorized, Login, LoginError};
pub use store::TokenStore;
pub use tokens::{Profile, TokenError};
