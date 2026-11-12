//! Measuring a pack over HTTP: the measure page, each refusal, replacement,
//! the large-benefit flag, and that evidence ranks packs.
//!
//! Members sign in through the mock provider at the clock's start, so their
//! feed accounts are 3 days old once the clock has moved on; `newbie`'s GitHub
//! account is 10 days old, so it never is established. A measurement by a
//! member who is not established cannot be entered through the page, so the
//! one the ranking test uses for `newbie` is inserted with SQL, and says so.
//!
//! Eval set `000000000001` has 6 planted errors and 8 kept claims. With 3
//! runs, a net rate has denominators 18 (planted x runs) and 24 (kept x runs),
//! and a benefit is `(dc x 8 - dd x 6) / 144` for `dc`, the change in
//! corrected counts and `dd` the change in damaged counts.

mod common;

use std::path::{Path, PathBuf};

use common::{Options, RawServer, Reply, TestClient, TestFeed};
use hippocamp_feed::rank;
use time::Duration;

const SET: &str = "000000000001";

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/feed")
}

struct Setup {
    feed: TestFeed,
    index: PathBuf,
    _raw: RawServer,
    _dir: tempfile::TempDir,
}

fn setup(eval_sets: Option<&str>) -> Setup {
    let raw = RawServer::start_default();
    let dir = tempfile::tempdir().expect("tempdir");
    let index = dir.path().join("index.json");
    std::fs::copy(fixtures().join("registry-index.json"), &index).expect("copy");
    let feed = TestFeed::start(Options {
        registry_index: Some(index.display().to_string()),
        raw_url: Some(raw.url.clone()),
        eval_sets: eval_sets.map(str::to_string),
        ..Options::default()
    });
    assert!(feed.sync().ok());
    Setup {
        feed,
        index,
        _raw: raw,
        _dir: dir,
    }
}

fn eval_setup() -> Setup {
    setup(Some(&format!("{SET}:6:8")))
}

fn text(feed: &TestFeed, sql: &str) -> String {
    feed.conn().query_row(sql, [], |r| r.get(0)).expect("text")
}

fn count(feed: &TestFeed, sql: &str) -> i64 {
    feed.conn().query_row(sql, [], |r| r.get(0)).expect("count")
}

fn post_id(feed: &TestFeed, pack: &str) -> i64 {
    count(feed, &format!("SELECT id FROM posts WHERE pack = '{pack}'"))
}

fn hash_of(feed: &TestFeed, pack: &str) -> String {
    text(
        feed,
        &format!("SELECT pack_hash FROM packs WHERE name = '{pack}'"),
    )
}

/// Signs `login` in and loads a page, so the client has a CSRF token.
fn signed_in(feed: &TestFeed, login: &str) -> TestClient {
    let mut client = feed.client();
    client.sign_in(login);
    client.get("/");
    client
}

