//! `hippo fleet [--json]`: today's sessions by state (live, stopped at a
//! usage limit, interrupted, finished, waiting on you) and headroom per
//! profile, from the client's cached meter or an estimate from the
//! profile's own transcripts. `hippo resume <id> [--on
//! <profile>] [--json]`: the command that resumes a session, with a short
//! handoff.

use std::io::{self, Write};
use std::process::ExitCode;

use anyhow::{anyhow, Result};
use hippo_core::adapters::transcript_roots;
use hippo_core::index::Index;
use hippo_core::rollup::Rollup;
use hippo_core::usage::{self, Snapshot};
use hippo_view::fleet::{self, Fleet, State};
use serde_json::json;
use time::{OffsetDateTime, UtcOffset};

use super::load_history;

#[derive(clap::Args)]
pub struct Args {
    /// Print one JSON object
    #[arg(long)]
    json: bool,
}

#[derive(clap::Args)]
pub struct ResumeArgs {
    /// The session id or a unique prefix of it (from `hippo fleet`, `hippo ask` or `hippo show day`)
    id: String,
    /// Resume on this Claude profile instead (`default`, `work`, …); the
    /// command copies the transcript there first, never over a file
    #[arg(long)]
    on: Option<String>,
    /// Print one JSON object
    #[arg(long)]
    json: bool,
}

fn load() -> Result<(Fleet, UtcOffset, hippo_core::config::Config)> {
    let (config, _) = load_history()?;
    let now = OffsetDateTime::now_utc();
    let offset = usage::local_offset_at(now);
    let roots = transcript_roots(&config);
    let snap = Snapshot::read(&config.home, &roots, now, offset)?;
    let rollup = Rollup::open(&config.home).ok().flatten();
    let fleet = fleet::build(&fleet::Inputs {
        now,
        offset,
        roots: &roots,
        rollup: rollup.as_ref(),
        usage: &snap,
    });
    Ok((fleet, offset, config))
}

fn hm(t: Option<OffsetDateTime>, offset: UtcOffset) -> String {
    t.map(|t| {
        let t = t.to_offset(offset);
        format!("{:02}:{:02}", t.hour(), t.minute())
    })
    .unwrap_or_else(|| "--:--".into())
}

fn state_text(s: &State, offset: UtcOffset) -> String {
    match s {
        State::Live => "live".into(),
        State::Stopped {
            reset,
            at,
            carried_on,
        } => {
            let mut t = format!("usage limit {}", hm(Some(*at), offset));
            if let Some(r) = reset {
                t.push_str(&format!(", {r}"));
            }
            if *carried_on {
                t.push_str(", carried on");
            }
            t
        }
        State::Interrupted { detail, at } => format!(
            "interrupted {}{}",
            hm(Some(*at), offset),
            detail
                .as_deref()
                .map(|d| format!(", {d}"))
                .unwrap_or_default()
        ),
        State::Finished { at } => format!("done {}", hm(Some(*at), offset)),
        State::Quiet { since } => format!("quiet since {}", hm(*since, offset)),
        State::Waiting { at, plan, .. } => format!(
            "{} {}",
            if *plan {
                "plan to approve"
            } else {
                "waiting on you"
            },
            hm(Some(*at), offset)
        ),
    }
}

