//! Incidents: what went wrong, which agent did it, at which step, and
//! whether it had been told.
//!
//! An incident is shown only with evidence a person can check in ten
//! seconds: the person's own words, an edit's lines, an exit code, a hook's
//! message. Detection makes **no model call**. The cause is set by the
//! rules in [`Cause`] alone; where none applies it is
//! [`Cause::NotEstablished`].
//!
//! Each refresh of the rollup writes a session's [`signals`] (corrections,
//! refusals, destructive commands) and its edits' lines; [`rebuild`] then
//! builds the `incidents` table from them, the claims, the tasks and the
//! sessions of the last [`WINDOW_DAYS`] days. Every excerpt comes from the
//! index's redacted text.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::{Duration, OffsetDateTime};

use crate::config::Workspace;
use crate::error::Result;
use crate::index::Index;
use crate::lessons::detect::{key_terms, same_terms};
use crate::rollup::{parse_ts, row_facts, SessionFacts, Stored, SESSION_COLUMNS};
use crate::session_meta::{self, TurnKind};
use crate::util::{hex, sha256};

pub mod signals;
pub mod subject;

use signals::{one_line, SignalKind};

/// How far back incidents are built, in days.
pub const WINDOW_DAYS: i64 = 14;
/// A redo counts within this long of the first edit.
pub const REDO_WITHIN: Duration = Duration::hours(48);
/// The floor for a redo: this many of the first lane's lines…
pub const REDO_LINES: usize = 20;
/// …or half of them when it added at least this many.
pub const REDO_HALF_FROM: usize = 8;
/// Two live lanes' edits of one file this close are a collision.
pub const COLLIDE_WITHIN: Duration = Duration::minutes(30);
/// A correction this soon after a destructive command marks harm.
pub const HARM_WITHIN: Duration = Duration::minutes(60);
/// The longest excerpt kept, in characters.
pub const EXCERPT: usize = 300;

/// What kind of incident.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Corrected,
    Unverified,
    Blocked,
    Failed,
    Redone,
    Collision,
    Destructive,
}

/// The heading hints are listed under, apart from incidents.
pub const HINTS_HEADING: &str = "Hints to check: a check claimed with no run after the last edit";

impl Kind {
    pub const ALL: [Kind; 7] = [
        Kind::Corrected,
        Kind::Unverified,
        Kind::Blocked,
        Kind::Failed,
        Kind::Redone,
        Kind::Collision,
        Kind::Destructive,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Corrected => "corrected",
            Kind::Unverified => "unverified",
            Kind::Blocked => "blocked",
            Kind::Failed => "failed",
            Kind::Redone => "redone",
            Kind::Collision => "collision",
            Kind::Destructive => "destructive",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.as_str() == s)
    }

    /// A hint to check rather than an incident: left out of the counts
    /// and listed apart, under [`HINTS_HEADING`]. A check claimed with no
    /// run after the last edit is one: the rule was right
    /// only about 3 times in 4, below the 80 % an incident needs.
    pub fn is_hint(self) -> bool {
        self == Kind::Unverified
    }

    /// The signal's name as a reader says it.
    pub fn label(self) -> &'static str {
        match self {
            Kind::Corrected => "corrected by you",
            Kind::Unverified => "claimed without checking",
            Kind::Blocked => "blocked",
            Kind::Failed => "failed and left",
            Kind::Redone => "redone by another session",
            Kind::Collision => "collision",
            Kind::Destructive => "destructive command",
        }
    }
}

/// Why it happened, set only by rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Cause {
    /// A kept rule or an injected note covered it.
    HadRule,
    /// The session's start block is recorded and nothing in it covered it.
    NoRule,
    /// The request or the brief asked for the step.
    Brief,
    Claimed,
    Collided,
    ToolFailed,
    Hook,
    Person,
    NotEstablished,
}

impl Cause {
    pub const ALL: [Cause; 9] = [
        Cause::HadRule,
        Cause::NoRule,
        Cause::Brief,
        Cause::Claimed,
        Cause::Collided,
        Cause::ToolFailed,
        Cause::Hook,
        Cause::Person,
        Cause::NotEstablished,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Cause::HadRule => "had-rule",
            Cause::NoRule => "no-rule",
            Cause::Brief => "brief",
            Cause::Claimed => "claimed",
            Cause::Collided => "collided",
            Cause::ToolFailed => "tool-failed",
            Cause::Hook => "hook",
            Cause::Person => "person",
            Cause::NotEstablished => "not-established",
        }
    }

    pub fn parse(s: &str) -> Option<Cause> {
        Cause::ALL.into_iter().find(|c| c.as_str() == s)
    }

    pub fn label(self) -> &'static str {
        match self {
            Cause::HadRule => "had the rule",
            Cause::NoRule => "no rule covered it",
            Cause::Brief => "the brief asked for it",
            Cause::Claimed => "claimed without checking",
            Cause::Collided => "two agents collided",
            Cause::ToolFailed => "the tool failed",
            Cause::Hook => "a hook stopped it",
            Cause::Person => "the person stopped it",
            Cause::NotEstablished => "cause not established",
        }
    }

    /// Whether the cause is one the person acts on: their memory or their
    /// brief.
    pub fn needs_person(self) -> bool {
        matches!(self, Cause::HadRule | Cause::NoRule | Cause::Brief)
    }
}

/// A turn an incident points at, with its words.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    pub session: String,
    pub ordinal: u32,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    /// One line, at most [`EXCERPT`] characters of redacted index text.
    pub text: String,
}

/// Who did it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chain {
    /// The top-level session, following sub-agents and delegations up.
    pub root: String,
    pub root_title: Option<String>,
    /// The session the step is in.
    pub session: String,
    /// The sub-agent's or delegated task's title, when the step is in one.
    pub sub: Option<String>,
    /// `sub-agent` or `delegated`.
    pub sub_kind: Option<String>,
    pub profile: Option<String>,
    pub project: Option<String>,
    pub tool: String,
    pub model: Option<String>,
    pub effort: Option<String>,
}

/// The rule it had.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    /// The note (relative to the workspace), or the sub-agent's brief.
    pub source: String,
    /// The paragraph or kept rule, one line.
    pub text: String,
    /// `opened it` or `never opened it`, from the x-ray's use of the part;
    /// `None` for a brief.
    pub used: Option<String>,
    /// A kept lesson's title, when the rule is one.
    pub lesson: Option<String>,
}

/// The request or brief before the step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Told {
    /// `person` (the person's request) or `parent` (the parent agent's
    /// brief).
    pub whose: String,
    pub text: String,
    pub session: String,
    pub ordinal: Option<u32>,
}

/// Everything about one incident a reader shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Body {
    pub chain: Chain,
    /// What it was told, and the rule it had.
    pub told: Option<Told>,
    pub rule: Option<Rule>,
    /// What it did.
    pub did: Step,
    /// What showed it was wrong.
    pub showed: Step,
    /// What fixed it: the redo.
    pub fixed: Option<Step>,
    /// The other lane, for a redo or a collision.
    pub other: Option<Chain>,
    /// Notes it read or cited at or before the step, newest first.
    pub read: Vec<String>,
    /// A few words more about the cause (`hook:block-force-push`).
    pub detail: Option<String>,
    /// What a correction is about; absent for other kinds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<subject::Subject>,
}

