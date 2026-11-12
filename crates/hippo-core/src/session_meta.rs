//! What a reader needs to recognise a session beyond its record: where it
//! ran and who started it (its [`Origin`], read from the head of its
//! transcript), a human [`title`], and what kind each turn is
//! ([`classify`]). The app and
//! `hippo show session` read sessions through these functions alike.
//!
//! Everything here reads; nothing writes.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

use serde_json::Value;

use crate::adapters::{dsh, opencode};
use crate::error::Result;
use crate::index::Index;
use crate::redact::redact;

/// The most bytes [`origin`] reads from a transcript.
pub const HEAD_BYTES: u64 = 1 << 20;
/// Once the working directory, session and entrypoint are known, the head
/// is read no further than this looking for a title record.
const HEAD_SOFT_BYTES: usize = 64 << 10;
/// The most lines [`origin`] reads from a transcript's head.
const HEAD_LINES: usize = 40;
/// The most bytes [`origin`] reads from a transcript's end, for a title
/// record written after its head.
pub const TAIL_BYTES: u64 = 64 << 10;
/// How many opening user turns a title looks at, and how much of each.
pub const OPENINGS: usize = 4;
pub const OPENING_CHARS: usize = 4000;
/// The longest title, in characters.
pub const TITLE_CHARS: usize = 100;

/// Where a session ran and who started it, from its transcript's head.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Origin {
    /// The working directory, when the transcript names one.
    pub cwd: Option<PathBuf>,
    /// For a Claude Code sub-agent's transcript, the session that started
    /// it.
    pub parent: Option<String>,
    /// A sub-agent's description, from the `.meta.json` Claude Code writes
    /// beside its transcript; redacted.
    pub description: Option<String>,
    /// The title the tool or its client gave the session: Claude Code's
    /// `custom-title` record (a client's thread name, or `/rename`), else
    /// its `ai-title`; the last one in the transcript wins. Redacted.
    pub named: Option<String>,
    /// Whether [`Origin::named`] is a `custom-title`, which outranks a
    /// sub-agent's description.
    pub named_by_person: bool,
    /// Started by a program rather than a person: Claude Code's `sdk-cli`
    /// entrypoint (`claude -p`), or a working directory under macOS's
    /// per-user temporary directories (`/var/folders`). Not `/tmp`.
    pub automated: bool,
    /// Claude Code's `entrypoint`: `cli` in a terminal, `t3-code` in T3
    /// Code, `sdk-cli` for `claude -p`.
    pub entrypoint: Option<String>,
    /// The first git branch the transcript's head names: Claude Code's
    /// `gitBranch` (not a detached `HEAD`), Codex's `session_meta`
    /// `payload.git.branch`. Redacted.
    pub branch: Option<String>,
}

impl Origin {
    /// The client a person ran the session in, when the transcript says:
    /// `T3 Code` or `terminal`.
    pub fn client(&self) -> Option<&'static str> {
        match self.entrypoint.as_deref()? {
            "t3-code" => Some("T3 Code"),
            "cli" => Some("terminal"),
            _ => None,
        }
    }

    /// The working directory's last component: the project, as rows show
    /// it.
    /// The home folder is never a project.
    pub fn project(&self) -> Option<String> {
        let cwd = self.cwd.as_ref()?;
        if crate::config::user_home().is_some_and(|h| h == *cwd) {
            return None;
        }
        cwd.file_name().map(|n| n.to_string_lossy().into_owned())
    }
}

/// Every indexed session's origin, by id. Reads each transcript's head;
/// a transcript that is gone or unreadable gives an empty origin.
pub fn origins(index: &Index) -> Result<HashMap<String, Origin>> {
    Ok(index
        .sources()?
        .into_iter()
        .map(|(id, (tool, path))| {
            let o = origin(&id, &tool, &path);
            (id, o)
        })
        .collect())
}

