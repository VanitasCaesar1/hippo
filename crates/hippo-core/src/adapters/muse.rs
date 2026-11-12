//! Muse Code session logs: `<root>/<YYYY>/<MM>/<DD>/<session id>/session.jsonl`
//! under `${XDG_DATA_HOME:-~/.local/share}/muse/sessions` ([`default_root`]).
//!
//! Muse is closed source; this follows what Muse 1.4.18 wrote in real
//! sessions (see `docs/designs/muse-adapter.md`). Each line is a record
//! envelope, `{schema_version, stream: {kind, id}, recorded_at, payload_type,
//! payload_schema_version, payload}`, with `recorded_at` in microseconds
//! since the Unix epoch; or a retained frame, `{retained_frame, children:
//! [{record_json: "<an envelope as a JSON string>"}]}`, read as its children
//! in order. An envelope or frame whose version is not 1 is skipped.
//!
//! The first envelope's `stream.id` names the session, `muse-<id>`, and its
//! `recorded_at` is the start. `runtime.session` run events make turns:
//! `started` (`prompt`) a user turn, `assistant_message_committed` (`text`)
//! an assistant turn, `assistant_tool_calls_committed` one tool turn per
//! call (`<name> <args>`), `tool_result_batch_committed` one tool turn per
//! result, named by the call with the same `call_id`. Everything else is
//! skipped. Sub-agent logs (`subagent/<id>/session.jsonl`) are not read.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;
use time::OffsetDateTime;

use super::{
    each_json_line, invalid_data, push_turn, transcript_roots, Adapter, Parsed, RawTurn, Role, Tool,
};
use crate::config::Config;
use crate::error::Result;

/// Muse's sessions folder when `XDG_DATA_HOME` is not set.
pub const DEFAULT_ROOT: &str = "~/.local/share/muse/sessions";

/// The only envelope, payload and frame version read.
const VERSION: u64 = 1;

/// How many lines [`workspace_root`] reads before it gives up: the
/// metadata record comes second in every log seen.
const HEAD_LINES: usize = 64;

/// The Muse adapter.
#[derive(Debug, Clone, Copy, Default)]
pub struct Muse;

impl Adapter for Muse {
    fn tool(&self) -> Tool {
        Tool::Muse
    }

    fn roots(&self, cfg: &Config) -> Vec<PathBuf> {
        transcript_roots(cfg)
    }

    fn file_matches(&self, path: &Path) -> bool {
        is_log(path)
    }

    fn parse(&self, _root: &Path, path: &Path) -> Result<Parsed> {
        let mut session: Option<(String, OffsetDateTime)> = None;
        let mut call_names = HashMap::new();
        let mut turns = Vec::new();
        let mut skipped: Vec<String> = Vec::new();
        each_json_line(path, |line| {
            for envelope in envelopes(line, &mut skipped) {
                if !version_ok(&envelope, &mut skipped) {
                    continue;
                }
                let Some(ts) = recorded_at(&envelope) else {
                    continue;
                };
                if session.is_none() {
                    if let Some(id) = stream_id(&envelope) {
                        session = Some((id, ts));
                    }
                }
                add_record(&mut turns, &mut call_names, ts, &envelope);
            }
        })?;
        if !skipped.is_empty() {
            skipped.sort();
            skipped.dedup();
            tracing::warn!(
                "{}: skipped records in versions this hippo does not read: {}",
                path.display(),
                skipped.join(", ")
            );
        }
        let Some((id, started)) = session else {
            return Err(invalid_data(path, "no Muse record that names a session"));
        };
        Ok(Parsed {
            id: format!("muse-{id}"),
            tool: Tool::Muse,
            started,
            turns,
        })
    }
}

/// Muse's sessions folder: `$XDG_DATA_HOME/muse/sessions` when
/// `XDG_DATA_HOME` is an absolute path, else [`DEFAULT_ROOT`].
pub fn default_root() -> String {
    match std::env::var_os("XDG_DATA_HOME").map(PathBuf::from) {
        Some(dir) if dir.is_absolute() => dir
            .join("muse")
            .join("sessions")
            .to_string_lossy()
            .into_owned(),
        _ => DEFAULT_ROOT.to_owned(),
    }
}

/// Whether `path` is a top-level Muse log,
/// `…/<YYYY>/<MM>/<DD>/<session id>/session.jsonl`.
pub fn is_log(path: &Path) -> bool {
    if path.file_name().is_none_or(|n| n != "session.jsonl") {
        return false;
    }
    let digits = |p: Option<&Path>, n: usize| {
        p.and_then(Path::file_name)
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.len() == n && s.bytes().all(|b| b.is_ascii_digit()))
    };
    let session = path.parent();
    let day = session.and_then(Path::parent);
    let month = day.and_then(Path::parent);
    let year = month.and_then(Path::parent);
    session.and_then(Path::file_name).is_some()
        && digits(day, 2)
        && digits(month, 2)
        && digits(year, 4)
}

/// Whether `path` is a Muse log, read or not: a top-level log
/// ([`is_log`]) or a sub-agent's, `<session>/subagent/<id>/session.jsonl`.
/// No other adapter claims one.
pub fn is_muse_file(path: &Path) -> bool {
    if is_log(path) {
        return true;
    }
    let parent = path.parent().and_then(Path::parent);
    path.file_name().is_some_and(|n| n == "session.jsonl")
        && parent
            .and_then(Path::file_name)
            .is_some_and(|n| n == "subagent")
        && parent
            .and_then(Path::parent)
            .is_some_and(|session| is_log(&session.join("session.jsonl")))
}

