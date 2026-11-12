//! Fleet: today's sessions by state, headroom per profile, and how to
//! resume a stopped one.
//!
//! Read-only. A session's state comes from its transcript's last turn
//! (live: a turn in the last [`LIVE_FOR`]) and the rollup's ending; a
//! session the rollup has not read yet is listed from its transcript head.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use hippo_core::adapters::dsh;
use hippo_core::index::Index;
use hippo_core::resume::{self, Target};
use hippo_core::rollup::{self, Ending, Rollup, SessionFacts};
use hippo_core::session_meta::{self, Origin};
use hippo_core::usage::{self, Headroom, Snapshot};
use serde::Serialize;
use time::format_description::well_known::Rfc3339;
use time::{Duration, OffsetDateTime, UtcOffset};

use crate::home;

pub mod tail;

/// A session with a turn this recent is live.
pub const LIVE_FOR: Duration = Duration::minutes(2);

/// How far from its end a transcript is read for its last turn.
const TAIL_BYTES: u64 = 256 * 1024;

/// A session's state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum State {
    /// A turn in the last [`LIVE_FOR`].
    Live,
    /// Its last reply is the usage-limit message.
    Stopped {
        /// `resets 6:10pm`, as the message says.
        reset: Option<String>,
        #[serde(with = "time::serde::rfc3339")]
        at: OffsetDateTime,
        /// A later session carried its work on (the Home's rule).
        carried_on: bool,
    },
    /// Stopped by the person, an API error, a rate limit, a sign-in
    /// failure, or quiet mid-step for half an hour.
    Interrupted {
        detail: Option<String>,
        #[serde(with = "time::serde::rfc3339")]
        at: OffsetDateTime,
    },
    /// Its last word is a reply.
    Finished {
        #[serde(with = "time::serde::rfc3339")]
        at: OffsetDateTime,
    },
    /// Not live, and the rollup has not read its newest turns (or it is
    /// mid-step and under half an hour quiet).
    Quiet {
        #[serde(with = "time::serde::rfc3339::option")]
        since: Option<OffsetDateTime>,
    },
    /// It stopped to ask the person: its last turn is a question
    /// or an unanswered `AskUserQuestion` or `ExitPlanMode` call.
    Waiting {
        #[serde(with = "time::serde::rfc3339")]
        at: OffsetDateTime,
        asked: Option<String>,
        /// A plan to approve rather than a question.
        plan: bool,
    },
}

impl State {
    pub fn is_live(&self) -> bool {
        matches!(self, State::Live)
    }

    pub fn is_waiting(&self) -> bool {
        matches!(self, State::Waiting { .. })
    }

    /// Stopped at a usage limit, and nothing carried it on.
    pub fn needs_resume(&self) -> bool {
        matches!(
            self,
            State::Stopped {
                carried_on: false,
                ..
            }
        )
    }
}

/// One session of the day.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Row {
    pub id: String,
    pub title: String,
    /// `claude-code`, `codex` or `dsh`.
    pub tool: String,
    pub profile: Option<String>,
    pub project: Option<String>,
    /// `T3 Code` or `terminal`, when the transcript says.
    pub client: Option<&'static str>,
    pub cwd: Option<PathBuf>,
    pub transcript: PathBuf,
    #[serde(with = "time::serde::rfc3339::option")]
    pub started: Option<OffsetDateTime>,
    /// Its newest turn, its sub-agents' included.
    #[serde(with = "time::serde::rfc3339::option")]
    pub last: Option<OffsetDateTime>,
    /// `None` until the rollup has read it.
    pub turns: Option<u64>,
    pub agents: Option<u64>,
    pub edits: u64,
    #[serde(flatten)]
    pub state: State,
    /// Pull requests it opened or merged: `(url, "opened" | "merged")`.
    pub pulls: Vec<(String, &'static str)>,
    /// Its claims and their evidence, as the rollup checked them
    ///empty until the rollup has read it.
    pub claims: Claims,
    /// For a live session, or a quiet one mid-call, what it is doing:
    /// its newest tool call and the call's age (`Run the fleet tests ·
    /// 4m`, `editing memory.rs · now`, `3 sub-agents working · 2m`), or
    /// `last: cargo test · 6m` once the call returned and nothing followed
    /// for 2 minutes.
    pub activity: Option<String>,
    /// The activity is a `last:` line (drawn muted).
    pub activity_last: bool,
    /// The newest call as a tool line, for a live session or a
    /// quiet one mid-call.
    #[serde(skip)]
    pub call: Option<tail::Activity>,
    /// The current turn: the newest request and the calls since,
    /// for a live session or a quiet one mid-call.
    #[serde(skip)]
    pub turn: Option<tail::Turn>,
    /// Its edit calls' lines per file, for the x-ray.
    #[serde(skip)]
    pub changed: Vec<rollup::Changed>,
    /// The branch it worked on.
    pub branch: Option<String>,
    /// Lines its edit calls changed; `None` until the
    /// rollup has read it or when it edited nothing.
    pub lines: Option<LineTotals>,
    /// Its sub-agents and background commands.
    pub tasks: Vec<SubTask>,
    /// Pull requests it opened that another session merged.
    pub merged_elsewhere: Vec<String>,
    /// What the sessions below it did: its sub-agents', workflow
    /// agents' and delegated sessions', nested ones included.
    pub sub: SubWork,
    /// Its sub-agents writing now, newest first, each with its current
    /// step.
    pub live_subs: Vec<LiveSub>,
    /// The session whose T3 Code delegated task this one is.
    pub delegated_by: Option<String>,
    /// The first request and the last reply, for a handoff.
    #[serde(skip)]
    pub asked: Option<String>,
    #[serde(skip)]
    pub outcome: Option<String>,
}

/// What the sessions below a session did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SubWork {
    /// How many sessions are below it.
    pub sessions: usize,
    /// Of them, how many changed a file.
    pub editing: usize,
    /// Their edit calls' lines, files counted once; `None` when none
    /// edited.
    pub lines: Option<LineTotals>,
    /// Tokens their results' usage blocks count, summed; `None` when none
    /// says.
    pub tokens: Option<u64>,
    /// Each file they changed, with the session that changed it.
    #[serde(skip)]
    pub changed: Vec<(String, rollup::Changed)>,
}

