//! A session's turns as a reader sees them: a request, a reply, a tool call
//! with its result, or text a tool put in the user's place, one entry each
//! ([`entries`]); the entries as a dialogue, each run of tool calls between
//! two messages one group of steps ([`items`]); and what happened, read from
//! the entries alone ([`digest`]).

use hippocamp_core::index::StoredTurn;
use hippocamp_core::sessions::{call_summary, classify, request_text, Injection, TurnKind};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

/// What an entry is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    /// Something a person asked.
    Request,
    Assistant,
    /// A tool call and its result, either of which may be missing.
    Tool {
        name: String,
        /// The call's one-line summary ([`call_summary`]).
        summary: String,
        /// The call's input, without the tool's name.
        input: Option<String>,
        result: Option<String>,
    },
    /// Text a tool or client put in the user's place.
    Injection(Injection),
}

/// One entry of the conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The first turn's ordinal.
    pub ordinal: u32,
    /// Every ordinal the entry covers (a call and its result).
    pub ordinals: Vec<u32>,
    /// RFC 3339, UTC: the first turn's.
    pub ts: String,
    pub kind: Kind,
    /// The text of a request, a reply or an injection; empty for a tool.
    pub text: String,
}

impl Entry {
    /// Whether the entry is drawn as one line until opened.
    pub fn folds(&self) -> bool {
        matches!(self.kind, Kind::Tool { .. } | Kind::Injection(_))
    }

    /// Whether the entry is the harness's plumbing rather than the work: a
    /// tool search, a skill load, a to-do list, a reminder or notification
    /// the client injected, an image a tool read. Hidden inside a group of
    /// steps until asked for.
    pub fn is_harness(&self) -> bool {
        match &self.kind {
            Kind::Tool { name, .. } => HARNESS_TOOLS.contains(&name.as_str()),
            Kind::Injection(i) => matches!(
                i,
                Injection::SystemReminder
                    | Injection::Instructions
                    | Injection::TaskNotification
                    | Injection::Skill
                    | Injection::Image
            ),
            _ => false,
        }
    }

    /// Whether the entry is a step of the work, grouped with its neighbours
    /// between two messages, rather than a message of its own.
    pub fn is_step(&self) -> bool {
        matches!(self.kind, Kind::Tool { .. }) || self.is_harness()
    }
}

/// Tools that are the harness's plumbing ([`Entry::is_harness`]).
pub const HARNESS_TOOLS: &[&str] = &[
    "ToolSearch",
    "Skill",
    "TodoWrite",
    "TaskOutput",
    "ListMcpResourcesTool",
    "ReadMcpResourceTool",
];

/// The entries of a session's turns, in order. A tool result joins the
/// oldest earlier call of the same tool that has no result yet.
pub fn entries(turns: &[StoredTurn]) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::with_capacity(turns.len());
    for turn in turns {
        let kind = classify(&turn.role, turn.tool_name.as_deref(), &turn.text);
        match kind {
            TurnKind::ToolCall => {
                let name = turn.tool_name.clone().unwrap_or_default();
                let input = turn
                    .text
                    .strip_prefix(name.as_str())
                    .unwrap_or(&turn.text)
                    .trim()
                    .to_string();
                out.push(Entry {
                    ordinal: turn.ordinal,
                    ordinals: vec![turn.ordinal],
                    ts: turn.ts.clone(),
                    kind: Kind::Tool {
                        summary: step_summary(&name, &turn.text),
                        name,
                        input: Some(input),
                        result: None,
                    },
                    text: String::new(),
                });
            }
            TurnKind::ToolResult => {
                let name = turn.tool_name.clone().unwrap_or_default();
                let waiting = out.iter_mut().rev().take(64).rev().find(|e| {
                    matches!(&e.kind, Kind::Tool { name: n, result: None, input: Some(_), .. } if *n == name)
                });
                match waiting {
                    Some(call) => {
                        call.ordinals.push(turn.ordinal);
                        if let Kind::Tool { result, .. } = &mut call.kind {
                            *result = Some(turn.text.clone());
                        }
                    }
                    None => out.push(Entry {
                        ordinal: turn.ordinal,
                        ordinals: vec![turn.ordinal],
                        ts: turn.ts.clone(),
                        kind: Kind::Tool {
                            summary: result_summary(&turn.text),
                            name: if name.is_empty() { "tool".into() } else { name },
                            input: None,
                            result: Some(turn.text.clone()),
                        },
                        text: String::new(),
                    }),
                }
            }
            TurnKind::Request => out.push(text_entry(turn, Kind::Request)),
            TurnKind::Assistant => out.push(text_entry(turn, Kind::Assistant)),
            TurnKind::Injection(i) => out.push(text_entry(turn, Kind::Injection(i))),
        }
    }
    out
}

