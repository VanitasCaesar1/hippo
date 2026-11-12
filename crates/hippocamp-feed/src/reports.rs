//! Reports: filing with a snapshot, the automatic hide, resolution, the
//! moderators' queue and the 90-day clean-up of report text.
//!
//! A member files a report; the system files its own through
//! [`file_system_report`] (rings, large benefits). A report is resolved as
//! upheld or dismissed by a moderation action (`moderation.rs`), never by
//! deleting it. Every function that writes takes the `&Transaction` of the
//! change it makes and the time as a parameter.

use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde_json::{json, Value};
use time::{Duration, OffsetDateTime};

use crate::audit;
use crate::clock;
use crate::comments;
use crate::db::{Member, UserId};
use crate::error::{AppError, FormError};
use crate::page::{self, Page};
use crate::policy;
use crate::posts;
use crate::text;
use crate::users::{self, Standing};

/// The reasons a member can give.
pub const REASONS: [&str; 6] = [
    "spam",
    "abuse",
    "malicious",
    "licence",
    "misleading",
    "other",
];

/// What a report is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReportTarget {
    Post(i64),
    Comment(i64),
    Measurement(i64),
    User(i64),
}

impl ReportTarget {
    /// The type as the `reports` table and the audit log write it.
    pub fn type_str(self) -> &'static str {
        match self {
            ReportTarget::Post(_) => "post",
            ReportTarget::Comment(_) => "comment",
            ReportTarget::Measurement(_) => "measurement",
            ReportTarget::User(_) => "user",
        }
    }

    pub fn id(self) -> i64 {
        match self {
            ReportTarget::Post(id)
            | ReportTarget::Comment(id)
            | ReportTarget::Measurement(id)
            | ReportTarget::User(id) => id,
        }
    }

    /// The target of a form's `target_type` and `target_id`.
    pub fn parse(target_type: &str, id: i64) -> Option<Self> {
        match target_type {
            "post" => Some(ReportTarget::Post(id)),
            "comment" => Some(ReportTarget::Comment(id)),
            "measurement" => Some(ReportTarget::Measurement(id)),
            "user" => Some(ReportTarget::User(id)),
            _ => None,
        }
    }

    /// The audit log's target: type and id.
    pub fn audit(self) -> (&'static str, String) {
        (self.type_str(), self.id().to_string())
    }
}

/// How a report was resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    Upheld,
    Dismissed,
}

impl Resolution {
    pub fn as_str(self) -> &'static str {
        match self {
            Resolution::Upheld => "upheld",
            Resolution::Dismissed => "dismissed",
        }
    }
}

/// A target as moderation needs to see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Described {
    /// The author, owner or (for a member) the member themselves; `None` for
    /// a pack's post and for an item whose author deleted their account.
    pub owner: Option<i64>,
    /// `visible`, `hidden`, `removed` or `deleted` for a post or comment.
    pub state: Option<String>,
    /// The target's text as a snapshot keeps it.
    pub text: String,
    /// The post a page link goes to: the post itself, a comment's post, a
    /// measurement's pack post.
    pub post_id: Option<i64>,
    /// The pack, for a pack's post and for a measurement.
    pub pack: Option<String>,
}

fn pack_post_id(conn: &Connection, pack: &str) -> rusqlite::Result<Option<i64>> {
    conn.query_row("SELECT id FROM posts WHERE pack = ?1", [pack], |r| r.get(0))
        .optional()
}

/// The target now, or `None` if it does not exist.
pub fn describe(conn: &Connection, target: ReportTarget) -> rusqlite::Result<Option<Described>> {
    Ok(match target {
        ReportTarget::Post(id) => conn
            .query_row(
                "SELECT author_id, state, title, link, body, pack FROM posts WHERE id = ?1",
                [id],
                |r| {
                    let title: String = r.get(2)?;
                    let link: Option<String> = r.get(3)?;
                    let body: String = r.get(4)?;
                    let mut parts = vec![title];
                    parts.extend(link);
                    if !body.is_empty() {
                        parts.push(body);
                    }
                    Ok(Described {
                        owner: r.get(0)?,
                        state: Some(r.get(1)?),
                        text: parts.join("\n\n"),
                        post_id: Some(id),
                        pack: r.get(5)?,
                    })
                },
            )
            .optional()?,
        ReportTarget::Comment(id) => conn
            .query_row(
                "SELECT author_id, state, body, post_id FROM comments WHERE id = ?1",
                [id],
                |r| {
                    Ok(Described {
                        owner: r.get(0)?,
                        state: Some(r.get(1)?),
                        text: r.get(2)?,
                        post_id: Some(r.get(3)?),
                        pack: None,
                    })
                },
            )
            .optional()?,
        ReportTarget::Measurement(id) => {
            let row = conn
                .query_row(
                    "SELECT user_id, pack, pack_hash, eval_set, runs, corrected_without,
                            damaged_without, corrected_with, damaged_with, hippocamp, model, report_url
                     FROM measurements WHERE id = ?1",
                    [id],
                    |r| {
                        let pack: String = r.get(1)?;
                        let report: Option<String> = r.get(11)?;
                        let text = format!(
                            "pack {pack} content {} eval set {}, {} runs; without: corrected {} damaged {}; \
                             with: corrected {} damaged {}; hippocamp {}; model {}{}",
                            r.get::<_, String>(2)?,
                            r.get::<_, String>(3)?,
                            r.get::<_, i64>(4)?,
                            r.get::<_, i64>(5)?,
                            r.get::<_, i64>(6)?,
                            r.get::<_, i64>(7)?,
                            r.get::<_, i64>(8)?,
                            r.get::<_, String>(9)?,
                            r.get::<_, String>(10)?,
                            report.map(|u| format!("; report {u}")).unwrap_or_default(),
                        );
                        Ok((r.get::<_, i64>(0)?, pack, text))
                    },
                )
                .optional()?;
            match row {
                None => None,
                Some((owner, pack, text)) => Some(Described {
                    owner: Some(owner),
                    state: None,
                    text,
                    post_id: pack_post_id(conn, &pack)?,
                    pack: Some(pack),
                }),
            }
        }
        ReportTarget::User(id) => conn
            .query_row(
                "SELECT login FROM users WHERE id = ?1 AND deleted_at IS NULL",
                [id],
                |r| {
                    Ok(Described {
                        owner: Some(id),
                        state: None,
                        text: r.get(0)?,
                        post_id: None,
                        pack: None,
                    })
                },
            )
            .optional()?,
    })
}