/// The longest a sub-task with no recorded end is called running.
pub const RUNNING_FOR: Duration = Duration::hours(6);

/// [`SubTask::settle`] for each task whose own session the rollup holds;
/// a delegated task whose session has ended ends with it.
pub fn settle_all(tasks: &mut [SubTask], rollup: &Rollup) {
    let now = OffsetDateTime::now_utc();
    for t in tasks.iter_mut().filter(|t| t.ended.is_none()) {
        let Some(c) = t.child.as_deref() else {
            continue;
        };
        let Some(f) = rollup.session(c).ok().flatten() else {
            continue;
        };
        if t.kind == "delegate" {
            let quiet = last_turn_at(&f.transcript_path)
                .or(f.last_turn)
                .is_some_and(|l| now - l > LIVE_FOR);
            let status = match f.ending(now) {
                Ending::Completed => Some("completed"),
                Ending::Interrupted | Ending::RateLimited | Ending::UsageLimit => Some("stopped"),
                Ending::Open => None,
            };
            if let (true, Some(s)) = (quiet, status) {
                t.ended = f.ending_at.or(f.last_turn);
                t.status = Some(s.to_owned());
            }
        } else {
            t.settle(&f.transcript_path);
        }
    }
}

impl SubWork {
    /// The sessions below `id` in `rollup`, and what they changed.
    pub fn of(rollup: &Rollup, id: &str, own_tasks: &[SubTask]) -> SubWork {
        let below = rollup.descendants_of(id).unwrap_or_default();
        let mut out = SubWork {
            sessions: below.len(),
            ..SubWork::default()
        };
        let mut tokens: Option<u64> = own_tasks
            .iter()
            .filter_map(|t| t.tokens)
            .reduce(|a, b| a + b);
        for d in &below {
            let changed = rollup.changes_of(d).unwrap_or_default();
            out.editing += usize::from(!changed.is_empty());
            out.changed
                .extend(changed.into_iter().map(|c| (d.clone(), c)));
            for t in rollup.tasks_of(d).unwrap_or_default() {
                if let Some(n) = t.tokens {
                    tokens = Some(tokens.unwrap_or(0) + n);
                }
            }
        }
        out.tokens = tokens;
        out.lines = lines_once(out.changed.iter().map(|(_, c)| c), &[]);
        out
    }
}

/// Lines summed over `changes` and `more`, each path counted once among
/// the files.
pub fn lines_once<'a>(
    changes: impl IntoIterator<Item = &'a rollup::Changed>,
    more: &'a [rollup::Changed],
) -> Option<LineTotals> {
    let mut t = LineTotals::default();
    let mut paths: HashSet<&str> = HashSet::new();
    let mut any = false;
    for c in changes.into_iter().chain(more.iter()) {
        any = true;
        t.added += c.lines.added;
        t.removed += c.lines.removed;
        t.uncounted += c.uncounted;
        paths.insert(c.path.as_str());
    }
    t.files = paths.len() as u64;
    any.then_some(t)
}

impl Row {
    /// Its own lines and its sub-agents', files counted once.
    pub fn total_lines(&self) -> Option<LineTotals> {
        if self.sub.changed.is_empty() {
            return self.lines;
        }
        lines_once(self.sub.changed.iter().map(|(_, c)| c), &self.changed)
    }
}

/// A sub-agent writing now, with its current step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LiveSub {
    /// Its description, when its client wrote one.
    pub title: Option<String>,
    /// Its newest call as a line: `Run the tests`, `editing memory.rs`.
    pub step: String,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
}

/// A session's diff totals, summed from its edit calls (not a `git diff`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct LineTotals {
    pub added: u64,
    pub removed: u64,
    pub files: u64,
    /// Edit calls whose lines are not known (a file replaced by `Write`).
    pub uncounted: u64,
}

impl LineTotals {
    /// `+429 −215`.
    pub fn counts(&self) -> String {
        format!("+{} −{}", self.added, self.removed)
    }

    /// `4 files, from its edits · 1 rewrite not counted`.
    pub fn note(&self) -> String {
        let mut out = format!(
            "{} file{}, from its edits",
            self.files,
            if self.files == 1 { "" } else { "s" }
        );
        if self.uncounted > 0 {
            out.push_str(&format!(
                " · {} rewrite{} not counted",
                self.uncounted,
                if self.uncounted == 1 { "" } else { "s" }
            ));
        }
        out
    }

    fn of(changes: &[rollup::Changed]) -> Option<LineTotals> {
        if changes.is_empty() {
            return None;
        }
        let mut t = LineTotals {
            files: changes.len() as u64,
            ..LineTotals::default()
        };
        for c in changes {
            t.added += c.lines.added;
            t.removed += c.lines.removed;
            t.uncounted += c.uncounted;
        }
        Some(t)
    }
}

/// A sub-agent or background command a session started.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SubTask {
    /// `agent`, `bash` or `delegate`.
    pub kind: &'static str,
    pub title: String,
    #[serde(with = "time::serde::rfc3339::option")]
    pub started: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub ended: Option<OffsetDateTime>,
    /// `completed`, `failed`, `killed` or `stopped`; `None` while it runs.
    pub status: Option<String>,
    /// The session of its own transcript, when the rollup knows it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub child: Option<String>,
    /// What it was given and what it gave back.
    #[serde(skip)]
    pub input: Option<String>,
    #[serde(skip)]
    pub report: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit: Option<i64>,
}

impl SubTask {
    pub fn of(t: &rollup::Task) -> SubTask {
        let ts = |s: &str| OffsetDateTime::parse(s, &Rfc3339).ok();
        SubTask {
            kind: t.kind.as_str(),
            title: t.title.clone(),
            started: ts(&t.started),
            ended: t.ended.as_deref().and_then(ts),
            status: t.status.clone(),
            child: t.child.clone(),
            input: t.input.clone(),
            report: t.report.clone(),
            tokens: t.tokens,
            tools: t.tools,
            exit: t.exit,
        }
    }

