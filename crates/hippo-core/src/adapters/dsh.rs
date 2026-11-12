//! DeepSeek Harness (`dsh`) session logs, as `docs/designs/dsh-adapter.md`
//! describes them.
//!
//! dsh's JSONL backend keeps each session in its own folder,
//! `<root>/--<project>--/<escaped id>/` (`_no-cwd` for a session without a
//! working directory), as `session.v<N>.jsonl`, or `session.jsonl` for
//! format 0, Zstandard-compressed as `….jsonl.zstd` by default. A folder can
//! hold several generations; only the highest is the session.
//!
//! The first record of type `session` is the header (`version`, `id`,
//! `createdAt` in Unix milliseconds, `cwd`, `parentSession`, `isSeeded`,
//! `origin`); anything before it is skipped. Formats 3 and 4 are read; a
//! header with any other version is an error. Every later line is one event,
//! `{"type", "seq", "time", "data"}`, `time` in Unix milliseconds. Turns,
//! each stamped with its event's `time`:
//!
//! - `user/message` whose `source.kind` is `user`: one `user` turn per text
//!   block; text dsh adds itself (other kinds) is skipped;
//! - `assistant/message`: one `assistant` turn per non-empty text block;
//!   reasoning and `tool-call` blocks are not turns;
//! - `tool/call`: one `tool` turn, `<name> <arguments>`, with the core tools
//!   stored under Claude Code's names ([`stored_name`]);
//! - `tool/result`: one `tool` turn named by its call, whose text is the
//!   result's text blocks, when there are any. A failed result is written
//!   the way hippo's readers know a failure ([`result_text`]).
//!
//! In a fork, the events up to the last `session/end-seed` marked
//! `inherited` are the parent's and are skipped. A sub-agent (`origin:
//! "subagent"`) is `dsh-<top>-agent-<id>`, `<top>` the session at the top
//! of its `parentSession` chain under the same root; any other session is
//! `dsh-<id>`.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

use ruzstd::decoding::errors::{FrameDecoderError, ReadFrameHeaderError};
use ruzstd::decoding::{BlockDecodingStrategy, FrameDecoder};
use serde_json::Value;
use time::OffsetDateTime;

use super::{each_json_line_in, invalid_data, push_turn, Adapter, Parsed, RawTurn, Role, Tool};
use crate::config::Config;
use crate::error::{Error, Result};
use crate::util::{hex, sha256};

/// The log formats this adapter reads.
pub const VERSIONS: [u64; 2] = [3, 4];

/// The folder dsh keeps sessions in by default, under `$DSH_HOME`.
pub const DEFAULT_ROOT: &str = "~/.dsh/sessions";

/// The longest id part kept as written; a longer one, or one with other
/// characters, is replaced by a hash of it.
const ID_CHARS: usize = 52;

/// How many parents up a sub-agent's chain is followed.
const CHAIN_DEPTH: usize = 16;

/// How many lines are read looking for the header.
const HEAD_LINES: usize = 8;

/// The dsh adapter.
#[derive(Debug, Clone, Copy, Default)]
pub struct Dsh;

impl Adapter for Dsh {
    fn tool(&self) -> Tool {
        Tool::Dsh
    }

    fn roots(&self, cfg: &Config) -> Vec<PathBuf> {
        super::transcript_roots(cfg)
    }

    fn file_matches(&self, path: &Path) -> bool {
        is_log(path) && is_newest(path)
    }

    fn parse(&self, _root: &Path, path: &Path) -> Result<Parsed> {
        let log = read(path)?;
        let id = session_id(path, &log.header);
        let mut call_names: HashMap<String, String> = HashMap::new();
        let mut turns = Vec::new();
        for event in &log.events[log.own_from..] {
            add_event(&mut turns, &mut call_names, path, event);
        }
        Ok(Parsed {
            id,
            tool: Tool::Dsh,
            started: log.header.created,
            turns,
        })
    }

