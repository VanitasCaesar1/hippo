//! `GET /mod`, `GET /mod/u/<login>` and `POST /mod/action`: the moderators'
//! queue, a member's page for moderators, and the actions.
//!
//! Everything here answers 404 to anyone who is not a moderator or an admin.
//! The pages are protected: built-in styling only.

use std::collections::HashMap;

use axum::extract::{Path, RawQuery, State};
use axum::response::{IntoResponse, Redirect, Response};
use maud::{html, Markup};
use rusqlite::Connection;
use serde::Deserialize;

use crate::auth::session::{CsrfForm, RequireModerator};
use crate::clock;
use crate::db::{Member, Role, UserId};
use crate::error::{AppError, FormError};
use crate::moderation::{self, Actor, AuditRow, RestrictFor, REASONS};
use crate::reports::{self, QueueEntry, QueueReport, ReportTarget, Reporter};
use crate::rings;
use crate::users::{self, Standing};
use crate::web::pages::moderation::{actor_cell, page_number};
use crate::web::{local_path, AppState, Ctx};

fn standing_text(s: Standing) -> &'static str {
    match s {
        Standing::New => "new",
        Standing::Established => "established",
        Standing::Restricted => "restricted",
        Standing::Banned => "banned",
    }
}

const UNTIL: [(&str, &str); 4] = [
    ("1d", "1 day"),
    ("7d", "7 days"),
    ("30d", "30 days"),
    ("indefinite", "indefinitely"),
];

/// The select of a reason code, with its label.
fn reason_field(id: &str, chosen: &str) -> Markup {
    html! {
        div class="hp-field" {
            label class="hp-label" for=(id) { "Reason" }
            select class="hp-select" id=(id) name="reason" {
                @for reason in REASONS {
                    option value=(reason) selected[reason == chosen] { (reason) }
                }
            }
        }
    }
}

fn until_field(id: &str) -> Markup {
    html! {
        div class="hp-field" {
            label class="hp-label" for=(id) { "Restrict for" }
            select class="hp-select" id=(id) name="until" {
                @for (value, label) in UNTIL {
                    option value=(value) selected[value == "7d"] { (label) }
                }
            }
        }
    }
}

/// The actions a moderator may take on a target in a state.
fn actions_for(
    target: ReportTarget,
    state: Option<&str>,
    exists: bool,
    admin: bool,
) -> Vec<(&'static str, &'static str)> {
    let mut out: Vec<(&'static str, &'static str)> = Vec::new();
    let live = matches!(state, Some("visible" | "hidden"));
    let restorable = matches!(state, Some("hidden" | "removed"));
    if exists {
        match target {
            ReportTarget::Post(_) => {
                if live {
                    out.push(("post.remove", "Remove"));
                }
                if restorable {
                    out.push(("post.restore", "Restore"));
                }
                out.push(("post.lock", "Lock"));
                out.push(("post.unlock", "Unlock"));
            }
            ReportTarget::Comment(_) => {
                if live {
                    out.push(("comment.remove", "Remove"));
                }
                if restorable {
                    out.push(("comment.restore", "Restore"));
                }
            }
            ReportTarget::Measurement(_) => {
                out.push(("measurement.invalidate", "Invalidate"));
                out.push(("measurement.restore", "Restore"));
            }
            ReportTarget::User(_) => {
                out.push(("user.restrict", "Restrict"));
                out.push(("user.unrestrict", "Lift the restriction"));
                if admin {
                    out.push(("user.ban", "Ban"));
                    out.push(("user.unban", "Unban"));
                    out.push(("user.spam_ban", "Spam-ban"));
                }
            }
        }
    }
    out
}

/// One form with an action chooser, a reason, and, where restricting is on
/// offer, a duration.
fn action_form(
    member: &Member,
    n: &str,
    return_to: &str,
    target: ReportTarget,
    actions: &[(&str, &str)],
    dismiss: bool,
) -> Markup {
    let restrict = actions.iter().any(|(a, _)| *a == "user.restrict");
    let id = |what: &str| format!("{what}-{n}");
    html! {
        form class="hp-form" method="post" action="/mod/action" {
            input type="hidden" name="csrf" value=(member.csrf);
            input type="hidden" name="target_type" value=(target.type_str());
            input type="hidden" name="target_id" value=(target.id());
            input type="hidden" name="return_to" value=(return_to);
            div class="hp-field" {
                label class="hp-label" for=(id("action")) { "Action" }
                select class="hp-select" id=(id("action")) name="action" {
                    @for (value, label) in actions { option value=(value) { (label) } }
                    @if dismiss { option value="report.dismiss" { "Dismiss the reports" } }
                }
            }
            (reason_field(&id("reason"), "spam"))
            @if restrict { (until_field(&id("until"))) }
            button class="hp-button" type="submit" { "Apply" }
        }
    }
}