fn parse_note(raw: &str) -> Result<String, FormError> {
    if text::normalise(raw).is_empty() {
        return Ok(String::new());
    }
    text::one_line("note", raw, policy::REPORT_NOTE_MAX)
}

/// Members who are established at `now`, as SQL over `users u` with `?1` the
/// time, `?2` the latest feed-account creation time and `?3` the latest
/// GitHub-account creation time.
const ESTABLISHED_SQL: &str = "u.banned = 0 AND u.deleted_at IS NULL
     AND u.created_at <= ?2 AND u.github_created_at IS NOT NULL AND u.github_created_at <= ?3
     AND (u.restricted_until IS NULL OR u.restricted_until <= ?1)";

/// Files a member's report and returns its id. The target must be a visible
/// post or comment, a measurement, or a member (404 otherwise); the reporter
/// must not be restricted or banned (403), the reason one of [`REASONS`] and
/// the note at most 1,000 characters (422); a member reports a target once
/// and never their own content or account (422). The report keeps a snapshot
/// of the target's text. When a visible item now has open reports from three
/// distinct members who are all established now, it becomes `hidden` in this
/// transaction with an `auto.hide` row (the system as actor, public, with the
/// number of open reports). The per-member report limit is
/// `limits::check`'s.
pub fn file(
    tx: &Transaction,
    reporter: &Member,
    target: ReportTarget,
    reason: &str,
    note: &str,
    now: OffsetDateTime,
) -> Result<i64, AppError> {
    if matches!(
        users::standing(reporter, now),
        Standing::Restricted | Standing::Banned
    ) {
        return Err(AppError::forbidden("A restricted account cannot report."));
    }
    let reason = reason.trim();
    if !REASONS.contains(&reason) {
        return Err(AppError::form(FormError::new(
            "reason",
            format!("must be one of {}", REASONS.join(", ")),
        )));
    }
    let note = parse_note(note).map_err(AppError::form)?;
    let described = describe(tx, target)?.ok_or_else(AppError::not_found)?;
    if matches!(described.state.as_deref(), Some(s) if s != "visible") {
        return Err(AppError::not_found());
    }
    if described.owner == Some(reporter.id.0) {
        let what = if matches!(target, ReportTarget::User(_)) {
            "your own account"
        } else {
            "your own content"
        };
        return Err(AppError::form(FormError::new(
            "target",
            format!("you cannot report {what}"),
        )));
    }
    let already: bool = tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM reports
                        WHERE reporter_id = ?1 AND target_type = ?2 AND target_id = ?3)",
        params![reporter.id.0, target.type_str(), target.id()],
        |r| r.get(0),
    )?;
    if already {
        return Err(AppError::form(FormError::new(
            "target",
            "you have already reported this",
        )));
    }
    tx.execute(
        "INSERT INTO reports (reporter_id, system, target_type, target_id, reason, note, snapshot,
                              created_at)
         VALUES (?1, 0, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            reporter.id.0,
            target.type_str(),
            target.id(),
            reason,
            note,
            described.text,
            clock::storage(now)
        ],
    )?;
    let id = tx.last_insert_rowid();
    auto_hide(tx, target, now)?;
    Ok(id)
}

fn auto_hide(tx: &Transaction, target: ReportTarget, now: OffsetDateTime) -> Result<(), AppError> {
    if !matches!(target, ReportTarget::Post(_) | ReportTarget::Comment(_)) {
        return Ok(());
    }
    let sql = format!(
        "SELECT COUNT(DISTINCT r.reporter_id)
         FROM reports r JOIN users u ON u.id = r.reporter_id
         WHERE r.target_type = ?4 AND r.target_id = ?5 AND r.status = 'open' AND r.system = 0
           AND {ESTABLISHED_SQL}"
    );
    let established: i64 = tx.query_row(
        &sql,
        params![
            clock::storage(now),
            clock::storage(now - Duration::days(policy::ESTABLISHED_FEED_AGE_DAYS)),
            clock::storage(now - Duration::days(policy::ESTABLISHED_GITHUB_AGE_DAYS)),
            target.type_str(),
            target.id()
        ],
        |r| r.get(0),
    )?;
    if established < i64::from(policy::AUTO_HIDE_REPORTERS) {
        return Ok(());
    }
    let changed = match target {
        ReportTarget::Post(id) => posts::hide_by_reports(tx, id)?,
        ReportTarget::Comment(id) => comments::hide_by_reports(tx, id)?,
        _ => false,
    };
    if changed {
        let open = open_count(tx, target)?;
        audit::write(
            tx,
            now,
            None,
            audit::AUTO_HIDE,
            Some(target.audit()),
            "",
            true,
            json!({ "reports": open }),
        )?;
    }
    Ok(())
}

