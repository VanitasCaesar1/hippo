//! `hippocamp recall <query> [--limit N] [--json] [--raw]`: one line per turn,
//! `<session> <tool> <ts> <role> [<tool_name>] <snippet>`, or with `--json`
//! one object per line, which also carries the turn's `ordinal`. The text
//! output does not print it.

use std::io::{self, Write};
use std::process::ExitCode;

use anyhow::Result;
use hippocamp_core::config::Config;
use hippocamp_core::index::{Index, MAX_LIMIT};
use serde_json::json;

use super::one_line;

#[derive(clap::Args)]
pub struct Args {
    /// What to search for, as a phrase (or FTS5 syntax with --raw)
    query: String,
    /// The most turns to print, at most 200
    #[arg(long, default_value_t = 20)]
    limit: usize,
    /// Print one JSON object per line
    #[arg(long)]
    json: bool,
    /// Pass the query to the index as written
    #[arg(long)]
    raw: bool,
}

pub fn run(args: Args) -> Result<ExitCode> {
    let config = Config::load()?;
    let index = Index::open(&config.home, false)?;
    let hits = index.recall(&args.query, args.limit.min(MAX_LIMIT), args.raw)?;
    let mut out = io::stdout().lock();
    if hits.is_empty() && !args.json {
        // An empty result names the next step.
        writeln!(out, "no turn matches \"{}\"", one_line(&args.query))?;
        writeln!(
            out,
            "try fewer or other words, or rank whole sessions:  hippocamp ask --sources-only \"<question>\""
        )?;
        return Ok(ExitCode::SUCCESS);
    }
    for hit in hits {
        let snippet = one_line(&hit.snippet);
        if args.json {
            let line = json!({
                "session": hit.session,
                "tool": hit.tool,
                "ordinal": hit.ordinal,
                "ts": hit.ts,
                "role": hit.role,
                "tool_name": hit.tool_name,
                "snippet": snippet,
                "score": hit.score,
            });
            writeln!(out, "{line}")?;
        } else {
            let tool_name = hit
                .tool_name
                .map(|name| format!(" {name}"))
                .unwrap_or_default();
            writeln!(
                out,
                "{} {} {} {}{tool_name} {snippet}",
                hit.session, hit.tool, hit.ts, hit.role
            )?;
        }
    }
    Ok(ExitCode::SUCCESS)
}
