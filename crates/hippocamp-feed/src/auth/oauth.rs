//! GitHub's OAuth web application flow: `state`, PKCE (`S256`) and no scope.
//!
//! This module holds the flow's pieces; the pages that drive them are in
//! `web::pages::login`.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use rusqlite::{params, OptionalExtension, Transaction};
use sha2::{Digest, Sha256};
use time::{Duration, OffsetDateTime};
use url::Url;

use crate::auth::session::{hash_token, random_token};
use crate::clock;
use crate::config::{Config, GithubCredentials};
use crate::error::AppError;
use crate::policy;

/// A new random `state` or PKCE verifier: 32 random bytes as base64url
/// without padding.
pub fn random_value() -> Result<String, AppError> {
    random_token()
}

/// The PKCE `code_challenge` of a verifier: the base64url, without padding,
/// of the SHA-256 of the verifier's ASCII text (RFC 7636, `S256`).
pub fn code_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// `<FEED_BASE_URL>/auth/github/callback`.
pub fn redirect_uri(config: &Config) -> String {
    format!(
        "{}/auth/github/callback",
        config.base_url.as_deref().unwrap_or_default()
    )
}

/// Where the browser goes to sign in: the authorize URL with `client_id`,
/// `redirect_uri`, `state`, `code_challenge`, `code_challenge_method=S256` and
/// `allow_signup=true`, and no `scope`.
pub fn authorize_url(
    config: &Config,
    credentials: &GithubCredentials,
    state: &str,
    challenge: &str,
) -> Result<String, AppError> {
    let mut url = Url::parse(&config.oauth_authorize_url)
        .map_err(|e| AppError::internal(format!("FEED_OAUTH_AUTHORIZE_URL: {e}")))?;
    url.query_pairs_mut()
        .append_pair("client_id", &credentials.client_id)
        .append_pair("redirect_uri", &redirect_uri(config))
        .append_pair("state", state)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("allow_signup", "true");
    Ok(url.to_string())
}

/// Stores the state's hash with the verifier, the `return_to` and the time.
pub fn store_state(
    tx: &Transaction,
    state: &str,
    verifier: &str,
    return_to: &str,
    now: OffsetDateTime,
) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO oauth_states (state_hash, verifier, return_to, created_at)
         VALUES (?1, ?2, ?3, ?4)",
        params![hash_token(state), verifier, return_to, clock::storage(now)],
    )?;
    Ok(())
}

/// What a consumed state held.
#[derive(Debug, PartialEq, Eq)]
pub struct StoredState {
    pub verifier: String,
    pub return_to: String,
}

/// Deletes the state's row, so a state works once, and returns what it held
/// if the row was at most 10 minutes old.
pub fn consume_state(
    tx: &Transaction,
    state: &str,
    now: OffsetDateTime,
) -> rusqlite::Result<Option<StoredState>> {
    let row: Option<(String, String, String)> = tx
        .query_row(
            "DELETE FROM oauth_states WHERE state_hash = ?1
             RETURNING verifier, return_to, created_at",
            [hash_token(state)],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let oldest = clock::storage(now - Duration::minutes(policy::OAUTH_STATE_MINUTES));
    Ok(row.and_then(|(verifier, return_to, created_at)| {
        (created_at >= oldest).then_some(StoredState {
            verifier,
            return_to,
        })
    }))
}

/// A GitHub login: 1 to 39 ASCII letters, digits and hyphens. `~` cannot
/// occur, so the feed's placeholder logins never collide with a real one.
pub fn valid_login(login: &str) -> bool {
    (1..=39).contains(&login.len())
        && login
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc_7636_appendix_b_challenge() {
        assert_eq!(
            code_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn random_values_are_43_characters() {
        assert_eq!(random_value().expect("value").len(), 43);
    }

    #[test]
    fn logins() {
        assert!(valid_login("ada"));
        assert!(valid_login("a-b-1"));
        assert!(!valid_login(""));
        assert!(!valid_login("~former-1"));
        assert!(!valid_login("a b"));
        assert!(!valid_login(&"a".repeat(40)));
    }
}
