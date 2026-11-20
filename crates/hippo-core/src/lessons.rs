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
pub mod groups;
pub mod memory;
pub mod team;

use std::collections::{BTreeMap, BTreeSet};
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
    /// The runner that reviewed; absent before 0.5.0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner: Option<crate::runners::RunnerRecord>,
}

/// What Keep did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeptBy {
    pub receipt: Ulid,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    pub edited: bool,
    /// Who kept it; absent before 0.8.0, when only the person could.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<Keeper>,
}

/// Who kept a rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Keeper {
    /// The person, with no agent marker in the environment.
    Person,
    /// An agent's command, answering hippo's ask.
    Agent,
    /// `auto_keep`.
    Auto,
}

/// Where a rule applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// How the person wants agents to work anywhere: delivered in every
    /// session, inside the workspace or not.
    Machine,
    /// About the workspace's code, notes or product.
    Workspace,
}

/// A lesson's place among the person's rules (`docs/designs/remember.md`):
/// optional fields, flat in the record, absent before 0.8.0.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Links {
    /// Its group in `groups.json`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<Ulid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<Scope>,
    /// For a rule read from an agent memory: the source's id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// The lessons whose rules this one replaces.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supersedes: Vec<Ulid>,
    /// The lesson that replaced this one; set with status `dismissed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<Ulid>,
    /// How many times its group was drafted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drafts: Option<u32>,
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
    /// Its directory in the shared Inbox (`L-<id>`), once shared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared: Option<String>,
    #[serde(flatten)]
    pub links: Links,
}

impl Lesson {
    /// Saves the record atomically.
    pub fn save(&self, home: &Path) -> Result<()> {
        let dir = dir(home);
        fs::create_dir_all(&dir).map_err(|e| Error::io_at(&dir, e))?;
        write_atomic(
            &dir.join(format!("{}.json", self.id)),
            &records::encode_shared(self)?,
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
    /// Clusters that matched a lesson in the shared Inbox: counted, not
    /// drafted.
    #[serde(default)]
    pub shared_matched: usize,
    /// Corrections placed into groups this run.
    #[serde(default)]
    pub placed: usize,
    /// Match calls made (counted in `model_calls` too).
    #[serde(default)]
    pub match_calls: usize,
    /// Corrections left for a later run: the call cap, or a model error.
    #[serde(default)]
    pub left: usize,
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
    /// What became of the rules it replaces, one line each.
    pub replaced: Vec<String>,
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
    let lesson = get(&cfg.home, id)?;
    keep_lesson(cfg, ws, lesson, title, text, now)
}

/// [`keep`] on a lesson record in hand: a local one, or one made from a
/// shared lesson's wording. Its bullet carries the lesson's marker, and
/// keeping refuses when a note already holds the marker of its lesson or
/// of one linked to it. On a shared lesson it also writes the person's
/// decision.
pub fn keep_lesson(
    cfg: &Config,
    ws: &Workspace,
    lesson: Lesson,
    title: Option<&str>,
    text: Option<&str>,
    now: OffsetDateTime,
) -> Result<Kept> {
    keep_lesson_by(cfg, ws, lesson, title, text, now, keeper())
}

/// [`keep_lesson`], kept by `by`.
fn keep_lesson_by(
    cfg: &Config,
    ws: &Workspace,
    mut lesson: Lesson,
    title: Option<&str>,
    text: Option<&str>,
    now: OffsetDateTime,
    by: Keeper,
) -> Result<Kept> {
    let id = lesson.id;
    let dir = team::dir_name(id);
    let shared = team::read(ws);
    let groups = team::groups(&shared);
    let linked: Vec<String> = match team::group_of(&shared, &groups, &dir) {
        Some(g) => groups[g].iter().map(|&i| shared[i].dir.clone()).collect(),
        None => vec![dir.clone()],
    };
    let linked_refs: Vec<&str> = linked.iter().map(String::as_str).collect();
    if let Some((note, _)) = team::markers_in_notes(ws, &linked_refs).into_iter().next() {
        return Err(Error::InvalidProposal(format!(
            "lesson {dir} is already kept: its marker is in {}",
            note.display()
        )));
    }
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
            "---\ntype: policy\nstatus: current\nupdated: {date}\n---\n\n# {title}\n\n{} {}\n",
            rule.trim_end(),
            team::marker(&dir)
        );
        (ProposalKind::Create, String::new(), new, Vec::new())
    } else {
        notes::validate_proposal_path(&ws.root, &ws.notes_dir, &target.note)?;
        if crate::dream::validate::is_guard_note(ws, &target.note) {
            return Err(Error::InvalidProposal(
                crate::dream::validate::GUARD_NOTE.to_owned(),
            ));
        }
        let bytes = fs::read(&note_path).map_err(|e| Error::io_at(&note_path, e))?;
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let bullet = bullet_marked(&title, &rule, date, &dir);
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
        runner: None,
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
        by: Some(by),
    });
    // A shared lesson: the person's decision travels with the repository.
    if team::inbox_dir(ws).join(&dir).is_dir() {
        if let Some(member) = cfg.member.as_deref() {
            team::decide(
                ws,
                member,
                &dir,
                &team::Decision {
                    verdict: team::Verdict::Keep,
                    by: member.to_owned(),
                    at: now,
                    receipt: Some(receipt.id),
                    edited,
                    reason: None,
                    sessions_at: 0,
                },
            )?;
            lesson.shared = Some(dir.clone());
        }
    }
    lesson.save(&cfg.home)?;
    let mut replaced = Vec::new();
    for old in lesson.links.supersedes.clone() {
        replaced.push(supersede(cfg, ws, old, id, now)?);
    }
    Ok(Kept {
        lesson,
        receipt,
        proposal,
        replaced,
    })
}

/// A kept rule as a session gets it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Rule {
    pub id: Ulid,
    pub text: String,
    /// A machine rule: every session gets it, inside the workspace or not.
    pub machine: bool,
    /// How many sessions its corrections span.
    pub sessions: usize,
    /// Its key terms, to match a prompt's.
    pub terms: Vec<String>,
}

/// The kept rules in force, best first (most sessions, then newest kept).
/// A replaced rule is left out, and so is a workspace rule whose bullet is
/// no longer in its note (reverted or removed by hand) or, with no
/// workspace, any workspace rule.
pub fn kept_rules(home: &Path, ws: Option<&Workspace>) -> Vec<Rule> {
    let mut kept: Vec<(Rule, OffsetDateTime)> = Vec::new();
    for l in list(home).unwrap_or_default() {
        if l.status != Status::Kept || l.links.superseded_by.is_some() || l.rule.trim().is_empty() {
            continue;
        }
        let machine = l.links.scope == Some(Scope::Machine) || l.target.is_none();
        if !machine {
            let (Some(ws), Some(target)) = (ws, &l.target) else {
                continue;
            };
            let text = fs::read_to_string(ws.root.join(&target.note)).unwrap_or_default();
            let marker = team::marker(&team::dir_name(l.id));
            let rule_line = l.rule.split_whitespace().collect::<Vec<_>>().join(" ");
            if !text.contains(&marker) && !text.contains(&rule_line) {
                continue;
            }
        }
        let at = l.kept.as_ref().map_or(l.updated, |k| k.at);
        kept.push((
            Rule {
                id: l.id,
                text: detect::one_line(&l.rule, 400),
                machine,
                sessions: l.seen.sessions,
                terms: if l.terms.is_empty() {
                    detect::key_terms(&l.rule)
                } else {
                    l.terms.clone()
                },
            },
            at,
        ));
    }
    kept.sort_by(|a, b| {
        b.0.sessions
            .cmp(&a.0.sessions)
            .then_with(|| b.1.cmp(&a.1))
            .then_with(|| a.0.id.cmp(&b.0.id))
    });
    kept.into_iter().map(|(r, _)| r).collect()
}

