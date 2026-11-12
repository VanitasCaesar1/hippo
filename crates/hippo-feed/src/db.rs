//! The SQLite database: migrations, and read and write access.
//!
//! Work runs on a connection opened for the request, in `spawn_blocking`, at
//! most [`policy::DB_PERMITS`] at a time. A request that waits more than the
//! wait for its turn gets 503.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context};
use rusqlite::{Connection, Transaction, TransactionBehavior};
use time::OffsetDateTime;
use tokio::sync::Semaphore;

use crate::clock::{self, Clock, SystemClock};
use crate::error::AppError;
use crate::policy;

/// A member's internal id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UserId(pub i64);

/// A member's role. Admin is not a role in the database: it is a GitHub id in
/// `FEED_ADMINS`, checked on each request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    None,
    Moderator,
}

/// The signed-in member as a page needs them.
#[derive(Debug, Clone)]
pub struct Member {
    pub id: UserId,
    /// The GitHub numeric id; `FEED_ADMINS` is checked against it.
    pub github_id: Option<i64>,
    pub login: String,
    pub role: Role,
    pub created_at: OffsetDateTime,
    pub github_created_at: Option<OffsetDateTime>,
    pub restricted_until: Option<OffsetDateTime>,
    pub banned: bool,
    /// `auto`, `default`, `dark` or `pack:<name>`.
    pub theme: String,
    /// Whether the member has saved personal CSS; the CSS itself is not here.
    pub has_personal_css: bool,
    /// The session's CSRF token, for the forms a page draws.
    pub csrf: String,
    /// Computed when read (item 22 fills it); 0 until then.
    pub karma: i64,
}

/// Migration `n` is `MIGRATIONS[n - 1]`; a released migration is never edited.
pub const MIGRATIONS: &[(i64, &str)] = &[(1, include_str!("../migrations/0001_init.sql"))];

/// The newest schema version this binary knows.
pub fn latest_version() -> i64 {
    MIGRATIONS.last().map_or(0, |(n, _)| *n)
}

/// Opens a connection with the settings every connection uses.
pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_millis(5000))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    Ok(conn)
}

/// The database's schema version.
pub fn user_version(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("PRAGMA user_version", [], |row| row.get(0))
}

/// Applies the migrations above the database's version, each in one
/// transaction that also sets the version. Returns the versions before and
/// after. A database newer than this binary is refused.
pub fn migrate(path: &Path) -> anyhow::Result<(i64, i64)> {
    migrate_at(path, &SystemClock)
}

/// [`migrate`] with a given clock, which stamps `meta.created_at` on a new
/// database.
pub fn migrate_at(path: &Path, clock: &dyn Clock) -> anyhow::Result<(i64, i64)> {
    let mut conn =
        open(path).with_context(|| format!("cannot open the database {}", path.display()))?;
    let from = user_version(&conn)?;
    let newest = latest_version();
    if from > newest {
        bail!(
            "the database is at version {from}, newer than this binary's {newest}; \
             run a newer hippo-feed"
        );
    }
    for (number, sql) in MIGRATIONS.iter().filter(|(n, _)| *n > from) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)
            .with_context(|| format!("migration {number} failed"))?;
        tx.pragma_update(None, "user_version", number)?;
        if from == 0 && *number == 1 {
            tx.execute(
                "INSERT INTO meta (key, value) VALUES ('created_at', ?1)",
                [clock::storage(clock.now())],
            )?;
        }
        tx.commit()?;
    }
    Ok((from, user_version(&conn)?))
}

/// Requires the database to be at this binary's schema version, as `serve`
/// and `/healthz` do.
pub fn check_version(conn: &Connection) -> anyhow::Result<()> {
    let have = user_version(conn)?;
    let want = latest_version();
    if have < want {
        bail!("the database is at version {have}, older than this binary's {want}; run `hippo-feed migrate`");
    }
    if have > want {
        bail!("the database is at version {have}, newer than this binary's {want}; run a newer hippo-feed");
    }
    Ok(())
}

