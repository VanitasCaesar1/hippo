//! The read-only API v1.
//!
//! Two unauthenticated `GET` endpoints, `/api/v1/posts` and
//! `/api/v1/posts/{id}`. The JSON is built from the structs below, so every
//! field name lives here. Within v1 fields are only added. The session is
//! ignored (so is hiding), no cookie or CORS header is written, and theme
//! packs appear in no list. Errors are `{"error": ...}`, rendered by the
//! response layer under `/api/`.

use std::collections::HashMap;

use axum::extract::{Path, RawQuery, State};
use axum::http::{header, HeaderValue};
use axum::response::{IntoResponse, Response};
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use time::OffsetDateTime;

use crate::clock;
use crate::comments::{self, CommentNode};
use crate::config::Config;
use crate::deeplink;
use crate::error::AppError;
use crate::evidence::{self, ContentEvidence, Fraction, Status};
use crate::policy;
use crate::rank::{self, Rec, REC_COLUMNS};
use crate::registry::{Kind as PackKind, PackRow};
use crate::votes::{self, Target};
use crate::web::pages::list::pack_row;
use crate::web::AppState;

/// `Cache-Control` of every successful API response.
const CACHE: &str = "public, max-age=30";

// --- The JSON shapes ---

/// `GET /api/v1/posts`.
#[derive(Debug, Serialize)]
pub struct PostList {
    pub sort: &'static str,
    pub page: usize,
    pub posts: Vec<ApiPost>,
}

/// A post, as in a list.
#[derive(Debug, Serialize)]
pub struct ApiPost {
    pub id: i64,
    /// `"pack"` or `"discussion"`.
    pub kind: &'static str,
    /// Null when the post is not visible.
    pub title: Option<String>,
    pub url: String,
    /// A discussion's link URL; null otherwise and when not visible.
    pub link: Option<String>,
    /// The author's login; null for a pack's post or a deleted author.
    pub author: Option<String>,
    pub created_at: String,
    pub edited_at: Option<String>,
    /// `visible`, `hidden`, `removed` or `deleted`.
    pub state: String,
    pub votes: i64,
    pub comments: i64,
    pub pack: Option<ApiPack>,
}

#[derive(Debug, Serialize)]
pub struct ApiPack {
    pub name: String,
    /// `"passes"` or `"theme"`.
    pub kind: &'static str,
    pub version: String,
    pub description: String,
    pub owner: String,
    pub maintainers: Vec<String>,
    pub first_party: bool,
    pub license: String,
    pub source: Option<String>,
    pub pack_hash: String,
    pub listed: bool,
    pub passes: Vec<ApiPass>,
    pub install: Option<ApiInstall>,
    pub evidence: Option<ApiEvidence>,
}

