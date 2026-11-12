//! Helpers for the integration tests: a server on a loopback port, a raw file
//! server for theme stylesheets, and a client that checks every page against
//! the page contract and every `GET` for writes.

#![allow(dead_code)] // Each test file uses some of these helpers, not all.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::State;
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::Router;
use hippo_feed::auth::mock::{self, MockHandle, MockProvider, MockUser};
use hippo_feed::clock::{Clock, FixedClock};
use hippo_feed::config::{Command, Config};
use hippo_feed::db::{self, Db, UserId};
use hippo_feed::jobs::HourlyReport;
use hippo_feed::policy::AddressQuota;
use hippo_feed::registry::SyncReport;
use hippo_feed::web::{AppState, Quotas};
use regex::Regex;
use time::macros::datetime;
use time::OffsetDateTime;

/// How a test server is set up.
#[derive(Clone)]
pub struct Options {
    /// The `FixedClock`'s start time.
    pub start: OffsetDateTime,
    /// Per-address quotas; generous unless a test lowers them.
    pub quotas: Quotas,
    pub admins: Vec<i64>,
    pub eval_sets: Option<String>,
    pub registry_index: Option<String>,
    pub raw_url: Option<String>,
    pub trust_proxy: bool,
    pub contact: Option<String>,
    /// How long a request waits for a database turn.
    pub db_wait: Duration,
    /// A database file to start on, instead of a new one.
    pub database: Option<PathBuf>,
    /// Whether the server has OAuth credentials (and so sign-in).
    pub github: bool,
    /// The mock provider's users, instead of the demo users.
    pub mock_users: Option<Vec<MockUser>>,
}

impl Default for Options {
    fn default() -> Self {
        let generous = AddressQuota {
            count: 100_000,
            per: Duration::from_secs(60),
        };
        Self {
            start: datetime!(2026-11-08 10:00:00 UTC),
            quotas: Quotas {
                sign_in: generous,
                api: generous,
                all: generous,
            },
            admins: Vec::new(),
            eval_sets: None,
            registry_index: None,
            raw_url: None,
            trust_proxy: false,
            contact: None,
            db_wait: Duration::from_secs(5),
            database: None,
            github: true,
            mock_users: None,
        }
    }
}

/// A running server in dev mode with a temporary database, a `FixedClock`
/// and no background task.
pub struct TestFeed {
    pub base_url: String,
    pub clock: Arc<FixedClock>,
    pub db_path: PathBuf,
    pub db: Db,
    /// What happened at the mock provider (tokens issued and revoked).
    pub mock: MockHandle,
    /// The mock provider's address, `http://127.0.0.1:<port>`.
    pub mock_url: String,
    mock_runtime: Option<tokio::runtime::Runtime>,
    state: Option<AppState>,
    options: Options,
    _dir: tempfile::TempDir,
    runtime: Option<tokio::runtime::Runtime>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
}

impl TestFeed {
    pub fn start(options: Options) -> TestFeed {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("feed.sqlite");
        if let Some(from) = &options.database {
            std::fs::copy(from, &db_path).expect("copy the database");
        }
        let clock = Arc::new(FixedClock::new(options.start));
        db::migrate_at(&db_path, clock.as_ref()).expect("migrate");
        // The mock provider lives on its own runtime, so a restart of the
        // server keeps it (and its address) as it was.
        let mock_runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("runtime");
        let mock_listener = mock_runtime
            .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
            .expect("bind");
        let mock_url = format!("http://{}", mock_listener.local_addr().expect("addr"));
        let users = options
            .mock_users
            .clone()
            .unwrap_or_else(|| mock::demo_users(options.start));
        let (mock_app, mock) = MockProvider::new(mock::CLIENT_ID, mock::CLIENT_SECRET, users);
        mock_runtime.spawn(async move {
            let _ = axum::serve(mock_listener, mock_app).await;
        });
        let mut feed = TestFeed {
            base_url: String::new(),
            clock,
            mock,
            mock_url,
            mock_runtime: Some(mock_runtime),
            state: None,
            db: Db::with_wait(&db_path, options.db_wait),
            db_path,
            options,
            _dir: dir,
            runtime: None,
            stop: None,
        };
        feed.launch();
        feed
    }

