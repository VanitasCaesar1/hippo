//! Accounts: sign-in's account step, standing, roles, export and deletion.

use std::path::Path;

use anyhow::{bail, Context};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde_json::{json, Value};
use time::{Duration, OffsetDateTime};

use crate::audit;
use crate::clock;
use crate::config::Config;
use crate::db::{self, Member, UserId};
use crate::error::AppError;
use crate::policy;

/// A member's standing, computed from the clock whenever it is needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    New,
    Established,
    Restricted,
    Banned,
}

/// Banned beats restricted beats established. Established means the feed
/// account is at least 3 days old and the GitHub account at least 90 days
/// old; a restriction lasts until its time, which is no longer in force at
/// that time.
pub fn standing(member: &Member, now: OffsetDateTime) -> Standing {
    if member.banned {
        return Standing::Banned;
    }
    if member.restricted_until.is_some_and(|until| now < until) {
        return Standing::Restricted;
    }
    let feed_old = now - member.created_at >= Duration::days(policy::ESTABLISHED_FEED_AGE_DAYS);
    let github_old = member
        .github_created_at
        .is_some_and(|t| now - t >= Duration::days(policy::ESTABLISHED_GITHUB_AGE_DAYS));
    if feed_old && github_old {
        Standing::Established
    } else {
        Standing::New
    }
}

/// A member is an admin while their GitHub id is in `FEED_ADMINS`.
pub fn is_admin(member: &Member, config: &Config) -> bool {
    member
        .github_id
        .is_some_and(|id| config.admins.contains(&id))
}

/// A member is a moderator while they hold the role or are an admin.
pub fn is_moderator(member: &Member, config: &Config) -> bool {
    member.role == crate::db::Role::Moderator || is_admin(member, config)
}

/// What the account step of sign-in decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignIn {
    Admitted(UserId),
    /// The id belongs to a banned account, deleted or not.
    Banned,
}

/// Sets aside the login of any account other than `except` that holds
/// `login`, as `~former-<its id>`; `~` cannot occur in a GitHub login.
fn set_aside(tx: &Transaction, login: &str, except: i64) -> rusqlite::Result<()> {
    tx.execute(
        "UPDATE users SET login = '~former-' || id WHERE login = ?1 AND id != ?2",
        params![login, except],
    )?;
    Ok(())
}

