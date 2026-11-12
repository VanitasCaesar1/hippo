//! `hippocamp-feed activity`: one row per ISO week of what the community did.

use std::fmt::Write;
use std::path::Path;

use anyhow::Context;
use rusqlite::Connection;
use time::{Date, Duration, OffsetDateTime, Time};

use crate::clock;
use crate::db;

/// The most weeks the table lists.
pub const WEEKS_MAX: u32 = 104;
/// The weeks it lists by default.
pub const WEEKS_DEFAULT: u32 = 12;

fn count(conn: &Connection, sql: &str, from: &str, to: &str) -> rusqlite::Result<i64> {
    conn.query_row(sql, [from, to], |r| r.get(0))
}

/// The Markdown table `week | active | new | posts | comments | votes |
/// measurements | reports`, newest week first, the current ISO week marked
/// `(partial)`. `weeks` is held to 1 through 104. Active members are the
/// distinct members who posted, commented, voted or measured that week,
/// counted from the rows that still exist.
pub fn table(conn: &Connection, now: OffsetDateTime, weeks: u32) -> rusqlite::Result<String> {
    let weeks = weeks.clamp(1, WEEKS_MAX);
    let today = now.to_offset(time::UtcOffset::UTC).date();
    let monday = today - Duration::days(i64::from(today.weekday().number_days_from_monday()));
    let mut out = String::from(
        "| week | active | new | posts | comments | votes | measurements | reports |\n\
         |---|---|---|---|---|---|---|---|\n",
    );
    for k in 0..i64::from(weeks) {
        let start: Date = monday - Duration::days(7 * k);
        let at = |d: Date| clock::storage(d.with_time(Time::MIDNIGHT).assume_utc());
        let (from, to) = (at(start), at(start + Duration::days(7)));
        let (year, week, _) = start.to_iso_week_date();
        let label = format!(
            "{year}-W{week:02}{}",
            if k == 0 { " (partial)" } else { "" }
        );
        let active = count(
            conn,
            "SELECT COUNT(*) FROM (
               SELECT author_id FROM posts
                 WHERE kind = 'discussion' AND author_id IS NOT NULL
                   AND created_at >= ?1 AND created_at < ?2
               UNION SELECT author_id FROM comments
                 WHERE author_id IS NOT NULL AND created_at >= ?1 AND created_at < ?2
               UNION SELECT user_id FROM votes WHERE at >= ?1 AND at < ?2
               UNION SELECT user_id FROM measurements WHERE updated_at >= ?1 AND updated_at < ?2)",
            &from,
            &to,
        )?;
        let new = count(
            conn,
            "SELECT COUNT(*) FROM users WHERE created_at >= ?1 AND created_at < ?2",
            &from,
            &to,
        )?;
        let posts = count(
            conn,
            "SELECT COUNT(*) FROM posts
             WHERE kind = 'discussion' AND created_at >= ?1 AND created_at < ?2",
            &from,
            &to,
        )?;
        let comments = count(
            conn,
            "SELECT COUNT(*) FROM comments WHERE created_at >= ?1 AND created_at < ?2",
            &from,
            &to,
        )?;
        let votes = count(
            conn,
            "SELECT COUNT(*) FROM votes WHERE at >= ?1 AND at < ?2",
            &from,
            &to,
        )?;
        let measurements = count(
            conn,
            "SELECT COUNT(*) FROM measurements WHERE created_at >= ?1 AND created_at < ?2",
            &from,
            &to,
        )?;
        let reports = count(
            conn,
            "SELECT COUNT(*) FROM reports WHERE created_at >= ?1 AND created_at < ?2",
            &from,
            &to,
        )?;
        let _ = writeln!(
            out,
            "| {label} | {active} | {new} | {posts} | {comments} | {votes} | {measurements} | {reports} |"
        );
    }
    Ok(out)
}

