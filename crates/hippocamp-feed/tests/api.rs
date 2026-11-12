//! The read-only API v1 over HTTP.
//!
//! The setup goes through the library's own functions: the fixture registry
//! synced with `RawServer`, members, discussions with and without a link,
//! comments with replies, votes, measurements through `measurements::enter`,
//! one comment hidden by three reports (`reports::file`) and one removed by a
//! moderator. Members are inserted with SQL (sign-in is not what is tested),
//! with the GitHub ids of the mock provider so a session can be started for
//! them. The golden files are `fixtures/feed/api/{posts,post}.json`;
//! `HIPPOCAMP_UPDATE_GOLDEN=1 cargo test -p hippocamp-feed --test api` rewrites them
//! from the actual output, which is then checked by hand.

mod common;

use std::path::{Path, PathBuf};

use common::{Options, RawServer, Reply, TestFeed};
use hippocamp_feed::db::{Member, UserId};
use hippocamp_feed::measurements::{self, MeasurementForm};
use hippocamp_feed::moderation::{self, Actor};
use hippocamp_feed::policy::AddressQuota;
use hippocamp_feed::reports::{self, ReportTarget};
use hippocamp_feed::web::Quotas;
use hippocamp_feed::{clock, comments, posts, users, votes};
use rusqlite::{params, Transaction};
use serde_json::{json, Value};
use time::Duration;

const INDEX: &str = "../../fixtures/feed/registry-index.json";
const EVAL_SET: &str = "3f2a9c01b7d4";

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/feed")
}

struct World {
    feed: TestFeed,
    _raw: RawServer,
    /// A copy of the index, when the test edits it.
    index: Option<(tempfile::TempDir, PathBuf)>,
    // Posts and comments.
    d1: i64,
    d2: i64,
    removed_post: i64,
    hidden_post: i64,
    deleted_post: i64,
    c_top: i64,
    c_reply: i64,
    c_hidden: i64,
    c_hidden_reply: i64,
    c_removed: i64,
    c_removed_reply: i64,
    c_gone: i64,
    glossary: i64,
    decision: i64,
    theme: i64,
}

fn tx<R>(feed: &TestFeed, f: impl FnOnce(&Transaction) -> R) -> R {
    let mut conn = feed.conn();
    let t = conn.transaction().expect("transaction");
    let out = f(&t);
    t.commit().expect("commit");
    out
}

fn member(t: &Transaction, id: i64, login: &str, role: &str, feed: &TestFeed) -> Member {
    let now = feed.clock_now();
    t.execute(
        "INSERT INTO users (github_id, login, github_created_at, created_at, role)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            id,
            login,
            clock::storage(now - Duration::days(900)),
            clock::storage(now - Duration::days(38)),
            role
        ],
    )
    .expect("member");
    users::load_member(t, UserId(t.last_insert_rowid()))
        .expect("load")
        .expect("member")
}

fn post_of(t: &Transaction, pack: &str) -> i64 {
    t.query_row("SELECT id FROM posts WHERE pack = ?1", [pack], |r| r.get(0))
        .expect("pack post")
}

fn form(hash: &str, counts: [i64; 4]) -> MeasurementForm {
    MeasurementForm {
        pack_hash: hash.to_string(),
        eval_set: EVAL_SET.to_string(),
        hippocamp: "0.1.0".to_string(),
        model: "claude".to_string(),
        runs: "3".to_string(),
        corrected_without: counts[0].to_string(),
        damaged_without: counts[1].to_string(),
        corrected_with: counts[2].to_string(),
        damaged_with: counts[3].to_string(),
        report_url: String::new(),
    }
}

