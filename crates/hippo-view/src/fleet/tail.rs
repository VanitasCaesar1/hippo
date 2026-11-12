//! What a transcript's tail says about a session right now: its newest
//! turn, whether it stopped to ask the person something, and its newest
//! tool call.
//!
//! Read from the file's last [`TAIL_BYTES`], like [`super::last_turn_at`];
//! a dsh log from its newest [`DSH_EVENTS`] turn events, an OpenCode
//! session from its newest [`DSH_EVENTS`] messages.
//! Precision over recall: a tool call that is still running cannot be told
//! from one waiting at a permission prompt, so neither is "waiting".

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use hippo_core::adapters::{dsh, opencode};
use hippo_core::rollup::changes::{self, Lines};
use serde_json::Value;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

/// How far from its end a transcript is read.
pub const TAIL_BYTES: u64 = 256 * 1024;

/// The most characters an activity label keeps.
pub const LABEL_CHARS: usize = 40;

/// A session that stopped to ask the person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Waiting {
    pub at: OffsetDateTime,
    /// The question, as asked (its last sentence, or the first question of
    /// an `AskUserQuestion` call).
    pub asked: Option<String>,
    /// A plan waiting for approval (`ExitPlanMode`).
    pub plan: bool,
}

/// A session's newest tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Activity {
    /// `cargo test`, `editing memory.rs`, …
    pub label: String,
    pub at: OffsetDateTime,
    /// No result for it yet.
    pub running: bool,
    /// It hands work to sub-agents (`Agent`, `Task`).
    pub delegates: bool,
    /// The call as a tool line: `Edit`, `Write`, `Read`, `Bash`,
    /// `Search`, `Agent`, or the tool's own name.
    pub verb: String,
    /// The file or command it acts on, for the mono chip.
    pub target: Option<String>,
    /// The call's own description, when it has one (a step line).
    pub step: Option<String>,
    /// Lines it added and removed: its result's `structuredPatch` when the
    /// tail holds it, else counted from its input.
    pub lines: Option<Lines>,
}

/// What a transcript's tail holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tail {
    /// Its newest turn.
    pub last: Option<OffsetDateTime>,
    pub waiting: Option<Waiting>,
    pub activity: Option<Activity>,
}

/// The most calls a [`Turn`] keeps.
pub const TURN_CALLS: usize = 40;

/// The session's current turn: the person's newest request and
/// the tool calls made since, for Fleet's open row and the x-ray's
/// Activity. Main-thread records only; sub-agents' own calls are theirs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Turn {
    /// The newest request, as typed (its first 400 characters).
    pub asked: Option<String>,
    pub asked_at: Option<OffsetDateTime>,
    /// Its calls, newest first, at most [`TURN_CALLS`].
    pub calls: Vec<Activity>,
    /// How many calls it made (more than `calls` holds when long).
    pub count: usize,
    /// The context its newest reply was given, in tokens: the newest
    /// assistant record's `usage` (input plus cache reads and writes).
    pub tokens: Option<u64>,
    /// The request was further back than the tail reads.
    pub cut: bool,
}

/// Reads the current turn of the transcript at `path`.
pub fn read_turn(path: &Path) -> Turn {
    let Some(lines) = tail_lines(path) else {
        return Turn::default();
    };
    turn_of_lines(&lines.iter().map(String::as_str).collect::<Vec<_>>())
}

/// Reads a sub-agent's run from its own transcript's tail: every
/// record of it is a side chain, and its prompt is the request.
pub fn read_sub_turn(path: &Path) -> Turn {
    let Some(lines) = tail_lines(path) else {
        return Turn::default();
    };
    turn_of(&lines.iter().map(String::as_str).collect::<Vec<_>>(), true)
}

/// The calls of a run as the index holds them, newest first, at
/// most [`RUN_CALLS`]: a sub-agent's whole run, which its transcript's
/// tail may not reach. A call with no result yet is running.
pub fn turn_of_stored(turns: &[hippo_core::index::StoredTurn]) -> Turn {
    let mut turn = Turn::default();
    let pairs = hippo_core::rollup::work::pairs_of(turns);
    let empty = HashMap::new();
    for (k, &(call, result)) in pairs.iter().enumerate().rev() {
        let t = &turns[call];
        let Some(name) = t.tool_name.as_deref() else {
            continue;
        };
        let Some(ts) = OffsetDateTime::parse(&t.ts, &Rfc3339).ok() else {
            continue;
        };
        let input = t
            .text
            .get(name.len() + 1..)
            .and_then(|i| serde_json::from_str::<Value>(i).ok())
            .unwrap_or(Value::Null);
        let id = format!("stored-{k}");
        let block = serde_json::json!({"type": "tool_use", "id": id, "name": name, "input": input});
        let mut answered = HashSet::new();
        if result.is_some() {
            answered.insert(id);
        }
        turn.count += 1;
        if turn.calls.len() < RUN_CALLS {
            turn.calls
                .extend(claude_call(&block, ts, &answered, &empty));
        }
    }
    turn
}

/// The most calls [`turn_of_stored`] keeps.
pub const RUN_CALLS: usize = 400;

