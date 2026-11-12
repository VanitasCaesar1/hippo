//! `hippo show session|proposal|note|day`: read-only. Prints what the
//! desktop app shows for a session's turns, a dreaming proposal with its
//! receipts and evidence, a note's lint findings and change receipts, and a
//! day's edition facts from the rollup. Text
//! output flattens each text to one line; `--json` prints compact JSON,
//! each record as its file holds it.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{anyhow, Context as _, Result};
use hippo_core::config::{Config, Workspace};
use hippo_core::dream::validate;
use hippo_core::error::Error;
use hippo_core::index::{Index, SessionTurn};
use hippo_core::notes::{self, Notes};
use hippo_core::records::{list_proposals, list_receipts};
use hippo_core::sessions;
use serde_json::{json, Value};
use ulid::Ulid;

use super::one_line;

#[derive(clap::Subcommand)]
pub enum Command {
    /// A session's log line and its indexed turns
    Session {
        /// The session id
        id: String,
        /// Print the record and one object per turn
        #[arg(long)]
        json: bool,
    },
    /// A dreaming proposal with its receipts and evidence turns
    Proposal {
        /// The proposal id
        id: String,
        /// Print one object
        #[arg(long)]
        json: bool,
    },
    /// A day's sessions, how each ended, and its pull requests, from the
    /// rollup `hippo ingest` keeps
    Day {
        /// The day, `YYYY-MM-DD`; today when left out
        date: Option<String>,
        /// The day's UTC offset, `+09:30`; UTC when left out
        #[arg(long, default_value = "+00:00", allow_hyphen_values = true)]
        offset: String,
        /// Print one object
        #[arg(long)]
        json: bool,
    },
    /// A note's lint findings and change receipts
    Note {
        /// The note's path relative to the workspace
        path: PathBuf,
        /// Print one object
        #[arg(long)]
        json: bool,
    },
}

pub fn run(command: Command) -> Result<ExitCode> {
    // A session's turns and a day need only the history; a proposal and a
    // note need the notes folder.
    let (config, workspace) = super::load_history()?;
    let needs = || workspace.as_ref().ok_or(Error::NoWorkspace);
    match command {
        Command::Session { id, json } => session(&config, workspace.as_ref(), &id, json)?,
        Command::Proposal { id, json } => proposal(&config, needs()?, &id, json)?,
        Command::Note { path, json } => note(needs()?, &path, json)?,
        Command::Day { date, offset, json } => day(&config, date.as_deref(), &offset, json)?,
    }
    Ok(ExitCode::SUCCESS)
}

/// A file's JSON as it is on disk.
fn read_json(path: &Path) -> Result<Value> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))
}

fn field<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}

fn open_index(config: &Config) -> Result<Index> {
    Ok(Index::open(&config.home, false)?)
}

fn turn_json(turn: &SessionTurn) -> Value {
    json!({
        "ordinal": turn.ordinal,
        "role": turn.role,
        "ts": turn.ts,
        "tool_name": turn.tool_name,
        "text": turn.text,
    })
}

fn session(config: &Config, ws: Option<&Workspace>, id: &str, as_json: bool) -> Result<()> {
    if !sessions::is_valid_id(id) {
        return Err(Error::InvalidSessionId(id.to_owned()).into());
    }
    let index = open_index(config)?;
    // An id or a unique prefix; a declared record
    // whose session is not indexed keeps its own id.
    let id = &match super::resolve_session(&index, id) {
        Ok(full) => full,
        Err(err) => match ws {
            Some(ws) if sessions::log_row(ws, id, None)?.is_some() => id.to_owned(),
            _ => return Err(err),
        },
    };
    let turns = index.turns_for_session(id)?;
    let count = (!turns.is_empty()).then_some(turns.len() as u64);
    let row = match ws {
        Some(ws) => sessions::log_row(ws, id, count)?,
        None => None,
    };
    if row.is_none() && turns.is_empty() {
        return Err(anyhow!("no session {id}"));
    }
    if as_json {
        let record = match (row.as_ref(), ws) {
            (Some(_), Some(ws)) => {
                read_json(&ws.hippo_dir().join("sessions").join(format!("{id}.json")))?
            }
            _ => Value::Null,
        };
        println!("{record}");
        for turn in &turns {
            println!(
                "{}",
                json!({
                    "ordinal": turn.ordinal,
                    "role": turn.role,
                    "ts": turn.ts,
                    "tool_name": turn.tool_name,
                    "text": turn.text,
                })
            );
        }
        return Ok(());
    }
    match (&row, ws) {
        (Some(row), Some(ws)) => println!("{}", sessions::log_line(ws, row)),
        _ => println!("(no session record)"),
    }
    // What the desktop app shows for the session besides its record.
    let origin = index
        .sources()?
        .get(id)
        .map(|(tool, path)| sessions::origin(id, tool, path));
    let task = row.as_ref().map_or("", |r| r.task.as_str());
    if let Some(title) = sessions::title(task, &sessions::openings_of(&turns), origin.as_ref()) {
        println!("title: {title}");
    }
    if let Some(origin) = &origin {
        if let Some(cwd) = &origin.cwd {
            println!("project: {}", cwd.display());
        }
        if let Some(parent) = &origin.parent {
            println!("parent: {parent}");
        }
        if origin.automated {
            println!("started by a program");
        }
    }
    if turns.is_empty() {
        println!("no indexed turns");
    }
    for turn in &turns {
        let tool = turn
            .tool_name
            .as_deref()
            .map(|name| format!(" {name}"))
            .unwrap_or_default();
        println!(
            "{} {}{tool} {}",
            turn.ordinal,
            turn.role,
            one_line(&turn.text)
        );
    }
    Ok(())
}

