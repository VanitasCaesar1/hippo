//! Comments on pages: the thread under a post, the comment form,
//! `POST /p/<id>/comment`, `GET /c/<id>/reply`, `GET`/`POST /c/<id>/edit` and
//! `POST /c/<id>/delete`.
//!
//! A form that fails a rule is shown again, status 422, with the text
//! entered. A top-level comment and a reply share the "Reply" page for that
//! second look. Every success redirects to the post at the comment's anchor.

use std::collections::HashSet;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use maud::{html, Markup, PreEscaped};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use time::{Duration, OffsetDateTime};

use crate::auth::session::{CsrfForm, RequireUser};
use crate::clock;
use crate::comments::{self, CommentNode, NewestComment, Shown};
use crate::config::Config;
use crate::db::Member;
use crate::error::AppError;
use crate::markdown;
use crate::policy;
use crate::users::{self, Standing};
use crate::web::pages::submit::licence_line;
use crate::web::rows::{Author, CommentBody, CommentRow, ReportLink, Viewer};
use crate::web::{AppState, Ctx};

/// The comment form's values.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Values {
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub parent: String,
}

pub fn parse_id(id: &str) -> Result<i64, AppError> {
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
        return Err(AppError::not_found());
    }
    id.parse().map_err(|_| AppError::not_found())
}

/// The ids in `ids` that `member` has voted on, as comments.
fn voted_ids(conn: &Connection, member: &Member, ids: &[i64]) -> rusqlite::Result<HashSet<i64>> {
    let json = serde_json::to_string(ids).unwrap_or_else(|_| "[]".into());
    let mut stmt = conn.prepare(
        "SELECT target_id FROM votes WHERE user_id = ?1 AND target_type = 'comment'
           AND target_id IN (SELECT value FROM json_each(?2))",
    )?;
    let rows = stmt.query_map(params![member.id.0, json], |r| r.get(0))?;
    rows.collect()
}

/// The ids in `ids` of kind `kind` that `member` has reported.
pub fn reported_ids(
    conn: &Connection,
    member: &Member,
    kind: &str,
    ids: &[i64],
) -> rusqlite::Result<HashSet<i64>> {
    let json = serde_json::to_string(ids).unwrap_or_else(|_| "[]".into());
    let mut stmt = conn.prepare(
        "SELECT target_id FROM reports WHERE reporter_id = ?1 AND target_type = ?2
           AND target_id IN (SELECT value FROM json_each(?3))",
    )?;
    let rows = stmt.query_map(params![member.id.0, kind, json], |r| r.get(0))?;
    rows.collect()
}

/// A comment as the row builder needs it.
struct Facts {
    id: i64,
    post_id: i64,
    parent_id: Option<i64>,
    depth: u32,
    author_id: Option<i64>,
    author_login: Option<String>,
    state: String,
    body: String,
    removed_reason: Option<String>,
    created_at: OffsetDateTime,
    edited_at: Option<OffsetDateTime>,
    on: Option<String>,
}

impl From<CommentNode> for Facts {
    fn from(n: CommentNode) -> Self {
        Facts {
            id: n.id,
            post_id: 0,
            parent_id: n.parent_id,
            depth: n.depth,
            author_id: n.author_id,
            author_login: n.author_login,
            state: n.state,
            body: n.body,
            removed_reason: n.removed_reason,
            created_at: n.created_at,
            edited_at: n.edited_at,
            on: None,
        }
    }
}

/// What every row of one page shares.
struct Env<'a> {
    member: Option<&'a Member>,
    moderator: bool,
    restricted: bool,
    now: OffsetDateTime,
    /// The page's path and query, to which a comment's anchor is added.
    base: &'a str,
    /// Whether comments may be added under the post(s) shown.
    open: bool,
    voted: HashSet<i64>,
    reported: HashSet<i64>,
}

