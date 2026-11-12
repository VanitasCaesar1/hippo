//! Reports, moderation and the admin pages over HTTP: the public log,
//! the admin list and the protected styling of `/mod` and `/admin`.
//!
//! Members sign in through the mock provider. A measurement is inserted
//! with SQL where the test says so.

mod common;

use std::path::{Path, PathBuf};

use common::{Options, RawServer, Reply, TestClient, TestFeed};
use hippo_feed::auth::mock::{demo_users, MockUser};
use time::macros::datetime;
use time::Duration;

const OLD: &str = "2026-01-01T00:00:00.000000Z";
const BOSS: i64 = 2001;

fn count(feed: &TestFeed, sql: &str) -> i64 {
    feed.conn().query_row(sql, [], |r| r.get(0)).expect("count")
}

fn users() -> Vec<MockUser> {
    let mut users = demo_users(datetime!(2026-11-08 10:00:00 UTC));
    for (id, login) in [(BOSS, "boss"), (2002, "maya")] {
        users.push(MockUser::new(id, login, datetime!(2015-01-01 00:00:00 UTC)));
    }
    users
}

fn options() -> Options {
    Options {
        mock_users: Some(users()),
        admins: vec![BOSS],
        ..Options::default()
    }
}

/// Signs `login` in. Every member but `newbie` is made established by SQL.
fn member(feed: &TestFeed, login: &str) -> TestClient {
    let mut client = feed.client();
    client.sign_in(login);
    feed.conn()
        .execute(
            "UPDATE users SET created_at = ?1 WHERE login = ?2 AND ?2 != 'newbie'",
            [OLD, login],
        )
        .expect("age");
    client.get("/");
    client
}

fn location(reply: &Reply) -> String {
    reply.header("location").unwrap_or("").to_string()
}

fn submit(client: &mut TestClient, title: &str, body: &str) -> i64 {
    let r = client.post_form("/submit", &[("title", title), ("link", ""), ("body", body)]);
    assert_eq!(r.status, 303, "{}", r.body);
    location(&r)
        .strip_prefix("/p/")
        .and_then(|l| l.parse().ok())
        .expect("post id")
}

fn report(client: &mut TestClient, kind: &str, id: i64, reason: &str, note: &str) -> Reply {
    client.post_form(
        "/report",
        &[
            ("target_type", kind),
            ("target_id", &id.to_string()),
            ("reason", reason),
            ("note", note),
        ],
    )
}

fn act(
    client: &mut TestClient,
    action: &str,
    kind: &str,
    id: i64,
    reason: &str,
    until: &str,
) -> Reply {
    client.post_form(
        "/mod/action",
        &[
            ("action", action),
            ("target_type", kind),
            ("target_id", &id.to_string()),
            ("reason", reason),
            ("until", until),
            ("return_to", "/mod"),
        ],
    )
}

fn user_id(feed: &TestFeed, login: &str) -> i64 {
    feed.conn()
        .query_row("SELECT id FROM users WHERE login = ?1", [login], |r| {
            r.get(0)
        })
        .expect("user")
}

fn appoint(boss: &mut TestClient, feed: &TestFeed, login: &str) {
    boss.get("/admin");
    let id = user_id(feed, login);
    let r = boss.post_form(
        "/admin/role",
        &[
            ("user_id", &id.to_string()),
            ("role", "moderator"),
            ("reason", "other"),
        ],
    );
    assert_eq!(location(&r), "/admin", "{}", r.body);
}

// --- Reports and moderation ---

