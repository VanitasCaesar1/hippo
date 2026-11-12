//! `hippocamp config show|set|unset|add|remove`: one setting in `config.toml`
//! or the workspace's `hippocamp.toml` (`hippocamp_core::settings`). Without `--yes` a change is only printed: the file, the
//! line before and the line after.

use std::process::ExitCode;

use anyhow::Result;
use hippocamp_core::config;
use hippocamp_core::settings::{self, Op};

#[derive(clap::Subcommand)]
pub enum Command {
    /// Print the settings the app's Manage reads
    Show {
        /// Print one JSON object
        #[arg(long)]
        json: bool,
    },
    /// Set a key: ingest_interval_minutes, dream_at, budget_bytes, lessons (on|off), snapshot_keep_days, model_command (a JSON array or words)
    Set {
        key: String,
        value: String,
        /// Write the change; without it, only print it
        #[arg(long)]
        yes: bool,
    },
    /// Remove a key's line, so its default applies
    Unset {
        key: String,
        #[arg(long)]
        yes: bool,
    },
    /// Add a value to a list: transcript_roots, exclude_roots, required_notes, passes_off
    Add {
        key: String,
        value: String,
        #[arg(long)]
        yes: bool,
    },
    /// Remove a value from a list
    Remove {
        key: String,
        value: String,
        #[arg(long)]
        yes: bool,
    },
}

pub fn run(command: Command) -> Result<ExitCode> {
    let home = config::home();
    let (key, op, yes) = match command {
        Command::Show { json } => {
            let shown = settings::show(&home)?;
            if json {
                println!("{}", serde_json::to_string(&shown)?);
            } else {
                print_shown(&shown);
            }
            return Ok(ExitCode::SUCCESS);
        }
        Command::Set { key, value, yes } => (key, Op::Set(value), yes),
        Command::Unset { key, yes } => (key, Op::Unset, yes),
        Command::Add { key, value, yes } => (key, Op::Add(value), yes),
        Command::Remove { key, value, yes } => (key, Op::Remove(value), yes),
    };
    let plan = settings::plan(&home, &key, &op)?;
    print!("{}", plan.diff());
    if !plan.changed {
        return Ok(ExitCode::SUCCESS);
    }
    if yes {
        settings::apply(&plan)?;
        println!("written");
        if matches!(key.as_str(), "ingest_interval_minutes" | "dream_at") {
            println!("run hippocamp schedule install for the launchd jobs to use it");
        }
    } else {
        println!("nothing written; add --yes to write it");
    }
    Ok(ExitCode::SUCCESS)
}

fn print_shown(s: &settings::Shown) {
    println!("config.toml: {}", s.config.display());
    match &s.workspace {
        Some(w) => println!("workspace = {}", w.display()),
        None => println!("workspace = none"),
    }
    println!("transcript_roots = {:?}", s.transcript_roots);
    println!("exclude_roots = {:?}", s.exclude_roots);
    println!("ingest_interval_minutes = {}", s.ingest_interval_minutes);
    println!("dream_at = {}", s.dream_at);
    if s.workspace.is_some() {
        println!("required_notes = {:?}", s.required_notes);
        if let Some(b) = s.budget_bytes {
            println!("budget_bytes = {b}");
        }
        println!("passes_off = {:?}", s.passes_off);
        println!("lessons = {}", s.lessons.unwrap_or(true));
        match s.snapshot_keep_days {
            Some(d) => println!("snapshot_keep_days = {d}"),
            None => println!("snapshot_keep_days = keep all"),
        }
    }
}
