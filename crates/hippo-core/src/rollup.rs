//! The daily rollup: a cache of what each indexed session did, kept in
//! `<home>/rollup.sqlite` beside the index, so that a day's summary is a
//! read of a few thousand rows rather than of every turn.
//!
//! Like the index, the file is an observation, not a record: it can be
//! deleted at any time and [`refresh`] builds it again from the index (and
//! each session's transcript head, for its title and parent). Nothing reads
//! it as a contract; a rollup of another format is deleted and rebuilt.
//!
//! Per session it keeps:
//!
//! - the facts a reader ranks and counts by: turns, tool calls, edits
//!   (`Edit`, `Write`, `MultiEdit`, `NotebookEdit` and Codex `apply_patch`
//!   calls), files edited, sub-agents started (`Agent`, `Task`,
//!   `spawn_agent` calls), the title, project, parent and whether a program
//!   started it ([`crate::session_meta`]); the project is where it worked
//!   ([`work`]): the repository of most of its edits and `git` and `gh`
//!   commands, else its working directory's;
//! - the first request and the last reply, each cut to [`EXCERPT_CHARS`],
//!   line breaks kept;
//! - how it ended ([`classify`]): completed, at the usage limit,
//!   rate-limited, interrupted, or unfinished, which a reader resolves to
//!   still open or interrupted by the time of its last turn ([`Ending`]);
//! - its turns and edits per 15-minute bucket (`activity`), so that a day in
//!   any time zone, and its hours, add up from buckets;
//! - the pull requests it opened or merged, each with the time of the call
//!   (`pulls`, [`work`]); pull requests it only names are not kept;
//! - failures along the way (`events`): a delegated task that failed, an
//!   API error;
//! - the notes it read and edited, as [`crate::dream::observed`] finds them
//!   in its turns (`notes`);
//! - lines added and removed per file its edit calls named (`changes`), the
//!   branch it worked on, and its sub-agents and background commands
//!   (`tasks`), as [`changes`] reads them.
//!
//! All text in it comes from the index, which holds redacted text only
//! (Invariant 2), or from [`crate::session_meta::origin`], which redacts.
//!
//! [`refresh`] runs at the end of every `hippo ingest`, under the ingest
//! lock, and recomputes only the sessions whose transcript hash changed. A
//! failed refresh is a warning: the index is what ingest is for.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, Transaction};
use time::format_description::well_known::Rfc3339;
use time::{Date, Duration, OffsetDateTime, UtcOffset};

use crate::config::Workspace;
use crate::dream::observed::{self, Kind, Resolver};
use crate::error::{Error, Result};
use crate::index::{Index, StoredTurn};
use crate::notes::{normalise, Notes};
use crate::session_meta;

pub mod changes;
pub mod t3;
pub mod work;
pub use changes::{Lines, Task, TaskKind};
pub use work::{Changed, PullDone, Repos};

/// The rollup's file name inside `$HIPPO_HOME`.
pub const FILE_NAME: &str = "rollup.sqlite";

/// `PRAGMA user_version` of a rollup this code reads and writes.
/// 2: the first request and last reply keep their line breaks.
/// 3: the project is where the session worked; pulls are only those it
/// opened or merged.
/// 4: a sign-in failure as the last reply is an interruption, not a reply.
/// 6: a session with no request is asked by the slash command it ran
/// (`/login`); the home folder is never a project.
/// 7: claims and context use per top-level session; a
/// request is looked for in more user turns before a slash command.
/// 8: the claims rules tuned on the app's real-data check (scripts run with
/// the edit, CI sentences, the session id only outside paths).
/// 9: lines changed per file, the branch and sub-tasks.
/// 10: a sub-task's own session, what it was given and gave back, its
/// usage and exit code; T3 Code's delegated tasks.
/// 11: signals, edit lines and incidents.
/// 12: a pull request's title, and a correction's subject.
/// 13: claims checked by cause, and rules that name no one project's words.
const FORMAT: i64 = 13;

/// The width of an activity bucket, in seconds. Every UTC offset in use is
/// a whole number of quarter hours, so a local day is whole buckets.
pub const BUCKET_SECONDS: i64 = 900;

/// How many user turns are searched for the first request before a slash
/// command stands for it.
const ASKED_TURNS: usize = 60;

/// The longest first request or last reply kept, in characters.
pub const EXCERPT_CHARS: usize = 600;

/// An unfinished session whose last turn is at most this old is still open;
/// an older one was interrupted.
pub const OPEN_FOR: Duration = Duration::minutes(30);

/// Sessions written per transaction while refreshing.
const BATCH: usize = 100;

const DDL: &str = "\
CREATE TABLE sessions(id TEXT PRIMARY KEY, file_hash TEXT NOT NULL, tool TEXT NOT NULL, transcript_path TEXT NOT NULL, first_turn TEXT, last_turn TEXT, turns INTEGER NOT NULL, tool_calls INTEGER NOT NULL, edits INTEGER NOT NULL, files INTEGER NOT NULL, agents INTEGER NOT NULL, title TEXT, project TEXT, project_weight INTEGER NOT NULL, parent TEXT, automated INTEGER NOT NULL, asked TEXT, outcome TEXT, ending TEXT NOT NULL, ending_detail TEXT, ending_at TEXT, branch TEXT);
CREATE TABLE activity(session_id TEXT NOT NULL, bucket INTEGER NOT NULL, turns INTEGER NOT NULL, edits INTEGER NOT NULL, PRIMARY KEY(session_id, bucket)) WITHOUT ROWID;
CREATE INDEX activity_bucket ON activity(bucket);
CREATE TABLE pulls(session_id TEXT NOT NULL, url TEXT NOT NULL, done TEXT NOT NULL, ts TEXT NOT NULL, title TEXT, PRIMARY KEY(session_id, url, done)) WITHOUT ROWID;
CREATE TABLE events(session_id TEXT NOT NULL, ts TEXT NOT NULL, kind TEXT NOT NULL, detail TEXT NOT NULL);
CREATE INDEX events_session ON events(session_id);
CREATE TABLE notes(session_id TEXT NOT NULL, note TEXT NOT NULL, kind TEXT NOT NULL, ts TEXT NOT NULL, PRIMARY KEY(session_id, note)) WITHOUT ROWID;
CREATE INDEX notes_note ON notes(note);
CREATE TABLE claims(session_id TEXT NOT NULL, ord INTEGER NOT NULL, kind TEXT NOT NULL, status TEXT NOT NULL, claim TEXT NOT NULL, evidence TEXT NOT NULL, ordinal INTEGER NOT NULL, pr INTEGER, PRIMARY KEY(session_id, ord)) WITHOUT ROWID;
CREATE TABLE context(session_id TEXT NOT NULL, ord INTEGER NOT NULL, source TEXT NOT NULL, kind TEXT NOT NULL, label TEXT NOT NULL, note TEXT, bytes INTEGER NOT NULL, use TEXT NOT NULL, PRIMARY KEY(session_id, ord)) WITHOUT ROWID;
CREATE TABLE changes(session_id TEXT NOT NULL, ord INTEGER NOT NULL, path TEXT NOT NULL, added INTEGER NOT NULL, removed INTEGER NOT NULL, calls INTEGER NOT NULL, uncounted INTEGER NOT NULL, PRIMARY KEY(session_id, ord)) WITHOUT ROWID;
CREATE TABLE tasks(session_id TEXT NOT NULL, ord INTEGER NOT NULL, kind TEXT NOT NULL, title TEXT NOT NULL, started TEXT NOT NULL, ended TEXT, status TEXT, ref TEXT, thread TEXT, child TEXT, input TEXT, report TEXT, tokens INTEGER, tools INTEGER, exit INTEGER, PRIMARY KEY(session_id, ord)) WITHOUT ROWID;
CREATE INDEX tasks_child ON tasks(child);
CREATE INDEX sessions_parent ON sessions(parent);
PRAGMA user_version=13;";

