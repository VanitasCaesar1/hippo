//! What a correction is about.
//!
//! Four kinds, tried in this order, by rule only:
//!
//! 1. [`SubjectKind::Step`]: a call of the request the correction answers
//!    that the correction names, with the working minutes since it.
//! 2. [`SubjectKind::Earlier`]: earlier work of the same project the
//!    correction's words name: up to three [`Candidate`]s, from the
//!    correction's own session, its sub-agents and delegated sessions, and
//!    the project's other sessions of the last [`EARLIER_DAYS`] days.
//! 3. [`SubjectKind::Said`]: the agent's last message, when the correction
//!    came within [`SAID_WITHIN`] of it.
//! 4. [`SubjectKind::None`]: none of these.

use std::collections::{BTreeSet, HashMap, HashSet};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::{Duration, OffsetDateTime};

use super::Step;
use crate::error::Result;
use crate::index::{Index, TurnMark};
use crate::lessons::detect;
use crate::rollup::{is_usage_limit, parse_ts, profile_of};
use crate::session_meta::{classify, TurnKind};

/// A gap between two turns no longer than this is work.
pub const IDLE_GAP: Duration = Duration::minutes(5);
/// The longest gap after a tool call counted as the call running.
pub const RUN_CAP: Duration = Duration::minutes(60);
/// How far back a request is looked for.
const WINDOW_CAP: Duration = Duration::days(3);
/// A word held by fewer than this share of the index's turns is rare.
pub const RARE_SHARE: u64 = 100;
/// ... and the count under which a word is rare however small the home.
const RARE_FLOOR: usize = 20;
/// The fewest letters a term may have.
const MIN_TERM: usize = 4;
/// How far back other sessions' work is searched.
pub const EARLIER_DAYS: i64 = 7;
/// A correction this soon after the agent's message answers it.
pub const SAID_WITHIN: Duration = Duration::hours(2);
/// The most candidates kept.
pub const CANDIDATES: usize = 3;
/// The most characters of the agent's message kept.
pub const SAID_CHARS: usize = 2000;

/// Frustration words the lessons pass keeps but that name nothing.
const FILLER: &[&str] = &[
    "wtf", "dude", "idiot", "damn", "god", "gods", "sake", "isnt", "lot", "place", "places", "tho",
    "though", "ever", "even", "sure", "stuff",
];

/// Words of ordinary work that name no particular thing.
const GENERIC: &[&str] = &[
    "add",
    "update",
    "build",
    "git",
    "fix",
    "check",
    "start",
    "new",
    "page",
    "time",
    "data",
    "file",
    "name",
    "list",
    "read",
    "test",
    "run",
    "open",
    "key",
    "info",
    "text",
    "option",
    "after",
    "ask",
    "end",
    "report",
    "module",
    "first",
    "work",
    "agent",
    "code",
    "app",
    "thing",
    "plan",
    "fine",
    "issue",
    "error",
    "done",
    "push",
    "commit",
    "merge",
    "main",
    "branch",
    "line",
    "lines",
    "screen",
    "screenshot",
    "change",
    "changes",
    "before",
    "again",
    "right",
    "left",
    "still",
    "every",
    "other",
    "think",
    "people",
    "well",
    "back",
    "into",
    "doing",
    "about",
    "there",
    "where",
    "which",
    "while",
    "would",
    "could",
    "should",
    "please",
];

/// Which of the four kinds a correction's subject is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SubjectKind {
    Step,
    Earlier,
    Said,
    None,
}

impl SubjectKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SubjectKind::Step => "step",
            SubjectKind::Earlier => "earlier",
            SubjectKind::Said => "said",
            SubjectKind::None => "none",
        }
    }
}

/// What a candidate unit of earlier work is to the correction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Relation {
    /// The correction's own top-level session.
    #[serde(rename = "self")]
    Own,
    /// A Claude Code sub-agent (or workflow agent) below it.
    SubAgent,
    /// A delegated task's session below it.
    Delegated,
    /// Another top-level session of the project, or one below it.
    Other,
}

impl Relation {
    pub fn label(self) -> &'static str {
        match self {
            Relation::Own => "this session",
            Relation::SubAgent => "its sub-agent",
            Relation::Delegated => "a delegated task",
            Relation::Other => "another session",
        }
    }
}

/// A pull request a candidate opened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRef {
    pub url: String,
    pub title: Option<String>,
}

/// One unit of earlier work the correction's words name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    pub session: String,
    /// A sub-agent's or delegated task's title, else the session's.
    pub title: Option<String>,
    pub relation: Relation,
    pub profile: Option<String>,
    /// The matched terms, and the correction's words as typed.
    pub terms: Vec<String>,
    pub words: Vec<String>,
    /// The names that matched (a title, a path, a pull request's title),
    /// at most three.
    pub names: Vec<String>,
    /// Its first matching item: the edit, the pull request's call, or its
    /// first turn for a title.
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    pub ordinal: Option<u32>,
    /// How many files it changed under the matched path.
    pub files: usize,
    /// The matched path: a folder, or the file the term named.
    pub under: Option<String>,
    pub pull: Option<PullRef>,
}

