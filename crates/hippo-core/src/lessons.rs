//! The lessons pass: corrections a person gave
//! and failures that repeat across sessions, proposed as rules.
//!
//! [`detect`] finds candidates in the index without a model; [`draft`]
//! drafts a rule for each new cluster with one model call per batch and has
//! a reviewer in a fresh context confirm each one; [`scan`] runs both and keeps
//! the records in `$HIPPO_HOME/lessons/`. Nothing reaches a note before the
//! person keeps a lesson: [`keep`] writes it as dreaming writes, through a
//! proposal and a confirm receipt whose reviewer is `human`, so `hippo
//! revert` undoes it.

pub mod detect;
pub mod draft;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use ulid::Ulid;

use crate::config::{Config, Workspace};
use crate::dream::apply::{self, Applied};
use crate::dream::reviewer::Review;
use crate::dream::{sections, validate};
use crate::error::{Error, Result};
use crate::index::Index;
use crate::notes::{self, Notes};
use crate::records::{self, ProposalKind, Verdict};
use crate::rollup::Rollup;
use crate::util::{hex, json_files, sha256, write_atomic};

/// A lesson's kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Correction,
    Failure,
    AskRule,
    AskNote,
    /// From a search of the notes that found nothing.
    Miss,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Correction => "correction",
            Kind::Failure => "failure",
            Kind::AskRule => "ask-rule",
            Kind::AskNote => "ask-note",
            Kind::Miss => "miss",
        }
    }
}

/// Where a lesson is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Confirmed by the reviewer, waiting in the Inbox.
    Pending,
    Kept,
    /// Dismissed by the person.
    Dismissed,
    /// The drafter found no lasting rule.
    Declined,
    /// The reviewer rejected the draft.
    Rejected,
}

impl Status {
    /// Dismissed, declined or rejected: reopens after 3 more sessions.
    pub fn closed(self) -> bool {
        matches!(
            self,
            Status::Dismissed | Status::Declined | Status::Rejected
        )
    }
}

/// How many more sessions reopen a closed lesson.
pub const REOPEN_AFTER: usize = 3;

/// One thing hippo saw.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    pub session: String,
    pub ordinal: u32,
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    /// The person's words, or a failure's signature; at most 160
    /// characters of redacted index text.
    pub quote: String,
    /// The agent action before it.
    #[serde(default)]
    pub action: Option<String>,
}

/// Where a kept lesson goes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    /// Relative to the workspace.
    pub note: PathBuf,
    /// A heading's text, without its `#`s; `None` for the note's end.
    #[serde(default)]
    pub section: Option<String>,
    /// The note does not exist yet.
    #[serde(default)]
    pub new: bool,
}

/// How often it was seen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seen {
    pub sessions: usize,
    pub profiles: Vec<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub first: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub last: OffsetDateTime,
    /// The most sessions on one local day.
    #[serde(default)]
    pub most_in_a_day: usize,
}

/// The review of the draft, made in a fresh context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LessonReview {
    pub reviewer: String,
    pub invocation: String,
    pub verdict: Verdict,
    pub notes: String,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
}

/// What Keep did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeptBy {
    pub receipt: Ulid,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    pub edited: bool,
}

/// `$HIPPO_HOME/lessons/<id>.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lesson {
    pub id: Ulid,
    pub kind: Kind,
    pub key: String,
    #[serde(default)]
    pub terms: Vec<String>,
    pub title: String,
    pub rule: String,
    pub target: Option<Target>,
    pub evidence: Vec<Evidence>,
    pub seen: Seen,
    pub status: Status,
    #[serde(default)]
    pub review: Option<LessonReview>,
    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
    #[serde(default)]
    pub closed_sessions: Option<usize>,
    #[serde(default)]
    pub kept: Option<KeptBy>,
    /// For a lesson from Ask: the question.
    #[serde(default)]
    pub ask: Option<String>,
}

impl Lesson {
    /// Saves the record atomically.
    pub fn save(&self, home: &Path) -> Result<()> {
        let dir = dir(home);
        fs::create_dir_all(&dir).map_err(|e| Error::io_at(&dir, e))?;
        write_atomic(
            &dir.join(format!("{}.json", self.id)),
            &records::encode(self)?,
        )
    }

