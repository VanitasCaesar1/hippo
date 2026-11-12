//! `GET /moderation`: the public log, newest first, 50 a page. It shows the
//! date, action, target, reason and actor of each public row, and never a
//! reporter, a report note or a snapshot.

use axum::extract::{RawQuery, State};
use axum::response::{IntoResponse, Response};
use maud::{html, Markup};

use crate::clock;
use crate::error::AppError;
use crate::moderation::{self, LogActor, LogRow, LogTarget};
use crate::web::{AppState, Ctx};

/// `page` from a query: absent is 1; anything but a positive number is 404.
pub fn page_number(query: Option<&str>) -> Result<usize, AppError> {
    let Some(raw) = query else { return Ok(1) };
    let value = url::form_urlencoded::parse(raw.as_bytes()).find(|(k, _)| k == "page");
    let Some((_, value)) = value else {
        return Ok(1);
    };
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(AppError::not_found());
    }
    match value.parse::<usize>() {
        Ok(n) if n >= 1 => Ok(n),
        _ => Err(AppError::not_found()),
    }
}

/// A log target as a link, or `[deleted]`.
pub fn target_cell(target: &LogTarget) -> Markup {
    html! {
        @match target {
            LogTarget::None => { "-" }
            LogTarget::Deleted => { span class="hp-placeholder" { "[deleted]" } }
            LogTarget::Post { post_id } => { a href=(format!("/p/{post_id}")) { "post " (post_id) } }
            LogTarget::Comment { post_id, comment_id } => {
                a href=(format!("/p/{post_id}#c{comment_id}")) { "comment " (comment_id) }
            }
            LogTarget::Measurement { post_id: Some(post_id), measurement_id } => {
                a href=(format!("/p/{post_id}#m{measurement_id}")) { "measurement " (measurement_id) }
            }
            LogTarget::Measurement { post_id: None, measurement_id } => { "measurement " (measurement_id) }
            LogTarget::Member { login } => { "member " (login) }
            LogTarget::Pack { name, post_id: Some(post_id) } => {
                a href=(format!("/p/{post_id}")) { "pack " (name) }
            }
            LogTarget::Pack { name, post_id: None } => { "pack " (name) }
        }
    }
}

pub fn actor_cell(actor: &LogActor) -> Markup {
    html! {
        @match actor {
            LogActor::System => { "system" }
            LogActor::Member(login) => { (login) }
            LogActor::Deleted => { span class="hp-placeholder" { "[deleted]" } }
        }
    }
}

fn table(rows: &[LogRow]) -> Markup {
    html! {
        table class="hp-table" {
            thead {
                tr {
                    th scope="col" { "Date" }
                    th scope="col" { "Action" }
                    th scope="col" { "Target" }
                    th scope="col" { "Reason" }
                    th scope="col" { "Actor" }
                }
            }
            tbody {
                @for row in rows {
                    tr {
                        td { (clock::date_text(row.at)) }
                        td { (row.action) }
                        td { (target_cell(&row.target)) }
                        td { (row.reason) }
                        td { (actor_cell(&row.actor)) }
                    }
                }
            }
        }
    }
}

pub async fn log(
    State(state): State<AppState>,
    ctx: Ctx,
    RawQuery(query): RawQuery,
) -> Result<Response, AppError> {
    let page = page_number(query.as_deref())?;
    let found = state
        .db
        .read(move |conn| Ok(moderation::public_log(conn, page)?))
        .await?;
    Ok(ctx
        .page("moderation", "Moderation log")
        .body(html! {
            p class="hp-help" {
                "Every action that moderators and admins take on content and accounts, with the reason."
                " Reports and who filed them are not shown."
            }
            @if found.rows.is_empty() {
                p class="hp-notice" { "Nothing has been logged yet." }
            } @else {
                (table(&found.rows))
            }
            @if found.more {
                p { a class="hp-more" href=(format!("/moderation?page={}", page + 1)) rel="next" { "More" } }
            }
        })
        .into_response())
}
