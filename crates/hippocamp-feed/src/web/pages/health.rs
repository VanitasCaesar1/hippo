//! `/healthz`: what an external monitor reads.

use axum::extract::State;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use time::{Duration, OffsetDateTime};

use crate::clock;
use crate::db;
use crate::policy;
use crate::web::AppState;

/// The JSON body of `/healthz`.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Health {
    pub ok: bool,
    pub db_version: Option<i64>,
    pub last_sync: Option<String>,
    pub last_sync_ok: Option<bool>,
    pub last_successful_sync: Option<String>,
    pub last_backup: Option<String>,
    pub oldest_open_report: Option<String>,
    pub stale: Vec<&'static str>,
}

impl Health {
    fn down(db_version: Option<i64>) -> Self {
        Self {
            ok: false,
            db_version,
            last_sync: None,
            last_sync_ok: None,
            last_successful_sync: None,
            last_backup: None,
            oldest_open_report: None,
            stale: Vec::new(),
        }
    }
}

/// The times `stale` is worked out from.
pub struct StaleInputs {
    pub now: OffsetDateTime,
    pub started_at: OffsetDateTime,
    pub database_created_at: Option<OffsetDateTime>,
    pub last_backup: Option<OffsetDateTime>,
    pub index_configured: bool,
    pub last_successful_sync: Option<OffsetDateTime>,
    pub oldest_open_report: Option<OffsetDateTime>,
}

/// What needs attention: `backup` when the newest backup is over 36 hours old
/// (or none exists and the database is over 36 hours old); `sync` when an
/// index is configured and no sync has succeeded in 6 hours, once the server
/// has run for an hour; `reports` when a report has been open over 48 hours.
pub fn stale(i: &StaleInputs) -> Vec<&'static str> {
    let mut out = Vec::new();
    let older = |t: OffsetDateTime, hours: i64| i.now - t > Duration::hours(hours);
    let backup_old = match (i.last_backup, i.database_created_at) {
        (Some(last), _) => older(last, policy::STALE_BACKUP_HOURS),
        (None, Some(created)) => older(created, policy::STALE_BACKUP_HOURS),
        (None, None) => false,
    };
    if backup_old {
        out.push("backup");
    }
    let running_long_enough = i.now - i.started_at >= Duration::hours(policy::SYNC_GRACE_HOURS);
    if i.index_configured && running_long_enough {
        let sync_old = i
            .last_successful_sync
            .is_none_or(|t| older(t, policy::STALE_SYNC_HOURS));
        if sync_old {
            out.push("sync");
        }
    }
    if i.oldest_open_report
        .is_some_and(|t| older(t, policy::STALE_REPORT_HOURS))
    {
        out.push("reports");
    }
    out
}

fn meta(conn: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
        .optional()
}

fn gather(
    conn: &Connection,
    now: OffsetDateTime,
    started_at: OffsetDateTime,
    index_configured: bool,
) -> rusqlite::Result<Health> {
    let version = db::user_version(conn)?;
    if db::check_version(conn).is_err() {
        return Ok(Health::down(Some(version)));
    }
    let stored = |key: &str| -> rusqlite::Result<Option<String>> { meta(conn, key) };
    let last_sync = stored("last_sync")?;
    let last_successful = stored("last_successful_sync")?;
    let last_backup = stored("last_backup")?;
    let created = stored("created_at")?;
    let last_sync_ok = stored("last_sync_ok")?.map(|v| v == "1" || v == "true");
    let oldest: Option<String> = conn.query_row(
        "SELECT MIN(created_at) FROM reports WHERE status = 'open'",
        [],
        |r| r.get(0),
    )?;
    let parse = |v: &Option<String>| v.as_deref().and_then(clock::parse_storage);
    let rfc = |v: &Option<String>| v.as_deref().and_then(clock::storage_to_rfc3339);
    Ok(Health {
        ok: true,
        db_version: Some(version),
        last_sync: rfc(&last_sync),
        last_sync_ok,
        last_successful_sync: rfc(&last_successful),
        last_backup: rfc(&last_backup),
        oldest_open_report: rfc(&oldest),
        stale: stale(&StaleInputs {
            now,
            started_at,
            database_created_at: parse(&created),
            last_backup: parse(&last_backup),
            index_configured,
            last_successful_sync: parse(&last_successful),
            oldest_open_report: parse(&oldest),
        }),
    })
}

pub async fn healthz(State(state): State<AppState>) -> Response {
    let now = state.clock.now();
    let started_at = state.started_at;
    let index_configured = state.config.registry_index.is_some();
    let result = state
        .db
        .read(move |conn| Ok(gather(conn, now, started_at, index_configured)?))
        .await;
    // Any failure to read the database at the binary's version is "not ok".
    let health = result.unwrap_or_else(|_| Health::down(None));
    let status = if health.ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    let body = serde_json::to_string(&health).unwrap_or_else(|_| "{\"ok\":false}".to_string());
    let mut response = (status, body).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn inputs() -> StaleInputs {
        StaleInputs {
            now: datetime!(2026-11-10 12:00:00 UTC),
            started_at: datetime!(2026-11-10 00:00:00 UTC),
            database_created_at: Some(datetime!(2026-11-10 00:00:00 UTC)),
            last_backup: Some(datetime!(2026-11-10 02:30:00 UTC)),
            index_configured: true,
            last_successful_sync: Some(datetime!(2026-11-10 11:45:00 UTC)),
            oldest_open_report: None,
        }
    }

    #[test]
    fn nothing_is_stale_when_all_is_recent() {
        assert!(stale(&inputs()).is_empty());
    }

    #[test]
    fn backup_is_stale_after_36_hours() {
        let mut i = inputs();
        i.last_backup = Some(i.now - Duration::hours(36));
        assert!(stale(&i).is_empty(), "exactly 36 hours is not over");
        i.last_backup = Some(i.now - Duration::hours(37));
        assert_eq!(stale(&i), vec!["backup"]);
        // No backup: judged by the database's age.
        i.last_backup = None;
        i.database_created_at = Some(i.now - Duration::hours(10));
        assert!(stale(&i).is_empty());
        i.database_created_at = Some(i.now - Duration::hours(40));
        assert_eq!(stale(&i), vec!["backup"]);
    }

    #[test]
    fn sync_is_stale_after_6_hours_once_the_server_has_run_an_hour() {
        let mut i = inputs();
        i.last_successful_sync = Some(i.now - Duration::hours(7));
        assert_eq!(stale(&i), vec!["sync"]);
        i.last_successful_sync = None;
        assert_eq!(stale(&i), vec!["sync"]);
        // Just started: not yet.
        i.started_at = i.now - Duration::minutes(30);
        assert!(stale(&i).is_empty());
        // No index configured: never.
        i.started_at = i.now - Duration::hours(10);
        i.index_configured = false;
        assert!(stale(&i).is_empty());
    }

    #[test]
    fn reports_are_stale_after_48_hours() {
        let mut i = inputs();
        i.oldest_open_report = Some(i.now - Duration::hours(47));
        assert!(stale(&i).is_empty());
        i.oldest_open_report = Some(i.now - Duration::hours(49));
        assert_eq!(stale(&i), vec!["reports"]);
    }
}