/// A user record's text, when it is a request the person typed (not a
/// tool result, a command's caveat or output, or a reminder).
/// With `side`, a sub-agent's own side-chain records count.
fn typed_request_in(v: &Value, side: bool) -> Option<String> {
    if v.get("isMeta") == Some(&Value::Bool(true))
        || (!side && v.get("isSidechain") == Some(&Value::Bool(true)))
        || v.get("toolUseResult").is_some()
    {
        return None;
    }
    let content = v.pointer("/message/content")?;
    let text = match content {
        Value::String(t) => t.clone(),
        Value::Array(blocks) => {
            if blocks
                .iter()
                .any(|b| b.get("type").and_then(Value::as_str) == Some("tool_result"))
            {
                return None;
            }
            blocks
                .iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        }
        _ => return None,
    };
    let t = text.trim();
    if t.is_empty()
        || t.starts_with("<command-")
        || t.starts_with("<local-command")
        || t.starts_with("<system-reminder>")
        || t.starts_with("<task-notification>")
        || t.starts_with("Caveat:")
        || t.starts_with("[Request interrupted")
    {
        return None;
    }
    Some(cut(t, 400))
}

/// [`read_turn`] on a transcript's lines, oldest first.
pub fn turn_of_lines(lines: &[&str]) -> Turn {
    turn_of(lines, false)
}

/// With `side`, a sub-agent's own transcript: side-chain records are its
/// own.
fn turn_of(lines: &[&str], side: bool) -> Turn {
    let mut turn = Turn::default();
    let mut answered: HashSet<String> = HashSet::new();
    let mut patched: HashMap<String, Lines> = HashMap::new();
    for line in lines.iter().rev() {
        let claude = line.contains("\"type\":\"user\"") || line.contains("\"type\":\"assistant\"");
        let codex = line.contains("\"type\":\"response_item\"");
        if !claude && !codex {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(ts) = at(&v) else {
            continue;
        };
        match v.get("type").and_then(Value::as_str).unwrap_or("") {
            "user" => {
                if let Some(asked) = typed_request_in(&v, side) {
                    turn.asked = Some(asked);
                    turn.asked_at = Some(ts);
                    return turn;
                }
                let lines = result_lines(v.get("toolUseResult"));
                for block in v
                    .pointer("/message/content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if block.get("type").and_then(Value::as_str) == Some("tool_result") {
                        if let Some(id) = block.get("tool_use_id").and_then(Value::as_str) {
                            answered.insert(id.to_owned());
                            if let Some(l) = lines {
                                patched.insert(id.to_owned(), l);
                            }
                        }
                    }
                }
            }
            "assistant" => {
                if !side && v.get("isSidechain") == Some(&Value::Bool(true)) {
                    continue;
                }
                if turn.tokens.is_none() {
                    if let Some(u) = v.pointer("/message/usage") {
                        let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
                        let total = n("input_tokens")
                            + n("cache_read_input_tokens")
                            + n("cache_creation_input_tokens");
                        turn.tokens = (total > 0).then_some(total);
                    }
                }
                let blocks = v.pointer("/message/content").and_then(Value::as_array);
                for block in blocks.into_iter().flatten().rev() {
                    if block.get("type").and_then(Value::as_str) != Some("tool_use") {
                        continue;
                    }
                    turn.count += 1;
                    if turn.calls.len() < TURN_CALLS {
                        turn.calls
                            .extend(claude_call(block, ts, &answered, &patched));
                    }
                }
            }
            "response_item" => {
                let Some(p) = v.get("payload") else {
                    continue;
                };
                let ptype = p.get("type").and_then(Value::as_str).unwrap_or("");
                if ptype == "function_call_output" || ptype == "custom_tool_call_output" {
                    if let Some(id) = p.get("call_id").and_then(Value::as_str) {
                        answered.insert(id.to_owned());
                    }
                } else if matches!(
                    ptype,
                    "function_call" | "custom_tool_call" | "local_shell_call"
                ) {
                    turn.count += 1;
                    if turn.calls.len() < TURN_CALLS {
                        turn.calls.extend(codex_activity(p, ts, &answered));
                    }
                } else if ptype == "message"
                    && p.get("role").and_then(Value::as_str) == Some("user")
                {
                    let text = codex_text(p);
                    let t = text.trim();
                    if !t.is_empty() && !t.starts_with('<') {
                        turn.asked = Some(cut(t, 400));
                        turn.asked_at = Some(ts);
                        return turn;
                    }
                }
            }
            _ => {}
        }
    }
    turn.cut = true;
    turn
}

