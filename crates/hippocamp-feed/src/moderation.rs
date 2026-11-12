//! Moderation: one function per moderation action
//! table, the public log and a member's page for moderators.
//!
//! Each action runs in the caller's transaction: it changes its target,
//! resolves the target's open reports, and writes exactly one audit row with the table's public flag. On an
//! error the caller drops the transaction, so nothing is half done. A reason
//! is a code from [`REASONS`]; nothing here takes free text.

use axum::http::StatusCode;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde_json::{json, Value};
use time::{Duration, OffsetDateTime};

use crate::audit;
use crate::clock;
use crate::comments;
use crate::db::{Member, Role, UserId};
use crate::error::{AppError, FormError};
use crate::page::{self, Page};
use crate::policy;
use crate::posts;
use crate::reports::{self, ReportTarget, Resolution};
use crate::users::{self, Standing};

/// The reasons a moderation action takes, which the public log shows.
pub const REASONS: [&str; 9] = [
    "spam",
    "abuse",
    "malicious",
    "licence",
    "misleading",
    "off-topic",
    "duplicate",
    "ring",
    "other",
];

/// A reason code of [`REASONS`], or the form error.
pub fn check_reason(reason: &str) -> Result<String, AppError> {
    let reason = reason.trim();
    if REASONS.contains(&reason) {
        Ok(reason.to_string())
    } else {
        Err(AppError::form(FormError::new(
            "reason",
            format!("must be one of {}", REASONS.join(", ")),
        )))
    }
}

fn conflict(message: impl Into<String>) -> AppError {
    AppError::new(StatusCode::CONFLICT, message)
}

/// The member taking an action: a moderator or an admin.
#[derive(Debug, Clone, Copy)]
pub struct Actor<'a> {
    pub id: UserId,
    pub is_admin: bool,
    /// `FEED_ADMINS`, to tell whether a target is an admin.
    pub admins: &'a [i64],
}

impl<'a> Actor<'a> {
    /// `member` as an actor, or 403 if they are neither a moderator nor an
    /// admin (a GitHub id in `admins`).
    pub fn new(member: &Member, admins: &'a [i64]) -> Result<Self, AppError> {
        let is_admin = member.github_id.is_some_and(|id| admins.contains(&id));
        if !is_admin && member.role != Role::Moderator {
            return Err(AppError::forbidden("Moderators and admins only."));
        }
        Ok(Self {
            id: member.id,
            is_admin,
            admins,
        })
    }

    fn require_admin(&self) -> Result<(), AppError> {
        if self.is_admin {
            Ok(())
        } else {
            Err(AppError::forbidden("Admins only."))
        }
    }
}

/// How long a restriction lasts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestrictFor {
    Day,
    Week,
    Month,
    Indefinite,
}

impl RestrictFor {
    /// The form's `until`: `1d`, `7d`, `30d` or `indefinite`.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "1d" => Some(Self::Day),
            "7d" => Some(Self::Week),
            "30d" => Some(Self::Month),
            "indefinite" => Some(Self::Indefinite),
            _ => None,
        }
    }

    /// The time the restriction ends, as stored; the indefinite one is
    /// [`policy::INDEFINITE`].
    pub fn until(self, now: OffsetDateTime) -> String {
        let days = match self {
            Self::Day => 1,
            Self::Week => 7,
            Self::Month => 30,
            Self::Indefinite => return policy::INDEFINITE.to_string(),
        };
        clock::storage(now + Duration::days(days))
    }
}

#[allow(clippy::too_many_arguments)]
fn audit_row(
    tx: &Transaction,
    now: OffsetDateTime,
    actor: &Actor,
    action: &str,
    target: ReportTarget,
    reason: &str,
    public: bool,
    details: Value,
) -> Result<(), AppError> {
    audit::write(
        tx,
        now,
        Some(actor.id),
        action,
        Some(target.audit()),
        reason,
        public,
        details,
    )?;
    Ok(())
}

// --- Posts ---

/// Removes a visible or hidden post; its open reports are upheld. Public
/// `post.remove`.
pub fn post_remove(
    tx: &Transaction,
    actor: &Actor,
    id: i64,
    reason: &str,
    now: OffsetDateTime,
) -> Result<(), AppError> {
    let reason = check_reason(reason)?;
    posts::remove(tx, id, &reason)?;
    let target = ReportTarget::Post(id);
    reports::resolve_target(tx, target, Resolution::Upheld, Some(actor.id), now)?;
    audit_row(
        tx,
        now,
        actor,
        audit::POST_REMOVE,
        target,
        &reason,
        true,
        json!({}),
    )
}

/// Restores a hidden or removed post; its open reports are dismissed. Public
/// `post.restore`.
pub fn post_restore(
    tx: &Transaction,
    actor: &Actor,
    id: i64,
    reason: &str,
    now: OffsetDateTime,
) -> Result<(), AppError> {
    let reason = check_reason(reason)?;
    posts::restore(tx, id)?;
    let target = ReportTarget::Post(id);
    reports::resolve_target(tx, target, Resolution::Dismissed, Some(actor.id), now)?;
    audit_row(
        tx,
        now,
        actor,
        audit::POST_RESTORE,
        target,
        &reason,
        true,
        json!({}),
    )
}

fn post_locked(tx: &Transaction, id: i64) -> Result<bool, AppError> {
    tx.query_row("SELECT locked FROM posts WHERE id = ?1", [id], |r| r.get(0))
        .optional()?
        .ok_or_else(AppError::not_found)
}

/// Locks a post (409 if it is locked). Public `post.lock`; reports are left
/// as they are.
pub fn post_lock(
    tx: &Transaction,
    actor: &Actor,
    id: i64,
    reason: &str,
    now: OffsetDateTime,
) -> Result<(), AppError> {
    let reason = check_reason(reason)?;
    if post_locked(tx, id)? {
        return Err(conflict("This post is already locked."));
    }
    posts::lock(tx, id)?;
    audit_row(
        tx,
        now,
        actor,
        audit::POST_LOCK,
        ReportTarget::Post(id),
        &reason,
        true,
        json!({}),
    )
}

/// Unlocks a post (409 if it is not locked). Public `post.unlock`.
pub fn post_unlock(
    tx: &Transaction,
    actor: &Actor,
    id: i64,
    reason: &str,
    now: OffsetDateTime,
) -> Result<(), AppError> {
    let reason = check_reason(reason)?;
    if !post_locked(tx, id)? {
        return Err(conflict("This post is not locked."));
    }
    posts::unlock(tx, id)?;
    audit_row(
        tx,
        now,
        actor,
        audit::POST_UNLOCK,
        ReportTarget::Post(id),
        &reason,
        true,
        json!({}),
    )
}

// --- Comments ---

/// Removes a visible or hidden comment; its open reports are upheld. Public
/// `comment.remove`.
pub fn comment_remove(
    tx: &Transaction,
    actor: &Actor,
    id: i64,
    reason: &str,
    now: OffsetDateTime,
) -> Result<(), AppError> {
    let reason = check_reason(reason)?;
    comments::remove(tx, id, &reason)?;
    let target = ReportTarget::Comment(id);
    reports::resolve_target(tx, target, Resolution::Upheld, Some(actor.id), now)?;
    audit_row(
        tx,
        now,
        actor,
        audit::COMMENT_REMOVE,
        target,
        &reason,
        true,
        json!({}),
    )
}

/// Restores a hidden or removed comment; its open reports are dismissed.
/// Public `comment.restore`.
pub fn comment_restore(
    tx: &Transaction,
    actor: &Actor,
    id: i64,
    reason: &str,
    now: OffsetDateTime,
) -> Result<(), AppError> {
    let reason = check_reason(reason)?;
    comments::restore(tx, id)?;
    let target = ReportTarget::Comment(id);
    reports::resolve_target(tx, target, Resolution::Dismissed, Some(actor.id), now)?;
    audit_row(
        tx,
        now,
        actor,
        audit::COMMENT_RESTORE,
        target,
        &reason,
        true,
        json!({}),
    )
}

// --- Measurements ---

fn set_invalid(tx: &Transaction, id: i64, invalid: bool) -> Result<(), AppError> {
    let n = tx.execute(
        "UPDATE measurements SET invalid = ?1 WHERE id = ?2 AND invalid = ?3",
        params![invalid, id, !invalid],
    )?;
    if n > 0 {
        return Ok(());
    }
    let exists: bool = tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM measurements WHERE id = ?1)",
        [id],
        |r| r.get(0),
    )?;
    if exists {
        Err(conflict(if invalid {
            "This measurement is already invalid."
        } else {
            "This measurement is not invalid."
        }))
    } else {
        Err(AppError::not_found())
    }
}