fn row_of(facts: Facts, env: &Env) -> CommentRow {
    let visible = facts.state == "visible";
    let own = env
        .member
        .zip(facts.author_id)
        .is_some_and(|(m, a)| m.id.0 == a);
    let shown_body = CommentNode {
        id: facts.id,
        parent_id: facts.parent_id,
        depth: facts.depth,
        author_id: facts.author_id,
        author_login: None,
        state: facts.state.clone(),
        body: facts.body.clone(),
        removed_reason: facts.removed_reason.clone(),
        created_at: facts.created_at,
        edited_at: facts.edited_at,
        points: 0,
    };
    let body = match shown_body.shown(env.moderator) {
        Shown::Text(text) if visible => CommentBody::Text(markdown::render(text).0),
        Shown::Text(text) => CommentBody::Marked {
            mark: if facts.state == "hidden" {
                "[hidden]".to_string()
            } else {
                format!(
                    "[removed: {}]",
                    facts.removed_reason.as_deref().unwrap_or_default()
                )
            },
            html: markdown::render(text).0,
        },
        Shown::Placeholder(text) => CommentBody::Placeholder(text),
    };
    let in_window = env.now - facts.created_at <= Duration::hours(policy::EDIT_WINDOW_HOURS);
    let report = match env.member {
        Some(_) if !visible || own || env.restricted => ReportLink::None,
        Some(_) if env.reported.contains(&facts.id) => ReportLink::Done,
        Some(_) => ReportLink::Link,
        None => ReportLink::None,
    };
    let anchor = format!("{}#c{}", env.base, facts.id);
    let viewer = match env.member {
        Some(m) => Viewer::Member {
            may_vote: visible && !own && !env.restricted,
            voted: env.voted.contains(&facts.id),
            hidden: false,
            csrf: m.csrf.clone(),
            return_to: anchor,
        },
        None => Viewer::SignedOut { return_to: anchor },
    };
    CommentRow {
        id: facts.id,
        post_id: facts.post_id,
        depth: facts.depth,
        author: facts.author_login.map_or(Author::Deleted, Author::Name),
        age: clock::age_text(facts.created_at, env.now),
        edited: facts.edited_at.is_some(),
        parent: facts.parent_id,
        body,
        on: facts.on,
        reply: visible && env.open && facts.depth < policy::COMMENT_DEPTH_MAX,
        edit: own && visible && in_window && !env.restricted,
        delete: own && facts.state != "deleted",
        report,
        viewer,
    }
}

fn restricted(member: Option<&Member>, now: OffsetDateTime) -> bool {
    member.is_some_and(|m| {
        matches!(
            users::standing(m, now),
            Standing::Restricted | Standing::Banned
        )
    })
}

/// The comments of a post as rows, in thread order. `open` says whether the
/// post takes comments now (visible and not locked); `base` is the page's
/// path and query.
pub fn thread_rows(
    conn: &Connection,
    config: &Config,
    member: Option<&Member>,
    post_id: i64,
    open: bool,
    base: &str,
    now: OffsetDateTime,
) -> rusqlite::Result<Vec<CommentRow>> {
    let nodes = comments::thread(conn, post_id, now)?;
    let ids: Vec<i64> = nodes.iter().map(|n| n.id).collect();
    let (voted, reported) = match member {
        Some(m) => (
            voted_ids(conn, m, &ids)?,
            reported_ids(conn, m, "comment", &ids)?,
        ),
        None => Default::default(),
    };
    let env = Env {
        member,
        moderator: member.is_some_and(|m| users::is_moderator(m, config)),
        restricted: restricted(member, now),
        now,
        base,
        open,
        voted,
        reported,
    };
    Ok(nodes
        .into_iter()
        .map(|n| {
            let mut facts = Facts::from(n);
            facts.post_id = post_id;
            row_of(facts, &env)
        })
        .collect())
}

/// The rows of `/comments`.
pub fn newest_rows(
    conn: &Connection,
    member: Option<&Member>,
    list: Vec<NewestComment>,
    base: &str,
    now: OffsetDateTime,
) -> rusqlite::Result<Vec<CommentRow>> {
    let ids: Vec<i64> = list.iter().map(|c| c.id).collect();
    let (voted, reported) = match member {
        Some(m) => (
            voted_ids(conn, m, &ids)?,
            reported_ids(conn, m, "comment", &ids)?,
        ),
        None => Default::default(),
    };
    let mut depths = std::collections::HashMap::new();
    for id in &ids {
        let depth: u32 = conn.query_row("SELECT depth FROM comments WHERE id = ?1", [id], |r| {
            r.get(0)
        })?;
        depths.insert(*id, depth);
    }
    let env = Env {
        member,
        moderator: false,
        restricted: restricted(member, now),
        now,
        base,
        open: true,
        voted,
        reported,
    };
    Ok(list
        .into_iter()
        .map(|c| {
            let facts = Facts {
                id: c.id,
                post_id: c.post_id,
                parent_id: c.parent_id,
                depth: depths.get(&c.id).copied().unwrap_or(0),
                author_id: c.author_id,
                author_login: c.author_login,
                state: "visible".to_string(),
                body: c.body,
                removed_reason: None,
                created_at: c.created_at,
                edited_at: c.edited_at,
                on: Some(c.post_title),
            };
            row_of(facts, &env)
        })
        .collect())
}

