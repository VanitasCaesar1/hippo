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
//!
//! With `--agent gemini` each is a Gemini CLI hook (`~/.gemini/settings.json`):
//! as for Codex, with `gemini-<session_id>` and `BeforeAgent` for the
//! prompt.
//!
//! With `--agent cursor`, `session-start` is a Cursor `sessionStart` hook
//! (`~/.cursor/hooks.json`): read-only, it prints the required parts as
//! `{"additional_context": …}` when one of the input's `workspace_roots` is
//! in the workspace, and the machine rules otherwise. Cursor has no prompt
//! hook that adds context, and hippo writes no session record for it:
//! ingest reads the session from its transcript.
//!
//! Every start block carries the kept rules (`docs/designs/remember.md`).
//! Outside the workspace the Codex, Gemini CLI and Cursor hooks and the
//! plugin's print the machine rules only, and `hippo hook session-start
//! --rules-only` (the Claude Code user hook, `hippo hooks install --user`)
//! does the same. A later prompt of a long session gets the notes and
//! rules of a new topic when the topic changes. Each output is logged in
//! `$HIPPO_HOME/logs/injections.jsonl`.

use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{anyhow, Result};
use hippo_core::config::{self, Config, Workspace};
use hippo_core::context::topic::{self, Injection, Topic};
use hippo_core::context::{self, Compiled, PartSelection};
use hippo_core::hooks;
use hippo_core::lessons;
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
        /// Print only the kept rules: the machine rules outside the
        /// workspace, and nothing inside it when a workspace hook gives the
        /// whole block (the Claude Code user hook)
        #[arg(long, conflicts_with = "plugin")]
        rules_only: bool,
        /// The agent running the hook: claude-code, codex, cursor or gemini
        #[arg(long, value_enum, default_value_t)]
        agent: AgentArg,
    },
    /// UserPromptSubmit: on the first prompt, record it as the task and print
    /// the relevant notes and the linter counts
    Prompt {
        /// The agent running the hook: claude-code, codex or gemini
        #[arg(long, value_enum, default_value_t)]
        agent: AgentArg,
    },
    /// SessionEnd: end the session (for Codex, with the last assistant
    /// message as a derived handoff)
    SessionEnd {
        /// The agent running the hook: claude-code, codex or gemini
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
    if let Command::SessionStart {
        rules_only: true, ..
    } = command
    {
        if let Err(err) = rules_only_session_start() {
            eprintln!("hippo hook: {}", one_line(&format!("{err:#}")));
        }
        return ExitCode::SUCCESS;
    }
    let result = match command.agent() {
        AgentArg::ClaudeCode => hook(command),
        AgentArg::Codex => codex_hook(command, Hooked::Codex),
        AgentArg::Gemini => codex_hook(command, Hooked::Gemini),
        AgentArg::Cursor => cursor_hook(command),
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
    let (cfg, workspace) = super::load()?;
    match command {
        // Start the session, print parts 1 to 4 and the rules, and record
        // their size.
        Command::SessionStart { .. } => {
            let session = sessions::start(&workspace, Tool::ClaudeCode, id, None)?;
            let mut compiled = context::compile(
                &workspace,
                None,
                None,
                None,
                Some(&session),
                workspace.budget_bytes,
                PartSelection::Required,
            )?;
            let rules = add_rules(&cfg, &workspace, &mut compiled);
            let text = compiled.text();
            print!("{text}");
            sessions::set_context_start(&workspace, id, compiled.bytes, Some(&text))?;
            started(&cfg, id, "claude-code", rules, text.len());
        }
        // On the first prompt only: parts 5 and 6 within what is left of
        // the budget, then the prompt becomes the task.
        Command::Prompt { .. } => {
            let Some(session) = sessions::get(&workspace, id)? else {
                return Ok(());
            };
            let prompt = input
                .get("prompt")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if session.task_seen {
                if let Some(text) = new_topic(&cfg, &workspace, id, "claude-code", prompt)? {
                    print!("{text}");
                }
                return Ok(());
            }
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
            first_prompt(&cfg, id, "claude-code", prompt, &compiled, text.len());
        }
        Command::SessionEnd { .. } => {
            sessions::end(&workspace, id, &[], None, None)?;
        }
    }
    Ok(())
}

/// An agent whose hooks work as Codex's do: JSON on stdin with
/// `session_id`, `cwd`, `prompt` and `transcript_path`, and
/// `hookSpecificOutput.additionalContext` on stdout.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Hooked {
    Codex,
    /// Gemini CLI: the same fields; its prompt event is `BeforeAgent`.
    Gemini,
}

