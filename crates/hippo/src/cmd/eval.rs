//! `hippo eval <set> [--with-model]`: runs the eval suites over a set
//! directory and prints the Markdown report. Exits 1 after printing when a
//! suite failed. Without `--with-model` eval reads no configuration and
//! runs no model; with it, the dreaming suite runs with the `model_command`
//! and `reviewer_command_override` of `$HIPPO_HOME/config.toml`.
//!
//! `hippo eval relevant [--days N] [--holdout F] [--subagents] [--json]`
//! scores the relevant notes against the notes the user's own recent
//! sessions used. Read-only. The plain output holds
//! aggregate numbers only; `--json` adds every case, with its request text,
//! for the user's own inspection.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{anyhow, Context, Result};
use hippo_core::config::Config;
use hippo_core::context::rank::Params;
use hippo_core::eval::{self, relevant, Models};
use hippo_core::index::Index;
use hippo_core::rollup::Rollup;
use time::OffsetDateTime;

#[derive(clap::Args)]
#[command(args_conflicts_with_subcommands = true)]
pub struct Args {
    #[command(subcommand)]
    command: Option<Sub>,
    /// The eval set directory, fixtures/eval in this repository
    set: Option<PathBuf>,
    /// Run the dreaming suite with the configured model
    #[arg(long)]
    with_model: bool,
}

#[derive(clap::Subcommand)]
enum Sub {
    /// Score the relevant notes against what recent sessions used
    Relevant(RelevantArgs),
    /// The remember suite's oracle model: reads a prompt on stdin, answers from the set's answer key
    #[command(hide = true)]
    Oracle {
        #[arg(long)]
        key: PathBuf,
        /// The system prompt
        system: String,
    },
}

#[derive(clap::Args)]
struct RelevantArgs {
    /// Sessions that started in the last N days
    #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(i64).range(1..=365))]
    days: i64,
    /// The most recent share of sessions reported apart, 0 to 1
    #[arg(long, default_value_t = 0.3)]
    holdout: f64,
    /// Count sub-agent sessions too, each with its own first request
    #[arg(long)]
    subagents: bool,
    /// Print every case (with its request text) as JSON
    #[arg(long)]
    json: bool,
    /// Ranking parameters to try, as JSON; unnamed ones keep their default
    #[arg(long, hide = true)]
    params: Option<String>,
}

pub fn run(args: Args) -> Result<ExitCode> {
    match args.command {
        Some(Sub::Relevant(r)) => return run_relevant(r),
        Some(Sub::Oracle { key, system }) => {
            use std::io::Read as _;
            let mut prompt = String::new();
            std::io::stdin().read_to_string(&mut prompt)?;
            let key = eval::remember::Key::load(&key)?;
            println!("{}", eval::remember::oracle(&key, &system, &prompt));
            return Ok(ExitCode::SUCCESS);
        }
        None => {}
    }
    let set = args
        .set
        .ok_or_else(|| anyhow!("name an eval set directory, or `relevant`"))?;
    let models = if args.with_model {
        let config = Config::load()?;
        Some(Models {
            model_command: config.model_command,
            reviewer_command_override: config.reviewer_command_override,
        })
    } else {
        None
    };
    let report = eval::run(&set, models.as_ref())?;
    print!("{}", report.render());
    // The remember suite, when the set has one: offline, with its oracle.
    let key = set.join("remember.toml");
    if key.is_file() {
        let key = std::fs::canonicalize(&key)?;
        let exe = std::env::current_exe()?.display().to_string();
        let oracle = vec![
            exe,
            "eval".to_owned(),
            "oracle".to_owned(),
            "--key".to_owned(),
            key.display().to_string(),
            "{system_prompt}".to_owned(),
        ];
        let loaded = eval::Set::load(&set)?;
        let result = eval::remember::run(
            &key,
            &set.join("notes"),
            &loaded.entry_note.to_string_lossy(),
            oracle,
        )?;
        print!("\n## Remember\n\n{}", result.summary());
    }
    Ok(if report.failed() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

fn run_relevant(args: RelevantArgs) -> Result<ExitCode> {
    if !(0.0..=1.0).contains(&args.holdout) {
        return Err(anyhow!("--holdout is a share from 0 to 1"));
    }
    let params: Params = match &args.params {
        Some(json) => serde_json::from_str(json).context("--params")?,
        None => Params::default(),
    };
    let (config, workspace) = super::load()?;
    let index = Index::open(&config.home, false)?;
    let rollup = Rollup::open(&config.home)?
        .ok_or_else(|| anyhow!("no rollup yet: run `hippo ingest` first"))?;
    let report = relevant::run(
        &workspace,
        &index,
        &rollup,
        &relevant::Options {
            days: args.days,
            holdout: args.holdout,
            subagents: args.subagents,
            params,
            now: OffsetDateTime::now_utc(),
        },
    )?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", report.render());
    }
    Ok(ExitCode::SUCCESS)
}
