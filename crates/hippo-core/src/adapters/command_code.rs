//! Command Code sessions: `~/.commandcode/projects/<slug>/<session id>.jsonl`
//! ([`DEFAULT_ROOT`]), as Command Code 1.88 writes them (see
//! `docs/designs/command-code-adapter.md`).
//!
//! The first JSON line is the header, `{"type": "session", "version": 3,
//! "id", "timestamp", "cwd"}`; a file without one, or in another version,
//! is not read. The session is `command-code-<id>`, started at the header's
//! `timestamp`. Every `message` entry after it makes turns, one per content
//! block, stamped with the entry's `timestamp`, in file order (every
//! branch of the entry tree): `text` a user or assistant turn by the
//! message's role, `tool_use` a tool turn `<name> <input>`, `tool_result` a
//! tool turn named by the `tool_use` with the same id. Thinking blocks and
//! every other entry type are skipped.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde_json::Value;
use time::OffsetDateTime;

use super::{
    each_json_line, has_extension_jsonl, invalid_data, push_turn, timestamp, transcript_roots,
    Adapter, Parsed, RawTurn, Role, Tool,
};
use crate::config::Config;
use crate::error::Result;

/// Where Command Code keeps its sessions. No setting moves it.
pub const DEFAULT_ROOT: &str = "~/.commandcode/projects";

/// The only session file version read.
pub const VERSION: u64 = 3;

/// The Command Code adapter.
#[derive(Debug, Clone, Copy, Default)]
pub struct CommandCode;

impl Adapter for CommandCode {
    fn tool(&self) -> Tool {
        Tool::CommandCode
    }

    fn roots(&self, cfg: &Config) -> Vec<PathBuf> {
        transcript_roots(cfg)
    }

    fn file_matches(&self, path: &Path) -> bool {
        is_session(path)
    }

    fn parse(&self, _root: &Path, path: &Path) -> Result<Parsed> {
        let mut header: Option<Header> = None;
        let mut first = true;
        let mut bad_header: Option<String> = None;
        let mut call_names = HashMap::new();
        let mut turns = Vec::new();
        each_json_line(path, |line| {
            if is_marker(&line) {
                return;
            }
            if first {
                first = false;
                match read_header(&line) {
                    Ok(h) => header = Some(h),
                    Err(why) => bad_header = Some(why),
                }
                return;
            }
            if header.is_none() {
                return;
            }
            if line.get("type").and_then(Value::as_str) != Some("message") {
                return;
            }
            let Some(ts) = timestamp(line.get("timestamp")) else {
                tracing::warn!(
                    "{}: skipping a message without a usable timestamp",
                    path.display()
                );
                return;
            };
            if let Some(message) = line.get("message") {
                add_message(&mut turns, &mut call_names, ts, message);
            }
        })?;
        let Some(header) = header else {
            let why = bad_header.unwrap_or_else(|| "no session header".to_owned());
            return Err(invalid_data(path, &why));
        };
        Ok(Parsed {
            id: format!("command-code-{}", header.id),
            tool: Tool::CommandCode,
            started: header.started,
            turns,
        })
    }
}

/// A session file's header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub id: String,
    pub started: OffsetDateTime,
    pub cwd: Option<PathBuf>,
}

/// Whether `path` is a Command Code session file: `<id>.jsonl` (no other
/// dot in the name, so not `<id>.checkpoints.jsonl` or `<id>.prompts.jsonl`)
/// in a project folder under a `.commandcode` folder.
pub fn is_session(path: &Path) -> bool {
    has_extension_jsonl(path)
        && path
            .file_stem()
            .and_then(|s| s.to_str())
            .is_some_and(|s| !s.is_empty() && !s.contains('.'))
        && path
            .parent()
            .and_then(Path::parent)
            .is_some_and(|projects| {
                projects.file_name().is_some_and(|n| n == "projects")
                    && projects
                        .parent()
                        .and_then(Path::file_name)
                        .is_some_and(|n| n == ".commandcode")
            })
}

/// Whether `path` lies under a `.commandcode` folder: none of its files is
/// a Claude Code transcript.
pub fn is_under_commandcode(path: &Path) -> bool {
    path.components().any(|c| c.as_os_str() == ".commandcode")
}

/// The header of the session file at `path`, from its first lines; `None`
/// when it cannot be read or has none in the version read.
pub fn header(path: &Path) -> Option<Header> {
    let file = std::fs::File::open(path).ok()?;
    for line in BufReader::new(file).lines().take(4) {
        let Ok(value) = serde_json::from_str::<Value>(&line.ok()?) else {
            continue;
        };
        if is_marker(&value) {
            continue;
        }
        return read_header(&value).ok();
    }
    None
}

/// A fixture's `{"synthetic": true}` first line.
fn is_marker(line: &Value) -> bool {
    line.get("synthetic").is_some() && line.get("type").is_none()
}

fn read_header(line: &Value) -> std::result::Result<Header, String> {
    if line.get("type").and_then(Value::as_str) != Some("session") {
        return Err("no session header".to_owned());
    }
    match line.get("version").and_then(Value::as_u64) {
        Some(VERSION) => {}
        Some(v) => {
            return Err(format!(
                "Command Code session format v{v} is not one hippo reads (v{VERSION}); Command Code converts it when it next opens the session"
            ))
        }
        None => return Err("a session header without a version".to_owned()),
    }
    let id = line
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| "a session header without an id".to_owned())?;
    let started = timestamp(line.get("timestamp"))
        .ok_or_else(|| "a session header without a usable timestamp".to_owned())?;
    let cwd = line
        .get("cwd")
        .and_then(Value::as_str)
        .filter(|c| !c.is_empty())
        .map(PathBuf::from);
    Ok(Header {
        id: id.to_owned(),
        started,
        cwd,
    })
}

/// Adds the turns of one message. `call_names` maps each `tool_use` id
/// seen so far to its tool's name.
fn add_message(
    turns: &mut Vec<RawTurn>,
    call_names: &mut HashMap<String, String>,
    ts: OffsetDateTime,
    message: &Value,
) {
    let role = match message.get("role").and_then(Value::as_str) {
        Some("user") => Role::User,
        Some("assistant") => Role::Assistant,
        _ => return,
    };
    let blocks = match message.get("content") {
        Some(Value::String(text)) => {
            if !text.is_empty() {
                push_turn(turns, role, ts, None, text.clone());
            }
            return;
        }
        Some(Value::Array(blocks)) => blocks,
        _ => return,
    };
    for block in blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    if !text.is_empty() {
                        push_turn(turns, role, ts, None, text.to_owned());
                    }
                }
            }
            Some("tool_use") => {
                let Some(name) = block.get("name").and_then(Value::as_str) else {
                    continue;
                };
                if let Some(id) = block.get("id").and_then(Value::as_str) {
                    call_names.insert(id.to_owned(), name.to_owned());
                }
                let text = match block.get("input") {
                    Some(Value::Null) | None => name.to_owned(),
                    Some(input) => format!("{name} {input}"),
                };
                push_turn(turns, Role::Tool, ts, Some(name.to_owned()), text);
            }
            Some("tool_result") => {
                let name = block
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .and_then(|id| call_names.get(id))
                    .cloned();
                let text = match block.get("content") {
                    Some(Value::String(text)) => text.clone(),
                    Some(Value::Array(parts)) => parts
                        .iter()
                        .filter_map(|p| p.get("text").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join("\n"),
                    _ => String::new(),
                };
                push_turn(turns, Role::Tool, ts, name, text);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests;
