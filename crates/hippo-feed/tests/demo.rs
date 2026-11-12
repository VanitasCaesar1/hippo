//! `seed-demo`: the fixture's registry, synced as the other tests do it, then
//! the demo data on top; the front page shows it, and the rows are the ones
//! the demo is meant to create.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use common::{Options, RawServer, TestFeed};
use hippo_feed::db::UserId;
use hippo_feed::{demo, posts, users, votes};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/feed")
}

struct Setup {
    feed: TestFeed,
    _raw: RawServer,
    _dir: tempfile::TempDir,
}

fn setup() -> Setup {
    let fixture = std::fs::read_to_string(fixtures().join("registry-index.json")).expect("read");
    setup_with(&fixture)
}

/// The fixture's index without the entries named in `drop`.
fn index_without(drop: &[&str]) -> String {
    let fixture = std::fs::read_to_string(fixtures().join("registry-index.json")).expect("read");
    let mut index: serde_json::Value = serde_json::from_str(&fixture).expect("json");
    index["packs"]
        .as_array_mut()
        .expect("packs")
        .retain(|p| !drop.contains(&p["name"].as_str().unwrap_or("")));
    index.to_string()
}

fn setup_with(index_json: &str) -> Setup {
    let raw = RawServer::start_default();
    let dir = tempfile::tempdir().expect("tempdir");
    let index = dir.path().join("index.json");
    std::fs::write(&index, index_json).expect("write");
    let feed = TestFeed::start(Options {
        registry_index: Some(index.display().to_string()),
        raw_url: Some(raw.url.clone()),
        eval_sets: Some(format!(
            "{}:{}:{}",
            demo::EVAL_SET,
            demo::PLANTED,
            demo::KEPT
        )),
        ..Options::default()
    });
    Setup {
        feed,
        _raw: raw,
        _dir: dir,
    }
}

fn count(feed: &TestFeed, sql: &str) -> i64 {
    feed.conn().query_row(sql, [], |r| r.get(0)).expect("count")
}

#[test]
fn seeding_refuses_without_packs_and_a_second_time() {
    let s = setup();
    let err = demo::seed(&s.feed.db_path, s.feed.clock_now()).expect_err("no packs");
    assert!(err.to_string().contains("run sync-registry first"), "{err}");
    assert_eq!(count(&s.feed, "SELECT COUNT(*) FROM users"), 0);
    assert!(s.feed.sync().ok());
    demo::seed(&s.feed.db_path, s.feed.clock_now()).expect("seeded");
    let err = demo::seed(&s.feed.db_path, s.feed.clock_now()).expect_err("twice");
    assert!(err.to_string().contains("already exist"), "{err}");
    assert_eq!(count(&s.feed, "SELECT COUNT(*) FROM users"), 8);
}

#[test]
fn seeding_measures_only_the_listed_packs_and_refuses_when_none_is() {
    // The repository's own registry lists glossary-terms and decision-log but
    // no review-dates; the demo measures the two it finds.
    let s = setup_with(&index_without(&["review-dates"]));
    assert!(s.feed.sync().ok());
    let seeded = demo::seed(&s.feed.db_path, s.feed.clock_now()).expect("seeded");
    assert_eq!(seeded.measurements, 6);
    assert_eq!(
        count(
            &s.feed,
            "SELECT COUNT(*) FROM measurements WHERE pack = 'review-dates'"
        ),
        0
    );

    let s = setup_with(&index_without(&[
        "glossary-terms",
        "decision-log",
        "review-dates",
    ]));
    assert!(s.feed.sync().ok());
    let err = demo::seed(&s.feed.db_path, s.feed.clock_now()).expect_err("none listed");
    assert!(err.to_string().contains("run sync-registry first"), "{err}");
    assert_eq!(count(&s.feed, "SELECT COUNT(*) FROM users"), 0);
}

