//! The vote and measurement ring check, and restricting the accounts it names.
//!
//! The check is an aid for moderators, not a gate: it files system reports
//! and penalises no one. `jobs.rs` calls [`check`] hourly; a moderator's
//! *Restrict the named accounts* calls [`restrict_named`].

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{params, OptionalExtension, Transaction};
use serde_json::{json, Value};
use time::{Duration, OffsetDateTime};

use crate::audit;
use crate::clock;
use crate::db::UserId;
use crate::error::AppError;
use crate::moderation::{self, Actor, RestrictFor, Skip};
use crate::policy;
use crate::reports::{self, ReportTarget, Resolution};
use axum::http::StatusCode;

/// Whose items and contents the accounts acted on.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Owner {
    User(i64),
    Pack(String),
}

struct Act {
    user: i64,
    owner: Owner,
    /// The item or content acted on, as `post:<id>`, `comment:<id>` or
    /// `content:<pack>:<hash>`.
    target: String,
    at: i64,
}

fn unix(text: &str) -> Option<i64> {
    clock::parse_storage(text).map(|t| t.unix_timestamp())
}

/// The votes and measurements of the last 30 days by accounts that are not
/// restricted at `now`.
fn acts(tx: &Transaction, now: OffsetDateTime) -> rusqlite::Result<Vec<Act>> {
    let since = clock::storage(now - Duration::days(policy::RING_DAYS));
    let now_text = clock::storage(now);
    let mut out = Vec::new();

    let mut stmt = tx.prepare(
        "SELECT v.user_id, v.target_type, v.target_id, v.at,
                CASE v.target_type WHEN 'post' THEN p.author_id ELSE c.author_id END, p.pack
         FROM votes v
         JOIN users u ON u.id = v.user_id
         LEFT JOIN posts p ON v.target_type = 'post' AND p.id = v.target_id
         LEFT JOIN comments c ON v.target_type = 'comment' AND c.id = v.target_id
         WHERE v.at >= ?1 AND (u.restricted_until IS NULL OR u.restricted_until <= ?2)",
    )?;
    let rows = stmt.query_map(params![since, now_text], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, Option<i64>>(4)?,
            r.get::<_, Option<String>>(5)?,
        ))
    })?;
    for row in rows {
        let (user, kind, id, at, author, pack) = row?;
        let owner = match (pack, author) {
            (Some(pack), _) => Owner::Pack(pack),
            (None, Some(author)) => Owner::User(author),
            (None, None) => continue,
        };
        let Some(at) = unix(&at) else { continue };
        out.push(Act {
            user,
            owner,
            target: format!("{kind}:{id}"),
            at,
        });
    }

    let mut stmt = tx.prepare(
        "SELECT m.user_id, m.pack, m.pack_hash, m.updated_at
         FROM measurements m JOIN users u ON u.id = m.user_id
         WHERE m.updated_at >= ?1 AND (u.restricted_until IS NULL OR u.restricted_until <= ?2)",
    )?;
    let rows = stmt.query_map(params![since, now_text], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
        ))
    })?;
    for row in rows {
        let (user, pack, hash, at) = row?;
        let Some(at) = unix(&at) else { continue };
        out.push(Act {
            user,
            target: format!("content:{pack}:{hash}"),
            owner: Owner::Pack(pack),
            at,
        });
    }
    Ok(out)
}

fn find(parent: &mut BTreeMap<i64, i64>, x: i64) -> i64 {
    let mut root = x;
    while let Some(&p) = parent.get(&root) {
        if p == root {
            break;
        }
        root = p;
    }
    parent.insert(x, root);
    root
}

/// A linked group of accounts: who, and what they acted on together.
struct Group {
    owner: Owner,
    accounts: BTreeSet<i64>,
    targets: BTreeSet<String>,
}

