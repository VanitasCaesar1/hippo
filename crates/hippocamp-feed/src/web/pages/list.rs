//! The lists of posts: `/`, `/new`, `/past`, `/ask`, `/show`, `/packs`,
//! `/evidence` and `/themes`, in the orders of `rank.rs`.
//!
//! Each list is computed when the page is read; a row counts the post's
//! visible comments.

use axum::extract::RawQuery;
use axum::response::{IntoResponse, Response};
use maud::{html, Markup};
use rusqlite::{params, Connection};
use time::{Date, OffsetDateTime};

use crate::config::Config;
use crate::db::Member;
use crate::deeplink;
use crate::error::AppError;
use crate::evidence as ev;
use crate::posts;
use crate::rank::{self, Listing};
use crate::registry::{Kind as PackKind, PackRow};
use crate::themes;
use crate::users::{self, Standing};
use crate::votes;
use crate::web::rows::{self, Author, Evidence, Kind, PostRow, Viewer};
use crate::web::{Ctx, Nav};
use crate::{clock, policy};

pub use crate::rank::{Rec, REC_COLUMNS};

pub fn pack_row(conn: &Connection, name: &str) -> rusqlite::Result<Option<PackRow>> {
    use rusqlite::OptionalExtension;
    conn.query_row(
        &format!("SELECT {} FROM packs WHERE name = ?1", PackRow::COLUMNS),
        [name],
        PackRow::from_row,
    )
    .optional()
}

/// The viewer of a row.
pub fn viewer_of(
    conn: &Connection,
    member: Option<&Member>,
    rec: &Rec,
    now: OffsetDateTime,
    return_to: &str,
) -> rusqlite::Result<Viewer> {
    let Some(member) = member else {
        return Ok(Viewer::SignedOut {
            return_to: return_to.to_owned(),
        });
    };
    let voted: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM votes
                        WHERE user_id = ?1 AND target_type = 'post' AND target_id = ?2)",
        params![member.id.0, rec.id],
        |r| r.get(0),
    )?;
    let mut may_vote = rec.author_id != Some(member.id.0)
        && !matches!(
            users::standing(member, now),
            Standing::Restricted | Standing::Banned
        );
    if may_vote {
        if let Some(pack) = &rec.pack {
            let sql = format!("SELECT {}", votes::maintainer_sql("?1", "?2", "?3"));
            let maintainer: bool =
                conn.query_row(&sql, params![pack, member.github_id, member.login], |r| {
                    r.get(0)
                })?;
            may_vote = !maintainer;
        }
    }
    Ok(Viewer::Member {
        may_vote,
        voted,
        hidden: false,
        csrf: member.csrf.clone(),
        return_to: return_to.to_owned(),
    })
}

