//! `hippo hooks install|uninstall|status [--agent <agent>] [--workspace <dir>]`:
//! hippo's three hooks in an agent's hooks file, merged with what is there
//! (`hippo_core::hooks`). For Claude Code (the default) that is the
//! workspace's `.claude/settings.json`, the workspace being the configured
//! one unless `--workspace` names another; for Codex, the user's
//! `$CODEX_HOME/hooks.json`.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{anyhow, bail, Result};
use hippo_core::config::Config;
use hippo_core::hooks::{self, Agent};

/// The agent whose hooks a command means.
#[derive(clap::ValueEnum, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AgentArg {
    /// Claude Code: the workspace's .claude/settings.json
    #[default]
    ClaudeCode,
    /// Codex: $CODEX_HOME/hooks.json (~/.codex/hooks.json)
    Codex,
}

#[derive(clap::Args)]
pub struct Target {
    /// The agent: claude-code (the workspace's .claude/settings.json) or
    /// codex ($CODEX_HOME/hooks.json, for every Codex session)
    #[arg(long, value_enum, default_value_t)]
    agent: AgentArg,
    /// The workspace, for Claude Code; the configured one when not given
    #[arg(long)]
    workspace: Option<PathBuf>,
}

#[derive(clap::Subcommand)]
pub enum Command {
    /// Add the session-start, prompt and session-end hooks, once each
    Install(Target),
    /// Remove hippo's three hooks, leaving every other hook and key
    Uninstall(Target),
    /// Say which of the three hooks are there
    Status(Target),
}

/// The hooks file and the agent a target means.
fn file(target: Target) -> Result<(PathBuf, Agent)> {
    match target.agent {
        AgentArg::ClaudeCode => {
            let ws = match target.workspace {
                Some(path) => path,
                None => Config::load()?.workspace_dir()?.to_path_buf(),
            };
            Ok((hooks::settings_path(&ws), Agent::ClaudeCode))
        }
        AgentArg::Codex => {
            if target.workspace.is_some() {
                bail!("--workspace is for Claude Code; Codex's hooks are in $CODEX_HOME/hooks.json, for every session, and hippo checks each session's directory");
            }
            let home = hooks::codex_home()
                .ok_or_else(|| anyhow!("cannot find Codex's home; set CODEX_HOME or HOME"))?;
            Ok((hooks::codex_hooks_path(&home), Agent::Codex))
        }
    }
}

pub fn run(command: Command) -> Result<ExitCode> {
    match command {
        Command::Install(target) => {
            let (path, agent) = file(target)?;
            let added = hooks::install_file(&path, agent)?;
            if added.is_empty() {
                println!("{}: the three hooks are already there", path.display());
            } else {
                println!("{}: added {}", path.display(), added.join(", "));
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
            let (path, agent) = file(target)?;
            let removed = hooks::uninstall_file(&path, agent)?;
            if removed.is_empty() {
                println!("{}: no hippo hook there", path.display());
            } else {
                println!("{}: removed {}", path.display(), removed.join(", "));
            }
        }
        Command::Status(target) => {
            let (path, agent) = file(target)?;
            for (event, has) in hooks::status_file(&path, agent)? {
                println!("{event}: {}", if has { "installed" } else { "missing" });
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}
