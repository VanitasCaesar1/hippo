//! Sign-in through the mock provider, sessions, CSRF and the sign-in limit.

mod common;

use std::time::Duration;

use common::{Options, TestClient, TestFeed};
use hippo_feed::auth::mock::MockUser;
use hippo_feed::clock;
use hippo_feed::policy::AddressQuota;
use hippo_feed::web::Quotas;
use time::macros::datetime;

fn user_row(feed: &TestFeed, github_id: i64) -> (i64, String, Option<String>) {
    feed.conn()
        .query_row(
            "SELECT id, login, deleted_at FROM users WHERE github_id = ?1",
            [github_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .expect("user row")
}

fn count(feed: &TestFeed, sql: &str) -> i64 {
    feed.conn().query_row(sql, [], |r| r.get(0)).expect("count")
}

/// The file's bytes, empty when it does not exist.
fn bytes(path: &std::path::Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_default()
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|w| w == needle.as_bytes())
}

#[test]
fn a_full_sign_in_revokes_the_token_and_keeps_no_token_in_the_database() {
    let feed = TestFeed::start(Options::default());
    let mut client = feed.client();

    // Start: a redirect to the provider with state and PKCE, and the state cookie.
    let start = client.login_start("/");
    assert_eq!(start.status, 303);
    let authorize = start.header("location").expect("location").to_string();
    assert!(authorize.starts_with(&format!("{}/login/oauth/authorize?", feed.mock_url)));
    for part in [
        "client_id=hippo-feed-dev",
        "code_challenge_method=S256",
        "allow_signup=true",
        "code_challenge=",
        "state=",
        "redirect_uri=http%3A%2F%2F127.0.0.1",
    ] {
        assert!(authorize.contains(part), "{part} in {authorize}");
    }
    assert!(!authorize.contains("scope"), "{authorize}");
    let cookies = start.set_cookies();
    let oauth = cookies
        .iter()
        .find(|c| c.starts_with("hippo_oauth="))
        .expect("state cookie");
    for part in ["HttpOnly", "SameSite=Lax", "Path=/auth", "Max-Age=600"] {
        assert!(oauth.contains(part), "{part} in {oauth}");
    }
    assert!(!oauth.contains("Secure"), "http base URL: {oauth}");
    assert_eq!(count(&feed, "SELECT COUNT(*) FROM oauth_states"), 1);

    let callback = client.mock_approve("ada", &authorize);
    let reply = client.get(&callback);
    assert_eq!(reply.status, 303);
    assert_eq!(reply.header("location"), Some("/"));
    let cookies = reply.set_cookies();
    let session = cookies
        .iter()
        .find(|c| c.starts_with("hippo_session="))
        .expect("session cookie");
    for part in ["HttpOnly", "SameSite=Lax", "Path=/", "Max-Age=2592000"] {
        assert!(session.contains(part), "{part} in {session}");
    }
    assert!(!session.contains("Secure"));
    // The state works once and is gone; the state cookie is cleared.
    assert_eq!(count(&feed, "SELECT COUNT(*) FROM oauth_states"), 0);
    assert!(client.cookie("hippo_oauth").is_none());

    // The account.
    let (id, login, deleted) = user_row(&feed, 1001);
    assert_eq!((login.as_str(), deleted), ("ada", None));
    let created: String = feed
        .conn()
        .query_row(
            "SELECT github_created_at FROM users WHERE id = ?1",
            [id],
            |r| r.get(0),
        )
        .expect("created");
    assert_eq!(created, "2015-03-01T00:00:00.000000Z");

    // The top bar shows the member, karma and logout.
    let page = client.get("/about");
    assert!(
        page.body.contains(r#"<a href="/settings">ada</a>"#),
        "{}",
        page.body
    );
    assert!(page.body.contains("(0)"));
    assert!(page.body.contains(r#"action="/logout""#));
    assert!(!page.body.contains("/login?return_to"));

    // The mock saw the revocation of the one token it issued.
    let issued = feed.mock.issued();
    assert_eq!(issued.len(), 1);
    assert_eq!(feed.mock.revoked(), issued);

    // Neither the access token nor the session token is in the database or its WAL.
    let session_token = client.cookie("hippo_session").expect("token");
    let db = bytes(&feed.db_path);
    let mut wal_path = feed.db_path.clone().into_os_string();
    wal_path.push("-wal");
    let wal = bytes(std::path::Path::new(&wal_path));
    assert!(!wal.is_empty() || !db.is_empty());
    for token in [issued[0].as_str(), session_token.as_str()] {
        assert!(!contains(&db, token), "token in the database file");
        assert!(!contains(&wal, token), "token in the WAL file");
    }
    // The database keeps the session's hash.
    let hash = hippo_feed::auth::session::hash_token(&session_token);
    assert_eq!(
        count(
            &feed,
            &format!("SELECT COUNT(*) FROM sessions WHERE token_hash = '{hash}'")
        ),
        1
    );

    // Sign-out deletes the session and signs the browser out.
    let out = client.post_form("/logout", &[]);
    assert_eq!(out.status, 303);
    assert_eq!(out.header("location"), Some("/"));
    assert_eq!(count(&feed, "SELECT COUNT(*) FROM sessions"), 0);
    let page = client.get("/about");
    assert!(page.body.contains("/login?return_to"));
    assert!(!page.body.contains("/logout"));
}

#[test]
fn a_session_ends_when_it_expires_or_the_account_is_banned() {
    let feed = TestFeed::start(Options::default());
    let mut client = feed.client();
    client.sign_in("ada");
    assert!(client.get("/about").body.contains(r#"action="/logout""#));
    feed.clock
        .advance(time::Duration::days(30) - time::Duration::seconds(1));
    assert!(client.get("/about").body.contains(r#"action="/logout""#));
    feed.clock.advance(time::Duration::seconds(2));
    assert!(!client.get("/about").body.contains(r#"action="/logout""#));

    let mut client = feed.client();
    client.sign_in("grace");
    assert!(client.get("/about").body.contains(r#"action="/logout""#));
    feed.conn()
        .execute("UPDATE users SET banned = 1 WHERE github_id = 1002", [])
        .expect("ban");
    assert!(!client.get("/about").body.contains(r#"action="/logout""#));
}

fn assert_failed(reply: &common::Reply, status: u16) {
    assert_eq!(reply.status, status, "{}", reply.body);
    assert!(reply.body.contains("Sign-in failed"), "{}", reply.body);
    assert!(reply.body.contains("Try again"));
    assert!(reply
        .set_cookies()
        .iter()
        .all(|c| !c.starts_with("hippo_session=")));
}

/// Starts sign-in and approves as `login`; returns the callback path.
fn prepared(client: &mut TestClient, login: &str) -> String {
    let start = client.login_start("/");
    let authorize = start.header("location").expect("location").to_string();
    client.mock_approve(login, &authorize)
}

#[test]
fn sign_in_fails_for_a_bad_state_a_missing_cookie_an_old_state_and_a_bad_verifier() {
    let feed = TestFeed::start(Options::default());

    // A wrong state.
    let mut client = feed.client();
    let callback = prepared(&mut client, "ada");
    let wrong = callback.replace("state=", "state=x");
    assert_failed(&client.get(&wrong), 400);

    // A used state: the replay finds no row, even with the cookie put back.
    let mut client = feed.client();
    let callback = prepared(&mut client, "ada");
    let state = client.cookie("hippo_oauth").expect("cookie");
    assert_eq!(client.get(&callback).status, 303);
    client.set_cookie("hippo_oauth", &state);
    assert_failed(&client.get(&callback), 400);

    // A missing cookie.
    let mut client = feed.client();
    let callback = prepared(&mut client, "ada");
    client.remove_cookie("hippo_oauth");
    assert_failed(&client.get(&callback), 400);

    // A state older than 10 minutes; exactly 10 minutes is still good.
    let mut client = feed.client();
    let callback = prepared(&mut client, "ada");
    feed.clock
        .advance(time::Duration::minutes(10) + time::Duration::seconds(1));
    assert_failed(&client.get(&callback), 400);
    let mut client = feed.client();
    let callback = prepared(&mut client, "ada");
    feed.clock.advance(time::Duration::minutes(10));
    assert_eq!(client.get(&callback).status, 303);

    // A wrong verifier: the provider refuses the exchange.
    let mut client = feed.client();
    let callback = prepared(&mut client, "grace");
    feed.conn()
        .execute("UPDATE oauth_states SET verifier = 'not-the-verifier'", [])
        .expect("tamper");
    assert_failed(&client.get(&callback), 502);

    // No code, or GitHub's error parameter.
    let mut client = feed.client();
    assert_failed(&client.get("/auth/github/callback"), 400);
    let cancelled = client.get("/auth/github/callback?error=access_denied");
    assert_eq!(cancelled.status, 400);
    assert!(cancelled.body.contains("Sign-in was cancelled."));
}

#[test]
fn return_to_keeps_a_local_path_and_drops_anything_else() {
    let feed = TestFeed::start(Options::default());
    let mut client = feed.client();
    let reply = client.sign_in_to("ada", "/p/1");
    assert_eq!(reply.status, 303);
    assert_eq!(reply.header("location"), Some("/p/1"));

    let mut client = feed.client();
    let reply = client.sign_in_to("grace", "//evil.example");
    assert_eq!(reply.header("location"), Some("/"));

    // A member who is already signed in goes straight to return_to.
    let direct = client.login_start("/new");
    assert_eq!(direct.status, 303);
    assert_eq!(direct.header("location"), Some("/new"));
    let direct = client.login_start("https://evil.example/");
    assert_eq!(direct.header("location"), Some("/"));
}

#[test]
fn a_banned_id_is_refused_and_admitted_after_an_unban() {
    let feed = TestFeed::start(Options::default());
    feed.client().sign_in("ada");
    feed.conn()
        .execute("UPDATE users SET banned = 1 WHERE github_id = 1001", [])
        .expect("ban");
    // A ban deletes the account's sessions (item 31's code does it; here SQL).
    feed.conn()
        .execute("DELETE FROM sessions", [])
        .expect("sessions");

    let mut client = feed.client();
    let reply = client.sign_in("ada");
    assert_eq!(reply.status, 403, "{}", reply.body);
    assert!(reply.body.contains("This account cannot sign in."));
    assert!(client.cookie("hippo_session").is_none());
    assert_eq!(count(&feed, "SELECT COUNT(*) FROM sessions"), 0);

    // A banned account that was deleted is refused too.
    feed.conn()
        .execute(
            "UPDATE users SET deleted_at = '2026-11-08T10:00:00.000000Z' WHERE github_id = 1001",
            [],
        )
        .expect("delete");
    assert_eq!(feed.client().sign_in("ada").status, 403);

    feed.conn()
        .execute(
            "UPDATE users SET banned = 0, deleted_at = NULL WHERE github_id = 1001",
            [],
        )
        .expect("unban");
    let mut client = feed.client();
    assert_eq!(client.sign_in("ada").status, 303);
    assert!(client.cookie("hippo_session").is_some());
}

#[test]
fn a_restricted_account_that_was_deleted_signs_in_again_as_a_revived_account() {
    let feed = TestFeed::start(Options::default());
    let mut client = feed.client();
    client.sign_in("ada");
    let (id, _, _) = user_row(&feed, 1001);
    let until = clock::storage(datetime!(2026-12-08 10:00:00 UTC));
    feed.conn()
        .execute(
            "UPDATE users SET restricted_until = ?1, theme = 'dark' WHERE id = ?2",
            rusqlite::params![until, id],
        )
        .expect("restrict");

    // The member deletes the account; the id stays while the restriction lasts.
    client.get("/settings");
    let deleted = client.post_form("/settings/delete", &[("confirm", "ada")]);
    assert_eq!(deleted.status, 303, "{}", deleted.body);
    let (same, login, gone) = user_row(&feed, 1001);
    assert_eq!(same, id);
    assert_eq!(login, format!("~deleted-{id}"));
    assert!(gone.is_some());

    // Two days later the same GitHub id signs in: the row is revived as new.
    feed.clock.advance(time::Duration::days(2));
    let mut again = feed.client();
    assert_eq!(again.sign_in("ada").status, 303);
    let (revived, login, gone) = user_row(&feed, 1001);
    assert_eq!((revived, login.as_str(), gone), (id, "ada", None));
    let (created, restricted, theme): (String, String, String) = feed
        .conn()
        .query_row(
            "SELECT created_at, restricted_until, theme FROM users WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .expect("row");
    assert_eq!(created, clock::storage(feed.clock_now()));
    assert_eq!(restricted, until);
    assert_eq!(theme, "auto");

    // Standing: restricted while the restriction lasts; the feed age counts from the revival.
    let conn = feed.conn();
    let hash =
        hippo_feed::auth::session::hash_token(&again.cookie("hippo_session").expect("token"));
    let member = hippo_feed::auth::session::lookup(&conn, &hash, feed.clock_now())
        .expect("lookup")
        .expect("member");
    assert_eq!(
        hippo_feed::users::standing(&member, feed.clock_now()),
        hippo_feed::users::Standing::Restricted
    );
    let later = datetime!(2026-12-08 10:00:00 UTC);
    assert_eq!(
        hippo_feed::users::standing(&member, later),
        hippo_feed::users::Standing::Established,
        "28 days after the revival, with the restriction over"
    );
    let soon = feed.clock_now() + time::Duration::days(1);
    assert_eq!(
        hippo_feed::users::standing(&member, soon),
        hippo_feed::users::Standing::Restricted
    );
}

#[test]
fn a_login_held_by_an_earlier_account_is_set_aside_as_former() {
    let epoch = datetime!(2015-03-01 00:00:00 UTC);
    let options = Options {
        mock_users: Some(vec![
            MockUser::new(1001, "ada", epoch),
            MockUser::new(2000, "Ada", epoch),
        ]),
        ..Options::default()
    };
    let feed = TestFeed::start(options);
    feed.client().sign_in("ada");
    let (first, _, _) = user_row(&feed, 1001);

    // GitHub id 2000 now holds the login that account 1001 had.
    assert_eq!(feed.client().sign_in("Ada").status, 303);
    let (second, login, _) = user_row(&feed, 2000);
    assert_eq!(login, "Ada");
    assert_ne!(first, second);
    assert_eq!(user_row(&feed, 1001).1, format!("~former-{first}"));

    // When the first owner signs in again, the login moves back.
    assert_eq!(feed.client().sign_in("ada").status, 303);
    assert_eq!(user_row(&feed, 1001).1, "ada");
    assert_eq!(user_row(&feed, 2000).1, format!("~former-{second}"));
}

#[test]
fn a_post_needs_a_session_the_csrf_token_and_a_local_origin() {
    let feed = TestFeed::start(Options::default());

    // No session: 403, whatever the token.
    let mut visitor = feed.client();
    assert_eq!(
        visitor
            .post_form_with("/logout", &[("csrf", "x")], &[], false)
            .status,
        403
    );
    let denied = visitor.post_form_with("/settings/theme", &[("theme", "dark")], &[], false);
    assert_eq!(denied.status, 403);

    let mut client = feed.client();
    client.sign_in("ada");
    client.get("/about");
    let own = feed.base_url.clone();

    // No token, a wrong token and an empty one.
    for fields in [vec![], vec![("csrf", "wrong")], vec![("csrf", "")]] {
        let r = client.post_form_with("/logout", &fields, &[], false);
        assert_eq!(r.status, 403, "{fields:?}");
    }
    // A foreign Origin, `null` and a cross-site fetch are refused with a good token.
    for headers in [
        vec![("Origin", "http://evil.example")],
        vec![("Origin", "null")],
        vec![("Sec-Fetch-Site", "cross-site")],
    ] {
        let r = client.post_form_with("/logout", &[], &headers, true);
        assert_eq!(r.status, 403, "{headers:?}");
    }
    // Still signed in after every refusal.
    assert_eq!(count(&feed, "SELECT COUNT(*) FROM sessions"), 1);

    // The feed's own origin and a same-origin fetch pass.
    let ok = client.post_form_with(
        "/settings/theme",
        &[("theme", "dark")],
        &[("Origin", own.as_str()), ("Sec-Fetch-Site", "same-origin")],
        true,
    );
    assert_eq!(ok.status, 303, "{}", ok.body);
    let ok = client.post_form("/logout", &[]);
    assert_eq!(ok.status, 303);
}

#[test]
fn a_member_page_sends_a_visitor_to_sign_in_and_back() {
    let feed = TestFeed::start(Options::default());
    let mut visitor = feed.client();
    let r = visitor.get("/settings");
    assert_eq!(r.status, 303);
    assert_eq!(r.header("location"), Some("/login?return_to=%2Fsettings"));
    let r = visitor.get("/settings/export");
    assert_eq!(
        r.header("location"),
        Some("/login?return_to=%2Fsettings%2Fexport")
    );
    // The top bar's login link carries the current path.
    let page = visitor.get("/about?x=1");
    assert!(
        page.body
            .contains(r#"href="/login?return_to=%2Fabout%3Fx%3D1""#),
        "{}",
        page.body
    );
    assert!(page.body.contains(r#"rel="nofollow""#));
}

#[test]
fn sign_in_is_unavailable_without_credentials_and_limited_per_address() {
    let feed = TestFeed::start(Options {
        github: false,
        ..Options::default()
    });
    let reply = feed.client().get("/login");
    assert_eq!(reply.status, 503);
    assert!(reply
        .body
        .contains("Sign-in is not available on this server."));

    let generous = Options::default().quotas.all;
    let limited = TestFeed::start(Options {
        quotas: Quotas {
            sign_in: AddressQuota {
                count: 2,
                per: Duration::from_secs(600),
            },
            all: generous,
            api: generous,
        },
        ..Options::default()
    });
    let mut client = limited.client();
    assert_eq!(client.login_start("/").status, 303);
    assert_eq!(client.login_start("/").status, 303);
    let third = client.login_start("/");
    assert_eq!(third.status, 429);
    assert!(third.header("retry-after").is_some());
    assert!(
        third.body.contains("2 requests per 10 minutes"),
        "{}",
        third.body
    );
}
