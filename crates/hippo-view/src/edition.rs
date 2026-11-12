//! The Edition: one day of what the agents did and what the memory learned,
//! read from the rollup (`hippo_core::rollup`), the notes and the dreaming
//! reports.
//!
//! [`days`] lists the editions for the dock; [`build`] makes one edition.
//! Both read; neither writes.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use hippo_core::dream::inputs::Trace;
use hippo_core::error::Result;
use hippo_core::rollup::{self, Ending, InRange, Rollup, SessionFacts};
use hippo_core::sessions::LogRow;
use time::macros::format_description;
use time::{Date, OffsetDateTime, UtcOffset};

use crate::brain::NoteRow;
use crate::digest::Digest;
use crate::dreaming::Run;
use crate::text::visible;

/// How many stories an edition lists before "more".
pub const STORIES: usize = 8;
/// How many of them carry a summary.
pub const LEADS: usize = 3;
/// How many changed notes are named one per line.
pub const NOTES: usize = 6;
/// The longest sentence a summary takes from a request or a final report,
/// in characters.
pub const SENTENCE_CHARS: usize = 150;
/// How many limit stops Problems names one per line.
pub const LIMITS: usize = 4;

/// A piece of the deck; `strong` pieces are drawn in the text colour.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub strong: bool,
}

/// One day in the dock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DayRow {
    pub date: Date,
    pub sessions: usize,
    pub turns: u64,
    pub agents: usize,
    pub edits: u64,
    pub pulls: usize,
    /// `dream ok`, `dream: nothing changed`, `dream dry run`, `dream failed`.
    pub dream: Option<String>,
}

/// How a story ended, when that is news.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ended {
    pub label: String,
    /// Still open: drawn in the accent colour.
    pub open: bool,
    /// A limit or a failure.
    pub problem: bool,
}

/// One story: a top-level session, or several with the same title.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Story {
    /// The threads, the one with the most turns that day first: a click
    /// opens it.
    pub ids: Vec<String>,
    pub title: String,
    pub project: Option<String>,
    /// What it did, for a lead story ([`summary`]): the request, the files
    /// edited and pull requests opened and merged, and the first sentence
    /// of its final report.
    pub summary: Option<String>,
    /// `13:19 → 23:51 · 10 h 32 m · 1,999 turns · …`, without the ending.
    pub facts: String,
    pub ended: Option<Ended>,
    /// Pull requests it or its sub-agents opened that day.
    pub opened: Vec<String>,
    /// Pull requests it or its sub-agents merged that day.
    pub merged: Vec<String>,
    pub score: u64,
}

/// Pull requests of one repository opened or merged that day.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shipped {
    /// `owner/repo`.
    pub repo: String,
    /// Every number opened or merged.
    pub numbers: Vec<u64>,
    pub opened: usize,
    pub merged: usize,
    /// The first pull request's URL, to open.
    pub url: String,
}

/// A note that changed that day.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteChange {
    /// Relative to the workspace.
    pub path: PathBuf,
    /// `Folder / Title`.
    pub name: String,
    /// Created that day.
    pub new: bool,
    /// `21:13`.
    pub detail: String,
    /// The story that edited it, when the rollup knows.
    pub editor: Option<String>,
    pub modified: OffsetDateTime,
}

/// A line of Problems.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub error: bool,
    pub text: String,
    pub detail: Option<String>,
    /// A session to open, when the line is about one.
    pub session: Option<String>,
    /// A note to open.
    pub note: Option<PathBuf>,
}

/// One day's edition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edition {
    pub date: Date,
    /// `The edition · Sunday`.
    pub kicker: String,
    /// `8 November 2026`.
    pub headline: String,
    pub deck: Vec<Span>,
    /// Turns per local hour.
    pub hours: [u64; 24],
    /// `busiest 13:00–15:00`.
    pub busiest: Option<String>,
    pub stories: Vec<Story>,
    /// The other stories' titles, most active first.
    pub more: Vec<String>,
    pub shipped: Vec<Shipped>,
    /// Newest first; the app names the first [`NOTES`] one per line.
    pub notes: Vec<NoteChange>,
    /// How many notes changed.
    pub notes_changed: usize,
    /// Dreaming as one paragraph: a lead (`Dreaming, 22:55.`) and the rest.
    pub dreaming: Option<(String, String)>,
    /// The run the paragraph is about, to open.
    pub dreaming_report: Option<PathBuf>,
    /// What a person can act on: limit stops not resumed in a thread of
    /// the same title, failures, a
    /// failed dreaming run, an ingest that stopped.
    pub problems: Vec<Problem>,
    /// The engine's housekeeping, in plain words, for the quiet health line
    /// under Problems: notes too big for dreaming, sessions that left no
    /// trace of a handoff (by `hippo doctor`'s rule).
    pub health: Vec<String>,
    /// Top-level sessions still open.
    pub in_flight: usize,
    pub sessions: usize,
    pub agents: usize,
}

/// What [`build`] reads besides the rollup.
#[derive(Debug, Clone, Copy)]
pub struct Inputs<'a> {
    pub date: Date,
    pub offset: UtcOffset,
    pub now: OffsetDateTime,
    /// The workspace root, where note paths start.
    pub root: &'a Path,
    pub notes: &'a [NoteRow],
    /// Newest first, as `dreaming::load` gives them.
    pub runs: &'a [Run],
    /// The session records, for handoffs.
    pub records: &'a [LogRow],
    /// What the transcripts of the records that ended `unspecified` show
    ///a record missing here left no trace.
    pub traces: &'a BTreeMap<String, Trace>,
    /// When the last ingest finished, if it is stale (`hippo doctor`'s
    /// rule): the index has stopped following the transcripts.
    pub ingest_stopped: Option<OffsetDateTime>,
}

