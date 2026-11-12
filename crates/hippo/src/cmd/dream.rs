//! `hippo dream [--pass <name>] [--dry-run]`: one dreaming run. It prints
//! the report's path and the run's counts. A failed run, which still writes
//! its report, prints the error and exits 2. After the passes it runs the
//! lessons pass, except with `--dry-run`, `--pass`
//! or `--no-lessons`; a lessons failure is printed and recorded in
//! `lessons-state.json` and never changes the exit code.

use std::process::ExitCode;

use anyhow::Result;
use hippo_core::dream::{self, RunSummary};

#[derive(clap::Args)]
pub struct Args {
    /// Run only the pass with this file stem, such as verify
    #[arg(long)]
    pass: Option<String>,
    /// Review but write nothing of dreaming except the report
    #[arg(long)]
    dry_run: bool,
    /// Skip the lessons pass that follows dreaming
    #[arg(long)]
    no_lessons: bool,
}

pub fn run(args: Args) -> ExitCode {
    let code = match dream(&args) {
        Ok(summary) => {
            println!("{summary}");
            if summary.proposals == 0 {
                explain_nothing(&summary.report);
            }
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("{err:#}");
            ExitCode::from(2)
        }
    };
    if !(args.dry_run || args.pass.is_some() || args.no_lessons) {
        lessons();
    }
    code
}

/// The lessons pass after dreaming: over the last 7 days, or 21 on the
/// first scan; skipped when the workspace sets
/// `lessons = false`.
fn lessons() {
    let Ok((config, workspace)) = super::load() else {
        return;
    };
    if !workspace.lessons {
        println!("lessons: off (lessons = false in hippo.toml)");
        return;
    }
    let days = hippo_core::lessons::first_scan_days(&config.home);
    match hippo_core::lessons::scan(&config, &workspace, days, false) {
        Ok(s) => println!(
            "lessons: {} sessions read, {} candidates, {} confirmed into the Inbox, {} model calls",
            s.state.sessions, s.state.candidates, s.state.confirmed, s.state.model_calls
        ),
        Err(err) => {
            eprintln!("lessons: {err}");
            let state = hippo_core::lessons::State {
                at: Some(time::OffsetDateTime::now_utc()),
                days,
                error: Some(err.to_string()),
                ..Default::default()
            };
            let _ = state.save(&config.home);
        }
    }
}

/// A run that proposed nothing says why on stdout, from its report's
/// skip lines, and what dreaming needs.
fn explain_nothing(report: &std::path::Path) {
    let Ok(text) = std::fs::read_to_string(report) else {
        return;
    };
    let mut why: Vec<&str> = text
        .lines()
        .filter_map(|l| l.strip_prefix("- "))
        .filter(|l| {
            l.ends_with(": no session record has a turn here")
                || l.ends_with(": no note to work on")
                || l.contains(": skipped: ")
        })
        .collect();
    why.dedup();
    if why.is_empty() {
        return;
    }
    println!("nothing proposed:");
    for line in why.iter().take(6) {
        println!("  {line}");
    }
    if why.len() > 6 {
        println!("  and {} more in the report", why.len() - 6);
    }
    println!(
        "dreaming works on notes that the window's sessions touched or cited (the first run's window is the last 24 hours); \
         sessions indexed before the notes folder existed get records with hippo ingest --full"
    );
}

fn dream(args: &Args) -> Result<RunSummary> {
    let (config, workspace) = super::load()?;
    Ok(dream::run(
        &config,
        &workspace,
        args.pass.as_deref(),
        args.dry_run,
    )?)
}
