//! `hippocamp packs check <dir>`: validates a pack directory. It reads only
//! that directory: no configuration, no workspace, no recovery, no model.
//! Prints `skipped <path>`, `error <text>` and `warning <text>` lines, then
//! `ok <name> <version>` when there is no error; exits 1 on any error.
//!
//! `list`, `diff`, `update` and `remove` load the configuration and the
//! workspace, finish an interrupted update first, and work on
//! `.hippocamp/packs/`. `diff` makes no model call and writes nothing; `update`
//! and `remove` exit 1 on a refusal.

use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use hippocamp_core::packs::install::{self, Flags, UpdateOptions, UpdatePlan};
use hippocamp_core::packs::{escape, escape_block, files, testing, validate};

use super::{git_options, load_recovered, print_schedule_warning};

#[derive(clap::Subcommand)]
pub enum Command {
    /// Validate a pack directory; exits 1 on any error
    Check {
        /// The pack directory
        dir: PathBuf,
    },
    /// Run a pack's fixture tests against a stub model; exits 1 on any failure
    Test {
        /// The pack directory
        dir: PathBuf,
        /// Run only this case
        #[arg(long)]
        case: Option<String>,
    },
    /// The stub model of `packs test`: replies with a file's text
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
    /// List the installed packs: name, version, state and source
    List,
    /// Print what an update of a pack would change; writes nothing
    Diff {
        /// The pack's name
        name: String,
        /// Read this branch, tag or commit of a git source
        #[arg(long = "ref")]
        git_ref: Option<String>,
        /// The index.json file of a registry pack, in place of the recorded one
        #[arg(long)]
        registry: Option<PathBuf>,
    },
    /// Update a pack from its source, after a review
    Update {
        /// The pack's name
        name: String,
        /// Read this branch, tag or commit of a git source
        #[arg(long = "ref")]
        git_ref: Option<String>,
        /// The index.json file of a registry pack, in place of the recorded one
        #[arg(long)]
        registry: Option<PathBuf>,
        /// Keep this workspace's version of files changed on both sides
        #[arg(long)]
        keep_local: bool,
        /// Accept an update that widens what a pass reads or works on
        #[arg(long)]
        accept_reads: bool,
        /// Accept a registry entry that now names another repository or owner
        #[arg(long)]
        accept_origin: bool,
    },
    /// Delete an installed pack
    Remove {
        /// The pack's name
        name: String,
        /// Remove a pack that has no origin.json
        #[arg(long)]
        force: bool,
    },
}

pub fn run(command: Command) -> Result<ExitCode> {
    match command {
        Command::Check { dir } => check(&dir),
        Command::Test { dir, case } => test(&dir, case.as_deref()),
        Command::StubModel {
            reply,
            log,
            system_prompt,
        } => stub_model(&reply, &log, &system_prompt),
        Command::List => list(),
        Command::Diff {
            name,
            git_ref,
            registry,
        } => diff(&name, git_ref, registry),
        Command::Update {
            name,
            git_ref,
            registry,
            keep_local,
            accept_reads,
            accept_origin,
        } => update(
            &name,
            UpdateOptions { git_ref, registry },
            Flags {
                keep_local,
                accept_reads,
                accept_origin,
            },
        ),
        Command::Remove { name, force } => remove(&name, force),
    }
}

/// `text` with every control, line-separator and bidirectional character
/// written as `\u{..}`, so a file name cannot start a line or move the
/// cursor.
fn escaped(text: &str) -> String {
    escape(text)
}

fn check(dir: &std::path::Path) -> Result<ExitCode> {
    let (pack, skipped) = match files::read_dir(dir) {
        Ok(read) => read,
        Err(err) => {
            println!("error {}", escaped(&err.to_string()));
            return Ok(ExitCode::FAILURE);
        }
    };
    for path in &skipped {
        println!("skipped {}", escaped(path));
    }
    let result = validate::validate(&pack);
    for error in &result.errors {
        println!("error {}", escaped(error));
    }
    for warning in &result.warnings {
        println!("warning {}", escaped(warning));
    }
    if !result.is_ok() {
        return Ok(ExitCode::FAILURE);
    }
    if let Some(manifest) = &result.manifest {
        println!("ok {} {}", manifest.name, manifest.version);
    }
    Ok(ExitCode::SUCCESS)
}

