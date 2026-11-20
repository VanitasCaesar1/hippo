//! `hippo guard check|test|install|uninstall|allow|status|accept`: the guard
//! in each agent's pre-tool hook (`docs/designs/guard.md`, Part B).
//! Warn-only in this release: every rule warns, and only the
//! self-protection rules block.
//!
//! `check` is what the hooks run, on every tool call: it reads the
//! compiled rules (compiling them again only when a source changed), the
//! pending call on stdin, and prints the agent's answer. It answers within
//! `[guard] timeout_ms` itself; when it cannot, the call proceeds, or with
//! `fail = "closed"` is refused, and the failure is logged.

use std::io::{self, Read as _, Write as _};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use hippo_core::accept::Fail;
use hippo_core::config::{self, Config, Workspace};
use hippo_core::guard::{self, Action, Decision};
use hippo_core::hooks::{self, GuardAgent};

/// The agents `install` and `uninstall` mean.
#[derive(clap::ValueEnum, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InstallAgent {
    /// Every Claude Code account's settings.json
    #[default]
    Claude,
    /// Gemini CLI's ~/.gemini/settings.json
    Gemini,
}

impl InstallAgent {
    fn agent(self) -> GuardAgent {
        match self {
            InstallAgent::Claude => GuardAgent::ClaudeCode,
            InstallAgent::Gemini => GuardAgent::Gemini,
        }
    }
}

#[derive(clap::Subcommand)]
pub enum Command {
    /// Run as an agent's pre-tool hook: read one pending call as JSON on stdin and print the answer
    Check {
        /// The agent whose payload and answer format to use
        #[arg(long, value_parser = guard::AGENTS)]
        agent: String,
    },
    /// Count what the rules would have flagged in your indexed history, without acting
    Test {
        /// How many days back
        #[arg(long, default_value_t = 30)]
        days: i64,
        /// Print one JSON object
        #[arg(long)]
        json: bool,
    },
    /// Add the guard to every Claude Code account (or Gemini CLI); lists each file, writes with --yes
    Install {
        #[arg(long, value_enum, default_value_t)]
        agent: InstallAgent,
        #[arg(long)]
        yes: bool,
    },
    /// Remove the guard from every Claude Code account (or Gemini CLI); writes with --yes
    Uninstall {
        #[arg(long, value_enum, default_value_t)]
        agent: InstallAgent,
        #[arg(long)]
        yes: bool,
    },
    /// Add an allow entry (host or origin) to your machine rules, or the team's note with --note; writes with --yes, and waits for hippo accept
    Allow {
        /// host (for remote-script) or origin (for fresh-clone-exec)
        #[arg(value_parser = ["host", "origin"])]
        what: String,
        /// The host with an optional path prefix, or the clone origin
        value: String,
        /// Write to the workspace's guard note instead of $HIPPO_HOME/guard/machine.toml
        #[arg(long)]
        note: bool,
        #[arg(long)]
        yes: bool,
    },
    /// Where the guard is installed, the rules in force, what waits, and the last day's failures
    Status,
    /// The same as hippo accept
    Accept(super::accept::Args),
}

pub fn run(command: Command) -> Result<ExitCode> {
    match command {
        Command::Check { agent } => Ok(check(&agent)),
        Command::Test { days, json } => test(days, json),
        Command::Install { agent, yes } => install(agent.agent(), yes, false),
        Command::Uninstall { agent, yes } => install(agent.agent(), yes, true),
        Command::Allow {
            what,
            value,
            note,
            yes,
        } => allow(&what, &value, note, yes),
        Command::Status => status(),
        Command::Accept(args) => super::accept::run(args),
    }
}

/// Prints `text` once, whichever of the check and the watchdog gets there
/// first, and exits.
fn answer_once(answered: &AtomicBool, text: &str) -> bool {
    if answered.swap(true, Ordering::SeqCst) {
        return false;
    }
    if !text.is_empty() {
        let mut out = io::stdout().lock();
        let _ = writeln!(out, "{text}");
        let _ = out.flush();
    }
    true
}

/// The answer when the guard could not decide: nothing (the call goes
/// on), or with `fail = "closed"` a refusal.
fn failed(agent: &str, fail: Fail, why: &str) -> String {
    match fail {
        Fail::Open => guard::respond(agent, &guard::decide(Vec::new(), false)),
        Fail::Closed => guard::respond(
            agent,
            &Decision {
                action: Some(Action::Block),
                configured: Some(Action::Block),
                message: format!(
                    "hippo guard could not check this call ({why}) and fail = \"closed\", so it is refused. Tell the person."
                ),
                hits: Vec::new(),
            },
        ),
    }
}