/// Files the system's own report and returns its id. `reason` is one of
/// [`REASONS`] or `ring`; `details` is JSON holding ids and numbers, never
/// free text. The report keeps a snapshot of the target when it exists. The
/// system's reports never count towards the automatic hide.
pub fn file_system_report(
    tx: &Transaction,
    target: ReportTarget,
    reason: &str,
    details: Value,
    now: OffsetDateTime,
) -> rusqlite::Result<i64> {
    let snapshot = describe(tx, target)?.map(|d| d.text).unwrap_or_default();
    tx.execute(
        "INSERT INTO reports (reporter_id, system, target_type, target_id, reason, snapshot,
                              details, created_at)
         VALUES (NULL, 1, ?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            target.type_str(),
            target.id(),
            reason,
            snapshot,
            details.to_string(),
            clock::storage(now)
        ],
    )?;
    Ok(tx.last_insert_rowid())
}

/// The number of open reports on a target.
pub fn open_count(conn: &Connection, target: ReportTarget) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM reports
         WHERE target_type = ?1 AND target_id = ?2 AND status = 'open'",
        params![target.type_str(), target.id()],
        |r| r.get(0),
    )
}

/// Resolves every open report on a target. `by` is the moderator, or `None`
/// for the system. Returns how many it resolved.
pub fn resolve_target(
    tx: &Transaction,
    target: ReportTarget,
    resolution: Resolution,
    by: Option<UserId>,
    now: OffsetDateTime,
) -> rusqlite::Result<usize> {
    tx.execute(
        "UPDATE reports SET status = ?1, resolved_by = ?2, resolved_at = ?3
         WHERE target_type = ?4 AND target_id = ?5 AND status = 'open'",
        params![
            resolution.as_str(),
            by.map(|u| u.0),
            clock::storage(now),
            target.type_str(),
            target.id()
        ],
    )
}

/// Resolves one report if it is open. Returns whether it was.
pub fn resolve_report(
    tx: &Transaction,
    id: i64,
    resolution: Resolution,
    by: Option<UserId>,
    now: OffsetDateTime,
) -> rusqlite::Result<bool> {
    let n = tx.execute(
        "UPDATE reports SET status = ?1, resolved_by = ?2, resolved_at = ?3
         WHERE id = ?4 AND status = 'open'",
        params![
            resolution.as_str(),
            by.map(|u| u.0),
            clock::storage(now),
            id
        ],
    )?;
    Ok(n > 0)
}

/// Deletes the note and snapshot of reports resolved more than 90 days ago
/// (the hourly job calls this). Returns how many reports it blanked.
pub fn purge_resolved_text(tx: &Transaction, now: OffsetDateTime) -> rusqlite::Result<usize> {
    let cutoff = clock::storage(now - Duration::days(policy::REPORT_TEXT_DAYS));
    tx.execute(
        "UPDATE reports SET note = '', snapshot = ''
         WHERE status != 'open' AND resolved_at IS NOT NULL AND resolved_at <= ?1
           AND (note != '' OR snapshot != '')",
        [cutoff],
    )
}

/// Who filed a report, as the queue shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reporter {
    /// An automatic report.
    System,
    /// A member who has deleted their account.
    Deleted,
    Member {
        id: i64,
        login: String,
    },
}

/// One open report in the queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueReport {
    pub id: i64,
    pub reporter: Reporter,
    pub reason: String,
    pub note: String,
    pub snapshot: String,
    /// The system report's JSON details (`{}` for a member's).
    pub details: Value,
    pub created_at: OffsetDateTime,
}

/// A target with open reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueEntry {
    pub target: ReportTarget,
    /// The target now; `None` if it no longer exists.
    pub described: Option<Described>,
    /// A preview of the target's text: its first 200 characters.
    pub preview: String,
    /// Open reports by reason, most first (then by reason).
    pub counts: Vec<(String, u32)>,
    /// The open reports, oldest first.
    pub reports: Vec<QueueReport>,
    /// When the newest open report was filed.
    pub newest: OffsetDateTime,
}

impl QueueEntry {
    /// Whether this is a `malicious` report on a pack's post, which the queue
    /// reminds moderators to forward to the registry maintainer.
    pub fn forward_to_registry(&self) -> bool {
        matches!(self.target, ReportTarget::Post(_))
            && self.described.as_ref().is_some_and(|d| d.pack.is_some())
            && self.counts.iter().any(|(reason, _)| reason == "malicious")
    }
}

fn preview(text: &str) -> String {
    let mut out: String = text.chars().take(200).collect();
    if text.chars().count() > 200 {
        out.push('…');
    }
    out
}

