//! `hippo stats repeats`: how often the person repeats a rule they already
//! gave, per 100 person-typed sessions per week, and why each recent
//! repeat happened (`docs/designs/remember.md`, "The metric").
//!
//! Definitions, the same on anyone's history:
//! - a person-typed session is a top-level session a person started with
//!   at least one turn they typed ([`super::detect::person_sessions`]);
//! - a correction is a group member from a transcript;
//! - a repeat is a session in which the person corrected a group that
//!   already had a correction in an earlier, different session, or a
//!   memory line hippo read before; one per (group, session).
//!
//! Every week is computed from the current group store, so a better
//! grouping changes past weeks too.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use time::{Date, Duration, OffsetDateTime};
use ulid::Ulid;

use super::groups::{Group, Source, Store};
use super::{Lesson, Status};

/// Why a repeat happened, from its group's rule when it happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Cause {
    /// No rule had been drafted, or it was declined, rejected or dismissed.
    NotLearned,
    /// A rule was drafted and waiting for the person.
    Pending,
    /// The rule was kept and the session was not given it.
    NotDelivered,
    /// The rule was in what the session was given.
    DeliveredIgnored,
}

impl Cause {
    pub fn as_str(self) -> &'static str {
        match self {
            Cause::NotLearned => "not learned",
            Cause::Pending => "pending",
            Cause::NotDelivered => "learned, not delivered",
            Cause::DeliveredIgnored => "delivered and ignored",
        }
    }
}

/// One repeat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Repeat {
    pub group: Ulid,
    pub session: String,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    pub cause: Cause,
}

/// One ISO week (UTC).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Week {
    /// `2026-W47`.
    pub week: String,
    /// Its Monday.
    pub start: Date,
    pub sessions: usize,
    pub corrections: usize,
    pub repeats: usize,
    pub corrections_per_100: Option<f64>,
    pub repeats_per_100: Option<f64>,
}

/// What `hippo stats repeats` prints.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Report {
    /// Oldest first; weeks before [`Report::covered_from`] are left out.
    pub weeks: Vec<Week>,
    /// The repeats of the last 30 days by cause, every cause listed.
    pub breakdown: Vec<(Cause, usize)>,
    /// Groups are complete from this time; `None` after a whole backfill.
    #[serde(with = "time::serde::rfc3339::option")]
    pub covered_from: Option<OffsetDateTime>,
    pub backfilled: bool,
}

impl Report {
    /// This week's repeats per 100 sessions and those of the week 4 weeks
    /// before, when both weeks are covered.
    pub fn headline(&self) -> (Option<f64>, Option<f64>) {
        let n = self.weeks.len();
        let this = self.weeks.last().and_then(|w| w.repeats_per_100);
        let before = (n >= 5)
            .then(|| self.weeks[n - 5].repeats_per_100)
            .flatten();
        (this, before)
    }
}

fn monday(d: Date) -> Date {
    d - Duration::days(i64::from(d.weekday().number_days_from_monday()))
}

fn week_name(d: Date) -> String {
    let (year, week, _) = d.to_iso_week_date();
    format!("{year}-W{week:02}")
}

/// What a session was given: kept rules by id, from the delivery log, and,
/// for sessions before it, the text of its recorded start block.
pub struct Delivered<'a> {
    pub log: &'a BTreeMap<String, BTreeSet<Ulid>>,
    pub text: &'a dyn Fn(&str) -> Option<String>,
}

/// The cause of a repeat in `session` at `at`, by the state of `lesson`
/// then.
pub fn cause(lesson: Option<&Lesson>, session: &str, at: OffsetDateTime, d: &Delivered) -> Cause {
    let Some(l) = lesson.filter(|l| l.created <= at) else {
        return Cause::NotLearned;
    };
    if let Some(k) = l.kept.as_ref().filter(|k| k.at <= at) {
        let _ = k;
        let undone = l.status != Status::Kept && l.updated <= at;
        if undone {
            return Cause::NotLearned;
        }
        let logged = d.log.get(session).is_some_and(|ids| ids.contains(&l.id));
        let in_text = (d.text)(session).is_some_and(|t| {
            let rule = l.rule.split_whitespace().collect::<Vec<_>>().join(" ");
            !rule.is_empty()
                && t.split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .contains(&rule)
        });
        return if logged || in_text {
            Cause::DeliveredIgnored
        } else {
            Cause::NotDelivered
        };
    }
    let confirmed = l
        .review
        .as_ref()
        .is_some_and(|r| r.verdict == crate::records::Verdict::Confirm);
    let closed_before = matches!(
        l.status,
        Status::Dismissed | Status::Declined | Status::Rejected
    ) && l.updated <= at;
    if confirmed && !closed_before {
        Cause::Pending
    } else {
        Cause::NotLearned
    }
}