/// Marks a measurement invalid, so it stops counting; its open reports are
/// upheld. Public `measurement.invalidate`.
pub fn measurement_invalidate(
    tx: &Transaction,
    actor: &Actor,
    id: i64,
    reason: &str,
    now: OffsetDateTime,
) -> Result<(), AppError> {
    let reason = check_reason(reason)?;
    set_invalid(tx, id, true)?;
    let target = ReportTarget::Measurement(id);
    reports::resolve_target(tx, target, Resolution::Upheld, Some(actor.id), now)?;
    audit_row(
        tx,
        now,
        actor,
        audit::MEASUREMENT_INVALIDATE,
        target,
        &reason,
        true,
        json!({}),
    )
}

/// Restores an invalid measurement; its open reports are dismissed. Public
/// `measurement.restore`.
pub fn measurement_restore(
    tx: &Transaction,
    actor: &Actor,
    id: i64,
    reason: &str,
    now: OffsetDateTime,
) -> Result<(), AppError> {
    let reason = check_reason(reason)?;
    set_invalid(tx, id, false)?;
    let target = ReportTarget::Measurement(id);
    reports::resolve_target(tx, target, Resolution::Dismissed, Some(actor.id), now)?;
    audit_row(
        tx,
        now,
        actor,
        audit::MEASUREMENT_RESTORE,
        target,
        &reason,
        true,
        json!({}),
    )
}

// --- Members ---

struct Account {
    github_id: Option<i64>,
    role: String,
    banned: bool,
    deleted: bool,
}

fn account(tx: &Transaction, user: UserId) -> Result<Account, AppError> {
    tx.query_row(
        "SELECT github_id, role, banned, deleted_at IS NOT NULL FROM users WHERE id = ?1",
        [user.0],
        |r| {
            Ok(Account {
                github_id: r.get(0)?,
                role: r.get(1)?,
                banned: r.get(2)?,
                deleted: r.get(3)?,
            })
        },
    )
    .optional()?
    .ok_or_else(AppError::not_found)
}

/// Nobody takes account actions against admins, and moderators take none
/// against moderators.
fn may_act_on(actor: &Actor, account: &Account) -> bool {
    let target_is_admin = account
        .github_id
        .is_some_and(|id| actor.admins.contains(&id));
    !target_is_admin && (actor.is_admin || account.role != "moderator")
}

fn forbidden_target() -> AppError {
    AppError::forbidden("You cannot take account actions against this member.")
}

/// Why a named account was not restricted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skip {
    /// An admin, or a moderator when the actor is not an admin.
    NotPermitted,
    Banned,
    Deleted,
}

/// Restricts a member until the end of `duration`, replacing any earlier
/// restriction, and resolves the open reports on the member as upheld. Public
/// `user.restrict`, with the end time (or `indefinite`) and the ring report,
/// if any. Returns `Err(Skip)` inside `Ok` when the actor may not act on the
/// account or it is banned or deleted, changing nothing; rings use that to
/// list what they skipped.
pub(crate) fn restrict_member(
    tx: &Transaction,
    actor: &Actor,
    user: UserId,
    duration: RestrictFor,
    reason: &str,
    ring_report: Option<i64>,
    now: OffsetDateTime,
) -> Result<Result<(), Skip>, AppError> {
    let acct = account(tx, user)?;
    if !may_act_on(actor, &acct) {
        return Ok(Err(Skip::NotPermitted));
    }
    if acct.deleted {
        return Ok(Err(Skip::Deleted));
    }
    if acct.banned {
        return Ok(Err(Skip::Banned));
    }
    let until = duration.until(now);
    tx.execute(
        "UPDATE users SET restricted_until = ?1 WHERE id = ?2",
        params![until, user.0],
    )?;
    let target = ReportTarget::User(user.0);
    reports::resolve_target(tx, target, Resolution::Upheld, Some(actor.id), now)?;
    let until_text = if duration == RestrictFor::Indefinite {
        "indefinite".to_string()
    } else {
        clock::storage_to_rfc3339(&until).unwrap_or(until)
    };
    let mut details = json!({ "until": until_text });
    if let Some(report) = ring_report {
        details["ring_report"] = json!(report);
    }
    audit_row(
        tx,
        now,
        actor,
        audit::USER_RESTRICT,
        target,
        reason,
        true,
        details,
    )?;
    Ok(Ok(()))
}

/// Restricts a member for 1, 7 or 30 days or indefinitely. 403 for an admin
/// or, unless the actor is an admin, a moderator; 409 for a banned or deleted
/// account.
pub fn user_restrict(
    tx: &Transaction,
    actor: &Actor,
    user: UserId,
    duration: RestrictFor,
    reason: &str,
    now: OffsetDateTime,
) -> Result<(), AppError> {
    let reason = check_reason(reason)?;
    match restrict_member(tx, actor, user, duration, &reason, None, now)? {
        Ok(()) => Ok(()),
        Err(Skip::NotPermitted) => Err(forbidden_target()),
        Err(Skip::Banned) => Err(conflict("A banned account cannot be restricted.")),
        Err(Skip::Deleted) => Err(conflict("A deleted account cannot be restricted.")),
    }
}

/// Lifts a restriction that is in force (409 if there is none). Public
/// `user.unrestrict`; reports are left as they are.
pub fn user_unrestrict(
    tx: &Transaction,
    actor: &Actor,
    user: UserId,
    reason: &str,
    now: OffsetDateTime,
) -> Result<(), AppError> {
    let reason = check_reason(reason)?;
    let acct = account(tx, user)?;
    if !may_act_on(actor, &acct) {
        return Err(forbidden_target());
    }
    let n = tx.execute(
        "UPDATE users SET restricted_until = NULL
         WHERE id = ?1 AND restricted_until IS NOT NULL AND restricted_until > ?2",
        params![user.0, clock::storage(now)],
    )?;
    if n == 0 {
        return Err(conflict("This account is not restricted."));
    }
    audit_row(
        tx,
        now,
        actor,
        audit::USER_UNRESTRICT,
        ReportTarget::User(user.0),
        &reason,
        true,
        json!({}),
    )
}

/// An account that can be banned: not an admin or (for a moderator actor)
/// a moderator, and not a deleted account that has shed its GitHub id.
fn bannable(tx: &Transaction, actor: &Actor, user: UserId) -> Result<Account, AppError> {
    actor.require_admin()?;
    let acct = account(tx, user)?;
    if !may_act_on(actor, &acct) {
        return Err(forbidden_target());
    }
    if acct.deleted && acct.github_id.is_none() {
        return Err(conflict("A deleted account cannot be banned."));
    }
    Ok(acct)
}

/// Bans a member and deletes their sessions; the open reports on the member
/// are upheld. Admins only. Public `user.ban`. 409 if already banned.
pub fn user_ban(
    tx: &Transaction,
    actor: &Actor,
    user: UserId,
    reason: &str,
    now: OffsetDateTime,
) -> Result<(), AppError> {
    let reason = check_reason(reason)?;
    let acct = bannable(tx, actor, user)?;
    if acct.banned {
        return Err(conflict("This account is already banned."));
    }
    ban_now(tx, user)?;
    let target = ReportTarget::User(user.0);
    reports::resolve_target(tx, target, Resolution::Upheld, Some(actor.id), now)?;
    audit_row(
        tx,
        now,
        actor,
        audit::USER_BAN,
        target,
        &reason,
        true,
        json!({}),
    )
}

fn ban_now(tx: &Transaction, user: UserId) -> rusqlite::Result<()> {
    tx.execute("UPDATE users SET banned = 1 WHERE id = ?1", [user.0])?;
    tx.execute("DELETE FROM sessions WHERE user_id = ?1", [user.0])?;
    Ok(())
}

/// Lifts a ban (409 if the account is not banned). Admins only. Public
/// `user.unban`.
pub fn user_unban(
    tx: &Transaction,
    actor: &Actor,
    user: UserId,
    reason: &str,
    now: OffsetDateTime,
) -> Result<(), AppError> {
    let reason = check_reason(reason)?;
    actor.require_admin()?;
    let acct = account(tx, user)?;
    if !may_act_on(actor, &acct) {
        return Err(forbidden_target());
    }
    if !acct.banned {
        return Err(conflict("This account is not banned."));
    }
    tx.execute("UPDATE users SET banned = 0 WHERE id = ?1", [user.0])?;
    audit_row(
        tx,
        now,
        actor,
        audit::USER_UNBAN,
        ReportTarget::User(user.0),
        &reason,
        true,
        json!({}),
    )
}

/// What a spam-ban touched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpamBan {
    pub items_removed: usize,
    pub measurements_invalidated: usize,
}

fn ids(tx: &Transaction, sql: &str, user: UserId) -> rusqlite::Result<Vec<i64>> {
    tx.prepare(sql)?
        .query_map([user.0], |r| r.get(0))?
        .collect()
}

