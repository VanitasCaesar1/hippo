//! The web layer: shared state, the route table, and the page frame.

pub mod assets;
pub mod layout;
pub mod pages;
pub mod rows;
pub mod security;

use std::sync::Arc;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::Uri;
use axum::middleware::from_fn_with_state;
use axum::routing::{get, post};
use axum::Router;
use time::OffsetDateTime;

use crate::api;
use crate::auth::session;
use crate::clock::Clock;
use crate::config::Config;
use crate::db::{Db, Member};
use crate::error::AppError;
use crate::http::Http;
use crate::policy;
use crate::themes;

pub use layout::{is_protected, Nav, Page};
pub use security::{Limiters, Quotas};

/// What every handler shares.
#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub config: Arc<Config>,
    pub clock: Arc<dyn Clock>,
    pub limiters: Arc<Limiters>,
    pub assets: Arc<assets::Assets>,
    /// The outbound agent for sign-in and registry sync.
    pub http: Http,
    /// When the server started, by `clock`; `/healthz` waits an hour after it
    /// before it reports a missing sync.
    pub started_at: OffsetDateTime,
}

impl AppState {
    pub fn new(config: Config, db: Db, clock: Arc<dyn Clock>, quotas: Quotas) -> Self {
        let started_at = clock.now();
        Self {
            db,
            config: Arc::new(config),
            clock,
            limiters: Arc::new(Limiters::new(quotas)),
            assets: Arc::new(assets::Assets::new()),
            http: Http::new(),
            started_at,
        }
    }
}

/// What a page handler knows about its request: the state, the path, whether
/// the address carries `?safe=1`, and the signed-in member.
pub struct Ctx {
    pub state: AppState,
    pub path: String,
    /// The current path and query as a local path, for `return_to`.
    pub return_to: String,
    pub safe: bool,
    pub member: Option<Member>,
    /// The member's theme pack stylesheet, when the feed serves it and the
    /// page may link custom CSS.
    pub pack_href: Option<String>,
}

impl Ctx {
    pub fn from_uri(state: &AppState, uri: &Uri) -> Self {
        let safe = uri.query().is_some_and(|q| {
            url::form_urlencoded::parse(q.as_bytes()).any(|(k, v)| k == "safe" && v == "1")
        });
        let full = match uri.query() {
            Some(q) => format!("{}?{}", uri.path(), q),
            None => uri.path().to_string(),
        };
        Self {
            state: state.clone(),
            path: uri.path().to_string(),
            return_to: local_path(&full),
            safe,
            member: None,
            pack_href: None,
        }
    }

    /// Starts a page named `name` (a name from `hippocamp_theme::PAGES`) with the
    /// given heading, which is also its `<title>`.
    pub fn page(&self, name: &'static str, h1: impl Into<String>) -> Page {
        Page::new(self, name, h1.into())
    }
}

impl FromRequestParts<AppState> for Ctx {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let mut ctx = Ctx::from_uri(state, &parts.uri);
        ctx.member = session::resolve_cached(parts, state).await?;
        ctx.pack_href = pack_href(state, &ctx).await?;
        Ok(ctx)
    }
}

/// The stylesheet of a member's theme pack, for a page that may link it.
pub async fn pack_href(state: &AppState, ctx: &Ctx) -> Result<Option<String>, AppError> {
    let Some(name) = ctx
        .member
        .as_ref()
        .and_then(|m| m.theme.strip_prefix("pack:"))
        .map(str::to_string)
    else {
        return Ok(None);
    };
    if is_protected(&ctx.path) || ctx.safe {
        return Ok(None);
    }
    let lookup = name.clone();
    let hash = state
        .db
        .read(move |c| Ok(themes::serves(c, &lookup)?))
        .await?;
    Ok(hash.map(|h| themes::href(&name, &h)))
}