fn check(agent: &str) -> ExitCode {
    let started = Instant::now();
    let home = config::home();
    let compiled = guard::for_check(&home);
    let (fail, timeout_ms) = match &compiled {
        Ok(c) => (c.fail, c.timeout_ms),
        Err(_) => (Fail::Open, hippo_core::accept::DEFAULT_TIMEOUT_MS),
    };
    let answered = Arc::new(AtomicBool::new(false));
    {
        let answered = Arc::clone(&answered);
        let agent = agent.to_owned();
        let home = home.clone();
        let left = Duration::from_millis(timeout_ms).saturating_sub(started.elapsed());
        std::thread::spawn(move || {
            std::thread::sleep(left);
            let text = failed(&agent, fail, &format!("no answer within {timeout_ms} ms"));
            if answer_once(&answered, &text) {
                guard::log_failure(
                    &home,
                    &agent,
                    &format!("timeout after {timeout_ms} ms"),
                    fail,
                );
                std::process::exit(0);
            }
        });
    }
    let result = (|| -> Result<(String, Option<(guard::Call, Decision)>)> {
        let compiled = compiled.map_err(|e| anyhow::anyhow!("rules: {e}"))?;
        let mut input = String::new();
        io::stdin().read_to_string(&mut input)?;
        let payload: serde_json::Value = serde_json::from_str(&input)?;
        let Some(call) = guard::parse_call(agent, &payload) else {
            // Not a tool call this guard reads: nothing to say.
            return Ok((
                guard::respond(agent, &guard::decide(Vec::new(), false)),
                None,
            ));
        };
        let decision = guard::decide(guard::evaluate(&compiled, &call), call.headless);
        Ok((guard::respond(agent, &decision), Some((call, decision))))
    })();
    match result {
        Ok((text, logged)) => {
            if answer_once(&answered, &text) {
                if let Some((call, decision)) = logged {
                    guard::log(&home, agent, &call, &decision);
                }
                guard::count_call(&home);
            }
        }
        Err(err) => {
            let why = one_line(&format!("{err:#}"));
            if answer_once(&answered, &failed(agent, fail, &why)) {
                guard::log_failure(&home, agent, &why, fail);
            }
        }
    }
    ExitCode::SUCCESS
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn load() -> Result<(Config, Option<Workspace>)> {
    let cfg = Config::load()?;
    let ws = Workspace::load_optional(&cfg)?;
    Ok((cfg, ws))
}

fn test(days: i64, json: bool) -> Result<ExitCode> {
    if days < 1 {
        bail!("--days must be at least 1");
    }
    let (cfg, ws) = load()?;
    let compiled = guard::current(&cfg, ws.as_ref())?;
    let (counts, calls) = guard::test_history(&cfg.home, &compiled, days)?;
    if json {
        println!(
            "{}",
            serde_json::json!({"days": days, "calls": calls, "rules": counts})
        );
        return Ok(ExitCode::SUCCESS);
    }
    println!(
        "{calls} tool calls in the last {days} days; nothing was blocked or changed. Counts are what each rule would do once ask and block act; in this release every one of them warns, and only self-protection blocks."
    );
    if counts.is_empty() {
        println!("no rule would have fired");
    }
    for c in &counts {
        println!(
            "{}: {} warn, {} ask, {} block",
            c.rule, c.warn, c.ask, c.block
        );
        for e in &c.examples {
            let shown: String = e.chars().take(140).collect();
            println!("    {shown}");
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn install(agent: GuardAgent, yes: bool, remove: bool) -> Result<ExitCode> {
    let user_home = config::user_home().ok_or_else(|| anyhow::anyhow!("no home folder"))?;
    let targets = hippo_core::guard::install::targets(&user_home, &[agent]);
    if targets.is_empty() {
        match agent {
            GuardAgent::ClaudeCode => bail!(
                "no Claude Code account folder found (~/.claude, ~/.claude-*, $CLAUDE_CONFIG_DIR)"
            ),
            GuardAgent::Gemini => bail!("no ~/.gemini folder found"),
        }
    }
    let home = config::home();
    let (timeout_ms, fail) = match load() {
        Ok((cfg, ws)) => {
            let c = guard::current(&cfg, ws.as_ref())?;
            (c.timeout_ms, c.fail)
        }
        Err(_) => (hippo_core::accept::DEFAULT_TIMEOUT_MS, Fail::Open),
    };
    let bin = hooks::binary_path()?;
    let bin_text = bin.to_string_lossy().into_owned();
    let mut plans = Vec::new();
    for (agent, path) in &targets {
        let plan = if remove {
            hooks::guard_remove_plan(path, *agent)?
        } else {
            hooks::guard_plan(path, *agent, &bin_text, timeout_ms, fail == Fail::Closed)?
        };
        let shown = match plan.change {
            "add" => format!("add {}", agent_event(*agent)),
            "update" => format!("update {}", agent_event(*agent)),
            "remove" => format!("remove {}", agent_event(*agent)),
            _ => "no change".to_owned(),
        };
        println!("{}: {shown}", path.display());
        plans.push(plan);
    }
    if !remove {
        println!("the hook runs: {}", agent.command(&bin_text));
    }
    if plans.iter().all(|p| p.change == "none") {
        return Ok(ExitCode::SUCCESS);
    }
    if !yes {
        println!("nothing written; add --yes to write these files");
        return Ok(ExitCode::SUCCESS);
    }
    for plan in &plans {
        plan.apply()?;
    }
    if !remove {
        hooks::write_bin_path(&home, &bin)?;
    }
    println!("written; new sessions use it");
    Ok(ExitCode::SUCCESS)
}

fn agent_event(agent: GuardAgent) -> &'static str {
    match agent {
        GuardAgent::ClaudeCode => "the PreToolUse guard hook",
        GuardAgent::Gemini => "the BeforeTool guard hook",
    }
}

fn allow(what: &str, value: &str, note: bool, yes: bool) -> Result<ExitCode> {
    let (cfg, ws) = load()?;
    let value = value
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    if value.is_empty()
        || value
            .chars()
            .any(|c| c.is_whitespace() || c == '"' || c.is_control())
    {
        bail!("{what}: {value:?} is not a host or origin");
    }
    let key = if what == "host" {
        "hosts"
    } else {
        "clone_origins"
    };
    let path: PathBuf = if note {
        let Some(ws) = &ws else {
            bail!("no workspace, so no guard note");
        };
        ws.guard_note_path()
    } else {
        guard::machine_path(&cfg.home)
    };
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let block = format!("[allow]\n{key} = [\"{value}\"]\n");
    let new = if note {
        format!(
            "{}\n```hippo-guard\n{block}```\n",
            if text.is_empty() {
                "# Guard\n".to_owned()
            } else {
                text.trim_end().to_owned() + "\n"
            }
        )
    } else if text.trim().is_empty() {
        block.clone()
    } else {
        // A second [allow] table would not parse: add to the file's own.
        let mut parsed: toml::Table = toml::from_str(&text)?;
        let allow = parsed
            .entry("allow")
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        let Some(table) = allow.as_table_mut() else {
            bail!("{}: allow is not a table", path.display());
        };
        let list = table
            .entry(key)
            .or_insert_with(|| toml::Value::Array(Vec::new()));
        let Some(items) = list.as_array_mut() else {
            bail!("{}: allow.{key} is not a list", path.display());
        };
        if items.iter().any(|v| v.as_str() == Some(value)) {
            println!("{}: {value} is already allowed", path.display());
            return Ok(ExitCode::SUCCESS);
        }
        items.push(toml::Value::String(value.to_owned()));
        toml::to_string(&parsed)?
    };
    println!("{}", path.display());
    println!("+ {key} = [\"{value}\"] (in [allow])");
    if !yes {
        println!("nothing written; add --yes to write it");
        return Ok(ExitCode::SUCCESS);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, new)?;
    println!(
        "written. An allow entry loosens the rules: it takes effect after you run hippo accept{}",
        if note {
            " on each machine, once the note is committed and pulled"
        } else {
            ""
        }
    );
    Ok(ExitCode::SUCCESS)
}

fn status() -> Result<ExitCode> {
    let (cfg, ws) = load()?;
    let compiled = guard::current(&cfg, ws.as_ref())?;
    let user_home = config::user_home().ok_or_else(|| anyhow::anyhow!("no home folder"))?;
    for (agent, path) in hippo_core::guard::install::targets(
        &user_home,
        &[GuardAgent::ClaudeCode, GuardAgent::Gemini],
    ) {
        let state = match hooks::guard_installed(&path, agent) {
            Ok(Some(command)) => format!("installed: {command}"),
            Ok(None) => "not installed".to_owned(),
            Err(e) => format!("unreadable: {e}"),
        };
        println!("{}: {state}", path.display());
    }
    println!("dsh and OpenCode: the plugin's guard: true option");
    println!(
        "rules in force: {}; fail = {}, timeout {} ms; warn-only in this release",
        guard::summary(&compiled.in_force),
        compiled.fail,
        compiled.timeout_ms
    );
    for w in &compiled.waiting {
        println!("loosening waiting: {w}");
    }
    for p in &compiled.problems {
        println!("problem: {p}");
    }
    let a = guard::activity(&cfg.home);
    println!(
        "today: {} calls checked; last day: {} matched a rule, {} failures",
        a.checked_today, a.matched_day, a.failures_day
    );
    Ok(ExitCode::SUCCESS)
}
