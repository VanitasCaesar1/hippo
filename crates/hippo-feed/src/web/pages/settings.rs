//! `/settings`: theme and personal CSS, export, deletion; and the personal
//! stylesheet at `/u/<id>/theme.css`.

use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use maud::{html, Markup};
use serde::Deserialize;

use crate::auth::session::{
    append_cookie, is_https, session_clear_cookie, CsrfForm, MaybeUser, RequireUser,
};
use crate::db::Member;
use crate::error::AppError;
use crate::policy;
use crate::themes;
use crate::users;
use crate::web::assets::sha256_hex;
use crate::web::{local_path, AppState, Ctx};

const WARNING: &str =
    "Personal CSS changes how this site looks to you. Paste only CSS you understand.";

/// What the settings page shows besides the member: the theme packs on offer
/// and the values to put in the form.
struct View {
    packs: Vec<String>,
    theme: String,
    css: String,
    error: Option<String>,
}

/// Normalises line endings and trims, as every text input is.
fn normalise(text: &str) -> String {
    text.replace("\r\n", "\n").trim().to_string()
}

/// The message for personal CSS that breaks a rule.
fn css_error(css: &str) -> Option<String> {
    if css.len() > policy::CSS_MAX_BYTES {
        return Some(format!(
            "Personal CSS is {} bytes; the limit is 65,536.",
            css.len()
        ));
    }
    hippo_theme::check_personal_css(css)
        .err()
        .map(|e| format!("Personal CSS, line {}: {}", e.line, e.what))
}

fn theme_label(value: &str) -> String {
    match value {
        "auto" => "auto (follows your device)".to_string(),
        "default" => "default".to_string(),
        "dark" => "dark".to_string(),
        other => other
            .strip_prefix("pack:")
            .map_or_else(|| other.to_string(), |n| format!("{n} (theme pack)")),
    }
}

fn form_markup(member: &Member, view: &View) -> Markup {
    let mut options = vec![
        "auto".to_string(),
        "default".to_string(),
        "dark".to_string(),
    ];
    options.extend(view.packs.iter().map(|n| format!("pack:{n}")));
    // The member's current pack stays selectable while the feed cannot serve it.
    if member.theme.starts_with("pack:") && !options.contains(&member.theme) {
        options.push(member.theme.clone());
    }
    html! {
        form class="hp-form" method="post" action="/settings/theme" {
            input type="hidden" name="csrf" value=(member.csrf);
            div class="hp-field" {
                label class="hp-label" for="theme" { "Theme" }
                select class="hp-select" id="theme" name="theme" {
                    @for value in &options {
                        option value=(value) selected[*value == view.theme] { (theme_label(value)) }
                    }
                }
            }
            div class="hp-field" {
                label class="hp-label" for="personal_css" { "Personal CSS" }
                p class="hp-help" { (WARNING) }
                textarea class="hp-textarea" id="personal_css" name="personal_css" rows="10" { (view.css) }
            }
            button class="hp-button" type="submit" { "Save" }
        }
    }
}

fn settings_page(ctx: &Ctx, member: &Member, view: &View) -> Response {
    ctx.page("settings", "Settings")
        .body(html! {
            @if let Some(message) = &view.error {
                p class="hp-error" { (message) }
            }
            section {
                h2 { "Look" }
                (form_markup(member, view))
            }
            section {
                h2 { "Your posts" }
                p { a href="/hidden" { "Posts you have hidden" } }
            }
            section {
                h2 { "Your data" }
                p { a href="/settings/export" { "Export your data as JSON" } }
            }
            section {
                h2 { "Delete your account" }
                p class="hp-help" {
                    "This deletes your sessions, votes, measurements and hides, resets your theme, "
                    "removes your personal CSS, and replaces your posts and comments with [deleted]. "
                    "It cannot be undone."
                }
                form class="hp-form" method="post" action="/settings/delete" {
                    input type="hidden" name="csrf" value=(member.csrf);
                    div class="hp-field" {
                        label class="hp-label" for="confirm" { "Type your login (" (member.login) ") to confirm" }
                        input class="hp-input" id="confirm" name="confirm" type="text" autocomplete="off";
                    }
                    button class="hp-button" type="submit" { "Delete my account" }
                }
            }
        })
        .into_response()
}