/// One incident.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Incident {
    /// Stable across rebuilds: a hash of the kind and its steps.
    pub id: String,
    pub kind: Kind,
    /// A destructive command nothing harmful followed.
    pub quiet: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    pub cause: Cause,
    /// Minutes from the step to the correction or the redo.
    pub minutes: Option<i64>,
    /// Lines redone.
    pub lines: Option<i64>,
    /// Tokens the sub-task used, when the index has them.
    pub tokens: Option<i64>,
    pub body: Body,
}

impl Incident {
    /// The order within a day: costliest first (minutes, lines, tokens).
    pub fn cost_key(&self) -> (i64, i64, i64) {
        (
            self.minutes.unwrap_or(0),
            self.lines.unwrap_or(0),
            self.tokens.unwrap_or(0),
        )
    }
}

pub(crate) const DDL: &str = "\
CREATE TABLE signals(session_id TEXT NOT NULL, ord INTEGER NOT NULL, kind TEXT NOT NULL, ordinal INTEGER NOT NULL, ts TEXT NOT NULL, step_ordinal INTEGER, step_ts TEXT, step TEXT NOT NULL, text TEXT NOT NULL, extra TEXT NOT NULL, PRIMARY KEY(session_id, ord)) WITHOUT ROWID;
CREATE INDEX signals_ts ON signals(ts);
CREATE TABLE edits(session_id TEXT NOT NULL, ord INTEGER NOT NULL, ordinal INTEGER NOT NULL, ts TEXT NOT NULL, path TEXT NOT NULL, file TEXT NOT NULL, added TEXT NOT NULL, removed TEXT NOT NULL, PRIMARY KEY(session_id, ord)) WITHOUT ROWID;
CREATE INDEX edits_ts ON edits(ts);
CREATE TABLE incidents(id TEXT PRIMARY KEY, kind TEXT NOT NULL, quiet INTEGER NOT NULL, at TEXT NOT NULL, session_id TEXT NOT NULL, root TEXT NOT NULL, cause TEXT NOT NULL, minutes INTEGER, lines INTEGER, tokens INTEGER, profile TEXT, project TEXT, body TEXT NOT NULL);
CREATE INDEX incidents_at ON incidents(at);
";