    /// Reads one record, refusing a missing or different `protocol`.
    pub fn load(path: &Path) -> Result<Lesson> {
        records::decode(&fs::read(path).map_err(|e| Error::io_at(path, e))?)
    }
}

/// `$HIPPO_HOME/lessons/`.
pub fn dir(home: &Path) -> PathBuf {
    home.join("lessons")
}

/// Every lesson record that loads, newest first. A missing directory is
/// none; a record that does not load is left out.
pub fn list(home: &Path) -> Result<Vec<Lesson>> {
    let mut out: Vec<Lesson> = json_files(&dir(home))?
        .iter()
        .filter_map(|p| Lesson::load(p).ok())
        .collect();
    out.sort_by(|a, b| b.seen.last.cmp(&a.seen.last).then_with(|| b.id.cmp(&a.id)));
    Ok(out)
}

/// The lessons waiting in the Inbox, newest first.
pub fn pending(home: &Path) -> Result<Vec<Lesson>> {
    Ok(list(home)?
        .into_iter()
        .filter(|l| l.status == Status::Pending)
        .collect())
}

/// One lesson by id.
pub fn get(home: &Path, id: Ulid) -> Result<Lesson> {
    let path = dir(home).join(format!("{id}.json"));
    if !path.exists() {
        return Err(Error::InvalidProposal(format!("no lesson {id}")));
    }
    Lesson::load(&path)
}

/// `$HIPPO_HOME/lessons-state.json`: the last scan.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub at: Option<OffsetDateTime>,
    pub days: u32,
    pub sessions: usize,
    pub candidates: usize,
    pub drafted: usize,
    pub confirmed: usize,
    pub model_calls: usize,
    pub error: Option<String>,
}

impl State {
    pub fn path(home: &Path) -> PathBuf {
        home.join("lessons-state.json")
    }

    /// The last scan's state; `None` before the first.
    pub fn load(home: &Path) -> Option<State> {
        fs::read(State::path(home))
            .ok()
            .and_then(|b| records::decode(&b).ok())
    }

    pub fn save(&self, home: &Path) -> Result<()> {
        write_atomic(&State::path(home), &records::encode(self)?)
    }
}

// ---------------------------------------------------------------------------
// Keep and Dismiss

/// What [`keep`] wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Kept {
    pub lesson: Lesson,
    pub receipt: records::Receipt,
    pub proposal: records::Proposal,
}

