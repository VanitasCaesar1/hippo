//! `hippo stats repeats [--weeks N] [--json]`: corrections and repeats per
//! 100 person-typed sessions, by week, and why each repeat of the last 30
//! days happened (`docs/designs/remember.md`, "The metric"). Read-only;
//! `--json` carries counts only, never a quote.

use std::process::ExitCode;

use anyhow::Result;
use hippo_core::config::{Config, Workspace};
use hippo_core::lessons::repeats;
use time::OffsetDateTime;

#[derive(clap::Subcommand)]
pub enum Command {
    /// Repeats of a rule you already gave, per 100 sessions you typed in, by week
    Repeats {
        /// Weeks to show, ending with this one
        #[arg(long, default_value_t = 12, value_parser = clap::value_parser!(u32).range(1..=104))]
        weeks: u32,
        /// Print one JSON object
        #[arg(long)]
        json: bool,
    },
}

pub fn run(command: Command) -> Result<ExitCode> {
    let Command::Repeats { weeks, json } = command;
    let cfg = Config::load()?;
    let ws = match cfg.workspace {
        Some(_) => Workspace::load(&cfg).ok(),
        None => None,
    };
    let report = repeats::from_home(&cfg, ws.as_ref(), weeks as usize, OffsetDateTime::now_utc())?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(ExitCode::SUCCESS);
    }
    let fmt = |v: Option<f64>| v.map_or_else(|| "–".to_owned(), |x| format!("{x:.1}"));
    let (this, before) = report.headline();
    println!(
        "{} repeats per 100 sessions this week; {} four weeks ago",
        fmt(this),
        fmt(before)
    );
    println!();
    println!("week       sessions  corrections  repeats  corrections/100  repeats/100");
    for w in &report.weeks {
        println!(
            "{:<10} {:>8}  {:>11}  {:>7}  {:>15}  {:>11}",
            w.week,
            w.sessions,
            w.corrections,
            w.repeats,
            fmt(w.corrections_per_100),
            fmt(w.repeats_per_100)
        );
    }
    println!();
    let total: usize = report.breakdown.iter().map(|(_, n)| n).sum();
    println!("repeats in the last 30 days: {total}");
    for (cause, n) in &report.breakdown {
        println!("  {:<24} {n}", cause.as_str());
    }
    if !report.backfilled {
        println!();
        println!(
            "Groups reach back to {} only: run `hippo lessons backfill` to place your whole history, and earlier weeks appear.",
            report
                .covered_from
                .map(|c| c.date().to_string())
                .unwrap_or_else(|| "the first scan".to_owned())
        );
    }
    Ok(ExitCode::SUCCESS)
}
