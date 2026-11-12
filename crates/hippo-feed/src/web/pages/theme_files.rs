//! `GET /themes/<name>/<pack hash>.css`: a theme pack's stylesheet.

use axum::extract::{Path, State};
use axum::http::{header, HeaderValue};
use axum::response::{IntoResponse, Response};
use rusqlite::OptionalExtension;

use crate::error::AppError;
use crate::themes;
use crate::web::AppState;

/// The accepted stylesheet, while the feed serves that pack hash for the
/// pack; 404 otherwise (a hash that is not the served one, a pack that is
/// delisted or whose post is not visible, a name that is no theme pack).
pub async fn theme_file(
    State(state): State<AppState>,
    Path((name, file)): Path<(String, String)>,
) -> Result<Response, AppError> {
    let Some(hash) = file.strip_suffix(".css").map(str::to_owned) else {
        return Err(AppError::not_found());
    };
    let css: Option<String> = state
        .db
        .read(move |conn| {
            if themes::serves(conn, &name)?.as_deref() != Some(hash.as_str()) {
                return Ok(None);
            }
            Ok(conn
                .query_row(
                    "SELECT css FROM theme_files WHERE pack = ?1 AND pack_hash = ?2
                     AND css IS NOT NULL",
                    [&name, &hash],
                    |r| r.get(0),
                )
                .optional()?)
        })
        .await?;
    let css = css.ok_or_else(AppError::not_found)?;
    let mut response = css.into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/css; charset=utf-8"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    Ok(response)
}
