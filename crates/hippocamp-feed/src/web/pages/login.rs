//! `/login`, `/auth/github/callback` and `/logout`.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};

use axum::extract::{ConnectInfo, Query, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use maud::html;
use serde::Deserialize;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::auth::oauth;
use crate::auth::session::{
    self, append_cookie, constant_time_eq, cookie_value, is_https, CsrfForm, OAUTH_COOKIE,
};
use crate::error::AppError;
use crate::users::{self, SignIn};
use crate::web::layout::login_href;
use crate::web::security::client_ip;
use crate::web::{local_path, AppState, Ctx};

/// The sign-in page, shown only for a failure: a message and a retry link.
fn failure(ctx: &Ctx, https: bool, status: StatusCode, message: &str, retry_to: &str) -> Response {
    let mut response = ctx
        .page("login", "Sign in")
        .body(html! {
            p class="hp-error" { (message) }
            p { a href=(login_href(retry_to)) rel="nofollow" { "Try again" } }
        })
        .into_response();
    *response.status_mut() = status;
    append_cookie(&mut response, &session::oauth_clear_cookie(https));
    response
}

/// `GET /login`: starts sign-in. A member who is already signed in goes
/// straight to `return_to`.
pub async fn login(
    State(state): State<AppState>,
    ctx: Ctx,
    parts: Parts,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    let return_to = local_path(query.get("return_to").map_or("/", String::as_str));
    if ctx.member.is_some() {
        return Ok(Redirect::to(&return_to).into_response());
    }
    let Some(credentials) = state.config.github.clone() else {
        return Err(AppError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "Sign-in is not available on this server.",
        ));
    };
    let connection = parts
        .extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map_or(IpAddr::V6(Ipv6Addr::LOCALHOST), |c| c.0.ip());
    let ip = client_ip(&parts.headers, connection, state.config.trust_proxy);
    state.limiters.check_sign_in(ip)?;

    let oauth_state = oauth::random_value()?;
    let verifier = oauth::random_value()?;
    let url = oauth::authorize_url(
        &state.config,
        &credentials,
        &oauth_state,
        &oauth::code_challenge(&verifier),
    )?;
    let now = state.clock.now();
    let (s, v, r) = (oauth_state.clone(), verifier, return_to);
    state
        .db
        .write(move |tx| Ok(oauth::store_state(tx, &s, &v, &r, now)?))
        .await?;
    let mut response = Redirect::to(&url).into_response();
    append_cookie(
        &mut response,
        &session::oauth_set_cookie(&oauth_state, is_https(&state)),
    );
    Ok(response)
}

/// `GET /auth/github/callback`: checks the state, exchanges the code, reads
/// the member, revokes the token and starts a session.
pub async fn callback(
    State(state): State<AppState>,
    ctx: Ctx,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    let https = is_https(&state);
    let retry = "/";
    if query.contains_key("error") {
        tracing::info!("sign-in refused: GitHub returned an error parameter");
        return Ok(failure(
            &ctx,
            https,
            StatusCode::BAD_REQUEST,
            "Sign-in was cancelled.",
            retry,
        ));
    }
    let failed = |status: StatusCode, reason: &str, retry_to: &str| {
        tracing::info!("sign-in failed: {reason}");
        failure(&ctx, https, status, "Sign-in failed", retry_to)
    };
    let (Some(code), Some(returned)) = (query.get("code"), query.get("state")) else {
        return Ok(failed(
            StatusCode::BAD_REQUEST,
            "missing code or state",
            retry,
        ));
    };
    let Some(cookie) = cookie_value(&headers, OAUTH_COOKIE) else {
        return Ok(failed(StatusCode::BAD_REQUEST, "no state cookie", retry));
    };
    if !constant_time_eq(&cookie, returned) {
        return Ok(failed(
            StatusCode::BAD_REQUEST,
            "state does not match the cookie",
            retry,
        ));
    }
    let Some(credentials) = state.config.github.clone() else {
        return Err(AppError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "Sign-in is not available on this server.",
        ));
    };

    let now = state.clock.now();
    let presented = returned.clone();
    let stored = state
        .db
        .write(move |tx| Ok(oauth::consume_state(tx, &presented, now)?))
        .await?;
    let Some(stored) = stored else {
        return Ok(failed(
            StatusCode::BAD_REQUEST,
            "state unknown, used or expired",
            retry,
        ));
    };
    let retry_to = stored.return_to.clone();

    // Exchange, read the member, revoke: one blocking task.
    let http = state.http.clone();
    let config = state.config.clone();
    let code = code.clone();
    let verifier = stored.verifier;
    let github = tokio::task::spawn_blocking(move || {
        let token = http.exchange_code(
            &config.oauth_token_url,
            &credentials.client_id,
            &credentials.client_secret,
            &code,
            &oauth::redirect_uri(&config),
            &verifier,
        )?;
        let user = http.fetch_user(&config.oauth_api_url, &token);
        if let Err(e) = http.revoke(
            &config.oauth_api_url,
            &credentials.client_id,
            &credentials.client_secret,
            &token,
        ) {
            tracing::warn!("token revocation failed: {e}");
        }
        user
    })
    .await
    .map_err(|e| AppError::internal(format!("sign-in task failed: {e}")))?;
    let github = match github {
        Ok(user) => user,
        Err(e) => {
            return Ok(failed(StatusCode::BAD_GATEWAY, &e.to_string(), &retry_to));
        }
    };
    let created = OffsetDateTime::parse(&github.created_at, &Rfc3339);
    let created = match created {
        Ok(t) if oauth::valid_login(&github.login) => t,
        _ => {
            return Ok(failed(
                StatusCode::BAD_GATEWAY,
                "GitHub's user reply had an invalid login or date",
                &retry_to,
            ))
        }
    };

    let (id, login) = (github.id, github.login);
    let token = state
        .db
        .write(
            move |tx| match users::sign_in(tx, id, &login, created, now)? {
                SignIn::Banned => Ok(None),
                SignIn::Admitted(user) => Ok(Some(session::create(tx, user, now)?)),
            },
        )
        .await?;
    let Some(token) = token else {
        tracing::info!("sign-in refused: banned account");
        let mut response = failure(
            &ctx,
            https,
            StatusCode::FORBIDDEN,
            "This account cannot sign in.",
            "/",
        );
        // A refusal is not an invitation to retry.
        *response.status_mut() = StatusCode::FORBIDDEN;
        return Ok(response);
    };
    let mut response = Redirect::to(&retry_to).into_response();
    append_cookie(&mut response, &session::session_set_cookie(&token, https));
    append_cookie(&mut response, &session::oauth_clear_cookie(https));
    Ok(response)
}

#[derive(Deserialize)]
pub struct Nothing {}

/// `POST /logout`: deletes the session.
pub async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
    CsrfForm(_, _member): CsrfForm<Nothing>,
) -> Result<Response, AppError> {
    let https = is_https(&state);
    if let Some(token) = cookie_value(&headers, session::session_cookie_name(https)) {
        let hash = session::hash_token(&token);
        state
            .db
            .write(move |tx| Ok(session::delete(tx, &hash)?))
            .await?;
    }
    let mut response = Redirect::to("/").into_response();
    append_cookie(&mut response, &session::session_clear_cookie(https));
    Ok(response)
}