/// A call's one line as a reader wants it ([`call_summary`]), never raw
/// JSON: an input with none of the known keys gives its first short string
/// value, else its keys. A long absolute path keeps its last three parts.
pub fn step_summary(name: &str, text: &str) -> String {
    let summary = call_summary(name, text);
    if !(summary.starts_with('{') || summary.starts_with('[')) {
        return short_path(&summary);
    }
    let input = text.strip_prefix(name).unwrap_or(text).trim();
    let value = match serde_json::from_str::<serde_json::Value>(input) {
        Ok(serde_json::Value::String(s)) => serde_json::from_str(&s).ok(),
        v => v.ok(),
    };
    fn first_string(v: &serde_json::Value) -> Option<String> {
        match v {
            serde_json::Value::String(s) if !s.trim().is_empty() => Some(first_line(s)),
            serde_json::Value::Array(items) => items.iter().find_map(first_string),
            serde_json::Value::Object(map) => map.values().find_map(first_string),
            _ => None,
        }
    }
    match &value {
        Some(serde_json::Value::Object(map)) if map.is_empty() => String::new(),
        Some(v @ serde_json::Value::Object(map)) => first_string(v)
            .map(|s| short_path(&s))
            .unwrap_or_else(|| map.keys().cloned().collect::<Vec<_>>().join(", ")),
        Some(v) => first_string(v).unwrap_or_default(),
        None => String::new(),
    }
}

/// An orphan result's one line: its first line, or `output · 2 KB` when it
/// is JSON.
fn result_summary(text: &str) -> String {
    let t = text.trim_start();
    if t.starts_with('{') || t.starts_with('[') {
        format!("output · {}", size(text.len()))
    } else {
        first_line(text)
    }
}

/// `/Users/me/code/app/src/ui/tab.rs` as `…/src/ui/tab.rs`: an absolute
/// path of more than three parts, alone on its line, keeps its last three.
pub fn short_path(text: &str) -> String {
    let abs = text.starts_with('/') || text.starts_with("~/");
    if !abs || text.contains(char::is_whitespace) {
        return text.to_string();
    }
    let parts: Vec<&str> = text.split('/').filter(|p| !p.is_empty()).collect();
    if parts.len() <= 3 {
        return text.to_string();
    }
    format!("…/{}", parts[parts.len() - 3..].join("/"))
}

fn text_entry(turn: &StoredTurn, kind: Kind) -> Entry {
    Entry {
        ordinal: turn.ordinal,
        ordinals: vec![turn.ordinal],
        ts: turn.ts.clone(),
        kind,
        text: turn.text.clone(),
    }
}

fn first_line(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let mut out: String = line.chars().take(160).collect();
    if line.chars().count() > 160 {
        out.push('…');
    }
    out
}

/// One message of an earlier thread that a context handoff carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Carried {
    /// `user` or `assistant`, as the handoff names it.
    pub role: String,
    pub text: String,
}

/// A context handoff's messages, without their bracketed provider ids, and
/// its `Selected … omitted …` line when it has one.
pub fn handoff(text: &str) -> (Option<String>, Vec<Carried>) {
    let note = text
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("Selected ") && l.contains("omitted"))
        .map(|l| {
            l.split(". ")
                .next()
                .unwrap_or(l)
                .trim_end_matches('.')
                .to_string()
        });
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("[Historical ") {
        let after = &rest[start + "[Historical ".len()..];
        let Some(close) = after.find(']') else { break };
        let role = after[..close]
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_string();
        let body = &after[close + 1..];
        let end = body.find("[Historical ").unwrap_or(body.len());
        let message = body[..end].trim();
        if !message.is_empty() {
            out.push(Carried {
                role,
                text: message.to_string(),
            });
        }
        rest = &body[end..];
    }
    (note, out)
}