/// Keeps pending lesson `id`: one change against its target
/// note as it is now, written through a proposal and a confirm receipt
/// whose reviewer is `human`, with a snapshot, so that `hippo revert`
/// undoes it. `title` and `text` replace the drafted title and rule (Edit).
pub fn keep(
    cfg: &Config,
    ws: &Workspace,
    id: Ulid,
    title: Option<&str>,
    text: Option<&str>,
    now: OffsetDateTime,
) -> Result<Kept> {
    let mut lesson = get(&cfg.home, id)?;
    if lesson.status != Status::Pending {
        return Err(Error::InvalidProposal(format!(
            "lesson {id} is {}, not pending",
            serde_json::to_value(lesson.status)?
                .as_str()
                .unwrap_or("closed")
        )));
    }
    let target = lesson
        .target
        .clone()
        .ok_or_else(|| Error::InvalidProposal(format!("lesson {id} has no target note")))?;
    let edited = title.is_some_and(|t| t.trim() != lesson.title)
        || text.is_some_and(|t| t.trim() != lesson.rule);
    let title = title.map_or(lesson.title.clone(), |t| t.trim().to_owned());
    let rule = text.map_or(lesson.rule.clone(), |t| t.trim().to_owned());
    if title.is_empty() || rule.is_empty() {
        return Err(Error::InvalidProposal(
            "a rule needs a title and a text".to_owned(),
        ));
    }
    let note_path = ws.root.join(&target.note);
    let exists = note_path.exists();
    let date = now.to_offset(crate::usage::local_offset_at(now)).date();
    let (kind, old, new, before) = if !exists {
        notes::validate_new_note_path(&ws.root, &ws.notes_dir, &target.note)?;
        let new = format!(
            "---\ntype: policy\nstatus: current\nupdated: {date}\n---\n\n# {title}\n\n{}\n",
            rule.trim_end()
        );
        (ProposalKind::Create, String::new(), new, Vec::new())
    } else {
        notes::validate_proposal_path(&ws.root, &ws.notes_dir, &target.note)?;
        let bytes = fs::read(&note_path).map_err(|e| Error::io_at(&note_path, e))?;
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let bullet = bullet(&title, &rule, date);
        let (kind, old, new) = insertion(&text, target.section.as_deref(), &bullet);
        (kind, old, new, bytes)
    };
    let after = notes::change(&before, kind, old.as_bytes(), new.as_bytes())?;
    let proposal = records::Proposal {
        id: Ulid::generate(),
        pass: "lessons".to_owned(),
        note: target.note.clone(),
        kind,
        before_hash: hex(&sha256(&before)),
        after_hash: hex(&sha256(&after)),
        old,
        new,
        evidence: lesson
            .evidence
            .iter()
            .map(|e| records::Evidence {
                session: e.session.clone(),
                ordinal: e.ordinal,
            })
            .collect(),
        reason: format!("lesson {id} ({}), kept by the person", lesson.kind.as_str()),
    };
    validate::write(ws, &proposal)?;
    let model = lesson.review.as_ref().map_or_else(
        || "no model review".to_owned(),
        |r| {
            format!(
                "{} {} it at {}",
                r.reviewer,
                if r.verdict == Verdict::Confirm {
                    "confirmed"
                } else {
                    "rejected"
                },
                r.at
            )
        },
    );
    let review = Review {
        verdict: Verdict::Confirm,
        notes: format!(
            "kept from lesson {id}; {model}{}",
            if edited {
                "; the person edited the text"
            } else {
                ""
            }
        ),
        reviewer: "human".to_owned(),
        invocation: format!("hippo lessons keep {id}"),
        at: now,
    };
    let receipt = match apply::confirm(ws, &proposal, &review)? {
        Applied::Written(r) => r,
        Applied::NoteChanged(_) => {
            return Err(Error::InvalidProposal(format!(
                "{} changed while the lesson was kept; try again",
                target.note.display()
            )))
        }
    };
    lesson.title = title;
    lesson.rule = rule;
    lesson.status = Status::Kept;
    lesson.updated = now;
    lesson.kept = Some(KeptBy {
        receipt: receipt.id,
        at: now,
        edited,
    });
    lesson.save(&cfg.home)?;
    Ok(Kept {
        lesson,
        receipt,
        proposal,
    })
}

/// Dismisses pending lesson `id`: it reopens only after
/// [`REOPEN_AFTER`] more sessions.
pub fn dismiss(cfg: &Config, id: Ulid, now: OffsetDateTime) -> Result<Lesson> {
    let mut lesson = get(&cfg.home, id)?;
    if lesson.status != Status::Pending {
        return Err(Error::InvalidProposal(format!(
            "lesson {id} is not pending"
        )));
    }
    lesson.status = Status::Dismissed;
    lesson.closed_sessions = Some(lesson.seen.sessions);
    lesson.updated = now;
    lesson.save(&cfg.home)?;
    Ok(lesson)
}

/// The line a kept rule adds to a note.
pub fn bullet(title: &str, rule: &str, date: time::Date) -> String {
    let title = title.trim().trim_end_matches(['.', ':', ';']);
    let rule = rule.split_whitespace().collect::<Vec<_>>().join(" ");
    format!("- **{title}.** {rule} *(lesson kept {date})*\n")
}