fn world_with(options: Options, index: Option<&str>, copy_index: bool) -> World {
    let raw = RawServer::start_default();
    let (copy, index) = if copy_index {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("index.json");
        std::fs::copy(fixtures().join("registry-index.json"), &path).expect("copy");
        let shown = path.display().to_string();
        (Some((dir, path)), shown)
    } else {
        (None, index.unwrap_or(INDEX).to_string())
    };
    let feed = TestFeed::start(Options {
        registry_index: Some(index),
        raw_url: Some(raw.url.clone()),
        eval_sets: Some(format!("{EVAL_SET}:6:8")),
        ..options
    });
    assert!(feed.sync().ok());
    let now = feed.clock_now();
    let h = Duration::hours;
    let ids = tx(&feed, |t| {
        let ada = member(t, 1001, "ada", "none", &feed);
        let grace = member(t, 1002, "grace", "none", &feed);
        let linus = member(t, 1003, "linus", "none", &feed);
        let alan = member(t, 1005, "alan", "none", &feed);
        let hopper = member(t, 1006, "hopper", "none", &feed);
        let dennis = member(t, 1007, "dennis", "none", &feed);
        let moderator = member(t, 1008, "margaret", "moderator", &feed);
        let actor = Actor::new(&moderator, &[]).expect("actor");

        let discussion = |title: &str, link: &str, body: &str| {
            posts::Discussion::parse(title, link, body).expect("discussion")
        };
        let d1 = posts::create_discussion(
            t,
            &grace,
            &discussion(
                "What does dreaming change in your notes?",
                "https://example.com/dreaming",
                "I keep a **weekly** log.",
            ),
            now - h(5),
        )
        .expect("d1");
        let d2 = posts::create_discussion(
            t,
            &linus,
            &discussion(
                "Ask hippocamp: how do you review a pack?",
                "",
                "Read the passes first.",
            ),
            now - h(4),
        )
        .expect("d2");
        let removed_post = posts::create_discussion(
            t,
            &alan,
            &discussion("A removed post", "https://example.com/removed", "gone"),
            now - h(3),
        )
        .expect("removed");
        let hidden_post = posts::create_discussion(
            t,
            &alan,
            &discussion("A hidden post", "https://example.com/hidden", "hidden"),
            now - h(3),
        )
        .expect("hidden");
        let deleted_post = posts::create_discussion(
            t,
            &hopper,
            &discussion("A deleted post", "https://example.com/deleted", "deleted"),
            now - h(3),
        )
        .expect("deleted");

        // Comments on d1: a thread with a reply, a comment hidden by reports
        // with a visible reply, a removed one with a visible reply, and a
        // removed one with nothing below it.
        let c_top = comments::create(t, &linus, d1, None, "First, with *emphasis*.", now - h(4))
            .expect("c");
        let c_reply =
            comments::create(t, &alan, d1, Some(c_top), "A reply.", now - h(3)).expect("c");
        let c_hidden =
            comments::create(t, &hopper, d1, None, "Buy things here", now - h(3)).expect("c");
        let c_hidden_reply = comments::create(
            t,
            &ada,
            d1,
            Some(c_hidden),
            "Please ignore the above.",
            now - h(2),
        )
        .expect("c");
        let c_removed = comments::create(t, &alan, d1, None, "Rude words", now - h(2)).expect("c");
        let c_removed_reply = comments::create(
            t,
            &linus,
            d1,
            Some(c_removed),
            "Let us be kind.",
            now - h(1),
        )
        .expect("c");
        let c_gone =
            comments::create(t, &alan, d1, None, "More rude words", now - h(1)).expect("c");

        // Votes (all by established members who are not the author, and for a
        // pack's post not a maintainer).
        for voter in [&linus, &alan, &hopper] {
            votes::vote(t, voter, votes::Target::Post(d1), now - h(2)).expect("vote");
        }
        votes::vote(t, &grace, votes::Target::Post(d2), now - h(2)).expect("vote");
        for voter in [&alan, &hopper] {
            votes::vote(t, voter, votes::Target::Comment(c_top), now - h(2)).expect("vote");
        }
        votes::vote(t, &grace, votes::Target::Comment(c_reply), now - h(2)).expect("vote");
        let glossary = post_of(t, "glossary-terms");
        let decision = post_of(t, "decision-log");
        for voter in [&grace, &linus] {
            votes::vote(t, voter, votes::Target::Post(glossary), now - h(2)).expect("vote");
        }
        votes::vote(t, &alan, votes::Target::Post(decision), now - h(2)).expect("vote");

        // Reports: three members hide a comment; a moderator removes two
        // comments and a post; reports hide another post; an author deletes
        // theirs.
        for reporter in [&ada, &grace, &alan] {
            reports::file(
                t,
                reporter,
                ReportTarget::Comment(c_hidden),
                "spam",
                "",
                now - h(1),
            )
            .expect("report");
        }
        moderation::comment_remove(t, &actor, c_removed, "abuse", now - h(1)).expect("remove");
        moderation::comment_remove(t, &actor, c_gone, "abuse", now - h(1)).expect("remove");
        moderation::post_remove(t, &actor, removed_post, "spam", now - h(1)).expect("remove");
        assert!(posts::hide_by_reports(t, hidden_post).expect("hide"));
        posts::delete_post(t, &hopper, deleted_post).expect("delete");

        // Measurements of glossary-terms (maintainer: ada): three counted, one
        // of the maintainer's. Eval set 6 planted, 8 kept; 3 runs each.
        let hash = measurements::pack_facts(t, "glossary-terms")
            .expect("facts")
            .expect("pack")
            .hashes[0]
            .clone();
        let sets = hippocamp_feed::config::EvalSet {
            id: EVAL_SET.to_string(),
            planted: 6,
            kept: 8,
        };
        let sets = [sets];
        let enter = |who: &Member, counts: [i64; 4], at| {
            measurements::enter(t, who, "glossary-terms", &form(&hash, counts), &sets, at)
                .expect("measurement")
        };
        enter(&grace, [6, 2, 10, 2], now - h(30));
        enter(&linus, [5, 1, 8, 2], now - h(28));
        enter(&alan, [7, 2, 12, 1], now - h(26));
        enter(&ada, [6, 2, 12, 2], now - h(24));

        // decision-log: an invalid measurement, and one whose member later
        // stops being established (their feed account is made recent below,
        // which no function of the library does).
        let hash = measurements::pack_facts(t, "decision-log")
            .expect("facts")
            .expect("pack")
            .hashes[0]
            .clone();
        let m_invalid = measurements::enter(
            t,
            &hopper,
            "decision-log",
            &form(&hash, [6, 2, 10, 2]),
            &sets,
            now - h(20),
        )
        .expect("measurement");
        measurements::enter(
            t,
            &dennis,
            "decision-log",
            &form(&hash, [6, 2, 9, 2]),
            &sets,
            now - h(19),
        )
        .expect("measurement");
        moderation::measurement_invalidate(t, &actor, m_invalid, "misleading", now - h(18))
            .expect("invalidate");
        t.execute(
            "UPDATE users SET created_at = ?1 WHERE login = 'dennis'",
            [clock::storage(now - Duration::days(1))],
        )
        .expect("recent");

        let theme = post_of(t, "night-ink");
        (
            d1,
            d2,
            removed_post,
            hidden_post,
            deleted_post,
            c_top,
            c_reply,
            c_hidden,
            c_hidden_reply,
            c_removed,
            c_removed_reply,
            c_gone,
            glossary,
            decision,
            theme,
        )
    });
    World {
        feed,
        _raw: raw,
        index: copy,
        d1: ids.0,
        d2: ids.1,
        removed_post: ids.2,
        hidden_post: ids.3,
        deleted_post: ids.4,
        c_top: ids.5,
        c_reply: ids.6,
        c_hidden: ids.7,
        c_hidden_reply: ids.8,
        c_removed: ids.9,
        c_removed_reply: ids.10,
        c_gone: ids.11,
        glossary: ids.12,
        decision: ids.13,
        theme: ids.14,
    }
}

