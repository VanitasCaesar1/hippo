//! The Dreaming panel: runs and their changes, and one change's tab.
//!
//! hippocamp records no run id. A run is a report and the proposals it names;
//! proposals no report names belong to runs a later run on the same UTC day
//! replaced, and are grouped by the day in their id.

use std::collections::HashSet;
use std::io;
use std::path::PathBuf;

use hippocamp_core::config::Workspace;
use hippocamp_core::dream::report::{self, ReportRun};
use hippocamp_core::dream::{revert_state_in, Change, History, RevertState};
use hippocamp_core::error::{Error, Result};
use hippocamp_core::index::{Index, SessionTurn};
use hippocamp_core::records::{Listed, Verdict};

use crate::digest::{self, Digest};
use time::{Date, OffsetDateTime};
use ulid::Ulid;

/// The reports, with the changes each names, and the changes none names.
#[derive(Debug, Clone, PartialEq)]
pub struct Dreaming {
    /// One per report, newest first.
    pub runs: Vec<Run>,
    /// Changes no readable report names, by the UTC day of their id, newest
    /// day first, by id within a day.
    pub earlier: Vec<(Date, Vec<Change>)>,
    /// Receipts and proposal files that do not load, from the history.
    pub unreadable: Vec<(PathBuf, String)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    pub report: Listed<ReportRun>,
    /// What the report says the run did; `None` when it cannot be read.
    pub digest: Option<Digest>,
    /// When the report was written, at the end of the run.
    pub written: Option<std::time::SystemTime>,
    /// The changes the report's Proposals list names, in its order.
    pub changes: Vec<Change>,
    /// Ids it names that have no proposal file and no receipt, as a dry
    /// run's report does.
    pub missing: Vec<Ulid>,
}

/// One change's tab.
#[derive(Debug, Clone, PartialEq)]
pub struct ChangeView {
    pub change: Change,
    /// Each evidence turn in the proposal's order; `Err((session, ordinal))`
    /// for a turn the index does not hold, or for every turn when there is
    /// no index.
    pub evidence: Vec<std::result::Result<SessionTurn, (String, u32)>>,
    /// What may be done with the change's applied confirm receipt; `None`
    /// when it has none.
    pub revert: Option<RevertState>,
}

/// Reads the reports and groups `history`'s changes by them.
pub fn load(ws: &Workspace, history: &History) -> Result<Dreaming> {
    let mut named = HashSet::new();
    let mut runs = Vec::new();
    for report in report::list(ws)? {
        let mut changes = Vec::new();
        let mut missing = Vec::new();
        if let Ok(run) = &report.record {
            for id in &run.proposals {
                named.insert(*id);
                match history.changes.iter().find(|c| c.proposal_id == *id) {
                    Some(change) => changes.push(change.clone()),
                    None => missing.push(*id),
                }
            }
        }
        let digest = std::fs::read_to_string(&report.path)
            .ok()
            .map(|text| digest::parse(&text));
        let written = std::fs::metadata(&report.path)
            .and_then(|m| m.modified())
            .ok();
        runs.push(Run {
            report,
            digest,
            written,
            changes,
            missing,
        });
    }

    let mut earlier: Vec<(Date, Vec<Change>)> = Vec::new();
    // `history.changes` is in id order, which is time order.
    for change in history
        .changes
        .iter()
        .filter(|c| !named.contains(&c.proposal_id))
    {
        let day = day_of(change.proposal_id);
        match earlier.iter_mut().find(|(d, _)| *d == day) {
            Some((_, changes)) => changes.push(change.clone()),
            None => earlier.push((day, vec![change.clone()])),
        }
    }
    earlier.sort_by_key(|(day, _)| std::cmp::Reverse(*day));

    Ok(Dreaming {
        runs,
        earlier,
        unreadable: history.unreadable.clone(),
    })
}

/// The UTC day in a proposal id's timestamp.
fn day_of(id: Ulid) -> Date {
    let ms = i128::from(id.timestamp_ms());
    OffsetDateTime::from_unix_timestamp_nanos(ms * 1_000_000)
        .map_or(Date::MIN, OffsetDateTime::date)
}

/// The change `proposal` with its evidence turns and revert state. Fails
/// when the history holds no such change, or when `revert_state_in` fails.
pub fn view(
    ws: &Workspace,
    history: &History,
    index: Option<&Index>,
    proposal: Ulid,
) -> Result<ChangeView> {
    let change = history
        .changes
        .iter()
        .find(|c| c.proposal_id == proposal)
        .ok_or_else(|| {
            Error::Io(io::Error::new(
                io::ErrorKind::NotFound,
                format!("no change {proposal}"),
            ))
        })?
        .clone();

    let pairs: Vec<(String, u32)> = change
        .proposal
        .record
        .as_ref()
        .map(|p| {
            p.evidence
                .iter()
                .map(|e| (e.session.clone(), e.ordinal))
                .collect()
        })
        .unwrap_or_default();
    let found = match index {
        Some(index) if !pairs.is_empty() => index.turns(&pairs)?,
        _ => Vec::new(),
    };
    let evidence = pairs
        .into_iter()
        .map(|(session, ordinal)| {
            found
                .iter()
                .find(|t| t.session == session && t.ordinal == ordinal)
                .cloned()
                .ok_or((session, ordinal))
        })
        .collect();

    let applied = change
        .receipts
        .iter()
        .find(|r| r.verdict == Verdict::Confirm && r.applied.is_some() && r.snapshot.is_some());
    let revert = match applied {
        Some(receipt) => Some(revert_state_in(ws, history, receipt.id)?),
        None => None,
    };
    Ok(ChangeView {
        change,
        evidence,
        revert,
    })
}
