//! `hippocamp doctor [--model]`: prints the `check | status | detail` table and
//! exits 1 on any problem. Nothing leaves the machine unless `--model` asks
//! for the one-word model check.

use std::process::ExitCode;

use anyhow::Result;
use hippocamp_core::{config, doctor};

#[derive(clap::Args)]
pub struct Args {
    /// Also send one word through model_command to check it answers (a nightly check should pass this)
    #[arg(long)]
    model: bool,
}

pub fn run(args: Args) -> Result<ExitCode> {
    let report = doctor::run_in(&config::home(), args.model);
    print!("{report}");
    Ok(if report.has_problem() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}