/// The session's working directory, the `workspace_root` of its
/// `runtime.session.metadata` record, from the first lines of the log at
/// `path`. `None` when the file cannot be read or does not say.
pub fn workspace_root(path: &Path) -> Option<PathBuf> {
    use std::io::{BufRead, BufReader};
    let file = std::fs::File::open(path).ok()?;
    let mut skipped = Vec::new();
    for line in BufReader::new(file).lines().take(HEAD_LINES) {
        let Ok(value) = serde_json::from_str::<Value>(&line.ok()?) else {
            continue;
        };
        for envelope in envelopes(value, &mut skipped) {
            if envelope.get("payload_type").and_then(Value::as_str)
                == Some("runtime.session.metadata")
            {
                let root = envelope
                    .pointer("/payload/record/workspace_root")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())?;
                return Some(PathBuf::from(root));
            }
        }
    }
    None
}

/// The envelopes on one line: the line itself, or a frame's children in
/// order. A frame in another version is noted in `skipped` and gives none.
fn envelopes(line: Value, skipped: &mut Vec<String>) -> Vec<Value> {
    if line.get("retained_frame").is_none() {
        return vec![line];
    }
    match line.get("frame_schema_version").and_then(Value::as_u64) {
        Some(VERSION) => {}
        other => {
            skipped.push(format!("frame {}", version_label(other)));
            return Vec::new();
        }
    }
    line.get("children")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|child| child.get("record_json")?.as_str())
        .filter_map(|text| serde_json::from_str(text).ok())
        .collect()
}

/// Whether an envelope's `schema_version` and (when present)
/// `payload_schema_version` are [`VERSION`]; otherwise notes it in
/// `skipped`, unless the line has no `schema_version` at all.
fn version_ok(envelope: &Value, skipped: &mut Vec<String>) -> bool {
    // A line with no version at all, such as a fixture's synthetic marker,
    // is not an envelope.
    let Some(schema) = envelope.get("schema_version") else {
        return false;
    };
    let schema = schema.as_u64();
    if schema != Some(VERSION) {
        skipped.push(format!("schema {}", version_label(schema)));
        return false;
    }
    match envelope.get("payload_schema_version") {
        None => true,
        Some(v) if v.as_u64() == Some(VERSION) => true,
        Some(v) => {
            skipped.push(format!("payload {}", version_label(v.as_u64())));
            false
        }
    }
}

fn version_label(v: Option<u64>) -> String {
    v.map_or_else(|| "none".to_owned(), |v| v.to_string())
}

/// `recorded_at`, microseconds since the epoch, as UTC.
fn recorded_at(envelope: &Value) -> Option<OffsetDateTime> {
    let micros = envelope.get("recorded_at")?.as_i64()?;
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(micros) * 1000).ok()
}

/// The session the envelope's stream names.
fn stream_id(envelope: &Value) -> Option<String> {
    let stream = envelope.get("stream")?;
    if stream.get("kind").and_then(Value::as_str) != Some("session") {
        return None;
    }
    stream
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
}

/// Adds the turns one envelope makes, if any. `call_names` maps each tool
/// call's `call_id` seen so far to its tool's name.
fn add_record(
    turns: &mut Vec<RawTurn>,
    call_names: &mut HashMap<String, String>,
    ts: OffsetDateTime,
    envelope: &Value,
) {
    if envelope.get("payload_type").and_then(Value::as_str) != Some("runtime.session") {
        return;
    }
    let Some(payload) = envelope.get("payload") else {
        return;
    };
    if payload.get("kind").and_then(Value::as_str) != Some("run") {
        return;
    }
    let Some(event) = payload.get("event") else {
        return;
    };
    let text = |key: &str| event.get(key).and_then(Value::as_str);
    match event.get("kind").and_then(Value::as_str) {
        Some("started") => {
            if let Some(prompt) = text("prompt").filter(|p| !p.is_empty()) {
                push_turn(turns, Role::User, ts, None, prompt.to_owned());
            }
        }
        Some("assistant_message_committed") => {
            if let Some(text) = text("text").filter(|t| !t.is_empty()) {
                push_turn(turns, Role::Assistant, ts, None, text.to_owned());
            }
        }
        Some("assistant_tool_calls_committed") => {
            let calls = event.get("tool_calls").and_then(Value::as_array);
            for call in calls.into_iter().flatten() {
                let Some(name) = call.get("name").and_then(Value::as_str) else {
                    continue;
                };
                if let Some(id) = call.get("call_id").and_then(Value::as_str) {
                    call_names.insert(id.to_owned(), name.to_owned());
                }
                let text = match call.get("args") {
                    Some(Value::String(args)) => format!("{name} {args}"),
                    Some(Value::Null) | None => name.to_owned(),
                    Some(args) => format!("{name} {args}"),
                };
                push_turn(turns, Role::Tool, ts, Some(name.to_owned()), text);
            }
        }
        Some("tool_result_batch_committed") => {
            let results = event.get("results").and_then(Value::as_array);
            for result in results.into_iter().flatten() {
                let name = result
                    .get("tool_call_id")
                    .and_then(Value::as_str)
                    .and_then(|id| call_names.get(id))
                    .cloned();
                let text = result
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                push_turn(turns, Role::Tool, ts, name, text.to_owned());
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests;