type Fields = Vec<(&'static str, String)>;

fn values(hash: &str, counts: [i64; 4]) -> Fields {
    vec![
        ("pack_hash", hash.to_string()),
        ("eval_set", SET.to_string()),
        ("hippocamp", "0.1.0".to_string()),
        ("model", "claude".to_string()),
        ("runs", "3".to_string()),
        ("corrected_without", counts[0].to_string()),
        ("damaged_without", counts[1].to_string()),
        ("corrected_with", counts[2].to_string()),
        ("damaged_with", counts[3].to_string()),
        ("report_url", String::new()),
    ]
}

fn send(client: &mut TestClient, post: i64, fields: &[(&str, String)]) -> Reply {
    let borrowed: Vec<(&str, &str)> = fields.iter().map(|(k, v)| (*k, v.as_str())).collect();
    client.post_form(&format!("/p/{post}/measure"), &borrowed)
}

fn measure(client: &mut TestClient, post: i64, hash: &str, counts: [i64; 4]) -> Reply {
    send(client, post, &values(hash, counts))
}

/// The message of a refused entry: 422 and the rule, with the entered values
/// shown again.
fn refusal(reply: &Reply) -> String {
    assert_eq!(reply.status, 422, "{}", reply.body);
    let start = reply
        .body
        .find(r#"<p class="hp-error">"#)
        .expect("an error");
    let rest = &reply.body[start + r#"<p class="hp-error">"#.len()..];
    rest[..rest.find("</p>").expect("end")].to_string()
}

fn after_three_days(feed: &TestFeed) {
    feed.clock.advance(Duration::days(3) + Duration::hours(1));
}

fn within(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 1e-6,
        "{actual} is not {expected}"
    );
}

#[test]
fn goal_3_measurements_rank_a_pack_on_the_evidence_page_and_the_front_page() {
    let s = eval_setup();
    let feed = &s.feed;
    let mut ada = signed_in(feed, "ada");
    let mut grace = signed_in(feed, "grace");
    let mut linus = signed_in(feed, "linus");
    let mut alan = signed_in(feed, "alan");
    let mut newbie = signed_in(feed, "newbie");
    // Established once 3 days have passed; 73 hours after the sync, which
    // created every pack's post, so `h` below is 73.
    after_three_days(feed);

    let glossary = post_id(feed, "glossary-terms");
    let decision = post_id(feed, "decision-log");
    let hash = hash_of(feed, "glossary-terms");

    // grace: dc 4, dd 0       -> 32/144 = 0.2222
    // linus: dc 3, dd 1       -> (24 - 6)/144 = 0.125
    // alan:  dc 5, dd -1      -> (40 + 6)/144 = 0.3194
    // ada (a maintainer): dc 6, dd 0 -> 48/144 = 0.3333, not counted
    for (client, counts) in [
        (&mut grace, [6, 2, 10, 2]),
        (&mut linus, [5, 1, 8, 2]),
        (&mut alan, [7, 2, 12, 1]),
        (&mut ada, [6, 2, 12, 2]),
    ] {
        let r = measure(client, glossary, &hash, counts);
        assert_eq!(r.status, 303, "{}", r.body);
    }
    // A new member's entry is refused through the page...
    let r = measure(&mut newbie, glossary, &hash, [6, 2, 12, 2]);
    assert_eq!(
        refusal(&r),
        "member: must be an established account to measure"
    );
    // ...so the measurement they need is inserted with SQL.
    feed.conn()
        .execute(
            "INSERT INTO measurements (pack, pack_hash, user_id, eval_set, planted, kept, runs,
                 corrected_without, damaged_without, corrected_with, damaged_with, hippocamp, model,
                 created_at, updated_at)
             SELECT 'glossary-terms', ?1, id, ?2, 6, 8, 3, 0, 0, 18, 0, '0.1.0', 'claude',
                    '2026-11-11T10:00:00.000000Z', '2026-11-11T10:00:00.000000Z'
             FROM users WHERE login = 'newbie'",
            [&hash, SET],
        )
        .expect("newbie's measurement");

    // Votes: grace and linus on decision-log, alan on glossary-terms.
    for (client, post) in [
        (&mut grace, decision),
        (&mut linus, decision),
        (&mut alan, glossary),
    ] {
        let r = client.post_form(
            "/vote",
            &[
                ("target_type", "post"),
                ("target_id", &post.to_string()),
                ("return_to", "/"),
            ],
        );
        assert_eq!(r.status, 303, "{}", r.body);
    }

    // By hand: n = 3, median 2/9, E = 2/9 x 3/5 = 0.1333.
    let now = feed.clock_now();
    let conn = feed.conn();
    let by_evidence = rank::by_evidence(&conn, None, now, 1).expect("evidence");
    let first = &by_evidence.entries[0];
    assert_eq!(first.rec.pack.as_deref(), Some("glossary-terms"));
    let found = first
        .summary
        .as_ref()
        .and_then(|s| s.evidence.as_ref())
        .expect("evidence");
    assert_eq!(found.evidence.n, 3);
    within(found.evidence.score, 0.133_333_333);
    within(found.evidence.median.to_f64(), 2.0 / 9.0);

    // rank = (V + 50 E)/(h + 2)^1.8 with V = 1, E = 0.13333, h = 73:
    // 7.66667 / 75^1.8 = 7.66667 / 2371.976 = 0.0032322; decision-log:
    // 2 / 2371.976 = 0.00084318; the rest 0.
    let front = rank::front(&conn, None, now, 1).expect("front");
    let order: Vec<&str> = front
        .entries
        .iter()
        .map(|e| e.rec.pack.as_deref().unwrap_or("?"))
        .collect();
    assert_eq!(
        order,
        [
            "glossary-terms",
            "decision-log",
            "handoff-check",
            "stale-links",
            "review-dates"
        ]
    );
    within(front.entries[0].rank(now), 0.003_232_186);
    within(front.entries[1].rank(now), 0.000_843_179);
    assert_eq!(front.entries[2].rank(now), 0.0);
    drop(conn);

    // The pages show the same orders and the evidence of the row.
    let mut visitor = feed.client();
    let page = visitor.get("/");
    let at = |name: &str| page.body.find(&format!("/p/{}\"", post_id(feed, name)));
    let positions: Vec<usize> = ["glossary-terms", "decision-log", "handoff-check"]
        .iter()
        .map(|n| at(n).unwrap_or_else(|| panic!("{n} on /")))
        .collect();
    assert!(positions.windows(2).all(|w| w[0] < w[1]), "{positions:?}");
    assert!(page
        .body
        .contains(r#"<span class="hp-evidence">+0.13 evidence (3)</span>"#));
    assert!(page.body.contains("1 point"));

    // /evidence: glossary-terms above zero; then no evidence by points (2
    // for decision-log), then by name.
    let page = visitor.get("/evidence");
    let names = [
        "glossary-terms",
        "decision-log",
        "handoff-check",
        "review-dates",
        "stale-links",
    ];
    let positions: Vec<usize> = names
        .iter()
        .map(|n| {
            page.body
                .find(&format!("/p/{}\"", post_id(feed, n)))
                .unwrap_or_else(|| panic!("{n} on /evidence"))
        })
        .collect();
    assert!(positions.windows(2).all(|w| w[0] < w[1]), "{positions:?}");
    assert!(page.body.contains("+0.13 evidence (3)"));
    assert!(page.body.contains("0 of 3 measurements"));

    // The pack page: evidence to three decimals with its count, each
    // measurement with its status and an anchor.
    let page = visitor.get(&format!("/p/{glossary}"));
    assert!(page
        .body
        .contains(r#"<span class="hp-evidence">+0.133 (3)</span>"#));
    let status_of = |login: &str| -> String {
        let id = count(
            feed,
            &format!(
                "SELECT m.id FROM measurements m JOIN users u ON u.id = m.user_id
                 WHERE u.login = '{login}'"
            ),
        );
        let row_start = page
            .body
            .find(&format!(r#"<tr id="m{id}">"#))
            .unwrap_or_else(|| panic!("an anchor m{id}"));
        let row = &page.body[row_start..];
        let row = &row[..row.find("</tr>").expect("row end")];
        let cells: Vec<&str> = row.split("<td>").collect();
        // member, date, set, hippocamp, model, runs, two rates, benefit, revisions,
        // status, report.
        assert!(cells[1].starts_with(login), "{row}");
        cells[11].trim_end_matches("</td>").to_string()
    };
    assert_eq!(status_of("grace"), "counted");
    assert_eq!(status_of("linus"), "counted");
    assert_eq!(status_of("alan"), "counted");
    assert_eq!(status_of("ada"), "maintainer's");
    assert_eq!(status_of("newbie"), "not counted");

    // A newer content that has no measurements: the pack's evidence is
    // labelled with the version it comes from.
    let text_index = std::fs::read_to_string(&s.index).expect("read");
    let mut json: serde_json::Value = serde_json::from_str(&text_index).expect("json");
    for pack in json["packs"].as_array_mut().expect("packs") {
        if pack["name"] == "glossary-terms" {
            pack["version"] = "1.1.0".into();
            pack["pack_hash"] = "9".repeat(64).into();
        }
    }
    std::fs::write(&s.index, json.to_string()).expect("write");
    feed.clock.advance(Duration::minutes(5));
    assert!(feed.sync().ok());
    let page = visitor.get(&format!("/p/{glossary}"));
    assert!(page.body.contains("evidence from 1.0.0"), "{}", page.body);
    assert!(page.body.contains("0 of 3 measurements"));
}

#[test]
fn the_measure_page_explains_and_lists_what_can_be_measured() {
    let s = eval_setup();
    let feed = &s.feed;
    let glossary = post_id(feed, "glossary-terms");

    // A visitor is sent to sign in; the pack's page links the measure page.
    let mut visitor = feed.client();
    let r = visitor.get(&format!("/p/{glossary}/measure"));
    assert_eq!(r.status, 303);
    assert!(r
        .header("location")
        .is_some_and(|l| l.starts_with("/login")));
    let page = visitor.get(&format!("/p/{glossary}"));
    assert!(page.body.contains(&format!(
        r#"<a href="/p/{glossary}/measure" rel="nofollow">Measure this pack</a>"#
    )));

    let mut grace = signed_in(feed, "grace");
    let page = grace.get(&format!("/p/{glossary}/measure"));
    assert_eq!(page.status, 200);
    assert!(page.body.contains("<h1>Measure</h1>"));
    assert!(page
        .body
        .contains("3 times without the pack and 3 times with it"));
    assert!(page.body.contains("Measurements are public"));
    // The eval set with its planted and kept counts, and the content.
    assert!(page.body.contains(&format!("<code>{SET}</code>")));
    assert!(page.body.contains("<td>6</td><td>8</td>"));
    assert!(page.body.contains(&format!(
        r#"<option value="{}" selected>"#,
        hash_of(feed, "glossary-terms")
    )));
    // No form on a discussion or a missing post.
    assert_eq!(grace.get("/p/9999/measure").status, 404);
    assert_eq!(grace.get("/p/x/measure").status, 404);
    // A theme pack's page says it cannot be measured, and its POST is refused.
    let theme = post_id(feed, "night-ink");
    let page = grace.get(&format!("/p/{theme}/measure"));
    assert_eq!(page.status, 200);
    assert!(page.body.contains("cannot be measured"));
    assert!(!page.body.contains(r#"name="runs""#));
    let r = measure(
        &mut grace,
        theme,
        &hash_of(feed, "night-ink"),
        [6, 2, 12, 2],
    );
    assert!(refusal(&r).starts_with("pack: must be a listed pack of passes"));
}

#[test]
fn measuring_is_not_open_without_eval_sets() {
    let s = setup(None);
    let feed = &s.feed;
    let glossary = post_id(feed, "glossary-terms");
    let mut grace = signed_in(feed, "grace");
    let page = grace.get(&format!("/p/{glossary}/measure"));
    assert_eq!(page.status, 200);
    assert!(page.body.contains("Measuring is not open yet"));
    assert!(!page.body.contains(r#"name="runs""#));
    let r = measure(
        &mut grace,
        glossary,
        &hash_of(feed, "glossary-terms"),
        [6, 2, 12, 2],
    );
    assert_eq!(refusal(&r), "eval_set: measuring is not open");
    assert_eq!(count(feed, "SELECT COUNT(*) FROM measurements"), 0);
}

#[test]
fn every_refusal_is_shown_again_with_the_values_and_the_rule() {
    let s = eval_setup();
    let feed = &s.feed;
    let mut grace = signed_in(feed, "grace");
    after_three_days(feed);
    let glossary = post_id(feed, "glossary-terms");
    let hash = hash_of(feed, "glossary-terms");

    let refused = |client: &mut TestClient, change: &dyn Fn(&mut Fields)| {
        let mut fields = values(&hash, [6, 2, 12, 2]);
        change(&mut fields);
        let r = send(client, glossary, &fields);
        (refusal(&r), r)
    };
    let set = |fields: &mut Fields, name: &str, value: &str| {
        for f in fields.iter_mut() {
            if f.0 == name {
                f.1 = value.to_string();
            }
        }
    };

    let (rule, r) = refused(&mut grace, &|f| set(f, "pack_hash", &"f".repeat(64)));
    assert_eq!(rule, "pack_hash: must be a recorded version of this pack");
    assert!(r.body.contains("corrected_with"), "the form is shown again");
    let (rule, _) = refused(&mut grace, &|f| set(f, "eval_set", "ffffffffffff"));
    assert_eq!(rule, "eval_set: must be one of the listed eval sets");
    let (rule, r) = refused(&mut grace, &|f| set(f, "runs", "2"));
    assert_eq!(rule, "runs: must be between 3 and 10");
    // The entered values come back.
    assert!(r
        .body
        .contains(r#"id="runs" name="runs" type="text" inputmode="numeric" value="2""#));
    assert!(r.body.contains(r#"value="claude""#));
    let (rule, _) = refused(&mut grace, &|f| set(f, "runs", "11"));
    assert_eq!(rule, "runs: must be between 3 and 10");
    let (rule, _) = refused(&mut grace, &|f| set(f, "runs", "three"));
    assert_eq!(rule, "runs: must be a whole number");
    let (rule, _) = refused(&mut grace, &|f| set(f, "runs", ""));
    assert_eq!(rule, "runs: is required");
    // 6 planted x 3 runs = 18; 8 kept x 3 runs = 24.
    let (rule, _) = refused(&mut grace, &|f| set(f, "corrected_without", "19"));
    assert_eq!(rule, "corrected_without: must be between 0 and 18");
    let (rule, _) = refused(&mut grace, &|f| set(f, "corrected_with", "-1"));
    assert_eq!(rule, "corrected_with: must be between 0 and 18");
    let (rule, _) = refused(&mut grace, &|f| set(f, "damaged_without", "25"));
    assert_eq!(rule, "damaged_without: must be between 0 and 24");
    let (rule, _) = refused(&mut grace, &|f| set(f, "damaged_with", "25"));
    assert_eq!(rule, "damaged_with: must be between 0 and 24");
    // With 10 runs the limits grow: 60 and 80.
    let (rule, _) = refused(&mut grace, &|f| {
        set(f, "runs", "10");
        set(f, "corrected_with", "61");
    });
    assert_eq!(rule, "corrected_with: must be between 0 and 60");
    let (rule, _) = refused(&mut grace, &|f| set(f, "hippocamp", "-1"));
    assert!(rule.starts_with("hippocamp: must be a version"), "{rule}");
    let (rule, _) = refused(&mut grace, &|f| set(f, "model", &"m".repeat(201)));
    assert!(rule.starts_with("model:"), "{rule}");
    let (rule, _) = refused(&mut grace, &|f| {
        set(f, "report_url", "http://example.com/r")
    });
    assert_eq!(rule, "report_url: must start with https://");
    let (rule, _) = refused(&mut grace, &|f| {
        set(
            f,
            "report_url",
            &format!("https://example.com/{}", "r".repeat(300)),
        )
    });
    assert_eq!(rule, "report_url: must be at most 300 bytes");
    assert_eq!(count(feed, "SELECT COUNT(*) FROM measurements"), 0);

    // Not established: the account is made recent again.
    feed.conn()
        .execute(
            "UPDATE users SET created_at = ?1 WHERE login = 'grace'",
            [hippocamp_feed::clock::storage(
                feed.clock_now() - Duration::days(1),
            )],
        )
        .expect("recent");
    let r = measure(&mut grace, glossary, &hash, [6, 2, 12, 2]);
    assert_eq!(
        refusal(&r),
        "member: must be an established account to measure"
    );

    // A POST without a session, or without the token, is refused as for
    // every form.
    let mut visitor = feed.client();
    let r = visitor.post_form(&format!("/p/{glossary}/measure"), &[]);
    assert_eq!(r.status, 403);
    assert_eq!(count(feed, "SELECT COUNT(*) FROM measurements"), 0);
}

#[test]
fn a_replacement_keeps_its_id_and_first_time_and_an_invalid_one_cannot_be_replaced() {
    let s = eval_setup();
    let feed = &s.feed;
    let mut grace = signed_in(feed, "grace");
    after_three_days(feed);
    let glossary = post_id(feed, "glossary-terms");
    let hash = hash_of(feed, "glossary-terms");

    let r = measure(&mut grace, glossary, &hash, [6, 2, 10, 2]);
    assert_eq!(
        r.header("location"),
        Some(format!("/p/{glossary}#m1").as_str())
    );
    let first_time = text(feed, "SELECT created_at FROM measurements WHERE id = 1");
    assert_eq!(
        count(feed, "SELECT revisions FROM measurements WHERE id = 1"),
        0
    );

    feed.clock.advance(Duration::minutes(10));
    let r = measure(&mut grace, glossary, &hash, [6, 2, 14, 1]);
    assert_eq!(
        r.header("location"),
        Some(format!("/p/{glossary}#m1").as_str())
    );
    assert_eq!(count(feed, "SELECT COUNT(*) FROM measurements"), 1);
    assert_eq!(
        count(feed, "SELECT revisions FROM measurements WHERE id = 1"),
        1
    );
    assert_eq!(
        count(feed, "SELECT corrected_with FROM measurements WHERE id = 1"),
        14
    );
    assert_eq!(
        text(feed, "SELECT created_at FROM measurements WHERE id = 1"),
        first_time
    );
    assert_ne!(
        text(feed, "SELECT updated_at FROM measurements WHERE id = 1"),
        first_time
    );
    // The page shows the revision count.
    let page = feed.client().get(&format!("/p/{glossary}"));
    let row = &page.body[page.body.find(r#"<tr id="m1">"#).expect("row")..];
    assert!(row[..row.find("</tr>").expect("end")].contains("<td>1</td>"));

    // Marked invalid (by SQL; the moderator's action is tested elsewhere): no
    // replacement, and the form says so.
    feed.conn()
        .execute("UPDATE measurements SET invalid = 1 WHERE id = 1", [])
        .expect("invalid");
    let r = measure(&mut grace, glossary, &hash, [6, 2, 10, 2]);
    assert_eq!(
        refusal(&r),
        "pack_hash: your measurement of this version was marked invalid and cannot be replaced"
    );
    assert_eq!(
        count(feed, "SELECT corrected_with FROM measurements WHERE id = 1"),
        14
    );
}

#[test]
fn a_large_benefit_files_one_report_and_replacing_files_no_second() {
    let s = eval_setup();
    let feed = &s.feed;
    let mut grace = signed_in(feed, "grace");
    let mut linus = signed_in(feed, "linus");
    after_three_days(feed);
    let glossary = post_id(feed, "glossary-terms");
    let hash = hash_of(feed, "glossary-terms");
    let system_reports = "SELECT COUNT(*) FROM reports
        WHERE system = 1 AND reason = 'misleading' AND target_type = 'measurement'";
    let flags = "SELECT COUNT(*) FROM audit_log WHERE action = 'evidence.flag'";

    // 48/144 = 0.333 is below 0.5 and files nothing.
    measure(&mut linus, glossary, &hash, [6, 2, 12, 2]);
    assert_eq!(count(feed, system_reports), 0);

    // dc 18 -> 144/144 = 1.0, a large benefit.
    let r = measure(&mut grace, glossary, &hash, [0, 0, 18, 0]);
    assert_eq!(r.status, 303);
    assert_eq!(count(feed, system_reports), 1);
    assert_eq!(count(feed, flags), 1);
    // Replacing it with another large benefit (in the other direction) files
    // no second report while the first is open.
    feed.clock.advance(Duration::minutes(1));
    let r = measure(&mut grace, glossary, &hash, [18, 0, 0, 0]);
    assert_eq!(r.status, 303);
    assert_eq!(count(feed, "SELECT revisions FROM measurements WHERE user_id = (SELECT id FROM users WHERE login = 'grace')"), 1);
    assert_eq!(count(feed, system_reports), 1);
    assert_eq!(count(feed, flags), 1);
}

#[test]
fn the_per_member_limit_answers_429() {
    let s = eval_setup();
    let feed = &s.feed;
    let mut grace = signed_in(feed, "grace");
    after_three_days(feed);
    let glossary = post_id(feed, "glossary-terms");
    // The limit counts rows by their last change, so 20 entries need 20
    // contents; 19 more are recorded with SQL (the sync would record them).
    let mut hashes = vec![hash_of(feed, "glossary-terms")];
    for i in 1..20 {
        let hash = format!("{i:064x}");
        feed.conn()
            .execute(
                "INSERT INTO pack_contents (pack, pack_hash, version, commit_id, first_seen)
                 VALUES ('glossary-terms', ?1, '0.0.1', ?2, '2026-11-08T10:00:00.000000Z')",
                [&hash, &"a".repeat(40)],
            )
            .expect("content");
        hashes.push(hash);
    }
    // 20 entries in 24 hours are allowed.
    for (i, hash) in hashes.iter().enumerate() {
        let r = measure(&mut grace, glossary, hash, [6, 2, 10, 2]);
        assert_eq!(r.status, 303, "entry {i}: {}", r.body);
    }
    let r = measure(&mut grace, glossary, &hashes[0], [6, 2, 11, 2]);
    assert_eq!(r.status, 429, "{}", r.body);
    assert!(r.header("retry-after").is_some());
    assert_eq!(count(feed, "SELECT COUNT(*) FROM measurements"), 20);
    // After the window the member can enter again.
    feed.clock.advance(Duration::hours(25));
    let r = measure(&mut grace, glossary, &hashes[0], [6, 2, 11, 2]);
    assert_eq!(r.status, 303);
}
