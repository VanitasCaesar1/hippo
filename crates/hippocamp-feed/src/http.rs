//! The outbound HTTP agent: sign-in's calls to GitHub (or the mock provider)
//! and the capped fetches of registry sync.
//!
//! One `ureq` agent with a 10-second overall timeout, which hands back every
//! status as a response and follows no redirect. Every function here blocks,
//! so callers run it in `tokio::task::spawn_blocking`.

use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use serde::Deserialize;

use crate::error::AppError;

/// The registry index is read up to this many bytes (5 MB).
pub const REGISTRY_LIMIT: u64 = 5_000_000;
/// A theme pack's stylesheet is read up to this many bytes (64 KiB).
pub const THEME_LIMIT: u64 = 64 * 1024;
/// Replies of GitHub's token and user endpoints are small.
const API_LIMIT: u64 = 64 * 1024;

/// An outbound request failed. The message never holds a secret.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct HttpError(pub String);

impl From<HttpError> for AppError {
    fn from(e: HttpError) -> Self {
        AppError::internal(e)
    }
}

fn fail(what: &str, e: impl std::fmt::Display) -> HttpError {
    HttpError(format!("{what}: {e}"))
}

/// The one agent, cheap to clone.
#[derive(Clone)]
pub struct Http {
    agent: ureq::Agent,
}

impl Default for Http {
    fn default() -> Self {
        Self::new()
    }
}

impl Http {
    pub fn new() -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(10)))
            .http_status_as_error(false)
            .max_redirects(0)
            .max_redirects_will_error(false)
            .user_agent(format!("hippocamp-feed/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        Self { agent }
    }

    /// Exchanges the authorisation `code` for an access token: a `POST` to
    /// `token_url` with the form fields GitHub requires. Errors name the
    /// failure, never the code, the secret or the token.
    pub fn exchange_code(
        &self,
        token_url: &str,
        client_id: &str,
        client_secret: &str,
        code: &str,
        redirect_uri: &str,
        code_verifier: &str,
    ) -> Result<String, HttpError> {
        let mut response = self
            .agent
            .post(token_url)
            .header("Accept", "application/json")
            .send_form([
                ("client_id", client_id),
                ("client_secret", client_secret),
                ("code", code),
                ("redirect_uri", redirect_uri),
                ("code_verifier", code_verifier),
            ])
            .map_err(|e| fail("token request failed", e))?;
        let status = response.status().as_u16();
        let body = response
            .body_mut()
            .with_config()
            .limit(API_LIMIT)
            .read_to_vec()
            .map_err(|e| fail("token reply unreadable", e))?;
        #[derive(Deserialize)]
        struct Reply {
            access_token: Option<String>,
            error: Option<String>,
        }
        let reply: Reply = serde_json::from_slice(&body)
            .map_err(|_| HttpError(format!("token endpoint answered {status} without JSON")))?;
        match (reply.access_token, reply.error) {
            (Some(token), _) if status == 200 && !token.is_empty() => Ok(token),
            (_, Some(error)) => Err(HttpError(format!("token endpoint refused: {error}"))),
            _ => Err(HttpError(format!("token endpoint answered {status}"))),
        }
    }

    /// `GET <api_url>/user` with the token.
    pub fn fetch_user(&self, api_url: &str, token: &str) -> Result<GithubUser, HttpError> {
        let mut response = self
            .agent
            .get(format!("{}/user", api_url.trim_end_matches('/')))
            .header("Authorization", format!("Bearer {token}"))
            .header("Accept", "application/vnd.github+json")
            .call()
            .map_err(|e| fail("user request failed", e))?;
        let status = response.status().as_u16();
        if status != 200 {
            return Err(HttpError(format!("user endpoint answered {status}")));
        }
        let body = response
            .body_mut()
            .with_config()
            .limit(API_LIMIT)
            .read_to_vec()
            .map_err(|e| fail("user reply unreadable", e))?;
        serde_json::from_slice(&body).map_err(|e| fail("user reply is not the expected JSON", e))
    }

    /// `DELETE <api_url>/applications/<client_id>/token`, with basic
    /// authentication of the OAuth app and the token in a JSON body.
    pub fn revoke(
        &self,
        api_url: &str,
        client_id: &str,
        client_secret: &str,
        token: &str,
    ) -> Result<(), HttpError> {
        let basic = STANDARD.encode(format!("{client_id}:{client_secret}"));
        let body = serde_json::json!({ "access_token": token }).to_string();
        let response = self
            .agent
            .delete(format!(
                "{}/applications/{client_id}/token",
                api_url.trim_end_matches('/')
            ))
            .force_send_body()
            .header("Authorization", format!("Basic {basic}"))
            .header("Accept", "application/vnd.github+json")
            .content_type("application/json")
            .send(body)
            .map_err(|e| fail("revocation failed", e))?;
        let status = response.status().as_u16();
        if status == 204 {
            Ok(())
        } else {
            Err(HttpError(format!("revocation answered {status}")))
        }
    }

    /// `GET url`, reading at most `limit` bytes of the decoded body. A body
    /// over the limit, a non-200 status and a transport error are errors.
    pub fn fetch_capped(&self, url: &str, limit: u64) -> Result<Vec<u8>, HttpError> {
        let mut response = self
            .agent
            .get(url)
            .call()
            .map_err(|e| fail("fetch failed", e))?;
        let status = response.status().as_u16();
        if status != 200 {
            return Err(HttpError(format!("the server answered {status}")));
        }
        response
            .body_mut()
            .with_config()
            .limit(limit)
            .read_to_vec()
            .map_err(|e| fail("the body is unreadable or over the limit", e))
    }

    /// The registry index: at most [`REGISTRY_LIMIT`] bytes.
    pub fn fetch_registry(&self, url: &str) -> Result<Vec<u8>, HttpError> {
        self.fetch_capped(url, REGISTRY_LIMIT)
    }

    /// A theme pack's stylesheet: at most [`THEME_LIMIT`] bytes.
    pub fn fetch_theme(&self, url: &str) -> Result<Vec<u8>, HttpError> {
        self.fetch_capped(url, THEME_LIMIT)
    }
}

/// What a theme stylesheet fetch found: a 200 body within the limit, or a 200
/// body past it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fetched {
    Body(Vec<u8>),
    TooLarge,
}

impl Http {
    /// A theme pack's stylesheet, telling a body past [`THEME_LIMIT`] (a
    /// refusal) from a failed fetch (no connection, a timeout, a redirect or
    /// a status other than 200: an error, so nothing is recorded).
    pub fn fetch_theme_checked(&self, url: &str) -> Result<Fetched, HttpError> {
        let mut response = self
            .agent
            .get(url)
            .call()
            .map_err(|e| fail("fetch failed", e))?;
        let status = response.status().as_u16();
        if status != 200 {
            return Err(HttpError(format!("the server answered {status}")));
        }
        match response
            .body_mut()
            .with_config()
            .limit(THEME_LIMIT)
            .read_to_vec()
        {
            Ok(body) => Ok(Fetched::Body(body)),
            Err(ureq::Error::BodyExceedsLimit(_)) => Ok(Fetched::TooLarge),
            Err(e) => Err(fail("the body is unreadable", e)),
        }
    }
}

/// What the feed reads of GitHub's `/user`.
#[derive(Debug, Clone, Deserialize)]
pub struct GithubUser {
    pub id: i64,
    pub login: String,
    pub created_at: String,
}