    /// A sub-agent the rollup saw start but not end, whose own transcript
    /// is at `child`: Claude Code's `.meta.json` beside it says when it
    /// finished, for a client that tells the parent in a record the index
    /// does not hold. It ends at its transcript's newest turn.
    pub fn settle(&mut self, child: &Path) {
        if self.ended.is_some() || self.kind != "agent" {
            return;
        }
        if session_meta::sub_agent_meta(child).finished == Some(true) {
            self.ended = last_turn_at(child).or(self.started);
            self.status = Some("completed".to_owned());
        }
    }

    /// Still running: no end, started in the last [`RUNNING_FOR`], and its
    /// session is live or mid-step. A sub-task whose end the transcript
    /// never recorded (its client told the parent in a record the index
    /// does not hold) is not called running for days.
    pub fn running(&self, session: &State) -> bool {
        self.ended.is_none()
            && self
                .started
                .is_none_or(|s| OffsetDateTime::now_utc() - s <= RUNNING_FOR)
            && matches!(
                session,
                State::Live | State::Quiet { .. } | State::Waiting { .. }
            )
    }
}

/// The sidebar shelf a session sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Shelf {
    NeedsYou,
    Working,
    ToReview,
    Settled,
}

/// Which shelf `row` sits on. `opened` says whether the person has opened
/// a session in Hippo. Needs you: waiting, or stopped at a limit nothing
/// carried on. An unverified claim is a hint to check, shown on the row; it
/// does not put a session in Needs you. Working: live or
/// quiet mid-step. Settled: finished and every pull request it opened is
/// merged (by it or another session), or it opened none and the person
/// has opened it. To review: the rest.
pub fn shelf(row: &Row, opened: impl Fn(&str) -> bool) -> Shelf {
    if row.state.is_waiting() || row.state.needs_resume() {
        return Shelf::NeedsYou;
    }
    if matches!(row.state, State::Live | State::Quiet { .. }) {
        return Shelf::Working;
    }
    let mine: Vec<&str> = row
        .pulls
        .iter()
        .filter(|(_, d)| *d == "opened")
        .map(|(u, _)| u.as_str())
        .collect();
    let merged: HashSet<&str> = row
        .pulls
        .iter()
        .filter(|(_, d)| *d == "merged")
        .map(|(u, _)| u.as_str())
        .chain(row.merged_elsewhere.iter().map(String::as_str))
        .collect();
    let settled = if mine.is_empty() {
        opened(&row.id)
    } else {
        mine.iter().all(|u| merged.contains(u))
    };
    if settled {
        Shelf::Settled
    } else {
        Shelf::ToReview
    }
}

/// A session's claims, counted, with the first unverified one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Claims {
    pub verified: u32,
    pub unverified: u32,
    pub unclear: u32,
    /// The first unverified claim's words and why.
    pub first_unverified: Option<(String, String)>,
}

impl Claims {
    pub fn of(claims: &[hippo_core::xray::Claim]) -> Claims {
        use hippo_core::xray::Status;
        let mut out = Claims::default();
        for c in claims {
            match c.status {
                Status::Verified => out.verified += 1,
                Status::Unclear => out.unclear += 1,
                Status::Unverified => {
                    out.unverified += 1;
                    if out.first_unverified.is_none() {
                        out.first_unverified = Some((c.claim.clone(), c.evidence.clone()));
                    }
                }
            }
        }
        out
    }

    /// The line without the reason: `1 claim unverified: "deployed"`, for
    /// narrow places (Home's Fleet panel).
    pub fn short_line(&self) -> Option<String> {
        let line = self.line()?;
        Some(match line.split_once(" · ") {
            Some((head, _)) if self.first_unverified.is_some() => head.to_owned(),
            _ => line,
        })
    }

    pub fn total(&self) -> u32 {
        self.verified + self.unverified + self.unclear
    }

    /// `1 claim unverified: "deployed" · no deploy command in the
    /// transcript…`, or `3 claims verified`, or nothing.
    pub fn line(&self) -> Option<String> {
        if let Some((claim, why)) = &self.first_unverified {
            let n = self.unverified;
            return Some(format!(
                "{n} claim{} unverified: \"{}\" · {}",
                if n == 1 { "" } else { "s" },
                short(claim, 60),
                short(why.split(';').next().unwrap_or(why), 60)
            ));
        }
        (self.verified > 0).then(|| {
            format!(
                "{} claim{} verified",
                self.verified,
                if self.verified == 1 { "" } else { "s" }
            )
        })
    }
}

fn short(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut s: String = text.chars().take(max - 1).collect();
    s.push('…');
    s
}

impl Row {
    /// Finished or stopped with an unverified claim: Fleet's `check` state.
    pub fn needs_check(&self) -> bool {
        self.claims.unverified > 0 && !self.state.is_live()
    }
}

/// Whether a request is only a slash command (`/login`, `/compact`).
pub fn is_slash_command(asked: &str) -> bool {
    asked.starts_with('/') && !asked.trim().contains(char::is_whitespace)
}

/// Whether a session's only input was local slash commands (`/login`),
/// with no tool call and no reply: not agent work.
pub fn command_only(f: &SessionFacts) -> bool {
    f.tool_calls == 0 && f.outcome.is_none() && f.asked.as_deref().is_some_and(is_slash_command)
}

/// Today's fleet.
#[derive(Debug, Clone, Serialize)]
pub struct Fleet {
    pub rows: Vec<Row>,
    #[serde(skip)]
    pub headroom: Vec<Headroom>,
    /// The Claude profile with the most headroom, leaving out those in
    /// `signed_out`.
    pub use_next: Option<String>,
    /// Profiles whose newest session in the last 3 days ended because its
    /// sign-in failed.
    pub signed_out: Vec<String>,
    /// Whether the usage cache was there; without it no limit is learned.
    pub usage_cached: bool,
}

impl Fleet {
    pub fn live(&self) -> impl Iterator<Item = &Row> {
        self.rows.iter().filter(|r| r.state.is_live())
    }

    pub fn waiting(&self) -> impl Iterator<Item = &Row> {
        self.rows.iter().filter(|r| r.state.is_waiting())
    }

