//! The foundation: health, headers, the page frame, static files, per-address
//! limits, database wait and backups, through a real server.

mod common;

use std::time::Duration;

use common::{Answer, Options, RawServer, TestFeed};
use hippocamp_feed::backup;
use hippocamp_feed::clock::{self, Clock};
use hippocamp_feed::policy::AddressQuota;
use hippocamp_feed::web::Quotas;
use regex::Regex;

const CSP: &str = "default-src 'none'; script-src 'none'; style-src 'self'; img-src 'self' data:; \
font-src 'self' data:; connect-src 'none'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'";

fn json(body: &str) -> serde_json::Value {
    serde_json::from_str(body).expect("json")
}

fn quota(count: u32) -> AddressQuota {
    AddressQuota {
        count,
        per: Duration::from_secs(60),
    }
}

fn only_all(count: u32) -> Options {
    let mut options = Options::default();
    options.quotas = Quotas {
        all: quota(count),
        ..options.quotas
    };
    options
}

#[test]
fn healthz_answers_and_lists_backup_once_it_is_stale() {
    let feed = TestFeed::start(Options::default());
    let mut client = feed.client();
    let reply = client.get("/healthz");
    assert_eq!(reply.status, 200);
    assert_eq!(reply.header("content-type"), Some("application/json"));
    assert!(reply.body.contains(r#""stale":[]"#), "{}", reply.body);
    let health = json(&reply.body);
    assert_eq!(health["ok"], true);
    assert_eq!(health["db_version"], 1);
    for null in [
        "last_sync",
        "last_sync_ok",
        "last_successful_sync",
        "last_backup",
        "oldest_open_report",
    ] {
        assert!(health[null].is_null(), "{null}");
    }

    // 36 hours is not over; past it, the backup is stale.
    feed.clock.advance(time::Duration::hours(36));
    assert!(client.get("/healthz").body.contains(r#""stale":[]"#));
    feed.clock.advance(time::Duration::hours(1));
    let reply = client.get("/healthz");
    assert_eq!(reply.status, 200);
    assert_eq!(json(&reply.body)["stale"], serde_json::json!(["backup"]));

    // A backup clears it, and /healthz shows when it was taken in RFC 3339.
    let dir = tempfile::tempdir().expect("tempdir");
    backup::backup(&feed.db_path, dir.path(), 14, feed.clock.now()).expect("backup");
    let health = json(&client.get("/healthz").body);
    assert_eq!(health["stale"], serde_json::json!([]));
    assert_eq!(health["last_backup"], clock::rfc3339(feed.clock.now()));
}

#[test]
fn healthz_is_503_when_the_database_is_not_at_the_binarys_version() {
    let feed = TestFeed::start(Options::default());
    let conn = rusqlite::Connection::open(&feed.db_path).expect("open");
    conn.pragma_update(None, "user_version", 0)
        .expect("downgrade");
    let mut client = feed.client();
    let reply = client.get("/healthz");
    assert_eq!(reply.status, 503);
    let health = json(&reply.body);
    assert_eq!(health["ok"], false);
    assert_eq!(health["db_version"], 0);
}

#[test]
fn every_response_carries_the_security_headers_and_robots_lists_the_paths() {
    let feed = TestFeed::start(Options::default());
    let mut client = feed.client();
    for path in [
        "/about",
        "/healthz",
        "/robots.txt",
        "/static/base.css",
        "/static/icon.svg",
        "/nothing-here",
        "/api/v1/nothing",
    ] {
        let reply = client.get(path);
        assert_eq!(reply.header("content-security-policy"), Some(CSP), "{path}");
        assert_eq!(
            reply.header("x-content-type-options"),
            Some("nosniff"),
            "{path}"
        );
        assert_eq!(
            reply.header("referrer-policy"),
            Some("same-origin"),
            "{path}"
        );
        // The base URL is http here, so there is no HSTS.
        assert_eq!(reply.header("strict-transport-security"), None, "{path}");
        assert!(!reply.body.contains("<script"), "{path}");
    }

    let robots = client.get("/robots.txt");
    assert_eq!(robots.status, 200);
    assert!(robots
        .header("content-type")
        .is_some_and(|v| v.starts_with("text/plain")));
    for path in [
        "/login",
        "/auth/",
        "/submit",
        "/report",
        "/settings",
        "/hide",
        "/hidden",
        "/u/",
        "/mod",
        "/admin",
        "/c/",
        "/p/*/",
    ] {
        assert!(
            robots.body.contains(&format!("Disallow: {path}\n")),
            "{path}: {}",
            robots.body
        );
    }
    assert!(robots.body.starts_with("User-agent: *\n"));
}

#[test]
fn about_has_the_top_bar_the_stylesheets_in_order_and_marks_no_link() {
    let feed = TestFeed::start(Options {
        contact: Some("hello@feed.example".to_string()),
        ..Options::default()
    });
    let mut client = feed.client();
    let reply = client.get("/about");
    assert_eq!(reply.status, 200);
    assert_eq!(reply.header("cache-control"), Some("private, no-store"));
    assert!(reply.is_html());
    let html = &reply.body;

    let base = html.find("/static/base.css?v=").expect("base.css");
    let auto = html.find("/static/themes/auto.css?v=").expect("auto.css");
    assert!(base < auto);
    assert!(!html.contains("themes/dark.css"));
    let hashes =
        Regex::new(r#"href="/static/(?:base|themes/auto)\.css\?v=([0-9a-f]{12})""#).expect("regex");
    assert_eq!(hashes.captures_iter(html).count(), 2);

    // The top bar: the mark and the name, every navigation link, login.
    assert!(
        html.contains(r#"<a class="hp-home" href="/"><svg"#),
        "{html}"
    );
    assert!(html.contains(r#"<span class="hp-name">hippocamp</span>"#));
    for link in [
        "new", "past", "comments", "ask", "show", "packs", "evidence", "submit",
    ] {
        assert!(
            html.contains(&format!(r#"<a href="/{link}">{link}</a>"#)),
            "{link}"
        );
    }
    assert!(
        html.contains(r#"<a href="/login?return_to=%2Fabout" rel="nofollow">login</a>"#),
        "{html}"
    );
    assert!(
        !html.contains("hp-nav-current"),
        "no link is marked on /about"
    );
    assert!(html.contains(r#"<body class="hp-page hp-page-about">"#));
    assert!(html.contains("<title>About | hippocamp</title>"));
    assert!(html.contains("<h1>About</h1>"));
    // The footer and the contact.
    let footer = &html[html.find(r#"<footer class="hp-footer">"#).expect("footer")..];
    for link in ["/about", "/themes", "/moderation"] {
        assert!(footer.contains(&format!(r#"href="{link}""#)), "{link}");
    }
    assert!(html.contains("hello@feed.example"));
    // The ranking constants and the licences come from the policy.
    assert!(html.contains("1.8") && html.contains("CC0-1.0") && html.contains("CC BY 4.0"));
}

#[test]
fn the_stylesheets_and_the_icon_are_served_with_their_types_and_caching() {
    let feed = TestFeed::start(Options::default());
    let mut client = feed.client();
    let cases = [
        (
            "/static/base.css",
            "text/css; charset=utf-8",
            hippocamp_theme::BASE_CSS,
        ),
        (
            "/static/themes/dark.css",
            "text/css; charset=utf-8",
            hippocamp_theme::DARK_CSS,
        ),
        (
            "/static/themes/auto.css",
            "text/css; charset=utf-8",
            hippocamp_theme::AUTO_CSS,
        ),
        (
            "/static/icon.svg",
            "image/svg+xml",
            hippocamp_theme::ICON_SVG,
        ),
    ];
    for (path, content_type, body) in cases {
        let reply = client.get(path);
        assert_eq!(reply.status, 200, "{path}");
        assert_eq!(reply.header("content-type"), Some(content_type), "{path}");
        assert_eq!(
            reply.header("x-content-type-options"),
            Some("nosniff"),
            "{path}"
        );
        assert_eq!(
            reply.header("cache-control"),
            Some("public, max-age=31536000, immutable"),
            "{path}"
        );
        assert_eq!(reply.body, body, "{path}");
        // The ?v= value only changes the URL.
        assert_eq!(
            client.get(&format!("{path}?v=whatever")).body,
            body,
            "{path}"
        );
    }
}

#[test]
fn an_error_page_is_a_protected_page_named_error() {
    let feed = TestFeed::start(Options::default());
    let mut client = feed.client();
    let reply = client.get("/nothing-here?safe=1");
    assert_eq!(reply.status, 404);
    assert!(reply.is_html());
    assert_eq!(reply.header("cache-control"), Some("private, no-store"));
    assert!(reply
        .body
        .contains(r#"<body class="hp-page hp-page-error">"#));
    assert!(reply.body.contains("<h1>Not Found</h1>"));
    assert!(reply.body.contains(r#"<p class="hp-error">"#));
    // An error under /api/ is JSON.
    let reply = client.get("/api/v1/nothing");
    assert_eq!(reply.status, 404);
    assert_eq!(reply.header("content-type"), Some("application/json"));
    assert!(json(&reply.body)["error"].is_string());
    // A wrong method gets the same treatment.
    let reply = client.post_form("/about", &[]);
    assert_eq!(reply.status, 405);
    assert!(reply.body.contains("hp-page-error"));
}

#[test]
fn the_all_limiter_counts_pages_not_static_files() {
    let feed = TestFeed::start(only_all(3));
    let mut client = feed.client();
    // Static files do not count, however many.
    for _ in 0..20 {
        assert_eq!(client.get("/static/base.css").status, 200);
    }
    for _ in 0..3 {
        assert_eq!(client.get("/about").status, 200);
    }
    let reply = client.get("/about");
    assert_eq!(reply.status, 429);
    let wait: u64 = reply
        .header("retry-after")
        .expect("Retry-After")
        .parse()
        .expect("seconds");
    assert!((1..=60).contains(&wait), "{wait}");
    assert!(reply.is_html());
    assert!(
        reply.body.contains("3 requests per minute"),
        "{}",
        reply.body
    );
    // Static files still answer while the address is limited.
    assert_eq!(client.get("/static/base.css").status, 200);
}

#[test]
fn the_api_limiter_answers_json_with_retry_after() {
    let mut options = Options::default();
    options.quotas.api = quota(2);
    let feed = TestFeed::start(options);
    let mut client = feed.client();
    // Every API request counts, answered or not.
    assert_eq!(client.get("/api/v1/posts").status, 200);
    assert_eq!(client.get("/api/v1/nothing").status, 404);
    let reply = client.get("/api/v1/posts");
    assert_eq!(reply.status, 429);
    assert_eq!(reply.header("content-type"), Some("application/json"));
    assert!(reply.header("retry-after").is_some());
    assert!(json(&reply.body)["error"]
        .as_str()
        .is_some_and(|m| m.contains("2 requests per minute")));
    // Pages are not API requests.
    assert_eq!(client.get("/about").status, 200);
}

#[test]
fn ipv6_addresses_in_one_slash_64_count_together_behind_a_trusted_proxy() {
    let mut options = only_all(3);
    options.trust_proxy = true;
    let feed = TestFeed::start(options);
    let mut client = feed.client();
    let from = |ip: &'static str| [("X-Forwarded-For", ip)];
    // Two addresses of one /64 share a quota of 3.
    assert_eq!(
        client.get_with("/about", &from("2001:db8:1:2::1")).status,
        200
    );
    assert_eq!(
        client.get_with("/about", &from("2001:db8:1:2::2")).status,
        200
    );
    assert_eq!(
        client
            .get_with("/about", &from("2001:db8:1:2:aaaa::3"))
            .status,
        200
    );
    assert_eq!(
        client.get_with("/about", &from("2001:db8:1:2::4")).status,
        429
    );
    // Another /64, and an IPv4 address, have their own.
    assert_eq!(
        client.get_with("/about", &from("2001:db8:1:3::1")).status,
        200
    );
    assert_eq!(client.get_with("/about", &from("203.0.113.5")).status, 200);
    // Only the last entry counts: a client cannot pick its own address.
    for forged in [
        "198.51.100.1, 203.0.113.99",
        "198.51.100.2, 203.0.113.99",
        "198.51.100.3, 203.0.113.99",
    ] {
        assert_eq!(
            client.get_with("/about", &from(forged)).status,
            200,
            "{forged}"
        );
    }
    assert_eq!(
        client
            .get_with("/about", &from("198.51.100.4, 203.0.113.99"))
            .status,
        429
    );
    // A missing or malformed header falls back to the connection's address.
    assert_eq!(
        client.get_with("/about", &from("not an address")).status,
        200
    );
}

#[test]
fn x_forwarded_for_is_ignored_without_a_trusted_proxy() {
    let feed = TestFeed::start(only_all(3));
    let mut client = feed.client();
    for ip in ["203.0.113.1", "203.0.113.2", "203.0.113.3"] {
        assert_eq!(
            client.get_with("/about", &[("X-Forwarded-For", ip)]).status,
            200
        );
    }
    // A fourth request from a "new" address is still the same client.
    assert_eq!(
        client
            .get_with("/about", &[("X-Forwarded-For", "203.0.113.4")])
            .status,
        429
    );
}

#[test]
fn a_request_waiting_too_long_for_the_database_gets_503() {
    let feed = TestFeed::start(Options {
        db_wait: Duration::from_millis(100),
        ..Options::default()
    });
    let mut client = feed.client();
    let permits = feed.db.permits();
    let held = feed
        .block_on(permits.acquire_many_owned(hippocamp_feed::policy::DB_PERMITS as u32))
        .expect("hold");
    let reply = client.get("/healthz");
    assert_eq!(reply.status, 503);
    assert_eq!(json(&reply.body)["ok"], false);
    drop(held);
    assert_eq!(client.get("/healthz").status, 200);
}

#[test]
fn a_backup_taken_while_another_thread_writes_restores() {
    let feed = TestFeed::start(Options::default());
    let dir = tempfile::tempdir().expect("tempdir");

    // Another thread writes as fast as it can while the backup runs.
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let writer = {
        let stop = stop.clone();
        let path = feed.db_path.clone();
        std::thread::spawn(move || {
            let conn = hippocamp_feed::db::open(&path).expect("open");
            let mut n = 0;
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                conn.execute(
                    "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, 'x')",
                    [format!("filler-{}", n % 500)],
                )
                .expect("write");
                n += 1;
            }
        })
    };
    let info = backup::backup(&feed.db_path, dir.path(), 14, feed.clock.now()).expect("backup");
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    writer.join().expect("writer");

    let copy = rusqlite::Connection::open(&info.path).expect("copy");
    let verdict: String = copy
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .expect("check");
    assert_eq!(verdict, "ok");
    drop(copy);

    // A server started on the copy answers /healthz with ok and the same
    // schema version.
    let restored = TestFeed::start(Options {
        database: Some(info.path.clone()),
        ..Options::default()
    });
    let mut client = restored.client();
    let health = json(&client.get("/healthz").body);
    assert_eq!(health["ok"], true);
    assert_eq!(health["db_version"], info.version);
    assert_eq!(health["db_version"], 1);
    // Pages work on the copy too.
    assert_eq!(client.get("/about").status, 200);
}

#[test]
fn pruning_deletes_old_copies_keeps_the_newest_and_leaves_other_files() {
    let feed = TestFeed::start(Options::default());
    let dir = tempfile::tempdir().expect("tempdir");
    for name in [
        "feed-20260101T000000Z.sqlite",
        "feed-20261030T000000Z.sqlite",
        "readme.txt",
    ] {
        std::fs::write(dir.path().join(name), b"x").expect("write");
    }
    // The clock is 2026-11-08: 14 days back is 2026-10-25.
    backup::backup(&feed.db_path, dir.path(), 14, feed.clock.now()).expect("backup");
    let mut names: Vec<String> = std::fs::read_dir(dir.path())
        .expect("read")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec![
            "feed-20261030T000000Z.sqlite".to_string(),
            "feed-20261108T100000Z.sqlite".to_string(),
            "readme.txt".to_string(),
        ]
    );
}

#[test]
fn a_restart_keeps_the_data_and_the_server_answers_again() {
    let mut feed = TestFeed::start(Options::default());
    let mut client = feed.client();
    assert_eq!(client.get("/healthz").status, 200);
    feed.restart();
    let mut client = feed.client();
    assert_eq!(client.get("/healthz").status, 200);
    assert_eq!(client.get("/about").status, 200);
}

#[test]
fn the_raw_server_serves_files_and_the_answers_it_is_told() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("alice/packs/abc")).expect("dirs");
    std::fs::write(dir.path().join("alice/packs/abc/theme.css"), b":root {}").expect("file");
    let raw = RawServer::start(dir.path().to_path_buf());
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .max_redirects(0)
        .max_redirects_will_error(false)
        .build()
        .into();
    let get = |path: &str| {
        let mut r = agent
            .get(format!("{}{path}", raw.url))
            .call()
            .expect("call");
        let status = r.status().as_u16();
        let location = r
            .headers()
            .get("location")
            .map(|v| v.to_str().unwrap_or("").to_string());
        (
            status,
            r.body_mut().read_to_string().unwrap_or_default(),
            location,
        )
    };
    assert_eq!(
        get("/alice/packs/abc/theme.css"),
        (200, ":root {}".to_string(), None)
    );
    assert_eq!(get("/alice/packs/abc/missing.css").0, 404);
    assert_eq!(get("/../etc/passwd").0, 404);

    raw.answer(
        "/alice/packs/abc/theme.css",
        Answer::Bytes(b"other".to_vec()),
    );
    assert_eq!(get("/alice/packs/abc/theme.css").1, "other");
    raw.answer("/alice/packs/abc/theme.css", Answer::ServerError);
    assert_eq!(get("/alice/packs/abc/theme.css").0, 500);
    raw.answer(
        "/alice/packs/abc/theme.css",
        Answer::Redirect("/elsewhere".to_string()),
    );
    let (status, _, location) = get("/alice/packs/abc/theme.css");
    assert_eq!((status, location.as_deref()), (302, Some("/elsewhere")));
    raw.clear();
    assert_eq!(get("/alice/packs/abc/theme.css").0, 200);
}
