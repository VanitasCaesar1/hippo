//! `hippo session start --tool <t> --session <id> [--task <text>]` and
//! `hippo session end --session <id> [--note <path>]… [--handoff <file>|-]
//! [--no-change <reason>]`.

use std::fs;
use std::io::{self, Read};
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context as _, Result};
use hippo_core::records::Tool;
use hippo_core::sessions;

#[derive(clap::Subcommand)]
pub enum Command {
    /// Write a declared session record, or reopen one
    Start {
        /// The tool the session runs in
        #[arg(long, value_enum)]
        tool: ToolName,
        /// The tool's session id
        #[arg(long)]
        session: String,
        /// What the session is for
        #[arg(long)]
        task: Option<String>,
    },
    /// End a declared session
    End {
        #[arg(long)]
        session: String,
        /// A note the session touched, relative to the workspace (repeatable)
        #[arg(long = "note")]
        notes: Vec<PathBuf>,
        /// A Markdown handoff naming notes with [[...]] links: a file, or - for stdin
        #[arg(long)]
        handoff: Option<String>,
        /// Why the session changed no note
        #[arg(long)]
        no_change: Option<String>,
    },
}

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum ToolName {
    ClaudeCode,
    Codex,
}

pub fn run(command: Command) -> Result<ExitCode> {
    let (_, workspace) = super::load()?;
    match command {
        Command::Start {
            tool,
            session,
            task,
        } => {
            let tool = match tool {
                ToolName::ClaudeCode => Tool::ClaudeCode,
                ToolName::Codex => Tool::Codex,
            };
            sessions::start(&workspace, tool, &session, task.as_deref())?;
        }
        Command::End {
            session,
            notes,
            handoff,
            no_change,
        } => {
            let handoff = match handoff.as_deref() {
                None => None,
                Some("-") => {
                    let mut text = String::new();
                    io::stdin()
                        .read_to_string(&mut text)
                        .context("reading the handoff from stdin")?;
                    Some(text)
                }
                Some(file) => {
                    Some(fs::read_to_string(file).with_context(|| format!("reading {file}"))?)
                }
            };
            sessions::end(
                &workspace,
                &session,
                &notes,
                handoff.as_deref(),
                no_change.as_deref(),
            )?;
        }
    }
    Ok(ExitCode::SUCCESS)
}