impl Hooked {
    fn tool(self) -> Tool {
        match self {
            Hooked::Codex => Tool::Codex,
            Hooked::Gemini => Tool::Gemini,
        }
    }

    /// The session id hippo and the agent's adapter give the session.
    fn id(self, raw: &str) -> String {
        format!("{}-{raw}", self.tool().as_str())
    }

    fn prompt_event(self) -> &'static str {
        match self {
            Hooked::Codex => "UserPromptSubmit",
            Hooked::Gemini => "BeforeAgent",
        }
    }
}

/// A Codex or Gemini CLI hook. Nothing is printed or written unless hippo
/// has a workspace and the session's `cwd` is in it; a sub-agent's prompt,
/// a later prompt, and a session hippo did not start are passed over.
fn codex_hook(command: Command, agent: Hooked) -> Result<()> {
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
    let id = agent.id(raw);
    let cwd = input.get("cwd").and_then(Value::as_str).map(PathBuf::from);
    let workspace = match &config.workspace {
        Some(_) => Some(Workspace::load(&config)?),
        None => None,
    };
    let inside = match (&workspace, &cwd) {
        (Some(ws), Some(cwd)) => canonical(cwd).starts_with(canonical(&ws.root)),
        _ => false,
    };
    let (Some(workspace), true) = (workspace, inside) else {
        // Outside the workspace: the machine rules at the start, nothing
        // else.
        let top = input.get("agent_id").is_none_or(Value::is_null);
        if matches!(command, Command::SessionStart { .. }) && top {
            let (text, rules) = machine_rules(&config);
            print_codex_context("SessionStart", &text);
            if !text.is_empty() {
                log_injection(
                    &config,
                    &id,
                    agent.tool().as_str(),
                    "rules-only",
                    rules,
                    Vec::new(),
                    text.len(),
                );
            }
        }
        return Ok(());
    };
    let Some(cwd) = cwd else { return Ok(()) };
    match command {
        Command::SessionStart { .. } => {
            let session = sessions::start(&workspace, agent.tool(), &id, None)?;
            let mut compiled = context::compile(
                &workspace,
                None,
                None,
                None,
                Some(&session),
                workspace.budget_bytes,
                PartSelection::Required,
            )?;
            let rules = add_rules(&config, &workspace, &mut compiled);
            let text = compiled.text();
            print_codex_context("SessionStart", &text);
            sessions::set_context_start(&workspace, &id, compiled.bytes, Some(&text))?;
            started(&config, &id, agent.tool().as_str(), rules, text.len());
        }
        Command::Prompt { .. } => {
            if input.get("agent_id").is_some_and(|v| !v.is_null()) {
                return Ok(());
            }
            let Some(session) = sessions::get(&workspace, &id)? else {
                return Ok(());
            };
            if session.derived {
                return Ok(());
            }
            let prompt = input
                .get("prompt")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if session.task_seen {
                if let Some(text) =
                    new_topic(&config, &workspace, &id, agent.tool().as_str(), prompt)?
                {
                    print_codex_context(agent.prompt_event(), &text);
                }
                return Ok(());
            }
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
            print_codex_context(agent.prompt_event(), &text);
            for line in compiled.omitted_lines() {
                eprintln!("{line}");
            }
            sessions::set_task_seen_and_context(&workspace, &id, prompt, Some(&text))?;
            first_prompt(
                &config,
                &id,
                agent.tool().as_str(),
                prompt,
                &compiled,
                text.len(),
            );
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
                .and_then(|path| last_assistant_text(agent, Path::new(path)));
            sessions::end_with_derived_handoff(&workspace, &id, handoff.as_deref())?;
        }
    }
    Ok(())
}

