//! Per-member limits and the content rules.
//!
//! Limits are counted from the database over a sliding window, so a restart
//! does not reset them. They apply to every role. The per-address limits are
//! in memory, in `web::security`.

use std::fmt;

use rusqlite::Connection;
use time::{Duration, OffsetDateTime};

use crate::clock;
use crate::db::UserId;
use crate::error::{AppError, FormError};
use crate::policy::{self, Limit};
use crate::users::{self, Standing};

/// What a limit counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Discussion posts.
    Post,
    Comment,
    Vote,
    Report,
    /// Measurements entered or replaced.
    Measurement,
}

impl Action {
    fn noun(self) -> &'static str {
        match self {
            Action::Post => "discussion posts",
            Action::Comment => "comments",
            Action::Vote => "votes",
            Action::Report => "reports",
            Action::Measurement => "measurements",
        }
    }

    /// The count's SQL: rows by this member from `since` on, oldest first.
    fn sql(self) -> &'static str {
        match self {
            Action::Post => {
                "SELECT created_at FROM posts
                 WHERE author_id = ?1 AND kind = 'discussion' AND created_at > ?2
                 ORDER BY created_at"
            }
            Action::Comment => {
                "SELECT created_at FROM comments
                 WHERE author_id = ?1 AND created_at > ?2 ORDER BY created_at"
            }
            Action::Vote => "SELECT at FROM votes WHERE user_id = ?1 AND at > ?2 ORDER BY at",
            Action::Report => {
                "SELECT created_at FROM reports
                 WHERE reporter_id = ?1 AND created_at > ?2 ORDER BY created_at"
            }
            Action::Measurement => {
                "SELECT updated_at FROM measurements
                 WHERE user_id = ?1 AND updated_at > ?2 ORDER BY updated_at"
            }
        }
    }
}

/// A limit that has been reached.
///
/// `count == 0` in `limit` means the action is not allowed to this member at
/// all (a new member's measurements; any action by a restricted or banned
/// member); then `resets_at` is the time asked about, since nothing here
/// resets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limited {
    pub action: Action,
    pub limit: Limit,
    pub resets_at: OffsetDateTime,
}

impl Limited {
    /// Whether the action is closed to this member rather than used up.
    pub fn not_allowed(&self) -> bool {
        self.limit.count == 0
    }

    /// The limit in words, as a page names it: "5 discussion posts per 24 hours".
    pub fn describe(&self) -> String {
        let secs = self.limit.window_secs;
        let window = if secs % 86_400 == 0 {
            let d = secs / 86_400;
            if d == 1 {
                "24 hours".to_string()
            } else {
                format!("{d} days")
            }
        } else if secs % 3_600 == 0 {
            let h = secs / 3_600;
            if h == 1 {
                "hour".to_string()
            } else {
                format!("{h} hours")
            }
        } else {
            format!("{secs} seconds")
        };
        format!("{} {} per {window}", self.limit.count, self.action.noun())
    }
}

impl fmt::Display for Limited {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.not_allowed() {
            write!(f, "{} are not open to your account.", self.action.noun())
        } else {
            write!(
                f,
                "The limit is {}. It resets at {}.",
                self.describe(),
                clock::rfc3339(self.resets_at)
            )
        }
    }
}

impl Limited {
    /// The page's answer: 429 with the seconds until the reset as
    /// `Retry-After`, or 403 when the action is not open to the member.
    pub fn into_error(self, now: OffsetDateTime) -> AppError {
        if self.not_allowed() {
            AppError::forbidden(self.to_string())
        } else {
            let wait = (self.resets_at - now).whole_seconds().max(1);
            AppError::too_many_requests(self.to_string(), wait as u64)
        }
    }
}

