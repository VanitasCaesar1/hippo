//! Export and deletion: `/settings/export`, `/settings/delete` and the
//! `account` subcommands.

mod common;

use std::process::{Command, Output};

use common::{Options, TestFeed};
use rusqlite::Connection;
use serde_json::{json, Value};

const T0: &str = "2026-11-01T09:00:00.000000Z";

/// Rows of kinds other code creates (posts, comments, votes,
/// hides, a measurement, reports), inserted with SQL. Returns ada's and
/// grace's user ids.
fn seed(conn: &Connection) -> (i64, i64) {
    let id = |gh: i64| -> i64 {
        conn.query_row("SELECT id FROM users WHERE github_id = ?1", [gh], |r| {
            r.get(0)
        })
        .expect("user")
    };
    let (ada, grace) = (id(1001), id(1002));
    let run = |sql: &str, params: &[&dyn rusqlite::ToSql]| {
        conn.execute(sql, params).expect(sql);
    };
    run(
        "INSERT INTO posts (id, kind, author_id, title, link, body, created_at, state)
         VALUES (3, 'discussion', ?1, 'Ada asks', 'https://example.org/a', 'Ada body', ?2, 'visible')",
        &[&ada, &T0],
    );
    run(
        "INSERT INTO posts (id, kind, author_id, title, body, created_at)
         VALUES (4, 'discussion', ?1, 'Grace posts', 'Grace body', ?2)",
        &[&grace, &T0],
    );
    run(
        "INSERT INTO comments (id, post_id, depth, author_id, body, created_at)
         VALUES (9, 4, 0, ?1, 'Ada comments', ?2)",
        &[&ada, &T0],
    );
    run(
        "INSERT INTO comments (id, post_id, depth, author_id, body, created_at)
         VALUES (10, 3, 0, ?1, 'Grace comments', ?2)",
        &[&grace, &T0],
    );
    run(
        "INSERT INTO votes (user_id, target_type, target_id, at) VALUES (?1, 'post', 4, ?2)",
        &[&ada, &T0],
    );
    run(
        "INSERT INTO votes (user_id, target_type, target_id, at) VALUES (?1, 'post', 3, ?2)",
        &[&grace, &T0],
    );
    run(
        "INSERT INTO hides (user_id, post_id, at) VALUES (?1, 4, ?2)",
        &[&ada, &T0],
    );
    run(
        "INSERT INTO packs (name, kind, version, description, authors, owner, owner_id, maintainers,
             first_party, license, repo, repo_id, path, commit_id, pack_hash, passes, review_id,
             review_at, listed, synced_at)
         VALUES ('glossary-terms', 'passes', '1.0.0', 'd', '[]', 'alice', 7, '[]', 0, 'MIT',
             'alice/packs', 8, 'p', 'c', 'hash1', '[]', 'r', ?1, 1, ?1)",
        &[&T0],
    );
    run(
        "INSERT INTO pack_contents (pack, pack_hash, version, commit_id, first_seen)
         VALUES ('glossary-terms', 'hash1', '1.0.0', 'c', ?1)",
        &[&T0],
    );
    run(
        "INSERT INTO measurements (id, pack, pack_hash, user_id, eval_set, planted, kept, runs,
             corrected_without, damaged_without, corrected_with, damaged_with, hippocamp, model,
             created_at, updated_at)
         VALUES (4, 'glossary-terms', 'hash1', ?1, '3f2a9c01b7d4', 6, 8, 3, 9, 2, 12, 1, '0.1.0',
             'a model', ?2, ?2)",
        &[&ada, &T0],
    );
    // Ada reported a post; Grace reported Ada.
    run(
        "INSERT INTO reports (id, reporter_id, target_type, target_id, reason, note, snapshot, created_at)
         VALUES (2, ?1, 'post', 4, 'spam', 'ada note', 'snap', ?2)",
        &[&ada, &T0],
    );
    run(
        "INSERT INTO reports (id, reporter_id, target_type, target_id, reason, note, snapshot, created_at)
         VALUES (3, ?1, 'user', ?2, 'abuse', 'grace note about ada', 'snap', ?3)",
        &[&grace, &ada, &T0],
    );
    (ada, grace)
}

fn signed_in(feed: &TestFeed, logins: &[&str]) -> common::TestClient {
    let mut last = feed.client();
    for login in logins {
        last = feed.client();
        last.sign_in(login);
    }
    last
}