    /// What needs the person, in the order Home lists it: waiting
    /// sessions (longest wait first), then those stopped at a usage limit
    /// that nothing carried on, then finished ones with an unverified claim.
    pub fn needs_you(&self) -> Vec<Need<'_>> {
        let mut waiting: Vec<&Row> = self.waiting().collect();
        waiting.sort_by_key(|r| match r.state {
            State::Waiting { at, .. } => at,
            _ => OffsetDateTime::UNIX_EPOCH,
        });
        let mut out: Vec<Need<'_>> = waiting
            .into_iter()
            .map(|row| Need {
                row,
                why: Why::Waiting,
            })
            .collect();
        out.extend(self.stopped().map(|row| Need {
            row,
            why: Why::Stopped,
        }));
        out
    }

    /// Sessions working now: live or waiting.
    pub fn working(&self) -> usize {
        self.rows.iter().filter(|r| r.state.is_live()).count()
    }

    pub fn stopped(&self) -> impl Iterator<Item = &Row> {
        self.rows.iter().filter(|r| r.state.needs_resume())
    }

    /// Neither live, waiting on the person, nor waiting to be resumed.
    pub fn others(&self) -> impl Iterator<Item = &Row> {
        self.rows
            .iter()
            .filter(|r| !r.state.is_live() && !r.state.is_waiting() && !r.state.needs_resume())
    }

    pub fn row(&self, id: &str) -> Option<&Row> {
        self.rows.iter().find(|r| r.id == id)
    }

    /// A profile's headroom.
    pub fn room(&self, profile: &str) -> Option<&Headroom> {
        self.headroom.iter().find(|h| h.profile == profile)
    }

    /// `(live, today)` session counts of `profile`.
    pub fn counts(&self, profile: &str) -> (usize, usize) {
        let mine = self
            .rows
            .iter()
            .filter(|r| r.profile.as_deref() == Some(profile));
        let (mut live, mut all) = (0, 0);
        for r in mine {
            all += 1;
            live += usize::from(r.state.is_live());
        }
        (live, all)
    }
}

/// Why a session needs the person.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    Waiting,
    Stopped,
    Check,
}

/// One row of Home's Needs you list.
#[derive(Debug, Clone, Copy)]
pub struct Need<'a> {
    pub row: &'a Row,
    pub why: Why,
}

/// What [`build`] reads.
pub struct Inputs<'a> {
    pub now: OffsetDateTime,
    pub offset: UtcOffset,
    /// The configuration's transcript roots, globs matched.
    pub roots: &'a [PathBuf],
    pub rollup: Option<&'a Rollup>,
    pub usage: &'a Snapshot,
}

/// A transcript's place: the top-level session it belongs to.
fn session_of(path: &Path) -> Option<(String, &'static str, bool)> {
    if dsh::is_log(path) {
        let h = dsh::header(path)?;
        return Some(match dsh::parent_id(path, &h) {
            Some(parent) => (parent, "dsh", false),
            None => (dsh::session_id(path, &h), "dsh", true),
        });
    }
    let stem = path.file_stem()?.to_str()?;
    if stem.starts_with("rollout-") {
        let uuid = stem.get(stem.len().checked_sub(36)?..)?;
        return Some((format!("codex-{uuid}"), "codex", true));
    }
    let parent = path.parent()?;
    if parent.file_name()?.to_str()? == "subagents" {
        let root = parent.parent()?.file_name()?.to_str()?;
        return Some((root.to_owned(), "claude-code", false));
    }
    if parent.parent()?.file_name()?.to_str()? == "projects" {
        return Some((stem.to_owned(), "claude-code", true));
    }
    None
}

/// The time of the newest turn in a transcript's last [`TAIL_BYTES`]: a
/// Claude Code `user` or `assistant` record, or a Codex `response_item`;
/// for a dsh log, its newest message, call or result.
/// Title and summary records a client writes later do not count.
pub fn last_turn_at(path: &Path) -> Option<OffsetDateTime> {
    if dsh::is_log(path) {
        return dsh::last_turn_at(path);
    }
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let from = len.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(from)).ok()?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    let mut lines: Vec<&str> = text.lines().collect();
    if from > 0 && !lines.is_empty() {
        lines.remove(0);
    }
    for line in lines.iter().rev() {
        let turn = line.contains("\"type\":\"user\"")
            || line.contains("\"type\":\"assistant\"")
            || line.contains("\"type\":\"response_item\"");
        if !turn {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let kind = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if !matches!(kind, "user" | "assistant" | "response_item") {
            continue;
        }
        if let Some(ts) = v
            .get("timestamp")
            .and_then(|t| t.as_str())
            .and_then(|t| OffsetDateTime::parse(t, &Rfc3339).ok())
        {
            return Some(ts);
        }
    }
    None
}

struct Found {
    tool: &'static str,
    /// The session's own transcript, when found.
    path: Option<PathBuf>,
    /// Its own and its sub-agents' files modified today.
    files: Vec<PathBuf>,
}

