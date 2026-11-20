//! Morning: the last nightly pass, the rules waiting for the person, and
//! what the pass applied and what its reviewer rejected.
//!
//! The pass is the newest dreaming report that is not a dry run's. Its
//! window (`## Window`) says which sessions it read; its `## Inputs` say
//! which notes each pass checked; its proposals' receipts say what was
//! applied and what was rejected, the reviewer's reason in the receipt's
//! `notes`. Pending rules are the lessons waiting in the Inbox, each with
//! the evidence it was drafted from.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use hippo_core::config::Workspace;
use hippo_core::dream::{History, RevertState};
use hippo_core::lessons::{Kind, Lesson, Status};
use hippo_core::records::{ProposalKind, Verdict};
use hippo_core::rollup::Rollup;
use serde::Serialize;
use time::{OffsetDateTime, UtcOffset};
use ulid::Ulid;

use super::{agent, count, day_hm, hm, long_date, took, Agent};
use crate::dreaming::{Dreaming, Run};
use crate::worddiff::{Piece, WordDiff};

/// At most this many characters of a was/now pair's side.
const SIDE_CHARS: usize = 90;

/// Morning's model.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Morning {
    /// `Friday 20 November · night pass at 03:30, 4 min 12 s`.
    pub kicker: String,
    /// `Last night hippo read 36 sessions`.
    pub headline: String,
    /// `It drafted 3 rules from what you told your agents and fixed 2
    /// notes. The second model rejected 1 fix.`
    pub lead: String,
    pub pass: Option<Pass>,
    pub figures: Figures,
    /// Pending rules, newest evidence first.
    pub rules: Vec<RuleCard>,
    /// The pass's applied changes.
    pub applied: Vec<Fix>,
    /// The pass's rejected changes.
    pub rejected: Vec<Fix>,
}

/// The last pass.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Pass {
    /// The report, relative to the workspace.
    pub report: PathBuf,
    #[serde(with = "time::serde::rfc3339::option")]
    pub started: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub ended: Option<OffsetDateTime>,
    /// `ended - started`, when the report records its start.
    pub seconds: Option<i64>,
    /// `03:30`, local.
    pub at: String,
    /// `4 min 12 s`.
    pub took: Option<String>,
    pub failed: bool,
    /// Top-level sessions with a turn in the pass's window.
    pub sessions_read: usize,
    /// The same, by agent, most first.
    pub by_agent: Vec<(Agent, usize)>,
    /// Distinct notes a pass checked.
    pub notes_checked: usize,
}

/// B's four figures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct Figures {
    pub rules_waiting: usize,
    pub notes_fixed: usize,
    pub fixes_rejected: usize,
    pub notes_checked: usize,
}

/// One pending rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuleCard {
    pub id: Ulid,
    /// `correction`, `failure`, `ask-rule`, `ask-note`, `miss`.
    pub kind: &'static str,
    /// `From 2 corrections`, `From 2 failed runs`, `From a question`.
    pub from: String,
    pub title: String,
    pub rule: String,
    /// The note it goes into, relative to the workspace.
    pub note: Option<PathBuf>,
    /// `Engineering/Preview and staging`, without `.md`.
    pub note_label: Option<String>,
    /// The note does not exist yet.
    pub new_note: bool,
    /// The reviewer's one line.
    pub verdict: Option<String>,
    /// The reviewer confirmed it.
    pub agrees: bool,
    pub evidence: Vec<Said>,
    /// Shared with the team's Inbox.
    pub shared: bool,
}

/// One thing the person said, or one failure, that a rule came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Said {
    pub session: String,
    pub ordinal: u32,
    pub agent: Option<Agent>,
    pub profile: Option<String>,
    pub session_title: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    /// `Thu 10:07`.
    pub when: String,
    /// The person's words (a correction).
    pub said: Option<String>,
    /// A failure's signature.
    pub failed: Option<String>,
}

/// One change the pass applied or its reviewer rejected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Fix {
    pub proposal: Ulid,
    /// The applied confirm receipt; the reject receipt for a rejection.
    pub receipt: Option<Ulid>,
    pub note: PathBuf,
    pub title: String,
    /// What it replaced, cut around the change; `None` for an addition.
    pub was: Option<String>,
    pub now: String,
    /// The reviewer's notes: why it confirmed or rejected.
    pub reason: String,
    /// `03:32`, local, when applied or reviewed.
    pub at: String,
    /// `hippo revert` arguments for Undo; `None` when it cannot be undone
    /// now (a newer change first, the snapshot gone) or was rejected.
    pub undo: Option<Vec<String>>,
    /// It was reverted since.
    pub reverted: bool,
}

