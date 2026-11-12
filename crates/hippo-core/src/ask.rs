//! `hippo ask`: has any agent already solved this? Searches every indexed
//! session and the workspace's notes for a question, groups the hits by
//! session or note, and ranks them as sources; optionally one model call
//! writes an answer that cites them.
//!
//! The search needs no model and writes nothing. The written answer goes
//! through `model_command` ([`crate::model`]), given only the top sources'
//! excerpts, which come from the index (redacted text only) and the notes.
//! It must cite at least one source and only returned ones, or it is
//! dropped and the sources stand alone.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::Instant;

use regex::Regex;
use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::config::{Config, Workspace};
use crate::error::Result;
use crate::index::{Index, SessionTurn};
use crate::notes::Notes;
use crate::rollup::{self, Rollup};
use crate::session_meta;

/// The most question words searched for.
const MAX_TERMS: usize = 8;
/// The most matching turns read.
const MAX_TURNS: usize = 600;
/// Turns kept per source as excerpts for the answer.
const EXCERPTS: usize = 3;
/// The longest excerpt of one turn, in characters.
const EXCERPT_CHARS: usize = 1500;
/// The most excerpt text given to the model, in bytes.
const PROMPT_BUDGET: usize = 24 * 1024;
/// The default number of sources.
pub const DEFAULT_LIMIT: usize = 6;

const STOP: &[&str] = &[
    "the",
    "and",
    "for",
    "has",
    "have",
    "had",
    "any",
    "anyone",
    "anybody",
    "agent",
    "agents",
    "already",
    "this",
    "that",
    "with",
    "from",
    "into",
    "how",
    "what",
    "when",
    "where",
    "which",
    "who",
    "why",
    "did",
    "does",
    "done",
    "was",
    "were",
    "are",
    "is",
    "can",
    "could",
    "should",
    "would",
    "will",
    "set",
    "get",
    "got",
    "our",
    "out",
    "about",
    "there",
    "their",
    "them",
    "then",
    "than",
    "you",
    "your",
    "yours",
    "use",
    "used",
    "using",
    "been",
    "being",
    "some",
    "all",
    "not",
    "but",
    "its",
    "it's",
    "also",
    "just",
    "like",
    "make",
    "made",
    "one",
    "two",
    "via",
    "per",
    "solved",
    "before",
    "ever",
    "someone",
    "something",
    "way",
    "ways",
    "need",
    "want",
];

/// The question's search words: lower-case, without stop words and words
/// under three letters, in order, at most [`MAX_TERMS`].
pub fn terms(question: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    question
        .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-' || c == '.'))
        .map(|w| w.trim_matches(|c| c == '.' || c == '-').to_lowercase())
        .filter(|w| w.chars().count() >= 3 && !STOP.contains(&w.as_str()))
        .filter(|w| seen.insert(w.clone()))
        .take(MAX_TERMS)
        .collect()
}

/// A word's stem for matching: a plural or `-ing`/`-ed` ending off, for
/// words long enough to keep a root.
fn stem(term: &str) -> String {
    for suffix in ["ing", "ed", "es", "s"] {
        if term.chars().count() > 5 {
            if let Some(s) = term.strip_suffix(suffix) {
                return s.to_owned();
            }
        }
    }
    term.to_owned()
}

/// An FTS5 query of `terms`, each a prefix of its stem, joined by `op`
/// (`" "` is AND, `" OR "`).
fn fts_query(terms: &[String], op: &str) -> String {
    terms
        .iter()
        .map(|t| {
            let s = stem(t).replace('"', "");
            format!("\"{s}\"*")
        })
        .collect::<Vec<_>>()
        .join(op)
}

/// How many times did the search look.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Searched {
    pub sessions: u64,
    pub notes: u64,
    pub ms: u64,
}

/// One ranked source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Source {
    /// From 1, as the answer cites it.
    pub n: usize,
    /// `session` or `note`.
    pub kind: String,
    /// A session's id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// A note's path, relative to the workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    pub title: String,
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub tool: Option<String>,
    #[serde(default)]
    pub project: Option<String>,
    /// RFC 3339: the last matching turn, or the note's last change.
    #[serde(default)]
    pub time: Option<String>,
    /// The best hit, short: `you: …`, `agent: …`, `Bash: …`.
    pub why: String,
    pub matched: Vec<String>,
    pub hits: u32,
    /// The best matching turn, to open the session at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ordinal: Option<u32>,
    /// The command that resumes the session ([`crate::resume::command`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume: Option<String>,
    /// What the model is given.
    #[serde(skip)]
    pub excerpts: Vec<String>,
}

