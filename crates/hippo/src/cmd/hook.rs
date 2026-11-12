//! `hippo hook session-start|prompt|session-end`, run by Claude Code hooks
//! with the hook's JSON on stdin (`session_id`, and `prompt` for
//! `UserPromptSubmit`). A hook never fails the agent's session: on any
//! error it prints one line to stderr and exits 0.
//!
//! `hippo hook session-start --plugin` is the Claude Code plugin's hook:
//! silent unless there are notes to give and no
//! workspace hook gives them already, and read-only.
//!
//! With `--agent codex` each is a Codex hook (`$CODEX_HOME/hooks.json`,
//! which runs for every Codex session): it acts only for a session whose
//! `cwd` is in the workspace, records it as `codex-<session_id>`, the id
//! the Codex adapter gives the same session, and prints Codex's JSON hook
//! output.

use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{anyhow, Result};
use hippo_core::config::{self, Config, Workspace};
use hippo_core::context::{self, PartSelection};
use hippo_core::hooks;
use hippo_core::records::Tool;
use hippo_core::sessions;
use serde_json::Value;

use super::hooks::AgentArg;
use super::one_line;

#[derive(clap::Subcommand)]
pub enum Command {
    /// SessionStart: start the session and print the required context
    SessionStart {
        /// As the Claude Code plugin's hook: print the required context
        /// read-only, and nothing at all when there is no notes folder, the
        /// session is outside the workspace, or a workspace hook already
        /// prints it
        #[arg(long, conflicts_with = "agent")]
        plugin: bool,
        /// The agent running the hook: claude-code or codex
        #[arg(long, value_enum, default_value_t)]
        agent: AgentArg,
    },
    /// UserPromptSubmit: on the first prompt, record it as the task and print
    /// the relevant notes and the linter counts
    Prompt {
        /// The agent running the hook: claude-code or codex
        #[arg(long, value_enum, default_value_t)]
        agent: AgentArg,
    },
    /// SessionEnd: end the session (for Codex, with the last assistant
    /// message as a derived handoff)
    SessionEnd {
        /// The agent running the hook: claude-code or codex
        #[arg(long, value_enum, default_value_t)]
        agent: AgentArg,
    },
}

impl Command {
    fn agent(&self) -> AgentArg {
        match self {
            Command::SessionStart { agent, .. }
            | Command::Prompt { agent }
            | Command::SessionEnd { agent } => *agent,
        }
    }
}

pub fn run(command: Command) -> ExitCode {
    if let Command::SessionStart { plugin: true, .. } = command {
        // Silent on every error: a plugin hook runs in every session.
        let _ = plugin_session_start();
        return ExitCode::SUCCESS;
    }
    let result = match command.agent() {
        AgentArg::ClaudeCode => hook(command),
        AgentArg::Codex => codex_hook(command),
    };
    if let Err(err) = result {
        eprintln!("hippo hook: {}", one_line(&format!("{err:#}")));
    }
    ExitCode::SUCCESS
}

