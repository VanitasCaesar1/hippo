//! Discussions, votes, hiding, the lists and their pages over HTTP.
//!
//! Members sign in through the mock provider. Everything not created through
//! the pages (an established standing needs three days, so the tests set
//! `users.created_at` with SQL; measurements are inserted with SQL too)
//! says so where it happens.

mod common;

use std::path::{Path, PathBuf};

use common::{Options, RawServer, Reply, TestClient, TestFeed};
use hippo_feed::clock;
use time::Duration;

const OLD: &str = "2026-01-01T00:00:00.000000Z";

fn count(feed: &TestFeed, sql: &str) -> i64 {
    feed.conn().query_row(sql, [], |r| r.get(0)).expect("count")
}

/// Signs `login` in. Every demo member but `newbie` is made established by
/// SQL (the feed account is set to have been created long ago).
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

fn id_of(reply: &Reply) -> i64 {
    assert_eq!(reply.status, 303, "{}", reply.body);
    reply
        .header("location")
        .and_then(|l| l.strip_prefix("/p/"))
        .and_then(|l| l.parse().ok())
        .unwrap_or_else(|| panic!("no post in {:?}", reply.header("location")))
}

fn submit(client: &mut TestClient, title: &str, link: &str, body: &str) -> Reply {
    client.post_form(
        "/submit",
        &[("title", title), ("link", link), ("body", body)],
    )
}

fn link_of(reply: &Reply) -> String {
    reply.header("location").unwrap_or("").to_string()
}