fn proposal(config: &Config, ws: &Workspace, id: &str, as_json: bool) -> Result<()> {
    let id: Ulid = id
        .parse()
        .with_context(|| format!("not a proposal id: {id}"))?;
    let path = validate::path(ws, id);
    if !path.exists() {
        return Err(anyhow!("no proposal {id}"));
    }
    let record = read_json(&path)?;
    let proposal = hippo_core::records::Proposal::load(&path)?;
    let receipts: Vec<Value> = list_receipts(ws)?
        .into_iter()
        .filter(|listed| listed.record.as_ref().is_ok_and(|r| r.proposal == id))
        .map(|listed| read_json(&listed.path))
        .collect::<Result<_>>()?;
    let pairs: Vec<(String, u32)> = proposal
        .evidence
        .iter()
        .map(|e| (e.session.clone(), e.ordinal))
        .collect();
    let found = if pairs.is_empty() {
        Vec::new()
    } else {
        open_index(config)?.turns(&pairs)?
    };
    let evidence: Vec<(&(String, u32), Option<&SessionTurn>)> = pairs
        .iter()
        .map(|pair| {
            let turn = found
                .iter()
                .find(|t| t.session == pair.0 && t.ordinal == pair.1);
            (pair, turn)
        })
        .collect();
    if as_json {
        let evidence: Vec<Value> = evidence
            .iter()
            .map(|((session, ordinal), turn)| match turn {
                Some(turn) => {
                    let mut value = turn_json(turn);
                    value["session"] = json!(session);
                    value
                }
                None => json!({"session": session, "ordinal": ordinal, "missing": true}),
            })
            .collect();
        println!(
            "{}",
            json!({"proposal": record, "receipts": receipts, "evidence": evidence})
        );
        return Ok(());
    }
    println!(
        "proposal {id} {} {} {}",
        field(&record, "pass"),
        proposal.note.display(),
        field(&record, "kind")
    );
    println!("old: {}", one_line(&proposal.old));
    println!("new: {}", one_line(&proposal.new));
    println!("reason: {}", one_line(&proposal.reason));
    for receipt in &receipts {
        println!(
            "receipt {} {} {} applied {} {}",
            field(receipt, "id"),
            field(receipt, "verdict"),
            field(receipt, "reviewer"),
            applied(receipt),
            one_line(field(receipt, "notes")),
        );
    }
    for ((session, ordinal), turn) in evidence {
        match turn {
            Some(turn) => println!(
                "evidence {session} {ordinal} {} {}",
                turn.role,
                one_line(&turn.text)
            ),
            None => println!("evidence {session} {ordinal} (not in the index)"),
        }
    }
    Ok(())
}

fn applied(receipt: &Value) -> &str {
    receipt
        .get("applied")
        .and_then(Value::as_str)
        .unwrap_or("no")
}