/// The health line for sessions that ended in `start..end` without a
/// handoff, by `hippo doctor`'s rule: of the
/// declared sessions that ended then, those that ended `unspecified` and
/// whose transcripts show neither a session-end command nor a note edit
/// left no trace. A line only when they are more than half.
pub fn handoff_health(
    records: &[LogRow],
    traces: &BTreeMap<String, Trace>,
    start: OffsetDateTime,
    end: OffsetDateTime,
) -> Option<String> {
    let ended: Vec<&LogRow> = records
        .iter()
        .filter(|r| !r.derived && r.ended.is_some_and(|t| t >= start && t < end))
        .collect();
    let no_trace = ended
        .iter()
        .filter(|r| r.no_change_reason.as_deref() == Some("unspecified"))
        .filter(|r| !traces.get(&r.id).is_some_and(|t| t.any()))
        .count();
    (no_trace * 2 > ended.len()).then(|| {
        format!(
            "{no_trace} of {} left no handoff, note edit or session-end command",
            count(ended.len() as u64, "session", "sessions")
        )
    })
}

// ---------------------------------------------------------------------------
// Words

/// `1,999`.
pub fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A count as a word up to twelve, else digits: `three`, `1,204`.
fn word(n: u64) -> String {
    const WORDS: [&str; 13] = [
        "no", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
        "eleven", "twelve",
    ];
    WORDS
        .get(usize::try_from(n).unwrap_or(usize::MAX))
        .map_or_else(|| thousands(n), |w| (*w).to_owned())
}

/// `n thing` or `n things`, `n` in digits.
fn count(n: u64, one: &str, many: &str) -> String {
    format!("{} {}", thousands(n), if n == 1 { one } else { many })
}