/// The change that puts `bullet` into `text`: after the
/// last non-blank line of `section` when it is not the note's last
/// section, as a replacement whose `old` is that line (with lines above
/// it until it occurs once, at most 5); else an append.
pub fn insertion(
    text: &str,
    section: Option<&str>,
    bullet: &str,
) -> (ProposalKind, String, String) {
    let append = || {
        let lead = if text.is_empty() || text.ends_with("\n\n") {
            ""
        } else if text.ends_with('\n') {
            if last_line_is_bullet(text) {
                ""
            } else {
                "\n"
            }
        } else {
            "\n"
        };
        (
            ProposalKind::Append,
            String::new(),
            format!("{lead}{bullet}"),
        )
    };
    let Some(section) = section else {
        return append();
    };
    let parts = sections::sections(text.as_bytes());
    let wanted = section.trim().trim_start_matches('#').trim().to_lowercase();
    let Some(k) = parts.iter().position(|r| {
        let line = text[r.clone()].lines().next().unwrap_or_default();
        line.starts_with('#') && line.trim_start_matches('#').trim().to_lowercase() == wanted
    }) else {
        return append();
    };
    if k + 1 == parts.len() {
        return append();
    }
    let body = &text[parts[k].clone()];
    let lines: Vec<&str> = body.split_inclusive('\n').collect();
    let Some(last) = lines.iter().rposition(|l| !l.trim().is_empty()) else {
        return append();
    };
    for above in 0..5 {
        if above > last {
            break;
        }
        let mut old: String = lines[last - above..=last].concat();
        if !old.ends_with('\n') {
            old.push('\n');
        }
        let (count, _) = notes::occurrences(text.as_bytes(), old.as_bytes());
        if count == 1 {
            let gap = if lines[last].trim_start().starts_with("- ") {
                ""
            } else {
                "\n"
            };
            let new = format!("{old}{gap}{bullet}");
            if old.len() + new.len() <= text.len() / 2 {
                return (ProposalKind::Replace, old, new);
            }
            break;
        }
    }
    append()
}

fn last_line_is_bullet(text: &str) -> bool {
    text.trim_end()
        .lines()
        .last()
        .is_some_and(|l| l.trim_start().starts_with("- "))
}

/// A candidate note for a lesson: its path relative to the workspace, its
/// title and headings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Candidate {
    pub path: PathBuf,
    pub title: String,
    pub headings: Vec<String>,
    /// The entry note or a required note: where the workspace keeps the
    /// rules every agent reads.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub agent_rules: bool,
}

/// The notes the context compiler's relevance score ranks highest for
/// `terms` (a term found in a note's title or a heading scores one), at
/// most `n`, then the entry note and the required notes (marked
/// `agent_rules`) when not among them. The session log and notes of type
/// `log` are never a target.
pub fn candidate_notes(
    ws: &Workspace,
    notes: &Notes,
    terms: &[String],
    n: usize,
) -> Vec<Candidate> {
    let log = notes::normalise(&ws.log_path);
    let mut scored: Vec<(usize, Option<time::Date>, &PathBuf, &notes::Note)> = Vec::new();
    for (path, note) in &notes.notes {
        let full = notes::normalise(&ws.notes_dir.join(path));
        if full == log || note.frontmatter.r#type.as_deref() == Some("log") {
            continue;
        }
        let title = note.title().to_lowercase();
        let headings: Vec<String> = note.headings().iter().map(|h| h.to_lowercase()).collect();
        let score = terms
            .iter()
            .filter(|t| {
                title.contains(t.as_str()) || headings.iter().any(|h| h.contains(t.as_str()))
            })
            .count();
        if score > 0 {
            scored.push((score, note.frontmatter.updated, path, note));
        }
    }
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| b.1.cmp(&a.1))
            .then_with(|| a.2.cmp(b.2))
    });
    let mut chosen: Vec<(&PathBuf, &notes::Note)> = scored
        .into_iter()
        .take(n)
        .map(|(_, _, path, note)| (path, note))
        .collect();
    // Rules usually live in the entry note and the required notes: offer
    // them too.
    let rules: Vec<PathBuf> = std::iter::once(&ws.entry_note)
        .chain(&ws.required_notes)
        .filter_map(|w| {
            notes::normalise(w)
                .strip_prefix(notes::normalise(&ws.notes_dir))
                .map(Path::to_path_buf)
                .ok()
        })
        .collect();
    for wanted in std::iter::once(&ws.entry_note).chain(&ws.required_notes) {
        if chosen.len() >= n + 3 {
            break;
        }
        let Ok(rel) = notes::normalise(wanted)
            .strip_prefix(notes::normalise(&ws.notes_dir))
            .map(Path::to_path_buf)
        else {
            continue;
        };
        if let Some((path, note)) = notes.notes.get_key_value(&rel) {
            if !chosen.iter().any(|(p, _)| *p == path) {
                chosen.push((path, note));
            }
        }
    }
    chosen
        .into_iter()
        .map(|(path, note)| Candidate {
            agent_rules: rules.contains(path),
            path: notes::normalise(&ws.notes_dir.join(path)),
            title: note.title(),
            headings: note
                .headings()
                .iter()
                .map(|h| h.trim_start_matches('#').trim().to_owned())
                .filter(|h| !h.is_empty())
                .take(20)
                .collect(),
        })
        .collect()
}