/// Bans a member (a member who is already banned stays banned), removes their
/// visible and hidden posts and comments with reason `spam`, and invalidates
/// their measurements that are valid; deleted items are left alone. The open
/// reports on the member and on everything it touched are upheld. Admins
/// only. Public `user.spam_ban`, with the numbers touched.
pub fn user_spam_ban(
    tx: &Transaction,
    actor: &Actor,
    user: UserId,
    reason: &str,
    now: OffsetDateTime,
) -> Result<SpamBan, AppError> {
    let reason = check_reason(reason)?;
    bannable(tx, actor, user)?;
    ban_now(tx, user)?;
    let post_ids = ids(
        tx,
        "SELECT id FROM posts WHERE author_id = ?1 AND state IN ('visible','hidden') ORDER BY id",
        user,
    )?;
    let comment_ids = ids(
        tx,
        "SELECT id FROM comments WHERE author_id = ?1 AND state IN ('visible','hidden') ORDER BY id",
        user,
    )?;
    let measurement_ids = ids(
        tx,
        "SELECT id FROM measurements WHERE user_id = ?1 AND invalid = 0 ORDER BY id",
        user,
    )?;
    for id in &post_ids {
        posts::remove(tx, *id, "spam")?;
        reports::resolve_target(
            tx,
            ReportTarget::Post(*id),
            Resolution::Upheld,
            Some(actor.id),
            now,
        )?;
    }
    for id in &comment_ids {
        comments::remove(tx, *id, "spam")?;
        reports::resolve_target(
            tx,
            ReportTarget::Comment(*id),
            Resolution::Upheld,
            Some(actor.id),
            now,
        )?;
    }
    for id in &measurement_ids {
        set_invalid(tx, *id, true)?;
        reports::resolve_target(
            tx,
            ReportTarget::Measurement(*id),
            Resolution::Upheld,
            Some(actor.id),
            now,
        )?;
    }
    let target = ReportTarget::User(user.0);
    reports::resolve_target(tx, target, Resolution::Upheld, Some(actor.id), now)?;
    let result = SpamBan {
        items_removed: post_ids.len() + comment_ids.len(),
        measurements_invalidated: measurement_ids.len(),
    };
    audit_row(
        tx,
        now,
        actor,
        audit::USER_SPAM_BAN,
        target,
        &reason,
        true,
        json!({
            "items_removed": result.items_removed,
            "measurements_invalidated": result.measurements_invalidated,
        }),
    )?;
    Ok(result)
}

fn role_text(role: Role) -> &'static str {
    match role {
        Role::None => "none",
        Role::Moderator => "moderator",
    }
}

/// Appoints a moderator or removes one (`Role::None`). Admins only; never
/// against an admin; 409 for a deleted account or no change. Public
/// `user.role`, with the old and new role.
pub fn user_set_role(
    tx: &Transaction,
    actor: &Actor,
    user: UserId,
    role: Role,
    reason: &str,
    now: OffsetDateTime,
) -> Result<(), AppError> {
    let reason = check_reason(reason)?;
    actor.require_admin()?;
    let acct = account(tx, user)?;
    if !may_act_on(actor, &acct) {
        return Err(forbidden_target());
    }
    if acct.deleted {
        return Err(conflict("A deleted account cannot take a role."));
    }
    let old = if acct.role == "moderator" {
        Role::Moderator
    } else {
        Role::None
    };
    if old == role {
        return Err(conflict("The member already has this role."));
    }
    tx.execute(
        "UPDATE users SET role = ?1 WHERE id = ?2",
        params![role_text(role), user.0],
    )?;
    audit_row(
        tx,
        now,
        actor,
        audit::USER_ROLE,
        ReportTarget::User(user.0),
        &reason,
        true,
        json!({ "old": role_text(old), "new": role_text(role) }),
    )
}

// --- Reports ---

/// What a dismissal did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dismissal {
    pub reports: usize,
    /// Whether the target was a hidden item, which became visible again.
    pub restored: bool,
}

/// Dismisses the open reports on a target (404 if there are none). On a
/// hidden item it makes the item visible again and is recorded as a public
/// restore; anything else is a non-public `report.dismiss` with the number of
/// reports.
pub fn dismiss(
    tx: &Transaction,
    actor: &Actor,
    target: ReportTarget,
    reason: &str,
    now: OffsetDateTime,
) -> Result<Dismissal, AppError> {
    let reason = check_reason(reason)?;
    let open = reports::open_count(tx, target)?;
    if open == 0 {
        return Err(AppError::not_found());
    }
    let hidden = reports::describe(tx, target)?
        .and_then(|d| d.state)
        .is_some_and(|s| s == "hidden");
    let reports_resolved =
        reports::resolve_target(tx, target, Resolution::Dismissed, Some(actor.id), now)?;
    if hidden {
        let action = match target {
            ReportTarget::Post(id) => {
                posts::restore(tx, id)?;
                audit::POST_RESTORE
            }
            ReportTarget::Comment(id) => {
                comments::restore(tx, id)?;
                audit::COMMENT_RESTORE
            }
            _ => unreachable!("only posts and comments have a state"),
        };
        audit_row(tx, now, actor, action, target, &reason, true, json!({}))?;
    } else {
        audit_row(
            tx,
            now,
            actor,
            audit::REPORT_DISMISS,
            target,
            &reason,
            false,
            json!({ "reports": reports_resolved }),
        )?;
    }
    Ok(Dismissal {
        reports: reports_resolved,
        restored: hidden,
    })
}

// --- The public log ---

/// Who acted, as the public log shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogActor {
    System,
    Member(String),
    /// An account that has been deleted.
    Deleted,
}

/// What a log row is about, as a page links it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogTarget {
    /// The action has no target (a sync).
    None,
    /// The target no longer exists, or is an account that was deleted: the
    /// page reads `[deleted]`.
    Deleted,
    Post {
        post_id: i64,
    },
    /// Links to the post, at anchor `c<comment_id>`.
    Comment {
        post_id: i64,
        comment_id: i64,
    },
    /// Links to the pack's post (if it has one), at anchor `m<measurement_id>`.
    Measurement {
        post_id: Option<i64>,
        measurement_id: i64,
    },
    Member {
        login: String,
    },
    Pack {
        name: String,
        post_id: Option<i64>,
    },
}

/// A row of `/moderation`.
#[derive(Debug, Clone, PartialEq)]
pub struct LogRow {
    pub id: i64,
    pub at: OffsetDateTime,
    pub action: String,
    pub target: LogTarget,
    pub reason: String,
    pub actor: LogActor,
    /// Ids, counts, times and rule names, never free text.
    pub details: Value,
}

fn log_actor(conn: &Connection, actor_id: Option<i64>) -> rusqlite::Result<LogActor> {
    let Some(id) = actor_id else {
        return Ok(LogActor::System);
    };
    let login: Option<(String, bool)> = conn
        .query_row(
            "SELECT login, deleted_at IS NOT NULL FROM users WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(match login {
        Some((login, false)) => LogActor::Member(login),
        _ => LogActor::Deleted,
    })
}

fn log_target(
    conn: &Connection,
    action: &str,
    kind: Option<&str>,
    id: Option<&str>,
) -> rusqlite::Result<LogTarget> {
    let (Some(kind), Some(id)) = (kind, id) else {
        return Ok(LogTarget::None);
    };
    let number: Option<i64> = id.parse().ok();
    let pack_post = |name: &str| -> rusqlite::Result<Option<i64>> {
        conn.query_row("SELECT id FROM posts WHERE pack = ?1", [name], |r| r.get(0))
            .optional()
    };
    Ok(match (kind, number) {
        ("post", Some(n)) => {
            let exists: bool = conn.query_row(
                "SELECT EXISTS (SELECT 1 FROM posts WHERE id = ?1)",
                [n],
                |r| r.get(0),
            )?;
            if exists {
                LogTarget::Post { post_id: n }
            } else {
                LogTarget::Deleted
            }
        }
        ("comment", Some(n)) => conn
            .query_row("SELECT post_id FROM comments WHERE id = ?1", [n], |r| {
                r.get(0)
            })
            .optional()?
            .map_or(LogTarget::Deleted, |post_id| LogTarget::Comment {
                post_id,
                comment_id: n,
            }),
        ("measurement", Some(n)) => {
            let pack: Option<String> = conn
                .query_row("SELECT pack FROM measurements WHERE id = ?1", [n], |r| {
                    r.get(0)
                })
                .optional()?;
            match pack {
                None => LogTarget::Deleted,
                Some(pack) => LogTarget::Measurement {
                    post_id: pack_post(&pack)?,
                    measurement_id: n,
                },
            }
        }
        // A deletion's row never names the member.
        ("user", Some(n)) if action != audit::USER_DELETE => conn
            .query_row(
                "SELECT login FROM users WHERE id = ?1 AND deleted_at IS NULL",
                [n],
                |r| r.get(0),
            )
            .optional()?
            .map_or(LogTarget::Deleted, |login| LogTarget::Member { login }),
        ("pack", _) => LogTarget::Pack {
            name: id.to_string(),
            post_id: pack_post(id)?,
        },
        _ => LogTarget::Deleted,
    })
}