fn world() -> World {
    world_with(Options::default(), None, false)
}

fn json_of(reply: &Reply) -> Value {
    assert_eq!(reply.status, 200, "{}", reply.body);
    serde_json::from_str(&reply.body).expect("json")
}

fn get(w: &World, path: &str) -> Reply {
    w.feed.client().get(path)
}

fn api(w: &World, path: &str) -> Value {
    json_of(&get(w, path))
}

/// The response with the test server's address (a new port every run)
/// replaced by a fixed one.
fn stable(w: &World, path: &str) -> Value {
    let reply = get(w, path);
    assert_eq!(reply.status, 200, "{}", reply.body);
    let text = reply.body.replace(&w.feed.base_url, "https://feed.example");
    serde_json::from_str(&text).expect("json")
}

fn golden(name: &str, actual: &Value) {
    let path = fixtures().join("api").join(name);
    let pretty = format!(
        "{}\n",
        serde_json::to_string_pretty(actual).expect("pretty")
    );
    if std::env::var_os("HIPPOCAMP_UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().expect("dir")).expect("dir");
        std::fs::write(&path, pretty).expect("write");
        return;
    }
    let text = std::fs::read_to_string(&path).expect("golden file");
    let expected: Value = serde_json::from_str(&text).expect("golden json");
    assert_eq!(
        &expected, actual,
        "{name} differs; rerun with HIPPOCAMP_UPDATE_GOLDEN=1 to rewrite it"
    );
}

