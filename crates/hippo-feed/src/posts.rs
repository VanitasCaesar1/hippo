//! Discussion posts: input rules, create, edit, delete, the state rules that
//! moderation calls, and a member's own hiding.
//!
//! This module owns no list queries; `rank.rs` does. Every function that
//! writes takes the `&Transaction` of the change it makes and the time as a
//! parameter, so every decision reads the one `Clock`.

use rusqlite::{params, OptionalExtension, Transaction};
use time::{Duration, OffsetDateTime};
use url::Url;

use crate::clock;
use crate::db::{Member, UserId};
use crate::error::{AppError, FormError};
use crate::policy;
use crate::text;
use crate::users::{self, Standing};
use axum::http::StatusCode;

/// The title prefix of an ask post.
pub const ASK_PREFIX: &str = "Ask hippo:";
/// The title prefix of a show post.
pub const SHOW_PREFIX: &str = "Show hippo:";
/// The text a deleted post's title and body hold.
pub const DELETED_TEXT: &str = "[deleted]";

fn starts_with_ignore_case(title: &str, prefix: &str) -> bool {
    title
        .as_bytes()
        .get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix.as_bytes()))
}

/// Whether the title starts with `Ask hippo:`, ASCII case ignored.
pub fn is_ask(title: &str) -> bool {
    starts_with_ignore_case(title, ASK_PREFIX)
}

/// Whether the title starts with `Show hippo:`, ASCII case ignored.
pub fn is_show(title: &str) -> bool {
    starts_with_ignore_case(title, SHOW_PREFIX)
}

