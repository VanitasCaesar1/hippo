//! `hippo` alone and `hippo start [--yes] [--add-root <path>]…`: one command from a new install to a useful screen.
//!
//! Without `config.toml` it prints what `hippo detect` finds and offers to
//! index it, history only; a yes writes a history-only `config.toml`,
//! ingests with visible progress and ends on the overview. With
//! `config.toml` it brings the index up to date when it is older than one
//! ingest interval, then prints the overview: the last active day, read from
//! the rollup and the index, no model call.

use std::collections::HashMap;
use std::io::{self, BufRead, IsTerminal, Write};
use std::process::ExitCode;
use std::time::Instant;

use anyhow::Result;
use hippo_core::config::{self, Config, InitOptions, Workspace};
use hippo_core::detect;
use hippo_core::incidents::Kind;
use hippo_core::index::Index;
use hippo_core::ingest::{self, IngestSummary};
use hippo_core::records::IngestState;
use hippo_core::rollup::{self, PullDone, Rollup};
use hippo_core::usage;
use hippo_view::replay;
use time::{Date, Duration, OffsetDateTime, UtcOffset};

use super::detect::print_found;

/// `Tue 10 Nov 2026`.
const DAY: &[time::format_description::FormatItem<'static>] = time::macros::format_description!(
    "[weekday repr:short] [day padding:none] [month repr:short] [year]"
);

/// How many sessions the overview lists.
const SESSIONS: usize = 5;

#[derive(clap::Args, Default)]
pub struct Args {
    /// Index the history without asking
    #[arg(long)]
    yes: bool,
    /// Another folder of transcripts to read, absolute or under ~/ (repeatable)
    #[arg(long = "add-root", value_name = "PATH")]
    add_roots: Vec<String>,
}

/// `hippo start`.
pub fn run(args: Args) -> Result<ExitCode> {
    first_run(&args, true)
}

/// `hippo` with no arguments.
pub fn bare() -> Result<ExitCode> {
    first_run(&Args::default(), false)
}

fn first_run(args: &Args, asked_for_start: bool) -> Result<ExitCode> {
    let home = config::home();
    let config_path = home.join("config.toml");
    if config_path.exists() {
        if asked_for_start {
            println!("hippo is already set up ({}).\n", config_path.display());
        } else {
            println!(
                "hippo · what your coding agents did, read from their history on this machine\n"
            );
        }
        let (config, ws) = super::load_history()?;
        refresh(&config, ws.as_ref())?;
        overview(&config)?;
        return Ok(ExitCode::SUCCESS);
    }

    let user_home = config::user_home();
    let mut patterns = config::default_transcript_roots();
    patterns.extend(args.add_roots.iter().cloned());
    let found = detect::detect(&patterns, &home, user_home.as_deref());
    println!(
        "hippo reads what your coding agents did, from the history they keep on this machine.\n"
    );
    println!("Looking for agent history:");
    print_found(&found);
    println!();
    if found.files() == 0 {
        println!("No agent history found, so there is nothing to index yet.");
        println!(
            "hippo reads Claude Code (~/.claude/projects and ~/.claude-<profile>/projects) and Codex (~/.codex/sessions)."
        );
        println!("If yours is somewhere else:  hippo start --add-root <folder>");
        return Ok(ExitCode::SUCCESS);
    }
    println!("hippo can index this history now:");
    println!(
        "  - it reads only the folders above and keeps its index in {}",
        home.display()
    );
    println!("  - history only: no notes folder needed (add one later with hippo init)");
    println!(
        "  - nothing is sent anywhere (later, only dreaming and ask's written answers call a model)"
    );
    if !args.yes {
        let stdin = io::stdin();
        if !stdin.is_terminal() {
            println!("\nRun  hippo start --yes  to index it.");
            return Ok(ExitCode::SUCCESS);
        }
        print!("\nIndex it now? [Y/n] ");
        io::stdout().flush()?;
        let mut answer = String::new();
        stdin.lock().read_line(&mut answer)?;
        let answer = answer.trim().to_lowercase();
        if !(answer.is_empty() || answer == "y" || answer == "yes") {
            println!("Nothing written. Run  hippo start --yes  when you want it.");
            return Ok(ExitCode::SUCCESS);
        }
    }
    println!();

    // History only: never adopt a workspace from the current directory,
    // which `init` does when it holds `.hippo/hippo.toml`.
    std::fs::create_dir_all(&home)?;
    std::env::set_current_dir(&home)?;
    let report = config::init(InitOptions {
        workspace: None,
        notes_dir: None,
        entry_note: None,
        add_roots: args.add_roots.clone(),
        force: false,
    })?;
    for path in &report.written {
        println!("wrote {}", path.display());
    }
    let config = Config::load()?;
    let started = Instant::now();
    let summary = ingest_with_progress(&config, None)?;
    let seconds = started.elapsed().as_secs_f64();
    match summary {
        IngestSummary::AlreadyRunning => {
            println!("another ingest is running; try again in a moment")
        }
        IngestSummary::Done {
            indexed, skipped, ..
        } => println!(
            "read {indexed} {} in {seconds:.1} s{}",
            plural(indexed, "file", "files"),
            if skipped > 0 {
                format!(" ({skipped} skipped: unchanged or unreadable)")
            } else {
                String::new()
            }
        ),
    }
    println!();
    overview(&config)?;
    Ok(ExitCode::SUCCESS)
}

