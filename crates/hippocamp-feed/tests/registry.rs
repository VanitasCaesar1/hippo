//! The registry sync and the pages of packs. Every test syncs the fixture
//! from a file source with `RawServer` as `FEED_RAW_URL`. States that other
//! code creates (a removed post, a renamed member) are set with SQL,
//! and each such test says so.

mod common;

use std::path::{Path, PathBuf};

use common::{Answer, Options, RawServer, TestFeed};
use hippocamp_feed::db::UserId;
use serde_json::Value;

const NIGHT_PATH: &str =
    "/grace/hippocamp-themes/1111111111111111111111111111111111111111/themes/night-ink/theme.css";
const NIGHT_V2_PATH: &str =
    "/grace/hippocamp-themes/6666666666666666666666666666666666666666/themes/night-ink/theme.css";
const BAD_HASH_PATH: &str =
    "/grace/hippocamp-themes/4444444444444444444444444444444444444444/themes/bad-hash/theme.css";
const BAD_RULES_PATH: &str =
    "/grace/hippocamp-themes/5555555555555555555555555555555555555555/themes/bad-rules/theme.css";

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/feed")
}

struct Setup {
    feed: TestFeed,
    raw: RawServer,
    index: PathBuf,
    _dir: tempfile::TempDir,
}

fn setup_with(options: Options) -> Setup {
    let raw = RawServer::start_default();
    let dir = tempfile::tempdir().expect("tempdir");
    let index = dir.path().join("index.json");
    std::fs::copy(fixtures().join("registry-index.json"), &index).expect("copy");
    let feed = TestFeed::start(Options {
        registry_index: Some(index.display().to_string()),
        raw_url: Some(raw.url.clone()),
        ..options
    });
    Setup {
        feed,
        raw,
        index,
        _dir: dir,
    }
}

fn setup() -> Setup {
    setup_with(Options::default())
}

impl Setup {
    /// Changes the entries of the index file.
    fn edit(&self, f: impl FnOnce(&mut Vec<Value>)) {
        let text = std::fs::read_to_string(&self.index).expect("read");
        let mut json: Value = serde_json::from_str(&text).expect("json");
        f(json["packs"].as_array_mut().expect("packs"));
        std::fs::write(&self.index, json.to_string()).expect("write");
    }

    fn use_fixture(&self, name: &str) {
        std::fs::copy(fixtures().join(name), &self.index).expect("copy");
    }

    fn write_index(&self, text: &str) {
        std::fs::write(&self.index, text).expect("write");
    }

    fn count(&self, sql: &str) -> i64 {
        self.feed
            .conn()
            .query_row(sql, [], |r| r.get(0))
            .expect("count")
    }

    fn text(&self, sql: &str) -> String {
        self.feed
            .conn()
            .query_row(sql, [], |r| r.get(0))
            .expect("text")
    }

    fn post_id(&self, pack: &str) -> i64 {
        self.feed
            .conn()
            .query_row("SELECT id FROM posts WHERE pack = ?1", [pack], |r| r.get(0))
            .expect("post")
    }

    fn audit(&self, action: &str) -> i64 {
        self.count(&format!(
            "SELECT COUNT(*) FROM audit_log WHERE action = '{action}'"
        ))
    }

    fn packs_snapshot(&self) -> String {
        let conn = self.feed.conn();
        let mut stmt = conn
            .prepare("SELECT name, version, pack_hash, listed, synced_at FROM packs ORDER BY name")
            .expect("prepare");
        let rows: Vec<String> = stmt
            .query_map([], |r| {
                Ok(format!(
                    "{}|{}|{}|{}|{}",
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, bool>(3)?,
                    r.get::<_, String>(4)?
                ))
            })
            .expect("rows")
            .collect::<Result<_, _>>()
            .expect("collect");
        rows.join("\n")
    }
}

const PASS_PACKS: [&str; 5] = [
    "glossary-terms",
    "decision-log",
    "review-dates",
    "stale-links",
    "handoff-check",
];
const FAILING: [&str; 4] = ["copyleft-notes", "-oops", "short-hash", "twin"];