    fn launch(&mut self) {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("runtime");
        let listener = runtime
            .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let base = format!("http://{addr}");

        let o = &self.options;
        let mut vars: BTreeMap<String, String> = BTreeMap::new();
        vars.insert("FEED_DEV".into(), "1".into());
        vars.insert("FEED_BASE_URL".into(), base.clone());
        vars.insert("FEED_BIND".into(), addr.to_string());
        vars.insert("FEED_DB".into(), self.db_path.display().to_string());
        vars.insert("FEED_REGISTRY_SYNC_MINUTES".into(), "0".into());
        if o.github {
            vars.insert("FEED_GITHUB_CLIENT_ID".into(), mock::CLIENT_ID.into());
            vars.insert(
                "FEED_GITHUB_CLIENT_SECRET".into(),
                mock::CLIENT_SECRET.into(),
            );
            vars.insert(
                "FEED_OAUTH_AUTHORIZE_URL".into(),
                format!("{}/login/oauth/authorize", self.mock_url),
            );
            vars.insert(
                "FEED_OAUTH_TOKEN_URL".into(),
                format!("{}/login/oauth/access_token", self.mock_url),
            );
            vars.insert("FEED_OAUTH_API_URL".into(), self.mock_url.clone());
        }
        if !o.admins.is_empty() {
            let ids: Vec<String> = o.admins.iter().map(i64::to_string).collect();
            vars.insert("FEED_ADMINS".into(), ids.join(","));
        }
        if let Some(v) = &o.eval_sets {
            vars.insert("FEED_EVAL_SETS".into(), v.clone());
        }
        if let Some(v) = &o.registry_index {
            vars.insert("FEED_REGISTRY_INDEX".into(), v.clone());
        }
        if let Some(v) = &o.raw_url {
            vars.insert("FEED_RAW_URL".into(), v.clone());
        }
        if o.trust_proxy {
            vars.insert("FEED_TRUST_PROXY".into(), "1".into());
        }
        if let Some(v) = &o.contact {
            vars.insert("FEED_CONTACT".into(), v.clone());
        }
        let vars = vars.into_iter().collect();
        let config = Config::from_vars(Command::Serve, &vars).expect("config");

        let state = AppState::new(config, self.db.clone(), self.clock.clone(), o.quotas);
        self.state = Some(state.clone());
        let app = hippo_feed::router(state);
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
        runtime.spawn(async move {
            let _ = axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(async {
                let _ = stop_rx.await;
            })
            .await;
        });
        self.base_url = base;
        self.runtime = Some(runtime);
        self.stop = Some(stop_tx);
    }

    fn shutdown(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_timeout(Duration::from_secs(2));
        }
    }

    /// Stops the server and starts a new one on the same database, on a new
    /// port (the base URL changes).
    pub fn restart(&mut self) {
        self.shutdown();
        self.launch();
    }

    /// Runs a future on the server's runtime.
    pub fn block_on<F: std::future::Future>(&self, future: F) -> F::Output {
        self.runtime.as_ref().expect("running").block_on(future)
    }

    /// The server's state, for the functions a test calls itself.
    pub fn state(&self) -> &AppState {
        self.state.as_ref().expect("running")
    }

    /// Syncs the registry now, as the timer does (no actor), from
    /// `FEED_REGISTRY_INDEX`.
    pub fn sync(&self) -> SyncReport {
        self.sync_as(None)
    }

    /// A sync for `actor` (an admin's), from `FEED_REGISTRY_INDEX`.
    pub fn sync_as(&self, actor: Option<UserId>) -> SyncReport {
        self.block_on(hippo_feed::jobs::registry_sync(self.state(), actor))
            .expect("sync")
            .expect("an index is configured")
    }

    /// Runs the hourly clean-up at the clock's time.
    pub fn hourly(&self) -> HourlyReport {
        let now = self.clock_now();
        self.block_on(hippo_feed::jobs::hourly(self.state(), now))
            .expect("hourly")
    }

    pub fn client(&self) -> TestClient {
        let mut client = TestClient::new(&self.base_url, &self.db_path);
        client.mock_url = Some(self.mock_url.clone());
        client
    }

    /// The server's clock now.
    pub fn clock_now(&self) -> OffsetDateTime {
        self.clock.now()
    }

    /// A connection of the test's own to the database, for SQL the test does
    /// itself (state the pages do not create, or checks).
    pub fn conn(&self) -> rusqlite::Connection {
        let conn = rusqlite::Connection::open(&self.db_path).expect("open");
        conn.busy_timeout(Duration::from_secs(5)).expect("timeout");
        conn
    }
}

impl Drop for TestFeed {
    fn drop(&mut self) {
        self.shutdown();
        if let Some(runtime) = self.mock_runtime.take() {
            runtime.shutdown_timeout(Duration::from_secs(2));
        }
    }
}

// ---------------------------------------------------------------------------
// The raw file server for theme stylesheets.