/// The origin of session `id`, a `tool` session (`claude-code`, `codex`,
/// `dsh` or `opencode`) whose transcript is `path`.
pub fn origin(id: &str, tool: &str, path: &Path) -> Origin {
    if tool == "dsh" {
        return dsh_origin(path);
    }
    if tool == "opencode" {
        return opencode_origin(path);
    }
    if tool == "command-code" {
        let cwd = crate::adapters::command_code::header(path).and_then(|h| h.cwd);
        return Origin {
            automated: cwd.as_deref().is_some_and(is_temporary),
            cwd,
            ..Origin::default()
        };
    }
    if tool == "muse" {
        let cwd = crate::adapters::muse::workspace_root(path);
        return Origin {
            automated: cwd.as_deref().is_some_and(is_temporary),
            cwd,
            ..Origin::default()
        };
    }
    let mut out = Origin::default();
    let Ok(file) = File::open(path) else {
        return out;
    };
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    // A sub-agent's transcript (`<sessionId>-<stem>`) has no title record:
    // its first record says all there is to read.
    let nested = id != stem && id.ends_with(&format!("-{stem}"));
    let mut reader = BufReader::new(file.take(HEAD_BYTES));
    let mut line = String::new();
    let mut read = 0usize;
    let mut session_id: Option<String> = None;
    let mut entrypoint: Option<String> = None;
    let mut titles: (Option<String>, Option<String>) = (None, None);
    for _ in 0..HEAD_LINES {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(n) => read += n,
        }
        if tool == "codex" {
            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                if v.get("type").and_then(Value::as_str) == Some("session_meta") {
                    out.cwd = v
                        .pointer("/payload/cwd")
                        .and_then(Value::as_str)
                        .map(PathBuf::from);
                    out.branch = v
                        .pointer("/payload/git/branch")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    break;
                }
            }
            continue;
        }
        // Claude Code: the keys sit at the record's top level, after the
        // message; read them without parsing a large message.
        if out.cwd.is_none() {
            out.cwd = top_level_string(&line, "cwd").map(PathBuf::from);
        }
        if session_id.is_none() {
            session_id = top_level_string(&line, "sessionId");
        }
        if entrypoint.is_none() {
            entrypoint = top_level_string(&line, "entrypoint");
        }
        if out.branch.is_none() {
            out.branch = top_level_string(&line, "gitBranch").filter(|b| b != "HEAD");
        }
        note_title(&line, &mut titles);
        let known = out.cwd.is_some() && session_id.is_some() && entrypoint.is_some();
        if known && (nested || has_title(&titles) || read >= HEAD_SOFT_BYTES) {
            break;
        }
    }
    if tool != "codex" && !nested {
        read_tail_titles(path, &mut titles);
        let (custom, ai) = titles;
        out.named_by_person = custom.is_some();
        out.named = custom
            .or(ai)
            .map(|t| collapse(redact(&t).as_str()))
            .filter(|t| !t.is_empty());
    }
    if let Some(parent) = session_id {
        // The adapter names a sub-agent's transcript `<sessionId>-<stem>`.
        if id == format!("{parent}-{stem}") {
            out.parent = Some(parent);
            out.description = sub_agent_description(path);
        }
    }
    out.automated =
        entrypoint.as_deref() == Some("sdk-cli") || out.cwd.as_deref().is_some_and(is_temporary);
    out.entrypoint = entrypoint;
    out.branch = out
        .branch
        .take()
        .map(|b| collapse(redact(&b).as_str()))
        .filter(|b| !b.is_empty() && b != "HEAD" && b.chars().count() <= 120);
    out
}

/// A dsh session's origin, from its log's header and [`dsh::facts`]: the
/// working directory, the session a sub-agent belongs under, its label as
/// the description, and the newest title as the name, which dsh writes
/// itself (not the person). No entrypoint or branch: dsh logs neither.
fn dsh_origin(path: &Path) -> Origin {
    let Some(h) = dsh::header(path) else {
        return Origin::default();
    };
    let facts = dsh::facts(path);
    let clean = |t: Option<String>| {
        t.map(|t| collapse(redact(&t).as_str()))
            .filter(|t| !t.is_empty() && t.chars().count() <= 200)
    };
    let parent = dsh::parent_id(path, &h);
    Origin {
        automated: h.cwd.as_deref().is_some_and(is_temporary),
        description: clean(facts.label).filter(|_| parent.is_some()),
        named: clean(facts.title).filter(|_| parent.is_none()),
        named_by_person: false,
        cwd: h.cwd,
        parent,
        entrypoint: None,
        branch: None,
    }
}

/// An OpenCode session's origin, from its row in the database: the
/// working directory, the session a sub-agent belongs under, and its title,
/// which OpenCode writes itself (not the person): a sub-agent's is the task
/// it was given, its description; another session's is its name. No
/// entrypoint or branch: OpenCode stores neither.
fn opencode_origin(path: &Path) -> Origin {
    let Some(info) = opencode::info(path) else {
        return Origin::default();
    };
    let parent = opencode::ids(path).and_then(|(_, parent)| parent);
    let title = info
        .title
        .filter(|t| !opencode::is_placeholder_title(t))
        .map(|t| collapse(redact(&t).as_str()))
        .filter(|t| !t.is_empty() && t.chars().count() <= 200);
    Origin {
        automated: info.directory.as_deref().is_some_and(is_temporary),
        description: title.clone().filter(|_| parent.is_some()),
        named: title.filter(|_| parent.is_none()),
        named_by_person: false,
        cwd: info.directory,
        parent,
        entrypoint: None,
        branch: None,
    }
}

/// How a session ran, as its transcript records it: the git branch, the
/// model and the reasoning effort, each the newest the transcript names.
/// Read for one session at a time (the app's session tab), not for a list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunFacts {
    pub branch: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
}

/// The most bytes [`run_facts`] reads from a transcript's end.
pub const RUN_TAIL_BYTES: u64 = 1 << 20;

