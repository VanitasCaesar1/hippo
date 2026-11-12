//! The page frame: head, top bar, main, footer, and the stylesheet links.
//!
//! The markup follows the web theme contract (see `hippo-theme`): every class
//! is one of `hippo_theme::CLASSES` and the body names its page.

use axum::http::{header, HeaderValue};
use axum::response::{IntoResponse, Response};
use maud::{html, Markup, DOCTYPE};
use url::form_urlencoded;

use crate::db::Member;
use crate::web::assets::Assets;
use crate::web::Ctx;
use std::sync::Arc;

/// A link of the top bar's navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nav {
    New,
    Past,
    Comments,
    Ask,
    Show,
    Packs,
    Evidence,
    Submit,
}

impl Nav {
    pub const ALL: [Nav; 8] = [
        Nav::New,
        Nav::Past,
        Nav::Comments,
        Nav::Ask,
        Nav::Show,
        Nav::Packs,
        Nav::Evidence,
        Nav::Submit,
    ];

    pub fn href(self) -> &'static str {
        match self {
            Nav::New => "/new",
            Nav::Past => "/past",
            Nav::Comments => "/comments",
            Nav::Ask => "/ask",
            Nav::Show => "/show",
            Nav::Packs => "/packs",
            Nav::Evidence => "/evidence",
            Nav::Submit => "/submit",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Nav::New => "new",
            Nav::Past => "past",
            Nav::Comments => "comments",
            Nav::Ask => "ask",
            Nav::Show => "show",
            Nav::Packs => "packs",
            Nav::Evidence => "evidence",
            Nav::Submit => "submit",
        }
    }
}

/// The protected pages never link a theme pack or personal CSS: `/login`,
/// `/settings`, `/mod` and `/admin` and the paths below them, and the paths
/// below `/auth/`. (Every error page is protected too; the error renderer says
/// so itself.) The public `/moderation` log is not protected.
pub fn is_protected(path: &str) -> bool {
    let below = |root: &str| path == root || path.starts_with(&format!("{root}/"));
    below("/login")
        || below("/settings")
        || below("/mod")
        || below("/admin")
        || path.starts_with("/auth/")
}

/// A page being built: its name, heading, body and who it is for.
pub struct Page {
    /// The page's name, the `<name>` of `hp-page-<name>`.
    pub name: &'static str,
    /// The page's `h1`, also its `<title>` before ` | hippo`.
    pub h1: String,
    /// List and post pages hide their `h1` from sight, not from screen readers.
    pub h1_hidden: bool,
    /// The navigation link of the list being shown.
    pub current: Option<Nav>,
    /// Protected pages link the member's built-in theme and nothing custom.
    pub protected: bool,
    pub member: Option<Member>,
    /// The stylesheet of the member's theme pack, when the feed serves it.
    pub pack_href: Option<String>,
    pub return_to: String,
    pub body: Markup,
    assets: Arc<Assets>,
}

impl Page {
    pub fn new(ctx: &Ctx, name: &'static str, h1: String) -> Self {
        Self {
            name,
            h1,
            h1_hidden: false,
            current: None,
            protected: is_protected(&ctx.path) || ctx.safe,
            member: ctx.member.clone(),
            pack_href: ctx.pack_href.clone(),
            return_to: ctx.return_to.clone(),
            body: html! {},
            assets: ctx.state.assets.clone(),
        }
    }

    pub fn body(mut self, body: Markup) -> Self {
        self.body = body;
        self
    }

    /// Hides the `h1` visually (the list pages and post pages).
    pub fn hidden_h1(mut self) -> Self {
        self.h1_hidden = true;
        self
    }

    /// Marks a navigation link as the current one.
    pub fn current(mut self, nav: Nav) -> Self {
        self.current = Some(nav);
        self
    }

    /// Makes the page protected whatever its path (error pages).
    pub fn protected(mut self) -> Self {
        self.protected = true;
        self
    }

