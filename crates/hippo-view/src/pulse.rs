//! Pulse: Fleet's default view, read from today's fleet, its lanes and
//! the usage snapshot ([`crate::fleet`], [`crate::replay`],
//! [`hippo_core::usage::drain`]). Counts, a histogram, the nightly pass's
//! forecast and one ranked list of what will want the person next. No
//! model call, nothing written.

use std::collections::{BTreeMap, HashSet};

use hippo_core::incidents::{Incident, Kind as IncidentKind};
use hippo_core::lessons::{Lesson, Status as LessonStatus};
use hippo_core::rollup::Rollup;
use time::{Duration, OffsetDateTime};
use ulid::Ulid;

use crate::fleet::{Fleet, Row, State};
use crate::replay::{beat_of, Day, SpanKind, BEAT};

/// The span of "tool calls lately".
pub const LATELY: Duration = Duration::minutes(5);

/// The header's numbers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Totals {
    /// Live sessions, and sub-agents running under them.
    pub working: usize,
    pub subs: usize,
    /// Sessions waiting on the person or stopped at a limit nothing
    /// carried on.
    pub need_you: usize,
    /// Tool calls in the last [`LATELY`], sub-agents' included.
    pub calls_lately: usize,
    /// Working time today, sub-agents' included.
    pub work: Duration,
    /// Lines added and removed today, by each session and its sub-agents,
    /// files counted once per session.
    pub added: u64,
    pub removed: u64,
}

/// The header's numbers at `now`.
pub fn totals(fleet: &Fleet, day: Option<&Day>, now: OffsetDateTime) -> Totals {
    let mut t = Totals {
        working: fleet.working(),
        need_you: fleet.needs_you().len(),
        ..Totals::default()
    };
    for r in fleet.rows.iter().filter(|r| r.state.is_live()) {
        t.subs += r
            .tasks
            .iter()
            .filter(|k| k.kind == "agent" && k.running(&r.state))
            .count()
            .max(r.live_subs.len());
    }
    for r in &fleet.rows {
        if let Some(l) = r.total_lines() {
            t.added += l.added;
            t.removed += l.removed;
        }
    }
    if let Some(day) = day {
        t.work = day.line.work;
        for lane in &day.lanes {
            t.calls_lately += lane
                .track
                .calls_between(now - LATELY, now + Duration::SECOND);
            for c in &lane.children {
                t.calls_lately += c.calls_between(now - LATELY, now + Duration::SECOND);
            }
        }
    }
    t
}

/// Activity per beat, stacked by profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Histogram {
    /// The stacks' profiles, in order (`None` for a session with none).
    pub profiles: Vec<Option<String>>,
    /// One bar per beat from `from`: each profile's activity, in the
    /// order of `profiles`.
    pub bars: Vec<Vec<u32>>,
    pub from: OffsetDateTime,
}

impl Histogram {
    /// The tallest bar.
    pub fn peak(&self) -> u32 {
        self.bars
            .iter()
            .map(|b| b.iter().sum::<u32>())
            .max()
            .unwrap_or(0)
    }
}

/// The day's activity from `from` up to `to`, a bar per beat, each stacked
/// by its lanes' profiles (sub-agents count with their lane). Profiles
/// are in `order` first, then by name.
pub fn histogram(
    day: &Day,
    from: OffsetDateTime,
    to: OffsetDateTime,
    order: &[String],
) -> Histogram {
    let mut profiles: Vec<Option<String>> = order.iter().cloned().map(Some).collect();
    let mut rest: Vec<Option<String>> = day
        .lanes
        .iter()
        .map(|l| l.profile.clone())
        .filter(|p| !profiles.contains(p))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    rest.sort();
    profiles.extend(rest);
    let a = beat_of(day.start, from.max(day.start)).unwrap_or(0);
    let b = beat_of(day.start, to.min(day.end)).map_or(0, |b| b + 1);
    let n = b.saturating_sub(a);
    let mut bars = vec![vec![0u32; profiles.len()]; n];
    for lane in &day.lanes {
        let Some(p) = profiles.iter().position(|p| *p == lane.profile) else {
            continue;
        };
        for t in std::iter::once(&lane.track).chain(lane.children.iter()) {
            for (k, v) in t.beats.iter().enumerate().skip(a).take(n) {
                bars[k - a][p] += u32::from(*v);
            }
        }
    }
    Histogram {
        profiles,
        bars,
        from: day.start + BEAT * a as i32,
    }
}