/// Reads a `tool` session's [`RunFacts`] from its transcript at `path`:
/// Claude Code's `gitBranch` and its assistant records' `message.model` and
/// `message.output_config.effort`; Codex's `session_meta` `payload.git.branch`
/// and its `turn_context` `payload.model` and `payload.effort`. The newest
/// value in the last [`RUN_TAIL_BYTES`] wins, else the first in the head.
/// A detached `HEAD` and Claude Code's `<synthetic>` model are not values.
/// Reads only; a missing or unreadable transcript gives nothing.
pub fn run_facts(tool: &str, path: &Path) -> RunFacts {
    if tool == "opencode" {
        let (model, effort) = opencode::model(path);
        let clean = |v: Option<String>| {
            v.map(|s| collapse(redact(&s).as_str()))
                .filter(|s| !s.is_empty())
        };
        return RunFacts {
            branch: None,
            model: clean(model),
            effort: clean(effort),
        };
    }
    if tool == "dsh" {
        let facts = dsh::facts(path);
        let clean = |v: Option<String>| {
            v.map(|s| collapse(redact(&s).as_str()))
                .filter(|s| !s.is_empty())
        };
        return RunFacts {
            branch: None,
            model: clean(facts.model),
            effort: clean(facts.effort),
        };
    }
    let mut out = RunFacts::default();
    let tail = read_tail(path, RUN_TAIL_BYTES).unwrap_or_default();
    // Newest first; the first line of the tail may be cut and is skipped
    // by the parser.
    for line in tail.lines().rev() {
        note_run_fact(tool, line, &mut out);
        if out.branch.is_some() && out.model.is_some() && out.effort.is_some() {
            break;
        }
    }
    if out.branch.is_none() || out.model.is_none() {
        if let Ok(file) = File::open(path) {
            let mut reader = BufReader::new(file.take(HEAD_BYTES));
            let mut line = String::new();
            for _ in 0..HEAD_LINES {
                line.clear();
                match reader.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                let mut head = RunFacts::default();
                note_run_fact(tool, &line, &mut head);
                out.branch = out.branch.take().or(head.branch);
                out.model = out.model.take().or(head.model);
                out.effort = out.effort.take().or(head.effort);
            }
        }
    }
    let clean = |v: Option<String>| {
        v.map(|s| collapse(redact(&s).as_str()))
            .filter(|s| !s.is_empty())
    };
    RunFacts {
        branch: clean(out.branch),
        model: clean(out.model),
        effort: clean(out.effort),
    }
}

/// Fills what `out` lacks from one transcript line.
fn note_run_fact(tool: &str, line: &str, out: &mut RunFacts) {
    let set = |slot: &mut Option<String>, v: Option<&str>| {
        if slot.is_none() {
            if let Some(v) = v.filter(|v| !v.is_empty()) {
                *slot = Some(v.to_string());
            }
        }
    };
    if tool == "codex" {
        let interesting = line.contains("\"turn_context\"") || line.contains("\"session_meta\"");
        if !interesting {
            return;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            return;
        };
        match v.get("type").and_then(Value::as_str) {
            Some("turn_context") => {
                set(
                    &mut out.model,
                    v.pointer("/payload/model").and_then(Value::as_str),
                );
                set(
                    &mut out.effort,
                    v.pointer("/payload/effort").and_then(Value::as_str),
                );
            }
            Some("session_meta") => {
                set(
                    &mut out.branch,
                    v.pointer("/payload/git/branch").and_then(Value::as_str),
                );
            }
            _ => {}
        }
        return;
    }
    if out.branch.is_none() {
        let b = top_level_string(line, "gitBranch");
        set(&mut out.branch, b.as_deref().filter(|b| *b != "HEAD"));
    }
    if (out.model.is_none() || out.effort.is_none())
        && line.starts_with("{")
        && line.contains("\"type\":\"assistant\"")
    {
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            set(
                &mut out.model,
                v.pointer("/message/model")
                    .and_then(Value::as_str)
                    .filter(|m| !m.starts_with('<')),
            );
            set(
                &mut out.effort,
                v.pointer("/message/output_config/effort")
                    .and_then(Value::as_str),
            );
        }
    }
}

/// A file's last `max` bytes, as text.
fn read_tail(path: &Path, max: u64) -> Option<String> {
    use std::io::{Seek, SeekFrom};
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(max))).ok()?;
    let mut bytes = Vec::new();
    file.take(max).read_to_end(&mut bytes).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// Keeps the last `custom-title` and `ai-title` seen.
fn note_title(line: &str, titles: &mut (Option<String>, Option<String>)) {
    if line.starts_with("{\"type\":\"custom-title\"") {
        if let Some(t) = top_level_string(line, "customTitle") {
            titles.0 = Some(t);
        }
    } else if line.starts_with("{\"type\":\"ai-title\"") {
        if let Some(t) = top_level_string(line, "aiTitle") {
            titles.1 = Some(t);
        }
    }
}

/// Whether a title has been seen; the head stops early only then.
fn has_title(titles: &(Option<String>, Option<String>)) -> bool {
    titles.0.is_some() || titles.1.is_some()
}

/// Title records in the transcript's last [`TAIL_BYTES`]: a session renamed
/// late has its newest title there.
fn read_tail_titles(path: &Path, titles: &mut (Option<String>, Option<String>)) {
    use std::io::{Seek, SeekFrom};
    let Ok(mut file) = File::open(path) else {
        return;
    };
    let Ok(len) = file.metadata().map(|m| m.len()) else {
        return;
    };
    if file
        .seek(SeekFrom::Start(len.saturating_sub(TAIL_BYTES)))
        .is_err()
    {
        return;
    }
    let mut bytes = Vec::new();
    if file.take(TAIL_BYTES).read_to_end(&mut bytes).is_err() {
        return;
    }
    let text = String::from_utf8_lossy(&bytes);
    // The first line may be cut; title records are short and whole.
    for line in text.lines() {
        note_title(line, titles);
    }
}

/// `"key":"value"` at the top level of one JSON line, decoded. A key inside
/// a string value is escaped (`\"key\"`) and so never matches.
fn top_level_string(line: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\":\"");
    let start = line.find(&needle)? + needle.len() - 1;
    let rest = &line[start..];
    // Find the closing quote, skipping escaped characters.
    let bytes = rest.as_bytes();
    let mut i = 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return serde_json::from_str(&rest[..=i]).ok(),
            _ => i += 1,
        }
    }
    None
}

