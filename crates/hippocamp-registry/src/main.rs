//! `hippocamp-registry`: checks the registry's entries, runs the fresh-context
//! review of a pack and builds the registry index. Errors print to stderr and exit 1.

mod build;
mod check;
mod review;
mod site;

use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Context;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "hippocamp-registry",
    version,
    about = "The hippocamp pack registry's tool"
)]
struct Cli {
    /// The repository that holds `registry/`
    #[arg(long, global = true, default_value = ".")]
    repo_root: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Check each entry (or one); exits 1 on any error
    Check(check::Args),
    /// Review a pack with a fresh-context model and save the review
    Review(review::Args),
    /// Write the index of the entries that pass the checks
    Build(build::Args),
    /// The stub model of the fixture tests: replies with a file's text
    #[command(hide = true)]
    StubModel {
        /// The file whose text is the reply
        #[arg(long)]
        reply: PathBuf,
        /// The file the prompt is appended to, as one JSON line
        #[arg(long)]
        log: PathBuf,
        /// The system prompt the caller passed
        #[arg(long, default_value = "")]
        system_prompt: String,
    },
}

fn stub_model(
    reply: &std::path::Path,
    log: &std::path::Path,
    system_prompt: &str,
) -> anyhow::Result<ExitCode> {
    let mut stdin = String::new();
    std::io::stdin()
        .read_to_string(&mut stdin)
        .context("reading stdin")?;
    println!(
        "{}",
        hippocamp_core::packs::testing::stub_model(reply, log, system_prompt, &stdin)?
    );
    Ok(ExitCode::SUCCESS)
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Check(args) => check::run(&cli.repo_root, &args),
        Command::Review(args) => review::run(&cli.repo_root, &args),
        Command::Build(args) => build::run(&cli.repo_root, &args),
        Command::StubModel {
            reply,
            log,
            system_prompt,
        } => stub_model(&reply, &log, &system_prompt),
    };
    match result {
        Ok(code) => code,
        Err(err) => {
            eprintln!(
                "error: {}",
                hippocamp_core::packs::escape_block(&format!("{err:#}"))
            );
            ExitCode::FAILURE
        }
    }
}
