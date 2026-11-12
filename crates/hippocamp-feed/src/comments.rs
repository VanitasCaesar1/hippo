//! Comments: create, reply, edit, delete, the state rules that moderation
//! calls, the thread of a post and the `/comments` list.
//!
//! Every function that writes takes the `&Transaction` of the change it makes
//! and the time as a parameter, so every decision reads the one `Clock`. The
//! callers (pages) apply `limits::check` and `limits::content_rule`;
//! [`create_checked`] and [`edit_checked`] do both for them.

use std::collections::HashMap;

use axum::http::StatusCode;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use time::{Duration, OffsetDateTime};

use crate::clock;
use crate::db::{Member, UserId};
use crate::error::{AppError, FormError};
use crate::limits::{self, Action};
use crate::markdown;
use crate::page::{self, Page};
use crate::policy;
use crate::text;
use crate::users::{self, Standing};
use crate::votes::{self, Target};

/// The text a deleted comment holds.
pub const DELETED_TEXT: &str = "[deleted]";
/// What others see of a hidden comment.
pub const HIDDEN_PLACEHOLDER: &str = "Hidden while a moderator reviews it.";

/// A comment's body: 1 to 10,000 bytes after the input rules.
pub fn parse_body(raw: &str) -> Result<String, AppError> {
    let body = text::body("body", raw, policy::COMMENT_BODY_MAX_BYTES).map_err(AppError::form)?;
    if body.is_empty() {
        return Err(AppError::form(FormError::new("body", "is required")));
    }
    Ok(body)
}

fn conflict(message: impl Into<String>) -> AppError {
    AppError::new(StatusCode::CONFLICT, message)
}

fn may_publish(member: &Member, now: OffsetDateTime) -> Result<Standing, AppError> {
    match users::standing(member, now) {
        Standing::Restricted | Standing::Banned => Err(AppError::forbidden(
            "A restricted account cannot comment or edit.",
        )),
        s => Ok(s),
    }
}