/// `description` from `<transcript stem>.meta.json`, redacted.
/// The description of sub-agent `agent_id` of the Claude Code session
/// whose transcript is `transcript`, from the `.meta.json` beside the
/// sub-agent's transcript (`<dir>/<session>/subagents/agent-<id>.jsonl`).
pub fn agent_description(transcript: &Path, agent_id: &str) -> Option<String> {
    if agent_id.is_empty() || !agent_id.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    let sub = transcript
        .with_extension("")
        .join("subagents")
        .join(format!("agent-{agent_id}.jsonl"));
    sub_agent_description(&sub)
}

fn sub_agent_description(path: &Path) -> Option<String> {
    sub_agent_meta(path).description
}

/// What Claude Code's `.meta.json` beside a sub-agent's transcript says:
/// its description and model, redacted. Nothing else is kept.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SubAgentMeta {
    pub description: Option<String>,
    /// `haiku`, `sonnet`, `opus`, or a model id.
    pub model: Option<String>,
    /// Claude Code says the sub-agent finished (`"finished": true`).
    pub finished: Option<bool>,
}

/// The `.meta.json` beside the sub-agent transcript at `path`; at most
/// 64 KiB is read.
pub fn sub_agent_meta(path: &Path) -> SubAgentMeta {
    let mut out = SubAgentMeta::default();
    if opencode::split(path).is_some() {
        let clean = |t: Option<String>, max: usize| {
            t.map(|t| collapse(redact(&t).as_str()))
                .filter(|t| !t.is_empty() && t.chars().count() <= max)
        };
        out.description = clean(
            opencode::info(path)
                .and_then(|i| i.title)
                .filter(|t| !opencode::is_placeholder_title(t)),
            200,
        );
        out.model = clean(opencode::model(path).0, 60);
        return out;
    }
    if dsh::is_log(path) {
        let facts = dsh::facts(path);
        let clean = |t: Option<String>, max: usize| {
            t.map(|t| collapse(redact(&t).as_str()))
                .filter(|t| !t.is_empty() && t.chars().count() <= max)
        };
        out.description = clean(facts.label, 200);
        out.model = clean(facts.agent_model, 60);
        return out;
    }
    let meta = path.with_extension("meta.json");
    let Ok(file) = File::open(meta) else {
        return out;
    };
    let mut text = String::new();
    if file.take(64 << 10).read_to_string(&mut text).is_err() {
        return out;
    }
    let Ok(v) = serde_json::from_str::<Value>(&text) else {
        return out;
    };
    let field = |k: &str| {
        let d = collapse(redact(v.get(k)?.as_str()?).as_str());
        (!d.is_empty() && d.chars().count() <= 200).then_some(d)
    };
    out.description = field("description");
    out.model = field("model").filter(|m| m.chars().count() <= 60);
    out.finished = v.get("finished").and_then(Value::as_bool);
    out
}

/// macOS's per-user temporary directories (`$TMPDIR`), where programs, not
/// people, start sessions. `/tmp` is not one: people work there too.
fn is_temporary(cwd: &Path) -> bool {
    [Path::new("/var/folders"), Path::new("/private/var/folders")]
        .iter()
        .any(|r| cwd.starts_with(r))
}

/// What kind of text a user turn carries that is not a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Injection {
    /// A client's summary of an earlier thread (`Context handoff (…):`).
    ContextHandoff,
    SystemReminder,
    TaskNotification,
    /// A slash command, its caveat or its output.
    Command,
    /// A skill's text, loaded into the conversation.
    Skill,
    /// `[Request interrupted by user]` and the like.
    Interrupted,
    /// An image the client put in the conversation (`[Image: …]`): a
    /// pasted screenshot, or a picture a tool read.
    Image,
    /// A summary of the conversation before a compaction: Claude Code's
    /// (`This session is being continued from a previous conversation …`)
    /// and the OpenCode adapter's (`[Compaction summary, auto]`).
    Summary,
    /// Instructions a client put first in the conversation: Codex's
    /// `# AGENTS.md instructions for <dir>`, `<user_instructions>` and
    /// `<environment_context>`.
    Instructions,
}

impl Injection {
    /// The label a reader sees.
    pub fn label(self) -> &'static str {
        match self {
            Injection::ContextHandoff => "Context handoff",
            Injection::SystemReminder => "System reminder",
            Injection::TaskNotification => "Task notification",
            Injection::Command => "Command",
            Injection::Skill => "Skill",
            Injection::Interrupted => "Interrupted",
            Injection::Image => "Image",
            Injection::Summary => "Earlier conversation summary",
            Injection::Instructions => "Instructions",
        }
    }
}

/// What a turn is, for a reader.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnKind {
    /// Something a person asked.
    Request,
    /// Text a tool or client put in the user's place.
    Injection(Injection),
    Assistant,
    /// A tool call: its text is `<name> <input>`.
    ToolCall,
    /// A tool's output.
    ToolResult,
}

const INJECTIONS: &[(&str, Injection)] = &[
    ("Context handoff (", Injection::ContextHandoff),
    ("<system-reminder>", Injection::SystemReminder),
    ("<task-notification>", Injection::TaskNotification),
    ("<local-command-caveat>", Injection::Command),
    ("<local-command-stdout>", Injection::Command),
    ("<local-command-stderr>", Injection::Command),
    ("<command-name>", Injection::Command),
    ("<command-message>", Injection::Command),
    ("Caveat: The messages below", Injection::Command),
    ("Base directory for this skill:", Injection::Skill),
    ("[Request interrupted", Injection::Interrupted),
    ("[Image: ", Injection::Image),
    ("# AGENTS.md instructions", Injection::Instructions),
    ("<user_instructions>", Injection::Instructions),
    ("<environment_context>", Injection::Instructions),
    (
        "This session is being continued from a previous conversation",
        Injection::Summary,
    ),
    // The OpenCode adapter's turn for a compaction summary.
    ("[Compaction summary", Injection::Summary),
];