/// The repeats in `store`: for each group, each session after the first
/// that corrected it (or after a memory line read earlier), once.
pub fn repeats(store: &Store, lessons: &[Lesson], d: &Delivered) -> Vec<Repeat> {
    let mut out = Vec::new();
    for g in &store.groups {
        out.extend(group_repeats(g, lessons, d));
    }
    out.sort_by_key(|r| r.at);
    out
}

fn root(id: &str) -> &str {
    id.split_once("-agent-").map_or(id, |(p, _)| p)
}

fn group_repeats(g: &Group, lessons: &[Lesson], d: &Delivered) -> Vec<Repeat> {
    let lesson = g.lesson.and_then(|id| lessons.iter().find(|l| l.id == id));
    let memory_first = g
        .members
        .iter()
        .filter(|m| m.source == Source::Memory)
        .map(|m| m.at)
        .min();
    // Each session's first correction of the group, oldest first.
    let mut first: BTreeMap<&str, OffsetDateTime> = BTreeMap::new();
    for m in g.members.iter().filter(|m| m.source == Source::Correction) {
        // A member is a turn the person typed by construction.
        let Some(s) = m.session.as_deref().map(root) else {
            continue;
        };
        let e = first.entry(s).or_insert(m.at);
        if m.at < *e {
            *e = m.at;
        }
    }
    let mut by_time: Vec<(&str, OffsetDateTime)> = first.into_iter().collect();
    by_time.sort_by_key(|(_, at)| *at);
    let mut out = Vec::new();
    for (k, (session, at)) in by_time.iter().enumerate() {
        let earlier = k > 0 || memory_first.is_some_and(|m| m < *at);
        if earlier {
            out.push(Repeat {
                group: g.id,
                session: (*session).to_owned(),
                at: *at,
                cause: cause(lesson, session, *at, d),
            });
        }
    }
    out
}

/// The report for the `weeks` ISO weeks up to the one holding `now`.
pub fn report(
    store: &Store,
    lessons: &[Lesson],
    people: &BTreeMap<String, OffsetDateTime>,
    d: &Delivered,
    weeks: usize,
    now: OffsetDateTime,
) -> Report {
    let all = repeats(store, lessons, d);
    let this = monday(now.date());
    let covered_from = if store.backfilled.is_some() {
        None
    } else {
        store.covered_from
    };
    let first_week = covered_from.map(|c| monday(c.date()));
    let mut out = Vec::new();
    for k in (0..weeks.max(1)).rev() {
        let start = this - Duration::weeks(k as i64);
        // A week only part covered by the store is left out.
        if first_week.is_some_and(|f| start <= f && covered_from.is_some_and(|c| c.date() > start))
        {
            continue;
        }
        let end = start + Duration::weeks(1);
        let in_week = |t: OffsetDateTime| t.date() >= start && t.date() < end;
        let sessions = people.values().filter(|t| in_week(**t)).count();
        let corrections = store
            .groups
            .iter()
            .flat_map(|g| &g.members)
            .filter(|m| m.source == Source::Correction && in_week(m.at))
            .count();
        let repeats = all.iter().filter(|r| in_week(r.at)).count();
        let per = |n: usize| (sessions > 0).then(|| (n as f64) * 100.0 / sessions as f64);
        out.push(Week {
            week: week_name(start),
            start,
            sessions,
            corrections,
            repeats,
            corrections_per_100: per(corrections),
            repeats_per_100: per(repeats),
        });
    }
    let since = now - Duration::days(30);
    let mut breakdown: BTreeMap<Cause, usize> = [
        Cause::NotLearned,
        Cause::Pending,
        Cause::NotDelivered,
        Cause::DeliveredIgnored,
    ]
    .into_iter()
    .map(|c| (c, 0))
    .collect();
    for r in all.iter().filter(|r| r.at >= since) {
        *breakdown.entry(r.cause).or_default() += 1;
    }
    Report {
        weeks: out,
        breakdown: breakdown.into_iter().collect(),
        covered_from,
        backfilled: store.backfilled.is_some(),
    }
}

