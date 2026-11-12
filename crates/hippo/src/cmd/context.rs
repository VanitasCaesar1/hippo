//! `hippo context [--task <text>] [--session <id>] [--budget <bytes>]
//! [--parts required|optional|all]`: prints the block; each optional part
//! that did not fit goes to stderr as `omitted: <part> (<bytes>)`. With
//! `--json`: one line, `text`, `bytes`, `budget`, `omitted`, `required` and
//! `relevant`.

use std::process::ExitCode;

use anyhow::{anyhow, Result};
use hippo_core::context::{self, PartSelection};
use hippo_core::sessions;

#[derive(clap::Args)]
pub struct Args {
    /// The task, for the relevant notes
    #[arg(long)]
    task: Option<String>,
    /// A session id, for the session line
    #[arg(long)]
    session: Option<String>,
    /// The budget in bytes; the workspace's budget_bytes when not given
    #[arg(long)]
    budget: Option<usize>,
    /// Which parts to compile
    #[arg(long, value_enum, default_value_t = Parts::All)]
    parts: Parts,
    /// Print one JSON line instead of the block
    #[arg(long)]
    json: bool,
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum Parts {
    Required,
    Optional,
    All,
}

pub fn run(args: Args) -> Result<ExitCode> {
    let (_, workspace) = super::load()?;
    let session = match &args.session {
        Some(id) => Some(sessions::get(&workspace, id)?.ok_or_else(|| anyhow!("no session {id}"))?),
        None => None,
    };
    let selection = match args.parts {
        Parts::Required => PartSelection::Required,
        Parts::Optional => PartSelection::Optional,
        Parts::All => PartSelection::All,
    };
    let budget = args.budget.unwrap_or(workspace.budget_bytes);
    // The repository this command runs in lifts the notes that name it.
    let repo = std::env::current_dir()
        .ok()
        .and_then(|dir| context::rank::repo_name(&dir, &workspace.root));
    let compiled = context::compile(
        &workspace,
        None,
        args.task.as_deref(),
        repo.as_deref(),
        session.as_ref(),
        budget,
        selection,
    )?;
    if args.json {
        let paths = |paths: &[std::path::PathBuf]| -> Vec<String> {
            paths.iter().map(|p| p.display().to_string()).collect()
        };
        let omitted: Vec<_> = compiled
            .omitted
            .iter()
            .map(|(part, bytes)| serde_json::json!({"part": part.as_str(), "bytes": bytes}))
            .collect();
        println!(
            "{}",
            serde_json::json!({
                "text": compiled.text(),
                "bytes": compiled.bytes,
                "budget": budget,
                "omitted": omitted,
                "required": paths(&compiled.required),
                "relevant": paths(&compiled.relevant),
            })
        );
        return Ok(ExitCode::SUCCESS);
    }
    print!("{}", compiled.text());
    for line in compiled.omitted_lines() {
        eprintln!("{line}");
    }
    Ok(ExitCode::SUCCESS)
}