/// A new note's path for `title`: `<notes_dir>/<title>.md`, the title's
/// characters that are not letters, digits, spaces or `-` dropped.
pub fn new_note_path(ws: &Workspace, title: &str) -> PathBuf {
    let clean: String = title
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '-')
        .collect();
    let clean = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    let stem: String = clean.chars().take(60).collect();
    let stem = if stem.is_empty() {
        "Lesson".to_owned()
    } else {
        stem
    };
    let mut path = ws.notes_dir.join(format!("{stem}.md"));
    let mut k = 2;
    while ws.root.join(&path).exists() {
        path = ws.notes_dir.join(format!("{stem} {k}.md"));
        k += 1;
    }
    notes::normalise(&path)
}

// ---------------------------------------------------------------------------
// The scan

/// What [`scan`] did.
#[derive(Debug, Clone, Default)]
pub struct Scanned {
    pub state: State,
    /// Lines for a report: drafts dropped, model errors.
    pub notes: Vec<String>,
    /// Lessons that reached the Inbox this run.
    pub confirmed: Vec<Lesson>,
}

/// How many days the next scan `hippo dream` runs reads: 21 when no scan
/// has run yet (no `lessons-state.json`), else 7.
pub fn first_scan_days(home: &Path) -> u32 {
    if home.join("lessons-state.json").exists() {
        7
    } else {
        21
    }
}

/// Runs the lessons pass over the last `days` days: detection, then
/// drafting and review of new clusters, then the records. With `dry_run`
/// it drafts and reviews but writes no record or state.
pub fn scan(cfg: &Config, ws: &Workspace, days: u32, dry_run: bool) -> Result<Scanned> {
    let now = OffsetDateTime::now_utc();
    let offset = crate::usage::local_offset_at(now);
    let index = Index::open(&cfg.home, false)?;
    let rollup = Rollup::open(&cfg.home)?.ok_or(Error::MissingIndex)?;
    let since = now - time::Duration::days(i64::from(days.clamp(1, 30)));
    let found = detect::find(&index, &rollup, since, offset)?;
    let notes = Notes::scan(&ws.notes_root())?;
    let mut records = list(&cfg.home)?;
    let mut out = Scanned {
        state: State {
            at: Some(now),
            days,
            sessions: found.sessions,
            candidates: found.corrections.len() + found.failures.len(),
            ..State::default()
        },
        ..Scanned::default()
    };

    // Clusters that match a record update it; the rest are drafted.
    let mut to_draft: Vec<detect::Cluster> = Vec::new();
    for cluster in found.corrections.into_iter().chain(found.failures) {
        match records.iter_mut().find(|l| matches(l, &cluster)) {
            Some(l) => {
                let grew = cluster.sessions.len() > l.seen.sessions;
                merge(l, &cluster, now);
                let reopen = l.status.closed()
                    && l.seen.sessions >= l.closed_sessions.unwrap_or(0) + REOPEN_AFTER;
                if reopen {
                    to_draft.push(cluster);
                } else if grew && !dry_run {
                    l.save(&cfg.home)?;
                }
            }
            None => to_draft.push(cluster),
        }
    }
    to_draft.sort_by(|a, b| {
        b.sessions
            .len()
            .cmp(&a.sessions.len())
            .then_with(|| b.last.cmp(&a.last))
    });
    let mut chosen: Vec<detect::Cluster> = Vec::new();
    for kind in [Kind::Correction, Kind::Failure] {
        chosen.extend(
            to_draft
                .iter()
                .filter(|c| c.kind == kind)
                .take(draft::PER_RUN)
                .cloned(),
        );
    }
    let drafted = draft::draft_clusters(cfg, ws, &notes, &chosen, &mut out)?;
    for (cluster, outcome) in chosen.iter().zip(drafted) {
        let Some(outcome) = outcome else { continue };
        let existing = records.iter().position(|l| matches(l, cluster));
        let mut lesson = match existing {
            Some(i) => records[i].clone(),
            None => new_record(cluster, now),
        };
        merge(&mut lesson, cluster, now);
        match outcome {
            draft::Outcome::Declined => {
                lesson.status = Status::Declined;
                lesson.closed_sessions = Some(lesson.seen.sessions);
            }
            draft::Outcome::Reviewed {
                title,
                rule,
                target,
                review,
            } => {
                lesson.title = title;
                lesson.rule = rule;
                lesson.target = Some(target);
                let confirmed = review.verdict == Verdict::Confirm;
                lesson.review = Some(review);
                if confirmed {
                    lesson.status = Status::Pending;
                    lesson.closed_sessions = None;
                    out.state.confirmed += 1;
                    out.confirmed.push(lesson.clone());
                } else {
                    lesson.status = Status::Rejected;
                    lesson.closed_sessions = Some(lesson.seen.sessions);
                }
            }
        }
        lesson.updated = now;
        if !dry_run {
            lesson.save(&cfg.home)?;
        }
        match existing {
            Some(i) => records[i] = lesson,
            None => records.push(lesson),
        }
    }
    if !dry_run {
        out.state.save(&cfg.home)?;
    }
    Ok(out)
}