/// The kind of a turn with this `role` (`user`, `assistant` or `tool`),
/// tool name and text.
pub fn classify(role: &str, tool_name: Option<&str>, text: &str) -> TurnKind {
    match role {
        "assistant" => TurnKind::Assistant,
        "tool" => match tool_name {
            Some(name)
                if text == name
                    || text
                        .strip_prefix(name)
                        .is_some_and(|r| r.starts_with(" {") || r.starts_with(" [")) =>
            {
                TurnKind::ToolCall
            }
            _ => TurnKind::ToolResult,
        },
        _ => injection(text).map_or(TurnKind::Request, TurnKind::Injection),
    }
}

fn injection(text: &str) -> Option<Injection> {
    let t = text.trim_start();
    INJECTIONS
        .iter()
        .find(|(prefix, _)| t.starts_with(prefix))
        .map(|(_, kind)| *kind)
}

/// The opening user turns among a session's own turns, as
/// [`Index::openings`] gives them for every session: the user turns among
/// the first [`crate::index::OPENING_WINDOW`] ordinals, at most
/// [`OPENINGS`], each cut to [`OPENING_CHARS`] characters.
pub fn openings_of(turns: &[crate::index::StoredTurn]) -> Vec<String> {
    turns
        .iter()
        .filter(|t| i64::from(t.ordinal) < crate::index::OPENING_WINDOW && t.role == "user")
        .take(OPENINGS)
        .map(|t| t.text.chars().take(OPENING_CHARS).collect())
        .collect()
}

/// A session's title: the name a person or client gave it (`custom-title`);
/// a sub-agent's description; the tool's own `ai-title`; else the first request
/// among `openings` (its opening user turns), what is not a request taken
/// out; else the record's `task`, likewise. `None` when none gives one.
pub fn title(task: &str, openings: &[String], origin: Option<&Origin>) -> Option<String> {
    if let Some(o) = origin {
        let named = o
            .named
            .as_deref()
            .map(headline)
            .filter(|t| !is_junk_title(t));
        let described = o.description.as_deref().map(headline);
        let first = if o.named_by_person {
            named.clone().or(described)
        } else {
            described.or(named.clone())
        };
        if first.is_some() {
            return first;
        }
    }
    openings
        .iter()
        .map(String::as_str)
        .chain(std::iter::once(task))
        .find_map(request_text)
        .map(|r| headline(&r))
        .filter(|h| !h.is_empty())
}

/// The request in a user turn's text, or `None` when it carries none. A
/// context handoff's request is its first `[Historical user; …]` entry.
pub fn request_text(text: &str) -> Option<String> {
    let t = text.trim();
    let body = match injection(t) {
        Some(Injection::ContextHandoff) => handoff_request(t)?,
        Some(_) => return None,
        None => t,
    };
    let body = strip_preamble(body);
    let body = strip_citations(body);
    let body = strip_attachments(&body);
    let body = body.trim();
    (!body.is_empty() && injection(body).is_none()).then(|| body.to_string())
}

static FILE_TAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[(?:File|Attached [^:\]]*|Image):?[^\]]*\]").expect("valid regex")
});
static CLIENT_BLOCK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)<(t3_context|attachments?)\b[^>]*>.*?</(?:t3_context|attachments?)>")
        .expect("valid regex")
});
static TAG_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^[ \t]*</?[A-Za-z_][\w-]*(?:\s[^>\n]*)?>[ \t]*").expect("valid regex")
});
static LEADING_ID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}|[0-9a-f]{12,40})\b[\s\-–—:]*")
        .expect("valid regex")
});

/// A request without what only tags an attachment: a
/// pasted file's `[File: …]`, a client's `<t3_context>` block, a tag on a
/// line of its own or opening one (`<pasted_content id="…">`), and a
/// leading session id or hash (`15cef8bc-… - continue this`).
fn strip_attachments(text: &str) -> String {
    let t = FILE_TAG.replace_all(text, "");
    let t = CLIENT_BLOCK.replace_all(&t, "");
    let t = TAG_LINE.replace_all(&t, "");
    let t = LEADING_ID.replace(t.trim_start(), "");
    t.into_owned()
}

/// A client's title that names nothing: a session id or
/// hash, a path, a tag, or a slash command's caveat.
pub fn is_junk_title(title: &str) -> bool {
    let t = title.trim();
    let bare = t.strip_prefix("Session ").unwrap_or(t);
    t.is_empty()
        || t.starts_with('/')
        || t.starts_with("~/")
        || t.starts_with('<')
        || t.starts_with("Caveat:")
        || LEADING_ID
            .find(bare)
            .is_some_and(|m| m.end() >= bare.len().saturating_sub(1))
}

/// The first `[Historical user; …]` entry's text in a context handoff.
fn handoff_request(text: &str) -> Option<&str> {
    let start = text.find("[Historical user;")?;
    let rest = &text[start..];
    let close = rest.find(']')?;
    let rest = &rest[close + 1..];
    let end = rest.find("[Historical ").unwrap_or(rest.len());
    Some(&rest[..end])
}