fn groups(acts: Vec<Act>) -> Vec<Group> {
    let window = policy::RING_WINDOW_MINUTES * 60;
    // owner -> target -> (user, time)
    let mut by_owner: BTreeMap<Owner, BTreeMap<String, Vec<(i64, i64)>>> = BTreeMap::new();
    for act in acts {
        by_owner
            .entry(act.owner)
            .or_default()
            .entry(act.target)
            .or_default()
            .push((act.user, act.at));
    }
    let mut out = Vec::new();
    for (owner, targets) in by_owner {
        // pair -> targets on which both acted within the window
        let mut pairs: BTreeMap<(i64, i64), BTreeSet<String>> = BTreeMap::new();
        for (target, mut who) in targets {
            who.sort_by_key(|&(_, at)| at);
            for i in 0..who.len() {
                for j in i + 1..who.len() {
                    if who[j].1 - who[i].1 > window {
                        break;
                    }
                    if who[i].0 == who[j].0 {
                        continue;
                    }
                    let pair = (who[i].0.min(who[j].0), who[i].0.max(who[j].0));
                    pairs.entry(pair).or_default().insert(target.clone());
                }
            }
        }
        let linked: Vec<_> = pairs
            .iter()
            .filter(|(_, t)| t.len() >= policy::RING_MIN_TOGETHER as usize)
            .collect();
        let mut parent: BTreeMap<i64, i64> = BTreeMap::new();
        for ((a, b), _) in &linked {
            parent.entry(*a).or_insert(*a);
            parent.entry(*b).or_insert(*b);
            let (ra, rb) = (find(&mut parent, *a), find(&mut parent, *b));
            if ra != rb {
                parent.insert(ra, rb);
            }
        }
        let mut by_root: BTreeMap<i64, Group> = BTreeMap::new();
        for ((a, b), shared) in &linked {
            let root = find(&mut parent, *a);
            let group = by_root.entry(root).or_insert_with(|| Group {
                owner: owner.clone(),
                accounts: BTreeSet::new(),
                targets: BTreeSet::new(),
            });
            group.accounts.extend([*a, *b]);
            group.targets.extend(shared.iter().cloned());
        }
        out.extend(by_root.into_values());
    }
    out
}

/// The accounts a ring report names.
fn named_accounts(details: &str) -> Vec<i64> {
    serde_json::from_str::<Value>(details)
        .ok()
        .and_then(|v| v.get("accounts").and_then(Value::as_array).cloned())
        .map(|a| a.iter().filter_map(Value::as_i64).collect())
        .unwrap_or_default()
}

/// The hourly check. Reads the last 30 days of votes and measurements; for
/// each owner, accounts that are not restricted and acted on at least 3 of
/// the owner's items or contents within 600 seconds of each other are linked,
/// and each connected group of linked accounts becomes a system report with
/// reason `ring` on the owner (the member, or the pack's post), whose
/// `details` hold `accounts` (ids) and `acted_on`, plus one non-public
/// `ring.flag` row. No report is filed when a ring report on that owner, of
/// any status, already names every account of the group. Returns the new
/// reports' ids.
pub fn check(tx: &Transaction, now: OffsetDateTime) -> rusqlite::Result<Vec<i64>> {
    let mut filed = Vec::new();
    for group in groups(acts(tx, now)?) {
        let target = match &group.owner {
            Owner::User(id) => ReportTarget::User(*id),
            Owner::Pack(name) => {
                let post: Option<i64> = tx
                    .query_row("SELECT id FROM posts WHERE pack = ?1", [name], |r| r.get(0))
                    .optional()?;
                match post {
                    Some(id) => ReportTarget::Post(id),
                    None => continue,
                }
            }
        };
        let mut stmt = tx.prepare(
            "SELECT details FROM reports
             WHERE system = 1 AND reason = 'ring' AND target_type = ?1 AND target_id = ?2",
        )?;
        let existing: Vec<String> = stmt
            .query_map(params![target.type_str(), target.id()], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        drop(stmt);
        let covered = existing.iter().any(|details| {
            let named: BTreeSet<i64> = named_accounts(details).into_iter().collect();
            group.accounts.is_subset(&named)
        });
        if covered {
            continue;
        }
        let acted_on: Vec<&String> = group.targets.iter().take(100).collect();
        let details = json!({
            "accounts": group.accounts,
            "acted_on": acted_on,
            "acted_on_total": group.targets.len(),
        });
        let id = reports::file_system_report(tx, target, "ring", details.clone(), now)?;
        let mut flag = details;
        flag["report"] = json!(id);
        audit::write(
            tx,
            now,
            None,
            audit::RING_FLAG,
            Some(target.audit()),
            "ring",
            false,
            flag,
        )?;
        filed.push(id);
    }
    Ok(filed)
}

/// What *Restrict the named accounts* did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedRestriction {
    pub restricted: Vec<UserId>,
    /// Accounts left alone, and why: an admin, a moderator (unless the actor
    /// is an admin), a banned account or a deleted one.
    pub skipped: Vec<(UserId, Skip)>,
}