/// What a correction is about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subject {
    pub kind: SubjectKind,
    /// The step (kind `step`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<Step>,
    /// The title of the sub-agent or delegated session the step ran in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sub: Option<String>,
    /// The words that tied the correction to the step.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub matched: Vec<String>,
    /// Working minutes from the step to the correction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minutes: Option<i64>,
    /// Earlier work it may be about (kind `earlier`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<Candidate>,
    /// The agent's last message (kind `said`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub said: Option<Step>,
}

impl Subject {
    fn none() -> Subject {
        Subject {
            kind: SubjectKind::None,
            step: None,
            sub: None,
            matched: Vec::new(),
            minutes: None,
            candidates: Vec::new(),
            said: None,
        }
    }
}

/// Which words are rare in this home, asked of the index once each.
pub struct Rarity<'a> {
    index: &'a Index,
    cap: usize,
    seen: HashMap<String, bool>,
}

impl<'a> Rarity<'a> {
    pub fn new(index: &'a Index) -> Rarity<'a> {
        let total = index.turn_total().unwrap_or(0);
        let cap = usize::try_from(total / RARE_SHARE)
            .unwrap_or(usize::MAX)
            .max(RARE_FLOOR);
        Rarity {
            index,
            cap,
            seen: HashMap::new(),
        }
    }

    /// Whether fewer than the cap of the index's turns hold a word starting
    /// with `term`.
    pub fn rare(&mut self, term: &str) -> bool {
        if let Some(r) = self.seen.get(term) {
            return *r;
        }
        let n = self.index.turns_holding(term, self.cap).unwrap_or(self.cap);
        let r = n < self.cap;
        self.seen.insert(term.to_owned(), r);
        r
    }
}

/// `text`'s terms as this module compares them: words split at
/// punctuation and at a lower-to-upper case change, a plural or `-ing` /
/// `-ed` ending cut, then the lessons detector's key terms (stop words out, six
/// letters).
pub fn stems(text: &str) -> Vec<String> {
    let mut words = String::new();
    let mut word = String::new();
    let mut prev_lower = false;
    let flush = |word: &mut String, words: &mut String| {
        if !word.is_empty() {
            words.push_str(&cut_ending(&word.to_lowercase()));
            words.push(' ');
            word.clear();
        }
    };
    for c in text.chars() {
        if c.is_alphanumeric() || c == '\'' {
            if c.is_uppercase() && prev_lower {
                flush(&mut word, &mut words);
            }
            prev_lower = c.is_lowercase();
            word.push(c);
        } else {
            prev_lower = false;
            flush(&mut word, &mut words);
        }
    }
    flush(&mut word, &mut words);
    detect::key_terms(&words)
}

fn cut_ending(w: &str) -> String {
    for suffix in ["ing", "ed", "s"] {
        if let Some(stem) = w.strip_suffix(suffix) {
            if stem.chars().count() >= 3 && !w.ends_with("ss") {
                return stem.to_owned();
            }
        }
    }
    w.to_owned()
}

/// A shell command's first words: its first line, without leading `cd`,
/// `source`, `export` and assignments, at most eight words.
pub fn command_head(cmd: &str) -> String {
    let line = cmd.lines().next().unwrap_or("");
    let mut out: Vec<&str> = Vec::new();
    for part in line.split(['&', ';', '|']) {
        let words: Vec<&str> = part.split_whitespace().collect();
        let Some(first) = words.first() else { continue };
        if matches!(*first, "cd" | "source" | "." | "export" | "setopt" | "set")
            || (first.contains('=') && words.len() == 1)
        {
            continue;
        }
        for w in words {
            if out.len() == 8 {
                return out.join(" ");
            }
            if out.is_empty() && w.contains('=') {
                continue;
            }
            out.push(w);
        }
    }
    out.join(" ")
}

fn ts(t: &TurnMark) -> Option<OffsetDateTime> {
    parse_ts(&t.ts)
}

/// A person's own request (not an injection, a notification or relayed
/// text).
pub fn is_request(t: &TurnMark) -> bool {
    t.role == "user"
        && classify("user", None, &t.head) == TurnKind::Request
        && !detect::relayed(t.head.trim_start())
}

/// The words a call is known by: the file an edit changed, its
/// description or title, and a command's first words.
fn names(t: &TurnMark) -> BTreeSet<String> {
    let mut text = String::new();
    let edits = matches!(
        t.tool_name.as_deref(),
        Some("Edit" | "MultiEdit" | "Write" | "NotebookEdit")
    );
    if let (true, Some(f)) = (edits, &t.file) {
        text.push_str(f);
        text.push('\n');
    }
    if let Some(a) = &t.about {
        text.push_str(a);
        text.push('\n');
    }
    if let Some(c) = &t.command {
        text.push_str(&command_head(c));
    }
    stems(&text).into_iter().collect()
}

fn word_lists() -> (BTreeSet<String>, BTreeSet<String>) {
    let filler = FILLER.iter().flat_map(|w| stems(w)).collect();
    let generic = GENERIC.iter().flat_map(|w| stems(w)).collect();
    (filler, generic)
}

/// The correction's terms that can name a step.
pub fn terms(correction: &str, project: Option<&str>) -> Vec<String> {
    let project: BTreeSet<String> = project.map(stems).unwrap_or_default().into_iter().collect();
    let (filler, generic) = word_lists();
    stems(correction)
        .into_iter()
        .filter(|w| w.chars().count() >= MIN_TERM)
        .filter(|w| !filler.contains(w) && !generic.contains(w) && !project.contains(w))
        .collect()
}

/// The correction's terms for earlier work, each with the word as typed:
/// the four-letter floor is on the word as typed and a
/// stop word is dropped before its ending is cut (`labs` gives `lab`,
/// `this` gives nothing).
pub fn typed_terms(correction: &str, project: Option<&str>) -> Vec<(String, String)> {
    let project: BTreeSet<String> = project.map(stems).unwrap_or_default().into_iter().collect();
    let (filler, generic) = word_lists();
    let mut out: Vec<(String, String)> = Vec::new();
    for word in correction.split(|c: char| !(c.is_alphanumeric() || c == '\'')) {
        let word = word.trim_matches('\'');
        if word.chars().count() < MIN_TERM || detect::key_terms(word).is_empty() {
            continue;
        }
        for s in stems(word) {
            if filler.contains(&s) || generic.contains(&s) || project.contains(&s) {
                continue;
            }
            if !out.iter().any(|(t, _)| *t == s) {
                out.push((s, word.to_lowercase()));
            }
        }
    }
    out
}

/// The call among `calls` (newest first) that `terms` name, with the
/// terms it holds: a term must name at most a third of the calls (at
/// least two) and be rare; the call holding the most such terms wins,
/// then the newest.
pub fn choose<'t>(
    terms: &[String],
    calls: &[&'t TurnMark],
    rare: &mut dyn FnMut(&str) -> bool,
) -> Option<(&'t TurnMark, Vec<String>)> {
    if terms.is_empty() || calls.is_empty() {
        return None;
    }
    let named: Vec<BTreeSet<String>> = calls.iter().map(|t| names(t)).collect();
    let lim = (calls.len() / 3).max(2);
    let distinctive: Vec<&String> = terms
        .iter()
        .filter(|w| {
            let n = named.iter().filter(|s| s.contains(*w)).count();
            n > 0 && n <= lim
        })
        .collect();
    let mut best: Option<(usize, usize)> = None;
    for (k, set) in named.iter().enumerate() {
        let held: Vec<&&String> = distinctive.iter().filter(|w| set.contains(**w)).collect();
        if held.is_empty() || !held.iter().any(|w| rare(w)) {
            continue;
        }
        if best.is_none_or(|(n, _)| held.len() > n) {
            best = Some((held.len(), k));
        }
    }
    let (_, k) = best?;
    let held = distinctive
        .iter()
        .filter(|w| named[k].contains(**w))
        .map(|w| (*w).clone())
        .collect();
    Some((calls[k], held))
}

/// Working time over `turns` (sorted by time) from `from` to the last of
/// them: a gap counts when it is at most [`IDLE_GAP`], or, after a tool
/// call, at most [`RUN_CAP`] (the call was running).
pub fn working(turns: &[(OffsetDateTime, bool)], from: OffsetDateTime) -> Duration {
    let mut total = Duration::ZERO;
    let mut prev: Option<(OffsetDateTime, bool)> = None;
    for &(at, call) in turns.iter().filter(|(at, _)| *at >= from) {
        if let Some((p, was_call)) = prev {
            let gap = at - p;
            if gap <= IDLE_GAP || (was_call && gap <= RUN_CAP) {
                total += gap;
            }
        }
        prev = Some((at, call));
    }
    total
}

fn fmt(t: OffsetDateTime) -> String {
    t.format(&Rfc3339).unwrap_or_default()
}

/// The sessions session `id` started: its sub-agents (`parent`) and the
/// sessions its delegated tasks ran as, with whether each was delegated.
fn children(conn: &Connection, id: &str) -> Result<Vec<(String, bool)>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, 0 FROM sessions WHERE parent = ?1 UNION SELECT s.id, 1 FROM tasks t JOIN sessions s ON s.id = t.child WHERE t.session_id = ?1 AND t.kind = 'delegate'",
    )?;
    let rows = stmt.query_map([id], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? == 1)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Every session below `id`, each once, with whether it was delegated.
