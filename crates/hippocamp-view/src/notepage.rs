//! The note page: what a note's file does not say about it. Its sections
//! with their sizes and the newest date each names, who links to it and
//! where it links, what dreaming did with it, and which sessions read or
//! edited it.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use hippocamp_core::dream::sections::sections;
use hippocamp_core::notes::{self, Link};
use hippocamp_core::rollup::Rollup;
use time::macros::format_description;
use time::{Date, Month, OffsetDateTime, UtcOffset};

use crate::brain::Brain;
use crate::digest::size;
use crate::dreaming::Run;
use crate::text::visible;

/// How many runs the Dreaming column names.
pub const RUNS: usize = 3;
/// How many sessions "Read and edited by" names.
pub const SESSIONS: usize = 8;

/// One section of the note, a heading of level 1 or 2 with everything
/// under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// The heading without its `#`s; `(top)` for text before the first
    /// heading.
    pub heading: String,
    pub bytes: usize,
    /// The newest date the section names, as `3 Nov`.
    pub dated: Option<String>,
}

/// A note that links here, or one this note links to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteRef {
    /// Relative to the workspace.
    pub path: PathBuf,
    pub title: String,
    /// The folder below the notes directory, when there is one.
    pub folder: Option<String>,
}

/// A session that read or edited the note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reader {
    pub id: String,
    pub title: String,
    /// `read`, `discussed` or `edited`.
    pub kind: String,
    /// `8 Nov`.
    pub when: String,
}

/// A note's page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotePage {
    pub path: PathBuf,
    /// `OPERATIONS · RUNBOOK · STATUS MIXED`, its parts.
    pub kicker: Vec<String>,
    /// `67 KB · 354 lines · 20 sections · updated 31 Oct · …`, its parts;
    /// the ones a reader should see first are `strong`.
    pub facts: Vec<(String, bool)>,
    /// A line to act on: a review date passed, dreaming left it out.
    pub alert: Option<String>,
    pub sections: Vec<Section>,
    /// The largest section's size, for the bars.
    pub largest: usize,
    /// What dreaming did with it, newest run first: a lead and the rest.
    pub dreaming: Vec<(String, String)>,
    /// Every run that looked at it as a compact row, newest first: when, and the outcome in
    /// words (`verified, no change`, `change written`, …).
    pub runs: Vec<RunRow>,
    /// Changes dreaming confirmed and wrote to it over those runs.
    pub changes: usize,
    pub linked_from: Vec<NoteRef>,
    pub links_to: Vec<NoteRef>,
    /// Links that name no note.
    pub dangling: usize,
    pub readers: Vec<Reader>,
    /// All the sessions that read or edited it (`readers` names the newest).
    pub reader_count: usize,
}

fn month(name: &str) -> Option<Month> {
    let short = name.get(..3)?;
    Some(match short {
        "Jan" => Month::January,
        "Feb" => Month::February,
        "Mar" => Month::March,
        "Apr" => Month::April,
        "May" => Month::May,
        "Jun" => Month::June,
        "Jul" => Month::July,
        "Aug" => Month::August,
        "Sep" => Month::September,
        "Oct" => Month::October,
        "Nov" => Month::November,
        "Dec" => Month::December,
        _ => return None,
    })
}

/// `2026-11-03` at the start of `s`.
fn iso_at(s: &str) -> Option<Date> {
    let b = s.as_bytes();
    if b.len() < 10 || b[4] != b'-' || b[7] != b'-' || !s.starts_with("20") {
        return None;
    }
    if b.get(10).is_some_and(u8::is_ascii_digit) {
        return None;
    }
    let y: i32 = s.get(..4)?.parse().ok()?;
    let m: u8 = s.get(5..7)?.parse().ok()?;
    let d: u8 = s.get(8..10)?.parse().ok()?;
    Date::from_calendar_date(y, Month::try_from(m).ok()?, d).ok()
}

/// The newest date `text` names: `2026-11-03` or `3 Nov 2026` (the month
/// written short or long).
pub fn newest_date(text: &str) -> Option<Date> {
    let mut best: Option<Date> = None;
    let mut keep = |d: Date| best = Some(best.map_or(d, |b| b.max(d)));
    for (i, _) in text.char_indices() {
        let before_ok = i == 0 || !text.as_bytes()[i - 1].is_ascii_alphanumeric();
        if !before_ok {
            continue;
        }
        if let Some(d) = iso_at(&text[i..]) {
            keep(d);
        }
    }
    let words: Vec<&str> = text
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|w| !w.is_empty())
        .collect();
    for w in words.windows(3) {
        let (Ok(day), Some(m)) = (w[0].parse::<u8>(), month(w[1])) else {
            continue;
        };
        let year = w[2].trim_end_matches(['.', ')', ';', ':']);
        if year.len() == 4 && year.starts_with("20") {
            if let Ok(y) = year.parse::<i32>() {
                if let Ok(d) = Date::from_calendar_date(y, m, day) {
                    keep(d);
                }
            }
        }
    }
    best
}

