//! What a session changed and what it handed off, read from its own calls:
//! lines added and removed per edit call, the branch it worked on, and its
//! sub-tasks (sub-agents and background commands).
//!
//! Everything here reads the call text the index holds (`<tool> <input>`)
//! and the results after it. Nothing runs `git` or reads a repository.

use serde_json::Value;

use crate::index::StoredTurn;

/// Lines added and removed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Lines {
    pub added: u64,
    pub removed: u64,
}

impl Lines {
    pub fn add(&mut self, other: Lines) {
        self.added += other.added;
        self.removed += other.removed;
    }
}

/// One file an edit call named, with its lines when they are known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileLines {
    pub path: String,
    /// `None`: the call changed the file by an amount its input does not
    /// say (a `Write` that replaced a file, a deleted file).
    pub lines: Option<Lines>,
}

/// The most cells the line diff fills before it gives up and counts every
/// old line removed and every new line added.
const DIFF_CELLS: usize = 4_000_000;

/// The lines of `text` as a diff counts them: none for an empty text.
fn lines_of(text: &str) -> Vec<&str> {
    if text.is_empty() {
        Vec::new()
    } else {
        text.split('\n').collect()
    }
}

/// Lines added and removed from `old` to `new`: a longest common
/// subsequence of their lines after the common head and tail are cut.
pub fn line_diff(old: &str, new: &str) -> Lines {
    let a = lines_of(old);
    let b = lines_of(new);
    let head = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let (a, b) = (&a[head..], &b[head..]);
    let tail = a
        .iter()
        .rev()
        .zip(b.iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (a, b) = (&a[..a.len() - tail], &b[..b.len() - tail]);
    if a.is_empty() || b.is_empty() || a.len().saturating_mul(b.len()) > DIFF_CELLS {
        return Lines {
            added: b.len() as u64,
            removed: a.len() as u64,
        };
    }
    // One row of the LCS table at a time.
    let mut prev = vec![0u32; b.len() + 1];
    let mut row = vec![0u32; b.len() + 1];
    for x in a {
        for (j, y) in b.iter().enumerate() {
            row[j + 1] = if x == y {
                prev[j] + 1
            } else {
                prev[j + 1].max(row[j])
            };
        }
        std::mem::swap(&mut prev, &mut row);
    }
    let common = u64::from(prev[b.len()]);
    Lines {
        added: b.len() as u64 - common,
        removed: a.len() as u64 - common,
    }
}

/// The lines of a Claude Code `toolUseResult.structuredPatch`: its hunks'
/// `+` and `-` lines. `None` when `v` is not a list of hunks.
pub fn of_structured_patch(v: &Value) -> Option<Lines> {
    let hunks = v.as_array()?;
    let mut out = Lines::default();
    for h in hunks {
        for l in h.get("lines")?.as_array()? {
            match l.as_str().and_then(|s| s.chars().next()) {
                Some('+') => out.added += 1,
                Some('-') => out.removed += 1,
                _ => {}
            }
        }
    }
    Some(out)
}

/// The files a Codex patch (`*** Begin Patch` … `*** End Patch`) changes,
/// with their `+` and `-` lines; a deleted file is uncounted.
pub fn patch_lines(patch: &str) -> Vec<FileLines> {
    let mut out: Vec<FileLines> = Vec::new();
    for line in patch.lines() {
        if let Some(rest) = line.strip_prefix("*** ") {
            let file = |p: &str| p.trim().to_owned();
            if let Some(p) = rest.strip_prefix("Add File: ") {
                out.push(FileLines {
                    path: file(p),
                    lines: Some(Lines::default()),
                });
            } else if let Some(p) = rest.strip_prefix("Update File: ") {
                out.push(FileLines {
                    path: file(p),
                    lines: Some(Lines::default()),
                });
            } else if let Some(p) = rest.strip_prefix("Delete File: ") {
                out.push(FileLines {
                    path: file(p),
                    lines: None,
                });
            }
            continue;
        }
        let Some(Some(lines)) = out.last_mut().map(|f| f.lines.as_mut()) else {
            continue;
        };
        if line.starts_with('+') {
            lines.added += 1;
        } else if line.starts_with('-') {
            lines.removed += 1;
        }
    }
    out
}

/// The patch text of a Codex `apply_patch` call's input: a raw string (a
/// custom call), a JSON object's `input` or `patch`, or a JSON string.
fn patch_text(input: &str) -> String {
    match serde_json::from_str::<Value>(input) {
        Ok(Value::String(s)) => s,
        Ok(v) => ["input", "patch"]
            .iter()
            .find_map(|k| v.get(*k).and_then(Value::as_str))
            .unwrap_or_default()
            .to_owned(),
        Err(_) if !input.contains('\n') && input.contains("\\n") => input.replace("\\n", "\n"),
        Err(_) => input.to_owned(),
    }
}

/// The files an edit call changed and by how many lines, from its input
/// and, for `Write`, its result. `name` is the tool, `input` the call's text
/// after the tool's name.
pub fn of_call(name: &str, input: &str, result: Option<&str>) -> Vec<FileLines> {
    if name == "apply_patch" {
        return patch_lines(&patch_text(input));
    }
    let Ok(v) = serde_json::from_str::<Value>(input) else {
        return Vec::new();
    };
    let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
    let Some(path) = s(&v, "file_path").or_else(|| s(&v, "notebook_path")) else {
        return Vec::new();
    };
    let lines = match name {
        "Edit" => Some(line_diff(
            &s(&v, "old_string").unwrap_or_default(),
            &s(&v, "new_string").unwrap_or_default(),
        )),
        "MultiEdit" => {
            let mut sum = Lines::default();
            for e in v
                .get("edits")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                sum.add(line_diff(
                    &s(e, "old_string").unwrap_or_default(),
                    &s(e, "new_string").unwrap_or_default(),
                ));
            }
            Some(sum)
        }
        "Write" => {
            let created = result.is_some_and(|r| {
                let r = r.to_lowercase();
                r.contains("created") && !r.contains("updated")
            });
            created.then(|| Lines {
                added: s(&v, "content").unwrap_or_default().lines().count() as u64,
                removed: 0,
            })
        }
        _ => None,
    };
    vec![FileLines { path, lines }]
}

