//! `/submit`, `/p/<id>/edit` and `/p/<id>/delete`: a member's discussion
//! posts.
//!
//! A form that fails an input rule or a content rule is shown again, status
//! 422, with the values entered. Over a limit the answer is 429.

use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect, Response};
use maud::{html, Markup};
use rusqlite::OptionalExtension;
use serde::Deserialize;
use time::{Duration, OffsetDateTime};

use crate::auth::session::{CsrfForm, RequireUser};
use crate::clock;
use crate::db::Member;
use crate::error::{AppError, FormError};
use crate::limits::{self, Action};
use crate::markdown;
use crate::policy;
use crate::posts::{self, Discussion};
use crate::users;
use crate::web::{AppState, Ctx};

/// The values of the form, as entered.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Values {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub link: String,
    #[serde(default)]
    pub body: String,
}

/// The line beside every submit button that publishes text.
pub fn licence_line() -> Markup {
    html! {
        p class="hp-help" {
            "Your text is published under "
            a href="https://creativecommons.org/licenses/by/4.0/" rel="nofollow noopener noreferrer" { "CC BY 4.0" }
            "."
        }
    }
}

fn form_markup(member: &Member, action: &str, button: &str, values: &Values) -> Markup {
    html! {
        form class="hp-form" method="post" action=(action) {
            input type="hidden" name="csrf" value=(member.csrf);
            div class="hp-field" {
                label class="hp-label" for="title" { "Title" }
                input class="hp-input" id="title" name="title" type="text" value=(values.title)
                    maxlength="400" required;
            }
            div class="hp-field" {
                label class="hp-label" for="link" { "Link (optional)" }
                input class="hp-input" id="link" name="link" type="text" value=(values.link)
                    autocomplete="off";
            }
            div class="hp-field" {
                label class="hp-label" for="body" { "Text (optional, Markdown)" }
                textarea class="hp-textarea" id="body" name="body" rows="8" { (values.body) }
            }
            p class="hp-help" { "Give a link, a text, or both." }
            button class="hp-button" type="submit" { (button) }
            (licence_line())
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn page(
    ctx: &Ctx,
    member: &Member,
    name: &'static str,
    h1: &str,
    action: &str,
    button: &str,
    values: &Values,
    error: Option<&str>,
) -> Response {
    ctx.page(name, h1)
        .body(html! {
            @if let Some(message) = error {
                p class="hp-error" { (message) }
            }
            (form_markup(member, action, button, values))
        })
        .into_response()
}

#[allow(clippy::too_many_arguments)]
fn refused(
    ctx: &Ctx,
    member: &Member,
    name: &'static str,
    h1: &str,
    action: &str,
    button: &str,
    values: &Values,
    error: &FormError,
) -> Response {
    let message = format!("{}: {}", error.field, error.rule);
    let mut response = page(
        ctx,
        member,
        name,
        h1,
        action,
        button,
        values,
        Some(&message),
    );
    *response.status_mut() = axum::http::StatusCode::UNPROCESSABLE_ENTITY;
    response
}

/// The input rules, then the content rules, for a discussion's three texts.
fn check(member: &Member, values: &Values, now: OffsetDateTime) -> Result<Discussion, FormError> {
    let discussion = match Discussion::parse(&values.title, &values.link, &values.body) {
        Ok(d) => d,
        Err(e) => return Err(e.form.unwrap_or_else(|| FormError::new("form", e.message))),
    };
    let standing = users::standing(member, now);
    let (_, title_links) = markdown::render(&discussion.title);
    let (_, body_links) = markdown::render(&discussion.body);
    limits::content_rule(
        &discussion.title,
        title_links,
        discussion.link.as_deref(),
        standing,
    )
    .map_err(|r| r.form_error("title"))?;
    limits::content_rule(&discussion.body, body_links, None, standing)
        .map_err(|r| r.form_error("body"))?;
    Ok(discussion)
}

/// `GET /submit`.
pub async fn form(ctx: Ctx, RequireUser(member): RequireUser) -> Response {
    page(
        &ctx,
        &member,
        "submit",
        "Submit",
        "/submit",
        "Submit",
        &Values::default(),
        None,
    )
}

/// `POST /submit`.
pub async fn create(
    State(state): State<AppState>,
    ctx: Ctx,
    CsrfForm(values, member): CsrfForm<Values>,
) -> Result<Response, AppError> {
    let now = state.clock.now();
    let discussion = match check(&member, &values, now) {
        Ok(d) => d,
        Err(e) => {
            return Ok(refused(
                &ctx, &member, "submit", "Submit", "/submit", "Submit", &values, &e,
            ))
        }
    };
    let author = member.clone();
    let id = state
        .db
        .write(move |tx| {
            limits::check(tx, author.id, Action::Post, now).map_err(|l| l.into_error(now))?;
            posts::create_discussion(tx, &author, &discussion, now)
        })
        .await?;
    Ok(Redirect::to(&format!("/p/{id}")).into_response())
}

/// What the edit page needs of a post.
struct Loaded {
    values: Values,
}

fn parse_id(id: &str) -> Result<i64, AppError> {
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
        return Err(AppError::not_found());
    }
    id.parse().map_err(|_| AppError::not_found())
}

type PostFields = (
    String,
    Option<i64>,
    String,
    String,
    Option<String>,
    String,
    String,
);

/// Whether `member` may edit post `id` now, with its current values.
fn editable(
    conn: &rusqlite::Connection,
    member: &Member,
    id: i64,
    now: OffsetDateTime,
) -> Result<Loaded, AppError> {
    let found: Option<PostFields> = conn
        .query_row(
            "SELECT kind, author_id, state, created_at, link, title, body FROM posts WHERE id = ?1",
            [id],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                ))
            },
        )
        .optional()?;
    let (kind, author, state, created, link, title, body) =
        found.ok_or_else(AppError::not_found)?;
    if kind != "discussion" || author != Some(member.id.0) {
        return Err(AppError::forbidden("Only the author can edit this post."));
    }
    if state != "visible" {
        return Err(AppError::forbidden(
            "A post that is not visible cannot be edited.",
        ));
    }
    let created = clock::parse_storage(&created).unwrap_or(OffsetDateTime::UNIX_EPOCH);
    if now - created > Duration::hours(policy::EDIT_WINDOW_HOURS) {
        return Err(AppError::forbidden(format!(
            "A post can be edited for {} hours after it is created.",
            policy::EDIT_WINDOW_HOURS
        )));
    }
    Ok(Loaded {
        values: Values {
            title,
            link: link.unwrap_or_default(),
            body,
        },
    })
}

