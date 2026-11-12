//! Sessions, cookies, the request extractors and CSRF.
//!
//! The session token is 32 random bytes as base64url; the database keeps only
//! its SHA-256. Cookies are built and parsed here by hand: there is no cookie
//! crate.

use axum::extract::{FromRequest, FromRequestParts, Request};
use axum::http::request::Parts;
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::de::DeserializeOwned;
use time::{Duration, OffsetDateTime};

use crate::clock;
use crate::db::{Member, Role, UserId};
use crate::error::AppError;
use crate::policy;
use crate::users;
use crate::votes;
use crate::web::assets::sha256_hex;
use crate::web::layout::login_href;
use crate::web::{local_path, AppState};

/// The most a form body may hold: personal CSS of 64 KiB, percent-encoded.
const FORM_LIMIT: usize = 512 * 1024;

/// The session cookie's name: `__Host-hippocamp_session` on `https`,
/// `hippocamp_session` on `http`.
pub fn session_cookie_name(https: bool) -> &'static str {
    if https {
        "__Host-hippocamp_session"
    } else {
        "hippocamp_session"
    }
}

/// The sign-in state cookie's name.
pub const OAUTH_COOKIE: &str = "hippocamp_oauth";

/// Whether the configured base URL is `https`.
pub fn is_https(state: &AppState) -> bool {
    state
        .config
        .base_url
        .as_deref()
        .is_some_and(|u| u.starts_with("https://"))
}

/// 32 random bytes as base64url without padding (43 characters).
pub fn random_token() -> Result<String, AppError> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| AppError::internal(format!("no randomness: {e}")))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

/// The database's form of a token: lower-case hex of its SHA-256.
pub fn hash_token(token: &str) -> String {
    sha256_hex(token.as_bytes())
}

/// The session cookie's `Set-Cookie` value.
pub fn session_set_cookie(token: &str, https: bool) -> String {
    let name = session_cookie_name(https);
    let max_age = policy::SESSION_DAYS * 86_400;
    if https {
        format!("{name}={token}; Secure; HttpOnly; SameSite=Lax; Path=/; Max-Age={max_age}")
    } else {
        format!("{name}={token}; HttpOnly; SameSite=Lax; Path=/; Max-Age={max_age}")
    }
}

/// The `Set-Cookie` value that removes the session cookie.
pub fn session_clear_cookie(https: bool) -> String {
    let name = session_cookie_name(https);
    if https {
        format!("{name}=; Secure; HttpOnly; SameSite=Lax; Path=/; Max-Age=0")
    } else {
        format!("{name}=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0")
    }
}

/// The sign-in state cookie: the `state` itself, for ten minutes, sent only
/// below `/auth`.
pub fn oauth_set_cookie(state_value: &str, https: bool) -> String {
    let max_age = policy::OAUTH_STATE_MINUTES * 60;
    let secure = if https { "; Secure" } else { "" };
    format!("{OAUTH_COOKIE}={state_value}; HttpOnly; SameSite=Lax; Path=/auth; Max-Age={max_age}{secure}")
}

/// The `Set-Cookie` value that removes the sign-in state cookie.
pub fn oauth_clear_cookie(https: bool) -> String {
    let secure = if https { "; Secure" } else { "" };
    format!("{OAUTH_COOKIE}=; HttpOnly; SameSite=Lax; Path=/auth; Max-Age=0{secure}")
}

/// The value of cookie `name` in a `Cookie` header: pairs split on `;` and
/// trimmed; the first match wins.
pub fn parse_cookie<'a>(header_value: &'a str, name: &str) -> Option<&'a str> {
    header_value
        .split(';')
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(k, _)| k.trim() == name)
        .map(|(_, v)| v.trim())
}

/// The value of cookie `name` across all `Cookie` headers of a request.
pub fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(|v| parse_cookie(v, name))
        .map(str::to_string)
}

/// Appends a `Set-Cookie` header.
pub fn append_cookie(response: &mut Response, value: &str) {
    if let Ok(v) = HeaderValue::from_str(value) {
        response.headers_mut().append(header::SET_COOKIE, v);
    }
}

/// Compares two strings in time that depends only on their lengths.
pub fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// A new session for `user`: returns the token for the cookie. The database
/// keeps the token's hash, a CSRF token and the times.
pub fn create(tx: &Transaction, user: UserId, now: OffsetDateTime) -> Result<String, AppError> {
    let token = random_token()?;
    let csrf = random_token()?;
    tx.execute(
        "INSERT INTO sessions (token_hash, user_id, csrf, created_at, expires_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            hash_token(&token),
            user.0,
            csrf,
            clock::storage(now),
            clock::storage(now + Duration::days(policy::SESSION_DAYS)),
        ],
    )?;
    Ok(token)
}

/// Deletes the session named by a token's hash.
pub fn delete(tx: &Transaction, token_hash: &str) -> rusqlite::Result<()> {
    tx.execute("DELETE FROM sessions WHERE token_hash = ?1", [token_hash])?;
    Ok(())
}