/// Whether record `l` is `cluster`'s lesson.
fn matches(l: &Lesson, cluster: &detect::Cluster) -> bool {
    if l.kind != cluster.kind {
        return false;
    }
    if l.key == cluster.key {
        return true;
    }
    l.kind == Kind::Correction && {
        let terms: BTreeSet<String> = l.terms.iter().cloned().collect();
        detect::same_terms(&terms, &cluster.terms)
    }
}

fn new_record(cluster: &detect::Cluster, now: OffsetDateTime) -> Lesson {
    Lesson {
        id: Ulid::generate(),
        kind: cluster.kind,
        key: cluster.key.clone(),
        terms: cluster.terms.iter().cloned().collect(),
        title: String::new(),
        rule: String::new(),
        target: None,
        evidence: Vec::new(),
        seen: Seen {
            sessions: 0,
            profiles: Vec::new(),
            first: cluster.first,
            last: cluster.last,
            most_in_a_day: 0,
        },
        status: Status::Declined,
        review: None,
        created: now,
        updated: now,
        closed_sessions: None,
        kept: None,
        ask: None,
    }
}

/// Adds `cluster`'s evidence and counts to `l`: evidence by (session,
/// ordinal) without repeats, newest first, at most 6; counts never shrink,
/// since the window moves on.
fn merge(l: &mut Lesson, cluster: &detect::Cluster, now: OffsetDateTime) {
    for e in &cluster.evidence {
        if !l
            .evidence
            .iter()
            .any(|x| x.session == e.session && x.ordinal == e.ordinal)
        {
            l.evidence.push(e.clone());
        }
    }
    l.evidence.sort_by_key(|e| std::cmp::Reverse(e.at));
    l.evidence.truncate(detect::KEEP_EVIDENCE);
    let mut terms: BTreeSet<String> = l.terms.iter().cloned().collect();
    terms.extend(cluster.terms.iter().cloned());
    l.terms = terms.into_iter().collect();
    l.seen.sessions = l.seen.sessions.max(cluster.sessions.len());
    let mut profiles: BTreeSet<String> = l.seen.profiles.iter().cloned().collect();
    profiles.extend(cluster.profiles.iter().cloned());
    l.seen.profiles = profiles.into_iter().collect();
    l.seen.first = l.seen.first.min(cluster.first);
    l.seen.last = l.seen.last.max(cluster.last);
    l.seen.most_in_a_day = l.seen.most_in_a_day.max(cluster.most_in_a_day);
    let _ = now;
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests;

#[cfg(test)]
mod first_scan_tests {
    #[test]
    fn the_first_scan_reads_21_days_then_7() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(super::first_scan_days(home.path()), 21);
        std::fs::write(home.path().join("lessons-state.json"), "{}").unwrap();
        assert_eq!(super::first_scan_days(home.path()), 7);
    }
}