pub(crate) const SESSION_COLUMNS: &str = "id, tool, transcript_path, first_turn, last_turn, turns, tool_calls, edits, files, agents, title, project, parent, automated, asked, outcome, ending, ending_detail, ending_at, project_weight, branch";

// ---------------------------------------------------------------------------
// How a session ended

/// How a session's transcript ends, as stored: [`Stored::Unfinished`] is
/// resolved against the clock by [`Ending::of`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stored {
    /// The last word is an ordinary reply.
    Completed,
    /// The last reply is the client's usage-limit message.
    UsageLimit,
    /// The last reply is an API error for a rate limit or overload.
    RateLimited,
    /// The person stopped it, or the last reply is another API error.
    Interrupted,
    /// The transcript ends mid-step: a request, a tool call or a result
    /// with no reply after it.
    Unfinished,
}

impl Stored {
    pub fn as_str(self) -> &'static str {
        match self {
            Stored::Completed => "completed",
            Stored::UsageLimit => "usage_limit",
            Stored::RateLimited => "rate_limited",
            Stored::Interrupted => "interrupted",
            Stored::Unfinished => "unfinished",
        }
    }

    fn parse(s: &str) -> Stored {
        match s {
            "completed" => Stored::Completed,
            "usage_limit" => Stored::UsageLimit,
            "rate_limited" => Stored::RateLimited,
            "interrupted" => Stored::Interrupted,
            _ => Stored::Unfinished,
        }
    }
}

/// How a session ended, as a reader says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Ending {
    Completed,
    UsageLimit,
    RateLimited,
    Interrupted,
    /// Unfinished, and its last turn is at most [`OPEN_FOR`] old.
    Open,
}

impl Ending {
    /// `stored` at `now`, for a session whose last turn was at `last`.
    pub fn of(stored: Stored, last: Option<OffsetDateTime>, now: OffsetDateTime) -> Ending {
        match stored {
            Stored::Completed => Ending::Completed,
            Stored::UsageLimit => Ending::UsageLimit,
            Stored::RateLimited => Ending::RateLimited,
            Stored::Interrupted => Ending::Interrupted,
            Stored::Unfinished => match last {
                Some(t) if now - t <= OPEN_FOR => Ending::Open,
                _ => Ending::Interrupted,
            },
        }
    }

    /// A few words: `completed`, `usage limit`, `rate-limited`,
    /// `interrupted`, `still open`.
    pub fn label(self) -> &'static str {
        match self {
            Ending::Completed => "completed",
            Ending::UsageLimit => "usage limit",
            Ending::RateLimited => "rate-limited",
            Ending::Interrupted => "interrupted",
            Ending::Open => "still open",
        }
    }

    /// The ending is a problem worth naming: a limit or an interruption
    /// the person did not choose is.
    pub fn is_problem(self) -> bool {
        matches!(self, Ending::UsageLimit | Ending::RateLimited)
    }
}

/// What [`classify`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Classified {
    pub stored: Stored,
    /// When the client gives one: the reset (`resets 6:10pm`), the API
    /// error's first line, `stopped by you`.
    pub detail: Option<String>,
    /// The time of the turn that decided it.
    pub at: Option<String>,
}

static USAGE_LIMIT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bhit your (?:[\w-]+ ){0,3}limit\b|usage limit reached|\blimit reached\s*\|")
        .expect("valid regex")
});
static RESET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bresets?\s+(?:at\s+)?([0-9]{1,2}(?::[0-9]{2})?\s*[ap]\.?m\.?)")
        .expect("valid regex")
});
static RATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b429\b|\b529\b|rate.?limit|overloaded").expect("valid regex")
});
static SIGN_IN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(?:Failed to authenticate|Invalid API key|OAuth token has expired|Not logged in)",
    )
    .expect("valid regex")
});

/// The detail of a session the client stopped because its sign-in failed.
pub const SIGNED_OUT: &str = "sign-in failed";

static DELEGATE_FAILED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)^Delegated task \S+ \((.+?)\) \[failed\]:\s*(.+)$").expect("valid regex")
});

/// A turn is talk (a person's or the assistant's words), not a tool's.
fn is_talk(t: &StoredTurn) -> bool {
    (t.role == "assistant" || t.role == "user") && !t.text.trim().is_empty()
}

/// Whether an assistant reply is the client's usage-limit message (`You've
/// hit your 5-hour limit · resets 6:10pm`), as [`classify`] reads it.
pub fn is_usage_limit(text: &str) -> bool {
    USAGE_LIMIT.is_match(text.trim())
}

/// How a session's turns end. Reads the last turn and the last talk
/// before it:
///
/// - the last reply is the usage-limit message (`You've hit your 5-hour
///   limit · resets 6:10pm`): [`Stored::UsageLimit`], the reset as detail;
/// - it is an `API Error:` for a rate limit, 429, 529 or an overload:
///   [`Stored::RateLimited`]; another API error: [`Stored::Interrupted`],
///   the error as detail;
/// - it is the client's sign-in failure (`Failed to authenticate: …`):
///   [`Stored::Interrupted`], [`SIGNED_OUT`];
/// - the last word is `[Request interrupted by user…]`:
///   [`Stored::Interrupted`], `stopped by you`;
/// - the last turn is a reply: [`Stored::Completed`];
/// - anything else (a request, a call or a result with no reply after
///   it): [`Stored::Unfinished`].
pub fn classify(turns: &[StoredTurn]) -> Classified {
    let Some(last) = turns.last() else {
        return Classified {
            stored: Stored::Unfinished,
            detail: None,
            at: None,
        };
    };
    let talk = turns.iter().rposition(is_talk).map(|i| &turns[i]);
    if let Some(t) = talk {
        let text = t.text.trim();
        if t.role == "assistant" && USAGE_LIMIT.is_match(text) {
            return Classified {
                stored: Stored::UsageLimit,
                detail: RESET
                    .captures(text)
                    .map(|c| format!("resets {}", c[1].replace(' ', "").to_lowercase())),
                at: Some(t.ts.clone()),
            };
        }
        if t.role == "assistant" && SIGN_IN.is_match(text) {
            return Classified {
                stored: Stored::Interrupted,
                detail: Some(SIGNED_OUT.to_owned()),
                at: Some(t.ts.clone()),
            };
        }
        if t.role == "assistant" && text.starts_with("API Error") {
            let first = text.lines().next().unwrap_or(text);
            let stored = if RATE.is_match(first) {
                Stored::RateLimited
            } else {
                Stored::Interrupted
            };
            return Classified {
                stored,
                detail: Some(cut(first, 120)),
                at: Some(t.ts.clone()),
            };
        }
        if t.role == "user" && text.starts_with("[Request interrupted by user") {
            return Classified {
                stored: Stored::Interrupted,
                detail: Some("stopped by you".to_owned()),
                at: Some(t.ts.clone()),
            };
        }
    }
    let stored = if last.role == "assistant" && is_talk(last) {
        Stored::Completed
    } else {
        Stored::Unfinished
    };
    Classified {
        stored,
        detail: None,
        at: Some(last.ts.clone()),
    }
}

// ---------------------------------------------------------------------------
// Facts of one session