fn target_link(entry: &QueueEntry) -> Markup {
    let id = entry.target.id();
    let post = entry.described.as_ref().and_then(|d| d.post_id);
    html! {
        @match (entry.target, post) {
            (ReportTarget::Post(_), _) => { a href=(format!("/p/{id}")) { "Post " (id) } }
            (ReportTarget::Comment(_), Some(post)) => { a href=(format!("/p/{post}#c{id}")) { "Comment " (id) } }
            (ReportTarget::Measurement(_), Some(post)) => { a href=(format!("/p/{post}#m{id}")) { "Measurement " (id) } }
            (ReportTarget::Measurement(_), None) => { "Measurement " (id) }
            (ReportTarget::Comment(_), None) => { "Comment " (id) }
            (ReportTarget::User(_), _) => { "Member " (id) }
        }
    }
}

fn report_item(
    member: &Member,
    n: &str,
    return_to: &str,
    report: &QueueReport,
    logins: &HashMap<i64, String>,
    admin: bool,
) -> Markup {
    let accounts: Vec<i64> = report
        .details
        .get("accounts")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_i64()).collect())
        .unwrap_or_default();
    let acted: Vec<String> = report
        .details
        .get("acted_on")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let is_ring = matches!(report.reporter, Reporter::System) && report.reason == "ring";
    let _ = admin;
    html! {
        li {
            span {
                @match &report.reporter {
                    Reporter::System => { "system" }
                    Reporter::Deleted => { span class="hp-placeholder" { "[deleted]" } }
                    Reporter::Member { login, .. } => { a href=(format!("/mod/u/{login}")) { (login) } }
                }
            }
            ": " (report.reason) " (" (clock::rfc3339(report.created_at)) ")"
            @if !report.note.is_empty() {
                p { "Note: " (report.note) }
            }
            @if !report.snapshot.is_empty() {
                p class="hp-help" { "Snapshot at filing:" }
                pre class="hp-code" { code { (report.snapshot) } }
            }
            @if is_ring {
                p {
                    "Named accounts: "
                    @for (i, id) in accounts.iter().enumerate() {
                        @if i > 0 { ", " }
                        @match logins.get(id) {
                            Some(login) => { a href=(format!("/mod/u/{login}")) { (login) } }
                            None => { "[deleted]" }
                        }
                    }
                }
                @if !acted.is_empty() {
                    p { "Acted on: " (acted.join(", ")) }
                }
                form class="hp-form" method="post" action="/mod/action" {
                    input type="hidden" name="csrf" value=(member.csrf);
                    input type="hidden" name="action" value="ring.restrict";
                    input type="hidden" name="target_type" value="report";
                    input type="hidden" name="target_id" value=(report.id);
                    input type="hidden" name="reason" value="ring";
                    input type="hidden" name="return_to" value=(return_to);
                    (until_field(&format!("until-ring-{n}-{}", report.id)))
                    button class="hp-button" type="submit" { "Restrict the named accounts" }
                }
            }
        }
    }
}

fn entry_section(
    member: &Member,
    n: usize,
    return_to: &str,
    entry: &QueueEntry,
    logins: &HashMap<i64, String>,
    admin: bool,
) -> Markup {
    let state = entry.described.as_ref().and_then(|d| d.state.as_deref());
    let owner = entry
        .described
        .as_ref()
        .and_then(|d| d.owner)
        .and_then(|id| logins.get(&id));
    let key = n.to_string();
    let actions = actions_for(entry.target, state, entry.described.is_some(), admin);
    html! {
        section {
            h2 { (target_link(entry)) }
            p {
                @if let Some(state) = state { "State: " (state) ". " }
                @if let Some(owner) = owner {
                    "By " a href=(format!("/mod/u/{owner}")) { (owner) } ". "
                }
            }
            p { "Preview: " (entry.preview) }
            p {
                "Reports: "
                @for (i, (reason, count)) in entry.counts.iter().enumerate() {
                    @if i > 0 { ", " }
                    (reason) " " (count)
                }
            }
            @if entry.forward_to_registry() {
                p class="hp-notice" {
                    "A malicious report on a pack: forward it to the registry maintainer through hippocamp's private vulnerability reporting ("
                    a href="https://github.com/VanitasCaesar1/hippocamp/blob/main/SECURITY.md" rel="nofollow noopener noreferrer" { "SECURITY.md" }
                    "). Removing the post takes its install links off the feed at once; only the registry can delist the pack."
                }
            }
            ul {
                @for report in &entry.reports {
                    (report_item(member, &key, return_to, report, logins, admin))
                }
            }
            (action_form(member, &key, return_to, entry.target, &actions, true))
        }
    }
}

