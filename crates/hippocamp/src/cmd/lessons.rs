//! `hippocamp lessons`: corrections and repeated failures proposed as rules.
//! `hippocamp lessons [--json] [--all]` lists them;
//! `scan` runs the pass; `keep` and `dismiss` decide one; `propose` makes
//! one from an Ask answer.

use std::io::{self, Write};
use std::process::ExitCode;

use anyhow::{Context as _, Result};
use hippocamp_core::index::Index;
use hippocamp_core::lessons::{self, detect, draft, Lesson, State, Status};
use hippocamp_core::rollup::Rollup;
use serde_json::json;
use time::OffsetDateTime;
use ulid::Ulid;

use super::load;

#[derive(clap::Args)]
#[command(args_conflicts_with_subcommands = true)]
pub struct Args {
    #[command(subcommand)]
    command: Option<Command>,
    /// Print one JSON object
    #[arg(long)]
    json: bool,
    /// Every lesson, not only those waiting in the Inbox
    #[arg(long)]
    all: bool,
}

#[derive(clap::Subcommand)]
pub enum Command {
    /// Run the lessons pass: find candidates, draft rules, review them
    Scan {
        /// Days to look back (1 to 30)
        #[arg(long, default_value_t = 7)]
        days: u32,
        /// Draft and review, but write no record
        #[arg(long)]
        dry_run: bool,
        /// Only list what the pass would draft: no model call, no write
        #[arg(long)]
        candidates: bool,
        /// Print one JSON object
        #[arg(long)]
        json: bool,
    },
    /// Write a pending lesson into its note (a receipt; hippocamp revert undoes it)
    Keep {
        id: String,
        /// The rule's text, edited
        #[arg(long)]
        text: Option<String>,
        /// The rule's title, edited
        #[arg(long)]
        title: Option<String>,
    },
    /// Dismiss a pending lesson; it returns only if it recurs in 3 more sessions
    Dismiss { id: String },
    /// Propose a rule (or with --note, a new note) from an Ask answer
    Propose {
        #[arg(long, required_unless_present = "miss")]
        question: Option<String>,
        #[arg(long, required_unless_present = "miss")]
        answer: Option<String>,
        /// A search of the notes that found nothing, as `<session>:<turn>`
        /// (from `hippocamp xray`): a rule saying where it is found
        #[arg(long, conflicts_with_all = ["question", "answer", "note"])]
        miss: Option<String>,
        /// A new note instead of a rule
        #[arg(long)]
        note: bool,
        /// Print one JSON object
        #[arg(long)]
        json: bool,
    },
}

fn parse_id(text: &str) -> Result<Ulid> {
    text.parse()
        .with_context(|| format!("not a lesson id: {text}"))
}