/// `GET /p/<id>/edit`.
pub async fn edit_form(
    State(state): State<AppState>,
    ctx: Ctx,
    RequireUser(member): RequireUser,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let id = parse_id(&id)?;
    let now = state.clock.now();
    let who = member.clone();
    let loaded = state
        .db
        .read(move |conn| editable(conn, &who, id, now))
        .await?;
    Ok(page(
        &ctx,
        &member,
        "edit",
        "Edit",
        &format!("/p/{id}/edit"),
        "Save",
        &loaded.values,
        None,
    ))
}

/// `POST /p/<id>/edit`.
pub async fn edit(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<String>,
    CsrfForm(values, member): CsrfForm<Values>,
) -> Result<Response, AppError> {
    let id = parse_id(&id)?;
    let now = state.clock.now();
    let action = format!("/p/{id}/edit");
    let discussion = match check(&member, &values, now) {
        Ok(d) => d,
        Err(e) => {
            return Ok(refused(
                &ctx, &member, "edit", "Edit", &action, "Save", &values, &e,
            ))
        }
    };
    let author = member.clone();
    state
        .db
        .write(move |tx| posts::edit_discussion(tx, &author, id, &discussion, now))
        .await?;
    Ok(Redirect::to(&format!("/p/{id}")).into_response())
}

#[derive(Debug, Deserialize)]
pub struct Nothing {}

/// `POST /p/<id>/delete`.
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
    CsrfForm(_, member): CsrfForm<Nothing>,
) -> Result<Response, AppError> {
    let id = parse_id(&id)?;
    state
        .db
        .write(move |tx| posts::delete_post(tx, &member, id))
        .await?;
    Ok(Redirect::to(&format!("/p/{id}")).into_response())
}