/// Reads the tail of the transcript at `path`. A dsh log is read through
/// [`dsh::as_claude_lines`], and a call waiting for the person's approval
/// is a wait.
pub fn read(path: &Path) -> Tail {
    if opencode::split(path).is_some() {
        let lines = opencode::as_claude_lines(path, DSH_EVENTS);
        let mut tail = of_lines(&lines.iter().map(String::as_str).collect::<Vec<_>>());
        if let Some(t) = opencode::last_turn_at(path) {
            tail.last = Some(tail.last.map_or(t, |l| l.max(t)));
        }
        return tail;
    }
    if dsh::is_log(path) {
        let Ok(log) = dsh::read(path) else {
            return Tail::default();
        };
        let lines = dsh::as_claude_lines(&log, DSH_EVENTS);
        let mut tail = of_lines(&lines.iter().map(String::as_str).collect::<Vec<_>>());
        // dsh's own messages and the calls themselves are events too.
        if let Some(t) = dsh::newest_turn_at(&log) {
            tail.last = Some(tail.last.map_or(t, |l| l.max(t)));
        }
        if let Some(a) = dsh::pending_approval(&log) {
            let reason = a
                .reason
                .as_deref()
                .map(|r| hippo_core::redact::redact(r).as_str().to_owned());
            tail.waiting = Some(Waiting {
                at: a.at,
                asked: Some(cut(
                    &match reason {
                        Some(r) => format!("Approve {}? {}", a.tool, r.trim()),
                        None => format!("Approve {}?", a.tool),
                    },
                    120,
                )),
                plan: false,
            });
            tail.last = Some(tail.last.map_or(a.at, |l| l.max(a.at)));
        }
        return tail;
    }
    let Some(lines) = tail_lines(path) else {
        return Tail::default();
    };
    of_lines(&lines.iter().map(String::as_str).collect::<Vec<_>>())
}

/// How many of a dsh log's newest turn events the tail reads.
pub const DSH_EVENTS: usize = 400;

fn tail_lines(path: &Path) -> Option<Vec<String>> {
    if opencode::split(path).is_some() {
        return Some(opencode::as_claude_lines(path, DSH_EVENTS));
    }
    if dsh::is_log(path) {
        let log = dsh::read(path).ok()?;
        return Some(dsh::as_claude_lines(&log, DSH_EVENTS));
    }
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let from = len.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(from)).ok()?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    if from > 0 && !lines.is_empty() {
        lines.remove(0);
    }
    Some(lines)
}

fn at(v: &Value) -> Option<OffsetDateTime> {
    v.get("timestamp")
        .and_then(Value::as_str)
        .and_then(|t| OffsetDateTime::parse(t, &Rfc3339).ok())
}