/// [`count`] with the number as a [`word`].
fn count_word(n: u64, one: &str, many: &str) -> String {
    format!("{} {}", word(n), if n == 1 { one } else { many })
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn clock(t: OffsetDateTime, offset: UtcOffset) -> String {
    t.to_offset(offset)
        .format(format_description!("[hour]:[minute]"))
        .unwrap_or_default()
}

/// `10 h 32 m`, `45 m`, `under a minute`.
fn span(seconds: i64) -> String {
    let m = seconds / 60;
    match (m / 60, m % 60) {
        (0, 0) => "under a minute".to_owned(),
        (0, m) => format!("{m} m"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} m"),
    }
}

/// Markdown as plain text, on one line: emphasis, code marks, heading
/// marks, list marks and link targets taken out.
pub fn plain(text: &str) -> String {
    let mut lines = Vec::new();
    let mut fenced = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced || t.starts_with('|') || t.chars().all(|c| "-=*_ ".contains(c)) {
            continue;
        }
        let t = t.trim_start_matches('#').trim_start();
        let t = t
            .strip_prefix("- ")
            .or_else(|| t.strip_prefix("* "))
            .unwrap_or(t);
        lines.push(t.to_owned());
    }
    let mut s = lines.join(" ");
    // [text](url) → text
    while let Some(open) = s.find("](") {
        let Some(start) = s[..open].rfind('[') else {
            break;
        };
        let Some(close) = s[open..].find(')') else {
            break;
        };
        let label = s[start + 1..open].to_owned();
        s.replace_range(start..open + close + 1, &label);
    }
    let s = s.replace("**", "").replace("__", "").replace('`', "");
    let s = single_emphasis(&s);
    visible(&s.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// `*word*` and `_word_` without their marks; a `*` or `_` inside a word
/// (`2*3`, `snake_case`) or standing alone stays.
fn single_emphasis(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    for (i, &c) in chars.iter().enumerate() {
        if c == '*' || c == '_' {
            let before = i.checked_sub(1).map(|j| chars[j]);
            let after = chars.get(i + 1).copied();
            let edge = |x: Option<char>| {
                x.is_none_or(|x| x.is_whitespace() || "([{\"'“‘.,;:!?)]}”’".contains(x))
            };
            let opens = edge(before) && after.is_some_and(|a| !a.is_whitespace() && a != c);
            let closes = edge(after) && before.is_some_and(|b| !b.is_whitespace() && b != c);
            if opens || closes {
                continue;
            }
        }
        out.push(c);
    }
    out
}

/// `text`'s whole sentences up to `max` characters; when even the first
/// is longer, cut at a word with an ellipsis.
pub fn sentences(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out = String::new();
    for s in text.split_inclusive(". ") {
        if out.chars().count() + s.chars().count() > max {
            break;
        }
        out.push_str(s);
    }
    let out = out.trim().to_owned();
    if !out.is_empty() {
        return out;
    }
    let mut cut = String::new();
    for w in text.split(' ') {
        if cut.chars().count() + w.chars().count() + 1 > max.saturating_sub(1) {
            break;
        }
        if !cut.is_empty() {
            cut.push(' ');
        }
        cut.push_str(w);
    }
    cut.push('…');
    cut
}

// ---------------------------------------------------------------------------
// The dock

/// What one dreaming run did, in two or three words: `dream ok` when it
/// confirmed, applied or proposed a change; `dream: nothing changed` when it
/// ran and changed nothing; `dream dry run`; `dream failed`.
pub fn run_word(run: &Run) -> &'static str {
    let status = run.report.record.as_ref().map(|r| &r.status);
    match status {
        Ok(s) if s.failed => "dream failed",
        Ok(s) if s.dry_run => "dream dry run",
        Ok(_) if run_changed(run) => "dream ok",
        Ok(_) => "dream: nothing changed",
        Err(_) => "dream: unreadable",
    }
}

/// How many changes a run confirmed or applied.
pub fn run_changes(run: &Run) -> usize {
    let confirmed = run.digest.as_ref().map_or(0, |d| {
        d.verdicts
            .values()
            .filter(|v| v.verdict == "confirm")
            .count()
    });
    let applied = run.report.record.as_ref().map_or(0, |r| r.applied.len());
    confirmed.max(applied)
}

/// The run confirmed, applied or proposed something.
pub fn run_changed(run: &Run) -> bool {
    let applied = run
        .report
        .record
        .as_ref()
        .is_ok_and(|r| !r.applied.is_empty() || !r.proposals.is_empty());
    applied
        || !run.changes.is_empty()
        || run
            .digest
            .as_ref()
            .is_some_and(|d| d.verdicts.values().any(|v| v.verdict == "confirm"))
}

/// The local day a run belongs to: a run written before noon is the
/// night's run of the day before (the nightly job runs in the small hours),
/// one written after noon is that day's; without the time, the date in its
/// file name.
fn run_day(run: &Run, offset: UtcOffset) -> Option<Date> {
    if let Some(t) = run.written {
        let local = OffsetDateTime::from(t).to_offset(offset);
        return Some(if local.hour() < 12 {
            local.date().previous_day().unwrap_or(local.date())
        } else {
            local.date()
        });
    }
    run.report.record.as_ref().ok().map(|r| r.status.date)
}

/// The editions, newest first: every local day with a turn, with the
/// dreaming run of that day.
pub fn days(rollup: &Rollup, runs: &[Run], offset: UtcOffset) -> Result<Vec<DayRow>> {
    let mut by_day: HashMap<Date, &Run> = HashMap::new();
    // Oldest first, so the newest run of a day wins.
    for run in runs.iter().rev() {
        if let Some(d) = run_day(run, offset) {
            by_day.insert(d, run);
        }
    }
    Ok(rollup
        .days(offset)?
        .into_iter()
        .filter_map(|d| {
            let date = d.date?;
            Some(DayRow {
                date,
                sessions: d.sessions,
                turns: d.turns,
                agents: d.agents,
                edits: d.edits,
                pulls: d.pulls,
                dream: by_day.get(&date).map(|r| run_word(r).to_owned()),
            })
        })
        .collect())
}

/// The day the home opens on: today once it has a top-level session and
/// it is past noon, else the newest day before today with one.
pub fn default_day(days: &[DayRow], now: OffsetDateTime, offset: UtcOffset) -> Option<Date> {
    let local = now.to_offset(offset);
    let today = local.date();
    let busy_today = days.iter().any(|d| d.date == today && d.sessions > 0);
    if busy_today && local.hour() >= 12 {
        return Some(today);
    }
    days.iter()
        .find(|d| d.date < today && d.sessions > 0)
        .or_else(|| days.first())
        .map(|d| d.date)
}

// ---------------------------------------------------------------------------
// One edition

/// A story's title for a session: the rollup's title, else its request,
/// else its id's start.
fn title_of(s: &InRange) -> String {
    s.facts
        .title
        .clone()
        .or_else(|| {
            s.facts
                .asked
                .as_deref()
                .map(hippo_core::session_meta::headline)
        })
        .filter(|t| !t.trim().is_empty())
        .map(|t| visible(&t))
        .unwrap_or_else(|| s.facts.id.chars().take(8).collect())
}

/// Pull request numbers as a reader wants them: `#4`, `#1–#8`, `#73 #76 → #93`.
pub fn numbers(mut ns: Vec<u64>) -> String {
    ns.sort_unstable();
    ns.dedup();
    match ns.as_slice() {
        [] => String::new(),
        [one] => format!("#{one}"),
        [first, .., last] if usize::try_from(last - first).ok() == Some(ns.len() - 1) => {
            format!("#{first}–#{last}")
        }
        _ if ns.len() <= 4 => ns
            .iter()
            .map(|n| format!("#{n}"))
            .collect::<Vec<_>>()
            .join(" "),
        _ => format!("#{} #{} #{} → #{}", ns[0], ns[1], ns[2], ns[ns.len() - 1]),
    }
}

/// `owner/repo` and the number of a pull request URL.
fn pull_parts(url: &str) -> Option<(String, u64)> {
    let rest = url.strip_prefix("https://github.com/")?;
    let mut parts = rest.split('/');
    let owner = parts.next()?;
    let repo = parts.next()?;
    let _ = parts.next()?;
    let n = parts.next()?.parse().ok()?;
    Some((format!("{owner}/{repo}"), n))
}

/// The pull requests of one story, by repository: `widgets #4–#7`.
fn pulls_label(urls: &[String]) -> Vec<String> {
    let mut by_repo: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    for u in urls {
        if let Some((repo, n)) = pull_parts(u) {
            let short = repo.rsplit('/').next().unwrap_or(&repo).to_owned();
            by_repo.entry(short).or_default().push(n);
        }
    }
    by_repo
        .into_iter()
        .map(|(repo, ns)| format!("{repo} {}", numbers(ns)))
        .collect()
}

fn ended_of(threads: &[&InRange], now: OffsetDateTime) -> Option<Ended> {
    let last = threads.iter().max_by_key(|s| s.facts.last_turn)?;
    let ending = last.facts.ending(now);
    let limited_before = threads.iter().any(|s| {
        s.facts.ending(now) == Ending::UsageLimit && s.facts.last_turn < last.facts.last_turn
    });
    let (label, open, problem) = match ending {
        Ending::Open => ("still open".to_owned(), true, false),
        Ending::UsageLimit => (
            match &last.facts.ending_detail {
                Some(d) => format!("usage limit, {d}"),
                None => "usage limit".to_owned(),
            },
            false,
            true,
        ),
        Ending::RateLimited => ("rate-limited".to_owned(), false, true),
        Ending::Interrupted if last.facts.ending_detail.as_deref() == Some("stopped by you") => {
            ("stopped by you".to_owned(), false, false)
        }
        Ending::Interrupted if last.facts.ending_detail.as_deref() == Some(rollup::SIGNED_OUT) => {
            (rollup::SIGNED_OUT.to_owned(), false, true)
        }
        Ending::Completed | Ending::Interrupted if limited_before => {
            ("limit, then resumed".to_owned(), false, false)
        }
        Ending::Completed | Ending::Interrupted => return None,
    };
    Some(Ended {
        label,
        open,
        problem,
    })
}

/// `Opened 4 pull requests and merged 3`, `Merged one pull request`; `None`
/// when it did neither.
fn pulls_sentence(opened: usize, merged: usize) -> Option<String> {
    let n = |k: usize, one: &str, many: &str| count_word(k as u64, one, many);
    match (opened, merged) {
        (0, 0) => None,
        (o, 0) => Some(format!("opened {}", n(o, "pull request", "pull requests"))),
        (0, m) => Some(format!("merged {}", n(m, "pull request", "pull requests"))),
        (o, m) => Some(format!(
            "opened {} and merged {}",
            n(o, "pull request", "pull requests"),
            word(m as u64)
        )),
    }
}

/// The first sentence of `text`, plain, at most [`SENTENCE_CHARS`]: cut at
/// `. `, `! ` or `? `, else at a word with an ellipsis.
pub fn first_sentence(text: &str) -> String {
    let mut t = text.trim();
    loop {
        let end = t
            .char_indices()
            .find(|&(i, c)| matches!(c, '.' | '!' | '?') && t[i + 1..].starts_with(' '))
            .map(|(i, _)| i + 1);
        match end {
            // A leading word or two (`Yes.`, `Done.`) says little: the
            // next sentence is the news.
            Some(e) if t[..e].split_whitespace().count() < 3 => t = t[e..].trim_start(),
            Some(e) if t[..e].chars().count() > 8 => return sentences(&t[..e], SENTENCE_CHARS),
            _ => return sentences(t, SENTENCE_CHARS),
        }
    }
}

/// The final report's first real sentence: the first line of its text that
/// is not a heading, a fence, a table or a line of a few words (`Done.`),
/// as plain text; else the first line there is.
fn report_sentence(outcome: &str) -> Option<String> {
    let mut fenced = false;
    let mut fallback = None;
    for line in outcome.lines() {
        let t = line.trim();
        if t.starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced || t.is_empty() || t.starts_with('|') || t.starts_with('#') {
            continue;
        }
        let p = plain(t);
        if p.is_empty() {
            continue;
        }
        if p.split_whitespace().count() >= 4 {
            return Some(first_sentence(&p));
        }
        fallback.get_or_insert(p);
    }
    fallback.map(|p| first_sentence(&p))
}

/// What a story did, in at most three sentences and without a model: the
/// request (left out when the title says it already), the files edited and
/// pull requests opened and merged, and the first sentence of its final
/// report.
pub fn summary(
    title: &str,
    asked: Option<&str>,
    outcome: Option<&str>,
    files: u64,
    opened: &[String],
    merged: &[String],
) -> Option<String> {
    let mut out: Vec<String> = Vec::new();
    if let Some(request) = asked.map(hippo_core::session_meta::headline) {
        let request = plain(&request);
        let (r, t) = (request.to_lowercase(), title.to_lowercase());
        // A request that is only a link or a word or two says nothing.
        let said = r.split_whitespace().count() < 3
            || r.starts_with("http")
            || t.starts_with(r.trim_end_matches(['.', '…']))
            || r.starts_with(t.trim_end_matches(['.', '…']));
        if !said {
            let mut sentence = capitalise(&first_sentence(&request));
            if !sentence.ends_with(['.', '!', '?', '…']) {
                sentence.push('.');
            }
            out.push(sentence);
        }
    }
    let mut did = Vec::new();
    if files > 0 {
        did.push(format!("edited {}", count(files, "file", "files")));
    }
    let mut all: Vec<String> = opened.iter().chain(merged).cloned().collect();
    all.sort();
    all.dedup();
    if let Some(p) = pulls_sentence(opened.len(), merged.len()) {
        did.push(format!("{p} ({})", pulls_label(&all).join(", ")));
    }
    if !did.is_empty() {
        out.push(format!("{}.", capitalise(&did.join("; "))));
    }
    if let Some(r) = outcome.and_then(report_sentence) {
        out.push(r);
    }
    (!out.is_empty()).then(|| out.join(" "))
}

/// The story a session belongs to: its own, or its parent's for a
/// sub-agent.
fn root_of(s: &InRange) -> &str {
    s.facts.parent.as_deref().unwrap_or(&s.facts.id)
}

/// Builds the edition of `inp.date` from `rollup` and the rest of `inp`.
pub fn build(rollup: &Rollup, inp: &Inputs<'_>) -> Result<Edition> {
    let (start, end) = rollup::day_range(inp.date, inp.offset);
    let all = rollup.sessions_in(start, end)?;
    let mut agents_of: HashMap<&str, u64> = HashMap::new();
    // Each session's sub-agents that day, by parent.
    let mut children: HashMap<&str, Vec<&InRange>> = HashMap::new();
    for s in &all {
        if let Some(p) = s.facts.parent.as_deref() {
            *agents_of.entry(p).or_default() += 1;
            children.entry(p).or_default().push(s);
        }
    }
    let agents_total = all.iter().filter(|s| s.facts.parent.is_some()).count();
    let top: Vec<&InRange> = all
        .iter()
        .filter(|s| s.facts.parent.is_none() && !s.facts.automated)
        .collect();
    let turns: u64 = all.iter().map(|s| s.turns).sum();
    let edits: u64 = all.iter().map(|s| s.edits).sum();
    let pulls = rollup.pulls_in(start, end)?;
    // Pull requests by the story they belong to: a sub-agent's are its
    // parent's.
    let root: HashMap<&str, &str> = all
        .iter()
        .map(|s| (s.facts.id.as_str(), root_of(s)))
        .collect();
    let mut pulls_by_root: HashMap<&str, (Vec<String>, Vec<String>)> = HashMap::new();
    for p in &pulls {
        let r = root
            .get(p.session_id.as_str())
            .copied()
            .unwrap_or(p.session_id.as_str());
        let slot = pulls_by_root.entry(r).or_default();
        let list = match p.done {
            rollup::PullDone::Opened => &mut slot.0,
            rollup::PullDone::Merged => &mut slot.1,
        };
        if !list.contains(&p.url) {
            list.push(p.url.clone());
        }
    }

    // Stories: threads with one title together.
    let mut groups: Vec<(String, Vec<&InRange>)> = Vec::new();
    for s in &top {
        let title = title_of(s);
        let key = title.to_lowercase();
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, v)) => v.push(s),
            None => groups.push((key, vec![s])),
        }
    }
    let mut stories: Vec<Story> = groups
        .into_iter()
        .map(|(_, mut threads)| {
            threads.sort_by_key(|s| std::cmp::Reverse(s.turns));
            let lead = threads[0];
            let turns: u64 = threads.iter().map(|s| s.turns).sum();
            let edits: u64 = threads.iter().map(|s| s.edits).sum();
            let kids: Vec<&InRange> = threads
                .iter()
                .flat_map(|s| {
                    children
                        .get(s.facts.id.as_str())
                        .cloned()
                        .unwrap_or_default()
                })
                .collect();
            let files: u64 = threads
                .iter()
                .chain(kids.iter())
                .map(|s| s.facts.files)
                .sum();
            let agents: u64 = threads
                .iter()
                .map(|s| {
                    agents_of
                        .get(s.facts.id.as_str())
                        .copied()
                        .unwrap_or(s.facts.agents)
                })
                .sum();
            let (mut opened, mut merged) = (Vec::new(), Vec::new());
            for s in &threads {
                if let Some((o, m)) = pulls_by_root.get(s.facts.id.as_str()) {
                    opened.extend(o.iter().cloned());
                    merged.extend(m.iter().cloned());
                }
            }
            for list in [&mut opened, &mut merged] {
                list.sort();
                list.dedup();
            }
            let mut urls: Vec<String> = opened.iter().chain(&merged).cloned().collect();
            urls.sort();
            urls.dedup();
            // Where it worked: the project its threads and sub-agents point
            // at most; the lead's working directory's when none did.
            let mut weights: Vec<(String, u64)> = Vec::new();
            for s in threads.iter().chain(kids.iter()) {
                if let (Some(p), w) = (&s.facts.project, s.facts.project_weight) {
                    if w == 0 {
                        continue;
                    }
                    match weights.iter_mut().find(|(n, _)| n == p) {
                        Some(slot) => slot.1 += w,
                        None => weights.push((p.clone(), w)),
                    }
                }
            }
            let worked = weights
                .iter()
                .enumerate()
                .max_by(|(i, a), (j, b)| a.1.cmp(&b.1).then(j.cmp(i)))
                .map(|(_, (n, _))| n.clone());
            let first = threads.iter().filter_map(|s| s.first).min();
            let last = threads.iter().filter_map(|s| s.last).max();
            let ended = ended_of(&threads, inp.now);
            let mut facts = Vec::new();
            match (first, last) {
                // Busy before the day began and after it ended.
                (Some(a), Some(b)) if a == start && b == end => facts.push("all day".to_owned()),
                (Some(a), Some(b)) => {
                    let to = if ended.as_ref().is_some_and(|e| e.open) {
                        "open".to_owned()
                    } else if b == end {
                        "24:00".to_owned()
                    } else {
                        clock(b, inp.offset)
                    };
                    facts.push(format!("{} → {to}", clock(a, inp.offset)));
                    if (b - a).whole_minutes() >= 60 {
                        facts.push(span((b - a).whole_seconds()));
                    }
                }
                (Some(a), None) => facts.push(clock(a, inp.offset)),
                _ => {}
            }
            facts.push(count(turns, "turn", "turns"));
            if agents > 0 {
                facts.push(count(agents, "sub-agent", "sub-agents"));
            }
            if files > 0 {
                facts.push(count(files, "file", "files"));
            } else if edits > 0 {
                facts.push(count(edits, "edit", "edits"));
            }
            facts.extend(pulls_label(&urls));
            let mut profiles: Vec<String> =
                threads.iter().filter_map(|s| s.facts.profile()).collect();
            profiles.sort();
            profiles.dedup();
            if !profiles.is_empty() && profiles != ["default"] {
                facts.push(profiles.join(", "));
            }
            let title = title_of(lead);
            let summary = summary(
                &title,
                lead.facts.asked.as_deref(),
                lead.facts.outcome.as_deref(),
                files,
                &opened,
                &merged,
            );
            let mut project = worked.or_else(|| lead.facts.project.clone());
            if threads.len() > 1 {
                project = Some(format!(
                    "{} · {} threads",
                    project.unwrap_or_default(),
                    threads.len()
                ))
                .map(|p| p.trim_start_matches(" · ").to_owned());
            }
            Story {
                ids: threads.iter().map(|s| s.facts.id.clone()).collect(),
                title,
                project,
                summary,
                facts: facts.join(" · "),
                ended,
                score: edits + files * 5 + agents * 20 + urls.len() as u64 * 30 + turns / 20,
                opened,
                merged,
            }
        })
        .collect();
    stories.sort_by(|a, b| b.score.cmp(&a.score).then(a.title.cmp(&b.title)));
    let more: Vec<String> = stories
        .iter()
        .skip(STORIES)
        .map(|s| s.title.clone())
        .collect();
    // Limit stops are read from every story, listed or not. A stop whose
    // work a later session carried on (Home's rule, `home::carried_on`) is
    // no problem: its story says so and Problems leaves it out.
    let limit_stops: Vec<(usize, &SessionFacts, OffsetDateTime)> = stories
        .iter()
        .enumerate()
        .filter(|(_, s)| s.ended.as_ref().is_some_and(|e| e.problem))
        .filter_map(|(i, s)| {
            let f = &top
                .iter()
                .filter(|t| s.ids.contains(&t.facts.id))
                .max_by_key(|t| t.facts.last_turn)?
                .facts;
            let limited = matches!(f.ending(inp.now), Ending::UsageLimit | Ending::RateLimited);
            limited
                .then(|| f.ending_at.or(f.last_turn))
                .flatten()
                .map(|at| (i, f, at))
        })
        .collect();
    // Stops at the usage limit that day, carried on or not, for the deck.
    let usage_stops = limit_stops
        .iter()
        .filter(|(_, f, _)| f.ending(inp.now) == Ending::UsageLimit)
        .count();
    if let Some(since) = limit_stops
        .iter()
        .filter_map(|(_, f, _)| f.first_turn)
        .min()
    {
        let later = rollup.sessions_in(since, inp.now.max(end))?;
        let pulls = crate::home::pulls_by_session(rollup, &later);
        let carried: BTreeSet<usize> = limit_stops
            .iter()
            .filter(|(_, f, at)| crate::home::carried_on(f, *at, &later, &pulls, inp.now))
            .map(|(i, _, _)| *i)
            .collect();
        for i in carried {
            if let Some(e) = stories[i].ended.as_mut() {
                // A reset time is past news once the work carried on.
                e.label = if e.label.starts_with("usage limit") {
                    "usage limit, carried on later".to_owned()
                } else {
                    format!("{}, carried on later", e.label)
                };
                e.problem = false;
            }
        }
    }
    let stopped: Vec<(String, String, Option<String>, Option<OffsetDateTime>)> = stories
        .iter()
        .filter(|s| s.ended.as_ref().is_some_and(|e| e.problem))
        .map(|s| {
            let thread = top
                .iter()
                .filter(|t| s.ids.contains(&t.facts.id))
                .max_by_key(|t| t.facts.last_turn);
            (
                s.ids[0].clone(),
                s.title.clone(),
                s.ended.as_ref().map(|e| e.label.clone()),
                thread.and_then(|t| t.facts.ending_at),
            )
        })
        .collect();
    stories.truncate(STORIES);
    for s in stories.iter_mut().skip(LEADS) {
        s.summary = None;
    }

    // Hours.
    let mut hours = [0u64; 24];
    for (t, n, _) in rollup.activity(start, end)? {
        hours[usize::from(t.to_offset(inp.offset).hour())] += n;
    }
    let busiest = busiest(&hours);

    // Shipped.
    // By repository: every number, those opened, those merged, a URL.
    type Numbers = (BTreeSet<u64>, BTreeSet<u64>, BTreeSet<u64>, String);
    let mut repos: BTreeMap<String, Numbers> = BTreeMap::new();
    for p in &pulls {
        if let Some((repo, n)) = pull_parts(&p.url) {
            let e = repos.entry(repo).or_insert_with(|| {
                (
                    BTreeSet::new(),
                    BTreeSet::new(),
                    BTreeSet::new(),
                    p.url.clone(),
                )
            });
            e.0.insert(n);
            match p.done {
                rollup::PullDone::Opened => e.1.insert(n),
                rollup::PullDone::Merged => e.2.insert(n),
            };
        }
    }
    let mut shipped: Vec<Shipped> = repos
        .into_iter()
        .map(|(repo, (ns, opened, merged, url))| Shipped {
            repo,
            numbers: ns.into_iter().collect(),
            opened: opened.len(),
            merged: merged.len(),
            url,
        })
        .collect();
    shipped.sort_by(|a, b| {
        b.numbers
            .len()
            .cmp(&a.numbers.len())
            .then(a.repo.cmp(&b.repo))
    });
    let opened_count: usize = shipped.iter().map(|s| s.opened).sum();
    let merged_count: usize = shipped.iter().map(|s| s.merged).sum();

    // The memory learned.
    let titles: HashMap<&str, String> = top
        .iter()
        .map(|s| (s.facts.id.as_str(), title_of(s)))
        .collect();
    let mut notes: Vec<NoteChange> = Vec::new();
    for row in inp.notes {
        let Ok(meta) = std::fs::metadata(inp.root.join(&row.path)) else {
            continue;
        };
        let Some(modified) = meta.modified().ok().map(OffsetDateTime::from) else {
            continue;
        };
        if modified < start || modified >= end {
            continue;
        }
        let created = meta.created().ok().map(OffsetDateTime::from);
        let new = created.is_some_and(|c| c >= start && c < end);
        let editor = rollup.sessions_of_note(&row.path).ok().and_then(|seen| {
            seen.into_iter()
                .filter(|n| n.kind == "edit")
                .find_map(|n| titles.get(n.session_id.as_str()).cloned())
        });
        let detail = clock(modified, inp.offset);
        notes.push(NoteChange {
            path: row.path.clone(),
            name: note_name(&row.path, &row.title),
            new,
            detail,
            editor,
            modified,
        });
    }
    notes.sort_by_key(|n| std::cmp::Reverse(n.modified));
    let notes_changed = notes.len();

    // Dreaming.
    let run = inp
        .runs
        .iter()
        .find(|r| run_day(r, inp.offset) == Some(inp.date));
    let dreaming = run.map(|r| dreaming_paragraph(r, inp.offset));
    let dreaming_report = run.map(|r| r.report.path.clone());

    // Problems: what a person can act on.
    let mut problems = Vec::new();
    let at_limit: Vec<&(String, String, Option<String>, Option<OffsetDateTime>)> = stopped
        .iter()
        .filter(|s| s.2.as_deref().is_some_and(|l| l.starts_with("usage limit")))
        .collect();
    for (id, title, label, at) in at_limit
        .iter()
        .take(LIMITS)
        .map(|s| (&s.0, &s.1, &s.2, s.3))
    {
        let reset = label
            .as_deref()
            .and_then(|l| l.strip_prefix("usage limit, "))
            .map(str::to_owned);
        let mut detail: Vec<String> = Vec::new();
        detail.extend(at.map(|t| clock(t, inp.offset)));
        detail.extend(reset);
        detail.push("open it to resume".to_owned());
        problems.push(Problem {
            error: true,
            text: format!("{title} stopped at the usage limit"),
            detail: Some(detail.join(" · ")),
            session: Some(id.clone()),
            note: None,
        });
    }
    if at_limit.len() > LIMITS {
        problems.push(Problem {
            error: true,
            text: format!(
                "{} more stopped at the usage limit",
                count((at_limit.len() - LIMITS) as u64, "session", "sessions")
            ),
            detail: None,
            session: None,
            note: None,
        });
    }
    for (id, title, _, at) in stopped
        .iter()
        .filter(|s| s.2.as_deref() == Some(rollup::SIGNED_OUT))
    {
        problems.push(Problem {
            error: true,
            text: format!("{title} stopped: its sign-in failed"),
            detail: Some(match at {
                Some(t) => format!(
                    "{} · run claude auth login, then open it to resume",
                    clock(*t, inp.offset)
                ),
                None => "run claude auth login, then open it to resume".to_owned(),
            }),
            session: Some(id.clone()),
            note: None,
        });
    }
    for (id, title, _, at) in stopped
        .iter()
        .filter(|s| s.2.as_deref() == Some("rate-limited"))
    {
        problems.push(Problem {
            error: true,
            text: format!("{title} was rate-limited"),
            detail: Some(
                at.map(|t| format!("{} · open it to resume", clock(t, inp.offset)))
                    .unwrap_or_else(|| "open it to resume".to_owned()),
            ),
            session: Some(id.clone()),
            note: None,
        });
    }
    let mut seen_events: BTreeMap<String, (Vec<String>, String)> = BTreeMap::new();
    for e in rollup.events_in(start, end)? {
        let text = match e.kind.as_str() {
            "delegate_failed" => {
                let reason = e
                    .detail
                    // `<task name>: <reason>`; a name can hold `: ` too.
                    .rsplit_once(": ")
                    .map_or(e.detail.as_str(), |(_, r)| r);
                format!("Delegated task failed: {}", reason.trim_end_matches('.'))
            }
            _ => e.detail.clone(),
        };
        let at = OffsetDateTime::parse(&e.ts, &time::format_description::well_known::Rfc3339)
            .map(|t| clock(t, inp.offset))
            .unwrap_or_default();
        let entry = seen_events
            .entry(text)
            .or_insert_with(|| (Vec::new(), e.session_id.clone()));
        if !entry.0.contains(&at) {
            entry.0.push(at);
        }
    }
    for (text, (mut at, session)) in seen_events {
        at.sort();
        let n = at.len();
        problems.push(Problem {
            error: true,
            text,
            detail: Some(if n > 3 {
                format!("{n} times from {}", at[0])
            } else {
                at.join(", ")
            }),
            session: Some(session),
            note: None,
        });
    }
    if let Some(r) = run {
        if let Ok(rec) = &r.report.record {
            if rec.status.failed {
                problems.push(Problem {
                    error: true,
                    text: "Dreaming failed".to_owned(),
                    detail: rec.status.error.clone(),
                    session: None,
                    note: None,
                });
            }
        }
    }
    let today = inp.now.to_offset(inp.offset).date();
    if let Some(last) = inp.ingest_stopped {
        if inp.date >= today.previous_day().unwrap_or(today) {
            problems.push(Problem {
                error: true,
                text: format!(
                    "Ingest has not run since {}",
                    when(last, inp.now, inp.offset)
                ),
                detail: Some("new sessions are missing; run hippo doctor".to_owned()),
                session: None,
                note: None,
            });
        }
    }

    // Health: the engine's housekeeping.
    let mut health = Vec::new();
    if let Some(d) = run.and_then(|r| r.digest.as_ref()) {
        let big: BTreeSet<&str> = d
            .skipped
            .iter()
            .filter(|s| too_big(&s.why))
            .map(|s| s.note.as_str())
            .collect();
        match big.len() {
            0 => {}
            1 => health.push(format!(
                "{} was too big for dreaming to read whole",
                note_title(big.iter().next().copied().unwrap_or_default())
            )),
            n => health.push(format!(
                "{} were too big for dreaming to read whole",
                capitalise(&count_word(n as u64, "note", "notes"))
            )),
        }
    }
    health.extend(handoff_health(inp.records, inp.traces, start, end));

    let in_flight = top
        .iter()
        .filter(|s| s.facts.ending(inp.now) == Ending::Open)
        .count();

    // The deck.
    let mut deck = Vec::new();
    let strong = |t: String| Span {
        text: t,
        strong: true,
    };
    let weak = |t: &str| Span {
        text: t.to_owned(),
        strong: false,
    };
    if top.is_empty() {
        deck.push(weak("No session ran that day."));
    } else {
        deck.push(strong(capitalise(&count(
            top.len() as u64,
            "session",
            "sessions",
        ))));
        if agents_total > 0 {
            // Sub-agents, said as such: a day of delegated work runs to
            // hundreds of them, which is not hundreds of agents.
            deck.push(weak(" and "));
            deck.push(strong(count(
                agents_total as u64,
                "sub-agent",
                "sub-agents",
            )));
            deck.push(weak(&format!(
                " ran {} and {}.",
                count(turns, "turn", "turns"),
                count(edits, "edit", "edits")
            )));
        } else {
            deck.push(weak(&format!(
                " ran {} and {}.",
                count(turns, "turn", "turns"),
                count(edits, "edit", "edits")
            )));
        }
    }
    if let Some(p) = pulls_sentence(opened_count, merged_count) {
        deck.push(weak(&format!(" They {p}.")));
    }
    if notes_changed > 0 {
        deck.push(weak(&format!(
            " {} changed.",
            capitalise(&count_word(notes_changed as u64, "note", "notes"))
        )));
    }
    if let Some(r) = run {
        deck.push(weak(&format!(" {}", dreaming_sentence(r))));
    }
    // Agrees with Home's Resume rows: a stop carried on later is said as
    // such, and only the rest is news.
    let waiting = at_limit.len();
    if usage_stops > 0 {
        let stops = capitalise(&count_word(usage_stops as u64, "session", "sessions"));
        deck.push(weak(" "));
        match (usage_stops, waiting) {
            (1, 0) => deck.push(weak(&format!(
                "{stops} stopped at the usage limit and was carried on later."
            ))),
            (_, 0) => deck.push(weak(&format!(
                "{stops} stopped at the usage limit; all were carried on later."
            ))),
            (n, w) if n == w => deck.push(strong(format!("{stops} stopped at the usage limit."))),
            (_, w) => {
                deck.push(weak(&format!("{stops} stopped at the usage limit; ")));
                deck.push(strong(format!(
                    "{} {} not carried on.",
                    word(w as u64),
                    if w == 1 { "was" } else { "were" }
                )));
            }
        }
    }

    let local = start.to_offset(inp.offset);
    Ok(Edition {
        date: inp.date,
        kicker: format!(
            "The edition · {}",
            local
                .format(format_description!("[weekday]"))
                .unwrap_or_default()
        ),
        headline: format!(
            "{} {} {}",
            inp.date.day(),
            inp.date.month(),
            inp.date.year()
        ),
        deck,
        hours,
        busiest,
        stories,
        more,
        shipped,
        notes_changed,
        notes,
        dreaming,
        dreaming_report,
        problems,
        health,
        in_flight,
        sessions: top.len(),
        agents: agents_total,
    })
}