/// Writes a session's signals and edit lines (`write_session`'s part).
/// Edits are kept for sessions with a turn in the last 30 days.
pub(crate) fn write_session(
    tx: &rusqlite::Transaction<'_>,
    id: &str,
    turns: &[crate::index::StoredTurn],
    person: bool,
    repos: &mut crate::rollup::work::Repos,
) -> Result<()> {
    let mut stmt = tx.prepare_cached(
        "INSERT INTO signals(session_id, ord, kind, ordinal, ts, step_ordinal, step_ts, step, text, extra) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
    )?;
    for (i, s) in signals::signals(id, turns, person).iter().enumerate() {
        stmt.execute(params![
            id,
            i as i64,
            s.kind.as_str(),
            s.ordinal,
            s.ts,
            s.step_ordinal,
            s.step_ts,
            s.step,
            s.text,
            s.extra,
        ])?;
    }
    let recent = turns
        .last()
        .and_then(|t| parse_ts(&t.ts))
        .is_some_and(|t| OffsetDateTime::now_utc() - t <= Duration::days(30));
    if !recent {
        return Ok(());
    }
    let mut stmt = tx.prepare_cached(
        "INSERT INTO edits(session_id, ord, ordinal, ts, path, file, added, removed) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )?;
    for (i, e) in signals::edits(turns, repos).iter().enumerate() {
        stmt.execute(params![
            id,
            i as i64,
            e.ordinal,
            e.ts,
            e.path,
            e.file,
            e.added.join(" "),
            e.removed.join(" "),
        ])?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Building

fn ts(s: &str) -> OffsetDateTime {
    parse_ts(s).unwrap_or(OffsetDateTime::UNIX_EPOCH)
}

/// `t` in UTC, as the index stores turn times: the column is compared as
/// text, so a time in another offset would compare wrong.
fn fmt(t: OffsetDateTime) -> String {
    t.to_offset(time::UtcOffset::UTC)
        .format(&Rfc3339)
        .unwrap_or_default()
}

fn stable_id(parts: &[&str]) -> String {
    hex(&sha256(parts.join("\u{1f}").as_bytes()))[..16].to_owned()
}

/// A sub-task that is a session of its own: the task that started it.
#[derive(Debug, Clone)]
struct Started {
    parent: String,
    title: String,
    input: Option<String>,
    tokens: Option<i64>,
    delegated: bool,
}

/// What [`rebuild`] reads once.
struct World<'a> {
    index: &'a Index,
    ws: Option<&'a Workspace>,
    sessions: HashMap<String, SessionFacts>,
    started: HashMap<String, Started>,
    /// Session → its injected notes: (note, use).
    context: HashMap<String, Vec<(String, String)>>,
    /// Whether the session's start block is recorded.
    recorded: HashSet<String>,
    /// Session → notes it read: (note, ts).
    notes: HashMap<String, Vec<(String, String)>>,
    kept: Vec<crate::lessons::Lesson>,
    note_text: std::cell::RefCell<HashMap<String, Option<String>>>,
}

impl World<'_> {
    fn facts(&self, id: &str) -> Option<&SessionFacts> {
        self.sessions.get(id)
    }

    /// The lane a session belongs to: up its sub-agent parents.
    fn lane(&self, id: &str) -> String {
        let mut at = id.to_owned();
        for _ in 0..8 {
            let up = self
                .facts(&at)
                .and_then(|f| f.parent.clone())
                .or_else(|| at.split_once("-agent-").map(|(p, _)| p.to_owned()))
                .or_else(|| {
                    self.started
                        .get(&at)
                        .filter(|s| !s.delegated)
                        .map(|s| s.parent.clone())
                });
            match up {
                Some(p) if p != at => at = p,
                _ => break,
            }
        }
        at
    }

    /// The top-level session: up sub-agents and delegations.
    fn root(&self, id: &str) -> String {
        let mut at = self.lane(id);
        for _ in 0..8 {
            match self.started.get(&at).filter(|s| s.delegated) {
                Some(s) if s.parent != at => at = self.lane(&s.parent),
                _ => break,
            }
        }
        at
    }

    /// Whether one lane delegated the other directly.
    fn delegated_between(&self, a: &str, b: &str) -> bool {
        let by = |x: &str, y: &str| {
            self.started
                .get(x)
                .is_some_and(|s| s.delegated && self.lane(&s.parent) == y)
        };
        by(a, b) || by(b, a)
    }

    fn chain(&self, session: &str) -> Chain {
        let root = self.root(session);
        let f = self.facts(session);
        let rf = self.facts(&root);
        let started = (session != root)
            .then(|| self.started.get(session))
            .flatten();
        let mut chain = Chain {
            root_title: rf.and_then(|f| f.title.clone()),
            root: root.clone(),
            session: session.to_owned(),
            sub: started.map(|s| s.title.clone()).or_else(|| {
                (session != root)
                    .then(|| f.and_then(|f| f.title.clone()))
                    .flatten()
            }),
            sub_kind: (session != root).then(|| {
                if started.is_some_and(|s| s.delegated) {
                    "delegated".to_owned()
                } else {
                    "sub-agent".to_owned()
                }
            }),
            profile: f
                .and_then(SessionFacts::profile)
                .or_else(|| rf.and_then(SessionFacts::profile)),
            project: f
                .and_then(|f| f.project.clone())
                .or_else(|| rf.and_then(|f| f.project.clone())),
            tool: f.map(|f| f.tool.clone()).unwrap_or_default(),
            model: None,
            effort: None,
        };
        if let Some(f) = f {
            if session.contains("-agent-") && f.tool == "claude-code" {
                chain.model = session_meta::sub_agent_meta(&f.transcript_path).model;
            }
        }
        if let Some(s) = started.filter(|s| s.delegated) {
            if let Some((model, effort)) = self.delegated_target(s) {
                chain.model = chain.model.or(model);
                chain.effort = effort;
            }
        }
        chain
    }

    /// The model and effort a delegated task's call named.
    fn delegated_target(&self, s: &Started) -> Option<(Option<String>, Option<String>)> {
        let input = s.input.as_deref()?;
        let needle: String = input.chars().take(60).collect();
        let needle = serde_json::to_string(&needle).ok()?;
        let needle = needle.trim_matches('"');
        let call = self
            .index
            .calls_holding(&s.parent, needle)
            .ok()?
            .into_iter()
            .next()?;
        let json = call.text.split_once(' ')?.1;
        let v: serde_json::Value = serde_json::from_str(json).ok()?;
        let target = v.get("target")?;
        let model = target
            .get("model")
            .and_then(|m| m.as_str())
            .map(str::to_owned);
        let effort = target
            .pointer("/options/effort")
            .and_then(|m| m.as_str())
            .map(str::to_owned);
        Some((model, effort))
    }

    /// The turn's time and words, from the index.
    fn step(&self, session: &str, ordinal: u32) -> Option<Step> {
        let t = self.index.turn(session, ordinal).ok()??;
        Some(Step {
            session: session.to_owned(),
            ordinal,
            at: ts(&t.ts),
            text: one_line(&t.text, EXCERPT),
        })
    }

    /// What the step's session was told before it: the person's newest
    /// request in a top-level session, else the brief its parent gave.
    fn told(&self, session: &str, ordinal: u32) -> Option<Told> {
        if let Some(s) = self.started.get(session) {
            if let Some(input) = s.input.as_deref().filter(|i| !i.trim().is_empty()) {
                return Some(Told {
                    whose: "parent".to_owned(),
                    text: one_line(input, EXCERPT),
                    session: s.parent.clone(),
                    ordinal: None,
                });
            }
        }
        let before = self.index.turns_before(session, ordinal, 400, 4000).ok()?;
        let req = before.iter().find(|t| {
            session_meta::classify(&t.role, t.tool_name.as_deref(), &t.text) == TurnKind::Request
        })?;
        let text = session_meta::request_text(&req.text).unwrap_or_else(|| req.text.clone());
        let parent_task = self.started.contains_key(session);
        Some(Told {
            whose: if parent_task { "parent" } else { "person" }.to_owned(),
            text: one_line(&text, EXCERPT),
            session: session.to_owned(),
            ordinal: Some(req.ordinal),
        })
    }

    /// Notes the lane read or cited at or before `at`, newest first.
    fn read(&self, session: &str, at: OffsetDateTime) -> Vec<String> {
        let mut seen: Vec<(String, String)> = Vec::new();
        for s in [session.to_owned(), self.lane(session)] {
            for (n, t) in self.notes.get(&s).into_iter().flatten() {
                if ts(t) <= at && !seen.iter().any(|(m, _)| m == n) {
                    seen.push((n.clone(), t.clone()));
                }
            }
        }
        seen.sort_by(|a, b| b.1.cmp(&a.1));
        seen.into_iter().take(5).map(|(n, _)| n).collect()
    }

    fn note_text(&self, note: &str) -> Option<String> {
        let ws = self.ws?;
        let mut cache = self.note_text.borrow_mut();
        cache
            .entry(note.to_owned())
            .or_insert_with(|| std::fs::read_to_string(ws.root.join(note)).ok())
            .clone()
    }

    /// The injected parts of the lane's top session: (note, use).
    fn injected(&self, session: &str) -> Vec<(String, String)> {
        let lane = self.lane(session);
        self.context.get(&lane).cloned().unwrap_or_default()
    }

    /// A rule that covers a correction's terms, or a destructive pattern's
    /// phrase, among what the session was given before `at`.
    fn rule_for(
        &self,
        session: &str,
        at: OffsetDateTime,
        terms: &[String],
        phrase: Option<&str>,
    ) -> Option<Rule> {
        let wanted: BTreeSet<String> = terms.iter().cloned().collect();
        let injected = self.injected(session);
        let used = |u: &str| {
            if u == "ignored" {
                "never opened it".to_owned()
            } else {
                "opened it".to_owned()
            }
        };
        if !wanted.is_empty() {
            for l in &self.kept {
                let Some(kept) = &l.kept else { continue };
                let Some(target) = &l.target else { continue };
                if kept.at > at {
                    continue;
                }
                let lt: BTreeSet<String> = l.terms.iter().cloned().collect();
                if !same_terms(&lt, &wanted) {
                    continue;
                }
                let note = target.note.to_string_lossy().into_owned();
                if let Some((_, u)) = injected.iter().find(|(n, _)| *n == note) {
                    return Some(Rule {
                        source: note,
                        text: one_line(&l.rule, EXCERPT),
                        used: Some(used(u)),
                        lesson: Some(l.title.clone()),
                    });
                }
            }
        }
        let covers = |para: &str| -> bool {
            if let Some(p) = phrase {
                return para.to_lowercase().contains(p);
            }
            if wanted.is_empty() {
                return false;
            }
            let have: BTreeSet<String> = key_terms(para).into_iter().collect();
            wanted.is_subset(&have)
        };
        for (note, u) in &injected {
            let Some(text) = self.note_text(note) else {
                continue;
            };
            if let Some(para) = paragraphs(&text).into_iter().find(|p| covers(p)) {
                return Some(Rule {
                    source: note.clone(),
                    text: one_line(&para, EXCERPT),
                    used: Some(used(u)),
                    lesson: None,
                });
            }
        }
        if let Some(s) = self.started.get(session) {
            if let Some(input) = s.input.as_deref() {
                if let Some(para) = paragraphs(input).into_iter().find(|p| covers(p)) {
                    return Some(Rule {
                        source: "its brief".to_owned(),
                        text: one_line(&para, EXCERPT),
                        used: None,
                        lesson: None,
                    });
                }
            }
        }
        None
    }
}

/// A text's paragraphs and list items, each on its own.
fn paragraphs(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for line in text.lines() {
        let t = line.trim();
        let item = t.starts_with("- ") || t.starts_with("* ") || t.starts_with('#');
        if t.is_empty() || item {
            if !cur.trim().is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            cur.clear();
        }
        if !t.is_empty() {
            cur.push_str(t);
            cur.push(' ');
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

/// The words in a request that would ask for a destructive pattern.
fn asks_for(pattern: &str) -> &'static [&'static str] {
    match pattern {
        "rm -rf" => &["rm -rf", "rm -fr", "rm -r"],
        "git reset --hard" => &["reset --hard"],
        "force push" => &["force push", "force-push", "--force", "push -f"],
        "git clean -f" => &["git clean"],
        "git checkout ." => &["checkout .", "checkout -- ."],
        "git restore ." => &["restore ."],
        "SQL drop" => &["drop table", "drop database", "drop schema", "truncate"],
        _ => &[],
    }
}

/// A signal row: session, kind, ordinal, time, the step's ordinal and
/// time, the step, the text and the extra.
type SignalRow = (
    String,
    String,
    u32,
    String,
    Option<u32>,
    Option<String>,
    String,
    String,
    String,
);

/// Rebuilds the `incidents` table from the rollup's other tables (and the
/// index for excerpts). `home` holds the lesson records.
pub(crate) fn rebuild(
    conn: &mut Connection,
    index: &Index,
    ws: Option<&Workspace>,
    home: &Path,
    now: OffsetDateTime,
) -> Result<usize> {
    let since = now - Duration::days(WINDOW_DAYS);
    let found = detect(conn, index, ws, home, since)?;
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM incidents", [])?;
    {
        let mut stmt = tx.prepare_cached(
            "INSERT OR REPLACE INTO incidents(id, kind, quiet, at, session_id, root, cause, minutes, lines, tokens, profile, project, body) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        )?;
        for i in &found {
            stmt.execute(params![
                i.id,
                i.kind.as_str(),
                i.quiet,
                fmt(i.at),
                i.body.chain.session,
                i.body.chain.root,
                i.cause.as_str(),
                i.minutes,
                i.lines,
                i.tokens,
                i.body.chain.profile,
                i.body.chain.project,
                serde_json::to_string(&i.body).unwrap_or_default(),
            ])?;
        }
    }
    tx.commit()?;
    Ok(found.len())
}

/// Finds the incidents since `since`, without writing.
pub(crate) fn detect(
    conn: &Connection,
    index: &Index,
    ws: Option<&Workspace>,
    home: &Path,
    since: OffsetDateTime,
) -> Result<Vec<Incident>> {
    let w = world(conn, index, ws, home, since)?;
    let mut out: Vec<Incident> = Vec::new();
    let s = fmt(since);

    // Signals.
    let rows: Vec<SignalRow> = {
        let mut stmt = conn.prepare(
            "SELECT session_id, kind, ordinal, ts, step_ordinal, step_ts, step, text, extra FROM signals WHERE ts >= ?1 ORDER BY ts",
        )?;
        let rows = stmt.query_map([&s], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
                r.get(7)?,
                r.get(8)?,
            ))
        })?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let corrections: Vec<(String, OffsetDateTime, u32, String)> = rows
        .iter()
        .filter(|r| r.1 == "correction")
        .map(|r| (w.lane(&r.0), ts(&r.3), r.2, r.7.clone()))
        .collect();
    let mut destructive = Vec::new();
    let mut reader = subject::Reader::new(index, conn);
    for (session, kind, ordinal, at, step_ord, step_ts, step, text, extra) in &rows {
        let Some(kind) = SignalKind::parse(kind) else {
            continue;
        };
        let at = ts(at);
        let did = Step {
            session: session.clone(),
            ordinal: step_ord.unwrap_or(*ordinal),
            at: step_ts.as_deref().map_or(at, ts),
            text: step.clone(),
        };
        let showed = Step {
            session: session.clone(),
            ordinal: *ordinal,
            at,
            text: text.clone(),
        };
        let chain = w.chain(session);
        match kind {
            SignalKind::Correction => {
                if !objects(text) {
                    continue;
                }
                let terms: Vec<String> = extra.split_whitespace().map(str::to_owned).collect();
                let rule = w.rule_for(session, did.at, &terms, None);
                let cause = if rule.is_some() {
                    Cause::HadRule
                } else if w.recorded.contains(&w.lane(session)) {
                    Cause::NoRule
                } else {
                    Cause::NotEstablished
                };
                let told = w.told(session, did.ordinal);
                let read = w.read(session, did.at);
                // What it is about: the step replaces the
                // nearest action, else the correction stands for itself.
                let about =
                    subject::subject(&mut reader, &showed, &chain.root, chain.project.as_deref());
                let (did, minutes, chain) = match &about.step {
                    Some(step) => (
                        step.clone(),
                        about.minutes,
                        if step.session == *session {
                            chain
                        } else {
                            w.chain(&step.session)
                        },
                    ),
                    None => (showed.clone(), None, chain),
                };
                out.push(Incident {
                    id: stable_id(&["corrected", session, &ordinal.to_string()]),
                    kind: Kind::Corrected,
                    quiet: false,
                    at,
                    cause,
                    minutes,
                    lines: None,
                    tokens: None,
                    body: Body {
                        told,
                        rule,
                        read,
                        chain,
                        did,
                        showed,
                        fixed: None,
                        other: None,
                        detail: None,
                        subject: Some(about),
                    },
                });
            }
            SignalKind::Blocked => {
                let (cause, detail) = match extra.as_str() {
                    "person" => (Cause::Person, None),
                    "check" => (
                        Cause::Hook,
                        Some("the client's removal safety check".to_owned()),
                    ),
                    h => (Cause::Hook, Some(h.trim_start_matches("hook:").to_owned())),
                };
                out.push(Incident {
                    id: stable_id(&["blocked", session, &ordinal.to_string()]),
                    kind: Kind::Blocked,
                    // Listed, not counted.
                    quiet: true,
                    at,
                    cause,
                    minutes: None,
                    lines: None,
                    tokens: None,
                    body: Body {
                        told: w.told(session, did.ordinal),
                        rule: None,
                        read: w.read(session, did.at),
                        chain,
                        did,
                        showed,
                        fixed: None,
                        other: None,
                        detail,
                        subject: None,
                    },
                });
            }
            SignalKind::Destructive => destructive.push((
                session.clone(),
                *ordinal,
                at,
                did,
                extra.clone(),
                chain,
                text.clone(),
            )),
        }
    }

    // Claims without checking.
    {
        let mut stmt = conn.prepare(
            "SELECT c.session_id, c.claim, c.evidence, c.ordinal, c.kind FROM claims c JOIN sessions s ON s.id = c.session_id WHERE c.status = 'unverified' AND s.last_turn >= ?1 AND c.kind IN ('tests', 'typecheck', 'lint', 'fmt', 'build', 'brain-check')",
        )?;
        let rows: Vec<(String, String, String, u32, String)> = stmt
            .query_map([&s], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        // A check that a sentence plans ("…, then commit") is not a claim
        // it made; the claims pass reads some as one.
        static PLAN: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"(?i)\b(then|next|will|going to|let me|now running)\b")
                .expect("valid regex")
        });
        for (session, claim, evidence, ordinal, kind) in rows {
            if PLAN.is_match(&claim) {
                continue;
            }
            let Some(said) = w.step(&session, ordinal) else {
                continue;
            };
            if said.at < since {
                continue;
            }
            // Relayed from a delegated session whose own transcript shows
            // the check passing.
            if relayed(conn, &w, &session, &kind, ordinal)? {
                continue;
            }
            out.push(Incident {
                id: stable_id(&["unverified", &session, &ordinal.to_string()]),
                kind: Kind::Unverified,
                quiet: false,
                at: said.at,
                cause: Cause::Claimed,
                minutes: None,
                lines: None,
                tokens: None,
                body: Body {
                    chain: w.chain(&session),
                    told: w.told(&session, ordinal),
                    rule: None,
                    did: Step {
                        text: format!("\u{201c}{}\u{201d}", one_line(&claim, EXCERPT)),
                        ..said.clone()
                    },
                    showed: Step {
                        text: one_line(&evidence, EXCERPT),
                        ..said.clone()
                    },
                    fixed: None,
                    other: None,
                    read: w.read(&session, said.at),
                    detail: None,
                    subject: None,
                },
            });
        }
    }

    // Failed and left.
    {
        let mut stmt = conn.prepare(
            "SELECT session_id, ord, kind, title, started, ended, status, input, report, tokens, exit, child FROM tasks WHERE status = 'failed' AND (exit IS NULL OR exit < 128) AND started >= ?1",
        )?;
        type Row = (
            String,
            i64,
            String,
            String,
            String,
            Option<String>,
            String,
            Option<String>,
            Option<String>,
            Option<i64>,
            Option<i64>,
            Option<String>,
        );
        let rows: Vec<Row> = stmt
            .query_map([&s], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                    r.get(8)?,
                    r.get(9)?,
                    r.get(10)?,
                    r.get(11)?,
                ))
            })?
            .collect::<rusqlite::Result<_>>()?;
        let mut later = conn.prepare(
            "SELECT COUNT(*) FROM tasks WHERE session_id = ?1 AND kind = ?2 AND lower(trim(title)) = lower(trim(?3)) AND started > ?4",
        )?;
        for (
            session,
            ord,
            kind,
            title,
            started,
            ended,
            status,
            input,
            report,
            tokens,
            exit,
            child,
        ) in rows
        {
            let redone: i64 =
                later.query_row(params![session, kind, title, started], |r| r.get(0))?;
            if redone > 0 {
                continue;
            }
            if kind == "bash" {
                if let Some(cmd) = input.as_deref() {
                    let needle: String = cmd.chars().take(80).collect();
                    let needle = serde_json::to_string(&needle).unwrap_or_default();
                    let needle = needle.trim_matches('"');
                    let start = ts(&started) + Duration::seconds(1);
                    let again = index
                        .calls_holding(&session, needle)?
                        .iter()
                        .any(|t| ts(&t.ts) > start);
                    if again {
                        continue;
                    }
                }
            }
            let started_at = ts(&started);
            let end_at = ended.as_deref().map_or(started_at, ts);
            let mut words = format!(
                "{} {status}",
                match kind.as_str() {
                    "bash" => "Command",
                    "delegate" => "Delegated task",
                    _ => "Sub-agent",
                }
            );
            if let Some(e) = exit {
                words.push_str(&format!(" · exit {e}"));
            }
            let first_report = report
                .as_deref()
                .and_then(|r| r.lines().map(str::trim).find(|l| !l.is_empty()))
                .map(|l| one_line(l, EXCERPT - 40));
            if let Some(r) = &first_report {
                words.push_str(&format!(" · {r}"));
            }
            let failed_tool = report.as_deref().is_some_and(tool_failure);
            let in_session = child.clone().unwrap_or_else(|| session.clone());
            out.push(Incident {
                id: stable_id(&["failed", &session, &ord.to_string(), &started]),
                kind: Kind::Failed,
                // Listed, not counted.
                quiet: true,
                at: end_at,
                cause: if failed_tool {
                    Cause::ToolFailed
                } else {
                    Cause::NotEstablished
                },
                minutes: None,
                lines: None,
                tokens,
                body: Body {
                    chain: w.chain(&in_session),
                    told: input.as_deref().map(|i| Told {
                        whose: "parent".to_owned(),
                        text: one_line(i, EXCERPT),
                        session: session.clone(),
                        ordinal: None,
                    }),
                    rule: None,
                    did: Step {
                        session: session.clone(),
                        ordinal: 0,
                        at: started_at,
                        text: one_line(&title, EXCERPT),
                    },
                    showed: Step {
                        session: in_session.clone(),
                        ordinal: 0,
                        at: end_at,
                        text: words,
                    },
                    fixed: None,
                    other: None,
                    read: Vec::new(),
                    detail: None,
                    subject: None,
                },
            });
        }
        // Sessions that ended on an error other than a limit.
        for f in w.sessions.values() {
            if f.stored != Stored::Interrupted {
                continue;
            }
            let Some(detail) = f.ending_detail.as_deref() else {
                continue;
            };
            if detail == "stopped by you" {
                continue;
            }
            let Some(at) = f.ending_at.or(f.last_turn) else {
                continue;
            };
            if at < since {
                continue;
            }
            let last = index.last_ordinal(&f.id)?.unwrap_or(0);
            out.push(Incident {
                id: stable_id(&["failed-session", &f.id]),
                kind: Kind::Failed,
                // Listed, not counted.
                quiet: true,
                at,
                cause: Cause::ToolFailed,
                minutes: None,
                lines: None,
                tokens: None,
                body: Body {
                    chain: w.chain(&f.id),
                    told: w.told(&f.id, last + 1),
                    rule: None,
                    did: Step {
                        session: f.id.clone(),
                        ordinal: last,
                        at,
                        text: "the session's last step".to_owned(),
                    },
                    showed: Step {
                        session: f.id.clone(),
                        ordinal: last,
                        at,
                        text: format!("Session ended: {detail}"),
                    },
                    fixed: None,
                    other: None,
                    read: Vec::new(),
                    detail: Some(detail.to_owned()),
                    subject: None,
                },
            });
        }
    }

    // Redone and collisions.
    let redos = redone_and_collisions(conn, &w, &s, &mut out)?;

    // Destructive commands: an incident only when harm followed.
    for (session, ordinal, at, did, pattern, chain, words) in destructive {
        let lane = w.lane(&session);
        let root = w.root(&session);
        let harm = corrections.iter().find(|(l, t, _, text)| {
            (*l == lane || *l == root) && *t > at && *t - at <= HARM_WITHIN && names_harm(text)
        });
        let redo = redos
            .iter()
            .find(|(a, t)| *a == lane && *t > at && *t - at <= REDO_WITHIN);
        let quiet = harm.is_none() && redo.is_none();
        let told = w.told(&session, ordinal);
        let phrases = asks_for(&pattern);
        let asked = told
            .as_ref()
            .is_some_and(|t| phrases.iter().any(|p| t.text.to_lowercase().contains(p)));
        // A note that names the command is not a rule against it: on real
        // data the paragraphs it found were about something else.
        let rule = None;
        let cause = if asked {
            Cause::Brief
        } else {
            Cause::NotEstablished
        };
        let showed = match harm {
            Some((l, t, o, text)) => Step {
                session: if *l == lane {
                    lane.clone()
                } else {
                    root.clone()
                },
                ordinal: *o,
                at: *t,
                text: text.clone(),
            },
            None => Step {
                session: session.clone(),
                ordinal,
                at,
                text: words.clone(),
            },
        };
        out.push(Incident {
            id: stable_id(&["destructive", &session, &ordinal.to_string()]),
            kind: Kind::Destructive,
            quiet,
            at,
            cause,
            minutes: harm.map(|h| (h.1 - at).whole_minutes()),
            lines: None,
            tokens: None,
            body: Body {
                chain,
                told,
                rule,
                read: w.read(&session, at),
                did,
                showed,
                fixed: None,
                other: None,
                detail: Some(pattern),
                subject: None,
            },
        });
    }

    // Tokens for a step inside a sub-task.
    for i in &mut out {
        if i.tokens.is_none() {
            i.tokens = w.started.get(&i.body.chain.session).and_then(|s| s.tokens);
        }
    }
    out.sort_by(|a, b| b.at.cmp(&a.at).then(a.id.cmp(&b.id)));
    out.dedup_by(|a, b| a.id == b.id);
    Ok(out)
}