    fn cwd(&self, _id: &str, path: &Path) -> Option<PathBuf> {
        header(path).and_then(|h| h.cwd)
    }
}

/// A log's header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub version: u64,
    /// dsh's session id, as written.
    pub id: String,
    pub created: OffsetDateTime,
    pub cwd: Option<PathBuf>,
    /// The session that created this one, for a sub-agent.
    pub parent: Option<String>,
    /// Started by another session's sub-agent tool (`origin: "subagent"`).
    pub sub_agent: bool,
    /// Holds history copied from another session (a fork).
    pub seeded: bool,
}

/// A whole log.
#[derive(Debug, Clone)]
pub struct Log {
    pub header: Header,
    /// Every event after the header, in order.
    pub events: Vec<Value>,
    /// The index of the first event that is this session's own: after the
    /// last inherited `session/end-seed` marker of a fork, else 0.
    pub own_from: usize,
}

/// A dsh log file name's format version: `session.jsonl` is 0,
/// `session.v<N>.jsonl` is N, either optionally `.zstd`. `None` for any
/// other name, a leading zero or `v0` included, as dsh reads them.
pub fn name_version(path: &Path) -> Option<u64> {
    let name = path.file_name()?.to_str()?;
    let name = name.strip_suffix(".zstd").unwrap_or(name);
    let stem = name.strip_suffix(".jsonl")?;
    if stem == "session" {
        return Some(0);
    }
    let digits = stem.strip_prefix("session.v")?;
    let canonical = !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit())
        && !digits.starts_with('0');
    canonical.then(|| digits.parse().ok()).flatten()
}

/// Whether `path` is a dsh session log: a log name in a session folder of
/// a project folder (`--<project>--` or `_no-cwd`).
pub fn is_log(path: &Path) -> bool {
    name_version(path).is_some()
        && path
            .parent()
            .and_then(Path::parent)
            .and_then(Path::file_name)
            .and_then(|n| n.to_str())
            .is_some_and(|n| {
                n == "_no-cwd" || (n.len() > 4 && n.starts_with("--") && n.ends_with("--"))
            })
}

/// Whether no other log in `path`'s folder has a higher version: the one
/// dsh reads. Of two with the same version, the compressed one.
fn is_newest(path: &Path) -> bool {
    let Some(mine) = name_version(path) else {
        return false;
    };
    let rank = |p: &Path, v: u64| (v, p.extension().is_some_and(|e| e == "zstd"));
    let me = rank(path, mine);
    let Some(dir) = path.parent() else {
        return true;
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return true;
    };
    !entries.flatten().any(|e| {
        let other = e.path();
        other != path && name_version(&other).is_some_and(|v| rank(&other, v) > me)
    })
}

/// The newest log of each session folder under `root`, as ingest claims
/// them.
pub fn logs_under(root: &Path) -> Vec<PathBuf> {
    walkdir::WalkDir::new(root)
        .max_depth(3)
        .into_iter()
        .flatten()
        .filter(|e| e.file_type().is_file() && is_log(e.path()) && is_newest(e.path()))
        .map(|e| e.into_path())
        .collect()
}

