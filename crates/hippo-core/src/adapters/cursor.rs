//! Cursor agent transcripts: `~/.cursor/projects/<slug>/agent-transcripts/`
//! ([`DEFAULT_ROOT`]), written by Cursor's agent in the editor and by the
//! `cursor-agent` CLI (see `docs/designs/cursor-adapter.md`).
//!
//! A session is `<id>/<id>.jsonl` (an older layout: `<id>.jsonl` directly in
//! `agent-transcripts/`), and each of its sub-agents is
//! `<id>/subagents/<sub>.jsonl`. Every line is one message, `{"role":
//! "user"|"assistant", "message": {"content": [blocks]}}`, or a turn marker
//! `{"type": "turn_ended", "status"}`, which is skipped. A `text` block
//! makes a user or assistant turn by the message's role; a `tool_use` block
//! makes a tool turn `<name> <input>`. Cursor writes no tool results.
//!
//! The file has no timestamps of its own. Cursor puts the time of each
//! prompt in the person's message, `<timestamp>Friday, Nov 6, 2026, 9:58
//! PM (UTC-7:00)</timestamp>`; each turn takes the time of the prompt it
//! follows, and turns before the first prompt with a time take that first
//! time, or the file's modification time when no prompt has one. Of a
//! person's message only the prompt inside `<user_query>` is indexed when
//! there is one: the rest is context Cursor added.
//!
//! The session is `cursor-<id>`, a sub-agent `cursor-<id>-agent-<sub>`. Its
//! directory is the project folder's `<slug>` read back against the file
//! system ([`resolve_slug`]).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde_json::Value;
use time::{Date, Month, OffsetDateTime, PrimitiveDateTime, Time, UtcOffset};

use super::{
    each_json_line, has_extension_jsonl, push_turn, transcript_roots, Adapter, Parsed, RawTurn,
    Role, Tool,
};
use crate::config::Config;
use crate::error::{Error, Result};

/// Where Cursor keeps its projects' agent transcripts.
pub const DEFAULT_ROOT: &str = "~/.cursor/projects";

/// The folder of one project's transcripts.
const TRANSCRIPTS: &str = "agent-transcripts";

/// The Cursor adapter.
#[derive(Debug, Clone, Copy, Default)]
pub struct Cursor;

impl Adapter for Cursor {
    fn tool(&self) -> Tool {
        Tool::Cursor
    }

    fn roots(&self, cfg: &Config) -> Vec<PathBuf> {
        transcript_roots(cfg)
    }

    fn file_matches(&self, path: &Path) -> bool {
        is_transcript(path)
    }

    fn parse(&self, _root: &Path, path: &Path) -> Result<Parsed> {
        let id = session_id(path).ok_or_else(|| {
            Error::io_at(
                path,
                std::io::Error::new(std::io::ErrorKind::InvalidData, "not a Cursor transcript"),
            )
        })?;
        let mut messages: Vec<(Role, Vec<Block>, Option<OffsetDateTime>)> = Vec::new();
        each_json_line(path, |line| {
            let Some(role) = line.get("role").and_then(Value::as_str) else {
                return;
            };
            let role = match role {
                "user" => Role::User,
                "assistant" => Role::Assistant,
                _ => return,
            };
            let content = line.get("message").and_then(|m| m.get("content"));
            let (blocks, at) = blocks(role, content);
            messages.push((role, blocks, at));
        })?;
        let first = messages.iter().find_map(|(_, _, at)| *at);
        let fallback = first.unwrap_or_else(|| modified(path));
        let mut now = fallback;
        let mut turns: Vec<RawTurn> = Vec::new();
        for (role, blocks, at) in messages {
            if let Some(at) = at {
                now = at;
            }
            for block in blocks {
                match block {
                    Block::Text(text) => push_turn(&mut turns, role, now, None, text),
                    Block::Call(name, text) => {
                        push_turn(&mut turns, Role::Tool, now, Some(name), text)
                    }
                }
            }
        }
        Ok(Parsed {
            id,
            tool: Tool::Cursor,
            started: fallback,
            turns,
        })
    }

    fn cwd(&self, _id: &str, path: &Path) -> Option<PathBuf> {
        cwd_of(path)
    }
}

/// One content block hippo indexes.
enum Block {
    Text(String),
    /// A tool call: the tool's name and `<name> <input>`.
    Call(String, String),
}

