//! Response headers, request logging, error rendering and per-address limits.

use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::num::NonZeroU32;
use std::time::Duration;

use axum::extract::{ConnectInfo, MatchedPath, Request, State};
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use governor::clock::{Clock as _, DefaultClock};
use governor::state::keyed::DashMapStateStore;
use governor::{Quota, RateLimiter};

use crate::error::AppError;
use crate::policy::{self, AddressQuota};
use crate::web::{AppState, Ctx};

/// The content security policy: no scripts, and images and fonts only from
/// the feed or inline `data:` URLs, which personal CSS may use.
pub const CSP: &str = "default-src 'none'; script-src 'none'; style-src 'self'; \
img-src 'self' data:; font-src 'self' data:; connect-src 'none'; form-action 'self'; \
frame-ancestors 'none'; base-uri 'none'";

/// A client address as the limiters count it: an IPv4 address, or the /64
/// prefix of an IPv6 address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AddressKey {
    V4([u8; 4]),
    V6Prefix([u8; 8]),
}

impl AddressKey {
    pub fn from_ip(ip: IpAddr) -> Self {
        match ip.to_canonical() {
            IpAddr::V4(v4) => AddressKey::V4(v4.octets()),
            IpAddr::V6(v6) => {
                let o = v6.octets();
                AddressKey::V6Prefix([o[0], o[1], o[2], o[3], o[4], o[5], o[6], o[7]])
            }
        }
    }
}

/// The client address of a request. Behind Caddy (`trust_proxy`) it is the
/// last entry of `X-Forwarded-For`, the one Caddy appends; if the header is
/// missing or malformed, or the proxy is not trusted, it is the connection's
/// address.
pub fn client_ip(headers: &HeaderMap, connection: IpAddr, trust_proxy: bool) -> IpAddr {
    if !trust_proxy {
        return connection;
    }
    let last = headers
        .get_all("x-forwarded-for")
        .iter()
        .next_back()
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.rsplit(',').next())
        .map(str::trim)
        .and_then(|v| v.parse::<IpAddr>().ok());
    last.unwrap_or(connection)
}

/// The per-address quotas.
#[derive(Debug, Clone, Copy)]
pub struct Quotas {
    pub sign_in: AddressQuota,
    pub api: AddressQuota,
    pub all: AddressQuota,
}

impl Default for Quotas {
    fn default() -> Self {
        Self {
            sign_in: policy::SIGN_IN_QUOTA,
            api: policy::API_QUOTA,
            all: policy::ALL_QUOTA,
        }
    }
}

type Keyed = RateLimiter<AddressKey, DashMapStateStore<AddressKey>, DefaultClock>;

/// One keyed limiter and the quota it enforces.
struct Limiter {
    inner: Keyed,
    quota: AddressQuota,
}

impl Limiter {
    fn new(quota: AddressQuota) -> Self {
        let count = quota.count.max(1);
        let period = (quota.per / count).max(Duration::from_millis(1));
        let burst = NonZeroU32::new(count).unwrap_or(NonZeroU32::MIN);
        let q = Quota::with_period(period)
            .unwrap_or_else(|| Quota::per_second(NonZeroU32::MIN))
            .allow_burst(burst);
        Self {
            inner: RateLimiter::keyed(q),
            quota,
        }
    }

    /// `Ok` if the request is allowed; otherwise the whole seconds until the
    /// next one would be, at least 1.
    fn check(&self, key: AddressKey) -> Result<(), u64> {
        match self.inner.check_key(&key) {
            Ok(()) => Ok(()),
            Err(not_until) => {
                let wait = not_until.wait_time_from(DefaultClock::default().now());
                Err(wait.as_secs_f64().ceil().max(1.0) as u64)
            }
        }
    }

    fn describe(&self) -> String {
        format!(
            "{} requests per {}",
            self.quota.count,
            describe_duration(self.quota.per)
        )
    }
}

fn describe_duration(d: Duration) -> String {
    let secs = d.as_secs();
    // `is_multiple_of` needs Rust 1.90; the workspace's minimum is 1.89.
    if secs / 60 * 60 == secs {
        match secs / 60 {
            1 => "minute".to_string(),
            n => format!("{n} minutes"),
        }
    } else {
        format!("{secs} seconds")
    }
}

/// The three per-address limiters, `sign_in`, `api` and `all`.
pub struct Limiters {
    sign_in: Limiter,
    api: Limiter,
    all: Limiter,
}

