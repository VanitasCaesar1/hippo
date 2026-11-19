//! `hippo hooks install|uninstall|status [--agent <agent>] [--workspace <dir>]`:
//! hippo's three hooks in an agent's hooks file, merged with what is there
//! (`hippo_core::hooks`). For Claude Code (the default) that is the
//! workspace's `.claude/settings.local.json`, the workspace being the
//! configured one unless `--workspace` names another; for Codex, the user's
//! `$CODEX_HOME/hooks.json`; for Cursor, `~/.cursor/hooks.json`, with the
//! session-start hook only; for Gemini CLI, `~/.gemini/settings.json`.
//!
//! Every hook runs this binary by its absolute path, and `install` also
//! writes that path to `$HIPPO_HOME/bin-path` for the plugins. Hooks an
//! older hippo put in a workspace's shared `.claude/settings.json` are
//! moved to `.claude/settings.local.json`: `install` prints the move and
//! makes it with `--yes`, taking out hippo's entries only.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{anyhow, bail, Context as _, Result};
use hippo_core::config::Config;
use hippo_core::hooks::{self, Agent};

/// The agent whose hooks a command means.
#[derive(clap::ValueEnum, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AgentArg {
    /// Claude Code: the workspace's .claude/settings.local.json
    #[default]
    ClaudeCode,
    /// Codex: $CODEX_HOME/hooks.json (~/.codex/hooks.json)
    Codex,
    /// Cursor: ~/.cursor/hooks.json, the session-start hook only
    Cursor,
    /// Gemini CLI: ~/.gemini/settings.json
    Gemini,
}

#[derive(clap::Args)]
pub struct Target {
    /// The agent: claude-code (the workspace's .claude/settings.local.json),
    /// codex ($CODEX_HOME/hooks.json, for every Codex session), cursor
    /// (~/.cursor/hooks.json) or gemini (~/.gemini/settings.json)
    #[arg(long, value_enum, default_value_t)]
    agent: AgentArg,
    /// The workspace, for Claude Code; the configured one when not given
    #[arg(long)]
    workspace: Option<PathBuf>,
}

#[derive(clap::Args)]
pub struct InstallArgs {
    #[command(flatten)]
    target: Target,
    /// Move hippo's hooks out of a workspace's shared .claude/settings.json, where an older hippo put them
    #[arg(long)]
    yes: bool,
}

#[derive(clap::Subcommand)]
pub enum Command {
    /// Add the session-start, prompt and session-end hooks, once each, running this binary by its path
    Install(InstallArgs),
    /// Remove hippo's three hooks, leaving every other hook and key (for Claude Code, from settings.local.json and settings.json)
    Uninstall(Target),
    /// Say which of the three hooks are there
    Status(Target),
}

/// The workspace a Claude Code target means.
fn workspace(target: &Target) -> Result<PathBuf> {
    Ok(match &target.workspace {
        Some(path) => path.clone(),
        None => Config::load()?.workspace_dir()?.to_path_buf(),
    })
}

/// The hooks file and the agent a target means.
fn file(target: Target) -> Result<(PathBuf, Agent)> {
    match target.agent {
        AgentArg::ClaudeCode => Ok((
            hooks::settings_path(&workspace(&target)?),
            Agent::ClaudeCode,
        )),
        AgentArg::Codex => {
            if target.workspace.is_some() {
                bail!("--workspace is for Claude Code; Codex's hooks are in $CODEX_HOME/hooks.json, for every session, and hippo checks each session's directory");
            }
            let home = hooks::codex_home()
                .ok_or_else(|| anyhow!("cannot find Codex's home; set CODEX_HOME or HOME"))?;
            Ok((hooks::codex_hooks_path(&home), Agent::Codex))
        }
        AgentArg::Cursor => {
            if target.workspace.is_some() {
                bail!("--workspace is for Claude Code; Cursor's hooks are in ~/.cursor/hooks.json, for every session, and hippo checks each session's folders");
            }
            let path = hooks::cursor_hooks_path()
                .ok_or_else(|| anyhow!("cannot find the home directory; set HOME"))?;
            Ok((path, Agent::Cursor))
        }
        AgentArg::Gemini => {
            if target.workspace.is_some() {
                bail!("--workspace is for Claude Code; Gemini CLI's hooks are in ~/.gemini/settings.json, for every session, and hippo checks each session's directory");
            }
            let path = hooks::gemini_settings_path()
                .ok_or_else(|| anyhow!("cannot find the home directory; set HOME"))?;
            Ok((path, Agent::Gemini))
        }
    }
}