/// Whether a shell command carries a Codex patch.
pub fn has_patch(command: &str) -> bool {
    command.contains("*** Begin Patch")
}

/// The longest branch name kept.
const BRANCH_CHARS: usize = 120;

fn branch_name(b: &str) -> Option<String> {
    let b = b.trim().trim_matches(|c| c == '"' || c == '\'');
    let ok = !b.is_empty()
        && b != "HEAD"
        && !b.starts_with('-')
        && !b.contains(char::is_whitespace)
        && b.chars().count() <= BRANCH_CHARS;
    ok.then(|| b.to_owned())
}

/// The value after one of `names` in `words` (`-b x` or `--head=x`).
fn value_of(words: &[String], names: &[&str]) -> Option<String> {
    let mut it = words.iter();
    while let Some(w) = it.next() {
        if names.contains(&w.as_str()) {
            return it.next().cloned();
        }
        for n in names.iter().filter(|n| n.starts_with("--")) {
            if let Some(v) = w.strip_prefix(&format!("{n}=")) {
                return Some(v.to_owned());
            }
        }
    }
    None
}

/// The branch a `git` command (its words, `git` first) creates, switches to
/// with `-c`, or pushes with an upstream or an explicit refspec.
pub fn branch_of_git(words: &[String]) -> Option<String> {
    // Skip `git`'s own options and their values.
    let mut i = 1;
    while i < words.len() && words[i].starts_with('-') {
        i += if matches!(words[i].as_str(), "-C" | "-c") {
            2
        } else {
            1
        };
    }
    let sub = words.get(i)?.as_str();
    let rest = &words[i + 1..];
    match sub {
        "checkout" => value_of(rest, &["-b", "-B"]).and_then(|b| branch_name(&b)),
        "switch" => value_of(rest, &["-c", "-C", "--create", "--force-create"])
            .and_then(|b| branch_name(&b)),
        "worktree" if rest.first().map(String::as_str) == Some("add") => {
            value_of(&rest[1..], &["-b", "-B"]).and_then(|b| branch_name(&b))
        }
        "push" => {
            if rest
                .iter()
                .any(|w| matches!(w.as_str(), "--delete" | "-d" | "--tags" | "--all"))
            {
                return None;
            }
            let positionals: Vec<&String> = rest
                .iter()
                .filter(|w| !w.starts_with('-'))
                .filter(|w| !w.starts_with('+') || w.len() > 1)
                .collect();
            // `<remote> <refspec>`: the branch is the refspec's destination.
            let refspec = positionals.get(1)?;
            let dst = refspec.rsplit(':').next().unwrap_or(refspec);
            let dst = dst
                .trim_start_matches('+')
                .trim_start_matches("refs/heads/");
            branch_name(dst)
        }
        _ => None,
    }
}

/// The branch a `gh` command (its words, `gh` first) names: `gh pr create
/// --head <b>`.
pub fn branch_of_gh(words: &[String]) -> Option<String> {
    let pos: Vec<&str> = words
        .iter()
        .skip(1)
        .filter(|w| !w.starts_with('-'))
        .map(String::as_str)
        .collect();
    if pos.starts_with(&["pr", "create"]) {
        return value_of(&words[1..], &["--head", "-H"]).and_then(|b| branch_name(&b));
    }
    None
}

/// What kind of sub-task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskKind {
    /// A sub-agent: `Agent`, `Task`, `spawn_agent`.
    Agent,
    /// A background shell command.
    Bash,
    /// A T3 Code delegated task (`delegate_task`): a child thread with its
    /// own transcript.
    Delegate,
}

impl TaskKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TaskKind::Agent => "agent",
            TaskKind::Bash => "bash",
            TaskKind::Delegate => "delegate",
        }
    }

    pub fn parse(s: &str) -> Option<TaskKind> {
        match s {
            "agent" => Some(TaskKind::Agent),
            "bash" => Some(TaskKind::Bash),
            "delegate" => Some(TaskKind::Delegate),
            _ => None,
        }
    }
}

