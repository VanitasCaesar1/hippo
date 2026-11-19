//! Gemini CLI sessions: `~/.gemini/tmp/<project>/chats/session-*.json` or
//! `session-*.jsonl` ([`DEFAULT_ROOT`]), as Gemini CLI records every
//! session for `--resume` (see `docs/designs/gemini-adapter.md`).
//!
//! Older releases write one JSON document per session, `{"sessionId",
//! "projectHash", "startTime", "lastUpdated", "messages": [...]}`; newer
//! ones append JSON Lines: a metadata record with `sessionId`, then one
//! record per message, with `{"$set": {...}}` records that update the
//! metadata (a `messages` array there replaces the messages read so far)
//! and other `$`-records that hippo skips. A message record whose `id` was
//! seen before replaces that message. A sub-agent's session is
//! `chats/<parent session id>/<id>.jsonl`.
//!
//! A message is `{"id", "timestamp", "type", "content", "toolCalls"}`.
//! `type` `user` makes a user turn of `content`, `gemini` an assistant turn
//! and, for each tool call, a tool turn `<name> <args>` and a tool turn of
//! its result, named by the call. Thoughts and `info`, `error` and
//! `warning` messages are never indexed. `content` is a string or a list of
//! parts, whose `text`s are joined.
//!
//! The session is `gemini-<sessionId>`, a sub-agent's
//! `gemini-<parent>-agent-<sessionId>`. Its directory is in the project
//! folder's `.project_root` file, which newer releases write; an older,
//! hashed project folder has none.

use std::path::{Path, PathBuf};

use serde_json::Value;
use time::OffsetDateTime;

use super::{
    each_json_line, invalid_data, push_turn, timestamp, transcript_roots, Adapter, Parsed, RawTurn,
    Role, Tool,
};
use crate::config::Config;
use crate::error::{Error, Result};

/// Where Gemini CLI keeps its projects' sessions.
pub const DEFAULT_ROOT: &str = "~/.gemini/tmp";

/// The Gemini CLI adapter.
#[derive(Debug, Clone, Copy, Default)]
pub struct Gemini;

impl Adapter for Gemini {
    fn tool(&self) -> Tool {
        Tool::Gemini
    }

    fn roots(&self, cfg: &Config) -> Vec<PathBuf> {
        transcript_roots(cfg)
    }

    fn file_matches(&self, path: &Path) -> bool {
        is_session(path)
    }

    fn parse(&self, _root: &Path, path: &Path) -> Result<Parsed> {
        let session = read(path)?;
        let raw_id = session
            .id
            .clone()
            .or_else(|| path.file_stem().and_then(|s| s.to_str()).map(str::to_owned))
            .ok_or_else(|| invalid_data(path, "no sessionId"))?;
        let id = match parent_session(path) {
            Some(parent) => format!("gemini-{parent}-agent-{raw_id}"),
            None => format!("gemini-{raw_id}"),
        };
        let first = session
            .messages
            .iter()
            .find_map(|m| timestamp(m.get("timestamp")));
        let started = session
            .started
            .or(first)
            .ok_or_else(|| invalid_data(path, "no start time"))?;
        let mut turns = Vec::new();
        let mut now = started;
        for message in &session.messages {
            if let Some(ts) = timestamp(message.get("timestamp")) {
                now = ts;
            }
            add_message(&mut turns, now, message);
        }
        Ok(Parsed {
            id,
            tool: Tool::Gemini,
            started,
            turns,
        })
    }

    fn cwd(&self, _id: &str, path: &Path) -> Option<PathBuf> {
        project_root(path)
    }
}

/// A session file, read: its id, start and messages in order.
#[derive(Debug, Default)]
pub struct Session {
    pub id: Option<String>,
    pub started: Option<OffsetDateTime>,
    pub messages: Vec<Value>,
}

impl Session {
    /// Takes the metadata fields of `record`.
    fn meta(&mut self, record: &Value) {
        if let Some(id) = record.get("sessionId").and_then(Value::as_str) {
            if !id.is_empty() {
                self.id = Some(id.to_owned());
            }
        }
        if let Some(at) = timestamp(record.get("startTime")) {
            self.started = Some(at);
        }
    }

    /// Adds `message`, or replaces the message with its `id`.
    fn upsert(&mut self, message: Value) {
        let id = message.get("id").and_then(Value::as_str).map(str::to_owned);
        if let Some(id) = id {
            if let Some(slot) = self
                .messages
                .iter_mut()
                .find(|m| m.get("id").and_then(Value::as_str) == Some(id.as_str()))
            {
                *slot = message;
                return;
            }
        }
        self.messages.push(message);
    }
}

/// Whether `record` is a message: it has a `type` Gemini CLI gives one.
fn is_message(record: &Value) -> bool {
    matches!(
        record.get("type").and_then(Value::as_str),
        Some("user" | "gemini" | "info" | "error" | "warning")
    )
}