#[test]
fn the_export_holds_everything_stored_about_the_member_and_nothing_about_others() {
    let feed = TestFeed::start(Options::default());
    signed_in(&feed, &["grace"]);
    let mut client = signed_in(&feed, &["ada"]);
    let (ada, _) = seed(&feed.conn());

    let reply = client.get("/settings/export");
    assert_eq!(reply.status, 200);
    assert_eq!(reply.header("content-type"), Some("application/json"));
    assert_eq!(
        reply.header("content-disposition"),
        Some(r#"attachment; filename="hippocamp-feed-ada.json""#)
    );
    assert_eq!(reply.header("cache-control"), Some("private, no-store"));
    let export: Value = serde_json::from_str(&reply.body).expect("json");

    let mut keys: Vec<&str> = export
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "account",
            "comments",
            "exported_at",
            "hides",
            "measurements",
            "personal_css",
            "posts",
            "reports",
            "sessions",
            "votes"
        ]
    );
    assert_eq!(export["exported_at"], "2026-11-08T10:00:00Z");
    assert_eq!(
        export["account"],
        json!({
            "id": ada, "github_id": 1001, "login": "ada",
            "github_created_at": "2015-03-01T00:00:00Z",
            "created_at": "2026-11-08T10:00:00Z", "role": "none",
            "restricted_until": null, "banned": false, "theme": "auto"
        })
    );
    assert_eq!(export["personal_css"], "");
    assert_eq!(
        export["posts"],
        json!([{"id": 3, "kind": "discussion", "title": "Ada asks", "link": "https://example.org/a",
                "body": "Ada body", "created_at": "2026-11-01T09:00:00Z", "edited_at": null,
                "state": "visible"}])
    );
    assert_eq!(
        export["comments"],
        json!([{"id": 9, "post_id": 4, "parent_id": null, "body": "Ada comments",
                "created_at": "2026-11-01T09:00:00Z", "edited_at": null, "state": "visible"}])
    );
    assert_eq!(
        export["votes"],
        json!([{"target_type": "post", "target_id": 4, "at": "2026-11-01T09:00:00Z"}])
    );
    assert_eq!(
        export["hides"],
        json!([{"post_id": 4, "at": "2026-11-01T09:00:00Z"}])
    );
    assert_eq!(
        export["measurements"],
        json!([{"id": 4, "pack": "glossary-terms", "pack_hash": "hash1", "version": "1.0.0",
                "eval_set": "3f2a9c01b7d4", "runs": 3, "corrected_without": 9, "damaged_without": 2,
                "corrected_with": 12, "damaged_with": 1, "benefit": 0.208, "hippocamp": "0.1.0",
                "model": "a model", "report_url": null, "created_at": "2026-11-01T09:00:00Z",
                "updated_at": "2026-11-01T09:00:00Z", "revisions": 0, "invalid": false}])
    );
    // Only the reports Ada filed; not the one against her, and no audit rows.
    assert_eq!(
        export["reports"],
        json!([{"id": 2, "target_type": "post", "target_id": 4, "reason": "spam",
                "note": "ada note", "created_at": "2026-11-01T09:00:00Z", "status": "open"}])
    );
    assert_eq!(
        export["sessions"],
        json!([{"created_at": "2026-11-08T10:00:00Z", "expires_at": "2026-12-08T10:00:00Z"}])
    );
    for other in ["Grace", "grace note", "1002"] {
        assert!(
            !reply.body.contains(other),
            "{other} leaked into the export"
        );
    }
    // No session token hash or CSRF token either.
    assert!(!reply.body.contains("token_hash") && !reply.body.contains("csrf"));
}