/// One sub-task a session started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    pub kind: TaskKind,
    pub title: String,
    /// RFC 3339, the call's time.
    pub started: String,
    /// When it ended, if the transcript says.
    pub ended: Option<String>,
    /// `completed`, `failed`, `killed` or `stopped`, when it ended.
    pub status: Option<String>,
    /// The id its result gave it: a sub-agent's `agentId`, a background
    /// command's id, a delegated task's `taskId`.
    pub id: Option<String>,
    /// A delegated task's child thread (`childThreadId`).
    pub thread: Option<String>,
    /// What it was given: a sub-agent's prompt, the command, the delegated
    /// task; at most [`INPUT_CHARS`].
    pub input: Option<String>,
    /// What it gave back to the session: a sub-agent's answer or its
    /// notification's result, a delegated task's completion; at most
    /// [`REPORT_CHARS`], without the client's trailer.
    pub report: Option<String>,
    /// Tokens and tool calls its result's `<usage>` counts.
    pub tokens: Option<u64>,
    pub tools: Option<u64>,
    /// A background command's exit code, from its notification.
    pub exit: Option<i64>,
    /// The session of its own transcript, when the rollup knows it: set by
    /// the rollup, not by [`tasks`].
    pub child: Option<String>,
}

/// The longest task title kept.
pub const TITLE_CHARS: usize = 80;
/// The longest input and report kept, in characters.
pub const INPUT_CHARS: usize = 2000;
pub const REPORT_CHARS: usize = 2000;

fn cut(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(n - 1).collect();
    out.push('…');
    out
}

/// A tag's text in a `<task-notification>`.
fn tag<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = text.find(&open)? + open.len();
    let end = text[start..].find(&close)? + start;
    Some(text[start..end].trim())
}

/// A task notification: its task id, kind, the title its summary quotes,
/// and its status.
struct Notice {
    id: Option<String>,
    kind: TaskKind,
    quoted: Option<String>,
    status: String,
    /// A sub-agent's `<result>`.
    result: Option<String>,
    usage: Usage,
    /// A command's exit code, from its summary.
    exit: Option<i64>,
}

/// What a `<usage>` block counts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Usage {
    tokens: Option<u64>,
    tools: Option<u64>,
}

/// The `<usage>` block of a sub-agent's result: `subagent_tokens` (or
/// `total_tokens`) and `tool_uses`.
fn usage_of(text: &str) -> Usage {
    let Some(block) = tag(text, "usage") else {
        return Usage::default();
    };
    let num = |keys: &[&str]| {
        block.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            keys.contains(&k.trim())
                .then(|| v.trim().parse::<u64>().ok())
                .flatten()
        })
    };
    Usage {
        tokens: num(&["subagent_tokens", "total_tokens"]),
        tools: num(&["tool_uses"]),
    }
}

/// A sub-agent's answer without the client's trailer: the `agentId: …`
/// line and the `<usage>` block.
fn report_of(text: &str) -> Option<String> {
    let mut body = text.to_owned();
    if let Some(a) = body.find("<usage>") {
        let end = body[a..].find("</usage>").map_or(body.len(), |b| a + b + 8);
        body.replace_range(a..end, "");
    }
    let kept: Vec<&str> = body
        .lines()
        .filter(|l| !l.trim_start().starts_with("agentId:"))
        .collect();
    let out = kept.join("\n").trim().to_owned();
    (!out.is_empty()).then(|| cut(&out, REPORT_CHARS))
}

/// `exit code 1` in a command's summary.
fn exit_of(summary: &str) -> Option<i64> {
    let at = summary.find("exit code")? + "exit code".len();
    let digits: String = summary[at..]
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '-')
        .collect();
    digits.parse().ok()
}

fn notification(text: &str) -> Option<Notice> {
    let text = text.trim_start();
    if !text.starts_with("<task-notification>") {
        return None;
    }
    let summary = tag(text, "summary").unwrap_or_default();
    let status = tag(text, "status").unwrap_or("completed").to_owned();
    let kind = if summary.starts_with("Agent") {
        TaskKind::Agent
    } else if summary.starts_with("Background command") || summary.contains("shell command") {
        TaskKind::Bash
    } else {
        // `Monitor …` and anything else is not a task.
        return None;
    };
    let quoted = summary
        .find('"')
        .and_then(|a| {
            summary
                .rfind('"')
                .filter(|b| *b > a)
                .map(|b| &summary[a + 1..b])
        })
        .map(|q| normal(&q.replace("\\\"", "\"")));
    Some(Notice {
        id: tag(text, "task-id")
            .map(str::to_owned)
            .filter(|i| !i.is_empty()),
        exit: (kind == TaskKind::Bash).then(|| exit_of(summary)).flatten(),
        result: tag(text, "result").and_then(report_of),
        usage: usage_of(text),
        kind,
        quoted,
        status,
    })
}