fn stub_model(
    reply: &std::path::Path,
    log: &std::path::Path,
    system_prompt: &str,
) -> Result<ExitCode> {
    let mut stdin = String::new();
    std::io::stdin()
        .read_to_string(&mut stdin)
        .context("reading stdin")?;
    println!(
        "{}",
        testing::stub_model(reply, log, system_prompt, &stdin)?
    );
    Ok(ExitCode::SUCCESS)
}

fn test(dir: &std::path::Path, only: Option<&str>) -> Result<ExitCode> {
    let pack = match files::read_dir(dir) {
        Ok((pack, _skipped)) => pack,
        Err(err) => {
            println!("error {}", escaped(&err.to_string()));
            return Ok(ExitCode::FAILURE);
        }
    };
    let result = validate::validate(&pack);
    for error in &result.errors {
        println!("error {}", escaped(error));
    }
    let Some(manifest) = result.manifest.as_ref().filter(|_| result.is_ok()) else {
        return Ok(ExitCode::FAILURE);
    };
    if let Some(only) = only {
        if !result.cases.iter().any(|(name, _)| name == only) {
            println!("error no case named {}", escaped(only));
            return Ok(ExitCode::FAILURE);
        }
    }
    let exe = std::env::current_exe().context("finding the hippocamp executable")?;
    let stub = [
        exe.to_string_lossy().into_owned(),
        "packs".to_owned(),
        "stub-model".to_owned(),
    ];
    let mut failed = false;
    for (name, case) in &result.cases {
        if only.is_some_and(|only| only != name) {
            continue;
        }
        let label = format!("{}/{name}", manifest.name);
        let failures = match testing::run_case(&pack, name, case, &stub) {
            Ok(outcome) => outcome.failures,
            Err(err) => vec![err.to_string()],
        };
        if failures.is_empty() {
            println!("ok {}", escaped(&label));
        }
        for failure in &failures {
            println!("FAIL {}: {}", escaped(&label), escaped(failure));
        }
        failed |= !failures.is_empty();
    }
    Ok(if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

fn list() -> Result<ExitCode> {
    let (_, workspace) = load_recovered()?;
    for pack in install::list(&workspace)? {
        println!(
            "{} {} {} {}",
            escaped(&pack.name),
            pack.version
                .as_deref()
                .map_or_else(|| "-".to_owned(), escaped),
            pack.state,
            pack.source
                .as_deref()
                .map_or_else(|| "-".to_owned(), escaped)
        );
    }
    Ok(ExitCode::SUCCESS)
}

/// The plan's lines, then its diff.
fn print_plan(plan: &UpdatePlan) {
    for line in plan.lines() {
        println!("{line}");
    }
    print!("{}", escape_block(&plan.diff));
}

fn diff(name: &str, git_ref: Option<String>, registry: Option<PathBuf>) -> Result<ExitCode> {
    let (_, workspace) = load_recovered()?;
    let options = UpdateOptions { git_ref, registry };
    let plan = install::plan_update(&workspace, name, &options, &git_options())?;
    if plan.up_to_date {
        println!("{name} is up to date");
    } else {
        print_plan(&plan);
    }
    Ok(ExitCode::SUCCESS)
}

fn update(name: &str, options: UpdateOptions, flags: Flags) -> Result<ExitCode> {
    let (config, workspace) = load_recovered()?;
    print_schedule_warning();
    let plan = install::plan_update(&workspace, name, &options, &git_options())?;
    let up_to_date = plan.up_to_date;
    if !up_to_date {
        print_plan(&plan);
    }
    let report = install::apply_update(&config, &workspace, plan, flags)?;
    for line in &report.lines {
        println!("{line}");
    }
    Ok(if report.updated || up_to_date {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn remove(name: &str, force: bool) -> Result<ExitCode> {
    let (_, workspace) = load_recovered()?;
    install::remove(&workspace, name, force)?;
    println!("removed .hippocamp/packs/{name}/");
    Ok(ExitCode::SUCCESS)
}