/// A time as a short phrase at `now`: `02:44` today, `yesterday 03:30`,
/// `Sat 7 Nov 03:30` before.
pub fn when(t: OffsetDateTime, now: OffsetDateTime, offset: UtcOffset) -> String {
    let (local, today) = (t.to_offset(offset), now.to_offset(offset).date());
    let at = clock(t, offset);
    if local.date() == today {
        at
    } else if Some(local.date()) == today.previous_day() {
        format!("yesterday {at}")
    } else {
        format!(
            "{} {at}",
            local
                .format(format_description!(
                    "[weekday repr:short] [day padding:none] [month repr:short]"
                ))
                .unwrap_or_default()
        )
    }
}

/// A skipped note's reason is its size: a size in it, or "too large".
/// Notes left out by rank are not too big.
fn too_big(why: &str) -> bool {
    why.contains("too large") || why_bytes(why) > 0.0
}

/// `busiest 13:00–15:00`: the two busiest consecutive hours.
fn busiest(hours: &[u64; 24]) -> Option<String> {
    let (i, total) = (0..23)
        .map(|i| (i, hours[i] + hours[i + 1]))
        .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))?;
    (total > 0).then(|| format!("busiest {i:02}:00–{:02}:00", i + 2))
}

/// `Folder / Title` for a note at `path` (relative to the workspace).
pub fn note_name(path: &Path, title: &str) -> String {
    let folder = path
        .parent()
        .and_then(|p| p.file_name())
        .map(|f| f.to_string_lossy().into_owned());
    // The notes directory itself is not a folder worth naming.
    let depth = path.components().count();
    match folder {
        Some(f) if depth > 2 => format!("{} / {title}", visible(&f)),
        _ => title.to_owned(),
    }
}

