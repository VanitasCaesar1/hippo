//! Agents' *Recent* column: the last finished sessions, each with one line
//! on how it ended.

use serde::Serialize;
use time::{OffsetDateTime, UtcOffset};

use super::{agent, day_hm, Agent};
use crate::fleet::{Fleet, Row, State};

/// How many finished sessions *Recent* lists.
pub const RECENT: usize = 5;

/// One finished session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Recent {
    pub id: String,
    pub title: String,
    pub agent: Agent,
    pub profile: Option<String>,
    pub project: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    /// `08:02`, `Thu 17:20`.
    pub when: String,
    /// `Merged #212. Its claim "all tests pass" was checked.`
    pub line: String,
    /// `ok`, `unverified`, `bad` or empty: the mark beside it.
    pub mark: &'static str,
}

/// When a session ended, if it did.
fn ended(r: &Row) -> Option<OffsetDateTime> {
    match &r.state {
        State::Finished { at } | State::Interrupted { at, .. } | State::Stopped { at, .. } => {
            Some(*at)
        }
        State::Quiet { since } => *since,
        State::Live | State::Waiting { .. } => None,
    }
}

fn pull_number(url: &str) -> String {
    url.trim_end_matches('/')
        .rsplit('/')
        .next()
        .map(|n| format!("#{n}"))
        .unwrap_or_default()
}

/// The line and the mark for a finished row.
pub fn line(r: &Row, offset: UtcOffset) -> (String, &'static str) {
    let merged = r
        .pulls
        .iter()
        .find(|p| p.1 == "merged")
        .map(|p| pull_number(&p.0));
    let opened = r
        .pulls
        .iter()
        .find(|p| p.1 == "opened")
        .map(|p| pull_number(&p.0));
    let claims = &r.claims;
    if let Some((claim, _)) = &claims.first_unverified {
        let head = match &merged {
            Some(n) => format!("Merged {n}. "),
            None => "Done. ".to_owned(),
        };
        return (
            format!(
                "{head}\"{}\" has no run behind it.",
                crate::edition::sentences(claim, 60)
            ),
            "unverified",
        );
    }
    match &r.state {
        State::Stopped { reset, .. } => (
            match reset {
                Some(r) => format!("Stopped at the usage limit; {r}."),
                None => "Stopped at the usage limit.".to_owned(),
            },
            "bad",
        ),
        State::Interrupted { detail, .. } => (
            match detail {
                Some(d) => format!("Stopped: {}.", d.trim_end_matches('.')),
                None => "Stopped before it finished.".to_owned(),
            },
            "bad",
        ),
        State::Quiet { since } => (
            match since {
                Some(t) => format!("Quiet since {}.", super::hm(*t, offset)),
                None => "Quiet.".to_owned(),
            },
            "",
        ),
        _ => {
            let checked = claims.verified > 0;
            let s = match (&merged, &opened, checked) {
                (Some(n), _, true) => format!("Merged {n}. Its claims were checked."),
                (Some(n), _, false) => format!("Merged {n}."),
                (None, Some(n), _) => format!("Opened {n}."),
                (None, None, true) => "Done. Its claims were checked.".to_owned(),
                (None, None, false) => "Done.".to_owned(),
            };
            (
                s,
                if checked || merged.is_some() {
                    "ok"
                } else {
                    ""
                },
            )
        }
    }
}

/// The last [`RECENT`] finished top-level sessions, newest first.
pub fn recent(fleet: &Fleet, now: OffsetDateTime, offset: UtcOffset) -> Vec<Recent> {
    let mut rows: Vec<(&Row, OffsetDateTime)> = fleet
        .rows
        .iter()
        .filter(|r| r.delegated_by.is_none())
        .filter_map(|r| ended(r).map(|t| (r, t)))
        .collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.1));
    rows.into_iter()
        .take(RECENT)
        .map(|(r, at)| {
            let (line, mark) = line(r, offset);
            Recent {
                id: r.id.clone(),
                title: r.title.clone(),
                agent: agent(&r.tool),
                profile: r.profile.clone(),
                project: r.project.clone(),
                at,
                when: day_hm(at, now, offset),
                line,
                mark,
            }
        })
        .collect()
}
