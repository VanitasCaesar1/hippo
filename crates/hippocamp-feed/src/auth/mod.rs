//! Sign-in: GitHub's OAuth flow, sessions with CSRF, and the mock provider.

pub mod mock;
pub mod oauth;
pub mod session;

pub use session::{CsrfForm, MaybeUser, RequireAdmin, RequireModerator, RequireUser};
