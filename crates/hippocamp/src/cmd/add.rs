//! `hippocamp add <source> [--registry <path>] [--dry-run]`: copies a pack into
//! the workspace from a path, an https git repository or a registry index, after validation and a pack review.
//! Exits 1 on a refusal (an invalid pack, an existing pack or pass name, a
//! `reject`) and on an error (a model that could not run).

use std::process::ExitCode;

use anyhow::Result;
use hippocamp_core::config;
use hippocamp_core::packs::install::{self, AddOutcome};
use hippocamp_core::packs::source;

use super::{git_options, load_recovered, print_schedule_warning};

#[derive(clap::Args)]
pub struct Args {
    /// A path (./dir, ../dir, /dir, ~/dir), an https git URL
    /// (https://host/repo[//dir][@ref]) or a pack name listed in the index
    /// given with --registry
    source: String,
    /// The index.json file to look a registry name up in
    #[arg(long)]
    registry: Option<std::path::PathBuf>,
    /// Validate and print the summary; write nothing and make no model call
    #[arg(long)]
    dry_run: bool,
}

pub fn run(args: Args) -> Result<ExitCode> {
    let (config, workspace) = load_recovered()?;
    print_schedule_warning();
    let cwd = std::env::current_dir()?;
    let source = source::parse(&args.source, &cwd, config::user_home().as_deref())?;
    let incoming = install::fetch(
        &workspace,
        &source,
        args.registry.as_deref(),
        &git_options(),
    )?;
    let report = install::add(&config, &workspace, incoming, args.dry_run)?;
    for line in &report.lines {
        println!("{line}");
    }
    Ok(match report.outcome {
        AddOutcome::Refused => ExitCode::FAILURE,
        AddOutcome::Written(_) | AddOutcome::DryRun => ExitCode::SUCCESS,
    })
}
