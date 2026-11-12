//! The row of a list of posts, in the contract's classes.
//!
//! Callers (items 19, 22, 24 and 30) fill a [`PostRow`] from the database and
//! the clock; this module only draws it.

use maud::{html, Markup, PreEscaped};
use url::form_urlencoded;

/// What kind of post a row is, with what the row's links need.
#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Discussion,
    /// A pack of passes. `install` is true while the post is visible and the
    /// pack listed, so the row links to the install box.
    Pack {
        name: String,
        install: bool,
    },
    /// A theme pack. `served` is true while the feed serves its stylesheet,
    /// which is when *use* is offered.
    ThemePack {
        name: String,
        served: bool,
    },
}

/// A pack's evidence as a row shows it.
#[derive(Debug, Clone, PartialEq)]
pub enum Evidence {
    None,
    /// A score from `n` counted measurements.
    Score {
        score: f64,
        n: u32,
    },
    /// Fewer than three counted measurements: `<n> of 3 measurements`.
    Count {
        n: u32,
    },
}

/// Who a row credits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Author {
    /// A member's login, or a pack's owner from the index.
    Name(String),
    /// An author who deleted their account.
    Deleted,
}

/// Who is looking at the row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Viewer {
    SignedOut {
        return_to: String,
    },
    Member {
        /// Whether the member may vote on this post.
        may_vote: bool,
        /// Whether the member has voted on it.
        voted: bool,
        /// Whether the member has hidden it (on `/hidden`, which offers
        /// *un-hide* instead of *hide*).
        hidden: bool,
        csrf: String,
        return_to: String,
    },
}

/// One row of a list of posts.
#[derive(Debug, Clone, PartialEq)]
pub struct PostRow {
    pub rank: usize,
    pub id: i64,
    pub kind: Kind,
    pub title: String,
    /// The link URL of a discussion, as stored.
    pub link: Option<String>,
    /// The domain shown in grey parentheses.
    pub domain: Option<String>,
    pub points: i64,
    pub author: Author,
    /// The age as text ("3 hours ago").
    pub age: String,
    pub evidence: Evidence,
    /// Visible comments.
    pub comments: u32,
    pub viewer: Viewer,
}

pub use crate::evidence::signed_two_decimals;

fn plural(n: i64, one: &str, many: &str) -> String {
    if n == 1 {
        format!("{n} {one}")
    } else {
        format!("{n} {many}")
    }
}