/// A note's title from its path: the file name without `.md`.
fn note_title(path: &str) -> String {
    let name = Path::new(path)
        .file_stem()
        .map_or_else(|| path.to_owned(), |s| s.to_string_lossy().into_owned());
    visible(&name)
}

fn run_clock(run: &Run, offset: UtcOffset) -> Option<String> {
    run.written.map(|t| clock(OffsetDateTime::from(t), offset))
}

/// The deck's sentence about dreaming.
fn dreaming_sentence(run: &Run) -> String {
    let status = run.report.record.as_ref().ok().map(|r| &r.status);
    if status.is_some_and(|s| s.failed) {
        return "Dreaming failed.".to_owned();
    }
    let Some(d) = run.digest.as_ref() else {
        return "Dreaming ran.".to_owned();
    };
    let checked = d.notes_checked().len() as u64;
    let (confirmed, _) = verdicts(d);
    let dry = if status.is_some_and(|s| s.dry_run) {
        " in a dry run"
    } else {
        ""
    };
    if confirmed == 0 {
        format!(
            "Dreaming checked {}{dry} and confirmed nothing.",
            count_word(checked, "note", "notes")
        )
    } else {
        format!(
            "Dreaming checked {}{dry} and confirmed {}.",
            count_word(checked, "note", "notes"),
            count_word(confirmed, "change", "changes")
        )
    }
}