fn descendants(conn: &Connection, id: &str) -> Result<Vec<(String, bool)>> {
    let mut out = Vec::new();
    let mut seen: HashSet<String> = HashSet::from([id.to_owned()]);
    let mut queue = vec![id.to_owned()];
    while let Some(at) = queue.pop() {
        for (c, delegated) in children(conn, &at)? {
            if seen.insert(c.clone()) {
                out.push((c.clone(), delegated));
                queue.push(c);
            }
        }
    }
    Ok(out)
}

/// What [`subject`] reads with: the index, the rollup being rebuilt and
/// the rare-word cache.
pub struct Reader<'a> {
    pub index: &'a Index,
    pub conn: &'a Connection,
    pub rarity: Rarity<'a>,
}

impl<'a> Reader<'a> {
    pub fn new(index: &'a Index, conn: &'a Connection) -> Reader<'a> {
        Reader {
            index,
            conn,
            rarity: Rarity::new(index),
        }
    }
}

/// What the reading of the request found.
struct Request {
    step: Option<(Step, Option<String>, Vec<String>, i64)>,
    said: Option<Step>,
}

/// Kind 1 and the agent's last message: the request the correction
/// answers, read from the index.
fn request(r: &mut Reader<'_>, c: &Step, project: Option<&str>) -> Option<Request> {
    let index = r.index;
    let s = c.session.as_str();
    let own = index
        .turn_marks(s, c.at - WINDOW_CAP, c.at + Duration::seconds(1))
        .ok()?;
    let before: Vec<&TurnMark> = own.iter().filter(|t| t.ordinal < c.ordinal).collect();
    let start_k = before
        .iter()
        .rposition(|t| is_request(t) && ts(t).is_some_and(|at| at < c.at));
    let start = start_k
        .and_then(|k| ts(before[k]))
        .unwrap_or(c.at - WINDOW_CAP);
    let said = before
        .iter()
        .rev()
        .take_while(|t| !is_request(t))
        .find(|t| t.role == "assistant" && !t.head.trim().is_empty() && !is_usage_limit(&t.head))
        .and_then(|t| {
            let text = index
                .turn(s, t.ordinal)
                .ok()
                .flatten()
                .map_or_else(|| t.head.clone(), |full| full.text);
            Some(Step {
                session: s.to_owned(),
                ordinal: t.ordinal,
                at: ts(t)?,
                text: text.trim().chars().take(SAID_CHARS).collect(),
            })
        });
    // The request's turns: the session's after its request, and those of
    // every session below it in that time.
    let mut window: Vec<(String, TurnMark)> = before
        .iter()
        .skip(start_k.map_or(0, |k| k + 1))
        .map(|t| (s.to_owned(), (*t).clone()))
        .collect();
    for (child, _) in descendants(r.conn, s).unwrap_or_default() {
        if let Ok(marks) = index.turn_marks(&child, start, c.at) {
            window.extend(marks.into_iter().map(|t| (child.clone(), t)));
        }
    }
    window.sort_by(|a, b| a.1.ts.cmp(&b.1.ts));
    let wanted = terms(&c.text, project);
    let calls: Vec<&TurnMark> = window
        .iter()
        .rev()
        .map(|(_, t)| t)
        .filter(|t| t.call)
        .collect();
    let rarity = &mut r.rarity;
    let mut rare = |w: &str| rarity.rare(w);
    let Some((call, matched)) = choose(&wanted, &calls, &mut rare) else {
        return Some(Request { step: None, said });
    };
    let session = window
        .iter()
        .find(|(_, t)| std::ptr::eq(t, call))
        .map(|(id, _)| id.clone())
        .unwrap_or_else(|| s.to_owned());
    let at = ts(call)?;
    let text = index
        .turn(&session, call.ordinal)
        .ok()
        .flatten()
        .map(|t| detect::call_line(&t))
        .unwrap_or_else(|| call.tool_name.clone().unwrap_or_default());
    let sub = if session != s {
        title_of(r.conn, &session)
    } else {
        None
    };
    let line: Vec<(OffsetDateTime, bool)> = window
        .iter()
        .filter_map(|(_, t)| Some((ts(t)?, t.call)))
        .filter(|(t, _)| *t < c.at)
        .collect();
    let minutes = working(&line, at).whole_minutes();
    Some(Request {
        step: Some((
            Step {
                session,
                ordinal: call.ordinal,
                at,
                text,
            },
            sub,
            matched,
            minutes,
        )),
        said,
    })
}