/// One session's beats from `from` up to `to`, its own and its
/// sub-agents', with a flag per beat for whether it was waiting on the
/// person.
pub fn lane_beats(
    day: &Day,
    id: &str,
    from: OffsetDateTime,
    to: OffsetDateTime,
) -> Option<(Vec<u16>, Vec<bool>)> {
    let lane = day.lanes.iter().find(|l| l.track.id == id)?;
    let a = beat_of(day.start, from.max(day.start)).unwrap_or(0);
    let b = beat_of(day.start, to.min(day.end)).map_or(0, |b| b + 1);
    let n = b.saturating_sub(a);
    let mut beats = vec![0u16; n];
    for t in std::iter::once(&lane.track).chain(lane.children.iter()) {
        for (k, v) in t.beats.iter().enumerate().skip(a).take(n) {
            beats[k - a] = beats[k - a].saturating_add(*v);
        }
    }
    let mut waiting = vec![false; n];
    for s in lane
        .track
        .spans
        .iter()
        .filter(|s| s.kind == SpanKind::Waiting)
    {
        let (Some(x), Some(y)) = (beat_of(day.start, s.start), beat_of(day.start, s.end)) else {
            continue;
        };
        for k in x.max(a)..=y.min(b.saturating_sub(1)) {
            if let Some(w) = waiting.get_mut(k - a) {
                *w = true;
            }
        }
    }
    Some((beats, waiting))
}

/// What the next nightly pass will read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tonight {
    /// Its clock time, `HH:MM` (the configuration's `dream_at`).
    pub at: String,
    /// Top-level sessions with a turn since the last pass.
    pub sessions: usize,
    pub corrections: usize,
    pub destructive: usize,
    /// Unverified claims of those sessions.
    pub unverified: usize,
    /// Notes whose `review_after` has passed.
    pub notes_due: usize,
}

/// The nightly pass's forecast: what happened since `since` (its last
/// success, else a day before `now`).
pub fn tonight(
    rollup: &Rollup,
    at: &str,
    since: Option<OffsetDateTime>,
    notes_due: usize,
    now: OffsetDateTime,
) -> Tonight {
    let since = since.unwrap_or(now - Duration::days(1));
    let ids = rollup.top_level_in(since, now).unwrap_or_default();
    let mut out = Tonight {
        at: at.to_owned(),
        sessions: ids.len(),
        notes_due,
        ..Tonight::default()
    };
    for i in rollup.incidents(since).unwrap_or_default() {
        if i.quiet || i.at >= now {
            continue;
        }
        match i.kind {
            IncidentKind::Corrected => out.corrections += 1,
            IncidentKind::Destructive => out.destructive += 1,
            _ => {}
        }
    }
    for id in &ids {
        out.unverified += rollup
            .claims_of(id)
            .unwrap_or_default()
            .iter()
            .filter(|c| c.status == hippo_core::xray::Status::Unverified)
            .count();
    }
    out
}

/// What will happen to a correction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fate {
    /// A pending lesson already holds it: a rule candidate, seen in this
    /// many sessions.
    Candidate { lesson: Ulid, seen: usize },
    /// The nightly pass reads it.
    Tonight,
}

/// A correction of the person's today.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Corrected {
    pub incident: String,
    /// The top-level session.
    pub session: String,
    pub title: Option<String>,
    /// The person's words.
    pub said: String,
    pub at: OffsetDateTime,
    pub fate: Fate,
}

/// Today's corrections (incidents since `start`), newest first, each with
/// the pending lesson that holds it, if any.
pub fn corrections(
    incidents: &[Incident],
    lessons: &[Lesson],
    start: OffsetDateTime,
) -> Vec<Corrected> {
    let mut out: Vec<Corrected> = incidents
        .iter()
        .filter(|i| i.kind == IncidentKind::Corrected && !i.quiet && i.at >= start)
        .map(|i| {
            let showed = &i.body.showed;
            let fate = lessons
                .iter()
                .filter(|l| l.status == LessonStatus::Pending)
                .find(|l| {
                    l.evidence.iter().any(|e| {
                        e.session == showed.session
                            && (e.ordinal == showed.ordinal || e.at == showed.at)
                    })
                })
                .map_or(Fate::Tonight, |l| Fate::Candidate {
                    lesson: l.id,
                    seen: l.seen.sessions,
                });
            Corrected {
                incident: i.id.clone(),
                session: i.body.chain.root.clone(),
                title: i.body.chain.root_title.clone(),
                said: showed.text.clone(),
                at: i.at,
                fate,
            }
        })
        .collect();
    out.sort_by_key(|c| std::cmp::Reverse(c.at));
    out
}