/// The sign-in step that creates or updates the account. A banned id is refused. A deleted account under
/// a restriction is revived as a new account whose feed age counts from
/// `now`, the restriction still in force. A known id has its login and GitHub
/// creation date refreshed; a new id gets an account.
pub fn sign_in(
    tx: &Transaction,
    github_id: i64,
    login: &str,
    github_created_at: OffsetDateTime,
    now: OffsetDateTime,
) -> rusqlite::Result<SignIn> {
    let row: Option<(i64, bool, Option<String>)> = tx
        .query_row(
            "SELECT id, banned, deleted_at FROM users WHERE github_id = ?1",
            [github_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let created = clock::storage(github_created_at);
    match row {
        Some((_, true, _)) => Ok(SignIn::Banned),
        Some((id, false, Some(_))) => {
            set_aside(tx, login, id)?;
            tx.execute(
                "UPDATE users SET deleted_at = NULL, created_at = ?1, login = ?2,
                        github_created_at = ?3, role = 'none', theme = 'auto', personal_css = ''
                 WHERE id = ?4",
                params![clock::storage(now), login, created, id],
            )?;
            Ok(SignIn::Admitted(UserId(id)))
        }
        Some((id, false, None)) => {
            set_aside(tx, login, id)?;
            tx.execute(
                "UPDATE users SET login = ?1, github_created_at = ?2 WHERE id = ?3",
                params![login, created, id],
            )?;
            Ok(SignIn::Admitted(UserId(id)))
        }
        None => {
            set_aside(tx, login, -1)?;
            tx.execute(
                "INSERT INTO users (github_id, login, github_created_at, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![github_id, login, created, clock::storage(now)],
            )?;
            Ok(SignIn::Admitted(UserId(tx.last_insert_rowid())))
        }
    }
}

/// The member with this id as standing needs them (no session: an empty CSRF
/// token and no karma), or `None` if there is no such account.
pub fn load_member(conn: &Connection, id: UserId) -> rusqlite::Result<Option<Member>> {
    conn.query_row(
        "SELECT github_id, login, role, created_at, github_created_at, restricted_until,
                banned, theme, personal_css != ''
         FROM users WHERE id = ?1",
        [id.0],
        |r| {
            let parse = |text: Option<String>| text.as_deref().and_then(clock::parse_storage);
            Ok(Member {
                id,
                github_id: r.get(0)?,
                login: r.get(1)?,
                role: if r.get::<_, String>(2)? == "moderator" {
                    db::Role::Moderator
                } else {
                    db::Role::None
                },
                created_at: parse(r.get(3)?).unwrap_or(OffsetDateTime::UNIX_EPOCH),
                github_created_at: parse(r.get(4)?),
                restricted_until: parse(r.get(5)?),
                banned: r.get(6)?,
                theme: r.get(7)?,
                has_personal_css: r.get(8)?,
                csrf: String::new(),
                karma: 0,
            })
        },
    )
    .optional()
}

/// The member with this login, deleted accounts excluded.
pub fn find_by_login(conn: &Connection, login: &str) -> rusqlite::Result<Option<UserId>> {
    conn.query_row(
        "SELECT id FROM users WHERE login = ?1 AND deleted_at IS NULL",
        [login],
        |r| r.get(0).map(UserId),
    )
    .optional()
}

fn rfc(text: Option<String>) -> Value {
    match text {
        None => Value::Null,
        Some(t) => Value::String(clock::storage_to_rfc3339(&t).unwrap_or(t)),
    }
}

fn rows<T>(
    conn: &Connection,
    sql: &str,
    user: UserId,
    map: impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
) -> rusqlite::Result<Vec<T>> {
    let mut stmt = conn.prepare(sql)?;
    let out = stmt.query_map([user.0], map)?.collect();
    out
}

/// Everything stored about a member that the privacy table lists as theirs.
/// It leaves out audit rows and reports filed against the member.
pub fn export(conn: &Connection, user: UserId, now: OffsetDateTime) -> Result<Value, AppError> {
    let account = conn
        .query_row(
            "SELECT id, github_id, login, github_created_at, created_at, role,
                    restricted_until, banned, theme, personal_css
             FROM users WHERE id = ?1",
            [user.0],
            |r| {
                Ok((
                    json!({
                        "id": r.get::<_, i64>(0)?,
                        "github_id": r.get::<_, Option<i64>>(1)?,
                        "login": r.get::<_, String>(2)?,
                        "github_created_at": rfc(r.get(3)?),
                        "created_at": rfc(r.get(4)?),
                        "role": r.get::<_, String>(5)?,
                        "restricted_until": rfc(r.get(6)?),
                        "banned": r.get::<_, bool>(7)?,
                        "theme": r.get::<_, String>(8)?,
                    }),
                    r.get::<_, String>(9)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(AppError::not_found)?;
    let (account, personal_css) = account;

    let posts = rows(
        conn,
        "SELECT id, kind, title, link, body, created_at, edited_at, state
         FROM posts WHERE author_id = ?1 ORDER BY id",
        user,
        |r| {
            Ok(json!({
                "id": r.get::<_, i64>(0)?, "kind": r.get::<_, String>(1)?,
                "title": r.get::<_, String>(2)?, "link": r.get::<_, Option<String>>(3)?,
                "body": r.get::<_, String>(4)?, "created_at": rfc(r.get(5)?),
                "edited_at": rfc(r.get(6)?), "state": r.get::<_, String>(7)?,
            }))
        },
    )?;
    let comments = rows(
        conn,
        "SELECT id, post_id, parent_id, body, created_at, edited_at, state
         FROM comments WHERE author_id = ?1 ORDER BY id",
        user,
        |r| {
            Ok(json!({
                "id": r.get::<_, i64>(0)?, "post_id": r.get::<_, i64>(1)?,
                "parent_id": r.get::<_, Option<i64>>(2)?, "body": r.get::<_, String>(3)?,
                "created_at": rfc(r.get(4)?), "edited_at": rfc(r.get(5)?),
                "state": r.get::<_, String>(6)?,
            }))
        },
    )?;
    let votes = rows(
        conn,
        "SELECT target_type, target_id, at FROM votes WHERE user_id = ?1 ORDER BY at, target_id",
        user,
        |r| {
            Ok(json!({
                "target_type": r.get::<_, String>(0)?, "target_id": r.get::<_, i64>(1)?,
                "at": rfc(r.get(2)?),
            }))
        },
    )?;
    let hides = rows(
        conn,
        "SELECT post_id, at FROM hides WHERE user_id = ?1 ORDER BY at, post_id",
        user,
        |r| Ok(json!({ "post_id": r.get::<_, i64>(0)?, "at": rfc(r.get(1)?) })),
    )?;
    let measurements = rows(
        conn,
        "SELECT m.id, m.pack, m.pack_hash, c.version, m.eval_set, m.planted, m.kept, m.runs,
                m.corrected_without, m.damaged_without, m.corrected_with, m.damaged_with,
                m.hippo, m.model, m.report_url, m.created_at, m.updated_at, m.revisions, m.invalid
         FROM measurements m JOIN pack_contents c ON c.pack = m.pack AND c.pack_hash = m.pack_hash
         WHERE m.user_id = ?1 ORDER BY m.id",
        user,
        |r| {
            let (planted, kept, runs) = (
                r.get::<_, i64>(5)? as f64,
                r.get::<_, i64>(6)? as f64,
                r.get::<_, i64>(7)? as f64,
            );
            let (c0, d0, c1, d1) = (
                r.get::<_, i64>(8)? as f64,
                r.get::<_, i64>(9)? as f64,
                r.get::<_, i64>(10)? as f64,
                r.get::<_, i64>(11)? as f64,
            );
            let benefit = (((c1 - c0) * kept - (d1 - d0) * planted) / (planted * kept * runs))
                .clamp(-1.0, 1.0);
            Ok(json!({
                "id": r.get::<_, i64>(0)?, "pack": r.get::<_, String>(1)?,
                "pack_hash": r.get::<_, String>(2)?, "version": r.get::<_, String>(3)?,
                "eval_set": r.get::<_, String>(4)?, "runs": r.get::<_, i64>(7)?,
                "corrected_without": r.get::<_, i64>(8)?, "damaged_without": r.get::<_, i64>(9)?,
                "corrected_with": r.get::<_, i64>(10)?, "damaged_with": r.get::<_, i64>(11)?,
                "benefit": (benefit * 1000.0).round() / 1000.0,
                "hippo": r.get::<_, String>(12)?, "model": r.get::<_, String>(13)?,
                "report_url": r.get::<_, Option<String>>(14)?,
                "created_at": rfc(r.get(15)?), "updated_at": rfc(r.get(16)?),
                "revisions": r.get::<_, i64>(17)?, "invalid": r.get::<_, bool>(18)?,
            }))
        },
    )?;
    let reports = rows(
        conn,
        "SELECT id, target_type, target_id, reason, note, created_at, status
         FROM reports WHERE reporter_id = ?1 ORDER BY id",
        user,
        |r| {
            Ok(json!({
                "id": r.get::<_, i64>(0)?, "target_type": r.get::<_, String>(1)?,
                "target_id": r.get::<_, i64>(2)?, "reason": r.get::<_, String>(3)?,
                "note": r.get::<_, String>(4)?, "created_at": rfc(r.get(5)?),
                "status": r.get::<_, String>(6)?,
            }))
        },
    )?;
    let sessions = rows(
        conn,
        "SELECT created_at, expires_at FROM sessions WHERE user_id = ?1 ORDER BY created_at",
        user,
        |r| Ok(json!({ "created_at": rfc(r.get(0)?), "expires_at": rfc(r.get(1)?) })),
    )?;

    Ok(json!({
        "exported_at": clock::rfc3339(now),
        "account": account,
        "personal_css": personal_css,
        "posts": posts,
        "comments": comments,
        "votes": votes,
        "hides": hides,
        "measurements": measurements,
        "reports": reports,
        "sessions": sessions,
    }))
}

/// The login of a member, for the export's file name.
pub fn login_of(conn: &Connection, user: UserId) -> rusqlite::Result<String> {
    conn.query_row("SELECT login FROM users WHERE id = ?1", [user.0], |r| {
        r.get(0)
    })
}

/// Deletes an account, in
/// the caller's transaction. `actor` is the member who asked, or `None` for
/// the operator's command; the audit row is public only for the operator and
/// never names the member. The GitHub id stays while a ban or a restriction
/// that has not ended lasts.
pub fn delete_account(
    tx: &Transaction,
    user: UserId,
    actor: Option<UserId>,
    now: OffsetDateTime,
) -> Result<(), AppError> {
    let (banned, restricted_until): (bool, Option<String>) = tx
        .query_row(
            "SELECT banned, restricted_until FROM users WHERE id = ?1 AND deleted_at IS NULL",
            [user.0],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
        .ok_or_else(AppError::not_found)?;
    let restricted = restricted_until.is_some_and(|t| t > clock::storage(now));
    let keep_id = banned || restricted;

    for table in ["sessions", "votes", "measurements", "hides"] {
        tx.execute(&format!("DELETE FROM {table} WHERE user_id = ?1"), [user.0])?;
    }
    tx.execute(
        "UPDATE posts SET title = '[deleted]', body = '[deleted]', link = NULL,
                author_id = NULL, state = 'deleted' WHERE author_id = ?1",
        [user.0],
    )?;
    tx.execute(
        "UPDATE comments SET body = '[deleted]', author_id = NULL, state = 'deleted'
         WHERE author_id = ?1",
        [user.0],
    )?;
    tx.execute(
        "UPDATE reports SET reporter_id = NULL WHERE reporter_id = ?1",
        [user.0],
    )?;
    tx.execute(
        "UPDATE users SET theme = 'auto', personal_css = '', github_created_at = NULL,
                login = '~deleted-' || id,
                github_id = CASE WHEN ?2 THEN github_id ELSE NULL END,
                deleted_at = ?3
         WHERE id = ?1",
        params![user.0, keep_id, clock::storage(now)],
    )?;
    let (by, public) = match actor {
        Some(_) => ("member", false),
        None => ("operator", true),
    };
    audit::write(
        tx,
        now,
        actor,
        audit::USER_DELETE,
        Some(("user", user.0.to_string())),
        "",
        public,
        json!({ "by": by }),
    )?;
    Ok(())
}

fn open_migrated(path: &Path) -> anyhow::Result<Connection> {
    let conn =
        db::open(path).with_context(|| format!("cannot open the database {}", path.display()))?;
    db::check_version(&conn)?;
    Ok(conn)
}

/// `account export <login>`: the export as indented JSON.
pub fn export_by_login(path: &Path, login: &str, now: OffsetDateTime) -> anyhow::Result<String> {
    let conn = open_migrated(path)?;
    let Some(id) = find_by_login(&conn, login)? else {
        bail!("no member has the login {login}");
    };
    let value = export(&conn, id, now).map_err(|e| anyhow::anyhow!("{}", e.message))?;
    Ok(serde_json::to_string_pretty(&value)?)
}

/// `account delete <login>`: deletes with the system as actor and writes a
/// public `user.delete` row.
pub fn delete_by_login(path: &Path, login: &str, now: OffsetDateTime) -> anyhow::Result<()> {
    let mut conn = open_migrated(path)?;
    let Some(id) = find_by_login(&conn, login)? else {
        bail!("no member has the login {login}");
    };
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    delete_account(&tx, id, None, now).map_err(|e| anyhow::anyhow!("{}", e.message))?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Role;
    use time::macros::datetime;

    fn member(created: OffsetDateTime, github: Option<OffsetDateTime>) -> Member {
        Member {
            id: UserId(1),
            github_id: Some(1001),
            login: "ada".into(),
            role: Role::None,
            created_at: created,
            github_created_at: github,
            restricted_until: None,
            banned: false,
            theme: "auto".into(),
            has_personal_css: false,
            csrf: String::new(),
            karma: 0,
        }
    }

    #[test]
    fn standing_at_the_three_and_ninety_day_edges() {
        let now = datetime!(2026-11-08 10:00:00 UTC);
        let old_github = Some(now - Duration::days(1000));
        let at =
            |feed: Duration, gh: Option<OffsetDateTime>| standing(&member(now - feed, gh), now);
        let just = Duration::seconds(1);
        assert_eq!(at(Duration::days(3) - just, old_github), Standing::New);
        assert_eq!(at(Duration::days(3), old_github), Standing::Established);
        let feed_old = Duration::days(30);
        assert_eq!(
            at(feed_old, Some(now - Duration::days(90) + just)),
            Standing::New
        );
        assert_eq!(
            at(feed_old, Some(now - Duration::days(90))),
            Standing::Established
        );
        assert_eq!(at(feed_old, None), Standing::New);
    }

    #[test]
    fn standing_restricted_banned_and_a_restrictions_end() {
        let now = datetime!(2026-11-08 10:00:00 UTC);
        let mut m = member(now - Duration::days(30), Some(now - Duration::days(1000)));
        m.restricted_until = Some(now + Duration::seconds(1));
        assert_eq!(standing(&m, now), Standing::Restricted);
        m.restricted_until = Some(now);
        assert_eq!(standing(&m, now), Standing::Established);
        m.restricted_until = Some(now + Duration::days(1));
        m.banned = true;
        assert_eq!(standing(&m, now), Standing::Banned);
    }

    fn config_with_admins(ids: &str) -> Config {
        let vars = [
            ("FEED_DB".to_string(), "/tmp/x".to_string()),
            ("FEED_ADMINS".to_string(), ids.to_string()),
        ]
        .into_iter()
        .collect();
        Config::from_vars(crate::config::Command::Migrate, &vars)
            .map(|mut c| {
                c.admins = ids.split(',').filter_map(|s| s.parse().ok()).collect();
                c
            })
            .expect("config")
    }

    #[test]
    fn is_admin_needs_the_github_id_in_the_list() {
        let now = datetime!(2026-11-08 10:00:00 UTC);
        let m = member(now, None);
        assert!(is_admin(&m, &config_with_admins("1001,1002")));
        assert!(!is_admin(&m, &config_with_admins("1002")));
        assert!(!is_admin(&m, &config_with_admins("")));
        let mut none = m.clone();
        none.github_id = None;
        assert!(!is_admin(&none, &config_with_admins("1001")));
    }

    fn fresh() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("feed.sqlite");
        db::migrate(&path).expect("migrate");
        (dir, db::open(&path).expect("open"))
    }

    #[test]
    fn deletion_clears_the_account_fields_and_keeps_a_sanctioned_id() {
        let now = datetime!(2026-11-08 10:00:00 UTC);
        let (_dir, mut conn) = fresh();
        let tx = conn.transaction().expect("tx");
        let gh = datetime!(2015-03-01 00:00:00 UTC);
        let a = match sign_in(&tx, 1001, "ada", gh, now).expect("a") {
            SignIn::Admitted(id) => id,
            SignIn::Banned => panic!("banned"),
        };
        let b = match sign_in(&tx, 1002, "grace", gh, now).expect("b") {
            SignIn::Admitted(id) => id,
            SignIn::Banned => panic!("banned"),
        };
        tx.execute(
            "UPDATE users SET restricted_until = ?1 WHERE id = ?2",
            params![clock::storage(now + Duration::days(1)), b.0],
        )
        .expect("restrict");
        tx.execute(
            "UPDATE users SET theme = 'dark', personal_css = 'a{}' WHERE id = ?1",
            [a.0],
        )
        .expect("css");
        delete_account(&tx, a, Some(a), now).expect("delete a");
        delete_account(&tx, b, None, now).expect("delete b");
        let field = |id: UserId| -> (String, Option<i64>, Option<String>, String, String) {
            tx.query_row(
                "SELECT login, github_id, github_created_at, theme, personal_css FROM users WHERE id = ?1",
                [id.0],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .expect("row")
        };
        assert_eq!(
            field(a),
            (
                format!("~deleted-{}", a.0),
                None,
                None,
                "auto".into(),
                String::new()
            )
        );
        let kept = field(b);
        assert_eq!(kept.0, format!("~deleted-{}", b.0));
        assert_eq!(kept.1, Some(1002));
        let (public_a, public_b): (bool, bool) = (
            tx.query_row(
                "SELECT public FROM audit_log WHERE actor_id = ?1",
                [a.0],
                |r| r.get(0),
            )
            .expect("a"),
            tx.query_row(
                "SELECT public FROM audit_log WHERE actor_id IS NULL",
                [],
                |r| r.get(0),
            )
            .expect("b"),
        );
        assert!(!public_a && public_b);
    }
}