fn has_post(body: &str, id: i64) -> bool {
    body.contains(&format!(r#"href="/p/{id}""#))
}

// --- Listing ---

#[test]
fn the_lists_show_the_five_packs_of_passes_and_no_theme_pack() {
    let s = setup();
    let report = s.feed.sync();
    assert_eq!(
        report.line(),
        "listed 10, delisted 0, updated 0, skipped 5, unchanged 0, themes 1"
    );
    let mut client = s.feed.client();
    for path in ["/", "/new", "/packs", "/evidence"] {
        let body = client.get(path).body;
        for name in PASS_PACKS {
            assert!(has_post(&body, s.post_id(name)), "{path}: {name}");
        }
        for name in [
            "night-ink",
            "bad-hash",
            "bad-rules",
            "desk-only",
            "future-theme",
        ] {
            let theme = s.count(&format!("SELECT COUNT(*) FROM posts WHERE pack = '{name}'"));
            if theme == 1 {
                assert!(!has_post(&body, s.post_id(name)), "{path}: {name}");
            }
        }
        for name in FAILING {
            assert!(!body.contains(name), "{path}: {name}");
        }
    }
    // A pack of passes offers its install command and deep link.
    let id = s.post_id("glossary-terms");
    let page = client.get(&format!("/p/{id}")).body;
    let hash = s.text("SELECT pack_hash FROM packs WHERE name = 'glossary-terms'");
    assert!(page.contains(r#"id="install""#), "{page}");
    assert!(
        page.contains(&format!(
            "hippocamp add glossary-terms --registry {}",
            s.index.display()
        )),
        "{page}"
    );
    assert!(
        page.contains(&format!(
            "hippocamp://add?pack=glossary-terms&amp;version=1.0.0&amp;sha256={hash}"
        )),
        "{page}"
    );
    assert!(page.contains("Open in Hippocamp.app"));
    assert!(page.contains("github.com/ada/hippocamp-packs/tree/"));
    // The list rows link the install box, and every list is free of the
    // failing entries on /themes too.
    assert!(client
        .get("/packs")
        .body
        .contains(&format!("/p/{id}#install")));
    assert!(!client.get("/themes").body.contains("glossary-terms"));
}

#[test]
fn the_themes_page_shows_three_theme_packs_and_two_are_not_available() {
    let s = setup();
    s.feed.sync();
    let mut client = s.feed.client();
    let body = client.get("/themes").body;
    for name in ["night-ink", "bad-hash", "bad-rules"] {
        assert!(has_post(&body, s.post_id(name)), "{name}");
    }
    assert_eq!(
        body.matches("This theme is not available.").count(),
        2,
        "{body}"
    );
    assert!(!body.contains("desk-only") && !body.contains("future-theme"));
    for name in ["desk-only", "future-theme"] {
        assert_eq!(
            s.count(&format!("SELECT COUNT(*) FROM posts WHERE pack = '{name}'")),
            0
        );
        assert_eq!(
            s.count(&format!(
                "SELECT COUNT(*) FROM packs WHERE name = '{name}' AND listed = 1"
            )),
            1
        );
    }
    // No *use* for anyone signed out; a member gets it for night-ink only.
    assert!(!body.contains(">use<"));
    let mut ada = s.feed.client();
    ada.sign_in("ada");
    let body = ada.get("/themes").body;
    assert_eq!(body.matches(">use<").count(), 1, "{body}");
    // The refused ones' stylesheets answer 404; night-ink's is served.
    for name in ["bad-hash", "bad-rules"] {
        let hash = s.text(&format!(
            "SELECT pack_hash FROM packs WHERE name = '{name}'"
        ));
        assert_eq!(
            client.get(&format!("/themes/{name}/{hash}.css")).status,
            404
        );
        let page = client.get(&format!("/p/{}", s.post_id(name))).body;
        assert!(page.contains("This theme is not available."));
        assert!(!page.contains("Use this theme"));
    }
    let hash = s.text("SELECT pack_hash FROM packs WHERE name = 'night-ink'");
    let css = client.get(&format!("/themes/night-ink/{hash}.css"));
    assert_eq!(css.status, 200);
    assert_eq!(css.header("content-type"), Some("text/css; charset=utf-8"));
    assert_eq!(
        css.header("cache-control"),
        Some("public, max-age=31536000, immutable")
    );
    assert!(css.body.contains("--hp-link"));
    // Only the served hash, only as <hash>.css.
    assert_eq!(client.get(&format!("/themes/night-ink/{hash}")).status, 404);
    assert_eq!(
        client
            .get(&format!("/themes/night-ink/{}.css", "0".repeat(64)))
            .status,
        404
    );
    assert_eq!(client.get("/themes/nope/x.css").status, 404);
    // The page of a theme pack: the box without passes, install box or the
    // like, and the use button.
    let page = ada.get(&format!("/p/{}", s.post_id("night-ink"))).body;
    assert!(page.contains("Use this theme") && page.contains("hp-pack-box"));
    assert!(!page.contains("hp-install-box") && !page.contains("hp-passes"));
}

#[test]
fn a_delisted_packs_post_keeps_its_url_and_loses_its_install_links() {
    let s = setup();
    s.feed.sync();
    let id = s.post_id("glossary-terms");
    s.edit(|packs| packs.retain(|p| p["name"] != "glossary-terms"));
    let report = s.feed.sync();
    assert_eq!(report.delisted, 1);
    let mut client = s.feed.client();
    let page = client.get(&format!("/p/{id}"));
    assert_eq!(page.status, 200);
    assert!(!page.body.contains(r#"id="install""#) && !page.body.contains("hippocamp://"));
    assert!(page.body.contains("no longer listed"));
    for path in ["/", "/new", "/packs", "/evidence"] {
        assert!(!has_post(&client.get(path).body, id), "{path}");
    }
}

#[test]
fn lists_paginate_thirty_to_a_page_and_ranks_continue() {
    let s = setup();
    {
        let conn = s.feed.conn();
        for i in 0..65 {
            conn.execute(
                "INSERT INTO posts (kind, title, created_at) VALUES ('discussion', ?1, ?2)",
                rusqlite::params![
                    format!("Discussion {i:02}"),
                    format!("2026-11-08T09:{:02}:00.000000Z", i % 60)
                ],
            )
            .expect("post");
        }
    }
    let mut client = s.feed.client();
    let first = client.get("/new").body;
    assert_eq!(first.matches("<li class=\"hp-item ").count(), 30);
    assert!(
        first.contains(r#"start="1""#) && first.contains(r#"<span class="hp-rank">30.</span>"#)
    );
    assert!(first.contains(r#"href="/new?page=2""#));
    let second = client.get("/new?page=2").body;
    assert!(
        second.contains(r#"start="31""#) && second.contains(r#"<span class="hp-rank">60.</span>"#)
    );
    assert!(second.contains(r#"href="/new?page=3""#));
    let third = client.get("/new?page=3").body;
    assert!(
        third.contains(r#"start="61""#) && third.contains(r#"<span class="hp-rank">65.</span>"#)
    );
    assert!(!third.contains("hp-more"));
    for bad in ["0", "21", "abc", "-1", ""] {
        assert_eq!(client.get(&format!("/new?page={bad}")).status, 404, "{bad}");
    }
    assert_eq!(client.get("/new?page=20").status, 200);
    assert!(client.get("/").body.contains("hp-more"));
}

// --- Syncing ---

#[test]
fn entries_are_listed_or_skipped_with_their_reasons() {
    let s = setup();
    s.feed.sync();
    assert_eq!(s.count("SELECT COUNT(*) FROM packs WHERE listed = 1"), 10);
    assert_eq!(s.count("SELECT COUNT(*) FROM packs"), 10);
    assert_eq!(s.audit("registry.list"), 10);
    assert_eq!(s.audit("registry.skip"), 4);
    let reason = |name: &str| {
        s.text(&format!(
            "SELECT reason FROM audit_log WHERE action = 'registry.skip' AND target_id = '{name}'"
        ))
    };
    assert!(
        reason("copyleft-notes").contains("license"),
        "{}",
        reason("copyleft-notes")
    );
    assert!(reason("-oops").contains("name"));
    assert!(reason("short-hash").contains("pack_hash"));
    assert!(reason("twin").contains("more than once"));
    assert_eq!(
        s.count("SELECT COUNT(*) FROM audit_log WHERE public = 0 OR actor_id IS NOT NULL"),
        0
    );
    // Only packs of passes and theme packs with a web theme have posts.
    assert_eq!(s.count("SELECT COUNT(*) FROM posts"), 8);
    assert_eq!(
        s.count("SELECT COUNT(*) FROM posts WHERE author_id IS NOT NULL"),
        0
    );
    // A second sync changes nothing, and writes no audit row.
    let before = s.count("SELECT COUNT(*) FROM audit_log");
    let again = s.feed.sync();
    assert_eq!(
        again.line(),
        "listed 0, delisted 0, updated 0, skipped 5, unchanged 10, themes 0"
    );
    assert_eq!(s.count("SELECT COUNT(*) FROM audit_log"), before);
    assert_eq!(
        s.text("SELECT value FROM meta WHERE key = 'last_sync_ok'"),
        "1"
    );
}

#[test]
fn a_failing_entry_delists_and_a_passing_one_relists() {
    let s = setup();
    s.feed.sync();
    let set_license = |license: &'static str| {
        s.edit(move |packs| {
            for p in packs.iter_mut().filter(|p| p["name"] == "glossary-terms") {
                p["license"] = license.into();
            }
        })
    };
    set_license("GPL-3.0-only");
    let report = s.feed.sync();
    assert_eq!((report.delisted, report.skipped), (1, 5));
    assert_eq!(s.audit("registry.delist"), 1);
    let reason = s.text("SELECT reason FROM audit_log WHERE action = 'registry.delist'");
    assert!(reason.contains("license"), "{reason}");
    assert_eq!(
        s.count("SELECT listed FROM packs WHERE name = 'glossary-terms'"),
        0
    );
    // A failing entry for a delisted pack is not skipped again, nor delisted again.
    let again = s.feed.sync();
    assert_eq!((again.delisted, again.skipped), (0, 5));
    assert_eq!(s.audit("registry.delist"), 1);
    let id = s.post_id("glossary-terms");
    set_license("MIT");
    let relisted = s.feed.sync();
    assert_eq!(relisted.listed, 1);
    assert_eq!(s.audit("registry.list"), 11);
    assert!(has_post(&s.feed.client().get("/packs").body, id));
    // The post and its id survived.
    assert_eq!(s.post_id("glossary-terms"), id);
}

#[test]
fn a_pack_absent_from_the_index_is_delisted() {
    let s = setup();
    s.feed.sync();
    s.edit(|packs| packs.retain(|p| p["name"] != "decision-log"));
    let report = s.feed.sync();
    assert_eq!(report.delisted, 1);
    let reason = s.text("SELECT reason FROM audit_log WHERE action = 'registry.delist'");
    assert_eq!(reason, "absent from the index");
}

#[test]
fn a_new_pack_hash_is_recorded_with_its_version_and_commit() {
    let s = setup();
    s.feed.sync();
    assert_eq!(
        s.count("SELECT COUNT(*) FROM pack_contents WHERE pack = 'decision-log'"),
        1
    );
    s.edit(|packs| {
        for p in packs.iter_mut().filter(|p| p["name"] == "decision-log") {
            p["version"] = "1.1.0".into();
            p["pack_hash"] = "9".repeat(64).into();
            p["commit"] = "8".repeat(40).into();
        }
    });
    let report = s.feed.sync();
    assert_eq!((report.updated, report.listed), (1, 0));
    assert_eq!(
        s.text(
            "SELECT version || ' ' || commit_id FROM pack_contents
             WHERE pack = 'decision-log' AND pack_hash = '9999999999999999999999999999999999999999999999999999999999999999'"
        ),
        format!("1.1.0 {}", "8".repeat(40))
    );
    assert_eq!(
        s.count("SELECT COUNT(*) FROM pack_contents WHERE pack = 'decision-log'"),
        2
    );
    assert_eq!(
        s.text("SELECT version FROM packs WHERE name = 'decision-log'"),
        "1.1.0"
    );
    // An update is not a listing: no new audit row.
    assert_eq!(s.audit("registry.list"), 10);
}

#[test]
fn a_posts_title_follows_the_description_cut_to_120_characters() {
    let s = setup();
    s.feed.sync();
    let id = s.post_id("decision-log");
    s.edit(|packs| {
        for p in packs.iter_mut().filter(|p| p["name"] == "decision-log") {
            p["description"] = "Now it says something else.".into();
        }
    });
    s.feed.sync();
    assert_eq!(
        s.text(&format!("SELECT title FROM posts WHERE id = {id}")),
        "decision-log: Now it says something else."
    );
    s.edit(|packs| {
        for p in packs.iter_mut().filter(|p| p["name"] == "decision-log") {
            p["description"] = "x".repeat(300).into();
        }
    });
    s.feed.sync();
    let title = s.text(&format!("SELECT title FROM posts WHERE id = {id}"));
    assert_eq!(title.chars().count(), 120);
    assert!(title.starts_with("decision-log: xxx"));
    assert!(client_sees(&s, id, &title));
}

fn client_sees(s: &Setup, id: i64, title: &str) -> bool {
    s.feed
        .client()
        .get(&format!("/p/{id}"))
        .body
        .contains(title)
}

#[test]
fn the_owner_is_matched_by_owner_id_not_by_login() {
    let s = setup();
    s.feed.sync();
    let id = s.post_id("glossary-terms");
    let upvote = "Upvote: ";
    let mut ada = s.feed.client();
    ada.sign_in("ada");
    let mut grace = s.feed.client();
    grace.sign_in("grace");
    let arrow = |body: &str| body.contains(&format!("{upvote}glossary-terms"));
    // The owner has no arrow on her own pack's row; another member has.
    assert!(!arrow(&ada.get("/packs").body));
    assert!(arrow(&grace.get("/packs").body));
    // The owner's login changes in the index (and so does the repository's
    // address); her id still names her.
    s.edit(|packs| {
        for p in packs.iter_mut().filter(|p| p["name"] == "glossary-terms") {
            p["owner"] = "ada-renamed".into();
            p["maintainers"] = serde_json::json!(["ada-renamed"]);
            p["repo"] = "https://github.com/ada-renamed/hippocamp-packs".into();
        }
    });
    let report = s.feed.sync();
    assert_eq!(report.updated, 1);
    assert!(!arrow(&ada.get("/packs").body));
    assert!(arrow(&grace.get("/packs").body));
    assert!(ada.get(&format!("/p/{id}")).body.contains("ada-renamed"));
}

#[test]
fn a_maintainer_who_renames_stays_bound() {
    let s = setup();
    let mut linus = s.feed.client();
    linus.sign_in("linus");
    s.feed.sync();
    let bound = || {
        s.count(
            "SELECT COUNT(*) FROM pack_maintainer_ids WHERE pack = 'review-dates' AND github_id = 1003",
        )
    };
    assert_eq!(bound(), 1);
    assert_eq!(s.audit("registry.maintainers"), 1);
    let details = s.text("SELECT details FROM audit_log WHERE action = 'registry.maintainers'");
    assert!(details.contains("linus"), "{details}");
    // The owner is not bound by login; nobody else has signed in.
    assert_eq!(s.count("SELECT COUNT(*) FROM pack_maintainer_ids"), 1);
    // He renames on GitHub: the feed's row (set with SQL, as a later
    // sign-in would) shows the new login; the entry still names the old one.
    s.feed
        .conn()
        .execute(
            "UPDATE users SET login = 'linus-new' WHERE github_id = 1003",
            [],
        )
        .expect("rename");
    s.feed.sync();
    assert_eq!(bound(), 1);
    assert_eq!(s.audit("registry.maintainers"), 1);
    // The entry drops him: unbound, with a row saying so.
    s.edit(|packs| {
        for p in packs.iter_mut().filter(|p| p["name"] == "review-dates") {
            p["maintainers"] = serde_json::json!(["grace"]);
        }
    });
    s.feed.sync();
    assert_eq!(bound(), 0);
    assert_eq!(s.audit("registry.maintainers"), 2);
}

#[test]
fn a_registry_sync_row_is_written_only_for_an_admins_sync() {
    let s = setup();
    s.feed
        .conn()
        .execute(
            "INSERT INTO users (id, github_id, login, created_at) VALUES (7, 7, 'root', 'x')",
            [],
        )
        .expect("admin");
    s.feed.sync();
    assert_eq!(s.audit("registry.sync"), 0);
    s.feed.sync_as(Some(UserId(7)));
    assert_eq!(s.audit("registry.sync"), 1);
    let (actor, details, public): (i64, String, bool) = s
        .feed
        .conn()
        .query_row(
            "SELECT actor_id, details, public FROM audit_log WHERE action = 'registry.sync'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .expect("row");
    assert_eq!(actor, 7);
    assert!(public);
    assert_eq!(details, r#"{"delisted":0,"listed":0,"skipped":5}"#);
}

#[test]
fn a_bad_protocol_a_failed_fetch_and_invalid_json_change_no_pack() {
    let s = setup();
    s.feed.sync();
    let before = s.packs_snapshot();
    let audits = s.count("SELECT COUNT(*) FROM audit_log");
    let good = s.text("SELECT value FROM meta WHERE key = 'last_successful_sync'");
    s.feed.clock.advance(time::Duration::seconds(60));
    let wrong = std::fs::read_to_string(&s.index)
        .expect("read")
        .replace("HIPPOCAMP_V1", "HIPPOCAMP_V2");
    s.write_index(&wrong);
    let r = s.feed.sync();
    assert!(!r.ok() && r.listed + r.delisted + r.updated == 0, "{r:?}");
    s.write_index("{ this is not json");
    assert!(!s.feed.sync().ok());
    s.write_index("[]");
    assert!(!s.feed.sync().ok());
    std::fs::remove_file(&s.index).expect("remove");
    let r = s.feed.sync();
    assert!(r.error.as_deref().unwrap_or("").contains("index"), "{r:?}");
    assert_eq!(s.packs_snapshot(), before);
    assert_eq!(s.count("SELECT COUNT(*) FROM audit_log"), audits);
    assert_eq!(
        s.text("SELECT value FROM meta WHERE key = 'last_sync_ok'"),
        "0"
    );
    assert_eq!(
        s.text("SELECT value FROM meta WHERE key = 'last_successful_sync'"),
        good
    );
    assert_ne!(
        s.text("SELECT value FROM meta WHERE key = 'last_sync'"),
        good
    );
    let health = s.feed.client().get("/healthz").body;
    assert!(health.contains(r#""last_sync_ok":false"#), "{health}");
    // The pages still show every pack.
    assert!(has_post(
        &s.feed.client().get("/packs").body,
        s.post_id("glossary-terms")
    ));
}

#[test]
fn health_lists_sync_as_stale_after_six_hours_without_a_success() {
    let s = setup();
    s.feed.sync();
    let mut client = s.feed.client();
    s.feed.clock.advance(time::Duration::seconds(5 * 3600));
    assert!(client.get("/healthz").body.contains(r#""stale":[]"#));
    s.feed.clock.advance(time::Duration::seconds(2 * 3600));
    assert!(client.get("/healthz").body.contains(r#""sync""#));
    s.feed.sync();
    assert!(client.get("/healthz").body.contains(r#""stale":[]"#));
}

// --- Fetching ---

#[test]
fn night_inks_stylesheet_is_accepted_and_not_fetched_again() {
    let s = setup();
    let r = s.feed.sync();
    assert_eq!(r.themes, 1);
    assert_eq!(
        s.count("SELECT COUNT(*) FROM theme_files WHERE pack = 'night-ink' AND css IS NOT NULL"),
        1
    );
    assert_eq!(s.raw.hits(NIGHT_PATH), 1);
    s.feed.sync();
    assert_eq!(s.raw.hits(NIGHT_PATH), 1);
    assert_eq!(s.raw.hits(BAD_HASH_PATH), 1);
}

#[test]
fn refusals_are_recorded_once_and_tried_again_after_they_are_cleared() {
    let s = setup();
    s.feed.sync();
    let reason = |name: &str| {
        s.text(&format!(
            "SELECT failure FROM theme_files WHERE pack = '{name}' AND failure IS NOT NULL"
        ))
    };
    assert_eq!(reason("bad-hash"), "hash mismatch");
    let rules = reason("bad-rules");
    assert!(rules.starts_with("theme.css:"), "{rules}");
    s.feed.sync();
    assert_eq!(
        (s.raw.hits(BAD_HASH_PATH), s.raw.hits(BAD_RULES_PATH)),
        (1, 1)
    );
    let cleared = hippocamp_feed::registry::clear_refusals(&s.feed.conn()).expect("clear");
    assert_eq!(cleared, 2);
    assert_eq!(
        s.count("SELECT COUNT(*) FROM theme_files WHERE css IS NOT NULL"),
        1
    );
    s.feed.sync();
    assert_eq!(
        (s.raw.hits(BAD_HASH_PATH), s.raw.hits(BAD_RULES_PATH)),
        (2, 2)
    );
    assert_eq!(reason("bad-hash"), "hash mismatch");
    // Accepted content is not fetched again by clearing refusals.
    assert_eq!(s.raw.hits(NIGHT_PATH), 1);
}

#[test]
fn a_server_error_records_nothing_and_the_next_sync_fetches() {
    let s = setup();
    s.raw.answer(NIGHT_PATH, Answer::ServerError);
    let r = s.feed.sync();
    assert_eq!(r.themes, 0);
    assert_eq!(
        s.count("SELECT COUNT(*) FROM theme_files WHERE pack = 'night-ink'"),
        0
    );
    assert_eq!(s.raw.hits(NIGHT_PATH), 1);
    s.raw.clear();
    let r = s.feed.sync();
    assert_eq!(r.themes, 1);
    assert_eq!(s.raw.hits(NIGHT_PATH), 2);
}

#[test]
fn a_redirect_is_not_followed() {
    let s = setup();
    s.raw
        .answer(NIGHT_PATH, Answer::Redirect(NIGHT_V2_PATH.to_owned()));
    let r = s.feed.sync();
    assert_eq!(r.themes, 0);
    assert_eq!(s.raw.hits(NIGHT_V2_PATH), 0);
    assert_eq!(
        s.count("SELECT COUNT(*) FROM theme_files WHERE pack = 'night-ink'"),
        0
    );
}

#[test]
fn a_body_over_64_kib_and_a_body_that_is_not_utf8_are_refused() {
    let s = setup();
    s.raw.answer(NIGHT_PATH, Answer::Bytes(vec![b'a'; 70_000]));
    s.raw
        .answer(BAD_HASH_PATH, Answer::Bytes(vec![0xff, 0xfe, 0xfd]));
    s.feed.sync();
    let reason = |name: &str| {
        s.text(&format!(
            "SELECT failure FROM theme_files WHERE pack = '{name}' AND failure IS NOT NULL"
        ))
    };
    assert_eq!(reason("night-ink"), "over 64 KiB");
    assert_eq!(reason("bad-hash"), "not UTF-8");
    // Refused content is not fetched again.
    s.feed.sync();
    assert_eq!(s.raw.hits(NIGHT_PATH), 1);
}

// --- Choosing ---

fn links_theme_pack(body: &str) -> bool {
    body.contains("/themes/night-ink/")
}

#[test]
fn a_member_who_chose_night_ink_gets_it_while_the_feed_serves_it() {
    let s = setup();
    s.feed.sync();
    let hash = s.text("SELECT pack_hash FROM packs WHERE name = 'night-ink'");
    let href = format!("/themes/night-ink/{hash}.css");
    let mut a = s.feed.client();
    a.sign_in("ada");
    a.get("/settings");
    assert_eq!(
        a.post_form("/settings/theme", &[("theme", "pack:night-ink")])
            .status,
        303
    );
    for path in ["/about", "/"] {
        let body = a.get(path).body;
        assert!(body.contains(&href), "{path}: {body}");
        assert!(!body.contains("auto.css"), "{path}");
    }
    // Protected pages, error pages and ?safe=1 link `auto` and no theme pack.
    for path in ["/settings", "/login", "/nope", "/?safe=1"] {
        let r = a.get(path);
        if r.is_html() {
            assert!(r.body.contains("/static/themes/auto.css"), "{path}");
            assert!(!links_theme_pack(&r.body), "{path}");
        }
    }
    // A refused second version leaves members on the first.
    s.use_fixture("registry-index-2.json");
    let r = s.feed.sync();
    assert_eq!(r.updated, 1);
    assert_eq!(r.themes, 0);
    let refused =
        s.text("SELECT failure FROM theme_files WHERE pack = 'night-ink' AND failure IS NOT NULL");
    assert!(refused.starts_with("theme.css:"), "{refused}");
    assert!(a.get("/about").body.contains(&href));
    assert_eq!(a.get(&href).status, 200);

    // The post is removed (set with SQL): `auto`, and the stylesheet is gone.
    let conn = s.feed.conn();
    conn.execute(
        "UPDATE posts SET state = 'removed' WHERE pack = 'night-ink'",
        [],
    )
    .expect("remove");
    let body = a.get("/about").body;
    assert!(!links_theme_pack(&body) && body.contains("/static/themes/auto.css"));
    assert_eq!(a.get(&href).status, 404);
    assert!(a
        .get(&format!("/p/{}", s.post_id("night-ink")))
        .body
        .contains("This theme is not available."));
    // Another member cannot choose it now; the member who has it keeps it.
    let mut b = s.feed.client();
    b.sign_in("grace");
    b.get("/settings");
    assert_eq!(
        b.post_form("/settings/theme", &[("theme", "pack:night-ink")])
            .status,
        422
    );
    conn.execute(
        "UPDATE posts SET state = 'visible' WHERE pack = 'night-ink'",
        [],
    )
    .expect("restore");
    assert!(a.get("/about").body.contains(&href));

    // The pack is delisted: `auto` again, and the choice is kept.
    s.edit(|packs| packs.retain(|p| p["name"] != "night-ink"));
    assert_eq!(s.feed.sync().delisted, 1);
    let body = a.get("/").body;
    assert!(!links_theme_pack(&body) && body.contains("/static/themes/auto.css"));
    assert_eq!(a.get(&href).status, 404);
    let settings = a.get("/settings").body;
    assert!(
        settings.contains(r#"value="pack:night-ink" selected"#),
        "{settings}"
    );
    assert_eq!(
        a.post_form(
            "/settings/theme",
            &[("theme", "pack:night-ink"), ("personal_css", "")]
        )
        .status,
        303
    );
}

#[test]
fn settings_offers_the_served_theme_packs() {
    let s = setup();
    s.feed.sync();
    let mut a = s.feed.client();
    a.sign_in("ada");
    let body = a.get("/settings").body;
    assert!(body.contains(r#"value="pack:night-ink""#));
    assert!(!body.contains("pack:bad-hash") && !body.contains("pack:bad-rules"));
    assert!(!body.contains("pack:desk-only"));
}