/// The written answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer {
    pub text: String,
    /// The source numbers it cites, in order of first citation.
    pub citations: Vec<usize>,
}

/// `hippo ask`'s result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Asked {
    pub question: String,
    pub terms: Vec<String>,
    pub searched: Searched,
    pub sources: Vec<Source>,
    pub answer: Option<Answer>,
    /// Why there is no answer although one was asked for.
    pub answer_dropped: Option<String>,
}

/// What a search reads.
pub struct Inputs<'a> {
    pub index: &'a Index,
    pub rollup: Option<&'a Rollup>,
    /// The notes and their directory relative to the workspace, which
    /// note sources' paths start with.
    pub notes: Option<(&'a Notes, &'a std::path::Path)>,
    pub limit: usize,
}

struct Group {
    root: String,
    turns: Vec<(SessionTurn, f64)>,
    matched: BTreeSet<String>,
    score: f64,
    last: String,
}

/// A tool call that runs hippo's own search or context. `hippo_ask` is the
/// dsh plugin's tool (`plugins/dsh/`), which the dsh
/// adapter stores as `hippo_ask {"question": …}`.
static HIPPO_CALL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\bhippo(\s+(ask|recall|context)\b|_ask\b)").expect("valid regex")
});

/// Whether `t` is a tool call that runs `hippo ask`, `hippo recall` or
/// `hippo context` (or the dsh plugin's `hippo_ask`), or a tool result of
/// one: a result whose nearest earlier
/// tool calls (those before it with only calls and results between) hold
/// such a call.
pub fn hippo_output(index: &Index, t: &SessionTurn) -> Result<bool> {
    if t.role != "tool" {
        return Ok(false);
    }
    let kind = |t: &SessionTurn| session_meta::classify(&t.role, t.tool_name.as_deref(), &t.text);
    if kind(t) == session_meta::TurnKind::ToolCall {
        return Ok(HIPPO_CALL.is_match(&t.text));
    }
    for before in index.turns_before(&t.session, t.ordinal, 8, 4000)? {
        match kind(&before) {
            session_meta::TurnKind::ToolCall => {
                if HIPPO_CALL.is_match(&before.text) {
                    return Ok(true);
                }
            }
            session_meta::TurnKind::ToolResult => {}
            _ => break,
        }
    }
    Ok(false)
}

fn root_of(id: &str) -> &str {
    id.split_once("-agent-").map_or(id, |(p, _)| p)
}

fn matched_terms(text: &str, terms: &[String]) -> Vec<String> {
    let lower = text.to_lowercase();
    terms
        .iter()
        .filter(|t| lower.contains(&stem(t)))
        .cloned()
        .collect()
}

/// About `width` characters of `text` around its first match of `terms`,
/// on one line, with `…` where it goes on.
pub fn snippet(text: &str, terms: &[String], width: usize) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let lower = flat.to_lowercase();
    let at = terms
        .iter()
        .filter_map(|t| lower.find(&stem(t)))
        .min()
        .unwrap_or(0);
    let chars: Vec<(usize, char)> = flat.char_indices().collect();
    let pos = chars.iter().position(|(i, _)| *i >= at).unwrap_or(0);
    let start = pos.saturating_sub(width / 3);
    let end = (start + width).min(chars.len());
    let mut s: String = chars[start..end].iter().map(|(_, c)| c).collect();
    if start > 0 {
        s = format!("…{}", s.trim_start());
    }
    if end < chars.len() {
        s = format!("{}…", s.trim_end());
    }
    s
}

/// A turn's text as a reader sees it: a tool call by its summary (its
/// command, path or query), not its JSON.
fn readable(t: &SessionTurn) -> String {
    match t.tool_name.as_deref() {
        Some(name) if t.role == "tool" && t.text.starts_with(name) => {
            session_meta::call_summary(name, &t.text)
        }
        _ => t.text.clone(),
    }
}

/// The most distinct question words in one paragraph of `body` (or its
/// title): a note matches where the words sit together, not when a long
/// note names each somewhere. The words and the paragraph.
fn best_paragraph<'b>(title: &str, body: &'b str, terms: &[String]) -> (Vec<String>, &'b str) {
    let in_title = matched_terms(title, terms);
    let mut best: (Vec<String>, &str) = (Vec::new(), "");
    for para in body.split("\n\n") {
        let mut m = matched_terms(para, terms);
        for t in &in_title {
            if !m.contains(t) {
                m.push(t.clone());
            }
        }
        if m.len() > best.0.len() {
            best = (m, para);
        }
    }
    if best.0.is_empty() && !in_title.is_empty() {
        best.0 = in_title;
    }
    best
}