/// The public audit rows, newest first, 50 a page. Never reporters, report
/// notes or snapshots: the log table holds none.
pub fn public_log(conn: &Connection, page: usize) -> rusqlite::Result<Page<LogRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, at, actor_id, action, target_type, target_id, reason, details
         FROM audit_log WHERE public = 1
         ORDER BY at DESC, id DESC LIMIT ?1 OFFSET ?2",
    )?;
    let raw: Vec<_> = stmt
        .query_map(
            params![
                (policy::LIST_PAGE_SIZE + 1) as i64,
                page::offset(page, policy::LIST_PAGE_SIZE) as i64
            ],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<i64>>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, String>(7)?,
                ))
            },
        )?
        .collect::<Result<_, _>>()?;
    let paged = Page::from_overfetched(raw, policy::LIST_PAGE_SIZE);
    let mut rows = Vec::new();
    for (id, at, actor_id, action, kind, target_id, reason, details) in paged.rows {
        rows.push(LogRow {
            id,
            at: clock::parse_storage(&at).unwrap_or(OffsetDateTime::UNIX_EPOCH),
            target: log_target(conn, &action, kind.as_deref(), target_id.as_deref())?,
            actor: log_actor(conn, actor_id)?,
            action,
            reason,
            details: serde_json::from_str(&details).unwrap_or(Value::Null),
        });
    }
    Ok(Page {
        rows,
        more: paged.more,
    })
}

// --- A member, for moderators ---

/// An audit row about a member, as `/mod/u/{login}` lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct AuditRow {
    pub id: i64,
    pub at: OffsetDateTime,
    pub actor: LogActor,
    pub action: String,
    pub target_type: Option<String>,
    pub target_id: Option<String>,
    pub reason: String,
    pub public: bool,
    pub details: Value,
}

/// What `/mod/u/{login}` shows.
#[derive(Debug, Clone, PartialEq)]
pub struct MemberView {
    pub id: UserId,
    pub login: String,
    pub standing: Standing,
    /// `none` or `moderator`; an admin is told apart by `is_admin`.
    pub role: Role,
    pub is_admin: bool,
    pub deleted: bool,
    pub created_at: OffsetDateTime,
    pub restricted_until: Option<OffsetDateTime>,
    pub banned: bool,
    pub posts: i64,
    pub comments: i64,
    pub votes: i64,
    pub measurements: i64,
    pub reports_filed: i64,
    /// Reports of any status on the member, their posts, their comments and
    /// their measurements.
    pub reports_against: i64,
    /// The audit rows whose target is the member or one of their items,
    /// newest first, at most 50.
    pub audit: Vec<AuditRow>,
}