/// A session's title as a unit: its task's title when a task started it,
/// else its own.
fn title_of(conn: &Connection, id: &str) -> Option<String> {
    let task: Option<String> = conn
        .query_row(
            "SELECT title FROM tasks WHERE child = ?1 ORDER BY started LIMIT 1",
            [id],
            |r| r.get(0),
        )
        .optional()
        .ok()
        .flatten();
    task.filter(|t| !t.trim().is_empty()).or_else(|| {
        conn.query_row("SELECT title FROM sessions WHERE id = ?1", [id], |r| {
            r.get::<_, Option<String>>(0)
        })
        .optional()
        .ok()
        .flatten()
        .flatten()
    })
}

/// Where a name came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NameKind {
    Title,
    Path,
    Pull,
}

/// One name of a unit: a title, a changed file's path, a pull request's
/// title.
#[derive(Debug, Clone)]
struct Name {
    kind: NameKind,
    text: String,
    stems: BTreeSet<String>,
    at: OffsetDateTime,
    ordinal: Option<u32>,
    url: Option<String>,
}

/// One session searched for earlier work.
#[derive(Debug, Clone)]
struct Unit {
    session: String,
    title: Option<String>,
    relation: Relation,
    profile: Option<String>,
    names: Vec<Name>,
}

/// The path inside its repository of an `edits.file` (`<repository>:<path>`),
/// `None` for a file outside a known repository.
fn repo_path(file: &str) -> Option<&str> {
    if file.starts_with('/') {
        return None;
    }
    file.split_once(':')
        .map(|(_, p)| p)
        .filter(|p| !p.is_empty())
}

