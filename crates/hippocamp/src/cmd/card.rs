//! `hippocamp card [--day <date>] [--week] [--out <path>]`: the overview's numbers for a day, or the seven
//! days ending on it, as one image to share and a text version. Read-only,
//! no model call, nothing sent anywhere.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{bail, Context, Result};
use hippocamp_core::config::Config;
use hippocamp_core::incidents::Kind;
use hippocamp_core::index::Index;
use hippocamp_core::rollup::{self, PullDone, Rollup};
use hippocamp_core::usage;
use hippocamp_view::replay;
use time::{Date, Duration, OffsetDateTime, UtcOffset};

use crate::card::{self, Facts, Span};

#[derive(clap::Args)]
pub struct Args {
    /// The day, YYYY-MM-DD in local time (default: the last active day)
    #[arg(long, value_name = "DATE")]
    day: Option<String>,
    /// The seven days that end on that day, summed
    #[arg(long)]
    week: bool,
    /// Where to write the PNG (default: $HIPPOCAMP_HOME/cards/)
    #[arg(long, value_name = "PATH")]
    out: Option<PathBuf>,
}

pub fn run(args: Args) -> Result<ExitCode> {
    let config = Config::load()?;
    let (Some(rollup), Ok(index)) = (
        Rollup::open(&config.home)?,
        Index::open(&config.home, false),
    ) else {
        println!("Nothing is indexed yet. Run  hippocamp  to read your agents' history.");
        return Ok(ExitCode::FAILURE);
    };
    let now = OffsetDateTime::now_utc();
    let offset = usage::local_offset_at(now);
    let today = now.to_offset(offset).date();
    let last_active = super::start::pick_day(&rollup, today, offset)?;
    let Some(last_active) = last_active else {
        println!("No session is indexed yet, so there is no day to show.");
        println!("  hippocamp detect   where hippocamp looks for agent history");
        return Ok(ExitCode::FAILURE);
    };
    let day = match &args.day {
        Some(text) => Date::parse(
            text,
            time::macros::format_description!("[year]-[month]-[day]"),
        )
        .with_context(|| format!("--day {text}: give a date as YYYY-MM-DD"))?,
        None => last_active,
    };
    let facts = if args.week {
        week(&rollup, &index, day, offset, now)?
    } else {
        let span = if day == today {
            Span::Today(day)
        } else {
            Span::Day(day)
        };
        one_day(&rollup, &index, day, offset, now, span)?
    };
    if facts.threads == 0 {
        println!(
            "no session on {}; the last active day is {last_active}  (hippocamp card --day {last_active})",
            if args.week {
                format!("the seven days to {day}")
            } else {
                day.to_string()
            }
        );
        return Ok(ExitCode::FAILURE);
    }

    let words = card::words(&facts);
    let png = card::render(&words);
    let path = match args.out {
        Some(path) => path,
        None => default_path(&config, &facts.span)?,
    };
    std::fs::write(&path, &png).with_context(|| format!("writing {}", path.display()))?;
    print!("{}", card::text(&words));
    println!(
        "wrote {} ({} x {})",
        path.display(),
        card::WIDTH,
        card::HEIGHT
    );
    Ok(ExitCode::SUCCESS)
}

/// The figures the overview shows for `date`.
fn one_day(
    rollup: &Rollup,
    index: &Index,
    date: Date,
    offset: UtcOffset,
    now: OffsetDateTime,
    span: Span,
) -> Result<Facts> {
    let day = replay::load(rollup, index, date, offset, now, &HashMap::new())?;
    let (start, end) = rollup::day_range(date, offset);
    let top = rollup
        .sessions_in(start, end)?
        .iter()
        .filter(|s| s.facts.parent.is_none() && !s.facts.automated)
        .count();
    let line = &day.line;
    Ok(Facts {
        span,
        work: line.work,
        threads: line.threads.max(top),
        subs: line.subs,
        merged: merged(rollup, start, end)?,
        waited: line.waited,
        questions: line.questions,
        corrections: corrections(rollup, start, end)?,
    })
}

/// The seven days that end on `last`: work, wait and questions summed day
/// by day; sessions, pull requests and corrections counted once over the
/// span.
fn week(
    rollup: &Rollup,
    index: &Index,
    last: Date,
    offset: UtcOffset,
    now: OffsetDateTime,
) -> Result<Facts> {
    let first = last - Duration::days(6);
    let (mut work, mut waited, mut questions) = (Duration::ZERO, Duration::ZERO, 0);
    let mut date = first;
    while date <= last {
        let line = replay::load(rollup, index, date, offset, now, &HashMap::new())?.line;
        work += line.work;
        waited += line.waited;
        questions += line.questions;
        date = date.next_day().context("a date past the calendar")?;
    }
    let (start, _) = rollup::day_range(first, offset);
    let (_, end) = rollup::day_range(last, offset);
    let sessions = rollup.sessions_in(start, end)?;
    Ok(Facts {
        span: Span::Week { first, last },
        work,
        threads: sessions
            .iter()
            .filter(|s| s.facts.parent.is_none() && !s.facts.automated)
            .count(),
        subs: sessions.iter().filter(|s| s.facts.parent.is_some()).count(),
        merged: merged(rollup, start, end)?,
        waited,
        questions,
        corrections: corrections(rollup, start, end)?,
    })
}

fn merged(rollup: &Rollup, start: OffsetDateTime, end: OffsetDateTime) -> Result<usize> {
    Ok(rollup
        .pulls_in(start, end)?
        .into_iter()
        .filter(|p| p.done == PullDone::Merged)
        .map(|p| p.url)
        .collect::<HashSet<_>>()
        .len())
}

fn corrections(rollup: &Rollup, start: OffsetDateTime, end: OffsetDateTime) -> Result<usize> {
    Ok(rollup
        .incidents(start)?
        .into_iter()
        .filter(|i| i.kind == Kind::Corrected && i.at < end)
        .count())
}

/// `$HIPPOCAMP_HOME/cards/card-<date>.png` (`card-week-<last>.png`), or
/// the system temporary folder when that folder is inside a git work tree.
fn default_path(config: &Config, span: &Span) -> Result<PathBuf> {
    let name = match span {
        Span::Today(d) | Span::Day(d) => format!("card-{d}.png"),
        Span::Week { last, .. } => format!("card-week-{last}.png"),
    };
    let cards = config.home.join("cards");
    let dir = if in_git_tree(&cards) {
        std::env::temp_dir()
    } else {
        std::fs::create_dir_all(&cards).with_context(|| format!("creating {}", cards.display()))?;
        cards
    };
    if !dir.is_dir() {
        bail!("{} is not a folder", dir.display());
    }
    Ok(dir.join(name))
}

/// Whether `dir` or a parent holds `.git`.
fn in_git_tree(dir: &Path) -> bool {
    dir.ancestors().any(|d| d.join(".git").exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_under_a_git_tree_is_seen() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".git")).unwrap();
        assert!(in_git_tree(&root.path().join("a/b")));
    }
}