pub fn run(args: Args) -> Result<ExitCode> {
    let (fleet, offset, config) = load()?;
    let mut out = io::stdout().lock();
    if args.json {
        let rooms: Vec<_> = fleet
            .headroom
            .iter()
            .map(|h| {
                json!({
                    "profile": h.profile,
                    "tool": if h.codex { "codex" } else { "claude-code" },
                    "left": h.left(),
                    "resets": h.resets(),
                    "limit": h.limit,
                    "limit_from": h.limit_from,
                    "weekly_until": h.weekly_until,
                    "window_open": !matches!(h.window, usage::WindowState::None),
                    "usable": h.usable(),
                    "last_seen": h.last_seen,
                    "five_hour": h.five.map(|l| window_json(&l)),
                    "seven_day": h.seven.map(|l| window_json(&l)),
                    "meter_stale_since": h.meter_stale,
                    "summary": fleet::room_line(h, offset),
                })
            })
            .collect();
        let v = json!({
            "headroom_note": "the client's cached usage meter where a profile has one (source \"meter\"); else an estimate from your own transcripts (source \"estimate\")",
            "use_next": fleet.use_next,
            "usage_cached": fleet.usage_cached,
            "headroom": rooms,
            "sessions": fleet.rows,
        });
        writeln!(out, "{v}")?;
        return Ok(ExitCode::SUCCESS);
    }
    writeln!(
        out,
        "headroom (the client's usage meter; est. = an estimate from your own transcripts):"
    )?;
    for h in &fleet.headroom {
        let next = if fleet.use_next.as_deref() == Some(h.profile.as_str()) {
            "  ← use next"
        } else {
            ""
        };
        writeln!(
            out,
            "  {:<16} {}{next}",
            h.profile,
            fleet::room_line(h, offset)
        )?;
    }
    writeln!(out)?;
    if fleet.rows.is_empty() {
        // Name the last day that had work.
        writeln!(out, "No session today.")?;
        if let Some(day) = last_active_day(&config, offset) {
            writeln!(
                out,
                "The last day with one was {day}:  hippo show day {day}  (or just hippo, for its overview)"
            )?;
        }
    }
    for r in &fleet.rows {
        let state = if r.needs_check() {
            "check".to_owned()
        } else {
            state_text(&r.state, offset)
        };
        writeln!(
            out,
            "{:<28} {:<14} {:<12} {}  {}",
            state,
            r.profile.as_deref().unwrap_or("-"),
            r.project.as_deref().unwrap_or("-"),
            r.id,
            r.title
        )?;
        if let State::Waiting { asked: Some(q), .. } = &r.state {
            writeln!(out, "{:<28} asked: {q}", "")?;
        }
        if let Some(a) = &r.activity {
            writeln!(out, "{:<28} {a}", "")?;
        }
        if let Some(line) = r.claims.line() {
            writeln!(out, "{:<28} {line}", "")?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// The newest local day with a person-started top-level session.
fn last_active_day(config: &hippo_core::config::Config, offset: UtcOffset) -> Option<time::Date> {
    let rollup = Rollup::open(&config.home).ok().flatten()?;
    let days = rollup.days(offset).ok()?;
    days.iter().filter(|d| d.sessions > 0).find_map(|d| d.date)
}

fn window_json(l: &usage::Left) -> serde_json::Value {
    json!({
        "left": l.frac,
        "source": match l.source {
            usage::Source::Meter => "meter",
            usage::Source::Estimate => "estimate",
        },
        "resets": l.resets_at,
        "reset_passed": l.reset_passed,
    })
}

pub fn resume(args: ResumeArgs) -> Result<ExitCode> {
    let (fleet, _, config) = load()?;
    let index = Index::open(&config.home, false).ok();
    // An id or a unique prefix: today's fleet row
    // first, else any indexed session.
    let id = match (fleet.row(&args.id), &index) {
        (Some(_), _) => args.id.clone(),
        (None, Some(index)) => super::resolve_session(index, &args.id)?,
        (None, None) => return Err(anyhow!("no index yet; run hippo ingest")),
    };
    let other = match &args.on {
        Some(p) => Some(
            fleet
                .room(p)
                .filter(|h| !h.codex)
                .ok_or_else(|| anyhow!("no Claude profile named {p}"))?,
        ),
        None => None,
    };
    let r = match (fleet.row(&id), &index) {
        (Some(row), _) => fleet::resume(row, index.as_ref(), other),
        (None, Some(index)) => resume_indexed(&config, index, &id, other)?,
        (None, None) => None,
    }
    .ok_or_else(|| anyhow!("{id} cannot be resumed from here (a sub-agent, or an unknown tool)"))?;
    let mut out = io::stdout().lock();
    if args.json {
        writeln!(out, "{}", serde_json::to_string(&r)?)?;
    } else {
        writeln!(out, "{}", r.clipboard())?;
    }
    Ok(ExitCode::SUCCESS)
}

/// How to resume an indexed session that is not in today's fleet: the
/// command from the index's tool and transcript and the transcript's
/// working directory (as `hippo ask` builds it), the handoff from the
/// rollup's title, first request and final report, and the open items.
fn resume_indexed(
    config: &hippo_core::config::Config,
    index: &Index,
    id: &str,
    other: Option<&usage::Headroom>,
) -> Result<Option<fleet::Resume>> {
    use hippo_core::resume::{self as r, Target};
    let Some((tool, transcript)) = index.source(id)? else {
        return Ok(None);
    };
    let origin = hippo_core::session_meta::origin(id, &tool, &transcript);
    let target = Target {
        id,
        tool: &tool,
        transcript: &transcript,
        cwd: origin.cwd.as_deref(),
    };
    let facts = Rollup::open(&config.home)
        .ok()
        .flatten()
        .and_then(|rollup| rollup.session(id).ok().flatten());
    let own = facts.as_ref().and_then(|f| f.profile());
    let (command, on) = match other.and_then(|h| h.dir.as_deref().map(|d| (h, d))) {
        Some((h, dir)) if own.as_deref() != Some(h.profile.as_str()) => {
            (r::command_on(&target, dir), Some(h.profile.clone()))
        }
        _ => (r::command(&target), own),
    };
    let Some(command) = command else {
        return Ok(None);
    };
    let title = facts
        .as_ref()
        .and_then(|f| f.title.clone())
        .or(origin.named.clone())
        .unwrap_or_else(|| id.chars().take(8).collect());
    let open = index
        .turns_for_session(id)
        .map(|t| r::open_items(&t))
        .unwrap_or_default();
    Ok(Some(fleet::Resume {
        on,
        command,
        handoff: r::handoff(
            &title,
            facts.as_ref().and_then(|f| f.asked.as_deref()),
            facts.as_ref().and_then(|f| f.outcome.as_deref()),
            &open,
        ),
    }))
}