impl Limiters {
    pub fn new(quotas: Quotas) -> Self {
        Self {
            sign_in: Limiter::new(quotas.sign_in),
            api: Limiter::new(quotas.api),
            all: Limiter::new(quotas.all),
        }
    }

    fn refuse(limiter: &Limiter, seconds: u64) -> AppError {
        AppError::too_many_requests(
            format!(
                "Too many requests from your address (the limit is {}). \
                 Try again in {} second{}.",
                limiter.describe(),
                seconds,
                if seconds == 1 { "" } else { "s" }
            ),
            seconds,
        )
    }

    /// Counts a request of any kind against `all`.
    pub fn check_all(&self, ip: IpAddr) -> Result<(), AppError> {
        self.all
            .check(AddressKey::from_ip(ip))
            .map_err(|s| Self::refuse(&self.all, s))
    }

    /// Counts an API request against `api`.
    pub fn check_api(&self, ip: IpAddr) -> Result<(), AppError> {
        self.api
            .check(AddressKey::from_ip(ip))
            .map_err(|s| Self::refuse(&self.api, s))
    }

    /// Counts a sign-in start against `sign_in` (used by `/login`).
    pub fn check_sign_in(&self, ip: IpAddr) -> Result<(), AppError> {
        self.sign_in
            .check(AddressKey::from_ip(ip))
            .map_err(|s| Self::refuse(&self.sign_in, s))
    }

    /// Forgets addresses whose quota has fully recovered.
    pub fn prune(&self) {
        self.sign_in.inner.retain_recent();
        self.api.inner.retain_recent();
        self.all.inner.retain_recent();
    }
}

/// Whether a path is outside the `all` limiter: the static files, a member's
/// personal CSS and a theme pack's stylesheet.
pub fn exempt_from_all(path: &str) -> bool {
    if path.starts_with("/static/") {
        return true;
    }
    if let Some(rest) = path.strip_prefix("/u/") {
        let mut parts = rest.split('/');
        if let (Some(id), Some("theme.css"), None) = (parts.next(), parts.next(), parts.next()) {
            return !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit());
        }
        return false;
    }
    if let Some(rest) = path.strip_prefix("/themes/") {
        let parts: Vec<&str> = rest.split('/').collect();
        return parts.len() == 2
            && !parts[0].is_empty()
            && parts[1].ends_with(".css")
            && parts[1].len() > ".css".len();
    }
    false
}

fn connection_ip(req: &Request) -> IpAddr {
    req.extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip())
        .unwrap_or(IpAddr::V6(Ipv6Addr::LOCALHOST))
}

/// The per-address limits. Over a quota the answer is 429 with
/// `Retry-After`.
pub async fn limits(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    let ip = client_ip(req.headers(), connection_ip(&req), state.config.trust_proxy);
    if !exempt_from_all(&path) {
        if let Err(e) = state.limiters.check_all(ip) {
            return e.into_response();
        }
    }
    if path.starts_with("/api/") {
        if let Err(e) = state.limiters.check_api(ip) {
            return e.into_response();
        }
    }
    next.run(req).await
}

/// The outermost layer: renders errors, adds the response headers and logs
/// the request (method, route pattern and status; never the query string).
pub async fn respond(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let method = req.method().clone();
    let uri = req.uri().clone();
    let headers = req.headers().clone();
    let route = req
        .extensions()
        .get::<MatchedPath>()
        .map_or_else(|| "-".to_string(), |m| m.as_str().to_string());
    let response = next.run(req).await;
    let mut response = render_errors(&state, &uri, &headers, response).await;
    add_headers(&state, response.headers_mut());
    tracing::info!(
        method = %method,
        route = %route,
        status = response.status().as_u16(),
        "request"
    );
    response
}

