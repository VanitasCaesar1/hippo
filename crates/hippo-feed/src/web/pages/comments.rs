//! `GET /comments`: the newest comments, 30 a page, each with the post it is
//! on, leaving out the posts the viewer has hidden.

use axum::extract::{RawQuery, State};
use axum::response::{IntoResponse, Response};
use maud::html;

use crate::comments;
use crate::error::AppError;
use crate::web::pages::comment::newest_rows;
use crate::web::pages::list::page_of;
use crate::web::rows;
use crate::web::{AppState, Ctx, Nav};

pub async fn comments(
    State(state): State<AppState>,
    ctx: Ctx,
    RawQuery(query): RawQuery,
) -> Result<Response, AppError> {
    let page = page_of(query.as_deref())?;
    let now = state.clock.now();
    let member = ctx.member.clone();
    let base = ctx.return_to.clone();
    let (list, more) = state
        .db
        .read(move |conn| {
            let found = comments::newest(conn, member.as_ref().map(|m| m.id), now, page)?;
            let more = found.more;
            let list = newest_rows(conn, member.as_ref(), found.rows, &base, now)?;
            Ok((list, more))
        })
        .await?;
    Ok(ctx
        .page("comments", "Comments")
        .hidden_h1()
        .current(Nav::Comments)
        .body(html! {
            @if list.is_empty() {
                p class="hp-notice" { "No comments yet." }
            } @else {
                (rows::comment_list(&list))
            }
            @if more {
                p { a class="hp-more" href=(format!("/comments?page={}", page + 1)) rel="next" { "More" } }
            }
        })
        .into_response())
}