/// Today's fleet at `inp.now`.
pub fn build(inp: &Inputs<'_>) -> Fleet {
    let today = inp.now.to_offset(inp.offset).date();
    let (start, end) = rollup::day_range(today, inp.offset);
    let end = end.max(inp.now + Duration::minutes(1));

    // Transcripts written today, by top-level session.
    let mut found: BTreeMap<String, Found> = BTreeMap::new();
    let today_files = usage::transcripts(inp.roots, start.unix_timestamp())
        .into_iter()
        .map(|t| t.path)
        .chain(dsh::logs_since(inp.roots, start.unix_timestamp()));
    for path in today_files {
        let Some((id, tool, own)) = session_of(&path) else {
            continue;
        };
        let f = found.entry(id).or_insert(Found {
            tool,
            path: None,
            files: Vec::new(),
        });
        if own {
            f.path = Some(path.clone());
        }
        f.files.push(path);
    }

    // The rollup's sessions of the day, and the last week's for the
    // carried-on rule.
    let rolled: Vec<rollup::InRange> = inp
        .rollup
        .and_then(|r| r.sessions_in(start, end).ok())
        .unwrap_or_default();
    let recent: Vec<rollup::InRange> = inp
        .rollup
        .and_then(|r| r.sessions_in(inp.now - Duration::days(7), end).ok())
        .unwrap_or_default();
    let pulls_recent = inp
        .rollup
        .map(|r| home::pulls_by_session(r, &recent))
        .unwrap_or_default();

    let mut facts: HashMap<String, SessionFacts> = HashMap::new();
    let mut skip: Vec<String> = Vec::new();
    for s in &rolled {
        if s.facts.parent.is_none() && !s.facts.automated {
            if command_only(&s.facts) {
                skip.push(s.facts.id.clone());
                continue;
            }
            facts.insert(s.facts.id.clone(), s.facts.clone());
        }
    }
    for id in found.keys() {
        if !facts.contains_key(id) {
            if let Some(f) = inp.rollup.and_then(|r| r.session(id).ok().flatten()) {
                if command_only(&f) {
                    skip.push(id.clone());
                    continue;
                }
                if f.parent.is_none() && !f.automated {
                    facts.insert(id.clone(), f);
                }
            }
        }
    }

    let mut ids: Vec<String> = facts.keys().cloned().collect();
    for id in found.keys() {
        if !facts.contains_key(id) && !skip.contains(id) {
            ids.push(id.clone());
        }
    }

    let delegations: HashMap<String, String> = inp
        .rollup
        .and_then(|r| r.delegations().ok())
        .unwrap_or_default();
    let mut rows = Vec::new();
    for id in ids {
        let f = facts.get(&id);
        let fd = found.get(&id);
        let tool = f.map_or_else(
            || fd.map_or("claude-code", |d| d.tool).to_owned(),
            |f| f.tool.clone(),
        );
        let Some(path) = f
            .map(|f| f.transcript_path.clone())
            .or_else(|| fd.and_then(|d| d.path.clone()))
        else {
            continue;
        };
        let origin: Origin = session_meta::origin(&id, &tool, &path);
        if f.is_none() && (origin.automated || origin.parent.is_some()) {
            continue;
        }
        // The newest turn, from the files themselves: the session's own
        // transcript's tail (its question and newest tool call),
        // and its sub-agents' newest turns.
        let own = tail::read(&path);
        let mut last_file: Option<OffsetDateTime> = own.last;
        let mut newest_sub: Option<OffsetDateTime> = None;
        let mut subs_live = 0usize;
        let files: Vec<PathBuf> = match fd {
            Some(d) => d.files.clone(),
            None => vec![path.clone()],
        };
        let mut live_subs: Vec<LiveSub> = Vec::new();
        for p in files.iter().filter(|p| **p != path) {
            if let Some(t) = last_turn_at(p) {
                last_file = Some(last_file.map_or(t, |l: OffsetDateTime| l.max(t)));
                newest_sub = Some(newest_sub.map_or(t, |l: OffsetDateTime| l.max(t)));
                let live = inp.now - t <= LIVE_FOR;
                subs_live += usize::from(live);
                if live {
                    let sub = tail::read(p);
                    if let Some(a) = sub.activity {
                        live_subs.push(LiveSub {
                            title: session_meta::sub_agent_meta(p).description,
                            step: a.label,
                            at: a.at,
                        });
                    }
                }
            }
        }
        live_subs.sort_by_key(|l| std::cmp::Reverse(l.at));
        let last = match (f.and_then(|f| f.last_turn), last_file) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
        let live = last_file.is_some_and(|t| inp.now - t <= LIVE_FOR);
        let stale = match (f.and_then(|f| f.last_turn), last_file) {
            (Some(a), Some(b)) => b - a > Duration::seconds(90),
            (None, _) => true,
            _ => false,
        };
        // Waiting: the question is the newest turn of the session and of
        // its sub-agents, and the session is not stopped at a limit.
        let limited = f.is_some_and(|f| f.ending(inp.now) == Ending::UsageLimit) && !stale;
        let waiting = own
            .waiting
            .clone()
            .filter(|w| newest_sub.is_none_or(|t| t <= w.at) && !limited);
        let state = if let Some(w) = waiting {
            State::Waiting {
                at: w.at,
                asked: w.asked,
                plan: w.plan,
            }
        } else if live {
            State::Live
        } else if stale || f.is_none() {
            State::Quiet { since: last }
        } else {
            let f = f.expect("checked");
            let at = f.ending_at.or(f.last_turn).unwrap_or(inp.now);
            match f.ending(inp.now) {
                Ending::UsageLimit => State::Stopped {
                    reset: f.ending_detail.as_deref().map(crate::text::twenty_four),
                    at,
                    carried_on: home::carried_on(f, at, &recent, &pulls_recent, inp.now),
                },
                Ending::RateLimited => State::Interrupted {
                    detail: Some("rate-limited".into()),
                    at,
                },
                Ending::Interrupted => State::Interrupted {
                    detail: f.ending_detail.clone(),
                    at,
                },
                Ending::Completed => State::Finished { at },
                Ending::Open => State::Quiet { since: f.last_turn },
            }
        };
        let title = match f {
            Some(f) => home::title(f),
            None => origin
                .named
                .clone()
                .unwrap_or_else(|| id.chars().take(8).collect()),
        };
        let pulls: Vec<(String, &'static str)> = inp
            .rollup
            .and_then(|r| r.pulls_of(&id).ok())
            .unwrap_or_default()
            .into_iter()
            .map(|p| (p.url, p.done.as_str()))
            .collect();
        let merged_elsewhere: Vec<String> = pulls
            .iter()
            .filter(|(u, d)| {
                *d == "opened"
                    && !pulls.iter().any(|(v, e)| v == u && *e == "merged")
                    && inp
                        .rollup
                        .is_some_and(|r| r.merged_by_any(u).unwrap_or(false))
            })
            .map(|(u, _)| u.clone())
            .collect();
        let rolled_here = inp.rollup.filter(|_| f.is_some());
        let changed = rolled_here
            .and_then(|r| r.changes_of(&id).ok())
            .unwrap_or_default();
        let lines = LineTotals::of(&changed);
        let mut tasks: Vec<SubTask> = rolled_here
            .and_then(|r| r.tasks_of(&id).ok())
            .unwrap_or_default()
            .iter()
            .map(SubTask::of)
            .collect();
        if let Some(r) = rolled_here {
            settle_all(&mut tasks, r);
        }
        let sub = rolled_here
            .map(|r| SubWork::of(r, &id, &tasks))
            .unwrap_or_default();
        // A quiet session mid-call (a long build writes no turn)
        // says what it runs too.
        let mid_call = matches!(state, State::Quiet { .. })
            && own.activity.as_ref().is_some_and(|a| a.running);
        let (activity, activity_last) = if state.is_live() || mid_call {
            match activity_line(&own, subs_live, inp.now) {
                Some((line, last)) => (Some(line), last),
                None => (None, false),
            }
        } else {
            (None, false)
        };
        let call = (state.is_live() || mid_call)
            .then(|| own.activity.clone())
            .flatten();
        let turn = (state.is_live() || mid_call).then(|| tail::read_turn(&path));
        rows.push(Row {
            activity,
            activity_last,
            call,
            turn,
            changed,
            branch: f
                .and_then(|f| f.branch.clone())
                .or_else(|| origin.branch.clone()),
            lines,
            tasks,
            merged_elsewhere,
            sub,
            live_subs,
            delegated_by: delegations.get(&id).cloned(),
            title,
            profile: rollup::profile_of(&tool, &path),
            project: f
                .and_then(|f| f.project.clone())
                .or_else(|| origin.project()),
            client: origin.client(),
            cwd: origin.cwd.clone(),
            started: f.and_then(|f| f.first_turn),
            last,
            turns: f.map(|f| f.turns),
            agents: f.map(|f| f.agents),
            edits: f.map_or(0, |f| f.edits),
            state,
            pulls,
            claims: inp
                .rollup
                .filter(|_| f.is_some())
                .and_then(|r| r.claims_of(&id).ok())
                .map(|c| Claims::of(&c))
                .unwrap_or_default(),
            asked: f.and_then(|f| f.asked.clone()),
            outcome: f.and_then(|f| f.outcome.clone()),
            transcript: path,
            tool,
            id,
        });
    }
    // A delegated session writing now is its parent's sub-agent too.
    let live_children: Vec<(String, LiveSub)> = rows
        .iter()
        .filter(|r| r.state.is_live())
        .filter_map(|r| {
            let parent = r.delegated_by.clone()?;
            let a = r.call.as_ref()?;
            Some((
                parent,
                LiveSub {
                    title: Some(r.title.clone()),
                    step: a.label.clone(),
                    at: a.at,
                },
            ))
        })
        .collect();
    for (parent, live) in live_children {
        if let Some(p) = rows.iter_mut().find(|r| r.id == parent) {
            p.live_subs.push(live);
            p.live_subs.sort_by_key(|l| std::cmp::Reverse(l.at));
        }
    }
    rows.sort_by(|a, b| {
        let rank = |r: &Row| match r.state {
            State::Waiting { .. } => 0,
            State::Live => 1,
            State::Stopped {
                carried_on: false, ..
            } => 2,
            _ => 3,
        };
        rank(a)
            .cmp(&rank(b))
            .then_with(|| b.last.cmp(&a.last))
            .then_with(|| a.id.cmp(&b.id))
    });

    let headroom = usage::headroom(inp.usage, inp.now);
    let signed_out = inp
        .rollup
        .map(|r| signed_out(r, inp.now))
        .unwrap_or_default();
    let use_next =
        use_next(&headroom, &signed_out, inp.now.unix_timestamp()).map(|h| h.profile.clone());
    Fleet {
        rows,
        headroom,
        use_next,
        signed_out,
        usage_cached: inp.usage.cached,
    }
}

/// A session's live line from its transcript's tail:
/// `3 sub-agents working` while it delegates and sub-agents write, else its
/// newest tool call; then the call's age (`· now`, `· 4m`). When the call
/// has returned and nothing was written for [`LIVE_FOR`], the line is
/// `last: <label> · <age>` and the flag is set (the app mutes it).
pub fn activity_line(
    own: &tail::Tail,
    subs_live: usize,
    now: OffsetDateTime,
) -> Option<(String, bool)> {
    let age = |at: OffsetDateTime| {
        let d = now - at;
        if d < Duration::minutes(1) {
            "now".to_owned()
        } else {
            span(d.whole_seconds())
        }
    };
    if subs_live > 0 && own.activity.as_ref().is_none_or(|a| a.delegates) {
        let mut line = format!(
            "{subs_live} sub-agent{} working",
            if subs_live == 1 { "" } else { "s" }
        );
        if let Some(a) = &own.activity {
            line.push_str(&format!(" · {}", age(a.at)));
        }
        return Some((line, false));
    }
    let a = own.activity.as_ref()?;
    let quiet = own.last.is_none_or(|l| now - l >= LIVE_FOR);
    if !a.running && quiet {
        return Some((format!("last: {} · {}", a.label, age(a.at)), true));
    }
    Some((format!("{} · {}", a.label, age(a.at)), false))
}

/// The second line of a live or mid-call row on Home and in Fleet:
/// `<live line> · running 33m · <profile>`.
pub fn live_facts(row: &Row, now: OffsetDateTime) -> String {
    let mut f: Vec<String> = Vec::new();
    f.extend(row.activity.clone());
    if let Some(s) = row.started {
        f.push(format!("running {}", span((now - s).whole_seconds())));
    }
    f.extend(row.profile.clone());
    f.join(" · ")
}

/// A duration in the app's one form: `45s` under a minute, then
/// `33m`, `1h 4m`, `2d 3h`.
pub fn span(secs: i64) -> String {
    let secs = secs.max(0);
    let m = secs / 60;
    if m < 1 {
        format!("{secs}s")
    } else if m < 60 {
        format!("{m}m")
    } else if m < 48 * 60 {
        match m % 60 {
            0 => format!("{}h", m / 60),
            r => format!("{}h {r}m", m / 60),
        }
    } else {
        match (m / 60) % 24 {
            0 => format!("{}d", m / 1440),
            h => format!("{}d {h}h", m / 1440),
        }
    }
}

/// A meter fetched longer ago than this reads `meter n days old`.
pub const METER_OLD: Duration = Duration::hours(24);

/// Whether a window's figure is inferred: the meter named a reset
/// that has passed and was not fetched since, so `100%` is a guess.
pub fn inferred(l: &usage::Left) -> bool {
    l.reset_passed.is_some()
}

/// The meter's age in seconds when it is older than [`METER_OLD`].
pub fn meter_old(h: &Headroom, now: i64) -> Option<i64> {
    let m = h.meter?;
    let age = now - m.fetched_at;
    (age > METER_OLD.whole_seconds()).then_some(age)
}

/// Whether every figure on a profile's card is current: metered, no
/// inferred window, the meter at most [`METER_OLD`].
pub fn current(h: &Headroom, now: i64) -> bool {
    h.metered()
        && meter_old(h, now).is_none()
        && [h.five, h.seven].iter().flatten().all(|l| !inferred(l))
}

/// The share of the tighter window a profile must have left for a current
/// meter to win Use next over any inferred figure.
pub const CONFIDENT: f64 = 0.40;

/// Use next: among Claude profiles usable now and not signed out,
/// one with current figures and at least [`CONFIDENT`] left beats any
/// other; then the most left, ties to the most recently used. With none
/// current and confident, hippo-core's `use_next_except` rule.
pub fn use_next<'a>(rooms: &'a [Headroom], skip: &[String], now: i64) -> Option<&'a Headroom> {
    rooms
        .iter()
        .filter(|h| !h.codex && h.usable() && !skip.contains(&h.profile))
        .filter_map(|h| h.left().map(|l| (h, l)))
        .filter(|(h, l)| current(h, now) && *l >= CONFIDENT)
        .max_by(|(a, la), (b, lb)| {
            la.total_cmp(lb)
                .then_with(|| a.last_seen.unwrap_or(0).cmp(&b.last_seen.unwrap_or(0)))
        })
        .map(|(h, _)| h)
        .or_else(|| usage::use_next_except(rooms, skip))
}