/// `GET /mod`.
pub async fn queue(
    State(state): State<AppState>,
    ctx: Ctx,
    RequireModerator(member): RequireModerator,
    RawQuery(query): RawQuery,
) -> Result<Response, AppError> {
    let page = page_number(query.as_deref())?;
    let admin = users::is_admin(&member, &state.config);
    let (found, logins) = state
        .db
        .read(move |conn| {
            let found = reports::open_queue(conn, page)?;
            let mut ids: Vec<i64> = Vec::new();
            for entry in &found.rows {
                ids.extend(entry.described.as_ref().and_then(|d| d.owner));
                for report in &entry.reports {
                    if let Some(accounts) =
                        report.details.get("accounts").and_then(|v| v.as_array())
                    {
                        ids.extend(accounts.iter().filter_map(|v| v.as_i64()));
                    }
                }
            }
            Ok((found, logins_of(conn, &ids)?))
        })
        .await?;
    let return_to = ctx.return_to.clone();
    Ok(ctx
        .page("mod", "Moderation queue")
        .body(html! {
            @if found.rows.is_empty() {
                p class="hp-notice" { "No open reports." }
            }
            @for (i, entry) in found.rows.iter().enumerate() {
                (entry_section(&member, i, &return_to, entry, &logins, admin))
            }
            @if found.more {
                p { a class="hp-more" href=(format!("/mod?page={}", page + 1)) rel="next" { "More" } }
            }
            @if admin {
                p { a href="/admin" { "Admin" } }
            }
        })
        .into_response())
}

fn logins_of(conn: &Connection, ids: &[i64]) -> rusqlite::Result<HashMap<i64, String>> {
    let json = serde_json::to_string(ids).unwrap_or_else(|_| "[]".into());
    let mut stmt = conn.prepare(
        "SELECT id, login FROM users WHERE deleted_at IS NULL
           AND id IN (SELECT value FROM json_each(?1))",
    )?;
    let rows = stmt.query_map([json], |r| Ok((r.get(0)?, r.get(1)?)))?;
    rows.collect()
}

fn audit_table(rows: &[AuditRow]) -> Markup {
    html! {
        table class="hp-table" {
            thead {
                tr {
                    th scope="col" { "Date" }
                    th scope="col" { "Actor" }
                    th scope="col" { "Action" }
                    th scope="col" { "Target" }
                    th scope="col" { "Reason" }
                    th scope="col" { "Public" }
                    th scope="col" { "Details" }
                }
            }
            tbody {
                @for row in rows {
                    tr {
                        td { (clock::rfc3339(row.at)) }
                        td { (actor_cell(&row.actor)) }
                        td { (row.action) }
                        td {
                            (row.target_type.as_deref().unwrap_or("-")) " "
                            (row.target_id.as_deref().unwrap_or(""))
                        }
                        td { (row.reason) }
                        td { @if row.public { "yes" } @else { "no" } }
                        td { code { (row.details.to_string()) } }
                    }
                }
            }
        }
    }
}

/// `GET /mod/u/<login>`.
pub async fn member_page(
    State(state): State<AppState>,
    ctx: Ctx,
    RequireModerator(member): RequireModerator,
    Path(login): Path<String>,
) -> Result<Response, AppError> {
    let now = state.clock.now();
    let admins = state.config.admins.clone();
    let admin = users::is_admin(&member, &state.config);
    let name = login.clone();
    let view = state
        .db
        .read(move |conn| Ok(moderation::member_view(conn, &name, &admins, now)?))
        .await?
        .ok_or_else(AppError::not_found)?;
    let target = ReportTarget::User(view.id.0);
    let actions = actions_for(target, None, !view.deleted, admin);
    let return_to = ctx.return_to.clone();
    let role = if view.is_admin {
        "admin"
    } else if view.role == Role::Moderator {
        "moderator"
    } else {
        "member"
    };
    Ok(ctx
        .page("mod", format!("Member: {}", view.login))
        .body(html! {
            dl {
                dt { "Standing" } dd { (standing_text(view.standing)) }
                dt { "Role" } dd { (role) }
                dt { "Joined" } dd { (clock::rfc3339(view.created_at)) }
                @if let Some(until) = view.restricted_until {
                    dt { "Restricted until" } dd { (clock::rfc3339(until)) }
                }
                @if view.deleted { dt { "Account" } dd { "deleted" } }
                dt { "Posts" } dd { (view.posts) }
                dt { "Comments" } dd { (view.comments) }
                dt { "Votes" } dd { (view.votes) }
                dt { "Measurements" } dd { (view.measurements) }
                dt { "Reports filed" } dd { (view.reports_filed) }
                dt { "Reports against" } dd { (view.reports_against) }
            }
            h2 { "Actions" }
            @if actions.is_empty() {
                p class="hp-notice" { "No account actions are open for this member." }
            } @else {
                (action_form(&member, "member", &return_to, target, &actions, false))
            }
            h2 { "Audit log" }
            @if view.audit.is_empty() {
                p class="hp-help" { "Nothing is logged about this member." }
            } @else {
                (audit_table(&view.audit))
            }
            p { a href="/mod" { "Back to the queue" } }
        })
        .into_response())
}