/// The report from what is on disk: the store, the lessons, the index's
/// person-typed sessions, the delivery log and, with a workspace, the
/// session records.
pub fn from_home(
    cfg: &crate::config::Config,
    ws: Option<&crate::config::Workspace>,
    weeks: usize,
    now: OffsetDateTime,
) -> crate::error::Result<Report> {
    let index = crate::index::Index::open(&cfg.home, false)?;
    let rollup =
        crate::rollup::Rollup::open(&cfg.home)?.ok_or(crate::error::Error::MissingIndex)?;
    let since = now - Duration::weeks(weeks.max(1) as i64 + 1);
    let people = super::detect::person_sessions(&index, &rollup, since)?;
    let store = Store::load(&cfg.home)?;
    let lessons = super::list(&cfg.home)?;
    let mut log: BTreeMap<String, BTreeSet<Ulid>> = BTreeMap::new();
    for i in crate::context::topic::read_log(&cfg.home) {
        log.entry(i.session).or_default().extend(i.rules);
    }
    let text = |session: &str| -> Option<String> {
        let ws = ws?;
        let s = crate::sessions::get(ws, session).ok()??;
        Some(format!(
            "{}\n{}",
            s.context_start.unwrap_or_default(),
            s.context_prompt.unwrap_or_default()
        ))
    };
    let d = Delivered {
        log: &log,
        text: &text,
    };
    Ok(report(&store, &lessons, &people, &d, weeks, now))
}

#[cfg(test)]
mod tests {
    use super::super::groups::{Matched, Member};
    use super::*;

    fn at(day: i64) -> OffsetDateTime {
        // A Monday.
        OffsetDateTime::parse(
            "2026-11-02T10:00:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap()
            + Duration::days(day)
    }

    fn member(session: &str, day: i64) -> Member {
        Member {
            session: Some(session.to_owned()),
            ordinal: Some(1),
            at: at(day),
            profile: None,
            project: None,
            quote: "use pnpm".into(),
            action: None,
            terms: vec!["pnpm".into()],
            source: Source::Correction,
            memory: None,
            matched: Matched::New,
        }
    }

    #[test]
    fn repeats_count_sessions_after_the_first_once_each_by_week() {
        let mut store = Store {
            covered_from: Some(OffsetDateTime::UNIX_EPOCH),
            ..Store::default()
        };
        let g = Group {
            id: Ulid::nil(),
            reverses: None,
            lesson: None,
            members: vec![
                member("s1", 0),
                member("s2", 1),
                member("s2", 1),
                member("s3", 8),
            ],
            created: at(0),
        };
        store.groups.push(g);
        let people: BTreeMap<String, OffsetDateTime> = (1..=10)
            .map(|k| (format!("s{k}"), at(if k <= 5 { 0 } else { 8 })))
            .collect();
        let log = BTreeMap::new();
        let none = |_: &str| None;
        let d = Delivered {
            log: &log,
            text: &none,
        };
        let r = report(&store, &[], &people, &d, 2, at(9));
        assert_eq!(r.weeks.len(), 2);
        // Week 1: 5 sessions, 3 corrections, 1 repeat (s2 once).
        assert_eq!(
            (
                r.weeks[0].sessions,
                r.weeks[0].corrections,
                r.weeks[0].repeats
            ),
            (5, 3, 1)
        );
        assert_eq!(r.weeks[0].repeats_per_100, Some(20.0));
        assert_eq!((r.weeks[1].sessions, r.weeks[1].repeats), (5, 1));
        assert_eq!(r.breakdown[0], (Cause::NotLearned, 2));
    }

    #[test]
    fn a_repeat_after_a_kept_rule_is_delivered_or_not_by_the_log() {
        let lesson: Lesson = serde_json::from_value(serde_json::json!({
            "id": Ulid::nil().to_string(), "kind": "correction", "key": "k", "title": "t",
            "rule": "Use pnpm, never npm.", "target": null, "evidence": [],
            "seen": {"sessions": 2, "profiles": [], "first": "2026-11-01T00:00:00Z", "last": "2026-11-01T00:00:00Z"},
            "status": "kept", "created": "2026-11-01T00:00:00Z", "updated": "2026-11-01T00:00:00Z",
            "kept": {"receipt": Ulid::nil().to_string(), "at": "2026-11-01T00:00:00Z", "edited": false},
            "review": {"reviewer": "r", "invocation": "i", "verdict": "confirm", "notes": "", "at": "2026-11-01T00:00:00Z"}
        }))
        .unwrap();
        let mut log = BTreeMap::new();
        log.insert("s9".to_owned(), BTreeSet::from([Ulid::nil()]));
        let text = |s: &str| (s == "s8").then(|| "rules:\n- Use pnpm, never   npm.".to_owned());
        let d = Delivered {
            log: &log,
            text: &text,
        };
        assert_eq!(
            cause(Some(&lesson), "s9", at(5), &d),
            Cause::DeliveredIgnored
        );
        assert_eq!(
            cause(Some(&lesson), "s8", at(5), &d),
            Cause::DeliveredIgnored
        );
        assert_eq!(cause(Some(&lesson), "s7", at(5), &d), Cause::NotDelivered);
        assert_eq!(cause(Some(&lesson), "s7", at(-5), &d), Cause::NotLearned);
        assert_eq!(cause(None, "s7", at(5), &d), Cause::NotLearned);
    }
}