/// Checks a link URL and returns it as the `url` crate writes it: `http` or
/// `https` with a host, no user name or password, at most 2,000 bytes as
/// stored. Surrounding whitespace is trimmed; a control character inside
/// (which `url` would silently drop) is refused.
pub fn parse_link(raw: &str) -> Result<String, FormError> {
    const FIELD: &str = "link";
    let trimmed = text::normalise(raw);
    if trimmed.is_empty() {
        return Err(FormError::new(FIELD, "is required"));
    }
    if trimmed.chars().any(|c| c.is_control()) {
        return Err(FormError::new(FIELD, "may not contain control characters"));
    }
    let url = Url::parse(&trimmed)
        .map_err(|_| FormError::new(FIELD, "must be an http or https address"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(FormError::new(FIELD, "must start with http:// or https://"));
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err(FormError::new(FIELD, "must name a host"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(FormError::new(
            FIELD,
            "may not contain a user name or password",
        ));
    }
    let stored = url.to_string();
    if stored.len() > policy::LINK_MAX_BYTES {
        return Err(FormError::new(
            FIELD,
            format!("must be at most {} bytes", policy::LINK_MAX_BYTES),
        ));
    }
    Ok(stored)
}

/// The domain a row shows for a link URL as stored: its host without a
/// leading `www.` and without a port; an internationalised name in its ASCII
/// form and an IP address as the `url` crate writes them. `None` if the text
/// is not a URL with a host.
pub fn domain(link: &str) -> Option<String> {
    let url = Url::parse(link).ok()?;
    let host = url.host_str().filter(|h| !h.is_empty())?;
    Some(match host.strip_prefix("www.") {
        Some(rest) if !rest.is_empty() => rest.to_string(),
        _ => host.to_string(),
    })
}

/// The domain a pack's post shows: `github.com/<owner>`.
pub fn pack_domain(owner: &str) -> String {
    format!("github.com/{owner}")
}

/// A discussion's fields after the input rules; the content rules are the
/// caller's next step (they need the Markdown renderer's link count).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discussion {
    pub title: String,
    pub link: Option<String>,
    pub body: String,
}

impl Discussion {
    /// A title of 1 to 120 characters; a link URL, a body of 1 to 20,000
    /// bytes, or both. An empty `link` or `body` means none.
    pub fn parse(title: &str, link: &str, body: &str) -> Result<Self, AppError> {
        Self::parse_inner(title, link, body).map_err(AppError::form)
    }

    fn parse_inner(title: &str, link: &str, body: &str) -> Result<Self, FormError> {
        let title = text::one_line("title", title, policy::TITLE_MAX)?;
        let link = if text::normalise(link).is_empty() {
            None
        } else {
            Some(parse_link(link)?)
        };
        let body = text::body("body", body, policy::POST_BODY_MAX_BYTES)?;
        if link.is_none() && body.is_empty() {
            return Err(FormError::new("body", "give a link, a text, or both"));
        }
        Ok(Self { title, link, body })
    }
}

fn conflict(message: impl Into<String>) -> AppError {
    AppError::new(StatusCode::CONFLICT, message)
}

fn may_publish(member: &Member, now: OffsetDateTime) -> Result<(), AppError> {
    match users::standing(member, now) {
        Standing::Restricted | Standing::Banned => Err(AppError::forbidden(
            "A restricted account cannot post or edit.",
        )),
        Standing::New | Standing::Established => Ok(()),
    }
}

/// Creates a discussion post by `member` and returns its id. A restricted or
/// banned member cannot post. Limits and content rules are the caller's.
pub fn create_discussion(
    tx: &Transaction,
    member: &Member,
    discussion: &Discussion,
    now: OffsetDateTime,
) -> Result<i64, AppError> {
    may_publish(member, now)?;
    tx.execute(
        "INSERT INTO posts (kind, author_id, title, link, body, created_at)
         VALUES ('discussion', ?1, ?2, ?3, ?4, ?5)",
        params![
            member.id.0,
            discussion.title,
            discussion.link,
            discussion.body,
            clock::storage(now)
        ],
    )?;
    Ok(tx.last_insert_rowid())
}

struct Row {
    kind: String,
    author_id: Option<i64>,
    state: String,
    created_at: String,
}

fn load(tx: &Transaction, id: i64) -> Result<Row, AppError> {
    tx.query_row(
        "SELECT kind, author_id, state, created_at FROM posts WHERE id = ?1",
        [id],
        |r| {
            Ok(Row {
                kind: r.get(0)?,
                author_id: r.get(1)?,
                state: r.get(2)?,
                created_at: r.get(3)?,
            })
        },
    )
    .optional()?
    .ok_or_else(AppError::not_found)
}

/// Edits a visible discussion's title, link URL and body. Only its author may,
/// and only within 2 hours of creating it (the edit stamps `edited_at`).
pub fn edit_discussion(
    tx: &Transaction,
    member: &Member,
    id: i64,
    discussion: &Discussion,
    now: OffsetDateTime,
) -> Result<(), AppError> {
    let row = load(tx, id)?;
    if row.kind != "discussion" || row.author_id != Some(member.id.0) {
        return Err(AppError::forbidden("Only the author can edit this post."));
    }
    may_publish(member, now)?;
    if row.state != "visible" {
        return Err(AppError::forbidden(
            "A post that is not visible cannot be edited.",
        ));
    }
    let created = clock::parse_storage(&row.created_at).unwrap_or(OffsetDateTime::UNIX_EPOCH);
    if now - created > Duration::hours(policy::EDIT_WINDOW_HOURS) {
        return Err(AppError::forbidden(format!(
            "A post can be edited for {} hours after it is created.",
            policy::EDIT_WINDOW_HOURS
        )));
    }
    tx.execute(
        "UPDATE posts SET title = ?1, link = ?2, body = ?3, edited_at = ?4 WHERE id = ?5",
        params![
            discussion.title,
            discussion.link,
            discussion.body,
            clock::storage(now),
            id
        ],
    )?;
    Ok(())
}

/// Deletes a post at its author's request, in any state but `deleted`, a
/// restricted author included: the title and body become `[deleted]`, the link
/// URL and any removal reason are cleared, and the state is `deleted`. Open
/// reports are untouched.
pub fn delete_post(tx: &Transaction, member: &Member, id: i64) -> Result<(), AppError> {
    let row = load(tx, id)?;
    if row.author_id != Some(member.id.0) {
        return Err(AppError::forbidden("Only the author can delete this post."));
    }
    if row.state == "deleted" {
        return Err(conflict("This post is already deleted."));
    }
    tx.execute(
        "UPDATE posts SET title = ?1, body = ?1, link = NULL, removed_reason = NULL,
                state = 'deleted'
         WHERE id = ?2",
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
            "A {} post cannot become {to}.",
            row.state
        )));
    }
    tx.execute(
        "UPDATE posts SET state = ?1, removed_reason = ?2 WHERE id = ?3",
        params![to, reason, id],
    )?;
    Ok(())
}

/// A moderator removes a visible or hidden post, with the reason the public
/// placeholder shows. 404 for a missing post, 409 for any other state.
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