/// One row, with `rank` 0 for an unnumbered one.
#[allow(clippy::too_many_arguments)]
pub fn row_of(
    conn: &Connection,
    config: &Config,
    member: Option<&Member>,
    rec: &Rec,
    points: i64,
    rank: usize,
    now: OffsetDateTime,
    return_to: &str,
) -> rusqlite::Result<PostRow> {
    let age = clock::parse_storage(&rec.created_at)
        .map(|t| clock::age_text(t, now))
        .unwrap_or_default();
    let viewer = viewer_of(conn, member, rec, now, return_to)?;
    let pack = match &rec.pack {
        Some(name) => pack_row(conn, name)?,
        None => None,
    };
    let mut evidence = Evidence::None;
    let (kind, domain, author, link) = match pack {
        Some(pack) if pack.kind == PackKind::Theme => (
            Kind::ThemePack {
                name: pack.name.clone(),
                served: themes::serves(conn, &pack.name)?.is_some(),
            },
            Some(posts::pack_domain(&pack.owner)),
            Author::Name(pack.owner.clone()),
            None,
        ),
        Some(pack) => {
            let summary = ev::summary(conn, &pack.name, now)?;
            evidence = match summary.evidence {
                Some(found) => Evidence::Score {
                    score: found.evidence.score,
                    n: found.evidence.n,
                },
                None if !config.eval_sets.is_empty() => Evidence::Count {
                    n: summary.current_counted,
                },
                None => Evidence::None,
            };
            (
                Kind::Pack {
                    name: pack.name.clone(),
                    install: deeplink::install_links(rec.state == "visible", &pack, config)
                        .is_some(),
                },
                Some(posts::pack_domain(&pack.owner)),
                Author::Name(pack.owner.clone()),
                None,
            )
        }
        None => (
            Kind::Discussion,
            rec.link.as_deref().and_then(posts::domain),
            rec.author_login
                .clone()
                .map_or(Author::Deleted, Author::Name),
            rec.link.clone(),
        ),
    };
    Ok(PostRow {
        rank,
        id: rec.id,
        kind,
        title: rec.title.clone(),
        link,
        domain,
        points,
        author,
        age,
        evidence,
        comments: conn.query_row(
            "SELECT COUNT(*) FROM comments WHERE post_id = ?1 AND state = 'visible'",
            [rec.id],
            |r| r.get(0),
        )?,
        viewer,
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Which {
    Front,
    New,
    Past,
    Ask,
    Show,
    Packs,
    Evidence,
    Themes,
}

/// `page` from a query: absent is 1; anything but a number from 1 to 20 is 404.
pub fn page_of(query: Option<&str>) -> Result<usize, AppError> {
    let Some(raw) = query else { return Ok(1) };
    let value = url::form_urlencoded::parse(raw.as_bytes()).find(|(k, _)| k == "page");
    let Some((_, value)) = value else {
        return Ok(1);
    };
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(AppError::not_found());
    }
    match value.parse::<usize>() {
        Ok(n) if (1..=policy::PAGE_MAX).contains(&n) => Ok(n),
        _ => Err(AppError::not_found()),
    }
}

/// `day` from a query, as `YYYY-MM-DD`: `None` when absent; anything else
/// that is not a calendar date is 400.
fn day_of(query: Option<&str>) -> Result<Option<Date>, AppError> {
    let Some(raw) = query else { return Ok(None) };
    let value = url::form_urlencoded::parse(raw.as_bytes()).find(|(k, _)| k == "day");
    let Some((_, value)) = value else {
        return Ok(None);
    };
    let bad = || AppError::bad_request("The day must be written YYYY-MM-DD, such as 2026-11-07.");
    let b = value.as_bytes();
    let shape = b.len() == 10
        && b.iter().enumerate().all(|(i, c)| {
            if i == 4 || i == 7 {
                *c == b'-'
            } else {
                c.is_ascii_digit()
            }
        });
    if !shape {
        return Err(bad());
    }
    let number = |range: std::ops::Range<usize>| value[range].parse::<i32>().map_err(|_| bad());
    let month = u8::try_from(number(5..7)?)
        .ok()
        .and_then(|m| time::Month::try_from(m).ok())
        .ok_or_else(bad)?;
    let day = u8::try_from(number(8..10)?).map_err(|_| bad())?;
    Date::from_calendar_date(number(0..4)?, month, day)
        .map(Some)
        .map_err(|_| bad())
}

/// The rows of one page of a listing, numbered from `first`.
pub fn rows_of(
    conn: &Connection,
    config: &Config,
    member: Option<&Member>,
    listing: &Listing,
    page: usize,
    now: OffsetDateTime,
    return_to: &str,
) -> rusqlite::Result<Vec<PostRow>> {
    let first = (page - 1) * policy::PAGE_SIZE;
    listing
        .entries
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            row_of(
                conn,
                config,
                member,
                &entry.rec,
                entry.points,
                first + i + 1,
                now,
                return_to,
            )
        })
        .collect()
}

fn body(rows: &[PostRow], more: Option<String>) -> Markup {
    html! {
        @if rows.is_empty() {
            p class="hp-notice" { "Nothing here yet." }
        } @else {
            (rows::list(rows))
        }
        @if let Some(href) = more {
            p { a class="hp-more" href=(href) rel="next" { "More" } }
        }
    }
}

/// The links of `/past` to the day before, and to the day after unless the
/// day is yesterday or later.
fn past_links(day: Date, now: OffsetDateTime) -> Markup {
    let before = day.previous_day();
    let after = day.next_day().filter(|_| day < rank::yesterday(now));
    html! {
        p class="hp-help" {
            @if let Some(d) = before {
                a href=(format!("/past?day={d}")) rel="prev" { "Day before" }
            }
            @if before.is_some() && after.is_some() { " | " }
            @if let Some(d) = after {
                a href=(format!("/past?day={d}")) rel="next" { "Day after" }
            }
        }
    }
}