/// Whether an unverified claim of `kind` that `session` made at turn
/// `ordinal` relays a session it delegated: that
/// session's own transcript shows a check of the kind passing after its
/// last relevant edit, before the claim, and `session` edited no code in
/// that check's repository after it.
fn relayed(
    conn: &rusqlite::Connection,
    w: &World<'_>,
    session: &str,
    kind: &str,
    ordinal: u32,
) -> Result<bool> {
    let Some(kind) = crate::xray::claims::ClaimKind::parse(kind) else {
        return Ok(false);
    };
    let Some(said) = w.index.turn(session, ordinal)? else {
        return Ok(false);
    };
    let said_ts = said.ts.clone();
    let mut children = conn.prepare_cached(
        "SELECT DISTINCT child FROM tasks WHERE session_id = ?1 AND kind = 'delegate' AND child IS NOT NULL",
    )?;
    let children: Vec<String> = children
        .query_map([session], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let mut edits = conn
        .prepare_cached("SELECT path FROM edits WHERE session_id = ?1 AND ts > ?2 AND ts <= ?3")?;
    let notes_roots: Vec<std::path::PathBuf> =
        w.ws.map(|ws| vec![ws.root.join(&ws.notes_dir)])
            .unwrap_or_default();
    for child in children {
        let Some((tool, transcript)) = w.index.source(&child)? else {
            continue;
        };
        let turns = w.index.turns_for_session(&child)?;
        let subs: Vec<(String, Vec<crate::index::StoredTurn>)> = w
            .index
            .sub_agents(&child)?
            .into_iter()
            .map(|s| Ok((s.clone(), w.index.turns_for_session(&s)?)))
            .collect::<Result<_>>()?;
        let dir = session_meta::origin(&child, &tool, &transcript).cwd;
        let inp = crate::xray::claims::Inputs {
            turns: &turns,
            subs: &subs,
            dir: dir.as_deref(),
            notes_roots: &notes_roots,
        };
        let Some((ran, root)) = crate::xray::claims::shown(&inp, kind, &said_ts) else {
            continue;
        };
        let paths: Vec<String> = edits
            .query_map(rusqlite::params![session, ran, said_ts], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let code = paths.iter().any(|p| {
            let path = Path::new(p);
            root.as_deref().is_none_or(|r| path.starts_with(r))
                && !matches!(
                    path.extension()
                        .and_then(|e| e.to_str())
                        .map(str::to_lowercase)
                        .as_deref(),
                    Some("md" | "mdx" | "txt" | "rst")
                )
        });
        if !code {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Whether a correction is an objection to what the agent did, not news
/// about the world or a new instruction with a constraint in it. The
/// lessons pass reads "we dont have a staging server yet" or "test it all
/// first" as rule-shaped turns, which is right for a lesson and wrong for an
/// incident. Only a strong objection counts as a mistake: one that swears, shouts, says "again", "wrong",
/// "worse" or "I said". A plain "no, …", "why …?" or "not …" was as often
/// taste, a question or a new instruction.
pub fn objects(text: &str) -> bool {
    // News about the world: a subject that is not the agent (a pronoun
    // other than `you` and `it`, or a word any product has for the people
    // it serves) followed by a negation.
    static NEWS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?i)^\W*((we|they|he|she|our|their|people|users?|customers?|clients?|members?|visitors?|buyers?|sellers?)\s+(dont|don't|do not|didn'?t|cant|can'?t|wont|won'?t|never|are not|aren'?t|have no|haven'?t)\b|i\s+(dont|don'?t|do not)\s+(have|need|know|use|think)\b)",
        )
        .expect("valid regex")
    });
    let news = NEWS.is_match(text);
    static OBJECTION: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?i)\b(wtf|shit\w*|f+u+c*k\w*|idiot\w*|stupid|dumb|bruh|damn|again|wrong|worse|i (said|told you|asked)|how many times|for gods? sake)\b",
        )
        .expect("valid regex")
    });
    let letters: Vec<char> = text.chars().filter(|c| c.is_alphabetic()).collect();
    let shouting = letters.len() >= 10
        && letters.iter().filter(|c| c.is_uppercase()).count() * 10 >= letters.len() * 7;
    !news && (OBJECTION.is_match(text) || shouting)
}