/// A moderator restores a hidden or removed post, clearing its removal reason.
pub fn restore(tx: &Transaction, id: i64) -> Result<(), AppError> {
    transition(tx, id, &["hidden", "removed"], "visible", None)
}

/// Reports hide a visible post. Returns whether it changed: a post in any
/// other state, or none, is left as it is, so a late third report cannot undo
/// a removal.
pub fn hide_by_reports(tx: &Transaction, id: i64) -> rusqlite::Result<bool> {
    let n = tx.execute(
        "UPDATE posts SET state = 'hidden' WHERE id = ?1 AND state = 'visible'",
        [id],
    )?;
    Ok(n > 0)
}

fn set_locked(tx: &Transaction, id: i64, locked: bool) -> Result<(), AppError> {
    let n = tx.execute(
        "UPDATE posts SET locked = ?1 WHERE id = ?2",
        params![locked, id],
    )?;
    if n == 0 {
        return Err(AppError::not_found());
    }
    Ok(())
}

/// A moderator locks a post: it takes no new comments but still takes votes.
pub fn lock(tx: &Transaction, id: i64) -> Result<(), AppError> {
    set_locked(tx, id, true)
}

/// A moderator unlocks a post.
pub fn unlock(tx: &Transaction, id: i64) -> Result<(), AppError> {
    set_locked(tx, id, false)
}