/// Turns an `AppError` response, or a bare error status from the router, into
/// an error page, or into JSON under `/api/`.
async fn render_errors(
    state: &AppState,
    uri: &axum::http::Uri,
    headers: &HeaderMap,
    response: Response,
) -> Response {
    let error = match response.extensions().get::<AppError>() {
        Some(e) => e.clone(),
        None => {
            let status = response.status();
            let bare = (status.is_client_error() || status.is_server_error())
                && !response.headers().contains_key(header::CONTENT_TYPE);
            if !bare {
                return response;
            }
            let message = match status {
                StatusCode::NOT_FOUND => AppError::not_found().message,
                StatusCode::METHOD_NOT_ALLOWED => {
                    "This address does not accept that kind of request.".to_string()
                }
                _ => "The request could not be handled.".to_string(),
            };
            AppError::new(status, message)
        }
    };
    let keep: Vec<(HeaderName, HeaderValue)> = [header::RETRY_AFTER, header::ALLOW]
        .iter()
        .filter_map(|name| {
            response
                .headers()
                .get(name)
                .map(|v| (name.clone(), v.clone()))
        })
        .collect();

    let mut out = if uri.path().starts_with("/api/") {
        let mut r = serde_json::json!({ "error": error.message })
            .to_string()
            .into_response();
        r.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        r.headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        r
    } else {
        let mut ctx = Ctx::from_uri(state, uri);
        // An error page still shows who is signed in; it links no custom CSS.
        ctx.member = crate::auth::session::resolve(state, headers)
            .await
            .ok()
            .flatten();
        ctx.page("error", error.name())
            .protected()
            .body(maud::html! { p class="hp-error" { (error.message) } })
            .into_response()
    };
    *out.status_mut() = error.status;
    for (name, value) in keep {
        out.headers_mut().insert(name, value);
    }
    out
}

fn add_headers(state: &AppState, headers: &mut HeaderMap) {
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CSP),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("same-origin"),
    );
    let https = state
        .config
        .base_url
        .as_deref()
        .is_some_and(|u| u.starts_with("https://"));
    if https {
        headers.insert(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static("max-age=31536000"),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn xff(values: &[&str]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for v in values {
            h.append("x-forwarded-for", HeaderValue::from_str(v).expect("value"));
        }
        h
    }

    #[test]
    fn client_ip_uses_the_last_forwarded_entry_only_when_trusted() {
        let conn: IpAddr = "127.0.0.1".parse().expect("ip");
        let h = xff(&["203.0.113.9, 198.51.100.7"]);
        assert_eq!(
            client_ip(&h, conn, true),
            "198.51.100.7".parse::<IpAddr>().expect("ip")
        );
        assert_eq!(client_ip(&h, conn, false), conn);
        let two = xff(&["1.1.1.1", "198.51.100.8"]);
        assert_eq!(
            client_ip(&two, conn, true),
            "198.51.100.8".parse::<IpAddr>().expect("ip")
        );
        assert_eq!(client_ip(&xff(&["garbage"]), conn, true), conn);
        assert_eq!(client_ip(&HeaderMap::new(), conn, true), conn);
    }

    #[test]
    fn ipv6_addresses_share_a_slash_64() {
        let a = AddressKey::from_ip("2001:db8:1:2::1".parse().expect("ip"));
        let b = AddressKey::from_ip("2001:db8:1:2:ffff::9".parse().expect("ip"));
        let c = AddressKey::from_ip("2001:db8:1:3::1".parse().expect("ip"));
        assert_eq!(a, b);
        assert_ne!(a, c);
        // An IPv4-mapped IPv6 address counts as the IPv4 address.
        assert_eq!(
            AddressKey::from_ip("::ffff:192.0.2.1".parse().expect("ip")),
            AddressKey::from_ip("192.0.2.1".parse().expect("ip"))
        );
    }

    #[test]
    fn the_limiter_allows_its_burst_then_refuses_with_a_wait() {
        let limiter = Limiter::new(AddressQuota {
            count: 3,
            per: Duration::from_secs(60),
        });
        let key = AddressKey::from_ip("192.0.2.1".parse().expect("ip"));
        for _ in 0..3 {
            assert!(limiter.check(key).is_ok());
        }
        let wait = limiter.check(key).expect_err("fourth refused");
        assert!((1..=60).contains(&wait), "{wait}");
        let other = AddressKey::from_ip("192.0.2.2".parse().expect("ip"));
        assert!(limiter.check(other).is_ok());
    }

    #[test]
    fn exempt_paths_are_the_static_and_stylesheet_ones() {
        for p in [
            "/static/base.css",
            "/u/7/theme.css",
            "/themes/dark-pack/abc.css",
        ] {
            assert!(exempt_from_all(p), "{p}");
        }
        for p in [
            "/",
            "/about",
            "/u/7",
            "/u/x/theme.css",
            "/themes",
            "/themes/a/b",
            "/themes/a/.css",
            "/api/v1/posts",
        ] {
            assert!(!exempt_from_all(p), "{p}");
        }
    }
}