/// The member of the session with this token hash, if it is unexpired and the
/// account is neither banned nor deleted.
pub fn lookup(
    conn: &Connection,
    token_hash: &str,
    now: OffsetDateTime,
) -> rusqlite::Result<Option<Member>> {
    let member = conn
        .query_row(
            "SELECT u.id, u.login, u.role, u.created_at, u.github_created_at, u.restricted_until,
                u.banned, u.theme, u.personal_css != '', s.csrf, u.github_id
         FROM sessions s JOIN users u ON u.id = s.user_id
         WHERE s.token_hash = ?1 AND s.expires_at > ?2 AND u.banned = 0 AND u.deleted_at IS NULL",
            params![token_hash, clock::storage(now)],
            |r| {
                let parse = |text: Option<String>| text.as_deref().and_then(clock::parse_storage);
                Ok(Member {
                    id: UserId(r.get(0)?),
                    login: r.get(1)?,
                    role: if r.get::<_, String>(2)? == "moderator" {
                        Role::Moderator
                    } else {
                        Role::None
                    },
                    created_at: parse(r.get(3)?).unwrap_or(OffsetDateTime::UNIX_EPOCH),
                    github_created_at: parse(r.get(4)?),
                    restricted_until: parse(r.get(5)?),
                    banned: r.get(6)?,
                    theme: r.get(7)?,
                    has_personal_css: r.get(8)?,
                    csrf: r.get(9)?,
                    karma: 0,
                    github_id: r.get(10)?,
                })
            },
        )
        .optional()?;
    // Karma is computed when read, so every signed-in request carries it.
    member
        .map(|mut m| {
            m.karma = votes::karma(conn, m.id, now)?;
            Ok(m)
        })
        .transpose()
}

/// The member a request is signed in as, found by its cookie.
pub async fn resolve(state: &AppState, headers: &HeaderMap) -> Result<Option<Member>, AppError> {
    let Some(token) = cookie_value(headers, session_cookie_name(is_https(state))) else {
        return Ok(None);
    };
    let hash = hash_token(&token);
    let now = state.clock.now();
    state.db.read(move |c| Ok(lookup(c, &hash, now)?)).await
}

/// The result of resolving, kept on the request so that several extractors
/// share one lookup.
#[derive(Clone)]
struct Resolved(Option<Member>);

pub(crate) async fn resolve_cached(
    parts: &mut Parts,
    state: &AppState,
) -> Result<Option<Member>, AppError> {
    if let Some(Resolved(member)) = parts.extensions.get::<Resolved>() {
        return Ok(member.clone());
    }
    let member = resolve(state, &parts.headers).await?;
    parts.extensions.insert(Resolved(member.clone()));
    Ok(member)
}

/// The signed-in member, or none.
pub struct MaybeUser(pub Option<Member>);

impl FromRequestParts<AppState> for MaybeUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        Ok(MaybeUser(resolve_cached(parts, state).await?))
    }
}

/// A signed-in member. A `GET` without a session redirects (303) to sign-in
/// and back; any other method is refused with 403.
pub struct RequireUser(pub Member);

impl FromRequestParts<AppState> for RequireUser {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        match resolve_cached(parts, state).await {
            Ok(Some(member)) => Ok(RequireUser(member)),
            Ok(None) if parts.method == Method::GET || parts.method == Method::HEAD => {
                let full = match parts.uri.query() {
                    Some(q) => format!("{}?{q}", parts.uri.path()),
                    None => parts.uri.path().to_string(),
                };
                Err(Redirect::to(&login_href(&local_path(&full))).into_response())
            }
            Ok(None) => Err(AppError::forbidden("Sign in to do that.").into_response()),
            Err(e) => Err(e.into_response()),
        }
    }
}

/// A moderator or an admin; anyone else, signed in or not, gets 404.
pub struct RequireModerator(pub Member);

impl FromRequestParts<AppState> for RequireModerator {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        match resolve_cached(parts, state).await? {
            Some(m) if m.role == Role::Moderator || users::is_admin(&m, &state.config) => {
                Ok(RequireModerator(m))
            }
            _ => Err(AppError::not_found()),
        }
    }
}

/// An admin (a GitHub id in `FEED_ADMINS`); anyone else gets 404.
pub struct RequireAdmin(pub Member);

impl FromRequestParts<AppState> for RequireAdmin {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        match resolve_cached(parts, state).await? {
            Some(m) if users::is_admin(&m, &state.config) => Ok(RequireAdmin(m)),
            _ => Err(AppError::not_found()),
        }
    }
}

/// A form `POST` of a signed-in member that carries the session's CSRF token.
///
/// Before reading the body it requires a session, an `Origin` header that is
/// absent or the feed's own, and a `Sec-Fetch-Site` that is not `cross-site`;
/// then it compares the `csrf` field in constant time. Any failure is 403.
/// `.0` is the deserialised form, `.1` the member.
pub struct CsrfForm<T>(pub T, pub Member);

fn csrf_refused() -> AppError {
    AppError::forbidden("This request could not be verified. Reload the page and try again.")
}

