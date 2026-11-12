//! Discussions, comments, hiding, rings and the export over HTTP, and the comment cases that need a server.
//!
//! Members sign in through the mock provider. Rows not created through the
//! pages (a measurement; planted
//! comment chains too deep to type) are inserted with SQL where it says so.

mod common;

use common::{Options, Reply, TestClient, TestFeed};
use hippocamp_feed::auth::mock::{demo_users, MockUser};
use hippocamp_feed::rings;
use serde_json::Value;
use time::macros::datetime;
use time::Duration;

const OLD: &str = "2026-01-01T00:00:00.000000Z";

fn count(feed: &TestFeed, sql: &str) -> i64 {
    feed.conn().query_row(sql, [], |r| r.get(0)).expect("count")
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

fn post_id_of(reply: &Reply) -> i64 {
    assert_eq!(reply.status, 303, "{}", reply.body);
    location(reply)
        .strip_prefix("/p/")
        .and_then(|l| l.parse().ok())
        .unwrap_or_else(|| panic!("no post in {:?}", location(reply)))
}

fn submit(client: &mut TestClient, title: &str, body: &str) -> i64 {
    post_id_of(&client.post_form("/submit", &[("title", title), ("link", ""), ("body", body)]))
}

/// Posts a comment (or a reply) and returns its id from the redirect.
fn comment(client: &mut TestClient, post: i64, parent: Option<i64>, body: &str) -> i64 {
    let parent = parent.map(|p| p.to_string()).unwrap_or_default();
    let reply = client.post_form(
        &format!("/p/{post}/comment"),
        &[("body", body), ("parent", &parent)],
    );
    assert_eq!(reply.status, 303, "{}", reply.body);
    let to = location(&reply);
    let (path, anchor) = to.split_once("#c").expect("an anchor");
    assert_eq!(path, format!("/p/{post}"));
    anchor.parse().expect("comment id")
}

fn vote(client: &mut TestClient, kind: &str, id: i64) -> Reply {
    client.post_form(
        "/vote",
        &[("target_type", kind), ("target_id", &id.to_string())],
    )
}

/// The href of a *report* link, as the page writes it.
fn report_href(kind: &str, id: i64) -> String {
    format!("/report?target_type={kind}&amp;target_id={id}\"")
}

fn at(body: &str, needle: &str) -> usize {
    body.find(needle)
        .unwrap_or_else(|| panic!("{needle} is not on the page:\n{body}"))
}

// --- Discussions and comments ---

#[test]
fn cuj4_a_discussion_with_comments_replies_and_votes() {
    let feed = TestFeed::start(Options::default());
    let mut ada = member(&feed, "ada");
    let mut grace = member(&feed, "grace");
    let mut linus = member(&feed, "linus");
    let mut alan = member(&feed, "alan");

    let post = submit(
        &mut ada,
        "Ask hippocamp: how do you review notes?",
        "Weekly?",
    );
    // It is on /new at once and on / by its rank.
    assert!(feed
        .client()
        .get("/new")
        .body
        .contains("Ask hippocamp: how do you review notes?"));
    assert!(feed
        .client()
        .get("/")
        .body
        .contains("Ask hippocamp: how do you review notes?"));

    let c1 = comment(&mut grace, post, None, "Weekly, on Fridays.");
    let form = linus.get(&format!("/c/{c1}/reply"));
    assert_eq!(form.status, 200);
    assert!(form.body.contains("<h1>Reply</h1>"));
    assert!(form.body.contains("Replying to grace"));
    assert!(form.body.contains("Weekly, on Fridays."));
    assert!(form
        .body
        .contains(&format!(r#"name="parent" value="{c1}""#)));
    assert!(form.body.contains("CC BY 4.0"));
    let c2 = comment(&mut linus, post, Some(c1), "Same here, with *tags*.");
    let c3 = comment(&mut alan, post, None, "Monthly.");

    // Comments nest under their parents, each with its anchor.
    let page = ada.get(&format!("/p/{post}"));
    assert_eq!(page.status, 200);
    let body = &page.body;
    assert!(body.contains(&format!(r#"id="c{c1}""#)));
    assert!(body.contains(&format!(
        r#"<ul class="hp-children"><li class="hp-comment" id="c{c2}">"#
    )));
    assert!(at(body, &format!(r#"id="c{c1}""#)) < at(body, &format!(r#"id="c{c2}""#)));
    assert!(body.contains("<em>tags</em>"));
    // The grey line: author, age, parent only on a reply, reply, report.
    assert!(body.contains(&format!(
        r#"<a class="hp-age" href="/p/{post}#c{c2}">just now</a>"#
    )));
    assert!(body.contains(&format!(r##"<a href="/p/{post}#c{c1}">parent</a>"##)));
    assert!(body.contains(&format!(r#"href="/c/{c2}/reply""#)));
    assert!(body.contains(&report_href("comment", c3)));
    // A comment shows no points; the post row says "0 comments" as visible ones.
    assert!(body.contains("3 comments"));

    // Votes from established members count at once.
    let r = vote(&mut alan, "comment", c2);
    assert_eq!(
        location(&r),
        format!("/p/{post}#c{c2}"),
        "no return_to: back to the comment"
    );
    let r = vote(&mut grace, "post", post);
    assert_eq!(r.status, 303);
    assert!(feed.client().get("/new").body.contains("1 point</span>"));
    // Alan sees `unvote` on the comment he voted on, and no arrow for it.
    let page = alan.get(&format!("/p/{post}"));
    assert!(page.body.contains(&format!(
        r#"<input type="hidden" name="target_id" value="{c2}"><input type="hidden" name="return_to" value="/p/{post}#c{c2}"><input type="hidden" name="csrf" value=""#
    )));
    assert!(page.body.contains("unvote"));
    let r = alan.post_form(
        "/vote",
        &[
            ("target_type", "comment"),
            ("target_id", &c2.to_string()),
            ("withdraw", "1"),
            ("return_to", &format!("/p/{post}#c{c2}")),
        ],
    );
    assert_eq!(location(&r), format!("/p/{post}#c{c2}"));
    assert_eq!(
        count(
            &feed,
            "SELECT COUNT(*) FROM votes WHERE target_type = 'comment'"
        ),
        0
    );
}

// --- Comments over HTTP ---

#[test]
fn the_edit_window_and_the_author_tools() {
    let feed = TestFeed::start(Options::default());
    let mut ada = member(&feed, "ada");
    let mut grace = member(&feed, "grace");
    let mut linus = member(&feed, "linus");
    let post = submit(&mut ada, "A post", "text");
    let c = comment(&mut grace, post, None, "First draft");

    let page = grace.get(&format!("/p/{post}"));
    assert!(page.body.contains(&format!("/c/{c}/edit")));
    assert!(page.body.contains(&format!(r#"action="/c/{c}/delete""#)));
    assert!(
        !page.body.contains(&report_href("comment", c)),
        "no report on one's own"
    );
    let other = linus.get(&format!("/p/{post}"));
    assert!(!other.body.contains(&format!("/c/{c}/edit")));
    assert!(!other.body.contains(&format!("/c/{c}/delete")));
    assert_eq!(linus.get(&format!("/c/{c}/edit")).status, 403);
    assert_eq!(
        linus
            .post_form(&format!("/c/{c}/edit"), &[("body", "mine now")])
            .status,
        403
    );
    assert_eq!(linus.post_form(&format!("/c/{c}/delete"), &[]).status, 403);

    let form = grace.get(&format!("/c/{c}/edit"));
    assert_eq!(form.status, 200);
    assert!(form.body.contains("<h1>Edit</h1>"));
    assert!(form.body.contains("First draft"));
    // An empty edit is shown again with a message.
    let r = grace.post_form(&format!("/c/{c}/edit"), &[("body", "  ")]);
    assert_eq!(r.status, 422);
    assert!(r.body.contains("body"));
    let r = grace.post_form(&format!("/c/{c}/edit"), &[("body", "Second draft")]);
    assert_eq!(location(&r), format!("/p/{post}#c{c}"));
    let page = grace.get(&format!("/p/{post}"));
    assert!(page.body.contains("Second draft"));
    assert!(!page.body.contains("First draft"));
    assert!(page.body.contains("just now</a> edited"));

    // After 2 hours the text is frozen; delete stays, at any time.
    feed.clock.advance(Duration::minutes(121));
    assert_eq!(grace.get(&format!("/c/{c}/edit")).status, 403);
    assert_eq!(
        grace
            .post_form(&format!("/c/{c}/edit"), &[("body", "Late")])
            .status,
        403
    );
    let page = grace.get(&format!("/p/{post}"));
    assert!(!page.body.contains(&format!("/c/{c}/edit")));
    assert!(page.body.contains(&format!(r#"action="/c/{c}/delete""#)));
    comment(&mut linus, post, Some(c), "a reply that stays");
    let r = grace.post_form(&format!("/c/{c}/delete"), &[]);
    assert_eq!(location(&r), format!("/p/{post}#c{c}"));
    let page = feed.client().get(&format!("/p/{post}"));
    assert!(page.body.contains("[deleted]"));
    assert!(
        page.body.contains("a reply that stays"),
        "a placeholder keeps its replies"
    );
    assert!(!page.body.contains("Second draft"));
    assert_eq!(
        grace.post_form(&format!("/c/{c}/delete"), &[]).status,
        409,
        "deleting twice"
    );
}

#[test]
fn depth_limit_hidden_parents_and_locked_posts() {
    let feed = TestFeed::start(Options::default());
    let mut ada = member(&feed, "ada");
    let mut grace = member(&feed, "grace");
    let mut alan = member(&feed, "alan");
    let post = submit(&mut ada, "A deep thread", "text");
    let top = comment(&mut grace, post, None, "top");

    // Ten levels below the top comment, planted with SQL: typing them would
    // take an hour of quota. The limit then bites on the form.
    let linus_id: i64 = {
        let mut linus = member(&feed, "linus");
        linus.get("/");
        count(&feed, "SELECT id FROM users WHERE login = 'linus'")
    };
    let mut parent = top;
    let mut chain = vec![top];
    let conn = feed.conn();
    for depth in 1..=10 {
        conn.execute(
            "INSERT INTO comments (post_id, parent_id, depth, author_id, body, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![post, parent, depth, linus_id, format!("level {depth}"), OLD],
        )
        .expect("comment");
        parent = conn.last_insert_rowid();
        chain.push(parent);
    }
    let last = parent;
    let page = alan.get(&format!("/p/{post}"));
    assert_eq!(page.body.matches(r#"<ul class="hp-children">"#).count(), 10);
    assert!(page.body.contains(&format!("/c/{}/reply", chain[9])));
    assert!(
        !page.body.contains(&format!("/c/{last}/reply")),
        "no reply at depth 10"
    );
    let r = alan.get(&format!("/c/{last}/reply"));
    assert_eq!(r.status, 422);
    let r = alan.post_form(
        &format!("/p/{post}/comment"),
        &[
            ("body", "too deep, kept text"),
            ("parent", &last.to_string()),
        ],
    );
    assert_eq!(r.status, 422);
    assert!(r.body.contains("too deep, kept text"));
    assert_eq!(
        count(&feed, "SELECT COUNT(*) FROM comments WHERE depth = 10"),
        1
    );

    // A reply to a hidden comment is refused; the placeholder keeps its
    // replies; a moderator sees the text, marked.
    let hidden = chain[3];
    feed.conn()
        .execute(
            "UPDATE comments SET state = 'hidden' WHERE id = ?1",
            [hidden],
        )
        .expect("hide");
    assert_eq!(alan.get(&format!("/c/{hidden}/reply")).status, 404);
    assert_eq!(
        alan.post_form(
            &format!("/p/{post}/comment"),
            &[("body", "no"), ("parent", &hidden.to_string())]
        )
        .status,
        404
    );
    let page = alan.get(&format!("/p/{post}"));
    assert!(page.body.contains("Hidden while a moderator reviews it."));
    assert!(!page.body.contains("level 3<"), "the text is hidden");
    assert!(page.body.contains("level 4"), "the replies are kept");
    feed.conn()
        .execute(
            "UPDATE users SET role = 'moderator' WHERE login = 'alan'",
            [],
        )
        .expect("role");
    let page = alan.get(&format!("/p/{post}"));
    assert!(page.body.contains("[hidden]"));
    assert!(page.body.contains("level 3"));
    feed.conn()
        .execute(
            "UPDATE comments SET state = 'removed', removed_reason = 'spam' WHERE id = ?1",
            [hidden],
        )
        .expect("remove");
    let visitor = feed.client().get(&format!("/p/{post}"));
    assert!(visitor.body.contains("[removed: spam]"));
    assert!(!visitor.body.contains("level 3<"));
    assert!(alan
        .get(&format!("/p/{post}"))
        .body
        .contains("[removed: spam]"));
    feed.conn()
        .execute("UPDATE users SET role = 'none' WHERE login = 'alan'", [])
        .expect("role");

    // A locked post takes no new comments, still takes votes, and its form
    // says so.
    feed.conn()
        .execute("UPDATE posts SET locked = 1 WHERE id = ?1", [post])
        .expect("lock");
    let page = alan.get(&format!("/p/{post}"));
    assert!(page.body.contains("takes no new comments"));
    assert!(!page
        .body
        .contains(&format!(r#"action="/p/{post}/comment""#)));
    assert!(!page.body.contains(&format!("/c/{top}/reply")));
    assert_eq!(alan.get(&format!("/c/{top}/reply")).status, 403);
    assert_eq!(
        alan.post_form(
            &format!("/p/{post}/comment"),
            &[("body", "late"), ("parent", "")]
        )
        .status,
        403
    );
    assert_eq!(vote(&mut alan, "post", post).status, 303);
    assert_eq!(
        count(
            &feed,
            "SELECT COUNT(*) FROM votes WHERE target_type = 'post'"
        ),
        1
    );

    // A signed-out visitor is offered sign-in under the comments.
    feed.conn()
        .execute("UPDATE posts SET locked = 0 WHERE id = ?1", [post])
        .expect("unlock");
    let visitor = feed.client().get(&format!("/p/{post}"));
    assert!(visitor.body.contains("Sign in to comment"));
    let mut visitor = feed.client();
    let r = visitor.get(&format!("/c/{top}/reply"));
    assert_eq!(r.status, 303);
    assert!(location(&r).starts_with("/login?return_to="));
}

#[test]
fn the_thread_orders_by_points_then_age_then_id() {
    let feed = TestFeed::start(Options::default());
    let mut ada = member(&feed, "ada");
    let mut grace = member(&feed, "grace");
    let mut linus = member(&feed, "linus");
    let mut alan = member(&feed, "alan");
    let post = submit(&mut ada, "Ordering", "text");
    let a = comment(&mut grace, post, None, "first by age");
    feed.clock.advance(Duration::minutes(1));
    let b = comment(&mut linus, post, None, "second by age");
    feed.clock.advance(Duration::minutes(1));
    let c = comment(&mut alan, post, None, "third by age");
    // No votes: oldest first.
    let body = ada.get(&format!("/p/{post}")).body;
    assert!(at(&body, &format!(r#"id="c{a}""#)) < at(&body, &format!(r#"id="c{b}""#)));
    assert!(at(&body, &format!(r#"id="c{b}""#)) < at(&body, &format!(r#"id="c{c}""#)));
    // Points first: c has two, a and b one each (a is older).
    vote(&mut ada, "comment", c);
    vote(&mut grace, "comment", c);
    vote(&mut ada, "comment", a);
    vote(&mut ada, "comment", b);
    let body = feed.client().get(&format!("/p/{post}")).body;
    let (pa, pb, pc) = (
        at(&body, &format!(r#"id="c{a}""#)),
        at(&body, &format!(r#"id="c{b}""#)),
        at(&body, &format!(r#"id="c{c}""#)),
    );
    assert!(pc < pa && pa < pb, "{pc} {pa} {pb}");
}

#[test]
fn comments_lists_the_newest_first_and_leaves_out_hidden_posts() {
    let feed = TestFeed::start(Options::default());
    let mut ada = member(&feed, "ada");
    let mut grace = member(&feed, "grace");
    let mut linus = member(&feed, "linus");
    let p1 = submit(&mut ada, "First post", "text");
    let p2 = submit(&mut ada, "Second post", "text");
    let old = comment(&mut grace, p1, None, "oldest comment");
    feed.clock.advance(Duration::minutes(5));
    let mid = comment(&mut linus, p2, None, "middle comment");
    feed.clock.advance(Duration::minutes(5));
    let new = comment(&mut grace, p1, Some(old), "newest comment");

    let page = feed.client().get("/comments");
    assert_eq!(page.status, 200);
    assert!(page.body.contains("<h1 class=\"hp-sr\">Comments</h1>"));
    let (pn, pm, po) = (
        at(&page.body, "newest comment"),
        at(&page.body, "middle comment"),
        at(&page.body, "oldest comment"),
    );
    assert!(pn < pm && pm < po);
    assert!(page
        .body
        .contains(&format!(r#"<a href="/p/{p2}">Second post</a>"#)));
    assert!(page.body.contains("| on: "));
    assert!(page
        .body
        .contains(&format!(r##"href="/p/{p1}#c{old}">parent</a>"##)));
    let _ = (mid, new);

    // A member who hid the second post no longer sees its comments there.
    let r = linus.post_form(
        "/hide",
        &[
            ("post_id", &p2.to_string()),
            ("unhide", "0"),
            ("return_to", "/"),
        ],
    );
    assert_eq!(r.status, 303);
    let page = linus.get("/comments");
    assert!(!page.body.contains("middle comment"));
    assert!(page.body.contains("newest comment"));
    assert!(feed
        .client()
        .get("/comments")
        .body
        .contains("middle comment"));
    assert_eq!(feed.client().get("/comments?page=2").status, 200);
    assert_eq!(feed.client().get("/comments?page=x").status, 404);
}

#[test]
fn a_report_link_turns_into_reported() {
    let feed = TestFeed::start(Options::default());
    let mut ada = member(&feed, "ada");
    let mut grace = member(&feed, "grace");
    let post = submit(&mut ada, "Reportable", "text");
    let c = comment(&mut grace, post, None, "a comment");

    let page = ada.get(&format!("/p/{post}"));
    assert!(page.body.contains(&report_href("comment", c)));
    let form = ada.get(&format!("/report?target_type=comment&target_id={c}"));
    assert_eq!(form.status, 200);
    assert!(form.body.contains("<h1>Report</h1>"));
    assert!(form.body.contains(r#"name="target_type" value="comment""#));
    let r = ada.post_form(
        "/report",
        &[
            ("target_type", "comment"),
            ("target_id", &c.to_string()),
            ("reason", "spam"),
            ("note", "looks like an advert"),
        ],
    );
    assert_eq!(location(&r), format!("/p/{post}#c{c}"));
    let page = ada.get(&format!("/p/{post}"));
    assert!(page.body.contains("| reported"));
    assert!(!page.body.contains(&report_href("comment", c)));
    // A second report is shown again with the reason.
    let r = ada.post_form(
        "/report",
        &[
            ("target_type", "comment"),
            ("target_id", &c.to_string()),
            ("reason", "spam"),
            ("note", "again"),
        ],
    );
    assert_eq!(r.status, 422);
    assert!(r.body.contains("already reported"));
    assert!(r.body.contains(r#"value="again""#));
}

// --- Rings ---

fn users_with(extra: &[(i64, &str)]) -> Vec<MockUser> {
    let mut users = demo_users(datetime!(2026-11-08 10:00:00 UTC));
    for (id, login) in extra {
        users.push(MockUser::new(
            *id,
            login,
            datetime!(2015-01-01 00:00:00 UTC),
        ));
    }
    users
}

#[test]
fn cuj6_a_moderator_restricts_the_accounts_a_ring_names() {
    let feed = TestFeed::start(Options {
        mock_users: Some(users_with(&[(1010, "rita"), (1011, "maya")])),
        ..Options::default()
    });
    let mut ada = member(&feed, "ada");
    let mut voters: Vec<TestClient> = ["grace", "linus", "alan", "rita"]
        .iter()
        .map(|l| member(&feed, l))
        .collect();
    let mut maya = member(&feed, "maya");
    feed.conn()
        .execute(
            "UPDATE users SET role = 'moderator' WHERE login = 'maya'",
            [],
        )
        .expect("moderator");

    // Four accounts upvote the same member's three posts within minutes.
    let posts: Vec<i64> = (1..=3)
        .map(|i| submit(&mut ada, &format!("Ring post {i}"), "text"))
        .collect();
    for post in &posts {
        for voter in &mut voters {
            assert_eq!(vote(voter, "post", *post).status, 303);
            feed.clock.advance(Duration::seconds(20));
        }
    }
    assert!(feed.client().get("/new").body.contains("4 points</span>"));

    // The hourly check (called directly) files one system report.
    let now = feed.clock_now();
    let filed = feed
        .block_on(feed.db.write(move |tx| Ok(rings::check(tx, now)?)))
        .expect("check");
    assert_eq!(filed.len(), 1);
    let again = feed
        .block_on(feed.db.write(move |tx| Ok(rings::check(tx, now)?)))
        .expect("check");
    assert!(again.is_empty(), "a second run adds nothing");

    // The moderator sees the report with the named accounts.
    let queue = maya.get("/mod");
    assert_eq!(queue.status, 200);
    for login in ["grace", "linus", "alan", "rita"] {
        assert!(
            queue.body.contains(&format!(r#"href="/mod/u/{login}""#)),
            "{login}"
        );
    }
    assert!(queue.body.contains("Restrict the named accounts"));
    assert!(queue.body.contains("system"));
    // Nobody else does.
    assert_eq!(voters[0].get("/mod").status, 404);
    assert_eq!(feed.client().get("/mod").status, 404);

    let r = maya.post_form(
        "/mod/action",
        &[
            ("action", "ring.restrict"),
            ("target_type", "report"),
            ("target_id", &filed[0].to_string()),
            ("reason", "ring"),
            ("until", "30d"),
            ("return_to", "/mod"),
        ],
    );
    assert_eq!(location(&r), "/mod");
    assert_eq!(
        count(
            &feed,
            "SELECT COUNT(*) FROM users WHERE restricted_until IS NOT NULL"
        ),
        4
    );
    // Their votes stop counting on the next page load.
    assert!(feed.client().get("/new").body.contains("0 points</span>"));
    assert!(maya.get("/mod").body.contains("No open reports."));
    // A restricted member may not vote.
    assert_eq!(vote(&mut voters[0], "post", posts[0]).status, 403);
    // Each restriction is on the public log with reason ring; the ring
    // report itself is not.
    let log = feed.client().get("/moderation").body;
    assert_eq!(log.matches("user.restrict").count(), 4);
    assert!(!log.contains("ring.flag"));
    assert!(!log.contains("Named accounts"));
    // When the restriction ends, the votes count again.
    feed.clock.advance(Duration::days(31));
    assert!(feed.client().get("/new").body.contains("4 points</span>"));
}

// --- Export and deletion ---

#[test]
fn cuj7_a_member_exports_and_deletes_their_data() {
    let feed = TestFeed::start(Options::default());
    let mut ada = member(&feed, "ada");
    let mut grace = member(&feed, "grace");
    let ada_post = submit(&mut ada, "Ada's post", "ada text");
    let grace_post = submit(&mut grace, "Grace's post", "grace text");
    let c = comment(&mut grace, ada_post, None, "grace comment");
    vote(&mut grace, "post", ada_post);
    // A measurement by SQL: its entry form is not used here.
    {
        let conn = feed.conn();
        conn.execute(
            "INSERT INTO packs (name, kind, version, description, authors, owner, owner_id,
                 maintainers, first_party, license, repo, repo_id, path, commit_id, pack_hash,
                 passes, review_id, review_at, listed, synced_at)
             VALUES ('glossary-terms', 'passes', '1.0.0', 'd', '[]', 'alice', 7, '[]', 0, 'MIT',
                 'alice/packs', 8, 'p', 'c', 'hash1', '[]', 'r', ?1, 1, ?1)",
            [OLD],
        )
        .expect("pack");
        conn.execute(
            "INSERT INTO pack_contents (pack, pack_hash, version, commit_id, first_seen)
             VALUES ('glossary-terms', 'hash1', '1.0.0', 'c', ?1)",
            [OLD],
        )
        .expect("content");
        conn.execute(
            "INSERT INTO measurements (pack, pack_hash, user_id, eval_set, planted, kept, runs,
                 corrected_without, damaged_without, corrected_with, damaged_with, hippocamp, model,
                 created_at, updated_at)
             VALUES ('glossary-terms', 'hash1', (SELECT id FROM users WHERE login = 'grace'),
                 '3f2a9c01b7d4', 6, 8, 3, 9, 2, 12, 1, '0.1.0', 'a model', ?1, ?1)",
            [OLD],
        )
        .expect("measurement");
    }

    let export = grace.get("/settings/export");
    assert_eq!(export.status, 200);
    let json: Value = serde_json::from_str(&export.body).expect("json");
    assert_eq!(json["account"]["login"], "grace");
    let len = |key: &str| json[key].as_array().map_or(0, Vec::len);
    assert_eq!(
        (
            len("posts"),
            len("comments"),
            len("votes"),
            len("measurements")
        ),
        (1, 1, 1, 1)
    );
    assert_eq!(json["posts"][0]["id"], grace_post);
    assert_eq!(json["comments"][0]["id"], c);
    assert!(!export.body.contains("Ada's post"));

    // Deletion needs the login typed; then the member is signed out.
    let r = grace.post_form("/settings/delete", &[("confirm", "wrong")]);
    assert_eq!(r.status, 422);
    let r = grace.post_form("/settings/delete", &[("confirm", "grace")]);
    assert_eq!(r.status, 303);
    assert_eq!(location(&r), "/");
    let r = grace.get("/settings");
    assert_eq!(r.status, 303);
    assert!(location(&r).starts_with("/login"));

    // The state after deletion.
    for table in ["sessions", "votes", "measurements", "hides"] {
        assert_eq!(
            count(
                &feed,
                &format!(
                    "SELECT COUNT(*) FROM {table} WHERE user_id = (SELECT id FROM users WHERE github_id IS NULL AND deleted_at IS NOT NULL)"
                )
            ),
            0,
            "{table}"
        );
    }
    let row: (String, Option<i64>, Option<String>, String) = feed
        .conn()
        .query_row(
            "SELECT login, github_id, github_created_at, theme FROM users WHERE deleted_at IS NOT NULL",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .expect("deleted row");
    assert!(row.0.starts_with("~deleted-"));
    assert_eq!((row.1, row.2, row.3.as_str()), (None, None, "auto"));
    let page = feed.client().get(&format!("/p/{grace_post}"));
    assert_eq!(page.status, 200);
    assert!(page.body.contains("[deleted]"));
    assert!(!page.body.contains("Grace's post") && !page.body.contains("grace text"));
    // A deleted comment with no replies is left out of the thread.
    let page = feed.client().get(&format!("/p/{ada_post}"));
    assert!(!page.body.contains("grace comment"));
    assert!(!page.body.contains(">grace<"));
}

// --- Hiding ---

#[test]
fn cuj10_a_member_hides_a_post() {
    let feed = TestFeed::start(Options::default());
    let mut ada = member(&feed, "ada");
    let mut grace = member(&feed, "grace");
    let post = submit(&mut ada, "Front page post", "text");
    let title = "Front page post";
    assert!(grace.get("/").body.contains(title));
    assert!(grace
        .get("/")
        .body
        .contains(&format!(r#"name="post_id" value="{post}""#)));

    let r = grace.post_form(
        "/hide",
        &[
            ("post_id", &post.to_string()),
            ("unhide", "0"),
            ("return_to", "/"),
        ],
    );
    assert_eq!(location(&r), "/");
    assert!(!grace.get("/").body.contains(title));
    assert!(!grace.get("/new").body.contains(title));
    let hidden = grace.get("/hidden");
    assert_eq!(hidden.status, 200);
    assert!(hidden.body.contains("<h1>Hidden by you</h1>"));
    assert!(hidden.body.contains(title));
    assert!(hidden.body.contains("un-hide"));
    // Other people see no change.
    assert!(ada.get("/").body.contains(title));
    assert!(feed.client().get("/").body.contains(title));

    let r = grace.post_form(
        "/hide",
        &[
            ("post_id", &post.to_string()),
            ("unhide", "1"),
            ("return_to", "/hidden"),
        ],
    );
    assert_eq!(location(&r), "/hidden");
    assert!(grace.get("/").body.contains(title));
    assert!(grace
        .get("/hidden")
        .body
        .contains("You have not hidden any posts."));
}