#[test]
fn submit_edit_and_delete_a_discussion() {
    let feed = TestFeed::start(Options::default());
    let mut ada = member(&feed, "ada");
    let mut grace = member(&feed, "grace");

    let form = ada.get("/submit");
    assert_eq!(form.status, 200);
    assert!(form.body.contains("CC BY 4.0"), "the licence line");
    assert!(form.body.contains("<h1>Submit</h1>"));

    // A visitor is sent to sign in.
    let mut visitor = feed.client();
    let r = visitor.get("/submit");
    assert_eq!(r.status, 303);
    assert!(link_of(&r).starts_with("/login?return_to="));

    // With a body only.
    let id = id_of(&submit(&mut ada, "A note", "", "Hello *world*"));
    let page = ada.get(&format!("/p/{id}"));
    assert_eq!(page.status, 200);
    assert!(page.body.contains(r#"<h1 class="hp-sr">A note</h1>"#));
    assert!(page.body.contains("<em>world</em>"));
    assert!(!page.body.contains("hp-rank"), "the row is unnumbered");
    assert!(page.body.contains(&format!("/p/{id}/edit")));
    assert!(page.body.contains(&format!("/p/{id}/delete")));
    // Others see no author tools.
    let other = grace.get(&format!("/p/{id}"));
    assert!(!other.body.contains(&format!("/p/{id}/edit")));
    assert_eq!(grace.get(&format!("/p/{id}/edit")).status, 403);
    assert_eq!(
        grace
            .post_form(&format!("/p/{id}/edit"), &[("title", "x"), ("body", "y")])
            .status,
        403
    );
    assert_eq!(grace.post_form(&format!("/p/{id}/delete"), &[]).status, 403);

    // Edit within the window: a link is added, and the post shows *edited*.
    let edit = ada.get(&format!("/p/{id}/edit"));
    assert_eq!(edit.status, 200);
    assert!(edit.body.contains(r#"value="A note""#));
    let r = ada.post_form(
        &format!("/p/{id}/edit"),
        &[
            ("title", "A better note"),
            ("link", "https://example.com/a"),
            ("body", "Hello again"),
        ],
    );
    assert_eq!(link_of(&r), format!("/p/{id}"));
    let page = ada.get(&format!("/p/{id}"));
    assert!(page
        .body
        .contains(r#"<h1 class="hp-sr">A better note</h1>"#));
    assert!(page.body.contains(r#"<p class="hp-help">edited</p>"#));
    assert!(page
        .body
        .contains(r#"href="https://example.com/a" rel="nofollow ugc noopener noreferrer""#));
    // An edit that breaks a rule is shown again with the values entered.
    let r = ada.post_form(
        &format!("/p/{id}/edit"),
        &[("title", ""), ("body", "kept text")],
    );
    assert_eq!(r.status, 422);
    assert!(r.body.contains("kept text"));

    // After 2 hours the text is frozen and the edit link is gone; delete stays.
    feed.clock.advance(Duration::minutes(121));
    assert_eq!(ada.get(&format!("/p/{id}/edit")).status, 403);
    let r = ada.post_form(
        &format!("/p/{id}/edit"),
        &[("title", "Late"), ("body", "late")],
    );
    assert_eq!(r.status, 403);
    let page = ada.get(&format!("/p/{id}"));
    assert!(!page.body.contains(&format!("/p/{id}/edit")));
    assert!(page.body.contains(&format!("/p/{id}/delete")));

    // Delete: the page stays at 200 with the placeholder, the lists drop it.
    let r = ada.post_form(&format!("/p/{id}/delete"), &[]);
    assert_eq!(link_of(&r), format!("/p/{id}"));
    let page = ada.get(&format!("/p/{id}"));
    assert_eq!(page.status, 200);
    assert!(page.body.contains("[deleted]"));
    assert!(!page.body.contains("A better note"));
    assert!(!ada.get("/new").body.contains(&format!("/p/{id}\"")));
    // Deleting twice is a conflict.
    assert_eq!(ada.post_form(&format!("/p/{id}/delete"), &[]).status, 409);
}

#[test]
fn a_post_page_answers_200_in_every_state_with_its_placeholder() {
    let feed = TestFeed::start(Options::default());
    let mut ada = member(&feed, "ada");
    let id = id_of(&submit(&mut ada, "Secret title", "", "secret body"));
    // Hidden, removed and deleted by moderation or reports are set with SQL.
    for (state, reason, shown) in [
        ("hidden", None, "Hidden while a moderator reviews it."),
        ("removed", Some("spam"), "[removed: spam]"),
        ("deleted", None, "[deleted]"),
    ] {
        feed.conn()
            .execute(
                "UPDATE posts SET state = ?1, removed_reason = ?2 WHERE id = ?3",
                rusqlite::params![state, reason, id],
            )
            .expect("state");
        let mut visitor = feed.client();
        let page = visitor.get(&format!("/p/{id}"));
        assert_eq!(page.status, 200, "{state}");
        assert!(page.body.contains(shown), "{state}: {}", page.body);
        assert!(!page.body.contains("Secret title"), "{state}");
        assert!(!page.body.contains("secret body"), "{state}");
        assert!(!visitor.get("/new").body.contains(&format!("/p/{id}\"")));
    }
    assert_eq!(feed.client().get("/p/999").status, 404);
    assert_eq!(feed.client().get("/p/x").status, 404);
}

#[test]
fn links_show_their_domain_and_are_stored_as_the_url_crate_writes_them() {
    let feed = TestFeed::start(Options::default());
    let cases = [
        (
            "ada",
            "HTTP://Example.com",
            "http://example.com/",
            "example.com",
        ),
        (
            "grace",
            "https://www.Example.org:8443/x?a=1&b=2",
            "https://www.example.org:8443/x?a=1&b=2",
            "example.org",
        ),
        (
            "linus",
            "https://b\u{fc}cher.example/",
            "https://xn--bcher-kva.example/",
            "xn--bcher-kva.example",
        ),
        (
            "alan",
            "http://192.168.1.5:8080/",
            "http://192.168.1.5:8080/",
            "192.168.1.5",
        ),
    ];
    for (login, entered, stored, domain) in cases {
        let mut client = member(&feed, login);
        let id = id_of(&submit(&mut client, &format!("By {login}"), entered, ""));
        let got: String = feed
            .conn()
            .query_row("SELECT link FROM posts WHERE id = ?1", [id], |r| r.get(0))
            .expect("link");
        assert_eq!(got, stored);
        let list = client.get("/new").body;
        let escaped = stored.replace('&', "&amp;");
        assert!(
            list.contains(&format!(
                r#"<a class="hp-title-link" href="{escaped}" rel="nofollow ugc noopener noreferrer">By {login}</a> <span class="hp-domain">({domain})</span>"#
            )),
            "{login}: {list}"
        );
    }
    // Refused: a user name, another scheme, no host.
    let mut ada = member(&feed, "ada");
    for (link, rule) in [
        ("https://user:pw@example.com/", "user name"),
        ("ftp://example.com/", "http"),
        ("https://", "http"),
    ] {
        let r = submit(&mut ada, "Refused", link, "");
        assert_eq!(r.status, 422, "{link}");
        assert!(r.body.contains(rule), "{link}: {}", r.body);
        assert!(r.body.contains(r#"name="title""#));
    }
    assert_eq!(count(&feed, "SELECT COUNT(*) FROM posts"), 4);
}

#[test]
fn votes_withdrawals_and_karma() {
    let feed = TestFeed::start(Options::default());
    let mut ada = member(&feed, "ada");
    let mut grace = member(&feed, "grace");
    let mut newbie = member(&feed, "newbie");
    let id = id_of(&submit(&mut ada, "Worth reading", "", "text"));

    let list = grace.get("/new").body;
    assert!(list.contains(&format!(
        r#"<input type="hidden" name="target_id" value="{id}">"#
    )));
    assert!(list.contains("0 points"));
    // The author sees no arrow on their own post.
    assert!(!ada
        .get("/new")
        .body
        .contains(&format!(r#"name="target_id" value="{id}""#)));

    let vote = |client: &mut TestClient, extra: &[(&str, &str)]| {
        let id = id.to_string();
        let mut fields = vec![("target_type", "post"), ("target_id", id.as_str())];
        fields.extend_from_slice(extra);
        client.post_form("/vote", &fields)
    };
    // Redirects: return_to when local, else the target's page.
    let r = vote(&mut grace, &[("return_to", "/new")]);
    assert_eq!(link_of(&r), "/new");
    let r = vote(&mut grace, &[("return_to", "//evil.example")]);
    assert_eq!(link_of(&r), format!("/p/{id}"));
    assert_eq!(
        count(&feed, "SELECT COUNT(*) FROM votes"),
        1,
        "twice is once"
    );
    // A new member's vote is stored but not counted.
    assert_eq!(vote(&mut newbie, &[]).status, 303);
    assert_eq!(count(&feed, "SELECT COUNT(*) FROM votes"), 2);

    let list = grace.get("/new").body;
    assert!(list.contains("1 point</span>"), "{list}");
    assert!(list.contains(">unvote<"));
    // Karma in the top bar of a list page and of /about.
    for path in ["/new", "/about"] {
        assert!(
            ada.get(path)
                .body
                .contains(r#"<span class="hp-karma">(1)</span>"#),
            "{path}"
        );
        assert!(grace
            .get(path)
            .body
            .contains(r#"<span class="hp-karma">(0)</span>"#));
    }
    // One's own post cannot be voted on.
    assert_eq!(vote(&mut ada, &[]).status, 403);
    // Withdraw.
    let r = vote(&mut grace, &[("withdraw", "1"), ("return_to", "/new")]);
    assert_eq!(link_of(&r), "/new");
    assert!(grace.get("/new").body.contains("0 points"));
    assert!(ada
        .get("/about")
        .body
        .contains(r#"<span class="hp-karma">(0)</span>"#));
    // Malformed forms.
    assert_eq!(
        grace
            .post_form("/vote", &[("target_type", "x"), ("target_id", "1")])
            .status,
        400
    );
    assert_eq!(
        grace
            .post_form("/vote", &[("target_type", "post"), ("target_id", "999")])
            .status,
        404
    );
    // A vote on a hidden post is refused.
    feed.conn()
        .execute("UPDATE posts SET state = 'hidden' WHERE id = ?1", [id])
        .expect("state");
    assert_eq!(vote(&mut grace, &[]).status, 404);
}

#[test]
fn hiding_and_the_hidden_page() {
    let feed = TestFeed::start(Options::default());
    let mut ada = member(&feed, "ada");
    let mut grace = member(&feed, "grace");
    let mut ids = Vec::new();
    for i in 0..32 {
        // 32 posts exceed ada's limit; insert by SQL, a post at a minute.
        let t = clock::storage(feed.clock_now() - Duration::minutes(100 - i));
        let conn = feed.conn();
        conn.execute(
            "INSERT INTO posts (kind, author_id, title, body, created_at)
                 VALUES ('discussion', (SELECT id FROM users WHERE login = 'ada'), ?1, 'b', ?2)",
            rusqlite::params![format!("Post {i:02}"), t],
        )
        .expect("post");
        ids.push(conn.last_insert_rowid());
    }
    let hide = |client: &mut TestClient, id: i64, unhide: &str, to: &str| {
        let id = id.to_string();
        client.post_form(
            "/hide",
            &[
                ("post_id", id.as_str()),
                ("unhide", unhide),
                ("return_to", to),
            ],
        )
    };
    // A missing or non-visible post cannot be hidden.
    assert_eq!(hide(&mut grace, 9999, "0", "/").status, 404);
    feed.conn()
        .execute("UPDATE posts SET state = 'removed' WHERE id = ?1", [ids[0]])
        .expect("state");
    assert_eq!(hide(&mut grace, ids[0], "0", "/").status, 404);
    feed.conn()
        .execute("UPDATE posts SET state = 'visible' WHERE id = ?1", [ids[0]])
        .expect("state");

    // Hide in order; twice changes nothing and keeps the first time.
    for id in &ids {
        let r = hide(&mut grace, *id, "0", "/new");
        assert_eq!(r.status, 303, "{}", r.body);
        assert_eq!(link_of(&r), "/new");
        feed.clock.advance(Duration::seconds(1));
    }
    let first: String = feed
        .conn()
        .query_row("SELECT at FROM hides WHERE post_id = ?1", [ids[0]], |r| {
            r.get(0)
        })
        .expect("at");
    hide(&mut grace, ids[0], "0", "/");
    assert_eq!(count(&feed, "SELECT COUNT(*) FROM hides"), 32);
    let again: String = feed
        .conn()
        .query_row("SELECT at FROM hides WHERE post_id = ?1", [ids[0]], |r| {
            r.get(0)
        })
        .expect("at");
    assert_eq!(first, again);
    // The default redirect is the post.
    let r = grace.post_form("/hide", &[("post_id", &ids[1].to_string())]);
    assert_eq!(link_of(&r), format!("/p/{}", ids[1]));

    // Gone from grace's lists, still on ada's and on a visitor's.
    for path in ["/", "/new"] {
        assert!(grace.get(path).body.contains("Nothing here yet."), "{path}");
        assert!(ada.get(path).body.contains("Post 31"), "{path}");
        assert!(feed.client().get(path).body.contains("Post 31"), "{path}");
    }
    // /hidden: most recently hidden first, 30 a page, numbered on.
    let page = grace.get("/hidden");
    assert_eq!(page.status, 200);
    assert!(page.body.contains("<h1>Hidden by you</h1>"));
    let hidden_titles: Vec<&str> = page.body.matches("hp-title-link").collect();
    assert_eq!(hidden_titles.len(), 30);
    let newest = page.body.find("Post 31").expect("newest hidden");
    let older = page.body.find("Post 30").expect("older hidden");
    assert!(newest < older);
    assert!(page.body.contains(r#"href="/hidden?page=2""#));
    assert!(page.body.contains(">un-hide<"));
    let page2 = grace.get("/hidden?page=2");
    assert!(page2.body.contains("Post 01") && page2.body.contains("Post 00"));
    assert!(!page2.body.contains("Post 31"));
    assert!(page2.body.contains(r#"start="31""#));
    assert!(!page2.body.contains("hp-more"));
    // Only visible posts are listed; un-hide still works on one that is not.
    feed.conn()
        .execute(
            "UPDATE posts SET state = 'removed' WHERE id = ?1",
            [ids[31]],
        )
        .expect("state");
    assert!(!grace.get("/hidden").body.contains("Post 31"));
    let r = hide(&mut grace, ids[31], "1", "/hidden");
    assert_eq!(link_of(&r), "/hidden");
    let left: i64 = feed
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM hides WHERE post_id = ?1",
            [ids[31]],
            |r| r.get(0),
        )
        .expect("count");
    assert_eq!(left, 0);
}

#[test]
fn a_restricted_member_can_hide_and_unhide_but_not_vote() {
    let feed = TestFeed::start(Options::default());
    let mut ada = member(&feed, "ada");
    let mut grace = member(&feed, "grace");
    let id = id_of(&submit(&mut ada, "A post", "", "b"));
    // A restriction is a moderator's action; set with SQL.
    feed.conn()
        .execute(
            "UPDATE users SET restricted_until = '9999-12-31T23:59:59.999999Z' WHERE login = 'grace'",
            [],
        )
        .expect("restrict");
    let ids = id.to_string();
    assert_eq!(grace.post_form("/hide", &[("post_id", &ids)]).status, 303);
    assert_eq!(count(&feed, "SELECT COUNT(*) FROM hides"), 1);
    assert_eq!(
        grace
            .post_form("/vote", &[("target_type", "post"), ("target_id", &ids)])
            .status,
        403
    );
    assert_eq!(
        grace
            .post_form("/hide", &[("post_id", &ids), ("unhide", "1")])
            .status,
        303
    );
    assert_eq!(count(&feed, "SELECT COUNT(*) FROM hides"), 0);
    // A hiding changes nothing for anyone else (the API ignores it).
    assert!(ada.get("/new").body.contains("A post"));
}

#[test]
fn ask_show_and_past() {
    let feed = TestFeed::start(Options::default());
    let mut ada = member(&feed, "ada");
    let mut grace = member(&feed, "grace");
    let ask = id_of(&submit(&mut ada, "Ask hippo: why?", "", "text"));
    let show = id_of(&submit(&mut grace, "show HIPPO: a demo", "", "text"));
    let plain = id_of(&submit(&mut grace, "Plain", "", "text"));

    let page = ada.get("/ask");
    assert!(page.body.contains("Ask hippo: why?"));
    assert!(!page.body.contains("a demo") && !page.body.contains("Plain"));
    assert!(page.body.contains(r#"<h1 class="hp-sr">Ask</h1>"#));
    assert!(page
        .body
        .contains(r#"<a class="hp-nav-current" href="/ask" aria-current="page">ask</a>"#));
    let page = ada.get("/show");
    assert!(page.body.contains("a demo"));
    assert!(!page.body.contains("Ask hippo") && !page.body.contains("Plain"));
    assert!(page
        .body
        .contains(r#"<a class="hp-nav-current" href="/show" aria-current="page">show</a>"#));
    assert!(!ada.get("/").body.contains("hp-nav-current"));
    // An edit moves a post between lists: it follows the current title.
    feed.conn()
        .execute(
            "UPDATE posts SET title = 'Show hippo: now' WHERE id = ?1",
            [plain],
        )
        .expect("title");
    assert!(ada.get("/show").body.contains("Show hippo: now"));

    // /past defaults to yesterday (2026-11-07).
    let yesterday = clock::storage(feed.clock_now() - Duration::days(1));
    feed.conn()
        .execute(
            "UPDATE posts SET created_at = ?1 WHERE id = ?2",
            [yesterday, ask.to_string()],
        )
        .expect("age");
    let day_before = clock::storage(feed.clock_now() - Duration::days(3));
    feed.conn()
        .execute(
            "UPDATE posts SET created_at = ?1 WHERE id = ?2",
            [day_before, show.to_string()],
        )
        .expect("age");
    let page = ada.get("/past");
    assert_eq!(page.status, 200);
    assert!(page
        .body
        .contains("<title>Past: 2026-11-07 | hippo</title>"));
    assert!(page.body.contains("Ask hippo: why?"));
    assert!(!page.body.contains("a demo"));
    assert!(
        page.body.contains(r#"href="/past?day=2026-11-06""#),
        "the day before"
    );
    assert!(
        !page.body.contains("Day after"),
        "yesterday has no day after"
    );
    assert!(page
        .body
        .contains(r#"<a class="hp-nav-current" href="/past" aria-current="page">past</a>"#));
    // An earlier day links both ways.
    let page = ada.get("/past?day=2026-11-05");
    assert!(page.body.contains("a demo"));
    assert!(page.body.contains(r#"href="/past?day=2026-11-04""#));
    assert!(page.body.contains(r#"href="/past?day=2026-11-06""#));
    // Malformed is 400; the future lists nothing.
    for bad in ["day=2026-13-01", "day=x", "day=2026-1-1", "day="] {
        assert_eq!(ada.get(&format!("/past?{bad}")).status, 400, "{bad}");
    }
    let page = ada.get("/past?day=2030-01-01");
    assert_eq!(page.status, 200);
    assert!(page.body.contains("Nothing here yet."));
}

#[test]
fn page_numbers_continue_and_bad_pages_are_404() {
    let feed = TestFeed::start(Options::default());
    let _ada = member(&feed, "ada");
    for i in 0..31 {
        let t = clock::storage(feed.clock_now() - Duration::minutes(i + 1));
        feed.conn()
            .execute(
                "INSERT INTO posts (kind, author_id, title, body, created_at)
                 VALUES ('discussion', 1, ?1, 'b', ?2)",
                rusqlite::params![format!("Entry {i:02}"), t],
            )
            .expect("post");
    }
    let mut visitor = feed.client();
    for path in ["/", "/new"] {
        let one = visitor.get(path);
        assert!(one
            .body
            .contains(&format!(r#"href="{path}?page=2""#).replace("//", "/")));
        let two = visitor.get(&format!("{path}?page=2"));
        assert!(two.body.contains(r#"start="31""#), "{path}");
        assert!(two.body.contains(r#"<span class="hp-rank">31.</span>"#));
        assert!(!two.body.contains("hp-more"));
        for bad in ["0", "21", "x", "-1", ""] {
            assert_eq!(
                visitor.get(&format!("{path}?page={bad}")).status,
                404,
                "{bad}"
            );
        }
    }
}

#[test]
fn per_member_limits_and_the_content_rule_over_http() {
    let feed = TestFeed::start(Options::default());
    // A new member: no link URL, no hyperlink, no http:// in any letter case.
    let mut newbie = member(&feed, "newbie");
    for (link, body, rule) in [
        (
            "https://example.com/",
            "",
            "new members cannot post links yet",
        ),
        ("", "see HTTPS://example.com", "cannot write http"),
        (
            "",
            "mail <a@example.com>",
            "new members cannot include links yet",
        ),
    ] {
        let r = submit(&mut newbie, "Title", link, body);
        assert_eq!(r.status, 422, "{link} {body}");
        assert!(r.body.contains(rule), "{rule}: {}", r.body);
        assert!(r.body.contains(r#"name="body""#));
    }
    let r = submit(&mut newbie, "see http://x.example", "", "b");
    assert_eq!(r.status, 422, "a title is checked too");
    assert_eq!(count(&feed, "SELECT COUNT(*) FROM posts"), 0);
    // One post a day; the second answers 429 and says when it resets.
    assert_eq!(submit(&mut newbie, "First", "", "plain text").status, 303);
    let r = submit(&mut newbie, "Second", "", "plain text");
    assert_eq!(r.status, 429);
    assert!(r.header("retry-after").is_some());
    assert!(r.body.contains("discussion posts"), "{}", r.body);
    assert_eq!(count(&feed, "SELECT COUNT(*) FROM posts"), 1);
    feed.clock
        .advance(Duration::hours(24) + Duration::seconds(1));
    assert_eq!(submit(&mut newbie, "Second", "", "plain text").status, 303);

    // An established member has no content rule but 5 posts a day.
    let mut ada = member(&feed, "ada");
    for i in 0..5 {
        let r = submit(
            &mut ada,
            &format!("Linked {i}"),
            "https://example.com/",
            "see https://x.example",
        );
        assert_eq!(r.status, 303, "{}", r.body);
    }
    let r = submit(&mut ada, "Sixth", "", "text");
    assert_eq!(r.status, 429);
    // Votes are limited too: 300 an hour.
    let mut grace = member(&feed, "grace");
    let target = id_of(&submit(&mut grace, "Target", "", "text")).to_string();
    let uid: i64 = feed
        .conn()
        .query_row("SELECT id FROM users WHERE login = 'ada'", [], |r| r.get(0))
        .expect("id");
    let now = clock::storage(feed.clock_now());
    {
        let conn = feed.conn();
        for n in 0..300 {
            conn.execute(
                "INSERT INTO votes (user_id, target_type, target_id, at) VALUES (?1, 'post', ?2, ?3)",
                rusqlite::params![uid, 10_000 + n, now],
            )
            .expect("vote");
        }
    }
    let r = ada.post_form("/vote", &[("target_type", "post"), ("target_id", &target)]);
    assert_eq!(r.status, 429);
}

#[test]
fn responses_follow_the_rules() {
    let feed = TestFeed::start(Options::default());
    let mut ada = member(&feed, "ada");
    let mut visitor = feed.client();

    // No session, a bad token: 403.
    for path in ["/submit", "/vote", "/hide"] {
        assert_eq!(
            visitor
                .post_form_with(path, &[("csrf", "x")], &[], false)
                .status,
            403,
            "{path}"
        );
        assert_eq!(
            ada.post_form_with(path, &[("csrf", "wrong")], &[], false)
                .status,
            403,
            "{path}"
        );
    }
    // A form that fails validation is shown again, 422, with what was entered.
    let r = submit(&mut ada, "", "https://example.com/", "text I typed");
    assert_eq!(r.status, 422);
    assert!(r.body.contains("title"), "names the rule: {}", r.body);
    assert!(r.body.contains("text I typed"));
    assert!(r.body.contains(r#"value="https://example.com/""#));
    let r = submit(&mut ada, "Title", "", "");
    assert_eq!(r.status, 422);
    assert!(r.body.contains("give a link, a text, or both"));
    // /mod and /admin do not advertise themselves.
    for path in ["/mod", "/admin", "/mod/u/ada"] {
        assert_eq!(visitor.get(path).status, 404, "{path}");
        assert_eq!(ada.get(path).status, 404, "{path}");
    }
    // The submit redirect is the new post; a post's own forms redirect to it.
    let id = id_of(&submit(&mut ada, "Redirected", "", "text"));
    assert_eq!(
        link_of(&ada.post_form(
            &format!("/p/{id}/edit"),
            &[("title", "Again"), ("body", "t")]
        )),
        format!("/p/{id}")
    );
    // GET of a member page by a visitor: sign in and back.
    let r = visitor.get("/hidden");
    assert_eq!(r.status, 303);
    assert!(link_of(&r).contains("return_to=%2Fhidden"));
}

#[test]
fn every_page_has_the_csp_one_h1_the_landmarks_and_no_script() {
    let feed = TestFeed::start(Options::default());
    let mut ada = member(&feed, "ada");
    let id = id_of(&submit(&mut ada, "A page", "", "body"));
    let mut visitor = feed.client();
    let paths = [
        "/".to_string(),
        "/new".into(),
        "/past".into(),
        "/ask".into(),
        "/show".into(),
        "/packs".into(),
        "/evidence".into(),
        "/themes".into(),
        "/about".into(),
        "/hidden".into(),
        "/submit".into(),
        format!("/p/{id}"),
        format!("/p/{id}/edit"),
        "/nowhere".into(),
        "/past?day=bad".into(),
    ];
    for client in [&mut visitor, &mut ada] {
        for path in &paths {
            let r = client.get(path);
            let csp = r.header("content-security-policy").unwrap_or("");
            assert!(csp.contains("script-src 'none'"), "{path}");
            if r.is_html() {
                assert_eq!(r.body.matches("<h1").count(), 1, "{path}");
                for needle in ["<header", "<nav", "<main", "<footer", "hp-skip"] {
                    assert!(r.body.contains(needle), "{path}: {needle}");
                }
                assert!(!r.body.contains("<script"));
            }
        }
    }
}

#[test]
fn a_server_started_on_a_backup_serves_the_same_front_page() {
    let feed = TestFeed::start(Options::default());
    let mut ada = member(&feed, "ada");
    let mut grace = member(&feed, "grace");
    let id = id_of(&submit(
        &mut ada,
        "Kept through a backup",
        "https://example.com/x",
        "b",
    ));
    grace.post_form(
        "/vote",
        &[("target_type", "post"), ("target_id", &id.to_string())],
    );
    let before = feed.client().get("/").body;
    let dir = tempfile::tempdir().expect("tempdir");
    let info = hippo_feed::backup::backup(&feed.db_path, dir.path(), 14, feed.clock_now())
        .expect("backup");
    let restored = TestFeed::start(Options {
        database: Some(info.path.clone()),
        ..Options::default()
    });
    let after = restored.client().get("/").body;
    assert!(before.contains("Kept through a backup"));
    assert_eq!(before, after);
}

// --- Pack pages and rows with evidence ---

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/feed")
}

#[test]
fn pack_rows_and_pages_show_evidence_and_measurements() {
    let raw = RawServer::start_default();
    let dir = tempfile::tempdir().expect("tempdir");
    let index = dir.path().join("index.json");
    std::fs::copy(fixtures().join("registry-index.json"), &index).expect("copy");
    let feed = TestFeed::start(Options {
        registry_index: Some(index.display().to_string()),
        raw_url: Some(raw.url.clone()),
        eval_sets: Some("aaaaaaaaaaaa:6:10".into()),
        ..Options::default()
    });
    feed.sync();
    let mut ada = member(&feed, "ada"); // maintainer of glossary-terms
    let mut grace = member(&feed, "grace");
    let _linus = member(&feed, "linus");
    let _alan = member(&feed, "alan");
    let _newbie = member(&feed, "newbie");

    let post = |name: &str| -> i64 {
        feed.conn()
            .query_row("SELECT id FROM posts WHERE pack = ?1", [name], |r| r.get(0))
            .expect("post")
    };
    let hash = |name: &str| -> String {
        feed.conn()
            .query_row(
                "SELECT pack_hash FROM pack_contents WHERE pack = ?1",
                [name],
                |r| r.get(0),
            )
            .expect("hash")
    };
    // Measurements by SQL: the entry form is not used here. A benefit
    // of +0.2 is (12 - 6) * 10 - (7 - 3) * 6 = 36 over 180.
    let measure = |pack: &str, login: &str, invalid: bool| -> i64 {
        let conn = feed.conn();
        conn.execute(
            "INSERT INTO measurements (pack, pack_hash, user_id, eval_set, planted, kept, runs,
                corrected_without, damaged_without, corrected_with, damaged_with, hippo, model,
                report_url, created_at, updated_at, invalid)
             VALUES (?1, ?2, (SELECT id FROM users WHERE login = ?3), 'aaaaaaaaaaaa', 6, 10, 3,
                6, 3, 12, 7, '1.0.0', 'model-x', 'https://example.com/report', ?4, ?4, ?5)",
            rusqlite::params![pack, hash(pack), login, OLD, invalid],
        )
        .expect("measurement");
        conn.last_insert_rowid()
    };
    // Before any measurement: the count of the current content.
    let list = grace.get("/packs").body;
    assert!(list.contains("0 of 3 measurements"), "{list}");
    let m_grace = measure("glossary-terms", "grace", false);
    let m_linus = measure("glossary-terms", "linus", false);
    let list = grace.get("/packs").body;
    assert!(list.contains("2 of 3 measurements"));
    measure("glossary-terms", "alan", false);
    let m_ada = measure("glossary-terms", "ada", false); // the maintainer's
    let m_new = measure("glossary-terms", "newbie", false); // a new member's
    measure("decision-log", "grace", false);

    // 0.2 * 3/5 = 0.12; the maintainer's and the new member's do not count.
    let list = grace.get("/").body;
    assert!(
        list.contains(r#"<span class="hp-evidence">+0.12 evidence (3)</span>"#),
        "{list}"
    );
    assert!(list.contains("1 of 3 measurements"), "decision-log");
    let page = grace.get(&format!("/p/{}", post("glossary-terms")));
    assert_eq!(page.status, 200);
    assert!(page.body.contains("+0.120 (3)"), "{}", page.body);
    assert!(
        !page.body.contains("evidence from"),
        "the content is current"
    );
    for (id, status) in [
        (m_grace, "counted"),
        (m_linus, "counted"),
        (m_ada, "maintainer's"),
        (m_new, "not counted"),
    ] {
        let at = page
            .body
            .find(&format!(r#"<tr id="m{id}">"#))
            .expect("anchor");
        let row = &page.body[at..at + page.body[at..].find("</tr>").unwrap()];
        assert!(
            row.contains(&format!("<td>{status}</td>")) || row.contains(status),
            "{id} {status}: {row}"
        );
        assert!(
            row.contains("+0.233") && row.contains("+0.433") && row.contains("+0.200"),
            "{row}"
        );
        assert!(row.contains(r#"href="https://example.com/report""#));
    }
    // /evidence puts glossary-terms (above zero) first.
    let list = ada.get("/evidence").body;
    let first = list.find("glossary-terms").expect("glossary");
    let other = list.find("review-dates").expect("review-dates");
    assert!(first < other);

    // A newer content with no measurements: the page labels the old evidence
    // (the content is recorded by SQL, as a registry sync of a new version would).
    feed.conn()
        .execute(
            "INSERT INTO pack_contents (pack, pack_hash, version, commit_id, first_seen)
             VALUES ('glossary-terms', ?1, '2.0.0', 'c', ?2)",
            rusqlite::params!["9".repeat(64), clock::storage(feed.clock_now())],
        )
        .expect("content");
    feed.conn()
        .execute(
            "UPDATE packs SET pack_hash = ?1, version = '2.0.0' WHERE name = 'glossary-terms'",
            ["9".repeat(64)],
        )
        .expect("pack");
    let page = grace.get(&format!("/p/{}", post("glossary-terms")));
    assert!(page.body.contains("evidence from 1.0.0"), "{}", page.body);
    assert!(page.body.contains("0 of 3 measurements"));
    // With measuring closed a pack without evidence has no evidence part.
}

#[test]
fn a_pack_without_evidence_shows_no_part_while_measuring_is_closed() {
    let raw = RawServer::start_default();
    let dir = tempfile::tempdir().expect("tempdir");
    let index = dir.path().join("index.json");
    std::fs::copy(fixtures().join("registry-index.json"), &index).expect("copy");
    let feed = TestFeed::start(Options {
        registry_index: Some(index.display().to_string()),
        raw_url: Some(raw.url.clone()),
        ..Options::default()
    });
    feed.sync();
    let list = feed.client().get("/packs").body;
    assert!(list.contains("glossary-terms"));
    assert!(!list.contains("measurements</span>") && !list.contains("evidence ("));
}