/// Ingests, showing `reading <n> of <m> files` in place on a terminal.
fn ingest_with_progress(config: &Config, ws: Option<&Workspace>) -> Result<IngestSummary> {
    let tty = io::stderr().is_terminal();
    let mut last = 0u64;
    let mut err = io::stderr().lock();
    let summary = ingest::ingest_with_progress(config, ws, false, &mut |p| {
        if tty && (p.files_done == p.files_total || p.files_done >= last + 5) {
            last = p.files_done;
            let _ = write!(err, "\rreading {} of {} files", p.files_done, p.files_total);
            let _ = err.flush();
        }
    })?;
    if tty && last > 0 {
        let _ = write!(err, "\r\x1b[2K");
        let _ = err.flush();
    }
    Ok(summary)
}

/// Brings the index up to date when its last ingest is older than one
/// ingest interval, printing one line when it read anything.
fn refresh(config: &Config, ws: Option<&Workspace>) -> Result<()> {
    let state = IngestState::load(&config.home.join("ingest-state.json")).ok();
    let interval = Duration::minutes(i64::try_from(config.ingest_interval_minutes).unwrap_or(10));
    let fresh = state.is_some_and(|s| OffsetDateTime::now_utc() - s.last_ingest <= interval);
    if fresh {
        return Ok(());
    }
    if let IngestSummary::Done { indexed, .. } = ingest_with_progress(config, ws)? {
        if indexed > 0 {
            println!(
                "updated the index: read {indexed} new or changed {}\n",
                plural(indexed, "file", "files")
            );
        }
    }
    Ok(())
}

fn plural(n: u64, one: &'static str, many: &'static str) -> &'static str {
    if n == 1 {
        one
    } else {
        many
    }
}

/// `3h 05m`, or `42m`.
fn hm(d: Duration) -> String {
    let minutes = d.whole_minutes().max(0);
    if minutes >= 60 {
        format!("{}h {:02}m", minutes / 60, minutes % 60)
    } else {
        format!("{minutes}m")
    }
}

/// At most `max` characters of `text` on one line, `…` when cut.
fn cut(text: &str, max: usize) -> String {
    let text = super::one_line(text);
    if text.chars().count() <= max {
        return text;
    }
    let mut s: String = text.chars().take(max - 1).collect();
    s.push('…');
    s
}

/// The day the overview shows: today when it has a person-started
/// top-level session, else the newest day that has one.
pub(crate) fn pick_day(rollup: &Rollup, today: Date, offset: UtcOffset) -> Result<Option<Date>> {
    let days = rollup.days(offset)?;
    if days.iter().any(|d| d.date == Some(today) && d.sessions > 0) {
        return Ok(Some(today));
    }
    Ok(days.iter().filter(|d| d.sessions > 0).find_map(|d| d.date))
}

/// `All history: 33 sessions and 120 sub-agents on 12 days, Fri 2 Oct
/// 2026 to Tue 10 Nov 2026`, then a blank line.
fn history_line(rollup: &Rollup, offset: UtcOffset, now: OffsetDateTime) -> Result<()> {
    let active: Vec<Date> = rollup
        .days(offset)?
        .iter()
        .filter(|d| d.sessions > 0)
        .filter_map(|d| d.date)
        .collect();
    let (Some(last), Some(first)) = (active.first(), active.last()) else {
        return Ok(());
    };
    let all = rollup.sessions_in(OffsetDateTime::UNIX_EPOCH, now + Duration::days(1))?;
    let tops = all
        .iter()
        .filter(|s| s.facts.parent.is_none() && !s.facts.automated)
        .count();
    let subs = all.iter().filter(|s| s.facts.parent.is_some()).count();
    let day = |d: &Date| d.format(&DAY).unwrap_or_else(|_| d.to_string());
    let span = if first == last {
        day(first)
    } else {
        format!("{} to {}", day(first), day(last))
    };
    println!(
        "All history: {tops} {}, {subs} {} on {} {}, {span}\n",
        if tops == 1 { "session" } else { "sessions" },
        if subs == 1 { "sub-agent" } else { "sub-agents" },
        active.len(),
        if active.len() == 1 { "day" } else { "days" },
    );
    Ok(())
}