/// Whether a correction names the harm a destructive command does: a
/// word for losing work, as a whole word.
fn names_harm(correction: &str) -> bool {
    static HARM: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?i)\b(delet\w*|remov\w*|wip(e|ed|ing)|lost|gone|reset|force[ds]?|overwr\w*|drop(ped)?|eras\w*|undo|restor\w*|where (is|are|did))\b",
        )
        .expect("valid regex")
    });
    HARM.is_match(correction)
}

/// A `PreToolUse` hook that would have blocked a destructive pattern:
/// the script's file name, the
/// script, and the `settings.json` entry that runs it. Hippo shows it to
/// copy and never installs it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Hook {
    pub file: String,
    pub script: String,
    pub settings: String,
}

/// The hook for a destructive pattern (`Incident::body.detail`), or `None`
/// for a pattern no hook is written for.
pub fn hook_for(pattern: &str) -> Option<Hook> {
    let (slug, test, what) = match pattern {
        "rm -rf" => ("rm-rf", r#"*"rm -rf"*|*"rm -fr"*|*"rm -Rf"*"#, "rm -rf"),
        "git reset --hard" => ("git-reset-hard", r#"*"reset --hard"*"#, "git reset --hard"),
        "force push" => (
            "force-push",
            r#"*"push --force "*|*"push -f "*|*"push --force"|*"push -f""#,
            "a force push",
        ),
        "git clean -f" => (
            "git-clean",
            r#"*"git clean -f"*|*"git clean -df"*|*"git clean -fd"*"#,
            "git clean -f",
        ),
        "git checkout ." => (
            "git-checkout-dot",
            r#"*"checkout -- ."*|*"checkout ."*"#,
            "git checkout .",
        ),
        "git restore ." => ("git-restore-dot", r#"*"restore ."*"#, "git restore ."),
        "SQL drop" => (
            "sql-drop",
            r#"*"DROP TABLE"*|*"drop table"*|*"DROP DATABASE"*|*"TRUNCATE"*"#,
            "DROP or TRUNCATE",
        ),
        _ => return None,
    };
    let file = format!("block-{slug}.sh");
    let script = format!(
        "#!/bin/sh\n# Blocks {what} until the person runs it themselves.\n# Claude Code passes the call as JSON on stdin; exit 2 refuses it.\ncmd=$(/usr/bin/python3 -c 'import json,sys; print(json.load(sys.stdin).get(\"tool_input\", {{}}).get(\"command\", \"\"))')\ncase \"$cmd\" in\n  {test})\n    echo \"Blocked: {what} needs the person's go-ahead. Ask them to run it.\" >&2\n    exit 2 ;;\nesac\nexit 0\n"
    );
    let settings = format!(
        "{{\n  \"hooks\": {{\n    \"PreToolUse\": [\n      {{\n        \"matcher\": \"Bash\",\n        \"hooks\": [{{ \"type\": \"command\", \"command\": \"~/.claude/hooks/{file}\" }}]\n      }}\n    ]\n  }}\n}}\n"
    );
    Some(Hook {
        file,
        script,
        settings,
    })
}

/// A report that says the tool, not the agent, failed.
fn tool_failure(report: &str) -> bool {
    let r = report.to_lowercase();
    [
        "usage limit",
        "rate limit",
        "rate-limit",
        "sign-in",
        "sign in",
        "login",
        "user not found",
        "overloaded",
        "crash",
        "panicked",
        "api error",
        "529",
        "credit balance",
    ]
    .iter()
    .any(|w| r.contains(w))
}

fn world<'a>(
    conn: &Connection,
    index: &'a Index,
    ws: Option<&'a Workspace>,
    home: &Path,
    since: OffsetDateTime,
) -> Result<World<'a>> {
    // Lanes need their parents even when older than the window.
    let from = fmt(since - Duration::days(7));
    let sessions: HashMap<String, SessionFacts> = {
        let mut stmt = conn.prepare(&format!(
            "SELECT {SESSION_COLUMNS} FROM sessions WHERE last_turn >= ?1"
        ))?;
        let rows = stmt.query_map([&from], row_facts)?;
        rows.map(|r| r.map(|f| (f.id.clone(), f)))
            .collect::<rusqlite::Result<_>>()?
    };
    let started: HashMap<String, Started> = {
        let mut stmt = conn.prepare(
            "SELECT child, session_id, title, input, tokens, kind FROM tasks WHERE child IS NOT NULL",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                Started {
                    parent: r.get(1)?,
                    title: r.get(2)?,
                    input: r.get(3)?,
                    tokens: r.get(4)?,
                    delegated: r.get::<_, String>(5)? == "delegate",
                },
            ))
        })?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let mut context: HashMap<String, Vec<(String, String)>> = HashMap::new();
    let mut recorded = HashSet::new();
    {
        let mut stmt = conn.prepare("SELECT session_id, note, use FROM context")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let (id, note, used) = row?;
            if !sessions.contains_key(&id) {
                continue;
            }
            recorded.insert(id.clone());
            if let Some(n) = note {
                context.entry(id).or_default().push((n, used));
            }
        }
    }
    let mut notes: HashMap<String, Vec<(String, String)>> = HashMap::new();
    {
        let mut stmt = conn.prepare("SELECT session_id, note, ts FROM notes WHERE ts >= ?1")?;
        let rows = stmt.query_map([&from], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let (id, note, t) = row?;
            notes.entry(id).or_default().push((note, t));
        }
    }
    let kept = crate::lessons::list(home)
        .unwrap_or_default()
        .into_iter()
        .filter(|l| l.status == crate::lessons::Status::Kept)
        .collect();
    Ok(World {
        index,
        ws,
        sessions,
        started,
        context,
        recorded,
        notes,
        kept,
        note_text: std::cell::RefCell::new(HashMap::new()),
    })
}