/// What [`build`] reads.
pub struct Inputs<'a> {
    pub ws: &'a Workspace,
    pub history: &'a History,
    pub dreaming: &'a Dreaming,
    /// Every lesson (`lessons::list`).
    pub lessons: &'a [Lesson],
    pub rollup: Option<&'a Rollup>,
    /// Note titles by path relative to the workspace.
    pub titles: &'a HashMap<PathBuf, String>,
    pub now: OffsetDateTime,
    pub offset: UtcOffset,
}

/// The newest real run: not a dry run's report, readable.
pub fn last_run(d: &Dreaming) -> Option<&Run> {
    d.runs.iter().find(|r| {
        r.report
            .record
            .as_ref()
            .is_ok_and(|rec| !rec.status.dry_run)
    })
}

/// The `from` and `to` of a report's window bullets
/// (`<from> to <to>: done`), earliest and latest.
fn window(run: &Run) -> Option<(OffsetDateTime, OffsetDateTime)> {
    let d = run.digest.as_ref()?;
    let fmt = &time::format_description::well_known::Rfc3339;
    let mut lo: Option<OffsetDateTime> = None;
    let mut hi: Option<OffsetDateTime> = None;
    for w in &d.window {
        let Some((a, rest)) = w.split_once(" to ") else {
            continue;
        };
        let b = rest.rsplit_once(": ").map_or(rest, |(b, _)| b);
        if let (Ok(a), Ok(b)) = (
            OffsetDateTime::parse(a.trim(), fmt),
            OffsetDateTime::parse(b.trim(), fmt),
        ) {
            lo = Some(lo.map_or(a, |x| x.min(a)));
            hi = Some(hi.map_or(b, |x| x.max(b)));
        }
    }
    Some((lo?, hi?))
}

fn label(path: &Path, notes_dir: &Path) -> String {
    let below = path.strip_prefix(notes_dir).unwrap_or(path);
    below.with_extension("").to_string_lossy().into_owned()
}

fn title_of(path: &Path, titles: &HashMap<PathBuf, String>) -> String {
    titles.get(path).cloned().unwrap_or_else(|| {
        path.file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    })
}

/// A was/now pair cut around the first change: the shared words next to
/// it and the words each side has.
pub fn was_now(old: &str, new: &str) -> (Option<String>, String) {
    let (old_p, new_p) = (crate::edition::plain(old), crate::edition::plain(new));
    if old.trim().is_empty() {
        return (None, crate::edition::sentences(&new_p, SIDE_CHARS));
    }
    let diff = WordDiff::new(&old_p, &new_p);
    if !diff.changed() {
        return (
            Some(crate::edition::sentences(&old_p, SIDE_CHARS)),
            crate::edition::sentences(&new_p, SIDE_CHARS),
        );
    }
    let mut was = Vec::new();
    let mut now = Vec::new();
    for p in diff.excerpt(2, 1, 12) {
        match p {
            Piece::Same(t) => {
                was.push(t.clone());
                now.push(t);
            }
            Piece::Old(t) => was.push(t),
            Piece::New(t) => now.push(t),
            Piece::Cut => {}
        }
    }
    let join = |v: Vec<String>| {
        let s = v.join(" ");
        crate::edition::sentences(s.trim(), SIDE_CHARS)
    };
    let was = join(was);
    (Some(was).filter(|w| !w.is_empty()), join(now))
}

