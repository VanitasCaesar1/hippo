//! `hippo setup [--agent claude-code|codex] [--workspace <dir>]
//! [--notes-dir <rel>] [--entry-note <rel>] [--yes]`: the whole first run
//! for one agent, the command the Claude Code plugin's `/hippo:setup` and
//! the Codex plugin's `$hippo` run.
//!
//! Without `--yes` it prints the plan and writes nothing. With `--yes` it
//! runs, as this same binary: `hippo init` (unless the workspace is
//! already hippo's), `hippo hooks install` for the agent, and `hippo
//! ingest`, stopping at the first that fails. It never schedules anything:
//! it ends by naming `hippo schedule install`, which the plugins run only
//! after the person says yes, and on macOS `hippo app install`.
//!
//! Design: docs/designs/setup.md.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use anyhow::{bail, Context as _, Result};
use hippo_core::config::Config;

/// The agent the setup is for.
#[derive(clap::ValueEnum, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SetupAgent {
    /// Claude Code: the workspace's .claude/settings.local.json
    #[default]
    ClaudeCode,
    /// Codex: $CODEX_HOME/hooks.json (~/.codex/hooks.json)
    Codex,
}

impl SetupAgent {
    fn name(self) -> &'static str {
        match self {
            SetupAgent::ClaudeCode => "Claude Code",
            SetupAgent::Codex => "Codex",
        }
    }

    fn arg(self) -> &'static str {
        match self {
            SetupAgent::ClaudeCode => "claude-code",
            SetupAgent::Codex => "codex",
        }
    }
}

#[derive(clap::Args)]
pub struct Args {
    /// The agent this runs in: its hooks are the ones installed
    #[arg(long, value_enum, default_value_t)]
    agent: SetupAgent,
    /// The project; the enclosing Git repository's root, else the current directory
    #[arg(long)]
    workspace: Option<PathBuf>,
    /// The notes folder, relative to the project [default: notes]
    #[arg(long)]
    notes_dir: Option<String>,
    /// The entry note, relative to the project [default: <notes>/START HERE.md]
    #[arg(long)]
    entry_note: Option<String>,
    /// Run the steps; without it, print them and write nothing
    #[arg(long)]
    yes: bool,
}

/// One step: what it runs and what it writes.
struct Step {
    args: Vec<String>,
    what: String,
}

/// The steps for `args`, given the configuration as it is.
fn plan(args: &Args, config: Option<&Config>, root: &Path) -> Result<Vec<Step>> {
    let notes = args.notes_dir.clone().unwrap_or_else(|| "notes".to_owned());
    let entry = args
        .entry_note
        .clone()
        .unwrap_or_else(|| format!("{notes}/START HERE.md"));
    let configured = config.and_then(|c| c.workspace.clone());
    let mut steps = Vec::new();
    match configured {
        Some(ws) if same(&ws, root) => {}
        Some(ws) => bail!(
            "hippo's workspace is {}, not {}. hippo has one workspace per machine; to move it here run\n  hippo init --workspace {} --notes-dir {notes} --entry-note \"{entry}\" --force\n(that replaces {} in config.toml)",
            ws.display(),
            root.display(),
            shell_quote(&root.display().to_string()),
            ws.display(),
        ),
        None => steps.push(Step {
            args: vec![
                "init".into(),
                "--workspace".into(),
                root.display().to_string(),
                "--notes-dir".into(),
                notes.clone(),
                "--entry-note".into(),
                entry.clone(),
            ],
            what: format!(
                "writes ~/.hippo/config.toml and {root}/.hippo/, creates {notes}/ and {entry} if they are missing, and adds hippo's lines to .gitignore and .gitattributes (it commits nothing)",
                root = root.display()
            ),
        }),
    }
    let hooks_file = match args.agent {
        SetupAgent::ClaudeCode => format!("{}/.claude/settings.local.json", root.display()),
        SetupAgent::Codex => "$CODEX_HOME/hooks.json (~/.codex/hooks.json)".to_owned(),
    };
    steps.push(Step {
        args: vec![
            "hooks".into(),
            "install".into(),
            "--agent".into(),
            args.agent.arg().into(),
        ],
        what: format!(
            "adds the session-start, prompt and session-end hooks to {hooks_file}; they act only for sessions in this project"
        ),
    });
    steps.push(Step {
        args: vec!["ingest".into()],
        what: "indexes the agent history already on this machine into ~/.hippo; nothing is sent anywhere".into(),
    });
    Ok(steps)
}

