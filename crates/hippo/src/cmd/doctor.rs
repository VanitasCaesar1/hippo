//! `hippo doctor [--model [--canary] [--runner <name>]] [--fix-permissions]`: prints the `check | status | detail` table and
//! exits 1 on any problem. Nothing leaves the machine unless `--model` asks
//! for the one-word model check.

use std::process::ExitCode;

use anyhow::Result;
use hippo_core::{config, doctor};

#[derive(clap::Args)]
pub struct Args {
    /// Also send one word through model_command to check it answers (a nightly check should pass this)
    #[arg(long)]
    model: bool,
    /// First make every file under $HIPPO_HOME 0600 and every folder 0700
    #[arg(long)]
    fix_permissions: bool,
    /// With --model: plant two canary files and check each runner reads neither (one model call per runner)
    #[arg(long)]
    canary: bool,
    /// With --model: check only this runner
    #[arg(long)]
    runner: Option<String>,
}

pub fn run(args: Args) -> Result<ExitCode> {
    let home = config::home();
    if args.fix_permissions {
        let fixed = doctor::fix_permissions(&home)?;
        println!(
            "{}: {fixed} files and folders made private (0600 files, 0700 folders)",
            home.display()
        );
    }
    let report = doctor::run_with_options(
        &home,
        &doctor::ModelOptions {
            model: args.model,
            canary: args.canary,
            runner: args.runner,
        },
    );
    print!("{report}");
    Ok(if report.has_problem() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}