/// The newest log of each session under `roots` modified at or after
/// `since` (Unix seconds).
pub fn logs_since(roots: &[PathBuf], since: i64) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for root in roots {
        for path in logs_under(root) {
            let modified = std::fs::metadata(&path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
            if modified >= since {
                out.push(path);
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Whether an event is a turn's: a message, a tool call or a result.
pub fn is_turn_event(event: &Value) -> bool {
    matches!(
        event.get("type").and_then(Value::as_str),
        Some("user/message" | "assistant/message" | "tool/call" | "tool/result")
    )
}

/// The time of the newest turn event of the log at `path`: a message
/// (dsh's own included), a call or a result.
pub fn last_turn_at(path: &Path) -> Option<OffsetDateTime> {
    newest_turn_at(&read(path).ok()?)
}

/// The time of the newest turn event of `log`.
pub fn newest_turn_at(log: &Log) -> Option<OffsetDateTime> {
    log.events[log.own_from..]
        .iter()
        .rev()
        .filter(|e| is_turn_event(e))
        .find_map(|e| millis(e.get("time")))
}

/// A tool call waiting for the person's approval: an `approval/asked` with
/// no `approval/decided` after it, among the log's own events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Approval {
    pub at: OffsetDateTime,
    /// The tool's stored name.
    pub tool: String,
    /// Why the call needs approval, as dsh wrote it (not redacted).
    pub reason: Option<String>,
}

/// The newest unanswered approval in `log`, when no turn event came after
/// it.
pub fn pending_approval(log: &Log) -> Option<Approval> {
    let own = &log.events[log.own_from..];
    let mut decided: Vec<&str> = Vec::new();
    for event in own.iter().rev() {
        let kind = event.get("type").and_then(Value::as_str).unwrap_or("");
        let id = event.pointer("/data/id").and_then(Value::as_str);
        match kind {
            "approval/decided" => decided.extend(id),
            "approval/asked" if id.is_none_or(|id| !decided.contains(&id)) => {
                return Some(Approval {
                    at: millis(event.get("time"))?,
                    tool: stored_name(
                        event
                            .pointer("/data/toolName")
                            .and_then(Value::as_str)
                            .unwrap_or(""),
                    )
                    .to_owned(),
                    reason: event
                        .pointer("/data/reason")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                });
            }
            "user/message" | "assistant/message" | "tool/result" => return None,
            _ => {}
        }
    }
    None
}

/// The log's last `max` own turn events as Claude Code transcript records,
/// one JSON line each, oldest first: the shape Fleet's tail reader knows.
/// A person's message is a `user` record of text; a reply an `assistant`
/// record of its text blocks and `tool_use` blocks (stored names, parsed
/// arguments), its `usage` in Claude Code's keys, and `stop_reason`
/// `end_turn` when it makes no call; a result a `user` record of one
/// `tool_result`. Every record carries `timestamp`. Nothing is redacted:
/// these lines are read and dropped, never stored.
pub fn as_claude_lines(log: &Log, max: usize) -> Vec<String> {
    use serde_json::json;
    let own: Vec<&Value> = log.events[log.own_from..]
        .iter()
        .filter(|e| is_turn_event(e))
        .collect();
    let from = own.len().saturating_sub(max);
    let rfc = |e: &Value| {
        millis(e.get("time")).and_then(|t| {
            t.format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
    };
    let mut out = Vec::new();
    for e in &own[from..] {
        let Some(ts) = rfc(e) else { continue };
        let data = e.get("data").cloned().unwrap_or(Value::Null);
        let record = match e.get("type").and_then(Value::as_str) {
            Some("user/message") => {
                if data.pointer("/source/kind").and_then(Value::as_str) != Some("user") {
                    continue;
                }
                let text = text_of(data.get("content"));
                json!({"type": "user", "timestamp": ts,
                       "message": {"role": "user", "content": [{"type": "text", "text": text}]}})
            }
            Some("assistant/message") => {
                let mut content = Vec::new();
                let mut calls = false;
                for b in blocks(data.pointer("/message/content")) {
                    match b.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            content.push(json!({"type": "text", "text": b.get("text")}))
                        }
                        Some("tool-call") => {
                            calls = true;
                            let input = b
                                .get("arguments")
                                .and_then(Value::as_str)
                                .and_then(|a| serde_json::from_str::<Value>(a).ok())
                                .unwrap_or(Value::Null);
                            let name = b.get("name").and_then(Value::as_str).unwrap_or("");
                            content.push(json!({"type": "tool_use", "id": b.get("id"),
                                                "name": stored_name(name), "input": input}));
                        }
                        _ => {}
                    }
                }
                let n = |k: &str| {
                    data.pointer(&format!("/usage/{k}"))
                        .and_then(Value::as_u64)
                        .unwrap_or(0)
                };
                let interrupted = data.get("interrupted") == Some(&Value::Bool(true));
                let stop = (!calls && !interrupted).then_some("end_turn");
                json!({"type": "assistant", "timestamp": ts,
                       "message": {"role": "assistant", "content": content, "stop_reason": stop,
                                   "usage": {"input_tokens": n("inputTokens"),
                                             "cache_read_input_tokens": n("cacheReadTokens"),
                                             "cache_creation_input_tokens": n("cacheWriteTokens")}}})
            }
            Some("tool/result") => {
                let id = result_call_id(&data).unwrap_or("");
                json!({"type": "user", "timestamp": ts,
                       "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": id,
                                   "content": text_of(data.pointer("/message/content"))}]}})
            }
            _ => continue,
        };
        out.push(record.to_string());
    }
    out
}