fn same(a: &Path, b: &Path) -> bool {
    let c = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    c(a) == c(b)
}

fn shell_quote(s: &str) -> String {
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-~".contains(c))
    {
        s.to_owned()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// The project: `--workspace`, else the Git root above the current
/// directory, else the current directory.
fn project(args: &Args) -> Result<PathBuf> {
    let here = std::env::current_dir().context("cannot read the current directory")?;
    let dir = match &args.workspace {
        Some(w) if w.is_absolute() => w.clone(),
        Some(w) => here.join(w),
        None => here
            .ancestors()
            .find(|d| d.join(".git").exists())
            .map_or(here.clone(), Path::to_path_buf),
    };
    if !dir.is_dir() {
        bail!("{} is not a directory", dir.display());
    }
    Ok(dir.canonicalize().unwrap_or(dir))
}

fn command_line(step: &Step) -> String {
    let mut out = String::from("hippo");
    for a in &step.args {
        out.push(' ');
        out.push_str(&shell_quote(a));
    }
    out
}

pub fn run(args: Args) -> Result<ExitCode> {
    let root = project(&args)?;
    let config = Config::load().ok();
    let steps = plan(&args, config.as_ref(), &root)?;
    let agent = args.agent.name();
    if !args.yes {
        println!("hippo setup for {agent} in {}:", root.display());
        if config.as_ref().and_then(|c| c.workspace.as_ref()).is_some() && steps.len() == 2 {
            println!("  (hippo is already set up here: init is skipped)");
        }
        for (i, step) in steps.iter().enumerate() {
            println!("  {}. {}", i + 1, command_line(step));
            println!("     {}", step.what);
        }
        println!("It schedules nothing. Run it with --yes to do these steps.");
        return Ok(ExitCode::SUCCESS);
    }
    let exe = std::env::current_exe().context("cannot find the running hippo binary")?;
    for (i, step) in steps.iter().enumerate() {
        println!("[{}/{}] {}", i + 1, steps.len(), command_line(step));
        let status = Command::new(&exe)
            .args(&step.args)
            .current_dir(&root)
            .status()
            .with_context(|| format!("cannot run {}", command_line(step)))?;
        if !status.success() {
            eprintln!(
                "hippo setup stopped: {} failed ({status}); what it printed above says why. Nothing after it ran.",
                command_line(step)
            );
            return Ok(ExitCode::from(1));
        }
    }
    println!();
    println!(
        "Ready. Start a new {agent} session in {}: it begins with hippo's start block.",
        root.display()
    );
    if args.agent == SetupAgent::Codex {
        println!("Codex lists hippo's three hooks for review at its next start; they run once you trust them there (or with /hooks).");
    }
    println!("Not set up: the nightly schedule. `hippo schedule install` indexes every 10 minutes and runs the night pass at 03:30 with your claude CLI; nothing runs on its own until you do that.");
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(agent: SetupAgent) -> Args {
        Args {
            agent,
            workspace: None,
            notes_dir: None,
            entry_note: None,
            yes: false,
        }
    }

    #[test]
    fn a_new_machine_gets_init_hooks_and_ingest() {
        let steps = plan(&args(SetupAgent::Codex), None, Path::new("/work/tern")).unwrap();
        let lines: Vec<String> = steps.iter().map(command_line).collect();
        assert_eq!(
            lines,
            [
                "hippo init --workspace /work/tern --notes-dir notes --entry-note 'notes/START HERE.md'",
                "hippo hooks install --agent codex",
                "hippo ingest",
            ]
        );
    }

    #[test]
    fn the_notes_folder_can_be_named() {
        let mut a = args(SetupAgent::ClaudeCode);
        a.notes_dir = Some("docs/agents".into());
        let steps = plan(&a, None, Path::new("/work/tern")).unwrap();
        assert!(command_line(&steps[0])
            .ends_with("--notes-dir docs/agents --entry-note 'docs/agents/START HERE.md'"));
        assert!(steps[1]
            .what
            .contains("/work/tern/.claude/settings.local.json"));
    }

    #[test]
    fn nothing_schedules() {
        let steps = plan(&args(SetupAgent::Codex), None, Path::new("/w")).unwrap();
        assert!(steps.iter().all(|s| s.args[0] != "schedule"));
    }
}