/// What a new comment needs of its post and parent: the depth it will have.
fn check_target(tx: &Transaction, post_id: i64, parent: Option<i64>) -> Result<i64, AppError> {
    let (state, locked): (String, bool) = tx
        .query_row(
            "SELECT state, locked FROM posts WHERE id = ?1",
            [post_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
        .ok_or_else(AppError::not_found)?;
    if state != "visible" {
        return Err(AppError::not_found());
    }
    if locked {
        return Err(AppError::forbidden("This post is locked: no new comments."));
    }
    let depth = match parent {
        None => 0,
        Some(parent_id) => {
            let (parent_post, parent_state, parent_depth): (i64, String, i64) = tx
                .query_row(
                    "SELECT post_id, state, depth FROM comments WHERE id = ?1",
                    [parent_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?
                .ok_or_else(AppError::not_found)?;
            if parent_post != post_id || parent_state != "visible" {
                return Err(AppError::not_found());
            }
            if parent_depth + 1 > i64::from(policy::COMMENT_DEPTH_MAX) {
                return Err(AppError::form(FormError::new(
                    "body",
                    format!(
                        "replies can be at most {} levels deep",
                        policy::COMMENT_DEPTH_MAX
                    ),
                )));
            }
            parent_depth + 1
        }
    };
    let count: i64 = tx.query_row(
        "SELECT COUNT(*) FROM comments WHERE post_id = ?1",
        [post_id],
        |r| r.get(0),
    )?;
    if count >= i64::from(policy::COMMENTS_PER_POST_MAX) {
        return Err(AppError::form(FormError::new(
            "body",
            format!(
                "a post takes at most {} comments",
                policy::COMMENTS_PER_POST_MAX
            ),
        )));
    }
    Ok(depth)
}

fn insert(
    tx: &Transaction,
    member: &Member,
    post_id: i64,
    parent: Option<i64>,
    depth: i64,
    body: &str,
    now: OffsetDateTime,
) -> Result<i64, AppError> {
    tx.execute(
        "INSERT INTO comments (post_id, parent_id, depth, author_id, body, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            post_id,
            parent,
            depth,
            member.id.0,
            body,
            clock::storage(now)
        ],
    )?;
    Ok(tx.last_insert_rowid())
}

/// Creates a top-level comment (`parent` is `None`) or a reply by `member`
/// and returns its id. `body` is a [`parse_body`] result. Refused: a
/// restricted or banned member (403); a post that is missing or not visible,
/// or a parent that is missing, on another post or not visible (404); a
/// locked post (403); a reply deeper than 10 levels and a post that already
/// has 1,000 comments of any state (422). Limits and content rules are the
/// caller's; see [`create_checked`].
pub fn create(
    tx: &Transaction,
    member: &Member,
    post_id: i64,
    parent: Option<i64>,
    body: &str,
    now: OffsetDateTime,
) -> Result<i64, AppError> {
    may_publish(member, now)?;
    let depth = check_target(tx, post_id, parent)?;
    insert(tx, member, post_id, parent, depth, body, now)
}

fn content_check(standing: Standing, body: &str) -> Result<(), AppError> {
    let (_, links) = markdown::render(body);
    limits::content_rule(body, links, None, standing)
        .map_err(|rule| AppError::form(rule.form_error("body")))
}

/// [`create`] with the input rules, the per-member comment limit and the
/// content rule, in the order a page reports them: standing, target, body,
/// limit (429 or 403), content rule (422).
pub fn create_checked(
    tx: &Transaction,
    member: &Member,
    post_id: i64,
    parent: Option<i64>,
    raw_body: &str,
    now: OffsetDateTime,
) -> Result<i64, AppError> {
    let standing = may_publish(member, now)?;
    let depth = check_target(tx, post_id, parent)?;
    let body = parse_body(raw_body)?;
    limits::check(tx, member.id, Action::Comment, now).map_err(|l| l.into_error(now))?;
    content_check(standing, &body)?;
    insert(tx, member, post_id, parent, depth, &body, now)
}

struct Row {
    author_id: Option<i64>,
    state: String,
    created_at: String,
}

fn load(tx: &Transaction, id: i64) -> Result<Row, AppError> {
    tx.query_row(
        "SELECT author_id, state, created_at FROM comments WHERE id = ?1",
        [id],
        |r| {
            Ok(Row {
                author_id: r.get(0)?,
                state: r.get(1)?,
                created_at: r.get(2)?,
            })
        },
    )
    .optional()?
    .ok_or_else(AppError::not_found)
}

/// Edits a visible comment's body. Only its author may, and only within 2
/// hours of creating it; the edit stamps `edited_at`. `body` is a
/// [`parse_body`] result.
pub fn edit(
    tx: &Transaction,
    member: &Member,
    id: i64,
    body: &str,
    now: OffsetDateTime,
) -> Result<(), AppError> {
    let row = load(tx, id)?;
    if row.author_id != Some(member.id.0) {
        return Err(AppError::forbidden(
            "Only the author can edit this comment.",
        ));
    }
    may_publish(member, now)?;
    if row.state != "visible" {
        return Err(AppError::forbidden(
            "A comment that is not visible cannot be edited.",
        ));
    }
    let created = clock::parse_storage(&row.created_at).unwrap_or(OffsetDateTime::UNIX_EPOCH);
    if now - created > Duration::hours(policy::EDIT_WINDOW_HOURS) {
        return Err(AppError::forbidden(format!(
            "A comment can be edited for {} hours after it is created.",
            policy::EDIT_WINDOW_HOURS
        )));
    }
    tx.execute(
        "UPDATE comments SET body = ?1, edited_at = ?2 WHERE id = ?3",
        params![body, clock::storage(now), id],
    )?;
    Ok(())
}

/// [`edit`] with the input rules and the content rule again.
pub fn edit_checked(
    tx: &Transaction,
    member: &Member,
    id: i64,
    raw_body: &str,
    now: OffsetDateTime,
) -> Result<(), AppError> {
    let standing = may_publish(member, now)?;
    let body = parse_body(raw_body)?;
    content_check(standing, &body)?;
    edit(tx, member, id, &body, now)
}

/// Deletes a comment at its author's request, in any state but `deleted`, a
/// restricted author included: the body becomes `[deleted]` and any removal
/// reason is cleared. Open reports are untouched.
pub fn delete(tx: &Transaction, member: &Member, id: i64) -> Result<(), AppError> {
    let row = load(tx, id)?;
    if row.author_id != Some(member.id.0) {
        return Err(AppError::forbidden(
            "Only the author can delete this comment.",
        ));
    }
    if row.state == "deleted" {
        return Err(conflict("This comment is already deleted."));
    }
    tx.execute(
        "UPDATE comments SET body = ?1, removed_reason = NULL, state = 'deleted' WHERE id = ?2",
        params![DELETED_TEXT, id],
    )?;
    Ok(())
}

fn transition(
    tx: &Transaction,
    id: i64,
    from: &[&str],
    to: &str,
    reason: Option<&str>,
) -> Result<(), AppError> {
    let row = load(tx, id)?;
    if !from.contains(&row.state.as_str()) {
        return Err(conflict(format!(
            "A {} comment cannot become {to}.",
            row.state
        )));
    }
    tx.execute(
        "UPDATE comments SET state = ?1, removed_reason = ?2 WHERE id = ?3",
        params![to, reason, id],
    )?;
    Ok(())
}

/// A moderator removes a visible or hidden comment, with the reason the
/// public placeholder shows. 404 for a missing comment, 409 for any other
/// state.
pub fn remove(tx: &Transaction, id: i64, reason: &str) -> Result<(), AppError> {
    let reason = reason.trim();
    if reason.is_empty() || reason.chars().any(|c| c.is_control()) {
        return Err(AppError::form(FormError::new(
            "reason",
            "is required, on one line",
        )));
    }
    transition(tx, id, &["visible", "hidden"], "removed", Some(reason))
}

/// A moderator restores a hidden or removed comment, clearing its removal
/// reason.
pub fn restore(tx: &Transaction, id: i64) -> Result<(), AppError> {
    transition(tx, id, &["hidden", "removed"], "visible", None)
}

/// Reports hide a visible comment. Returns whether it changed: a comment in
/// any other state, or none, is left as it is.
pub fn hide_by_reports(tx: &Transaction, id: i64) -> rusqlite::Result<bool> {
    let n = tx.execute(
        "UPDATE comments SET state = 'hidden' WHERE id = ?1 AND state = 'visible'",
        [id],
    )?;
    Ok(n > 0)
}

/// What a page shows in place of a comment's text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shown<'a> {
    Text(&'a str),
    Placeholder(String),
}

/// One comment of a thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentNode {
    pub id: i64,
    pub parent_id: Option<i64>,
    /// Levels below a top-level comment (0 for one).
    pub depth: u32,
    pub author_id: Option<i64>,
    pub author_login: Option<String>,
    /// The state: `visible`, `hidden`, `removed` or `deleted`.
    pub state: String,
    /// The stored text. A page shows it through [`CommentNode::shown`], so a
    /// removed or hidden text reaches moderators only.
    pub body: String,
    pub removed_reason: Option<String>,
    pub created_at: OffsetDateTime,
    pub edited_at: Option<OffsetDateTime>,
    /// Counted votes; 0 for a comment that is not visible.
    pub points: i64,
}

impl CommentNode {
    /// What to show: the text of a visible comment, `[deleted]` for a deleted
    /// one, and for a hidden or removed one the placeholder, or the text for a
    /// moderator.
    pub fn shown(&self, moderator: bool) -> Shown<'_> {
        match self.state.as_str() {
            "visible" => Shown::Text(&self.body),
            "hidden" if moderator => Shown::Text(&self.body),
            "hidden" => Shown::Placeholder(HIDDEN_PLACEHOLDER.to_string()),
            "removed" if moderator => Shown::Text(&self.body),
            "removed" => Shown::Placeholder(format!(
                "[removed: {}]",
                self.removed_reason.as_deref().unwrap_or("")
            )),
            _ => Shown::Placeholder(DELETED_TEXT.to_string()),
        }
    }
}

fn time(text: Option<String>) -> Option<OffsetDateTime> {
    text.as_deref().and_then(clock::parse_storage)
}

/// The comments of a post in thread order, as a flat list in which each
/// comment is followed by its replies (read `depth` to indent): siblings go
/// by counted votes (most first), then age (oldest first), then id. A comment
/// that is not visible stays only when it has a visible descendant, so a
/// placeholder keeps its replies; one with none is left out with whatever it
/// covers.
pub fn thread(
    conn: &Connection,
    post_id: i64,
    now: OffsetDateTime,
) -> rusqlite::Result<Vec<CommentNode>> {
    let mut stmt = conn.prepare(
        "SELECT c.id, c.parent_id, c.depth, c.author_id, u.login, c.state, c.body,
                c.removed_reason, c.created_at, c.edited_at
         FROM comments c LEFT JOIN users u ON u.id = c.author_id
         WHERE c.post_id = ?1",
    )?;
    let mut nodes: Vec<CommentNode> = stmt
        .query_map([post_id], |r| {
            Ok(CommentNode {
                id: r.get(0)?,
                parent_id: r.get(1)?,
                depth: r.get(2)?,
                author_id: r.get(3)?,
                author_login: r.get(4)?,
                state: r.get(5)?,
                body: r.get(6)?,
                removed_reason: r.get(7)?,
                created_at: time(r.get(8)?).unwrap_or(OffsetDateTime::UNIX_EPOCH),
                edited_at: time(r.get(9)?),
                points: 0,
            })
        })?
        .collect::<Result<_, _>>()?;
    let targets: Vec<Target> = nodes
        .iter()
        .filter(|n| n.state == "visible")
        .map(|n| Target::Comment(n.id))
        .collect();
    let counts = votes::counted_votes(conn, &targets, now)?;
    for node in &mut nodes {
        node.points = counts.get(&Target::Comment(node.id)).copied().unwrap_or(0);
    }

    let mut children: HashMap<Option<i64>, Vec<usize>> = HashMap::new();
    for (i, node) in nodes.iter().enumerate() {
        children.entry(node.parent_id).or_default().push(i);
    }
    for list in children.values_mut() {
        list.sort_by(|&a, &b| {
            let (a, b) = (&nodes[a], &nodes[b]);
            b.points
                .cmp(&a.points)
                .then(a.created_at.cmp(&b.created_at))
                .then(a.id.cmp(&b.id))
        });
    }
    // Depth is at most 10, so recursion is shallow.
    fn keeps(i: usize, nodes: &[CommentNode], children: &HashMap<Option<i64>, Vec<usize>>) -> bool {
        nodes[i].state == "visible"
            || children
                .get(&Some(nodes[i].id))
                .is_some_and(|kids| kids.iter().any(|&k| keeps(k, nodes, children)))
    }
    fn walk(
        parent: Option<i64>,
        nodes: &[CommentNode],
        children: &HashMap<Option<i64>, Vec<usize>>,
        out: &mut Vec<usize>,
    ) {
        for &i in children.get(&parent).map_or(&[][..], Vec::as_slice) {
            if keeps(i, nodes, children) {
                out.push(i);
                walk(Some(nodes[i].id), nodes, children, out);
            }
        }
    }
    let mut order = Vec::new();
    walk(None, &nodes, &children, &mut order);
    let mut slots: Vec<Option<CommentNode>> = nodes.into_iter().map(Some).collect();
    Ok(order.into_iter().filter_map(|i| slots[i].take()).collect())
}

/// A row of `/comments`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewestComment {
    pub id: i64,
    pub post_id: i64,
    pub post_title: String,
    pub parent_id: Option<i64>,
    pub author_id: Option<i64>,
    pub author_login: Option<String>,
    pub body: String,
    pub created_at: OffsetDateTime,
    pub edited_at: Option<OffsetDateTime>,
    pub points: i64,
}