async fn show(
    ctx: Ctx,
    which: Which,
    query: Option<String>,
    path: &'static str,
) -> Result<Response, AppError> {
    let page = page_of(query.as_deref())?;
    let state = ctx.state.clone();
    let member = ctx.member.clone();
    let return_to = ctx.return_to.clone();
    let now = state.clock.now();
    let day = if which == Which::Past {
        Some(day_of(query.as_deref())?.unwrap_or_else(|| rank::yesterday(now)))
    } else {
        None
    };
    let (rows, more) = state
        .db
        .read(move |conn| {
            let viewer = member.as_ref().map(|m| m.id);
            let listing = match which {
                Which::Front => rank::front(conn, viewer, now, page)?,
                Which::New => rank::newest(conn, viewer, now, page)?,
                Which::Past => rank::past(
                    conn,
                    viewer,
                    now,
                    day.unwrap_or_else(|| rank::yesterday(now)),
                    page,
                )?,
                Which::Ask => rank::ask(conn, viewer, now, page)?,
                Which::Show => rank::show(conn, viewer, now, page)?,
                Which::Packs => rank::packs(conn, viewer, now, page)?,
                Which::Evidence => rank::by_evidence(conn, viewer, now, page)?,
                Which::Themes => rank::themes(conn, viewer, now, page)?,
            };
            let rows = rows_of(
                conn,
                &state.config,
                member.as_ref(),
                &listing,
                page,
                now,
                &return_to,
            )?;
            Ok((rows, listing.more))
        })
        .await?;
    let (name, h1, nav) = match which {
        Which::Front => ("front", "Front page".to_string(), None),
        Which::New => ("new", "New".to_string(), Some(Nav::New)),
        Which::Past => (
            "past",
            format!("Past: {}", day.map(|d| d.to_string()).unwrap_or_default()),
            Some(Nav::Past),
        ),
        Which::Ask => ("ask", "Ask".to_string(), Some(Nav::Ask)),
        Which::Show => ("show", "Show".to_string(), Some(Nav::Show)),
        Which::Packs => ("packs", "Packs".to_string(), Some(Nav::Packs)),
        Which::Evidence => ("evidence", "Evidence".to_string(), Some(Nav::Evidence)),
        Which::Themes => ("themes", "Themes".to_string(), None),
    };
    let more = more.then(|| match day {
        Some(d) => format!("{path}?day={d}&page={}", page + 1),
        None => format!("{path}?page={}", page + 1),
    });
    let mut view = ctx.page(name, h1).hidden_h1();
    if let Some(nav) = nav {
        view = view.current(nav);
    }
    let links = day.map(|d| past_links(d, now));
    Ok(view
        .body(html! {
            @if let Some(links) = links { (links) }
            (body(&rows, more))
        })
        .into_response())
}

/// `GET /`.
pub async fn front(ctx: Ctx, RawQuery(q): RawQuery) -> Result<Response, AppError> {
    show(ctx, Which::Front, q, "/").await
}

/// `GET /new`.
pub async fn new(ctx: Ctx, RawQuery(q): RawQuery) -> Result<Response, AppError> {
    show(ctx, Which::New, q, "/new").await
}

/// `GET /past`.
pub async fn past(ctx: Ctx, RawQuery(q): RawQuery) -> Result<Response, AppError> {
    show(ctx, Which::Past, q, "/past").await
}

/// `GET /ask`.
pub async fn ask(ctx: Ctx, RawQuery(q): RawQuery) -> Result<Response, AppError> {
    show(ctx, Which::Ask, q, "/ask").await
}

/// `GET /show`.
pub async fn show_list(ctx: Ctx, RawQuery(q): RawQuery) -> Result<Response, AppError> {
    show(ctx, Which::Show, q, "/show").await
}

/// `GET /packs`.
pub async fn packs(ctx: Ctx, RawQuery(q): RawQuery) -> Result<Response, AppError> {
    show(ctx, Which::Packs, q, "/packs").await
}

/// `GET /evidence`.
pub async fn evidence(ctx: Ctx, RawQuery(q): RawQuery) -> Result<Response, AppError> {
    show(ctx, Which::Evidence, q, "/evidence").await
}

/// `GET /themes`.
pub async fn themes(ctx: Ctx, RawQuery(q): RawQuery) -> Result<Response, AppError> {
    show(ctx, Which::Themes, q, "/themes").await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_numbers() {
        assert_eq!(page_of(None).unwrap(), 1);
        assert_eq!(page_of(Some("x=1")).unwrap(), 1);
        assert_eq!(page_of(Some("page=20")).unwrap(), 20);
        for bad in [
            "page=0",
            "page=21",
            "page=",
            "page=a",
            "page=%2B1",
            "page=-1",
        ] {
            assert!(page_of(Some(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn days() {
        assert_eq!(day_of(None).unwrap(), None);
        assert_eq!(day_of(Some("page=2")).unwrap(), None);
        assert_eq!(
            day_of(Some("day=2026-11-07&page=2")).unwrap(),
            Some(time::macros::date!(2026 - 11 - 07))
        );
        for bad in [
            "day=2026-13-01",
            "day=2026-02-30",
            "day=2026-1-1",
            "day=",
            "day=x",
            "day=2026-11-071",
            "day=%2B026-11-07",
            "day=2026/11/07",
        ] {
            assert_eq!(day_of(Some(bad)).unwrap_err().status.as_u16(), 400, "{bad}");
        }
    }
}