fn error_of(reply: &Reply, status: u16) -> String {
    assert_eq!(reply.status, status, "{}", reply.body);
    let body: Value = serde_json::from_str(&reply.body).expect("json error");
    assert_eq!(body.as_object().expect("object").len(), 1);
    body["error"].as_str().expect("message").to_string()
}

fn post_in(list: &Value, id: i64) -> &Value {
    list["posts"]
        .as_array()
        .expect("posts")
        .iter()
        .find(|p| p["id"] == id)
        .unwrap_or_else(|| panic!("post {id} is not listed"))
}

fn pack_listed(post: &Value) -> bool {
    post["pack"]["listed"].as_bool().unwrap_or(true)
}

fn ids_of(list: &Value) -> Vec<i64> {
    list["posts"]
        .as_array()
        .expect("posts")
        .iter()
        .map(|p| p["id"].as_i64().expect("id"))
        .collect()
}

#[test]
fn the_front_list_and_a_pack_post_equal_their_golden_files() {
    let w = world();
    golden("posts.json", &stable(&w, "/api/v1/posts"));
    golden(
        "post.json",
        &stable(&w, &format!("/api/v1/posts/{}", w.glossary)),
    );
}

#[test]
fn headers_carry_no_cookie_and_no_cors() {
    let w = world();
    for path in [
        "/api/v1/posts".to_string(),
        format!("/api/v1/posts/{}", w.glossary),
    ] {
        let reply = get(&w, &path);
        assert_eq!(reply.status, 200);
        assert_eq!(
            reply.header("content-type"),
            Some("application/json; charset=utf-8")
        );
        assert_eq!(reply.header("cache-control"), Some("public, max-age=30"));
        assert!(reply.set_cookies().is_empty());
        assert!(reply
            .headers
            .iter()
            .all(|(k, _)| !k.to_ascii_lowercase().starts_with("access-control-")));
    }
}

#[test]
fn the_defaults_and_the_three_sorts() {
    let w = world();
    let front = api(&w, "/api/v1/posts");
    assert_eq!(front["sort"], "front");
    assert_eq!(front["page"], 1);
    assert_eq!(front, api(&w, "/api/v1/posts?sort=front&page=1"));

    let new = api(&w, "/api/v1/posts?sort=new");
    assert_eq!(new["sort"], "new");
    // Newest first: the packs (created at the sync, now), then d2 and d1.
    let ids = ids_of(&new);
    let pos = |id: i64| ids.iter().position(|&x| x == id).expect("listed");
    assert!(pos(w.glossary) < pos(w.d2) && pos(w.d2) < pos(w.d1));

    let evidence = api(&w, "/api/v1/posts?sort=evidence");
    assert_eq!(evidence["sort"], "evidence");
    // Only packs of passes; glossary-terms has evidence above zero and leads.
    let ids = ids_of(&evidence);
    assert_eq!(ids[0], w.glossary);
    assert!(!ids.contains(&w.d1) && !ids.contains(&w.d2));
    for post in evidence["posts"].as_array().expect("posts") {
        assert_eq!(post["kind"], "pack");
    }

    // The front list holds both kinds; a post past the last page is empty.
    let front_ids = ids_of(&front);
    assert!(front_ids.contains(&w.d1) && front_ids.contains(&w.glossary));
    let page_2 = api(&w, "/api/v1/posts?page=2");
    assert_eq!(page_2["page"], 2);
    assert_eq!(page_2["posts"], json!([]));
    assert_eq!(api(&w, "/api/v1/posts?page=20")["page"], 20);
}