fn verdicts(d: &Digest) -> (u64, u64) {
    let confirmed = d
        .verdicts
        .values()
        .filter(|v| v.verdict == "confirm")
        .count() as u64;
    let rejected = d
        .verdicts
        .values()
        .filter(|v| v.verdict == "reject")
        .count() as u64;
    (confirmed, rejected)
}

/// The Memory column's paragraph: `("Dreaming, 22:55.", "Checked three
/// notes against 173 sessions; confirmed nothing. …")`.
fn dreaming_paragraph(run: &Run, offset: UtcOffset) -> (String, String) {
    let status = run.report.record.as_ref().ok().map(|r| &r.status);
    let at = run_clock(run, offset);
    let lead = match (status.is_some_and(|s| s.dry_run), &at) {
        (true, Some(at)) => format!("Dreaming dry run, {at}."),
        (true, None) => "Dreaming dry run.".to_owned(),
        (false, Some(at)) => format!("Dreaming, {at}."),
        (false, None) => "Dreaming.".to_owned(),
    };
    if let Some(s) = status.filter(|s| s.failed) {
        return (
            lead,
            format!(
                "Failed: {}",
                s.error
                    .clone()
                    .unwrap_or_else(|| "see the report".to_owned())
            ),
        );
    }
    let Some(d) = run.digest.as_ref() else {
        return (lead, "Its report could not be read.".to_owned());
    };
    let mut out = Vec::new();
    let checked = d.notes_checked();
    let sessions = d.sessions_read() as u64;
    let (confirmed, rejected) = verdicts(d);
    let mut first = format!(
        "Checked {}",
        count_word(checked.len() as u64, "note", "notes")
    );
    if sessions > 0 {
        first.push_str(&format!(
            " against {}",
            count(sessions, "session", "sessions")
        ));
    }
    first.push_str("; ");
    first.push_str(&match (confirmed, rejected) {
        (0, 0) => "confirmed nothing.".to_owned(),
        (c, 0) => format!("confirmed {}.", count_word(c, "change", "changes")),
        (0, r) => format!("rejected {}.", count_word(r, "proposal", "proposals")),
        (c, r) => format!(
            "confirmed {} and rejected {}.",
            count_word(c, "change", "changes"),
            word(r)
        ),
    });
    out.push(first);
    if !d.failures.is_empty() {
        let on: Vec<String> = d
            .failures
            .iter()
            .filter_map(|f| f.note.as_deref().map(note_title))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let n = d.failures.len() as u64;
        out.push(if on.is_empty() {
            format!(
                "{} failed validation.",
                capitalise(&count_word(n, "proposal", "proposals"))
            )
        } else {
            format!(
                "{} on {} failed validation.",
                capitalise(&count_word(n, "proposal", "proposals")),
                on.join(", ")
            )
        });
    }
    // Largest first, by the size the reason names.
    let mut sized: BTreeMap<String, f64> = BTreeMap::new();
    for s in d.skipped.iter().filter(|s| too_big(&s.why)) {
        let bytes = why_bytes(&s.why);
        let slot = sized.entry(note_title(&s.note)).or_insert(bytes);
        *slot = slot.max(bytes);
    }
    let mut big: Vec<(String, f64)> = sized.into_iter().collect();
    big.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    let big: Vec<String> = big.into_iter().map(|(n, _)| n).collect();
    match big.len() {
        0 => {}
        1 => out.push(format!("{} was too big to check whole.", big[0])),
        2 | 3 => out.push(format!("{} were too big to check whole.", join_and(&big))),
        n => out.push(format!(
            "{} were too big to check whole; the largest are {}.",
            capitalise(&count_word(n as u64, "note", "notes")),
            join_and(&big[..2])
        )),
    }
    (lead, out.join(" "))
}

/// The size a reason names, in bytes; 0 when it names none.
fn why_bytes(why: &str) -> f64 {
    let words: Vec<&str> = why.split_whitespace().collect();
    words
        .windows(2)
        .find_map(|w| {
            let n: f64 = w[0].parse().ok()?;
            match w[1].trim_end_matches([',', '.', ';']) {
                "B" => Some(n),
                "KB" => Some(n * 1024.0),
                "MB" => Some(n * 1024.0 * 1024.0),
                _ => None,
            }
        })
        .unwrap_or(0.0)
}

fn join_and(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)] // the tests build a synthetic home
mod tests;