fn who(t: &SessionTurn) -> String {
    match t.role.as_str() {
        "user" => "you".to_owned(),
        "assistant" => "agent".to_owned(),
        _ => t.tool_name.clone().unwrap_or_else(|| "tool".to_owned()),
    }
}

/// Searches for `question`: ranked sources, no answer.
pub fn search(inp: &Inputs<'_>, question: &str) -> Result<Asked> {
    let started = Instant::now();
    let terms = terms(question);
    let mut asked = Asked {
        question: question.to_owned(),
        terms: terms.clone(),
        searched: Searched {
            sessions: inp.index.session_count()?,
            notes: inp.notes.map_or(0, |(n, _)| n.notes.len() as u64),
            ms: 0,
        },
        sources: Vec::new(),
        answer: None,
        answer_dropped: None,
    };
    if terms.is_empty() {
        asked.searched.ms = started.elapsed().as_millis() as u64;
        return Ok(asked);
    }
    let limit = inp.limit.max(1);

    // Sessions: turns with every word, and turns with any.
    let mut groups: HashMap<String, Group> = HashMap::new();
    let mut seen: HashSet<(String, u32)> = HashSet::new();
    let automated = |id: &str| -> bool {
        inp.rollup
            .and_then(|r| r.session(id).ok().flatten())
            .is_some_and(|f| f.automated)
    };
    let mut skip: HashMap<String, bool> = HashMap::new();
    for op in [" ", " OR "] {
        if op == " OR " && terms.len() == 1 {
            break;
        }
        let turns = inp.index.search_top(&fts_query(&terms, op), MAX_TURNS)?;
        for (rank, t) in turns.into_iter().enumerate() {
            if !seen.insert((t.session.clone(), t.ordinal)) {
                continue;
            }
            let root = root_of(&t.session).to_owned();
            if *skip.entry(root.clone()).or_insert_with(|| automated(&root)) {
                continue;
            }
            // hippo's own ask, recall and context, and what they printed,
            // are not a source.
            if hippo_output(inp.index, &t)? {
                continue;
            }
            let m = matched_terms(&t.text, &terms);
            // bm25 order: earlier is better; an AND hit counts double.
            let weight = if op == " " { 2.0 } else { 1.0 } / (1.0 + rank as f64 / 50.0);
            let g = groups.entry(root.clone()).or_insert_with(|| Group {
                root,
                turns: Vec::new(),
                matched: BTreeSet::new(),
                score: 0.0,
                last: String::new(),
            });
            g.matched.extend(m);
            g.score += weight;
            if t.ts > g.last {
                g.last = t.ts.clone();
            }
            g.turns.push((t, weight));
        }
    }

    // A session scores by its best turns, not by how long it is, and a
    // newer one a little higher.
    let now = OffsetDateTime::now_utc();
    let mut sessions: Vec<Group> = groups.into_values().collect();
    for g in &mut sessions {
        g.turns
            .sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| b.0.ts.cmp(&a.0.ts)));
        let best: f64 = g.turns.iter().take(8).map(|t| t.1).sum();
        let age_days = OffsetDateTime::parse(&g.last, &Rfc3339)
            .map(|t| (now - t).whole_hours() as f64 / 24.0)
            .unwrap_or(30.0)
            .max(0.0);
        g.score = best * (1.0 + 0.5 * (-age_days / 14.0).exp());
    }
    let max_s = sessions
        .iter()
        .map(|g| g.score)
        .fold(0.0, f64::max)
        .max(1e-9);
    // Only the leading sessions are read in full (titles, origins).
    sessions.sort_by(|a, b| {
        b.matched
            .len()
            .cmp(&a.matched.len())
            .then_with(|| b.score.total_cmp(&a.score))
    });
    sessions.truncate(limit * 2);
    struct Ranked {
        coverage: usize,
        score: f64,
        time: String,
        source: Source,
    }
    let mut ranked: Vec<Ranked> = Vec::new();
    for g in sessions.drain(..) {
        // The why-line: the best turn a person or the agent said, else the
        // best tool call.
        let best = g
            .turns
            .iter()
            .find(|(t, _)| t.role != "tool")
            .map_or(&g.turns[0].0, |(t, _)| t);
        let facts = inp.rollup.and_then(|r| r.session(&g.root).ok().flatten());
        let src = inp.index.source(&g.root)?;
        let (tool, path) = match (&facts, &src) {
            (Some(f), _) => (Some(f.tool.clone()), Some(f.transcript_path.clone())),
            (None, Some((tool, path))) => (Some(tool.clone()), Some(path.clone())),
            (None, None) => (None, None),
        };
        let origin = match (&tool, &path) {
            (Some(t), Some(p)) => Some(session_meta::origin(&g.root, t, p)),
            _ => None,
        };
        let title = facts
            .as_ref()
            .and_then(|f| f.title.clone())
            .or_else(|| origin.as_ref().and_then(|o| o.named.clone()))
            .unwrap_or_else(|| g.root.chars().take(8).collect());
        let resume = match (&tool, &path) {
            (Some(t), Some(p)) => crate::resume::command(&crate::resume::Target {
                id: &g.root,
                tool: t,
                transcript: p,
                cwd: origin.as_ref().and_then(|o| o.cwd.as_deref()),
            }),
            _ => None,
        };
        let excerpts = g
            .turns
            .iter()
            .take(EXCERPTS)
            .map(|(t, _)| {
                format!(
                    "{}: {}",
                    who(t),
                    snippet(&readable(t), &terms, EXCERPT_CHARS)
                )
            })
            .collect();
        ranked.push(Ranked {
            coverage: g.matched.len(),
            score: g.score / max_s,
            time: g.last.clone(),
            source: Source {
                n: 0,
                kind: "session".into(),
                id: Some(g.root.clone()),
                path: None,
                title,
                profile: tool
                    .as_deref()
                    .zip(path.as_deref())
                    .and_then(|(t, p)| rollup::profile_of(t, p)),
                tool,
                project: facts
                    .as_ref()
                    .and_then(|f| f.project.clone())
                    .or_else(|| origin.as_ref().and_then(|o| o.project())),
                time: Some(g.last.clone()),
                why: format!("{}: {}", who(best), snippet(&readable(best), &terms, 140)),
                matched: g.matched.iter().cloned().collect(),
                hits: g.turns.len() as u32,
                ordinal: (best.session == g.root).then_some(best.ordinal),
                resume,
                excerpts,
            },
        });
    }

    // Notes: matched in memory, by the paragraph (or title) that holds the
    // most of the question's words.
    if let Some((notes, notes_dir)) = inp.notes {
        let mut found = Vec::new();
        for (rel, note) in &notes.notes {
            let title = note.title();
            let body = String::from_utf8_lossy(&note.body);
            let (matched, para) = best_paragraph(&title, &body, &terms);
            if matched.is_empty() {
                continue;
            }
            let score = (matched.len() + matched_terms(&title, &terms).len()) as f64;
            let para = if para.is_empty() { &body } else { para }.to_owned();
            let time = std::fs::metadata(&note.path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| OffsetDateTime::from(t).format(&Rfc3339).ok());
            let path = crate::notes::normalise(&notes_dir.join(rel));
            found.push((matched, score, time, path, title, para));
        }
        // A note's strength is absolute: the share of the words its best
        // paragraph and its title hold, so the best note is not 1 by fiat.
        let max_n = 2.0 * terms.len().max(1) as f64;
        for (matched, score, time, path, title, para) in found {
            ranked.push(Ranked {
                coverage: matched.len(),
                score: score / max_n,
                time: time.clone().unwrap_or_default(),
                source: Source {
                    n: 0,
                    kind: "note".into(),
                    id: None,
                    path: Some(path),
                    title,
                    profile: None,
                    tool: None,
                    project: None,
                    time,
                    why: snippet(&para, &terms, 140),
                    matched,
                    hits: 1,
                    ordinal: None,
                    resume: None,
                    excerpts: vec![snippet(&para, &terms, EXCERPT_CHARS * 2)],
                },
            });
        }
    }

    // Words matched first, then strength within the kind; notes take at
    // most a third of the places while sessions remain.
    let n_terms = terms.len().max(1) as f64;
    let key = |r: &Ranked| r.coverage as f64 / n_terms + 0.5 * r.score;
    ranked.sort_by(|a, b| key(b).total_cmp(&key(a)).then_with(|| b.time.cmp(&a.time)));
    let max_notes = limit.div_ceil(3);
    let sessions_found = ranked.iter().filter(|r| r.source.kind == "session").count();
    let mut notes_taken = 0;
    let mut kept = Vec::new();
    for r in ranked {
        if kept.len() == limit {
            break;
        }
        if r.source.kind == "note" {
            let room_for_sessions = limit - kept.len()
                > sessions_found.saturating_sub(
                    kept.iter()
                        .filter(|k: &&Ranked| k.source.kind == "session")
                        .count(),
                );
            if notes_taken >= max_notes && !room_for_sessions {
                continue;
            }
            notes_taken += 1;
        }
        kept.push(r);
    }
    asked.sources = kept
        .into_iter()
        .enumerate()
        .map(|(i, r)| Source {
            n: i + 1,
            ..r.source
        })
        .collect();
    asked.searched.ms = started.elapsed().as_millis() as u64;
    Ok(asked)
}