/// What keeping a rule did, wherever it went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeptRule {
    pub lesson: Lesson,
    /// The note it was written into; `None` for a machine rule, which
    /// lives in `$HIPPO_HOME` only.
    pub note: Option<PathBuf>,
    /// The receipt: a note change's, or the line in `lessons/kept.jsonl`.
    pub receipt: Ulid,
    /// What became of the rules it replaces, one line each.
    pub replaced: Vec<String>,
}

/// `$HIPPO_HOME/lessons/kept.jsonl`: one line per keep or undo of a rule
/// that lives in `$HIPPO_HOME` only.
pub fn kept_log(home: &Path) -> PathBuf {
    dir(home).join("kept.jsonl")
}

/// Keeps pending lesson `id` where it belongs: a machine rule (a memory
/// lesson, or one the drafter scoped `machine`) in `$HIPPO_HOME` only,
/// never in a note; any other into its target note, as [`keep`] does.
/// `into_note` writes a machine rule with a target note into that note
/// instead, as a workspace rule.
pub fn keep_rule(
    cfg: &Config,
    ws: Option<&Workspace>,
    id: Ulid,
    title: Option<&str>,
    text: Option<&str>,
    into_note: bool,
    now: OffsetDateTime,
) -> Result<KeptRule> {
    keep_rule_by(cfg, ws, id, title, text, into_note, now, keeper())
}

/// [`keep_rule`], kept by `by`.
#[allow(clippy::too_many_arguments)]
fn keep_rule_by(
    cfg: &Config,
    ws: Option<&Workspace>,
    id: Ulid,
    title: Option<&str>,
    text: Option<&str>,
    into_note: bool,
    now: OffsetDateTime,
    by: Keeper,
) -> Result<KeptRule> {
    let mut lesson = get(&cfg.home, id)?;
    let machine =
        lesson.target.is_none() || (lesson.links.scope == Some(Scope::Machine) && !into_note);
    if !machine {
        let ws = ws.ok_or(Error::NoWorkspace)?;
        if into_note {
            lesson.links.scope = Some(Scope::Workspace);
        }
        let kept = keep_lesson_by(cfg, ws, lesson, title, text, now, by)?;
        return Ok(KeptRule {
            note: Some(kept.proposal.note.clone()),
            receipt: kept.receipt.id,
            replaced: kept.replaced,
            lesson: kept.lesson,
        });
    }
    if lesson.status != Status::Pending {
        return Err(Error::InvalidProposal(format!(
            "lesson {id} is not pending"
        )));
    }
    let edited = title.is_some_and(|t| t.trim() != lesson.title)
        || text.is_some_and(|t| t.trim() != lesson.rule);
    if let Some(t) = title {
        lesson.title = t.trim().to_owned();
    }
    if let Some(t) = text {
        lesson.rule = t.trim().to_owned();
    }
    if lesson.title.is_empty() || lesson.rule.is_empty() {
        return Err(Error::InvalidProposal(
            "a rule needs a title and a text".to_owned(),
        ));
    }
    let receipt = Ulid::generate();
    lesson.status = Status::Kept;
    lesson.links.scope = Some(Scope::Machine);
    lesson.updated = now;
    lesson.kept = Some(KeptBy {
        receipt,
        at: now,
        edited,
        by: Some(by),
    });
    log_kept(&cfg.home, receipt, &lesson, "keep", now)?;
    lesson.save(&cfg.home)?;
    let mut replaced = Vec::new();
    for old in lesson.links.supersedes.clone() {
        replaced.push(match ws {
            Some(ws) => supersede(cfg, ws, old, id, now)?,
            None => supersede_record(cfg, old, id, now)?,
        });
    }
    Ok(KeptRule {
        lesson,
        note: None,
        receipt,
        replaced,
    })
}

/// Takes back kept rule `id`: a rule in a note through the revert of its
/// receipt (as `hippo revert` does), a machine rule by a line in
/// [`kept_log`]. Either way the lesson is dismissed. Returns one line.
pub fn undo(cfg: &Config, ws: Option<&Workspace>, id: Ulid, now: OffsetDateTime) -> Result<String> {
    let mut lesson = get(&cfg.home, id)?;
    let Some(kept) = lesson
        .kept
        .clone()
        .filter(|_| lesson.status == Status::Kept)
    else {
        return Err(Error::InvalidProposal(format!("lesson {id} is not kept")));
    };
    let in_note = lesson.target.is_some() && lesson.links.scope != Some(Scope::Machine);
    let line = if in_note {
        // The bullet goes out through its own proposal and receipt, so a
        // later change to the same note does not stand in the way.
        let ws = ws.ok_or(Error::NoWorkspace)?;
        let note = lesson
            .target
            .as_ref()
            .map(|t| t.note.clone())
            .unwrap_or_default();
        let text = fs::read_to_string(ws.root.join(&note)).unwrap_or_default();
        let marker = team::marker(&team::dir_name(id));
        let Some(bullet) = text
            .lines()
            .find(|l| l.contains(&marker))
            .map(|l| format!("{l}\n"))
        else {
            return Err(Error::InvalidProposal(format!(
                "lesson {id}'s bullet is no longer in {}",
                note.display()
            )));
        };
        let reason = format!("lesson {id} taken back by the person");
        let receipt = remove_line(
            ws,
            &note,
            &text,
            &bullet,
            &reason,
            &format!("hippo lessons undo {id}"),
            now,
        )?;
        format!(
            "{id}: taken back; its bullet is out of {} (receipt {receipt}; hippo revert {receipt} puts it back)",
            note.display()
        )
    } else {
        log_kept(&cfg.home, kept.receipt, &lesson, "undo", now)?;
        format!("{id}: taken back; no session gets it now")
    };
    lesson.status = Status::Dismissed;
    lesson.closed_sessions = Some(lesson.seen.sessions);
    lesson.updated = now;
    lesson.save(&cfg.home)?;
    Ok(line)
}

/// Rules offered per ask, at most.
pub const ASK_MAX: usize = 3;
/// A rule is offered in an ask at most once per this many hours.
pub const ASK_EVERY_HOURS: i64 = 24;