/// A message's blocks, and the time a person's message carries.
fn blocks(role: Role, content: Option<&Value>) -> (Vec<Block>, Option<OffsetDateTime>) {
    let mut out = Vec::new();
    let mut at = None;
    let mut text_block = |text: &str, out: &mut Vec<Block>| {
        let text = if role == Role::User {
            if at.is_none() {
                at = prompt_time(text);
            }
            person_text(text)
        } else {
            text.trim().to_owned()
        };
        if !text.is_empty() {
            out.push(Block::Text(text));
        }
    };
    match content {
        Some(Value::String(text)) => text_block(text, &mut out),
        Some(Value::Array(items)) => {
            for item in items {
                match item.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        if let Some(text) = item.get("text").and_then(Value::as_str) {
                            text_block(text, &mut out);
                        }
                    }
                    Some("tool_use") => {
                        let name = item
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("tool")
                            .to_owned();
                        let input = item.get("input").map(Value::to_string).unwrap_or_default();
                        let text = format!("{name} {input}");
                        out.push(Block::Call(name, text.trim_end().to_owned()));
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    (out, at)
}

/// What the person typed: the text inside `<user_query>` when there is
/// one, else the message without its `<timestamp>` element.
pub fn person_text(text: &str) -> String {
    if let Some(query) = between(text, "<user_query>", "</user_query>") {
        return query.trim().to_owned();
    }
    let mut rest = text.to_owned();
    while let Some(start) = rest.find("<timestamp>") {
        let Some(len) = rest[start..].find("</timestamp>") else {
            break;
        };
        rest.replace_range(start..start + len + "</timestamp>".len(), "");
    }
    rest.trim().to_owned()
}

/// The text between the first `open` and the next `close`.
fn between<'a>(text: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = text.find(open)? + open.len();
    let end = text[start..].find(close)? + start;
    Some(&text[start..end])
}

/// The time in a person's message, `<timestamp>Friday, Nov 6, 2026, 9:58 PM
/// (UTC-7:00)</timestamp>`, as UTC. The weekday is not checked; the offset
/// may be left out (UTC) or written `UTC+5:30`, `UTC-07:00` or `UTC`.
pub fn prompt_time(text: &str) -> Option<OffsetDateTime> {
    let inner = between(text, "<timestamp>", "</timestamp>")?.trim();
    let (when, zone) = match inner.find('(') {
        Some(i) => (
            inner[..i].trim(),
            Some(inner[i + 1..].trim_end_matches(')').trim()),
        ),
        None => (inner, None),
    };
    // `Friday, Nov 6, 2026, 9:58 PM`: drop the weekday.
    let parts: Vec<&str> = when.split(',').map(str::trim).collect();
    let [_, month_day, year, clock] = parts.as_slice() else {
        return None;
    };
    let (month, day) = month_day.split_once(' ')?;
    let month = month_of(month)?;
    let day: u8 = day.trim().parse().ok()?;
    let year: i32 = year.parse().ok()?;
    let (hm, half) = clock.split_once(' ')?;
    let (h, m) = hm.split_once(':')?;
    let mut hour: u8 = h.parse().ok()?;
    let minute: u8 = m.parse().ok()?;
    match half.to_ascii_uppercase().as_str() {
        "AM" if hour == 12 => hour = 0,
        "AM" => {}
        "PM" if hour < 12 => hour += 12,
        "PM" => {}
        _ => return None,
    }
    let offset = match zone {
        None => UtcOffset::UTC,
        Some(z) => offset_of(z)?,
    };
    let date = Date::from_calendar_date(year, month, day).ok()?;
    let time = Time::from_hms(hour, minute, 0).ok()?;
    Some(
        PrimitiveDateTime::new(date, time)
            .assume_offset(offset)
            .to_offset(UtcOffset::UTC),
    )
}

fn month_of(name: &str) -> Option<Month> {
    let lower = name.get(..3)?.to_ascii_lowercase();
    Some(match lower.as_str() {
        "jan" => Month::January,
        "feb" => Month::February,
        "mar" => Month::March,
        "apr" => Month::April,
        "may" => Month::May,
        "jun" => Month::June,
        "jul" => Month::July,
        "aug" => Month::August,
        "sep" => Month::September,
        "oct" => Month::October,
        "nov" => Month::November,
        "dec" => Month::December,
        _ => return None,
    })
}

/// `UTC`, `UTC+5:30`, `UTC-07:00` or `UTC+9`.
fn offset_of(zone: &str) -> Option<UtcOffset> {
    let rest = zone
        .strip_prefix("UTC")
        .or_else(|| zone.strip_prefix("GMT"))?;
    if rest.is_empty() {
        return Some(UtcOffset::UTC);
    }
    let sign: i8 = match rest.as_bytes()[0] {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let (h, m) = rest[1..].split_once(':').unwrap_or((&rest[1..], "0"));
    let h: i8 = h.parse().ok()?;
    let m: i8 = m.parse().ok()?;
    UtcOffset::from_hms(sign * h, sign * m, 0).ok()
}

/// The file's modification time, or the Unix epoch when it cannot be read.
fn modified(path: &Path) -> OffsetDateTime {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .map(OffsetDateTime::from)
        .unwrap_or(OffsetDateTime::UNIX_EPOCH)
}

fn name(path: &Path) -> Option<&str> {
    path.file_name()?.to_str()
}

/// Whether `path` is a Cursor transcript: a `.jsonl` file named like its
/// folder under `agent-transcripts/` (or directly in it), or a sub-agent's
/// file in `agent-transcripts/<id>/subagents/`.
pub fn is_transcript(path: &Path) -> bool {
    session_id(path).is_some()
}

/// Whether `path` lies under a Cursor `agent-transcripts` folder: none of
/// its files is a Claude Code transcript.
pub fn is_under_transcripts(path: &Path) -> bool {
    path.components().any(|c| c.as_os_str() == TRANSCRIPTS)
}

/// The hippo session id of the transcript at `path`, when it is one.
pub fn session_id(path: &Path) -> Option<String> {
    if !has_extension_jsonl(path) {
        return None;
    }
    let stem = path.file_stem()?.to_str()?;
    if stem.is_empty() || stem.contains('.') {
        return None;
    }
    let parent = path.parent()?;
    let grand = parent.parent()?;
    if name(parent) == Some(TRANSCRIPTS) {
        return Some(format!("cursor-{stem}"));
    }
    if name(parent) == Some(stem) && name(grand) == Some(TRANSCRIPTS) {
        return Some(format!("cursor-{stem}"));
    }
    if name(parent) == Some("subagents") && name(grand.parent()?) == Some(TRANSCRIPTS) {
        return Some(format!("cursor-{}-agent-{stem}", name(grand)?));
    }
    None
}

/// The project folder name (`<slug>`) of the transcript at `path`: the
/// folder that holds its `agent-transcripts`.
pub fn slug_of(path: &Path) -> Option<&str> {
    let mut parts = path.components().rev().peekable();
    while let Some(c) = parts.next() {
        if c.as_os_str() == TRANSCRIPTS {
            return parts.next()?.as_os_str().to_str();
        }
    }
    None
}

/// The directory the session at `path` ran in, from its project's slug.
pub fn cwd_of(path: &Path) -> Option<PathBuf> {
    let slug = slug_of(path)?;
    static CACHE: OnceLock<Mutex<HashMap<String, Option<PathBuf>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(map) = cache.lock() {
        if let Some(found) = map.get(slug) {
            return found.clone();
        }
    }
    let found = resolve_slug(Path::new("/"), slug);
    if let Ok(mut map) = cache.lock() {
        map.insert(slug.to_owned(), found.clone());
    }
    found
}

/// `text` as Cursor's project slugs write a path's parts: every run of
/// characters other than ASCII letters and digits as one `-`, with none at
/// either end.
pub fn slug(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_owned()
}

/// The existing directory under `base` whose path, as a slug, is `name`
/// (Cursor names a project folder after its absolute path, `/` and other
/// punctuation as `-`, so the name alone cannot be read back): found by
/// walking down from `base`, at most 40 levels, following each child whose
/// slug matches the next parts of `name`. `None` when no directory matches,
/// as for a project that was moved or deleted, or a window with no folder.
pub fn resolve_slug(base: &Path, name: &str) -> Option<PathBuf> {
    let want = slug(name);
    if want.is_empty() {
        return None;
    }
    walk(base, &want, 0)
}

fn walk(dir: &Path, rest: &str, depth: usize) -> Option<PathBuf> {
    if depth > 40 {
        return None;
    }
    let entries = std::fs::read_dir(dir).ok()?;
    let mut fits: Vec<(PathBuf, String)> = Vec::new();
    for entry in entries.flatten() {
        let Some(child) = entry.file_name().to_str().map(slug) else {
            continue;
        };
        if child.is_empty() {
            continue;
        }
        let matches = rest == child
            || (rest.starts_with(&child) && rest.as_bytes().get(child.len()) == Some(&b'-'));
        if matches && entry.path().is_dir() {
            fits.push((entry.path(), child));
        }
    }
    // The longest name first: `my-app` before `my`.
    fits.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(&b.0)));
    for (path, child) in fits {
        if rest == child {
            return Some(path);
        }
        if let Some(found) = walk(&path, &rest[child.len() + 1..], depth + 1) {
            return Some(found);
        }
    }
    None
}

#[cfg(test)]
mod tests;