#[test]
fn lists_hold_no_theme_pack_and_only_visible_posts() {
    let w = world();
    for sort in ["front", "new", "evidence"] {
        let list = api(&w, &format!("/api/v1/posts?sort={sort}"));
        let ids = ids_of(&list);
        assert!(!ids.contains(&w.theme), "{sort}");
        for gone in [w.removed_post, w.hidden_post, w.deleted_post] {
            assert!(!ids.contains(&gone), "{sort}");
        }
        for post in list["posts"].as_array().expect("posts") {
            assert_eq!(post["state"], "visible");
            assert_ne!(post["pack"]["kind"], "theme");
            assert!(pack_listed(post));
        }
    }
}

#[test]
fn a_discussion_with_a_link_and_one_without() {
    let w = world();
    let list = api(&w, "/api/v1/posts?sort=new");
    let base = w.feed.base_url.clone();

    let d1 = post_in(&list, w.d1);
    assert_eq!(d1["kind"], "discussion");
    assert_eq!(d1["title"], "What does dreaming change in your notes?");
    assert_eq!(d1["url"], format!("{base}/p/{}", w.d1));
    assert_eq!(d1["link"], "https://example.com/dreaming");
    assert_eq!(d1["author"], "grace");
    assert_eq!(d1["edited_at"], Value::Null);
    assert_eq!(d1["state"], "visible");
    assert_eq!(d1["votes"], 3);
    // Visible comments: first, its reply and the reply under the hidden one
    // and the one under the removed one.
    assert_eq!(d1["comments"], 4);
    assert_eq!(d1["pack"], Value::Null);
    let created = d1["created_at"].as_str().expect("time");
    assert_eq!(created, "2026-11-08T05:00:00Z");

    let d2 = post_in(&list, w.d2);
    assert_eq!(d2["link"], Value::Null);
    assert_eq!(d2["author"], "linus");
    assert_eq!(d2["votes"], 1);
    assert_eq!(d2["comments"], 0);

    // The single post carries the body.
    let one = api(&w, &format!("/api/v1/posts/{}", w.d1));
    assert_eq!(one["post"]["body"], "I keep a **weekly** log.");
    assert_eq!(one["measurements"], json!([]));
    let one = api(&w, &format!("/api/v1/posts/{}", w.d2));
    assert_eq!(one["post"]["link"], Value::Null);
    assert_eq!(one["post"]["body"], "Read the passes first.");
}

