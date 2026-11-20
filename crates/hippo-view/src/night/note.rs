//! Brain's panel: one note's summary, rules, pending rules with their
//! evidence, readers by agent, links and receipts.

use std::path::{Path, PathBuf};

use hippo_core::config::Workspace;
use hippo_core::dream::{History, RevertState};
use hippo_core::lessons::{Lesson, Status};
use hippo_core::notes;
use hippo_core::records::Verdict;
use hippo_core::rollup::Rollup;
use serde::Serialize;
use time::{OffsetDateTime, UtcOffset};
use ulid::Ulid;

use super::map::{MapNote, NotesMap};
use super::morning::{RuleCard, Said};
use super::{agent, Agent};

/// The panel.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NotePanel {
    pub path: PathBuf,
    /// Empty for the notes directory's top level.
    pub folder: String,
    pub title: String,
    /// Up to three sentences.
    pub summary: String,
    pub chip: Option<(String, &'static str)>,
    pub reads_week: u32,
    pub rules_kept: usize,
    /// Changes with a receipt (dreaming and kept rules).
    pub changes: usize,
    pub rules: Vec<KeptRule>,
    pub pending: Vec<RuleCard>,
    pub read_by: Vec<(Agent, u32)>,
    pub links_out: Vec<(PathBuf, String)>,
    pub links_in: Vec<(PathBuf, String)>,
    pub receipts: Vec<ReceiptRow>,
}

/// A kept rule in the note.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KeptRule {
    pub rule: String,
    /// `kept 9 Nov`.
    pub kept: String,
}

/// One change to the note, with its undo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReceiptRow {
    pub receipt: Ulid,
    pub proposal: Ulid,
    /// `Port changed from 3000 to 4310`-like: the was/now pair as one line.
    pub what: String,
    /// `Sat 14 Nov, reviewer confirmed`.
    pub when: String,
    pub undo: Option<Vec<String>>,
    pub reverted: bool,
}

/// `9 Nov`.
fn day_month(t: OffsetDateTime, offset: UtcOffset) -> String {
    let d = t.to_offset(offset).date();
    let m = d.month().to_string();
    format!("{} {}", d.day(), &m[..3.min(m.len())])
}

/// What [`build`] reads.
pub struct Inputs<'a> {
    pub ws: &'a Workspace,
    pub map: &'a NotesMap,
    pub history: Option<&'a History>,
    pub lessons: &'a [Lesson],
    pub rollup: Option<&'a Rollup>,
    pub now: OffsetDateTime,
    pub offset: UtcOffset,
}