/// A failure along the way, from a session's turns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub session_id: String,
    pub ts: String,
    /// `delegate_failed` or `api_error`.
    pub kind: String,
    pub detail: String,
}

/// A pull request a session opened or merged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pull {
    pub session_id: String,
    pub url: String,
    pub done: PullDone,
    /// When the call that did it ran.
    pub ts: String,
    /// The title it was opened with, when the call named one.
    pub title: Option<String>,
}

/// A note a session read or edited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteSeen {
    pub session_id: String,
    /// Relative to the workspace.
    pub note: PathBuf,
    /// The strongest: `read`, `discuss` or `edit`.
    pub kind: String,
    /// The first turn that named it.
    pub ts: String,
}

/// One session's row in the rollup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionFacts {
    pub id: String,
    pub tool: String,
    pub transcript_path: PathBuf,
    pub first_turn: Option<OffsetDateTime>,
    pub last_turn: Option<OffsetDateTime>,
    pub turns: u64,
    pub tool_calls: u64,
    pub edits: u64,
    pub files: u64,
    pub agents: u64,
    pub title: Option<String>,
    /// The repository it worked in ([`work`]), else its working
    /// directory's.
    pub project: Option<String>,
    /// How many of its edits and `git`/`gh` commands point at `project`:
    /// 0 when the project is only its working directory's.
    pub project_weight: u64,
    pub parent: Option<String>,
    pub automated: bool,
    pub asked: Option<String>,
    pub outcome: Option<String>,
    pub stored: Stored,
    pub ending_detail: Option<String>,
    pub ending_at: Option<OffsetDateTime>,
    /// The branch it worked on.
    pub branch: Option<String>,
}

impl SessionFacts {
    /// How it ended, at `now`.
    pub fn ending(&self, now: OffsetDateTime) -> Ending {
        Ending::of(self.stored, self.last_turn, now)
    }

    /// The Claude profile the transcript is under: `default` for
    /// `~/.claude`, `<name>` for `~/.claude-<name>`; `codex` for Codex.
    pub fn profile(&self) -> Option<String> {
        profile_of(&self.tool, &self.transcript_path)
    }
}

/// The profile of a transcript at `path`, as [`SessionFacts::profile`].
pub fn profile_of(tool: &str, path: &Path) -> Option<String> {
    if tool == "codex" {
        return Some("codex".to_owned());
    }
    path.components().find_map(|c| {
        let part = c.as_os_str().to_str()?;
        if part == ".claude" {
            Some("default".to_owned())
        } else {
            part.strip_prefix(".claude-").map(str::to_owned)
        }
    })
}

/// A tool turn is a call (its text starts with its tool's name and a
/// space), not a result.
pub(crate) fn is_call(t: &StoredTurn) -> bool {
    t.role == "tool"
        && t.tool_name.as_deref().is_some_and(|name| {
            t.text.len() > name.len()
                && t.text.starts_with(name)
                && t.text.as_bytes()[name.len()] == b' '
        })
}

pub(crate) fn is_edit_tool(name: &str) -> bool {
    matches!(
        name,
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" | "apply_patch"
    )
}

pub(crate) fn is_agent_tool(name: &str) -> bool {
    matches!(name, "Agent" | "Task" | "spawn_agent")
}

/// The files an edit call's input names.
pub(crate) fn edited_paths(name: &str, input: &str) -> Vec<String> {
    match name {
        "apply_patch" => input
            .split("\\n")
            .flat_map(|l| l.split('\n'))
            .filter_map(|l| {
                ["*** Update File: ", "*** Add File: ", "*** Delete File: "]
                    .iter()
                    .find_map(|p| l.trim_start_matches('"').strip_prefix(p))
            })
            .map(|p| p.trim().trim_end_matches('"').to_owned())
            .collect(),
        _ => serde_json::from_str::<serde_json::Value>(input)
            .ok()
            .and_then(|v| {
                v.get("file_path")
                    .or_else(|| v.get("notebook_path"))
                    .and_then(|p| p.as_str())
                    .map(|p| vec![p.to_owned()])
            })
            .unwrap_or_default(),
    }
}

/// GitHub pull request URLs in `text`: `https://github.com/<o>/<r>/pull/<n>`,
/// cut after the number, in order, repeats kept.
pub fn pull_urls(text: &str) -> Vec<String> {
    const START: &str = "https://github.com/";
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find(START) {
        let tail = &rest[at + START.len()..];
        let parts: Vec<&str> = tail.splitn(4, '/').collect();
        if parts.len() == 4 && parts[2] == "pull" {
            let digits: String = parts[3].chars().take_while(char::is_ascii_digit).collect();
            let name_ok = |p: &str| {
                !p.is_empty()
                    && p.chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
            };
            if !digits.is_empty() && name_ok(parts[0]) && name_ok(parts[1]) {
                out.push(format!("{START}{}/{}/pull/{digits}", parts[0], parts[1]));
            }
        }
        rest = tail;
    }
    out
}

/// `text` with its whitespace collapsed, cut at a character boundary to
/// `max` characters (an ellipsis marks a cut).
fn cut(text: &str, max: usize) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= max {
        return collapsed;
    }
    let mut out: String = collapsed.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// `text` trimmed and cut at a character boundary to `max` characters (an
/// ellipsis marks a cut), its line breaks kept, so a reader can still see
/// its Markdown.
fn excerpt(text: &str, max: usize) -> String {
    let t = text.trim();
    if t.chars().count() <= max {
        return t.to_owned();
    }
    let mut out: String = t.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// An RFC 3339 time as the rollup stores it.
pub fn parse_ts(s: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(s, &Rfc3339).ok()
}

/// What [`facts`] computes for one session from its turns.
#[derive(Debug, Clone, Default)]
struct Computed {
    turns: u64,
    tool_calls: u64,
    edits: u64,
    files: u64,
    agents: u64,
    asked: Option<String>,
    outcome: Option<String>,
    activity: BTreeMap<i64, (u64, u64)>,
    events: Vec<(String, String, String)>,
}

fn compute(turns: &[StoredTurn]) -> Computed {
    let mut out = Computed {
        turns: turns.len() as u64,
        ..Computed::default()
    };
    let mut files: Vec<String> = Vec::new();
    for t in turns {
        let edit = is_call(t) && t.tool_name.as_deref().is_some_and(is_edit_tool);
        if let Some(ts) = parse_ts(&t.ts) {
            let bucket = ts.unix_timestamp().div_euclid(BUCKET_SECONDS);
            let slot = out.activity.entry(bucket).or_default();
            slot.0 += 1;
            slot.1 += u64::from(edit);
        }
        if is_call(t) {
            out.tool_calls += 1;
            let name = t.tool_name.as_deref().unwrap_or_default();
            let input = &t.text[name.len() + 1..];
            if edit {
                out.edits += 1;
                for f in edited_paths(name, input) {
                    if !files.contains(&f) {
                        files.push(f);
                    }
                }
            }
            if is_agent_tool(name) {
                out.agents += 1;
            }
        } else if t.role == "assistant" {
            let text = t.text.trim();
            if text.starts_with("API Error") {
                out.events.push((
                    t.ts.clone(),
                    "api_error".to_owned(),
                    cut(text.lines().next().unwrap_or(text), 120),
                ));
            }
        } else if t.role == "user" {
            if let Some(c) = DELEGATE_FAILED.captures(t.text.trim()) {
                out.events.push((
                    t.ts.clone(),
                    "delegate_failed".to_owned(),
                    format!("{}: {}", cut(&c[1], 80), cut(&c[2], 120)),
                ));
            }
        }
    }
    out.files = files.len() as u64;
    out.asked = turns
        .iter()
        .filter(|t| t.role == "user")
        .take(ASKED_TURNS)
        .find_map(|t| session_meta::request_text(&t.text))
        .map(|r| excerpt(&r, EXCERPT_CHARS))
        .or_else(|| {
            // No request: the slash command a person ran, such as `/login`.
            turns
                .iter()
                .filter(|t| t.role == "user")
                .take(session_meta::OPENINGS * 3)
                .find_map(|t| slash_command(&t.text))
        });
    out.outcome = turns
        .iter()
        .rev()
        .filter(|t| t.role == "assistant" && is_talk(t))
        .map(|t| t.text.trim())
        .find(|t| !t.starts_with("API Error") && !USAGE_LIMIT.is_match(t) && !SIGN_IN.is_match(t))
        .map(|t| excerpt(t, EXCERPT_CHARS));
    out
}

static COMMAND_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"<command-name>\s*(/[\w:-]+)\s*</command-name>").expect("valid regex")
});

