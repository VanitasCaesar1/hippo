//! `hippocamp hooks install|uninstall|status [--workspace <dir>]`: the three Claude Code
//! hooks in the workspace's `.claude/settings.json`, merged with what is
//! there (`hippocamp_core::hooks`). The workspace is the configured one unless
//! `--workspace` names another.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Result;
use hippocamp_core::config::Config;
use hippocamp_core::hooks;

#[derive(clap::Subcommand)]
pub enum Command {
    /// Add the session-start, prompt and session-end hooks, once each
    Install {
        /// The workspace; the configured one when not given
        #[arg(long)]
        workspace: Option<PathBuf>,
    },
    /// Remove hippocamp's three hooks, leaving every other hook and key
    Uninstall {
        /// The workspace; the configured one when not given
        #[arg(long)]
        workspace: Option<PathBuf>,
    },
    /// Say which of the three hooks the workspace has
    Status {
        /// The workspace; the configured one when not given
        #[arg(long)]
        workspace: Option<PathBuf>,
    },
}

fn workspace(given: Option<PathBuf>) -> Result<PathBuf> {
    match given {
        Some(path) => Ok(path),
        None => Ok(Config::load()?.workspace_dir()?.to_path_buf()),
    }
}

pub fn run(command: Command) -> Result<ExitCode> {
    match command {
        Command::Install { workspace: given } => {
            let ws = workspace(given)?;
            let added = hooks::install(&ws)?;
            let path = hooks::settings_path(&ws);
            if added.is_empty() {
                println!("{}: the three hooks are already there", path.display());
            } else {
                println!("{}: added {}", path.display(), added.join(", "));
            }
        }
        Command::Uninstall { workspace: given } => {
            let ws = workspace(given)?;
            let removed = hooks::uninstall(&ws)?;
            let path = hooks::settings_path(&ws);
            if removed.is_empty() {
                println!("{}: no hippocamp hook there", path.display());
            } else {
                println!("{}: removed {}", path.display(), removed.join(", "));
            }
        }
        Command::Status { workspace: given } => {
            let ws = workspace(given)?;
            for (event, has) in hooks::status(&ws)? {
                println!("{event}: {}", if has { "installed" } else { "missing" });
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}