/// Access to the database for request handlers.
#[derive(Clone)]
pub struct Db {
    path: Arc<PathBuf>,
    permits: Arc<Semaphore>,
    wait: Duration,
}

impl Db {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self::with_wait(path, policy::DB_WAIT)
    }

    /// A handle with a different wait for a permit; tests shorten it.
    pub fn with_wait(path: impl Into<PathBuf>, wait: Duration) -> Self {
        Self {
            path: Arc::new(path.into()),
            permits: Arc::new(Semaphore::new(policy::DB_PERMITS)),
            wait,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The permits, so a test can hold them all and see requests give up.
    pub fn permits(&self) -> Arc<Semaphore> {
        self.permits.clone()
    }

    /// Runs `f` on a new connection. The closure must not write.
    pub async fn read<T, F>(&self, f: F) -> Result<T, AppError>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> Result<T, AppError> + Send + 'static,
    {
        self.run(move |conn| f(conn)).await
    }

    /// Runs `f` inside a transaction opened with `BEGIN IMMEDIATE`, and
    /// commits it if `f` succeeds.
    pub async fn write<T, F>(&self, f: F) -> Result<T, AppError>
    where
        T: Send + 'static,
        F: FnOnce(&Transaction) -> Result<T, AppError> + Send + 'static,
    {
        self.run(move |conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let value = f(&tx)?;
            tx.commit()?;
            Ok(value)
        })
        .await
    }

    async fn run<T, F>(&self, f: F) -> Result<T, AppError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, AppError> + Send + 'static,
    {
        let permit =
            match tokio::time::timeout(self.wait, self.permits.clone().acquire_owned()).await {
                Ok(Ok(permit)) => permit,
                Ok(Err(_)) | Err(_) => return Err(AppError::busy()),
            };
        let path = self.path.clone();
        let joined = tokio::task::spawn_blocking(move || {
            // The permit is held until the work ends.
            let _permit = permit;
            let mut conn = open(&path)?;
            f(&mut conn)
        })
        .await;
        match joined {
            Ok(result) => result,
            Err(e) => Err(AppError::internal(format!("database task failed: {e}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn temp_db() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("feed.sqlite");
        (dir, path)
    }

    #[test]
    fn an_empty_database_migrates_to_the_newest_version() {
        let (_dir, path) = temp_db();
        let clock = clock::FixedClock::new(datetime!(2026-11-08 10:00:00 UTC));
        let (from, to) = migrate_at(&path, &clock).expect("migrate");
        assert_eq!((from, to), (0, latest_version()));
        let conn = open(&path).expect("open");
        check_version(&conn).expect("current");
        let created: String = conn
            .query_row("SELECT value FROM meta WHERE key = 'created_at'", [], |r| {
                r.get(0)
            })
            .expect("created_at");
        assert_eq!(created, "2026-11-08T10:00:00.000000Z");
        let mode: String = conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .expect("mode");
        assert_eq!(mode, "wal");
    }

    #[test]
    fn a_second_run_changes_nothing() {
        let (_dir, path) = temp_db();
        migrate(&path).expect("first");
        let conn = open(&path).expect("open");
        let created: String = conn
            .query_row("SELECT value FROM meta WHERE key = 'created_at'", [], |r| {
                r.get(0)
            })
            .expect("created_at");
        let (from, to) = migrate(&path).expect("second");
        assert_eq!(from, to);
        let again: String = conn
            .query_row("SELECT value FROM meta WHERE key = 'created_at'", [], |r| {
                r.get(0)
            })
            .expect("created_at");
        assert_eq!(created, again);
    }

    #[test]
    fn migrate_refuses_a_newer_database_and_check_version_an_older_one() {
        let (_dir, path) = temp_db();
        // A database that was never migrated is older than the binary.
        let conn = open(&path).expect("open");
        let e = check_version(&conn).expect_err("older").to_string();
        assert!(e.contains("older") && e.contains("migrate"), "{e}");
        migrate(&path).expect("migrate");
        conn.pragma_update(None, "user_version", latest_version() + 1)
            .expect("bump");
        let e = migrate(&path).expect_err("newer").to_string();
        assert!(e.contains("newer"), "{e}");
        let e = check_version(&conn).expect_err("newer").to_string();
        assert!(e.contains("newer"), "{e}");
    }

    #[test]
    fn the_audit_log_is_append_only() {
        let (_dir, path) = temp_db();
        migrate(&path).expect("migrate");
        let conn = open(&path).expect("open");
        conn.execute(
            "INSERT INTO audit_log (at, action, public) VALUES ('2026-11-08T10:00:00.000000Z', 'x', 1)",
            [],
        )
        .expect("insert");
        let e = conn
            .execute("UPDATE audit_log SET action = 'y'", [])
            .expect_err("update");
        assert!(e.to_string().contains("audit_log is append-only"), "{e}");
        let e = conn
            .execute("DELETE FROM audit_log", [])
            .expect_err("delete");
        assert!(e.to_string().contains("audit_log is append-only"), "{e}");
    }

    #[test]
    fn schema_checks_hold() {
        let (_dir, path) = temp_db();
        migrate(&path).expect("migrate");
        let conn = open(&path).expect("open");
        // A pack post needs a pack; a discussion must not have one.
        let e = conn.execute(
            "INSERT INTO posts (kind, title, created_at) VALUES ('pack', 't', 'now')",
            [],
        );
        assert!(e.is_err());
        conn.execute(
            "INSERT INTO posts (kind, title, created_at) VALUES ('discussion', 't', 'now')",
            [],
        )
        .expect("discussion");
        // Logins are unique without regard to case.
        conn.execute(
            "INSERT INTO users (login, created_at) VALUES ('Ada', 'now')",
            [],
        )
        .expect("ada");
        assert!(conn
            .execute(
                "INSERT INTO users (login, created_at) VALUES ('ada', 'now')",
                []
            )
            .is_err());
    }

    #[tokio::test]
    async fn write_commits_and_failed_writes_roll_back() {
        let (_dir, path) = temp_db();
        migrate(&path).expect("migrate");
        let db = Db::new(&path);
        db.write(|tx| {
            tx.execute("INSERT INTO meta (key, value) VALUES ('k', 'v')", [])?;
            Ok(())
        })
        .await
        .expect("write");
        let failed: Result<(), AppError> = db
            .write(|tx| {
                tx.execute("INSERT INTO meta (key, value) VALUES ('k2', 'v')", [])?;
                Err(AppError::bad_request("no"))
            })
            .await;
        assert!(failed.is_err());
        let keys: Vec<String> = db
            .read(|conn| {
                let mut stmt =
                    conn.prepare("SELECT key FROM meta WHERE key LIKE 'k%' ORDER BY key")?;
                let rows = stmt.query_map([], |r| r.get(0))?;
                Ok(rows.collect::<Result<Vec<String>, _>>()?)
            })
            .await
            .expect("read");
        assert_eq!(keys, vec!["k".to_string()]);
    }

    #[tokio::test]
    async fn a_request_that_waits_too_long_gets_503() {
        let (_dir, path) = temp_db();
        migrate(&path).expect("migrate");
        let db = Db::with_wait(&path, Duration::from_millis(50));
        let held = db
            .permits()
            .acquire_many_owned(policy::DB_PERMITS as u32)
            .await
            .expect("hold");
        let e = db.read(|_| Ok(())).await.expect_err("busy");
        assert_eq!(e.status, axum::http::StatusCode::SERVICE_UNAVAILABLE);
        drop(held);
        db.read(|_| Ok(())).await.expect("free again");
    }
}