/// `3 Nov`.
pub fn short_date(d: Date) -> String {
    d.format(format_description!("[day padding:none] [month repr:short]"))
        .unwrap_or_default()
}

fn heading_of(text: &str) -> Option<(usize, String)> {
    let line = text.lines().next()?;
    let hashes = line.chars().take_while(|c| *c == '#').count();
    ((1..=6).contains(&hashes) && line[hashes..].starts_with(' '))
        .then(|| (hashes, visible(line[hashes..].trim())))
}

/// The note's level-1 and level-2 sections, each with the deeper ones
/// under it; text before the first heading is `(top)` when it holds more
/// than a line or two.
pub fn note_sections(body: &[u8]) -> Vec<Section> {
    let mut out: Vec<(String, usize, String)> = Vec::new();
    for range in sections(body) {
        let text = String::from_utf8_lossy(&body[range.clone()]);
        match heading_of(&text) {
            Some((level, heading)) if level <= 2 || out.is_empty() => {
                out.push((heading, range.len(), text.into_owned()));
            }
            Some(_) => {
                if let Some(last) = out.last_mut() {
                    last.1 += range.len();
                    last.2.push_str(&text);
                }
            }
            None if text.trim().len() > 120 => {
                out.push(("(top)".to_owned(), range.len(), text.into_owned()))
            }
            None => {}
        }
    }
    out.into_iter()
        .map(|(heading, bytes, text)| Section {
            heading,
            bytes,
            dated: newest_date(&unfenced(&text)).map(short_date),
        })
        .collect()
}

/// A digest's reason for leaving a note out, as the rest of a sentence:
/// `to fit the budget: 67 KB`, without its own `left out` or full stop.
fn why_words(why: &str) -> String {
    let w = why.trim().trim_end_matches('.');
    w.strip_prefix("left out ").unwrap_or(w).to_owned()
}

