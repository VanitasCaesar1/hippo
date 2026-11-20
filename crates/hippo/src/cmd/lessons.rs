//! `hippo lessons`: corrections and repeated failures proposed as rules.
//! `hippo lessons [--json] [--all]` lists them;
//! `scan` runs the pass; `keep` and `dismiss` decide one; `propose` makes
//! one from an Ask answer.

use std::io::{self, Write};
use std::process::ExitCode;

use anyhow::{Context as _, Result};
use hippo_core::index::Index;
use hippo_core::lessons::{self, detect, draft, Lesson, State, Status};
use hippo_core::rollup::Rollup;
use serde_json::json;
use time::OffsetDateTime;
use ulid::Ulid;

use super::{load, load_for_write};

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
    /// Place every correction in your history into groups once: prints the estimate, runs with --yes
    Backfill {
        /// The most match calls this run makes; run it again to go on
        #[arg(long, default_value_t = 200)]
        max_calls: usize,
        /// Run it (without, only the estimate is printed)
        #[arg(long)]
        yes: bool,
        /// Print one JSON object
        #[arg(long)]
        json: bool,
    },
    /// Keep a pending rule: a workspace rule goes into its note (a receipt; hippo revert undoes it), a machine rule stays in hippo's home
    Keep {
        id: String,
        /// The rule's text, edited
        #[arg(long)]
        text: Option<String>,
        /// The rule's title, edited
        #[arg(long)]
        title: Option<String>,
        /// Write a machine rule into its target note too, as a workspace rule
        #[arg(long)]
        note: bool,
    },
    /// The agent memories hippo can read rules from, and whether each is on
    Sources {
        /// Turn a source on: hippo reads its rule lines at each scan and sends them to your lessons runner
        #[arg(long, value_name = "ID")]
        on: Option<String>,
        /// Turn a source off
        #[arg(long, value_name = "ID", conflicts_with = "on")]
        off: Option<String>,
        /// Print one JSON object
        #[arg(long)]
        json: bool,
    },
    /// Dismiss a pending lesson; it returns only if it recurs in 3 more sessions
    Dismiss {
        id: String,
        /// For a shared lesson: wrong, duplicate (with --of) or not-for-us
        #[arg(long)]
        reason: Option<String>,
        /// With --reason duplicate: the shared lesson this one repeats (L-…)
        #[arg(long)]
        of: Option<String>,
    },
    /// Share your lessons with the team's Inbox: shows each file first and asks (you, at a terminal)
    Share,
    /// Add the lesson marker to bullets kept before 0.6.0; prints each, writes with --yes (hippo revert undoes it)
    Markers {
        #[arg(long)]
        yes: bool,
    },
    /// Propose a rule (or with --note, a new note) from an Ask answer
    Propose {
        #[arg(long, required_unless_present = "miss")]
        question: Option<String>,
        #[arg(long, required_unless_present = "miss")]
        answer: Option<String>,
        /// A search of the notes that found nothing, as `<session>:<turn>`
        /// (from `hippo xray`): a rule saying where it is found
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
        Some(Command::Backfill {
            max_calls,
            yes,
            json,
        }) => backfill(max_calls, yes, json),
        Some(Command::Keep {
            id,
            text,
            title,
            note,
        }) => {
            if id.starts_with("L-") {
                let (cfg, ws) = load_for_write()?;
                let kept = lessons::keep_shared(
                    &cfg,
                    &ws,
                    &id,
                    title.as_deref(),
                    text.as_deref(),
                    OffsetDateTime::now_utc(),
                )?;
                println!(
                    "{id}: kept in {} (receipt {}; hippo revert {} undoes it)",
                    kept.proposal.note.display(),
                    kept.receipt.id,
                    kept.receipt.id
                );
                return Ok(ExitCode::SUCCESS);
            }
            let cfg = hippo_core::config::Config::load()?;
            let ws = match cfg.workspace {
                Some(_) => Some(load_for_write()?.1),
                None => None,
            };
            let kept = lessons::keep_rule(
                &cfg,
                ws.as_ref(),
                parse_id(&id)?,
                title.as_deref(),
                text.as_deref(),
                note,
                OffsetDateTime::now_utc(),
            )?;
            match &kept.note {
                Some(path) => println!(
                    "{id}: kept in {} (receipt {}; hippo revert {} undoes it)",
                    path.display(),
                    kept.receipt,
                    kept.receipt
                ),
                None => println!(
                    "{id}: kept as a machine rule: every agent session on this machine gets it (receipt {})",
                    kept.receipt
                ),
            }
            for line in &kept.replaced {
                println!("  {line}");
            }
            Ok(ExitCode::SUCCESS)
        }
        Some(Command::Sources { on, off, json }) => sources(on, off, json),
        Some(Command::Dismiss { id, reason, of }) if id.starts_with("L-") => {
            let (cfg, ws) = load_for_write()?;
            lessons::dismiss_shared(
                &cfg,
                &ws,
                &id,
                reason.as_deref(),
                of.as_deref(),
                OffsetDateTime::now_utc(),
            )?;
            println!(
                "{id}: dismissed{}; your decision is in .hippo/inbox/{id}/ for the team to pull",
                reason.map(|r| format!(" as {r}")).unwrap_or_default()
            );
            Ok(ExitCode::SUCCESS)
        }
        Some(Command::Share) => share(),
        Some(Command::Markers { yes }) => markers(yes),
        Some(Command::Dismiss { id, .. }) => {
            let (cfg, _) = load_for_write()?;
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

/// `hippo lessons share`: the person's consent, at a terminal.
fn share() -> Result<ExitCode> {
    use hippo_core::lessons::team;
    let (mut cfg, ws) = load_for_write()?;
    if !team::enabled(&ws) {
        anyhow::bail!(
            "the shared Inbox is not on for this workspace on this machine: [team] inbox = \"shared\" in .hippo/hippo.toml, accepted with hippo accept"
        );
    }
    if let Some(why) = hippo_core::accept::person_refusal("hippo lessons share") {
        anyhow::bail!(why);
    }
    let member = match cfg.member.clone() {
        Some(m) => m,
        None => {
            let suggested = team::suggested_handle(&ws.root);
            print!(
                "Your handle in the team's files (letters, digits, - and _){}: ",
                suggested
                    .as_deref()
                    .map(|s| format!(" [{s}]"))
                    .unwrap_or_default()
            );
            io::stdout().flush()?;
            let mut line = String::new();
            io::stdin().read_line(&mut line)?;
            let line = line.trim();
            let handle = if line.is_empty() {
                suggested.unwrap_or_default()
            } else {
                line.to_owned()
            };
            if !hippo_core::settings::valid_handle(&handle) {
                anyhow::bail!("{handle:?} is not a handle: letters, digits, - and _, at most 32");
            }
            handle
        }
    };
    let lessons = lessons::shareable(&cfg, &ws)?;
    println!("{}", team::consent(&member));
    if lessons.is_empty() {
        println!("No lesson of yours can be shared yet; later ones are shared by each scan once you say yes.");
    }
    for l in &lessons {
        for path in team::planned_files(&ws, &member, l) {
            println!(
                "  {}",
                path.strip_prefix(&ws.root).unwrap_or(&path).display()
            );
        }
        println!("      {}: {}", l.title, l.rule);
    }
    print!("{} [y/N] ", team::CONSENT_QUESTION);
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    if !matches!(answer.trim(), "y" | "Y" | "yes") {
        println!("nothing shared");
        return Ok(ExitCode::SUCCESS);
    }
    let dirs =
        lessons::share_with_consent(&mut cfg, &ws, &member, &lessons, OffsetDateTime::now_utc())?;
    println!(
        "shared {} lessons; share_lessons = \"rules\" and member = \"{member}\" are in config.toml. Read git status before you commit.",
        dirs.len()
    );
    Ok(ExitCode::SUCCESS)
}

fn markers(yes: bool) -> Result<ExitCode> {
    let (cfg, ws) = load_for_write()?;
    let plans = lessons::plan_markers(&cfg, &ws)?;
    if plans.is_empty() {
        println!("every kept lesson's bullet has its marker");
        return Ok(ExitCode::SUCCESS);
    }
    for p in &plans {
        println!("{}", p.note.display());
        println!("- {}", p.old.trim_end());
        println!("+ {}", p.new.trim_end());
    }
    if !yes {
        println!("nothing written; add --yes to write these (each gets a receipt; hippo revert undoes it)");
        return Ok(ExitCode::SUCCESS);
    }
    let receipts = lessons::write_markers(&ws, &plans, OffsetDateTime::now_utc())?;
    println!("{} markers written", receipts.len());
    Ok(ExitCode::SUCCESS)
}

/// The shared Inbox, as `hippo lessons` lists it: one entry per group,
/// who saw it, and the decisions as written, beside each file's committer.
fn list_shared(out: &mut impl Write, ws: &hippo_core::config::Workspace) -> Result<()> {
    use hippo_core::lessons::team;
    for e in team::entries(ws) {
        let seen = e.seen_words();
        writeln!(
            out,
            "{} {:<10} seen by {:<20} {}",
            e.dirs[0],
            e.kind.as_str(),
            if seen.is_empty() {
                "-".to_owned()
            } else {
                seen
            },
            e.title
        )?;
        writeln!(out, "    {}", e.rule)?;
        writeln!(
            out,
            "    reviewer: {} (as written in lesson-{}.json{})",
            e.verdict,
            e.worded_by,
            e.worded_committer
                .as_deref()
                .map(|c| format!(", pushed by {c}"))
                .unwrap_or_default()
        )?;
        for (member, title) in &e.also {
            writeln!(out, "    also worded by {member}: {title}")?;
        }
        for d in &e.decisions {
            let verdict = match d.verdict {
                team::Verdict::Keep => "kept".to_owned(),
                team::Verdict::Dismiss => format!(
                    "dismissed{}",
                    d.reason
                        .as_deref()
                        .map(|r| format!(" as {r}"))
                        .unwrap_or_default()
                ),
            };
            let by = match &d.committer {
                Some(c) => format!("{} (pushed by {c})", d.by),
                None => format!("{} (not committed yet)", d.by),
            };
            writeln!(out, "    {verdict} by {by} {}", d.at.date())?;
        }
        if e.state == team::State::Crossed {
            writeln!(out, "    crossed: kept and dismissed; talk it over")?;
        }
    }
    Ok(())
}

fn list(as_json: bool, all: bool) -> Result<ExitCode> {
    if !as_json {
        if let Ok((_, ws)) = load() {
            if hippo_core::lessons::team::enabled(&ws) {
                let mut out = io::stdout().lock();
                writeln!(out, "Shared Inbox (.hippo/inbox/):")?;
                list_shared(&mut out, &ws)?;
                writeln!(out, "Yours:")?;
            }
        }
    }
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

fn sources(on: Option<String>, off: Option<String>, as_json: bool) -> Result<ExitCode> {
    use hippo_core::lessons::memory;
    use hippo_core::settings::{self, Op};
    let mut cfg = hippo_core::config::Config::load()?;
    let found = memory::discover(&memory::Places::here());
    for (id, turn_on) in on
        .iter()
        .map(|i| (i, true))
        .chain(off.iter().map(|i| (i, false)))
    {
        if !found.iter().any(|s| &s.id == id) {
            anyhow::bail!("no source {id} on this machine; `hippo lessons sources` lists them");
        }
        let (add, remove) = if turn_on {
            ("memory_sources", "memory_sources_off")
        } else {
            ("memory_sources_off", "memory_sources")
        };
        for (key, op) in [(remove, Op::Remove(id.clone())), (add, Op::Add(id.clone()))] {
            let listed = match key {
                "memory_sources" => cfg.remember.memory_sources.contains(id),
                _ => cfg.remember.memory_sources_off.contains(id),
            };
            let wanted = matches!(op, Op::Add(_));
            if listed != wanted {
                settings::apply(&settings::plan(&cfg.home, key, &op)?)?;
            }
        }
        cfg = hippo_core::config::Config::load()?;
        println!(
            "{id}: {}",
            if turn_on {
                "on; the next scan reads its rule lines"
            } else {
                "off"
            }
        );
    }
    if on.is_some() || off.is_some() {
        return Ok(ExitCode::SUCCESS);
    }
    let domain = hippo_core::runners::plan(&cfg).ok().and_then(|p| {
        p.runner(hippo_core::runners::Role::Lessons)
            .map(|r| r.data_domain.clone())
    });
    if as_json {
        let rows: Vec<serde_json::Value> = found
            .iter()
            .map(|s| {
                json!({
                    "id": s.id,
                    "tool": s.tool,
                    "account": s.account,
                    "files": s.files.len(),
                    "kind": s.kind,
                    "state": memory::state(&cfg, &s.id),
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string(&json!({"sources": rows, "sent_to": domain}))?
        );
        return Ok(ExitCode::SUCCESS);
    }
    if found.is_empty() {
        println!("no agent memories found on this machine");
        return Ok(ExitCode::SUCCESS);
    }
    for s in &found {
        let what = match s.kind {
            memory::Kind::Rules => "rules file",
            memory::Kind::FeedbackMemory => "feedback memories, written by agents",
        };
        println!(
            "{:<10} {}  ({}, {} file{})",
            memory::state(&cfg, &s.id).as_str(),
            s.id,
            what,
            s.files.len(),
            if s.files.len() == 1 { "" } else { "s" }
        );
    }
    println!(
        "A source that is on is read at each scan, read-only; its rule lines go to your lessons runner{} and each is checked by a reviewer before it waits in the Inbox.",
        domain.map(|d| format!(" ({d})")).unwrap_or_default()
    );
    println!("Turn one on with `hippo lessons sources --on <id>`, in a terminal: an agent cannot.");
    Ok(ExitCode::SUCCESS)
}

fn backfill(max_calls: usize, yes: bool, as_json: bool) -> Result<ExitCode> {
    let (cfg, ws) = load_for_write()?;
    let estimate = lessons::backfill_estimate(&cfg)?;
    if !yes {
        if as_json {
            println!(
                "{}",
                serde_json::to_string(&json!({ "estimate": estimate }))?
            );
        } else {
            let e = &estimate;
            println!(
                "{} corrections in your history are not in a group yet: the pre-filter places {}, {} need the model.",
                e.corrections, e.local, e.need_model
            );
            println!(
                "That is {} match calls (at most {} this run), then at most {} drafting calls and {} reviews.",
                e.match_calls,
                max_calls.min(e.match_calls),
                e.draft_calls_max,
                e.reviews_max
            );
            println!("Run `hippo lessons backfill --yes` to place them.");
        }
        return Ok(ExitCode::SUCCESS);
    }
    let scanned = lessons::scan_with(
        &cfg,
        &ws,
        &lessons::ScanOptions {
            days: 7,
            dry_run: false,
            backfill: true,
            max_calls,
            places: lessons::memory::Places::here(),
        },
    )?;
    let s = &scanned.state;
    if as_json {
        println!(
            "{}",
            serde_json::to_string(&json!({
                "estimate": estimate,
                "state": s,
                "confirmed": scanned.confirmed,
                "notes": scanned.notes,
            }))?
        );
        return Ok(ExitCode::SUCCESS);
    }
    println!(
        "placed {} corrections with {} match calls; {} drafted, {} confirmed into the Inbox; {} model calls in all",
        s.placed, s.match_calls, s.drafted, s.confirmed, s.model_calls
    );
    if s.left > 0 {
        println!(
            "{} corrections are left: run `hippo lessons backfill --yes` again to go on",
            s.left
        );
    }
    for note in &scanned.notes {
        println!("  {note}");
    }
    Ok(ExitCode::SUCCESS)
}

fn scan(days: u32, dry_run: bool, as_json: bool) -> Result<ExitCode> {
    let (cfg, ws) = load_for_write()?;
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
    let rollup = Rollup::open(&cfg.home)?.context("no rollup; run hippo ingest")?;
    let now = OffsetDateTime::now_utc();
    let since = now - time::Duration::days(i64::from(days.clamp(1, 30)));
    let found = detect::find(
        &index,
        &rollup,
        since,
        hippo_core::usage::local_offset_at(now),
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