/// The lines of the log at `path`, decompressed when its name ends in
/// `.zstd`.
pub fn open(path: &Path) -> io::Result<Box<dyn BufRead>> {
    let file = BufReader::with_capacity(1 << 16, File::open(path)?);
    if path.extension().is_some_and(|e| e == "zstd") {
        Ok(Box::new(BufReader::with_capacity(
            1 << 16,
            Frames::new(file),
        )))
    } else {
        Ok(Box::new(file))
    }
}

/// The header of the log at `path`, whatever its version; `None` when the
/// file cannot be read or has none in its first lines.
pub fn header(path: &Path) -> Option<Header> {
    let reader = open(path).ok()?;
    for line in reader.lines().take(HEAD_LINES) {
        let Ok(line) = line else { return None };
        if let Ok(v) = serde_json::from_str::<Value>(&line) {
            if let Some(h) = header_of(&v) {
                return Some(h);
            }
        }
    }
    None
}

/// The whole log at `path`. An error when it cannot be read, has no
/// header, or is in a format version this adapter does not read.
pub fn read(path: &Path) -> Result<Log> {
    let reader = open(path).map_err(|e| Error::io_at(path, e))?;
    let mut header: Option<Header> = None;
    let mut bad_header = false;
    let mut events = Vec::new();
    each_json_line_in(reader, path, |v| {
        if header.is_some() {
            events.push(v);
        } else if v.get("type").and_then(Value::as_str) == Some("session") {
            header = header_of(&v);
            bad_header = header.is_none();
        }
    })?;
    let Some(header) = header else {
        let what = if bad_header {
            "the dsh header is not the shape hippo reads"
        } else {
            "no dsh session header"
        };
        return Err(invalid_data(path, what));
    };
    if !VERSIONS.contains(&header.version) {
        return Err(invalid_data(path, &unsupported(header.version)));
    }
    let own_from = if header.seeded {
        events
            .iter()
            .rposition(|e| {
                e.get("type").and_then(Value::as_str) == Some("session/end-seed")
                    && e.pointer("/data/inherited") == Some(&Value::Bool(true))
            })
            .map_or(0, |i| i + 1)
    } else {
        0
    };
    Ok(Log {
        header,
        events,
        own_from,
    })
}

/// The message for a log in format `version`, which is not read.
pub fn unsupported(version: u64) -> String {
    format!("dsh log format v{version} is not one hippo reads (it reads v3 and v4)")
}

fn header_of(v: &Value) -> Option<Header> {
    if v.get("type").and_then(Value::as_str) != Some("session") {
        return None;
    }
    let id = v
        .get("id")?
        .as_str()
        .filter(|id| !id.is_empty())?
        .to_owned();
    let str_of = |k: &str| v.get(k).and_then(Value::as_str).filter(|s| !s.is_empty());
    Some(Header {
        version: v.get("version")?.as_u64()?,
        id,
        created: millis(v.get("createdAt"))?,
        cwd: str_of("cwd").map(PathBuf::from),
        parent: str_of("parentSession").map(str::to_owned),
        sub_agent: str_of("origin") == Some("subagent"),
        seeded: v.get("isSeeded").and_then(Value::as_bool).unwrap_or(false),
    })
}