async fn load_css(state: &AppState, member: &Member) -> Result<(Vec<String>, String), AppError> {
    let id = member.id.0;
    state
        .db
        .read(move |c| {
            let css = c.query_row("SELECT personal_css FROM users WHERE id = ?1", [id], |r| {
                r.get::<_, String>(0)
            })?;
            Ok((themes::served(c)?, css))
        })
        .await
}

/// `GET /settings`.
pub async fn settings(
    State(state): State<AppState>,
    ctx: Ctx,
    RequireUser(member): RequireUser,
) -> Result<Response, AppError> {
    let (packs, css) = load_css(&state, &member).await?;
    let view = View {
        packs,
        theme: member.theme.clone(),
        css,
        error: None,
    };
    Ok(settings_page(&ctx, &member, &view))
}

/// A refusal: the settings page again, status 422, with the entered values.
async fn refuse(
    state: &AppState,
    ctx: &Ctx,
    member: &Member,
    theme: String,
    css: String,
    message: String,
) -> Result<Response, AppError> {
    let (packs, _) = load_css(state, member).await?;
    let view = View {
        packs,
        theme,
        css,
        error: Some(message),
    };
    let mut response = settings_page(ctx, member, &view);
    *response.status_mut() = StatusCode::UNPROCESSABLE_ENTITY;
    Ok(response)
}

#[derive(Deserialize)]
pub struct ThemeForm {
    theme: String,
    personal_css: Option<String>,
    return_to: Option<String>,
}

/// `POST /settings/theme`. The settings form sends `theme` and
/// `personal_css`; a *use* button sends `theme` and `return_to` and leaves
/// the personal CSS as it is.
pub async fn save_theme(
    State(state): State<AppState>,
    ctx: Ctx,
    CsrfForm(form, member): CsrfForm<ThemeForm>,
) -> Result<Response, AppError> {
    let css = form.personal_css.as_deref().map(normalise);
    let shown_css = match &css {
        Some(c) => c.clone(),
        None => load_css(&state, &member).await?.1,
    };
    let theme = form.theme.clone();

    let valid_shape = matches!(theme.as_str(), "auto" | "default" | "dark")
        || theme.strip_prefix("pack:").is_some_and(|n| !n.is_empty());
    if !valid_shape {
        return refuse(
            &state,
            &ctx,
            &member,
            member.theme.clone(),
            shown_css,
            "That theme does not exist.".into(),
        )
        .await;
    }
    if let Some(name) = theme.strip_prefix("pack:") {
        let name = name.to_string();
        let serving = state
            .db
            .read(move |c| Ok(themes::serves(c, &name)?))
            .await?
            .is_some();
        if !serving && theme != member.theme {
            return refuse(
                &state,
                &ctx,
                &member,
                theme,
                shown_css,
                "That theme is not available.".into(),
            )
            .await;
        }
    }
    if let Some(message) = css.as_deref().and_then(css_error) {
        return refuse(&state, &ctx, &member, theme, shown_css, message).await;
    }

    let id = member.id.0;
    state
        .db
        .write(move |tx| {
            match &css {
                Some(css) => tx.execute(
                    "UPDATE users SET theme = ?1, personal_css = ?2 WHERE id = ?3",
                    rusqlite::params![theme, css, id],
                )?,
                None => tx.execute(
                    "UPDATE users SET theme = ?1 WHERE id = ?2",
                    rusqlite::params![theme, id],
                )?,
            };
            Ok(())
        })
        .await?;
    let to = match form.return_to.as_deref() {
        Some(r) if local_path(r) == r => r.to_string(),
        _ => "/settings".to_string(),
    };
    Ok(Redirect::to(&to).into_response())
}

