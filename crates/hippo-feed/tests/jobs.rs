//! The hourly clean-up: each step happens at its time and not before.
//! Rows are set up with SQL, which is how the tests stand in for the code that
//! creates them.

mod common;

use common::{Options, TestFeed};
use time::{Duration, OffsetDateTime};

fn stamp(t: OffsetDateTime) -> String {
    hippo_feed::clock::storage(t)
}

fn count(feed: &TestFeed, sql: &str) -> i64 {
    feed.conn().query_row(sql, [], |r| r.get(0)).expect("count")
}

#[test]
fn sessions_are_deleted_once_they_expire() {
    let feed = TestFeed::start(Options::default());
    let t0 = feed.clock_now();
    let conn = feed.conn();
    conn.execute(
        "INSERT INTO users (id, login, created_at) VALUES (1, 'a', ?1)",
        [stamp(t0)],
    )
    .expect("user");
    for (hash, expires) in [
        ("soon", t0 + Duration::hours(1)),
        ("late", t0 + Duration::days(2)),
    ] {
        conn.execute(
            "INSERT INTO sessions (token_hash, user_id, csrf, created_at, expires_at)
             VALUES (?1, 1, 'c', ?2, ?3)",
            rusqlite::params![hash, stamp(t0), stamp(expires)],
        )
        .expect("session");
    }
    let r = feed.hourly();
    assert_eq!(r.sessions, 0);
    feed.clock.set(t0 + Duration::minutes(59));
    assert_eq!(feed.hourly().sessions, 0);
    feed.clock.set(t0 + Duration::hours(1));
    assert_eq!(feed.hourly().sessions, 1);
    assert_eq!(count(&feed, "SELECT COUNT(*) FROM sessions"), 1);
}

#[test]
fn oauth_states_go_after_ten_minutes() {
    let feed = TestFeed::start(Options::default());
    let t0 = feed.clock_now();
    feed.conn()
        .execute(
            "INSERT INTO oauth_states (state_hash, verifier, return_to, created_at)
             VALUES ('s', 'v', '/', ?1)",
            [stamp(t0)],
        )
        .expect("state");
    feed.clock.set(t0 + Duration::minutes(10));
    assert_eq!(feed.hourly().oauth_states, 0);
    feed.clock
        .set(t0 + Duration::minutes(10) + Duration::seconds(1));
    assert_eq!(feed.hourly().oauth_states, 1);
    assert_eq!(count(&feed, "SELECT COUNT(*) FROM oauth_states"), 0);
}

#[test]
fn a_deleted_accounts_github_id_goes_unless_banned_or_restricted() {
    let feed = TestFeed::start(Options::default());
    let t0 = feed.clock_now();
    let until = t0 + Duration::days(2);
    let conn = feed.conn();
    let rows: [(i64, i64, Option<String>, bool); 5] = [
        (1, 101, None, false),                                // deleted, free: goes
        (2, 102, None, true),                                 // deleted, banned: stays
        (3, 103, Some(stamp(until)), false), // deleted, restricted: stays until then
        (4, 104, None, false),               // not deleted: stays
        (5, 105, Some(stamp(t0 - Duration::days(1))), false), // restriction over: goes
    ];
    for (id, gid, restricted, banned) in rows {
        let deleted = if id == 4 { None } else { Some(stamp(t0)) };
        conn.execute(
            "INSERT INTO users (id, github_id, login, created_at, banned, restricted_until, deleted_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![id, gid, format!("u{id}"), stamp(t0), banned, restricted, deleted],
        )
        .expect("user");
    }
    assert_eq!(feed.hourly().github_ids, 2);
    let kept = |sql: &str| count(&feed, sql);
    assert_eq!(
        kept("SELECT COUNT(*) FROM users WHERE github_id IS NULL"),
        2
    );
    assert_eq!(
        kept("SELECT COUNT(*) FROM users WHERE id IN (2, 3, 4) AND github_id IS NOT NULL"),
        3
    );
    feed.clock.set(until);
    assert_eq!(feed.hourly().github_ids, 1);
    assert_eq!(kept("SELECT github_id FROM users WHERE id = 2"), 102);
}

#[test]
fn resolved_reports_lose_their_text_after_ninety_days() {
    let feed = TestFeed::start(Options::default());
    let t0 = feed.clock_now();
    let conn = feed.conn();
    for (id, status, resolved) in [
        (1, "dismissed", Some(stamp(t0))),
        (2, "open", None),
        (3, "upheld", Some(stamp(t0 + Duration::days(30)))),
    ] {
        conn.execute(
            "INSERT INTO reports (id, system, target_type, target_id, reason, note, snapshot,
                 created_at, status, resolved_at)
             VALUES (?1, 1, 'post', ?1, 'spam', 'a note', 'a snapshot', ?2, ?3, ?4)",
            rusqlite::params![id, stamp(t0), status, resolved],
        )
        .expect("report");
    }
    feed.clock.set(t0 + Duration::days(89));
    assert_eq!(feed.hourly().report_texts, 0);
    feed.clock.set(t0 + Duration::days(91));
    assert_eq!(feed.hourly().report_texts, 1);
    let blank = |id: i64| -> (String, String) {
        feed.conn()
            .query_row(
                "SELECT note, snapshot FROM reports WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("row")
    };
    assert_eq!(blank(1), (String::new(), String::new()));
    assert_eq!(blank(2).0, "a note");
    assert_eq!(blank(3).0, "a note");
    feed.clock.set(t0 + Duration::days(121));
    assert_eq!(feed.hourly().report_texts, 1);
    assert_eq!(blank(3), (String::new(), String::new()));
    assert_eq!(blank(2).1, "a snapshot");
}

#[test]
fn the_hourly_ring_check_reports_a_ring_once() {
    let feed = TestFeed::start(Options::default());
    let t0 = feed.clock_now();
    let conn = feed.conn();
    let old = stamp(t0 - Duration::days(60));
    // An owner, four voters and three posts the voters all voted on within
    // ten minutes of each other (rows set up with SQL).
    for id in 1..=5 {
        conn.execute(
            "INSERT INTO users (id, github_id, login, github_created_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?4)",
            rusqlite::params![id, 100 + id, format!("u{id}"), old],
        )
        .expect("user");
    }
    for post in 1..=3 {
        conn.execute(
            "INSERT INTO posts (id, kind, author_id, title, body, created_at)
             VALUES (?1, 'discussion', 1, 'p', 'b', ?2)",
            rusqlite::params![post, old],
        )
        .expect("post");
        for voter in 2..=5 {
            conn.execute(
                "INSERT INTO votes (user_id, target_type, target_id, at)
                 VALUES (?1, 'post', ?2, ?3)",
                rusqlite::params![voter, post, stamp(t0 - Duration::hours(2))],
            )
            .expect("vote");
        }
    }
    let first = feed
        .block_on(hippo_feed::jobs::ring_check(feed.state(), t0))
        .expect("check");
    assert_eq!(first.len(), 1);
    let again = feed
        .block_on(hippo_feed::jobs::ring_check(feed.state(), t0))
        .expect("check");
    assert!(again.is_empty());
    assert_eq!(
        count(
            &feed,
            "SELECT COUNT(*) FROM reports WHERE system = 1 AND reason = 'ring'"
        ),
        1
    );
}
