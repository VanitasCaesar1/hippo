//! `hippo ask "<question>" [--json] [--sources-only] [--limit N]`: has any
//! agent already solved this? Searches every indexed session and the notes,
//! prints ranked sources and, unless `--sources-only`, one model call's
//! answer that cites them (dropped when its citations do not check out).

use std::io::{self, Write};
use std::process::ExitCode;

use anyhow::Result;
use hippo_core::ask;

use super::{load_history, one_line};

#[derive(clap::Args)]
pub struct Args {
    /// The question, in your own words
    question: String,
    /// Print the result as one JSON object
    #[arg(long)]
    json: bool,
    /// Skip the model: sources only
    #[arg(long)]
    sources_only: bool,
    /// The most sources to list
    #[arg(long, default_value_t = ask::DEFAULT_LIMIT)]
    limit: usize,
}

pub fn run(args: Args) -> Result<ExitCode> {
    let (config, ws) = load_history()?;
    let mut asked = ask::run(&config, ws.as_ref(), &args.question, args.limit)?;
    if !args.sources_only {
        ask::answer(&config, &mut asked);
    }
    let mut out = io::stdout().lock();
    if args.json {
        writeln!(out, "{}", serde_json::to_string(&asked)?)?;
        return Ok(ExitCode::SUCCESS);
    }
    if asked.terms.is_empty() {
        writeln!(out, "nothing to search for in that question")?;
        return Ok(ExitCode::SUCCESS);
    }
    if let Some(a) = &asked.answer {
        writeln!(out, "{}\n", a.text)?;
    } else if let Some(why) = &asked.answer_dropped {
        writeln!(
            out,
            "(no written answer: {why})\n(the ranked sources below need no model; --sources-only skips the answer)\n"
        )?;
    }
    writeln!(
        out,
        "searched {} sessions and {} notes in {:.1} s for: {}",
        asked.searched.sessions,
        asked.searched.notes,
        asked.searched.ms as f64 / 1000.0,
        asked.terms.join(" ")
    )?;
    if asked.sources.is_empty() {
        writeln!(out, "no session or note matched")?;
    }
    for s in &asked.sources {
        let mut facts = vec![s.kind.clone()];
        facts.extend(s.profile.clone());
        facts.extend(s.project.clone());
        facts.extend(
            s.time
                .as_deref()
                .map(|t| t.get(..16).unwrap_or(t).replace('T', " ")),
        );
        writeln!(
            out,
            "\n[{}] {}  ({})",
            s.n,
            one_line(&s.title),
            facts.join(" · ")
        )?;
        writeln!(out, "    {}", one_line(&s.why))?;
        if let Some(r) = &s.resume {
            writeln!(out, "    {r}")?;
        }
        if let Some(p) = &s.path {
            writeln!(out, "    {}", p.display())?;
        }
    }
    Ok(ExitCode::SUCCESS)
}