/// `$HIPPO_HOME/lessons-asked.json`: when each pending rule was last
/// offered in an ask, by id.
pub fn asked_path(home: &Path) -> PathBuf {
    home.join("lessons-asked.json")
}

fn load_asked(home: &Path) -> BTreeMap<String, String> {
    fs::read(asked_path(home))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn asked_at(asked: &BTreeMap<String, String>, id: Ulid) -> Option<OffsetDateTime> {
    asked
        .get(&id.to_string())
        .and_then(|t| OffsetDateTime::parse(t, &time::format_description::well_known::Rfc3339).ok())
}

/// Whether rule `id` was offered in an ask in the last
/// [`ASK_EVERY_HOURS`] hours: the only rules an agent may keep or dismiss
/// for the person.
pub fn was_asked(home: &Path, id: Ulid, now: OffsetDateTime) -> bool {
    asked_at(&load_asked(home), id)
        .is_some_and(|t| now - t <= time::Duration::hours(ASK_EVERY_HOURS))
}

/// The ask for a start block: up to [`ASK_MAX`] pending rules drafted from
/// the person's corrections and not offered in the last
/// [`ASK_EVERY_HOURS`] hours, with how to record each answer; `None` when
/// there is none, with the ids it offers ([`mark_asked`] records them once
/// shown). Memory lessons and rules that replace a team rule are never in
/// it.
pub fn ask(home: &Path, now: OffsetDateTime) -> Option<(String, Vec<Ulid>)> {
    let all = list(home).ok()?;
    let asked = load_asked(home);
    let shared = |id: &Ulid| all.iter().any(|l| l.id == *id && l.shared.is_some());
    let mut offer: Vec<&Lesson> = all
        .iter()
        .filter(|l| l.status == Status::Pending && l.links.source.is_none())
        .filter(|l| !l.rule.trim().is_empty())
        .filter(|l| !l.links.supersedes.iter().any(&shared))
        .filter(|l| {
            asked_at(&asked, l.id).is_none_or(|t| now - t > time::Duration::hours(ASK_EVERY_HOURS))
        })
        .collect();
    if offer.is_empty() {
        return None;
    }
    offer.sort_by(|a, b| {
        b.seen
            .sessions
            .cmp(&a.seen.sessions)
            .then_with(|| b.seen.last.cmp(&a.seen.last))
    });
    offer.truncate(ASK_MAX);
    let n = offer.len();
    let mut text = format!(
        "hippo drafted {n} rule{} from the person's corrections. At a natural pause, ask the person once, in one message, and do not ask again in this session:",
        if n == 1 { "" } else { "s" }
    );
    for (k, l) in offer.iter().enumerate() {
        let replaces = l
            .links
            .supersedes
            .iter()
            .filter_map(|id| all.iter().find(|o| o.id == *id))
            .map(|o| format!("; replaces: \"{}\"", o.title))
            .collect::<String>();
        text.push_str(&format!(
            "\n  {}. {}   (seen in {} sessions{replaces})",
            k + 1,
            detect::one_line(&l.rule, 400),
            l.seen.sessions
        ));
    }
    text.push_str("\n  Keep? y / n / edit\nRecord each answer: `hippo lessons keep <id>`, `hippo lessons dismiss <id>`, or `hippo lessons keep <id> --text \"…\"`.\nids:");
    for (k, l) in offer.iter().enumerate() {
        text.push_str(&format!(" {} = {}", k + 1, l.id));
        if k + 1 < n {
            text.push(',');
        }
    }
    Some((text, offer.iter().map(|l| l.id).collect()))
}

/// Records that rules `ids` were offered in an ask now.
pub fn mark_asked(home: &Path, ids: &[Ulid], now: OffsetDateTime) {
    let mut asked = load_asked(home);
    let stamp = now
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();
    for id in ids {
        asked.insert(id.to_string(), stamp.clone());
    }
    if let Ok(bytes) = serde_json::to_vec(&asked) {
        let _ = write_atomic(&asked_path(home), &bytes);
    }
}

/// The rules `auto_keep` keeps after a scan: pending, drafted from
/// corrections in 3 or more sessions, replacing nothing.
pub const AUTO_KEEP_SESSIONS: usize = 3;

/// Keeps every pending rule that qualifies for `auto_keep`, as
/// [`Keeper::Auto`]; one line for each, for the report.
pub fn auto_keep(cfg: &Config, ws: &Workspace, now: OffsetDateTime) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(all) = list(&cfg.home) else {
        return out;
    };
    for l in all {
        if l.status != Status::Pending
            || l.links.source.is_some()
            || !l.links.supersedes.is_empty()
            || l.seen.sessions < AUTO_KEEP_SESSIONS
        {
            continue;
        }
        match keep_rule_by(cfg, Some(ws), l.id, None, None, false, now, Keeper::Auto) {
            Ok(_) => out.push(format!(
                "auto_keep kept: {} (seen in {} sessions; hippo lessons undo {} takes it back)",
                l.title, l.seen.sessions, l.id
            )),
            Err(err) => out.push(format!("auto_keep could not keep {}: {err}", l.id)),
        }
    }
    out
}

/// Appends one line to [`kept_log`].
fn log_kept(
    home: &Path,
    receipt: Ulid,
    lesson: &Lesson,
    action: &str,
    now: OffsetDateTime,
) -> Result<()> {
    use std::io::Write as _;
    let path = kept_log(home);
    fs::create_dir_all(dir(home)).map_err(|e| Error::io_at(&dir(home), e))?;
    let line = serde_json::json!({
        "receipt": receipt.to_string(),
        "lesson": lesson.id.to_string(),
        "action": action,
        "at": now.format(&time::format_description::well_known::Rfc3339).unwrap_or_default(),
        "by": lesson.kept.as_ref().and_then(|k| k.by),
        "title": lesson.title,
        "rule": lesson.rule,
    });
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| Error::io_at(&path, e))?;
    writeln!(f, "{line}").map_err(|e| Error::io_at(&path, e))?;
    Ok(())
}

/// [`supersede`] with no workspace: the record only.
fn supersede_record(cfg: &Config, old: Ulid, by: Ulid, now: OffsetDateTime) -> Result<String> {
    let mut lesson = get(&cfg.home, old)?;
    let line = format!("replaced: {}", lesson.title);
    lesson.status = Status::Dismissed;
    lesson.links.superseded_by = Some(by);
    lesson.closed_sessions = Some(lesson.seen.sessions);
    lesson.updated = now;
    lesson.save(&cfg.home)?;
    Ok(line)
}