/// The slash command a `<command-name>` turn records, such as `/login`.
fn slash_command(text: &str) -> Option<String> {
    COMMAND_NAME.captures(text).map(|c| c[1].to_owned())
}

// ---------------------------------------------------------------------------
// Writing

/// What a [`refresh`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Refreshed {
    /// Sessions computed again (new, or their transcript changed).
    pub updated: usize,
    /// Sessions the index no longer holds, removed.
    pub removed: usize,
}

fn create(conn: &Connection) -> Result<()> {
    conn.execute_batch(DDL)?;
    conn.execute_batch(crate::incidents::DDL)?;
    Ok(())
}

/// Opens `<home>/rollup.sqlite` for writing, creating it, or deleting and
/// creating it again when it is of another format or not a rollup.
fn open_for_writing(home: &Path) -> Result<Connection> {
    let path = home.join(FILE_NAME);
    let open = || -> Result<Option<Connection>> {
        let conn = Connection::open(&path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        let empty: bool =
            conn.query_row("SELECT NOT EXISTS(SELECT 1 FROM sqlite_master)", [], |r| {
                r.get(0)
            })?;
        if version == FORMAT {
            Ok(Some(conn))
        } else if version == 0 && empty {
            create(&conn)?;
            Ok(Some(conn))
        } else {
            Ok(None)
        }
    };
    match open() {
        Ok(Some(conn)) => Ok(conn),
        // Another format, or not a database: it is a cache, so start again.
        Ok(None) | Err(Error::Sqlite(_)) => {
            remove(home)?;
            open()?.ok_or_else(|| {
                Error::Io(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{}: cannot create the rollup", path.display()),
                ))
            })
        }
        Err(e) => Err(e),
    }
}

/// Deletes `<home>/rollup.sqlite` and its WAL files, if present.
pub fn remove(home: &Path) -> Result<()> {
    for suffix in ["", "-wal", "-shm"] {
        let path = home.join(format!("{FILE_NAME}{suffix}"));
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(Error::io_at(&path, e)),
        }
    }
    Ok(())
}

/// What [`refresh`] needs to find the notes a session named.
struct NoteFinder<'a> {
    resolver: Resolver<'a>,
    ws: &'a Workspace,
    notes: &'a Notes,
}

impl NoteFinder<'_> {
    fn notes(&self, tool: &str, path: &Path, turns: &[StoredTurn]) -> Vec<(PathBuf, Kind, String)> {
        let dir = self.resolver.session_dir(tool, path);
        let mut seen: BTreeMap<PathBuf, (Kind, String)> = BTreeMap::new();
        for t in turns {
            // As `Index::note_mentions` filters: talk and calls that hold a
            // path to a note or a link; never a tool's result.
            if !(is_call(t) || t.role != "tool")
                || !(t.text.contains(".md") || t.text.contains("[["))
            {
                continue;
            }
            for o in observed::observe(&self.resolver, dir.as_deref(), t) {
                let slot = seen.entry(o.note).or_insert((o.kind, t.ts.clone()));
                slot.0 = slot.0.max(o.kind);
            }
        }
        seen.into_iter().map(|(n, (k, ts))| (n, k, ts)).collect()
    }
}

/// Brings `<home>/rollup.sqlite` up to date with `index`: every session the
/// index holds whose transcript hash is not the one the rollup has is
/// computed again from its turns and transcript head, and sessions the
/// index no longer holds are removed. With `ws`, the notes each session
/// read and edited are found too. Writes only the rollup. Callers hold the
/// ingest lock, as `hippo ingest` does.
pub fn refresh(home: &Path, ws: Option<&Workspace>, index: &Index) -> Result<Refreshed> {
    refresh_with(home, ws, index, t3::state_path().as_deref())
}

/// [`refresh`], finding delegated tasks' sessions in the T3 Code state at
/// `t3`; `None` finds none.
pub fn refresh_with(
    home: &Path,
    ws: Option<&Workspace>,
    index: &Index,
    t3: Option<&Path>,
) -> Result<Refreshed> {
    let mut conn = open_for_writing(home)?;
    let out = refresh_into(&mut conn, ws, index, None)?;
    let mut linked = 0;
    if let Some(db) = t3 {
        linked = resolve_delegates(&mut conn, index, db)?;
    }
    // Incidents read every session of their window, so they are built
    // again whenever one changed.
    if out.updated > 0 || out.removed > 0 || linked > 0 || !crate::incidents::has_rows(&conn)? {
        crate::incidents::rebuild(&mut conn, index, ws, home, OffsetDateTime::now_utc())?;
    }
    Ok(out)
}

