//! `hippocamp ingest [--full] [--progress]`: prints `indexed <n> files, skipped
//! <m>, derived <k> sessions`, or `ingest already running` and exits 0.
//! With `--progress`, one JSON object per file goes to stderr as it is read
//! (`hippocamp_core::ingest::Progress`).

use std::io::Write as _;
use std::process::ExitCode;

use anyhow::Result;
use hippocamp_core::ingest;

#[derive(clap::Args)]
pub struct Args {
    /// Build a new index from every transcript and swap it in
    #[arg(long)]
    full: bool,
    /// Write one JSON line per file to stderr as it is read
    #[arg(long)]
    progress: bool,
}

pub fn run(args: Args) -> Result<ExitCode> {
    let (config, workspace) = super::load_history()?;
    let summary = if args.progress {
        let mut stderr = std::io::stderr().lock();
        ingest::ingest_with_progress(&config, workspace.as_ref(), args.full, &mut |p| {
            if let Ok(line) = serde_json::to_string(p) {
                let _ = writeln!(stderr, "{line}");
            }
        })?
    } else {
        ingest::ingest(&config, workspace.as_ref(), args.full)?
    };
    println!("{summary}");
    if let ingest::IngestSummary::Done {
        indexed: 0,
        skipped: 0,
        ..
    } = summary
    {
        // Nothing found is not silent.
        println!(
            "no transcript found under the transcript roots; hippocamp detect shows where hippocamp looks"
        );
    }
    Ok(ExitCode::SUCCESS)
}