/// One item of the dialogue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    /// A message: a request, a reply, a context handoff or summary, a
    /// command, an interruption. `entry` indexes the entries; `images` counts
    /// the images that directly follow a request (a pasted screenshot).
    Message { entry: usize, images: usize },
    /// The steps between two messages.
    Steps(Steps),
    /// A call that edits or writes files ([`crate::changes::is_edit`]),
    /// shown as its diff rather than folded into the steps around it.
    Edit { entry: usize },
}

/// A run of steps between two messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Steps {
    /// Every entry of the run, in order, harness entries included.
    pub entries: Vec<usize>,
    /// The entries that are work, not harness ([`Entry::is_harness`]).
    pub work: Vec<usize>,
    /// The work's calls by tool, most first, then by name: `(short name,
    /// count)`.
    pub tools: Vec<(String, usize)>,
    /// From the first entry's time to the last's.
    pub seconds: i64,
}

impl Steps {
    /// The group's one line: `47 steps · Bash 30, Read 9, Edit 8 · 2m 10s`.
    pub fn label(&self) -> String {
        let n = self.work.len();
        let mut parts = vec![if n == 1 {
            "1 step".to_string()
        } else {
            format!("{n} steps")
        }];
        let mut tools: Vec<String> = self
            .tools
            .iter()
            .take(3)
            .map(|(name, count)| {
                if n == 1 {
                    name.clone()
                } else {
                    format!("{name} {count}")
                }
            })
            .collect();
        if self.tools.len() > 3 {
            tools.push("…".into());
        }
        if !tools.is_empty() {
            parts.push(tools.join(", "));
        }
        if self.seconds >= 1 {
            parts.push(duration(self.seconds));
        }
        parts.join(" · ")
    }
}

/// The entries as a dialogue: messages in order, each run of steps between
/// two of them one [`Steps`]. A run with no work, only harness entries, is
/// left out. Images directly after a request belong to it. A context
/// handoff or summary that opens the session comes after the first request
/// rather than before it, so the session reads from what was asked.
pub fn items(entries: &[Entry]) -> Vec<Item> {
    let mut out = Vec::new();
    let mut run: Vec<usize> = Vec::new();
    let mut i = 0;
    while i < entries.len() {
        let e = &entries[i];
        if crate::changes::is_edit(e) {
            flush(entries, &mut run, &mut out);
            out.push(Item::Edit { entry: i });
            i += 1;
            continue;
        }
        if e.is_step() {
            run.push(i);
            i += 1;
            continue;
        }
        flush(entries, &mut run, &mut out);
        let mut images = 0;
        if e.kind == Kind::Request {
            while entries
                .get(i + 1 + images)
                .is_some_and(|n| n.kind == Kind::Injection(Injection::Image))
            {
                images += 1;
            }
        }
        out.push(Item::Message { entry: i, images });
        i += 1 + images;
    }
    flush(entries, &mut run, &mut out);

    // Move the first request before the handoffs and summaries that open the
    // session.
    let message = |item: &Item| match item {
        Item::Message { entry, .. } => Some(&entries[*entry].kind),
        Item::Steps(_) | Item::Edit { .. } => None,
    };
    if let Some(req) = out
        .iter()
        .position(|it| matches!(message(it), Some(Kind::Request)))
    {
        let before = &out[..req];
        let carried = |it: &Item| {
            matches!(
                message(it),
                Some(Kind::Injection(
                    Injection::ContextHandoff | Injection::Summary
                ))
            )
        };
        let only_opening = before
            .iter()
            .all(|it| carried(it) || matches!(it, Item::Steps(_) | Item::Edit { .. }));
        if let (Some(first), true) = (before.iter().position(carried), only_opening) {
            let item = out.remove(req);
            out.insert(first, item);
        }
    }
    out
}

fn flush(entries: &[Entry], run: &mut Vec<usize>, out: &mut Vec<Item>) {
    if run.is_empty() {
        return;
    }
    let all = std::mem::take(run);
    let work: Vec<usize> = all
        .iter()
        .copied()
        .filter(|&i| !entries[i].is_harness())
        .collect();
    if work.is_empty() {
        return;
    }
    let mut tools: Vec<(String, usize)> = Vec::new();
    for &i in &work {
        if let Kind::Tool { name, .. } = &entries[i].kind {
            let short = short_tool(name);
            match tools.iter_mut().find(|(n, _)| *n == short) {
                Some((_, c)) => *c += 1,
                None => tools.push((short, 1)),
            }
        }
    }
    tools.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let first = all.first().map(|&i| entries[i].ts.as_str());
    let last = all.last().map(|&i| entries[i].ts.as_str());
    let seconds = match (first.and_then(parse_ts), last.and_then(parse_ts)) {
        (Some(a), Some(b)) => (b - a).whole_seconds().max(0),
        _ => 0,
    };
    out.push(Item::Steps(Steps {
        entries: all,
        work,
        tools,
        seconds,
    }));
}

