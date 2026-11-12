//! `hippocamp`, the command line of the hippocamp memory engine. Errors print to
//! stderr and exit 1, except where a command says otherwise; `RUST_LOG`
//! sets the log level (warnings by default).

mod card;
mod cmd;

use std::process::ExitCode;

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(
    name = "hippocamp",
    version,
    about = "One brain for all your coding agents. hippocamp reads everything they did, keeps your notes true, and briefs every new session."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

/// The commands as `hippocamp --help` groups them, each
/// once; a test checks that every subcommand is in a group.
const GROUPS: [(&str, &[&str]); 6] = [
    ("Start here", &["start", "detect"]),
    (
        "Every day",
        &[
            "ask",
            "recall",
            "fleet",
            "incidents",
            "xray",
            "show",
            "resume",
            "brief",
            "card",
        ],
    ),
    (
        "Notes and dreaming",
        &["context", "dream", "lessons", "review", "revert", "eval"],
    ),
    ("Packs", &["add", "packs"]),
    (
        "Setup and checks",
        &["init", "ingest", "config", "doctor", "schedule", "hooks"],
    ),
    (
        "Run by hippocamp itself (hooks and jobs call these)",
        &["hook", "session", "log"],
    ),
];

/// The grouped command list for `--help`: `hippocamp` alone first, then each
/// group with its commands' one-line help.
fn grouped_help(cmd: &clap::Command) -> String {
    let about = |name: &str| {
        cmd.find_subcommand(name)
            .and_then(|c| c.get_about())
            .map(|a| a.to_string())
            .unwrap_or_default()
    };
    let width = GROUPS
        .iter()
        .flat_map(|(_, names)| names.iter())
        .map(|n| n.len())
        .max()
        .unwrap_or(0)
        .max(6);
    let mut out = String::new();
    for (title, names) in GROUPS {
        out.push_str(&format!("\n{title}:\n"));
        if title == "Start here" {
            out.push_str(&format!(
                "  {:<width$}  On its own: find your agents' history and index it; once set up, show the last active day\n",
                "hippocamp"
            ));
        }
        for name in names {
            out.push_str(&format!("  {name:<width$}  {}\n", about(name)));
        }
    }
    // The template reads `{…}` as tags.
    out.replace('{', "{{").replace('}', "}}")
}

#[derive(Subcommand)]
enum Command {
    /// Find your agents' history and index it (history only); --yes skips the question
    Start(cmd::start::Args),
    /// Write $HIPPOCAMP_HOME/config.toml and the workspace's .hippocamp/
    Init(cmd::init::Args),
    /// Copy a pack into the workspace from git or a path, after a review
    Add(cmd::add::Args),
    /// List the agent history on this machine, before anything is indexed
    Detect(cmd::detect::Args),
    /// Index the transcripts under the transcript roots
    Ingest(cmd::ingest::Args),
    /// Search every indexed transcript turn
    Recall(cmd::recall::Args),
    /// Has any agent already solved this? Ranked sources, and an answer that cites them
    Ask(cmd::ask::Args),
    /// Today's sessions (live, stopped at a usage limit, finished) and headroom per profile
    Fleet(cmd::fleet::Args),
    /// Print the command that resumes a session, with a short handoff
    Resume(cmd::fleet::ResumeArgs),
    /// What context a session was given and used, what it searched for, and its claims' evidence
    Xray(cmd::xray::Args),
    /// What went wrong, which agent did it, at which step, and why when a rule says so
    Incidents(cmd::incidents::Args),
    /// Print the context block an agent sees at session start
    Context(cmd::context::Args),
    /// The start block for a new agent on a task: required and relevant notes, past sessions, kept rules
    Brief(cmd::brief::Args),
    /// Your agents' day (or --week) as one image to share: numbers and a date, no names
    Card(cmd::card::Args),
    /// Start or end a declared session record
    #[command(subcommand)]
    Session(cmd::session::Command),
    /// Run as a Claude Code hook, reading the hook's JSON on stdin
    #[command(subcommand)]
    Hook(cmd::hook::Command),
    /// Install or check the three Claude Code hooks in the workspace's .claude/settings.json
    #[command(subcommand)]
    Hooks(cmd::hooks::Command),
    /// Show or change one setting in config.toml or hippocamp.toml; prints the change, writes it with --yes
    #[command(subcommand)]
    Config(cmd::settings::Command),
    /// Print a session's turns, a proposal with its evidence, or a note's lint and receipts
    #[command(subcommand)]
    Show(cmd::show::Command),
    /// Write the session log
    Log(cmd::log::Args),
    /// Check the setup and the last runs; exits 1 on any problem
    Doctor(cmd::doctor::Args),
    /// Run the dreaming passes over the window once; exits 2 on failure
    Dream(cmd::dream::Args),
    /// Corrections and repeated failures proposed as rules: list, scan, keep, dismiss
    Lessons(cmd::lessons::Args),
    /// Mark applied dreaming changes as read and right (--ok)
    Review(cmd::review::Args),
    /// Undo an applied dreaming change from its snapshot
    Revert(cmd::revert::Args),
    /// Measure context, recall and dreaming on an eval set
    Eval(cmd::eval::Args),
    /// Work with packs
    #[command(subcommand)]
    Packs(cmd::packs::Command),
    /// Schedule ingest and dreaming: launchd jobs on macOS, cron or systemd lines to copy elsewhere
    #[command(subcommand)]
    Schedule(cmd::schedule::Command),
}

fn main() -> ExitCode {
    hippocamp_core::default_sigpipe();
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
    let mut command = Cli::command();
    let groups = grouped_help(&command);
    command = command.help_template(format!(
        "{{before-help}}{{about-with-newline}}\n{{usage-heading}} {{usage}}\n{groups}\nOptions:\n{{options}}{{after-help}}"
    ));
    let cli = Cli::from_arg_matches(&command.get_matches()).unwrap_or_else(|e| e.exit());
    let Some(command) = cli.command else {
        return finish(cmd::start::bare());
    };
    let result = match command {
        Command::Start(args) => cmd::start::run(args),
        Command::Init(args) => cmd::init::run(args),
        Command::Add(args) => cmd::add::run(args),
        Command::Detect(args) => cmd::detect::run(args),
        Command::Ingest(args) => cmd::ingest::run(args),
        Command::Recall(args) => cmd::recall::run(args),
        Command::Ask(args) => cmd::ask::run(args),
        Command::Fleet(args) => cmd::fleet::run(args),
        Command::Resume(args) => cmd::fleet::resume(args),
        Command::Xray(args) => cmd::xray::run(args),
        Command::Incidents(args) => cmd::incidents::run(args),
        Command::Context(args) => cmd::context::run(args),
        Command::Brief(args) => cmd::brief::run(args),
        Command::Card(args) => cmd::card::run(args),
        Command::Session(command) => cmd::session::run(command),
        Command::Hook(command) => return cmd::hook::run(command),
        Command::Hooks(command) => cmd::hooks::run(command),
        Command::Config(command) => cmd::settings::run(command),
        Command::Show(command) => cmd::show::run(command),
        Command::Log(args) => cmd::log::run(args),
        Command::Doctor(args) => cmd::doctor::run(args),
        Command::Dream(args) => return cmd::dream::run(args),
        Command::Lessons(args) => cmd::lessons::run(args),
        Command::Review(args) => cmd::review::run(args),
        Command::Revert(args) => cmd::revert::run(args),
        Command::Eval(args) => cmd::eval::run(args),
        Command::Packs(command) => cmd::packs::run(command),
        Command::Schedule(command) => cmd::schedule::run(command),
    };
    finish(result)
}

/// A command's exit code, or its error on stderr and exit 1.
fn finish(result: anyhow::Result<ExitCode>) -> ExitCode {
    match result {
        Ok(code) => code,
        Err(err) => {
            eprintln!("{}", error_text(&err));
            ExitCode::FAILURE
        }
    }
}

/// An error as one message, with every control and bidirectional character
/// but line feed and tab written as `\u{..}`: a message may carry a file
/// name, a git line or an index field, and none may move the cursor.
fn error_text(err: &anyhow::Error) -> String {
    hippocamp_core::packs::escape_block(&format!("{err:#}"))
}

#[cfg(test)]
mod tests {
    use super::{error_text, Cli, GROUPS};
    use clap::CommandFactory;

    #[test]
    fn help_groups_hold_every_command_once() {
        let cmd = Cli::command();
        let mut grouped: Vec<&str> = GROUPS.iter().flat_map(|(_, n)| n.iter().copied()).collect();
        grouped.sort_unstable();
        let before = grouped.len();
        grouped.dedup();
        assert_eq!(before, grouped.len(), "a command is in two groups");
        let mut all: Vec<&str> = cmd.get_subcommands().map(|c| c.get_name()).collect();
        all.sort_unstable();
        assert_eq!(grouped, all);
    }

    #[test]
    fn an_error_prints_without_terminal_control_sequences() {
        let err = anyhow::anyhow!("a\u{1b}]0;pwned\u{7}b\u{1b}[2J\u{202e}c\nnext\tline");
        let text = error_text(&err);
        assert_eq!(
            text,
            "a\\u{1b}]0;pwned\\u{7}b\\u{1b}[2J\\u{202e}c\nnext\tline"
        );
    }
}