    /// The stylesheet links, in order: the base; the viewer's theme (none for
    /// `default`, `dark` or `auto`, or a theme pack's file; signed-out viewers
    /// and members whose pack the feed does not serve get `auto`); and a
    /// signed-in member's personal CSS. Protected pages link the member's
    /// built-in theme only, `auto` for a member whose theme is a pack.
    fn stylesheet_links(&self) -> Vec<String> {
        let mut links = vec![self.assets.base_href()];
        let theme = self.member.as_ref().map_or("auto", |m| m.theme.as_str());
        match theme {
            "default" => {}
            "dark" => links.push(self.assets.dark_href()),
            _ if theme.starts_with("pack:") && !self.protected && self.pack_href.is_some() => {
                links.extend(self.pack_href.clone());
            }
            _ => links.push(self.assets.auto_href()),
        }
        if let Some(member) = &self.member {
            if member.has_personal_css && !self.protected {
                links.push(format!("/u/{}/theme.css", member.id.0));
            }
        }
        links
    }

    pub fn render(&self) -> Markup {
        let title = format!("{} | hippo", self.h1);
        let body_class = format!("hp-page hp-page-{}", self.name);
        html! {
            (DOCTYPE)
            html lang="en" {
                head {
                    meta charset="utf-8";
                    meta name="viewport" content="width=device-width, initial-scale=1";
                    title { (title) }
                    link rel="icon" type="image/svg+xml" href=(self.assets.icon_href());
                    @for href in self.stylesheet_links() {
                        link rel="stylesheet" href=(href);
                    }
                }
                body class=(body_class) {
                    a class="hp-skip" href="#main" { "Skip to content" }
                    div class="hp-content" {
                        (self.top_bar())
                        main class="hp-main" id="main" {
                            @if self.h1_hidden {
                                h1 class="hp-sr" { (self.h1) }
                            } @else {
                                h1 { (self.h1) }
                            }
                            (self.body)
                        }
                        (footer())
                    }
                }
            }
        }
    }

    fn top_bar(&self) -> Markup {
        html! {
            header class="hp-topbar" {
                a class="hp-home" href="/" {
                    (maud::PreEscaped(hippo_theme::mark_svg("hp-mark")))
                    " "
                    span class="hp-name" { "hippo" }
                }
                nav class="hp-nav" aria-label="Main" {
                    @for (i, nav) in Nav::ALL.iter().enumerate() {
                        @if i > 0 { " " span class="hp-sep" { "|" } " " }
                        @if self.current == Some(*nav) {
                            a class="hp-nav-current" href=(nav.href()) aria-current="page" { (nav.label()) }
                        } @else {
                            a href=(nav.href()) { (nav.label()) }
                        }
                    }
                }
                div class="hp-account" {
                    @if let Some(member) = &self.member {
                        a href="/settings" { (member.login) }
                        " "
                        span class="hp-karma" { "(" (member.karma) ")" }
                        " "
                        form method="post" action="/logout" {
                            input type="hidden" name="csrf" value=(member.csrf);
                            button class="hp-link-button" type="submit" { "logout" }
                        }
                    } @else {
                        a href=(login_href(&self.return_to)) rel="nofollow" { "login" }
                    }
                }
            }
        }
    }
}

/// `/login?return_to=<the current path and query>`.
pub fn login_href(return_to: &str) -> String {
    format!(
        "/login?return_to={}",
        form_urlencoded::byte_serialize(return_to.as_bytes()).collect::<String>()
    )
}

fn footer() -> Markup {
    html! {
        footer class="hp-footer" {
            a href="/about" { "about" }
            " " span class="hp-sep" { "|" } " "
            a href="/themes" { "themes" }
            " " span class="hp-sep" { "|" } " "
            a href="/moderation" { "moderation" }
        }
    }
}

impl IntoResponse for Page {
    fn into_response(self) -> Response {
        let mut response = self.render().into_response();
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("private, no-store"),
        );
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protected_paths() {
        for p in [
            "/login",
            "/login/x",
            "/settings",
            "/settings/export",
            "/mod",
            "/mod/u/ada",
            "/admin",
            "/admin/role",
            "/auth/github/callback",
        ] {
            assert!(is_protected(p), "{p}");
        }
        for p in [
            "/",
            "/moderation",
            "/modx",
            "/p/3",
            "/settingsx",
            "/authx",
            "/about",
            "/hidden",
        ] {
            assert!(!is_protected(p), "{p}");
        }
    }

    #[test]
    fn login_link_encodes_the_return_path() {
        assert_eq!(
            login_href("/p/12?a=1&b=2"),
            "/login?return_to=%2Fp%2F12%3Fa%3D1%26b%3D2"
        );
    }
}
