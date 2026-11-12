//! `GET /admin`, `POST /admin/role` and `POST /admin/sync`: members and
//! moderators, the registry's sync status, and the sync button. Every route
//! answers 404 to anyone who is not an admin. The pages are protected:
//! built-in styling only.

use axum::extract::{RawQuery, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use maud::{html, Markup};
use rusqlite::{Connection, OptionalExtension};
use serde::Deserialize;

use crate::auth::session::{CsrfForm, RequireAdmin};
use crate::clock;
use crate::db::{Member, Role, UserId};
use crate::error::AppError;
use crate::jobs;
use crate::moderation::{self, Actor};
use crate::users::{self, Standing};
use crate::web::pages::moderation::page_number;
use crate::web::{AppState, Ctx};

struct Listed {
    login: String,
    role: &'static str,
    standing: &'static str,
    id: i64,
    joined: String,
}

fn listed(
    conn: &Connection,
    state: &AppState,
    id: i64,
    now: time::OffsetDateTime,
) -> rusqlite::Result<Option<Listed>> {
    let Some(m) = users::load_member(conn, UserId(id))? else {
        return Ok(None);
    };
    let role = if users::is_admin(&m, &state.config) {
        "admin"
    } else if m.role == Role::Moderator {
        "moderator"
    } else {
        "member"
    };
    let standing = match users::standing(&m, now) {
        Standing::New => "new",
        Standing::Established => "established",
        Standing::Restricted => "restricted",
        Standing::Banned => "banned",
    };
    Ok(Some(Listed {
        login: m.login,
        role,
        standing,
        id,
        joined: clock::date_text(m.created_at),
    }))
}

fn ids(
    conn: &Connection,
    sql: &str,
    params: &[&dyn rusqlite::ToSql],
) -> rusqlite::Result<Vec<i64>> {
    conn.prepare(sql)?
        .query_map(params, |r| r.get(0))?
        .collect()
}

struct Sync {
    configured: bool,
    last: Option<String>,
    ok: Option<bool>,
    last_ok: Option<String>,
}

fn meta(conn: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
        .optional()
}

fn role_form(admin: &Member, m: &Listed) -> Markup {
    let (value, label) = if m.role == "moderator" {
        ("none", "Remove moderator")
    } else {
        ("moderator", "Appoint moderator")
    };
    html! {
        form method="post" action="/admin/role" {
            input type="hidden" name="csrf" value=(admin.csrf);
            input type="hidden" name="user_id" value=(m.id);
            input type="hidden" name="role" value=(value);
            input type="hidden" name="reason" value="other";
            button class="hp-link-button" type="submit" { (label) }
        }
    }
}

fn members_table(admin: &Member, rows: &[Listed]) -> Markup {
    html! {
        table class="hp-table" {
            thead {
                tr {
                    th scope="col" { "Member" }
                    th scope="col" { "Role" }
                    th scope="col" { "Standing" }
                    th scope="col" { "Joined" }
                    th scope="col" { "Role change" }
                }
            }
            tbody {
                @for m in rows {
                    tr {
                        td { a href=(format!("/mod/u/{}", m.login)) { (m.login) } }
                        td { (m.role) }
                        td { (m.standing) }
                        td { (m.joined) }
                        td { @if m.role != "admin" { (role_form(admin, m)) } }
                    }
                }
            }
        }
    }
}

/// `GET /admin`.
pub async fn admin(
    State(state): State<AppState>,
    ctx: Ctx,
    RequireAdmin(member): RequireAdmin,
    RawQuery(query): RawQuery,
) -> Result<Response, AppError> {
    let page = page_number(query.as_deref())?;
    let now = state.clock.now();
    let size = crate::policy::LIST_PAGE_SIZE;
    let st = state.clone();
    let (moderators, members, more, sync) = state
        .db
        .read(move |conn| {
            let mod_ids = ids(
                conn,
                "SELECT id FROM users WHERE role = 'moderator' AND deleted_at IS NULL
                 ORDER BY login",
                &[],
            )?;
            let mut moderators = Vec::new();
            for id in mod_ids {
                moderators.extend(listed(conn, &st, id, now)?);
            }
            let offset = crate::page::offset(page, size) as i64;
            let mut member_ids = ids(
                conn,
                "SELECT id FROM users WHERE deleted_at IS NULL
                 ORDER BY created_at DESC, id DESC LIMIT ?1 OFFSET ?2",
                &[&((size + 1) as i64), &offset],
            )?;
            let more = member_ids.len() > size;
            member_ids.truncate(size);
            let mut members = Vec::new();
            for id in member_ids {
                members.extend(listed(conn, &st, id, now)?);
            }
            let sync = Sync {
                configured: st.config.registry_index.is_some(),
                last: meta(conn, "last_sync")?.and_then(|v| clock::storage_to_rfc3339(&v)),
                ok: meta(conn, "last_sync_ok")?.map(|v| v == "1" || v == "true"),
                last_ok: meta(conn, "last_successful_sync")?
                    .and_then(|v| clock::storage_to_rfc3339(&v)),
            };
            Ok((moderators, members, more, sync))
        })
        .await?;
    Ok(ctx
        .page("admin", "Admin")
        .body(html! {
            h2 { "Registry sync" }
            @if !sync.configured {
                p class="hp-notice" { "No registry index is configured." }
            }
            dl {
                dt { "Last sync" }
                dd { (sync.last.as_deref().unwrap_or("never")) }
                dt { "Last sync read the index" }
                dd { @match sync.ok { Some(true) => { "yes" } Some(false) => { "no" } None => { "-" } } }
                dt { "Last successful sync" }
                dd { (sync.last_ok.as_deref().unwrap_or("never")) }
            }
            form class="hp-form" method="post" action="/admin/sync" {
                input type="hidden" name="csrf" value=(member.csrf);
                button class="hp-button" type="submit" { "Sync the registry now" }
            }
            h2 { "Moderators" }
            @if moderators.is_empty() {
                p class="hp-help" { "No moderators are appointed." }
            } @else {
                (members_table(&member, &moderators))
            }
            h2 { "Members" }
            (members_table(&member, &members))
            @if more {
                p { a class="hp-more" href=(format!("/admin?page={}", page + 1)) rel="next" { "More" } }
            }
        })
        .into_response())
}

#[derive(Debug, Deserialize)]
pub struct RoleForm {
    #[serde(default)]
    user_id: String,
    #[serde(default)]
    role: String,
    #[serde(default)]
    reason: String,
}

/// `POST /admin/role` with `user_id`, `role` (`moderator` or `none`),
/// `reason` (default `other`) and `csrf`. Redirects to `/admin`.
pub async fn set_role(
    State(state): State<AppState>,
    CsrfForm(form, member): CsrfForm<RoleForm>,
) -> Result<Response, AppError> {
    if !users::is_admin(&member, &state.config) {
        return Err(AppError::not_found());
    }
    let id: i64 = form.user_id.parse().map_err(|_| AppError::not_found())?;
    let role = match form.role.as_str() {
        "moderator" => Role::Moderator,
        "none" => Role::None,
        _ => return Err(AppError::bad_request("role must be moderator or none.")),
    };
    let reason = if form.reason.is_empty() {
        "other".to_string()
    } else {
        form.reason
    };
    let now = state.clock.now();
    let admins = state.config.admins.clone();
    state
        .db
        .write(move |tx| {
            let actor = Actor::new(&member, &admins)?;
            moderation::user_set_role(tx, &actor, UserId(id), role, &reason, now)
        })
        .await?;
    Ok(Redirect::to("/admin").into_response())
}

#[derive(Debug, Deserialize)]
pub struct Nothing {}

/// `POST /admin/sync`: runs a registry sync with the admin as actor.
/// Redirects to `/admin`, whose status then shows the result.
pub async fn sync(
    State(state): State<AppState>,
    CsrfForm(_, member): CsrfForm<Nothing>,
) -> Result<Response, AppError> {
    if !users::is_admin(&member, &state.config) {
        return Err(AppError::not_found());
    }
    match jobs::registry_sync(&state, Some(member.id)).await? {
        Some(_) => Ok(Redirect::to("/admin").into_response()),
        None => Err(AppError::new(
            StatusCode::CONFLICT,
            "No registry index is configured.",
        )),
    }
}