#[test]
fn cuj5_spam_is_reported_hidden_removed_and_the_account_spam_banned() {
    let feed = TestFeed::start(options());
    let mut boss = member(&feed, "boss");
    let mut maya = member(&feed, "maya");
    let mut grace = member(&feed, "grace");
    let mut linus = member(&feed, "linus");
    let mut alan = member(&feed, "alan");
    let mut ada = member(&feed, "ada");
    let mut newbie = member(&feed, "newbie");
    appoint(&mut boss, &feed, "maya");

    // A new account posts spam, posts again a day later, and comments.
    let spam = submit(&mut newbie, "Cheap watches", "BUY CHEAP WATCHES NOW today");
    feed.clock.advance(Duration::hours(25));
    let other = submit(&mut newbie, "More offers", "SECOND SPAM TEXT");
    let honest = submit(&mut ada, "A real post", "hello");
    let r = newbie.post_form(
        &format!("/p/{honest}/comment"),
        &[("body", "SPAM COMMENT here"), ("parent", "")],
    );
    assert_eq!(r.status, 303);

    // Two reports do not hide it; the third, from an established member, does.
    for (client, note) in [(&mut grace, "advert one"), (&mut linus, "advert two")] {
        let r = report(client, "post", spam, "spam", note);
        assert_eq!(location(&r), format!("/p/{spam}"));
        assert!(client.get(&format!("/p/{spam}")).body.contains("reported"));
    }
    assert!(feed.client().get("/new").body.contains("Cheap watches"));
    assert_eq!(
        count(
            &feed,
            "SELECT COUNT(*) FROM audit_log WHERE action = 'auto.hide'"
        ),
        0
    );
    report(&mut alan, "post", spam, "spam", "advert three");
    let page = feed.client().get(&format!("/p/{spam}"));
    assert_eq!(page.status, 200);
    assert!(page.body.contains("Hidden while a moderator reviews it."));
    assert!(!feed.client().get("/new").body.contains("Cheap watches"));
    assert_eq!(
        count(
            &feed,
            "SELECT COUNT(*) FROM audit_log WHERE action = 'auto.hide' AND actor_id IS NULL AND public = 1"
        ),
        1
    );

    // The moderator's queue: counts, each note, snapshot and reporter.
    let queue = maya.get("/mod");
    assert_eq!(queue.status, 200);
    assert!(queue.body.contains("<h1>Moderation queue</h1>"));
    assert!(queue.body.contains("spam 3"));
    for needle in [
        "advert one",
        "advert two",
        "advert three",
        "BUY CHEAP WATCHES NOW today",
    ] {
        assert!(queue.body.contains(needle), "{needle}");
    }
    for login in ["grace", "linus", "alan"] {
        assert!(queue.body.contains(&format!(r#"href="/mod/u/{login}""#)));
    }
    assert!(queue.body.contains("Preview: Cheap watches"));
    let r = act(&mut maya, "post.remove", "post", spam, "spam", "");
    assert_eq!(location(&r), "/mod");
    let page = feed.client().get(&format!("/p/{spam}"));
    assert!(page.body.contains("[removed: spam]"));
    assert!(!page.body.contains("BUY CHEAP"));
    assert_eq!(
        count(
            &feed,
            "SELECT COUNT(*) FROM reports WHERE status = 'upheld'"
        ),
        3
    );
    assert!(maya.get("/mod").body.contains("No open reports."));

    // A moderator may not ban; an admin spam-bans, which removes the rest.
    assert_eq!(
        act(
            &mut maya,
            "user.spam_ban",
            "user",
            user_id(&feed, "newbie"),
            "spam",
            ""
        )
        .status,
        403
    );
    let view = boss.get("/mod/u/newbie");
    assert_eq!(view.status, 200);
    assert!(view.body.contains("<h1>Member: newbie</h1>"));
    assert!(view.body.contains("post.remove"));
    let r = act(
        &mut boss,
        "user.spam_ban",
        "user",
        user_id(&feed, "newbie"),
        "spam",
        "",
    );
    assert_eq!(location(&r), "/mod");
    assert_eq!(
        count(&feed, "SELECT COUNT(*) FROM posts WHERE state = 'removed'"),
        2
    );
    assert_eq!(
        count(
            &feed,
            "SELECT COUNT(*) FROM comments WHERE state = 'removed'"
        ),
        1
    );
    assert!(feed
        .client()
        .get(&format!("/p/{other}"))
        .body
        .contains("[removed: spam]"));
    // Banned: signed out, and may not sign back in.
    let r = newbie.get("/settings");
    assert_eq!(r.status, 303);
    assert!(location(&r).starts_with("/login"));

    // The public log shows each action and who took it, and not the
    // reporters, their notes or the snapshot.
    let log = feed.client().get("/moderation");
    assert_eq!(log.status, 200);
    assert!(log.body.contains("<h1>Moderation log</h1>"));
    for needle in [
        "auto.hide",
        "post.remove",
        "user.spam_ban",
        "user.role",
        "system",
        "maya",
        "boss",
    ] {
        assert!(log.body.contains(needle), "{needle}");
    }
    assert!(log.body.contains(&format!(r#"href="/p/{spam}""#)));
    for secret in [
        "grace",
        "linus",
        "alan",
        "advert one",
        "advert two",
        "advert three",
        "BUY CHEAP",
        "SECOND SPAM",
        "SPAM COMMENT",
        "Snapshot",
    ] {
        assert!(!log.body.contains(secret), "{secret} leaked into the log");
    }
    // The log is public, and a page past the end is empty, not an error.
    assert_eq!(feed.client().get("/moderation?page=3").status, 200);
    assert_eq!(feed.client().get("/moderation?page=0").status, 404);
}

#[test]
fn dismissing_reports_is_not_public_and_a_hidden_item_returns() {
    let feed = TestFeed::start(options());
    let mut maya = member(&feed, "maya");
    feed.conn()
        .execute(
            "UPDATE users SET role = 'moderator' WHERE login = 'maya'",
            [],
        )
        .expect("role");
    let mut ada = member(&feed, "ada");
    let mut grace = member(&feed, "grace");
    let mut linus = member(&feed, "linus");
    let mut alan = member(&feed, "alan");
    let post = submit(&mut ada, "Fine post", "fine");
    // One report, dismissed: recorded, but not public.
    assert_eq!(
        report(&mut grace, "post", post, "other", "SECRET NOTE").status,
        303
    );
    let r = act(&mut maya, "report.dismiss", "post", post, "other", "");
    assert_eq!(r.status, 303);
    assert_eq!(
        count(
            &feed,
            "SELECT COUNT(*) FROM audit_log WHERE action = 'report.dismiss' AND public = 0"
        ),
        1
    );
    let log = feed.client().get("/moderation").body;
    assert!(!log.contains("report.dismiss"));
    assert!(!log.contains("SECRET NOTE"));
    // Three more reports hide it; dismissing them makes it visible again and
    // is a public restore.
    let comment_post = submit(&mut ada, "Another post", "text");
    for client in [&mut grace, &mut linus, &mut alan] {
        report(client, "post", comment_post, "abuse", "");
    }
    assert!(feed
        .client()
        .get(&format!("/p/{comment_post}"))
        .body
        .contains("Hidden while"));
    act(
        &mut maya,
        "report.dismiss",
        "post",
        comment_post,
        "other",
        "",
    );
    assert!(feed
        .client()
        .get(&format!("/p/{comment_post}"))
        .body
        .contains(r#"<h1 class="hp-sr">Another post</h1>"#));
    assert!(feed
        .client()
        .get("/moderation")
        .body
        .contains("post.restore"));
    // The reporters can still not report their own, nor twice.
    let r = report(&mut ada, "post", post, "spam", "");
    assert_eq!(r.status, 422, "own content");
    let r = report(&mut grace, "post", comment_post, "spam", "");
    assert_eq!(r.status, 422, "twice");
}

// --- Admins ---

#[test]
fn an_admin_reaches_admin_until_the_id_leaves_the_environment() {
    let feed = TestFeed::start(options());
    let mut boss = member(&feed, "boss");
    let mut ada = member(&feed, "ada");
    let page = boss.get("/admin");
    assert_eq!(page.status, 200);
    assert!(page.body.contains("<h1>Admin</h1>"));
    assert!(page.body.contains("No registry index is configured."));
    assert_eq!(
        boss.get("/mod").status,
        200,
        "an admin can do what a moderator can"
    );
    // Nobody else, signed in or not, is told the page exists.
    assert_eq!(ada.get("/admin").status, 404);
    assert_eq!(feed.client().get("/admin").status, 404);
    assert_eq!(ada.get("/mod").status, 404);
    assert_eq!(ada.get("/mod/u/boss").status, 404);
    assert_eq!(
        ada.post_form("/admin/role", &[("user_id", "1"), ("role", "moderator")])
            .status,
        404
    );
    assert_eq!(ada.post_form("/admin/sync", &[]).status, 404);
    assert_eq!(
        act(&mut ada, "post.lock", "post", 1, "spam", "").status,
        404
    );
    // Syncing without an index is refused.
    assert_eq!(boss.post_form("/admin/sync", &[]).status, 409);

    // The operator removes the id and restarts: a copy of the database on a
    // server whose FEED_ADMINS is empty.
    let dir = tempfile::tempdir().expect("tempdir");
    let info = hippo_feed::backup::backup(&feed.db_path, dir.path(), 14, feed.clock_now())
        .expect("backup");
    let restarted = TestFeed::start(Options {
        database: Some(info.path),
        mock_users: Some(users()),
        ..Options::default()
    });
    let mut boss = restarted.client();
    boss.sign_in("boss");
    assert_eq!(boss.get("/admin").status, 404);
    assert_eq!(boss.get("/mod").status, 404);
    assert_eq!(boss.post_form("/admin/sync", &[]).status, 404);
}

#[test]
fn the_admin_appoints_and_removes_a_moderator() {
    let feed = TestFeed::start(options());
    let mut boss = member(&feed, "boss");
    let mut maya = member(&feed, "maya");
    assert_eq!(maya.get("/mod").status, 404);
    appoint(&mut boss, &feed, "maya");
    assert_eq!(maya.get("/mod").status, 200);
    assert_eq!(
        maya.get("/admin").status,
        404,
        "a moderator is not an admin"
    );
    let page = boss.get("/admin");
    assert!(page.body.contains("Remove moderator"));
    assert!(page.body.contains("<h2>Moderators</h2>"));
    // The role change is on the public log, with the admin as actor.
    let log = feed.client().get("/moderation").body;
    assert!(log.contains("user.role"));
    assert!(log.contains("boss"));
    let id = user_id(&feed, "maya");
    let r = boss.post_form(
        "/admin/role",
        &[
            ("user_id", &id.to_string()),
            ("role", "none"),
            ("reason", "other"),
        ],
    );
    assert_eq!(location(&r), "/admin");
    assert_eq!(maya.get("/mod").status, 404);
    // A bad role is a bad request; nobody takes a role by editing the form
    // into an admin.
    let r = boss.post_form(
        "/admin/role",
        &[("user_id", &id.to_string()), ("role", "admin")],
    );
    assert_eq!(r.status, 400);
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/feed")
}

#[test]
fn the_sync_button_runs_a_sync_as_the_admin_and_malicious_packs_remind_moderators() {
    let raw = RawServer::start_default();
    let dir = tempfile::tempdir().expect("tempdir");
    let index = dir.path().join("index.json");
    std::fs::copy(fixtures().join("registry-index.json"), &index).expect("copy");
    let feed = TestFeed::start(Options {
        registry_index: Some(index.display().to_string()),
        raw_url: Some(raw.url.clone()),
        eval_sets: Some("aaaaaaaaaaaa:6:10".into()),
        ..options()
    });
    let mut boss = member(&feed, "boss");
    let mut grace = member(&feed, "grace");
    let mut linus = member(&feed, "linus");

    let page = boss.get("/admin");
    assert!(page.body.contains("never"));
    let r = boss.post_form("/admin/sync", &[]);
    assert_eq!(location(&r), "/admin");
    let page = boss.get("/admin");
    assert!(page
        .body
        .contains("<dt>Last successful sync</dt><dd>2026-11-08T10:00:00Z</dd>"));
    assert!(page
        .body
        .contains("<dt>Last sync read the index</dt><dd>yes</dd>"));
    assert!(count(&feed, "SELECT COUNT(*) FROM packs") > 0);
    // An admin's sync is public, with the admin as actor.
    assert_eq!(
        count(
            &feed,
            "SELECT COUNT(*) FROM audit_log WHERE action = 'registry.sync' AND actor_id IS NOT NULL AND public = 1"
        ),
        1
    );
    assert!(feed
        .client()
        .get("/moderation")
        .body
        .contains("registry.sync"));

    // A malicious report on a pack's post: the queue says to forward it.
    let post: i64 = feed
        .conn()
        .query_row(
            "SELECT id FROM posts WHERE pack = 'glossary-terms'",
            [],
            |r| r.get(0),
        )
        .expect("post");
    let r = report(&mut grace, "post", post, "malicious", "reads my keys");
    assert_eq!(location(&r), format!("/p/{post}"));
    let queue = boss.get("/mod");
    assert!(queue.body.contains("SECURITY.md"));
    assert!(queue.body.contains("forward it to the registry maintainer"));
    assert!(queue.body.contains("reads my keys"));

    // A measurement (by SQL) has a report
    // link for other members, and *reported* once the viewer has reported.
    let hash: String = feed
        .conn()
        .query_row(
            "SELECT pack_hash FROM pack_contents WHERE pack = 'glossary-terms'",
            [],
            |r| r.get(0),
        )
        .expect("hash");
    feed.conn()
        .execute(
            "INSERT INTO measurements (pack, pack_hash, user_id, eval_set, planted, kept, runs,
                corrected_without, damaged_without, corrected_with, damaged_with, hippo, model,
                created_at, updated_at)
             VALUES ('glossary-terms', ?1, (SELECT id FROM users WHERE login = 'linus'),
                'aaaaaaaaaaaa', 6, 10, 3, 6, 3, 12, 7, '1.0.0', 'model-x', ?2, ?2)",
            rusqlite::params![hash, OLD],
        )
        .expect("measurement");
    let m: i64 = count(&feed, "SELECT id FROM measurements");
    let link = format!("/report?target_type=measurement&amp;target_id={m}\"");
    assert!(grace.get(&format!("/p/{post}")).body.contains(&link));
    assert!(
        !linus.get(&format!("/p/{post}")).body.contains(&link),
        "not one's own"
    );
    let r = report(&mut grace, "measurement", m, "misleading", "");
    assert_eq!(location(&r), format!("/p/{post}#m{m}"));
    let page = grace.get(&format!("/p/{post}"));
    assert!(!page.body.contains(&link));
    assert!(page.body.contains("<td>reported</td>"));
    let r = act(
        &mut boss,
        "measurement.invalidate",
        "measurement",
        m,
        "misleading",
        "",
    );
    assert_eq!(location(&r), "/mod");
    assert!(feed
        .client()
        .get("/moderation")
        .body
        .contains("measurement.invalidate"));
}

// --- Protected styling ---

#[test]
fn mod_and_admin_link_the_built_in_theme_and_no_custom_css() {
    let feed = TestFeed::start(options());
    let mut boss = member(&feed, "boss");
    let id = user_id(&feed, "boss");
    feed.conn()
        .execute(
            "UPDATE users SET theme = 'pack:glossy', personal_css = 'a { color: red }' WHERE id = ?1",
            [id],
        )
        .expect("theme");
    let css = format!("/u/{id}/theme.css");
    // The public pages still link the personal CSS, so the check is real.
    assert!(boss.get("/new").body.contains(&css));
    assert!(boss.get("/moderation").body.contains(&css));
    for path in ["/mod", "/admin", "/mod/u/boss", "/admin?page=1"] {
        let page = boss.get(path);
        assert_eq!(page.status, 200, "{path}");
        let links: Vec<&str> = page
            .body
            .match_indices("rel=\"stylesheet\" href=\"")
            .map(|(i, _)| {
                let rest = &page.body[i + 23..];
                &rest[..rest.find('"').expect("quote")]
            })
            .collect();
        assert_eq!(links.len(), 2, "{path}: {links:?}");
        assert!(links[0].starts_with("/static/base.css?v="), "{path}");
        assert!(links[1].starts_with("/static/themes/auto.css?v="), "{path}");
        assert!(!page.body.contains("/themes/glossy"), "{path}");
        assert!(!page.body.contains(&css), "{path}");
    }
    // `?safe=1` is the same for any page.
    assert!(!boss.get("/new?safe=1").body.contains(&css));
}
