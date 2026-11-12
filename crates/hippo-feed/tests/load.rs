//! The load test of the performance requirement:
//! 10,000 posts in the last 30 days, 100,000
//! votes, 2,000 members and 1,000 measurements, then the median of 20 warm
//! front-page requests (signed out, and signed in as a member with karma,
//! hides and 10 KiB of personal CSS) and the throughput and 95th percentile of
//! front-page requests from 8 concurrent clients.
//!
//! Marked `#[ignore]` so CI skips it. Run it by hand, on the
//! target VPS size:
//!
//! `cargo test -p hippo-feed --release --test load -- --ignored --nocapture`
//!
//! The targets: a median under 50 ms; at least 20
//! requests per second with a 95th percentile under 200 ms. The server runs
//! with `FEED_TRUST_PROXY=1`, the real per-address quotas and a different
//! `X-Forwarded-For` address per client, so the `all` limiter counts each
//! client apart; each client sends fewer requests than its quota's burst.

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::{Options, RawServer, TestFeed};
use hippo_feed::clock;
use hippo_feed::web::Quotas;
use rusqlite::params;

const POSTS: i64 = 10_000;
const MEMBERS: i64 = 2_000;
const VOTES_PER_POST: i64 = 10;
const MEASUREMENTS: i64 = 1_000;
const CLIENTS: usize = 8;
const REQUESTS_PER_CLIENT: usize = 100;

const MEDIAN_TARGET_MS: f64 = 50.0;
const THROUGHPUT_TARGET: f64 = 20.0;
const P95_TARGET_MS: f64 = 200.0;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/feed")
}

/// Rows are inserted with SQL, in one transaction, in a fixed layout.
fn build(feed: &TestFeed, viewer: i64) {
    let now = feed.clock_now();
    let mut conn = feed.conn();
    let tx = conn.transaction().expect("tx");
    let joined = clock::storage(now - time::Duration::days(60));
    for n in 0..MEMBERS {
        tx.execute(
            "INSERT INTO users (id, github_id, login, github_created_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?4)",
            params![1000 + n, 50_000 + n, format!("load-{n}"), joined],
        )
        .expect("member");
    }
    // The signed-in viewer has karma (the first 200 posts are theirs), 300
    // hides and 10 KiB of personal CSS.
    let author = |p: i64| -> i64 {
        if p < 200 {
            viewer
        } else {
            1000 + (p * 13 + 5) % MEMBERS
        }
    };
    {
        let mut post = tx
            .prepare(
                "INSERT INTO posts (id, kind, author_id, title, link, body, created_at)
                 VALUES (?1, 'discussion', ?2, ?3, ?4, '', ?5)",
            )
            .expect("prepare");
        let month = 30 * 24 * 60;
        for p in 0..POSTS {
            let age = (p * 2593) % (month - 60) + 5;
            let at = clock::storage(now - time::Duration::minutes(age));
            post.execute(params![
                100_000 + p,
                author(p),
                format!("Load post {p}"),
                format!("https://example.com/load/{p}"),
                at
            ])
            .expect("post");
        }
        let mut vote = tx
            .prepare(
                "INSERT INTO votes (user_id, target_type, target_id, at)
                 VALUES (?1, 'post', ?2, ?3)",
            )
            .expect("prepare");
        let at = clock::storage(now - time::Duration::minutes(4));
        for p in 0..POSTS {
            for j in 0..VOTES_PER_POST {
                let mut voter = 1000 + (p * 7 + j * 197) % MEMBERS;
                if voter == author(p) {
                    voter = 1000 + (voter - 1000 + 1) % MEMBERS;
                }
                vote.execute(params![voter, 100_000 + p, at]).expect("vote");
            }
        }
        let mut hide = tx
            .prepare("INSERT INTO hides (user_id, post_id, at) VALUES (?1, ?2, ?3)")
            .expect("prepare");
        for p in 200..500 {
            hide.execute(params![viewer, 100_000 + p, at])
                .expect("hide");
        }
    }
    // 10 KiB of valid personal CSS: comment-free rules.
    let mut css = String::new();
    while css.len() < 10 * 1024 - 64 {
        css.push_str(".hp-row{margin:0}\n");
    }
    tx.execute(
        "UPDATE users SET personal_css = ?1, theme = 'dark' WHERE id = ?2",
        params![css, viewer],
    )
    .expect("css");
    // Measurements of the synced packs' contents, one member each per content.
    let contents: Vec<(String, String)> = {
        let mut stmt = tx
            .prepare(
                "SELECT c.pack, c.pack_hash FROM pack_contents c JOIN packs p ON p.name = c.pack
                 WHERE p.kind = 'passes' AND p.listed = 1 ORDER BY c.pack, c.pack_hash",
            )
            .expect("prepare");
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("contents")
    };
    assert!(!contents.is_empty(), "the fixture lists no pack of passes");
    let c = contents.len() as i64;
    let at = clock::storage(now - time::Duration::days(3));
    for m in 0..MEASUREMENTS {
        let (pack, hash) = &contents[(m % c) as usize];
        let user = 1000 + ((m / c) * 37 + (m % c) * 11) % MEMBERS;
        tx.execute(
            "INSERT INTO measurements (pack, pack_hash, user_id, eval_set, planted, kept, runs,
                 corrected_without, damaged_without, corrected_with, damaged_with, hippo, model,
                 created_at, updated_at)
             VALUES (?1, ?2, ?3, '000000000000', 7, 8, 3, ?4, 2, ?5, 2, '0.9.0', 'load-model',
                 ?6, ?6)",
            params![pack, hash, user, 6 + m % 5, 9 + m % 7, at],
        )
        .expect("measurement");
    }
    tx.commit().expect("commit");
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .max_redirects(0)
        .build()
        .into()
}

