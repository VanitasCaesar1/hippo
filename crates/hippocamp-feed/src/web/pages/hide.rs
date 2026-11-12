//! `POST /hide` and `GET /hidden`: a member's own hiding of posts.

use axum::extract::{RawQuery, State};
use axum::response::{IntoResponse, Redirect, Response};
use maud::html;
use serde::Deserialize;

use crate::auth::session::{CsrfForm, RequireUser};
use crate::error::AppError;
use crate::posts;
use crate::rank;
use crate::web::pages::list::{page_of, rows_of};
use crate::web::rows::{self, Viewer};
use crate::web::{local_path, AppState, Ctx};

#[derive(Debug, Deserialize)]
pub struct HideForm {
    #[serde(default)]
    post_id: String,
    #[serde(default)]
    unhide: String,
    return_to: Option<String>,
}

/// `POST /hide` with `post_id`, `unhide` (`0` or `1`), `return_to` and
/// `csrf`. Hiding a post that is not visible is 404; un-hiding works on any
/// post the member hid. Redirects to `return_to` when it is a local path,
/// else to the post's page.
pub async fn hide(
    State(state): State<AppState>,
    CsrfForm(form, member): CsrfForm<HideForm>,
) -> Result<Response, AppError> {
    let id: i64 = form.post_id.parse().map_err(|_| AppError::not_found())?;
    let unhide = match form.unhide.as_str() {
        "" | "0" => false,
        "1" => true,
        _ => return Err(AppError::bad_request("unhide must be 0 or 1.")),
    };
    let now = state.clock.now();
    state
        .db
        .write(move |tx| {
            if unhide {
                posts::unhide(tx, member.id, id)?;
                Ok(())
            } else {
                posts::hide(tx, member.id, id, now)
            }
        })
        .await?;
    let to = match form.return_to.as_deref() {
        Some(r) if local_path(r) == r => r.to_string(),
        _ => format!("/p/{id}"),
    };
    Ok(Redirect::to(&to).into_response())
}

/// `GET /hidden`: "Hidden by you", the visible posts the member hid, most
/// recently hidden first, 30 a page, each with *un-hide*.
pub async fn hidden(
    State(state): State<AppState>,
    ctx: Ctx,
    RequireUser(member): RequireUser,
    RawQuery(query): RawQuery,
) -> Result<Response, AppError> {
    let page = page_of(query.as_deref())?;
    let now = state.clock.now();
    let return_to = ctx.return_to.clone();
    let who = member.clone();
    let (rows, more) = state
        .db
        .read(move |conn| {
            let listing = rank::hidden_by(conn, who.id, now, page)?;
            let mut rows = rows_of(
                conn,
                &state.config,
                Some(&who),
                &listing,
                page,
                now,
                &return_to,
            )?;
            for row in &mut rows {
                if let Viewer::Member { hidden, .. } = &mut row.viewer {
                    *hidden = true;
                }
            }
            Ok((rows, listing.more))
        })
        .await?;
    Ok(ctx
        .page("hidden", "Hidden by you")
        .body(html! {
            @if rows.is_empty() {
                p class="hp-notice" { "You have not hidden any posts." }
            } @else {
                (rows::list(&rows))
            }
            @if more {
                p { a class="hp-more" href=(format!("/hidden?page={}", page + 1)) rel="next" { "More" } }
            }
        })
        .into_response())
}