/// Restricts each account a ring report names for `until`, with reason `ring`
/// (one `user.restrict` row each, naming the report), skips and lists the
/// ones the moderator may not act on and those banned or deleted, and
/// resolves the report as upheld. 404 for a report that is not a ring report,
/// 409 for one already resolved.
pub fn restrict_named(
    tx: &Transaction,
    report_id: i64,
    actor: &Actor,
    until: RestrictFor,
    now: OffsetDateTime,
) -> Result<NamedRestriction, AppError> {
    let row: Option<(bool, String, String, String)> = tx
        .query_row(
            "SELECT system, reason, status, details FROM reports WHERE id = ?1",
            [report_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let Some((system, reason, status, details)) = row else {
        return Err(AppError::not_found());
    };
    if !system || reason != "ring" {
        return Err(AppError::not_found());
    }
    if status != "open" {
        return Err(AppError::new(
            StatusCode::CONFLICT,
            "This report is already resolved.",
        ));
    }
    let mut result = NamedRestriction {
        restricted: Vec::new(),
        skipped: Vec::new(),
    };
    for id in named_accounts(&details) {
        let user = UserId(id);
        match moderation::restrict_member(tx, actor, user, until, "ring", Some(report_id), now) {
            Ok(Ok(())) => result.restricted.push(user),
            Ok(Err(skip)) => result.skipped.push((user, skip)),
            Err(e) if e.status == StatusCode::NOT_FOUND => {
                result.skipped.push((user, Skip::Deleted));
            }
            Err(e) => return Err(e),
        }
    }
    reports::resolve_report(tx, report_id, Resolution::Upheld, Some(actor.id), now)?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Role;
    use crate::moderation::Actor;
    use crate::posts::testkit::*;
    use crate::users;
    use crate::votes::{self, Target};
    use rusqlite::Connection;
    use time::macros::datetime;

    const NOW: OffsetDateTime = datetime!(2026-11-08 10:00:00 UTC);

    fn old() -> OffsetDateTime {
        NOW - Duration::days(400)
    }

    struct World {
        _dir: tempfile::TempDir,
        conn: Connection,
        victim: i64,
        ring: Vec<i64>,
        moderator: i64,
        admin: i64,
        admins: Vec<i64>,
    }

    fn world() -> World {
        let (dir, conn) = fresh();
        let victim = user(&conn, "victim", old(), old());
        let ring: Vec<i64> = (0..4)
            .map(|i| user(&conn, &format!("ring{i}"), old(), old()))
            .collect();
        let moderator = user(&conn, "mod", old(), old());
        let admin = user(&conn, "root", old(), old());
        conn.execute(
            "UPDATE users SET role = 'moderator' WHERE id = ?1",
            [moderator],
        )
        .expect("role");
        let admins = vec![member(&conn, admin).github_id.expect("id")];
        World {
            _dir: dir,
            conn,
            victim,
            ring,
            moderator,
            admin,
            admins,
        }
    }

    impl World {
        fn run_check(&self, now: OffsetDateTime) -> Vec<i64> {
            let tx = self.conn.unchecked_transaction().expect("tx");
            let ids = check(&tx, now).expect("check");
            tx.commit().expect("commit");
            ids
        }

        /// `n` posts by the victim.
        fn victim_posts(&self, n: usize) -> Vec<i64> {
            (0..n)
                .map(|i| {
                    post(
                        &self.conn,
                        self.victim,
                        &format!("p{i}"),
                        NOW - Duration::days(2),
                    )
                })
                .collect()
        }

        /// Every account votes on every post, `gap` seconds after the previous account.
        fn vote_all(&self, accounts: &[i64], posts: &[i64], start: OffsetDateTime, gap: i64) {
            for (i, user) in accounts.iter().enumerate() {
                for p in posts {
                    cast(
                        &self.conn,
                        *user,
                        "post",
                        *p,
                        start + Duration::seconds(gap * i as i64),
                    );
                }
            }
        }

        fn ring_reports(&self) -> Vec<(String, i64, String, String)> {
            self.conn
                .prepare(
                    "SELECT target_type, target_id, status, details FROM reports
                     WHERE system = 1 AND reason = 'ring' ORDER BY id",
                )
                .expect("prepare")
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
                .expect("query")
                .collect::<Result<_, _>>()
                .expect("rows")
        }

        fn flags(&self) -> Vec<(Option<i64>, String, String, String, bool)> {
            self.conn
                .prepare(
                    "SELECT actor_id, target_type, target_id, reason, public FROM audit_log
                     WHERE action = 'ring.flag' ORDER BY id",
                )
                .expect("prepare")
                .query_map([], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
                })
                .expect("query")
                .collect::<Result<_, _>>()
                .expect("rows")
        }
    }

    fn accounts_of(details: &str) -> Vec<i64> {
        named_accounts(details)
    }

    #[test]
    fn a_ring_on_a_member_is_flagged_once() {
        let w = world();
        let posts = w.victim_posts(3);
        w.vote_all(&w.ring, &posts, NOW - Duration::hours(5), 60);
        let filed = w.run_check(NOW);
        assert_eq!(filed.len(), 1);
        let reports = w.ring_reports();
        assert_eq!(reports.len(), 1);
        assert_eq!(
            (reports[0].0.as_str(), reports[0].1, reports[0].2.as_str()),
            ("user", w.victim, "open")
        );
        assert_eq!(accounts_of(&reports[0].3), w.ring);
        let details: Value = serde_json::from_str(&reports[0].3).expect("json");
        assert_eq!(details["acted_on"].as_array().map(Vec::len), Some(3));
        assert!(details["acted_on"][0]
            .as_str()
            .expect("s")
            .starts_with("post:"));
        assert_eq!(
            w.flags(),
            vec![(
                None,
                "user".into(),
                w.victim.to_string(),
                "ring".into(),
                false
            )]
        );
        // The second run adds nothing.
        assert!(w.run_check(NOW + Duration::minutes(5)).is_empty());
        assert_eq!(w.ring_reports().len(), 1);
        assert_eq!(w.flags().len(), 1);
    }

    fn measure(w: &World, user: i64, pack: &str, hash: &str, at: OffsetDateTime) {
        w.conn
            .execute(
                "INSERT INTO measurements (pack, pack_hash, user_id, eval_set, planted, kept, runs,
                    corrected_without, damaged_without, corrected_with, damaged_with, hippocamp, model,
                    created_at, updated_at)
                 VALUES (?1, ?2, ?3, 's', 5, 5, 3, 1, 2, 3, 4, '0.1.0', 'm', ?4, ?4)",
                params![pack, hash, user, clock::storage(at)],
            )
            .expect("measurement");
    }

    #[test]
    fn a_ring_on_a_pack_targets_the_packs_post() {
        let w = world();
        let pack_post = pack_post(&w.conn, "glossary", 5, "[]", NOW - Duration::days(9));
        for hash in ["h1", "h2", "h3"] {
            w.conn
                .execute(
                    "INSERT INTO pack_contents (pack, pack_hash, version, commit_id, first_seen)
                     VALUES ('glossary', ?1, '1', 'c', ?2)",
                    params![hash, clock::storage(NOW - Duration::days(9))],
                )
                .expect("content");
            for (i, user) in w.ring.iter().enumerate() {
                measure(
                    &w,
                    *user,
                    "glossary",
                    hash,
                    NOW - Duration::hours(3) + Duration::seconds(30 * i as i64),
                );
            }
        }
        // Votes on the pack's post count as one more shared target of the same owner.
        w.vote_all(&w.ring, &[pack_post], NOW - Duration::hours(3), 10);
        assert_eq!(w.run_check(NOW).len(), 1);
        let reports = w.ring_reports();
        assert_eq!(reports.len(), 1);
        assert_eq!((reports[0].0.as_str(), reports[0].1), ("post", pack_post));
        assert_eq!(accounts_of(&reports[0].3), w.ring);
        let details: Value = serde_json::from_str(&reports[0].3).expect("json");
        assert_eq!(details["acted_on_total"], 4);
        assert_eq!(w.flags()[0].1, "post");
        assert!(w.run_check(NOW).is_empty(), "flagged once");
    }

    #[test]
    fn a_pack_ring_can_rest_on_measurements_alone() {
        let w = world();
        pack_post(&w.conn, "glossary", 5, "[]", NOW - Duration::days(9));
        for hash in ["h1", "h2", "h3"] {
            w.conn
                .execute(
                    "INSERT INTO pack_contents (pack, pack_hash, version, commit_id, first_seen)
                     VALUES ('glossary', ?1, '1', 'c', ?2)",
                    params![hash, clock::storage(NOW - Duration::days(9))],
                )
                .expect("content");
            measure(&w, w.ring[0], "glossary", hash, NOW - Duration::hours(3));
            measure(
                &w,
                w.ring[1],
                "glossary",
                hash,
                NOW - Duration::hours(3) + Duration::seconds(600),
            );
        }
        assert_eq!(w.run_check(NOW).len(), 1);
        assert_eq!(
            accounts_of(&w.ring_reports()[0].3),
            vec![w.ring[0], w.ring[1]]
        );
    }

    #[test]
    fn a_group_that_a_dismissed_report_already_names_is_not_reported_again() {
        let w = world();
        let posts = w.victim_posts(3);
        w.vote_all(&w.ring, &posts, NOW - Duration::hours(5), 30);
        assert_eq!(w.run_check(NOW).len(), 1);
        w.conn
            .execute(
                "UPDATE reports SET status = 'dismissed', resolved_at = ?1",
                [clock::storage(NOW)],
            )
            .expect("dismiss");
        assert!(w.run_check(NOW + Duration::hours(1)).is_empty());
        assert_eq!(w.ring_reports().len(), 1);
        // A group with one more account is a new group: it is reported.
        let fifth = user(&w.conn, "ring4", old(), old());
        w.vote_all(&[fifth], &posts, NOW - Duration::hours(5), 0);
        assert_eq!(w.run_check(NOW + Duration::hours(2)).len(), 1);
        assert_eq!(w.ring_reports().len(), 2);
        assert_eq!(accounts_of(&w.ring_reports()[1].3).len(), 5);
    }

    #[test]
    fn accounts_acting_hours_apart_are_not_flagged() {
        let w = world();
        let posts = w.victim_posts(3);
        // Each account votes three hours after the previous one.
        w.vote_all(&w.ring, &posts, NOW - Duration::days(1), 3 * 3600);
        assert!(w.run_check(NOW).is_empty());
        assert!(w.ring_reports().is_empty());
        assert!(w.flags().is_empty());
    }

    #[test]
    fn the_window_is_600_seconds_inclusive_and_three_shared_targets_link() {
        let w = world();
        let posts = w.victim_posts(3);
        let start = NOW - Duration::hours(2);
        // 600 s apart: linked.
        w.vote_all(&w.ring[..2], &posts, start, 600);
        assert_eq!(w.run_check(NOW).len(), 1);
        // 601 s apart: not linked. A fresh world for the other owner.
        let w2 = world();
        let posts2 = w2.victim_posts(3);
        w2.vote_all(&w2.ring[..2], &posts2, start, 601);
        assert!(w2.run_check(NOW).is_empty());
        // Only two shared targets: not linked.
        let w3 = world();
        let posts3 = w3.victim_posts(3);
        w3.vote_all(&w3.ring[..2], &posts3[..2], start, 10);
        cast(&w3.conn, w3.ring[0], "post", posts3[2], start);
        assert!(w3.run_check(NOW).is_empty());
    }

    #[test]
    fn shared_targets_are_counted_per_owner() {
        let w = world();
        // Two targets of each of two owners: 4 shared overall, never 3 for one owner.
        let other = user(&w.conn, "other", old(), old());
        let a = w.victim_posts(2);
        let b: Vec<i64> = (0..2)
            .map(|i| post(&w.conn, other, &format!("o{i}"), NOW - Duration::days(2)))
            .collect();
        let start = NOW - Duration::hours(2);
        w.vote_all(&w.ring[..2], &a, start, 5);
        w.vote_all(&w.ring[..2], &b, start, 5);
        assert!(w.run_check(NOW).is_empty());
    }

    #[test]
    fn old_votes_are_not_read_and_restricted_accounts_are_left_out() {
        let w = world();
        let posts = w.victim_posts(3);
        // 31 days ago: outside the 30 days.
        w.vote_all(&w.ring, &posts, NOW - Duration::days(31), 10);
        assert!(w.run_check(NOW).is_empty());
        // Now fresh votes, but two accounts are restricted: the other two link alone.
        let w = world();
        let posts = w.victim_posts(3);
        w.vote_all(&w.ring, &posts, NOW - Duration::hours(1), 10);
        for id in &w.ring[..2] {
            w.conn
                .execute(
                    "UPDATE users SET restricted_until = ?1 WHERE id = ?2",
                    params![clock::storage(NOW + Duration::days(1)), id],
                )
                .expect("restrict");
        }
        assert_eq!(w.run_check(NOW).len(), 1);
        assert_eq!(
            accounts_of(&w.ring_reports()[0].3),
            vec![w.ring[2], w.ring[3]]
        );
    }

    #[test]
    fn votes_on_deleted_authors_items_and_one_account_voting_alone_are_ignored() {
        let w = world();
        let posts = w.victim_posts(3);
        w.conn
            .execute("UPDATE posts SET author_id = NULL, state = 'deleted'", [])
            .expect("deleted authors");
        w.vote_all(&w.ring, &posts, NOW - Duration::hours(1), 10);
        assert!(w.run_check(NOW).is_empty());
        let w = world();
        let posts = w.victim_posts(3);
        w.vote_all(&w.ring[..1], &posts, NOW - Duration::hours(1), 10);
        assert!(w.run_check(NOW).is_empty());
    }

    #[test]
    fn restricting_the_named_accounts_resolves_the_report_and_removes_their_votes_from_counts() {
        let w = world();
        let posts = w.victim_posts(3);
        w.vote_all(&w.ring, &posts, NOW - Duration::hours(1), 10);
        let report = w.run_check(NOW)[0];
        let counts = |at: OffsetDateTime| {
            votes::counted_votes(&w.conn, &[Target::Post(posts[0])], at).expect("counts")
                [&Target::Post(posts[0])]
        };
        assert_eq!(counts(NOW), 4);
        let actor = Actor::new(&member(&w.conn, w.moderator), &w.admins).expect("actor");
        let tx = w.conn.unchecked_transaction().expect("tx");
        let out = restrict_named(&tx, report, &actor, RestrictFor::Week, NOW).expect("restrict");
        tx.commit().expect("commit");
        assert_eq!(
            out.restricted,
            w.ring.iter().map(|id| UserId(*id)).collect::<Vec<_>>()
        );
        assert!(out.skipped.is_empty());
        assert_eq!(counts(NOW), 0);
        assert_eq!(
            counts(NOW + Duration::days(7)),
            4,
            "back when the restriction ends"
        );
        let (status, by): (String, Option<i64>) = w
            .conn
            .query_row(
                "SELECT status, resolved_by FROM reports WHERE id = ?1",
                [report],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("report");
        assert_eq!((status.as_str(), by), ("upheld", Some(w.moderator)));
        // One user.restrict row each, public, naming the report.
        let rows: Vec<(String, bool, String, String)> = w
            .conn
            .prepare(
                "SELECT action, public, reason, details FROM audit_log
                 WHERE action = 'user.restrict' ORDER BY id",
            )
            .expect("prepare")
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("rows");
        assert_eq!(rows.len(), 4);
        assert!(rows.iter().all(|r| r.1 && r.2 == "ring"));
        assert!(rows[0].3.contains(&format!("\"ring_report\":{report}")));
        // The restricted accounts no longer make a ring, and a resolved report cannot be reused.
        let tx = w.conn.unchecked_transaction().expect("tx");
        let e = restrict_named(&tx, report, &actor, RestrictFor::Day, NOW).expect_err("resolved");
        assert_eq!(e.status, StatusCode::CONFLICT);
    }

    #[test]
    fn restrict_named_skips_what_the_moderator_may_not_touch() {
        let w = world();
        let posts = w.victim_posts(3);
        // The named group holds a moderator, an admin, a banned and a deleted account and one member.
        let group = [w.moderator, w.admin, w.ring[0], w.ring[1], w.ring[2]];
        w.vote_all(&group, &posts, NOW - Duration::hours(1), 10);
        w.conn
            .execute("UPDATE users SET banned = 1 WHERE id = ?1", [w.ring[0]])
            .expect("ban");
        let report = w.run_check(NOW)[0];
        // The deleted account's votes would be gone; name it by hand in the report.
        w.conn
            .execute(
                "UPDATE users SET deleted_at = 'x' WHERE id = ?1",
                [w.ring[1]],
            )
            .expect("delete");
        let actor = Actor::new(&member(&w.conn, w.moderator), &w.admins).expect("actor");
        let tx = w.conn.unchecked_transaction().expect("tx");
        let out = restrict_named(&tx, report, &actor, RestrictFor::Day, NOW).expect("restrict");
        tx.commit().expect("commit");
        assert_eq!(out.restricted, vec![UserId(w.ring[2])]);
        let skipped: BTreeMap<i64, Skip> = out.skipped.iter().map(|(u, s)| (u.0, *s)).collect();
        assert_eq!(skipped[&w.moderator], Skip::NotPermitted);
        assert_eq!(skipped[&w.admin], Skip::NotPermitted);
        assert_eq!(skipped[&w.ring[0]], Skip::Banned);
        assert_eq!(skipped[&w.ring[1]], Skip::Deleted);
        assert_eq!(skipped.len(), 4);
        let status: String = w
            .conn
            .query_row("SELECT status FROM reports WHERE id = ?1", [report], |r| {
                r.get(0)
            })
            .expect("status");
        assert_eq!(status, "upheld");
        assert!(member(&w.conn, w.moderator).restricted_until.is_none());
        assert!(member(&w.conn, w.admin).restricted_until.is_none());
        assert_eq!(
            users::standing(&member(&w.conn, w.ring[2]), NOW),
            users::Standing::Restricted
        );
        // An admin may restrict the moderator too.
        let w2 = world();
        let posts = w2.victim_posts(3);
        w2.vote_all(
            &[w2.moderator, w2.ring[0]],
            &posts,
            NOW - Duration::hours(1),
            10,
        );
        let report = w2.run_check(NOW)[0];
        let admin = Actor::new(&member(&w2.conn, w2.admin), &w2.admins).expect("admin");
        let tx = w2.conn.unchecked_transaction().expect("tx");
        let out =
            restrict_named(&tx, report, &admin, RestrictFor::Indefinite, NOW).expect("restrict");
        assert_eq!(out.restricted.len(), 2);
        assert_eq!(member(&w2.conn, w2.moderator).role, Role::Moderator);
    }

    #[test]
    fn only_open_ring_reports_can_be_restricted_through() {
        let w = world();
        let actor = Actor::new(&member(&w.conn, w.moderator), &w.admins).expect("actor");
        let tx = w.conn.unchecked_transaction().expect("tx");
        assert_eq!(
            restrict_named(&tx, 99, &actor, RestrictFor::Day, NOW)
                .expect_err("missing")
                .status,
            StatusCode::NOT_FOUND
        );
        let ordinary = reports::file_system_report(
            &tx,
            ReportTarget::User(w.victim),
            "misleading",
            json!({"accounts": [1]}),
            NOW,
        )
        .expect("report");
        assert_eq!(
            restrict_named(&tx, ordinary, &actor, RestrictFor::Day, NOW)
                .expect_err("not a ring")
                .status,
            StatusCode::NOT_FOUND
        );
    }
}