/// A Cursor hook: `session-start` prints the required parts as Cursor's
/// `sessionStart` output when a workspace root of the session is in
/// hippo's workspace; everything else does nothing.
fn cursor_hook(command: Command) -> Result<()> {
    if !matches!(command, Command::SessionStart { .. }) {
        return Ok(());
    }
    let mut stdin = String::new();
    io::stdin().read_to_string(&mut stdin)?;
    let input: Value = serde_json::from_str(&stdin)?;
    let config = match Config::load() {
        Ok(config) => config,
        Err(hippo_core::error::Error::NotSetUp(_)) => return Ok(()),
        Err(err) => return Err(err.into()),
    };
    let session = input
        .get("session_id")
        .or_else(|| input.get("conversation_id"))
        .and_then(Value::as_str)
        .map(|s| format!("cursor-{s}"))
        .unwrap_or_else(|| "cursor".to_owned());
    let workspace = match &config.workspace {
        Some(_) => Some(Workspace::load(&config)?),
        None => None,
    };
    let inside = workspace.as_ref().is_some_and(|ws| {
        let root = canonical(&ws.root);
        input
            .get("workspace_roots")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .any(|dir| canonical(Path::new(dir)).starts_with(&root))
    });
    let (text, rules, event) = match (workspace, inside) {
        (Some(workspace), true) => {
            let mut compiled = context::compile(
                &workspace,
                None,
                None,
                None,
                None,
                workspace.budget_bytes,
                PartSelection::Required,
            )?;
            let rules = add_rules(&config, &workspace, &mut compiled);
            (compiled.text(), rules, "start")
        }
        _ => {
            let (text, rules) = machine_rules(&config);
            (text, rules, "rules-only")
        }
    };
    if !text.trim().is_empty() {
        println!("{}", serde_json::json!({ "additional_context": text }));
        log_injection(
            &config,
            &session,
            "cursor",
            event,
            rules,
            Vec::new(),
            text.len(),
        );
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

/// The last assistant message of the Codex or Gemini CLI transcript at
/// `path`, redacted, as ingest would derive the handoff; `None` when the
/// file cannot be read or has none.
fn last_assistant_text(agent: Hooked, path: &Path) -> Option<String> {
    use hippo_core::adapters::{codex::Codex, gemini::Gemini, Adapter, Role};
    let root = path.parent().unwrap_or(path);
    let parsed = match agent {
        Hooked::Codex => Codex.parse(root, path).ok()?,
        Hooked::Gemini => Gemini.parse(root, path).ok()?,
    };
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
    let session = input
        .get("session_id")
        .and_then(Value::as_str)
        .unwrap_or("plugin")
        .to_owned();
    let dir = input
        .get("cwd")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("CLAUDE_PROJECT_DIR").map(PathBuf::from))
        .or_else(|| std::env::current_dir().ok())
        .ok_or_else(|| anyhow!("no session directory"))?;
    let dir = canonical(&dir);
    let workspace = match &config.workspace {
        Some(_) => Some(Workspace::load(&config)?),
        None => None,
    };
    let Some(workspace) = workspace.filter(|ws| dir.starts_with(canonical(&ws.root))) else {
        // Outside the workspace: the machine rules, unless the user hook
        // gives them.
        if user_settings().is_some_and(|f| hooks::file_runs_rules_only(&f)) {
            return Ok(());
        }
        let (text, rules) = machine_rules(&config);
        print!("{text}");
        if !text.is_empty() {
            log_injection(
                &config,
                &session,
                "plugin",
                "rules-only",
                rules,
                Vec::new(),
                text.len(),
            );
        }
        return Ok(());
    };
    let root = canonical(&workspace.root);
    if already_hooked(&dir, &root) {
        return Ok(());
    }
    let mut compiled = context::compile(
        &workspace,
        None,
        None,
        None,
        None,
        workspace.budget_bytes,
        PartSelection::Required,
    )?;
    let rules = add_rules(&config, &workspace, &mut compiled);
    let text = compiled.text();
    print!("{text}");
    log_injection(
        &config,
        &session,
        "plugin",
        "start",
        rules,
        Vec::new(),
        text.len(),
    );
    Ok(())
}

/// The Claude Code user hook (`hippo hooks install --user`): the machine
/// rules outside the workspace; inside it, nothing when a workspace hook
/// gives the whole block, else every kept rule. No session record.
fn rules_only_session_start() -> Result<()> {
    let mut stdin = String::new();
    io::stdin().read_to_string(&mut stdin)?;
    let input: Value = serde_json::from_str(&stdin).unwrap_or(Value::Null);
    let config = match Config::load() {
        Ok(config) => config,
        Err(hippo_core::error::Error::NotSetUp(_)) => return Ok(()),
        Err(err) => return Err(err.into()),
    };
    let session = input
        .get("session_id")
        .and_then(Value::as_str)
        .unwrap_or("user-hook")
        .to_owned();
    let dir = input
        .get("cwd")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .map(|d| canonical(&d));
    let workspace = match &config.workspace {
        Some(_) => Workspace::load(&config).ok(),
        None => None,
    };
    let inside = match (&workspace, &dir) {
        (Some(ws), Some(dir)) => dir.starts_with(canonical(&ws.root)),
        _ => false,
    };
    let (text, rules) = match (&workspace, inside, &dir) {
        (Some(ws), true, Some(dir)) => {
            let root = canonical(&ws.root);
            if already_hooked(dir, &root) {
                return Ok(());
            }
            let rules = lessons::kept_rules(&config.home, Some(ws));
            let refs: Vec<&lessons::Rule> = rules.iter().collect();
            let (t, ids) = context::rules_text(&refs, config.remember.rules_budget());
            (if t.is_empty() { t } else { t + "\n" }, ids)
        }
        _ => machine_rules(&config),
    };
    print!("{text}");
    if !text.is_empty() {
        log_injection(
            &config,
            &session,
            "claude-code",
            "rules-only",
            rules,
            Vec::new(),
            text.len(),
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Rules, topics and the delivery log

/// Adds the kept rules to a workspace start block; their ids.
fn add_rules(cfg: &Config, ws: &Workspace, compiled: &mut Compiled) -> Vec<ulid::Ulid> {
    let rules = lessons::kept_rules(&cfg.home, Some(ws));
    compiled.add_rules(&rules, true, cfg.remember.rules_budget(), ws.budget_bytes)
}

/// The machine rules alone, as a block, and their ids; empty when there
/// are none.
fn machine_rules(cfg: &Config) -> (String, Vec<ulid::Ulid>) {
    let rules = lessons::kept_rules(&cfg.home, None);
    let refs: Vec<&lessons::Rule> = rules.iter().filter(|r| r.machine).collect();
    let (text, ids) = context::rules_text(&refs, cfg.remember.rules_budget());
    if text.is_empty() {
        return (text, ids);
    }
    (text + "\n", ids)
}

/// Logs a delivery; a hook never fails on it.
fn log_injection(
    cfg: &Config,
    session: &str,
    agent: &str,
    event: &str,
    rules: Vec<ulid::Ulid>,
    notes: Vec<PathBuf>,
    bytes: usize,
) {
    let _ = topic::log(
        &cfg.home,
        &Injection {
            at: time::OffsetDateTime::now_utc(),
            session: session.to_owned(),
            agent: agent.to_owned(),
            event: event.to_owned(),
            rules,
            notes,
            bytes,
        },
    );
}

/// After a start block: the session's topic state begins with the rules it
/// carries, old state files are swept, and the delivery is logged.
fn started(cfg: &Config, session: &str, agent: &str, rules: Vec<ulid::Ulid>, bytes: usize) {
    topic::sweep(&cfg.home, std::time::SystemTime::now());
    let state = Topic {
        rules: rules.clone(),
        ..Topic::default()
    };
    let _ = state.save(&cfg.home, session);
    log_injection(cfg, session, agent, "start", rules, Vec::new(), bytes);
}

/// After the first prompt's notes: they join the topic state, with the
/// prompt's terms.
fn first_prompt(
    cfg: &Config,
    session: &str,
    agent: &str,
    prompt: &str,
    compiled: &Compiled,
    bytes: usize,
) {
    let mut state = Topic::load(&cfg.home, session).unwrap_or_default();
    state.changed(
        &lessons::detect::key_terms(prompt),
        cfg.remember.reinject_every(),
    );
    state.notes.extend(compiled.relevant.iter().cloned());
    let _ = state.save(&cfg.home, session);
    if bytes > 0 {
        log_injection(
            cfg,
            session,
            agent,
            "prompt",
            Vec::new(),
            compiled.relevant.clone(),
            bytes,
        );
    }
}

/// A later prompt: when its topic is new, the kept rules that share its
/// words and were not given yet, and the relevant notes not given yet,
/// within the workspace budget. `None` when the topic is the same, the
/// session has no state, or there is nothing new.
fn new_topic(
    cfg: &Config,
    ws: &Workspace,
    session: &str,
    agent: &str,
    prompt: &str,
) -> Result<Option<String>> {
    let Some(mut state) = Topic::load(&cfg.home, session) else {
        return Ok(None);
    };
    let terms = lessons::detect::key_terms(prompt);
    if !state.changed(&terms, cfg.remember.reinject_every()) {
        state.save(&cfg.home, session)?;
        return Ok(None);
    }
    let compiled = context::compile(
        ws,
        None,
        Some(prompt),
        None,
        None,
        ws.budget_bytes,
        PartSelection::Optional,
    )?;
    let notes = context::new_relevant_lines(&compiled, &state.notes);
    let rules: Vec<lessons::Rule> = lessons::kept_rules(&cfg.home, Some(ws))
        .into_iter()
        .filter(|r| !state.rules.contains(&r.id))
        .filter(|r| r.terms.iter().any(|t| terms.contains(t)))
        .take(3)
        .collect();
    if notes.is_empty() && rules.is_empty() {
        state.save(&cfg.home, session)?;
        return Ok(None);
    }
    let mut text = String::from("For the new topic:");
    let mut given_rules = Vec::new();
    for r in &rules {
        let line = format!("\n- {}", r.text);
        if text.len() + line.len() < ws.budget_bytes {
            text.push_str(&line);
            given_rules.push(r.id);
        }
    }
    let mut given_notes = Vec::new();
    if !notes.is_empty() && text.len() + 10 < ws.budget_bytes {
        text.push_str("\nrelevant:");
        for (path, line) in notes {
            if text.len() + line.len() + 1 < ws.budget_bytes {
                text.push('\n');
                text.push_str(&line);
                given_notes.push(path);
            }
        }
    }
    text.push('\n');
    state.rules.extend(given_rules.iter().copied());
    state.notes.extend(given_notes.iter().cloned());
    state.save(&cfg.home, session)?;
    log_injection(
        cfg,
        session,
        agent,
        "topic",
        given_rules,
        given_notes,
        text.len(),
    );
    Ok(Some(text))
}

/// The user's Claude Code `settings.json` (`$CLAUDE_CONFIG_DIR`, else
/// `~/.claude`).
fn user_settings() -> Option<PathBuf> {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| config::user_home().map(|h| h.join(".claude")))
        .map(|d| d.join("settings.json"))
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
