//! `GET /report` and `POST /report`: a member reports a post, comment,
//! measurement or member.
//!
//! The form is shown again, status 422, for a rule broken: an unknown reason,
//! a note that is too long, a duplicate report, or a report on the member's
//! own content. Over the limit the answer is 429, before anything else is
//! looked at. A success redirects to the reported item's page, which then
//! shows *reported* in place of the link.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use maud::{html, Markup};
use serde::Deserialize;

use crate::auth::session::{CsrfForm, RequireUser};
use crate::db::Member;
use crate::error::AppError;
use crate::limits::{self, Action};
use crate::policy;
use crate::reports::{self, ReportTarget, REASONS};
use crate::users;
use crate::web::{AppState, Ctx};

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Values {
    #[serde(default)]
    pub target_type: String,
    #[serde(default)]
    pub target_id: String,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Deserialize)]
pub struct Target {
    #[serde(default)]
    target_type: String,
    #[serde(default)]
    target_id: String,
}

fn target_of(kind: &str, id: &str) -> Result<ReportTarget, AppError> {
    let id: i64 = id.parse().map_err(|_| AppError::not_found())?;
    ReportTarget::parse(kind, id).ok_or_else(AppError::not_found)
}

fn form_markup(member: &Member, values: &Values, what: &str) -> Markup {
    html! {
        p { "You are reporting " (what) "." }
        form class="hp-form" method="post" action="/report" {
            input type="hidden" name="csrf" value=(member.csrf);
            input type="hidden" name="target_type" value=(values.target_type);
            input type="hidden" name="target_id" value=(values.target_id);
            div class="hp-field" {
                label class="hp-label" for="reason" { "Reason" }
                select class="hp-select" id="reason" name="reason" {
                    @for reason in REASONS {
                        option value=(reason) selected[values.reason == reason] { (reason) }
                    }
                }
            }
            div class="hp-field" {
                label class="hp-label" for="note" { "Note (optional, one line)" }
                input class="hp-input" id="note" name="note" type="text" value=(values.note)
                    maxlength=(policy::REPORT_NOTE_MAX) autocomplete="off";
            }
            p class="hp-help" {
                "Moderators read the report and your note. The public log never shows who reported."
            }
            button class="hp-button" type="submit" { "Report" }
        }
    }
}

/// A short line naming the target, for the form.
fn what(conn: &rusqlite::Connection, target: ReportTarget) -> Result<String, AppError> {
    let described = reports::describe(conn, target)?.ok_or_else(AppError::not_found)?;
    if matches!(described.state.as_deref(), Some(s) if s != "visible") {
        return Err(AppError::not_found());
    }
    let first: String = described
        .text
        .lines()
        .next()
        .unwrap_or_default()
        .chars()
        .take(120)
        .collect();
    Ok(match target {
        ReportTarget::Post(_) => format!("the post \u{201c}{first}\u{201d}"),
        ReportTarget::Comment(_) => format!("the comment \u{201c}{first}\u{201d}"),
        ReportTarget::Measurement(_) => "a measurement".to_string(),
        ReportTarget::User(_) => format!("the member {first}"),
    })
}

fn may_report(member: &Member, state: &AppState) -> Result<(), AppError> {
    if matches!(
        users::standing(member, state.clock.now()),
        users::Standing::Restricted | users::Standing::Banned
    ) {
        return Err(AppError::forbidden("A restricted account cannot report."));
    }
    Ok(())
}

/// `GET /report?target_type=…&target_id=…`.
pub async fn form(
    State(state): State<AppState>,
    ctx: Ctx,
    RequireUser(member): RequireUser,
    Query(query): Query<Target>,
) -> Result<Response, AppError> {
    may_report(&member, &state)?;
    let target = target_of(&query.target_type, &query.target_id)?;
    let what = state.db.read(move |conn| what(conn, target)).await?;
    let values = Values {
        target_type: target.type_str().to_string(),
        target_id: target.id().to_string(),
        reason: "spam".to_string(),
        note: String::new(),
    };
    Ok(ctx
        .page("report", "Report")
        .body(form_markup(&member, &values, &what))
        .into_response())
}

/// Where a report lands: the reported item's or member's page.
pub fn landing(
    conn: &rusqlite::Connection,
    target: ReportTarget,
    moderator: bool,
) -> rusqlite::Result<String> {
    Ok(match target {
        ReportTarget::Post(id) => format!("/p/{id}"),
        ReportTarget::Comment(id) => {
            let post: i64 =
                conn.query_row("SELECT post_id FROM comments WHERE id = ?1", [id], |r| {
                    r.get(0)
                })?;
            format!("/p/{post}#c{id}")
        }
        ReportTarget::Measurement(id) => {
            let post: Option<i64> = conn
                .query_row(
                    "SELECT p.id FROM measurements m JOIN posts p ON p.pack = m.pack
                     WHERE m.id = ?1",
                    [id],
                    |r| r.get(0),
                )
                .ok();
            match post {
                Some(post) => format!("/p/{post}#m{id}"),
                None => "/".to_string(),
            }
        }
        ReportTarget::User(id) if moderator => {
            let login: String =
                conn.query_row("SELECT login FROM users WHERE id = ?1", [id], |r| r.get(0))?;
            format!("/mod/u/{login}")
        }
        // Members have no public page of their own.
        ReportTarget::User(_) => "/".to_string(),
    })
}

/// `POST /report` with `target_type`, `target_id`, `reason`, `note` and
/// `csrf`.
pub async fn file(
    State(state): State<AppState>,
    ctx: Ctx,
    CsrfForm(values, member): CsrfForm<Values>,
) -> Result<Response, AppError> {
    let target = target_of(&values.target_type, &values.target_id)?;
    let now = state.clock.now();
    let moderator = users::is_moderator(&member, &state.config);
    let reporter = member.clone();
    let (reason, note) = (values.reason.clone(), values.note.clone());
    let result = state
        .db
        .write(move |tx| {
            limits::check(tx, reporter.id, Action::Report, now).map_err(|l| l.into_error(now))?;
            reports::file(tx, &reporter, target, &reason, &note, now)?;
            Ok(landing(tx, target, moderator)?)
        })
        .await;
    match result {
        Ok(to) => Ok(Redirect::to(&to).into_response()),
        Err(e) if e.status == StatusCode::UNPROCESSABLE_ENTITY => {
            let what = state
                .db
                .read(move |conn| what(conn, target))
                .await
                .unwrap_or_else(|_| "this item".to_string());
            let mut response = ctx
                .page("report", "Report")
                .body(html! {
                    p class="hp-error" { (e.message) }
                    (form_markup(&member, &values, &what))
                })
                .into_response();
            *response.status_mut() = StatusCode::UNPROCESSABLE_ENTITY;
            Ok(response)
        }
        Err(e) => Err(e),
    }
}