/// The ask pass's system prompt.
pub const SYSTEM_PROMPT: &str = "You are hippo's ask pass. Follow the rules in the message.";

/// The user prompt: the question, the rules and the sources' excerpts,
/// within [`PROMPT_BUDGET`].
pub fn prompt(asked: &Asked) -> String {
    let mut out = format!(
        "A developer asks whether any of their coding agents already solved something.\n\n\
         Question: {}\n\n\
         Rules:\n\
         - Answer only from the numbered sources below. Do not use outside knowledge.\n\
         - Cite every claim with the source number in square brackets, like [1] or [2].\n\
         - Cite only the numbers listed below.\n\
         - If the sources do not answer the question, say so in one sentence.\n\
         - Under 200 words. Plain prose; a short code block is fine if a source has the code.\n\n\
         Sources:\n",
        asked.question
    );
    let mut used = 0;
    for s in &asked.sources {
        let head = format!(
            "\n[{}] {} ({}{})\n",
            s.n,
            s.title,
            s.kind,
            s.time
                .as_deref()
                .map(|t| format!(", {}", &t[..t.len().min(10)]))
                .unwrap_or_default()
        );
        out.push_str(&head);
        for e in &s.excerpts {
            if used + e.len() > PROMPT_BUDGET {
                break;
            }
            used += e.len();
            out.push_str(e);
            out.push('\n');
        }
    }
    out
}