/// A profile as the app names it: its folder's suffix (`work`),
/// `default` for `~/.claude`, `codex`. Never `claude-`.
pub fn profile_label(profile: &str) -> &str {
    profile
}

/// A profile's two-letter mark and hue (0 to 5), unique among `profiles`
///Marks are the first letter upper-cased and the first later
/// letter that makes the mark unique; hues come from a hash of the name,
/// moved to the next free hue on a clash while one is free.
pub fn identities(profiles: &[&str]) -> BTreeMap<String, (String, usize)> {
    let mut names: Vec<&str> = profiles.to_vec();
    names.sort_unstable();
    names.dedup();
    let mut marks: BTreeMap<String, (String, usize)> = BTreeMap::new();
    let mut used_marks: Vec<String> = Vec::new();
    let mut used_hues: Vec<usize> = Vec::new();
    for name in names {
        let chars: Vec<char> = name.chars().filter(|c| c.is_alphanumeric()).collect();
        let first = chars.first().map_or('?', |c| c.to_ascii_uppercase());
        let mut mark = None;
        for c in chars.iter().skip(1) {
            let m = format!("{first}{}", c.to_ascii_lowercase());
            if !used_marks.contains(&m) {
                mark = Some(m);
                break;
            }
        }
        let mark = mark.unwrap_or_else(|| {
            (2..)
                .map(|n| format!("{first}{n}"))
                .find(|m| !used_marks.contains(m))
                .unwrap_or_default()
        });
        used_marks.push(mark.clone());
        let hash = name.bytes().fold(0x811c_9dc5_u32, |h, b| {
            (h ^ u32::from(b)).wrapping_mul(0x0100_0193)
        });
        let mut hue = (hash % 6) as usize;
        if used_hues.len() < 6 {
            while used_hues.contains(&hue) {
                hue = (hue + 1) % 6;
            }
        }
        used_hues.push(hue);
        marks.insert(name.to_owned(), (mark, hue));
    }
    marks
}