/// The visible comments on visible posts, newest first (then higher id), 30 a
/// page from page 1 to 20 (an empty page after that), leaving out the posts
/// `viewer` has hidden.
pub fn newest(
    conn: &Connection,
    viewer: Option<UserId>,
    now: OffsetDateTime,
    page: usize,
) -> rusqlite::Result<Page<NewestComment>> {
    if page > policy::PAGE_MAX {
        return Ok(Page::empty());
    }
    let mut stmt = conn.prepare(
        "SELECT c.id, c.post_id, p.title, c.parent_id, c.author_id, u.login, c.body,
                c.created_at, c.edited_at
         FROM comments c
         JOIN posts p ON p.id = c.post_id
         LEFT JOIN users u ON u.id = c.author_id
         WHERE c.state = 'visible' AND p.state = 'visible'
           AND NOT EXISTS (SELECT 1 FROM hides h WHERE h.user_id = ?1 AND h.post_id = p.id)
         ORDER BY c.created_at DESC, c.id DESC
         LIMIT ?2 OFFSET ?3",
    )?;
    let mut rows: Vec<NewestComment> = stmt
        .query_map(
            params![
                viewer.map(|v| v.0),
                (policy::PAGE_SIZE + 1) as i64,
                page::offset(page, policy::PAGE_SIZE) as i64
            ],
            |r| {
                Ok(NewestComment {
                    id: r.get(0)?,
                    post_id: r.get(1)?,
                    post_title: r.get(2)?,
                    parent_id: r.get(3)?,
                    author_id: r.get(4)?,
                    author_login: r.get(5)?,
                    body: r.get(6)?,
                    created_at: time(r.get(7)?).unwrap_or(OffsetDateTime::UNIX_EPOCH),
                    edited_at: time(r.get(8)?),
                    points: 0,
                })
            },
        )?
        .collect::<Result<_, _>>()?;
    let result = Page::from_overfetched(std::mem::take(&mut rows), policy::PAGE_SIZE);
    let mut rows = result.rows;
    let targets: Vec<Target> = rows.iter().map(|c| Target::Comment(c.id)).collect();
    let counts = votes::counted_votes(conn, &targets, now)?;
    for row in &mut rows {
        row.points = counts.get(&Target::Comment(row.id)).copied().unwrap_or(0);
    }
    Ok(Page {
        rows,
        more: result.more,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::posts::testkit::*;
    use time::macros::datetime;

    const NOW: OffsetDateTime = datetime!(2026-11-08 10:00:00 UTC);

    fn old() -> OffsetDateTime {
        NOW - Duration::days(400)
    }

    struct Setup {
        _dir: tempfile::TempDir,
        conn: Connection,
        ada: i64,
        bob: i64,
        post: i64,
    }

    fn setup() -> Setup {
        let (dir, conn) = fresh();
        let ada = user(&conn, "ada", old(), old());
        let bob = user(&conn, "bob", old(), old());
        let post = post(&conn, ada, "a post", NOW - Duration::hours(1));
        Setup {
            _dir: dir,
            conn,
            ada,
            bob,
            post,
        }
    }

    fn add(
        s: &Setup,
        who: i64,
        parent: Option<i64>,
        body: &str,
        at: OffsetDateTime,
    ) -> Result<i64, AppError> {
        let m = member(&s.conn, who);
        let tx = s.conn.unchecked_transaction().expect("tx");
        let id = create(&tx, &m, s.post, parent, body, at)?;
        tx.commit().expect("commit");
        Ok(id)
    }

    fn ids(nodes: &[CommentNode]) -> Vec<i64> {
        nodes.iter().map(|n| n.id).collect()
    }

    #[test]
    fn body_rules() {
        assert_eq!(parse_body(" a\r\nb ").as_deref(), Ok("a\nb"));
        assert_eq!(
            parse_body("  ").expect_err("empty").status,
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert!(parse_body(&"a".repeat(10_000)).is_ok());
        assert!(parse_body(&"a".repeat(10_001)).is_err());
        assert!(
            parse_body(&"é".repeat(5_001)).is_err(),
            "bytes, not scalars"
        );
        assert!(parse_body("a\u{0}b").is_err());
    }

    #[test]
    fn comments_and_replies_nest_with_their_depth() {
        let s = setup();
        let top = add(&s, s.bob, None, "top", NOW).expect("top");
        let reply = add(&s, s.ada, Some(top), "reply", NOW).expect("reply");
        let deeper = add(&s, s.bob, Some(reply), "deeper", NOW).expect("deeper");
        let depth = |id: i64| -> i64 {
            s.conn
                .query_row("SELECT depth FROM comments WHERE id = ?1", [id], |r| {
                    r.get(0)
                })
                .expect("depth")
        };
        assert_eq!((depth(top), depth(reply), depth(deeper)), (0, 1, 2));
        let tree = thread(&s.conn, s.post, NOW).expect("thread");
        assert_eq!(ids(&tree), vec![top, reply, deeper]);
        assert_eq!(
            tree.iter().map(|n| n.depth).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        assert_eq!(tree[1].parent_id, Some(top));
        assert_eq!(tree[0].author_login.as_deref(), Some("bob"));
    }

    #[test]
    fn replies_stop_at_ten_levels() {
        let s = setup();
        let mut parent = add(&s, s.bob, None, "level 0", NOW).expect("top");
        for level in 1..=10 {
            parent = add(&s, s.bob, Some(parent), "r", NOW)
                .unwrap_or_else(|e| panic!("level {level}: {e}"));
        }
        let e = add(&s, s.bob, Some(parent), "too deep", NOW).expect_err("level 11");
        assert_eq!(e.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(e.message.contains("10 levels"), "{}", e.message);
        // A top-level comment is still fine.
        add(&s, s.bob, None, "again", NOW).expect("top");
    }

    #[test]
    fn a_post_takes_1000_comments_counting_every_state() {
        let s = setup();
        let first = add(&s, s.bob, None, "first", NOW).expect("first");
        s.conn
            .execute(
                "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 998)
                 INSERT INTO comments (post_id, depth, author_id, body, created_at)
                 SELECT ?1, 0, ?2, 'c', ?3 FROM n",
                params![s.post, s.bob, clock::storage(NOW)],
            )
            .expect("bulk");
        // 999 so far; the 1,000th is fine.
        state(&s.conn, "comments", first, "deleted");
        add(&s, s.bob, None, "1000th", NOW).expect("1000th");
        let e = add(&s, s.bob, None, "1001st", NOW).expect_err("1001st");
        assert_eq!(e.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(e.message.contains("1000") || e.message.contains("1,000"));
        let e = add(&s, s.bob, Some(first), "reply", NOW).expect_err("reply to deleted");
        assert_eq!(
            e.status,
            StatusCode::NOT_FOUND,
            "the parent check comes first"
        );
    }

    #[test]
    fn replies_need_a_visible_parent_on_the_same_post() {
        let s = setup();
        let other_post = post(&s.conn, s.ada, "other", NOW);
        let top = add(&s, s.bob, None, "top", NOW).expect("top");
        for st in ["hidden", "removed", "deleted"] {
            state(&s.conn, "comments", top, st);
            let e = add(&s, s.ada, Some(top), "r", NOW).expect_err(st);
            assert_eq!(e.status, StatusCode::NOT_FOUND, "{st}");
        }
        state(&s.conn, "comments", top, "visible");
        add(&s, s.ada, Some(top), "ok", NOW).expect("visible parent");
        // A parent of another post, and a missing parent.
        let m = member(&s.conn, s.ada);
        let tx = s.conn.unchecked_transaction().expect("tx");
        assert_eq!(
            create(&tx, &m, other_post, Some(top), "x", NOW)
                .expect_err("other post")
                .status,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            create(&tx, &m, s.post, Some(999), "x", NOW)
                .expect_err("missing")
                .status,
            StatusCode::NOT_FOUND
        );
    }

    #[test]
    fn posts_that_are_not_visible_or_are_locked_take_no_comments() {
        let s = setup();
        for st in ["hidden", "removed", "deleted"] {
            state(&s.conn, "posts", s.post, st);
            let e = add(&s, s.bob, None, "c", NOW).expect_err(st);
            assert_eq!(e.status, StatusCode::NOT_FOUND, "{st}");
        }
        state(&s.conn, "posts", s.post, "visible");
        s.conn
            .execute("UPDATE posts SET locked = 1 WHERE id = ?1", [s.post])
            .expect("lock");
        let e = add(&s, s.bob, None, "c", NOW).expect_err("locked");
        assert_eq!(e.status, StatusCode::FORBIDDEN);
        s.conn
            .execute("UPDATE posts SET locked = 0 WHERE id = ?1", [s.post])
            .expect("unlock");
        add(&s, s.bob, None, "c", NOW).expect("unlocked");
        let m = member(&s.conn, s.bob);
        let tx = s.conn.unchecked_transaction().expect("tx");
        assert_eq!(
            create(&tx, &m, 999, None, "c", NOW)
                .expect_err("missing")
                .status,
            StatusCode::NOT_FOUND
        );
    }

    #[test]
    fn restricted_and_banned_members_cannot_comment() {
        let s = setup();
        let mut m = member(&s.conn, s.bob);
        m.restricted_until = Some(NOW + Duration::days(1));
        let tx = s.conn.unchecked_transaction().expect("tx");
        let e = create(&tx, &m, s.post, None, "c", NOW).expect_err("restricted");
        assert_eq!(e.status, StatusCode::FORBIDDEN);
        m.restricted_until = None;
        m.banned = true;
        assert!(create(&tx, &m, s.post, None, "c", NOW).is_err());
    }

    #[test]
    fn the_edit_window_is_two_hours_for_the_author_of_a_visible_comment() {
        let s = setup();
        let id = add(&s, s.bob, None, "old", NOW).expect("c");
        let bob = member(&s.conn, s.bob);
        let ada = member(&s.conn, s.ada);
        let tx = s.conn.unchecked_transaction().expect("tx");
        let at = NOW + Duration::hours(2);
        edit(&tx, &bob, id, "new", at).expect("edit at 2h");
        let (body, edited): (String, String) = tx
            .query_row(
                "SELECT body, edited_at FROM comments WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("row");
        assert_eq!((body.as_str(), edited), ("new", clock::storage(at)));
        let late = at + Duration::seconds(1);
        assert_eq!(
            edit(&tx, &bob, id, "x", late).expect_err("late").status,
            StatusCode::FORBIDDEN
        );
        assert!(edit(&tx, &ada, id, "x", NOW).is_err(), "not the author");
        assert_eq!(
            edit(&tx, &bob, 999, "x", NOW).expect_err("m").status,
            StatusCode::NOT_FOUND
        );
        for st in ["hidden", "removed", "deleted"] {
            state(&tx, "comments", id, st);
            assert!(edit(&tx, &bob, id, "x", NOW).is_err(), "{st}");
        }
        state(&tx, "comments", id, "visible");
        let mut restricted = bob.clone();
        restricted.restricted_until = Some(NOW + Duration::days(1));
        assert!(edit(&tx, &restricted, id, "x", NOW).is_err());
    }

    #[test]
    fn delete_clears_the_text_in_any_state_but_deleted() {
        let s = setup();
        let bob = member(&s.conn, s.bob);
        let ada = member(&s.conn, s.ada);
        for st in ["visible", "hidden", "removed"] {
            let id = add(&s, s.bob, None, "secret", NOW).expect("c");
            s.conn
                .execute(
                    "UPDATE comments SET state = ?1, removed_reason = 'spam' WHERE id = ?2",
                    params![st, id],
                )
                .expect("setup");
            let tx = s.conn.unchecked_transaction().expect("tx");
            assert!(delete(&tx, &ada, id).is_err(), "not the author");
            delete(&tx, &bob, id).expect(st);
            let (b, state, r): (String, String, Option<String>) = tx
                .query_row(
                    "SELECT body, state, removed_reason FROM comments WHERE id = ?1",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .expect("row");
            assert_eq!(
                (b.as_str(), state.as_str(), r),
                ("[deleted]", "deleted", None)
            );
            assert_eq!(
                delete(&tx, &bob, id).expect_err("twice").status,
                StatusCode::CONFLICT
            );
            tx.commit().expect("commit");
        }
        // A restricted author may delete.
        let id = add(&s, s.bob, None, "x", NOW).expect("c");
        let mut restricted = bob;
        restricted.restricted_until = Some(NOW + Duration::days(1));
        let tx = s.conn.unchecked_transaction().expect("tx");
        delete(&tx, &restricted, id).expect("restricted delete");
    }

    #[test]
    fn state_transitions_follow_the_table() {
        let s = setup();
        let id = add(&s, s.bob, None, "x", NOW).expect("c");
        let tx = s.conn.unchecked_transaction().expect("tx");
        let st = |tx: &Transaction| -> (String, Option<String>) {
            tx.query_row(
                "SELECT state, removed_reason FROM comments WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("state")
        };
        assert_eq!(hide_by_reports(&tx, id), Ok(true));
        assert_eq!(hide_by_reports(&tx, id), Ok(false));
        remove(&tx, id, " spam ").expect("remove");
        assert_eq!(st(&tx), ("removed".into(), Some("spam".into())));
        assert_eq!(hide_by_reports(&tx, id), Ok(false));
        assert_eq!(
            remove(&tx, id, "spam").expect_err("twice").status,
            StatusCode::CONFLICT
        );
        restore(&tx, id).expect("restore");
        assert_eq!(st(&tx), ("visible".into(), None));
        assert!(restore(&tx, id).is_err());
        assert!(remove(&tx, id, " ").is_err());
        state(&tx, "comments", id, "deleted");
        assert!(remove(&tx, id, "spam").is_err());
        assert!(restore(&tx, id).is_err());
        assert_eq!(
            restore(&tx, 999).expect_err("m").status,
            StatusCode::NOT_FOUND
        );
    }

    #[test]
    fn the_thread_is_ordered_by_points_then_age_then_id() {
        let s = setup();
        // Voters, established.
        let voters: Vec<i64> = (0..3)
            .map(|i| user(&s.conn, &format!("v{i}"), old(), old()))
            .collect();
        let t = |m: i64| NOW - Duration::minutes(m);
        let oldest = add(&s, s.bob, None, "oldest", t(50)).expect("c");
        let newer = add(&s, s.bob, None, "newer", t(40)).expect("c");
        let popular = add(&s, s.bob, None, "popular", t(30)).expect("c");
        let same_time_a = add(&s, s.bob, None, "a", t(20)).expect("c");
        let same_time_b = add(&s, s.bob, None, "b", t(20)).expect("c");
        // Replies sort the same way inside their parent.
        let r_old = add(&s, s.ada, Some(oldest), "r1", t(10)).expect("c");
        let r_voted = add(&s, s.ada, Some(oldest), "r2", t(5)).expect("c");
        cast(&s.conn, voters[0], "comment", popular, NOW);
        cast(&s.conn, voters[1], "comment", popular, NOW);
        cast(&s.conn, voters[0], "comment", r_voted, NOW);
        // An author's own vote does not count: ada votes on her reply.
        cast(&s.conn, s.ada, "comment", r_old, NOW);
        let tree = thread(&s.conn, s.post, NOW).expect("thread");
        assert_eq!(
            ids(&tree),
            vec![
                popular,
                oldest,
                r_voted,
                r_old,
                newer,
                same_time_a,
                same_time_b
            ]
        );
        let points: Vec<i64> = tree.iter().map(|n| n.points).collect();
        assert_eq!(points, vec![2, 0, 1, 0, 0, 0, 0]);
        // A vote from a new member does not count until established.
        let fresh_user = user(&s.conn, "fresh", NOW - Duration::days(1), old());
        cast(&s.conn, fresh_user, "comment", newer, NOW);
        let tree = thread(&s.conn, s.post, NOW).expect("thread");
        assert_eq!(tree[0].id, popular);
        assert_eq!(
            tree.iter().find(|n| n.id == newer).map(|n| n.points),
            Some(0)
        );
        let later = NOW + Duration::days(3);
        let tree = thread(&s.conn, s.post, later).expect("thread later");
        assert_eq!(
            ids(&tree)[..3],
            [popular, newer, oldest],
            "newer now has a point"
        );
        assert_eq!(
            tree.iter().find(|n| n.id == newer).map(|n| n.points),
            Some(1)
        );
    }

    #[test]
    fn placeholders_keep_their_replies_and_childless_ones_go() {
        let s = setup();
        let removed_parent = add(&s, s.bob, None, "removed", NOW).expect("c");
        let reply = add(&s, s.ada, Some(removed_parent), "reply", NOW).expect("c");
        let nested = add(&s, s.bob, Some(reply), "nested", NOW).expect("c");
        let lonely = add(&s, s.bob, None, "lonely", NOW).expect("c");
        let lonely_child = add(&s, s.ada, Some(lonely), "lc", NOW).expect("c");
        let kept = add(&s, s.ada, None, "kept", NOW).expect("c");
        s.conn
            .execute(
                "UPDATE comments SET state = 'removed', removed_reason = 'spam' WHERE id = ?1",
                [removed_parent],
            )
            .expect("remove");
        state(&s.conn, "comments", lonely, "hidden");
        state(&s.conn, "comments", lonely_child, "deleted");
        let tree = thread(&s.conn, s.post, NOW).expect("thread");
        assert_eq!(ids(&tree), vec![removed_parent, reply, nested, kept]);
        assert_eq!(
            tree[0].shown(false),
            Shown::Placeholder("[removed: spam]".into())
        );
        assert_eq!(tree[0].shown(true), Shown::Text("removed"));
        assert_eq!(tree[1].shown(false), Shown::Text("reply"));
        // A deleted comment with a visible reply stays as [deleted].
        state(&s.conn, "comments", removed_parent, "deleted");
        let tree = thread(&s.conn, s.post, NOW).expect("thread");
        assert_eq!(tree[0].shown(true), Shown::Placeholder("[deleted]".into()));
        // Hidden: others see the review notice, moderators the text.
        state(&s.conn, "comments", removed_parent, "hidden");
        let tree = thread(&s.conn, s.post, NOW).expect("thread");
        assert_eq!(
            tree[0].shown(false),
            Shown::Placeholder("Hidden while a moderator reviews it.".into())
        );
        assert_eq!(tree[0].shown(true), Shown::Text("removed"));
        // Without the visible descendants the placeholder goes too.
        state(&s.conn, "comments", reply, "removed");
        state(&s.conn, "comments", nested, "deleted");
        let tree = thread(&s.conn, s.post, NOW).expect("thread");
        assert_eq!(ids(&tree), vec![kept]);
    }

    #[test]
    fn newest_lists_visible_comments_on_visible_posts_leaving_out_hidden_posts() {
        let s = setup();
        let p2 = post(&s.conn, s.bob, "second", NOW - Duration::hours(1));
        let p3 = post(&s.conn, s.bob, "third", NOW - Duration::hours(1));
        let t = |m: i64| NOW - Duration::minutes(m);
        let c1 = comment(&s.conn, s.post, s.bob, t(30));
        let c2 = comment(&s.conn, p2, s.ada, t(20));
        let tie_a = comment(&s.conn, s.post, s.bob, t(10));
        let tie_b = comment(&s.conn, s.post, s.ada, t(10));
        let hidden_comment = comment(&s.conn, s.post, s.ada, t(5));
        let on_removed_post = comment(&s.conn, p3, s.ada, t(4));
        state(&s.conn, "comments", hidden_comment, "hidden");
        state(&s.conn, "posts", p3, "removed");
        let page = newest(&s.conn, None, NOW, 1).expect("newest");
        assert_eq!(
            page.rows.iter().map(|c| c.id).collect::<Vec<_>>(),
            vec![tie_b, tie_a, c2, c1]
        );
        assert!(!page.more);
        assert!(page.rows.iter().all(|c| c.id != on_removed_post));
        assert_eq!(page.rows[2].post_title, "second");
        assert_eq!(page.rows[0].author_login.as_deref(), Some("ada"));
        // Ada hid the second post.
        s.conn
            .execute(
                "INSERT INTO hides (user_id, post_id, at) VALUES (?1, ?2, ?3)",
                params![s.ada, p2, clock::storage(NOW)],
            )
            .expect("hide");
        let page = newest(&s.conn, Some(UserId(s.ada)), NOW, 1).expect("newest");
        assert_eq!(
            page.rows.iter().map(|c| c.id).collect::<Vec<_>>(),
            vec![tie_b, tie_a, c1]
        );
        let page = newest(&s.conn, Some(UserId(s.bob)), NOW, 1).expect("newest");
        assert_eq!(page.rows.len(), 4, "hiding is per member");
    }

    #[test]
    fn newest_pages_by_thirty_up_to_page_twenty() {
        let s = setup();
        s.conn
            .execute(
                "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 65)
                 INSERT INTO comments (post_id, depth, author_id, body, created_at)
                 SELECT ?1, 0, ?2, 'c', strftime('%Y-%m-%dT%H:%M:%S.000000Z', '2026-11-08 09:00:00', '-' || i || ' minutes') FROM n",
                params![s.post, s.bob],
            )
            .expect("bulk");
        let p1 = newest(&s.conn, None, NOW, 1).expect("p1");
        assert_eq!((p1.rows.len(), p1.more), (30, true));
        let p3 = newest(&s.conn, None, NOW, 3).expect("p3");
        assert_eq!((p3.rows.len(), p3.more), (5, false));
        assert_eq!(newest(&s.conn, None, NOW, 0).expect("p0").rows.len(), 30);
        assert!(newest(&s.conn, None, NOW, 21).expect("p21").rows.is_empty());
        assert!(p1.rows[0].created_at > p1.rows[29].created_at);
    }

    #[test]
    fn points_show_on_newest() {
        let s = setup();
        let c = comment(&s.conn, s.post, s.bob, NOW - Duration::minutes(1));
        let v = user(&s.conn, "voter", old(), old());
        cast(&s.conn, v, "comment", c, NOW);
        let page = newest(&s.conn, None, NOW, 1).expect("newest");
        assert_eq!(page.rows[0].points, 1);
    }

    #[test]
    fn create_checked_applies_the_limit_and_the_content_rule() {
        let s = setup();
        let newcomer = user(&s.conn, "new", NOW - Duration::days(1), old());
        let new_member = member(&s.conn, newcomer);
        let est = member(&s.conn, s.bob);
        let tx = s.conn.unchecked_transaction().expect("tx");
        // Content rules for a new member.
        for text in [
            "see https://example.com",
            "see [x](https://example.com)",
            "HTTP://X",
        ] {
            let e = create_checked(&tx, &new_member, s.post, None, text, NOW).expect_err(text);
            assert_eq!(e.status, StatusCode::UNPROCESSABLE_ENTITY, "{text}");
        }
        assert!(create_checked(&tx, &est, s.post, None, "see https://example.com", NOW).is_ok());
        // A new member's 6th comment in 24 hours is over the limit.
        for i in 0..5 {
            create_checked(&tx, &new_member, s.post, None, &format!("c{i}"), NOW)
                .unwrap_or_else(|e| panic!("{i}: {e}"));
        }
        let e = create_checked(&tx, &new_member, s.post, None, "c6", NOW).expect_err("limit");
        assert_eq!(e.status, StatusCode::TOO_MANY_REQUESTS);
        assert!(e.retry_after.is_some());
        // Bad body and a locked post come first.
        assert_eq!(
            create_checked(&tx, &est, s.post, None, " ", NOW)
                .expect_err("empty")
                .status,
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(
            create_checked(&tx, &est, 999, None, "x", NOW)
                .expect_err("m")
                .status,
            StatusCode::NOT_FOUND
        );
    }

    #[test]
    fn edit_checked_applies_the_content_rule_again() {
        let s = setup();
        let newcomer = user(&s.conn, "new", NOW - Duration::days(1), old());
        let new_member = member(&s.conn, newcomer);
        let id = {
            let tx = s.conn.unchecked_transaction().expect("tx");
            let id = create(&tx, &new_member, s.post, None, "plain", NOW).expect("c");
            tx.commit().expect("commit");
            id
        };
        let tx = s.conn.unchecked_transaction().expect("tx");
        assert!(edit_checked(&tx, &new_member, id, "now https://example.com", NOW).is_err());
        edit_checked(&tx, &new_member, id, " still plain ", NOW).expect("edit");
        let body: String = tx
            .query_row("SELECT body FROM comments WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .expect("body");
        assert_eq!(body, "still plain");
    }
}