fn note(ws: &Workspace, path: &Path, as_json: bool) -> Result<()> {
    let path = notes::normalise(path);
    if let Err(err) = notes::validate_proposal_path(&ws.root, &ws.notes_dir, &path) {
        let below = path.starts_with(notes::normalise(&ws.notes_dir));
        if below && !ws.root.join(&path).exists() {
            return Err(anyhow!("no note {}", path.display()));
        }
        return Err(err.into());
    }
    let notes_dir = notes::normalise(&ws.notes_dir);
    let found = Notes::scan(&ws.notes_root())?;
    let lint: Vec<(&'static str, String)> = found
        .lint()
        .iter()
        .filter(|finding| notes::normalise(&notes_dir.join(finding.note())) == path)
        .map(|finding| (finding.kind(), finding.detail()))
        .collect();
    let mine: HashSet<Ulid> = list_proposals(ws)?
        .into_iter()
        .filter_map(|listed| listed.record.ok())
        .filter(|proposal| notes::normalise(&proposal.note) == path)
        .map(|proposal| proposal.id)
        .collect();
    let mut receipts = Vec::new();
    for listed in list_receipts(ws)? {
        if let Ok(receipt) = &listed.record {
            if mine.contains(&receipt.proposal) {
                receipts.push(read_json(&listed.path)?);
            }
        }
    }
    if as_json {
        let lint: Vec<Value> = lint
            .iter()
            .map(|(kind, detail)| json!({"kind": kind, "detail": detail}))
            .collect();
        println!(
            "{}",
            json!({"note": path.display().to_string(), "lint": lint, "receipts": receipts})
        );
        return Ok(());
    }
    for (kind, detail) in &lint {
        println!("lint {kind} {}", one_line(detail));
    }
    for receipt in &receipts {
        println!(
            "receipt {} {} {} {}",
            field(receipt, "id"),
            field(receipt, "verdict"),
            field(receipt, "proposal"),
            applied(receipt),
        );
    }
    Ok(())
}

/// `hippo show day`: the day's totals, then one line per session with a
/// turn that day, most turns first.
fn day(config: &Config, date: Option<&str>, offset: &str, as_json: bool) -> Result<()> {
    use hippo_core::rollup::{self, Rollup};
    use time::{Date, OffsetDateTime, UtcOffset};

    let offset = UtcOffset::parse(
        offset,
        time::macros::format_description!("[offset_hour sign:mandatory]:[offset_minute]"),
    )
    .map_err(|e| anyhow!("--offset {offset}: {e}; give it as +09:30"))?;
    let now = OffsetDateTime::now_utc();
    let date = match date {
        Some(d) => Date::parse(d, time::macros::format_description!("[year]-[month]-[day]"))
            .map_err(|e| anyhow!("{d}: {e}; give the day as YYYY-MM-DD"))?,
        None => now.to_offset(offset).date(),
    };
    let Some(rollup) = Rollup::open(&config.home)? else {
        return Err(anyhow!(
            "no rollup in {}: run hippo ingest, which writes it",
            config.home.display()
        ));
    };
    let (start, end) = rollup::day_range(date, offset);
    let sessions = rollup.sessions_in(start, end)?;
    let pulls = rollup.pulls_in(start, end)?;
    let events = rollup.events_in(start, end)?;
    let top: Vec<_> = sessions
        .iter()
        .filter(|s| s.facts.parent.is_none() && !s.facts.automated)
        .collect();
    let agents = sessions.iter().filter(|s| s.facts.parent.is_some()).count();
    let turns: u64 = sessions.iter().map(|s| s.turns).sum();
    let edits: u64 = sessions.iter().map(|s| s.edits).sum();
    let distinct = |done: rollup::PullDone| {
        let mut urls: Vec<&str> = pulls
            .iter()
            .filter(|p| p.done == done)
            .map(|p| p.url.as_str())
            .collect();
        urls.sort_unstable();
        urls.dedup();
        urls
    };
    let (opened, merged) = (
        distinct(rollup::PullDone::Opened),
        distinct(rollup::PullDone::Merged),
    );
    if as_json {
        let rows: Vec<Value> = top
            .iter()
            .map(|s| {
                json!({
                    "id": s.facts.id,
                    "title": s.facts.title,
                    "project": s.facts.project,
                    "profile": s.facts.profile(),
                    "turns": s.turns,
                    "edits": s.edits,
                    "agents": s.facts.agents,
                    "ending": s.facts.ending(now).label(),
                    "ending_detail": s.facts.ending_detail,
                })
            })
            .collect();
        let events: Vec<Value> = events
            .iter()
            .map(|e| json!({"session": e.session_id, "ts": e.ts, "kind": e.kind, "detail": e.detail}))
            .collect();
        println!(
            "{}",
            json!({
                "date": date.to_string(),
                "sessions": top.len(),
                "agents": agents,
                "turns": turns,
                "edits": edits,
                "pulls": {"opened": opened, "merged": merged},
                "events": events,
                "rows": rows,
            })
        );
        return Ok(());
    }
    println!(
        "{date}: {} sessions, {agents} agents, {turns} turns, {edits} edits, {} pull requests opened, {} merged",
        top.len(),
        opened.len(),
        merged.len()
    );
    for s in &top {
        let f = &s.facts;
        let title = f.title.as_deref().unwrap_or(&f.id);
        let detail = f
            .ending_detail
            .as_deref()
            .map(|d| format!(" ({d})"))
            .unwrap_or_default();
        println!(
            "{} · {} · {} turns · {} edits · {}{detail} · {}",
            one_line(title),
            f.project.as_deref().unwrap_or("-"),
            s.turns,
            s.edits,
            f.ending(now).label(),
            f.id
        );
    }
    for e in &events {
        println!("{} {}: {}", e.ts, e.kind, one_line(&e.detail));
    }
    Ok(())
}