/// The headroom strip's order: Claude profiles by what is left of
/// the tighter window, most first (unusable and unknown after), ties to
/// the most recently used; Codex last.
pub fn ordered_rooms(rooms: &[Headroom]) -> Vec<&Headroom> {
    let mut out: Vec<&Headroom> = rooms.iter().collect();
    out.sort_by(|a, b| {
        let key = |h: &Headroom| match h.left() {
            Some(l) if h.usable() => l,
            Some(_) => -0.5,
            None => -1.0,
        };
        a.codex
            .cmp(&b.codex)
            .then_with(|| key(b).total_cmp(&key(a)))
            .then_with(|| b.last_seen.unwrap_or(0).cmp(&a.last_seen.unwrap_or(0)))
            .then_with(|| a.profile.cmp(&b.profile))
    });
    out
}

/// The profiles whose newest session in the 3 days before `now` ended
/// because its sign-in failed (the rollup's interrupted ending with detail
/// [`rollup::SIGNED_OUT`]), sorted.
pub fn signed_out(rollup: &Rollup, now: OffsetDateTime) -> Vec<String> {
    let Ok(recent) = rollup.sessions_in(now - Duration::days(3), now + Duration::minutes(1)) else {
        return Vec::new();
    };
    let mut newest: BTreeMap<String, (OffsetDateTime, bool)> = BTreeMap::new();
    for r in recent {
        let f = &r.facts;
        let (Some(profile), Some(last)) = (f.profile(), f.last_turn) else {
            continue;
        };
        let failed = f.ending_detail.as_deref() == Some(rollup::SIGNED_OUT);
        match newest.get(&profile) {
            Some((at, _)) if *at >= last => {}
            _ => {
                newest.insert(profile, (last, failed));
            }
        }
    }
    newest
        .into_iter()
        .filter(|(_, (_, failed))| *failed)
        .map(|(p, _)| p)
        .collect()
}

/// What a Resume button copies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Resume {
    /// The profile it resumes on.
    pub on: Option<String>,
    pub command: String,
    pub handoff: String,
}