/// Closes lesson `old`, which kept lesson `by` replaces: status
/// `dismissed` with `superseded_by`, and, when it was kept into a note, its
/// bullet removed through a proposal and a confirm receipt whose reviewer
/// is `human`, so `hippo revert` restores it. A shared rule's bullet is
/// left for the team. Returns one line saying what happened.
pub fn supersede(
    cfg: &Config,
    ws: &Workspace,
    old: Ulid,
    by: Ulid,
    now: OffsetDateTime,
) -> Result<String> {
    let mut lesson = get(&cfg.home, old)?;
    let title = if lesson.title.is_empty() {
        old.to_string()
    } else {
        lesson.title.clone()
    };
    let mut line = format!("replaced: {title}");
    if lesson.status == Status::Kept {
        let dir = team::dir_name(old);
        let shared = lesson.shared.is_some() || team::inbox_dir(ws).join(&dir).is_dir();
        match lesson.target.clone() {
            _ if shared => {
                line = format!("{title}: a team rule; change the team note by hand");
            }
            Some(target) => {
                let note_path = ws.root.join(&target.note);
                let text = fs::read_to_string(&note_path).unwrap_or_default();
                let marker = team::marker(&dir);
                let bullet = text
                    .lines()
                    .find(|l| l.contains(&marker))
                    .map(|l| format!("{l}\n"));
                match bullet {
                    Some(bullet) => {
                        let reason =
                            format!("lesson {old} replaced by lesson {by}, kept by the person");
                        match remove_line(
                            ws,
                            &target.note,
                            &text,
                            &bullet,
                            &reason,
                            &format!("hippo lessons keep {by}"),
                            now,
                        ) {
                            Ok(receipt) => {
                                line = format!(
                                    "replaced: {title} (its bullet in {} removed, receipt {receipt})",
                                    target.note.display()
                                );
                            }
                            Err(err) => {
                                line = format!(
                                    "replaced: {title}; remove its bullet in {} by hand ({err})",
                                    target.note.display()
                                );
                            }
                        }
                    }
                    None => {
                        line = format!(
                            "replaced: {title}; its bullet is no longer in {}",
                            target.note.display()
                        );
                    }
                }
            }
            None => {}
        }
    }
    lesson.status = Status::Dismissed;
    lesson.links.superseded_by = Some(by);
    lesson.closed_sessions = Some(lesson.seen.sessions);
    lesson.updated = now;
    lesson.save(&cfg.home)?;
    Ok(line)
}