/// What the raw server answers for a path instead of its file.
#[derive(Clone)]
pub enum Answer {
    Bytes(Vec<u8>),
    ServerError,
    Redirect(String),
}

struct RawShared {
    root: PathBuf,
    answers: Mutex<BTreeMap<String, Answer>>,
    hits: Mutex<Vec<String>>,
}

/// A small server that stands in for `raw.githubusercontent.com`: it serves
/// the files under `root` at the paths `FEED_RAW_URL` uses, and can be told to
/// answer a path with given bytes, a 500 or a redirect.
pub struct RawServer {
    pub url: String,
    shared: Arc<RawShared>,
    runtime: Option<tokio::runtime::Runtime>,
}

impl RawServer {
    /// Serves `fixtures/feed/raw/`.
    pub fn start_default() -> RawServer {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/feed/raw");
        RawServer::start(root)
    }

    pub fn start(root: PathBuf) -> RawServer {
        let shared = Arc::new(RawShared {
            root,
            answers: Mutex::new(BTreeMap::new()),
            hits: Mutex::new(Vec::new()),
        });
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("runtime");
        let listener = runtime
            .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
            .expect("bind");
        let url = format!("http://{}", listener.local_addr().expect("addr"));
        let app = Router::new()
            .fallback(raw_handler)
            .with_state(shared.clone());
        runtime.spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        RawServer {
            url,
            shared,
            runtime: Some(runtime),
        }
    }

    /// Answers `path` (such as `/alice/hippo-packs/<commit>/theme.css`) with
    /// `answer` from now on.
    pub fn answer(&self, path: &str, answer: Answer) {
        self.shared
            .answers
            .lock()
            .expect("lock")
            .insert(path.to_string(), answer);
    }

    /// How many times `path` has been asked for.
    pub fn hits(&self, path: &str) -> usize {
        self.shared
            .hits
            .lock()
            .expect("lock")
            .iter()
            .filter(|p| p.as_str() == path)
            .count()
    }

    /// Goes back to serving files.
    pub fn clear(&self) {
        self.shared.answers.lock().expect("lock").clear();
    }
}

impl Drop for RawServer {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_timeout(Duration::from_secs(2));
        }
    }
}

async fn raw_handler(State(shared): State<Arc<RawShared>>, uri: Uri) -> Response {
    let path = uri.path().to_string();
    shared.hits.lock().expect("lock").push(path.clone());
    let answer = shared.answers.lock().expect("lock").get(&path).cloned();
    match answer {
        Some(Answer::Bytes(bytes)) => return (StatusCode::OK, bytes).into_response(),
        Some(Answer::ServerError) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        Some(Answer::Redirect(to)) => {
            return (StatusCode::FOUND, [(header::LOCATION, to)]).into_response()
        }
        None => {}
    }
    let relative = path.trim_start_matches('/');
    let safe = !relative.is_empty()
        && Path::new(relative)
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)));
    if !safe {
        return StatusCode::NOT_FOUND.into_response();
    }
    match std::fs::read(shared.root.join(relative)) {
        Ok(bytes) => (StatusCode::OK, bytes).into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

// ---------------------------------------------------------------------------
// The client.

/// A response as the client saw it.
#[derive(Debug, Clone)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Reply {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// Every `Set-Cookie` value.
    pub fn set_cookies(&self) -> Vec<&str> {
        self.headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case("set-cookie"))
            .map(|(_, v)| v.as_str())
            .collect()
    }

    pub fn is_html(&self) -> bool {
        self.header("content-type")
            .is_some_and(|v| v.starts_with("text/html"))
    }
}

/// A `ureq` agent that returns every status as a response, follows no
/// redirects and keeps cookies. It checks every HTML response against the page
/// contract and every `GET` for writes.
pub struct TestClient {
    agent: ureq::Agent,
    base: String,
    cookies: BTreeMap<String, String>,
    csrf: Option<String>,
    watch: rusqlite::Connection,
    /// The mock provider's address, when the client came from a `TestFeed`.
    pub mock_url: Option<String>,
}

impl TestClient {
    pub fn new(base: &str, db_path: &Path) -> TestClient {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .max_redirects_will_error(false)
            .build()
            .into();
        TestClient {
            agent,
            base: base.to_string(),
            cookies: BTreeMap::new(),
            csrf: None,
            watch: rusqlite::Connection::open(db_path).expect("watch connection"),
            mock_url: None,
        }
    }

    pub fn get(&mut self, path: &str) -> Reply {
        self.get_with(path, &[])
    }