/// Links each delegated task with no session yet to the session T3 Code's
/// state says its child thread ran as, when the index holds it (a Codex
/// session's id is `codex-<uuid>`). Tasks of the last 14 days only.
fn resolve_delegates(conn: &mut Connection, index: &Index, db: &Path) -> Result<usize> {
    let since = (OffsetDateTime::now_utc() - Duration::days(14))
        .format(&Rfc3339)
        .unwrap_or_default();
    let open: Vec<(String, i64, String)> = {
        let mut stmt = conn.prepare(
            "SELECT session_id, ord, thread FROM tasks WHERE kind = 'delegate' AND child IS NULL \
             AND thread IS NOT NULL AND started >= ?1",
        )?;
        let rows = stmt.query_map([&since], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    if open.is_empty() {
        return Ok(0);
    }
    let threads: Vec<String> = open.iter().map(|(_, _, t)| t.clone()).collect();
    let natives = t3::natives(db, &threads);
    let tx = conn.transaction()?;
    let mut linked = 0;
    for (session, ord, thread) in &open {
        let Some(ids) = natives.get(thread) else {
            continue;
        };
        let found = ids.iter().find_map(|n| {
            [n.clone(), format!("codex-{n}")]
                .into_iter()
                .find(|c| c != session && index.source(c).ok().flatten().is_some())
        });
        if let Some(child) = found {
            tx.execute(
                "UPDATE tasks SET child = ?1 WHERE session_id = ?2 AND ord = ?3",
                params![child, session, ord],
            )?;
            linked += 1;
        }
    }
    tx.commit()?;
    Ok(linked)
}

/// [`refresh`] into `conn`; with `only`, for those sessions alone.
fn refresh_into(
    conn: &mut Connection,
    ws: Option<&Workspace>,
    index: &Index,
    only: Option<&[String]>,
) -> Result<Refreshed> {
    let current = index.session_hashes()?;
    let stored: HashMap<String, String> = {
        let mut stmt = conn.prepare("SELECT id, file_hash FROM sessions")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let scanned = match ws {
        Some(ws) => Notes::scan(&ws.notes_root()).ok().map(|n| (ws, n)),
        None => None,
    };
    let log = scanned.as_ref().map(|(ws, _)| normalise(&ws.log_path));
    let finder = scanned.as_ref().map(|(ws, notes)| NoteFinder {
        resolver: Resolver::new(&ws.root, &ws.notes_dir, notes, log.as_deref()),
        ws,
        notes,
    });

    let mut out = Refreshed::default();
    let removed: Vec<&String> = stored
        .keys()
        .filter(|id| !current.contains_key(*id))
        .collect();
    if only.is_none() && !removed.is_empty() {
        let tx = conn.transaction()?;
        for id in &removed {
            delete_session(&tx, id)?;
        }
        tx.commit()?;
        out.removed = removed.len();
    }
    let mut changed: Vec<(&String, &(String, String, PathBuf))> = current
        .iter()
        .filter(|(id, (hash, _, _))| stored.get(*id) != Some(hash))
        .filter(|(id, _)| only.is_none_or(|o| o.contains(id)))
        .collect();
    changed.sort_by(|a, b| a.0.cmp(b.0));
    let mut repos = Repos::new();
    for batch in changed.chunks(BATCH) {
        let tx = conn.transaction()?;
        for (id, (hash, tool, path)) in batch {
            let turns = index.turns_for_session(id)?;
            let at = Session {
                id,
                hash,
                tool,
                path,
            };
            write_session(&tx, &at, &turns, finder.as_ref(), &mut repos, index)?;
            out.updated += 1;
        }
        tx.commit()?;
    }
    Ok(out)
}

fn delete_session(tx: &Transaction<'_>, id: &str) -> Result<()> {
    for table in [
        "sessions", "activity", "pulls", "events", "notes", "claims", "context", "changes",
        "tasks", "signals", "edits",
    ] {
        let column = if table == "sessions" {
            "id"
        } else {
            "session_id"
        };
        tx.execute(&format!("DELETE FROM {table} WHERE {column} = ?1"), [id])?;
    }
    Ok(())
}

/// Which session [`write_session`] writes.
struct Session<'a> {
    id: &'a str,
    hash: &'a str,
    tool: &'a str,
    path: &'a Path,
}

fn write_session(
    tx: &Transaction<'_>,
    at: &Session<'_>,
    turns: &[StoredTurn],
    finder: Option<&NoteFinder<'_>>,
    repos: &mut Repos,
    index: &Index,
) -> Result<()> {
    let Session {
        id,
        hash,
        tool,
        path,
    } = *at;
    delete_session(tx, id)?;
    let c = compute(turns);
    let ending = classify(turns);
    let origin = session_meta::origin(id, tool, path);
    let openings = session_meta::openings_of(turns);
    let title = session_meta::title("", &openings, Some(&origin));
    let work = work::read(turns, origin.cwd.as_deref(), repos);
    let (project, weight) = match work.project {
        Some((name, weight)) => (Some(name), weight),
        None => (origin.project(), 0),
    };
    tx.execute(
        &format!("INSERT INTO sessions(file_hash, {SESSION_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22)"),
        params![
            hash,
            id,
            tool,
            path.to_string_lossy(),
            turns.first().map(|t| t.ts.as_str()),
            turns.last().map(|t| t.ts.as_str()),
            c.turns as i64,
            c.tool_calls as i64,
            c.edits as i64,
            c.files as i64,
            c.agents as i64,
            title,
            project,
            // A sub-agent whose transcript head cannot be read still names
            // its parent in its id, `<parent>-agent-<agent>`.
            origin
                .parent
                .clone()
                .or_else(|| id.split_once("-agent-").map(|(p, _)| p.to_owned())),
            origin.automated,
            c.asked,
            c.outcome,
            ending.stored.as_str(),
            ending.detail,
            ending.at,
            weight as i64,
            work.branch.clone().or_else(|| origin.branch.clone()),
        ],
    )?;
    let mut stmt = tx.prepare_cached(
        "INSERT INTO changes(session_id, ord, path, added, removed, calls, uncounted) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )?;
    for (i, c) in work.changes.iter().enumerate() {
        stmt.execute(params![
            id,
            i as i64,
            c.path,
            c.lines.added as i64,
            c.lines.removed as i64,
            c.calls as i64,
            c.uncounted as i64,
        ])?;
    }
    let mut stmt = tx.prepare_cached(
        "INSERT INTO tasks(session_id, ord, kind, title, started, ended, status, ref, thread, child, input, report, tokens, tools, exit) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
    )?;
    let describe = |agent: &str| {
        (tool == "claude-code")
            .then(|| session_meta::agent_description(path, agent))
            .flatten()
    };
    let tasks = changes::tasks(turns, &work.pairs, &describe);
    for (i, t) in tasks.iter().enumerate() {
        // A Claude Code sub-agent's transcript is the session
        // `<id>-agent-<agentId>`, when the index holds it; a delegated
        // task's is found through T3 Code's state after the batch
        // ([`resolve_delegates`]).
        let child = match (t.kind, t.id.as_deref()) {
            (TaskKind::Agent, Some(agent)) if tool == "claude-code" => {
                let c = format!("{id}-agent-{agent}");
                index.source(&c)?.is_some().then_some(c)
            }
            _ => None,
        };
        let n = |v: Option<u64>| v.map(|v| i64::try_from(v).unwrap_or(i64::MAX));
        stmt.execute(params![
            id,
            i as i64,
            t.kind.as_str(),
            t.title,
            t.started,
            t.ended,
            t.status,
            t.id,
            t.thread,
            child,
            t.input,
            t.report,
            n(t.tokens),
            n(t.tools),
            t.exit,
        ])?;
    }
    let mut stmt = tx.prepare_cached(
        "INSERT INTO activity(session_id, bucket, turns, edits) VALUES (?1, ?2, ?3, ?4)",
    )?;
    for (bucket, (n, edits)) in &c.activity {
        stmt.execute(params![id, bucket, *n as i64, *edits as i64])?;
    }
    let mut stmt = tx.prepare_cached(
        "INSERT INTO pulls(session_id, url, done, ts, title) VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;
    for (url, done, ts) in &work.pulls {
        let title = (*done == work::PullDone::Opened)
            .then(|| {
                work.pull_titles
                    .iter()
                    .find(|(u, _)| u == url)
                    .map(|(_, t)| t)
            })
            .flatten();
        stmt.execute(params![id, url, done.as_str(), ts, title])?;
    }
    let mut stmt = tx.prepare_cached(
        "INSERT INTO events(session_id, ts, kind, detail) VALUES (?1, ?2, ?3, ?4)",
    )?;
    for (ts, kind, detail) in &c.events {
        stmt.execute(params![id, ts, kind, detail])?;
    }
    if let Some(finder) = finder {
        let mut stmt = tx.prepare_cached(
            "INSERT INTO notes(session_id, note, kind, ts) VALUES (?1, ?2, ?3, ?4)",
        )?;
        for (note, kind, ts) in finder.notes(tool, path, turns) {
            stmt.execute(params![id, note.to_string_lossy(), kind.as_str(), ts])?;
        }
    }
    // The x-ray's claims and context use, for a person's top-level session.
    let top_level = origin.parent.is_none() && !id.contains("-agent-") && !origin.automated;
    crate::incidents::write_session(tx, id, turns, top_level, repos)?;
    if top_level {
        let subs: Vec<(String, Vec<StoredTurn>)> = index
            .sub_agents(id)?
            .into_iter()
            .map(|s| {
                let t = index.turns_for_session(&s)?;
                Ok((s, t))
            })
            .collect::<Result<_>>()?;
        let record = finder.and_then(|f| {
            crate::sessions::is_valid_id(id)
                .then(|| crate::sessions::get(f.ws, id).ok().flatten())
                .flatten()
        });
        let x = crate::xray::analyse(&crate::xray::Inputs {
            id,
            tool,
            transcript: path,
            turns,
            subs: &subs,
            record: record.as_ref(),
            notes: finder.map(|f| (f.ws, f.notes, &f.resolver)),
        });
        let mut stmt = tx.prepare_cached(
            "INSERT INTO claims(session_id, ord, kind, status, claim, evidence, ordinal, pr) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?;
        for (i, c) in x.claims.iter().enumerate() {
            stmt.execute(params![
                id,
                i as i64,
                c.kind.as_str(),
                c.status.as_str(),
                c.claim,
                c.evidence,
                i64::from(c.ordinal),
                c.pr.map(|n| n as i64),
            ])?;
        }
        let mut stmt = tx.prepare_cached(
            "INSERT INTO context(session_id, ord, source, kind, label, note, bytes, use) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?;
        for (i, p) in x.parts.iter().enumerate() {
            stmt.execute(params![
                id,
                i as i64,
                x.block.as_str(),
                p.kind.as_str(),
                p.label,
                p.note.as_ref().map(|n| n.to_string_lossy().into_owned()),
                p.bytes as i64,
                p.used.as_str(),
            ])?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Reading

/// One local day's totals, for a list of days.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DayTotals {
    pub date: Option<Date>,
    /// Top-level sessions a person started with a turn that day.
    pub sessions: usize,
    /// Sub-agent sessions with a turn that day.
    pub agents: usize,
    pub turns: u64,
    pub edits: u64,
    /// Pull requests opened or merged that day.
    pub pulls: usize,
}

/// A session's facts with its turns and edits inside a range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InRange {
    pub facts: SessionFacts,
    pub turns: u64,
    pub edits: u64,
    /// Its first turn inside the range: the first turn itself when it is
    /// inside, else the start of its first bucket there.
    pub first: Option<OffsetDateTime>,
    /// Its last turn inside the range: the last turn itself when it is
    /// inside, else the end of its last bucket there.
    pub last: Option<OffsetDateTime>,
}

/// What [`Rollup::summary`] gives.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    /// Top-level sessions a person started.
    pub sessions: usize,
    /// Profiles, sorted ([`profile_of`]).
    pub profiles: Vec<String>,
    pub first: Option<OffsetDateTime>,
}

/// A read-only view of the rollup.
pub struct Rollup {
    conn: Connection,
}

impl std::fmt::Debug for Rollup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Rollup").finish_non_exhaustive()
    }
}

/// The local day `date` at `offset`, as a UTC range.
pub fn day_range(date: Date, offset: UtcOffset) -> (OffsetDateTime, OffsetDateTime) {
    let start = date.midnight().assume_offset(offset);
    (start, start + Duration::days(1))
}

fn bucket_of(t: OffsetDateTime) -> i64 {
    t.unix_timestamp().div_euclid(BUCKET_SECONDS)
}

pub(crate) fn row_facts(r: &rusqlite::Row<'_>) -> rusqlite::Result<SessionFacts> {
    let ts = |i: usize| -> rusqlite::Result<Option<OffsetDateTime>> {
        Ok(r.get::<_, Option<String>>(i)?.as_deref().and_then(parse_ts))
    };
    let n =
        |i: usize| -> rusqlite::Result<u64> { Ok(u64::try_from(r.get::<_, i64>(i)?).unwrap_or(0)) };
    Ok(SessionFacts {
        id: r.get(0)?,
        tool: r.get(1)?,
        transcript_path: PathBuf::from(r.get::<_, String>(2)?),
        first_turn: ts(3)?,
        last_turn: ts(4)?,
        turns: n(5)?,
        tool_calls: n(6)?,
        edits: n(7)?,
        files: n(8)?,
        agents: n(9)?,
        title: r.get(10)?,
        project: r.get(11)?,
        parent: r.get(12)?,
        automated: r.get(13)?,
        asked: r.get(14)?,
        outcome: r.get(15)?,
        stored: Stored::parse(&r.get::<_, String>(16)?),
        ending_detail: r.get(17)?,
        ending_at: ts(18)?,
        project_weight: n(19)?,
        branch: r.get(20)?,
    })
}

fn pull_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Pull> {
    Ok(Pull {
        session_id: r.get(0)?,
        url: r.get(1)?,
        done: PullDone::parse(&r.get::<_, String>(2)?).unwrap_or(PullDone::Opened),
        ts: r.get(3)?,
        title: r.get(4)?,
    })
}

impl Rollup {
    /// Opens `<home>/rollup.sqlite` to read. `None` when there is none or
    /// it is of another format: a reader then has no rollup until the next
    /// `hippo ingest` writes one. Changes nothing.
    pub fn open(home: &Path) -> Result<Option<Rollup>> {
        let path = home.join(FILE_NAME);
        if !path.exists() {
            return Ok(None);
        }
        let conn = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        Ok((version == FORMAT).then_some(Rollup { conn }))
    }

    /// A rollup in memory for the sessions `ids` alone, computed from
    /// `index` now: what a reader uses before the first `hippo ingest` that
    /// writes the file. Slow for many sessions; writes nothing to disk.
    pub fn in_memory(ws: Option<&Workspace>, index: &Index, ids: &[String]) -> Result<Rollup> {
        let mut conn = Connection::open_in_memory()?;
        create(&conn)?;
        refresh_into(&mut conn, ws, index, Some(ids))?;
        Ok(Rollup { conn })
    }

    /// The incidents at or after `since`, newest first.
    pub fn incidents(&self, since: OffsetDateTime) -> Result<Vec<crate::incidents::Incident>> {
        crate::incidents::read(&self.conn, since)
    }

    /// One session's facts.
    pub fn session(&self, id: &str) -> Result<Option<SessionFacts>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {SESSION_COLUMNS} FROM sessions WHERE id = ?1"
        ))?;
        Ok(stmt.query_row([id], row_facts).optional()?)
    }

    /// Every session with a turn in `[start, end)`, with its turns and
    /// edits inside the range, most turns first.
    pub fn sessions_in(&self, start: OffsetDateTime, end: OffsetDateTime) -> Result<Vec<InRange>> {
        let columns: Vec<String> = SESSION_COLUMNS
            .split(", ")
            .map(|c| format!("s.{c}"))
            .collect();
        let sql = format!(
            "SELECT {}, a.turns, a.edits, a.lo, a.hi FROM (SELECT session_id, SUM(turns) AS turns, SUM(edits) AS edits, MIN(bucket) AS lo, MAX(bucket) AS hi FROM activity WHERE bucket >= ?1 AND bucket < ?2 GROUP BY session_id) a JOIN sessions s ON s.id = a.session_id ORDER BY a.turns DESC, s.id",
            columns.join(", ")
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let inside = |t: Option<OffsetDateTime>| t.filter(|t| *t >= start && *t < end);
        let at = |bucket: i64| OffsetDateTime::from_unix_timestamp(bucket * BUCKET_SECONDS).ok();
        let rows = stmt.query_map(params![bucket_of(start), bucket_of(end)], |r| {
            let facts = row_facts(r)?;
            let (lo, hi): (i64, i64) = (r.get(23)?, r.get(24)?);
            Ok(InRange {
                first: inside(facts.first_turn).or_else(|| at(lo)),
                last: inside(facts.last_turn).or_else(|| at(hi + 1).map(|t| t.min(end))),
                turns: u64::try_from(r.get::<_, i64>(21)?).unwrap_or(0),
                edits: u64::try_from(r.get::<_, i64>(22)?).unwrap_or(0),
                facts,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Turns and edits per bucket in `[start, end)`, every session summed:
    /// `(bucket start, turns, edits)`, in time order, empty buckets left
    /// out.
    pub fn activity(
        &self,
        start: OffsetDateTime,
        end: OffsetDateTime,
    ) -> Result<Vec<(OffsetDateTime, u64, u64)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT bucket, SUM(turns), SUM(edits) FROM activity WHERE bucket >= ?1 AND bucket < ?2 GROUP BY bucket ORDER BY bucket",
        )?;
        let rows = stmt.query_map(params![bucket_of(start), bucket_of(end)], |r| {
            let bucket: i64 = r.get(0)?;
            Ok((
                bucket,
                u64::try_from(r.get::<_, i64>(1)?).unwrap_or(0),
                u64::try_from(r.get::<_, i64>(2)?).unwrap_or(0),
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (bucket, turns, edits) = row?;
            if let Ok(t) = OffsetDateTime::from_unix_timestamp(bucket * BUCKET_SECONDS) {
                out.push((t, turns, edits));
            }
        }
        Ok(out)
    }

    /// The pull requests opened or merged in `[start, end)`, oldest first.
    pub fn pulls_in(&self, start: OffsetDateTime, end: OffsetDateTime) -> Result<Vec<Pull>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT session_id, url, done, ts, title FROM pulls WHERE ts >= ?1 AND ts < ?2 ORDER BY ts",
        )?;
        let rows = stmt.query_map(params![fixed(start), fixed(end)], pull_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The pull requests session `id` opened or merged, oldest first.
    pub fn pulls_of(&self, id: &str) -> Result<Vec<Pull>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT session_id, url, done, ts, title FROM pulls WHERE session_id = ?1 ORDER BY ts",
        )?;
        let rows = stmt.query_map([id], pull_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The files session `id` changed, in the order its edits first named
    /// them.
    pub fn changes_of(&self, id: &str) -> Result<Vec<Changed>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT path, added, removed, calls, uncounted FROM changes WHERE session_id = ?1 ORDER BY ord",
        )?;
        let n = |r: &rusqlite::Row<'_>, i: usize| -> rusqlite::Result<u64> {
            Ok(u64::try_from(r.get::<_, i64>(i)?).unwrap_or(0))
        };
        let rows = stmt.query_map([id], |r| {
            Ok(Changed {
                path: r.get(0)?,
                lines: Lines {
                    added: n(r, 1)?,
                    removed: n(r, 2)?,
                },
                calls: n(r, 3)?,
                uncounted: n(r, 4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The sub-tasks session `id` started, in call order.
    pub fn tasks_of(&self, id: &str) -> Result<Vec<Task>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT kind, title, started, ended, status, ref, thread, child, input, report, tokens, tools, exit FROM tasks WHERE session_id = ?1 ORDER BY ord",
        )?;
        let rows = stmt.query_map([id], task_of_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The sessions session `id` started: its Claude Code
    /// sub-agents (`parent`, workflow agents included) and the sessions its
    /// delegated tasks ran as, oldest first, each once.
    pub fn children_of(&self, id: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id FROM (SELECT id, first_turn FROM sessions WHERE parent = ?1              UNION SELECT s.id, s.first_turn FROM tasks t JOIN sessions s ON s.id = t.child              WHERE t.session_id = ?1 AND t.kind = 'delegate') ORDER BY first_turn, id",
        )?;
        let rows = stmt.query_map([id], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Every session below `id`: its children, theirs, and so on, each
    /// once, never `id` itself.
    pub fn descendants_of(&self, id: &str) -> Result<Vec<String>> {
        let mut out: Vec<String> = Vec::new();
        let mut seen: HashSet<String> = HashSet::from([id.to_owned()]);
        let mut queue = vec![id.to_owned()];
        while let Some(at) = queue.pop() {
            for c in self.children_of(&at)? {
                if seen.insert(c.clone()) {
                    out.push(c.clone());
                    queue.push(c);
                }
            }
        }
        Ok(out)
    }

    /// The session whose delegated task ran as session `id`, with that
    /// task.
    pub fn delegated_by(&self, id: &str) -> Result<Option<(String, Task)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT session_id, kind, title, started, ended, status, ref, thread, child, input, report, tokens, tools, exit              FROM tasks WHERE child = ?1 AND kind = 'delegate' ORDER BY started DESC LIMIT 1",
        )?;
        let got = stmt
            .query_row([id], |r| {
                let parent: String = r.get(0)?;
                let task = task_of_row_at(r, 1)?;
                Ok((parent, task))
            })
            .optional()?;
        Ok(got)
    }

    /// Every delegated session the rollup links: child → parent.
    pub fn delegations(&self) -> Result<HashMap<String, String>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT child, session_id FROM tasks WHERE kind = 'delegate' AND child IS NOT NULL",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Whether any session the rollup holds merged the pull request at
    /// `url`.
    pub fn merged_by_any(&self, url: &str) -> Result<bool> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT EXISTS(SELECT 1 FROM pulls WHERE url = ?1 AND done = 'merged')",
        )?;
        Ok(stmt.query_row([url], |r| r.get(0))?)
    }

    /// Failures along the way in `[start, end)`, oldest first.
    pub fn events_in(&self, start: OffsetDateTime, end: OffsetDateTime) -> Result<Vec<Event>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT session_id, ts, kind, detail FROM events WHERE ts >= ?1 AND ts < ?2 ORDER BY ts",
        )?;
        let rows = stmt.query_map(params![fixed(start), fixed(end)], |r| {
            Ok(Event {
                session_id: r.get(0)?,
                ts: r.get(1)?,
                kind: r.get(2)?,
                detail: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The notes session `id` read or edited.
    pub fn notes_of(&self, id: &str) -> Result<Vec<NoteSeen>> {
        self.notes_where("session_id = ?1", id)
    }

    /// The sessions that read or edited `note` (relative to the
    /// workspace), newest first.
    pub fn sessions_of_note(&self, note: &Path) -> Result<Vec<NoteSeen>> {
        self.notes_where("note = ?1", &note.to_string_lossy())
    }

    fn notes_where(&self, filter: &str, value: &str) -> Result<Vec<NoteSeen>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT session_id, note, kind, ts FROM notes WHERE {filter} ORDER BY ts DESC"
        ))?;
        let rows = stmt.query_map([value], |r| {
            Ok(NoteSeen {
                session_id: r.get(0)?,
                note: PathBuf::from(r.get::<_, String>(1)?),
                kind: r.get(2)?,
                ts: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Every note a session read, discussed or edited with its first
    /// mention in `[start, end)`, newest first: the Home's "Your memory,
    /// this week".
    pub fn notes_in(&self, start: OffsetDateTime, end: OffsetDateTime) -> Result<Vec<NoteSeen>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT session_id, note, kind, ts FROM notes ORDER BY ts DESC")?;
        let rows = stmt.query_map([], |r| {
            Ok(NoteSeen {
                session_id: r.get(0)?,
                note: PathBuf::from(r.get::<_, String>(1)?),
                kind: r.get(2)?,
                ts: r.get(3)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            let row = row?;
            if parse_ts(&row.ts).is_some_and(|t| t >= start && t < end) {
                out.push(row);
            }
        }
        Ok(out)
    }

    /// A session's claims, in turn order.
    pub fn claims_of(&self, id: &str) -> Result<Vec<crate::xray::Claim>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT kind, status, claim, evidence, ordinal, pr FROM claims WHERE session_id = ?1 ORDER BY ord",
        )?;
        let rows = stmt.query_map([id], |r| {
            let kind: String = r.get(0)?;
            let status: String = r.get(1)?;
            Ok((
                kind,
                status,
                r.get(2)?,
                r.get(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, Option<i64>>(5)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (kind, status, claim, evidence, ordinal, pr) = row?;
            let Some(kind) = crate::xray::ClaimKind::parse(&kind) else {
                continue;
            };
            out.push(crate::xray::Claim {
                kind,
                status: crate::xray::Status::parse(&status),
                claim,
                evidence,
                ordinal: u32::try_from(ordinal).unwrap_or(0),
                pr: pr.and_then(|n| u64::try_from(n).ok()),
            });
        }
        Ok(out)
    }

    /// A session's context parts and their use, as the
    /// last refresh classified them; empty when its block was not kept.
    pub fn context_of(&self, id: &str) -> Result<Vec<crate::xray::Part>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT kind, label, note, bytes, use FROM context WHERE session_id = ?1 ORDER BY ord",
        )?;
        let rows = stmt.query_map([id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, String>(4)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (kind, label, note, bytes, used) = row?;
            out.push(crate::xray::Part {
                kind: crate::xray::PartKind::parse(&kind),
                label,
                note: note.map(PathBuf::from),
                text: String::new(),
                bytes: usize::try_from(bytes).unwrap_or(0),
                note_bytes: None,
                used: crate::xray::Use::parse(&used),
                read_at: None,
                cited_at: None,
                cited: 0,
            });
        }
        Ok(out)
    }

    /// How many sessions' recorded start blocks carried `note` (relative
    /// to the workspace), and how many of those ignored it: the Memory
    /// note page's stats.
    pub fn note_injections(&self, note: &Path) -> Result<(u32, u32)> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT COUNT(DISTINCT session_id), COUNT(DISTINCT CASE WHEN use = 'ignored' THEN session_id END) FROM context WHERE note = ?1",
        )?;
        let (n, ignored): (i64, i64) =
            stmt.query_row([note.to_string_lossy()], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok((
            u32::try_from(n).unwrap_or(0),
            u32::try_from(ignored).unwrap_or(0),
        ))
    }

    /// The person's top-level sessions whose last turn is in `[start,
    /// end)`, by id.
    pub fn top_level_in(&self, start: OffsetDateTime, end: OffsetDateTime) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, last_turn FROM sessions WHERE parent IS NULL AND automated = 0 AND last_turn IS NOT NULL ORDER BY id",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut out = Vec::new();
        for row in rows {
            let (id, last) = row?;
            let Ok(t) =
                OffsetDateTime::parse(&last, &time::format_description::well_known::Rfc3339)
            else {
                continue;
            };
            if t >= start && t < end {
                out.push(id);
            }
        }
        Ok(out)
    }

    /// Every session's title, by id, where it has one.
    pub fn titles(&self) -> Result<HashMap<String, String>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT id, title FROM sessions WHERE title IS NOT NULL")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The whole rollup in a line: top-level sessions a person started,
    /// the profiles they ran under, and the first turn.
    pub fn summary(&self) -> Result<Summary> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT tool, transcript_path, parent IS NULL AND NOT automated, first_turn FROM sessions",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, bool>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        })?;
        let mut out = Summary::default();
        let mut profiles = std::collections::BTreeSet::new();
        for row in rows {
            let (tool, path, top, first) = row?;
            if top {
                out.sessions += 1;
            }
            if let Some(p) = profile_of(&tool, Path::new(&path)) {
                profiles.insert(p);
            }
            if let Some(t) = first.as_deref().and_then(parse_ts) {
                out.first = Some(out.first.map_or(t, |f: OffsetDateTime| f.min(t)));
            }
        }
        out.profiles = profiles.into_iter().collect();
        Ok(out)
    }

    /// Every local day at `offset` with a turn, newest first, with its
    /// totals. One pass over the activity rows.
    pub fn days(&self, offset: UtcOffset) -> Result<Vec<DayTotals>> {
        let kinds: HashMap<String, (bool, bool)> = {
            let mut stmt = self
                .conn
                .prepare_cached("SELECT id, parent IS NOT NULL, automated FROM sessions")?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let mut days: BTreeMap<Date, (DayTotals, Vec<String>, Vec<String>)> = BTreeMap::new();
        let shift = i64::from(offset.whole_seconds());
        let mut stmt = self
            .conn
            .prepare_cached("SELECT session_id, bucket, turns, edits FROM activity")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })?;
        for row in rows {
            let (id, bucket, turns, edits) = row?;
            let Ok(t) = OffsetDateTime::from_unix_timestamp(bucket * BUCKET_SECONDS + shift) else {
                continue;
            };
            let entry = days.entry(t.date()).or_default();
            entry.0.turns += u64::try_from(turns).unwrap_or(0);
            entry.0.edits += u64::try_from(edits).unwrap_or(0);
            match kinds.get(&id) {
                Some((true, _)) => entry.2.push(id),
                Some((false, false)) => entry.1.push(id),
                _ => {}
            }
        }
        let mut pulls: HashMap<Date, Vec<String>> = HashMap::new();
        let mut stmt = self.conn.prepare_cached("SELECT url, ts FROM pulls")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            let (url, ts) = row?;
            if let Some(t) = parse_ts(&ts) {
                let day = pulls.entry(t.to_offset(offset).date()).or_default();
                if !day.contains(&url) {
                    day.push(url);
                }
            }
        }
        Ok(days
            .into_iter()
            .rev()
            .map(|(date, (mut totals, mut sessions, mut agents))| {
                sessions.sort();
                sessions.dedup();
                agents.sort();
                agents.dedup();
                totals.date = Some(date);
                totals.sessions = sessions.len();
                totals.agents = agents.len();
                totals.pulls = pulls.get(&date).map_or(0, Vec::len);
                totals
            })
            .collect())
    }
}

/// `t` in the index's fixed-width UTC form, which sorts as time.
fn fixed(t: OffsetDateTime) -> String {
    let format = time::macros::format_description!(
        "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:6]Z"
    );
    t.to_offset(UtcOffset::UTC)
        .format(&format)
        .unwrap_or_default()
}

/// A [`Task`] from a `tasks` row's columns from `kind` on.
fn task_of_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Task> {
    task_of_row_at(r, 0)
}

fn task_of_row_at(r: &rusqlite::Row<'_>, at: usize) -> rusqlite::Result<Task> {
    let n = |i: usize| -> rusqlite::Result<Option<u64>> {
        Ok(r.get::<_, Option<i64>>(at + i)?
            .and_then(|v| u64::try_from(v).ok()))
    };
    Ok(Task {
        kind: TaskKind::parse(&r.get::<_, String>(at)?).unwrap_or(TaskKind::Agent),
        title: r.get(at + 1)?,
        started: r.get(at + 2)?,
        ended: r.get(at + 3)?,
        status: r.get(at + 4)?,
        id: r.get(at + 5)?,
        thread: r.get(at + 6)?,
        child: r.get(at + 7)?,
        input: r.get(at + 8)?,
        report: r.get(at + 9)?,
        tokens: n(10)?,
        tools: n(11)?,
        exit: r.get(at + 12)?,
    })
}

#[cfg(test)]
mod tests;
