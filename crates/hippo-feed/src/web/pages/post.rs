//! `GET /p/<id>`: a post, and for a pack's post the pack box, the install box
//! and, for a theme pack, the *use* button.

use std::collections::HashSet;

use axum::extract::Path;
use axum::response::{IntoResponse, Response};
use maud::{html, Markup, PreEscaped};
use rusqlite::OptionalExtension;
use time::Duration;

use crate::clock;
use crate::deeplink::{self, InstallLinks};
use crate::error::AppError;
use crate::evidence::{self, ContentEvidence};
use crate::markdown;
use crate::policy;
use crate::registry::{Kind as PackKind, PackRow};
use crate::themes;
use crate::votes::{self, Target};
use crate::web::layout::login_href;
use crate::web::pages::comment::{self, reported_ids};
use crate::web::pages::list::{pack_row, row_of, Rec, REC_COLUMNS};
use crate::web::rows::{self, CommentRow, PostRow};
use crate::web::Ctx;

struct View {
    rec: Rec,
    body: String,
    removed_reason: Option<String>,
    edited: bool,
    /// Whether the viewer may edit the post now.
    can_edit: bool,
    /// Whether the viewer may delete the post.
    can_delete: bool,
    contents: Vec<ContentEvidence>,
    row: PostRow,
    pack: Option<PackRow>,
    links: Option<InstallLinks>,
    served: bool,
    comments: Vec<CommentRow>,
    locked: bool,
    /// Whether the viewer may file reports (a signed-in member who is not
    /// restricted).
    may_report: bool,
    /// Whether the viewer has reported the post.
    post_reported: bool,
    /// The measurements the viewer has reported.
    reported_measurements: HashSet<i64>,
}

/// What the measurement table needs to offer *report*.
struct MeasurementReports<'a> {
    viewer: Option<&'a str>,
    allowed: bool,
    done: &'a HashSet<i64>,
}

fn pack_box(pack: &PackRow, measure_href: Option<String>) -> Markup {
    let review = clock::parse_storage(&pack.review_at)
        .map(clock::date_text)
        .unwrap_or_default();
    html! {
        section class="hp-pack-box" id="pack" {
            h2 { "Pack" }
            dl {
                dt { "Version" } dd { (pack.version) }
                dt { "Licence" } dd { (pack.license) }
                dt { "Owner" } dd { (pack.owner) }
                dt { "Maintainers" } dd { (pack.maintainers.join(", ")) }
                @if pack.first_party {
                    dt { "Origin" } dd { "First-party pack" }
                }
                @if let Some(source) = pack.source_link() {
                    dt { "Source" }
                    dd { a href=(source) rel="nofollow noopener noreferrer" { (source) } }
                }
                dt { "Pack hash" } dd { code { (pack.pack_hash) } }
                dt { "Registry review" } dd { (review) }
            }
            @if let Some(href) = &measure_href {
                p { a href=(href) rel="nofollow" { "Measure this pack" } }
            }
            @if pack.kind == PackKind::Passes {
                h3 { "Passes" }
                ul class="hp-passes" {
                    @for pass in &pack.passes {
                        li {
                            (pass.name) " reads "
                            @if pass.reads.is_empty() { "nothing" } @else { (pass.reads.join(", ")) }
                        }
                    }
                }
            }
        }
    }
}

fn install_box(links: &InstallLinks) -> Markup {
    html! {
        section class="hp-install-box" id="install" {
            h2 { "Install" }
            pre class="hp-code" { code { (links.command) } }
            p { a href=(links.deep_link) { "Open in Hippo.app" } }
            p class="hp-help" { "Hippo.app shows the pack and asks before it installs anything." }
        }
    }
}

fn rate(f: evidence::Fraction) -> String {
    f.signed_decimals(3)
}

