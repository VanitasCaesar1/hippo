//! `hippo view morning|strip|map|note|guard|counts [--json]`: the models
//! behind Hippo.app's Morning, Brain and Guard (`hippo_view::night`),
//! read-only. `--json` prints what the app draws; the site's web replica
//! is generated from it.

use std::collections::{BTreeSet, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Result;
use hippo_core::config::{Config, Workspace};
use hippo_core::dream::History;
use hippo_core::lessons::{self, Lesson};
use hippo_core::rollup::Rollup;
use hippo_core::usage;
use hippo_view::brain::Brain;
use hippo_view::dreaming::Dreaming;
use hippo_view::night::{counts, guard, map, morning, note, strip};
use time::{Duration, OffsetDateTime, UtcOffset};

#[derive(clap::Subcommand)]
pub enum Command {
    /// The last night pass, the rules waiting for you, and what was applied and rejected
    Morning {
        #[arg(long)]
        json: bool,
    },
    /// Yesterday's activity, the night with its pass, and this morning
    Strip {
        #[arg(long)]
        json: bool,
    },
    /// Every note with its folder, flags, reads this week and links, and the map's layout at each zoom level
    Map {
        #[arg(long)]
        json: bool,
    },
    /// One note's rules, pending rules with their evidence, readers, links and receipts
    Note {
        /// The note's path relative to the workspace
        path: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// The guard's incidents of the last days, in plain words, with what to do
    Guard {
        /// Days to cover (1 to 14)
        #[arg(long, default_value_t = 7, value_parser = clap::value_parser!(i64).range(1..=14))]
        days: i64,
        #[arg(long)]
        json: bool,
    },
    /// The top bar's badges and the runner that answers Ask
    Counts {
        #[arg(long)]
        json: bool,
    },
}

/// Everything the models read, once.
struct Loaded {
    ws: Workspace,
    history: History,
    dreaming: Dreaming,
    lessons: Vec<Lesson>,
    rollup: Option<Rollup>,
    brain: Brain,
    titles: HashMap<PathBuf, String>,
    repeats: Option<hippo_core::lessons::repeats::Report>,
    now: OffsetDateTime,
    offset: UtcOffset,
}

fn load() -> Result<Loaded> {
    let (config, ws) = super::load()?;
    let now = OffsetDateTime::now_utc();
    let offset = usage::local_offset_at(now);
    let history = hippo_core::dream::history(&ws)?;
    let dreaming = hippo_view::dreaming::load(&ws, &history)?;
    let lessons = lessons::list(&config.home).unwrap_or_default();
    let rollup = Rollup::open(&config.home).ok().flatten();
    let brain = hippo_view::brain::load(&ws, &history);
    let titles = brain
        .rows
        .iter()
        .map(|r| (r.path.clone(), r.title.clone()))
        .collect();
    let repeats = hippo_core::lessons::repeats::from_home(&config, Some(&ws), 6, now).ok();
    Ok(Loaded {
        ws,
        history,
        dreaming,
        lessons,
        rollup,
        brain,
        titles,
        repeats,
        now,
        offset,
    })
}

fn morning_of(l: &Loaded) -> morning::Morning {
    morning::build(&morning::Inputs {
        ws: &l.ws,
        history: &l.history,
        dreaming: &l.dreaming,
        lessons: &l.lessons,
        rollup: l.rollup.as_ref(),
        titles: &l.titles,
        repeats: l.repeats.as_ref(),
        now: l.now,
        offset: l.offset,
    })
}

/// Builds the map: the notes the last pass fixed and the ones it was
/// refused, from Morning.
fn map_of(l: &Loaded) -> map::NotesMap {
    let m = morning_of(l);
    let fixed: BTreeSet<PathBuf> = m
        .applied
        .iter()
        .filter(|f| !f.reverted)
        .map(|f| f.note.clone())
        .collect();
    let rejected: BTreeSet<PathBuf> = m.rejected.iter().map(|f| f.note.clone()).collect();
    map::build(&map::Inputs {
        ws: &l.ws,
        brain: &l.brain,
        rollup: l.rollup.as_ref(),
        lessons: &l.lessons,
        fixed: &fixed,
        rejected: &rejected,
        now: l.now,
        offset: l.offset,
    })
}

fn print_json<T: serde::Serialize>(value: &T) -> Result<()> {
    let mut out = std::io::stdout().lock();
    serde_json::to_writer(&mut out, value)?;
    writeln!(out)?;
    Ok(())
}

pub fn run(command: Command) -> Result<ExitCode> {
    match command {
        Command::Morning { json } => {
            let l = load()?;
            let m = morning_of(&l);
            if json {
                print_json(&m)?;
            } else {
                print_morning(&m);
            }
        }
        Command::Strip { json } => {
            let l = load()?;
            let m = morning_of(&l);
            let pass = m.pass.as_ref().map(|p| (p.started, p.ended));
            let s = strip::build(l.rollup.as_ref(), pass, l.now, l.offset);
            if json {
                print_json(&s)?;
            } else {
                println!("Day      {}", s.day);
                println!("Night    {}", s.night);
                println!("Morning  {}", s.morning);
                println!("{}", sparkline(&s.beats, &s.corrections));
            }
        }
        Command::Map { json } => {
            let l = load()?;
            let m = map_of(&l);
            if json {
                print_json(&m)?;
            } else {
                println!("{}\n{}\n{}\n", m.kicker, m.headline, m.lead);
                for f in &m.folders {
                    let name = if f.name.is_empty() {
                        "(top level)"
                    } else {
                        &f.name
                    };
                    println!(
                        "{name}  {}",
                        hippo_view::night::count(f.notes.len(), "note", "notes")
                    );
                    for &i in &f.notes {
                        print_note_line(&m.notes[i]);
                    }
                }
                let pinned: Vec<&map::MapNote> = m
                    .notes
                    .iter()
                    .filter(|n| n.flags.entry || n.flags.required)
                    .collect();
                if !pinned.is_empty() {
                    println!("Every session");
                    for n in pinned {
                        print_note_line(n);
                    }
                }
                println!("\n{} links", m.edges.len());
            }
        }
        Command::Note { path, json } => {
            let l = load()?;
            let m = map_of(&l);
            let Some(p) = note::build(
                &note::Inputs {
                    ws: &l.ws,
                    map: &m,
                    history: Some(&l.history),
                    lessons: &l.lessons,
                    rollup: l.rollup.as_ref(),
                    now: l.now,
                    offset: l.offset,
                },
                &note_path(&l.ws, &path),
            ) else {
                anyhow::bail!(
                    "no note {} in the notes folder (paths are relative to the workspace, as hippo view map lists them)",
                    path.display()
                );
            };
            if json {
                print_json(&p)?;
            } else {
                println!("{}\n\n{}\n", p.title, p.summary);
                println!(
                    "read {}× this week, {} kept, {} changes",
                    p.reads_week, p.rules_kept, p.changes
                );
                for r in &p.rules {
                    println!("  rule: {} ({})", r.rule, r.kept);
                }
                for r in &p.pending {
                    println!("  waiting: {}  (hippo lessons keep {})", r.rule, r.id);
                }
                for (path, title) in &p.links_out {
                    println!("  links to {title}  {}", path.display());
                }
                for (path, title) in &p.links_in {
                    println!("  linked from {title}  {}", path.display());
                }
                for r in &p.receipts {
                    println!("  {}  {}  {}", r.receipt, r.when, r.what);
                }
            }
        }
        Command::Guard { days, json } => {
            let (config, _) = super::load_history()?;
            let g = guard_of(&config, days, &|_| false)?;
            if json {
                print_json(&g)?;
            } else {
                println!("{}\n{}\n{}\n", g.kicker, g.headline, g.lead);
                for i in &g.items {
                    println!(
                        "{}  {}  {}",
                        match i.severity {
                            guard::Severity::Important => "important",
                            guard::Severity::Minor => "minor    ",
                        },
                        i.when,
                        i.title
                    );
                    println!("    {}", i.command);
                    println!("    {}: {}", i.target, i.target_note);
                    println!("    what to do: {}", i.what_to_do);
                }
            }
        }
        Command::Counts { json } => {
            let (config, ws) = super::load_history()?;
            let c = counts_of(&config, ws.as_ref());
            if json {
                print_json(&c)?;
            } else {
                println!("rules waiting      {}", c.rules_waiting);
                println!("sessions waiting   {}", c.sessions_waiting);
                println!("guard (important)  {}", c.guard_important);
                if let Some(t) = &c.indexed {
                    println!("indexed            {t}");
                }
                if let Some(a) = &c.ask {
                    println!("ask                {}", a.label);
                }
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// A path as given, or under the notes folder when it is relative to it.
fn note_path(ws: &Workspace, path: &Path) -> PathBuf {
    let p = hippo_core::notes::normalise(path);
    if ws.root.join(&p).is_file() {
        p
    } else {
        hippo_core::notes::normalise(&ws.notes_dir)
            .strip_prefix(&ws.root)
            .map(|d| d.join(&p))
            .unwrap_or(p)
    }
}

fn print_note_line(n: &map::MapNote) {
    let chip = n
        .flags
        .chip()
        .map(|c| format!("  [{}]", c.0))
        .unwrap_or_default();
    println!("  {}  {}{chip}", n.title, n.reads_label());
}

/// Guard over `days` days, leaving out what `dismissed` names.
pub fn guard_of(
    config: &Config,
    days: i64,
    dismissed: &dyn Fn(&str) -> bool,
) -> Result<guard::Guard> {
    let now = OffsetDateTime::now_utc();
    let offset = usage::local_offset_at(now);
    let rollup = Rollup::open(&config.home)?;
    let (list, sessions, titles) = match &rollup {
        Some(r) => {
            let since = now - Duration::days(days);
            let list = r.incidents(since)?;
            let sessions = r
                .sessions_in(since, now)?
                .iter()
                .filter(|s| s.facts.parent.is_none() && !s.facts.automated)
                .count();
            (list, sessions, r.titles().unwrap_or_default())
        }
        None => (Vec::new(), 0, HashMap::new()),
    };
    Ok(guard::build(
        &list,
        dismissed,
        sessions,
        days,
        &|id| titles.get(id).cloned(),
        now,
        offset,
    ))
}

/// The badges: lesson files, the fleet's waiting rows, the week's guard
/// incidents, the last ingest and the ask runner.
pub fn counts_of(config: &Config, ws: Option<&Workspace>) -> counts::Counts {
    let now = OffsetDateTime::now_utc();
    let offset = usage::local_offset_at(now);
    let rules_waiting = lessons::pending(&config.home).map_or(0, |l| l.len());
    let guard_important = guard_of(config, 7, &|_| false).map_or(0, |g| g.important);
    let roots = hippo_core::adapters::transcript_roots(config);
    let sessions_waiting = usage::Snapshot::read(&config.home, &roots, now, offset)
        .ok()
        .map(|snap| {
            let rollup = Rollup::open(&config.home).ok().flatten();
            hippo_view::fleet::build(&hippo_view::fleet::Inputs {
                now,
                offset,
                roots: &roots,
                rollup: rollup.as_ref(),
                usage: &snap,
            })
            .waiting()
            .count()
        })
        .unwrap_or(0);
    let status = match ws {
        Some(ws) => hippo_view::status::load(config, ws, now),
        None => hippo_view::status::load_history(config, now),
    };
    let ask = hippo_core::runners::plan(config)
        .ok()
        .and_then(|p| counts::asker(&p));
    counts::Counts {
        rules_waiting,
        sessions_waiting,
        guard_important,
        indexed: status.last_ingest.map(|t| hippo_view::night::hm(t, offset)),
        index_failed: status.errors.first().cloned(),
        ask,
    }
}

fn print_morning(m: &morning::Morning) {
    println!("{}\n{}\n{}\n", m.kicker, m.headline, m.lead);
    let f = &m.figures;
    println!(
        "{} rules waiting · {} notes fixed · {} fixes rejected · {} notes checked\n",
        f.rules_waiting, f.notes_fixed, f.fixes_rejected, f.notes_checked
    );
    if !m.rules.is_empty() {
        println!("Waiting for you");
        for r in &m.rules {
            println!("  {}  {}", r.from, r.rule);
            for e in &r.evidence {
                let words = e.said.as_deref().or(e.failed.as_deref()).unwrap_or("");
                println!("      {}  {words}", e.when);
            }
            if let Some(n) = &r.note_label {
                println!("      into {n}  (hippo lessons keep {})", r.id);
            }
        }
        println!();
    }
    if !m.applied.is_empty() {
        println!("Applied overnight");
        for x in &m.applied {
            match &x.was {
                Some(w) => println!("  {}: “{w}” → {}", x.title, x.now),
                None => println!("  {}: added “{}”", x.title, x.now),
            }
            if let Some(u) = &x.undo {
                println!("      undo: hippo {}", u.join(" "));
            }
        }
        println!();
    }
    if !m.rejected.is_empty() {
        println!("Rejected by the second model");
        for x in &m.rejected {
            println!("  {}: {}", x.title, x.reason);
        }
    }
}

/// The strip as one line of block characters, corrections as `!`.
fn sparkline(beats: &[u32], corrections: &[usize]) -> String {
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let max = beats.iter().copied().max().unwrap_or(0).max(1) as f32;
    beats
        .iter()
        .enumerate()
        .map(|(k, &b)| {
            if corrections.contains(&k) {
                '!'
            } else if b == 0 {
                ' '
            } else {
                BARS[((b as f32 / max) * 7.).round() as usize]
            }
        })
        .collect()
}