impl Resume {
    /// The command, then the handoff: what goes on the clipboard.
    pub fn clipboard(&self) -> String {
        format!("{}\n\n{}", self.command, self.handoff)
    }
}

/// How to resume `row`: on its own profile, or on `other` (a Claude
/// profile's headroom, which names its config dir). `index` gives the open
/// items; without it the handoff has none.
pub fn resume(row: &Row, index: Option<&Index>, other: Option<&Headroom>) -> Option<Resume> {
    let target = Target {
        id: &row.id,
        tool: &row.tool,
        transcript: &row.transcript,
        cwd: row.cwd.as_deref(),
    };
    let (command, on) = match other.and_then(|h| h.dir.as_deref().map(|d| (h, d))) {
        Some((h, dir)) if row.profile.as_deref() != Some(h.profile.as_str()) => {
            (resume::command_on(&target, dir)?, Some(h.profile.clone()))
        }
        _ => (resume::command(&target)?, row.profile.clone()),
    };
    let open = index
        .and_then(|i| i.turns_for_session(&row.id).ok())
        .map(|t| resume::open_items(&t))
        .unwrap_or_default();
    Some(Resume {
        on,
        command,
        handoff: resume::handoff(
            &row.title,
            row.asked.as_deref(),
            row.outcome.as_deref(),
            &open,
        ),
    })
}

/// A headroom card's facts as text, for the CLI and the app alike. With the
/// client's meter: both windows, `5h 92% left, resets 14:00
/// · 7d 23% left, resets Wed 01:30`, `est.` where the 5-hour figure is the
/// transcript estimate.
pub fn room_line(h: &Headroom, offset: UtcOffset) -> String {
    let now = OffsetDateTime::now_utc();
    let hm = |t: i64| when(t, now, offset);
    if !h.codex && h.metered() {
        let mut facts = Vec::new();
        if let Some(f) = h.five {
            let mut t = format!("5h {}% left", pct(f.frac));
            if f.source == usage::Source::Estimate {
                t.push_str(" (est.)");
            }
            match (f.resets_at, f.reset_passed) {
                (Some(r), _) => t.push_str(&format!(", resets {}", hm(r))),
                (None, Some(r)) => t.push_str(&format!(", reset {}", hm(r))),
                (None, None) => t.push_str(", no window"),
            }
            facts.push(t);
        }
        if let Some(sv) = h.seven {
            let mut t = format!("7d {}% left", pct(sv.frac));
            match (sv.resets_at, sv.reset_passed) {
                (Some(r), _) => t.push_str(&format!(", resets {}", hm(r))),
                (None, Some(r)) => t.push_str(&format!(", reset {} · not refreshed", hm(r))),
                (None, None) => {}
            }
            facts.push(t);
        }
        if let Some(at) = h.meter_stale {
            facts.push(format!("meter {} old", age(now.unix_timestamp() - at)));
        }
        if let Some(w) = h.weekly_until {
            facts.push(format!("weekly limit until {}", hm(w)));
        }
        if let usage::WindowState::Limited { reset } = h.window {
            facts.push(format!("limit hit, resets {}", hm(reset)));
        }
        return facts.join(" · ");
    }
    let left = match h.left() {
        Some(l) => format!("{}% left", pct(l)),
        None => "no estimate".to_owned(),
    };
    let mut facts = vec![left];
    if let Some(w) = h.weekly_until {
        facts.push(format!("weekly limit until {}", hm(w)));
    }
    match h.window {
        usage::WindowState::Limited { reset } => {
            facts.push(format!("limit hit, resets {}", hm(reset)))
        }
        usage::WindowState::Open { end, used, .. } if !h.codex => {
            facts.push(format!("resets {}", hm(end)));
            if h.left().is_none() {
                facts.push(format!("used {}", compact(used)));
            }
        }
        usage::WindowState::Open { end, .. } => facts.push(format!("resets {}", hm(end))),
        usage::WindowState::None => facts.push("no window open".into()),
    }
    if h.limit_from > 0 {
        facts.push(format!(
            "from {} limit {}",
            h.limit_from,
            if h.limit_from == 1 { "hit" } else { "hits" }
        ));
    } else if h.from_meter {
        facts.push("Codex's own figure".into());
    }
    if !h.codex {
        facts.push("est.".into());
    }
    facts.join(" · ")
}

/// A fraction as a whole percent.
pub fn pct(frac: f64) -> i64 {
    (frac * 100.0).round() as i64
}

/// `14:00` today (local), `Wed 01:30` within a week, else `10 Nov 01:30`
/// (the app's one date form).
pub fn when(t: i64, now: OffsetDateTime, offset: UtcOffset) -> String {
    // To the nearest minute: the meter's resets fall a moment before the
    // hour (`08:29:59.7`), which reads as 14:00, not 13:59.
    let t = (t + 30).div_euclid(60) * 60;
    let Ok(at) = OffsetDateTime::from_unix_timestamp(t) else {
        return String::new();
    };
    let at = at.to_offset(offset);
    let hm = format!("{:02}:{:02}", at.hour(), at.minute());
    let today = now.to_offset(offset).date();
    if at.date() == today {
        hm
    } else if (at.date() - today).whole_days().abs() < 7 {
        let day = at.weekday().to_string();
        format!("{} {hm}", &day[..3])
    } else {
        let month = at.month().to_string();
        format!("{} {} {hm}", at.day(), &month[..3])
    }
}

/// `4m`, `3h`, `2d`: an age to the largest unit (the app's duration form).
pub fn age(secs: i64) -> String {
    let m = secs.max(0) / 60;
    if m < 60 {
        format!("{m}m")
    } else if m < 48 * 60 {
        format!("{}h", m / 60)
    } else {
        format!("{}d", m / 1440)
    }
}

/// `1.2M`, `340k`, `900`.
pub fn compact(n: f64) -> String {
    if n >= 1e6 {
        format!("{:.1}M", n / 1e6)
    } else if n >= 1e3 {
        format!("{:.0}k", n / 1e3)
    } else {
        format!("{n:.0}")
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests;