/// The targets with open reports, newest report first (then type and higher
/// id), 50 a page, each with its reports, snapshot and counts by reason. A
/// page past the end is empty.
pub fn open_queue(conn: &Connection, page: usize) -> rusqlite::Result<Page<QueueEntry>> {
    let mut stmt = conn.prepare(
        "SELECT target_type, target_id, MAX(created_at) AS newest
         FROM reports WHERE status = 'open'
         GROUP BY target_type, target_id
         ORDER BY newest DESC, target_type, target_id DESC
         LIMIT ?1 OFFSET ?2",
    )?;
    let targets: Vec<(String, i64, String)> = stmt
        .query_map(
            params![
                (policy::LIST_PAGE_SIZE + 1) as i64,
                page::offset(page, policy::LIST_PAGE_SIZE) as i64
            ],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?
        .collect::<Result<_, _>>()?;
    let paged = Page::from_overfetched(targets, policy::LIST_PAGE_SIZE);
    let mut report_stmt = conn.prepare(
        "SELECT r.id, r.reporter_id, r.system, u.login, r.reason, r.note, r.snapshot, r.details,
                r.created_at
         FROM reports r LEFT JOIN users u ON u.id = r.reporter_id
         WHERE r.status = 'open' AND r.target_type = ?1 AND r.target_id = ?2
         ORDER BY r.created_at, r.id",
    )?;
    let mut rows = Vec::new();
    for (kind, id, newest) in paged.rows {
        let Some(target) = ReportTarget::parse(&kind, id) else {
            continue;
        };
        let reports: Vec<QueueReport> = report_stmt
            .query_map(params![kind, id], |r| {
                let system: bool = r.get(2)?;
                let reporter_id: Option<i64> = r.get(1)?;
                let login: Option<String> = r.get(3)?;
                let reporter = match (system, reporter_id, login) {
                    (true, _, _) => Reporter::System,
                    (false, Some(id), Some(login)) => Reporter::Member { id, login },
                    _ => Reporter::Deleted,
                };
                let details: String = r.get(7)?;
                Ok(QueueReport {
                    id: r.get(0)?,
                    reporter,
                    reason: r.get(4)?,
                    note: r.get(5)?,
                    snapshot: r.get(6)?,
                    details: serde_json::from_str(&details).unwrap_or(Value::Null),
                    created_at: clock::parse_storage(&r.get::<_, String>(8)?)
                        .unwrap_or(OffsetDateTime::UNIX_EPOCH),
                })
            })?
            .collect::<Result<_, _>>()?;
        let mut counts: Vec<(String, u32)> = Vec::new();
        for report in &reports {
            match counts
                .iter_mut()
                .find(|(reason, _)| *reason == report.reason)
            {
                Some((_, n)) => *n += 1,
                None => counts.push((report.reason.clone(), 1)),
            }
        }
        counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let described = describe(conn, target)?;
        let preview_text = match (&described, target) {
            (Some(d), ReportTarget::Post(_)) => {
                preview(d.text.split("\n\n").next().unwrap_or_default())
            }
            (Some(d), _) => preview(&d.text),
            (None, _) => "[deleted]".to_string(),
        };
        rows.push(QueueEntry {
            target,
            described,
            preview: preview_text,
            counts,
            reports,
            newest: clock::parse_storage(&newest).unwrap_or(OffsetDateTime::UNIX_EPOCH),
        });
    }
    Ok(Page {
        rows,
        more: paged.more,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::posts::testkit::*;
    use axum::http::StatusCode;
    use time::macros::datetime;

    const NOW: OffsetDateTime = datetime!(2026-11-08 10:00:00 UTC);

    fn old() -> OffsetDateTime {
        NOW - Duration::days(400)
    }

    struct Setup {
        _dir: tempfile::TempDir,
        conn: Connection,
        author: i64,
        post: i64,
        comment: i64,
    }

    fn setup() -> Setup {
        let (dir, conn) = fresh();
        let author = user(&conn, "author", old(), old());
        let post = post(&conn, author, "a title", NOW - Duration::hours(1));
        let comment = comment(&conn, post, author, NOW - Duration::hours(1));
        Setup {
            _dir: dir,
            conn,
            author,
            post,
            comment,
        }
    }

    fn established(s: &Setup, name: &str) -> Member {
        let id = user(&s.conn, name, old(), old());
        member(&s.conn, id)
    }

    fn file_now(
        s: &Setup,
        who: &Member,
        target: ReportTarget,
        reason: &str,
        note: &str,
    ) -> Result<i64, AppError> {
        let tx = s.conn.unchecked_transaction().expect("tx");
        let id = file(&tx, who, target, reason, note, NOW)?;
        tx.commit().expect("commit");
        Ok(id)
    }

    fn state_of(s: &Setup, table: &str, id: i64) -> String {
        s.conn
            .query_row(
                &format!("SELECT state FROM {table} WHERE id = ?1"),
                [id],
                |r| r.get(0),
            )
            .expect("state")
    }

    #[test]
    fn target_forms_round_trip() {
        for (t, id) in [("post", 1), ("comment", 2), ("measurement", 3), ("user", 4)] {
            let target = ReportTarget::parse(t, id).expect(t);
            assert_eq!((target.type_str(), target.id()), (t, id));
        }
        assert_eq!(ReportTarget::parse("pack", 1), None);
    }

    #[test]
    fn filing_keeps_the_snapshot_of_a_post_a_comment_and_a_member() {
        let s = setup();
        s.conn
            .execute(
                "UPDATE posts SET link = 'https://example.com/', body = 'the body' WHERE id = ?1",
                [s.post],
            )
            .expect("post");
        let rep = established(&s, "rep");
        let id = file_now(
            &s,
            &rep,
            ReportTarget::Post(s.post),
            "spam",
            "  looks like spam \r\n",
        )
        .expect("file");
        let row: (i64, i64, String, String, String, String, String, String) = s
            .conn
            .query_row(
                "SELECT reporter_id, system, target_type, reason, note, snapshot, status, created_at
                 FROM reports WHERE id = ?1",
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
                        r.get(7)?,
                    ))
                },
            )
            .expect("row");
        assert_eq!(row.0, rep.id.0);
        assert_eq!(row.1, 0);
        assert_eq!((row.2.as_str(), row.3.as_str()), ("post", "spam"));
        assert_eq!(row.4, "looks like spam");
        assert_eq!(row.5, "a title\n\nhttps://example.com/\n\nthe body");
        assert_eq!(row.6, "open");
        assert_eq!(row.7, clock::storage(NOW));
        file_now(&s, &rep, ReportTarget::Comment(s.comment), "abuse", "").expect("comment");
        let snap: String = s
            .conn
            .query_row(
                "SELECT snapshot FROM reports WHERE target_type = 'comment'",
                [],
                |r| r.get(0),
            )
            .expect("snap");
        assert_eq!(snap, "c");
        file_now(&s, &rep, ReportTarget::User(s.author), "other", "").expect("member");
        let snap: String = s
            .conn
            .query_row(
                "SELECT snapshot FROM reports WHERE target_type = 'user'",
                [],
                |r| r.get(0),
            )
            .expect("snap");
        assert_eq!(snap, "author");
    }

    #[test]
    fn a_measurement_can_be_reported_with_its_numbers_in_the_snapshot() {
        let s = setup();
        pack_post(&s.conn, "glossary", 5, "[]", NOW);
        s.conn
            .execute(
                "INSERT INTO pack_contents (pack, pack_hash, version, commit_id, first_seen)
                 VALUES ('glossary', 'h', '1', 'c', ?1)",
                [clock::storage(NOW)],
            )
            .expect("content");
        s.conn
            .execute(
                "INSERT INTO measurements (pack, pack_hash, user_id, eval_set, planted, kept, runs,
                    corrected_without, damaged_without, corrected_with, damaged_with, hippocamp, model,
                    created_at, updated_at)
                 VALUES ('glossary', 'h', ?1, 'set1', 5, 5, 3, 1, 2, 3, 4, '0.1.0', 'm1', ?2, ?2)",
                params![s.author, clock::storage(NOW)],
            )
            .expect("measurement");
        let mid = s.conn.last_insert_rowid();
        let rep = established(&s, "rep");
        // Not their own.
        let own = member(&s.conn, s.author);
        assert_eq!(
            file_now(&s, &own, ReportTarget::Measurement(mid), "spam", "")
                .expect_err("own")
                .status,
            StatusCode::UNPROCESSABLE_ENTITY
        );
        file_now(&s, &rep, ReportTarget::Measurement(mid), "misleading", "").expect("file");
        let snap: String = s
            .conn
            .query_row("SELECT snapshot FROM reports", [], |r| r.get(0))
            .expect("snap");
        assert!(snap.contains("glossary") && snap.contains("set1") && snap.contains("m1"));
        assert!(
            file_now(&s, &rep, ReportTarget::Measurement(999), "spam", "").is_err(),
            "missing"
        );
    }

    #[test]
    fn one_report_per_member_and_target() {
        let s = setup();
        let rep = established(&s, "rep");
        let other = established(&s, "other");
        file_now(&s, &rep, ReportTarget::Post(s.post), "spam", "").expect("first");
        let e = file_now(&s, &rep, ReportTarget::Post(s.post), "abuse", "").expect_err("again");
        assert_eq!(e.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(e.form.as_ref().map(|f| f.field.as_str()), Some("target"));
        // Another member, or another target of the same member, is fine.
        file_now(&s, &other, ReportTarget::Post(s.post), "spam", "").expect("other member");
        file_now(&s, &rep, ReportTarget::Comment(s.comment), "spam", "").expect("other target");
    }

    #[test]
    fn nobody_reports_their_own_content_or_account() {
        let s = setup();
        let own = member(&s.conn, s.author);
        for target in [
            ReportTarget::Post(s.post),
            ReportTarget::Comment(s.comment),
            ReportTarget::User(s.author),
        ] {
            let e = file_now(&s, &own, target, "spam", "").expect_err("own");
            assert_eq!(e.status, StatusCode::UNPROCESSABLE_ENTITY, "{target:?}");
        }
    }

    #[test]
    fn restricted_and_banned_members_cannot_report() {
        let s = setup();
        let mut rep = established(&s, "rep");
        rep.restricted_until = Some(NOW + Duration::days(1));
        assert_eq!(
            file_now(&s, &rep, ReportTarget::Post(s.post), "spam", "")
                .expect_err("restricted")
                .status,
            StatusCode::FORBIDDEN
        );
        rep.restricted_until = Some(NOW);
        file_now(&s, &rep, ReportTarget::Post(s.post), "spam", "").expect("restriction ended");
        rep.banned = true;
        assert!(file_now(&s, &rep, ReportTarget::Comment(s.comment), "spam", "").is_err());
    }

    #[test]
    fn targets_must_be_visible_and_exist() {
        let s = setup();
        let rep = established(&s, "rep");
        for st in ["hidden", "removed", "deleted"] {
            state(&s.conn, "posts", s.post, st);
            state(&s.conn, "comments", s.comment, st);
            for target in [ReportTarget::Post(s.post), ReportTarget::Comment(s.comment)] {
                assert_eq!(
                    file_now(&s, &rep, target, "spam", "").expect_err(st).status,
                    StatusCode::NOT_FOUND,
                    "{st} {target:?}"
                );
            }
        }
        for target in [
            ReportTarget::Post(99),
            ReportTarget::Comment(99),
            ReportTarget::User(99),
        ] {
            assert_eq!(
                file_now(&s, &rep, target, "spam", "")
                    .expect_err("missing")
                    .status,
                StatusCode::NOT_FOUND
            );
        }
        s.conn
            .execute(
                "UPDATE users SET deleted_at = 'x' WHERE id = ?1",
                [s.author],
            )
            .expect("delete");
        assert!(file_now(&s, &rep, ReportTarget::User(s.author), "spam", "").is_err());
    }

    #[test]
    fn reason_and_note_rules() {
        let s = setup();
        let rep = established(&s, "rep");
        let post = ReportTarget::Post(s.post);
        for bad in ["", "ring", "off-topic", "SPAM"] {
            let e = file_now(&s, &rep, post, bad, "").expect_err(bad);
            assert_eq!(e.form.as_ref().map(|f| f.field.as_str()), Some("reason"));
        }
        assert!(file_now(&s, &rep, post, "spam", &"a".repeat(1_001)).is_err());
        assert!(file_now(&s, &rep, post, "spam", "bad\u{7}note").is_err());
        assert!(file_now(&s, &rep, post, "spam", "two\nlines").is_err());
        // 1,000 characters, counted as scalar values, are fine.
        file_now(&s, &rep, post, "spam", &"é".repeat(1_000)).expect("1000 chars");
        for reason in REASONS {
            let r = established(&s, &format!("r-{reason}"));
            let fresh_post = crate::posts::testkit::post(&s.conn, s.author, "p", NOW);
            file_now(&s, &r, ReportTarget::Post(fresh_post), reason, "")
                .unwrap_or_else(|e| panic!("{reason}: {e}"));
        }
    }

    #[test]
    fn three_established_reporters_hide_a_post_with_one_audit_row() {
        let s = setup();
        let reporters: Vec<Member> = (0..3).map(|i| established(&s, &format!("r{i}"))).collect();
        file_now(&s, &reporters[0], ReportTarget::Post(s.post), "spam", "").expect("1");
        file_now(&s, &reporters[1], ReportTarget::Post(s.post), "spam", "").expect("2");
        assert_eq!(state_of(&s, "posts", s.post), "visible");
        file_now(&s, &reporters[2], ReportTarget::Post(s.post), "abuse", "").expect("3");
        assert_eq!(state_of(&s, "posts", s.post), "hidden");
        let rows: Vec<(Option<i64>, String, String, String, bool, String)> = s
            .conn
            .prepare(
                "SELECT actor_id, action, target_type, target_id, public, details FROM audit_log",
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
                ))
            })
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("rows");
        assert_eq!(
            rows,
            vec![(
                None,
                "auto.hide".to_string(),
                "post".to_string(),
                s.post.to_string(),
                true,
                r#"{"reports":3}"#.to_string()
            )]
        );
        // A report on the hidden post is refused (not visible).
        let late = established(&s, "late");
        assert!(file_now(&s, &late, ReportTarget::Post(s.post), "spam", "").is_err());
    }

    #[test]
    fn auto_hide_counts_only_reporters_established_when_the_third_is_filed() {
        let s = setup();
        let newcomer = {
            let id = user(&s.conn, "new", NOW - Duration::days(1), old());
            member(&s.conn, id)
        };
        let young_github = {
            let id = user(&s.conn, "young", old(), NOW - Duration::days(30));
            member(&s.conn, id)
        };
        let a = established(&s, "a");
        let b = established(&s, "b");
        let target = ReportTarget::Comment(s.comment);
        file_now(&s, &newcomer, target, "spam", "").expect("new");
        file_now(&s, &young_github, target, "spam", "").expect("young");
        file_now(&s, &a, target, "spam", "").expect("a");
        file_now(&s, &b, target, "spam", "").expect("b");
        assert_eq!(
            state_of(&s, "comments", s.comment),
            "visible",
            "four reports, two established"
        );
        // A reporter restricted since is not counted; one whose restriction ended is.
        let c = established(&s, "c");
        s.conn
            .execute(
                "UPDATE users SET restricted_until = ?1 WHERE id = ?2",
                params![clock::storage(NOW + Duration::days(1)), a.id.0],
            )
            .expect("restrict");
        file_now(&s, &c, target, "spam", "").expect("c");
        assert_eq!(
            state_of(&s, "comments", s.comment),
            "visible",
            "a is restricted"
        );
        // The third established reporter arrives later: a is no longer restricted.
        s.conn
            .execute(
                "UPDATE users SET restricted_until = ?1 WHERE id = ?2",
                params![clock::storage(NOW - Duration::days(1)), a.id.0],
            )
            .expect("lift");
        let d = established(&s, "d");
        file_now(&s, &d, target, "spam", "").expect("d");
        assert_eq!(state_of(&s, "comments", s.comment), "hidden");
        let n: i64 = s
            .conn
            .query_row(
                "SELECT COUNT(*) FROM audit_log WHERE action = 'auto.hide' AND target_type = 'comment'",
                [],
                |r| r.get(0),
            )
            .expect("count");
        assert_eq!(n, 1);
    }

    #[test]
    fn resolved_reports_and_system_reports_do_not_count_towards_the_hide() {
        let s = setup();
        let target = ReportTarget::Post(s.post);
        let a = established(&s, "a");
        let b = established(&s, "b");
        let c = established(&s, "c");
        file_now(&s, &a, target, "spam", "").expect("a");
        file_now(&s, &b, target, "spam", "").expect("b");
        {
            let tx = s.conn.unchecked_transaction().expect("tx");
            file_system_report(&tx, target, "misleading", json!({}), NOW).expect("system");
            tx.commit().expect("commit");
        }
        assert_eq!(
            state_of(&s, "posts", s.post),
            "visible",
            "two reporters and the system"
        );
        file_now(&s, &c, target, "spam", "").expect("c");
        assert_eq!(state_of(&s, "posts", s.post), "hidden", "a, b, c");
        // The same with one of them resolved first.
        let p2 = post(&s.conn, s.author, "second", NOW);
        let t2 = ReportTarget::Post(p2);
        file_now(&s, &a, t2, "spam", "").expect("a");
        file_now(&s, &b, t2, "spam", "").expect("b");
        {
            let tx = s.conn.unchecked_transaction().expect("tx");
            let n = resolve_target(&tx, t2, Resolution::Dismissed, None, NOW).expect("resolve");
            assert_eq!(n, 2);
            tx.commit().expect("commit");
        }
        file_now(&s, &c, t2, "spam", "").expect("c");
        assert_eq!(
            state_of(&s, "posts", p2),
            "visible",
            "resolved reports do not count"
        );
    }

    #[test]
    fn a_late_third_report_cannot_undo_a_removal() {
        let s = setup();
        let target = ReportTarget::Post(s.post);
        let a = established(&s, "a");
        let b = established(&s, "b");
        let c = established(&s, "c");
        file_now(&s, &a, target, "spam", "").expect("a");
        file_now(&s, &b, target, "spam", "").expect("b");
        state(&s.conn, "posts", s.post, "removed");
        // The post is removed: a third report is refused, state is unchanged.
        assert!(file_now(&s, &c, target, "spam", "").is_err());
        assert_eq!(state_of(&s, "posts", s.post), "removed");
    }

    #[test]
    fn measurement_and_member_reports_never_auto_hide() {
        let s = setup();
        for i in 0..4 {
            let r = established(&s, &format!("r{i}"));
            file_now(&s, &r, ReportTarget::User(s.author), "spam", "").expect("report");
        }
        let audit: i64 = s
            .conn
            .query_row("SELECT COUNT(*) FROM audit_log", [], |r| r.get(0))
            .expect("count");
        assert_eq!(audit, 0);
    }

    #[test]
    fn a_report_survives_the_items_deletion_with_its_snapshot() {
        let s = setup();
        let rep = established(&s, "rep");
        s.conn
            .execute(
                "UPDATE posts SET body = 'secret text' WHERE id = ?1",
                [s.post],
            )
            .expect("body");
        file_now(&s, &rep, ReportTarget::Post(s.post), "abuse", "note").expect("file");
        let author = member(&s.conn, s.author);
        {
            let tx = s.conn.unchecked_transaction().expect("tx");
            posts::delete_post(&tx, &author, s.post).expect("delete");
            tx.commit().expect("commit");
        }
        let queue = open_queue(&s.conn, 1).expect("queue");
        assert_eq!(queue.rows.len(), 1);
        let entry = &queue.rows[0];
        assert_eq!(entry.preview, "[deleted]");
        assert_eq!(entry.reports[0].snapshot, "a title\n\nsecret text");
        assert_eq!(
            entry
                .described
                .as_ref()
                .and_then(|d| d.state.clone())
                .as_deref(),
            Some("deleted")
        );
    }

    #[test]
    fn system_reports_and_resolution_helpers() {
        let s = setup();
        let moderator = established(&s, "mod");
        let tx = s.conn.unchecked_transaction().expect("tx");
        let target = ReportTarget::User(s.author);
        let sys = file_system_report(&tx, target, "ring", json!({"accounts": [1, 2]}), NOW)
            .expect("system");
        tx.commit().expect("commit");
        let (reporter, system, details, snapshot): (Option<i64>, bool, String, String) = s
            .conn
            .query_row(
                "SELECT reporter_id, system, details, snapshot FROM reports WHERE id = ?1",
                [sys],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .expect("row");
        assert_eq!(
            (reporter, system, details.as_str(), snapshot.as_str()),
            (None, true, r#"{"accounts":[1,2]}"#, "author")
        );
        // Resolution: one report, then the rest of a target.
        let rep = established(&s, "rep");
        file_now(&s, &rep, target, "spam", "").expect("member report");
        assert_eq!(open_count(&s.conn, target), Ok(2));
        let later = NOW + Duration::hours(1);
        let tx = s.conn.unchecked_transaction().expect("tx");
        assert_eq!(
            resolve_report(&tx, sys, Resolution::Upheld, Some(moderator.id), later),
            Ok(true)
        );
        assert_eq!(
            resolve_report(&tx, sys, Resolution::Dismissed, Some(moderator.id), later),
            Ok(false),
            "already resolved"
        );
        assert_eq!(
            resolve_target(&tx, target, Resolution::Dismissed, None, later),
            Ok(1)
        );
        tx.commit().expect("commit");
        let rows: Vec<(String, Option<i64>, String)> = s
            .conn
            .prepare("SELECT status, resolved_by, resolved_at FROM reports ORDER BY id")
            .expect("prepare")
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("rows");
        assert_eq!(
            rows,
            vec![
                ("upheld".into(), Some(moderator.id.0), clock::storage(later)),
                ("dismissed".into(), None, clock::storage(later)),
            ]
        );
        assert_eq!(open_count(&s.conn, target), Ok(0));
    }

    #[test]
    fn the_queue_lists_targets_newest_first_with_counts_reporters_and_previews() {
        let s = setup();
        let p2 = post(&s.conn, s.author, &"x".repeat(100), NOW);
        let a = established(&s, "alice");
        let b = established(&s, "bob");
        let gone = established(&s, "gone");
        let at = |m: i64| NOW + Duration::minutes(m);
        let file_at =
            |who: &Member, target: ReportTarget, reason: &str, note: &str, t: OffsetDateTime| {
                let tx = s.conn.unchecked_transaction().expect("tx");
                file(&tx, who, target, reason, note, t).expect("file");
                tx.commit().expect("commit");
            };
        file_at(&a, ReportTarget::Post(s.post), "spam", "n1", at(0));
        file_at(&b, ReportTarget::Post(s.post), "spam", "n2", at(1));
        file_at(&gone, ReportTarget::Post(s.post), "abuse", "n3", at(2));
        file_at(&a, ReportTarget::Comment(s.comment), "other", "", at(10));
        file_at(&a, ReportTarget::Post(p2), "malicious", "", at(5));
        {
            let tx = s.conn.unchecked_transaction().expect("tx");
            file_system_report(
                &tx,
                ReportTarget::User(s.author),
                "ring",
                json!({"accounts": [1]}),
                at(20),
            )
            .expect("system");
            tx.commit().expect("commit");
        }
        // The reporter `gone` deletes their account.
        {
            let tx = s.conn.unchecked_transaction().expect("tx");
            users::delete_account(&tx, gone.id, Some(gone.id), at(30)).expect("delete");
            tx.commit().expect("commit");
        }
        let queue = open_queue(&s.conn, 1).expect("queue");
        assert!(!queue.more);
        let targets: Vec<ReportTarget> = queue.rows.iter().map(|e| e.target).collect();
        assert_eq!(
            targets,
            vec![
                ReportTarget::User(s.author),
                ReportTarget::Comment(s.comment),
                ReportTarget::Post(p2),
                ReportTarget::Post(s.post),
            ]
        );
        let post_entry = &queue.rows[3];
        assert_eq!(
            post_entry.counts,
            vec![("spam".to_string(), 2), ("abuse".to_string(), 1)]
        );
        assert_eq!(post_entry.newest, at(2));
        assert_eq!(post_entry.preview, "a title");
        let reporters: Vec<Reporter> = post_entry
            .reports
            .iter()
            .map(|r| r.reporter.clone())
            .collect();
        assert_eq!(
            reporters,
            vec![
                Reporter::Member {
                    id: a.id.0,
                    login: "alice".into()
                },
                Reporter::Member {
                    id: b.id.0,
                    login: "bob".into()
                },
                Reporter::Deleted,
            ]
        );
        assert_eq!(post_entry.reports[0].note, "n1");
        assert_eq!(post_entry.reports[0].snapshot, "a title\n\nbody");
        let system_entry = &queue.rows[0];
        assert_eq!(system_entry.reports[0].reporter, Reporter::System);
        assert_eq!(system_entry.reports[0].details, json!({"accounts": [1]}));
        assert_eq!(system_entry.preview, "author");
        assert_eq!(queue.rows[2].preview.chars().count(), 100);
        assert!(!queue.rows[2].forward_to_registry(), "not a pack's post");
        // Resolved reports leave the queue.
        {
            let tx = s.conn.unchecked_transaction().expect("tx");
            resolve_target(
                &tx,
                ReportTarget::Post(s.post),
                Resolution::Upheld,
                None,
                at(40),
            )
            .expect("resolve");
            tx.commit().expect("commit");
        }
        assert_eq!(open_queue(&s.conn, 1).expect("queue").rows.len(), 3);
        assert!(open_queue(&s.conn, 2).expect("p2").rows.is_empty());
    }

    #[test]
    fn a_malicious_report_on_a_packs_post_asks_to_forward_it() {
        let s = setup();
        let pack = pack_post(&s.conn, "glossary", 5, "[]", NOW);
        let rep = established(&s, "rep");
        file_now(&s, &rep, ReportTarget::Post(pack), "malicious", "").expect("file");
        let queue = open_queue(&s.conn, 1).expect("queue");
        assert!(queue.rows[0].forward_to_registry());
        assert_eq!(
            queue.rows[0]
                .described
                .as_ref()
                .and_then(|d| d.pack.clone())
                .as_deref(),
            Some("glossary")
        );
    }

    #[test]
    fn the_queue_pages_by_fifty() {
        let s = setup();
        s.conn
            .execute(
                "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 55)
                 INSERT INTO reports (system, target_type, target_id, reason, created_at)
                 SELECT 1, 'user', i + 100, 'ring', strftime('%Y-%m-%dT%H:%M:%S.000000Z', '2026-11-08 09:00:00', '-' || i || ' minutes') FROM n",
                [],
            )
            .expect("bulk");
        let p1 = open_queue(&s.conn, 1).expect("p1");
        assert_eq!((p1.rows.len(), p1.more), (50, true));
        let p2 = open_queue(&s.conn, 2).expect("p2");
        assert_eq!((p2.rows.len(), p2.more), (5, false));
        assert_eq!(p1.rows[0].target, ReportTarget::User(101));
        assert_eq!(
            p1.rows[0].preview, "[deleted]",
            "a target that no longer exists"
        );
    }

    #[test]
    fn report_text_goes_ninety_days_after_resolution() {
        let s = setup();
        let rep = established(&s, "rep");
        let r1 = file_now(&s, &rep, ReportTarget::Post(s.post), "spam", "note").expect("1");
        let r2 = file_now(&s, &rep, ReportTarget::Comment(s.comment), "spam", "note").expect("2");
        let r3 = file_now(&s, &rep, ReportTarget::User(s.author), "spam", "note").expect("3");
        let tx = s.conn.unchecked_transaction().expect("tx");
        let day = |n: i64| NOW + Duration::days(n);
        resolve_report(&tx, r1, Resolution::Upheld, None, day(0)).expect("r1");
        resolve_report(&tx, r2, Resolution::Dismissed, None, day(1)).expect("r2");
        // r3 stays open.
        assert_eq!(purge_resolved_text(&tx, day(89)), Ok(0));
        assert_eq!(
            purge_resolved_text(&tx, day(90)),
            Ok(1),
            "r1 at exactly 90 days"
        );
        assert_eq!(
            purge_resolved_text(&tx, day(500)),
            Ok(1),
            "r2 later; r3 is open"
        );
        assert_eq!(
            purge_resolved_text(&tx, day(500)),
            Ok(0),
            "nothing left to blank"
        );
        tx.commit().expect("commit");
        let text = |id: i64| -> (String, String, String) {
            s.conn
                .query_row(
                    "SELECT note, snapshot, status FROM reports WHERE id = ?1",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .expect("row")
        };
        assert_eq!(text(r1), (String::new(), String::new(), "upheld".into()));
        assert_eq!(text(r2), (String::new(), String::new(), "dismissed".into()));
        assert_eq!(text(r3), ("note".into(), "author".into(), "open".into()));
    }
}