/// Builds Morning.
pub fn build(inp: &Inputs<'_>) -> Morning {
    let notes_dir = hippo_core::notes::normalise(&inp.ws.notes_dir);
    let run = last_run(inp.dreaming);
    let pass = run.map(|r| pass_of(inp, r));

    let mut applied = Vec::new();
    let mut rejected = Vec::new();
    if let Some(r) = run {
        for change in &r.changes {
            let Ok(p) = &change.proposal.record else {
                continue;
            };
            let (was, now) = match p.kind {
                ProposalKind::Append => (
                    None,
                    crate::edition::sentences(&crate::edition::plain(&p.new), SIDE_CHARS),
                ),
                _ => was_now(&p.old, &p.new),
            };
            let reverted = change.receipts.iter().any(|x| x.verdict == Verdict::Revert);
            let confirm = change
                .receipts
                .iter()
                .find(|x| x.verdict == Verdict::Confirm && x.applied.is_some());
            let reject = change
                .receipts
                .iter()
                .find(|x| x.verdict == Verdict::Reject);
            let title = title_of(&p.note, inp.titles);
            if let Some(c) = confirm {
                let state = hippo_core::dream::revert_state_in(inp.ws, inp.history, c.id).ok();
                let undo = match state {
                    Some(RevertState::Revertible) => Some(vec!["revert".into(), c.id.to_string()]),
                    Some(RevertState::NoteChanged) => {
                        Some(vec!["revert".into(), "--force".into(), c.id.to_string()])
                    }
                    _ => None,
                };
                applied.push(Fix {
                    proposal: change.proposal_id,
                    receipt: Some(c.id),
                    note: p.note.clone(),
                    title,
                    was,
                    now,
                    reason: c.notes.clone(),
                    at: hm(c.applied.unwrap_or(c.at), inp.offset),
                    undo,
                    reverted,
                });
            } else if let Some(x) = reject {
                rejected.push(Fix {
                    proposal: change.proposal_id,
                    receipt: Some(x.id),
                    note: p.note.clone(),
                    title,
                    was,
                    now,
                    reason: x.notes.clone(),
                    at: hm(x.at, inp.offset),
                    undo: None,
                    reverted: false,
                });
            }
        }
    }

    let mut rules: Vec<RuleCard> = inp
        .lessons
        .iter()
        .filter(|l| l.status == Status::Pending)
        .map(|l| rule_card(inp, l, &notes_dir))
        .collect();
    rules.sort_by(|a, b| {
        let last = |r: &RuleCard| r.evidence.iter().map(|e| e.at).max();
        last(b).cmp(&last(a)).then(a.id.cmp(&b.id))
    });

    let fixed: BTreeSet<&PathBuf> = applied
        .iter()
        .filter(|f| !f.reverted)
        .map(|f| &f.note)
        .collect();
    let figures = Figures {
        rules_waiting: rules.len(),
        notes_fixed: fixed.len(),
        fixes_rejected: rejected.len(),
        notes_checked: pass.as_ref().map_or(0, |p| p.notes_checked),
    };
    let today = inp.now.to_offset(inp.offset).date();
    let (kicker, headline) = match &pass {
        Some(p) => {
            let when = match &p.took {
                Some(t) => format!("night pass at {}, {t}", p.at),
                None => format!("night pass at {}", p.at),
            };
            let last = match p.ended.or(p.started) {
                Some(t)
                    if t.to_offset(inp.offset).date() == today
                        && t.to_offset(inp.offset).hour() < 12 =>
                {
                    "Last night"
                }
                Some(t) if t.to_offset(inp.offset).date() == today => "Today",
                Some(t) if t.to_offset(inp.offset).date().next_day() == Some(today) => "Yesterday",
                _ => "Last time",
            };
            (
                format!("{} · {when}", long_date(today)),
                if p.failed {
                    format!("{last} hippo's pass failed")
                } else {
                    format!(
                        "{last} hippo read {}",
                        count(p.sessions_read, "session", "sessions")
                    )
                },
            )
        }
        None => (
            long_date(today),
            "hippo has not run a night pass yet".to_owned(),
        ),
    };
    let lead = lead(&figures, pass.is_some());
    Morning {
        kicker,
        headline,
        lead,
        pass,
        figures,
        rules,
        applied,
        rejected,
    }
}

fn lead(f: &Figures, ran: bool) -> String {
    let mut parts = Vec::new();
    if f.rules_waiting > 0 {
        parts.push(format!(
            "drafted {} from what you told your agents",
            count(f.rules_waiting, "rule", "rules")
        ));
    }
    if f.notes_fixed > 0 {
        parts.push(format!("fixed {}", count(f.notes_fixed, "note", "notes")));
    }
    if !ran && parts.is_empty() {
        return "The first night pass reads the day's sessions and checks your notes against them."
            .to_owned();
    }
    let mut s = if parts.is_empty() {
        "Nothing waits for you".to_owned()
    } else {
        format!("It {}", parts.join(" and "))
    };
    s.push('.');
    if f.fixes_rejected > 0 {
        s.push_str(&format!(
            " The second model rejected {}.",
            count(f.fixes_rejected, "fix", "fixes")
        ));
    }
    s
}