#[test]
fn a_pack_post_field_by_field() {
    let w = world();
    let one = api(&w, &format!("/api/v1/posts/{}", w.glossary));
    let post = &one["post"];
    assert_eq!(post["kind"], "pack");
    assert_eq!(post["link"], Value::Null);
    assert_eq!(post["author"], Value::Null);
    assert_eq!(post["body"], "");
    assert_eq!(post["votes"], 2);
    assert_eq!(post["created_at"], "2026-11-08T10:00:00Z");
    let pack = &post["pack"];
    assert_eq!(pack["name"], "glossary-terms");
    assert_eq!(pack["kind"], "passes");
    assert_eq!(pack["listed"], true);
    assert_eq!(pack["owner"], "ada");
    assert_eq!(pack["maintainers"], json!(["ada"]));
    assert_eq!(pack["first_party"], false);
    assert_eq!(pack["pack_hash"].as_str().expect("hash").len(), 64);
    let install = &pack["install"];
    assert_eq!(
        install["command"],
        format!("hippocamp add glossary-terms --registry {INDEX}")
    );
    let deep = install["deep_link"].as_str().expect("link");
    assert_eq!(
        deep,
        format!(
            "hippocamp://add?pack=glossary-terms&version={}&sha256={}",
            pack["version"].as_str().expect("version"),
            pack["pack_hash"].as_str().expect("hash")
        )
    );

    // Benefits are 4/18 net-rate steps: 0.2222, 0.125 and 0.3194; the median
    // 0.2222 shrinks by 3/5 to 0.1333.
    let evidence = &pack["evidence"];
    assert_eq!(evidence["score"], 0.133);
    assert_eq!(evidence["measurements"], 3);
    assert_eq!(evidence["median_benefit"], 0.222);
    assert_eq!(evidence["version"], pack["version"]);
    assert_eq!(evidence["pack_hash"], pack["pack_hash"]);

    // Measurements, newest first: the maintainer's (24 h ago), alan, linus,
    // grace.
    let ms = one["measurements"].as_array().expect("measurements");
    let members: Vec<&str> = ms
        .iter()
        .map(|m| m["member"].as_str().expect("m"))
        .collect();
    assert_eq!(members, ["ada", "alan", "linus", "grace"]);
    let status: Vec<&str> = ms
        .iter()
        .map(|m| m["status"].as_str().expect("s"))
        .collect();
    assert_eq!(status, ["maintainer", "counted", "counted", "counted"]);
    let grace = &ms[3];
    assert_eq!(grace["eval_set"], EVAL_SET);
    assert_eq!(grace["hippocamp"], "0.1.0");
    assert_eq!(grace["model"], "claude");
    assert_eq!(grace["runs"], 3);
    assert_eq!(grace["net_without"], 0.25); // 6/18 - 2/24
    assert_eq!(grace["net_with"], 0.472); // 10/18 - 2/24
    assert_eq!(grace["benefit"], 0.222);
    assert_eq!(grace["revisions"], 0);
    assert_eq!(grace["report_url"], Value::Null);
    assert_eq!(grace["version"], pack["version"]);
    assert_eq!(grace["pack_hash"], pack["pack_hash"]);
    assert_eq!(grace["created_at"], grace["updated_at"]);
    assert_eq!(grace["created_at"], "2026-11-07T04:00:00Z");
}

#[test]
fn measurement_statuses_invalid_and_not_counted() {
    let w = world();
    let one = api(&w, &format!("/api/v1/posts/{}", w.decision));
    let ms = one["measurements"].as_array().expect("measurements");
    // dennis (not established now) entered after hopper (marked invalid).
    assert_eq!(ms[0]["member"], "dennis");
    assert_eq!(ms[0]["status"], "not_counted");
    assert_eq!(ms[1]["member"], "hopper");
    assert_eq!(ms[1]["status"], "invalid");
    // No content has 3 counted measurements.
    assert_eq!(one["post"]["pack"]["evidence"], Value::Null);
}

#[test]
fn comments_are_a_tree_with_placeholders_only_where_needed() {
    let w = world();
    let one = api(&w, &format!("/api/v1/posts/{}", w.d1));
    let top = one["comments"].as_array().expect("comments");
    let ids: Vec<i64> = top.iter().map(|c| c["id"].as_i64().expect("id")).collect();
    // Voted comment first, then by age; the removed comment with no reply is
    // left out.
    assert_eq!(ids, [w.c_top, w.c_hidden, w.c_removed]);
    assert!(!ids.contains(&w.c_gone));

    let first = &top[0];
    assert_eq!(first["author"], "linus");
    assert_eq!(first["body"], "First, with *emphasis*.");
    assert_eq!(first["state"], "visible");
    assert_eq!(first["votes"], 2);
    assert_eq!(first["edited_at"], Value::Null);
    assert_eq!(first["created_at"], "2026-11-08T06:00:00Z");
    let reply = &first["children"][0];
    assert_eq!(reply["id"], w.c_reply);
    assert_eq!(reply["author"], "alan");
    assert_eq!(reply["votes"], 1);
    assert_eq!(reply["children"], json!([]));

    let hidden = &top[1];
    assert_eq!(hidden["state"], "hidden");
    assert_eq!(hidden["author"], Value::Null);
    assert_eq!(hidden["body"], Value::Null);
    assert_eq!(hidden["votes"], 0);
    assert_eq!(hidden["children"][0]["id"], w.c_hidden_reply);
    assert_eq!(hidden["children"][0]["author"], "ada");

    let removed = &top[2];
    assert_eq!(removed["state"], "removed");
    assert_eq!(removed["author"], Value::Null);
    assert_eq!(removed["body"], Value::Null);
    assert_eq!(removed["children"][0]["id"], w.c_removed_reply);
    assert_eq!(removed["children"][0]["body"], "Let us be kind.");
}