/// One thing that will want the person soon.
#[derive(Debug, Clone, PartialEq)]
pub enum Next {
    /// An account's window empties before its reset.
    RunsDry {
        profile: String,
        empty_at: i64,
        reset: i64,
        /// Its live sessions, which will stop.
        sessions: Vec<String>,
        /// The account to resume them on.
        to: Option<String>,
    },
    /// A finished session said something no command or edit showed.
    Unverified {
        session: String,
        title: String,
        claim: String,
        why: String,
        at: OffsetDateTime,
    },
    /// A destructive command that harm followed.
    Destructive {
        session: String,
        title: Option<String>,
        incident: String,
        command: String,
        at: OffsetDateTime,
    },
    /// A correction a pending lesson already holds.
    Correction {
        session: String,
        said: String,
        at: OffsetDateTime,
        lesson: Ulid,
        seen: usize,
    },
}

impl Next {
    /// Its rank: lower comes first.
    pub fn rank(&self) -> u8 {
        match self {
            Next::RunsDry { .. } => 0,
            Next::Unverified { .. } => 1,
            Next::Destructive { .. } => 2,
            Next::Correction { .. } => 3,
        }
    }
}

/// What will want the person soon, most urgent first: accounts that run
/// dry before their reset (soonest first), finished sessions with an
/// unverified claim, today's destructive incidents and corrections that
/// are rule candidates (newest first).
pub fn next_up(
    fleet: &Fleet,
    incidents: &[Incident],
    corrected: &[Corrected],
    start: OffsetDateTime,
) -> Vec<Next> {
    let mut dry: Vec<Next> = fleet
        .drains
        .iter()
        .filter_map(|(p, d)| {
            let empty_at = d.empty_at?;
            let sessions: Vec<String> = fleet
                .rows
                .iter()
                .filter(|r| r.profile.as_deref() == Some(p) && r.state.is_live())
                .map(|r| r.id.clone())
                .collect();
            Some(Next::RunsDry {
                profile: p.clone(),
                empty_at,
                reset: d.end,
                sessions,
                to: fleet.use_next.clone().filter(|u| u != p),
            })
        })
        .collect();
    dry.sort_by_key(|n| match n {
        Next::RunsDry { empty_at, .. } => *empty_at,
        _ => 0,
    });
    let mut later: Vec<(OffsetDateTime, Next)> = Vec::new();
    for r in fleet.rows.iter().filter(|r| r.needs_check()) {
        let State::Finished { at } = r.state else {
            continue;
        };
        let Some((claim, why)) = r.claims.first_unverified.clone() else {
            continue;
        };
        later.push((
            at,
            Next::Unverified {
                session: r.id.clone(),
                title: r.title.clone(),
                claim,
                why,
                at,
            },
        ));
    }
    for i in incidents
        .iter()
        .filter(|i| i.kind == IncidentKind::Destructive && !i.quiet && i.at >= start)
    {
        later.push((
            i.at,
            Next::Destructive {
                session: i.body.chain.root.clone(),
                title: i.body.chain.root_title.clone(),
                incident: i.id.clone(),
                command: i.body.did.text.clone(),
                at: i.at,
            },
        ));
    }
    for c in corrected {
        if let Fate::Candidate { lesson, seen } = c.fate {
            later.push((
                c.at,
                Next::Correction {
                    session: c.session.clone(),
                    said: c.said.clone(),
                    at: c.at,
                    lesson,
                    seen,
                },
            ));
        }
    }
    later.sort_by(|(a, x), (b, y)| x.rank().cmp(&y.rank()).then(b.cmp(a)));
    dry.extend(later.into_iter().map(|(_, n)| n));
    dry
}

/// Notes in `ws` whose `review_after` has passed; 0 when the notes
/// cannot be read.
pub fn notes_due(ws: &hippo_core::config::Workspace) -> usize {
    hippo_core::notes::Notes::scan(&ws.notes_root())
        .map(|n| {
            n.lint()
                .iter()
                .filter(|f| matches!(f, hippo_core::notes::LintFinding::Overdue { .. }))
                .count()
        })
        .unwrap_or(0)
}

/// Sessions today that started with at least one kept rule.
pub fn served(rows: &[Row]) -> usize {
    rows.iter().filter(|r| !r.rules.is_empty()).count()
}

/// Live sessions per profile.
pub fn live_by_profile(rows: &[Row]) -> BTreeMap<String, usize> {
    let mut out = BTreeMap::new();
    for r in rows.iter().filter(|r| r.state.is_live()) {
        if let Some(p) = &r.profile {
            *out.entry(p.clone()).or_default() += 1;
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests;