fn pass_of(inp: &Inputs<'_>, run: &Run) -> Pass {
    let rec = run.report.record.as_ref().ok();
    let started = rec.and_then(|r| r.started);
    let ended = run.written.map(OffsetDateTime::from);
    let seconds = match (started, ended) {
        (Some(a), Some(b)) if b >= a => Some((b - a).whole_seconds()),
        _ => None,
    };
    let mut by: BTreeMap<String, usize> = BTreeMap::new();
    let mut read = 0;
    if let (Some(r), Some((from, to))) = (inp.rollup, window(run)) {
        for s in r.sessions_in(from, to).unwrap_or_default() {
            if s.facts.parent.is_some() || s.facts.automated {
                continue;
            }
            read += 1;
            *by.entry(s.facts.tool.clone()).or_default() += 1;
        }
    }
    let mut by_agent: Vec<(Agent, usize)> = by.into_iter().map(|(t, n)| (agent(&t), n)).collect();
    by_agent.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.name.cmp(&b.0.name)));
    let checked: BTreeSet<&str> = run
        .digest
        .iter()
        .flat_map(|d| &d.checked)
        .filter_map(|c| c.note.as_deref())
        .collect();
    let report = run
        .report
        .path
        .strip_prefix(&inp.ws.root)
        .unwrap_or(&run.report.path)
        .to_path_buf();
    Pass {
        report,
        started,
        ended,
        seconds,
        at: started
            .or(ended)
            .map(|t| hm(t, inp.offset))
            .unwrap_or_default(),
        took: seconds.map(took),
        failed: rec.is_some_and(|r| r.status.failed),
        sessions_read: read,
        by_agent,
        notes_checked: checked.len(),
    }
}

fn rule_card(inp: &Inputs<'_>, l: &Lesson, notes_dir: &Path) -> RuleCard {
    let n = l.seen.sessions.max(l.evidence.len());
    let from = match l.kind {
        Kind::Correction => format!("From {}", count(n, "correction", "corrections")),
        Kind::Failure => format!("From {}", count(n, "failed run", "failed runs")),
        Kind::AskRule | Kind::AskNote => "From a question you asked".to_owned(),
        Kind::Miss => format!(
            "From {}",
            count(
                n,
                "search that found nothing",
                "searches that found nothing"
            )
        ),
    };
    let evidence = l
        .evidence
        .iter()
        .map(|e| {
            let facts = inp
                .rollup
                .and_then(|r| r.session(&e.session).ok().flatten());
            let quote = crate::text::visible(&e.quote);
            let (said, failed) = match l.kind {
                Kind::Failure => (None, Some(quote)),
                _ => (Some(quote), None),
            };
            Said {
                session: e.session.clone(),
                ordinal: e.ordinal,
                agent: facts.as_ref().map(|f| agent(&f.tool)),
                profile: e
                    .profile
                    .clone()
                    .or_else(|| facts.as_ref().and_then(|f| f.profile())),
                session_title: facts.as_ref().and_then(|f| f.title.clone()),
                at: e.at,
                when: day_hm(e.at, inp.now, inp.offset),
                said,
                failed,
            }
        })
        .collect();
    let review = l.review.as_ref();
    RuleCard {
        id: l.id,
        kind: l.kind.as_str(),
        from,
        title: l.title.clone(),
        rule: l.rule.clone(),
        note: l.target.as_ref().map(|t| t.note.clone()),
        note_label: l.target.as_ref().map(|t| label(&t.note, notes_dir)),
        new_note: l.target.as_ref().is_some_and(|t| t.new),
        verdict: review.map(|r| crate::edition::sentences(&r.notes, 160)),
        agrees: review.is_some_and(|r| r.verdict == Verdict::Confirm),
        evidence,
        shared: l.shared.is_some(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn was_now_cuts_around_the_change() {
        let (was, now) = was_now(
            "Thumbnails are made at 3 sizes for every upload.",
            "Thumbnails are made at 4 sizes (1600 px added) for every upload.",
        );
        assert_eq!(was.as_deref(), Some("made at 3 sizes for every"));
        assert_eq!(now, "made at 4 sizes (1600 px added) for every");
    }

    #[test]
    fn an_addition_has_no_was() {
        let (was, now) = was_now("", "Exports pass --chunk 500.");
        assert_eq!(was, None);
        assert_eq!(now, "Exports pass --chunk 500.");
    }

    #[test]
    fn the_lead_names_what_happened() {
        let f = Figures {
            rules_waiting: 3,
            notes_fixed: 2,
            fixes_rejected: 1,
            notes_checked: 10,
        };
        assert_eq!(
            lead(&f, true),
            "It drafted 3 rules from what you told your agents and fixed 2 notes. The second model rejected 1 fix."
        );
        assert_eq!(lead(&Figures::default(), true), "Nothing waits for you.");
    }
}