/// Unix milliseconds as a UTC time.
pub fn millis(field: Option<&Value>) -> Option<OffsetDateTime> {
    let ms = i128::from(field?.as_i64()?);
    OffsetDateTime::from_unix_timestamp_nanos(ms * 1_000_000).ok()
}

/// An id part as hippo writes it: as written when it is letters, digits,
/// `_` and `-`, at most [`ID_CHARS`] long; else `h` and a hash of it.
pub(crate) fn id_part(id: &str) -> String {
    let plain = !id.is_empty()
        && id.len() <= ID_CHARS
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if plain {
        id.to_owned()
    } else {
        format!("h{}", &hex(&sha256(id.as_bytes()))[..32])
    }
}

/// hippo's id for the session whose log is `path` and whose header is `h`.
pub fn session_id(path: &Path, h: &Header) -> String {
    match parent_id(path, h) {
        Some(parent) => format!("{parent}-agent-{}", id_part(&h.id)),
        None => format!("dsh-{}", id_part(&h.id)),
    }
}

/// hippo's id for the session a sub-agent's log at `path` belongs under:
/// `dsh-<top>`. `None` for a session that is not a sub-agent.
pub fn parent_id(path: &Path, h: &Header) -> Option<String> {
    match (h.sub_agent, h.parent.as_deref()) {
        (true, Some(parent)) => Some(format!("dsh-{}", id_part(&top_of(root_of(path), parent)))),
        _ => None,
    }
}

/// What a log says about its session beyond its turns, each the newest
/// the log holds, as written (not yet redacted).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Facts {
    /// `session/title`'s `title`.
    pub title: Option<String>,
    /// A sub-agent's `subagent/descriptor` `label`.
    pub label: Option<String>,
    /// Its `agentModel`.
    pub agent_model: Option<String>,
    /// `request/header`'s `header.config.model` and `reasoningEffort`.
    pub model: Option<String>,
    pub effort: Option<String>,
}