/// The overview of the last active day. Claims
/// the rollup marks unverified are left out on purpose: that rule is not
/// precise enough yet to show a newcomer.
pub fn overview(config: &Config) -> Result<()> {
    let rollup = Rollup::open(&config.home)?;
    let index = Index::open(&config.home, false).ok();
    let (Some(rollup), Some(index)) = (rollup, index) else {
        println!("Nothing is indexed yet. Run  hippo ingest  to read your agents' history.");
        return Ok(());
    };
    let now = OffsetDateTime::now_utc();
    let offset = usage::local_offset_at(now);
    let today = now.to_offset(offset).date();
    let Some(date) = pick_day(&rollup, today, offset)? else {
        println!("No session is indexed yet.");
        println!("  hippo detect               where hippo looks for agent history");
        println!("  hippo init --add-root <dir>  read another folder, then  hippo ingest");
        return Ok(());
    };
    let ids: Vec<String> = index.sources()?.into_keys().collect();
    let short = |id: &str| super::short_id(id, &ids);
    let day = replay::load(&rollup, &index, date, offset, now, &HashMap::new())?;
    let line = &day.line;
    let (start, end) = rollup::day_range(date, offset);
    let sessions = rollup.sessions_in(start, end)?;
    let top: Vec<_> = sessions
        .iter()
        .filter(|s| s.facts.parent.is_none() && !s.facts.automated)
        .collect();
    let mut merged: Vec<String> = rollup
        .pulls_in(start, end)?
        .into_iter()
        .filter(|p| p.done == PullDone::Merged)
        .map(|p| replay::pull_name(&p.url))
        .collect();
    merged.sort();
    merged.dedup();
    let corrections: Vec<_> = rollup
        .incidents(start)?
        .into_iter()
        .filter(|i| i.kind == Kind::Corrected && i.at < end)
        .collect();

    history_line(&rollup, offset, now)?;
    let when = if date == today {
        "today"
    } else {
        "your last active day"
    };
    println!(
        "{} · {when}",
        date.format(&DAY).unwrap_or_else(|_| date.to_string())
    );
    let threads = line.threads.max(top.len());
    println!(
        "  {threads} {}, {} {}, {} of agent work",
        if threads == 1 { "session" } else { "sessions" },
        line.subs,
        if line.subs == 1 {
            "sub-agent"
        } else {
            "sub-agents"
        },
        hm(line.work)
    );
    if line.questions > 0 {
        println!(
            "  agents waited on you {} in {} {}",
            hm(line.waited),
            line.questions,
            if line.questions == 1 {
                "question"
            } else {
                "questions"
            }
        );
    }
    if line.limits > 0 {
        println!(
            "  {} {} at a usage limit",
            line.limits,
            if line.limits == 1 { "stop" } else { "stops" }
        );
    }
    if !merged.is_empty() {
        let shown: Vec<&str> = merged.iter().take(4).map(String::as_str).collect();
        let more = merged.len().saturating_sub(shown.len());
        println!(
            "  {} pull {} merged: {}{}",
            merged.len(),
            if merged.len() == 1 {
                "request"
            } else {
                "requests"
            },
            shown.join(", "),
            if more > 0 {
                format!(" and {more} more")
            } else {
                String::new()
            }
        );
    }
    if let Some(first) = corrections.first() {
        println!(
            "  {} {} by you, the first: \"{}\"",
            corrections.len(),
            if corrections.len() == 1 {
                "correction"
            } else {
                "corrections"
            },
            cut(&first.body.showed.text, 90)
        );
    }
    println!();
    println!("Busiest sessions");
    for s in top.iter().take(SESSIONS) {
        let f = &s.facts;
        let title = f
            .title
            .as_deref()
            .or(f.asked.as_deref())
            .unwrap_or(f.id.as_str());
        println!(
            "  {} · {} · {} turns · {} · {}",
            cut(title, 60),
            f.project.as_deref().unwrap_or("-"),
            s.turns,
            f.ending(now).label(),
            short(&f.id)
        );
    }
    if top.len() > SESSIONS {
        println!(
            "  and {} more:  hippo show day {date}",
            top.len() - SESSIONS
        );
    }
    println!();
    println!("Try next");
    let mut next: Vec<(String, &str)> = Vec::new();
    if !corrections.is_empty() {
        next.push((
            "hippo incidents".into(),
            "what went wrong, quoted, with the step that caused it",
        ));
    } else if date == today {
        next.push((
            "hippo fleet".into(),
            "today's sessions: live, waiting on you, stopped at a limit",
        ));
    } else {
        next.push((
            format!("hippo show day {date}"),
            "that day's sessions, how each ended, and its pull requests",
        ));
    }
    if let Some(busiest) = top.first() {
        next.push((
            format!("hippo xray {}", short(&busiest.facts.id)),
            "what the busiest session was given, searched for and used",
        ));
    }
    next.push((
        "hippo ask --sources-only \"<question>\"".into(),
        "has an agent already solved it? ranked past sessions",
    ));
    next.push((
        "hippo card".into(),
        "the day as one image to share: numbers and a date, no names",
    ));
    let width = next.iter().map(|(c, _)| c.len()).max().unwrap_or(0);
    for (command, why) in next {
        println!("  {command:<width$}  {why}");
    }
    println!("\nEvery command: hippo --help");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_read_as_hours_and_minutes() {
        assert_eq!(hm(Duration::minutes(42)), "42m");
        assert_eq!(hm(Duration::minutes(185)), "3h 05m");
        assert_eq!(hm(Duration::seconds(-5)), "0m");
    }

    #[test]
    fn long_text_is_cut_on_one_line() {
        assert_eq!(cut("a\nb", 10), "a b");
        assert_eq!(cut("abcdefghij", 5), "abcd…");
    }
}