/// Removes `bullet` (one whole line) from `note` through a proposal and a
/// human confirm receipt.
fn remove_line(
    ws: &Workspace,
    note: &Path,
    text: &str,
    bullet: &str,
    reason: &str,
    invocation: &str,
    now: OffsetDateTime,
) -> Result<Ulid> {
    notes::validate_proposal_path(&ws.root, &ws.notes_dir, note)?;
    let before = text.as_bytes();
    let after = notes::change(before, ProposalKind::Replace, bullet.as_bytes(), b"")?;
    let proposal = records::Proposal {
        id: Ulid::generate(),
        pass: "lessons".to_owned(),
        note: note.to_path_buf(),
        kind: ProposalKind::Replace,
        before_hash: hex(&sha256(before)),
        after_hash: hex(&sha256(&after)),
        old: bullet.to_owned(),
        new: String::new(),
        evidence: Vec::new(),
        reason: reason.to_owned(),
    };
    validate::write(ws, &proposal)?;
    let review = Review {
        verdict: Verdict::Confirm,
        notes: reason.to_owned(),
        reviewer: "human".to_owned(),
        invocation: invocation.to_owned(),
        at: now,
        runner: None,
    };
    match apply::confirm(ws, &proposal, &review)? {
        Applied::Written(r) => Ok(r.id),
        Applied::NoteChanged(_) => Err(Error::InvalidProposal(format!(
            "{} changed meanwhile",
            note.display()
        ))),
    }
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

/// [`bullet`] with the lesson's marker at the end, which Obsidian and
/// GitHub hide: `<!-- hippo:lesson L-… -->`.
pub fn bullet_marked(title: &str, rule: &str, date: time::Date, dir: &str) -> String {
    let line = bullet(title, rule, date);
    format!("{} {}\n", line.trim_end(), team::marker(dir))
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

/// Who is keeping a rule now: an agent when the environment carries an
/// agent's marker (`accept::AGENT_MARKERS`), else the person.
pub fn keeper() -> Keeper {
    if crate::accept::agent_marker(|k| std::env::var(k).ok()).is_some() {
        Keeper::Agent
    } else {
        Keeper::Person
    }
}

/// How [`scan_with`] runs.
#[derive(Debug, Clone)]
pub struct ScanOptions {
    /// Days of failures to read (1 to 30); corrections are read for
    /// [`CORRECTION_DAYS`], or all of them in a backfill.
    pub days: u32,
    /// Draft and review, but write no record, group or state.
    pub dry_run: bool,
    /// Place every unplaced correction in the index.
    pub backfill: bool,
    /// Match calls the run may make.
    pub max_calls: usize,
    /// Where agent memories are looked for; only sources the person turned
    /// on are read.
    pub places: memory::Places,
}

/// The days of corrections a scan places, when it is not a backfill.
pub const CORRECTION_DAYS: i64 = 30;
/// The most drafts a group gets when the drafter or the reviewer keeps
/// closing it.
pub const MAX_DRAFTS: u32 = 3;
/// A dismissed rule comes back only after this many new sessions…
pub const DISMISSED_SESSIONS: usize = 2;
/// …and this many days.
pub const DISMISSED_DAYS: i64 = 14;

/// Runs the lessons pass: the person's corrections of the last
/// [`CORRECTION_DAYS`] days are placed into groups, failures of the last
/// `days` days are clustered, and groups and failures that call for a rule
/// are drafted and reviewed. With `dry_run` it drafts and reviews but
/// writes no record, group or state.
pub fn scan(cfg: &Config, ws: &Workspace, days: u32, dry_run: bool) -> Result<Scanned> {
    scan_with(
        cfg,
        ws,
        &ScanOptions {
            days,
            dry_run,
            backfill: false,
            max_calls: groups::MATCH_CALLS_PER_RUN,
            places: memory::Places::here(),
        },
    )
}

/// Whether group `g`, whose lesson is `l`, is drafted (again) now: a group
/// with corrections in two sessions and no lesson; a declined or rejected
/// one that recurred in a new session, up to [`MAX_DRAFTS`] drafts; a
/// dismissed one after [`DISMISSED_SESSIONS`] new sessions and
/// [`DISMISSED_DAYS`] days. A replaced rule never comes back.
pub fn wants_draft(g: &groups::Group, l: Option<&Lesson>, now: OffsetDateTime) -> bool {
    if g.sessions().len() < 2 {
        return false;
    }
    let Some(l) = l else { return true };
    let since = |t: OffsetDateTime| {
        let before: BTreeSet<&str> = g
            .members
            .iter()
            .filter(|m| m.at <= t)
            .filter_map(|m| m.session.as_deref())
            .collect();
        g.members
            .iter()
            .filter(|m| m.at > t)
            .filter_map(|m| m.session.as_deref())
            .filter(|s| !before.contains(s))
            .collect::<BTreeSet<_>>()
            .len()
    };
    match l.status {
        Status::Pending | Status::Kept => false,
        Status::Declined | Status::Rejected => {
            l.links.drafts.unwrap_or(1) < MAX_DRAFTS && since(l.updated) >= 1
        }
        Status::Dismissed => {
            l.links.superseded_by.is_none()
                && since(l.updated) >= DISMISSED_SESSIONS
                && now - l.updated >= time::Duration::days(DISMISSED_DAYS)
        }
    }
}

/// [`scan`] with every option: `hippo lessons backfill` runs it with
/// `backfill` and its own call cap.
pub fn scan_with(cfg: &Config, ws: &Workspace, opts: &ScanOptions) -> Result<Scanned> {
    let now = OffsetDateTime::now_utc();
    let offset = crate::usage::local_offset_at(now);
    let index = Index::open(&cfg.home, false)?;
    let rollup = Rollup::open(&cfg.home)?.ok_or(Error::MissingIndex)?;
    let days = opts.days;
    let dry_run = opts.dry_run;
    let since = now - time::Duration::days(i64::from(days.clamp(1, 30)));
    let found = detect::find(&index, &rollup, since, offset)?;
    let notes = Notes::scan(&ws.notes_root())?;
    let mut records = list(&cfg.home)?;
    let mut out = Scanned {
        state: State {
            at: Some(now),
            days,
            sessions: found.sessions,
            ..State::default()
        },
        ..Scanned::default()
    };

    // The person's corrections, placed into groups by meaning.
    let corrections_since = if opts.backfill {
        OffsetDateTime::UNIX_EPOCH
    } else {
        now - time::Duration::days(CORRECTION_DAYS)
    };
    let (_, items) = detect::corrections(&index, &rollup, corrections_since)?;
    let mut store = groups::Store::load(&cfg.home)?;
    let titles = |g: &groups::Group| {
        g.lesson
            .and_then(|id| records.iter().find(|l| l.id == id))
            .filter(|l| !l.title.is_empty())
            .map(|l| l.title.clone())
    };
    let placed = groups::place(
        &mut store,
        items,
        groups::Calls::Model(cfg, opts.max_calls),
        &titles,
        now,
    );
    // Rules from the agent memories the person turned on.
    let mut lines = Vec::new();
    for source in memory::discover(&opts.places) {
        if memory::state(cfg, &source.id) == memory::State::On {
            for line in memory::lines(&source) {
                lines.push(groups::Member::from_memory(&source.id, &line, now));
            }
        }
    }
    let from_memory = groups::place_members(
        &mut store,
        lines,
        groups::Calls::Model(cfg, opts.max_calls.saturating_sub(placed.calls)),
        &titles,
        now,
    );
    for p in [&placed, &from_memory] {
        out.state.model_calls += p.calls;
        out.state.match_calls += p.calls;
        out.state.placed += p.local + p.new + p.model + p.unmatched;
        out.state.left += p.left;
        out.notes.extend(p.notes.iter().cloned());
    }
    if opts.backfill && placed.left == 0 {
        store.backfilled = Some(now);
    }
    // Lessons drafted before groups existed find their group by key and
    // terms, once.
    for g in store.groups.iter_mut().filter(|g| g.lesson.is_none()) {
        let Some(cluster) = g.cluster(offset) else {
            continue;
        };
        if let Some(l) = records
            .iter_mut()
            .find(|l| l.kind == Kind::Correction && l.links.group.is_none() && matches(l, &cluster))
        {
            g.lesson = Some(l.id);
            l.links.group = Some(g.id);
            if !dry_run {
                l.save(&cfg.home)?;
            }
        }
    }
    // A rule found only in agent memories becomes a memory lesson once a
    // reviewer in a fresh context confirms the line: no drafting call, the
    // line is the rule.
    let mut made: BTreeMap<String, usize> = BTreeMap::new();
    let wanted: Vec<(Ulid, String, String)> = store
        .groups
        .iter()
        .filter(|g| g.lesson.is_none() && g.has_memory() && g.sessions().len() < 2)
        .filter_map(|g| {
            let m = g
                .members
                .iter()
                .rev()
                .find(|m| m.source == groups::Source::Memory)?;
            Some((g.id, m.memory.as_ref()?.source.clone(), m.quote.clone()))
        })
        .collect();
    for (gid, source, text) in wanted {
        let n = made.entry(source.clone()).or_default();
        if *n >= memory::PER_SOURCE {
            continue;
        }
        *n += 1;
        out.state.model_calls += 1;
        let review = match memory::review(cfg, &source, &text) {
            Ok(r) => r,
            Err(err) => {
                out.notes.push(format!("memory review failed: {err}"));
                break;
            }
        };
        let lesson = memory_record(gid, &source, &text, review, now);
        if lesson.status == Status::Pending {
            out.state.confirmed += 1;
            out.confirmed.push(lesson.clone());
        }
        if !dry_run {
            lesson.save(&cfg.home)?;
        }
        if let Some(g) = store.get_mut(gid) {
            g.lesson = Some(lesson.id);
        }
        records.push(lesson);
    }
    let mut clusters: Vec<(Option<Ulid>, detect::Cluster)> = Vec::new();
    for g in &store.groups {
        let Some(mut c) = g.cluster(offset) else {
            continue;
        };
        c.reverses = reversed(&store, &records, g).map(|(_, rule)| rule);
        clusters.push((Some(g.id), c));
    }
    out.state.candidates = clusters.len() + found.failures.len();
    clusters.extend(found.failures.into_iter().map(|c| (None, c)));

    // The shared Inbox, when it is in force: a correction the team already
    // has is counted, not drafted.
    let team_on = team::enabled(ws);
    let shared = if team_on { team::read(ws) } else { Vec::new() };
    let team_groups = team::groups(&shared);
    let member = cfg
        .member
        .clone()
        .filter(|m| crate::settings::valid_handle(m));
    let writes_shared =
        team_on && !dry_run && member.is_some() && team::Share::of(cfg) == team::Share::Rules;

    // Clusters that match a record update it; the rest are drafted.
    let mut to_draft: Vec<(Option<Ulid>, detect::Cluster)> = Vec::new();
    for (gid, cluster) in clusters {
        let team_has = team_on && cluster.kind == Kind::Correction && {
            match team::find_group(&shared, &team_groups, &cluster.terms) {
                None => false,
                Some(tg) => {
                    let group = &team_groups[tg];
                    let held = match team::state(&shared, group) {
                        team::State::Kept | team::State::Crossed => true,
                        team::State::Open => true,
                        team::State::Dismissed => {
                            let at = team::decisions(&shared, group)
                                .iter()
                                .map(|d| d.record.sessions_at)
                                .max()
                                .unwrap_or(0);
                            team::sessions(&shared, group) < at + REOPEN_AFTER
                        }
                    };
                    if held {
                        out.state.shared_matched += 1;
                        if writes_shared {
                            let mine = records.iter().find(|l| record_of(l, gid, &cluster));
                            // Counts go to the person's own lesson's
                            // directory when they shared one, else to the
                            // group's first.
                            let dir = mine
                                .and_then(|l| l.shared.clone())
                                .filter(|d| group.iter().any(|&i| shared[i].dir == *d))
                                .unwrap_or_else(|| shared[group[0]].dir.clone());
                            team::write_seen(
                                ws,
                                member.as_deref().unwrap_or_default(),
                                &dir,
                                &team::Counts {
                                    sessions: cluster.sessions.len(),
                                    first: cluster.first,
                                    last: cluster.last,
                                    most_in_a_day: cluster.most_in_a_day,
                                    agents: cluster.profiles.iter().cloned().collect(),
                                },
                            )?;
                        }
                    }
                    held
                }
            }
        };
        let group = gid.and_then(|id| store.get(id));
        match records.iter_mut().find(|l| record_of(l, gid, &cluster)) {
            Some(l) => {
                let grew = cluster.sessions.len() > l.seen.sessions;
                merge(l, &cluster, now);
                let again = match group {
                    Some(g) => wants_draft(g, Some(l), now),
                    None => {
                        l.status.closed()
                            && l.seen.sessions >= l.closed_sessions.unwrap_or(0) + REOPEN_AFTER
                    }
                };
                if again && !team_has {
                    to_draft.push((gid, cluster));
                } else if grew && !dry_run {
                    l.save(&cfg.home)?;
                }
            }
            None if team_has => {}
            None => {
                let draft = match group {
                    Some(g) => wants_draft(g, None, now),
                    None => true,
                };
                if draft {
                    to_draft.push((gid, cluster));
                }
            }
        }
    }
    to_draft.sort_by(|a, b| {
        b.1.sessions
            .len()
            .cmp(&a.1.sessions.len())
            .then_with(|| b.1.last.cmp(&a.1.last))
    });
    let mut chosen: Vec<(Option<Ulid>, detect::Cluster)> = Vec::new();
    for kind in [Kind::Correction, Kind::Failure] {
        chosen.extend(
            to_draft
                .iter()
                .filter(|c| c.1.kind == kind)
                .take(draft::PER_RUN)
                .cloned(),
        );
    }
    let only: Vec<detect::Cluster> = chosen.iter().map(|(_, c)| c.clone()).collect();
    let drafted = draft::draft_clusters(cfg, ws, &notes, &only, &mut out)?;
    for ((gid, cluster), outcome) in chosen.iter().zip(drafted) {
        let Some(outcome) = outcome else { continue };
        let existing = records.iter().position(|l| record_of(l, *gid, cluster));
        let mut lesson = match existing {
            Some(i) => records[i].clone(),
            None => new_record(cluster, now),
        };
        merge(&mut lesson, cluster, now);
        if let Some(gid) = gid {
            lesson.links.group = Some(*gid);
            lesson.links.drafts = Some(lesson.links.drafts.map_or(1, |d| d + 1));
            lesson.links.supersedes = store
                .get(*gid)
                .and_then(|g| reversed(&store, &records, g))
                .and_then(|(id, _)| id)
                .filter(|id| {
                    records
                        .iter()
                        .any(|l| l.id == *id && matches!(l.status, Status::Pending | Status::Kept))
                })
                .into_iter()
                .collect();
        }
        match outcome {
            draft::Outcome::Declined => {
                lesson.status = Status::Declined;
                lesson.closed_sessions = Some(lesson.seen.sessions);
            }
            draft::Outcome::Reviewed {
                title,
                rule,
                target,
                scope,
                review,
            } => {
                lesson.title = title;
                lesson.rule = rule;
                lesson.target = Some(target);
                lesson.links.scope = scope;
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
        if let Some(g) = gid.and_then(|id| store.get_mut(id)) {
            g.lesson = Some(lesson.id);
        }
        match existing {
            Some(i) => records[i] = lesson,
            None => records.push(lesson),
        }
    }
    if team_on && !dry_run {
        share_after_scan(cfg, ws, &index, &mut records, member.as_deref(), &mut out)?;
    }
    if !dry_run {
        store.save(&cfg.home)?;
        if cfg.remember.auto_keep {
            let kept = auto_keep(cfg, ws, now);
            out.notes.extend(kept);
        }
        out.state.save(&cfg.home)?;
    }
    Ok(out)
}

/// A memory lesson for group `gid`: the line is the rule, machine-wide,
/// with no target note; pending when the reviewer confirmed it.
fn memory_record(
    gid: Ulid,
    source: &str,
    text: &str,
    review: LessonReview,
    now: OffsetDateTime,
) -> Lesson {
    let confirmed = review.verdict == Verdict::Confirm;
    Lesson {
        id: Ulid::generate(),
        kind: Kind::Correction,
        key: format!("memory:{source}"),
        terms: detect::key_terms(text),
        title: detect::one_line(text, 80),
        rule: text.to_owned(),
        target: None,
        evidence: Vec::new(),
        seen: Seen {
            sessions: 0,
            profiles: Vec::new(),
            first: now,
            last: now,
            most_in_a_day: 0,
        },
        status: if confirmed {
            Status::Pending
        } else {
            Status::Rejected
        },
        review: Some(review),
        created: now,
        updated: now,
        closed_sessions: None,
        kept: None,
        ask: None,
        shared: None,
        links: Links {
            group: Some(gid),
            scope: Some(Scope::Machine),
            source: Some(source.to_owned()),
            drafts: Some(1),
            ..Links::default()
        },
    }
}

/// The rule group `g` reverses: its lesson's id and rule, or, with no
/// lesson yet, the reversed group's newest quote.
fn reversed(
    store: &groups::Store,
    records: &[Lesson],
    g: &groups::Group,
) -> Option<(Option<Ulid>, String)> {
    let old = store.get(g.reverses?)?;
    let lesson = old
        .lesson
        .and_then(|id| records.iter().find(|l| l.id == id))
        .filter(|l| !l.rule.is_empty());
    Some(match lesson {
        Some(l) => (Some(l.id), l.rule.clone()),
        None => (None, old.newest_quote()?.to_owned()),
    })
}

/// What `hippo lessons backfill` would do: the corrections in the whole
/// index not placed yet, how many the pre-filter places, and the model
/// calls the rest and the drafts after them would need. No model call, no
/// write.
pub fn backfill_estimate(cfg: &Config) -> Result<groups::Estimate> {
    let now = OffsetDateTime::now_utc();
    let index = Index::open(&cfg.home, false)?;
    let rollup = Rollup::open(&cfg.home)?.ok_or(Error::MissingIndex)?;
    let (_, items) = detect::corrections(&index, &rollup, OffsetDateTime::UNIX_EPOCH)?;
    let store = groups::Store::load(&cfg.home)?;
    let records = list(&cfg.home)?;
    let draftable = |st: &groups::Store| {
        st.groups
            .iter()
            .filter(|g| {
                let l = g.lesson.and_then(|id| records.iter().find(|l| l.id == id));
                wants_draft(g, l, now)
            })
            .count()
    };
    Ok(groups::estimate(&store, items, &draftable, now))
}

/// Whether record `l` is the lesson of the cluster from group `gid` (by
/// its link), or of a failure cluster (by key).
fn record_of(l: &Lesson, gid: Option<Ulid>, cluster: &detect::Cluster) -> bool {
    match gid {
        Some(g) => l.links.group == Some(g),
        None => matches(l, cluster),
    }
}

/// After a scan with the shared Inbox in force: each confirmed correction
/// whose evidence all ran in the workspace is shared, when the person said
/// `share_lessons = "rules"`, or named as shareable when they have not;
/// lessons already shared get fresh counts.
fn share_after_scan(
    cfg: &Config,
    ws: &Workspace,
    index: &Index,
    records: &mut [Lesson],
    member: Option<&str>,
    out: &mut Scanned,
) -> Result<()> {
    let writes = member.is_some() && team::Share::of(cfg) == team::Share::Rules;
    let origins = crate::session_meta::origins(index)?;
    let roots = team::workspace_roots(ws);
    let store = groups::Store::load(&cfg.home)?;
    let confirmed: Vec<Ulid> = out.confirmed.iter().map(|l| l.id).collect();
    for lesson in records.iter_mut() {
        if let (Some(dir), true, Some(m)) = (&lesson.shared, writes, member) {
            team::write_seen(ws, m, dir, &team::counts_of(lesson))?;
            continue;
        }
        if lesson.kind != Kind::Correction || !confirmed.contains(&lesson.id) {
            continue;
        }
        if !may_share(lesson, &store, &origins, &roots) {
            continue;
        }
        match (writes, member) {
            (true, Some(m)) => {
                lesson.shared = Some(team::share(ws, m, lesson)?);
                lesson.save(&cfg.home)?;
            }
            _ => out.notes.push(format!(
                "lesson {} can be shared with the team; hippo lessons share shows the files it would write",
                lesson.id
            )),
        }
    }
    Ok(())
}

/// The local lessons that can be shared: corrections, waiting or kept,
/// not shared yet, whose every evidence session ran in the workspace or
/// one of its worktrees.
pub fn shareable(cfg: &Config, ws: &Workspace) -> Result<Vec<Lesson>> {
    let index = Index::open(&cfg.home, false)?;
    let origins = crate::session_meta::origins(&index)?;
    let roots = team::workspace_roots(ws);
    let store = groups::Store::load(&cfg.home)?;
    Ok(list(&cfg.home)?
        .into_iter()
        .filter(|l| {
            l.kind == Kind::Correction
                && matches!(l.status, Status::Pending | Status::Kept)
                && l.shared.is_none()
                && may_share(l, &store, &origins, &roots)
        })
        .collect())
}

/// Whether a correction lesson may go to the shared Inbox: not a machine
/// rule, no memory line in its group, and every correction of its group
/// (not only the evidence the lesson keeps) run in the workspace or one of
/// its worktrees.
fn may_share(
    lesson: &Lesson,
    store: &groups::Store,
    origins: &std::collections::HashMap<String, crate::session_meta::Origin>,
    roots: &[PathBuf],
) -> bool {
    if lesson.links.scope == Some(Scope::Machine) || lesson.links.source.is_some() {
        return false;
    }
    if !team::from_workspace(lesson, origins, roots) {
        return false;
    }
    let Some(group) = lesson.links.group.and_then(|g| store.get(g)) else {
        return true;
    };
    !group.has_memory()
        && group.members.iter().all(|m| {
            m.session.as_ref().is_some_and(|s| {
                origins
                    .get(s)
                    .and_then(|o| o.cwd.as_ref())
                    .is_some_and(|cwd| {
                        let cwd = fs::canonicalize(cwd).unwrap_or_else(|_| cwd.clone());
                        roots.iter().any(|r| cwd.starts_with(r))
                    })
            })
        })
}

/// Shares `lessons` as `member` (`hippo lessons share`, after the person
/// said yes): each gets its directory, wording and counts; a kept one also
/// the person's keep decision. Returns the directories.
pub fn share_lessons(
    cfg: &Config,
    ws: &Workspace,
    member: &str,
    lessons: &[Lesson],
    now: OffsetDateTime,
) -> Result<Vec<String>> {
    let mut dirs = Vec::new();
    for lesson in lessons {
        let mut lesson = lesson.clone();
        let dir = team::share(ws, member, &lesson)?;
        if let (Status::Kept, Some(kept)) = (lesson.status, &lesson.kept) {
            team::decide(
                ws,
                member,
                &dir,
                &team::Decision {
                    verdict: team::Verdict::Keep,
                    by: member.to_owned(),
                    at: now,
                    receipt: Some(kept.receipt),
                    edited: kept.edited,
                    reason: None,
                    sessions_at: 0,
                },
            )?;
        }
        lesson.shared = Some(dir.clone());
        lesson.save(&cfg.home)?;
        dirs.push(dir);
    }
    Ok(dirs)
}

/// The person's consent, given: writes `member` and
/// `share_lessons = "rules"` to `config.toml`, then shares `lessons`
/// ([`share_lessons`]). `hippo lessons share` calls it after its terminal
/// question, Hippo.app after its Share dialog; both show
/// [`team::consent`] first. Returns the directories written.
pub fn share_with_consent(
    cfg: &mut Config,
    ws: &Workspace,
    member: &str,
    lessons: &[Lesson],
    now: OffsetDateTime,
) -> Result<Vec<String>> {
    if !crate::settings::valid_handle(member) {
        return Err(Error::InvalidProposal(format!(
            "{member:?} is not a handle: letters, digits, - and _, at most 32"
        )));
    }
    for (key, value) in [("member", member), ("share_lessons", "rules")] {
        let plan =
            crate::settings::plan(&cfg.home, key, &crate::settings::Op::Set(value.to_owned()))?;
        crate::settings::apply(&plan)?;
    }
    cfg.member = Some(member.to_owned());
    cfg.share_lessons = Some("rules".into());
    share_lessons(cfg, ws, member, lessons, now)
}

/// The local record for shared lesson `dir`, or one made from the
/// wording a person sees first, waiting to be kept.
fn record_for_shared(cfg: &Config, ws: &Workspace, dir: &str) -> Result<Lesson> {
    let id: Ulid = dir
        .strip_prefix("L-")
        .and_then(|i| i.parse().ok())
        .ok_or_else(|| Error::InvalidProposal(format!("not a shared lesson: {dir}")))?;
    if let Ok(local) = get(&cfg.home, id) {
        return Ok(local);
    }
    let shared = team::read(ws);
    let lesson = shared.iter().find(|l| l.dir == dir).ok_or_else(|| {
        Error::InvalidProposal(format!(
            "no shared lesson {dir} in {}",
            team::inbox_dir(ws).display()
        ))
    })?;
    let wording = lesson
        .wordings
        .first()
        .ok_or_else(|| Error::InvalidProposal(format!("shared lesson {dir} has no wording")))?;
    let now = OffsetDateTime::now_utc();
    let first = lesson
        .seen
        .iter()
        .map(|s| s.record.first)
        .min()
        .unwrap_or(now);
    let last = lesson
        .seen
        .iter()
        .map(|s| s.record.last)
        .max()
        .unwrap_or(now);
    let w = &wording.record;
    Ok(Lesson {
        id,
        kind: w.kind,
        key: w.terms.join(" "),
        terms: w.terms.clone(),
        title: w.title.clone(),
        rule: w.rule.clone(),
        target: w.target.clone(),
        evidence: Vec::new(),
        seen: Seen {
            sessions: lesson.seen.iter().map(|s| s.record.sessions).sum(),
            profiles: Vec::new(),
            first,
            last,
            most_in_a_day: 0,
        },
        status: Status::Pending,
        review: None,
        created: now,
        updated: now,
        closed_sessions: None,
        kept: None,
        ask: None,
        shared: Some(dir.to_owned()),
        links: Links::default(),
    })
}

/// Keeps shared lesson `dir` (`hippo lessons keep L-…`): its note change as
/// any keep, and the person's decision in the shared Inbox.
pub fn keep_shared(
    cfg: &Config,
    ws: &Workspace,
    dir: &str,
    title: Option<&str>,
    text: Option<&str>,
    now: OffsetDateTime,
) -> Result<Kept> {
    if cfg.member.is_none() {
        return Err(Error::InvalidProposal(
            "a shared lesson's decision is written under your handle: set it first with hippo config set member <handle> --yes".into(),
        ));
    }
    let lesson = record_for_shared(cfg, ws, dir)?;
    keep_lesson(cfg, ws, lesson, title, text, now)
}

/// Dismisses shared lesson `dir` as the person: `reason` is `wrong`,
/// `not-for-us` or `duplicate` (then `of` names the lesson it repeats,
/// and the two are linked). A dismissal never edits a note.
pub fn dismiss_shared(
    cfg: &Config,
    ws: &Workspace,
    dir: &str,
    reason: Option<&str>,
    of: Option<&str>,
    now: OffsetDateTime,
) -> Result<()> {
    let member = cfg.member.as_deref().ok_or_else(|| {
        Error::InvalidProposal(
            "a shared lesson's decision is written under your handle: set it first with hippo config set member <handle> --yes".into(),
        )
    })?;
    if let Some(r) = reason {
        if !matches!(r, "wrong" | "duplicate" | "not-for-us") {
            return Err(Error::InvalidProposal(format!(
                "--reason is wrong, duplicate or not-for-us, found {r:?}"
            )));
        }
    }
    let shared = team::read(ws);
    let groups = team::groups(&shared);
    let g = team::group_of(&shared, &groups, dir)
        .ok_or_else(|| Error::InvalidProposal(format!("no shared lesson {dir}")))?;
    if reason == Some("duplicate") {
        let other = of.ok_or_else(|| {
            Error::InvalidProposal("a duplicate names the lesson it repeats: --of L-…".into())
        })?;
        if team::group_of(&shared, &groups, other).is_none() {
            return Err(Error::InvalidProposal(format!("no shared lesson {other}")));
        }
        team::link(ws, member, dir, other, now)?;
    }
    team::decide(
        ws,
        member,
        dir,
        &team::Decision {
            verdict: team::Verdict::Dismiss,
            by: member.to_owned(),
            at: now,
            receipt: None,
            edited: false,
            reason: reason.map(str::to_owned),
            sessions_at: team::sessions(&shared, &groups[g]),
        },
    )?;
    if let Some(id) = dir.strip_prefix("L-").and_then(|i| i.parse::<Ulid>().ok()) {
        if let Ok(mut local) = get(&cfg.home, id) {
            if local.status == Status::Pending {
                local.status = Status::Dismissed;
                local.closed_sessions = Some(local.seen.sessions);
                local.updated = now;
                local.save(&cfg.home)?;
            }
        }
    }
    Ok(())
}

/// One marker `hippo lessons markers` adds: a kept bullet, before and
/// after.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkerPlan {
    pub lesson: Ulid,
    pub note: PathBuf,
    pub old: String,
    pub new: String,
}

/// Finds each kept local lesson's bullet in its note (by the line keep
/// inserted: its bold title and `*(lesson kept`) that has no marker yet.
pub fn plan_markers(cfg: &Config, ws: &Workspace) -> Result<Vec<MarkerPlan>> {
    let mut out = Vec::new();
    for lesson in list(&cfg.home)? {
        if lesson.status != Status::Kept {
            continue;
        }
        let Some(target) = &lesson.target else {
            continue;
        };
        let Ok(text) = fs::read_to_string(ws.root.join(&target.note)) else {
            continue;
        };
        let title = lesson.title.trim().trim_end_matches(['.', ':', ';']);
        let start = format!("- **{title}.**");
        let dir = team::dir_name(lesson.id);
        if text.contains(&team::marker(&dir)) {
            continue;
        }
        let found: Vec<&str> = text
            .lines()
            .filter(|l| {
                l.starts_with(&start) && l.contains("*(lesson kept") && !l.contains("hippo:lesson")
            })
            .collect();
        if let [line] = found.as_slice() {
            let old = format!("{line}\n");
            if notes::occurrences(text.as_bytes(), old.as_bytes()).0 != 1 {
                continue;
            }
            out.push(MarkerPlan {
                lesson: lesson.id,
                note: target.note.clone(),
                new: format!("{} {}\n", line.trim_end(), team::marker(&dir)),
                old,
            });
        }
    }
    Ok(out)
}

/// Writes `plans` as human-reviewed changes with snapshots and receipts,
/// so `hippo revert` removes a marker. Returns the receipts' ids.
pub fn write_markers(
    ws: &Workspace,
    plans: &[MarkerPlan],
    now: OffsetDateTime,
) -> Result<Vec<Ulid>> {
    let mut receipts = Vec::new();
    for plan in plans {
        let path = ws.root.join(&plan.note);
        let before = fs::read(&path).map_err(|e| Error::io_at(&path, e))?;
        let after = notes::change(
            &before,
            ProposalKind::Replace,
            plan.old.as_bytes(),
            plan.new.as_bytes(),
        )?;
        let proposal = records::Proposal {
            id: Ulid::generate(),
            pass: "lessons".to_owned(),
            note: plan.note.clone(),
            kind: ProposalKind::Replace,
            before_hash: hex(&sha256(&before)),
            after_hash: hex(&sha256(&after)),
            old: plan.old.clone(),
            new: plan.new.clone(),
            evidence: Vec::new(),
            reason: format!("marker for kept lesson {}", plan.lesson),
        };
        validate::write(ws, &proposal)?;
        let review = Review {
            verdict: Verdict::Confirm,
            notes: "the lesson's marker on its kept bullet".to_owned(),
            reviewer: "human".to_owned(),
            invocation: "hippo lessons markers --yes".to_owned(),
            at: now,
            runner: None,
        };
        match apply::confirm(ws, &proposal, &review)? {
            Applied::Written(r) => receipts.push(r.id),
            Applied::NoteChanged(_) => {
                return Err(Error::InvalidProposal(format!(
                    "{} changed meanwhile; run hippo lessons markers again",
                    plan.note.display()
                )))
            }
        }
    }
    Ok(receipts)
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
        shared: None,
        links: Links::default(),
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