/// The member with this login (case ignored; a deleted account's placeholder
/// login too), or `None`.
pub fn member_view(
    conn: &Connection,
    login: &str,
    admins: &[i64],
    now: OffsetDateTime,
) -> rusqlite::Result<Option<MemberView>> {
    let id: Option<i64> = conn
        .query_row("SELECT id FROM users WHERE login = ?1", [login], |r| {
            r.get(0)
        })
        .optional()?;
    let Some(id) = id else {
        return Ok(None);
    };
    let Some(member) = users::load_member(conn, UserId(id))? else {
        return Ok(None);
    };
    let deleted: bool = conn.query_row(
        "SELECT deleted_at IS NOT NULL FROM users WHERE id = ?1",
        [id],
        |r| r.get(0),
    )?;
    let count = |sql: &str| -> rusqlite::Result<i64> { conn.query_row(sql, [id], |r| r.get(0)) };
    let reports_against = count(
        "SELECT COUNT(*) FROM reports r WHERE
            (r.target_type = 'user' AND r.target_id = ?1)
         OR (r.target_type = 'post' AND r.target_id IN (SELECT id FROM posts WHERE author_id = ?1))
         OR (r.target_type = 'comment'
             AND r.target_id IN (SELECT id FROM comments WHERE author_id = ?1))
         OR (r.target_type = 'measurement'
             AND r.target_id IN (SELECT id FROM measurements WHERE user_id = ?1))",
    )?;
    let mut stmt = conn.prepare(
        "SELECT a.id, a.at, a.actor_id, a.action, a.target_type, a.target_id, a.reason, a.public,
                a.details
         FROM audit_log a
         WHERE (a.target_type = 'user' AND a.target_id = CAST(?1 AS TEXT))
            OR (a.target_type = 'post' AND EXISTS
                  (SELECT 1 FROM posts p WHERE p.author_id = ?1 AND CAST(p.id AS TEXT) = a.target_id))
            OR (a.target_type = 'comment' AND EXISTS
                  (SELECT 1 FROM comments c WHERE c.author_id = ?1 AND CAST(c.id AS TEXT) = a.target_id))
            OR (a.target_type = 'measurement' AND EXISTS
                  (SELECT 1 FROM measurements m WHERE m.user_id = ?1 AND CAST(m.id AS TEXT) = a.target_id))
         ORDER BY a.at DESC, a.id DESC LIMIT ?2",
    )?;
    let raw: Vec<_> = stmt
        .query_map(params![id, policy::LIST_PAGE_SIZE as i64], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<i64>>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, bool>(7)?,
                r.get::<_, String>(8)?,
            ))
        })?
        .collect::<Result<_, _>>()?;
    let mut audit = Vec::new();
    for (row_id, at, actor_id, action, target_type, target_id, reason, public, details) in raw {
        audit.push(AuditRow {
            id: row_id,
            at: clock::parse_storage(&at).unwrap_or(OffsetDateTime::UNIX_EPOCH),
            actor: log_actor(conn, actor_id)?,
            action,
            target_type,
            target_id,
            reason,
            public,
            details: serde_json::from_str(&details).unwrap_or(Value::Null),
        });
    }
    Ok(Some(MemberView {
        id: member.id,
        standing: users::standing(&member, now),
        is_admin: member.github_id.is_some_and(|g| admins.contains(&g)),
        role: member.role,
        login: member.login,
        deleted,
        created_at: member.created_at,
        restricted_until: member.restricted_until,
        banned: member.banned,
        posts: count("SELECT COUNT(*) FROM posts WHERE author_id = ?1")?,
        comments: count("SELECT COUNT(*) FROM comments WHERE author_id = ?1")?,
        votes: count("SELECT COUNT(*) FROM votes WHERE user_id = ?1")?,
        measurements: count("SELECT COUNT(*) FROM measurements WHERE user_id = ?1")?,
        reports_filed: count("SELECT COUNT(*) FROM reports WHERE reporter_id = ?1")?,
        reports_against,
        audit,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::posts::testkit::*;
    use time::macros::datetime;

    const NOW: OffsetDateTime = datetime!(2026-11-08 10:00:00 UTC);

    fn old() -> OffsetDateTime {
        NOW - Duration::days(400)
    }

    struct World {
        _dir: tempfile::TempDir,
        conn: Connection,
        admin: i64,
        moderator: i64,
        other_moderator: i64,
        ada: i64,
        post: i64,
        comment: i64,
        admins: Vec<i64>,
    }

    fn world() -> World {
        let (dir, conn) = fresh();
        let admin = user(&conn, "root", old(), old());
        let moderator = user(&conn, "mod", old(), old());
        let other_moderator = user(&conn, "mod2", old(), old());
        let ada = user(&conn, "ada", old(), old());
        conn.execute(
            "UPDATE users SET role = 'moderator' WHERE id IN (?1, ?2)",
            [moderator, other_moderator],
        )
        .expect("roles");
        let admins = vec![member(&conn, admin).github_id.expect("github id")];
        let post = post(&conn, ada, "ada's post", NOW);
        let comment = comment(&conn, post, ada, NOW);
        World {
            _dir: dir,
            conn,
            admin,
            moderator,
            other_moderator,
            ada,
            post,
            comment,
            admins,
        }
    }

    impl World {
        fn actor(&self, id: i64) -> Actor<'_> {
            Actor::new(&member(&self.conn, id), &self.admins).expect("actor")
        }

        fn report(&self, who: &str, target: ReportTarget) -> i64 {
            let id = user(&self.conn, who, old(), old());
            let m = member(&self.conn, id);
            let tx = self.conn.unchecked_transaction().expect("tx");
            let rid = reports::file(&tx, &m, target, "spam", "note", NOW).expect("report");
            tx.commit().expect("commit");
            rid
        }

        /// Runs `f` in a transaction and commits it if it succeeds.
        fn run<T>(
            &self,
            f: impl FnOnce(&Transaction) -> Result<T, AppError>,
        ) -> Result<T, AppError> {
            let tx = self.conn.unchecked_transaction().expect("tx");
            let out = f(&tx)?;
            tx.commit().expect("commit");
            Ok(out)
        }

        fn statuses(&self) -> Vec<(String, Option<i64>)> {
            self.conn
                .prepare("SELECT status, resolved_by FROM reports ORDER BY id")
                .expect("prepare")
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .expect("query")
                .collect::<Result<_, _>>()
                .expect("rows")
        }

        #[allow(clippy::type_complexity)]
        fn audit(&self) -> Vec<(Option<i64>, String, String, String, String, bool, String)> {
            self.conn
                .prepare(
                    "SELECT actor_id, action, target_type, target_id, reason, public, details
                     FROM audit_log ORDER BY id",
                )
                .expect("prepare")
                .query_map([], |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                    ))
                })
                .expect("query")
                .collect::<Result<_, _>>()
                .expect("rows")
        }

        fn scalar(&self, sql: &str, id: i64) -> String {
            self.conn
                .query_row(sql, [id], |r| r.get::<_, rusqlite::types::Value>(0))
                .map(|v| match v {
                    rusqlite::types::Value::Text(t) => t,
                    rusqlite::types::Value::Integer(i) => i.to_string(),
                    rusqlite::types::Value::Null => "null".into(),
                    other => format!("{other:?}"),
                })
                .expect("scalar")
        }
    }

    #[test]
    fn only_moderators_and_admins_are_actors() {
        let w = world();
        assert!(Actor::new(&member(&w.conn, w.ada), &w.admins).is_err());
        assert!(!w.actor(w.moderator).is_admin);
        assert!(w.actor(w.admin).is_admin);
        assert_eq!(
            Actor::new(&member(&w.conn, w.ada), &w.admins)
                .expect_err("ada")
                .status,
            StatusCode::FORBIDDEN
        );
    }

    #[test]
    fn reasons_are_codes_and_never_free_text() {
        let w = world();
        let actor = w.actor(w.moderator);
        for bad in ["", "because I said so", "SPAM"] {
            let e = w
                .run(|tx| post_remove(tx, &actor, w.post, bad, NOW))
                .expect_err(bad);
            assert_eq!(e.status, StatusCode::UNPROCESSABLE_ENTITY);
        }
        assert_eq!(
            w.scalar("SELECT state FROM posts WHERE id = ?1", w.post),
            "visible"
        );
        assert!(w.audit().is_empty());
        for reason in REASONS {
            w.run(|tx| post_lock(tx, &actor, w.post, reason, NOW))
                .unwrap_or_else(|e| panic!("{reason}: {e}"));
            w.run(|tx| post_unlock(tx, &actor, w.post, reason, NOW))
                .expect("unlock");
        }
    }

    #[test]
    fn post_remove_and_restore_resolve_reports_and_write_one_public_row() {
        let w = world();
        let actor = w.actor(w.moderator);
        w.report("r1", ReportTarget::Post(w.post));
        w.report("r2", ReportTarget::Post(w.post));
        w.run(|tx| post_remove(tx, &actor, w.post, "spam", NOW))
            .expect("remove");
        assert_eq!(
            w.scalar("SELECT removed_reason FROM posts WHERE id = ?1", w.post),
            "spam"
        );
        assert_eq!(
            w.statuses(),
            vec![
                ("upheld".to_string(), Some(w.moderator)),
                ("upheld".to_string(), Some(w.moderator))
            ]
        );
        assert_eq!(
            w.audit(),
            vec![(
                Some(w.moderator),
                "post.remove".into(),
                "post".into(),
                w.post.to_string(),
                "spam".into(),
                true,
                "{}".into()
            )]
        );
        // A state error changes nothing and writes no row.
        assert_eq!(
            w.run(|tx| post_remove(tx, &actor, w.post, "spam", NOW))
                .expect_err("twice")
                .status,
            StatusCode::CONFLICT
        );
        assert_eq!(w.audit().len(), 1);
        // Restore: a report filed on a hidden post earlier is dismissed.
        w.run(|tx| post_restore(tx, &actor, w.post, "other", NOW))
            .expect("restore");
        assert_eq!(
            w.scalar("SELECT state FROM posts WHERE id = ?1", w.post),
            "visible"
        );
        assert_eq!(w.audit().len(), 2);
        assert_eq!(w.audit()[1].1, "post.restore");
        assert!(w.audit()[1].5);
        // Restoring resolves open reports as dismissed.
        state(&w.conn, "posts", w.post, "removed");
        w.conn
            .execute(
                "INSERT INTO reports (system, target_type, target_id, reason, created_at)
                 VALUES (1, 'post', ?1, 'misleading', ?2)",
                params![w.post, clock::storage(NOW)],
            )
            .expect("system report");
        w.run(|tx| post_restore(tx, &actor, w.post, "other", NOW))
            .expect("restore");
        assert_eq!(w.statuses().last().expect("last").0, "dismissed");
    }

    #[test]
    fn lock_and_unlock_each_write_one_row_and_leave_reports() {
        let w = world();
        let actor = w.actor(w.moderator);
        w.report("r1", ReportTarget::Post(w.post));
        w.run(|tx| post_lock(tx, &actor, w.post, "off-topic", NOW))
            .expect("lock");
        assert_eq!(
            w.scalar("SELECT locked FROM posts WHERE id = ?1", w.post),
            "1"
        );
        assert_eq!(
            w.run(|tx| post_lock(tx, &actor, w.post, "other", NOW))
                .expect_err("again")
                .status,
            StatusCode::CONFLICT
        );
        w.run(|tx| post_unlock(tx, &actor, w.post, "other", NOW))
            .expect("unlock");
        assert_eq!(
            w.run(|tx| post_unlock(tx, &actor, w.post, "other", NOW))
                .expect_err("again")
                .status,
            StatusCode::CONFLICT
        );
        let rows = w.audit();
        assert_eq!(rows.len(), 2);
        assert_eq!(
            (rows[0].1.as_str(), rows[1].1.as_str()),
            ("post.lock", "post.unlock")
        );
        assert!(rows.iter().all(|r| r.5));
        assert_eq!(w.statuses(), vec![("open".to_string(), None)]);
        assert_eq!(
            w.run(|tx| post_lock(tx, &actor, 999, "other", NOW))
                .expect_err("missing")
                .status,
            StatusCode::NOT_FOUND
        );
    }

    #[test]
    fn comment_remove_and_restore() {
        let w = world();
        let actor = w.actor(w.moderator);
        w.report("r1", ReportTarget::Comment(w.comment));
        w.run(|tx| comment_remove(tx, &actor, w.comment, "abuse", NOW))
            .expect("remove");
        assert_eq!(
            w.scalar(
                "SELECT state || removed_reason FROM comments WHERE id = ?1",
                w.comment
            ),
            "removedabuse"
        );
        assert_eq!(w.statuses()[0].0, "upheld");
        w.run(|tx| comment_restore(tx, &actor, w.comment, "other", NOW))
            .expect("restore");
        assert_eq!(
            w.scalar("SELECT state FROM comments WHERE id = ?1", w.comment),
            "visible"
        );
        let rows = w.audit();
        assert_eq!(
            rows.iter().map(|r| r.1.as_str()).collect::<Vec<_>>(),
            vec!["comment.remove", "comment.restore"]
        );
        assert!(rows.iter().all(|r| r.2 == "comment" && r.5));
    }

    fn add_measurement(w: &World, user: i64) -> i64 {
        pack_post(&w.conn, "glossary", 5, "[]", NOW);
        w.conn
            .execute(
                "INSERT OR IGNORE INTO pack_contents (pack, pack_hash, version, commit_id, first_seen)
                 VALUES ('glossary', 'h', '1', 'c', ?1)",
                [clock::storage(NOW)],
            )
            .expect("content");
        w.conn
            .execute(
                "INSERT INTO measurements (pack, pack_hash, user_id, eval_set, planted, kept, runs,
                    corrected_without, damaged_without, corrected_with, damaged_with, hippocamp, model,
                    created_at, updated_at)
                 VALUES ('glossary', 'h', ?1, 's', 5, 5, 3, 1, 2, 3, 4, '0.1.0', 'm', ?2, ?2)",
                params![user, clock::storage(NOW)],
            )
            .expect("measurement");
        w.conn.last_insert_rowid()
    }

    #[test]
    fn measurement_invalidate_and_restore() {
        let w = world();
        let actor = w.actor(w.moderator);
        let m = add_measurement(&w, w.ada);
        w.report("r1", ReportTarget::Measurement(m));
        w.run(|tx| measurement_invalidate(tx, &actor, m, "misleading", NOW))
            .expect("invalidate");
        assert_eq!(
            w.scalar("SELECT invalid FROM measurements WHERE id = ?1", m),
            "1"
        );
        assert_eq!(w.statuses()[0].0, "upheld");
        assert_eq!(
            w.run(|tx| measurement_invalidate(tx, &actor, m, "spam", NOW))
                .expect_err("twice")
                .status,
            StatusCode::CONFLICT
        );
        w.run(|tx| measurement_restore(tx, &actor, m, "other", NOW))
            .expect("restore");
        assert_eq!(
            w.scalar("SELECT invalid FROM measurements WHERE id = ?1", m),
            "0"
        );
        assert_eq!(
            w.run(|tx| measurement_restore(tx, &actor, m, "other", NOW))
                .expect_err("not invalid")
                .status,
            StatusCode::CONFLICT
        );
        assert_eq!(
            w.run(|tx| measurement_restore(tx, &actor, 999, "other", NOW))
                .expect_err("missing")
                .status,
            StatusCode::NOT_FOUND
        );
        let rows = w.audit();
        assert_eq!(
            rows.iter()
                .map(|r| (r.1.as_str(), r.2.as_str(), r.5))
                .collect::<Vec<_>>(),
            vec![
                ("measurement.invalidate", "measurement", true),
                ("measurement.restore", "measurement", true)
            ]
        );
    }

    #[test]
    fn a_restriction_replaces_an_earlier_one_and_resolves_reports() {
        let w = world();
        let actor = w.actor(w.moderator);
        w.report("r1", ReportTarget::User(w.ada));
        w.run(|tx| user_restrict(tx, &actor, UserId(w.ada), RestrictFor::Month, "abuse", NOW))
            .expect("30d");
        let until = |w: &World| member(&w.conn, w.ada).restricted_until;
        assert_eq!(until(&w), Some(NOW + Duration::days(30)));
        assert_eq!(w.statuses()[0], ("upheld".to_string(), Some(w.moderator)));
        // A shorter one replaces it.
        let later = NOW + Duration::hours(1);
        w.run(|tx| user_restrict(tx, &actor, UserId(w.ada), RestrictFor::Day, "abuse", later))
            .expect("1d");
        assert_eq!(until(&w), Some(later + Duration::days(1)));
        w.run(|tx| user_restrict(tx, &actor, UserId(w.ada), RestrictFor::Week, "spam", later))
            .expect("7d");
        assert_eq!(until(&w), Some(later + Duration::days(7)));
        w.run(|tx| {
            user_restrict(
                tx,
                &actor,
                UserId(w.ada),
                RestrictFor::Indefinite,
                "spam",
                later,
            )
        })
        .expect("indefinite");
        assert_eq!(
            w.scalar("SELECT restricted_until FROM users WHERE id = ?1", w.ada),
            policy::INDEFINITE
        );
        let rows = w.audit();
        assert_eq!(rows.len(), 4);
        assert!(rows
            .iter()
            .all(|r| r.1 == "user.restrict" && r.5 && r.2 == "user"));
        assert_eq!(rows[0].6, r#"{"until":"2026-12-08T10:00:00Z"}"#);
        assert_eq!(rows[3].6, r#"{"until":"indefinite"}"#);
        assert_eq!(rows[0].4, "abuse");
    }

    #[test]
    fn restrict_durations_parse() {
        assert_eq!(RestrictFor::parse("1d"), Some(RestrictFor::Day));
        assert_eq!(RestrictFor::parse("7d"), Some(RestrictFor::Week));
        assert_eq!(RestrictFor::parse("30d"), Some(RestrictFor::Month));
        assert_eq!(
            RestrictFor::parse("indefinite"),
            Some(RestrictFor::Indefinite)
        );
        assert_eq!(RestrictFor::parse("2d"), None);
    }

    #[test]
    fn banned_and_deleted_accounts_cannot_be_restricted() {
        let w = world();
        let actor = w.actor(w.moderator);
        w.conn
            .execute("UPDATE users SET banned = 1 WHERE id = ?1", [w.ada])
            .expect("ban");
        let e = w
            .run(|tx| user_restrict(tx, &actor, UserId(w.ada), RestrictFor::Day, "spam", NOW))
            .expect_err("banned");
        assert_eq!(e.status, StatusCode::CONFLICT);
        w.conn
            .execute(
                "UPDATE users SET banned = 0, deleted_at = 'x' WHERE id = ?1",
                [w.ada],
            )
            .expect("delete");
        assert_eq!(
            w.run(|tx| user_restrict(tx, &actor, UserId(w.ada), RestrictFor::Day, "spam", NOW))
                .expect_err("deleted")
                .status,
            StatusCode::CONFLICT
        );
        assert_eq!(
            w.run(|tx| user_restrict(tx, &actor, UserId(999), RestrictFor::Day, "spam", NOW))
                .expect_err("missing")
                .status,
            StatusCode::NOT_FOUND
        );
        assert!(w.audit().is_empty());
    }

    #[test]
    fn nobody_acts_on_an_admin_and_moderators_not_on_moderators() {
        let w = world();
        let moderator = w.actor(w.moderator);
        let admin = w.actor(w.admin);
        let restrict = |actor: &Actor, who: i64| {
            w.run(|tx| user_restrict(tx, actor, UserId(who), RestrictFor::Day, "spam", NOW))
        };
        for actor in [&moderator, &admin] {
            assert_eq!(
                restrict(actor, w.admin).expect_err("admin").status,
                StatusCode::FORBIDDEN
            );
        }
        assert_eq!(
            restrict(&moderator, w.other_moderator)
                .expect_err("mod")
                .status,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            restrict(&moderator, w.moderator).expect_err("self").status,
            StatusCode::FORBIDDEN
        );
        // An admin may restrict a moderator; a moderator may restrict a member.
        restrict(&admin, w.other_moderator).expect("admin on moderator");
        restrict(&moderator, w.ada).expect("moderator on member");
        // Unrestrict follows the same rule.
        assert_eq!(
            w.run(|tx| user_unrestrict(tx, &moderator, UserId(w.other_moderator), "other", NOW))
                .expect_err("mod")
                .status,
            StatusCode::FORBIDDEN
        );
        assert_eq!(w.audit().len(), 2);
    }

    #[test]
    fn unrestrict_lifts_a_restriction_in_force() {
        let w = world();
        let actor = w.actor(w.moderator);
        assert_eq!(
            w.run(|tx| user_unrestrict(tx, &actor, UserId(w.ada), "other", NOW))
                .expect_err("none")
                .status,
            StatusCode::CONFLICT
        );
        w.run(|tx| user_restrict(tx, &actor, UserId(w.ada), RestrictFor::Week, "spam", NOW))
            .expect("restrict");
        // A restriction that has ended is not in force.
        assert!(w
            .run(|tx| user_unrestrict(tx, &actor, UserId(w.ada), "other", NOW + Duration::days(8)))
            .is_err());
        w.run(|tx| user_unrestrict(tx, &actor, UserId(w.ada), "other", NOW))
            .expect("lift");
        assert_eq!(member(&w.conn, w.ada).restricted_until, None);
        let rows = w.audit();
        assert_eq!(
            rows.last().map(|r| (r.1.as_str(), r.5)),
            Some(("user.unrestrict", true))
        );
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn ban_deletes_sessions_and_unban_lifts_it() {
        let w = world();
        let admin = w.actor(w.admin);
        let moderator = w.actor(w.moderator);
        w.report("r1", ReportTarget::User(w.ada));
        w.conn
            .execute(
                "INSERT INTO sessions (token_hash, user_id, csrf, created_at, expires_at)
                 VALUES ('h1', ?1, 'c', 'a', 'b'), ('h2', ?1, 'c', 'a', 'b'), ('h3', ?2, 'c', 'a', 'b')",
                [w.ada, w.moderator],
            )
            .expect("sessions");
        assert_eq!(
            w.run(|tx| user_ban(tx, &moderator, UserId(w.ada), "spam", NOW))
                .expect_err("not admin")
                .status,
            StatusCode::FORBIDDEN
        );
        w.run(|tx| user_ban(tx, &admin, UserId(w.ada), "spam", NOW))
            .expect("ban");
        assert!(member(&w.conn, w.ada).banned);
        assert_eq!(
            w.conn
                .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get::<_, i64>(0))
                .expect("count"),
            1,
            "only ada's sessions go"
        );
        assert_eq!(w.statuses()[0].0, "upheld");
        assert_eq!(
            w.run(|tx| user_ban(tx, &admin, UserId(w.ada), "spam", NOW))
                .expect_err("twice")
                .status,
            StatusCode::CONFLICT
        );
        assert_eq!(
            w.run(|tx| user_unban(tx, &moderator, UserId(w.ada), "other", NOW))
                .expect_err("not admin")
                .status,
            StatusCode::FORBIDDEN
        );
        w.run(|tx| user_unban(tx, &admin, UserId(w.ada), "other", NOW))
            .expect("unban");
        assert!(!member(&w.conn, w.ada).banned);
        assert_eq!(
            w.run(|tx| user_unban(tx, &admin, UserId(w.ada), "other", NOW))
                .expect_err("not banned")
                .status,
            StatusCode::CONFLICT
        );
        assert_eq!(
            w.run(|tx| user_ban(tx, &admin, UserId(w.admin), "spam", NOW))
                .expect_err("admin")
                .status,
            StatusCode::FORBIDDEN
        );
        let rows = w.audit();
        assert_eq!(
            rows.iter().map(|r| (r.1.as_str(), r.5)).collect::<Vec<_>>(),
            vec![("user.ban", true), ("user.unban", true)]
        );
    }

    #[test]
    fn a_deleted_account_that_shed_its_id_cannot_be_banned() {
        let w = world();
        let admin = w.actor(w.admin);
        w.conn
            .execute(
                "UPDATE users SET deleted_at = 'x', github_id = NULL WHERE id = ?1",
                [w.ada],
            )
            .expect("delete");
        assert_eq!(
            w.run(|tx| user_ban(tx, &admin, UserId(w.ada), "spam", NOW))
                .expect_err("deleted")
                .status,
            StatusCode::CONFLICT
        );
        // One under a sanction keeps its id and can be.
        w.conn
            .execute("UPDATE users SET github_id = 4242 WHERE id = ?1", [w.ada])
            .expect("id");
        w.run(|tx| user_ban(tx, &admin, UserId(w.ada), "spam", NOW))
            .expect("ban");
    }

    #[test]
    fn spam_ban_removes_items_invalidates_measurements_and_leaves_deleted_ones() {
        let w = world();
        let admin = w.actor(w.admin);
        let p_hidden = post(&w.conn, w.ada, "hidden", NOW);
        let p_deleted = post(&w.conn, w.ada, "deleted", NOW);
        let p_removed = post(&w.conn, w.ada, "removed", NOW);
        let c_hidden = comment(&w.conn, w.post, w.ada, NOW);
        let c_deleted = comment(&w.conn, w.post, w.ada, NOW);
        state(&w.conn, "posts", p_hidden, "hidden");
        state(&w.conn, "posts", p_deleted, "deleted");
        state(&w.conn, "posts", p_removed, "removed");
        state(&w.conn, "comments", c_deleted, "deleted");
        let m = add_measurement(&w, w.ada);
        // Someone else's items stay.
        let bob = user(&w.conn, "bob", old(), old());
        let bobs = post(&w.conn, bob, "bob", NOW);
        w.report("r1", ReportTarget::Post(w.post));
        w.report("r2", ReportTarget::Comment(c_hidden));
        w.report("r3", ReportTarget::Measurement(m));
        w.report("r4", ReportTarget::User(w.ada));
        w.report("r5", ReportTarget::Post(bobs));
        state(&w.conn, "comments", c_hidden, "hidden");
        let out = w
            .run(|tx| user_spam_ban(tx, &admin, UserId(w.ada), "spam", NOW))
            .expect("spam ban");
        // post, p_hidden, comment, c_hidden removed; one measurement.
        assert_eq!(
            out,
            SpamBan {
                items_removed: 4,
                measurements_invalidated: 1
            }
        );
        let state_of = |table: &str, id: i64| -> String {
            w.scalar(&format!("SELECT state || ':' || COALESCE(removed_reason, '') FROM {table} WHERE id = ?1"), id)
        };
        assert_eq!(state_of("posts", w.post), "removed:spam");
        assert_eq!(state_of("posts", p_hidden), "removed:spam");
        assert_eq!(state_of("posts", p_deleted), "deleted:");
        assert_eq!(state_of("posts", p_removed), "removed:");
        assert_eq!(state_of("comments", w.comment), "removed:spam");
        assert_eq!(state_of("comments", c_hidden), "removed:spam");
        assert_eq!(state_of("comments", c_deleted), "deleted:");
        assert_eq!(state_of("posts", bobs), "visible:");
        assert_eq!(
            w.scalar("SELECT invalid FROM measurements WHERE id = ?1", m),
            "1"
        );
        assert!(member(&w.conn, w.ada).banned);
        let statuses: Vec<String> = w.statuses().into_iter().map(|s| s.0).collect();
        assert_eq!(
            statuses,
            vec!["upheld", "upheld", "upheld", "upheld", "open"]
        );
        // One audit row for the whole action.
        let rows = w.audit();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].1, "user.spam_ban");
        assert!(rows[0].5);
        assert_eq!(
            rows[0].6,
            r#"{"items_removed":4,"measurements_invalidated":1}"#
        );
        // Not for moderators.
        let moderator = w.actor(w.moderator);
        assert!(w
            .run(|tx| user_spam_ban(tx, &moderator, UserId(bob), "spam", NOW))
            .is_err());
    }

    #[test]
    fn roles_are_appointed_by_admins_with_old_and_new_in_the_row() {
        let w = world();
        let admin = w.actor(w.admin);
        let moderator = w.actor(w.moderator);
        assert_eq!(
            w.run(|tx| user_set_role(tx, &moderator, UserId(w.ada), Role::Moderator, "other", NOW))
                .expect_err("not admin")
                .status,
            StatusCode::FORBIDDEN
        );
        w.run(|tx| user_set_role(tx, &admin, UserId(w.ada), Role::Moderator, "other", NOW))
            .expect("appoint");
        assert_eq!(member(&w.conn, w.ada).role, Role::Moderator);
        assert_eq!(
            w.run(|tx| user_set_role(tx, &admin, UserId(w.ada), Role::Moderator, "other", NOW))
                .expect_err("no change")
                .status,
            StatusCode::CONFLICT
        );
        w.run(|tx| user_set_role(tx, &admin, UserId(w.ada), Role::None, "other", NOW))
            .expect("remove");
        assert_eq!(member(&w.conn, w.ada).role, Role::None);
        assert_eq!(
            w.run(|tx| user_set_role(tx, &admin, UserId(w.admin), Role::Moderator, "other", NOW))
                .expect_err("admin")
                .status,
            StatusCode::FORBIDDEN
        );
        let rows = w.audit();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].6, r#"{"new":"moderator","old":"none"}"#);
        assert_eq!(rows[1].6, r#"{"new":"none","old":"moderator"}"#);
        assert!(rows.iter().all(|r| r.1 == "user.role" && r.5));
    }

    #[test]
    fn dismissing_reports_on_a_hidden_item_restores_it_publicly() {
        let w = world();
        let actor = w.actor(w.moderator);
        let reporters: Vec<Member> = (0..3)
            .map(|i| {
                let id = user(&w.conn, &format!("r{i}"), old(), old());
                member(&w.conn, id)
            })
            .collect();
        for r in &reporters {
            let tx = w.conn.unchecked_transaction().expect("tx");
            reports::file(&tx, r, ReportTarget::Post(w.post), "spam", "", NOW).expect("file");
            tx.commit().expect("commit");
        }
        assert_eq!(
            w.scalar("SELECT state FROM posts WHERE id = ?1", w.post),
            "hidden"
        );
        let out = w
            .run(|tx| dismiss(tx, &actor, ReportTarget::Post(w.post), "other", NOW))
            .expect("dismiss");
        assert_eq!(
            out,
            Dismissal {
                reports: 3,
                restored: true
            }
        );
        assert_eq!(
            w.scalar("SELECT state FROM posts WHERE id = ?1", w.post),
            "visible"
        );
        assert!(w.statuses().iter().all(|s| s.0 == "dismissed"));
        let rows = w.audit();
        assert_eq!(rows.len(), 2, "auto.hide, then the restore");
        assert_eq!(rows[0].1, "auto.hide");
        assert_eq!((rows[1].1.as_str(), rows[1].5), ("post.restore", true));
        assert_eq!(rows[1].0, Some(w.moderator));
        // Nothing is left to dismiss.
        assert_eq!(
            w.run(|tx| dismiss(tx, &actor, ReportTarget::Post(w.post), "other", NOW))
                .expect_err("none open")
                .status,
            StatusCode::NOT_FOUND
        );
    }

    #[test]
    fn a_hidden_comment_dismissal_is_a_comment_restore() {
        let w = world();
        let actor = w.actor(w.moderator);
        w.report("r1", ReportTarget::Comment(w.comment));
        state(&w.conn, "comments", w.comment, "hidden");
        w.run(|tx| dismiss(tx, &actor, ReportTarget::Comment(w.comment), "other", NOW))
            .expect("dismiss");
        assert_eq!(
            w.scalar("SELECT state FROM comments WHERE id = ?1", w.comment),
            "visible"
        );
        assert_eq!(w.audit()[0].1, "comment.restore");
    }

    #[test]
    fn dismissing_anything_else_is_a_non_public_dismissal() {
        let w = world();
        let actor = w.actor(w.moderator);
        w.report("r1", ReportTarget::Post(w.post));
        w.report("r2", ReportTarget::Post(w.post));
        w.report("r3", ReportTarget::User(w.ada));
        let out = w
            .run(|tx| dismiss(tx, &actor, ReportTarget::Post(w.post), "duplicate", NOW))
            .expect("dismiss");
        assert_eq!(
            out,
            Dismissal {
                reports: 2,
                restored: false
            }
        );
        w.run(|tx| dismiss(tx, &actor, ReportTarget::User(w.ada), "other", NOW))
            .expect("dismiss user");
        assert!(w
            .statuses()
            .iter()
            .all(|s| s.0 == "dismissed" && s.1 == Some(w.moderator)));
        let rows = w.audit();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].1, "report.dismiss");
        assert!(!rows[0].5);
        assert_eq!(rows[0].6, r#"{"reports":2}"#);
        assert_eq!(rows[0].4, "duplicate");
        assert_eq!(
            w.scalar("SELECT state FROM posts WHERE id = ?1", w.post),
            "visible"
        );
        // A removed item with a report is not restored by a dismissal.
        w.report("r4", ReportTarget::Comment(w.comment));
        state(&w.conn, "comments", w.comment, "removed");
        w.run(|tx| dismiss(tx, &actor, ReportTarget::Comment(w.comment), "other", NOW))
            .expect("dismiss");
        assert_eq!(
            w.scalar("SELECT state FROM comments WHERE id = ?1", w.comment),
            "removed"
        );
    }

    #[test]
    fn the_public_log_lists_public_rows_newest_first_and_names_no_reporter() {
        let w = world();
        let actor = w.actor(w.moderator);
        let admin = w.actor(w.admin);
        w.report("secret-reporter", ReportTarget::Post(w.post));
        let t = |m: i64| NOW + Duration::minutes(m);
        w.run(|tx| post_remove(tx, &actor, w.post, "spam", t(1)))
            .expect("remove");
        w.run(|tx| comment_remove(tx, &actor, w.comment, "abuse", t(2)))
            .expect("remove");
        w.run(|tx| user_restrict(tx, &actor, UserId(w.ada), RestrictFor::Day, "spam", t(3)))
            .expect("restrict");
        // A non-public dismissal and a system row.
        let p2 = post(&w.conn, w.ada, "p2", NOW);
        w.report("another", ReportTarget::Post(p2));
        w.run(|tx| dismiss(tx, &actor, ReportTarget::Post(p2), "other", t(4)))
            .expect("dismiss");
        w.run(|tx| {
            audit::write(
                tx,
                t(5),
                None,
                audit::REGISTRY_LIST,
                Some(("pack", "glossary".into())),
                "",
                true,
                json!({}),
            )?;
            Ok(())
        })
        .expect("system row");
        w.run(|tx| user_ban(tx, &admin, UserId(w.ada), "spam", t(6)))
            .expect("ban");
        let page = public_log(&w.conn, 1).expect("log");
        assert!(!page.more);
        let actions: Vec<&str> = page.rows.iter().map(|r| r.action.as_str()).collect();
        assert_eq!(
            actions,
            vec![
                "user.ban",
                "registry.list",
                "user.restrict",
                "comment.remove",
                "post.remove"
            ]
        );
        let ban = &page.rows[0];
        assert_eq!(ban.actor, LogActor::Member("root".into()));
        assert_eq!(
            ban.target,
            LogTarget::Member {
                login: "ada".into()
            }
        );
        assert_eq!(ban.reason, "spam");
        assert_eq!(page.rows[1].actor, LogActor::System);
        assert!(
            matches!(&page.rows[1].target, LogTarget::Pack { name, post_id: None } if name == "glossary")
        );
        assert_eq!(
            page.rows[3].target,
            LogTarget::Comment {
                post_id: w.post,
                comment_id: w.comment
            }
        );
        assert_eq!(page.rows[4].target, LogTarget::Post { post_id: w.post });
        let rendered = format!("{:?}", page.rows);
        for secret in ["secret-reporter", "another", "note"] {
            assert!(!rendered.contains(secret), "{secret}");
        }
    }

    #[test]
    fn log_targets_that_are_gone_read_deleted_and_a_deletion_names_nobody() {
        let w = world();
        let m = add_measurement(&w, w.ada);
        w.run(|tx| {
            audit::write(
                tx,
                NOW,
                None,
                audit::USER_DELETE,
                Some(("user", w.ada.to_string())),
                "",
                true,
                json!({"by":"operator"}),
            )?;
            audit::write(
                tx,
                NOW,
                None,
                audit::MEASUREMENT_INVALIDATE,
                Some(("measurement", m.to_string())),
                "spam",
                true,
                json!({}),
            )?;
            audit::write(
                tx,
                NOW,
                None,
                audit::MEASUREMENT_RESTORE,
                Some(("measurement", "999".into())),
                "spam",
                true,
                json!({}),
            )?;
            audit::write(
                tx,
                NOW,
                None,
                audit::REGISTRY_SYNC,
                None,
                "",
                true,
                json!({}),
            )?;
            audit::write(
                tx,
                NOW,
                Some(UserId(w.ada)),
                audit::POST_LOCK,
                Some(("post", "888".into())),
                "spam",
                true,
                json!({}),
            )?;
            Ok(())
        })
        .expect("rows");
        let page = public_log(&w.conn, 1).expect("log");
        let by_action = |a: &str| page.rows.iter().find(|r| r.action == a).expect(a);
        assert_eq!(by_action("user.delete").target, LogTarget::Deleted);
        assert_eq!(
            by_action("measurement.invalidate").target,
            LogTarget::Measurement {
                post_id: Some(
                    w.conn
                        .query_row("SELECT id FROM posts WHERE pack = 'glossary'", [], |r| r
                            .get(0))
                        .expect("pack post")
                ),
                measurement_id: m
            }
        );
        assert_eq!(by_action("measurement.restore").target, LogTarget::Deleted);
        assert_eq!(by_action("registry.sync").target, LogTarget::None);
        assert_eq!(by_action("post.lock").target, LogTarget::Deleted);
        assert_eq!(by_action("post.lock").actor, LogActor::Member("ada".into()));
        // The actor deletes their account: the log reads [deleted].
        w.conn
            .execute(
                "UPDATE users SET deleted_at = 'x', login = '~deleted-4' WHERE id = ?1",
                [w.ada],
            )
            .expect("delete");
        let page = public_log(&w.conn, 1).expect("log");
        assert_eq!(
            page.rows
                .iter()
                .find(|r| r.action == "post.lock")
                .map(|r| r.actor.clone()),
            Some(LogActor::Deleted)
        );
    }

    #[test]
    fn the_public_log_pages_by_fifty() {
        let w = world();
        w.conn
            .execute(
                "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 60)
                 INSERT INTO audit_log (at, action, public)
                 SELECT strftime('%Y-%m-%dT%H:%M:%S.000000Z', '2026-11-08 09:00:00', '-' || i || ' minutes'),
                        'registry.sync', i % 2 FROM n",
                [],
            )
            .expect("bulk");
        // 30 public rows.
        let p1 = public_log(&w.conn, 1).expect("p1");
        assert_eq!((p1.rows.len(), p1.more), (30, false));
        w.conn
            .execute(
                "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 30)
                 INSERT INTO audit_log (at, action, public)
                 SELECT strftime('%Y-%m-%dT%H:%M:%S.000000Z', '2026-11-08 08:00:00', '-' || i || ' minutes'),
                        'registry.sync', 1 FROM n",
                [],
            )
            .expect("more public rows");
        let p1 = public_log(&w.conn, 1).expect("p1");
        assert_eq!((p1.rows.len(), p1.more), (50, true));
        assert_eq!(public_log(&w.conn, 2).expect("p2").rows.len(), 10);
        assert!(p1.rows[0].at > p1.rows[49].at);
    }

    #[test]
    fn the_member_view_has_numbers_and_audit_rows() {
        let w = world();
        let actor = w.actor(w.moderator);
        let m = add_measurement(&w, w.ada);
        cast(&w.conn, w.ada, "post", w.post, NOW);
        let bob = user(&w.conn, "bob", old(), old());
        let bobs = post(&w.conn, bob, "bobs", NOW);
        cast(&w.conn, w.ada, "post", bobs, NOW);
        // ada files one report; two are filed against her, her post and her measurement.
        {
            let me = member(&w.conn, w.ada);
            let tx = w.conn.unchecked_transaction().expect("tx");
            reports::file(&tx, &me, ReportTarget::Post(bobs), "spam", "", NOW).expect("file");
            tx.commit().expect("commit");
        }
        w.report("r1", ReportTarget::User(w.ada));
        w.report("r2", ReportTarget::Post(w.post));
        w.report("r3", ReportTarget::Measurement(m));
        w.report("r4", ReportTarget::Post(bobs));
        w.run(|tx| user_restrict(tx, &actor, UserId(w.ada), RestrictFor::Day, "spam", NOW))
            .expect("restrict");
        w.run(|tx| post_remove(tx, &actor, w.post, "spam", NOW + Duration::minutes(1)))
            .expect("remove");
        let view = member_view(&w.conn, "ADA", &w.admins, NOW + Duration::minutes(2))
            .expect("view")
            .expect("some");
        assert_eq!(view.login, "ada");
        assert_eq!(view.standing, Standing::Restricted);
        assert_eq!(view.role, Role::None);
        assert!(!view.is_admin && !view.banned && !view.deleted);
        assert_eq!(
            (view.posts, view.comments, view.votes, view.measurements),
            (1, 1, 2, 1)
        );
        assert_eq!((view.reports_filed, view.reports_against), (1, 3));
        let actions: Vec<&str> = view.audit.iter().map(|r| r.action.as_str()).collect();
        assert_eq!(actions, vec!["post.remove", "user.restrict"]);
        assert_eq!(view.audit[0].actor, LogActor::Member("mod".into()));
        let admin_view = member_view(&w.conn, "root", &w.admins, NOW)
            .expect("v")
            .expect("some");
        assert!(admin_view.is_admin);
        assert_eq!(
            member_view(&w.conn, "mod", &w.admins, NOW)
                .expect("v")
                .map(|v| v.role),
            Some(Role::Moderator)
        );
        assert_eq!(member_view(&w.conn, "nobody", &w.admins, NOW), Ok(None));
    }
}