/// A title as compared: whitespace runs as one space, trimmed.
fn normal(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A sub-task call's kind, title, whether it asked for the background, and
/// what it was given.
/// Every shell call is a candidate: one counts when it asked for the
/// background or its result says it went there.
fn task_call(name: &str, input: &str) -> Option<(TaskKind, String, bool, Option<String>)> {
    let v: Value = serde_json::from_str(input).ok()?;
    let s = |k: &str| {
        v.get(k)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    let first_line = |t: &str| {
        t.lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim()
            .to_owned()
    };
    let background = v.get("run_in_background").and_then(Value::as_bool) == Some(true);
    let given = |k: &str| s(k).map(|t| cut(t, INPUT_CHARS));
    match name {
        "Agent" | "Task" | "spawn_agent" => {
            let title = s("description")
                .map(str::to_owned)
                .or_else(|| s("prompt").or_else(|| s("message")).map(first_line))?;
            let input = given("prompt").or_else(|| given("message"));
            Some((TaskKind::Agent, title, background, input))
        }
        "Bash" => {
            let title = s("description")
                .map(str::to_owned)
                .or_else(|| s("command").map(first_line))?;
            Some((TaskKind::Bash, title, background, given("command")))
        }
        _ if is_delegate(name) => {
            let title = s("title")
                .map(str::to_owned)
                .or_else(|| s("task").map(first_line))?;
            Some((TaskKind::Delegate, title, true, given("task")))
        }
        _ => None,
    }
}

/// The id a result gives its task: `agentId: <id>` (a sub-agent, in the
/// foreground or not) or `running in background with ID: <id>`.
fn result_id(result: &str) -> Option<String> {
    let after = |key: &str| {
        let at = result.find(key)? + key.len();
        let id: String = result[at..]
            .trim_start()
            .chars()
            .take_while(char::is_ascii_alphanumeric)
            .collect();
        (!id.is_empty()).then_some(id)
    };
    after("agentId:").or_else(|| after("with ID:"))
}

/// A result that only says a background task started.
fn launched(result: &str) -> bool {
    let r = result.trim_start();
    r.starts_with("Async agent launched") || r.starts_with("Command running in background")
}

/// T3 Code's `delegate_task` tool, as Claude Code names an MCP tool.
fn is_delegate(name: &str) -> bool {
    name == "mcp__t3-code__delegate_task"
}

/// A `delegate_task` result that started a child thread: its `taskId` and
/// `childThreadId`. Any other result (a hook that refused the call, an
/// error) started nothing.
fn delegated(result: &str) -> Option<(Option<String>, String)> {
    let v: Value = serde_json::from_str(result.trim()).ok()?;
    let thread = v.get("childThreadId")?.as_str()?.to_owned();
    let id = v.get("taskId").and_then(Value::as_str).map(str::to_owned);
    Some((id, thread))
}

/// T3 Code's message when a delegated task ends: `Delegated task <taskId>
/// (<title>) [<status>]:` and its report on the lines after.
fn delegate_end(text: &str) -> Option<(&str, String, Option<String>)> {
    let rest = text.trim_start().strip_prefix("Delegated task ")?;
    let (head, report) = rest.split_once('\n').unwrap_or((rest, ""));
    let id = head.split_whitespace().next()?;
    let open = head.rfind('[')?;
    let close = head[open..].find(']')? + open;
    let status = match head[open + 1..close].trim() {
        "completed" | "succeeded" | "done" => "completed",
        "failed" | "error" => "failed",
        _ => "stopped",
    };
    let report = report.trim();
    Some((
        id,
        status.to_owned(),
        (!report.is_empty()).then(|| cut(report, REPORT_CHARS)),
    ))
}

struct Entry {
    task: Task,
    /// The full title, normalised, for matching.
    key: String,
    /// Runs in the background: ends with a notification.
    detached: bool,
    /// The id its launch result gave it.
    id: Option<String>,
    /// Its call's turn.
    call: usize,
}

/// The sub-tasks of a session, from its turns and its `(call, result)`
/// pairs (indexes into `turns`, in call order). `describe` gives a Claude
/// Code sub-agent's description by its id, so that the results of agents
/// run side by side, which come back in the order they finish, end the
/// right task.
pub fn tasks(
    turns: &[StoredTurn],
    pairs: &[(usize, Option<usize>)],
    describe: &dyn Fn(&str) -> Option<String>,
) -> Vec<Task> {
    let mut out: Vec<Entry> = Vec::new();
    // (entry, result turn) for results to place.
    let mut answers: Vec<(usize, usize)> = Vec::new();
    for &(call, result) in pairs {
        let t = &turns[call];
        let name = t.tool_name.as_deref().unwrap_or_default();
        let input = t.text.get(name.len() + 1..).unwrap_or_default();
        let Some((kind, title, background, given)) = task_call(name, input) else {
            continue;
        };
        let went = result.is_some_and(|r| launched(&turns[r].text));
        if kind == TaskKind::Bash && !background && !went {
            continue;
        }
        let mut task = Task {
            kind,
            title: cut(&title, TITLE_CHARS),
            started: t.ts.clone(),
            ended: None,
            status: None,
            id: None,
            thread: None,
            input: given,
            report: None,
            tokens: None,
            tools: None,
            exit: None,
            child: None,
        };
        if kind == TaskKind::Delegate {
            // A delegated task exists once its result names the child
            // thread; a refused or failed call started nothing.
            let Some((id, thread)) = result.and_then(|r| delegated(&turns[r].text)) else {
                continue;
            };
            task.id = id.clone();
            task.thread = Some(thread);
            out.push(Entry {
                task,
                key: normal(&title),
                detached: true,
                id,
                call,
            });
            continue;
        }
        out.push(Entry {
            task,
            key: normal(&title),
            detached: background || went,
            id: None,
            call,
        });
        if let Some(r) = result {
            answers.push((out.len() - 1, r));
        }
    }
    // Results: a launch gives its task an id, an answer ends a foreground
    // sub-agent. A result naming a sub-agent whose description is known
    // goes to the open task of that title.
    answers.sort_by_key(|(_, r)| *r);
    for (own, r) in answers {
        let text = &turns[r].text;
        let id = result_id(text);
        let kind = out[own].task.kind;
        let by_title = id
            .as_deref()
            .filter(|_| kind == TaskKind::Agent)
            .and_then(describe)
            .map(|d| normal(&d));
        let free = |e: &Entry| {
            e.task.kind == kind && e.call < r && e.task.ended.is_none() && e.id.is_none()
        };
        let target = by_title
            .and_then(|d| out.iter().position(|e| free(e) && e.key == d))
            .or_else(|| free(&out[own]).then_some(own));
        let Some(k) = target else { continue };
        if launched(text) {
            out[k].detached = true;
            out[k].id = id;
        } else if !out[k].detached {
            let usage = usage_of(text);
            let e = &mut out[k];
            e.task.ended = Some(turns[r].ts.clone());
            e.task.status = Some("completed".to_owned());
            if kind == TaskKind::Agent {
                e.task.report = report_of(text);
                e.task.tokens = usage.tokens;
                e.task.tools = usage.tools;
            }
            e.id = id;
        }
    }
    // Notifications end the background ones, in turn order: by the task
    // id, else by the title the summary quotes, else the one open task of
    // that kind when the summary quotes none.
    for (i, t) in turns.iter().enumerate() {
        if t.role != "user" {
            continue;
        }
        if let Some((id, status, report)) = delegate_end(&t.text) {
            let k = (0..out.len()).find(|&k| {
                let e = &out[k];
                e.call < i
                    && e.task.kind == TaskKind::Delegate
                    && e.task.ended.is_none()
                    && e.id.as_deref() == Some(id)
            });
            if let Some(k) = k {
                out[k].task.ended = Some(t.ts.clone());
                out[k].task.status = Some(status);
                out[k].task.report = report;
            }
            continue;
        }
        let Some(n) = notification(&t.text) else {
            continue;
        };
        let open: Vec<usize> = (0..out.len())
            .filter(|&k| {
                let e = &out[k];
                e.call < i && e.detached && e.task.ended.is_none() && e.task.kind == n.kind
            })
            .collect();
        let by_id = n.id.as_deref().and_then(|id| {
            open.iter()
                .copied()
                .find(|&k| out[k].id.as_deref() == Some(id))
        });
        let pick = by_id.or_else(|| match &n.quoted {
            Some(q) => open.iter().copied().find(|&k| out[k].key == *q),
            None => (open.len() == 1).then(|| open[0]),
        });
        if let Some(k) = pick {
            let e = &mut out[k];
            e.task.ended = Some(t.ts.clone());
            e.task.status = Some(n.status);
            e.task.exit = n.exit;
            if n.result.is_some() {
                e.task.report = n.result;
            }
            e.task.tokens = n.usage.tokens.or(e.task.tokens);
            e.task.tools = n.usage.tools.or(e.task.tools);
        }
    }
    out.into_iter()
        .map(|e| Task { id: e.id, ..e.task })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_owned).collect()
    }

    fn st(role: &str, ts: &str, tool: Option<&str>, text: &str) -> StoredTurn {
        StoredTurn {
            ordinal: 0,
            role: role.to_owned(),
            ts: ts.to_owned(),
            tool_name: tool.map(str::to_owned),
            text: text.to_owned(),
        }
    }

    #[test]
    fn an_edit_counts_only_the_lines_that_changed() {
        let old = "fn a() {\n    one();\n    two();\n}\n";
        let new = "fn a() {\n    one();\n    three();\n    four();\n}\n";
        assert_eq!(
            line_diff(old, new),
            Lines {
                added: 2,
                removed: 1
            }
        );
        assert_eq!(line_diff("same", "same"), Lines::default());
        assert_eq!(
            line_diff("", "a\nb"),
            Lines {
                added: 2,
                removed: 0
            }
        );
        // Lines that move count as a removal and an addition.
        assert_eq!(
            line_diff("a\nb\nc", "c\na\nb"),
            Lines {
                added: 1,
                removed: 1
            }
        );
    }

    #[test]
    fn edit_and_multi_edit_calls_count_from_their_input() {
        let edit = r#"{"file_path":"/r/src/a.rs","old_string":"x\ny","new_string":"x\nz\nw","replace_all":false}"#;
        assert_eq!(
            of_call(
                "Edit",
                edit,
                Some("The file /r/src/a.rs has been updated successfully.")
            ),
            vec![FileLines {
                path: "/r/src/a.rs".into(),
                lines: Some(Lines {
                    added: 2,
                    removed: 1
                })
            }]
        );
        let multi = r#"{"file_path":"/r/b.rs","edits":[{"old_string":"a","new_string":"b"},{"old_string":"c\nd","new_string":"c"}]}"#;
        assert_eq!(
            of_call("MultiEdit", multi, None)[0].lines,
            Some(Lines {
                added: 1,
                removed: 2
            })
        );
    }

    #[test]
    fn a_write_counts_when_it_created_the_file_and_not_when_it_replaced_one() {
        let input = r#"{"file_path":"/r/new.md","content":"one\ntwo\nthree\n"}"#;
        let created = of_call(
            "Write",
            input,
            Some("File created successfully at: /r/new.md"),
        );
        assert_eq!(
            created[0].lines,
            Some(Lines {
                added: 3,
                removed: 0
            })
        );
        let replaced = of_call(
            "Write",
            input,
            Some("The file /r/new.md has been updated successfully."),
        );
        assert_eq!(replaced[0].lines, None);
        assert_eq!(of_call("Write", input, None)[0].lines, None);
        assert_eq!(
            of_call(
                "NotebookEdit",
                r#"{"notebook_path":"/r/n.ipynb","new_source":"x"}"#,
                None
            )[0]
            .lines,
            None
        );
    }

    #[test]
    fn a_codex_patch_counts_its_plus_and_minus_lines_per_file() {
        let patch = "*** Begin Patch\n*** Add File: docs/new.md\n+# New\n+body\n*** Update File: src/lib.rs\n@@ fn main\n context\n-old\n+new\n+more\n*** Delete File: gone.txt\n*** End Patch";
        let want = vec![
            FileLines {
                path: "docs/new.md".into(),
                lines: Some(Lines {
                    added: 2,
                    removed: 0,
                }),
            },
            FileLines {
                path: "src/lib.rs".into(),
                lines: Some(Lines {
                    added: 2,
                    removed: 1,
                }),
            },
            FileLines {
                path: "gone.txt".into(),
                lines: None,
            },
        ];
        // A custom call's raw input, a function call's JSON, an escaped string.
        assert_eq!(of_call("apply_patch", patch, None), want);
        let json = serde_json::json!({ "input": patch }).to_string();
        assert_eq!(of_call("apply_patch", &json, None), want);
        assert_eq!(
            of_call("apply_patch", &patch.replace('\n', "\\n"), None),
            want
        );
    }

    #[test]
    fn a_structured_patch_counts_its_hunk_lines() {
        let v = serde_json::json!([{"oldStart":1,"oldLines":3,"newStart":1,"newLines":3,
            "lines":[" a","-b","+c","+d"," e"]}]);
        assert_eq!(
            of_structured_patch(&v),
            Some(Lines {
                added: 2,
                removed: 1
            })
        );
        assert_eq!(of_structured_patch(&serde_json::json!({})), None);
    }

    #[test]
    fn branch_commands_name_the_branch() {
        assert_eq!(
            branch_of_git(&w("git checkout -b feat/x origin/main")).as_deref(),
            Some("feat/x")
        );
        assert_eq!(
            branch_of_git(&w("git -C /r switch -c fix/y")).as_deref(),
            Some("fix/y")
        );
        assert_eq!(
            branch_of_git(&w("git worktree add ../wt -b docs/z main")).as_deref(),
            Some("docs/z")
        );
        assert_eq!(
            branch_of_git(&w("git push -u origin feat/x")).as_deref(),
            Some("feat/x")
        );
        assert_eq!(
            branch_of_git(&w("git push origin HEAD:refs/heads/feat/q")).as_deref(),
            Some("feat/q")
        );
        assert_eq!(branch_of_git(&w("git push -u origin HEAD")), None);
        assert_eq!(branch_of_git(&w("git push origin --delete feat/x")), None);
        assert_eq!(branch_of_git(&w("git push")), None);
        assert_eq!(branch_of_git(&w("git checkout main")), None);
        assert_eq!(branch_of_git(&w("git checkout -- src/a.rs")), None);
        assert_eq!(
            branch_of_gh(&w("gh pr create --title t --head feat/x")).as_deref(),
            Some("feat/x")
        );
        assert_eq!(branch_of_gh(&w("gh pr merge 3 --squash")), None);
    }

    #[test]
    fn sub_tasks_end_with_their_result_or_their_notification() {
        let none = |_: &str| None;
        let turns = vec![
            st("user", "2026-11-08T08:00:00Z", None, "go"),
            // 1: a foreground agent, answered at 08:05.
            st("tool", "2026-11-08T08:01:00Z", Some("Agent"), r#"Agent {"description":"Read the docs","prompt":"…","subagent_type":"Explore"}"#),
            st("tool", "2026-11-08T08:05:00Z", Some("Agent"), "The docs say …\nagentId: f1 (use SendMessage)"),
            st("assistant", "2026-11-08T08:05:10Z", None, "ok"),
            // 4–9: two background agents and a background command.
            st("tool", "2026-11-08T08:06:00Z", Some("Agent"), r#"Agent {"description":"Audit A","prompt":"x","run_in_background":true}"#),
            st("tool", "2026-11-08T08:06:00Z", Some("Agent"), r#"Agent {"description":"Audit B","prompt":"y","run_in_background":true}"#),
            st("tool", "2026-11-08T08:06:01Z", Some("Agent"), "Async agent launched successfully. agentId: a1"),
            st("tool", "2026-11-08T08:06:01Z", Some("Agent"), "Async agent launched successfully. agentId: a2"),
            st("tool", "2026-11-08T08:07:00Z", Some("Bash"), r#"Bash {"command":"cargo test","description":"Run the tests","run_in_background":true}"#),
            st("tool", "2026-11-08T08:07:00Z", Some("Bash"), "Command running in background with ID: b1"),
            st("assistant", "2026-11-08T08:07:05Z", None, "waiting"),
            // B finishes first, by its task id; a monitor's notification is
            // not a task; the command is killed, named by its title.
            st("user", "2026-11-08T08:20:00Z", None, "<task-notification>\n<task-id>a2</task-id>\n<status>completed</status>\n<summary>Agent \"Audit B\" finished</summary>\n</task-notification>"),
            st("user", "2026-11-08T08:21:00Z", None, "<task-notification>\n<status>completed</status>\n<summary>Monitor \"watch\" stream ended</summary>\n</task-notification>"),
            st("user", "2026-11-08T08:22:00Z", None, "<task-notification>\n<status>killed</status>\n<summary>Background command \"Run the tests\" was stopped</summary>\n</task-notification>"),
            // A title no open task has ends nothing; A's own id ends A.
            st("user", "2026-11-08T08:25:00Z", None, "<task-notification>\n<status>completed</status>\n<summary>Agent \"Someone else\" finished</summary>\n</task-notification>"),
            st("user", "2026-11-08T08:30:00Z", None, "<task-notification>\n<task-id>a1</task-id>\n<status>failed</status>\n<summary>Agent \"Audit A\" failed</summary>\n</task-notification>"),
        ];
        let pairs = vec![(1, Some(2)), (4, Some(6)), (5, Some(7)), (8, Some(9))];
        let got = tasks(&turns, &pairs, &none);
        let view: Vec<(&str, &str, Option<&str>, Option<&str>)> = got
            .iter()
            .map(|t| {
                (
                    t.kind.as_str(),
                    t.title.as_str(),
                    t.ended.as_deref(),
                    t.status.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            view,
            vec![
                (
                    "agent",
                    "Read the docs",
                    Some("2026-11-08T08:05:00Z"),
                    Some("completed")
                ),
                (
                    "agent",
                    "Audit A",
                    Some("2026-11-08T08:30:00Z"),
                    Some("failed")
                ),
                (
                    "agent",
                    "Audit B",
                    Some("2026-11-08T08:20:00Z"),
                    Some("completed")
                ),
                (
                    "bash",
                    "Run the tests",
                    Some("2026-11-08T08:22:00Z"),
                    Some("killed")
                ),
            ]
        );
        // Still running: no end.
        let open = tasks(&turns[..8], &pairs[..3], &none);
        assert_eq!(open[1].ended, None);
        assert_eq!(open[1].status, None);
    }

    #[test]
    fn agents_run_side_by_side_end_by_their_description() {
        let turns = vec![
            st(
                "tool",
                "2026-11-08T08:01:00Z",
                Some("Agent"),
                r#"Agent {"description":"Review X","prompt":"x"}"#,
            ),
            st(
                "tool",
                "2026-11-08T08:01:00Z",
                Some("Agent"),
                r#"Agent {"description":"Review Y","prompt":"y"}"#,
            ),
            // Y finishes first.
            st(
                "tool",
                "2026-11-08T08:03:00Z",
                Some("Agent"),
                "Y report\nagentId: ay (use SendMessage)",
            ),
            st(
                "tool",
                "2026-11-08T08:09:00Z",
                Some("Agent"),
                "X report\nagentId: ax (use SendMessage)",
            ),
        ];
        let describe = |id: &str| match id {
            "ax" => Some("Review X".to_owned()),
            "ay" => Some("Review Y".to_owned()),
            _ => None,
        };
        let got = tasks(&turns, &[(0, Some(2)), (1, Some(3))], &describe);
        assert_eq!(got[0].ended.as_deref(), Some("2026-11-08T08:09:00Z"));
        assert_eq!(got[1].ended.as_deref(), Some("2026-11-08T08:03:00Z"));
        // Without descriptions, results go to their own calls.
        let got = tasks(&turns, &[(0, Some(2)), (1, Some(3))], &|_| None);
        assert_eq!(got[0].ended.as_deref(), Some("2026-11-08T08:03:00Z"));
    }

    #[test]
    fn a_shell_call_sent_to_the_background_is_a_task_and_an_unquoted_notice_needs_one_open() {
        let turns = vec![
            st("tool", "2026-11-08T08:01:00Z", Some("Bash"), r#"Bash {"command":"make","description":"Build it"}"#),
            st("tool", "2026-11-08T08:03:00Z", Some("Bash"), "Command running in background with ID: b9"),
            st("tool", "2026-11-08T08:04:00Z", Some("Bash"), r#"Bash {"command":"ls","description":"List"}"#),
            st("tool", "2026-11-08T08:04:01Z", Some("Bash"), "a\nb"),
            st("user", "2026-11-08T09:00:00Z", None, "<task-notification>\n<status>stopped</status>\n<summary>Background shell command didn't finish before the previous session ended</summary>\n</task-notification>"),
        ];
        let got = tasks(&turns, &[(0, Some(1)), (2, Some(3))], &|_| None);
        assert_eq!(got.len(), 1, "a shell call that returned is no task");
        assert_eq!(got[0].title, "Build it");
        assert_eq!(got[0].status.as_deref(), Some("stopped"));
    }

    #[test]
    fn a_sub_task_keeps_what_it_was_given_its_report_usage_and_exit() {
        let turns = vec![
            st("tool", "2026-11-08T08:01:00Z", Some("Agent"), r#"Agent {"description":"Read the docs","prompt":"Read the docs and list the flags."}"#),
            st("tool", "2026-11-08T08:05:00Z", Some("Agent"), "The flags are -a and -b.\nagentId: f1 (use SendMessage with to: 'f1')\n<usage>subagent_tokens: 1200\ntool_uses: 7\nduration_ms: 4000</usage>"),
            st("tool", "2026-11-08T08:06:00Z", Some("Agent"), r#"Agent {"description":"Audit A","prompt":"Audit it.","run_in_background":true}"#),
            st("tool", "2026-11-08T08:06:01Z", Some("Agent"), "Async agent launched successfully. agentId: a1"),
            st("tool", "2026-11-08T08:07:00Z", Some("Bash"), r#"Bash {"command":"cargo test --all","description":"Run the tests","run_in_background":true}"#),
            st("tool", "2026-11-08T08:07:00Z", Some("Bash"), "Command running in background with ID: b1"),
            st("user", "2026-11-08T08:20:00Z", None, "<task-notification>\n<task-id>a1</task-id>\n<status>completed</status>\n<summary>Agent \"Audit A\" finished</summary>\n<result># Audit\nTwo faults.</result>\n<usage>subagent_tokens: 900\ntool_uses: 3</usage>\n</task-notification>"),
            st("user", "2026-11-08T08:22:00Z", None, "<task-notification>\n<task-id>b1</task-id>\n<status>failed</status>\n<summary>Background command \"Run the tests\" failed with exit code 101</summary>\n</task-notification>"),
        ];
        let got = tasks(&turns, &[(0, Some(1)), (2, Some(3)), (4, Some(5))], &|_| {
            None
        });
        assert_eq!(
            got[0].input.as_deref(),
            Some("Read the docs and list the flags.")
        );
        assert_eq!(got[0].report.as_deref(), Some("The flags are -a and -b."));
        assert_eq!((got[0].tokens, got[0].tools), (Some(1200), Some(7)));
        assert_eq!(got[0].id.as_deref(), Some("f1"));
        assert_eq!(got[1].report.as_deref(), Some("# Audit\nTwo faults."));
        assert_eq!(
            (got[1].tokens, got[1].id.as_deref()),
            (Some(900), Some("a1"))
        );
        assert_eq!(got[2].input.as_deref(), Some("cargo test --all"));
        assert_eq!(
            (got[2].exit, got[2].status.as_deref()),
            (Some(101), Some("failed"))
        );
        assert_eq!(got[2].report, None);
    }

    #[test]
    fn a_delegated_task_starts_with_its_child_thread_and_ends_with_its_report() {
        let call = |title: &str| {
            format!(
                r#"mcp__t3-code__delegate_task {{"title":"{title}","role":"implementation","mode":"async","task":"Build the parser.\nThen test it."}}"#
            )
        };
        let turns = vec![
            st(
                "tool",
                "2026-11-08T08:01:00Z",
                Some("mcp__t3-code__delegate_task"),
                &call("Lane A"),
            ),
            // A hook refused this one: it started nothing.
            st(
                "tool",
                "2026-11-08T08:01:01Z",
                Some("mcp__t3-code__delegate_task"),
                "PreToolUse hook error: blocked",
            ),
            st(
                "tool",
                "2026-11-08T08:02:00Z",
                Some("mcp__t3-code__delegate_task"),
                &call("Lane B"),
            ),
            st(
                "tool",
                "2026-11-08T08:02:01Z",
                Some("mcp__t3-code__delegate_task"),
                r#"{"taskId":"node:t-b","childThreadId":"thread:child-b","status":"running"}"#,
            ),
            st(
                "user",
                "2026-11-08T09:00:00Z",
                None,
                "Delegated task node:t-b (Lane B) [completed]:\nLane B is built.\nTests pass.",
            ),
        ];
        let got = tasks(&turns, &[(0, Some(1)), (2, Some(3))], &|_| None);
        assert_eq!(got.len(), 1, "a refused call is no task");
        let t = &got[0];
        assert_eq!((t.kind, t.title.as_str()), (TaskKind::Delegate, "Lane B"));
        assert_eq!(t.thread.as_deref(), Some("thread:child-b"));
        assert_eq!(t.input.as_deref(), Some("Build the parser.\nThen test it."));
        assert_eq!(t.ended.as_deref(), Some("2026-11-08T09:00:00Z"));
        assert_eq!(t.status.as_deref(), Some("completed"));
        assert_eq!(t.report.as_deref(), Some("Lane B is built.\nTests pass."));
        // Still running: no end.
        let open = tasks(&turns[..4], &[(0, Some(1)), (2, Some(3))], &|_| None);
        assert_eq!(
            (open[0].ended.as_deref(), open[0].status.as_deref()),
            (None, None)
        );
    }

    #[test]
    fn a_task_without_a_description_takes_its_first_line() {
        let turns = vec![st(
            "tool",
            "2026-11-08T08:01:00Z",
            Some("spawn_agent"),
            r#"spawn_agent {"message":"\n  Check the build\nand more"}"#,
        )];
        assert_eq!(
            tasks(&turns, &[(0, None)], &|_| None)[0].title,
            "Check the build"
        );
    }
}