pub fn run(command: Command) -> Result<ExitCode> {
    match command {
        Command::Install(InstallArgs { target, yes }) => {
            let bin = hooks::binary_path().context("finding this binary's path")?;
            let bin_text = bin
                .to_str()
                .ok_or_else(|| anyhow!("this binary's path is not UTF-8: {}", bin.display()))?
                .to_owned();
            let shared = if target.agent == AgentArg::ClaudeCode {
                let ws = workspace(&target)?;
                let events = hooks::claude_code_shared(&ws)?;
                (!events.is_empty()).then(|| (hooks::shared_settings_path(&ws), events))
            } else {
                None
            };
            let (path, agent) = file(target)?;
            if let Some((shared_path, events)) = &shared {
                println!(
                    "{}: hippo's hooks for {} are in the settings a team commits. They move to {}, which is yours alone, and run {} by its path.",
                    shared_path.display(),
                    events.join(", "),
                    path.display(),
                    bin.display()
                );
                if !yes {
                    println!("Nothing was written. Run again with --yes to move them; only hippo's entries leave {}.", shared_path.display());
                    return Ok(ExitCode::FAILURE);
                }
                let removed = hooks::uninstall_file(shared_path, Agent::ClaudeCode)?;
                println!("{}: removed {}", shared_path.display(), removed.join(", "));
            }
            let done = hooks::install_file(&path, agent, &bin_text)?;
            if done.is_empty() {
                println!("{}: hippo's hooks are already there", path.display());
            } else {
                if !done.added.is_empty() {
                    println!("{}: added {}", path.display(), done.added.join(", "));
                }
                if !done.updated.is_empty() {
                    println!(
                        "{}: {} now run {}",
                        path.display(),
                        done.updated.join(", "),
                        bin.display()
                    );
                }
            }
            let home = hippo_core::config::home();
            if hooks::write_bin_path(&home, &bin)? {
                println!(
                    "{}: {}",
                    hooks::bin_path_file(&home).display(),
                    bin.display()
                );
            }
            let added = done.added;
            if agent == Agent::Cursor && !added.is_empty() {
                println!("Cursor gives a new chat the start block from now on. For the notes relevant to a task, add hippo's MCP server too: hippo mcp install --client cursor");
            }
            if agent == Agent::Codex {
                if let Some(home) = path.parent() {
                    if hooks::codex_config_declares_hooks(home) {
                        println!(
                            "{} also declares hooks in [hooks]; Codex loads both files and warns",
                            home.join("config.toml").display()
                        );
                    }
                }
                if !added.is_empty() {
                    println!("Codex lists new hooks for review the next time it starts: trust them there, or later with /hooks. Until then Codex skips them.");
                }
            }
        }
        Command::Uninstall(target) => {
            let shared = (target.agent == AgentArg::ClaudeCode)
                .then(|| workspace(&target).map(|ws| hooks::shared_settings_path(&ws)))
                .transpose()?;
            let (path, agent) = file(target)?;
            if let Some(shared) = shared {
                let removed = hooks::uninstall_file(&shared, agent)?;
                if !removed.is_empty() {
                    println!("{}: removed {}", shared.display(), removed.join(", "));
                }
            }
            let removed = hooks::uninstall_file(&path, agent)?;
            if removed.is_empty() {
                println!("{}: no hippo hook there", path.display());
            } else {
                println!("{}: removed {}", path.display(), removed.join(", "));
            }
        }
        Command::Status(target) => {
            if target.agent == AgentArg::ClaudeCode {
                let ws = workspace(&target)?;
                let shared = hooks::claude_code_shared(&ws)?;
                if !shared.is_empty() {
                    println!(
                        "{}: hippo's hooks for {} are here, in the settings a team commits; hippo hooks install --yes moves them to {}",
                        hooks::shared_settings_path(&ws).display(),
                        shared.join(", "),
                        hooks::settings_path(&ws).display()
                    );
                }
            }
            let (path, agent) = file(target)?;
            for (event, has) in hooks::status_file(&path, agent)? {
                println!("{event}: {}", if has { "installed" } else { "missing" });
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}
