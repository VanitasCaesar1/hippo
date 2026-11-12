//! `POST /vote`: upvote an item, or withdraw the vote.

use axum::extract::State;
use axum::response::{IntoResponse, Redirect, Response};
use rusqlite::OptionalExtension;
use serde::Deserialize;

use crate::auth::session::CsrfForm;
use crate::error::AppError;
use crate::limits::{self, Action};
use crate::votes::{self, Target};
use crate::web::{local_path, AppState};

#[derive(Debug, Deserialize)]
pub struct VoteForm {
    #[serde(default)]
    target_type: String,
    #[serde(default)]
    target_id: String,
    #[serde(default)]
    withdraw: String,
    return_to: Option<String>,
}

/// The page of a target, for a vote that came without a usable `return_to`.
fn target_page(conn: &rusqlite::Connection, target: Target) -> rusqlite::Result<String> {
    Ok(match target {
        Target::Post(id) => format!("/p/{id}"),
        Target::Comment(id) => {
            let post: Option<i64> = conn
                .query_row("SELECT post_id FROM comments WHERE id = ?1", [id], |r| {
                    r.get(0)
                })
                .optional()?;
            match post {
                Some(post) => format!("/p/{post}#c{id}"),
                None => "/".to_string(),
            }
        }
    })
}

/// `POST /vote` with `target_type`, `target_id`, `withdraw` (`0` or `1`),
/// `return_to` and `csrf`. Redirects to `return_to` when it is a local path,
/// else to the target's page.
pub async fn vote(
    State(state): State<AppState>,
    CsrfForm(form, member): CsrfForm<VoteForm>,
) -> Result<Response, AppError> {
    let id: i64 = form
        .target_id
        .parse()
        .map_err(|_| AppError::bad_request("The vote has no valid target."))?;
    let target = Target::parse(&form.target_type, id)
        .ok_or_else(|| AppError::bad_request("The vote has no valid target."))?;
    let withdraw = match form.withdraw.as_str() {
        "" | "0" => false,
        "1" => true,
        _ => return Err(AppError::bad_request("withdraw must be 0 or 1.")),
    };
    let now = state.clock.now();
    let to = state
        .db
        .write(move |tx| {
            if withdraw {
                votes::withdraw(tx, member.id, target)?;
            } else {
                limits::check(tx, member.id, Action::Vote, now).map_err(|l| l.into_error(now))?;
                votes::vote(tx, &member, target, now)?;
            }
            Ok(match form.return_to.as_deref() {
                Some(r) if local_path(r) == r => r.to_string(),
                _ => target_page(tx, target)?,
            })
        })
        .await?;
    Ok(Redirect::to(&to).into_response())
}