#[test]
fn comments_nest_at_most_ten_levels() {
    let w = world();
    let now = w.feed.clock_now();
    tx(&w.feed, |t| {
        let linus = users::load_member(t, UserId(3))
            .expect("load")
            .expect("linus");
        let mut parent = None;
        for i in 0..11 {
            let id = comments::create(
                t,
                &linus,
                w.d2,
                parent,
                &format!("level {i}"),
                now - Duration::minutes(30 - i),
            )
            .expect("comment");
            parent = Some(id);
        }
        assert!(comments::create(t, &linus, w.d2, parent, "too deep", now).is_err());
    });
    let one = api(&w, &format!("/api/v1/posts/{}", w.d2));
    let mut level = &one["comments"][0];
    let mut depth = 0;
    while let Some(next) = level["children"].get(0) {
        level = next;
        depth += 1;
    }
    assert_eq!(depth, 10);
    assert_eq!(level["body"], "level 10");
}

#[test]
fn posts_that_are_not_visible_show_only_what_the_design_allows() {
    let w = world();
    for (id, state) in [
        (w.removed_post, "removed"),
        (w.hidden_post, "hidden"),
        (w.deleted_post, "deleted"),
    ] {
        let one = api(&w, &format!("/api/v1/posts/{id}"));
        let post = &one["post"];
        assert_eq!(post["state"], state);
        assert_eq!(post["title"], Value::Null);
        assert_eq!(post["link"], Value::Null);
        assert_eq!(post["body"], Value::Null);
        assert_eq!(post["kind"], "discussion");
        assert_eq!(post["url"], format!("{}/p/{id}", w.feed.base_url));
        assert_eq!(one["comments"], json!([]));
    }
}

#[test]
fn a_theme_pack_has_kind_theme_and_no_install_or_evidence() {
    let w = world();
    let one = api(&w, &format!("/api/v1/posts/{}", w.theme));
    let post = &one["post"];
    assert_eq!(post["kind"], "pack");
    let pack = &post["pack"];
    assert_eq!(pack["kind"], "theme");
    assert_eq!(pack["name"], "night-ink");
    assert_eq!(pack["listed"], true);
    assert_eq!(pack["install"], Value::Null);
    assert_eq!(pack["evidence"], Value::Null);
    assert_eq!(pack["passes"], json!([]));
    assert_eq!(one["measurements"], json!([]));
}

#[test]
fn a_delisted_pack_is_not_listed_and_has_no_install() {
    let w = world_with(Options::default(), None, true);
    let (_, index) = w.index.as_ref().expect("copied index");
    let text = std::fs::read_to_string(index).expect("read");
    let mut json: Value = serde_json::from_str(&text).expect("json");
    json["packs"]
        .as_array_mut()
        .expect("packs")
        .retain(|p| p["name"] != "decision-log");
    std::fs::write(index, json.to_string()).expect("write");
    assert!(w.feed.sync().ok());

    let one = api(&w, &format!("/api/v1/posts/{}", w.decision));
    let pack = &one["post"]["pack"];
    assert_eq!(pack["listed"], false);
    assert_eq!(pack["install"], Value::Null);
    assert_eq!(pack["kind"], "passes");
    // Its measurements stay public, and it leaves every list.
    assert_eq!(one["measurements"].as_array().expect("m").len(), 2);
    for sort in ["front", "new", "evidence"] {
        let list = api(&w, &format!("/api/v1/posts?sort={sort}"));
        assert!(!ids_of(&list).contains(&w.decision), "{sort}");
    }
    // A listed pack still has its install block.
    let other = api(&w, &format!("/api/v1/posts/{}", w.glossary));
    assert_ne!(other["post"]["pack"]["install"], Value::Null);
}

