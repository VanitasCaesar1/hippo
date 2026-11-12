//! `hippo brief --task <text>`: the start block for a new agent, in parts a
//! person can turn off. Read-only.
//!
//! The parts: the entry note's opening and each required line, as the
//! session-start hook gives them; each relevant note the context compiler
//! picks for the task; up to three past sessions Ask's search finds for the
//! task, each one short digest line; and kept lessons that share a key term
//! with the task. Excerpts come from the index and the rollup, which hold
//! redacted text only.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::ask;
use crate::config::{Config, Workspace};
use crate::context::{self, Part as CtxPart, PartSelection};
use crate::error::Result;
use crate::index::Index;
use crate::lessons::{self, Status};
use crate::notes::Notes;
use crate::resume;
use crate::rollup::Rollup;

/// How many past sessions a brief offers.
pub const SESSIONS: usize = 3;
/// How many kept rules a brief offers.
pub const RULES: usize = 5;
/// A session digest's limit, in bytes.
pub const DIGEST_BYTES: usize = 700;

/// Which group a part is in, in block order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Group {
    Required,
    Relevant,
    Session,
    Rule,
}

impl Group {
    /// The block's heading for the group; `None` for required, whose lines
    /// carry their own `required:` prefix.
    pub fn heading(self) -> Option<&'static str> {
        match self {
            Group::Required => None,
            Group::Relevant => Some("relevant:"),
            Group::Session => Some("already done:"),
            Group::Rule => Some("rules:"),
        }
    }
}

/// One part of a brief.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Part {
    /// `entry`, `required:<path>`, `relevant:<path>`, `session:<id>`,
    /// `rule:<lesson id>`: what `--off` names.
    pub id: String,
    pub group: Group,
    pub title: String,
    /// Why it is here, short: `entry note`, the ranking's reason (`names
    /// the repo <name>`, `matches <strongest words>`), `<profile> · <day>`.
    pub reason: String,
    /// The lines it adds to the block, without a trailing newline.
    pub text: String,
    /// `text`'s bytes and its newline.
    pub bytes: usize,
    /// A note's path, relative to the workspace.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<PathBuf>,
    /// The note's own size: what reading it costs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note_bytes: Option<u64>,
    /// A session's id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// A session's profile, for the menu's default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// A weak relevant pick: the app starts it off.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub weak: bool,
}

/// A brief: its parts, all on, and what the block is measured against.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Brief {
    pub task: String,
    /// The workspace's folder name.
    pub workspace: String,
    pub budget: usize,
    pub parts: Vec<Part>,
}

/// What a brief reads.
pub struct Inputs<'a> {
    pub cfg: &'a Config,
    pub ws: &'a Workspace,
    pub index: Option<&'a Index>,
    pub rollup: Option<&'a Rollup>,
    /// The notes and their directory relative to the workspace, for Ask's
    /// note matching; sessions are searched without them.
    pub notes: Option<(&'a Notes, &'a Path)>,
    /// The repository the new agent will work in, which lifts the notes
    /// that name it ([`context::rank::repo_name`]).
    pub repo: Option<&'a str>,
}

fn part(id: String, group: Group, title: String, reason: String, text: String) -> Part {
    Part {
        bytes: text.len() + 1,
        id,
        group,
        title,
        reason,
        text,
        note: None,
        note_bytes: None,
        session: None,
        profile: None,
        weak: false,
    }
}

/// Cuts `s` to at most `max` bytes at a character boundary, with `…`.
fn cut_bytes(s: &str, max: usize) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.len() <= max {
        return s;
    }
    let mut end = max.saturating_sub(3);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = s[..end].trim_end().to_owned();
    out.push('…');
    out
}