/// A member hides a visible post from their own lists. A post that is missing
/// or not visible answers 404; hiding twice changes nothing (the first time
/// stays). Restricted members may hide.
pub fn hide(tx: &Transaction, user: UserId, id: i64, now: OffsetDateTime) -> Result<(), AppError> {
    let visible: bool = tx
        .query_row(
            "SELECT state = 'visible' FROM posts WHERE id = ?1",
            [id],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(false);
    if !visible {
        return Err(AppError::not_found());
    }
    tx.execute(
        "INSERT OR IGNORE INTO hides (user_id, post_id, at) VALUES (?1, ?2, ?3)",
        params![user.0, id, clock::storage(now)],
    )?;
    Ok(())
}

/// A member un-hides a post they hid, whatever its state now; a post that is
/// not hidden by them is left alone.
pub fn unhide(tx: &Transaction, user: UserId, id: i64) -> rusqlite::Result<()> {
    tx.execute(
        "DELETE FROM hides WHERE user_id = ?1 AND post_id = ?2",
        params![user.0, id],
    )?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod testkit {
    //! Rows inserted by SQL on a migrated temporary database, shared by the
    //! tests of posts, votes and limits.

    use rusqlite::{params, Connection};
    use time::OffsetDateTime;

    use crate::clock;
    use crate::db::{self, Member, UserId};
    use crate::users;

    pub fn fresh() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("feed.sqlite");
        db::migrate(&path).expect("migrate");
        let conn = db::open(&path).expect("open");
        (dir, conn)
    }

    /// A user with a numeric GitHub id of `1000 + n` and the given ages.
    pub fn user(
        conn: &Connection,
        login: &str,
        created: OffsetDateTime,
        github_created: OffsetDateTime,
    ) -> i64 {
        conn.execute(
            "INSERT INTO users (github_id, login, github_created_at, created_at)
             VALUES ((SELECT COALESCE(MAX(github_id), 1000) + 1 FROM users), ?1, ?2, ?3)",
            params![
                login,
                clock::storage(github_created),
                clock::storage(created)
            ],
        )
        .expect("user");
        conn.last_insert_rowid()
    }

    pub fn member(conn: &Connection, id: i64) -> Member {
        users::load_member(conn, UserId(id))
            .expect("query")
            .expect("member")
    }

    pub fn post(conn: &Connection, author: i64, title: &str, created: OffsetDateTime) -> i64 {
        conn.execute(
            "INSERT INTO posts (kind, author_id, title, body, created_at)
             VALUES ('discussion', ?1, ?2, 'body', ?3)",
            params![author, title, clock::storage(created)],
        )
        .expect("post");
        conn.last_insert_rowid()
    }

    /// A listed pack with a post; `maintainers` is the entry's JSON list.
    pub fn pack_post(
        conn: &Connection,
        name: &str,
        owner_id: i64,
        maintainers: &str,
        created: OffsetDateTime,
    ) -> i64 {
        let at = clock::storage(created);
        conn.execute(
            "INSERT INTO packs (name, version, description, authors, owner, owner_id,
                maintainers, first_party, license, repo, repo_id, path, commit_id, pack_hash,
                passes, review_id, review_at, listed, synced_at)
             VALUES (?1, '1', 'd', '[]', 'owner', ?2, ?3, 0, 'MIT', 'r', 1, '.', 'c', 'h',
                '[]', 'r', ?4, 1, ?4)",
            params![name, owner_id, maintainers, at],
        )
        .expect("pack");
        conn.execute(
            "INSERT INTO posts (kind, pack, title, created_at) VALUES ('pack', ?1, ?1, ?2)",
            params![name, at],
        )
        .expect("pack post");
        conn.last_insert_rowid()
    }

    /// A recorded content of a pack.
    pub fn content(
        conn: &Connection,
        pack: &str,
        hash: &str,
        version: &str,
        first_seen: OffsetDateTime,
    ) {
        conn.execute(
            "INSERT INTO pack_contents (pack, pack_hash, version, commit_id, first_seen)
             VALUES (?1, ?2, ?3, 'c', ?4)",
            params![pack, hash, version, clock::storage(first_seen)],
        )
        .expect("content");
    }

    /// A measurement by SQL (not through the entry form) of an eval set of
    /// 6 planted errors and 10 kept claims over 3 runs, with the benefit
    /// `(dc * 10 - dd * 6) / 180` for `dc` more corrections and `dd` more
    /// damaged claims with the pack than without it.
    pub fn measure(conn: &Connection, pack: &str, hash: &str, user: i64, dc: i64, dd: i64) -> i64 {
        // Without the pack: 6 corrected, 3 damaged; with it: 6 + dc, 3 + dd.
        conn.execute(
            "INSERT INTO measurements (pack, pack_hash, user_id, eval_set, planted, kept, runs,
                corrected_without, damaged_without, corrected_with, damaged_with,
                hippo, model, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'set-a', 6, 10, 3, 6, 3, ?4, ?5, '1.0.0', 'm',
                '2026-11-01T00:00:00.000000Z', '2026-11-01T00:00:00.000000Z')",
            params![pack, hash, user, 6 + dc, 3 + dd],
        )
        .expect("measurement");
        conn.last_insert_rowid()
    }

    pub fn comment(conn: &Connection, post: i64, author: i64, created: OffsetDateTime) -> i64 {
        conn.execute(
            "INSERT INTO comments (post_id, depth, author_id, body, created_at)
             VALUES (?1, 0, ?2, 'c', ?3)",
            params![post, author, clock::storage(created)],
        )
        .expect("comment");
        conn.last_insert_rowid()
    }

    pub fn state(conn: &Connection, table: &str, id: i64, state: &str) {
        conn.execute(
            &format!("UPDATE {table} SET state = ?1 WHERE id = ?2"),
            params![state, id],
        )
        .expect("state");
    }

    pub fn cast(conn: &Connection, user: i64, target_type: &str, target: i64, at: OffsetDateTime) {
        conn.execute(
            "INSERT INTO votes (user_id, target_type, target_id, at) VALUES (?1, ?2, ?3, ?4)",
            params![user, target_type, target, clock::storage(at)],
        )
        .expect("vote");
    }
}

#[cfg(test)]
mod tests {
    use super::testkit::*;
    use super::*;
    use time::macros::datetime;

    const NOW: OffsetDateTime = datetime!(2026-11-08 10:00:00 UTC);

    fn old() -> OffsetDateTime {
        NOW - Duration::days(400)
    }

    fn disc(title: &str, link: &str, body: &str) -> Result<Discussion, AppError> {
        Discussion::parse(title, link, body)
    }

    #[test]
    fn link_urls_of_each_refused_form() {
        for bad in [
            "ftp://example.com/x",
            "javascript:alert(1)",
            "mailto:a@example.com",
            "http://",
            "https://:8080/",
            "example.com",
            "https://user@example.com/",
            "https://user:pw@example.com/",
            "https://exa\nmple.com/",
        ] {
            assert!(parse_link(bad).is_err(), "{bad}");
        }
        let long = |n: usize| format!("https://example.com/{}", "a".repeat(n));
        // "https://example.com/" is 20 bytes, so 1,980 more reach 2,000.
        assert_eq!(parse_link(&long(1_980)).map(|s| s.len()), Ok(2_000));
        let e = parse_link(&long(1_981)).expect_err("2,001 bytes");
        assert_eq!(e.field, "link");
    }