impl<T: DeserializeOwned> FromRequest<AppState> for CsrfForm<T> {
    type Rejection = AppError;

    async fn from_request(req: Request, state: &AppState) -> Result<Self, Self::Rejection> {
        let (mut parts, body) = req.into_parts();
        let member = resolve_cached(&mut parts, state)
            .await?
            .ok_or_else(csrf_refused)?;
        if let Some(origin) = parts.headers.get(header::ORIGIN) {
            let own = state.config.base_url.as_deref().unwrap_or_default();
            if origin.to_str().ok() != Some(own) {
                return Err(csrf_refused());
            }
        }
        if parts
            .headers
            .get("sec-fetch-site")
            .is_some_and(|v| v.as_bytes().eq_ignore_ascii_case(b"cross-site"))
        {
            return Err(csrf_refused());
        }
        let bytes = axum::body::to_bytes(body, FORM_LIMIT)
            .await
            .map_err(|_| AppError::new(StatusCode::PAYLOAD_TOO_LARGE, "The form is too large."))?;
        let token = url::form_urlencoded::parse(&bytes)
            .find(|(k, _)| k == "csrf")
            .map(|(_, v)| v.into_owned());
        let ok = token
            .as_deref()
            .is_some_and(|t| constant_time_eq(t, &member.csrf));
        if !ok {
            return Err(csrf_refused());
        }
        let form = serde_urlencoded::from_bytes(&bytes)
            .map_err(|_| AppError::bad_request("The form could not be read."))?;
        Ok(CsrfForm(form, member))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookies_parse_with_several_pairs_spaces_and_a_repeated_name() {
        let h = "a=1;  hippocamp_session=first ; b=2; hippocamp_session=second;c";
        assert_eq!(parse_cookie(h, "hippocamp_session"), Some("first"));
        assert_eq!(parse_cookie(h, "a"), Some("1"));
        assert_eq!(parse_cookie(h, "b"), Some("2"));
        assert_eq!(parse_cookie(h, "c"), None);
        assert_eq!(parse_cookie(h, "missing"), None);
        assert_eq!(parse_cookie("", "a"), None);
        assert_eq!(parse_cookie("a=x=y", "a"), Some("x=y"));
        // A name that only ends the same way is not a match.
        assert_eq!(
            parse_cookie("xhippocamp_session=1", "hippocamp_session"),
            None
        );
    }

    #[test]
    fn cookies_are_found_across_headers() {
        let mut h = HeaderMap::new();
        h.append(header::COOKIE, HeaderValue::from_static("a=1"));
        h.append(header::COOKIE, HeaderValue::from_static("b=2"));
        assert_eq!(cookie_value(&h, "b").as_deref(), Some("2"));
    }

    #[test]
    fn set_cookie_values() {
        let https = session_set_cookie("tok", true);
        assert_eq!(
            https,
            "__Host-hippocamp_session=tok; Secure; HttpOnly; SameSite=Lax; Path=/; Max-Age=2592000"
        );
        let http = session_set_cookie("tok", false);
        assert_eq!(
            http,
            "hippocamp_session=tok; HttpOnly; SameSite=Lax; Path=/; Max-Age=2592000"
        );
        assert_eq!(
            oauth_set_cookie("s", true),
            "hippocamp_oauth=s; HttpOnly; SameSite=Lax; Path=/auth; Max-Age=600; Secure"
        );
        assert_eq!(
            oauth_set_cookie("s", false),
            "hippocamp_oauth=s; HttpOnly; SameSite=Lax; Path=/auth; Max-Age=600"
        );
        assert!(session_clear_cookie(false).contains("Max-Age=0"));
    }

    #[test]
    fn tokens_are_43_url_safe_characters_and_differ() {
        let a = random_token().expect("token");
        let b = random_token().expect("token");
        assert_eq!(a.len(), 43);
        assert_ne!(a, b);
        assert!(a
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'));
        assert_eq!(hash_token("abc").len(), 64);
    }

    #[test]
    fn constant_time_eq_compares_values() {
        assert!(constant_time_eq("abc", "abc"));
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("abc", "abcd"));
        assert!(constant_time_eq("", ""));
    }

    #[test]
    fn lookup_carries_the_members_karma() {
        use crate::posts::testkit::*;
        use time::macros::datetime;
        let now = datetime!(2026-11-08 10:00:00 UTC);
        let old = now - Duration::days(400);
        let (_d, mut conn) = fresh();
        let ada = user(&conn, "ada", old, old);
        let bob = user(&conn, "bob", old, old);
        let id = post(&conn, ada, "t", now);
        cast(&conn, bob, "post", id, now);
        let tx = conn.transaction().expect("tx");
        let token = create(&tx, UserId(ada), now).expect("session");
        tx.commit().expect("commit");
        let member = lookup(&conn, &hash_token(&token), now)
            .expect("lookup")
            .expect("member");
        assert_eq!(member.karma, 1);
        assert!(lookup(&conn, "nope", now).expect("lookup").is_none());
    }
}
