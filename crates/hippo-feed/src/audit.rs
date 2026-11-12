//! The audit log writer.
//!
//! Every module records its actions through [`write`], inside the transaction
//! of the change it records; by convention nothing else inserts into
//! `audit_log`. Rows hold ids, counts, times, roles and rule names, never free
//! text.

use rusqlite::{params, Transaction};
use serde_json::Value;
use time::OffsetDateTime;

use crate::clock;
use crate::db::UserId;

pub const POST_REMOVE: &str = "post.remove";
pub const POST_RESTORE: &str = "post.restore";
pub const POST_LOCK: &str = "post.lock";
pub const POST_UNLOCK: &str = "post.unlock";
pub const COMMENT_REMOVE: &str = "comment.remove";
pub const COMMENT_RESTORE: &str = "comment.restore";
pub const MEASUREMENT_INVALIDATE: &str = "measurement.invalidate";
pub const MEASUREMENT_RESTORE: &str = "measurement.restore";
pub const USER_RESTRICT: &str = "user.restrict";
pub const USER_UNRESTRICT: &str = "user.unrestrict";
pub const USER_BAN: &str = "user.ban";
pub const USER_UNBAN: &str = "user.unban";
pub const USER_SPAM_BAN: &str = "user.spam_ban";
pub const USER_ROLE: &str = "user.role";
pub const USER_DELETE: &str = "user.delete";
pub const REPORT_DISMISS: &str = "report.dismiss";
pub const AUTO_HIDE: &str = "auto.hide";
pub const RING_FLAG: &str = "ring.flag";
pub const EVIDENCE_FLAG: &str = "evidence.flag";
pub const REGISTRY_LIST: &str = "registry.list";
pub const REGISTRY_DELIST: &str = "registry.delist";
pub const REGISTRY_SKIP: &str = "registry.skip";
pub const REGISTRY_MAINTAINERS: &str = "registry.maintainers";
pub const REGISTRY_SYNC: &str = "registry.sync";

/// Appends one row at `at`. `actor` is `None` for the system. `target` is the
/// target's type and id. The caller says whether the row is `public`, because
/// some actions are public or not depending on who takes them.
///
/// The row records a time, and every
/// decision that depends on the time reads the one `Clock`, so the caller
/// passes it.
#[allow(clippy::too_many_arguments)]
pub fn write(
    tx: &Transaction,
    at: OffsetDateTime,
    actor: Option<UserId>,
    action: &str,
    target: Option<(&str, String)>,
    reason: &str,
    public: bool,
    details: Value,
) -> rusqlite::Result<()> {
    let (target_type, target_id) = match target {
        Some((kind, id)) => (Some(kind.to_string()), Some(id)),
        None => (None, None),
    };
    tx.execute(
        "INSERT INTO audit_log (at, actor_id, action, target_type, target_id, reason, public, details)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            clock::storage(at),
            actor.map(|u| u.0),
            action,
            target_type,
            target_id,
            reason,
            public,
            details.to_string(),
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use serde_json::json;
    use time::macros::datetime;

    #[test]
    fn a_row_is_written_and_read_back() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("feed.sqlite");
        db::migrate(&path).expect("migrate");
        let mut conn = db::open(&path).expect("open");
        conn.execute(
            "INSERT INTO users (login, created_at) VALUES ('mod', 'now')",
            [],
        )
        .expect("user");
        let tx = conn.transaction().expect("tx");
        write(
            &tx,
            datetime!(2026-11-08 10:00:00 UTC),
            Some(UserId(1)),
            POST_REMOVE,
            Some(("post", "12".to_string())),
            "spam",
            true,
            json!({"reports": 3}),
        )
        .expect("write");
        write(
            &tx,
            datetime!(2026-11-08 10:01:00 UTC),
            None,
            REGISTRY_LIST,
            Some(("pack", "glossary-terms".to_string())),
            "",
            true,
            json!({}),
        )
        .expect("write");
        tx.commit().expect("commit");

        let row = conn
            .query_row(
                "SELECT at, actor_id, action, target_type, target_id, reason, public, details
                 FROM audit_log WHERE id = 1",
                [],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<i64>>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, Option<String>>(4)?,
                        r.get::<_, String>(5)?,
                        r.get::<_, bool>(6)?,
                        r.get::<_, String>(7)?,
                    ))
                },
            )
            .expect("row");
        assert_eq!(row.0, "2026-11-08T10:00:00.000000Z");
        assert_eq!(row.1, Some(1));
        assert_eq!(row.2, "post.remove");
        assert_eq!(row.3.as_deref(), Some("post"));
        assert_eq!(row.4.as_deref(), Some("12"));
        assert_eq!(row.5, "spam");
        assert!(row.6);
        assert_eq!(row.7, r#"{"reports":3}"#);

        let system: Option<i64> = conn
            .query_row("SELECT actor_id FROM audit_log WHERE id = 2", [], |r| {
                r.get(0)
            })
            .expect("system row");
        assert_eq!(system, None);
    }
}