fn encode(value: &str) -> String {
    form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

/// The rows of a list as an `ol`, numbered from the first row's rank.
pub fn list(rows: &[PostRow]) -> Markup {
    let start = rows.first().map_or(1, |r| r.rank);
    html! {
        ol class="hp-items" start=(start) {
            @for row in rows { (render(row)) }
        }
    }
}

/// One row: the rank, the arrow column, the title and domain, the meta line.
pub fn render(row: &PostRow) -> Markup {
    let kind_class = match row.kind {
        Kind::Discussion => "hp-item hp-item-discussion",
        Kind::Pack { .. } => "hp-item hp-item-pack",
        Kind::ThemePack { .. } => "hp-item hp-item-theme",
    };
    let page = format!("/p/{}", row.id);
    html! {
        li class=(kind_class) {
            @if row.rank > 0 {
                span class="hp-rank" { (row.rank) "." }
            }
            (arrow(row))
            span class="hp-title" {
                @if let Some(link) = &row.link {
                    a class="hp-title-link" href=(link) rel="nofollow ugc noopener noreferrer" { (row.title) }
                } @else {
                    a class="hp-title-link" href=(page) { (row.title) }
                }
                @if let Some(domain) = &row.domain {
                    " " span class="hp-domain" { "(" (domain) ")" }
                }
            }
            div class="hp-meta" { (meta(row)) }
        }
    }
}

/// The arrow column's three cases.
fn arrow(row: &PostRow) -> Markup {
    let label = format!("Upvote: {}", row.title);
    html! {
        span class="hp-vote" {
            @match &row.viewer {
                Viewer::Member { may_vote: true, voted: false, csrf, return_to, .. } => {
                    form method="post" action="/vote" {
                        input type="hidden" name="target_type" value="post";
                        input type="hidden" name="target_id" value=(row.id);
                        input type="hidden" name="return_to" value=(return_to);
                        input type="hidden" name="csrf" value=(csrf);
                        button class="hp-vote-button" type="submit" aria-label=(label) { "▲" }
                    }
                }
                Viewer::SignedOut { return_to } => {
                    a class="hp-vote-button"
                        href=(format!("/login?return_to={}", encode(return_to)))
                        rel="nofollow" aria-label=(label) { "▲" }
                }
                Viewer::Member { .. } => {}
            }
        }
    }
}

/// A form drawn as a link: hidden fields and a `hp-link-button`.
fn link_form(action: &str, fields: &[(&str, String)], label: &str) -> Markup {
    html! {
        form method="post" action=(action) {
            @for (name, value) in fields {
                input type="hidden" name=(name) value=(value);
            }
            button class="hp-link-button" type="submit" { (label) }
        }
    }
}

fn meta(row: &PostRow) -> Markup {
    let separator = html! { " | " };
    let member = match &row.viewer {
        Viewer::Member {
            csrf,
            return_to,
            voted,
            hidden,
            ..
        } => Some((csrf.as_str(), return_to.as_str(), *voted, *hidden)),
        Viewer::SignedOut { .. } => None,
    };
    let comments = if row.comments == 0 {
        "discuss".to_string()
    } else {
        plural(i64::from(row.comments), "comment", "comments")
    };
    html! {
        span class="hp-points" { (plural(row.points, "point", "points")) }
        " by "
        span class="hp-user" {
            @match &row.author {
                Author::Name(name) => { (name) }
                Author::Deleted => { span class="hp-placeholder" { "[deleted]" } }
            }
        }
        " "
        a class="hp-age" href=(format!("/p/{}", row.id)) { (row.age) }
        @if let Some((csrf, return_to, true, _)) = member {
            " "
            (link_form("/vote", &[
                ("target_type", "post".to_string()),
                ("target_id", row.id.to_string()),
                ("return_to", return_to.to_string()),
                ("csrf", csrf.to_string()),
                ("withdraw", "1".to_string()),
            ], "unvote"))
        }
        @match &row.evidence {
            Evidence::Score { score, n } => {
                (separator)
                span class="hp-evidence" { (signed_two_decimals(*score)) " evidence (" (n) ")" }
            }
            Evidence::Count { n } => {
                (separator)
                span class="hp-evidence" { (n) " of 3 measurements" }
            }
            Evidence::None => {}
        }
        @if let Some((csrf, return_to, _, hidden)) = member {
            (separator)
            @if hidden {
                (link_form("/hide", &[
                    ("post_id", row.id.to_string()),
                    ("return_to", return_to.to_string()),
                    ("csrf", csrf.to_string()),
                    ("unhide", "1".to_string()),
                ], "un-hide"))
            } @else {
                (link_form("/hide", &[
                    ("post_id", row.id.to_string()),
                    ("unhide", "0".to_string()),
                    ("return_to", return_to.to_string()),
                    ("csrf", csrf.to_string()),
                ], "hide"))
            }
        }
        @match &row.kind {
            Kind::Pack { install: true, .. } => {
                (separator)
                a class="hp-install" href=(format!("/p/{}#install", row.id)) { "install" }
            }
            Kind::ThemePack { served: false, .. } => {
                (separator)
                span class="hp-placeholder" { "This theme is not available." }
            }
            Kind::ThemePack { name, served: true } => {
                @if let Some((csrf, return_to, _, _)) = member {
                    (separator)
                    (link_form("/settings/theme", &[
                        ("theme", format!("pack:{name}")),
                        ("return_to", return_to.to_string()),
                        ("csrf", csrf.to_string()),
                    ], "use"))
                }
            }
            _ => {}
        }
        (separator)
        a class="hp-comments-link" href=(format!("/p/{}", row.id)) { (comments) }
    }
}

// --- Comments ---

/// What a comment shows in place of, or as, its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommentBody {
    /// The rendered Markdown of text anyone may read.
    Text(String),
    /// What others see of a hidden, removed or deleted comment.
    Placeholder(String),
    /// A moderator's view of a hidden or removed text: the mark, then the
    /// rendered text.
    Marked { mark: String, html: String },
}