fn measurement_table(content: &ContentEvidence, reports: &MeasurementReports) -> Markup {
    html! {
        table class="hp-table" {
            thead {
                tr {
                    th scope="col" { "Member" }
                    th scope="col" { "Date" }
                    th scope="col" { "Eval set" }
                    th scope="col" { "hippo" }
                    th scope="col" { "Model" }
                    th scope="col" { "Runs" }
                    th scope="col" { "Net rate without" }
                    th scope="col" { "Net rate with" }
                    th scope="col" { "Benefit" }
                    th scope="col" { "Revisions" }
                    th scope="col" { "Status" }
                    th scope="col" { "Report" }
                    th scope="col" { "Flag" }
                }
            }
            tbody {
                @for m in &content.measurements {
                    tr id=(format!("m{}", m.id)) {
                        td { @if let Some(name) = &m.member { (name) } @else { span class="hp-placeholder" { "[deleted]" } } }
                        td { (evidence::date_of(&m.created_at)) }
                        td { (m.eval_set) }
                        td { (m.hippo) }
                        td { (m.model) }
                        td { (m.runs) }
                        td { (rate(m.net_without)) }
                        td { (rate(m.net_with)) }
                        td { (rate(m.benefit)) }
                        td { (m.revisions) }
                        td { (m.status.label()) }
                        td {
                            @if let Some(url) = &m.report_url {
                                a href=(url) rel="nofollow ugc noopener noreferrer" { "report" }
                            }
                        }
                        td {
                            @if reports.done.contains(&m.id) {
                                "reported"
                            } @else if reports.allowed && m.member.as_deref() != reports.viewer {
                                a href=(format!("/report?target_type=measurement&target_id={}", m.id)) rel="nofollow" { "report" }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The evidence of every content, and the measurements of each.
fn evidence_box(
    contents: &[ContentEvidence],
    measuring_open: bool,
    reports: &MeasurementReports,
) -> Markup {
    let summary = evidence::summarise(contents);
    html! {
        section id="evidence" {
            h2 { "Evidence" }
            @if let Some(found) = &summary.evidence {
                p {
                    "Pack evidence: "
                    span class="hp-evidence" {
                        (evidence::signed_three_decimals(found.evidence.score))
                        " (" (found.evidence.n) ")"
                    }
                    @if !found.current {
                        " evidence from " (found.version)
                    }
                }
            } @else {
                p class="hp-help" { "Not measured yet: a content needs 3 counted measurements." }
            }
            @if contents.is_empty() {
                p class="hp-help" { "No content of this pack is recorded yet." }
            }
            ul {
                @for c in contents {
                    li {
                        "Version " (c.version) " (" code { (c.pack_hash.chars().take(12).collect::<String>()) } ")"
                        @if c.current { " current" }
                        ": "
                        @if let Some(e) = &c.evidence {
                            span class="hp-evidence" {
                                (evidence::signed_three_decimals(e.score)) " (" (e.n) ")"
                            }
                        } @else if measuring_open || c.counted > 0 {
                            (c.counted) " of 3 measurements"
                        } @else {
                            "not measured"
                        }
                    }
                }
            }
            @for c in contents {
                @if !c.measurements.is_empty() {
                    h3 { "Measurements of version " (c.version) }
                    (measurement_table(c, reports))
                }
            }
        }
    }
}

fn author_tools(view: &View, ctx: &Ctx) -> Markup {
    let Some(member) = &ctx.member else {
        return html! {};
    };
    let id = view.rec.id;
    let own = view.rec.author_id == Some(member.id.0);
    html! {
        @if view.rec.state == "visible" && !own && view.may_report {
            p class="hp-help" {
                @if view.post_reported {
                    "reported"
                } @else {
                    a href=(format!("/report?target_type=post&target_id={id}")) rel="nofollow" { "report" }
                }
            }
        }
        @if view.can_edit || view.can_delete {
            p class="hp-help" {
                @if view.can_edit {
                    a href=(format!("/p/{id}/edit")) rel="nofollow" { "edit" }
                }
                @if view.can_edit && view.can_delete { " | " }
                @if view.can_delete {
                    form method="post" action=(format!("/p/{id}/delete")) {
                        input type="hidden" name="csrf" value=(member.csrf);
                        button class="hp-link-button" type="submit" { "delete" }
                    }
                }
            }
        }
    }
}

fn theme_use(ctx: &Ctx, name: &str, served: bool) -> Markup {
    html! {
        @if !served {
            p class="hp-notice" { "This theme is not available." }
        } @else if let Some(member) = &ctx.member {
            form class="hp-form" method="post" action="/settings/theme" {
                input type="hidden" name="theme" value=(format!("pack:{name}"));
                input type="hidden" name="return_to" value=(ctx.return_to);
                input type="hidden" name="csrf" value=(member.csrf);
                button class="hp-button" type="submit" { "Use this theme" }
            }
        } @else {
            p { a href=(login_href(&ctx.return_to)) rel="nofollow" { "Sign in to use this theme" } }
        }
    }
}

/// `GET /p/<id>`.
pub async fn post(ctx: Ctx, Path(id): Path<String>) -> Result<Response, AppError> {
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
        return Err(AppError::not_found());
    }
    let id: i64 = id.parse().map_err(|_| AppError::not_found())?;
    let state = ctx.state.clone();
    let member = ctx.member.clone();
    let return_to = ctx.return_to.clone();
    let now = state.clock.now();
    let view: Option<View> = state
        .db
        .read(move |conn| {
            let found = conn
                .query_row(
                    &format!(
                        "SELECT {REC_COLUMNS}, p.body, p.removed_reason, p.edited_at IS NOT NULL
                         FROM posts p
                         LEFT JOIN users u ON u.id = p.author_id WHERE p.id = ?1"
                    ),
                    [id],
                    |r| {
                        Ok((
                            Rec::from_row(r)?,
                            r.get::<_, String>(8)?,
                            r.get::<_, Option<String>>(9)?,
                            r.get::<_, bool>(10)?,
                        ))
                    },
                )
                .optional()?;
            let Some((rec, body, removed_reason, edited)) = found else {
                return Ok(None);
            };
            let visible = rec.state == "visible";
            let counts = votes::counted_votes(conn, &[Target::Post(id)], now)?;
            let row = row_of(
                conn,
                &state.config,
                member.as_ref(),
                &rec,
                counts[&Target::Post(id)],
                0,
                now,
                &return_to,
            )?;
            let own = member
                .as_ref()
                .is_some_and(|m| rec.pack.is_none() && rec.author_id == Some(m.id.0));
            let created = clock::parse_storage(&rec.created_at).unwrap_or(now);
            let can_edit = own
                && visible
                && now - created <= Duration::hours(policy::EDIT_WINDOW_HOURS)
                && member.as_ref().is_some_and(|m| {
                    !matches!(
                        crate::users::standing(m, now),
                        crate::users::Standing::Restricted | crate::users::Standing::Banned
                    )
                });
            let can_delete = own && rec.state != "deleted";
            let contents = match &rec.pack {
                Some(name) => evidence::contents(conn, name, now)?,
                None => Vec::new(),
            };
            let pack = match &rec.pack {
                Some(name) => pack_row(conn, name)?,
                None => None,
            };
            let links = pack
                .as_ref()
                .and_then(|p| deeplink::install_links(visible, p, &state.config));
            let served = match &pack {
                Some(p) if p.kind == PackKind::Theme => {
                    visible && themes::serves(conn, &p.name)?.is_some()
                }
                _ => false,
            };
            let locked: bool =
                conn.query_row("SELECT locked FROM posts WHERE id = ?1", [id], |r| r.get(0))?;
            let may_report = member.as_ref().is_some_and(|m| {
                !matches!(
                    crate::users::standing(m, now),
                    crate::users::Standing::Restricted | crate::users::Standing::Banned
                )
            });
            let comments = comment::thread_rows(
                conn,
                &state.config,
                member.as_ref(),
                id,
                visible && !locked,
                &return_to,
                now,
            )?;
            let (post_reported, reported_measurements) = match &member {
                Some(m) => {
                    let ids: Vec<i64> = contents
                        .iter()
                        .flat_map(|c| c.measurements.iter().map(|x| x.id))
                        .collect();
                    (
                        reported_ids(conn, m, "post", &[id])?.contains(&id),
                        reported_ids(conn, m, "measurement", &ids)?,
                    )
                }
                None => (false, HashSet::new()),
            };
            Ok(Some(View {
                comments,
                locked,
                may_report,
                post_reported,
                reported_measurements,
                rec,
                body,
                removed_reason,
                edited,
                can_edit,
                can_delete,
                contents,
                row,
                pack,
                links,
                served,
            }))
        })
        .await?;
    let view = view.ok_or_else(AppError::not_found)?;
    let placeholder = match view.rec.state.as_str() {
        "visible" => None,
        "hidden" => Some("Hidden while a moderator reviews it.".to_string()),
        "removed" => Some(format!(
            "[removed: {}]",
            view.removed_reason.as_deref().unwrap_or_default()
        )),
        _ => Some("[deleted]".to_string()),
    };
    let h1 = placeholder
        .clone()
        .unwrap_or_else(|| view.rec.title.clone());
    let measuring_open = !ctx.state.config.eval_sets.is_empty();
    // The row (or the placeholder) shows the title just below, so the `h1`
    // is for screen readers only, as on the lists.
    let page = ctx.page("post", h1).hidden_h1().body(html! {
        article class="hp-post" {
            @if let Some(text) = &placeholder {
                p class="hp-placeholder" { (text) }
            } @else {
                ol class="hp-items" { (rows::render(&view.row)) }
                @if view.edited {
                    p class="hp-help" { "edited" }
                }
                @if view.pack.is_none() {
                    @if let Some(link) = &view.rec.link {
                        p { a href=(link) rel="nofollow ugc noopener noreferrer" { (link) } }
                    }
                    @if !view.body.is_empty() {
                        div class="hp-post-body hp-md" { (PreEscaped(markdown::render(&view.body).0)) }
                    }
                }
            }
            (author_tools(&view, &ctx))
        }
        @if let Some(pack) = &view.pack {
            (pack_box(
                pack,
                (view.rec.state == "visible" && pack.listed && pack.kind == PackKind::Passes)
                    .then(|| format!("/p/{}/measure", view.rec.id)),
            ))
            @if pack.kind == PackKind::Theme {
                (theme_use(&ctx, &pack.name, view.served))
            } @else {
                @if let Some(links) = &view.links {
                    (install_box(links))
                } @else if !pack.listed {
                    p class="hp-notice" { "This pack is no longer listed in the registry." }
                }
                (evidence_box(&view.contents, measuring_open, &MeasurementReports {
                    viewer: ctx.member.as_ref().map(|m| m.login.as_str()),
                    allowed: view.may_report,
                    done: &view.reported_measurements,
                }))
            }
        }
        section id="comments" {
            h2 { "Comments" }
            (comment::comment_box(
                ctx.member.as_ref(),
                view.rec.id,
                view.rec.state == "visible",
                view.locked,
                &ctx.return_to,
                ctx.state.clock.now(),
            ))
            @if view.comments.is_empty() {
                p class="hp-help" { "No comments yet." }
            } @else {
                (rows::thread(&view.comments))
            }
        }
    });
    Ok(page.into_response())
}
