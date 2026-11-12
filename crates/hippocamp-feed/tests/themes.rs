//! Custom CSS reaches only the member who chose it: linking,
//! serving and saving.

mod common;

use common::{Options, TestClient, TestFeed};
use sha2::{Digest, Sha256};

const CSS: &str = "body { color: #123456; }";

fn hex(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn id_of(feed: &TestFeed, github_id: i64) -> i64 {
    feed.conn()
        .query_row(
            "SELECT id FROM users WHERE github_id = ?1",
            [github_id],
            |r| r.get(0),
        )
        .expect("id")
}

fn signed_in(feed: &TestFeed, login: &str) -> TestClient {
    let mut client = feed.client();
    client.sign_in(login);
    client.get("/settings");
    client
}

/// Whether the page links personal CSS of any member.
fn links_personal_css(body: &str) -> bool {
    body.contains("/theme.css")
}

#[test]
fn custom_css_is_linked_only_into_the_pages_of_the_member_who_chose_it() {
    let feed = TestFeed::start(Options::default());
    let mut a = signed_in(&feed, "ada");
    let mut b = signed_in(&feed, "grace");
    let mut visitor = feed.client();
    let saved = a.post_form(
        "/settings/theme",
        &[("theme", "dark"), ("personal_css", CSS)],
    );
    assert_eq!(saved.status, 303);
    assert_eq!(saved.header("location"), Some("/settings"));
    let a_id = id_of(&feed, 1001);
    let own = format!("/u/{a_id}/theme.css");

    // A's ordinary page: base, dark, personal CSS, in that order.
    let page = a.get("/about").body;
    let base = page.find("/static/base.css?v=").expect("base");
    let dark = page.find("/static/themes/dark.css?v=").expect("dark");
    let mine = page.find(&own).expect("personal css");
    assert!(base < dark && dark < mine, "{page}");
    assert!(!page.contains("auto.css"));

    // B and a visitor get neither.
    for page in [b.get("/about").body, visitor.get("/about").body] {
        assert!(!links_personal_css(&page), "{page}");
        assert!(!page.contains("dark.css"));
        assert!(page.contains("/static/themes/auto.css?v="));
    }

    // Protected pages link A's built-in theme and no personal CSS: settings, the
    // sign-in page (reached through a failed callback, since a member who is
    // signed in never sees /login itself) and an error page.
    for path in [
        "/settings",
        "/auth/github/callback?error=x",
        "/no-such-page",
    ] {
        let page = a.get(path).body;
        assert!(page.contains("/static/themes/dark.css?v="), "{path}");
        assert!(!links_personal_css(&page), "{path}");
    }
    // ?safe=1 links no personal CSS either.
    let page = a.get("/about?safe=1").body;
    assert!(page.contains("/static/themes/dark.css?v="));
    assert!(!links_personal_css(&page));
}

#[test]
fn a_members_personal_css_is_served_to_that_member_only() {
    let feed = TestFeed::start(Options::default());
    let mut a = signed_in(&feed, "ada");
    let mut b = signed_in(&feed, "grace");
    let mut visitor = feed.client();
    a.post_form(
        "/settings/theme",
        &[("theme", "auto"), ("personal_css", CSS)],
    );
    let a_id = id_of(&feed, 1001);
    let path = format!("/u/{a_id}/theme.css");

    let reply = a.get(&path);
    assert_eq!(reply.status, 200);
    assert_eq!(reply.body, CSS);
    assert_eq!(
        reply.header("content-type"),
        Some("text/css; charset=utf-8")
    );
    assert_eq!(reply.header("cache-control"), Some("private, no-cache"));
    let etag = format!("\"{}\"", hex(CSS));
    assert_eq!(reply.header("etag"), Some(etag.as_str()));
    assert!(reply.header("content-security-policy").is_some());

    let again = a.get_with(&path, &[("If-None-Match", etag.as_str())]);
    assert_eq!(again.status, 304);
    assert!(again.body.is_empty());
    assert_eq!(again.header("etag"), Some(etag.as_str()));
    assert_eq!(again.header("cache-control"), Some("private, no-cache"));
    let other = a.get_with(&path, &[("If-None-Match", "\"stale\"")]);
    assert_eq!(other.status, 200);

    assert_eq!(b.get(&path).status, 404);
    assert_eq!(visitor.get(&path).status, 404);
    assert_eq!(a.get("/u/not-a-number/theme.css").status, 404);
    // B has none of their own either.
    let b_id = id_of(&feed, 1002);
    assert_eq!(b.get(&format!("/u/{b_id}/theme.css")).status, 404);

    // After A empties the box, nothing is served and nothing is linked.
    let cleared = a.post_form(
        "/settings/theme",
        &[("theme", "auto"), ("personal_css", "  \r\n ")],
    );
    assert_eq!(cleared.status, 303);
    assert_eq!(a.get(&path).status, 404);
    assert!(!links_personal_css(&a.get("/about").body));
    let stored: String = feed
        .conn()
        .query_row(
            "SELECT personal_css FROM users WHERE id = ?1",
            [a_id],
            |r| r.get(0),
        )
        .expect("css");
    assert_eq!(stored, "");
}

#[test]
fn personal_css_that_breaks_a_rule_is_refused_with_the_text_kept() {
    let feed = TestFeed::start(Options::default());
    let mut a = signed_in(&feed, "ada");

    let big = "a".repeat(65_537);
    let r = a.post_form(
        "/settings/theme",
        &[("theme", "dark"), ("personal_css", big.as_str())],
    );
    assert_eq!(r.status, 422);
    assert!(
        r.body
            .contains("Personal CSS is 65537 bytes; the limit is 65,536."),
        "size"
    );
    assert!(r.body.contains(&big), "the entered text is kept");
    // The same size is fine after line endings are normalised away.
    let crlf = format!("{}\r\n", "a".repeat(65_536));
    let r = a.post_form(
        "/settings/theme",
        &[("theme", "dark"), ("personal_css", crlf.as_str())],
    );
    assert_eq!(r.status, 303);

    for (what, bad) in [
        ("NUL", "p{}\n\u{0}q{}"),
        ("DEL", "p{}\n\u{7f}q{}"),
        ("C1", "p{}\n\u{85}q{}"),
    ] {
        let r = a.post_form(
            "/settings/theme",
            &[("theme", "dark"), ("personal_css", bad)],
        );
        assert_eq!(r.status, 422, "{what}");
        assert!(
            r.body.contains("Personal CSS, line 2: control character"),
            "{what}: {}",
            r.body
        );
        assert!(r.body.contains("p{}"), "{what}: text kept");
        assert!(r.body.contains(r#"class="hp-error""#));
    }
    // A refusal saved nothing: the 64 KiB of `a` from the good save is still there.
    let stored: i64 = feed
        .conn()
        .query_row(
            "SELECT length(personal_css) FROM users WHERE github_id = 1001",
            [],
            |r| r.get(0),
        )
        .expect("css");
    assert_eq!(stored, 65_536_i64);
}

#[test]
fn a_use_button_changes_the_theme_and_leaves_the_css() {
    let feed = TestFeed::start(Options::default());
    let mut a = signed_in(&feed, "ada");
    a.post_form(
        "/settings/theme",
        &[("theme", "dark"), ("personal_css", CSS)],
    );

    let r = a.post_form(
        "/settings/theme",
        &[("theme", "default"), ("return_to", "/p/7")],
    );
    assert_eq!(r.status, 303);
    assert_eq!(r.header("location"), Some("/p/7"));
    let (theme, css): (String, String) = feed
        .conn()
        .query_row(
            "SELECT theme, personal_css FROM users WHERE github_id = 1001",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("row");
    assert_eq!((theme.as_str(), css.as_str()), ("default", CSS));
    // `default` links no theme stylesheet, but still the personal CSS.
    let page = a.get("/about").body;
    assert!(!page.contains("/static/themes/"));
    assert!(links_personal_css(&page));

    let r = a.post_form(
        "/settings/theme",
        &[("theme", "dark"), ("return_to", "//evil.example")],
    );
    assert_eq!(r.header("location"), Some("/settings"));
    let r = a.post_form("/settings/theme", &[("theme", "nonsense")]);
    assert_eq!(r.status, 422);
}

fn insert_theme_pack(feed: &TestFeed, name: &str, hash: &str, listed: bool, post_state: &str) {
    let conn = feed.conn();
    let at = "2026-11-08T09:00:00.000000Z";
    conn.execute(
        "INSERT INTO packs (name, kind, version, description, authors, owner, owner_id, maintainers,
             first_party, license, repo, repo_id, path, commit_id, pack_hash, passes, theme_contract,
             theme_sha256, review_id, review_at, listed, synced_at)
         VALUES (?1, 'theme', '1.0.0', 'd', '[]', 'alice', 7, '[]', 0, 'MIT', 'alice/themes', 8, 'p',
             'c', ?2, '[]', 1, 'x', 'r', ?3, ?4, ?3)",
        rusqlite::params![name, hash, at, listed],
    )
    .expect("pack");
    conn.execute(
        "INSERT INTO posts (kind, pack, title, created_at, state) VALUES ('pack', ?1, 'A theme', ?2, ?3)",
        rusqlite::params![name, at, post_state],
    )
    .expect("post");
    conn.execute(
        "INSERT INTO pack_contents (pack, pack_hash, version, commit_id, first_seen)
         VALUES (?1, ?2, '1.0.0', 'c', ?3)",
        rusqlite::params![name, hash, at],
    )
    .expect("content");
    conn.execute(
        "INSERT INTO theme_files (pack, pack_hash, css, at) VALUES (?1, ?2, ':root{}', ?3)",
        rusqlite::params![name, hash, at],
    )
    .expect("file");
}

#[test]
fn a_theme_pack_is_chosen_only_while_the_feed_serves_it() {
    let feed = TestFeed::start(Options::default());
    let mut a = signed_in(&feed, "ada");

    // Nothing is served yet: a pack is refused with 422 and the message on /settings.
    let r = a.post_form("/settings/theme", &[("theme", "pack:solar")]);
    assert_eq!(r.status, 422);
    assert!(
        r.body.contains("That theme is not available."),
        "{}",
        r.body
    );
    assert!(r.body.contains("<h1>Settings</h1>"));

    // Served: it is offered, chosen, and linked on ordinary pages only.
    insert_theme_pack(&feed, "solar", "abc123", true, "visible");
    insert_theme_pack(&feed, "unlisted", "def456", false, "visible");
    insert_theme_pack(&feed, "removed", "0a0b0c", true, "removed");
    let settings = a.get("/settings").body;
    assert!(settings.contains(r#"value="pack:solar""#));
    assert!(!settings.contains("pack:unlisted") && !settings.contains("pack:removed"));
    assert_eq!(
        a.post_form("/settings/theme", &[("theme", "pack:solar")])
            .status,
        303
    );
    let page = a.get("/about").body;
    assert!(page.contains("/themes/solar/abc123.css"), "{page}");
    assert!(!page.contains("auto.css"));
    // Protected pages show `auto` for a member whose theme is a pack.
    let page = a.get("/settings").body;
    assert!(page.contains("/static/themes/auto.css?v="));
    assert!(!page.contains("/themes/solar"));
    assert!(!a.get("/about?safe=1").body.contains("/themes/solar"));

    // The pack stops being served: the member's choice is kept, shown as `auto`.
    feed.conn()
        .execute("UPDATE packs SET listed = 0 WHERE name = 'solar'", [])
        .expect("delist");
    let page = a.get("/about").body;
    assert!(!page.contains("/themes/solar"));
    assert!(page.contains("/static/themes/auto.css?v="));
    let settings = a.get("/settings").body;
    assert!(
        settings.contains(r#"value="pack:solar" selected"#),
        "{settings}"
    );
    let kept = a.post_form(
        "/settings/theme",
        &[("theme", "pack:solar"), ("personal_css", CSS)],
    );
    assert_eq!(kept.status, 303);
    // Another member cannot choose it now.
    let mut b = signed_in(&feed, "grace");
    assert_eq!(
        b.post_form("/settings/theme", &[("theme", "pack:solar")])
            .status,
        422
    );
}

#[test]
fn the_settings_page_holds_its_controls() {
    let feed = TestFeed::start(Options::default());
    let mut a = signed_in(&feed, "ada");
    let page = a.get("/settings").body;
    assert!(page.contains(
        "Personal CSS changes how this site looks to you. Paste only CSS you understand."
    ));
    for needle in [
        r#"href="/hidden""#,
        r#"href="/settings/export""#,
        r#"action="/settings/theme""#,
        r#"action="/settings/delete""#,
        r#"for="confirm""#,
        r#"for="personal_css""#,
        r#"for="theme""#,
        r#"value="auto""#,
        r#"value="default""#,
        r#"value="dark""#,
    ] {
        assert!(page.contains(needle), "{needle}");
    }
    assert!(page.contains("<h1>Settings</h1>"));
    assert!(page.contains("hp-page-settings"));
}