fn parse_ts(ts: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(ts, &Rfc3339).ok()
}

/// A tool's short name, for counts: an MCP tool's `mcp__<server>__<tool>`
/// as `<tool>`.
pub fn short_tool(name: &str) -> String {
    match name.strip_prefix("mcp__").and_then(|r| r.rsplit_once("__")) {
        Some((_, tool)) => tool.to_string(),
        None if name.is_empty() => "tool".to_string(),
        None => name.to_string(),
    }
}

/// A span of time as a reader says it: `45s`, `2m 10s`, `38m`, `2h 14m`,
/// `3d 4h`.
pub fn duration(seconds: i64) -> String {
    let s = seconds.max(0);
    let (d, h, m, sec) = (s / 86_400, s / 3600 % 24, s / 60 % 60, s % 60);
    match () {
        _ if d > 0 => format!("{d}d {h}h"),
        _ if h > 0 => format!("{h}h {m}m"),
        _ if m >= 10 || (m > 0 && sec == 0) => format!("{m}m"),
        _ if m > 0 => format!("{m}m {sec}s"),
        _ => format!("{sec}s"),
    }
}

/// What happened in a session, read from its entries alone: no model.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Digest {
    /// The first request, without orchestration boilerplate
    /// ([`clean_request`]). A session that opens with a context handoff and
    /// is answered before any new request gives the handoff's original
    /// request, and [`Digest::asked_earlier`] is set.
    pub asked: Option<String>,
    /// [`Digest::asked`] comes from an earlier thread, through a handoff.
    pub asked_earlier: bool,
    /// The last reply: how the session ended, or where it stands.
    pub outcome: Option<String>,
    /// From the first entry's time to the last's.
    pub seconds: Option<i64>,
    /// Sub-agents the session started (`Agent`, `Task`, `spawn_agent`
    /// calls).
    pub agents: usize,
    /// Pull request URLs the replies name, first seen first.
    pub pulls: Vec<String>,
    /// Files the session edited or wrote (`Edit`, `Write`, `MultiEdit`,
    /// `NotebookEdit`, Codex `apply_patch`), first seen first; an edit whose
    /// call failed is not counted.
    pub files: Vec<String>,
}

/// Reads a session's [`Digest`] from its entries.
pub fn digest(entries: &[Entry]) -> Digest {
    let first_reply = entries
        .iter()
        .position(|e| e.kind == Kind::Assistant)
        .unwrap_or(entries.len());
    let opening_request = entries[..first_reply]
        .iter()
        .filter(|e| e.kind == Kind::Request)
        .find_map(|e| clean_request(&e.text));
    let handoff_request = || {
        entries
            .iter()
            .filter(|e| e.kind == Kind::Injection(Injection::ContextHandoff))
            .find_map(|e| clean_request(&e.text))
    };
    let (asked, asked_earlier) = match opening_request {
        Some(r) => (Some(r), false),
        None => match handoff_request() {
            Some(r) => (Some(r), true),
            None => (
                entries
                    .iter()
                    .filter(|e| e.kind == Kind::Request)
                    .find_map(|e| clean_request(&e.text)),
                false,
            ),
        },
    };
    let outcome = entries
        .iter()
        .rev()
        .filter(|e| e.kind == Kind::Assistant)
        .map(|e| e.text.trim())
        .find(|t| !t.is_empty())
        .map(str::to_string);
    let seconds = match (
        entries.first().and_then(|e| parse_ts(&e.ts)),
        entries.last().and_then(|e| parse_ts(&e.ts)),
    ) {
        (Some(a), Some(b)) => Some((b - a).whole_seconds().max(0)),
        _ => None,
    };
    let mut agents = 0;
    let mut pulls: Vec<String> = Vec::new();
    let mut files: Vec<String> = Vec::new();
    for e in entries {
        match &e.kind {
            Kind::Assistant => {
                for url in pull_urls(&e.text) {
                    if !pulls.contains(&url) {
                        pulls.push(url);
                    }
                }
            }
            Kind::Tool {
                name,
                input,
                result,
                ..
            } => {
                if matches!(name.as_str(), "Agent" | "Task" | "spawn_agent") && input.is_some() {
                    agents += 1;
                }
                // A failed edit changed nothing.
                if result.as_deref().is_some_and(crate::changes::failed) {
                    continue;
                }
                for f in edited_files(name, input.as_deref().unwrap_or("")) {
                    if !files.contains(&f) {
                        files.push(f);
                    }
                }
            }
            _ => {}
        }
    }
    Digest {
        asked,
        asked_earlier,
        outcome,
        seconds,
        agents,
        pulls,
        files,
    }
}

