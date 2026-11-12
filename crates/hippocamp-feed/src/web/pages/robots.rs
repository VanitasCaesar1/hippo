//! `/robots.txt`: keeps crawlers off sign-in, forms, `/mod` and `/admin`.

use axum::http::{header, HeaderValue};
use axum::response::{IntoResponse, Response};

/// The paths crawlers are asked to skip. `/p/*/` covers a post's forms
/// (`/p/<id>/edit`, `/p/<id>/measure` and the rest) and not the post page.
pub const DISALLOWED: &[&str] = &[
    "/login",
    "/auth/",
    "/submit",
    "/report",
    "/settings",
    "/hide",
    "/hidden",
    "/u/",
    "/mod",
    "/admin",
    "/c/",
    "/p/*/",
];

pub async fn robots() -> Response {
    let mut text = String::from("User-agent: *\n");
    for path in DISALLOWED {
        text.push_str(&format!("Disallow: {path}\n"));
    }
    let mut response = text.into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response
}
