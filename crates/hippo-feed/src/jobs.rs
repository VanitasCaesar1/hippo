//! The background task: the registry sync at start and on a timer, and the
//! hourly clean-up.
//!
//! One Tokio task, started by `serve` through [`spawn`]. Tests do not start
//! it; they call [`hourly`] and [`registry_sync`] themselves with a settable
//! clock. A failed run is logged and tried again at the next tick.

use std::time::Duration;

use time::OffsetDateTime;
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

use crate::clock;
use crate::db::UserId;
use crate::error::AppError;
use crate::policy;
use crate::registry::{self, SyncContext, SyncReport};
use crate::rings;
use crate::web::AppState;

/// What one hourly run removed or blanked.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HourlyReport {
    pub sessions: usize,
    pub oauth_states: usize,
    pub github_ids: usize,
    pub report_texts: usize,
}

/// The sync's context from the configuration, or `None` when no registry
/// index is configured.
pub fn sync_context(state: &AppState) -> Option<SyncContext> {
    let source = state.config.registry_index.clone()?;
    Some(SyncContext {
        source,
        raw_url: state.config.raw_url.clone(),
        agent: state.http.clone(),
    })
}

/// Runs one registry sync now, as the timer (`actor` `None`) or for an admin.
/// `Ok(None)` when no index is configured.
pub async fn registry_sync(
    state: &AppState,
    actor: Option<UserId>,
) -> Result<Option<SyncReport>, AppError> {
    let Some(ctx) = sync_context(state) else {
        return Ok(None);
    };
    registry::sync(&state.db, &ctx, state.clock.now(), actor)
        .await
        .map(Some)
}

/// The hourly clean-up at `now`:
///
/// - prunes the per-address limiters;
/// - deletes sessions past `expires_at` and OAuth states older than 10
///   minutes;
/// - sets to null the GitHub id of deleted accounts that are neither banned
///   nor restricted;
/// - blanks the note and snapshot of reports resolved more than 90 days ago.
pub async fn hourly(state: &AppState, now: OffsetDateTime) -> Result<HourlyReport, AppError> {
    state.limiters.prune();
    let at = clock::storage(now);
    let states_before = clock::storage(now - time::Duration::minutes(policy::OAUTH_STATE_MINUTES));
    let reports_before = clock::storage(now - time::Duration::days(policy::REPORT_TEXT_DAYS));
    state
        .db
        .write(move |tx| {
            let sessions = tx.execute("DELETE FROM sessions WHERE expires_at <= ?1", [&at])?;
            let oauth_states = tx.execute(
                "DELETE FROM oauth_states WHERE created_at < ?1",
                [&states_before],
            )?;
            let github_ids = tx.execute(
                "UPDATE users SET github_id = NULL
                 WHERE deleted_at IS NOT NULL AND github_id IS NOT NULL AND banned = 0
                       AND (restricted_until IS NULL OR restricted_until <= ?1)",
                [&at],
            )?;
            let report_texts = tx.execute(
                "UPDATE reports SET note = '', snapshot = ''
                 WHERE status <> 'open' AND resolved_at IS NOT NULL AND resolved_at < ?1
                       AND (note <> '' OR snapshot <> '')",
                [&reports_before],
            )?;
            Ok(HourlyReport {
                sessions,
                oauth_states,
                github_ids,
                report_texts,
            })
        })
        .await
}

/// The ring check at `now`, in one write transaction: system reports for the
/// groups of accounts that voted or measured together
/// ([`rings::check`]). Returns the new reports' ids.
pub async fn ring_check(state: &AppState, now: OffsetDateTime) -> Result<Vec<i64>, AppError> {
    state
        .db
        .write(move |tx| rings::check(tx, now).map_err(AppError::from))
        .await
}

async fn run_sync(state: &AppState) {
    match registry_sync(state, None).await {
        Ok(Some(report)) if report.ok() => tracing::info!("registry sync: {}", report.line()),
        Ok(Some(report)) => tracing::warn!(
            "registry sync refused the index: {}",
            report.error.as_deref().unwrap_or("")
        ),
        Ok(None) => {}
        Err(e) => tracing::warn!("registry sync failed: {}", e.message),
    }
}

async fn run_hourly(state: &AppState) {
    let now = state.clock.now();
    match hourly(state, now).await {
        Ok(r) => tracing::info!(
            "hourly: {} sessions, {} sign-in states, {} github ids, {} report texts cleared",
            r.sessions,
            r.oauth_states,
            r.github_ids,
            r.report_texts
        ),
        Err(e) => tracing::warn!("hourly clean-up failed: {}", e.message),
    }
    // The ring check follows the clean-up. Its failure is logged the same
    // way and tried again at the next tick.
    match ring_check(state, now).await {
        Ok(reports) => tracing::info!("ring check: {} new reports", reports.len()),
        Err(e) => tracing::warn!("ring check failed: {}", e.message),
    }
}

/// The task's loop: a sync at start and every `FEED_REGISTRY_SYNC_MINUTES`
/// (never when that is 0 or no index is configured), and the clean-up every
/// hour.
pub async fn run(state: AppState) {
    let minutes = u64::from(state.config.registry_sync_minutes);
    let sync_on = minutes > 0 && state.config.registry_index.is_some();
    let mut sync = tokio::time::interval(Duration::from_secs(minutes.max(1) * 60));
    sync.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut hour = tokio::time::interval(Duration::from_secs(3_600));
    hour.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = sync.tick(), if sync_on => run_sync(&state).await,
            _ = hour.tick() => run_hourly(&state).await,
        }
    }
}

/// Starts the task. `serve` calls it once.
pub fn spawn(state: AppState) -> JoinHandle<()> {
    tokio::spawn(run(state))
}