/// A request as a person wrote it: without an orchestrator's preamble or
/// citation links ([`request_text`]), and without a client's attachment
/// lines (`[Attached image "a.png" is saved at: …]`,
/// `[User attached one or more files …]`). `None` when nothing is left.
pub fn clean_request(text: &str) -> Option<String> {
    let body = request_text(text)?;
    let kept: Vec<&str> = body
        .lines()
        .filter(|l| {
            let t = l.trim_start();
            !(t.starts_with("[Attached ") || t.starts_with("[User attached "))
        })
        .collect();
    let out = kept.join("\n").trim().to_string();
    (!out.is_empty()).then_some(out)
}

/// GitHub pull request URLs in `text`: `https://github.com/<o>/<r>/pull/<n>`,
/// cut after the number.
pub fn pull_urls(text: &str) -> Vec<String> {
    hippocamp_core::rollup::pull_urls(text)
}

/// A pull request URL as a reader wants it: `#87`, or `repo#87` when
/// `qualified`.
pub fn pull_label(url: &str, qualified: bool) -> String {
    let parts: Vec<&str> = url.trim_end_matches('/').rsplit('/').collect();
    match (parts.first(), parts.get(2)) {
        (Some(n), Some(repo)) if qualified => format!("{repo}#{n}"),
        (Some(n), _) => format!("#{n}"),
        _ => url.to_string(),
    }
}

/// The files a call edits or writes, from its input.
fn edited_files(name: &str, input: &str) -> Vec<String> {
    match name {
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => {
            let value: Option<serde_json::Value> = serde_json::from_str(input).ok();
            value
                .as_ref()
                .and_then(|v| v.get("file_path").or_else(|| v.get("notebook_path")))
                .and_then(|p| p.as_str())
                .map(|p| vec![p.to_string()])
                .unwrap_or_default()
        }
        "apply_patch" => {
            // Codex: the patch is the input, possibly a JSON string.
            let text = match serde_json::from_str::<serde_json::Value>(input) {
                Ok(serde_json::Value::String(s)) => s,
                Ok(v) => v
                    .get("input")
                    .or_else(|| v.get("patch"))
                    .and_then(|p| p.as_str())
                    .unwrap_or(input)
                    .to_string(),
                Err(_) => input.to_string(),
            };
            text.lines()
                .filter_map(|l| {
                    ["*** Update File: ", "*** Add File: ", "*** Delete File: "]
                        .iter()
                        .find_map(|p| l.strip_prefix(p))
                })
                .map(|p| p.trim().to_string())
                .collect()
        }
        _ => Vec::new(),
    }
}

/// A size as a reader sees it: `812 B`, `14 KB`, `1.2 MB`.
pub fn size(bytes: usize) -> String {
    match bytes {
        b if b < 1024 => format!("{b} B"),
        b if b < 1024 * 1024 => format!("{} KB", b.div_ceil(1024)),
        b => format!("{:.1} MB", b as f64 / (1024.0 * 1024.0)),
    }
}