/// One edit row read back.
struct EditRow {
    session: String,
    lane: String,
    ordinal: u32,
    at: OffsetDateTime,
    path: String,
    file: String,
    added: HashSet<String>,
    removed: HashSet<String>,
}

/// Finds redos and collisions; returns each redo's first lane and when it
/// was redone, for destructive commands' harm.
fn redone_and_collisions(
    conn: &Connection,
    w: &World<'_>,
    since: &str,
    out: &mut Vec<Incident>,
) -> Result<Vec<(String, OffsetDateTime)>> {
    let from = fmt(ts(since) - REDO_WITHIN);
    let rows: Vec<EditRow> = {
        let mut stmt = conn.prepare(
            "SELECT session_id, ordinal, ts, path, file, added, removed FROM edits WHERE ts >= ?1 ORDER BY ts",
        )?;
        let rows = stmt.query_map([&from], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, u32>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
            ))
        })?;
        let mut v = Vec::new();
        for row in rows {
            let (session, ordinal, t, path, file, added, removed) = row?;
            let set = |s: &str| {
                s.split_whitespace()
                    .map(str::to_owned)
                    .collect::<HashSet<_>>()
            };
            v.push(EditRow {
                lane: w.lane(&session),
                session,
                ordinal,
                at: ts(&t),
                path,
                file,
                added: set(&added),
                removed: set(&removed),
            });
        }
        v
    };
    let since_t = ts(since);
    let mut by_file: HashMap<&str, Vec<&EditRow>> = HashMap::new();
    for e in &rows {
        by_file.entry(e.file.as_str()).or_default().push(e);
    }
    let span = |lane: &str, session: &str| -> Option<(OffsetDateTime, OffsetDateTime)> {
        let f = w.facts(session).or_else(|| w.facts(lane))?;
        Some((f.first_turn?, f.last_turn?))
    };
    let mut redos = Vec::new();
    // (lane A, lane B, file) → (A's added lines, B's removed lines that A
    // added, A's first edit, B's last edit).
    type Pair<'e> = (HashSet<String>, HashSet<String>, &'e EditRow, &'e EditRow);
    let mut pairs: HashMap<(String, String, String), Pair<'_>> = HashMap::new();
    let mut collided: HashMap<(String, String, String), (&EditRow, &EditRow)> = HashMap::new();
    for (file, edits) in &by_file {
        for (j, b) in edits.iter().enumerate() {
            for a in &edits[..j] {
                if a.lane == b.lane || w.delegated_between(&a.lane, &b.lane) || a.at >= b.at {
                    continue;
                }
                // Collision: both live at both times, edits close together.
                if a.path == b.path && b.at - a.at <= COLLIDE_WITHIN {
                    let live = |e: &EditRow, at: OffsetDateTime| {
                        span(&e.lane, &e.session).is_some_and(|(s, l)| s <= at && at <= l)
                    };
                    if live(a, b.at) && live(b, a.at) {
                        let key = (a.lane.clone(), b.lane.clone(), a.path.clone());
                        collided.entry(key).or_insert((a, b));
                    }
                }
                if b.at - a.at > REDO_WITHIN || b.removed.is_empty() || a.added.is_empty() {
                    continue;
                }
                let hit: HashSet<String> = a.added.intersection(&b.removed).cloned().collect();
                let key = (a.lane.clone(), b.lane.clone(), (*file).to_owned());
                let slot = pairs
                    .entry(key)
                    .or_insert_with(|| (HashSet::new(), HashSet::new(), a, b));
                slot.0.extend(a.added.iter().cloned());
                slot.1.extend(hit);
                if b.at > slot.3.at {
                    slot.3 = b;
                }
                if a.at < slot.2.at {
                    slot.2 = a;
                }
            }
        }
    }
    for ((lane_a, lane_b, file), (added, hit, a, b)) in pairs {
        let n = hit.len();
        let floor = n >= REDO_LINES || (added.len() >= REDO_HALF_FROM && n * 2 >= added.len());
        if !floor || b.at < since_t {
            continue;
        }
        let both_live = span(&lane_a, &a.session).is_some_and(|(s, l)| s <= b.at && b.at <= l)
            && span(&lane_b, &b.session).is_some_and(|(s, l)| s <= a.at && a.at <= l);
        let did = w.step(&a.session, a.ordinal).unwrap_or(Step {
            session: a.session.clone(),
            ordinal: a.ordinal,
            at: a.at,
            text: a.path.clone(),
        });
        let fixed = w.step(&b.session, b.ordinal).unwrap_or(Step {
            session: b.session.clone(),
            ordinal: b.ordinal,
            at: b.at,
            text: b.path.clone(),
        });
        let name = Path::new(&a.path)
            .file_name()
            .map_or_else(|| a.path.clone(), |n| n.to_string_lossy().into_owned());
        redos.push((lane_a.clone(), b.at));
        out.push(Incident {
            id: stable_id(&["redone", &lane_a, &lane_b, &file]),
            kind: Kind::Redone,
            quiet: false,
            at: b.at,
            cause: if both_live { Cause::Collided } else { Cause::NotEstablished },
            minutes: Some((b.at - a.at).whole_minutes()),
            lines: Some(n as i64),
            tokens: None,
            body: Body {
                chain: w.chain(&a.session),
                told: w.told(&a.session, a.ordinal),
                rule: None,
                read: w.read(&a.session, a.at),
                showed: Step {
                    session: b.session.clone(),
                    ordinal: b.ordinal,
                    at: b.at,
                    text: format!(
                        "{n} of the {} lines it added to {name} were removed by another session {} later",
                        added.len(),
                        human_span(b.at - a.at)
                    ),
                },
                did,
                fixed: Some(fixed),
                other: Some(w.chain(&b.session)),
                detail: Some(file.clone()),
                subject: None,
            },
        });
    }
    for ((lane_a, lane_b, path), (a, b)) in collided {
        if b.at < since_t {
            continue;
        }
        let name = Path::new(&path)
            .file_name()
            .map_or_else(|| path.clone(), |n| n.to_string_lossy().into_owned());
        let did = w.step(&a.session, a.ordinal).unwrap_or(Step {
            session: a.session.clone(),
            ordinal: a.ordinal,
            at: a.at,
            text: path.clone(),
        });
        let other = w.step(&b.session, b.ordinal).unwrap_or(Step {
            session: b.session.clone(),
            ordinal: b.ordinal,
            at: b.at,
            text: path.clone(),
        });
        out.push(Incident {
            id: stable_id(&["collision", &lane_a, &lane_b, &path]),
            kind: Kind::Collision,
            quiet: false,
            at: b.at,
            cause: Cause::Collided,
            minutes: None,
            lines: None,
            tokens: None,
            body: Body {
                chain: w.chain(&a.session),
                told: w.told(&a.session, a.ordinal),
                rule: None,
                read: w.read(&a.session, a.at),
                did,
                showed: Step {
                    text: format!(
                        "Another session edited {name} {} later while both were working",
                        human_span(b.at - a.at)
                    ),
                    ..other
                },
                fixed: None,
                other: Some(w.chain(&b.session)),
                detail: Some(path.clone()),
                subject: None,
            },
        });
    }
    Ok(redos)
}