#[derive(Debug, Serialize)]
pub struct ApiPass {
    pub name: String,
    pub reads: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct ApiInstall {
    pub command: String,
    pub deep_link: String,
}

#[derive(Debug, Serialize)]
pub struct ApiEvidence {
    pub score: f64,
    pub measurements: u32,
    pub median_benefit: f64,
    pub version: String,
    pub pack_hash: String,
}

/// `GET /api/v1/posts/{id}`.
#[derive(Debug, Serialize)]
pub struct PostPage {
    pub post: PostDetail,
    pub measurements: Vec<ApiMeasurement>,
    pub comments: Vec<ApiComment>,
}

/// A post with its text.
#[derive(Debug, Serialize)]
pub struct PostDetail {
    #[serde(flatten)]
    pub post: ApiPost,
    /// Markdown; empty for a pack's post; null when the post is not visible.
    pub body: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ApiMeasurement {
    pub id: i64,
    pub member: String,
    pub created_at: String,
    pub updated_at: String,
    pub version: String,
    pub pack_hash: String,
    pub eval_set: String,
    pub hippo: String,
    pub model: String,
    pub runs: i64,
    pub net_without: f64,
    pub net_with: f64,
    pub benefit: f64,
    pub revisions: i64,
    /// `counted`, `maintainer`, `not_counted` or `invalid`.
    pub status: &'static str,
    pub report_url: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ApiComment {
    pub id: i64,
    /// Null when the comment is not visible.
    pub author: Option<String>,
    pub created_at: String,
    pub edited_at: Option<String>,
    pub votes: i64,
    /// Markdown; null when the comment is not visible.
    pub body: Option<String>,
    pub state: String,
    pub children: Vec<ApiComment>,
}

// --- Building them ---

/// Rounded half away from zero to 3 decimals, with no negative zero.
fn round3(value: f64) -> f64 {
    let rounded = (value * 1000.0).round() / 1000.0;
    if rounded == 0.0 {
        0.0
    } else {
        rounded
    }
}

fn fraction3(f: Fraction) -> f64 {
    round3(f.to_f64())
}

fn rfc3339(stored: &str) -> String {
    clock::storage_to_rfc3339(stored).unwrap_or_default()
}

fn status_str(status: Status) -> &'static str {
    match status {
        Status::Counted => "counted",
        Status::Maintainers => "maintainer",
        Status::NotCounted => "not_counted",
        Status::Invalid => "invalid",
    }
}

fn post_url(config: &Config, id: i64) -> String {
    let base = config.base_url.as_deref().unwrap_or("");
    format!("{}/p/{id}", base.trim_end_matches('/'))
}

fn pack_json(
    config: &Config,
    visible: bool,
    pack: &PackRow,
    contents: &[ContentEvidence],
) -> ApiPack {
    let theme = pack.kind == PackKind::Theme;
    let install = deeplink::install_links(visible, pack, config).map(|l| ApiInstall {
        command: l.command,
        deep_link: l.deep_link,
    });
    let evidence = if theme {
        None
    } else {
        contents.iter().find_map(|c| {
            c.evidence.map(|e| ApiEvidence {
                score: round3(e.score),
                measurements: e.n,
                median_benefit: fraction3(e.median),
                version: c.version.clone(),
                pack_hash: c.pack_hash.clone(),
            })
        })
    };
    ApiPack {
        name: pack.name.clone(),
        kind: if theme { "theme" } else { "passes" },
        version: pack.version.clone(),
        description: pack.description.clone(),
        owner: pack.owner.clone(),
        maintainers: pack.maintainers.clone(),
        first_party: pack.first_party,
        license: pack.license.clone(),
        source: pack.source_link(),
        pack_hash: pack.pack_hash.clone(),
        listed: pack.listed,
        passes: if theme {
            Vec::new()
        } else {
            pack.passes
                .iter()
                .map(|p| ApiPass {
                    name: p.name.clone(),
                    reads: p.reads.clone(),
                })
                .collect()
        },
        install,
        evidence,
    }
}

/// One post and, for a pack's post, the contents with their measurements.
fn post_json(
    conn: &Connection,
    config: &Config,
    rec: &Rec,
    points: i64,
    now: OffsetDateTime,
) -> rusqlite::Result<(ApiPost, Vec<ContentEvidence>)> {
    let visible = rec.state == "visible";
    let edited_at: Option<String> =
        conn.query_row("SELECT edited_at FROM posts WHERE id = ?1", [rec.id], |r| {
            r.get(0)
        })?;
    let comments: i64 = conn.query_row(
        "SELECT COUNT(*) FROM comments WHERE post_id = ?1 AND state = 'visible'",
        [rec.id],
        |r| r.get(0),
    )?;
    let (pack, contents) = match &rec.pack {
        Some(name) => match pack_row(conn, name)? {
            Some(row) => {
                let contents = if row.kind == PackKind::Passes {
                    evidence::contents(conn, name, now)?
                } else {
                    Vec::new()
                };
                (Some(pack_json(config, visible, &row, &contents)), contents)
            }
            None => (None, Vec::new()),
        },
        None => (None, Vec::new()),
    };
    let post = ApiPost {
        id: rec.id,
        kind: if rec.pack.is_some() {
            "pack"
        } else {
            "discussion"
        },
        title: visible.then(|| rec.title.clone()),
        url: post_url(config, rec.id),
        link: if visible && rec.pack.is_none() {
            rec.link.clone()
        } else {
            None
        },
        author: if rec.pack.is_some() {
            None
        } else {
            rec.author_login.clone()
        },
        created_at: rfc3339(&rec.created_at),
        edited_at: edited_at.as_deref().map(rfc3339),
        state: rec.state.clone(),
        votes: points,
        comments,
        pack,
    };
    Ok((post, contents))
}

/// Every measurement of every content, newest first.
fn measurements_json(
    conn: &Connection,
    pack: &str,
    contents: &[ContentEvidence],
) -> rusqlite::Result<Vec<ApiMeasurement>> {
    let mut stmt = conn.prepare("SELECT id, updated_at FROM measurements WHERE pack = ?1")?;
    let updated: HashMap<i64, String> = stmt
        .query_map([pack], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut all: Vec<(&str, i64, ApiMeasurement)> = Vec::new();
    for content in contents {
        for m in &content.measurements {
            all.push((
                &m.created_at,
                m.id,
                ApiMeasurement {
                    id: m.id,
                    member: m.member.clone().unwrap_or_else(|| "[deleted]".to_string()),
                    created_at: rfc3339(&m.created_at),
                    updated_at: updated
                        .get(&m.id)
                        .map_or_else(|| rfc3339(&m.created_at), |stored| rfc3339(stored)),
                    version: content.version.clone(),
                    pack_hash: content.pack_hash.clone(),
                    eval_set: m.eval_set.clone(),
                    hippo: m.hippo.clone(),
                    model: m.model.clone(),
                    runs: m.runs,
                    net_without: fraction3(m.net_without),
                    net_with: fraction3(m.net_with),
                    benefit: fraction3(m.benefit),
                    revisions: m.revisions,
                    status: status_str(m.status),
                    report_url: m.report_url.clone(),
                },
            ));
        }
    }
    all.sort_by(|a, b| b.0.cmp(a.0).then(b.1.cmp(&a.1)));
    Ok(all.into_iter().map(|(_, _, m)| m).collect())
}

fn comment_json(node: &CommentNode, children: Vec<ApiComment>) -> ApiComment {
    let visible = node.state == "visible";
    ApiComment {
        id: node.id,
        author: if visible {
            node.author_login.clone()
        } else {
            None
        },
        created_at: clock::rfc3339(node.created_at),
        edited_at: node.edited_at.map(clock::rfc3339),
        votes: node.points,
        body: visible.then(|| node.body.clone()),
        state: node.state.clone(),
        children,
    }
}

/// The nested tree of a thread given in thread order (each comment followed by
/// its replies). Depth is bounded when comments are written.
fn tree(flat: &[CommentNode]) -> Vec<ApiComment> {
    let mut kids: HashMap<Option<i64>, Vec<usize>> = HashMap::new();
    for (i, node) in flat.iter().enumerate() {
        kids.entry(node.parent_id).or_default().push(i);
    }
    fn build(
        parent: Option<i64>,
        flat: &[CommentNode],
        kids: &HashMap<Option<i64>, Vec<usize>>,
        depth: u32,
    ) -> Vec<ApiComment> {
        if depth > policy::COMMENT_DEPTH_MAX {
            return Vec::new();
        }
        kids.get(&parent)
            .map_or(&[][..], Vec::as_slice)
            .iter()
            .map(|&i| {
                let node = &flat[i];
                comment_json(node, build(Some(node.id), flat, kids, depth + 1))
            })
            .collect()
    }
    build(None, flat, &kids, 0)
}

// --- Handlers ---

fn json_response<T: Serialize>(value: &T) -> Result<Response, AppError> {
    let body = serde_json::to_vec(value).map_err(AppError::internal)?;
    let mut response = body.into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(CACHE));
    Ok(response)
}

fn query_value(query: Option<&str>, key: &str) -> Option<String> {
    url::form_urlencoded::parse(query?.as_bytes())
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
}

/// `GET /api/v1/posts?sort=front|new|evidence&page=<n>`.
pub async fn list(
    State(state): State<AppState>,
    RawQuery(query): RawQuery,
) -> Result<Response, AppError> {
    let sort = match query_value(query.as_deref(), "sort").as_deref() {
        None | Some("front") => "front",
        Some("new") => "new",
        Some("evidence") => "evidence",
        Some(_) => {
            return Err(AppError::bad_request(
                "sort must be front, new or evidence.",
            ))
        }
    };
    let page = match query_value(query.as_deref(), "page") {
        None => 1,
        Some(v) => match v.parse::<usize>() {
            Ok(n)
                if (1..=policy::PAGE_MAX).contains(&n) && v.bytes().all(|b| b.is_ascii_digit()) =>
            {
                n
            }
            _ => {
                return Err(AppError::bad_request(format!(
                    "page must be a number from 1 to {}.",
                    policy::PAGE_MAX
                )))
            }
        },
    };
    let now = state.clock.now();
    let config = state.config.clone();
    let list = PostList {
        sort,
        page,
        posts: state
            .db
            .read(move |conn| {
                let listing = match sort {
                    "new" => rank::newest(conn, None, now, page)?,
                    "evidence" => rank::by_evidence(conn, None, now, page)?,
                    _ => rank::front(conn, None, now, page)?,
                };
                listing
                    .entries
                    .iter()
                    .map(|e| {
                        post_json(conn, &config, &e.rec, e.points, now)
                            .map(|(post, _)| post)
                            .map_err(AppError::from)
                    })
                    .collect()
            })
            .await?,
    };
    json_response(&list)
}

/// `GET /api/v1/posts/{id}`.
pub async fn post(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
        return Err(AppError::not_found());
    }
    let id: i64 = id.parse().map_err(|_| AppError::not_found())?;
    if id < 1 {
        return Err(AppError::not_found());
    }
    let now = state.clock.now();
    let config = state.config.clone();
    let page = state
        .db
        .read(move |conn| {
            let found = conn
                .query_row(
                    &format!(
                        "SELECT {REC_COLUMNS}, p.body FROM posts p
                         LEFT JOIN users u ON u.id = p.author_id WHERE p.id = ?1"
                    ),
                    [id],
                    |r| Ok((Rec::from_row(r)?, r.get::<_, String>(8)?)),
                )
                .optional()?;
            let (rec, body) = found.ok_or_else(AppError::not_found)?;
            let visible = rec.state == "visible";
            let points = votes::counted_votes(conn, &[Target::Post(id)], now)?[&Target::Post(id)];
            let (post, contents) = post_json(conn, &config, &rec, points, now)?;
            let measurements = match &rec.pack {
                Some(name) => measurements_json(conn, name, &contents)?,
                None => Vec::new(),
            };
            // A post that is not visible shows no comments.
            let comments = if visible {
                tree(&comments::thread(conn, id, now)?)
            } else {
                Vec::new()
            };
            let body = (visible).then(|| {
                if rec.pack.is_some() {
                    String::new()
                } else {
                    body
                }
            });
            Ok(PostPage {
                post: PostDetail { post, body },
                measurements,
                comments,
            })
        })
        .await?;
    json_response(&page)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounding_is_to_three_decimals_with_no_negative_zero() {
        assert_eq!(round3(0.14285714), 0.143);
        assert_eq!(round3(-0.0004), 0.0);
        assert!(round3(-0.0004).is_sign_positive());
        assert_eq!(round3(0.2), 0.2);
        assert_eq!(round3(-0.6249), -0.625);
    }

    #[test]
    fn statuses_use_the_api_words() {
        assert_eq!(status_str(Status::Maintainers), "maintainer");
        assert_eq!(status_str(Status::NotCounted), "not_counted");
    }
}