static CITE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[(\d+(?:\s*[,–-]\s*\d+)*)\]").expect("valid regex"));

/// Checks an answer's citations against `n` sources: the numbers it cites,
/// in order, or why it cannot stand.
pub fn validate(text: &str, n: usize) -> std::result::Result<Vec<usize>, String> {
    let mut cited = Vec::new();
    for c in CITE.captures_iter(text) {
        for part in c[1].split([',', '–', '-']) {
            let k: usize = part
                .trim()
                .parse()
                .map_err(|_| format!("a citation is not a number: [{}]", &c[1]))?;
            if k == 0 || k > n {
                return Err(format!("it cites [{k}], but there are {n} sources"));
            }
            if !cited.contains(&k) {
                cited.push(k);
            }
        }
    }
    if cited.is_empty() {
        return Err("it cites no source".to_owned());
    }
    Ok(cited)
}

/// Adds the written answer to `asked`: one model call; a reply whose
/// citations do not check out is dropped, with the reason.
pub fn answer(cfg: &Config, asked: &mut Asked) {
    if asked.sources.is_empty() {
        asked.answer_dropped = Some("nothing matched, so there is nothing to answer from".into());
        return;
    }
    match crate::model::run(cfg, SYSTEM_PROMPT, &prompt(asked)) {
        Ok(reply) => {
            let text = reply.text.trim().to_owned();
            match validate(&text, asked.sources.len()) {
                Ok(citations) => asked.answer = Some(Answer { text, citations }),
                Err(why) => asked.answer_dropped = Some(format!("the answer was dropped: {why}")),
            }
        }
        Err(err) => asked.answer_dropped = Some(format!("the model call failed: {err}")),
    }
}

/// The search with the configuration's index, rollup and notes.
pub fn run(cfg: &Config, ws: Option<&Workspace>, question: &str, limit: usize) -> Result<Asked> {
    let index = Index::open(&cfg.home, false)?;
    let rollup = Rollup::open(&cfg.home).ok().flatten();
    let notes = ws.and_then(|w| Notes::scan(&w.notes_root()).ok());
    search(
        &Inputs {
            index: &index,
            rollup: rollup.as_ref(),
            notes: notes.as_ref().zip(ws.map(|w| w.notes_dir.as_path())),
            limit,
        },
        question,
    )
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests;