/// The *report* part of a comment's grey line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportLink {
    None,
    /// The viewer may report the item.
    Link,
    /// The viewer has: *reported* takes the link's place.
    Done,
}

/// One comment, as a thread or `/comments` draws it.
#[derive(Debug, Clone, PartialEq)]
pub struct CommentRow {
    pub id: i64,
    pub post_id: i64,
    /// Levels below a top-level comment.
    pub depth: u32,
    pub author: Author,
    pub age: String,
    pub edited: bool,
    pub parent: Option<i64>,
    pub body: CommentBody,
    /// The post's title, for `/comments`' `| on: <title>`.
    pub on: Option<String>,
    pub reply: bool,
    pub edit: bool,
    pub delete: bool,
    pub report: ReportLink,
    /// The arrow column's viewer; its `return_to` includes the comment's
    /// anchor.
    pub viewer: Viewer,
}

fn comment_arrow(row: &CommentRow) -> Markup {
    let who = match &row.author {
        Author::Name(name) => name.as_str(),
        Author::Deleted => "[deleted]",
    };
    let label = format!("Upvote: comment by {who}");
    if !matches!(row.body, CommentBody::Text(_)) {
        return html! { span class="hp-vote" {} };
    }
    html! {
        span class="hp-vote" {
            @match &row.viewer {
                Viewer::Member { may_vote: true, voted: false, csrf, return_to, .. } => {
                    form method="post" action="/vote" {
                        input type="hidden" name="target_type" value="comment";
                        input type="hidden" name="target_id" value=(row.id);
                        input type="hidden" name="return_to" value=(return_to);
                        input type="hidden" name="csrf" value=(csrf);
                        button class="hp-vote-button" type="submit" aria-label=(label) { "▲" }
                    }
                }
                Viewer::SignedOut { return_to } => {
                    a class="hp-vote-button"
                        href=(format!("/login?return_to={}", encode(return_to)))
                        rel="nofollow" aria-label=(label) { "▲" }
                }
                Viewer::Member { .. } => {}
            }
        }
    }
}

fn comment_meta(row: &CommentRow) -> Markup {
    let anchor = format!("/p/{}#c{}", row.post_id, row.id);
    let member = match &row.viewer {
        Viewer::Member {
            csrf,
            return_to,
            voted,
            ..
        } => Some((csrf.as_str(), return_to.as_str(), *voted)),
        Viewer::SignedOut { .. } => None,
    };
    html! {
        span class="hp-user" {
            @match &row.author {
                Author::Name(name) => { (name) }
                Author::Deleted => { span class="hp-placeholder" { "[deleted]" } }
            }
        }
        " "
        a class="hp-age" href=(anchor) { (row.age) }
        @if row.edited { " edited" }
        @if let Some((csrf, return_to, true)) = member {
            " "
            (link_form("/vote", &[
                ("target_type", "comment".to_string()),
                ("target_id", row.id.to_string()),
                ("return_to", return_to.to_string()),
                ("csrf", csrf.to_string()),
                ("withdraw", "1".to_string()),
            ], "unvote"))
        }
        @if let Some(parent) = row.parent {
            " | "
            a href=(format!("/p/{}#c{parent}", row.post_id)) { "parent" }
        }
        @if row.reply {
            " | "
            a href=(format!("/c/{}/reply", row.id)) rel="nofollow" { "reply" }
        }
        @if row.edit {
            " | "
            a href=(format!("/c/{}/edit", row.id)) rel="nofollow" { "edit" }
        }
        @if let (true, Some((csrf, _, _))) = (row.delete, member) {
            " | "
            (link_form(&format!("/c/{}/delete", row.id), &[("csrf", csrf.to_string())], "delete"))
        }
        @match row.report {
            ReportLink::Link => {
                " | "
                a href=(format!("/report?target_type=comment&target_id={}", row.id)) rel="nofollow" { "report" }
            }
            ReportLink::Done => { " | reported" }
            ReportLink::None => {}
        }
        @if let Some(title) = &row.on {
            " | on: "
            a href=(format!("/p/{}", row.post_id)) { (title) }
        }
    }
}