    /// A `GET` with extra request headers.
    pub fn get_with(&mut self, path: &str, headers: &[(&str, &str)]) -> Reply {
        let before = self.data_version();
        let mut request = self.agent.get(format!("{}{path}", self.base));
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        if let Some(cookie) = self.cookie_header() {
            request = request.header("Cookie", cookie);
        }
        let reply = Self::read(request.call());
        let after = self.data_version();
        let may_write = path == "/login"
            || path.starts_with("/login?")
            || path.starts_with("/auth/github/callback");
        if !may_write {
            assert_eq!(before, after, "GET {path} wrote to the database");
        }
        self.after(path, &reply);
        reply
    }

    /// A form `POST`, adding the CSRF token read from the last page's `csrf`
    /// field unless the caller gave one.
    pub fn post_form(&mut self, path: &str, fields: &[(&str, &str)]) -> Reply {
        self.post_form_with(path, fields, &[], true)
    }

    /// A form `POST` with extra request headers; `add_csrf` says whether to
    /// add the remembered CSRF token when the fields hold none.
    pub fn post_form_with(
        &mut self,
        path: &str,
        fields: &[(&str, &str)],
        headers: &[(&str, &str)],
        add_csrf: bool,
    ) -> Reply {
        let mut form = url::form_urlencoded::Serializer::new(String::new());
        let mut has_csrf = false;
        for (name, value) in fields {
            has_csrf |= *name == "csrf";
            form.append_pair(name, value);
        }
        if add_csrf && !has_csrf {
            if let Some(token) = &self.csrf {
                form.append_pair("csrf", token);
            }
        }
        let mut request = self
            .agent
            .post(format!("{}{path}", self.base))
            .content_type("application/x-www-form-urlencoded");
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        if let Some(cookie) = self.cookie_header() {
            request = request.header("Cookie", cookie);
        }
        let reply = Self::read(request.send(form.finish()));
        self.after(path, &reply);
        reply
    }

    /// A `GET` of an absolute URL (the mock provider) without cookies and
    /// without the page checks.
    pub fn get_absolute(&mut self, url: &str) -> Reply {
        Self::read(self.agent.get(url).call())
    }

    /// The value of a cookie the client holds.
    pub fn cookie(&self, name: &str) -> Option<String> {
        self.cookies.get(name).cloned()
    }

    pub fn set_cookie(&mut self, name: &str, value: &str) {
        self.cookies.insert(name.to_string(), value.to_string());
    }

    pub fn remove_cookie(&mut self, name: &str) {
        self.cookies.remove(name);
    }

    /// Starts sign-in: `GET /login?return_to=<return_to>`. The reply
    /// redirects to the mock provider's authorize page.
    pub fn login_start(&mut self, return_to: &str) -> Reply {
        let query = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("return_to", return_to)
            .finish();
        self.get(&format!("/login?{query}"))
    }

    /// Approves the authorize request `authorize_url` as `login` at the mock
    /// provider; returns the callback's path and query on the feed.
    pub fn mock_approve(&mut self, login: &str, authorize_url: &str) -> String {
        let original = url::Url::parse(authorize_url).expect("authorize url");
        let mut ser = url::form_urlencoded::Serializer::new(String::new());
        ser.append_pair("user", login);
        for (k, v) in original.query_pairs() {
            ser.append_pair(&k, &v);
        }
        let mock = self
            .mock_url
            .clone()
            .expect("a client from TestFeed::client");
        let reply = self.get_absolute(&format!(
            "{mock}/login/oauth/authorize/approve?{}",
            ser.finish()
        ));
        assert_eq!(reply.status, 303, "approve: {}", reply.body);
        let target = url::Url::parse(reply.header("location").expect("location")).expect("url");
        format!("{}?{}", target.path(), target.query().unwrap_or_default())
    }

    /// Signs in as a mock user and returns the callback's reply. Afterwards
    /// the client holds the session cookie.
    pub fn sign_in(&mut self, login: &str) -> Reply {
        self.sign_in_to(login, "/")
    }

    pub fn sign_in_to(&mut self, login: &str, return_to: &str) -> Reply {
        let start = self.login_start(return_to);
        assert_eq!(start.status, 303, "login: {}", start.body);
        let authorize = start.header("location").expect("authorize url").to_string();
        let callback = self.mock_approve(login, &authorize);
        self.get(&callback)
    }