/// A session's first and last turn, tool and transcript.
type UnitFacts = (Option<String>, Option<String>, String, String);

/// A unit's names before `before` (and, for another session's work, at or
/// after `since`); `None` when it had none then.
fn unit(
    conn: &Connection,
    id: &str,
    relation: Relation,
    since: Option<OffsetDateTime>,
    before: OffsetDateTime,
) -> Result<Option<Unit>> {
    let facts: Option<UnitFacts> = conn
        .query_row(
            "SELECT first_turn, last_turn, tool, transcript_path FROM sessions WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let Some((first, last, tool, path)) = facts else {
        return Ok(None);
    };
    let Some(first) = first.as_deref().and_then(parse_ts) else {
        return Ok(None);
    };
    if first >= before {
        return Ok(None);
    }
    if let Some(since) = since {
        if last.as_deref().and_then(parse_ts).is_none_or(|l| l < since) {
            return Ok(None);
        }
    }
    let lo = fmt(since.unwrap_or(OffsetDateTime::UNIX_EPOCH));
    let hi = fmt(before);
    let title = title_of(conn, id);
    let mut names = Vec::new();
    if let Some(t) = &title {
        names.push(Name {
            kind: NameKind::Title,
            text: t.clone(),
            stems: stems(t).into_iter().collect(),
            at: first,
            ordinal: None,
            url: None,
        });
    }
    {
        let mut stmt = conn.prepare_cached(
            "SELECT file, MIN(ts), ordinal FROM edits WHERE session_id = ?1 AND ts >= ?2 AND ts < ?3 GROUP BY file ORDER BY MIN(ts)",
        )?;
        let rows = stmt.query_map(params![id, lo, hi], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, u32>(2)?,
            ))
        })?;
        for row in rows {
            let (file, at, ordinal) = row?;
            let Some(p) = repo_path(&file) else { continue };
            names.push(Name {
                kind: NameKind::Path,
                text: p.to_owned(),
                stems: stems(p).into_iter().collect(),
                at: parse_ts(&at).unwrap_or(first),
                ordinal: Some(ordinal),
                url: None,
            });
        }
    }
    {
        let mut stmt = conn.prepare_cached(
            "SELECT url, ts, title FROM pulls WHERE session_id = ?1 AND done = 'opened' AND ts >= ?2 AND ts < ?3 ORDER BY ts",
        )?;
        let rows = stmt.query_map(params![id, lo, hi], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        })?;
        for row in rows {
            let (url, at, t) = row?;
            let t = t.unwrap_or_default();
            names.push(Name {
                kind: NameKind::Pull,
                stems: stems(&t).into_iter().collect(),
                text: t,
                at: parse_ts(&at).unwrap_or(first),
                ordinal: None,
                url: Some(url),
            });
        }
    }
    Ok(Some(Unit {
        session: id.to_owned(),
        title,
        relation,
        profile: profile_of(&tool, std::path::Path::new(&path)),
        names,
    }))
}