/// The index of the entry that covers `ordinal`, if any.
pub fn position(entries: &[Entry], ordinal: u32) -> Option<usize> {
    entries.iter().position(|e| e.ordinals.contains(&ordinal))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(ordinal: u32, role: &str, tool: Option<&str>, text: &str) -> StoredTurn {
        StoredTurn {
            ordinal,
            role: role.into(),
            ts: format!("2026-11-08T10:00:{ordinal:02}Z"),
            tool_name: tool.map(Into::into),
            text: text.into(),
        }
    }

    #[test]
    fn calls_meet_their_results_and_injections_are_labelled() {
        let turns = vec![
            turn(
                0,
                "user",
                None,
                "Context handoff (s):\n[Historical user; x] hi",
            ),
            turn(1, "user", None, "Fix the build"),
            turn(2, "assistant", None, "Looking."),
            turn(
                3,
                "tool",
                Some("Bash"),
                r#"Bash {"command":"cargo build","description":"Build it"}"#,
            ),
            turn(4, "tool", Some("Read"), r#"Read {"file_path":"/a.rs"}"#),
            turn(5, "tool", Some("Bash"), "error[E0425]: cannot find value"),
            turn(6, "tool", Some("Read"), "1\tfn main() {}"),
            turn(7, "tool", Some("Grep"), "orphan result"),
        ];
        let e = entries(&turns);
        assert_eq!(e.len(), 6);
        assert_eq!(e[0].kind, Kind::Injection(Injection::ContextHandoff));
        assert_eq!(
            (e[1].kind.clone(), e[1].text.as_str()),
            (Kind::Request, "Fix the build")
        );
        assert_eq!(e[2].kind, Kind::Assistant);
        match &e[3].kind {
            Kind::Tool {
                name,
                summary,
                input,
                result,
            } => {
                assert_eq!((name.as_str(), summary.as_str()), ("Bash", "Build it"));
                assert!(input.as_deref().unwrap().starts_with("{\"command\""));
                assert_eq!(result.as_deref(), Some("error[E0425]: cannot find value"));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(e[3].ordinals, vec![3, 5]);
        assert_eq!(e[4].ordinals, vec![4, 6]);
        assert!(
            matches!(&e[5].kind, Kind::Tool { input: None, summary, .. } if summary == "orphan result")
        );
        assert!(e[5].folds() && !e[1].folds());
        assert_eq!(position(&e, 5), Some(3));
        assert_eq!(position(&e, 99), None);
    }

    #[test]
    fn a_handoff_reads_as_its_messages() {
        let text = "Context handoff (full_thread_summary):\nProvider context handoff. Thread: t1.\nSelected 2 intact items; omitted 9 items. Historical material is context.\n\n[Historical user; user_message; thread=t1; item=a%3Ab] Make it dark\n\nplease [Historical assistant; assistant_message; thread=t1] Done: the theme is dark.";
        let (note, items) = handoff(text);
        assert_eq!(
            note.as_deref(),
            Some("Selected 2 intact items; omitted 9 items")
        );
        assert_eq!(
            items,
            vec![
                Carried {
                    role: "user".into(),
                    text: "Make it dark\n\nplease".into()
                },
                Carried {
                    role: "assistant".into(),
                    text: "Done: the theme is dark.".into()
                },
            ]
        );
        assert_eq!(handoff("no entries"), (None, vec![]));
    }

    /// A session shaped like a real one: a context handoff before the
    /// request, an orchestrator's preamble, a long run of tool calls with
    /// JSON arguments and harness noise mixed in, images, sub-agents, a
    /// compaction summary and replies that name pull requests.
    fn messy() -> Vec<StoredTurn> {
        let mut t = Vec::new();
        let mut n = 0u32;
        let mut push = |role: &str, tool: Option<&str>, text: String, secs: u32| {
            t.push(StoredTurn {
                ordinal: n,
                role: role.into(),
                ts: format!(
                    "2026-11-08T{:02}:{:02}:{:02}Z",
                    10 + secs / 3600,
                    secs / 60 % 60,
                    secs % 60
                ),
                tool_name: tool.map(Into::into),
                text,
            });
            n += 1;
        };
        push(
            "user",
            None,
            "Context handoff (full_thread_summary):\nSelected 1 intact items; omitted 4 items.\n\n[Historical user; user_message; thread=t1; item=a%3Ab] Make the gallery load faster".into(),
            0,
        );
        push(
            "user",
            None,
            "<system-reminder>Codebase instructions follow.</system-reminder>".into(),
            1,
        );
        push(
            "user",
            None,
            "Act as the implementation sub-agent for this task.\n\nCache thumbnails on disk, capped at 500 MB.\n\n[Attached image \"shot.png\" is saved at: /x/shot.png]".into(),
            2,
        );
        push("user", None, "[Image: source: /x/images/1.png]".into(), 2);
        push("assistant", None, "Reading the cache code first.".into(), 3);
        push(
            "tool",
            Some("ToolSearch"),
            r#"ToolSearch {"query":"select:mcp__docs__read"}"#.into(),
            4,
        );
        push("tool", Some("ToolSearch"), "found 1 tool".into(), 4);
        push(
            "tool",
            Some("Skill"),
            r#"Skill {"skill":"review"}"#.into(),
            5,
        );
        let mut secs = 6;
        for (tool, count) in [("Bash", 30), ("Read", 9), ("Edit", 8)] {
            for i in 0..count {
                let input = match tool {
                    "Bash" => format!(
                        r#"Bash {{"command":"cargo test -p thumbs {i}","description":"Run test {i}"}}"#
                    ),
                    "Read" => format!(r#"Read {{"file_path":"/src/thumbs/f{i}.rs"}}"#),
                    _ => format!(
                        r#"Edit {{"file_path":"/src/thumbs/f{}.rs","old_string":"a","new_string":"b"}}"#,
                        i % 3
                    ),
                };
                push("tool", Some(tool), input, secs);
                push("tool", Some(tool), format!("ok {i}"), secs + 1);
                secs += 2;
            }
            push(
                "user",
                None,
                "<task-notification><status>completed</status></task-notification>".into(),
                secs,
            );
        }
        push(
            "tool",
            Some("Agent"),
            r#"Agent {"description":"Review the cache","prompt":"Review it."}"#.into(),
            secs,
        );
        push("tool", Some("Agent"), "Looks right.".into(), secs + 30);
        push(
            "assistant",
            None,
            "Opened https://github.com/acme/tern/pull/87 for review.".into(),
            secs + 31,
        );
        push(
            "user",
            None,
            "This session is being continued from a previous conversation that ran out of context. Summary: the cache.".into(),
            secs + 40,
        );
        push(
            "tool",
            Some("TodoWrite"),
            r#"TodoWrite {"todos":[]}"#.into(),
            secs + 41,
        );
        push(
            "assistant",
            None,
            "Done: cold start is 240 ms. PRs https://github.com/acme/tern/pull/87 and https://github.com/acme/tern/pull/88/files are ready.".into(),
            8000,
        );
        t
    }

    #[test]
    fn a_messy_session_reads_as_a_dialogue() {
        let e = entries(&messy());
        let it = items(&e);
        let kinds: Vec<String> = it
            .iter()
            .map(|i| match i {
                Item::Message { entry, images } => match &e[*entry].kind {
                    Kind::Request => format!("request+{images}"),
                    Kind::Assistant => "reply".into(),
                    Kind::Injection(k) => k.label().into(),
                    Kind::Tool { .. } => "tool".into(),
                },
                Item::Steps(s) => format!("steps {}", s.work.len()),
                Item::Edit { .. } => "edit".into(),
            })
            .collect();
        // The request comes first, then the handoff it followed; the
        // reminder-only run between them is gone; the image belongs to the
        // request; each edit stands alone as its diff; the harness-only run
        // after the summary is gone.
        let mut want = vec!["request+1", "Context handoff", "reply", "steps 39"];
        want.extend(["edit"; 8]);
        want.extend(["steps 1", "reply", "Earlier conversation summary", "reply"]);
        assert_eq!(kinds, want);
        let Item::Steps(steps) = &it[3] else { panic!() };
        assert_eq!(
            steps.tools,
            vec![("Bash".to_string(), 30), ("Read".to_string(), 9)]
        );
        // ToolSearch, Skill and the notifications are in the run but not
        // its work.
        assert_eq!(steps.entries.len(), 39 + 2 + 2);
        assert_eq!(steps.label(), "39 steps · Bash 30, Read 9 · 1m 20s");
        // No JSON in the label.
        assert!(!steps.label().contains('{'));
        let Item::Steps(agent) = &it[12] else {
            panic!()
        };
        assert_eq!(agent.label(), "1 step · Agent");
    }

    #[test]
    fn the_digest_says_what_was_asked_and_how_it_ended() {
        let d = digest(&entries(&messy()));
        assert_eq!(
            d.asked.as_deref(),
            Some("Cache thumbnails on disk, capped at 500 MB.")
        );
        assert!(d
            .outcome
            .as_deref()
            .unwrap()
            .starts_with("Done: cold start"));
        assert_eq!(d.seconds, Some(8000));
        assert_eq!(d.agents, 1);
        assert_eq!(
            d.pulls,
            [
                "https://github.com/acme/tern/pull/87",
                "https://github.com/acme/tern/pull/88"
            ]
        );
        assert_eq!(d.files.len(), 3);
        assert!(!d.asked_earlier);
        // Only a handoff: its original request.
        let only = entries(&messy()[..1]);
        assert_eq!(
            digest(&only).asked.as_deref(),
            Some("Make the gallery load faster")
        );
        // A handoff answered before any new request: the earlier thread's
        // request, though a request comes later.
        let mut later = messy();
        later.swap(2, 4);
        let d = digest(&entries(&later));
        assert_eq!(d.asked.as_deref(), Some("Make the gallery load faster"));
        assert!(d.asked_earlier);
        assert_eq!(digest(&[]), Digest::default());
    }

    #[test]
    fn a_step_never_reads_as_json_or_a_long_path() {
        assert_eq!(
            step_summary(
                "mcp__docs__guide",
                r#"mcp__docs__guide {"items":["topic.index","topic.editing"]}"#
            ),
            "topic.index"
        );
        assert_eq!(step_summary("X", r#"X {"n":1,"m":true}"#), "m, n");
        assert_eq!(step_summary("X", "X {}"), "");
        assert_eq!(
            step_summary(
                "Read",
                r#"Read {"file_path":"/Users/me/code/app/src/ui/tab.rs"}"#
            ),
            "…/src/ui/tab.rs"
        );
        assert_eq!(short_path("/a/b.rs"), "/a/b.rs");
        assert_eq!(short_path("ls /a/b/c/d"), "ls /a/b/c/d");
        assert_eq!(result_summary("{\"ok\":true}"), "output · 11 B");
    }

    #[test]
    fn a_failed_edit_is_not_a_file_edited() {
        let turns = vec![
            StoredTurn {
                ordinal: 0,
                role: "tool".into(),
                ts: "2026-11-08T10:00:00Z".into(),
                tool_name: Some("Edit".into()),
                text: r#"Edit {"file_path":"/a.rs","old_string":"x","new_string":"y"}"#.into(),
            },
            StoredTurn {
                ordinal: 1,
                role: "tool".into(),
                ts: "2026-11-08T10:00:01Z".into(),
                tool_name: Some("Edit".into()),
                text: "<tool_use_error>String to replace not found</tool_use_error>".into(),
            },
        ];
        assert!(digest(&entries(&turns)).files.is_empty());
    }

    #[test]
    fn codex_patches_name_their_files() {
        let input = "\"*** Begin Patch\\n*** Update File: src/a.rs\\n@@\\n*** Add File: src/b.rs\\n*** End Patch\"";
        assert_eq!(edited_files("apply_patch", input), ["src/a.rs", "src/b.rs"]);
        assert!(edited_files("Bash", r#"{"file_path":"x"}"#).is_empty());
    }

    #[test]
    fn pull_requests_and_durations_read_short() {
        assert_eq!(
            pull_urls("see https://github.com/a/b/pull/12.) and https://github.com/a/b/issues/3"),
            ["https://github.com/a/b/pull/12"]
        );
        assert_eq!(
            pull_label("https://github.com/a/tern/pull/87", false),
            "#87"
        );
        assert_eq!(
            pull_label("https://github.com/a/tern/pull/87", true),
            "tern#87"
        );
        assert_eq!(duration(45), "45s");
        assert_eq!(duration(130), "2m 10s");
        assert_eq!(duration(38 * 60 + 5), "38m");
        assert_eq!(duration(2 * 3600 + 14 * 60), "2h 14m");
        assert_eq!(duration(3 * 86_400 + 4 * 3600), "3d 4h");
        assert_eq!(
            short_tool("mcp__t3-code__preview_evaluate"),
            "preview_evaluate"
        );
    }

    #[test]
    fn sizes_read_as_bytes_kilobytes_or_megabytes() {
        assert_eq!(size(812), "812 B");
        assert_eq!(size(14 * 1024 - 5), "14 KB");
        assert_eq!(size(1_300_000), "1.2 MB");
    }
}