/// The [`Facts`] of the log at `path`. Only lines that name one of the
/// events read are parsed; a log that cannot be read gives nothing.
pub fn facts(path: &Path) -> Facts {
    let mut out = Facts::default();
    let Ok(reader) = open(path) else { return out };
    let kinds = ["session/title", "subagent/descriptor", "request/header"];
    let text = |v: &Value, at: &str| {
        v.pointer(at)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    for line in reader.lines() {
        let Ok(line) = line else { break };
        let Some(kind) = kinds
            .iter()
            .find(|k| line.contains(&format!("\"type\":\"{k}\"")))
        else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        match *kind {
            "session/title" => out.title = text(&v, "/data/title").or(out.title.take()),
            "subagent/descriptor" => {
                out.label = text(&v, "/data/label").or(out.label.take());
                out.agent_model = text(&v, "/data/agentModel").or(out.agent_model.take());
            }
            _ => {
                if let Some(m) = text(&v, "/data/header/config/model") {
                    out.model = Some(m);
                    out.effort = text(&v, "/data/header/config/reasoningEffort");
                }
            }
        }
    }
    out
}

/// The root a log is under: its session folder's project folder's parent.
fn root_of(path: &Path) -> Option<&Path> {
    path.parent()?.parent()?.parent()
}

/// The session at the top of `parent`'s chain under `root`: `parent`
/// itself when its log is not there or it is not a sub-agent.
fn top_of(root: Option<&Path>, parent: &str) -> String {
    let mut top = parent.to_owned();
    let Some(root) = root else { return top };
    for _ in 0..CHAIN_DEPTH {
        let Some(h) = find(root, &top).and_then(|p| header(&p)) else {
            break;
        };
        match (h.sub_agent, h.parent) {
            (true, Some(up)) if up != top => top = up,
            _ => break,
        }
    }
    top
}

/// The newest log of session `id` under `root`, in whichever project
/// folder holds it.
pub fn find(root: &Path, id: &str) -> Option<PathBuf> {
    let segment = encode_segment(id);
    let projects = std::fs::read_dir(root).ok()?;
    for project in projects.flatten() {
        let dir = project.path().join(&segment);
        let Ok(files) = std::fs::read_dir(&dir) else {
            continue;
        };
        let newest = files
            .flatten()
            .map(|f| f.path())
            .filter(|p| name_version(p).is_some())
            .find(|p| is_newest(p));
        if newest.is_some() {
            return newest;
        }
    }
    None
}

/// dsh's escaping of a session id into one folder name: letters, digits,
/// `.`, `_` and `-` stay; every other UTF-16 code unit becomes `~XXXX`.
pub fn encode_segment(raw: &str) -> String {
    match raw {
        "." => return "~002E".to_owned(),
        ".." => return "~002E~002E".to_owned(),
        _ => {}
    }
    let mut out = String::new();
    for unit in raw.encode_utf16() {
        match char::from_u32(u32::from(unit)) {
            Some(c) if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') => out.push(c),
            _ => out.push_str(&format!("~{unit:04X}")),
        }
    }
    out
}

/// The name a dsh tool is stored under: Claude Code's name for the core
/// tools that take the same arguments, else dsh's own.
pub fn stored_name(name: &str) -> &str {
    match name {
        "bash" => "Bash",
        "read" => "Read",
        "write" => "Write",
        "edit" => "Edit",
        "glob" => "Glob",
        "grep" => "Grep",
        "web_fetch" => "WebFetch",
        "todo_write" => "TodoWrite",
        "ask_user_question" => "AskUserQuestion",
        "exit_plan_mode" => "ExitPlanMode",
        "subagent" | "subagent_fork" => "Agent",
        other => other,
    }
}

/// The `callId` a `tool/result` answers: `message.toolCallId` (format 4),
/// else `message.source.callId` (format 3).
pub fn result_call_id(data: &Value) -> Option<&str> {
    data.pointer("/message/toolCallId")
        .or_else(|| data.pointer("/message/source/callId"))
        .and_then(Value::as_str)
}

/// The text blocks of a message's `content`, joined by newlines.
pub fn text_of(content: Option<&Value>) -> String {
    content
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|b| b.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A result's text as hippo's readers of failures know it: a `Bash`
/// result whose last line is `[exit code: N]` starts `Exit code N` on a
/// line of its own instead; a result marked `isError` is wrapped in
/// `<tool_use_error>`. Anything else as written.
pub fn result_text(stored: Option<&str>, text: String, is_error: bool) -> String {
    let text = match stored {
        Some("Bash") => match exit_marker(&text) {
            Some((code, body)) => format!("Exit code {code}\n{body}"),
            None => text,
        },
        _ => text,
    };
    if is_error {
        format!("<tool_use_error>{text}</tool_use_error>")
    } else {
        text
    }
}

/// `(N, the text before it)` when the last line is `[exit code: N]`.
fn exit_marker(text: &str) -> Option<(i64, &str)> {
    let trimmed = text.trim_end();
    let (body, last) = match trimmed.rfind('\n') {
        Some(i) => (&trimmed[..i], &trimmed[i + 1..]),
        None => ("", trimmed),
    };
    let code = last
        .strip_prefix("[exit code: ")?
        .strip_suffix(']')?
        .parse()
        .ok()?;
    Some((code, body))
}

/// Adds the turns of one event, if it makes any. `call_names` maps each
/// call seen so far to its stored name.
fn add_event(
    turns: &mut Vec<RawTurn>,
    call_names: &mut HashMap<String, String>,
    path: &Path,
    event: &Value,
) {
    let kind = event.get("type").and_then(Value::as_str).unwrap_or("");
    if !matches!(
        kind,
        "user/message" | "assistant/message" | "tool/call" | "tool/result"
    ) {
        return;
    }
    let Some(ts) = millis(event.get("time")) else {
        tracing::warn!(
            "{}: skipping a {kind} event without a usable time",
            path.display()
        );
        return;
    };
    let Some(data) = event.get("data") else {
        return;
    };
    match kind {
        "user/message" => {
            if data.pointer("/source/kind").and_then(Value::as_str) != Some("user") {
                return;
            }
            for block in blocks(data.get("content")) {
                if let Some(text) = block_text(block) {
                    push_turn(turns, Role::User, ts, None, text.to_owned());
                }
            }
        }
        "assistant/message" => {
            for block in blocks(data.pointer("/message/content")) {
                if let Some(text) = block_text(block).filter(|t| !t.trim().is_empty()) {
                    push_turn(turns, Role::Assistant, ts, None, text.to_owned());
                }
            }
        }
        "tool/call" => {
            let Some(name) = data.get("name").and_then(Value::as_str) else {
                return;
            };
            let name = stored_name(name).to_owned();
            if let Some(id) = data.get("callId").and_then(Value::as_str) {
                call_names.insert(id.to_owned(), name.clone());
            }
            let text = match data.get("arguments") {
                Some(Value::String(a)) if !a.is_empty() => format!("{name} {a}"),
                Some(Value::String(_)) | None => name.clone(),
                Some(other) => format!("{name} {other}"),
            };
            push_turn(turns, Role::Tool, ts, Some(name), text);
        }
        "tool/result" => {
            let name = result_call_id(data)
                .and_then(|id| call_names.get(id))
                .cloned();
            let text = text_of(data.pointer("/message/content"));
            if text.is_empty() {
                return;
            }
            let is_error = data.pointer("/message/isError") == Some(&Value::Bool(true));
            let text = result_text(name.as_deref(), text, is_error);
            push_turn(turns, Role::Tool, ts, name, text);
        }
        _ => {}
    }
}

fn blocks(content: Option<&Value>) -> impl Iterator<Item = &Value> {
    content.and_then(Value::as_array).into_iter().flatten()
}

/// A content block's text, when it is a text block.
fn block_text(block: &Value) -> Option<&str> {
    (block.get("type").and_then(Value::as_str) == Some("text"))
        .then(|| block.get("text").and_then(Value::as_str))
        .flatten()
}

/// A run of Zstandard frames as one stream. A frame that does not decode
/// (a torn last frame, or damage) ends the stream: what came before it is
/// kept, and it and anything after are dropped with a warning.
struct Frames<R: BufRead> {
    src: R,
    dec: FrameDecoder,
    in_frame: bool,
    done: bool,
}

impl<R: BufRead> Frames<R> {
    fn new(src: R) -> Self {
        Frames {
            src,
            dec: FrameDecoder::new(),
            in_frame: false,
            done: false,
        }
    }

    fn stop(&mut self, why: &dyn std::fmt::Display) {
        tracing::warn!("dsh log: dropping a Zstandard frame that does not decode: {why}");
        self.done = true;
    }
}

impl<R: BufRead> Read for Frames<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            if self.done || buf.is_empty() {
                return Ok(0);
            }
            if !self.in_frame {
                if self.src.fill_buf()?.is_empty() {
                    self.done = true;
                    return Ok(0);
                }
                match self.dec.reset(&mut self.src) {
                    Ok(()) => self.in_frame = true,
                    Err(FrameDecoderError::ReadFrameHeaderError(
                        ReadFrameHeaderError::SkipFrame { length, .. },
                    )) => {
                        let skipped = io::copy(
                            &mut (&mut self.src).take(u64::from(length)),
                            &mut io::sink(),
                        )?;
                        if skipped < u64::from(length) {
                            self.done = true;
                        }
                    }
                    Err(e) => self.stop(&e),
                }
                continue;
            }
            if self.dec.can_collect() > 0 {
                return self.dec.read(buf);
            }
            if self.dec.is_finished() {
                self.in_frame = false;
                continue;
            }
            if let Err(e) = self
                .dec
                .decode_blocks(&mut self.src, BlockDecodingStrategy::UptoBlocks(1))
            {
                self.stop(&e);
            }
        }
    }
}

#[cfg(test)]
mod tests;