/// The units searched for a correction in `root`'s session at `at`: the
/// session, everything below it, then the project's other person-started
/// top-level sessions with work in the [`EARLIER_DAYS`] days before, each
/// with everything below it.
fn units(
    conn: &Connection,
    root: &str,
    project: Option<&str>,
    at: OffsetDateTime,
) -> Result<Vec<Unit>> {
    let mut out = Vec::new();
    let mut seen: HashSet<String> = HashSet::from([root.to_owned()]);
    if let Some(u) = unit(conn, root, Relation::Own, None, at)? {
        out.push(u);
    }
    for (id, delegated) in descendants(conn, root)? {
        seen.insert(id.clone());
        let rel = if delegated {
            Relation::Delegated
        } else {
            Relation::SubAgent
        };
        if let Some(u) = unit(conn, &id, rel, None, at)? {
            out.push(u);
        }
    }
    let Some(project) = project else {
        return Ok(out);
    };
    let since = at - Duration::days(EARLIER_DAYS);
    let others: Vec<String> = {
        let mut stmt = conn.prepare_cached(
            "SELECT s.id FROM sessions s WHERE s.project = ?1 AND s.parent IS NULL AND s.automated = 0 AND s.first_turn < ?2 AND s.last_turn >= ?3 AND NOT EXISTS (SELECT 1 FROM tasks t WHERE t.child = s.id AND t.kind = 'delegate') ORDER BY s.first_turn",
        )?;
        let rows = stmt.query_map(params![project, fmt(at), fmt(since)], |r| r.get(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    for top in others {
        if !seen.insert(top.clone()) {
            continue;
        }
        if let Some(u) = unit(conn, &top, Relation::Other, Some(since), at)? {
            out.push(u);
        }
        for (id, _) in descendants(conn, &top)? {
            if !seen.insert(id.clone()) {
                continue;
            }
            if let Some(u) = unit(conn, &id, Relation::Other, Some(since), at)? {
                out.push(u);
            }
        }
    }
    Ok(out)
}

/// The folder (or file) of `path` whose own name holds one of `terms`,
/// the shallowest first.
fn matched_dir(path: &str, terms: &[String]) -> Option<String> {
    let parts: Vec<&str> = path.split('/').collect();
    for k in 0..parts.len() {
        let part = parts[k];
        let part = if k + 1 == parts.len() {
            part.rsplit_once('.').map_or(part, |(stem, _)| stem)
        } else {
            part
        };
        if stems(part).iter().any(|s| terms.contains(s)) {
            return Some(parts[..=k].join("/"));
        }
    }
    None
}

/// Kind 2: the candidates of earlier work `terms` name among `units`.
fn earlier(units: &[Unit], typed: &[(String, String)]) -> Vec<Candidate> {
    if units.is_empty() || typed.is_empty() {
        return Vec::new();
    }
    let lim = (units.len() / 3).max(2);
    let holds = |u: &Unit, w: &str| u.names.iter().any(|n| n.stems.contains(w));
    let distinctive: Vec<&String> = typed
        .iter()
        .map(|(t, _)| t)
        .filter(|w| {
            let n = units.iter().filter(|u| holds(u, w)).count();
            n > 0 && n <= lim
        })
        .collect();
    let mut found: Vec<(u8, usize, usize, OffsetDateTime, Candidate)> = Vec::new();
    for u in units {
        let own = u.relation != Relation::Other;
        let matching: Vec<(&Name, Vec<String>)> = u
            .names
            .iter()
            .filter_map(|n| {
                let held: Vec<String> = distinctive
                    .iter()
                    .filter(|w| n.stems.contains(**w))
                    .map(|w| (*w).clone())
                    .collect();
                let ok = held.len() >= 2 || (own && n.kind == NameKind::Path && !held.is_empty());
                ok.then_some((n, held))
            })
            .collect();
        if matching.is_empty() {
            continue;
        }
        let mut terms: Vec<String> = Vec::new();
        for (_, held) in &matching {
            for h in held {
                if !terms.contains(h) {
                    terms.push(h.clone());
                }
            }
        }
        let first = matching
            .iter()
            .min_by_key(|(n, _)| n.at)
            .map(|(n, _)| *n)
            .expect("matching is not empty");
        let under = matching
            .iter()
            .filter(|(n, _)| n.kind == NameKind::Path)
            .min_by_key(|(n, _)| n.at)
            .and_then(|(n, held)| matched_dir(&n.text, held));
        let files = under.as_deref().map_or(0, |d| {
            u.names
                .iter()
                .filter(|n| {
                    n.kind == NameKind::Path
                        && (n.text == d || n.text.starts_with(&format!("{d}/")))
                })
                .count()
        });
        // The matching pull request with the most matched words, else the
        // first it opened after the match.
        let pull = matching
            .iter()
            .filter(|(n, _)| n.kind == NameKind::Pull)
            .min_by_key(|(n, held)| (std::cmp::Reverse(held.len()), n.at))
            .map(|(n, _)| *n)
            .or_else(|| {
                u.names
                    .iter()
                    .filter(|n| n.kind == NameKind::Pull && n.at >= first.at)
                    .min_by_key(|n| n.at)
            })
            .and_then(|n| {
                Some(PullRef {
                    url: n.url.clone()?,
                    title: Some(n.text.clone()).filter(|t| !t.is_empty()),
                })
            });
        let words: Vec<String> = terms
            .iter()
            .filter_map(|t| typed.iter().find(|(s, _)| s == t).map(|(_, w)| w.clone()))
            .collect();
        let rank = match u.relation {
            Relation::Own => 0,
            Relation::SubAgent | Relation::Delegated => 1,
            Relation::Other => 2,
        };
        let mut names: Vec<String> = Vec::new();
        for (n, _) in &matching {
            if names.len() < 3 && !names.contains(&n.text) {
                names.push(n.text.clone());
            }
        }
        found.push((
            rank,
            terms.len(),
            files,
            first.at,
            Candidate {
                session: u.session.clone(),
                title: u.title.clone(),
                relation: u.relation,
                profile: u.profile.clone(),
                terms,
                words,
                names,
                at: first.at,
                ordinal: first.ordinal,
                files,
                under,
                pull,
            },
        ));
    }
    found.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(b.1.cmp(&a.1))
            .then(b.2.cmp(&a.2))
            .then(b.3.cmp(&a.3))
    });
    found.into_iter().take(CANDIDATES).map(|f| f.4).collect()
}

/// The ordinal of the call at `at` in `session` that opened a pull request
/// (a `gh pr create` or an MCP call), for Open.
fn pull_call(index: &Index, session: &str, at: OffsetDateTime) -> Option<u32> {
    let marks = index
        .turn_marks(
            session,
            at - Duration::seconds(1),
            at + Duration::seconds(1),
        )
        .ok()?;
    marks
        .iter()
        .find(|t| t.call && ts(t) == Some(at))
        .or_else(|| marks.iter().find(|t| t.call))
        .map(|t| t.ordinal)
}

/// What the correction `c` (its words, session, turn and time) is about.
/// `root` is its top-level session; `project` the project it worked in.
pub fn subject(r: &mut Reader<'_>, c: &Step, root: &str, project: Option<&str>) -> Subject {
    let req = request(r, c, project);
    if let Some(Request {
        step: Some((step, sub, matched, minutes)),
        ..
    }) = &req
    {
        return Subject {
            kind: SubjectKind::Step,
            step: Some(step.clone()),
            sub: sub.clone(),
            matched: matched.clone(),
            minutes: Some(*minutes),
            ..Subject::none()
        };
    }
    let typed = typed_terms(&c.text, project);
    let units = units(r.conn, root, project, c.at).unwrap_or_default();
    let mut candidates = earlier(&units, &typed);
    if !candidates.is_empty() {
        for cand in &mut candidates {
            if cand.ordinal.is_none() {
                if let Some(p) = &cand.pull {
                    if cand.names.iter().any(|n| p.title.as_deref() == Some(n)) {
                        cand.ordinal = pull_call(r.index, &cand.session, cand.at);
                    }
                }
            }
        }
        return Subject {
            kind: SubjectKind::Earlier,
            candidates,
            ..Subject::none()
        };
    }
    if let Some(said) = req
        .and_then(|q| q.said)
        .filter(|s| c.at - s.at <= SAID_WITHIN)
    {
        return Subject {
            kind: SubjectKind::Said,
            said: Some(said),
            ..Subject::none()
        };
    }
    Subject::none()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(
        ord: u32,
        tool: &str,
        file: Option<&str>,
        about: Option<&str>,
        cmd: Option<&str>,
    ) -> TurnMark {
        TurnMark {
            ordinal: ord,
            role: "tool".to_owned(),
            ts: format!("2026-11-10T10:{:02}:00.000000Z", ord),
            tool_name: Some(tool.to_owned()),
            head: format!("{tool} {{}}"),
            tail: None,
            file: file.map(str::to_owned),
            about: about.map(str::to_owned),
            command: cmd.map(str::to_owned),
            call: true,
        }
    }

    #[test]
    fn stems_split_paths_and_case_and_cut_plurals() {
        let s = stems("src/components/tabs/SaveSheet.tsx runners");
        assert!(s.contains(&"tab".to_owned()), "{s:?}");
        assert!(s.contains(&"sheet".to_owned()), "{s:?}");
        assert!(s.contains(&"runner".to_owned()), "{s:?}");
    }

    #[test]
    fn terms_leave_out_filler_generic_and_the_project() {
        let t = terms(
            "wtf dude the covers grid is broken again, fix the build",
            Some("covers-app"),
        );
        assert_eq!(t, ["grid", "broken"]);
    }

    #[test]
    fn typed_terms_floor_the_word_as_typed() {
        let t = typed_terms("in the tabs, this grid is off", Some("covers-app"));
        let terms: Vec<&str> = t.iter().map(|(s, _)| s.as_str()).collect();
        assert_eq!(terms, ["tab", "grid"]);
        assert_eq!(t[0].1, "tabs");
    }

    #[test]
    fn command_head_drops_plumbing() {
        assert_eq!(
            command_head(
                "cd /w/x && FOO=1 runner exec job_1 --timeout 30 -- sh -c 'echo alive'\nmore"
            ),
            "runner exec job_1 --timeout 30 -- sh -c"
        );
    }

    #[test]
    fn a_rare_distinctive_word_picks_its_call_newest_first() {
        let a = call(
            1,
            "Bash",
            None,
            Some("Retry the runner API"),
            Some("runner exec x"),
        );
        let b = call(2, "Edit", Some("/w/src/grid.rs"), None, None);
        let c = call(3, "Bash", None, Some("Run the tests"), Some("cargo test"));
        let d = call(
            4,
            "Bash",
            None,
            Some("Start the runner again"),
            Some("runner new"),
        );
        let calls = vec![&d, &c, &b, &a];
        let terms = terms("stop starting the runner again and again", None);
        let (got, held) = choose(&terms, &calls, &mut |_| true).unwrap();
        assert_eq!(got.ordinal, 4);
        assert_eq!(held, ["runner"]);
    }

    #[test]
    fn a_common_word_or_one_in_every_call_picks_nothing() {
        let a = call(1, "Edit", Some("/w/covers/grid.rs"), None, None);
        let b = call(2, "Edit", Some("/w/covers/list.rs"), None, None);
        let c = call(3, "Edit", Some("/w/covers/tile.rs"), None, None);
        let calls = vec![&c, &b, &a];
        assert!(choose(&terms("the covers look wrong", None), &calls, &mut |_| true).is_none());
        assert!(choose(&terms("the grid look wrong", None), &calls, &mut |_| false).is_none());
        assert_eq!(
            choose(&terms("the grid look wrong", None), &calls, &mut |_| true)
                .map(|(t, _)| t.ordinal),
            Some(1)
        );
    }

    #[test]
    fn the_edit_text_is_not_a_name() {
        let mut a = call(1, "Edit", Some("/w/src/consult.tsx"), None, None);
        a.head = "Edit {\"new_string\":\"<button tabIndex={-1}>\"}".to_owned();
        let calls = vec![&a];
        assert!(
            choose(&terms("these tabs dont animate", None), &calls, &mut |_| {
                true
            })
            .is_none()
        );
    }

    #[test]
    fn working_time_leaves_out_idle_gaps_but_not_a_running_call() {
        let t = |m: i64| OffsetDateTime::UNIX_EPOCH + Duration::minutes(m);
        let line = vec![
            (t(0), true),
            (t(30), false),
            (t(32), false),
            (t(400), true),
            (t(402), false),
        ];
        assert_eq!(working(&line, t(0)).whole_minutes(), 34);
        assert_eq!(working(&line, t(31)).whole_minutes(), 2);
    }

    fn name(kind: NameKind, text: &str, min: i64) -> Name {
        Name {
            kind,
            text: text.to_owned(),
            stems: stems(text).into_iter().collect(),
            at: OffsetDateTime::UNIX_EPOCH + Duration::minutes(min),
            ordinal: Some(min as u32),
            url: (kind == NameKind::Pull).then(|| format!("https://github.com/a/b/pull/{min}")),
        }
    }

    fn u(id: &str, relation: Relation, names: Vec<Name>) -> Unit {
        Unit {
            session: id.to_owned(),
            title: names
                .iter()
                .find(|n| n.kind == NameKind::Title)
                .map(|n| n.text.clone()),
            relation,
            profile: Some("work".to_owned()),
            names,
        }
    }

    fn pool() -> Vec<Unit> {
        vec![
            u(
                "own",
                Relation::Own,
                vec![
                    name(NameKind::Title, "Settings polish", 0),
                    name(NameKind::Path, "src/billing/Invoice.tsx", 5),
                    name(NameKind::Path, "src/billing/Totals.tsx", 6),
                ],
            ),
            u(
                "other-1",
                Relation::Other,
                vec![
                    name(NameKind::Title, "Two-factor authenticator step", 1),
                    name(NameKind::Pull, "Sign-in asks for the authenticator code", 2),
                ],
            ),
            u(
                "other-2",
                Relation::Other,
                vec![name(NameKind::Title, "Fix request headers", 3)],
            ),
            u(
                "other-3",
                Relation::Other,
                vec![name(NameKind::Title, "Write demo-home seeding script", 4)],
            ),
            u(
                "other-4",
                Relation::Other,
                vec![name(NameKind::Title, "Unrelated", 5)],
            ),
            u(
                "other-5",
                Relation::Other,
                vec![name(NameKind::Title, "Unrelated too", 6)],
            ),
        ]
    }

    #[test]
    fn a_file_its_own_session_changed_is_a_candidate_with_its_folder() {
        let typed = typed_terms("why is billing copied like that", None);
        let got = earlier(&pool(), &typed);
        assert_eq!(got.len(), 1, "{got:?}");
        let c = &got[0];
        assert_eq!(c.session, "own");
        assert_eq!(c.relation, Relation::Own);
        assert_eq!(c.words, ["billing"]);
        assert_eq!(c.under.as_deref(), Some("src/billing"));
        assert_eq!(c.files, 2);
        assert_eq!(c.ordinal, Some(5));
    }

    #[test]
    fn another_session_needs_two_words_in_one_name() {
        // Two words in one title of another session: a candidate, with the
        // pull request it opened after.
        let typed = typed_terms("the sign-in asks for an authenticator code now", None);
        let got = earlier(&pool(), &typed);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].session, "other-1");
        assert!(got[0].pull.is_some());
        // One word in another session's title is not.
        assert!(earlier(&pool(), &typed_terms("the header looks lifeless", None)).is_empty());
        assert!(earlier(&pool(), &typed_terms("there is no proper home page", None)).is_empty());
    }

    #[test]
    fn matched_dir_is_the_shallowest_part_holding_a_term() {
        let t = stems("landing");
        assert_eq!(
            matched_dir("src/components/landing/Hero.tsx", &t),
            Some("src/components/landing".to_owned())
        );
        assert_eq!(
            matched_dir("src/ui/LandingHero.tsx", &t),
            Some("src/ui/LandingHero.tsx".to_owned())
        );
    }
}