#[test]
fn the_session_is_ignored_and_so_is_hiding() {
    let w = world();
    let anonymous = api(&w, "/api/v1/posts");
    let mut grace = w.feed.client();
    grace.sign_in("grace");
    grace.get("/");
    let r = grace.post_form("/hide", &[("post_id", &w.d2.to_string()), ("unhide", "0")]);
    assert_eq!(r.status, 303, "{}", r.body);
    // The page list leaves the post out for grace; the API does not.
    assert!(!grace
        .get("/new")
        .body
        .contains("Ask hippocamp: how do you review"));
    let reply = grace.get("/api/v1/posts");
    assert_eq!(json_of(&reply), anonymous);
    assert!(ids_of(&anonymous).contains(&w.d2));
    assert!(reply.set_cookies().is_empty());
}

#[test]
fn bad_sorts_and_pages_are_400() {
    let w = world();
    for path in [
        "/api/v1/posts?sort=hot",
        "/api/v1/posts?sort=",
        "/api/v1/posts?sort=Front",
        "/api/v1/posts?page=0",
        "/api/v1/posts?page=21",
        "/api/v1/posts?page=-1",
        "/api/v1/posts?page=abc",
        "/api/v1/posts?page=",
        "/api/v1/posts?page=%2B2",
        "/api/v1/posts?sort=new&page=99",
    ] {
        let reply = get(&w, path);
        assert!(!error_of(&reply, 400).is_empty(), "{path}");
        assert_eq!(reply.header("content-type"), Some("application/json"));
    }
}

#[test]
fn malformed_and_unknown_ids_are_404() {
    let w = world();
    for id in [
        "0",
        "-1",
        "abc",
        "1.5",
        "+3",
        "99999999",
        "99999999999999999999",
    ] {
        let reply = get(&w, &format!("/api/v1/posts/{id}"));
        assert!(!error_of(&reply, 404).is_empty(), "{id}");
    }
    assert_eq!(get(&w, "/api/v1/nothing").status, 404);
}

#[test]
fn the_api_limiter_answers_429_with_retry_after() {
    let generous = AddressQuota {
        count: 100_000,
        per: std::time::Duration::from_secs(60),
    };
    let w = world_with(
        Options {
            quotas: Quotas {
                sign_in: generous,
                api: AddressQuota {
                    count: 3,
                    per: std::time::Duration::from_secs(60),
                },
                all: generous,
            },
            ..Options::default()
        },
        None,
        false,
    );
    let mut client = w.feed.client();
    for _ in 0..3 {
        assert_eq!(client.get("/api/v1/posts").status, 200);
    }
    let reply = client.get("/api/v1/posts");
    assert!(!error_of(&reply, 429).is_empty());
    let retry: u64 = reply
        .header("retry-after")
        .expect("Retry-After")
        .parse()
        .expect("seconds");
    assert!(retry >= 1);
    // Pages are not counted against the API limiter.
    assert_eq!(client.get("/about").status, 200);
}

#[test]
fn a_busy_database_answers_503() {
    let w = world_with(
        Options {
            db_wait: std::time::Duration::from_millis(100),
            ..Options::default()
        },
        None,
        false,
    );
    let permits = w.feed.db.permits();
    let held = permits
        .clone()
        .try_acquire_many_owned(hippocamp_feed::policy::DB_PERMITS as u32)
        .expect("all permits");
    let reply = get(&w, "/api/v1/posts");
    assert!(!error_of(&reply, 503).is_empty());
    drop(held);
    assert_eq!(get(&w, "/api/v1/posts").status, 200);
}