/// `GET /settings/export`: the member's data as a JSON attachment.
pub async fn export(
    State(state): State<AppState>,
    RequireUser(member): RequireUser,
) -> Result<Response, AppError> {
    let id = member.id;
    let now = state.clock.now();
    let (login, value) = state
        .db
        .read(move |c| Ok((users::login_of(c, id)?, users::export(c, id, now)?)))
        .await?;
    let body = serde_json::to_string_pretty(&value).map_err(AppError::internal)?;
    let mut response = body.into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    if let Ok(v) =
        HeaderValue::from_str(&format!("attachment; filename=\"hippo-feed-{login}.json\""))
    {
        headers.insert(header::CONTENT_DISPOSITION, v);
    }
    Ok(response)
}

#[derive(Deserialize)]
pub struct DeleteForm {
    confirm: Option<String>,
}

/// `POST /settings/delete`: deletes the account after the member types their
/// login.
pub async fn delete(
    State(state): State<AppState>,
    ctx: Ctx,
    CsrfForm(form, member): CsrfForm<DeleteForm>,
) -> Result<Response, AppError> {
    if form.confirm.as_deref().map(str::trim) != Some(member.login.as_str()) {
        let (_, css) = load_css(&state, &member).await?;
        return refuse(
            &state,
            &ctx,
            &member,
            member.theme.clone(),
            css,
            "Type your login exactly to delete your account.".into(),
        )
        .await;
    }
    let id = member.id;
    let now = state.clock.now();
    state
        .db
        .write(move |tx| users::delete_account(tx, id, Some(id), now))
        .await?;
    let mut response = Redirect::to("/").into_response();
    append_cookie(&mut response, &session_clear_cookie(is_https(&state)));
    Ok(response)
}

fn etag_matches(headers: &HeaderMap, etag: &str) -> bool {
    headers
        .get_all(header::IF_NONE_MATCH)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(str::trim)
        .any(|v| v == "*" || v.trim_start_matches("W/") == etag)
}

/// `GET /u/<id>/theme.css`: the member's personal CSS, to that member only.
pub async fn personal_css(
    State(state): State<AppState>,
    MaybeUser(member): MaybeUser,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let member = match (member, id.parse::<i64>()) {
        (Some(m), Ok(n)) if m.id.0 == n && m.has_personal_css => m,
        _ => return Err(AppError::not_found()),
    };
    let user = member.id.0;
    let css: String = state
        .db
        .read(move |c| {
            Ok(c.query_row(
                "SELECT personal_css FROM users WHERE id = ?1",
                [user],
                |r| r.get(0),
            )?)
        })
        .await?;
    if css.is_empty() {
        return Err(AppError::not_found());
    }
    let etag = format!("\"{}\"", sha256_hex(css.as_bytes()));
    let status = if etag_matches(&headers, &etag) {
        StatusCode::NOT_MODIFIED
    } else {
        StatusCode::OK
    };
    let mut response = if status == StatusCode::OK {
        css.into_response()
    } else {
        status.into_response()
    };
    let h = response.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/css; charset=utf-8"),
    );
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-cache"),
    );
    if let Ok(v) = HeaderValue::from_str(&etag) {
        h.insert(header::ETAG, v);
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn css_is_normalised_and_checked() {
        assert_eq!(normalise("  a{}\r\nb{}  \r\n"), "a{}\nb{}");
        assert_eq!(css_error("a{}"), None);
        assert_eq!(
            css_error(&"a".repeat(65_537)).as_deref(),
            Some("Personal CSS is 65537 bytes; the limit is 65,536.")
        );
        assert_eq!(
            css_error("a{}\nb\u{0}").as_deref(),
            Some("Personal CSS, line 2: control character U+0000 is not allowed")
        );
        assert!(css_error("a\u{7f}").is_some());
        assert!(css_error("a\u{85}").is_some());
    }

    #[test]
    fn if_none_match_forms() {
        let mut h = HeaderMap::new();
        h.insert(
            header::IF_NONE_MATCH,
            HeaderValue::from_static("\"x\", W/\"abc\""),
        );
        assert!(etag_matches(&h, "\"abc\""));
        assert!(!etag_matches(&h, "\"y\""));
    }
}
