//! Votes, counted votes and karma.
//!
//! Votes are stored as cast and counted when read: a vote counts only while
//! its voter is established at the reading time, and
//! never when the voter is the item's author or, on a pack's post, one of the
//! pack's maintainers. Nothing here stores a count.

use std::collections::HashMap;

use axum::http::StatusCode;
use rusqlite::{params_from_iter, types::Value, Connection, OptionalExtension, Transaction};
use time::{Duration, OffsetDateTime};

use crate::clock;
use crate::db::{Member, UserId};
use crate::error::AppError;
use crate::policy;
use crate::users::{self, Standing};

/// What a vote is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Target {
    Post(i64),
    Comment(i64),
}

impl Target {
    pub fn type_str(self) -> &'static str {
        match self {
            Target::Post(_) => "post",
            Target::Comment(_) => "comment",
        }
    }

    pub fn id(self) -> i64 {
        match self {
            Target::Post(id) | Target::Comment(id) => id,
        }
    }

    /// The target of a form's `target_type` and `target_id`.
    pub fn parse(target_type: &str, id: i64) -> Option<Self> {
        match target_type {
            "post" => Some(Target::Post(id)),
            "comment" => Some(Target::Comment(id)),
            _ => None,
        }
    }
}

/// SQL that is true when the user (`gid`, `login` name their columns) is one
/// of the maintainers of the pack named by column `pack`: its owner by
/// `owner_id`, a bound GitHub id, or a login in the entry's JSON list (login
/// compared without regard to case). A malformed list matches no one.
pub fn maintainer_sql(pack: &str, gid: &str, login: &str) -> String {
    format!(
        "EXISTS (SELECT 1 FROM packs pk WHERE pk.name = {pack} AND (
            pk.owner_id = {gid}
            OR EXISTS (SELECT 1 FROM pack_maintainer_ids m
                       WHERE m.pack = pk.name AND m.github_id = {gid})
            OR EXISTS (SELECT 1 FROM json_each(
                         CASE WHEN json_valid(pk.maintainers) THEN pk.maintainers ELSE '[]' END) j
                       WHERE j.type = 'text' AND j.value = {login} COLLATE NOCASE)))"
    )
}

/// The conditions of an established member over alias `u`: not banned, not
/// deleted, a feed account and a GitHub account old enough, and not under a
/// restriction. `?1` is the reading time, `?2` the latest feed-account
/// creation time of an established member and `?3` the latest GitHub-account
/// creation time (see [`cutoffs`]).
pub(crate) const ESTABLISHED_WHERE: &str = "u.banned = 0 AND u.deleted_at IS NULL
         AND u.created_at <= ?2
         AND u.github_created_at IS NOT NULL AND u.github_created_at <= ?3
         AND (u.restricted_until IS NULL OR u.restricted_until <= ?1)";

/// The conditions that make a vote count, over aliases `v` (the vote), `u`
/// (its voter), `p` (the post, or null) and `c` (the comment, or null), with
/// the parameters of [`ESTABLISHED_WHERE`].
fn counted_where() -> String {
    format!(
        "{ESTABLISHED_WHERE}
         AND u.id IS NOT COALESCE(p.author_id, c.author_id)
         AND NOT (p.pack IS NOT NULL AND {})",
        maintainer_sql("p.pack", "u.github_id", "u.login")
    )
}

pub(crate) fn cutoffs(now: OffsetDateTime) -> [Value; 3] {
    [
        Value::Text(clock::storage(now)),
        Value::Text(clock::storage(
            now - Duration::days(policy::ESTABLISHED_FEED_AGE_DAYS),
        )),
        Value::Text(clock::storage(
            now - Duration::days(policy::ESTABLISHED_GITHUB_AGE_DAYS),
        )),
    ]
}