/// `text` without its fenced code blocks.
fn unfenced(text: &str) -> String {
    let mut fenced = false;
    let mut out = String::new();
    for line in text.lines() {
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if !fenced {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

fn folder_of(path: &Path, notes_dir: &Path) -> Option<String> {
    let below = path.strip_prefix(notes_dir).unwrap_or(path);
    let parent = below.parent()?;
    (!parent.as_os_str().is_empty()).then(|| visible(&parent.display().to_string()))
}

/// What one run did with the note at `key`, when it looked at it.
/// One dreaming run's outcome for a note, as the note page's Dreaming card
/// lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRow {
    /// `9 Nov 03:31`.
    pub when: String,
    pub outcome: &'static str,
    /// Changes it confirmed and wrote to the note.
    pub written: usize,
    /// A dry run: listed, marked, never counted as a run.
    pub dry_run: bool,
}

/// A run's outcome for the note at `path`; `None` when it did not look at it.
fn run_row(run: &Run, key: &str, path: &Path, offset: UtcOffset) -> Option<RunRow> {
    let d = run.digest.as_ref()?;
    let (when, _) = run_line(run, key, path, offset)?;
    let mut written = 0;
    let mut rejected = false;
    for c in &run.changes {
        let Ok(p) = &c.proposal.record else { continue };
        if notes::normalise(&p.note) != path {
            continue;
        }
        match d.verdicts.get(&p.id).map(|v| v.verdict.as_str()) {
            Some("confirm") => written += 1,
            Some("reject") => rejected = true,
            _ => {}
        }
    }
    let failed = d.failures.iter().any(|f| f.note.as_deref() == Some(key));
    let checked = d.checked.iter().any(|c| c.note.as_deref() == Some(key));
    let outcome = if written > 0 {
        if written == 1 {
            "change written"
        } else {
            "changes written"
        }
    } else if rejected {
        "change rejected"
    } else if failed {
        "proposal failed validation"
    } else if checked {
        "verified, no change"
    } else {
        "left out"
    };
    let dry_run = run.report.record.as_ref().is_ok_and(|r| r.status.dry_run);
    Some(RunRow {
        when: when.trim_end_matches('.').to_owned(),
        outcome,
        written: if dry_run { 0 } else { written },
        dry_run,
    })
}

fn run_line(run: &Run, key: &str, path: &Path, offset: UtcOffset) -> Option<(String, String)> {
    let d = run.digest.as_ref()?;
    let when = run
        .written
        // The app's one date form: `Mon 03:31` within a week, else `9 Nov
        // 03:31`.
        .map(|t| {
            crate::fleet::when(
                OffsetDateTime::from(t).unix_timestamp(),
                OffsetDateTime::now_utc(),
                offset,
            )
        })
        .unwrap_or_else(|| {
            run.report
                .record
                .as_ref()
                .map(|r| short_date(r.status.date))
                .unwrap_or_default()
        });
    let mut parts = Vec::new();
    let mut passes = BTreeSet::new();
    for c in d.checked.iter().filter(|c| c.note.as_deref() == Some(key)) {
        passes.insert(c.pass.clone());
        parts.push(format!("{}: {}.", c.pass, c.facts));
    }
    for s in d.skipped.iter().filter(|s| s.note == key) {
        passes.insert(s.pass.clone());
        parts.push(format!("{}: left out, {}.", s.pass, why_words(&s.why)));
    }
    for f in d.failures.iter().filter(|f| f.note.as_deref() == Some(key)) {
        passes.insert(f.pass.clone());
        parts.push(format!(
            "A proposal failed validation: {}.",
            f.reason.trim_end_matches('.')
        ));
    }
    for c in &run.changes {
        let Ok(p) = &c.proposal.record else { continue };
        if notes::normalise(&p.note) != path {
            continue;
        }
        let verdict = d
            .verdicts
            .get(&p.id)
            .map(|v| v.verdict.clone())
            .unwrap_or_else(|| "proposed".to_owned());
        parts.push(match verdict.as_str() {
            "confirm" => format!("{}: a change was confirmed and written.", p.pass),
            "reject" => format!("{}: a change was rejected.", p.pass),
            other => format!("{}: a change, {other}.", p.pass),
        });
    }
    if parts.is_empty() {
        return None;
    }
    if !parts.iter().any(|p| p.contains("confirmed"))
        && !d.failures.iter().any(|f| f.note.as_deref() == Some(key))
    {
        parts.push("No change confirmed.".to_owned());
    }
    Some((format!("{when}."), parts.join(" ")))
}

/// What [`page`] reads.
#[derive(Debug, Clone, Copy)]
pub struct Inputs<'a> {
    /// The workspace root.
    pub root: &'a Path,
    /// The notes directory, relative to the root.
    pub notes_dir: &'a Path,
    pub brain: &'a Brain,
    /// Newest first.
    pub runs: &'a [Run],
    pub rollup: Option<&'a Rollup>,
    /// Session titles by id, for readers.
    pub titles: &'a HashMap<String, String>,
    pub today: Date,
    pub offset: UtcOffset,
}

/// The page of the note at `path` (relative to the workspace); `None` when
/// the scan does not hold it.
pub fn page(inp: &Inputs<'_>, path: &Path) -> Option<NotePage> {
    let notes_dir = notes::normalise(inp.notes_dir);
    let path = notes::normalise(path);
    let below = path.strip_prefix(&notes_dir).ok()?.to_path_buf();
    let notes = &inp.brain.notes;
    let note = notes.notes.get(&below)?;
    let body = String::from_utf8_lossy(&note.body);
    let fm = &note.frontmatter;

    // Links in and out.
    let resolve = |text: &str| match notes.resolve_link(text) {
        Link::Resolved(p) => Some(p),
        Link::Dangling => None,
    };
    let mut links_to: BTreeMap<PathBuf, NoteRef> = BTreeMap::new();
    let mut dangling = 0;
    for l in notes::links(&body) {
        match resolve(l) {
            Some(p) if p != below => {
                let title = notes
                    .notes
                    .get(&p)
                    .map(|n| visible(&n.title()))
                    .unwrap_or_default();
                let full = notes_dir.join(&p);
                links_to.entry(p).or_insert(NoteRef {
                    folder: folder_of(&full, &notes_dir),
                    path: full,
                    title,
                });
            }
            Some(_) => {}
            None => dangling += 1,
        }
    }
    let mut linked_from = Vec::new();
    for (other_path, other) in &notes.notes {
        if *other_path == below {
            continue;
        }
        let text = String::from_utf8_lossy(&other.body);
        if notes::links(&text).any(|l| resolve(l).as_ref() == Some(&below)) {
            let full = notes_dir.join(other_path);
            linked_from.push(NoteRef {
                folder: folder_of(&full, &notes_dir),
                path: full,
                title: visible(&other.title()),
            });
        }
    }
    linked_from.sort_by_key(|n| n.title.to_lowercase());

    // Sessions.
    let mut readers = Vec::new();
    let mut reader_count = 0;
    if let Some(r) = inp.rollup {
        if let Ok(seen) = r.sessions_of_note(&path) {
            let mut by_session: BTreeMap<String, (String, String)> = BTreeMap::new();
            for s in seen {
                let slot = by_session
                    .entry(s.session_id.clone())
                    .or_insert((s.kind.clone(), s.ts.clone()));
                if s.kind == "edit" {
                    slot.0 = s.kind.clone();
                }
                if s.ts > slot.1 {
                    slot.1 = s.ts;
                }
            }
            reader_count = by_session.len();
            let mut all: Vec<(String, String, String)> = by_session
                .into_iter()
                .map(|(id, (k, ts))| (id, k, ts))
                .collect();
            all.sort_by(|a, b| b.2.cmp(&a.2));
            for (id, kind, ts) in all.into_iter().take(SESSIONS) {
                let when =
                    OffsetDateTime::parse(&ts, &time::format_description::well_known::Rfc3339)
                        .map(|t| short_date(t.to_offset(inp.offset).date()))
                        .unwrap_or_default();
                readers.push(Reader {
                    title: inp
                        .titles
                        .get(&id)
                        .cloned()
                        .unwrap_or_else(|| id.chars().take(8).collect()),
                    id,
                    kind: match kind.as_str() {
                        "edit" => "edited",
                        "discuss" => "discussed",
                        _ => "read",
                    }
                    .to_owned(),
                    when,
                });
            }
        }
    }

    // Facts.
    let modified = std::fs::metadata(inp.root.join(&path))
        .and_then(|m| m.modified())
        .ok()
        .map(|t| OffsetDateTime::from(t).to_offset(inp.offset).date());
    let secs = note_sections(&note.body);
    let mut facts = vec![
        (size(note.body.len() as u64), false),
        (format!("{} lines", body.lines().count()), false),
        (
            format!(
                "{} {}",
                secs.len(),
                if secs.len() == 1 {
                    "section"
                } else {
                    "sections"
                }
            ),
            false,
        ),
    ];
    match (fm.updated, modified) {
        (Some(u), _) => facts.push((format!("updated {}", short_date(u)), false)),
        (None, Some(m)) => facts.push((format!("changed {}", short_date(m)), false)),
        _ => {}
    }
    facts.push((
        match linked_from.len() {
            0 => "no note links here".to_owned(),
            1 => "1 note links here".to_owned(),
            n => format!("{n} notes link here"),
        },
        true,
    ));
    if !links_to.is_empty() {
        facts.push((format!("links to {}", links_to.len()), false));
    }
    if reader_count > 0 {
        facts.push((
            format!(
                "{} {} read or edited it",
                reader_count,
                if reader_count == 1 {
                    "session"
                } else {
                    "sessions"
                }
            ),
            true,
        ));
    }

    // Dreaming.
    let key = path.display().to_string();
    let dreaming: Vec<(String, String)> = inp
        .runs
        .iter()
        .filter_map(|r| run_line(r, &key, &path, inp.offset))
        .take(RUNS)
        .collect();
    let runs: Vec<RunRow> = inp
        .runs
        .iter()
        .filter_map(|r| run_row(r, &key, &path, inp.offset))
        .collect();
    let changes = runs.iter().map(|r| r.written).sum();

    // The alert: what the lint findings do not already say. (A passed
    // review date is a lint finding, shown under the facts.)
    let mut alert = Vec::new();
    if let Some(s) = inp
        .runs
        .first()
        .and_then(|r| r.digest.as_ref())
        .and_then(|d| d.skipped.iter().find(|s| s.note == key))
    {
        alert.push(format!(
            "The last dreaming run left it out, {}.",
            why_words(&s.why)
        ));
    }

    let mut kicker = Vec::new();
    if let Some(f) = folder_of(&path, &notes_dir) {
        kicker.push(f);
    }
    if let Some(t) = &fm.r#type {
        kicker.push(visible(t));
    }
    if let Some(s) = &fm.status {
        kicker.push(format!("status {}", visible(s)));
    }
    let largest = secs.iter().map(|s| s.bytes).max().unwrap_or(0);
    Some(NotePage {
        path,
        kicker,
        facts,
        alert: (!alert.is_empty()).then(|| alert.join(" ")),
        largest,
        sections: secs,
        dreaming,
        runs,
        changes,
        linked_from,
        links_to: links_to.into_values().collect(),
        dangling,
        readers,
        reader_count,
    })
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)] // the tests write synthetic notes
mod tests;