/// The comment form: for a top-level comment under a post, for a reply (with
/// `parent`), or for an edit (`action` is the edit route and `parent` is
/// `None`).
pub fn form_markup(
    member: &Member,
    action: &str,
    parent: Option<i64>,
    button: &str,
    body: &str,
) -> Markup {
    html! {
        form class="hp-form" method="post" action=(action) {
            input type="hidden" name="csrf" value=(member.csrf);
            @if let Some(parent) = parent {
                input type="hidden" name="parent" value=(parent);
            }
            div class="hp-field" {
                label class="hp-label" for="body" { "Comment (Markdown)" }
                textarea class="hp-textarea" id="body" name="body" rows="6" required { (body) }
            }
            button class="hp-button" type="submit" { (button) }
            (licence_line())
        }
    }
}

/// What sits under a post's comments: the form for a member who may comment,
/// a notice for one who may not, or a sign-in link.
pub fn comment_box(
    member: Option<&Member>,
    post_id: i64,
    visible: bool,
    locked: bool,
    return_to: &str,
    now: OffsetDateTime,
) -> Markup {
    html! {
        @if !visible {
        } @else if locked {
            p class="hp-notice" { "This post is locked: it takes no new comments." }
        } @else if let Some(member) = member {
            @if restricted(Some(member), now) {
                p class="hp-notice" { "A restricted account cannot comment." }
            } @else {
                (form_markup(member, &format!("/p/{post_id}/comment"), None, "Add comment", ""))
            }
        } @else {
            p class="hp-help" {
                a href=(crate::web::layout::login_href(return_to)) rel="nofollow" { "Sign in to comment" }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn error_page(
    ctx: &Ctx,
    member: &Member,
    name: &'static str,
    h1: &str,
    action: &str,
    parent: Option<i64>,
    button: &str,
    body: &str,
    message: &str,
) -> Response {
    let mut response = ctx
        .page(name, h1)
        .body(html! {
            p class="hp-error" { (message) }
            (form_markup(member, action, parent, button, body))
        })
        .into_response();
    *response.status_mut() = StatusCode::UNPROCESSABLE_ENTITY;
    response
}

fn quote(author: &str, html_text: &str) -> Markup {
    html! {
        p class="hp-help" { "Replying to " (author) ":" }
        div class="hp-comment-body hp-md" { (PreEscaped(html_text)) }
    }
}

/// What the reply page needs of the comment replied to.
struct Parent {
    post_id: i64,
    author: String,
    html: String,
}

type ParentFields = (i64, Option<String>, String, String, u32, String, bool);

fn parent_of(conn: &Connection, id: i64) -> Result<Parent, AppError> {
    let row: Option<ParentFields> = conn
        .query_row(
            "SELECT c.post_id, u.login, c.body, c.state, c.depth, p.state, p.locked
             FROM comments c JOIN posts p ON p.id = c.post_id
             LEFT JOIN users u ON u.id = c.author_id WHERE c.id = ?1",
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
    let (post_id, login, body, state, depth, post_state, locked) =
        row.ok_or_else(AppError::not_found)?;
    if state != "visible" || post_state != "visible" {
        return Err(AppError::not_found());
    }
    if locked {
        return Err(AppError::forbidden(
            "This post is locked: it takes no new comments.",
        ));
    }
    if depth >= policy::COMMENT_DEPTH_MAX {
        return Err(AppError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!(
                "A reply can be at most {} levels below a top-level comment.",
                policy::COMMENT_DEPTH_MAX
            ),
        ));
    }
    Ok(Parent {
        post_id,
        author: login.unwrap_or_else(|| "[deleted]".to_string()),
        html: markdown::render(&body).0,
    })
}

/// `GET /c/<id>/reply`.
pub async fn reply_form(
    State(state): State<AppState>,
    ctx: Ctx,
    RequireUser(member): RequireUser,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let id = parse_id(&id)?;
    let parent = state.db.read(move |conn| parent_of(conn, id)).await?;
    Ok(ctx
        .page("reply", "Reply")
        .body(html! {
            (quote(&parent.author, &parent.html))
            (form_markup(&member, &format!("/p/{}/comment", parent.post_id), Some(id), "Reply", ""))
        })
        .into_response())
}

/// `POST /p/<id>/comment` with `body`, `parent` (empty for a top-level
/// comment) and `csrf`.
pub async fn create(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(post): Path<String>,
    CsrfForm(values, member): CsrfForm<Values>,
) -> Result<Response, AppError> {
    let post_id = parse_id(&post)?;
    let parent = match values.parent.trim() {
        "" => None,
        p => Some(parse_id(p)?),
    };
    let now = state.clock.now();
    let author = member.clone();
    let body = values.body.clone();
    let result = state
        .db
        .write(move |tx| comments::create_checked(tx, &author, post_id, parent, &body, now))
        .await;
    match result {
        Ok(id) => Ok(Redirect::to(&format!("/p/{post_id}#c{id}")).into_response()),
        Err(e) if e.status == StatusCode::UNPROCESSABLE_ENTITY => Ok(error_page(
            &ctx,
            &member,
            "reply",
            "Reply",
            &format!("/p/{post_id}/comment"),
            parent,
            "Reply",
            &values.body,
            &e.message,
        )),
        Err(e) => Err(e),
    }
}

struct Editable {
    body: String,
}

fn editable(
    conn: &Connection,
    member: &Member,
    id: i64,
    now: OffsetDateTime,
) -> Result<Editable, AppError> {
    let row: Option<(Option<i64>, String, String, String, i64)> = conn
        .query_row(
            "SELECT author_id, state, created_at, body, post_id FROM comments WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?;
    let (author, state, created, body, _post_id) = row.ok_or_else(AppError::not_found)?;
    if author != Some(member.id.0) {
        return Err(AppError::forbidden(
            "Only the author can edit this comment.",
        ));
    }
    if state != "visible" {
        return Err(AppError::forbidden(
            "A comment that is not visible cannot be edited.",
        ));
    }
    let created = clock::parse_storage(&created).unwrap_or(OffsetDateTime::UNIX_EPOCH);
    if now - created > Duration::hours(policy::EDIT_WINDOW_HOURS) {
        return Err(AppError::forbidden(format!(
            "A comment can be edited for {} hours after it is created.",
            policy::EDIT_WINDOW_HOURS
        )));
    }
    Ok(Editable { body })
}

/// `GET /c/<id>/edit`.
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
    Ok(ctx
        .page("edit", "Edit")
        .body(form_markup(
            &member,
            &format!("/c/{id}/edit"),
            None,
            "Save",
            &loaded.body,
        ))
        .into_response())
}

/// `POST /c/<id>/edit` with `body` and `csrf`.
pub async fn edit(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<String>,
    CsrfForm(values, member): CsrfForm<Values>,
) -> Result<Response, AppError> {
    let id = parse_id(&id)?;
    let now = state.clock.now();
    let author = member.clone();
    let body = values.body.clone();
    let result = state
        .db
        .write(move |tx| {
            comments::edit_checked(tx, &author, id, &body, now)?;
            Ok(post_of(tx, id)?)
        })
        .await;
    match result {
        Ok(post_id) => Ok(Redirect::to(&format!("/p/{post_id}#c{id}")).into_response()),
        Err(e) if e.status == StatusCode::UNPROCESSABLE_ENTITY => Ok(error_page(
            &ctx,
            &member,
            "edit",
            "Edit",
            &format!("/c/{id}/edit"),
            None,
            "Save",
            &values.body,
            &e.message,
        )),
        Err(e) => Err(e),
    }
}

fn post_of(conn: &Connection, comment: i64) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT post_id FROM comments WHERE id = ?1",
        [comment],
        |r| r.get(0),
    )
}

#[derive(Debug, Deserialize)]
pub struct Nothing {}

/// `POST /c/<id>/delete`.
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
    CsrfForm(_, member): CsrfForm<Nothing>,
) -> Result<Response, AppError> {
    let id = parse_id(&id)?;
    let post_id = state
        .db
        .write(move |tx| {
            comments::delete(tx, &member, id)?;
            Ok(post_of(tx, id)?)
        })
        .await?;
    Ok(Redirect::to(&format!("/p/{post_id}#c{id}")).into_response())
}