    #[test]
    fn link_urls_are_stored_as_the_url_crate_writes_them() {
        assert_eq!(
            parse_link("  HTTP://Example.com ").as_deref(),
            Ok("http://example.com/")
        );
        assert_eq!(
            parse_link("https://example.com/a b").as_deref(),
            Ok("https://example.com/a%20b")
        );
    }

    #[test]
    fn domains() {
        let d = |l: &str| domain(&parse_link(l).expect("link"));
        assert_eq!(
            d("https://www.example.com/x").as_deref(),
            Some("example.com")
        );
        assert_eq!(d("https://example.com/x").as_deref(), Some("example.com"));
        assert_eq!(
            d("https://sub.www.example.com").as_deref(),
            Some("sub.www.example.com")
        );
        assert_eq!(
            d("http://example.com:8080/").as_deref(),
            Some("example.com")
        );
        assert_eq!(
            d("https://www.example.com:443/").as_deref(),
            Some("example.com")
        );
        assert_eq!(
            d("https://bücher.example/").as_deref(),
            Some("xn--bcher-kva.example")
        );
        assert_eq!(
            d("http://192.168.1.10:80/x").as_deref(),
            Some("192.168.1.10")
        );
        assert_eq!(d("http://[::1]/").as_deref(), Some("[::1]"));
        assert_eq!(domain("not a url"), None);
        assert_eq!(pack_domain("alice"), "github.com/alice");
    }

    #[test]
    fn ask_and_show_follow_the_title() {
        assert!(is_ask("Ask hippo: how do I?"));
        assert!(is_ask("ASK HIPPO: x"));
        assert!(!is_ask("Ask hippo how"));
        assert!(!is_ask("Please Ask hippo: x"));
        assert!(is_show("show HIPPO: my pack"));
        assert!(is_show("Show hippo:"));
        assert!(!is_show("Show hipp"));
        assert!(!is_show("Ask hippo: x"));
        // A multi-byte character inside the prefix length is no match, no panic.
        assert!(!is_ask("Äsk hippo: x"));
        assert!(!is_show("日本語日本語日本語日本語"));
    }