/// The counted votes of each target at `now`, in one query. Every target is in
/// the result, with 0 if nothing counts. Counting does not look at the item's
/// state: callers show only visible items' points.
pub fn counted_votes(
    conn: &Connection,
    targets: &[Target],
    now: OffsetDateTime,
) -> rusqlite::Result<HashMap<Target, i64>> {
    let mut counts: HashMap<Target, i64> = targets.iter().map(|t| (*t, 0)).collect();
    if targets.is_empty() {
        return Ok(counts);
    }
    let mut args: Vec<Value> = cutoffs(now).into();
    let mut clauses = Vec::new();
    for kind in ["post", "comment"] {
        let ids: Vec<i64> = targets
            .iter()
            .filter(|t| t.type_str() == kind)
            .map(|t| t.id())
            .collect();
        if ids.is_empty() {
            continue;
        }
        let marks: Vec<String> = ids
            .iter()
            .map(|id| {
                args.push(Value::Integer(*id));
                format!("?{}", args.len())
            })
            .collect();
        clauses.push(format!(
            "(v.target_type = '{kind}' AND v.target_id IN ({}))",
            marks.join(",")
        ));
    }
    let sql = format!(
        "SELECT v.target_type, v.target_id, COUNT(*)
         FROM votes v
         JOIN users u ON u.id = v.user_id
         LEFT JOIN posts p ON v.target_type = 'post' AND p.id = v.target_id
         LEFT JOIN comments c ON v.target_type = 'comment' AND c.id = v.target_id
         WHERE ({}) AND {}
         GROUP BY v.target_type, v.target_id",
        clauses.join(" OR "),
        counted_where()
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(args), |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, i64>(2)?,
        ))
    })?;
    for row in rows {
        let (kind, id, n) = row?;
        if let Some(target) = Target::parse(&kind, id) {
            counts.insert(target, n);
        }
    }
    Ok(counts)
}

/// A member's karma at `now`: the counted votes on their visible posts and
/// comments.
pub fn karma(conn: &Connection, user: UserId, now: OffsetDateTime) -> rusqlite::Result<i64> {
    let [a, b, c] = cutoffs(now);
    let sql = format!(
        "SELECT COUNT(*)
         FROM votes v
         JOIN users u ON u.id = v.user_id
         LEFT JOIN posts p ON v.target_type = 'post' AND p.id = v.target_id
              AND p.author_id = ?4 AND p.state = 'visible'
         LEFT JOIN comments c ON v.target_type = 'comment' AND c.id = v.target_id
              AND c.author_id = ?4 AND c.state = 'visible'
         WHERE (p.id IS NOT NULL OR c.id IS NOT NULL) AND {}",
        counted_where()
    );
    conn.query_row(&sql, [a, b, c, Value::Integer(user.0)], |r| r.get(0))
}