fn comment_item(row: &CommentRow, children: Markup) -> Markup {
    html! {
        li class="hp-comment" id=(format!("c{}", row.id)) {
            (comment_arrow(row))
            div class="hp-comment-meta" { (comment_meta(row)) }
            div class="hp-comment-body" {
                @match &row.body {
                    CommentBody::Text(markup) => { div class="hp-md" { (PreEscaped(markup)) } }
                    CommentBody::Placeholder(text) => { span class="hp-placeholder" { (text) } }
                    CommentBody::Marked { mark, html: markup } => {
                        span class="hp-placeholder" { (mark) }
                        div class="hp-md" { (PreEscaped(markup)) }
                    }
                }
            }
            (children)
        }
    }
}

fn level(rows: &[CommentRow], i: &mut usize, depth: u32) -> Markup {
    let mut out = Vec::new();
    while *i < rows.len() && rows[*i].depth >= depth {
        let row = &rows[*i];
        *i += 1;
        let children = match rows.get(*i) {
            Some(next) if next.depth > row.depth => {
                let child_depth = next.depth;
                html! { ul class="hp-children" { (level(rows, i, child_depth)) } }
            }
            _ => html! {},
        };
        out.push(comment_item(row, children));
    }
    html! { @for item in &out { (item) } }
}

/// A post's comments as nested lists: `ul.hp-comments`, with each comment's
/// replies in a `ul.hp-children` inside its `li`. `rows` is in thread order
/// (each comment followed by its replies).
pub fn thread(rows: &[CommentRow]) -> Markup {
    let mut i = 0;
    let top = rows.first().map_or(0, |r| r.depth);
    html! { ul class="hp-comments" { (level(rows, &mut i, top)) } }
}