/// The limits an action has for a standing; `None` where the action is not
/// open at all.
fn limits_for(action: Action, standing: Standing) -> Vec<Limit> {
    let established = match standing {
        Standing::Established => true,
        Standing::New => false,
        Standing::Restricted | Standing::Banned => return Vec::new(),
    };
    match (action, established) {
        (Action::Post, true) => vec![policy::POSTS_ESTABLISHED],
        (Action::Post, false) => vec![policy::POSTS_NEW],
        (Action::Comment, true) => {
            vec![
                policy::COMMENTS_ESTABLISHED_HOUR,
                policy::COMMENTS_ESTABLISHED_DAY,
            ]
        }
        (Action::Comment, false) => vec![policy::COMMENTS_NEW],
        (Action::Vote, _) => vec![policy::VOTES],
        (Action::Report, true) => vec![policy::REPORTS_ESTABLISHED],
        (Action::Report, false) => vec![policy::REPORTS_NEW],
        (Action::Measurement, true) => vec![policy::MEASUREMENTS_ESTABLISHED],
        (Action::Measurement, false) => Vec::new(),
    }
}

/// Whether `user` may take `action` at `now`, counting their rows in each
/// window by the table's time column. If more than one window is full, the
/// answer is the one that resets last. `resets_at` is when enough of the
/// oldest counted rows have left the window to free one place (the oldest row,
/// when the count equals the limit).
///
/// A restricted or banned member, a new member's measurement, or an account
/// that does not exist is refused with a limit of 0.
pub fn check(
    conn: &Connection,
    user: UserId,
    action: Action,
    now: OffsetDateTime,
) -> Result<(), Limited> {
    // A database failure is treated as "no rows"; callers' own queries on the
    // same connection surface it.
    let standing = users::load_member(conn, user)
        .ok()
        .flatten()
        .map_or(Standing::Banned, |m| users::standing(&m, now));
    let limits = limits_for(action, standing);
    if limits.is_empty() {
        return Err(Limited {
            action,
            limit: Limit {
                count: 0,
                window_secs: policy::VOTES.window_secs,
            },
            resets_at: now,
        });
    }
    let mut worst: Option<Limited> = None;
    for limit in limits {
        let since = clock::storage(now - Duration::seconds(limit.window_secs));
        let times: Vec<String> = conn
            .prepare(action.sql())
            .and_then(|mut stmt| {
                stmt.query_map(rusqlite::params![user.0, since], |r| r.get(0))?
                    .collect()
            })
            .unwrap_or_default();
        let count = limit.count as usize;
        if times.len() < count {
            continue;
        }
        let freeing = &times[times.len() - count];
        let resets_at =
            clock::parse_storage(freeing).map_or(now, |t| t + Duration::seconds(limit.window_secs));
        let hit = Limited {
            action,
            limit,
            resets_at,
        };
        if worst.is_none_or(|w| hit.resets_at > w.resets_at) {
            worst = Some(hit);
        }
    }
    worst.map_or(Ok(()), Err)
}

/// A content rule a text broke.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentRule {
    /// A new member's discussion may not have a link URL.
    LinkUrl,
    /// A new member's text must render with no hyperlink.
    Hyperlink,
    /// A new member's text may not contain `http://` or `https://`.
    UrlText,
}

impl ContentRule {
    /// The rule in words, for the form.
    pub fn rule(self) -> &'static str {
        match self {
            ContentRule::LinkUrl => "new members cannot post links yet",
            ContentRule::Hyperlink => "new members cannot include links yet",
            ContentRule::UrlText => "new members cannot write http:// or https:// yet",
        }
    }

    /// The form error for the field the text came from; a broken link rule is
    /// always the `link` field's.
    pub fn form_error(self, field: &str) -> FormError {
        let field = if self == ContentRule::LinkUrl {
            "link"
        } else {
            field
        };
        FormError::new(field, self.rule())
    }
}

