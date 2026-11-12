//! A mock of GitHub's OAuth endpoints, for tests and `hippocamp-feed mock-oauth`.
//!
//! It imitates the authorize, token, user and token-revocation endpoints,
//! PKCE check included. `serve` never mounts it; the dev-mode guard keeps a
//! production server from talking to it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{delete, get, post};
use axum::{Form, Json, Router};
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use maud::html;
use serde::Deserialize;
use serde_json::json;
use time::{Duration, OffsetDateTime};
use url::Url;

use crate::auth::oauth::code_challenge;
use crate::auth::session::{constant_time_eq, random_token};
use crate::clock;

/// The demo OAuth app's client id.
pub const CLIENT_ID: &str = "hippocamp-feed-dev";
/// The demo OAuth app's secret. Not a real secret.
pub const CLIENT_SECRET: &str = "dev-secret-not-real";

/// A user the mock can sign in as.
#[derive(Debug, Clone)]
pub struct MockUser {
    pub id: i64,
    pub login: String,
    pub created_at: OffsetDateTime,
}

impl MockUser {
    pub fn new(id: i64, login: &str, created_at: OffsetDateTime) -> Self {
        Self {
            id,
            login: login.to_string(),
            created_at,
        }
    }
}

/// The users of `hippocamp-feed mock-oauth`: `ada` (1001, GitHub account created
/// 1 March 2015), `grace` (1002, 15 June 2012), `linus` (1003, 1 September
/// 2019), `alan` (1005, 10 May 2016) and `newbie` (1004, ten days before
/// `now`).
pub fn demo_users(now: OffsetDateTime) -> Vec<MockUser> {
    use time::macros::datetime;
    vec![
        MockUser::new(1001, "ada", datetime!(2015-03-01 00:00:00 UTC)),
        MockUser::new(1002, "grace", datetime!(2012-06-15 00:00:00 UTC)),
        MockUser::new(1003, "linus", datetime!(2019-09-01 00:00:00 UTC)),
        MockUser::new(1005, "alan", datetime!(2016-05-10 00:00:00 UTC)),
        MockUser::new(1004, "newbie", now - Duration::days(10)),
    ]
}

struct Code {
    user: String,
    redirect_uri: String,
    challenge: String,
    used: bool,
}

#[derive(Default)]
struct Inner {
    codes: HashMap<String, Code>,
    /// Access token to login.
    tokens: HashMap<String, String>,
    issued: Vec<String>,
    revoked: Vec<String>,
}

struct Shared {
    client_id: String,
    client_secret: String,
    users: Vec<MockUser>,
    inner: Mutex<Inner>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// What a test can ask the mock afterwards.
#[derive(Clone)]
pub struct MockHandle {
    shared: Arc<Shared>,
}

impl MockHandle {
    /// The access tokens the mock has revoked, in order.
    pub fn revoked(&self) -> Vec<String> {
        self.shared.lock().revoked.clone()
    }

    /// Every access token the mock has issued, in order.
    pub fn issued(&self) -> Vec<String> {
        self.shared.lock().issued.clone()
    }
}

/// The mock provider.
pub struct MockProvider;

impl MockProvider {
    /// A router for the endpoints, and a handle to look at what happened.
    #[allow(clippy::new_ret_no_self)]
    pub fn new(client_id: &str, client_secret: &str, users: Vec<MockUser>) -> (Router, MockHandle) {
        let shared = Arc::new(Shared {
            client_id: client_id.to_string(),
            client_secret: client_secret.to_string(),
            users,
            inner: Mutex::new(Inner::default()),
        });
        let router = Router::new()
            .route("/login/oauth/authorize", get(authorize))
            .route("/login/oauth/authorize/approve", get(approve))
            .route("/login/oauth/access_token", post(access_token))
            .route("/user", get(user))
            .route("/applications/{client_id}/token", delete(revoke))
            .with_state(shared.clone());
        (router, MockHandle { shared })
    }