/// Removes leading orchestration lines, such as a repeated "Act as the
/// implementation sub-agent for this task.".
fn strip_preamble(mut text: &str) -> &str {
    loop {
        let t = text.trim_start();
        let line_end = t.find('\n').unwrap_or(t.len());
        let line = t[..line_end].trim();
        let preamble = line.starts_with("Act as the ")
            && (line.ends_with("for this task.") || line.ends_with("for this task"));
        if !preamble {
            return t;
        }
        text = &t[line_end..];
    }
}

/// Removes Markdown links to non-web citations (`[Assistant quote](t3-citation://…)`).
fn strip_citations(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        let after = &rest[open..];
        let link = after.find("](").and_then(|mid| {
            let url = &after[mid + 2..];
            let close = url.find(')')?;
            let scheme_end = url.find("://")?;
            let scheme = &url[..scheme_end];
            let web = matches!(scheme, "http" | "https" | "mailto");
            (!web && scheme_end < close && !after[..mid].contains('\n'))
                .then_some(mid + 2 + close + 1)
        });
        match link {
            Some(len) => {
                out.push_str(&rest[..open]);
                rest = &rest[open + len..];
            }
            None => {
                out.push_str(&rest[..=open]);
                rest = &rest[open + 1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// A title from a request: its first non-empty line, Markdown heading and
/// emphasis marks removed, whitespace collapsed; a line over
/// [`TITLE_CHARS`] is cut at its first sentence's end when that comes
/// early enough, else at a word boundary with an ellipsis.
pub fn headline(request: &str) -> String {
    let line = request
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let line = line
        .trim_start_matches('#')
        .replace("**", "")
        .replace('`', "");
    let line = collapse(&line);
    if line.chars().count() <= TITLE_CHARS {
        return line;
    }
    let mut cut = None;
    for (i, c) in line.char_indices() {
        if line[..i].chars().count() >= TITLE_CHARS {
            break;
        }
        if matches!(c, '.' | '?' | '!') && line[i + 1..].starts_with(' ') && i > 12 {
            cut = Some(i + 1);
            break;
        }
    }
    if let Some(end) = cut {
        return line[..end].to_string();
    }
    let mut out = String::new();
    for word in line.split(' ') {
        if out.chars().count() + word.chars().count() + 1 > TITLE_CHARS - 1 {
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    if out.is_empty() {
        out = line.chars().take(TITLE_CHARS - 1).collect();
    }
    out.push('…');
    out
}

fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A tool call's one-line summary: its input's description, command, path,
/// pattern, query or URL when it has one, else the input as compact JSON.
/// `text` is the call's turn text, `<name> <input>`.
pub fn call_summary(name: &str, text: &str) -> String {
    let input = text.strip_prefix(name).unwrap_or(text).trim();
    let value: Option<Value> = serde_json::from_str(input).ok();
    // Codex's arguments are a JSON string of JSON.
    let value = match value {
        Some(Value::String(s)) => serde_json::from_str(&s).ok(),
        v => v,
    };
    const KEYS: &[&str] = &[
        "description",
        "command",
        "cmd",
        "file_path",
        "path",
        "pattern",
        "query",
        "url",
        "skill",
        "prompt",
        "subject",
        "title",
    ];
    if let Some(Value::Object(map)) = &value {
        for key in KEYS {
            match map.get(*key) {
                Some(Value::String(s)) if !s.trim().is_empty() => return first_line(s),
                Some(Value::Array(parts)) if !parts.is_empty() => {
                    if let Some(Value::String(s)) = parts.last() {
                        return first_line(s);
                    }
                }
                _ => {}
            }
        }
    }
    first_line(input)
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

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn attachment_tags_and_leading_ids_are_not_the_request() {
        assert_eq!(
            request_text("[File: schema (1).sql; ref=file_1]  read this and give me your thoughts\n\n<t3_context version=\"1\">\n<context kind=\"file\">x</context>\n</t3_context>").as_deref(),
            Some("read this and give me your thoughts")
        );
        assert_eq!(
            request_text("15cef8bc-480c-46e1-b1cd-cb9a07affaba - continue this").as_deref(),
            Some("continue this")
        );
        assert_eq!(
            request_text("<pasted_content id=\"7be5\">\nCPU load is high\n</pasted_content>")
                .as_deref(),
            Some("CPU load is high")
        );
        assert_eq!(
            request_text("# AGENTS.md instructions\n\n<INSTRUCTIONS>x"),
            None
        );
        assert_eq!(request_text("<command-name>/login</command-name>"), None);
    }

    #[test]
    fn junk_client_titles_are_skipped() {
        for t in [
            "Session 15cef8bc-480c-46e1-b1cd-cb9a07affaba",
            "<inputs>",
            "/Volumes/x/HANDOFF.md",
            "Caveat: The messages below were generated",
        ] {
            assert!(is_junk_title(t), "{t}");
        }
        for t in ["ABC-129 share link", "Fix 3 bugs", "hippo: part 2"] {
            assert!(!is_junk_title(t), "{t}");
        }
    }
    use std::fs;

    #[test]
    fn a_handoff_titles_by_its_first_historical_request() {
        let handoff = "Context handoff (full_thread_summary):\nProvider context handoff. Thread: t1. Covered app runs: 1-3.\nSource item range: turn-item:message:x%3Ay.\n\n[Historical user; user_message; thread=t1; run=run:1; item=i%3A1; status=completed] Add a dark theme to the settings page\nand test it [Historical assistant; assistant_message; thread=t1] Done.";
        assert_eq!(
            request_text(handoff).as_deref(),
            Some("Add a dark theme to the settings page\nand test it")
        );
        assert_eq!(
            title("", &[handoff.to_string()], None).as_deref(),
            Some("Add a dark theme to the settings page")
        );
        assert_eq!(request_text("Context handoff (x):\nno entries"), None);
    }

    #[test]
    fn preamble_injections_and_citations_are_not_requests() {
        let wrapped = "Act as the implementation sub-agent for this task.\n\nAct as the implementation sub-agent for this task.\n\nFix the login retry loop.";
        assert_eq!(
            request_text(wrapped).as_deref(),
            Some("Fix the login retry loop.")
        );
        for t in [
            "<system-reminder>\nx\n</system-reminder>",
            "<task-notification>\n<task-id>a</task-id>",
            "<local-command-caveat>Caveat</local-command-caveat>",
            "Base directory for this skill: /x",
            "[Request interrupted by user]",
            "# AGENTS.md instructions for /w/widgets\n\n<INSTRUCTIONS>\nbe brief",
            "<environment_context>\n  <cwd>/w</cwd>",
            "   ",
        ] {
            assert_eq!(request_text(t), None, "{t}");
        }
        let quoted = "[Assistant quote](t3-citation://v1/a/b?text=Hi+there) \n\nmake it a table [docs](https://example.com)";
        assert_eq!(
            request_text(quoted).as_deref(),
            Some("make it a table [docs](https://example.com)")
        );
        let openings = vec![
            "<local-command-caveat>c</local-command-caveat>".to_string(),
            "Base directory for this skill: /s".to_string(),
            "Ship the release notes".to_string(),
        ];
        assert_eq!(
            title("task text", &openings, None).as_deref(),
            Some("Ship the release notes")
        );
        assert_eq!(
            title("  plan the week ", &[], None).as_deref(),
            Some("plan the week")
        );
        assert_eq!(title("", &[], None), None);
    }

    #[test]
    fn a_long_request_is_cut_at_its_first_sentence_or_a_word() {
        let long = format!(
            "You are resuming work that a rate limit interrupted. {}",
            "word ".repeat(40)
        );
        assert_eq!(
            headline(&long),
            "You are resuming work that a rate limit interrupted."
        );
        let run_on = "word ".repeat(40);
        let h = headline(&run_on);
        assert!(h.ends_with('…') && h.chars().count() <= TITLE_CHARS, "{h}");
        assert_eq!(headline("## **Plan** the `week`\nmore"), "Plan the week");
    }

    #[test]
    fn run_facts_are_the_newest_the_transcript_names() {
        let dir = tempfile::tempdir().unwrap();
        let claude = dir.path().join("s.jsonl");
        fs::write(
            &claude,
            concat!(
                "{\"type\":\"user\",\"gitBranch\":\"HEAD\",\"message\":{\"role\":\"user\",\"content\":\"hi\"}}\n",
                "{\"type\":\"assistant\",\"gitBranch\":\"feat/old\",\"message\":{\"model\":\"m-old\",\"output_config\":{\"effort\":\"low\"}}}\n",
                "{\"type\":\"assistant\",\"gitBranch\":\"feat/new\",\"message\":{\"model\":\"m-new\",\"output_config\":{\"effort\":\"high\"}}}\n",
                "{\"type\":\"assistant\",\"message\":{\"model\":\"<synthetic>\"}}\n",
            ),
        )
        .unwrap();
        assert_eq!(
            run_facts("claude-code", &claude),
            RunFacts {
                branch: Some("feat/new".into()),
                model: Some("m-new".into()),
                effort: Some("high".into()),
            }
        );
        let codex = dir.path().join("rollout.jsonl");
        fs::write(
            &codex,
            concat!(
                "{\"type\":\"session_meta\",\"payload\":{\"cwd\":\"/w\",\"git\":{\"branch\":\"main\"}}}\n",
                "{\"type\":\"turn_context\",\"payload\":{\"model\":\"gpt-x\",\"effort\":\"medium\"}}\n",
            ),
        )
        .unwrap();
        let f = run_facts("codex", &codex);
        assert_eq!(
            (f.branch.as_deref(), f.model.as_deref(), f.effort.as_deref()),
            (Some("main"), Some("gpt-x"), Some("medium"))
        );
        assert_eq!(
            run_facts("codex", &dir.path().join("gone")),
            RunFacts::default()
        );
    }

    #[test]
    fn turns_are_classified_for_a_reader() {
        assert_eq!(classify("user", None, "hello"), TurnKind::Request);
        assert_eq!(
            classify("user", None, "Context handoff (s):\n…"),
            TurnKind::Injection(Injection::ContextHandoff)
        );
        assert_eq!(classify("assistant", None, "Done."), TurnKind::Assistant);
        assert_eq!(
            classify("tool", Some("Bash"), r#"Bash {"command":"ls"}"#),
            TurnKind::ToolCall
        );
        assert_eq!(
            classify("tool", Some("Bash"), "file.txt\n"),
            TurnKind::ToolResult
        );
        assert_eq!(classify("tool", None, "output"), TurnKind::ToolResult);
        assert_eq!(
            classify("user", None, "[Image: source: /tmp/x/images/1.png]"),
            TurnKind::Injection(Injection::Image)
        );
        let compacted = "This session is being continued from a previous conversation that ran out of context. The summary below covers the earlier portion.";
        assert_eq!(
            classify("user", None, compacted),
            TurnKind::Injection(Injection::Summary)
        );
        // Neither is a request, so neither becomes a title.
        assert_eq!(request_text(compacted), None);
        assert_eq!(request_text("[Image: original 1280x4512]"), None);
    }

    #[test]
    fn a_call_summary_names_what_the_call_does() {
        assert_eq!(
            call_summary(
                "Bash",
                r#"Bash {"command":"cargo test","description":"Run the tests"}"#
            ),
            "Run the tests"
        );
        assert_eq!(
            call_summary("Read", r#"Read {"file_path":"/a/b.rs"}"#),
            "/a/b.rs"
        );
        assert_eq!(
            call_summary(
                "shell",
                r#"shell "{\"command\":[\"bash\",\"-lc\",\"ls -la\"]}""#
            ),
            "ls -la"
        );
        assert_eq!(call_summary("Odd", r#"Odd {"n":1}"#), r#"{"n":1}"#);
    }

    #[test]
    fn origins_come_from_the_transcript_head() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("proj/s1.jsonl");
        let agent = dir.path().join("proj/s1/subagents/agent-a1.jsonl");
        fs::create_dir_all(agent.parent().unwrap()).unwrap();
        fs::write(&main, "{\"type\":\"summary\"}\n{\"type\":\"user\",\"message\":{\"content\":\"say \\\"cwd\\\":\\\"/nope\\\"\"},\"entrypoint\":\"cli\",\"cwd\":\"/work/alpha\",\"sessionId\":\"s1\"}\n").unwrap();
        fs::write(&agent, "{\"type\":\"user\",\"message\":{\"content\":\"go\"},\"entrypoint\":\"cli\",\"cwd\":\"/work/alpha/sub\",\"sessionId\":\"s1\"}\n").unwrap();
        fs::write(
            agent.with_extension("meta.json"),
            r#"{"agentType":"general-purpose","description":"Review the  parser"}"#,
        )
        .unwrap();
        let o = origin("s1", "claude-code", &main);
        assert_eq!(o.cwd.as_deref(), Some(Path::new("/work/alpha")));
        assert_eq!(o.project().as_deref(), Some("alpha"));
        assert_eq!((o.parent, o.description, o.automated), (None, None, false));
        let a = origin("s1-agent-a1", "claude-code", &agent);
        assert_eq!(a.parent.as_deref(), Some("s1"));
        assert_eq!(a.description.as_deref(), Some("Review the parser"));
        assert_eq!(
            title("", &["go".into()], Some(&a)).as_deref(),
            Some("Review the parser")
        );

        let named = dir.path().join("proj/s3.jsonl");
        let filler = format!("{{\"type\":\"user\",\"message\":{{\"content\":\"{}\"}},\"cwd\":\"/w\",\"entrypoint\":\"cli\",\"sessionId\":\"s3\"}}\n", "x".repeat(300));
        let mut text = String::from(
            "{\"type\":\"ai-title\",\"aiTitle\":\"Early guess\",\"sessionId\":\"s3\"}\n",
        );
        text.push_str(&filler.repeat(300));
        text.push_str("{\"type\":\"custom-title\",\"customTitle\":\"Parser: lexer rewrite\",\"sessionId\":\"s3\"}\n");
        fs::write(&named, text).unwrap();
        let n = origin("s3", "claude-code", &named);
        assert_eq!(
            n.named.as_deref(),
            Some("Parser: lexer rewrite"),
            "the tail's title wins"
        );
        assert!(n.named_by_person);
        assert_eq!(
            title("", &["go".into()], Some(&n)).as_deref(),
            Some("Parser: lexer rewrite")
        );
        let ai = Origin {
            named: Some("Tool's guess".into()),
            description: Some("Read the parser".into()),
            ..Origin::default()
        };
        assert_eq!(
            title("", &[], Some(&ai)).as_deref(),
            Some("Read the parser"),
            "a description outranks an ai-title"
        );

        let in_tmp = dir.path().join("proj/s4.jsonl");
        fs::write(&in_tmp, "{\"type\":\"user\",\"entrypoint\":\"cli\",\"cwd\":\"/tmp/demo\",\"sessionId\":\"s4\"}\n").unwrap();
        assert!(
            !origin("s4", "claude-code", &in_tmp).automated,
            "people work in /tmp"
        );
        let in_tmpdir = dir.path().join("proj/s5.jsonl");
        fs::write(&in_tmpdir, "{\"type\":\"user\",\"entrypoint\":\"cli\",\"cwd\":\"/private/var/folders/x/T/title-1\",\"sessionId\":\"s5\"}\n").unwrap();
        assert!(origin("s5", "claude-code", &in_tmpdir).automated);

        let auto = dir.path().join("proj/s2.jsonl");
        fs::write(&auto, "{\"type\":\"user\",\"entrypoint\":\"sdk-cli\",\"cwd\":\"/work/x\",\"sessionId\":\"s2\"}\n").unwrap();
        assert!(origin("s2", "claude-code", &auto).automated);

        let rollout = dir.path().join("rollout-1.jsonl");
        fs::write(
            &rollout,
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"r1\",\"cwd\":\"/work/beta\"}}\n",
        )
        .unwrap();
        assert_eq!(
            origin("codex-r1", "codex", &rollout).project().as_deref(),
            Some("beta")
        );
        assert_eq!(
            origin("gone", "codex", &dir.path().join("missing.jsonl")),
            Origin::default()
        );
    }
}