pub fn run(args: Args) -> Result<ExitCode> {
    match args.command {
        None => list(args.json, args.all),
        Some(Command::Scan {
            days,
            dry_run,
            candidates,
            json,
        }) => {
            if candidates {
                return list_candidates(days, json);
            }
            scan(days, dry_run, json)
        }
        Some(Command::Keep { id, text, title }) => {
            let (cfg, ws) = load()?;
            let id = parse_id(&id)?;
            let kept = lessons::keep(
                &cfg,
                &ws,
                id,
                title.as_deref(),
                text.as_deref(),
                OffsetDateTime::now_utc(),
            )?;
            println!(
                "{id}: kept in {} (receipt {}; hippocamp revert {} undoes it)",
                kept.proposal.note.display(),
                kept.receipt.id,
                kept.receipt.id
            );
            Ok(ExitCode::SUCCESS)
        }
        Some(Command::Dismiss { id }) => {
            let (cfg, _) = load()?;
            let id = parse_id(&id)?;
            let l = lessons::dismiss(&cfg, id, OffsetDateTime::now_utc())?;
            println!(
                "{id}: dismissed; proposed again only if seen in {} sessions",
                l.seen.sessions + lessons::REOPEN_AFTER
            );
            Ok(ExitCode::SUCCESS)
        }
        Some(Command::Propose {
            question,
            answer,
            miss,
            note,
            json,
        }) => {
            let (cfg, ws) = load()?;
            let lesson = match miss {
                Some(m) => {
                    let (session, turn) = m
                        .rsplit_once(':')
                        .and_then(|(s, t)| Some((s.to_owned(), t.parse::<u32>().ok()?)))
                        .ok_or_else(|| anyhow::anyhow!("--miss takes <session>:<turn>"))?;
                    draft::propose_miss(&cfg, &ws, &session, turn)?
                }
                None => draft::propose(
                    &cfg,
                    &ws,
                    question.as_deref().unwrap_or_default(),
                    answer.as_deref().unwrap_or_default(),
                    note,
                )?,
            };
            if json {
                println!("{}", serde_json::to_string(&lesson)?);
            } else if lesson.status == Status::Pending {
                println!(
                    "Sent to the Inbox for you to keep: {} ({})",
                    lesson.title, lesson.id
                );
            } else {
                println!(
                    "The reviewer rejected it ({}): {}",
                    lesson.id,
                    lesson
                        .review
                        .as_ref()
                        .map(|r| r.notes.as_str())
                        .unwrap_or("")
                );
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn list(as_json: bool, all: bool) -> Result<ExitCode> {
    let (cfg, _) = load()?;
    let items: Vec<Lesson> = if all {
        lessons::list(&cfg.home)?
    } else {
        lessons::pending(&cfg.home)?
    };
    let state = State::load(&cfg.home);
    let mut out = io::stdout().lock();
    if as_json {
        writeln!(
            out,
            "{}",
            serde_json::to_string(&json!({ "lessons": items, "last_scan": state }))?
        )?;
        return Ok(ExitCode::SUCCESS);
    }
    if items.is_empty() {
        writeln!(out, "no lessons waiting")?;
    }
    for l in &items {
        let target = l.target.as_ref().map_or(String::new(), |t| {
            let section = t
                .section
                .as_deref()
                .map(|s| format!(" › {s}"))
                .unwrap_or_default();
            format!(
                " → {}{}{section}",
                if t.new { "new note " } else { "" },
                t.note.display()
            )
        });
        writeln!(
            out,
            "{} {:<10} {:<9} seen in {} sessions: {}{target}",
            l.id,
            l.kind.as_str(),
            serde_json::to_value(l.status)?.as_str().unwrap_or(""),
            l.seen.sessions,
            l.title
        )?;
        writeln!(out, "    {}", l.rule)?;
    }
    if let Some(s) = state {
        writeln!(
            out,
            "last scan: {} · {} sessions read · {} model calls{}",
            s.at.map(|t| t.to_string()).unwrap_or_default(),
            s.sessions,
            s.model_calls,
            s.error
                .map(|e| format!(" · failed: {e}"))
                .unwrap_or_default()
        )?;
    }
    Ok(ExitCode::SUCCESS)
}

fn scan(days: u32, dry_run: bool, as_json: bool) -> Result<ExitCode> {
    let (cfg, ws) = load()?;
    let result = lessons::scan(&cfg, &ws, days, dry_run);
    let scanned = match result {
        Ok(s) => s,
        Err(err) => {
            if !dry_run {
                let state = State {
                    at: Some(OffsetDateTime::now_utc()),
                    days,
                    error: Some(err.to_string()),
                    ..State::default()
                };
                let _ = state.save(&cfg.home);
            }
            return Err(err.into());
        }
    };
    if as_json {
        println!(
            "{}",
            serde_json::to_string(&json!({
                "state": scanned.state,
                "confirmed": scanned.confirmed,
                "notes": scanned.notes,
            }))?
        );
        return Ok(ExitCode::SUCCESS);
    }
    let s = &scanned.state;
    println!(
        "read {} sessions over {} days: {} candidates, {} drafted, {} confirmed into the Inbox, {} model calls{}",
        s.sessions,
        s.days,
        s.candidates,
        s.drafted,
        s.confirmed,
        s.model_calls,
        if dry_run { " (dry run: nothing written)" } else { "" }
    );
    for l in &scanned.confirmed {
        println!("  + {} {}", l.kind.as_str(), l.title);
    }
    for n in &scanned.notes {
        println!("  ! {n}");
    }
    // Every drafting call failed: a failure, not
    // an empty Inbox.
    let failed = scanned
        .notes
        .iter()
        .filter(|n| n.starts_with("drafting failed"))
        .count();
    if s.drafted == 0 && failed > 0 && failed == s.model_calls {
        eprintln!(
            "every drafting call failed ({failed}); the first: {}",
            scanned
                .notes
                .iter()
                .find(|n| n.starts_with("drafting failed"))
                .map_or("", String::as_str)
        );
        return Ok(ExitCode::FAILURE);
    }
    Ok(ExitCode::SUCCESS)
}

fn list_candidates(days: u32, as_json: bool) -> Result<ExitCode> {
    let (cfg, _) = load()?;
    let index = Index::open(&cfg.home, false)?;
    let rollup = Rollup::open(&cfg.home)?.context("no rollup; run hippocamp ingest")?;
    let now = OffsetDateTime::now_utc();
    let since = now - time::Duration::days(i64::from(days.clamp(1, 30)));
    let found = detect::find(
        &index,
        &rollup,
        since,
        hippocamp_core::usage::local_offset_at(now),
    )?;
    let show = |c: &detect::Cluster| {
        json!({
            "kind": c.kind.as_str(),
            "key": c.key,
            "sessions": c.sessions.len(),
            "profiles": c.profiles,
            "most_in_a_day": c.most_in_a_day,
            "evidence": c.evidence.iter().map(|e| json!({"at": e.at.to_string(), "profile": e.profile, "quote": e.quote, "action": e.action})).collect::<Vec<_>>(),
        })
    };
    if as_json {
        println!(
            "{}",
            serde_json::to_string(&json!({
                "sessions": found.sessions,
                "corrections": found.corrections.iter().map(show).collect::<Vec<_>>(),
                "failures": found.failures.iter().map(show).collect::<Vec<_>>(),
            }))?
        );
        return Ok(ExitCode::SUCCESS);
    }
    println!(
        "{} sessions: {} correction clusters, {} repeated failures (no model call made)",
        found.sessions,
        found.corrections.len(),
        found.failures.len()
    );
    for c in found.corrections.iter().chain(&found.failures) {
        println!(
            "{} ×{} [{}] {}",
            c.kind.as_str(),
            c.sessions.len(),
            c.key,
            c.evidence.first().map(|e| e.quote.as_str()).unwrap_or("")
        );
    }
    Ok(ExitCode::SUCCESS)
}