/// One front-page request; returns how long it took. Asserts a 200.
fn timed(agent: &ureq::Agent, url: &str, headers: &[(&str, &str)]) -> Duration {
    let mut request = agent.get(url);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let start = Instant::now();
    let mut response = request.call().expect("request");
    let status = response.status().as_u16();
    let _ = response.body_mut().read_to_string().expect("body");
    let took = start.elapsed();
    assert_eq!(status, 200, "GET / answered {status}");
    took
}

fn median_ms(url: &str, headers: &[(&str, &str)]) -> f64 {
    let agent = agent();
    for _ in 0..3 {
        timed(&agent, url, headers);
    }
    let mut times: Vec<f64> = (0..20)
        .map(|_| timed(&agent, url, headers).as_secs_f64() * 1000.0)
        .collect();
    times.sort_by(f64::total_cmp);
    (times[9] + times[10]) / 2.0
}

#[test]
#[ignore = "builds a large database; run by hand with --release"]
fn the_front_page_meets_the_performance_targets() {
    let raw = RawServer::start_default();
    let dir = tempfile::tempdir().expect("tempdir");
    let index = dir.path().join("index.json");
    std::fs::copy(fixtures().join("registry-index.json"), &index).expect("copy");
    let feed = TestFeed::start(Options {
        registry_index: Some(index.display().to_string()),
        raw_url: Some(raw.url.clone()),
        eval_sets: Some("000000000000:7:8".to_string()),
        trust_proxy: true,
        quotas: Quotas::default(),
        ..Options::default()
    });
    assert!(feed.sync().ok());

    // Sign in once as a mock user to get a session, then fill the database.
    let mut client = feed.client();
    client.sign_in("grace");
    let session = client
        .cookie("hippo_session")
        .expect("the session cookie (http)");
    let viewer: i64 = feed
        .conn()
        .query_row("SELECT id FROM users WHERE login = 'grace'", [], |r| {
            r.get(0)
        })
        .expect("viewer");
    let started = Instant::now();
    build(&feed, viewer);
    eprintln!("built the data in {:.1} s", started.elapsed().as_secs_f64());

    let url = format!("{}/", feed.base_url);
    let cookie = format!("hippo_session={session}");

    let signed_out = median_ms(&url, &[("X-Forwarded-For", "203.0.113.1")]);
    let signed_in = median_ms(
        &url,
        &[("X-Forwarded-For", "203.0.113.2"), ("Cookie", &cookie)],
    );
    eprintln!("median of 20, signed out: {signed_out:.1} ms");
    eprintln!("median of 20, signed in:  {signed_in:.1} ms");

    // 8 clients, each from its own address, each under its quota's burst.
    let began = Instant::now();
    let handles: Vec<_> = (0..CLIENTS)
        .map(|n| {
            let url = url.clone();
            std::thread::spawn(move || {
                let address = format!("198.51.100.{}", n + 10);
                let agent = agent();
                (0..REQUESTS_PER_CLIENT)
                    .map(|_| {
                        timed(&agent, &url, &[("X-Forwarded-For", address.as_str())]).as_secs_f64()
                            * 1000.0
                    })
                    .collect::<Vec<f64>>()
            })
        })
        .collect();
    let mut times: Vec<f64> = handles
        .into_iter()
        .flat_map(|h| h.join().expect("client"))
        .collect();
    let wall = began.elapsed().as_secs_f64();
    times.sort_by(f64::total_cmp);
    let throughput = times.len() as f64 / wall;
    let p95 = times[(times.len() * 95).div_ceil(100) - 1];
    eprintln!("{CLIENTS} clients: {throughput:.1} requests/s, 95th percentile {p95:.1} ms");

    assert!(
        signed_out < MEDIAN_TARGET_MS,
        "signed-out median {signed_out:.1} ms, target {MEDIAN_TARGET_MS} ms"
    );
    assert!(
        signed_in < MEDIAN_TARGET_MS,
        "signed-in median {signed_in:.1} ms, target {MEDIAN_TARGET_MS} ms"
    );
    assert!(
        throughput >= THROUGHPUT_TARGET,
        "{throughput:.1} requests/s, target {THROUGHPUT_TARGET}"
    );
    assert!(
        p95 < P95_TARGET_MS,
        "p95 {p95:.1} ms, target {P95_TARGET_MS} ms"
    );
}