    #[test]
    fn discussion_input_rules() {
        let ok = disc("  A title \r\n", "", "body\r\ntext\n").expect("ok");
        assert_eq!(ok.title, "A title");
        assert_eq!(ok.body, "body\ntext");
        assert_eq!(ok.link, None);
        assert!(disc("t", "https://example.com", "").is_ok(), "link only");
        let neither = disc("t", " ", " \n").expect_err("neither");
        assert_eq!(neither.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(disc("", "https://example.com", "").is_err());
        assert!(disc(&"a".repeat(120), "", "b").is_ok());
        assert!(disc(&"a".repeat(121), "", "b").is_err());
        assert!(
            disc(&"é".repeat(120), "", "b").is_ok(),
            "scalars, not bytes"
        );
        assert!(disc("t\tx", "", "b").is_err());
        assert!(disc("t", "", &"a".repeat(20_000)).is_ok());
        assert!(disc("t", "", &"a".repeat(20_001)).is_err());
        assert!(
            disc("t", "", &"é".repeat(10_001)).is_err(),
            "bytes, not scalars"
        );
        assert!(disc("t", "", "a\u{0}b").is_err());
    }

    #[test]
    fn create_stores_the_stored_link_and_refuses_the_restricted() {
        let (_d, mut conn) = fresh();
        let uid = user(&conn, "ada", old(), old());
        let m = member(&conn, uid);
        let tx = conn.transaction().expect("tx");
        let d = disc("Show hippo: x", "HTTP://Example.com", "b").expect("d");
        let id = create_discussion(&tx, &m, &d, NOW).expect("create");
        let (link, created, state): (String, String, String) = tx
            .query_row(
                "SELECT link, created_at, state FROM posts WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .expect("row");
        assert_eq!(link, "http://example.com/");
        assert_eq!(created, clock::storage(NOW));
        assert_eq!(state, "visible");
        let mut restricted = m.clone();
        restricted.restricted_until = Some(NOW + Duration::days(1));
        let e = create_discussion(&tx, &restricted, &d, NOW).expect_err("restricted");
        assert_eq!(e.status, StatusCode::FORBIDDEN);
        let mut banned = m;
        banned.banned = true;
        assert!(create_discussion(&tx, &banned, &d, NOW).is_err());
    }

    #[test]
    fn edit_within_the_window_sets_edited_at_and_is_refused_after() {
        let (_d, mut conn) = fresh();
        let uid = user(&conn, "ada", old(), old());
        let other = user(&conn, "bob", old(), old());
        let id = post(&conn, uid, "old title", NOW);
        let m = member(&conn, uid);
        let tx = conn.transaction().expect("tx");
        let new = disc("new title", "https://example.com", "new").expect("d");
        let at = NOW + Duration::hours(2);
        edit_discussion(&tx, &m, id, &new, at).expect("edit at 2h");
        let (title, link, edited): (String, String, String) = tx
            .query_row(
                "SELECT title, link, edited_at FROM posts WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .expect("row");
        assert_eq!(
            (title.as_str(), link.as_str()),
            ("new title", "https://example.com/")
        );
        assert_eq!(edited, clock::storage(at));
        let late = NOW + Duration::hours(2) + Duration::seconds(1);
        let e = edit_discussion(&tx, &m, id, &new, late).expect_err("late");
        assert_eq!(e.status, StatusCode::FORBIDDEN);
        // Not the author, and not found.
        let stranger = member(&tx, other);
        assert!(edit_discussion(&tx, &stranger, id, &new, NOW).is_err());
        let e = edit_discussion(&tx, &m, 999, &new, NOW).expect_err("missing");
        assert_eq!(e.status, StatusCode::NOT_FOUND);
    }

    #[test]
    fn edit_of_a_hidden_removed_or_deleted_post_is_refused() {
        let (_d, mut conn) = fresh();
        let uid = user(&conn, "ada", old(), old());
        let m = member(&conn, uid);
        let new = disc("t", "", "b").expect("d");
        for state_name in ["hidden", "removed", "deleted"] {
            let id = post(&conn, uid, "x", NOW);
            state(&conn, "posts", id, state_name);
            let tx = conn.transaction().expect("tx");
            let e = edit_discussion(&tx, &m, id, &new, NOW).expect_err(state_name);
            assert_eq!(e.status, StatusCode::FORBIDDEN, "{state_name}");
        }
    }

    #[test]
    fn a_pack_post_cannot_be_edited_or_deleted_by_a_member() {
        let (_d, mut conn) = fresh();
        let uid = user(&conn, "ada", old(), old());
        let id = pack_post(&conn, "glossary", 5, "[]", NOW);
        let m = member(&conn, uid);
        let tx = conn.transaction().expect("tx");
        let new = disc("t", "", "b").expect("d");
        assert!(edit_discussion(&tx, &m, id, &new, NOW).is_err());
        assert!(delete_post(&tx, &m, id).is_err());
    }

    #[test]
    fn delete_clears_the_text_in_any_state_but_deleted() {
        let (_d, mut conn) = fresh();
        let uid = user(&conn, "ada", old(), old());
        let other = user(&conn, "bob", old(), old());
        let m = member(&conn, uid);
        for state_name in ["visible", "hidden", "removed"] {
            let id = post(&conn, uid, "secret", NOW);
            conn.execute(
                "UPDATE posts SET link = 'https://example.com/', removed_reason = 'spam',
                        state = ?1 WHERE id = ?2",
                params![state_name, id],
            )
            .expect("setup");
            let tx = conn.transaction().expect("tx");
            let stranger = member(&tx, other);
            assert!(delete_post(&tx, &stranger, id).is_err());
            delete_post(&tx, &m, id).expect(state_name);
            let (t, b, l, s, r): (String, String, Option<String>, String, Option<String>) = tx
                .query_row(
                    "SELECT title, body, link, state, removed_reason FROM posts WHERE id = ?1",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
                )
                .expect("row");
            assert_eq!(
                (t.as_str(), b.as_str(), l, s.as_str(), r),
                ("[deleted]", "[deleted]", None, "deleted", None)
            );
            let again = delete_post(&tx, &m, id).expect_err("twice");
            assert_eq!(again.status, StatusCode::CONFLICT);
            tx.commit().expect("commit");
        }
        // A restricted author may delete.
        let id = post(&conn, uid, "x", NOW);
        let mut restricted = m;
        restricted.restricted_until = Some(NOW + Duration::days(1));
        let tx = conn.transaction().expect("tx");
        delete_post(&tx, &restricted, id).expect("restricted delete");
    }

    fn state_of(tx: &Transaction, id: i64) -> (String, Option<String>) {
        tx.query_row(
            "SELECT state, removed_reason FROM posts WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("state")
    }

    #[test]
    fn the_state_transitions_follow_the_table() {
        let (_d, mut conn) = fresh();
        let uid = user(&conn, "ada", old(), old());
        let id = post(&conn, uid, "x", NOW);
        let tx = conn.transaction().expect("tx");
        // visible -> hidden by reports, once.
        assert_eq!(hide_by_reports(&tx, id), Ok(true));
        assert_eq!(hide_by_reports(&tx, id), Ok(false));
        assert_eq!(state_of(&tx, id), ("hidden".into(), None));
        // hidden -> removed with a reason; removed does not hide again.
        remove(&tx, id, " spam ").expect("remove");
        assert_eq!(state_of(&tx, id), ("removed".into(), Some("spam".into())));
        assert_eq!(hide_by_reports(&tx, id), Ok(false));
        assert_eq!(
            remove(&tx, id, "spam").expect_err("twice").status,
            StatusCode::CONFLICT
        );
        // removed -> visible clears the reason.
        restore(&tx, id).expect("restore");
        assert_eq!(state_of(&tx, id), ("visible".into(), None));
        assert_eq!(
            restore(&tx, id).expect_err("visible").status,
            StatusCode::CONFLICT
        );
        // visible -> removed needs a reason.
        assert!(remove(&tx, id, "  ").is_err());
        remove(&tx, id, "other").expect("remove visible");
        // deleted becomes nothing.
        state(&tx, "posts", id, "deleted");
        assert!(remove(&tx, id, "spam").is_err());
        assert!(restore(&tx, id).is_err());
        assert_eq!(hide_by_reports(&tx, id), Ok(false));
        // Missing posts are 404.
        assert_eq!(
            remove(&tx, 99, "spam").expect_err("m").status,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            restore(&tx, 99).expect_err("m").status,
            StatusCode::NOT_FOUND
        );
    }

    #[test]
    fn lock_and_unlock_are_idempotent() {
        let (_d, mut conn) = fresh();
        let uid = user(&conn, "ada", old(), old());
        let id = post(&conn, uid, "x", NOW);
        let tx = conn.transaction().expect("tx");
        let locked = |tx: &Transaction| -> bool {
            tx.query_row("SELECT locked FROM posts WHERE id = ?1", [id], |r| r.get(0))
                .expect("locked")
        };
        lock(&tx, id).expect("lock");
        lock(&tx, id).expect("lock again");
        assert!(locked(&tx));
        unlock(&tx, id).expect("unlock");
        unlock(&tx, id).expect("unlock again");
        assert!(!locked(&tx));
        assert_eq!(lock(&tx, 99).expect_err("m").status, StatusCode::NOT_FOUND);
    }

    #[test]
    fn hiding_follows_the_design() {
        let (_d, mut conn) = fresh();
        let uid = user(&conn, "ada", old(), old());
        let id = post(&conn, uid, "x", NOW);
        let hidden_at = |tx: &Transaction| -> Option<String> {
            tx.query_row(
                "SELECT at FROM hides WHERE user_id = ?1 AND post_id = ?2",
                [uid, id],
                |r| r.get(0),
            )
            .optional()
            .expect("hides")
        };
        let tx = conn.transaction().expect("tx");
        hide(&tx, UserId(uid), id, NOW).expect("hide");
        hide(&tx, UserId(uid), id, NOW + Duration::hours(1)).expect("again");
        assert_eq!(
            hidden_at(&tx),
            Some(clock::storage(NOW)),
            "twice changes nothing"
        );
        // Missing and non-visible posts are 404.
        assert_eq!(
            hide(&tx, UserId(uid), 99, NOW).expect_err("m").status,
            StatusCode::NOT_FOUND
        );
        let removed = post(&tx, uid, "r", NOW);
        state(&tx, "posts", removed, "removed");
        assert_eq!(
            hide(&tx, UserId(uid), removed, NOW).expect_err("r").status,
            StatusCode::NOT_FOUND
        );
        // Un-hide works once the post is no longer visible, and when not hidden.
        state(&tx, "posts", id, "removed");
        unhide(&tx, UserId(uid), id).expect("unhide");
        assert_eq!(hidden_at(&tx), None);
        unhide(&tx, UserId(uid), id).expect("unhide again");
        unhide(&tx, UserId(uid), 99).expect("unhide missing");
    }
}
