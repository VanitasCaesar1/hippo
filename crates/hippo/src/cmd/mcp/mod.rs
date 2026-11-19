//! `hippo mcp [--allow-write]`: an MCP server over stdio, for any agent
//! that speaks the Model Context Protocol. `hippo mcp install --client
//! <client> [--write]` prints, or writes, that client's configuration for
//! it. See `docs/designs/mcp-server.md` and `docs/mcp.md`.

pub mod install;
pub mod ordered;
pub mod server;

use std::process::ExitCode;

use anyhow::Result;

#[derive(clap::Args)]
#[command(args_conflicts_with_subcommands = true)]
pub struct Args {
    #[command(subcommand)]
    command: Option<Command>,
    /// Also offer hippo_note_correction, which queues a correction to the
    /// Inbox after a fresh-context review (never to a note)
    #[arg(long)]
    allow_write: bool,
}

#[derive(clap::Subcommand)]
enum Command {
    /// Print a client's MCP configuration for hippo; --write merges it into the client's file
    Install(install::Args),
}

pub fn run(args: Args) -> Result<ExitCode> {
    match args.command {
        Some(Command::Install(args)) => install::run(args),
        None => server::serve(args.allow_write),
    }
}