/// The content rules, checked on every post
/// (title, link URL and body), comment and edit. `text` is one of those
/// texts, `rendered_links` the Markdown renderer's link count for it and
/// `link_url` the post's link URL, if any (pass it with each text of the
/// post, or alone with an empty text). Established members have no rule
/// beyond the limits; restricted and banned members cannot publish at all.
pub fn content_rule(
    text: &str,
    rendered_links: usize,
    link_url: Option<&str>,
    standing: Standing,
) -> Result<(), ContentRule> {
    if standing != Standing::New {
        return Ok(());
    }
    if link_url.is_some() {
        return Err(ContentRule::LinkUrl);
    }
    if rendered_links > 0 {
        return Err(ContentRule::Hyperlink);
    }
    let lower = text.to_ascii_lowercase();
    if lower.contains("http://") || lower.contains("https://") {
        return Err(ContentRule::UrlText);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown;
    use crate::posts::testkit::*;
    use time::macros::datetime;

    const NOW: OffsetDateTime = datetime!(2026-11-08 10:00:00 UTC);

    fn old() -> OffsetDateTime {
        NOW - Duration::days(400)
    }

    fn established(conn: &Connection) -> i64 {
        user(conn, "est", old(), old())
    }

    fn newcomer(conn: &Connection) -> i64 {
        user(conn, "new", NOW - Duration::days(1), old())
    }

    /// `n` posts by `uid`, the first at `first` and one minute apart.
    fn posts(conn: &Connection, uid: i64, n: usize, first: OffsetDateTime) {
        for i in 0..n {
            post(conn, uid, "t", first + Duration::minutes(i as i64));
        }
    }

    #[test]
    fn posts_established_and_new_with_resets_at() {
        let (_d, conn) = fresh();
        let est = established(&conn);
        let new = newcomer(&conn);
        let start = NOW - Duration::hours(20);
        posts(&conn, est, 4, start);
        assert_eq!(check(&conn, UserId(est), Action::Post, NOW), Ok(()));
        post(&conn, est, "t", start + Duration::minutes(4));
        let e = check(&conn, UserId(est), Action::Post, NOW).expect_err("5th");
        assert_eq!(e.limit, policy::POSTS_ESTABLISHED);
        assert_eq!(e.resets_at, start + Duration::hours(24));
        // New: one per 24 hours.
        assert_eq!(check(&conn, UserId(new), Action::Post, NOW), Ok(()));
        post(&conn, new, "t", NOW - Duration::hours(3));
        let e = check(&conn, UserId(new), Action::Post, NOW).expect_err("new");
        assert_eq!(e.limit, policy::POSTS_NEW);
        assert_eq!(e.resets_at, NOW + Duration::hours(21));
        assert_eq!(e.describe(), "1 discussion posts per 24 hours");
        // The row leaves the window at exactly its reset time.
        assert_eq!(check(&conn, UserId(new), Action::Post, e.resets_at), Ok(()));
        assert!(check(
            &conn,
            UserId(new),
            Action::Post,
            e.resets_at - Duration::seconds(1)
        )
        .is_err());
    }

    #[test]
    fn a_deleted_post_still_counts_and_pack_posts_do_not() {
        let (_d, conn) = fresh();
        let new = newcomer(&conn);
        let id = post(&conn, new, "t", NOW - Duration::hours(1));
        state(&conn, "posts", id, "deleted");
        assert!(check(&conn, UserId(new), Action::Post, NOW).is_err());
        let est = established(&conn);
        pack_post(&conn, "p", 1, "[]", NOW);
        assert_eq!(check(&conn, UserId(est), Action::Post, NOW), Ok(()));
    }

    #[test]
    fn comments_have_an_hour_and_a_day_window_for_established_members() {
        let (_d, conn) = fresh();
        let est = established(&conn);
        let new = newcomer(&conn);
        let p = post(&conn, est, "t", NOW - Duration::days(2));
        for i in 0..59 {
            comment(&conn, p, est, NOW - Duration::minutes(i));
        }
        assert_eq!(check(&conn, UserId(est), Action::Comment, NOW), Ok(()));
        comment(&conn, p, est, NOW - Duration::minutes(59));
        let e = check(&conn, UserId(est), Action::Comment, NOW).expect_err("hour");
        assert_eq!(e.limit, policy::COMMENTS_ESTABLISHED_HOUR);
        assert_eq!(e.resets_at, NOW + Duration::minutes(1));
        // 300 in the day, spread so the hour is clear: the day limit answers.
        let (_d2, conn2) = fresh();
        let est2 = established(&conn2);
        let p2 = post(&conn2, est2, "t", NOW - Duration::days(2));
        for i in 0..300 {
            comment(
                &conn2,
                p2,
                est2,
                NOW - Duration::hours(2) - Duration::seconds(i),
            );
        }
        let e = check(&conn2, UserId(est2), Action::Comment, NOW).expect_err("day");
        assert_eq!(e.limit, policy::COMMENTS_ESTABLISHED_DAY);
        assert_eq!(
            e.resets_at,
            NOW - Duration::hours(2) - Duration::seconds(299) + Duration::days(1)
        );
        // New: 5 per 24 hours.
        let pn = post(&conn, est, "t2", NOW);
        for i in 0..4 {
            comment(&conn, pn, new, NOW - Duration::hours(i));
        }
        assert_eq!(check(&conn, UserId(new), Action::Comment, NOW), Ok(()));
        comment(&conn, pn, new, NOW - Duration::hours(10));
        let e = check(&conn, UserId(new), Action::Comment, NOW).expect_err("new");
        assert_eq!(e.limit, policy::COMMENTS_NEW);
        assert_eq!(e.resets_at, NOW - Duration::hours(10) + Duration::days(1));
    }

    #[test]
    fn votes_limit_with_resets_at() {
        let (_d, conn) = fresh();
        let est = established(&conn);
        let new = newcomer(&conn);
        let author = user(&conn, "author", old(), old());
        let p = post(&conn, author, "t", NOW - Duration::days(1));
        let ids: Vec<i64> = (0..300)
            .map(|_| post(&conn, author, "t", NOW - Duration::days(1)))
            .collect();
        for who in [est, new] {
            let first = NOW - Duration::minutes(30);
            for (i, id) in ids.iter().enumerate() {
                cast(&conn, who, "post", *id, first + Duration::seconds(i as i64));
            }
            let e = check(&conn, UserId(who), Action::Vote, NOW).expect_err("300");
            assert_eq!(e.limit, policy::VOTES);
            assert_eq!(e.resets_at, first + Duration::hours(1));
            // Withdrawing one frees a place.
            conn.execute(
                "DELETE FROM votes WHERE user_id = ?1 AND target_id = ?2",
                [who, ids[0]],
            )
            .expect("withdraw");
            assert_eq!(check(&conn, UserId(who), Action::Vote, NOW), Ok(()));
            // A vote older than the hour is not counted.
            cast(&conn, who, "post", p, NOW - Duration::hours(2));
            assert_eq!(check(&conn, UserId(who), Action::Vote, NOW), Ok(()));
        }
    }

    #[test]
    fn reports_by_standing() {
        let (_d, conn) = fresh();
        let est = established(&conn);
        let new = newcomer(&conn);
        let report = |who: i64, n: i64, at: OffsetDateTime| {
            conn.execute(
                "INSERT INTO reports (reporter_id, target_type, target_id, reason, created_at)
                 VALUES (?1, 'post', ?2, 'spam', ?3)",
                rusqlite::params![who, n, clock::storage(at)],
            )
            .expect("report");
        };
        for i in 0..4 {
            report(new, i, NOW - Duration::hours(1) + Duration::minutes(i));
        }
        assert_eq!(check(&conn, UserId(new), Action::Report, NOW), Ok(()));
        report(new, 4, NOW - Duration::hours(1) + Duration::minutes(4));
        let e = check(&conn, UserId(new), Action::Report, NOW).expect_err("5");
        assert_eq!(e.limit, policy::REPORTS_NEW);
        assert_eq!(e.resets_at, NOW - Duration::hours(1) + Duration::days(1));
        for i in 0..19 {
            report(est, i, NOW - Duration::hours(2));
        }
        assert_eq!(check(&conn, UserId(est), Action::Report, NOW), Ok(()));
        report(est, 19, NOW - Duration::hours(2));
        let e = check(&conn, UserId(est), Action::Report, NOW).expect_err("20");
        assert_eq!(e.limit, policy::REPORTS_ESTABLISHED);
    }

    #[test]
    fn measurements_are_20_a_day_for_established_and_closed_to_new_members() {
        let (_d, conn) = fresh();
        let est = established(&conn);
        let new = newcomer(&conn);
        pack_post(&conn, "x", 1, "[]", NOW);
        for i in 0..20 {
            conn.execute(
                "INSERT INTO pack_contents (pack, pack_hash, version, commit_id, first_seen)
                 VALUES ('x', ?1, '1', 'c', 'x')",
                [format!("h{i}")],
            )
            .expect("content");
            conn.execute(
                "INSERT INTO measurements (pack, pack_hash, user_id, eval_set, planted, kept, runs,
                    corrected_without, damaged_without, corrected_with, damaged_with, hippocamp, model,
                    created_at, updated_at)
                 VALUES ('x', ?1, ?2, 'e', 5, 5, 3, 0, 0, 0, 0, 'h', 'm', 'x', ?3)",
                rusqlite::params![
                    format!("h{i}"),
                    est,
                    clock::storage(NOW - Duration::hours(5) + Duration::minutes(i))
                ],
            )
            .expect("measurement");
            if i == 18 {
                assert_eq!(check(&conn, UserId(est), Action::Measurement, NOW), Ok(()));
            }
        }
        let e = check(&conn, UserId(est), Action::Measurement, NOW).expect_err("20");
        assert_eq!(e.limit, policy::MEASUREMENTS_ESTABLISHED);
        assert_eq!(e.resets_at, NOW - Duration::hours(5) + Duration::days(1));
        let e = check(&conn, UserId(new), Action::Measurement, NOW).expect_err("new");
        assert!(e.not_allowed());
        assert_eq!(e.into_error(NOW).status, axum::http::StatusCode::FORBIDDEN);
    }

    #[test]
    fn restricted_banned_and_unknown_accounts_are_refused() {
        let (_d, conn) = fresh();
        let restricted = established(&conn);
        conn.execute(
            "UPDATE users SET restricted_until = ?1 WHERE id = ?2",
            rusqlite::params![clock::storage(NOW + Duration::days(1)), restricted],
        )
        .expect("restrict");
        for action in [
            Action::Post,
            Action::Comment,
            Action::Vote,
            Action::Report,
            Action::Measurement,
        ] {
            assert!(check(&conn, UserId(restricted), action, NOW)
                .expect_err("r")
                .not_allowed());
            assert!(check(&conn, UserId(999), action, NOW).is_err());
        }
        // Once the restriction ends the limits apply again.
        assert_eq!(
            check(
                &conn,
                UserId(restricted),
                Action::Post,
                NOW + Duration::days(1)
            ),
            Ok(())
        );
    }

    #[test]
    fn limited_becomes_429_with_retry_after() {
        let (_d, conn) = fresh();
        let new = newcomer(&conn);
        post(&conn, new, "t", NOW - Duration::hours(23));
        let e = check(&conn, UserId(new), Action::Post, NOW).expect_err("limited");
        let err = e.into_error(NOW);
        assert_eq!(err.status, axum::http::StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(err.retry_after, Some(3_600));
        assert!(err.message.contains("1 discussion posts per 24 hours"));
    }

    #[test]
    fn content_rule_for_new_and_established_members() {
        let rule = |text: &str, link: Option<&str>, standing| {
            let (_, links) = markdown::render(text);
            content_rule(text, links, link, standing)
        };
        let new = Standing::New;
        assert_eq!(rule("plain words", None, new), Ok(()));
        assert_eq!(
            rule("see [here](https://example.com)", None, new),
            Err(ContentRule::Hyperlink)
        );
        assert_eq!(
            rule("<https://example.com>", None, new),
            Err(ContentRule::Hyperlink)
        );
        assert_eq!(
            rule("go to HTTPS://example.com", None, new),
            Err(ContentRule::UrlText)
        );
        assert_eq!(rule("see hTtP://x", None, new), Err(ContentRule::UrlText));
        assert_eq!(
            rule("a title", Some("https://example.com/"), new),
            Err(ContentRule::LinkUrl)
        );
        assert_eq!(rule("example.com is fine", None, new), Ok(()));
        // Established members have no rule beyond the limits.
        for text in ["[x](https://example.com) https://a", "plain"] {
            assert_eq!(
                rule(text, Some("https://example.com/"), Standing::Established),
                Ok(())
            );
        }
        assert_eq!(ContentRule::LinkUrl.form_error("title").field, "link");
        assert_eq!(ContentRule::UrlText.form_error("body").field, "body");
    }
}
