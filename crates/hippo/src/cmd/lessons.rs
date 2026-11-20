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
    /// Write a pending lesson into its note (a receipt; hippo revert undoes it)
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
        Some(Command::Keep { id, text, title }) => {
            let (cfg, ws) = load_for_write()?;
            let kept = if id.starts_with("L-") {
                lessons::keep_shared(
                    &cfg,
                    &ws,
                    &id,
                    title.as_deref(),
                    text.as_deref(),
                    OffsetDateTime::now_utc(),
                )?
            } else {
                lessons::keep(
                    &cfg,
                    &ws,
                    parse_id(&id)?,
                    title.as_deref(),
                    text.as_deref(),
                    OffsetDateTime::now_utc(),
                )?
            };
            println!(
                "{id}: kept in {} (receipt {}; hippo revert {} undoes it)",
                kept.proposal.note.display(),
                kept.receipt.id,
                kept.receipt.id
            );
            Ok(ExitCode::SUCCESS)
        }
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
            let suggested = git_email_handle(&ws.root);
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
    println!(
        "Sharing writes your lessons' titles, rules, match terms and counts into the repository (no quotes, no session ids), under the handle {member}. You commit and push them as usual."
    );
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
    print!("Share these, and later lessons from this workspace? [y/N] ");
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    if !matches!(answer.trim(), "y" | "Y" | "yes") {
        println!("nothing shared");
        return Ok(ExitCode::SUCCESS);
    }
    for (key, value) in [("member", member.as_str()), ("share_lessons", "rules")] {
        let plan = hippo_core::settings::plan(
            &cfg.home,
            key,
            &hippo_core::settings::Op::Set(value.to_owned()),
        )?;
        hippo_core::settings::apply(&plan)?;
    }
    cfg.member = Some(member.clone());
    cfg.share_lessons = Some("rules".into());
    let dirs = lessons::share_lessons(&cfg, &ws, &member, &lessons, OffsetDateTime::now_utc())?;
    println!(
        "shared {} lessons; share_lessons = \"rules\" and member = \"{member}\" are in config.toml. Read git status before you commit.",
        dirs.len()
    );
    Ok(ExitCode::SUCCESS)
}

/// The local part of `git config user.email` in `root`, when it is a
/// valid handle.
fn git_email_handle(root: &std::path::Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["config", "user.email"])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let email = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    let local = email.split('@').next()?.to_owned();
    hippo_core::settings::valid_handle(&local).then_some(local)
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
    let shared = team::read(ws);
    let groups = team::groups(&shared);
    for g in &groups {
        let first = &shared[g[0]];
        let wording = first.wordings.first();
        let seen: Vec<String> = g
            .iter()
            .flat_map(|&i| shared[i].seen.iter())
            .map(|s| format!("{} {}", s.member, s.record.sessions))
            .collect();
        writeln!(
            out,
            "{} {:<10} seen by {:<20} {}",
            first.dir,
            wording.map_or("correction", |w| w.record.kind.as_str()),
            if seen.is_empty() {
                "-".to_owned()
            } else {
                seen.join(", ")
            },
            wording.map_or("", |w| w.record.title.as_str())
        )?;
        if let Some(w) = wording {
            writeln!(out, "    {}", w.record.rule)?;
            writeln!(
                out,
                "    reviewer: {} (as written in lesson-{}.json{})",
                w.record.verdict,
                w.member,
                team::committer(ws, &w.path)
                    .map(|c| format!(", pushed by {c}"))
                    .unwrap_or_default()
            )?;
        }
        for other in g.iter().flat_map(|&i| shared[i].wordings.iter()).skip(1) {
            writeln!(
                out,
                "    also worded by {}: {}",
                other.member, other.record.title
            )?;
        }
        let state = team::state(&shared, g);
        for d in team::decisions(&shared, g) {
            let who = team::committer(ws, &d.path);
            let verdict = match d.record.verdict {
                team::Verdict::Keep => "kept".to_owned(),
                team::Verdict::Dismiss => format!(
                    "dismissed{}",
                    d.record
                        .reason
                        .as_deref()
                        .map(|r| format!(" as {r}"))
                        .unwrap_or_default()
                ),
            };
            let by = match who {
                Some(c) => format!("{} (pushed by {c})", d.record.by),
                None => format!("{} (not committed yet)", d.record.by),
            };
            writeln!(out, "    {verdict} by {by} {}", d.record.at.date())?;
        }
        if state == team::State::Crossed {
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