#[test]
fn deleting_an_account_removes_what_goal_8_names_and_writes_one_audit_row() {
    let feed = TestFeed::start(Options::default());
    signed_in(&feed, &["grace"]);
    let mut client = signed_in(&feed, &["ada"]);
    let (ada, grace) = seed(&feed.conn());
    client.get("/settings");
    // A theme and personal CSS, to be reset.
    let saved = client.post_form(
        "/settings/theme",
        &[("theme", "dark"), ("personal_css", "body{}")],
    );
    assert_eq!(saved.status, 303);

    // The wrong text deletes nothing.
    for confirm in ["", "grace", "ADA x"] {
        let r = client.post_form("/settings/delete", &[("confirm", confirm)]);
        assert_eq!(r.status, 422, "{confirm:?}");
    }
    let conn = feed.conn();
    let live: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM users WHERE id = ?1 AND deleted_at IS NULL",
            [ada],
            |r| r.get(0),
        )
        .expect("count");
    assert_eq!(live, 1);

    let r = client.post_form("/settings/delete", &[("confirm", "ada")]);
    assert_eq!(r.status, 303);
    assert_eq!(r.header("location"), Some("/"));
    assert!(client.cookie("hippocamp_session").is_none());
    assert!(!client.get("/about").body.contains("/logout"));

    let n = |sql: &str, id: i64| -> i64 { conn.query_row(sql, [id], |r| r.get(0)).expect(sql) };
    // (a) sessions, votes, hides and measurements are gone; theme and CSS reset.
    for table in ["sessions", "votes", "measurements", "hides"] {
        assert_eq!(
            n(
                &format!("SELECT COUNT(*) FROM {table} WHERE user_id = ?1"),
                ada
            ),
            0,
            "{table}"
        );
    }
    let (theme, css): (String, String) = conn
        .query_row(
            "SELECT theme, personal_css FROM users WHERE id = ?1",
            [ada],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("row");
    assert_eq!((theme.as_str(), css.as_str()), ("auto", ""));
    // (b) posts and comments read [deleted] with no author or link.
    let post: (String, String, Option<String>, Option<i64>, String) = conn
        .query_row(
            "SELECT title, body, link, author_id, state FROM posts WHERE id = 3",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .expect("post");
    assert_eq!(
        post,
        (
            "[deleted]".into(),
            "[deleted]".into(),
            None,
            None,
            "deleted".into()
        )
    );
    let comment: (String, Option<i64>, String) = conn
        .query_row(
            "SELECT body, author_id, state FROM comments WHERE id = 9",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .expect("comment");
    assert_eq!(comment, ("[deleted]".into(), None, "deleted".into()));
    // The report Ada filed stays, without her; the one against her stays.
    let reporter: Option<i64> = conn
        .query_row("SELECT reporter_id FROM reports WHERE id = 2", [], |r| {
            r.get(0)
        })
        .expect("report");
    assert_eq!(reporter, None);
    assert_eq!(
        n(
            "SELECT COUNT(*) FROM reports WHERE id = 3 AND reporter_id = ?1",
            grace
        ),
        1
    );
    // (c) the account row holds no login, creation date or GitHub id.
    let row: (String, Option<String>, Option<i64>, Option<String>) = conn
        .query_row(
            "SELECT login, github_created_at, github_id, deleted_at FROM users WHERE id = ?1",
            [ada],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .expect("account");
    assert_eq!(row.0, format!("~deleted-{ada}"));
    assert_eq!((row.1, row.2), (None, None));
    assert_eq!(row.3.as_deref(), Some("2026-11-08T10:00:00.000000Z"));
    // Grace's own rows are untouched.
    assert_eq!(n("SELECT COUNT(*) FROM votes WHERE user_id = ?1", grace), 1);
    let intact: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM posts WHERE id = 4 AND title = 'Grace posts'",
            [],
            |r| r.get(0),
        )
        .expect("grace's post");
    assert_eq!(intact, 1);

    // One user.delete row: not public (the member asked), never the login.
    type AuditRow = (Option<i64>, String, Option<String>, String, bool, String);
    let audit: Vec<AuditRow> = conn
        .prepare("SELECT actor_id, action, target_id, reason, public, details FROM audit_log")
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
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].0, Some(ada));
    assert_eq!(audit[0].1, "user.delete");
    assert_eq!(audit[0].2.as_deref(), Some(ada.to_string().as_str()));
    assert!(!audit[0].4);
    assert!(!format!("{audit:?}").contains("ada\""), "{audit:?}");

    // Signing in again with the same GitHub id makes a new account.
    let mut again = feed.client();
    assert_eq!(again.sign_in("ada").status, 303);
    let new_id: i64 = conn
        .query_row("SELECT id FROM users WHERE github_id = 1001", [], |r| {
            r.get(0)
        })
        .expect("new");
    assert_ne!(new_id, ada);
}

fn cli(feed: &TestFeed, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_hippocamp-feed"))
        .env_clear()
        .env("FEED_DB", &feed.db_path)
        .args(args)
        .output()
        .expect("run hippocamp-feed")
}

#[test]
fn account_export_and_account_delete_use_the_same_code_as_settings() {
    let feed = TestFeed::start(Options::default());
    signed_in(&feed, &["grace"]);
    signed_in(&feed, &["ada"]);
    let (ada, grace) = seed(&feed.conn());

    let out = cli(&feed, &["account", "export", "ada"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let export: Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(export["account"]["login"], "ada");
    assert_eq!(export["posts"][0]["title"], "Ada asks");

    let out = cli(&feed, &["account", "export", "nobody"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("nobody"));

    let out = cli(&feed, &["account", "delete", "ada"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "deleted ada\n");
    let conn = feed.conn();
    let row: (String, Option<i64>) = conn
        .query_row(
            "SELECT login, github_id FROM users WHERE id = ?1",
            [ada],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("row");
    assert_eq!(row, (format!("~deleted-{ada}"), None));
    // A public row by the system, that does not name the member.
    let (actor, public, text): (Option<i64>, bool, String) = conn
        .query_row(
            "SELECT actor_id, public, action || target_type || target_id || reason || details
             FROM audit_log WHERE action = 'user.delete'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .expect("audit");
    assert_eq!(actor, None);
    assert!(public);
    assert!(!text.contains("ada"), "{text}");
    // The login is gone, so a second delete finds no one.
    assert_eq!(
        cli(&feed, &["account", "delete", "ada"]).status.code(),
        Some(1)
    );

    // A banned account keeps its GitHub id when it is deleted.
    conn.execute("UPDATE users SET banned = 1 WHERE id = ?1", [grace])
        .expect("ban");
    assert!(cli(&feed, &["account", "delete", "grace"]).status.success());
    let kept: Option<i64> = conn
        .query_row("SELECT github_id FROM users WHERE id = ?1", [grace], |r| {
            r.get(0)
        })
        .expect("grace");
    assert_eq!(kept, Some(1002));
}