fn hook(command: Command) -> Result<()> {
    let mut stdin = String::new();
    io::stdin().read_to_string(&mut stdin)?;
    let input: Value = serde_json::from_str(&stdin)?;
    let id = input
        .get("session_id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("the hook input has no session_id"))?;
    let (_, workspace) = super::load()?;
    match command {
        // Start the session, print parts 1 to 4, and record their size.
        Command::SessionStart { .. } => {
            let session = sessions::start(&workspace, Tool::ClaudeCode, id, None)?;
            let compiled = context::compile(
                &workspace,
                None,
                None,
                None,
                Some(&session),
                workspace.budget_bytes,
                PartSelection::Required,
            )?;
            let text = compiled.text();
            print!("{text}");
            sessions::set_context_start(&workspace, id, compiled.bytes, Some(&text))?;
        }
        // On the first prompt only: parts 5 and 6 within what is left of
        // the budget, then the prompt becomes the task.
        Command::Prompt { .. } => {
            let session =
                sessions::get(&workspace, id)?.ok_or_else(|| anyhow!("no session {id}"))?;
            if session.task_seen {
                return Ok(());
            }
            let prompt = input
                .get("prompt")
                .and_then(Value::as_str)
                .unwrap_or_default();
            // The repository the session works in: the hook's `cwd`, else
            // this process's directory.
            let cwd = input
                .get("cwd")
                .and_then(Value::as_str)
                .map(std::path::PathBuf::from)
                .or_else(|| std::env::current_dir().ok());
            let repo = cwd.and_then(|dir| context::rank::repo_name(&dir, &workspace.root));
            let budget = workspace
                .budget_bytes
                .saturating_sub(session.context_bytes_at_start);
            let compiled = context::compile(
                &workspace,
                None,
                Some(prompt),
                repo.as_deref(),
                None,
                budget,
                PartSelection::Optional,
            )?;
            let text = compiled.text();
            print!("{text}");
            for line in compiled.omitted_lines() {
                eprintln!("{line}");
            }
            sessions::set_task_seen_and_context(&workspace, id, prompt, Some(&text))?;
        }
        Command::SessionEnd { .. } => {
            sessions::end(&workspace, id, &[], None, None)?;
        }
    }
    Ok(())
}

/// A Codex hook. Nothing is printed or written unless hippo has a
/// workspace and the session's `cwd` is in it; a sub-agent's prompt, a
/// later prompt, and a session hippo did not start are passed over.
fn codex_hook(command: Command) -> Result<()> {
    let mut stdin = String::new();
    io::stdin().read_to_string(&mut stdin)?;
    let input: Value = serde_json::from_str(&stdin)?;
    let raw = input
        .get("session_id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("the hook input has no session_id"))?;
    let config = match Config::load() {
        Ok(config) => config,
        Err(hippo_core::error::Error::NotSetUp(_)) => return Ok(()),
        Err(err) => return Err(err.into()),
    };
    if config.workspace.is_none() {
        return Ok(());
    }
    let workspace = Workspace::load(&config)?;
    let Some(cwd) = input.get("cwd").and_then(Value::as_str) else {
        return Ok(());
    };
    let cwd = PathBuf::from(cwd);
    if !canonical(&cwd).starts_with(canonical(&workspace.root)) {
        return Ok(());
    }
    let id = format!("codex-{raw}");
    match command {
        Command::SessionStart { .. } => {
            let session = sessions::start(&workspace, Tool::Codex, &id, None)?;
            let compiled = context::compile(
                &workspace,
                None,
                None,
                None,
                Some(&session),
                workspace.budget_bytes,
                PartSelection::Required,
            )?;
            let text = compiled.text();
            print_codex_context("SessionStart", &text);
            sessions::set_context_start(&workspace, &id, compiled.bytes, Some(&text))?;
        }
        Command::Prompt { .. } => {
            if input.get("agent_id").is_some_and(|v| !v.is_null()) {
                return Ok(());
            }
            let Some(session) = sessions::get(&workspace, &id)? else {
                return Ok(());
            };
            if session.derived || session.task_seen {
                return Ok(());
            }
            let prompt = input
                .get("prompt")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let repo = context::rank::repo_name(&cwd, &workspace.root);
            let budget = workspace
                .budget_bytes
                .saturating_sub(session.context_bytes_at_start);
            let compiled = context::compile(
                &workspace,
                None,
                Some(prompt),
                repo.as_deref(),
                None,
                budget,
                PartSelection::Optional,
            )?;
            let text = compiled.text();
            print_codex_context("UserPromptSubmit", &text);
            for line in compiled.omitted_lines() {
                eprintln!("{line}");
            }
            sessions::set_task_seen_and_context(&workspace, &id, prompt, Some(&text))?;
        }
        Command::SessionEnd { .. } => {
            let Some(session) = sessions::get(&workspace, &id)? else {
                return Ok(());
            };
            if session.derived {
                return Ok(());
            }
            let handoff = input
                .get("transcript_path")
                .and_then(Value::as_str)
                .and_then(|path| last_assistant_text(Path::new(path)));
            sessions::end_with_derived_handoff(&workspace, &id, handoff.as_deref())?;
        }
    }
    Ok(())
}

/// Prints `text` as Codex's hook output for `event`: JSON, so a block
/// that starts with `[` or `{` is not mistaken for broken JSON output.
/// Nothing is printed for an empty text.
fn print_codex_context(event: &str, text: &str) {
    if text.trim().is_empty() {
        return;
    }
    let output = serde_json::json!({
        "hookSpecificOutput": {"hookEventName": event, "additionalContext": text}
    });
    println!("{output}");
}

/// The last assistant message of the Codex transcript at `path`, redacted,
/// as ingest would derive the handoff; `None` when the file cannot be read
/// or has none.
fn last_assistant_text(path: &Path) -> Option<String> {
    use hippo_core::adapters::{codex::Codex, Adapter, Role};
    let parsed = Codex.parse(path.parent().unwrap_or(path), path).ok()?;
    let turn = parsed
        .turns
        .iter()
        .rev()
        .find(|turn| turn.role == Role::Assistant)?;
    Some(hippo_core::redact::redact(&turn.text).as_str().to_owned())
}

/// The plugin's session start: the required parts, read-only, when hippo
/// has a workspace, the session's directory is in it, and no settings file
/// Claude Code reads for it already runs `hippo hook session-start`.
fn plugin_session_start() -> Result<()> {
    let mut stdin = String::new();
    io::stdin().read_to_string(&mut stdin)?;
    let input: Value = serde_json::from_str(&stdin).unwrap_or(Value::Null);
    let config = Config::load()?;
    let workspace = Workspace::load(&config)?;
    let root = canonical(&workspace.root);
    let dir = input
        .get("cwd")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("CLAUDE_PROJECT_DIR").map(PathBuf::from))
        .or_else(|| std::env::current_dir().ok())
        .ok_or_else(|| anyhow!("no session directory"))?;
    let dir = canonical(&dir);
    if !dir.starts_with(&root) {
        return Ok(());
    }
    if already_hooked(&dir, &root) {
        return Ok(());
    }
    let compiled = context::compile(
        &workspace,
        None,
        None,
        None,
        None,
        workspace.budget_bytes,
        PartSelection::Required,
    )?;
    print!("{}", compiled.text());
    Ok(())
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// Whether a settings file Claude Code reads for a session in `dir` already
/// runs `hippo hook session-start`: `.claude/settings.json` and
/// `.claude/settings.local.json` of every folder from `dir` up to the
/// workspace `root`, and the user's `settings.json` (`$CLAUDE_CONFIG_DIR`,
/// else `~/.claude`).
fn already_hooked(dir: &Path, root: &Path) -> bool {
    let mut files = Vec::new();
    for folder in dir.ancestors() {
        files.push(folder.join(".claude/settings.json"));
        files.push(folder.join(".claude/settings.local.json"));
        if folder == root {
            break;
        }
    }
    let user = std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| config::user_home().map(|h| h.join(".claude")));
    files.extend(user.map(|d| d.join("settings.json")));
    files.iter().any(|f| hooks::file_runs_session_start(f))
}