/// The fields of `POST /mod/action`.
#[derive(Debug, Deserialize)]
pub struct ActionForm {
    #[serde(default)]
    action: String,
    #[serde(default)]
    target_type: String,
    #[serde(default)]
    target_id: String,
    #[serde(default)]
    reason: String,
    #[serde(default)]
    until: String,
    #[serde(default)]
    return_to: Option<String>,
}

fn restrict_for(text: &str) -> Result<RestrictFor, AppError> {
    RestrictFor::parse(text).ok_or_else(|| {
        AppError::form(FormError::new(
            "until",
            "must be one of 1d, 7d, 30d, indefinite",
        ))
    })
}

/// Runs the action in the caller's transaction.
fn dispatch(
    tx: &rusqlite::Transaction,
    actor: &Actor,
    form: &ActionForm,
    id: i64,
    now: time::OffsetDateTime,
) -> Result<(), AppError> {
    let reason = form.reason.as_str();
    let user = UserId(id);
    match form.action.as_str() {
        "post.remove" => moderation::post_remove(tx, actor, id, reason, now),
        "post.restore" => moderation::post_restore(tx, actor, id, reason, now),
        "post.lock" => moderation::post_lock(tx, actor, id, reason, now),
        "post.unlock" => moderation::post_unlock(tx, actor, id, reason, now),
        "comment.remove" => moderation::comment_remove(tx, actor, id, reason, now),
        "comment.restore" => moderation::comment_restore(tx, actor, id, reason, now),
        "measurement.invalidate" => moderation::measurement_invalidate(tx, actor, id, reason, now),
        "measurement.restore" => moderation::measurement_restore(tx, actor, id, reason, now),
        "user.restrict" => {
            let until = restrict_for(&form.until)?;
            moderation::user_restrict(tx, actor, user, until, reason, now)
        }
        "user.unrestrict" => moderation::user_unrestrict(tx, actor, user, reason, now),
        "user.ban" => moderation::user_ban(tx, actor, user, reason, now),
        "user.unban" => moderation::user_unban(tx, actor, user, reason, now),
        "user.spam_ban" => moderation::user_spam_ban(tx, actor, user, reason, now).map(|_| ()),
        "report.dismiss" => {
            let target = ReportTarget::parse(&form.target_type, id)
                .ok_or_else(|| AppError::bad_request("The action has no valid target."))?;
            moderation::dismiss(tx, actor, target, reason, now).map(|_| ())
        }
        "ring.restrict" => {
            let until = restrict_for(&form.until)?;
            rings::restrict_named(tx, id, actor, until, now).map(|_| ())
        }
        _ => Err(AppError::bad_request("Unknown action.")),
    }
}

/// `POST /mod/action`. Redirects to `return_to` when it is a local path, else
/// to the target's page.
pub async fn action(
    State(state): State<AppState>,
    CsrfForm(form, member): CsrfForm<ActionForm>,
) -> Result<Response, AppError> {
    if !users::is_moderator(&member, &state.config) {
        return Err(AppError::not_found());
    }
    let id: i64 = form
        .target_id
        .parse()
        .map_err(|_| AppError::bad_request("The action has no valid target."))?;
    let now = state.clock.now();
    let admins = state.config.admins.clone();
    let to = state
        .db
        .write(move |tx| {
            let actor = Actor::new(&member, &admins)?;
            dispatch(tx, &actor, &form, id, now)?;
            Ok(match form.return_to.as_deref() {
                Some(r) if local_path(r) == r => r.to_string(),
                _ => match ReportTarget::parse(&form.target_type, id) {
                    Some(target) => crate::web::pages::report::landing(tx, target, true)?,
                    None => "/mod".to_string(),
                },
            })
        })
        .await?;
    Ok(Redirect::to(&to).into_response())
}
