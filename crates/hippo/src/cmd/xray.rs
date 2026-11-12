//! `hippo xray <session> [--json]`: the context a session was given and
//! whether it used it, what it searched the notes for, and whether its
//! claims have evidence. `hippo xray --fleet [--days N] [--json]`: the
//! fleet's use of its context and the tune tips. `hippo xray unrequire
//! <note> [--yes]`: makes a required note relevant-only.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{anyhow, bail, Result};
use hippo_core::index::Index;
use hippo_core::rollup::Rollup;
use hippo_core::xray::{self, human_bytes, Source, Status, Use};
use time::OffsetDateTime;

use super::{load, load_history};

#[derive(clap::Args)]
#[command(args_conflicts_with_subcommands = true)]
pub struct Args {
    #[command(subcommand)]
    command: Option<Command>,
    /// The session's id (from `hippo fleet`)
    session: Option<String>,
    /// The fleet's use of its context over the last days, and tune tips
    #[arg(long)]
    fleet: bool,
    /// Days the fleet stats cover (1 to 30)
    #[arg(long, default_value_t = 7, value_parser = clap::value_parser!(u32).range(1..=30))]
    days: u32,
    /// Print one JSON object
    #[arg(long)]
    json: bool,
}

#[derive(clap::Subcommand)]
enum Command {
    /// Remove a note from required_notes in .hippo/hippo.toml (prints the
    /// change; writes it only with --yes)
    Unrequire {
        /// The note, relative to the workspace (`brain/Open questions.md`)
        note: PathBuf,
        /// Write the change
        #[arg(long)]
        yes: bool,
    },
}

pub fn run(args: Args) -> Result<ExitCode> {
    if let Some(Command::Unrequire { note, yes }) = args.command {
        let (_, ws) = load()?;
        let path = ws.hippo_dir().join("hippo.toml");
        let text = std::fs::read_to_string(&path)?;
        let change = xray::unrequire(&text, &note).map_err(|e| anyhow!(e))?;
        println!("- {}", change.before);
        println!("+ {}", change.after);
        if yes {
            xray::unrequire_write(&ws, &note)?;
            println!("Wrote {}", path.display());
        } else {
            println!(
                "Nothing written; run again with --yes to change {}",
                path.display()
            );
        }
        return Ok(ExitCode::SUCCESS);
    }
    let (config, ws) = load_history()?;
    if args.fleet {
        let rollup = Rollup::open(&config.home)?
            .ok_or_else(|| anyhow!("no rollup yet; run hippo ingest"))?;
        let f = xray::fleet(ws.as_ref(), &rollup, args.days, OffsetDateTime::now_utc())?;
        if args.json {
            println!("{}", serde_json::to_string_pretty(&f)?);
            return Ok(ExitCode::SUCCESS);
        }
        println!(
            "{} top-level sessions in {} days, {} with a recorded context block",
            f.sessions, f.days, f.recorded
        );
        if let (Some(avg), Some(p)) = (f.avg_block(), f.ignored_percent()) {
            println!(
                "start blocks: {} each over {} sessions, {p}% never opened or cited",
                xray::human_bytes(avg),
                f.recorded
            );
        }
        println!("\nNOTE USE (sessions: read / cited / ignored)");
        for n in &f.notes {
            println!(
                "  {:<40} {:<9} {:>3}: {:>3} / {:>3} / {:>3}  {}",
                cut(&n.label, 40),
                n.kind.map(|k| k.as_str()).unwrap_or(""),
                n.sessions,
                n.read,
                n.cited,
                n.ignored,
                human_bytes(n.bytes)
            );
        }
        println!("\nTIPS");
        if f.tips.is_empty() {
            println!("  none: no required note was ignored in most sessions");
        }
        for t in &f.tips {
            println!("  {}", t.text);
            if t.action.as_deref() == Some("unrequire") {
                println!("    hippo xray unrequire '{}'", t.note.display());
            }
        }
        println!("\nCLAIMS  {}", f.claims.line());
        return Ok(ExitCode::SUCCESS);
    }
    let Some(id) = args.session else {
        bail!("name a session (hippo xray <id>), or use --fleet");
    };
    let index = Index::open(&config.home, false)?;
    let id = super::resolve_session(&index, &id)?;
    let x = xray::of(ws.as_ref(), &index, &id)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&x)?);
        return Ok(ExitCode::SUCCESS);
    }
    let rollup = Rollup::open(&config.home).ok().flatten();
    let title = rollup
        .as_ref()
        .and_then(|r| r.session(&id).ok().flatten())
        .and_then(|f| f.title.or(f.asked))
        .unwrap_or_else(|| id.clone());
    println!("{}  ({id})", cut(&title, 80));
    match x.block {
        Source::NotRecorded => {
            println!("\nINJECTED AT START  not recorded: this session's context block was not kept")
        }
        source => {
            let t = x.totals;
            println!(
                "\nCONTEXT USE  read {} · cited {} · ignored {} of {} injected  (from the {})",
                human_bytes(t.read),
                human_bytes(t.cited),
                human_bytes(t.ignored),
                human_bytes(t.total),
                if source == Source::Record {
                    "session record"
                } else {
                    "transcript's hook output"
                }
            );
            println!("\nINJECTED AT START");
            for p in &x.parts {
                let used = match p.used {
                    Use::Read => format!("read · turn {}", p.read_at.unwrap_or(0)),
                    Use::Cited => format!(
                        "cited · turn {}{}",
                        p.cited_at.unwrap_or(0),
                        if p.cited > 1 {
                            format!(" ({}×)", p.cited)
                        } else {
                            String::new()
                        }
                    ),
                    Use::Ignored if p.note.is_some() => "never opened".into(),
                    Use::Ignored => "ignored".into(),
                };
                let size = match p.note_bytes {
                    Some(n) => format!(
                        "{} · note {}",
                        human_bytes(p.bytes),
                        human_bytes(n as usize)
                    ),
                    None => human_bytes(p.bytes),
                };
                println!(
                    "  {:<36} {:<12} {:<22} {used}",
                    cut(&p.label, 36),
                    p.kind.as_str(),
                    size
                );
            }
        }
    }
    println!("\nWHAT IT SEARCHED FOR");
    if x.searches.is_empty() {
        println!("  no search of the notes");
    }
    for s in &x.searches {
        let hits = match s.hits {
            Some(0) => "0 hits".to_owned(),
            Some(n) => match s.used {
                Some(u) => format!("{n} hits · used {u}"),
                None => format!("{n} hits"),
            },
            None => "hits unknown".to_owned(),
        };
        let follow = s
            .follow
            .as_deref()
            .map(|f| format!(" · {f}"))
            .unwrap_or_default();
        println!(
            "  {:<6} {:<44} {hits}{follow}  (turn {})",
            s.tool,
            cut(&s.query, 44),
            s.ordinal
        );
    }
    println!("\nCLAIMS VS EVIDENCE");
    if x.claims.is_empty() {
        println!("  no claims");
    }
    for c in &x.claims {
        let mark = match c.status {
            Status::Verified => "verified  ",
            Status::Unverified => "UNVERIFIED",
            Status::Unclear => "unclear   ",
        };
        println!(
            "  {mark} \"{}\" (turn {})\n             {}",
            c.claim, c.ordinal, c.evidence
        );
    }
    println!("\nRead and cited come from tool calls and quotes in the transcript; what the model attended to inside its context cannot be known.");
    Ok(ExitCode::SUCCESS)
}

/// A full id, or the one indexed session an id prefix names.
fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut s: String = text.chars().take(max - 1).collect();
    s.push('…');
    s
}