/// [`table`] for the database file at `path`, which must be migrated.
pub fn table_at(path: &Path, now: OffsetDateTime, weeks: u32) -> anyhow::Result<String> {
    let conn =
        db::open(path).with_context(|| format!("cannot open the database {}", path.display()))?;
    db::check_version(&conn)?;
    Ok(table(&conn, now, weeks)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::posts::testkit::*;
    use rusqlite::params;
    use time::macros::datetime;

    // A Sunday, in ISO week 2026-W45 (Monday 2 Nov to Sunday 8 Nov).
    const NOW: OffsetDateTime = datetime!(2026-11-08 10:00:00 UTC);

    fn old() -> OffsetDateTime {
        datetime!(2020-01-01 0:00 UTC)
    }

    fn row(table: &str, label: &str) -> String {
        table
            .lines()
            .find(|l| l.starts_with(&format!("| {label}")))
            .unwrap_or_else(|| panic!("no row {label} in\n{table}"))
            .to_string()
    }

    #[test]
    fn an_empty_database_lists_zero_weeks_newest_first() {
        let (_d, conn) = fresh();
        let t = table(&conn, NOW, 12).expect("table");
        let lines: Vec<&str> = t.lines().collect();
        assert_eq!(lines.len(), 14);
        assert_eq!(lines[1], "|---|---|---|---|---|---|---|---|");
        assert_eq!(
            lines[2],
            "| 2026-W45 (partial) | 0 | 0 | 0 | 0 | 0 | 0 | 0 |"
        );
        assert_eq!(lines[3], "| 2026-W44 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |");
        assert_eq!(lines[13], "| 2026-W34 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |");
        assert!(t.ends_with('\n'));
    }

    #[test]
    fn weeks_are_held_to_1_through_104_and_cross_the_year_by_iso_numbering() {
        let (_d, conn) = fresh();
        assert_eq!(table(&conn, NOW, 0).expect("t").lines().count(), 3);
        assert_eq!(table(&conn, NOW, 500).expect("t").lines().count(), 2 + 104);
        // 2027-01-01 is a Friday in ISO week 2026-W53.
        let t = table(&conn, datetime!(2027-01-01 12:00 UTC), 3).expect("t");
        let labels: Vec<&str> = t
            .lines()
            .skip(2)
            .map(|l| l.split(" |").next().unwrap_or(""))
            .collect();
        assert_eq!(
            labels,
            vec!["| 2026-W53 (partial)", "| 2026-W52", "| 2026-W51"]
        );
        // Monday 2026-12-28 starts it; a Monday "now" is the start of its own week.
        let t = table(&conn, datetime!(2026-11-02 0:00 UTC), 1).expect("t");
        assert!(t.contains("| 2026-W45 (partial) |"));
    }

    #[test]
    fn a_planted_week_is_counted_by_its_rows() {
        let (_d, conn) = fresh();
        let at = |s: &str| clock::parse_storage(s).expect("time");
        // Three members; "new1" and "new2" were created in W45, ada earlier.
        let ada = user(&conn, "ada", datetime!(2026-10-01 0:00 UTC), old());
        let new1 = user(&conn, "new1", datetime!(2026-11-02 0:00:00 UTC), old());
        let new2 = user(&conn, "new2", datetime!(2026-11-08 23:59:59 UTC), old());
        let _quiet = user(&conn, "quiet", datetime!(2026-10-05 0:00 UTC), old());
        // W45: ada posts twice and comments; new1 votes; new2 measures.
        let p1 = post(&conn, ada, "a", datetime!(2026-11-03 9:00 UTC));
        post(&conn, ada, "b", datetime!(2026-11-04 9:00 UTC));
        comment(&conn, p1, ada, datetime!(2026-11-05 9:00 UTC));
        cast(&conn, new1, "post", p1, datetime!(2026-11-06 9:00 UTC));
        cast(&conn, new2, "post", p1, datetime!(2026-11-02 0:00:00 UTC));
        conn.execute(
            "INSERT INTO packs (name, version, description, authors, owner, owner_id, maintainers,
                first_party, license, repo, repo_id, path, commit_id, pack_hash, passes, review_id,
                review_at, listed, synced_at)
             VALUES ('g', '1', 'd', '[]', 'o', 1, '[]', 0, 'MIT', 'r', 1, '.', 'c', 'h', '[]', 'r', 'x', 1, 'x')",
            [],
        )
        .expect("pack");
        conn.execute(
            "INSERT INTO pack_contents (pack, pack_hash, version, commit_id, first_seen)
             VALUES ('g', 'h', '1', 'c', 'x')",
            [],
        )
        .expect("content");
        conn.execute(
            "INSERT INTO measurements (pack, pack_hash, user_id, eval_set, planted, kept, runs,
                corrected_without, damaged_without, corrected_with, damaged_with, hippocamp, model,
                created_at, updated_at)
             VALUES ('g', 'h', ?1, 's', 5, 5, 3, 1, 1, 1, 1, 'h', 'm', ?2, ?2)",
            params![new2, clock::storage(at("2026-11-07T00:00:00.000000Z"))],
        )
        .expect("measurement");
        // Reports: one by a member, one by the system.
        conn.execute(
            "INSERT INTO reports (reporter_id, system, target_type, target_id, reason, created_at)
             VALUES (?1, 0, 'post', ?2, 'spam', '2026-11-04T00:00:00.000000Z'),
                    (NULL, 1, 'user', ?1, 'ring', '2026-11-05T00:00:00.000000Z')",
            params![new1, p1],
        )
        .expect("reports");
        // W44: ada comments again, bob votes at the very end of the week.
        let bob = user(&conn, "bob", datetime!(2026-10-26 0:00:00 UTC), old());
        comment(&conn, p1, ada, datetime!(2026-11-01 23:59:59 UTC));
        cast(&conn, bob, "post", p1, datetime!(2026-11-01 23:59:59 UTC));
        // A post by a member who has since deleted their account has no author: not "active".
        let ghost = post(&conn, ada, "ghost", datetime!(2026-11-03 10:00 UTC));
        conn.execute("UPDATE posts SET author_id = NULL WHERE id = ?1", [ghost])
            .expect("ghost");

        let t = table(&conn, NOW, 3).expect("table");
        // active: ada, new1, new2 = 3; new: new1, new2; posts: a, b, ghost; comments 1;
        // votes 2; measurements 1; reports 2.
        assert_eq!(
            row(&t, "2026-W45"),
            "| 2026-W45 (partial) | 3 | 2 | 3 | 1 | 2 | 1 | 2 |"
        );
        // W44: active ada and bob; new bob (created 26 Oct is W44); posts 0 (p1 is W45).
        assert_eq!(
            row(&t, "2026-W44"),
            "| 2026-W44 | 2 | 1 | 0 | 1 | 1 | 0 | 0 |"
        );
        // W43 (19 to 25 Oct): nothing, though "quiet" was created 5 Oct and ada 1 Oct.
        assert_eq!(
            row(&t, "2026-W43"),
            "| 2026-W43 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |"
        );
        let w40 = table(&conn, NOW, 6).expect("table");
        assert_eq!(
            row(&w40, "2026-W41"),
            "| 2026-W41 | 0 | 1 | 0 | 0 | 0 | 0 | 0 |"
        );
    }
}