/// Reads the session file at `path`, whole JSON or JSON Lines.
pub fn read(path: &Path) -> Result<Session> {
    let bytes = std::fs::read(path).map_err(|e| Error::io_at(path, e))?;
    let mut session = Session::default();
    // One JSON document, as older releases write.
    if let Ok(Value::Object(doc)) = serde_json::from_slice::<Value>(&bytes) {
        let doc = Value::Object(doc);
        if doc.get("messages").is_some() || doc.get("sessionId").is_some() {
            session.meta(&doc);
            for m in doc
                .get("messages")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                session.upsert(m.clone());
            }
            return Ok(session);
        }
    }
    each_json_line(path, |record| {
        if let Some(set) = record.get("$set") {
            session.meta(set);
            if let Some(messages) = set.get("messages").and_then(Value::as_array) {
                session.messages = messages.clone();
            }
            return;
        }
        if record
            .as_object()
            .is_some_and(|o| o.keys().any(|k| k.starts_with('$')))
        {
            return;
        }
        if is_message(&record) {
            session.upsert(record);
        } else {
            session.meta(&record);
        }
    })?;
    if session.id.is_none() && session.messages.is_empty() {
        return Err(invalid_data(path, "no Gemini CLI session in it"));
    }
    Ok(session)
}

/// The turns of one message, stamped `ts`.
fn add_message(turns: &mut Vec<RawTurn>, ts: OffsetDateTime, message: &Value) {
    let role = match message.get("type").and_then(Value::as_str) {
        Some("user") => Role::User,
        Some("gemini") => Role::Assistant,
        _ => return,
    };
    let text = content_text(message.get("content"));
    if !text.trim().is_empty() {
        push_turn(turns, role, ts, None, text.trim().to_owned());
    }
    for call in message
        .get("toolCalls")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let name = call
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("tool")
            .to_owned();
        let at = timestamp(call.get("timestamp")).unwrap_or(ts);
        let args = call.get("args").map(Value::to_string).unwrap_or_default();
        push_turn(
            turns,
            Role::Tool,
            at,
            Some(name.clone()),
            format!("{name} {args}").trim_end().to_owned(),
        );
        if let Some(result) = result_text(call) {
            push_turn(turns, Role::Tool, at, Some(name), result);
        }
    }
}

/// A message's `content`: a string, or a list of parts whose `text`s are
/// joined with line feeds.
fn content_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| match p {
                Value::String(s) => Some(s.as_str()),
                _ => p.get("text").and_then(Value::as_str),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Some(Value::Object(_)) => content
            .and_then(|c| c.get("text"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        _ => String::new(),
    }
}

/// A tool call's result: the `functionResponse.response` of its `result`
/// parts (`output` when it is a string, else the response as JSON), or
/// `resultDisplay` when it is text. `None` for a call with neither.
fn result_text(call: &Value) -> Option<String> {
    let mut out = Vec::new();
    for part in call
        .get("result")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(response) = part.get("functionResponse").and_then(|f| f.get("response")) else {
            continue;
        };
        match response.get("output").or_else(|| response.get("error")) {
            Some(Value::String(s)) => out.push(s.clone()),
            _ => out.push(response.to_string()),
        }
    }
    if out.is_empty() {
        if let Some(Value::String(s)) = call.get("resultDisplay") {
            out.push(s.clone());
        }
    }
    let text = out.join("\n");
    (!text.trim().is_empty()).then_some(text)
}

/// Whether `path` is a Gemini CLI session: `session-*.json` or
/// `session-*.jsonl` in a `chats` folder, or a sub-agent's `*.jsonl` in a
/// folder inside it, under a `.gemini` folder.
pub fn is_session(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let ext_ok = name.ends_with(".json") || name.ends_with(".jsonl");
    if !ext_ok || !is_under_gemini(path) {
        return false;
    }
    let parent = path.parent().and_then(Path::file_name);
    if parent.is_some_and(|p| p == "chats") {
        return name.starts_with("session-");
    }
    parent_session(path).is_some() && name.ends_with(".jsonl")
}

/// The parent session of a sub-agent's file, `chats/<parent>/<id>.jsonl`.
fn parent_session(path: &Path) -> Option<&str> {
    let parent = path.parent()?;
    if parent.parent()?.file_name()? != "chats" {
        return None;
    }
    parent.file_name()?.to_str()
}

/// Whether `path` lies under a `.gemini` folder: none of its files is a
/// Claude Code transcript.
pub fn is_under_gemini(path: &Path) -> bool {
    path.components().any(|c| c.as_os_str() == ".gemini")
}

/// The project folder of the session at `path`: the folder that holds its
/// `chats`.
fn project_dir(path: &Path) -> Option<&Path> {
    path.ancestors()
        .find(|a| a.file_name().is_some_and(|n| n == "chats"))?
        .parent()
}

/// The directory the session at `path` ran in: the project folder's
/// `.project_root` file, which names it.
pub fn project_root(path: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(project_dir(path)?.join(".project_root")).ok()?;
    let root = text.trim();
    (!root.is_empty() && Path::new(root).is_absolute()).then(|| PathBuf::from(root))
}

#[cfg(test)]
mod tests;