    /// The demo app with the demo users.
    pub fn demo(now: OffsetDateTime) -> (Router, MockHandle) {
        Self::new(CLIENT_ID, CLIENT_SECRET, demo_users(now))
    }
}

fn error_page(status: StatusCode, message: &str) -> Response {
    (status, message.to_string()).into_response()
}

async fn authorize(
    State(shared): State<Arc<Shared>>,
    Query(query): Query<Vec<(String, String)>>,
) -> Response {
    let client = query.iter().find(|(k, _)| k == "client_id");
    if client.map(|(_, v)| v.as_str()) != Some(shared.client_id.as_str()) {
        return error_page(StatusCode::BAD_REQUEST, "unknown client_id");
    }
    let link = |login: &str| {
        let mut ser = url::form_urlencoded::Serializer::new(String::new());
        ser.append_pair("user", login);
        for (k, v) in &query {
            ser.append_pair(k, v);
        }
        format!("/login/oauth/authorize/approve?{}", ser.finish())
    };
    Html(
        html! {
            (maud::DOCTYPE)
            html lang="en" {
                head { meta charset="utf-8"; title { "Mock sign in" } }
                body {
                    h1 { "Mock GitHub: sign in as" }
                    ul {
                        @for u in &shared.users {
                            li { a href=(link(&u.login)) { (u.login) } " (id " (u.id) ")" }
                        }
                    }
                }
            }
        }
        .into_string(),
    )
    .into_response()
}

async fn approve(
    State(shared): State<Arc<Shared>>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let Some(login) = query.get("user") else {
        return error_page(StatusCode::BAD_REQUEST, "no user");
    };
    if !shared.users.iter().any(|u| &u.login == login) {
        return error_page(StatusCode::BAD_REQUEST, "unknown user");
    }
    let (Some(redirect_uri), Some(state)) = (query.get("redirect_uri"), query.get("state")) else {
        return error_page(
            StatusCode::BAD_REQUEST,
            "redirect_uri and state are required",
        );
    };
    let Ok(mut target) = Url::parse(redirect_uri) else {
        return error_page(StatusCode::BAD_REQUEST, "bad redirect_uri");
    };
    let Ok(code) = random_token() else {
        return error_page(StatusCode::INTERNAL_SERVER_ERROR, "no randomness");
    };
    shared.lock().codes.insert(
        code.clone(),
        Code {
            user: login.clone(),
            redirect_uri: redirect_uri.clone(),
            challenge: query.get("code_challenge").cloned().unwrap_or_default(),
            used: false,
        },
    );
    target
        .query_pairs_mut()
        .append_pair("code", &code)
        .append_pair("state", state);
    Redirect::to(target.as_str()).into_response()
}

#[derive(Deserialize)]
struct TokenForm {
    client_id: String,
    client_secret: String,
    code: String,
    redirect_uri: String,
    code_verifier: Option<String>,
}

fn oauth_error(code: &str) -> Response {
    Json(json!({ "error": code })).into_response()
}

async fn access_token(State(shared): State<Arc<Shared>>, Form(f): Form<TokenForm>) -> Response {
    if f.client_id != shared.client_id || !constant_time_eq(&f.client_secret, &shared.client_secret)
    {
        return oauth_error("incorrect_client_credentials");
    }
    let Ok(token) = random_token() else {
        return oauth_error("server_error");
    };
    let mut inner = shared.lock();
    let Some(code) = inner.codes.get_mut(&f.code) else {
        return oauth_error("bad_verification_code");
    };
    if code.used {
        return oauth_error("bad_verification_code");
    }
    if code.redirect_uri != f.redirect_uri {
        return oauth_error("redirect_uri_mismatch");
    }
    code.used = true;
    let verified = f
        .code_verifier
        .as_deref()
        .is_some_and(|v| constant_time_eq(&code_challenge(v), &code.challenge));
    if !verified {
        return oauth_error("bad_verification_code");
    }
    let login = code.user.clone();
    inner.tokens.insert(token.clone(), login);
    inner.issued.push(token.clone());
    Json(json!({ "access_token": token, "token_type": "bearer" })).into_response()
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

async fn user(State(shared): State<Arc<Shared>>, headers: HeaderMap) -> Response {
    let login = bearer(&headers).and_then(|t| shared.lock().tokens.get(t).cloned());
    let Some(login) = login else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    match shared.users.iter().find(|u| u.login == login) {
        Some(u) => Json(json!({
            "id": u.id,
            "login": u.login,
            "created_at": clock::rfc3339(u.created_at),
        }))
        .into_response(),
        None => StatusCode::UNAUTHORIZED.into_response(),
    }
}

async fn revoke(
    State(shared): State<Arc<Shared>>,
    Path(client_id): Path<String>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let expected = STANDARD.encode(format!("{}:{}", shared.client_id, shared.client_secret));
    let given = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Basic "));
    if client_id != shared.client_id || !given.is_some_and(|g| constant_time_eq(g, &expected)) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let token = serde_json::from_slice::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v["access_token"].as_str().map(str::to_string));
    let Some(token) = token else {
        return StatusCode::UNPROCESSABLE_ENTITY.into_response();
    };
    let mut inner = shared.lock();
    inner.tokens.remove(&token);
    inner.revoked.push(token);
    StatusCode::NO_CONTENT.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn demo_users_are_the_documented_ones() {
        let now = datetime!(2026-11-08 10:00:00 UTC);
        let users = demo_users(now);
        let ids: Vec<(i64, &str)> = users.iter().map(|u| (u.id, u.login.as_str())).collect();
        assert_eq!(
            ids,
            vec![
                (1001, "ada"),
                (1002, "grace"),
                (1003, "linus"),
                (1005, "alan"),
                (1004, "newbie")
            ]
        );
        assert_eq!(users[4].created_at, now - Duration::days(10));
    }
}