/// `4m`, `3h 10m`, `2d 1h`.
pub fn human_span(d: Duration) -> String {
    let m = d.whole_minutes().max(0);
    if m < 60 {
        format!("{m}m")
    } else if m < 24 * 60 {
        match m % 60 {
            0 => format!("{}h", m / 60),
            r => format!("{}h {r}m", m / 60),
        }
    } else {
        format!("{}d {}h", m / 1440, (m % 1440) / 60)
    }
}

/// Reads stored incidents since `since`, newest first.
pub(crate) fn read(conn: &Connection, since: OffsetDateTime) -> Result<Vec<Incident>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, kind, quiet, at, cause, minutes, lines, tokens, body FROM incidents WHERE at >= ?1 ORDER BY at DESC",
    )?;
    let rows = stmt.query_map([fmt(since)], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, bool>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, Option<i64>>(5)?,
            r.get::<_, Option<i64>>(6)?,
            r.get::<_, Option<i64>>(7)?,
            r.get::<_, String>(8)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, kind, quiet, at, cause, minutes, lines, tokens, body) = row?;
        let (Some(kind), Some(cause), Ok(body)) = (
            Kind::parse(&kind),
            Cause::parse(&cause),
            serde_json::from_str::<Body>(&body),
        ) else {
            continue;
        };
        out.push(Incident {
            id,
            kind,
            quiet,
            at: ts(&at),
            cause,
            minutes,
            lines,
            tokens,
            body,
        });
    }
    Ok(out)
}

/// Whether the incidents table holds any row (a rollup just rebuilt has
/// none until [`rebuild`] runs).
pub(crate) fn has_rows(conn: &Connection) -> Result<bool> {
    Ok(conn
        .query_row("SELECT 1 FROM incidents LIMIT 1", [], |_| Ok(()))
        .optional()?
        .is_some())
}

#[cfg(test)]
mod tests;