/// A flat list of comments (`/comments`), each with its `on:` post.
pub fn comment_list(rows: &[CommentRow]) -> Markup {
    html! {
        ul class="hp-comments" {
            @for row in rows { (comment_item(row, html! {})) }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn discussion() -> PostRow {
        PostRow {
            rank: 1,
            id: 7,
            kind: Kind::Discussion,
            title: "Ask hippo: how do you review notes?".to_string(),
            link: None,
            domain: None,
            points: 14,
            author: Author::Name("ada".to_string()),
            age: "3 hours ago".to_string(),
            evidence: Evidence::None,
            comments: 3,
            viewer: Viewer::SignedOut {
                return_to: "/".to_string(),
            },
        }
    }

    fn pack(evidence: Evidence) -> PostRow {
        PostRow {
            rank: 2,
            id: 9,
            kind: Kind::Pack {
                name: "glossary-terms".to_string(),
                install: true,
            },
            title: "glossary-terms: Adds the terms sessions define".to_string(),
            link: None,
            domain: Some("github.com/alice".to_string()),
            points: 14,
            author: Author::Name("alice".to_string()),
            age: "3 hours ago".to_string(),
            evidence,
            comments: 3,
            viewer: Viewer::SignedOut {
                return_to: "/".to_string(),
            },
        }
    }

    fn member(may_vote: bool, voted: bool) -> Viewer {
        Viewer::Member {
            may_vote,
            voted,
            hidden: false,
            csrf: "tok".to_string(),
            return_to: "/new?page=2".to_string(),
        }
    }

    const SIGNED_OUT_ARROW: &str = r#"<span class="hp-vote"><a class="hp-vote-button" href="/login?return_to=%2F" rel="nofollow" aria-label="Upvote: Ask hippo: how do you review notes?">▲</a></span>"#;

    #[test]
    fn a_discussion_without_a_link_url() {
        let html = render(&discussion()).into_string();
        let expected = format!(
            concat!(
                r#"<li class="hp-item hp-item-discussion"><span class="hp-rank">1.</span>{arrow}"#,
                r#"<span class="hp-title"><a class="hp-title-link" href="/p/7">Ask hippo: how do you review notes?</a></span>"#,
                r#"<div class="hp-meta"><span class="hp-points">14 points</span> by <span class="hp-user">ada</span> "#,
                r#"<a class="hp-age" href="/p/7">3 hours ago</a> | <a class="hp-comments-link" href="/p/7">3 comments</a></div></li>"#,
            ),
            arrow = SIGNED_OUT_ARROW
        );
        assert_eq!(html, expected);
    }

    #[test]
    fn a_discussion_with_a_link_url_and_domain() {
        let mut row = discussion();
        row.title = "A note".to_string();
        row.link = Some("https://example.com/a?b=1&c=2".to_string());
        row.domain = Some("example.com".to_string());
        let html = render(&row).into_string();
        assert!(
            html.contains(r#"<span class="hp-title"><a class="hp-title-link" href="https://example.com/a?b=1&amp;c=2" rel="nofollow ugc noopener noreferrer">A note</a> <span class="hp-domain">(example.com)</span></span>"#),
            "{html}"
        );
    }

    #[test]
    fn a_pack_with_each_kind_of_evidence() {
        let html = render(&pack(Evidence::Score { score: 0.143, n: 5 })).into_string();
        let expected = concat!(
            r#"<div class="hp-meta"><span class="hp-points">14 points</span> by <span class="hp-user">alice</span> "#,
            r#"<a class="hp-age" href="/p/9">3 hours ago</a> | <span class="hp-evidence">+0.14 evidence (5)</span> | "#,
            r#"<a class="hp-install" href="/p/9#install">install</a> | <a class="hp-comments-link" href="/p/9">3 comments</a></div></li>"#,
        );
        assert!(html.ends_with(expected), "{html}");
        assert!(html.starts_with(r#"<li class="hp-item hp-item-pack">"#));
        assert!(html.contains(r#"<span class="hp-domain">(github.com/alice)</span>"#));

        let html = render(&pack(Evidence::Score { score: -0.05, n: 3 })).into_string();
        assert!(
            html.contains(r#"<span class="hp-evidence">-0.05 evidence (3)</span>"#),
            "{html}"
        );
        let html = render(&pack(Evidence::Score { score: 0.0, n: 4 })).into_string();
        assert!(
            html.contains(r#"<span class="hp-evidence">0.00 evidence (4)</span>"#),
            "{html}"
        );
        let html = render(&pack(Evidence::Count { n: 2 })).into_string();
        assert!(
            html.contains(r#"<span class="hp-evidence">2 of 3 measurements</span>"#),
            "{html}"
        );
        let html = render(&pack(Evidence::None)).into_string();
        assert!(!html.contains("hp-evidence"), "{html}");
        // No install link when the pack is not installable.
        let mut row = pack(Evidence::None);
        row.kind = Kind::Pack {
            name: "glossary-terms".to_string(),
            install: false,
        };
        assert!(!render(&row).into_string().contains("install"));
    }

    #[test]
    fn evidence_rounds_half_away_from_zero() {
        // 0.125 is exact in binary, so the half is a true half.
        assert_eq!(signed_two_decimals(0.125), "+0.13");
        assert_eq!(signed_two_decimals(-0.125), "-0.13");
        assert_eq!(signed_two_decimals(0.004), "0.00");
        assert_eq!(signed_two_decimals(-0.004), "0.00");
        assert_eq!(signed_two_decimals(1.0), "+1.00");
        assert_eq!(signed_two_decimals(-1.0), "-1.00");
        assert_eq!(signed_two_decimals(0.1), "+0.10");
    }

    fn theme(served: bool, viewer: Viewer) -> PostRow {
        let mut row = pack(Evidence::None);
        row.kind = Kind::ThemePack {
            name: "midnight".to_string(),
            served,
        };
        row.viewer = viewer;
        row
    }

    #[test]
    fn a_theme_pack_served_and_not_served() {
        let html = render(&theme(true, member(true, false))).into_string();
        assert!(html.starts_with(r#"<li class="hp-item hp-item-theme">"#));
        assert!(
            html.contains(r#"<form method="post" action="/settings/theme"><input type="hidden" name="theme" value="pack:midnight"><input type="hidden" name="return_to" value="/new?page=2"><input type="hidden" name="csrf" value="tok"><button class="hp-link-button" type="submit">use</button></form>"#),
            "{html}"
        );
        assert!(!html.contains("install"));
        let html = render(&theme(false, member(true, false))).into_string();
        assert!(!html.contains(">use<"), "{html}");
        // *use* is for signed-in members only.
        let html = render(&theme(
            true,
            Viewer::SignedOut {
                return_to: "/".to_string(),
            },
        ))
        .into_string();
        assert!(!html.contains(">use<"), "{html}");
    }

    #[test]
    fn the_three_arrow_cases() {
        // Signed in, may vote, has not: a form with the four fields.
        let html = render(&{
            let mut r = discussion();
            r.viewer = member(true, false);
            r
        })
        .into_string();
        assert!(
            html.contains(r#"<span class="hp-vote"><form method="post" action="/vote"><input type="hidden" name="target_type" value="post"><input type="hidden" name="target_id" value="7"><input type="hidden" name="return_to" value="/new?page=2"><input type="hidden" name="csrf" value="tok"><button class="hp-vote-button" type="submit" aria-label="Upvote: Ask hippo: how do you review notes?">▲</button></form></span>"#),
            "{html}"
        );
        // Signed out: a link to sign-in.
        let html = render(&discussion()).into_string();
        assert!(html.contains(SIGNED_OUT_ARROW), "{html}");
        // Own post, maintained pack, or already voted: an empty span.
        for viewer in [member(false, false), member(true, true)] {
            let mut r = discussion();
            r.viewer = viewer;
            let html = render(&r).into_string();
            assert!(html.contains(r#"<span class="hp-vote"></span>"#), "{html}");
        }
        // A voted row offers unvote after the age.
        let mut r = discussion();
        r.viewer = member(true, true);
        let html = render(&r).into_string();
        assert!(
            html.contains(r#"3 hours ago</a> <form method="post" action="/vote">"#),
            "{html}"
        );
        assert!(html.contains(r#"<input type="hidden" name="withdraw" value="1"><button class="hp-link-button" type="submit">unvote</button>"#), "{html}");
    }

    #[test]
    fn hide_and_un_hide() {
        let mut r = discussion();
        r.viewer = member(true, false);
        let html = render(&r).into_string();
        assert!(
            html.contains(r#"<button class="hp-link-button" type="submit">hide</button>"#),
            "{html}"
        );
        r.viewer = Viewer::Member {
            may_vote: true,
            voted: false,
            hidden: true,
            csrf: "tok".to_string(),
            return_to: "/hidden".to_string(),
        };
        let html = render(&r).into_string();
        assert!(
            html.contains(r#"<button class="hp-link-button" type="submit">un-hide</button>"#),
            "{html}"
        );
        assert!(html.contains(r#"name="unhide" value="1""#));
        // Signed out sees neither.
        let html = render(&discussion()).into_string();
        assert!(!html.contains("hide"), "{html}");
    }

    #[test]
    fn singulars_and_discuss() {
        let mut r = discussion();
        r.points = 1;
        r.comments = 1;
        let html = render(&r).into_string();
        assert!(
            html.contains(r#"<span class="hp-points">1 point</span>"#),
            "{html}"
        );
        assert!(html.contains(">1 comment</a>"), "{html}");
        r.points = 0;
        r.comments = 0;
        let html = render(&r).into_string();
        assert!(
            html.contains(r#"<span class="hp-points">0 points</span>"#),
            "{html}"
        );
        assert!(html.contains(">discuss</a>"), "{html}");
    }

    #[test]
    fn a_deleted_author() {
        let mut r = discussion();
        r.author = Author::Deleted;
        let html = render(&r).into_string();
        assert!(
            html.contains(
                r#" by <span class="hp-user"><span class="hp-placeholder">[deleted]</span></span> "#
            ),
            "{html}"
        );
    }

    #[test]
    fn a_list_numbers_from_its_first_rank() {
        let mut a = discussion();
        a.rank = 31;
        let mut b = discussion();
        b.rank = 32;
        let html = list(&[a, b]).into_string();
        assert!(
            html.starts_with(r#"<ol class="hp-items" start="31">"#),
            "{html}"
        );
        assert!(html.contains(r#"<span class="hp-rank">32.</span>"#));
    }

    #[test]
    fn every_class_is_in_the_contract() {
        let mut r = pack(Evidence::Score { score: 0.2, n: 5 });
        r.viewer = member(true, true);
        r.author = Author::Deleted;
        r.domain = Some("x".to_string());
        let html = list(&[r]).into_string();
        let class = regex::Regex::new(r#"class="([^"]*)""#).expect("regex");
        for cap in class.captures_iter(&html) {
            for name in cap[1].split_whitespace() {
                assert!(
                    hippo_theme::CLASSES.contains(&name),
                    "{name} is not in the contract"
                );
            }
        }
    }

    fn comment(id: i64, depth: u32, parent: Option<i64>) -> CommentRow {
        CommentRow {
            id,
            post_id: 5,
            depth,
            author: Author::Name("ada".to_string()),
            age: "3 hours ago".to_string(),
            edited: false,
            parent,
            body: CommentBody::Text("<p>hi</p>".to_string()),
            on: None,
            reply: true,
            edit: false,
            delete: false,
            report: ReportLink::None,
            viewer: Viewer::SignedOut {
                return_to: "/p/5".to_string(),
            },
        }
    }

    #[test]
    fn a_thread_nests_replies_under_their_parents() {
        let rows = [
            comment(1, 0, None),
            comment(2, 1, Some(1)),
            comment(3, 2, Some(2)),
            comment(4, 1, Some(1)),
            comment(5, 0, None),
        ];
        let html = thread(&rows).into_string();
        assert_eq!(html.matches(r#"<ul class="hp-children">"#).count(), 2);
        let at = |id: i64| html.find(&format!(r#"id="c{id}""#)).expect("anchor");
        assert!(at(1) < at(2) && at(2) < at(3) && at(3) < at(4) && at(4) < at(5));
        // Comment 3 sits two lists deep, and comment 5 is back at the top.
        assert!(html.contains(r#"<li class="hp-comment" id="c5">"#));
        assert!(html.contains(r##"<a href="/p/5#c1">parent</a>"##));
        assert!(html.contains("3 hours ago</a>"));
        assert!(!html.contains("edited"));
    }

    #[test]
    fn a_placeholder_has_no_arrow() {
        let mut row = comment(1, 0, None);
        row.body = CommentBody::Placeholder("[deleted]".to_string());
        let html = comment_list(&[row]).into_string();
        assert!(html.contains(r#"<span class="hp-vote"></span>"#));
        assert!(html.contains(r#"<span class="hp-placeholder">[deleted]</span>"#));
    }
}