/// Casts `member`'s vote on a visible item at `now`; voting again changes
/// nothing. Refused: restricted or banned members (403), an item that is
/// missing or not visible (404), one's own item (403) and a pack's post for
/// its maintainers (403). The per-member vote limit is `limits::check`'s.
pub fn vote(
    tx: &Transaction,
    member: &Member,
    target: Target,
    now: OffsetDateTime,
) -> Result<(), AppError> {
    if matches!(
        users::standing(member, now),
        Standing::Restricted | Standing::Banned
    ) {
        return Err(AppError::forbidden("A restricted account cannot vote."));
    }
    let (author, state, pack): (Option<i64>, String, Option<String>) = match target {
        Target::Post(id) => tx.query_row(
            "SELECT author_id, state, pack FROM posts WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        ),
        Target::Comment(id) => tx.query_row(
            "SELECT author_id, state, NULL FROM comments WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        ),
    }
    .optional()?
    .ok_or_else(AppError::not_found)?;
    if state != "visible" {
        return Err(AppError::not_found());
    }
    if author == Some(member.id.0) {
        return Err(AppError::forbidden("You cannot vote on your own post."));
    }
    if let Some(pack) = pack {
        let sql = format!("SELECT {}", maintainer_sql("?1", "?2", "?3"));
        let maintainer: bool = tx.query_row(
            &sql,
            rusqlite::params![pack, member.github_id, member.login],
            |r| r.get(0),
        )?;
        if maintainer {
            return Err(AppError::new(
                StatusCode::FORBIDDEN,
                "A pack's maintainers cannot vote on its post.",
            ));
        }
    }
    tx.execute(
        "INSERT OR IGNORE INTO votes (user_id, target_type, target_id, at)
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![
            member.id.0,
            target.type_str(),
            target.id(),
            clock::storage(now)
        ],
    )?;
    Ok(())
}

/// Withdraws a vote, in any state of the item and for a restricted member
/// too. Returns whether there was one; withdrawing a missing vote changes
/// nothing.
pub fn withdraw(tx: &Transaction, user: UserId, target: Target) -> rusqlite::Result<bool> {
    let n = tx.execute(
        "DELETE FROM votes WHERE user_id = ?1 AND target_type = ?2 AND target_id = ?3",
        rusqlite::params![user.0, target.type_str(), target.id()],
    )?;
    Ok(n > 0)
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

    fn counted(conn: &Connection, target: Target, now: OffsetDateTime) -> i64 {
        counted_votes(conn, &[target], now).expect("counted")[&target]
    }

    #[test]
    fn refusals() {
        let (_d, mut conn) = fresh();
        let author = user(&conn, "ada", old(), old());
        let voter = user(&conn, "bob", old(), old());
        let post_id = post(&conn, author, "t", NOW);
        let hidden = post(&conn, author, "h", NOW);
        state(&conn, "posts", hidden, "hidden");
        let removed = post(&conn, author, "r", NOW);
        state(&conn, "posts", removed, "removed");
        let c = comment(&conn, post_id, author, NOW);
        let ada = member(&conn, author);
        let bob = member(&conn, voter);
        let tx = conn.transaction().expect("tx");

        let status = |r: Result<(), AppError>| r.expect_err("refused").status;
        // Own item.
        assert_eq!(
            status(vote(&tx, &ada, Target::Post(post_id), NOW)),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            status(vote(&tx, &ada, Target::Comment(c), NOW)),
            StatusCode::FORBIDDEN
        );
        // Hidden, removed and missing items.
        for id in [hidden, removed, 999] {
            assert_eq!(
                status(vote(&tx, &bob, Target::Post(id), NOW)),
                StatusCode::NOT_FOUND
            );
        }
        assert_eq!(
            status(vote(&tx, &bob, Target::Comment(999), NOW)),
            StatusCode::NOT_FOUND
        );
        // Restricted and banned members.
        let mut restricted = bob.clone();
        restricted.restricted_until = Some(NOW + Duration::hours(1));
        assert_eq!(
            status(vote(&tx, &restricted, Target::Post(post_id), NOW)),
            StatusCode::FORBIDDEN
        );
        let mut banned = bob.clone();
        banned.banned = true;
        assert!(vote(&tx, &banned, Target::Post(post_id), NOW).is_err());
        // The restriction ending lets the member vote.
        vote(
            &tx,
            &restricted,
            Target::Post(post_id),
            NOW + Duration::hours(1),
        )
        .expect("ended");
        // A new member may vote (it counts later); twice is one vote.
        vote(&tx, &bob, Target::Comment(c), NOW).expect("vote");
        vote(&tx, &bob, Target::Comment(c), NOW).expect("again");
        let n: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM votes WHERE target_type = 'comment'",
                [],
                |r| r.get(0),
            )
            .expect("count");
        assert_eq!(n, 1);
    }

    #[test]
    fn maintainers_cannot_vote_on_their_packs_post() {
        let (_d, mut conn) = fresh();
        let owner = user(&conn, "owner-login", old(), old());
        let owner_github: i64 = conn
            .query_row("SELECT github_id FROM users WHERE id = ?1", [owner], |r| {
                r.get(0)
            })
            .expect("gid");
        let bound = user(&conn, "renamed", old(), old());
        let bound_github: i64 = conn
            .query_row("SELECT github_id FROM users WHERE id = ?1", [bound], |r| {
                r.get(0)
            })
            .expect("gid");
        let listed = user(&conn, "Carol", old(), old());
        let other = user(&conn, "dave", old(), old());
        let pid = pack_post(&conn, "glossary", owner_github, r#"["carol"]"#, NOW);
        conn.execute(
            "INSERT INTO pack_maintainer_ids (pack, github_id, login, bound_at)
             VALUES ('glossary', ?1, 'old-name', 'x')",
            [bound_github],
        )
        .expect("bind");
        let tx = conn.transaction().expect("tx");
        for who in [owner, bound, listed] {
            let m = member(&tx, who);
            let e = vote(&tx, &m, Target::Post(pid), NOW).expect_err("maintainer");
            assert_eq!(e.status, StatusCode::FORBIDDEN);
        }
        vote(&tx, &member(&tx, other), Target::Post(pid), NOW).expect("others may");
    }

    #[test]
    fn withdraw_frees_the_vote_and_is_idempotent() {
        let (_d, mut conn) = fresh();
        let author = user(&conn, "ada", old(), old());
        let voter = user(&conn, "bob", old(), old());
        let pid = post(&conn, author, "t", NOW);
        let bob = member(&conn, voter);
        let tx = conn.transaction().expect("tx");
        vote(&tx, &bob, Target::Post(pid), NOW).expect("vote");
        assert_eq!(counted(&tx, Target::Post(pid), NOW), 1);
        // A restricted member, and a removed post, may still withdraw.
        state(&tx, "posts", pid, "removed");
        assert_eq!(withdraw(&tx, UserId(voter), Target::Post(pid)), Ok(true));
        assert_eq!(withdraw(&tx, UserId(voter), Target::Post(pid)), Ok(false));
        assert_eq!(counted(&tx, Target::Post(pid), NOW), 0);
    }

    #[test]
    fn a_new_members_vote_counts_once_the_clock_passes_three_days() {
        let (_d, conn) = fresh();
        let author = user(&conn, "ada", old(), old());
        let voter = user(&conn, "bob", NOW, old());
        let pid = post(&conn, author, "t", NOW);
        vote_row(&conn, voter, pid, NOW);
        let t = Target::Post(pid);
        let edge = NOW + Duration::days(3);
        assert_eq!(counted(&conn, t, NOW), 0);
        assert_eq!(counted(&conn, t, edge - Duration::seconds(1)), 0);
        assert_eq!(counted(&conn, t, edge), 1);
    }

    fn vote_row(conn: &Connection, user: i64, post: i64, at: OffsetDateTime) {
        cast(conn, user, "post", post, at);
    }

    #[test]
    fn standing_edges_of_counted_votes() {
        let (_d, conn) = fresh();
        let author = user(&conn, "ada", old(), old());
        let pid = post(&conn, author, "t", NOW);
        let t = Target::Post(pid);
        let day = Duration::days(1);
        // GitHub account 90 days old exactly, and one second short.
        let gh_edge = user(&conn, "gh-edge", old(), NOW - Duration::days(90));
        let gh_short = user(
            &conn,
            "gh-short",
            old(),
            NOW - Duration::days(90) + Duration::seconds(1),
        );
        let gh_unknown = user(&conn, "gh-unknown", old(), old());
        conn.execute(
            "UPDATE users SET github_created_at = NULL WHERE id = ?1",
            [gh_unknown],
        )
        .expect("null");
        let feed_edge = user(&conn, "feed-edge", NOW - Duration::days(3), old());
        let restricted = user(&conn, "restricted", old(), old());
        conn.execute(
            "UPDATE users SET restricted_until = ?1 WHERE id = ?2",
            rusqlite::params![clock::storage(NOW + day), restricted],
        )
        .expect("restrict");
        let banned = user(&conn, "banned", old(), old());
        conn.execute("UPDATE users SET banned = 1 WHERE id = ?1", [banned])
            .expect("ban");
        let deleted = user(&conn, "deleted", old(), old());
        conn.execute("UPDATE users SET deleted_at = 'x' WHERE id = ?1", [deleted])
            .expect("del");
        for u in [
            gh_edge, gh_short, gh_unknown, feed_edge, restricted, banned, deleted, author,
        ] {
            vote_row(&conn, u, pid, NOW);
        }
        // Counted now: gh_edge and feed_edge. The author never counts.
        assert_eq!(counted(&conn, t, NOW), 2);
        // gh_short crosses 90 days one second later.
        assert_eq!(counted(&conn, t, NOW + Duration::seconds(1)), 3);
        // The restriction ends at its time, and the restricted member counts.
        assert_eq!(counted(&conn, t, NOW + day - Duration::seconds(1)), 3);
        assert_eq!(counted(&conn, t, NOW + day), 4);
    }

    #[test]
    fn counts_exclude_pack_maintainers_and_cover_many_targets_in_one_call() {
        let (_d, conn) = fresh();
        let author = user(&conn, "ada", old(), old());
        let owner = user(&conn, "owner", old(), old());
        let owner_github: i64 = conn
            .query_row("SELECT github_id FROM users WHERE id = ?1", [owner], |r| {
                r.get(0)
            })
            .expect("gid");
        let listed = user(&conn, "listed", old(), old());
        let fan = user(&conn, "fan", old(), old());
        let pack = pack_post(
            &conn,
            "glossary",
            owner_github,
            r#"["LISTED", 5, null]"#,
            NOW,
        );
        let p = post(&conn, author, "t", NOW);
        let c = comment(&conn, p, author, NOW);
        for u in [owner, listed, fan] {
            vote_row(&conn, u, pack, NOW);
        }
        vote_row(&conn, fan, p, NOW);
        cast(&conn, fan, "comment", c, NOW);
        cast(&conn, author, "comment", c, NOW);
        let empty = post(&conn, author, "none", NOW);
        let targets = [
            Target::Post(pack),
            Target::Post(p),
            Target::Comment(c),
            Target::Post(empty),
        ];
        let got = counted_votes(&conn, &targets, NOW).expect("counted");
        assert_eq!(got[&Target::Post(pack)], 1);
        assert_eq!(got[&Target::Post(p)], 1);
        assert_eq!(got[&Target::Comment(c)], 1);
        assert_eq!(got[&Target::Post(empty)], 0);
        assert!(counted_votes(&conn, &[], NOW).expect("empty").is_empty());
    }

    #[test]
    fn a_malformed_maintainer_list_does_not_break_counting() {
        let (_d, conn) = fresh();
        let fan = user(&conn, "fan", old(), old());
        let pack = pack_post(&conn, "glossary", 1, "not json", NOW);
        vote_row(&conn, fan, pack, NOW);
        assert_eq!(counted(&conn, Target::Post(pack), NOW), 1);
    }

    #[test]
    fn karma_counts_only_counted_votes_on_visible_items() {
        let (_d, conn) = fresh();
        let ada = user(&conn, "ada", old(), old());
        let bob = user(&conn, "bob", old(), old());
        let cy = user(&conn, "cy", old(), old());
        let newbie = user(&conn, "newbie", NOW, old());
        let shown = post(&conn, ada, "shown", NOW);
        let hidden = post(&conn, ada, "hidden", NOW);
        state(&conn, "posts", hidden, "hidden");
        let removed_comment = comment(&conn, shown, ada, NOW);
        state(&conn, "comments", removed_comment, "removed");
        let shown_comment = comment(&conn, shown, ada, NOW);
        let others = post(&conn, bob, "bobs", NOW);
        for u in [bob, cy, newbie] {
            vote_row(&conn, u, shown, NOW);
            vote_row(&conn, u, hidden, NOW);
            cast(&conn, u, "comment", removed_comment, NOW);
            cast(&conn, u, "comment", shown_comment, NOW);
        }
        vote_row(&conn, ada, others, NOW);
        vote_row(&conn, ada, shown, NOW);
        // bob and cy count on the shown post and the shown comment: 4.
        assert_eq!(karma(&conn, UserId(ada), NOW), Ok(4));
        // The newcomer counts once established: 2 more.
        assert_eq!(karma(&conn, UserId(ada), NOW + Duration::days(3)), Ok(6));
        assert_eq!(karma(&conn, UserId(bob), NOW), Ok(1));
        assert_eq!(karma(&conn, UserId(newbie), NOW), Ok(0));
    }
}