/// A `return_to` value is kept only if it is a local path: it starts with `/`,
/// does not start with `//` or `/\`, and is at most 512 bytes. Anything else
/// becomes `/`.
pub fn local_path(value: &str) -> String {
    let ok = value.starts_with('/')
        && !value.starts_with("//")
        && !value.starts_with("/\\")
        && value.len() <= policy::RETURN_TO_MAX_BYTES
        && !value.chars().any(|c| c.is_control());
    if ok {
        value.to_string()
    } else {
        "/".to_string()
    }
}

async fn fallback() -> crate::error::AppError {
    crate::error::AppError::not_found()
}

/// The route table.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(pages::list::front))
        .route("/new", get(pages::list::new))
        .route("/past", get(pages::list::past))
        .route("/ask", get(pages::list::ask))
        .route("/show", get(pages::list::show_list))
        .route("/packs", get(pages::list::packs))
        .route("/evidence", get(pages::list::evidence))
        .route("/themes", get(pages::list::themes))
        .route("/themes/{name}/{file}", get(pages::theme_files::theme_file))
        .route("/p/{id}", get(pages::post::post))
        .route(
            "/p/{id}/measure",
            get(pages::measure::show).post(pages::measure::enter),
        )
        .route("/api/v1/posts", get(api::list))
        .route("/api/v1/posts/{id}", get(api::post))
        .route(
            "/submit",
            get(pages::submit::form).post(pages::submit::create),
        )
        .route(
            "/p/{id}/edit",
            get(pages::submit::edit_form).post(pages::submit::edit),
        )
        .route("/p/{id}/delete", post(pages::submit::delete))
        .route("/comments", get(pages::comments::comments))
        .route("/p/{id}/comment", post(pages::comment::create))
        .route("/c/{id}/reply", get(pages::comment::reply_form))
        .route(
            "/c/{id}/edit",
            get(pages::comment::edit_form).post(pages::comment::edit),
        )
        .route("/c/{id}/delete", post(pages::comment::delete))
        .route(
            "/report",
            get(pages::report::form).post(pages::report::file),
        )
        .route("/moderation", get(pages::moderation::log))
        .route("/mod", get(pages::mod_queue::queue))
        .route("/mod/u/{login}", get(pages::mod_queue::member_page))
        .route("/mod/action", post(pages::mod_queue::action))
        .route("/admin", get(pages::admin::admin))
        .route("/admin/role", post(pages::admin::set_role))
        .route("/admin/sync", post(pages::admin::sync))
        .route("/vote", post(pages::vote::vote))
        .route("/hide", post(pages::hide::hide))
        .route("/hidden", get(pages::hide::hidden))
        .route("/about", get(pages::about::about))
        .route("/healthz", get(pages::health::healthz))
        .route("/robots.txt", get(pages::robots::robots))
        .route("/login", get(pages::login::login))
        .route("/auth/github/callback", get(pages::login::callback))
        .route("/logout", post(pages::login::logout))
        .route("/settings", get(pages::settings::settings))
        .route("/settings/theme", post(pages::settings::save_theme))
        .route("/settings/export", get(pages::settings::export))
        .route("/settings/delete", post(pages::settings::delete))
        .route("/u/{id}/theme.css", get(pages::settings::personal_css))
        .route("/static/base.css", get(assets::base_css))
        .route("/static/themes/dark.css", get(assets::dark_css))
        .route("/static/themes/auto.css", get(assets::auto_css))
        .route("/static/icon.svg", get(assets::icon_svg))
        .fallback(fallback)
        .layer(from_fn_with_state(state.clone(), security::limits))
        .layer(from_fn_with_state(state.clone(), security::respond))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_paths_are_kept_and_everything_else_becomes_root() {
        assert_eq!(local_path("/p/12?x=1"), "/p/12?x=1");
        assert_eq!(local_path("/"), "/");
        for bad in [
            "",
            "p/12",
            "//evil.example",
            "/\\evil.example",
            "https://evil.example",
            "/a\nb",
        ] {
            assert_eq!(local_path(bad), "/", "{bad:?}");
        }
        assert_eq!(local_path(&format!("/{}", "a".repeat(511))).len(), 512);
        assert_eq!(local_path(&format!("/{}", "a".repeat(512))), "/");
    }
}