/// [`read`] on a transcript's lines, oldest first.
pub fn of_lines(lines: &[&str]) -> Tail {
    let mut tail = Tail::default();
    // Tool calls answered by a later record.
    let mut answered: HashSet<String> = HashSet::new();
    // Lines each answered edit changed, from its `toolUseResult`.
    let mut patched: HashMap<String, Lines> = HashMap::new();
    let mut newest_seen = false;
    for line in lines.iter().rev() {
        let turn = line.contains("\"type\":\"user\"")
            || line.contains("\"type\":\"assistant\"")
            || line.contains("\"type\":\"response_item\"");
        if !turn {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let kind = v.get("type").and_then(Value::as_str).unwrap_or("");
        let Some(ts) = at(&v) else {
            continue;
        };
        match kind {
            "user" | "assistant" => {
                let content = v.pointer("/message/content");
                if kind == "user" {
                    let lines = result_lines(v.get("toolUseResult"));
                    for block in content.and_then(Value::as_array).into_iter().flatten() {
                        if block.get("type").and_then(Value::as_str) == Some("tool_result") {
                            if let Some(id) = block.get("tool_use_id").and_then(Value::as_str) {
                                answered.insert(id.to_owned());
                                if let Some(l) = lines {
                                    patched.insert(id.to_owned(), l);
                                }
                            }
                        }
                    }
                }
                let newest = !newest_seen;
                if newest {
                    newest_seen = true;
                    tail.last = Some(ts);
                    if kind == "assistant" && v.get("isSidechain") != Some(&Value::Bool(true)) {
                        tail.waiting = claude_waiting(&v, ts, &answered);
                    }
                }
                if kind == "assistant" && tail.activity.is_none() {
                    tail.activity = claude_activity(content, ts, &answered, &patched);
                }
            }
            "response_item" => {
                let Some(p) = v.get("payload") else {
                    continue;
                };
                let ptype = p.get("type").and_then(Value::as_str).unwrap_or("");
                if ptype == "function_call_output" || ptype == "custom_tool_call_output" {
                    if let Some(id) = p.get("call_id").and_then(Value::as_str) {
                        answered.insert(id.to_owned());
                    }
                }
                // Reasoning and other bookkeeping items are not turns.
                let is_turn = matches!(
                    ptype,
                    "message" | "function_call" | "custom_tool_call" | "local_shell_call"
                ) || ptype.ends_with("_output");
                if !is_turn {
                    continue;
                }
                if !newest_seen {
                    newest_seen = true;
                    tail.last = Some(ts);
                    if ptype == "message"
                        && p.get("role").and_then(Value::as_str) == Some("assistant")
                    {
                        let text = codex_text(p);
                        if ends_with_question(&text) {
                            tail.waiting = Some(Waiting {
                                at: ts,
                                asked: last_sentence(&text),
                                plan: false,
                            });
                        }
                    }
                }
                if tail.activity.is_none() && ptype != "message" && !ptype.ends_with("_output") {
                    tail.activity = codex_activity(p, ts, &answered);
                }
            }
            _ => {}
        }
        if tail.activity.is_some() && newest_seen {
            break;
        }
    }
    tail
}

/// The newest assistant record's wait, if it is one.
fn claude_waiting(v: &Value, ts: OffsetDateTime, answered: &HashSet<String>) -> Option<Waiting> {
    if v.get("isApiErrorMessage") == Some(&Value::Bool(true)) {
        return None;
    }
    let content = v.pointer("/message/content")?.as_array()?;
    let mut text = String::new();
    let mut calls = false;
    for block in content {
        match block.get("type").and_then(Value::as_str) {
            Some("tool_use") => {
                calls = true;
                let name = block.get("name").and_then(Value::as_str).unwrap_or("");
                let id = block.get("id").and_then(Value::as_str).unwrap_or("");
                if matches!(name, "AskUserQuestion" | "ExitPlanMode") && !answered.contains(id) {
                    let asked = block
                        .pointer("/input/questions/0/question")
                        .and_then(Value::as_str)
                        .map(|q| cut(q.trim(), 120))
                        .or_else(|| {
                            // A plan's last line, where plans say what
                            // they ask (`Plan ready: … Approve?`).
                            let plan = block.pointer("/input/plan").and_then(Value::as_str)?;
                            let line = plan.lines().rev().find(|l| !l.trim().is_empty())?;
                            let line = line
                                .trim()
                                .trim_start_matches(['#', '*', '-', ' '])
                                .trim_end_matches(['*', ' ']);
                            (!line.is_empty()).then(|| cut(line, 120))
                        });
                    return Some(Waiting {
                        at: ts,
                        asked,
                        plan: name == "ExitPlanMode",
                    });
                }
            }
            Some("text") => {
                if let Some(t) = block.get("text").and_then(Value::as_str) {
                    text.push_str(t);
                }
            }
            _ => {}
        }
    }
    let stop = v.pointer("/message/stop_reason").and_then(Value::as_str);
    let ended = matches!(stop, Some("end_turn") | Some("stop_sequence"));
    if calls || !ended || !ends_with_question(&text) {
        return None;
    }
    Some(Waiting {
        at: ts,
        asked: last_sentence(&text),
        plan: false,
    })
}

/// The lines a Claude Code edit's `toolUseResult` says it changed: its
/// `structuredPatch`, or a created file's content.
fn result_lines(r: Option<&Value>) -> Option<Lines> {
    let r = r?;
    if r.get("type").and_then(Value::as_str) == Some("create") {
        let content = r.get("content").and_then(Value::as_str)?;
        return Some(Lines {
            added: content.lines().count() as u64,
            removed: 0,
        });
    }
    changes::of_structured_patch(r.get("structuredPatch")?)
}

/// The newest tool call in one assistant record's content, if any.
fn claude_activity(
    content: Option<&Value>,
    ts: OffsetDateTime,
    answered: &HashSet<String>,
    patched: &HashMap<String, Lines>,
) -> Option<Activity> {
    let block = content?
        .as_array()?
        .iter()
        .rev()
        .find(|b| b.get("type").and_then(Value::as_str) == Some("tool_use"))?;
    claude_call(block, ts, answered, patched)
}

/// One Claude Code `tool_use` block as an [`Activity`].
fn claude_call(
    block: &Value,
    ts: OffsetDateTime,
    answered: &HashSet<String>,
    patched: &HashMap<String, Lines>,
) -> Option<Activity> {
    let name = block.get("name").and_then(Value::as_str).unwrap_or("");
    let id = block.get("id").and_then(Value::as_str).unwrap_or("");
    let input = block.get("input").cloned().unwrap_or(Value::Null);
    let str_of = |k: &str| input.get(k).and_then(Value::as_str).unwrap_or("");
    let label = match name {
        // The call's own description says what it is for.
        "Bash" => match str_of("description").trim() {
            "" => command_label(str_of("command")),
            d => d.trim_end_matches('.').to_owned(),
        },
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => {
            let path = if str_of("file_path").is_empty() {
                str_of("notebook_path")
            } else {
                str_of("file_path")
            };
            format!("editing {}", file_name(path))
        }
        "Read" => format!("reading {}", file_name(str_of("file_path"))),
        "Grep" | "Glob" => "searching".to_owned(),
        "WebFetch" | "WebSearch" => "searching the web".to_owned(),
        "Agent" | "Task" => "delegating".to_owned(),
        "TodoWrite" => "planning".to_owned(),
        other => tool_word(other),
    };
    let first_line = |t: &str| {
        t.lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim()
            .to_owned()
    };
    let description = Some(
        str_of("description")
            .trim()
            .trim_end_matches('.')
            .to_owned(),
    )
    .filter(|d| !d.is_empty());
    let path = Some(if str_of("file_path").is_empty() {
        str_of("notebook_path")
    } else {
        str_of("file_path")
    })
    .filter(|p| !p.is_empty())
    .map(str::to_owned);
    let (verb, target, step) = match name {
        "Bash" => (
            "Bash".to_owned(),
            Some(first_line(str_of("command"))),
            description,
        ),
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" | "Read" => (name.to_owned(), path, None),
        "Grep" | "Glob" => (
            "Search".to_owned(),
            Some(str_of("pattern").to_owned()),
            None,
        ),
        "WebSearch" => ("Search".to_owned(), Some(str_of("query").to_owned()), None),
        "WebFetch" => ("Fetch".to_owned(), Some(str_of("url").to_owned()), None),
        "Agent" | "Task" => ("Agent".to_owned(), None, description),
        // A question it asked reads as one, not as the tool's name.
        "AskUserQuestion" => (
            "Asked".to_owned(),
            None,
            input
                .pointer("/questions/0/question")
                .and_then(Value::as_str)
                .map(|q| cut(q.trim(), 80)),
        ),
        other => (capitalised(&tool_word(other)), None, description),
    };
    let lines = match name {
        "Edit" | "Write" | "MultiEdit" => patched.get(id).copied().or_else(|| {
            let input_text = input.to_string();
            let files = changes::of_call(name, &input_text, None);
            files.first().and_then(|f| f.lines)
        }),
        _ => None,
    };
    Some(Activity {
        label: cut(&label, LABEL_CHARS),
        at: ts,
        running: !id.is_empty() && !answered.contains(id),
        delegates: matches!(name, "Agent" | "Task"),
        verb,
        target: target.filter(|t| !t.is_empty()),
        step,
        lines,
    })
}

fn capitalised(word: &str) -> String {
    let mut c = word.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

fn codex_activity(p: &Value, ts: OffsetDateTime, answered: &HashSet<String>) -> Option<Activity> {
    let name = p
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_else(|| p.get("type").and_then(Value::as_str).unwrap_or(""));
    let id = p.get("call_id").and_then(Value::as_str).unwrap_or("");
    let args: Value = p
        .get("arguments")
        .and_then(Value::as_str)
        .and_then(|a| serde_json::from_str(a).ok())
        .or_else(|| p.get("action").cloned())
        .unwrap_or(Value::Null);
    let command = match args.get("command").or_else(|| args.get("cmd")) {
        Some(Value::Array(parts)) => {
            let words: Vec<&str> = parts.iter().filter_map(Value::as_str).collect();
            // `bash -lc "<script>"`: the script is the command.
            match words.as_slice() {
                [sh, flag, script] if sh.ends_with("sh") && flag.starts_with('-') => {
                    (*script).to_owned()
                }
                _ => words.join(" "),
            }
        }
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    };
    let label = match name {
        "shell" | "exec_command" | "local_shell_call" | "local_shell" if !command.is_empty() => {
            command_label(&command)
        }
        "apply_patch" => {
            let input = p.get("input").and_then(Value::as_str).unwrap_or("");
            let file = input.lines().find_map(|l| {
                l.strip_prefix("*** Update File: ")
                    .or_else(|| l.strip_prefix("*** Add File: "))
            });
            format!("editing {}", file.map(file_name).unwrap_or("files"))
        }
        other => tool_word(other),
    };
    let (verb, target, lines) = match name {
        "shell" | "exec_command" | "local_shell_call" | "local_shell" if !command.is_empty() => {
            let first = command
                .lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("")
                .trim();
            ("Bash".to_owned(), Some(first.to_owned()), None)
        }
        "apply_patch" => {
            let input = p.get("input").and_then(Value::as_str).unwrap_or("");
            let files = changes::patch_lines(input);
            let mut sum = Lines::default();
            let mut known = !files.is_empty();
            for f in &files {
                match f.lines {
                    Some(l) => sum.add(l),
                    None => known = false,
                }
            }
            (
                "Edit".to_owned(),
                files.first().map(|f| f.path.clone()),
                known.then_some(sum),
            )
        }
        other => (capitalised(&tool_word(other)), None, None),
    };
    Some(Activity {
        label: cut(&label, LABEL_CHARS),
        at: ts,
        running: !id.is_empty() && !answered.contains(id),
        delegates: false,
        verb,
        target,
        step: None,
        lines,
    })
}

fn codex_text(p: &Value) -> String {
    p.get("content")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
}

/// `mcp__server__name` as `name`, `snake_case` with spaces.
fn tool_word(name: &str) -> String {
    let short = name.rsplit("__").next().unwrap_or(name);
    short.replace('_', " ")
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').find(|p| !p.is_empty()).unwrap_or(path)
}

/// A shell command as its program and the first word after it, past `cd …
/// &&`, `VAR=…` and wrappers such as `timeout 600`.
pub fn command_label(command: &str) -> String {
    let first_line = command.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    if inline_script(first_line) {
        return "running a script".to_owned();
    }
    let segments = first_line.split("&&").flat_map(|s| s.split(';'));
    for seg in segments {
        let mut words = seg.split_whitespace().peekable();
        // A loop or test's head says nothing; its body's command does.
        if words
            .peek()
            .is_some_and(|w| matches!(*w, "for" | "while" | "until" | "if" | "case" | "elif"))
        {
            continue;
        }
        if words
            .peek()
            .is_some_and(|w| matches!(*w, "do" | "then" | "else" | "{"))
        {
            words.next();
        }
        // Assignments and wrappers.
        while let Some(w) = words.peek() {
            let assign = w.contains('=') && !w.starts_with('-') && !w.starts_with('=');
            let wrapper = matches!(*w, "env" | "time" | "nohup" | "exec" | "command" | "(");
            if assign || wrapper {
                words.next();
            } else if *w == "timeout" {
                words.next();
                if words
                    .peek()
                    .is_some_and(|n| n.chars().next().is_some_and(|c| c.is_ascii_digit()))
                {
                    words.next();
                }
            } else {
                break;
            }
        }
        let Some(program) = words.next() else {
            continue;
        };
        // A function definition (`name() {`) says nothing either.
        if program.ends_with("()") || words.peek().is_some_and(|w| w.starts_with("()")) {
            continue;
        }
        let program = program.trim_matches(|c| c == '(' || c == '"' || c == '\'');
        if matches!(
            program,
            "cd" | "export"
                | "source"
                | "."
                | "set"
                | "pushd"
                | "true"
                | "}"
                | "done"
                | "fi"
                | "esac"
                | ")"
        ) {
            continue;
        }
        let program = file_name(program);
        let next = words
            .next()
            .map(|w| w.trim_matches(|c| c == '"' || c == '\''));
        return match next {
            Some(w)
                if !w.starts_with('-')
                    && !w.is_empty()
                    && !w.contains(['|', '>', '<', '$', '`']) =>
            {
                cut(&format!("{program} {}", file_name(w)), LABEL_CHARS)
            }
            _ => cut(program, LABEL_CHARS),
        };
    }
    "running a command".to_owned()
}

/// Whether a command line runs an inline script: an interpreter
/// with `-c` or `-e`, or a heredoc, whose program name says nothing.
fn inline_script(line: &str) -> bool {
    if line.contains("<<") {
        return true;
    }
    let words: Vec<&str> = line.split_whitespace().collect();
    words.windows(2).any(|w| {
        let prog = file_name(w[0]);
        let interp = matches!(
            prog,
            "python"
                | "python3"
                | "node"
                | "ruby"
                | "perl"
                | "osascript"
                | "sh"
                | "bash"
                | "zsh"
                | "deno"
                | "bun"
        );
        interp && matches!(w[1], "-c" | "-e" | "-lc" | "-ic" | "-")
    })
}

/// Whether a reply's last paragraph ends with a question mark, Markdown
/// marks and closing quotes and brackets trimmed.
pub fn ends_with_question(text: &str) -> bool {
    last_paragraph(text)
        .trim_end_matches(|c: char| c.is_whitespace() || "*_`\"')]”’».".contains(c) && c != '?')
        .ends_with('?')
}

fn last_paragraph(text: &str) -> &str {
    let t = text.trim_end();
    match t.rfind("\n\n") {
        Some(i) => t[i + 2..].trim(),
        None => t.trim(),
    }
}

/// The last sentence of a reply's last paragraph, Markdown marks dropped,
/// at most 120 characters.
pub fn last_sentence(text: &str) -> Option<String> {
    let para = last_paragraph(text);
    let line = para.lines().last().unwrap_or(para).trim();
    let body = line.trim_end_matches(|c: char| c.is_whitespace() || "*_`\"')]”’».?".contains(c));
    let start = body
        .char_indices()
        .rev()
        .find(|(i, c)| {
            matches!(c, '.' | '!' | '?' | ':') && body[i + c.len_utf8()..].starts_with(' ')
        })
        .map_or(0, |(i, c)| i + c.len_utf8());
    let sentence = line[start..]
        .trim()
        .trim_start_matches(['-', '*', '>', '#', ' '])
        .replace(['*', '`'], "");
    let sentence = sentence.trim();
    (!sentence.is_empty()).then(|| cut(sentence, 120))
}

/// At most `n` characters, with `…` when cut.
pub fn cut(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_owned()
    } else {
        let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
        out = out.trim_end().to_owned();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const T0: &str = "2026-11-10T10:00:00Z";
    const T1: &str = "2026-11-10T10:01:00Z";
    const T2: &str = "2026-11-10T10:02:00Z";

    fn assistant(ts: &str, content: Value, stop: Option<&str>) -> String {
        json!({"type": "assistant", "timestamp": ts, "isSidechain": false,
            "message": {"role": "assistant", "content": content, "stop_reason": stop}})
        .to_string()
    }

    fn result(ts: &str, id: &str) -> String {
        json!({"type": "user", "timestamp": ts,
            "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": id, "content": "ok"}]}})
        .to_string()
    }

    fn tail(lines: &[String]) -> Tail {
        of_lines(&lines.iter().map(String::as_str).collect::<Vec<_>>())
    }

    #[test]
    fn a_reply_ending_with_a_question_waits() {
        let t = tail(&[assistant(
            T1,
            json!([{"type": "text", "text": "Done with the parser.\n\nTests pass. **Should I open the pull request now?**"}]),
            Some("end_turn"),
        )]);
        let w = t.waiting.unwrap();
        assert_eq!(
            w.asked.as_deref(),
            Some("Should I open the pull request now?")
        );
        assert!(!w.plan);
    }

    #[test]
    fn a_question_mid_reply_or_an_unfinished_block_does_not_wait() {
        // The question is not in the last paragraph.
        let t = tail(&[assistant(
            T1,
            json!([{"type": "text", "text": "Why did it fail? The lock file was stale.\n\nFixed and pushed."}]),
            Some("end_turn"),
        )]);
        assert_eq!(t.waiting, None);
        // A text block still streaming (no stop reason) is not a wait.
        let t = tail(&[assistant(
            T1,
            json!([{"type": "text", "text": "Which one?"}]),
            None,
        )]);
        assert_eq!(t.waiting, None);
        // A reply with a tool call is work, not a question.
        let t = tail(&[assistant(
            T1,
            json!([{"type": "text", "text": "Shall I look?"}, {"type": "tool_use", "id": "a", "name": "Read", "input": {"file_path": "/w/x.rs"}}]),
            Some("tool_use"),
        )]);
        assert_eq!(t.waiting, None);
        // A sub-agent's reply never waits on the person.
        let side = json!({"type": "assistant", "timestamp": T1, "isSidechain": true,
            "message": {"role": "assistant", "content": [{"type": "text", "text": "Proceed?"}], "stop_reason": "end_turn"}})
        .to_string();
        assert_eq!(tail(&[side]).waiting, None);
    }

    #[test]
    fn an_unanswered_ask_or_plan_waits_and_an_answered_one_does_not() {
        let ask = assistant(
            T1,
            json!([{"type": "tool_use", "id": "q1", "name": "AskUserQuestion",
                "input": {"questions": [{"question": "Merge now or wait for review?", "options": []}]}}]),
            Some("tool_use"),
        );
        let w = tail(std::slice::from_ref(&ask)).waiting.unwrap();
        assert_eq!(w.asked.as_deref(), Some("Merge now or wait for review?"));
        let answered = tail(&[ask, result(T2, "q1")]);
        assert_eq!(answered.waiting, None);
        let plan = assistant(
            T1,
            json!([{"type": "tool_use", "id": "p1", "name": "ExitPlanMode", "input": {"plan": "1. do"}}]),
            Some("tool_use"),
        );
        assert!(tail(std::slice::from_ref(&plan)).waiting.unwrap().plan);
        let plan = assistant(
            T1,
            json!([{"type": "tool_use", "id": "p2", "name": "ExitPlanMode",
                "input": {"plan": "## Plan\n\n1. Add the store\n\n**Plan ready: 2 steps. Approve?**\n"}}]),
            Some("tool_use"),
        );
        assert_eq!(
            tail(&[plan]).waiting.unwrap().asked.as_deref(),
            Some("Plan ready: 2 steps. Approve?")
        );
    }

    #[test]
    fn activity_is_the_newest_tool_call_and_whether_it_has_a_result() {
        let t = tail(&[
            assistant(
                T0,
                json!([{"type": "tool_use", "id": "e1", "name": "Edit", "input": {"file_path": "/w/src/memory.rs"}}]),
                Some("tool_use"),
            ),
            result(T1, "e1"),
            assistant(
                T2,
                json!([{"type": "tool_use", "id": "b1", "name": "Bash", "input": {"command": "cd /w && CARGO_INCREMENTAL=0 timeout 600 cargo test -p core 2>&1 | tail"}}]),
                Some("tool_use"),
            ),
        ]);
        let a = t.activity.unwrap();
        assert_eq!(a.label, "cargo test");
        assert!(a.running);
        assert_eq!(t.waiting, None);
        let t = tail(&[
            assistant(
                T0,
                json!([{"type": "tool_use", "id": "e1", "name": "Edit", "input": {"file_path": "/w/src/memory.rs"}}]),
                Some("tool_use"),
            ),
            result(T1, "e1"),
        ]);
        let a = t.activity.unwrap();
        assert_eq!(a.label, "editing memory.rs");
        assert!(!a.running);
        assert_eq!(t.last, Some(OffsetDateTime::parse(T1, &Rfc3339).unwrap()));
    }

    #[test]
    fn command_labels_skip_setup_and_wrappers() {
        assert_eq!(command_label("git push -u origin x"), "git push");
        assert_eq!(command_label("cd '/a b' && npm run build"), "npm run");
        assert_eq!(
            command_label("FOO=1 BAR=2 python3 /x/scan.py --all"),
            "python3 scan.py"
        );
        assert_eq!(command_label("ls -la /tmp"), "ls");
        assert_eq!(command_label("export X=1; gh pr merge 3"), "gh pr");
        assert_eq!(command_label(""), "running a command");
        assert_eq!(command_label("python3 -c 'print(1)'"), "running a script");
        assert_eq!(
            command_label("cd /w && osascript -e 'beep'"),
            "running a script"
        );
        assert_eq!(command_label("cat > /tmp/x <<'EOF'"), "running a script");
        assert_eq!(command_label("for f in $(ls); do wc -l \"$f\"; done"), "wc");
        assert_eq!(
            command_label("shot() { echo; }; shot 1200 800"),
            "shot 1200"
        );
        assert!(
            command_label("averyveryverylongprogramname averyveryverylongargument")
                .chars()
                .count()
                <= LABEL_CHARS
        );
    }

    #[test]
    fn a_shell_call_s_description_is_its_label() {
        let t = tail(&[assistant(
            T0,
            json!([{"type": "tool_use", "id": "b1", "name": "Bash",
                "input": {"command": "osascript -e 'x'", "description": "Run the fleet tests across every crate and profile."}}]),
            Some("tool_use"),
        )]);
        let a = t.activity.unwrap();
        assert_eq!(a.label, "Run the fleet tests across every crate…");
        assert!(a.label.chars().count() <= LABEL_CHARS);
        let t = tail(&[assistant(
            T0,
            json!([{"type": "tool_use", "id": "b1", "name": "Bash",
                "input": {"command": "pkill -f x", "description": "Stop the old app"}}]),
            Some("tool_use"),
        )]);
        assert_eq!(t.activity.unwrap().label, "Stop the old app");
    }

    #[test]
    fn codex_waits_on_a_question_and_reads_shell_calls() {
        let call = json!({"type": "response_item", "timestamp": T0,
            "payload": {"type": "function_call", "name": "shell", "call_id": "c1",
                "arguments": "{\"command\":[\"bash\",\"-lc\",\"cargo clippy --all-targets\"]}"}})
        .to_string();
        let t = tail(std::slice::from_ref(&call));
        assert_eq!(t.activity.unwrap().label, "cargo clippy");
        let msg = json!({"type": "response_item", "timestamp": T1,
            "payload": {"type": "message", "role": "assistant",
                "content": [{"type": "output_text", "text": "Both work. Which do you prefer?"}]}})
        .to_string();
        let t = tail(&[call, msg]);
        assert_eq!(
            t.waiting.unwrap().asked.as_deref(),
            Some("Which do you prefer?")
        );
    }

    #[test]
    fn questions_and_sentences() {
        assert!(ends_with_question("Ready to merge?"));
        assert!(ends_with_question("Want me to do it?**"));
        assert!(ends_with_question("Go ahead? (yes/no)?"));
        assert!(!ends_with_question("Done."));
        assert!(!ends_with_question("Asked: why? Because."));
        assert_eq!(
            last_sentence("All good. Next: deploy. Want me to start?").as_deref(),
            Some("Want me to start?")
        );
    }

    #[test]
    fn the_turn_is_the_newest_request_and_its_calls_newest_first() {
        let req = |ts: &str, text: &str| {
            json!({"type": "user", "timestamp": ts,
                "message": {"role": "user", "content": text}})
            .to_string()
        };
        let mut used = assistant(
            T2,
            json!([{"type": "tool_use", "id": "b", "name": "Bash",
                "input": {"command": "cargo test", "description": "Run the tests"}}]),
            None,
        );
        // The newest reply says how much context it was given.
        let mut v: Value = serde_json::from_str(&used).unwrap();
        v["message"]["usage"] = json!({"input_tokens": 4, "cache_read_input_tokens": 180000,
            "cache_creation_input_tokens": 996, "output_tokens": 50});
        used = v.to_string();
        let lines = [
            req(T0, "an older request"),
            req(T0, "<command-name>/clear</command-name>"),
            req(T1, "fix the parser, then run the tests"),
            assistant(
                T1,
                json!([{"type": "tool_use", "id": "a", "name": "Edit",
                    "input": {"file_path": "/r/src/parse.rs", "old_string": "a\nb", "new_string": "a\nc\nd"}}]),
                None,
            ),
            result(T1, "a"),
            // A sub-agent's own call is not this turn's.
            json!({"type": "assistant", "timestamp": T2, "isSidechain": true,
                "message": {"role": "assistant", "content": [{"type": "tool_use", "id": "s", "name": "Read", "input": {"file_path": "/x"}}]}})
            .to_string(),
            used,
        ];
        let t = turn_of_lines(&lines.iter().map(String::as_str).collect::<Vec<_>>());
        assert_eq!(
            t.asked.as_deref(),
            Some("fix the parser, then run the tests")
        );
        assert_eq!(t.count, 2);
        assert_eq!(t.calls[0].verb, "Bash");
        assert!(t.calls[0].running);
        assert_eq!(t.calls[0].step.as_deref(), Some("Run the tests"));
        assert_eq!(t.calls[1].verb, "Edit");
        assert!(!t.calls[1].running);
        assert_eq!(
            t.calls[1].lines,
            Some(Lines {
                added: 2,
                removed: 1
            })
        );
        assert_eq!(t.tokens, Some(181_000));
        assert!(!t.cut);
        // With no request in the tail, the turn says it was cut.
        let t = turn_of_lines(&lines[3..].iter().map(String::as_str).collect::<Vec<_>>());
        assert!(t.cut);
        assert_eq!(t.asked, None);
    }

    #[test]
    fn a_question_reads_as_asked_with_its_question() {
        let t = tail(&[assistant(
            T1,
            json!([{"type": "tool_use", "id": "q", "name": "AskUserQuestion",
                "input": {"questions": [{"question": "Which plan should I keep?"}]}}]),
            None,
        )]);
        let a = t.activity.unwrap();
        assert_eq!(a.verb, "Asked");
        assert_eq!(a.step.as_deref(), Some("Which plan should I keep?"));
        assert_eq!(a.target, None);
    }
}