#[test]
fn the_front_page_shows_the_demo_and_the_rows_are_as_listed() {
    let s = setup();
    let feed = &s.feed;
    assert!(feed.sync().ok());
    let now = feed.clock_now();
    let seeded = demo::seed(&feed.db_path, now).expect("seeded");
    assert_eq!(
        (
            seeded.members,
            seeded.discussions,
            seeded.comments,
            seeded.votes,
            seeded.measurements
        ),
        (8, 12, 40, 150, 7)
    );

    let mut client = feed.client();
    let page = client.get("/");
    assert_eq!(page.status, 200);
    // Two packs have evidence: three counted measurements each.
    assert_eq!(page.body.matches(" evidence (3)</span>").count(), 2);
    assert!(page.body.contains("1 of 3 measurements"));
    let titles: Vec<String> = {
        let conn = feed.conn();
        let mut stmt = conn
            .prepare("SELECT title FROM posts WHERE kind = 'discussion' ORDER BY id")
            .expect("prepare");
        stmt.query_map([], |r| r.get(0))
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("titles")
    };
    assert_eq!(titles.len(), 12);
    for title in &titles {
        assert!(title.contains("[demo] "), "{title}");
        assert!(page.body.contains(title.as_str()), "{title} is not on /");
    }
    assert_eq!(titles.iter().filter(|t| posts::is_ask(t)).count(), 2);
    assert_eq!(titles.iter().filter(|t| posts::is_show(t)).count(), 2);
    assert_eq!(
        titles.iter().filter(|t| t.starts_with("[demo] ")).count(),
        8
    );

    // Links on example.com for six posts (four others and two shows); the
    // other six have a body only.
    assert_eq!(
        count(
            feed,
            "SELECT COUNT(*) FROM posts WHERE link LIKE 'https://example.com/%'"
        ),
        6
    );
    assert_eq!(
        count(
            feed,
            "SELECT COUNT(*) FROM posts WHERE kind = 'discussion' AND link IS NULL AND body <> ''"
        ),
        6
    );

    // Members: demo-1 to demo-8, ids -1 to -8, established.
    let conn = feed.conn();
    for n in 1..=8_i64 {
        let id: i64 = conn
            .query_row(
                "SELECT id FROM users WHERE login = ?1 AND github_id = ?2",
                rusqlite::params![format!("demo-{n}"), -n],
                |r| r.get(0),
            )
            .expect("member");
        let member = users::load_member(&conn, UserId(id))
            .expect("load")
            .expect("member");
        assert_eq!(users::standing(&member, now), users::Standing::Established);
    }

    // Comments: 40, none deeper than level two, replies on their parent's post.
    assert_eq!(count(feed, "SELECT COUNT(*) FROM comments"), 40);
    assert_eq!(count(feed, "SELECT MAX(depth) FROM comments"), 2);
    assert_eq!(
        count(
            feed,
            "SELECT COUNT(*) FROM comments c JOIN comments p ON p.id = c.parent_id
             WHERE p.post_id <> c.post_id OR c.depth <> p.depth + 1"
        ),
        0
    );

    // Votes: 150, none on one's own item.
    assert_eq!(count(feed, "SELECT COUNT(*) FROM votes"), 150);
    assert_eq!(
        count(
            feed,
            "SELECT COUNT(*) FROM votes v
             LEFT JOIN posts p ON v.target_type = 'post' AND p.id = v.target_id
             LEFT JOIN comments c ON v.target_type = 'comment' AND c.id = v.target_id
             WHERE v.user_id = COALESCE(p.author_id, c.author_id)"
        ),
        0
    );

    // Measurements: three of glossary-terms, three of decision-log, one of
    // review-dates, each by an established member who is not a maintainer.
    // (The page-level "two packs with evidence" check needs the evidence
    // queries of the pack list; this reads the rows.)
    for (pack, want) in [
        ("glossary-terms", 3),
        ("decision-log", 3),
        ("review-dates", 1),
    ] {
        let rows: Vec<(i64, String)> = {
            let mut stmt = conn
                .prepare("SELECT user_id, eval_set FROM measurements WHERE pack = ?1")
                .expect("prepare");
            stmt.query_map([pack], |r| Ok((r.get(0)?, r.get(1)?)))
                .expect("query")
                .collect::<Result<_, _>>()
                .expect("rows")
        };
        assert_eq!(rows.len(), want, "{pack}");
        for (user, set) in rows {
            assert_eq!(set, demo::EVAL_SET);
            let member = users::load_member(&conn, UserId(user))
                .expect("load")
                .expect("member");
            assert_eq!(users::standing(&member, now), users::Standing::Established);
            let sql = format!("SELECT {}", votes::maintainer_sql("?1", "?2", "?3"));
            let maintainer: bool = conn
                .query_row(
                    &sql,
                    rusqlite::params![pack, member.github_id, member.login],
                    |r| r.get(0),
                )
                .expect("maintainer");
            assert!(!maintainer, "{} maintains {pack}", member.login);
        }
    }
    assert_eq!(count(feed, "SELECT COUNT(*) FROM reports"), 0);

    // Nothing in the demo looks like a ring.
    let reports = feed
        .block_on(hippo_feed::jobs::ring_check(feed.state(), now))
        .expect("ring check");
    assert!(reports.is_empty(), "{reports:?}");
}

#[test]
fn the_command_seeds_once_in_dev_mode() {
    let raw = RawServer::start_default();
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("feed.sqlite");
    let index = dir.path().join("index.json");
    std::fs::copy(fixtures().join("registry-index.json"), &index).expect("copy");
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_hippo-feed"))
            .env_clear()
            .env("FEED_DB", &db)
            .env("FEED_DEV", "1")
            .env("FEED_REGISTRY_INDEX", &index)
            .env("FEED_RAW_URL", &raw.url)
            .args(args)
            .output()
            .expect("run")
    };
    let text = |b: &[u8]| String::from_utf8_lossy(b).to_string();
    assert!(run(&["migrate"]).status.success());
    let out = run(&["seed-demo"]);
    assert!(text(&out.stderr).contains("run sync-registry first"));
    let out = run(&["sync-registry"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let out = run(&["seed-demo"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(
        text(&out.stdout).contains("FEED_EVAL_SETS=000000000000:7:8"),
        "{}",
        text(&out.stdout)
    );
    let out = run(&["seed-demo"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("already exist"));
}