    fn read(result: Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Reply {
        let mut response = result.expect("the request failed");
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();
        let body = response.body_mut().read_to_string().unwrap_or_default();
        Reply {
            status,
            headers,
            body,
        }
    }

    fn after(&mut self, path: &str, reply: &Reply) {
        for (name, value) in &reply.headers {
            if name.eq_ignore_ascii_case("set-cookie") {
                self.store_cookie(value);
            }
        }
        if reply.is_html() {
            check_page(path, &reply.body);
            let csrf = Regex::new(r#"name="csrf" value="([^"]*)""#).expect("regex");
            if let Some(cap) = csrf.captures(&reply.body) {
                self.csrf = Some(cap[1].to_string());
            }
        }
    }

    fn store_cookie(&mut self, set_cookie: &str) {
        let mut parts = set_cookie.split(';');
        let pair = parts.next().unwrap_or("");
        let gone = parts.any(|p| p.trim().eq_ignore_ascii_case("max-age=0"));
        if let Some((name, value)) = pair.split_once('=') {
            if gone || value.is_empty() {
                self.cookies.remove(name.trim());
            } else {
                self.cookies
                    .insert(name.trim().to_string(), value.trim().to_string());
            }
        }
    }

    fn cookie_header(&self) -> Option<String> {
        if self.cookies.is_empty() {
            return None;
        }
        Some(
            self.cookies
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("; "),
        )
    }

    fn data_version(&self) -> i64 {
        self.watch
            .query_row("PRAGMA data_version", [], |r| r.get(0))
            .expect("data_version")
    }
}

/// The web theme's page contract, checked on the HTML
/// text: the head, one `h1`, the landmarks, the skip link, a `label` for every
/// field, only classes of the contract, and a page name from the contract.
pub fn check_page(path: &str, html: &str) {
    let fail = |what: &str| panic!("{path}: {what}\n{html}");
    let lower = html.to_ascii_lowercase();
    if !lower.starts_with("<!doctype html>") {
        fail("does not start with <!doctype html>");
    }
    for needle in [
        r#"<html lang="en">"#,
        r#"<meta charset="utf-8">"#,
        r#"<meta name="viewport" content="width=device-width, initial-scale=1">"#,
        r#"rel="icon""#,
        r#"/static/icon.svg"#,
        "<header",
        "<nav",
        "<main",
        "<footer",
        r#"class="hp-skip""#,
    ] {
        if !html.contains(needle) {
            fail(&format!("is missing {needle}"));
        }
    }
    let title = Regex::new(r"<title>([^<]+) \| hippo</title>").expect("regex");
    if !title.is_match(html) {
        fail("has no <title> of the form `<name> | hippo`");
    }
    if html.matches("<h1").count() != 1 {
        fail("does not have exactly one h1");
    }
    if html.contains("<script") {
        fail("contains a script tag");
    }

    // The body's page name is in the contract.
    let body = Regex::new(r#"<body class="([^"]*)""#).expect("regex");
    let Some(cap) = body.captures(html) else {
        return fail("has no body class");
    };
    let classes: Vec<&str> = cap[1].split_whitespace().collect();
    let names: Vec<&str> = classes
        .iter()
        .filter_map(|c| c.strip_prefix("hp-page-"))
        .collect();
    if names.len() != 1 || !hippo_theme::PAGES.contains(&names[0]) {
        fail("the body does not name exactly one page of the contract");
    }

    // Every class is in the contract.
    let class = Regex::new(r#"class="([^"]*)""#).expect("regex");
    for cap in class.captures_iter(html) {
        for name in cap[1].split_whitespace() {
            let page = name
                .strip_prefix("hp-page-")
                .is_some_and(|n| hippo_theme::PAGES.contains(&n));
            if !hippo_theme::CLASSES.contains(&name) && !page {
                fail(&format!(
                    "uses the class {name}, which is not in the contract"
                ));
            }
        }
    }

    // Every visible field has a label.
    let field = Regex::new(r"<(input|textarea|select)\b([^>]*)>").expect("regex");
    let id = Regex::new(r#"\bid="([^"]*)""#).expect("regex");
    for cap in field.captures_iter(html) {
        let attrs = &cap[2];
        if cap[1].eq_ignore_ascii_case("input") {
            let hidden_kind = ["hidden", "submit", "button"]
                .iter()
                .any(|t| attrs.contains(&format!(r#"type="{t}""#)));
            if hidden_kind {
                continue;
            }
        }
        let Some(field_id) = id.captures(attrs) else {
            fail("has a field without an id, so without a label");
            return;
        };
        let label = format!(r#"for="{}""#, &field_id[1]);
        if !html.contains(&label) {
            fail(&format!("has no label for the field {}", &field_id[1]));
        }
    }
}