/// `s` without Markdown that reads badly on one line: fenced code blocks
/// are dropped, emphasis marks and inline backticks go, links keep their
/// text.
fn plain(s: &str) -> String {
    let mut out = String::new();
    let mut fenced = false;
    for line in s.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        out.push_str(line.trim_start_matches(['#', '>', ' ']));
        out.push(' ');
    }
    // An unclosed fence on one line (`a ```code``` b`): drop between marks.
    let mut parts = out.split("```");
    let mut kept = String::new();
    let mut inside = false;
    for p in parts.by_ref() {
        if !inside {
            kept.push_str(p);
        }
        inside = !inside;
    }
    kept.replace("**", "")
        .replace("__", "")
        .replace('`', "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The note title a relevant line names: `- <title>: <first line>`.
fn relevant_title(line: &str) -> String {
    let rest = line.trim_start_matches("- ");
    rest.split_once(": ")
        .map(|(t, _)| t)
        .unwrap_or(rest)
        .to_owned()
}

/// Builds the brief for `task`.
pub fn build(inp: &Inputs<'_>, task: &str) -> Result<Brief> {
    let ws = inp.ws;
    let compiled = context::compile(
        ws,
        inp.index,
        Some(task),
        inp.repo,
        None,
        usize::MAX / 4,
        PartSelection::All,
    )?;
    let note_size = |p: &Path| std::fs::metadata(ws.root.join(p)).ok().map(|m| m.len());
    let task_terms: BTreeSet<String> = ask::terms(task).into_iter().collect();
    let mut parts = Vec::new();

    for (kind, text) in &compiled.parts {
        match kind {
            CtxPart::Entry => {
                let mut p = part(
                    "entry".into(),
                    Group::Required,
                    "Entry note".into(),
                    "its opening section".into(),
                    text.clone(),
                );
                p.note = Some(crate::notes::normalise(&ws.entry_note));
                p.note_bytes = note_size(&ws.entry_note);
                parts.push(p);
            }
            CtxPart::Required => {
                for (line, path) in text.lines().zip(compiled.required.iter()) {
                    let title = path
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let mut p = part(
                        format!("required:{}", path.display()),
                        Group::Required,
                        title,
                        "required in every session".into(),
                        line.to_owned(),
                    );
                    p.note = Some(path.clone());
                    p.note_bytes = note_size(path);
                    parts.push(p);
                }
            }
            CtxPart::Relevant => {
                let lines = text.lines().filter(|l| l.starts_with("- "));
                for (i, ((line, path), why)) in
                    lines.zip(&compiled.relevant).zip(&compiled.why).enumerate()
                {
                    let mut p = part(
                        format!("relevant:{}", path.display()),
                        Group::Relevant,
                        relevant_title(line),
                        why.clone(),
                        line.to_owned(),
                    );
                    p.note = Some(path.clone());
                    p.note_bytes = note_size(path);
                    p.weak = compiled.weak.get(i).copied().unwrap_or(false);
                    parts.push(p);
                }
            }
            _ => {}
        }
    }

    // Past sessions: Ask's search, no model.
    if let Some(index) = inp.index {
        let asked = ask::search(
            &ask::Inputs {
                index,
                rollup: inp.rollup,
                notes: None,
                limit: 8,
            },
            task,
        )?;
        for s in asked
            .sources
            .iter()
            .filter(|s| s.kind == "session")
            .take(SESSIONS)
        {
            let Some(id) = s.id.clone() else { continue };
            let facts = inp.rollup.and_then(|r| r.session(&id).ok().flatten());
            let day = s.time.as_deref().and_then(|t| t.get(..10)).unwrap_or("");
            let open = index
                .turns_for_session(&id)
                .map(|t| resume::open_items(&t))
                .unwrap_or_default();
            let mut line = format!(
                "- {} ({}{}{})",
                s.title,
                s.profile.as_deref().unwrap_or("?"),
                s.project
                    .as_deref()
                    .map(|p| format!(", {p}"))
                    .unwrap_or_default(),
                if day.is_empty() {
                    String::new()
                } else {
                    format!(", {day}")
                }
            );
            if let Some(out) = facts.as_ref().and_then(|f| f.outcome.as_deref()) {
                line.push_str(&format!(": {}", cut_bytes(&plain(out), 320)));
            }
            if !open.is_empty() {
                line.push_str(&format!(" · open: {}", cut_bytes(&open.join("; "), 160)));
            }
            if let Some(r) = &s.resume {
                line.push_str(&format!(" · resume: {r}"));
            }
            let mut p = part(
                format!("session:{id}"),
                Group::Session,
                s.title.clone(),
                format!(
                    "{} · {}",
                    s.profile.as_deref().unwrap_or("?"),
                    if day.is_empty() { "?" } else { day }
                ),
                cut_bytes(&line, DIGEST_BYTES),
            );
            p.session = Some(id);
            p.profile = s.profile.clone();
            parts.push(p);
        }
    }

    // Kept rules that share a key term with the task.
    if !task_terms.is_empty() {
        let mut kept: Vec<lessons::Lesson> = lessons::list(&inp.cfg.home)
            .unwrap_or_default()
            .into_iter()
            .filter(|l| l.status == Status::Kept)
            .filter(|l| {
                l.terms.iter().any(|t| task_terms.contains(t))
                    || ask::terms(&format!("{} {}", l.title, l.rule))
                        .iter()
                        .any(|t| task_terms.contains(t))
            })
            .collect();
        kept.sort_by_key(|l| std::cmp::Reverse(l.updated));
        for l in kept.into_iter().take(RULES) {
            parts.push(part(
                format!("rule:{}", l.id),
                Group::Rule,
                l.title.clone(),
                "kept rule".into(),
                format!("- {}", cut_bytes(&l.rule, 400)),
            ));
        }
    }

    let workspace = ws
        .root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok(Brief {
        task: task.trim().to_owned(),
        workspace,
        budget: ws.budget_bytes,
        parts,
    })
}

/// [`build`] with the configuration's index and rollup.
pub fn run(cfg: &Config, ws: &Workspace, task: &str) -> Result<Brief> {
    let index = Index::open(&cfg.home, false).ok();
    let rollup = Rollup::open(&cfg.home).ok().flatten();
    let repo = std::env::current_dir()
        .ok()
        .and_then(|dir| context::rank::repo_name(&dir, &ws.root));
    build(
        &Inputs {
            cfg,
            ws,
            index: index.as_ref(),
            rollup: rollup.as_ref(),
            notes: None,
            repo: repo.as_deref(),
        },
        task,
    )
}

/// The block with the parts `on` keeps: a header, the task, then the
/// groups in order, each under its heading, separated by blank lines.
pub fn assemble(brief: &Brief, on: impl Fn(&Part) -> bool) -> String {
    let mut body = format!("task: {}\n", brief.task);
    let mut group: Option<Group> = None;
    for p in brief.parts.iter().filter(|p| on(p)) {
        if group != Some(p.group) || p.id == "entry" {
            body.push('\n');
            if let Some(h) = p.group.heading() {
                body.push_str(h);
                body.push('\n');
            }
            group = Some(p.group);
        }
        body.push_str(&p.text);
        body.push('\n');
        if p.id == "entry" {
            // The required lines follow after a blank line, as in the
            // session-start block.
            group = None;
        }
    }
    // The header names the whole block's size, itself included.
    let mut size = body.len();
    for _ in 0..3 {
        let head = header(brief, size);
        let total = head.len() + body.len();
        if total == size {
            break;
        }
        size = total;
    }
    format!("{}{}", header(brief, size), body)
}

fn header(brief: &Brief, size: usize) -> String {
    format!(
        "# hippo brief · {} · {} of {}\n",
        brief.workspace,
        crate::xray::human_bytes(size),
        crate::xray::human_bytes(brief.budget)
    )
}

/// The block with every part on except those whose id `off` names.
pub fn assemble_without(brief: &Brief, off: &[String]) -> String {
    assemble(brief, |p| !off.contains(&p.id))
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    fn brief() -> Brief {
        let mk = |id: &str, group, text: &str| {
            part(id.into(), group, id.into(), String::new(), text.into())
        };
        Brief {
            task: "add a fleet key".into(),
            workspace: "tern".into(),
            budget: 25_600,
            parts: vec![
                mk(
                    "entry",
                    Group::Required,
                    "# Tern notes\n\nRead these first.",
                ),
                mk(
                    "required:notes/A.md",
                    Group::Required,
                    "required: notes/A.md",
                ),
                mk(
                    "required:notes/B.md",
                    Group::Required,
                    "required: notes/B.md",
                ),
                mk("relevant:notes/C.md", Group::Relevant, "- C: about fleets"),
                mk(
                    "session:s1",
                    Group::Session,
                    "- Fleet keys (default, hippo, 2026-11-09): done",
                ),
                mk(
                    "rule:r1",
                    Group::Rule,
                    "- Never type unless the window is frontmost",
                ),
            ],
        }
    }

    #[test]
    fn the_block_has_its_groups_in_order_and_names_its_size() {
        let b = brief();
        let text = assemble(&b, |_| true);
        let expected_body = "task: add a fleet key\n\n# Tern notes\n\nRead these first.\n\nrequired: notes/A.md\nrequired: notes/B.md\n\nrelevant:\n- C: about fleets\n\nalready done:\n- Fleet keys (default, hippo, 2026-11-09): done\n\nrules:\n- Never type unless the window is frontmost\n";
        assert!(text.ends_with(expected_body), "{text}");
        let head = text.lines().next().unwrap();
        assert_eq!(
            head,
            format!(
                "# hippo brief · tern · {} of 25.0 KB",
                crate::xray::human_bytes(text.len())
            )
        );
    }

    #[test]
    fn parts_turned_off_leave_their_heading_out_too() {
        let b = brief();
        let text = assemble_without(&b, &["session:s1".into(), "required:notes/B.md".into()]);
        assert!(!text.contains("already done:"));
        assert!(!text.contains("notes/B.md"));
        assert!(text.contains("required: notes/A.md\n\nrelevant:"));
    }

    #[test]
    fn digests_lose_markdown() {
        let out = "Done. - **Hit rate:** 61%\n```rust\nlet x = 1;\n```\nThe `cache` is warm. a ```inline``` b";
        assert_eq!(plain(out), "Done. - Hit rate: 61% The cache is warm. a b");
    }

    #[test]
    fn digests_are_cut_at_a_character_boundary() {
        let long = "é".repeat(800);
        let c = cut_bytes(&long, DIGEST_BYTES);
        assert!(c.len() <= DIGEST_BYTES && c.ends_with('…'));
    }
}