/// The panel for the note at `path` (relative to the workspace).
pub fn build(inp: &Inputs<'_>, path: &Path) -> Option<NotePanel> {
    let path = notes::normalise(path);
    let i = inp.map.find(&path)?;
    let n: &MapNote = &inp.map.notes[i];
    let summary = notes::Note::read(&inp.ws.root.join(&path))
        .ok()
        .map(|note| long_summary(&note))
        .unwrap_or_else(|| n.summary.clone());

    let mine = |l: &&Lesson| {
        l.target
            .as_ref()
            .is_some_and(|t| notes::normalise(&t.note) == path)
    };
    let mut kept: Vec<&Lesson> = inp
        .lessons
        .iter()
        .filter(|l| l.status == Status::Kept)
        .filter(mine)
        .collect();
    kept.sort_by_key(|l| std::cmp::Reverse(l.updated));
    let rules: Vec<KeptRule> = kept
        .iter()
        .map(|l| KeptRule {
            rule: l.rule.clone(),
            kept: format!(
                "kept {}",
                day_month(l.kept.as_ref().map_or(l.updated, |k| k.at), inp.offset)
            ),
        })
        .collect();
    let pending: Vec<RuleCard> = inp
        .lessons
        .iter()
        .filter(|l| l.status == Status::Pending)
        .filter(mine)
        .map(|l| pending_card(inp, l))
        .collect();

    let mut receipts = Vec::new();
    if let Some(h) = inp.history {
        for c in h.changes.iter().rev() {
            let Ok(p) = &c.proposal.record else { continue };
            if notes::normalise(&p.note) != path {
                continue;
            }
            let Some(confirm) = c
                .receipts
                .iter()
                .find(|r| r.verdict == Verdict::Confirm && r.applied.is_some())
            else {
                continue;
            };
            let reverted = c.receipts.iter().any(|r| r.verdict == Verdict::Revert);
            let (was, now) = super::morning::was_now(&p.old, &p.new);
            let what = match was {
                Some(w) => format!("“{w}” → {now}"),
                None => format!("Added “{now}”"),
            };
            let by = if confirm.reviewer == "human" {
                "you kept it"
            } else {
                "reviewer confirmed"
            };
            let at = confirm.applied.unwrap_or(confirm.at).to_offset(inp.offset);
            let undo = match hippo_core::dream::revert_state_in(inp.ws, h, confirm.id).ok() {
                Some(RevertState::Revertible) => {
                    Some(vec!["revert".into(), confirm.id.to_string()])
                }
                Some(RevertState::NoteChanged) => Some(vec![
                    "revert".into(),
                    "--force".into(),
                    confirm.id.to_string(),
                ]),
                _ => None,
            };
            receipts.push(ReceiptRow {
                receipt: confirm.id,
                proposal: c.proposal_id,
                what,
                when: format!(
                    "{} {}, {}{by}",
                    super::weekday(at.date()),
                    day_month(at, inp.offset),
                    if reverted { "undone; " } else { "" }
                ),
                undo: undo.filter(|_| !reverted),
                reverted,
            });
        }
    }

    let title_of = |j: usize| {
        (
            inp.map.notes[j].path.clone(),
            inp.map.notes[j].title.clone(),
        )
    };
    let links_out = inp
        .map
        .edges
        .iter()
        .filter(|e| e.0 == i)
        .map(|e| title_of(e.1))
        .collect();
    let links_in = inp
        .map
        .edges
        .iter()
        .filter(|e| e.1 == i)
        .map(|e| title_of(e.0))
        .collect();
    Some(NotePanel {
        path: path.clone(),
        folder: n.folder.clone(),
        title: n.title.clone(),
        summary,
        chip: n.flags.chip(),
        reads_week: n.reads_week,
        rules_kept: rules.len(),
        changes: receipts.len(),
        rules,
        pending,
        read_by: n.reads_by_agent.clone(),
        links_out,
        links_in,
        receipts,
    })
}

/// Up to three sentences of the first paragraph, or the `summary:` field.
fn long_summary(note: &notes::Note) -> String {
    let one = super::summary_of(note, 400);
    let body = crate::edition::plain(&String::from_utf8_lossy(&note.body));
    let para: String = body
        .lines()
        .skip_while(|l| l.trim().is_empty() || l.starts_with('#'))
        .take_while(|l| !l.trim().is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let para = crate::text::visible(&para.replace("[[", "").replace("]]", ""));
    if para.starts_with(one.trim_end_matches('…')) && para.len() > one.len() {
        crate::edition::sentences(&para, 320)
    } else {
        one
    }
}

fn pending_card(inp: &Inputs<'_>, l: &Lesson) -> RuleCard {
    let evidence = l
        .evidence
        .iter()
        .map(|e| {
            let facts = inp
                .rollup
                .and_then(|r| r.session(&e.session).ok().flatten());
            Said {
                session: e.session.clone(),
                ordinal: e.ordinal,
                agent: facts.as_ref().map(|f| agent(&f.tool)),
                profile: e.profile.clone(),
                session_title: facts.as_ref().and_then(|f| f.title.clone()),
                at: e.at,
                when: super::day_hm(e.at, inp.now, inp.offset),
                said: Some(crate::text::visible(&e.quote)),
                failed: None,
            }
        })
        .collect();
    RuleCard {
        id: l.id,
        kind: l.kind.as_str(),
        from: String::new(),
        title: l.title.clone(),
        rule: l.rule.clone(),
        note: l.target.as_ref().map(|t| t.note.clone()),
        note_label: None,
        new_note: l.target.as_ref().is_some_and(|t| t.new),
        verdict: l.review.as_ref().map(|r| r.notes.clone()),
        agrees: l
            .review
            .as_ref()
            .is_some_and(|r| r.verdict == Verdict::Confirm),
        evidence,
        shared: l.shared.is_some(),
    }
}
