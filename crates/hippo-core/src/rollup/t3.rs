//! A T3 Code delegated task's own session. The parent's
//! `delegate_task` result names the child thread (`childThreadId`); T3
//! Code's state names the session that thread ran as
//! (`nativeThreadRef.nativeId`). Read-only, ids only: no message, title or
//! setting of T3 Code's is read.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

/// T3 Code's state database: `HIPPO_T3_STATE` when set (the demo home and
/// checks), else `~/.t3/userdata/statev2.sqlite`.
pub fn state_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("HIPPO_T3_STATE") {
        return Some(PathBuf::from(p));
    }
    Some(crate::config::user_home()?.join(".t3/userdata/statev2.sqlite"))
}

/// For each of `threads`, the native session ids T3 Code ran it as, newest
/// first. A missing or unreadable database, or another schema, gives none.
/// Opened `mode=ro` (never `immutable`: T3 Code keeps a write-ahead log
/// while it runs).
pub fn natives(db: &Path, threads: &[String]) -> HashMap<String, Vec<String>> {
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    if threads.is_empty() || !db.is_file() {
        return out;
    }
    let uri = format!("file:{}?mode=ro", db.to_string_lossy());
    let Ok(conn) = Connection::open_with_flags(
        uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) else {
        return out;
    };
    if conn
        .busy_timeout(std::time::Duration::from_millis(500))
        .is_err()
    {
        return out;
    }
    let Ok(mut stmt) = conn.prepare(
        "SELECT json_extract(payload_json, '$.nativeThreadRef.nativeId') \
         FROM orchestration_v2_projection_provider_threads \
         WHERE thread_id = ?1 ORDER BY updated_at DESC",
    ) else {
        return out;
    };
    for t in threads {
        let Ok(rows) = stmt.query_map([t], |r| r.get::<_, Option<String>>(0)) else {
            continue;
        };
        let ids: Vec<String> = rows.flatten().flatten().filter(|n| valid(n)).collect();
        if !ids.is_empty() {
            out.insert(t.clone(), ids);
        }
    }
    out
}

/// A session id as T3 Code and hippo write them: letters, digits, `-`, `_`.
fn valid(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 80
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_thread_gives_its_native_sessions_newest_first_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("statev2.sqlite");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE orchestration_v2_projection_provider_threads(provider_thread_id TEXT PRIMARY KEY, thread_id TEXT, updated_at TEXT NOT NULL, payload_json TEXT NOT NULL);
             INSERT INTO orchestration_v2_projection_provider_threads VALUES
              ('p1', 'thread:child-1', '2026-11-08T08:00:00Z', '{\"nativeThreadRef\":{\"nativeId\":\"s-old\"}}'),
              ('p2', 'thread:child-1', '2026-11-08T09:00:00Z', '{\"nativeThreadRef\":{\"nativeId\":\"s-new\"}}'),
              ('p3', 'thread:child-2', '2026-11-08T09:00:00Z', '{\"nativeThreadRef\":null}'),
              ('p4', 'thread:child-3', '2026-11-08T09:00:00Z', '{\"nativeThreadRef\":{\"nativeId\":\"../../etc\"}}');",
        )
        .unwrap();
        drop(conn);
        let got = natives(
            &db,
            &[
                "thread:child-1".to_owned(),
                "thread:child-2".to_owned(),
                "thread:child-3".to_owned(),
                "thread:none".to_owned(),
            ],
        );
        assert_eq!(got.len(), 1);
        assert_eq!(got["thread:child-1"], ["s-new", "s-old"]);
        // No database, or another schema: nothing.
        assert!(natives(
            &dir.path().join("missing.sqlite"),
            &["thread:child-1".to_owned()]
        )
        .is_empty());
        let other = dir.path().join("other.sqlite");
        Connection::open(&other)
            .unwrap()
            .execute_batch("CREATE TABLE t(x)")
            .unwrap();
        assert!(natives(&other, &["thread:child-1".to_owned()]).is_empty());
    }
}
