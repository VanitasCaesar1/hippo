//! The Home: what needs the person, then the day.
//!
//! [`build`] reads the editions of today and yesterday (already built by
//! [`crate::edition`]), the dreaming history with its review marks
//! (`hippo_core::dream::seen`), and the rollup, and returns a [`Home`]: a
//! headline chosen by rule, one quiet status line, the "Needs you" rows
//! (Review, Answer, Resume, Failures), Today and Yesterday condensed,
//! Shipped, and the memory's week. It writes nothing; the app runs `hippo
//! review --ok` and `hippo revert` for the rows' buttons.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use hippo_core::config::Workspace;
use hippo_core::dream::{History, RevertState};
use hippo_core::records::Verdict;
use hippo_core::rollup::{self, Ending, Rollup, SessionFacts};
use time::{Date, Duration, OffsetDateTime, UtcOffset};
use ulid::Ulid;

use crate::brain::NoteRow;
use crate::dreaming::Run;
use crate::edition::{thousands, Edition, Shipped, Span};
use crate::status::Status;
use crate::text::visible;
use crate::worddiff::WordDiff;

/// Applied changes older than this leave the Review row: a missed week is
/// in the Dreaming panel, not on Home.
const REVIEW_DAYS: i64 = 7;
/// Threads that asked a question, at most.
const ANSWERS: usize = 3;
/// A review quote's old and new text at most, in characters, cut at a
/// sentence or a word; the app fits them to two lines.
const OLD_CHARS: usize = 120;
const NEW_CHARS: usize = 200;
/// The old and new text in a review row's tooltip at most, in characters.
const FULL_CHARS: usize = 600;
/// How far back Resume looks for stopped threads.
const LOOKBACK_DAYS: i64 = 7;
/// A stop older than this leaves Resume once anything later worked in its
/// project.
const STALE_HOURS: i64 = 24;
/// Delegated tasks and API errors older than this are history, not news.
const FAILURE_HOURS: i64 = 24;
/// Resume rows named one each; the rest are counted.
const RESUMES: usize = 4;

/// One applied dreaming change a person has not marked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewItem {
    pub receipt: Ulid,
    /// The change, to open: its tab holds Revert, behind a confirmation.
    pub proposal: Ulid,
    pub note: PathBuf,
    /// `Folder / Title`.
    pub label: String,
    /// `“old” → new`: [`ReviewItem::old`] and [`ReviewItem::new`] as one
    /// line.
    pub change: String,
    /// What the change replaced, as plain text in whole phrases; `None` for
    /// an addition.
    pub old: Option<String>,
    /// What it put there, as plain text in whole phrases.
    pub new: String,
    /// What changed, word by word, in the whole old and new text as plain
    /// text; `None` for an addition. The row shows an excerpt of it.
    pub diff: Option<WordDiff>,
    /// The old and new text, longer: the row's tooltip.
    pub full: String,
    /// The note changed after the fix: `an agent edited it since, 09:37`
    /// when a session that edited it was running then, else `edited since,
    /// 09:37`. Never "you": a hand edit and an agent's look the same on
    /// disk.
    pub edited_since: Option<String>,
    /// That session's title, when it is known.
    pub edited_by: Option<String>,
    /// `hippo revert` arguments for undo; `None` when a newer change to the
    /// note must be undone first, or the snapshot is gone.
    pub undo: Option<Vec<String>>,
}

/// One row of "Needs you".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Need {
    /// Dreaming fixed notes no one has looked at.
    Review {
        lead: Vec<Span>,
        items: Vec<ReviewItem>,
        /// The report of the newest run among them, to open.
        report: Option<PathBuf>,
    },
    /// A thread whose last reply asks the person something.
    Answer { text: Vec<Span>, session: String },
    /// A thread stopped at the usage limit and not picked up again.
    Resume {
        text: Vec<Span>,
        session: Option<String>,
    },
    /// A failure to act on, still unresolved.
    Failure {
        text: Vec<Span>,
        session: Option<String>,
        note: Option<PathBuf>,
        /// The person can fix it (sign in again, look at a failed run or a
        /// stopped ingest): it comes first.
        fix: bool,
    },
}

/// The Home.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Home {
    /// `Home · Monday 9 November · 02:50`.
    pub kicker: String,
    pub headline: String,
    /// One line: what hippo reads and when, the notes, dreaming, health.
    pub status: Vec<Span>,
    /// `health ok`, `health: 2 warnings`.
    pub health: String,
    pub health_ok: bool,
    /// What the health count stands for: the warning lines agents get
    /// (`warning: last dreaming run failed`), which the app explains.
    pub health_warnings: Vec<String>,
    /// And the rest of it, in plain words: a stopped ingest, data that
    /// could not be read, today's housekeeping.
    pub health_notes: Vec<String>,
    /// Only rows with something to do: a kind with nothing to show has no
    /// row.
    pub needs: Vec<Need>,
    /// Today so far, in a sentence.
    pub today: Vec<Span>,
    pub today_date: Option<Date>,
    pub yesterday: Option<Edition>,
    pub shipped: Vec<Shipped>,
    /// The week of memory; `None` without a notes folder.
    pub memory: Option<Memory>,
    /// The dock row's second line: `2 things need you · read at 02:44`.
    pub dock_line: String,
}

/// The memory's week.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Memory {
    pub lines: Vec<Vec<Span>>,
    /// Notes created this week, by path and title, to open.
    pub new_notes: Vec<(PathBuf, String)>,
}

/// What [`build`] reads.
pub struct Inputs<'a> {
    pub now: OffsetDateTime,
    pub offset: UtcOffset,
    pub today: Option<&'a Edition>,
    pub yesterday: Option<&'a Edition>,
    pub rollup: Option<&'a Rollup>,
    pub ws: Option<&'a Workspace>,
    pub history: Option<&'a History>,
    /// Receipts a person marked.
    pub marked: &'a HashSet<Ulid>,
    pub notes: &'a [NoteRow],
    pub runs: &'a [Run],
    pub status: Option<&'a Status>,
    /// The profiles the rollup has seen (`default`, `work`, `codex`).
    pub profiles: &'a [String],
    pub ingest_interval_minutes: u64,
    pub dream_at: &'a str,
}

fn plain(text: impl Into<String>) -> Span {
    Span {
        text: text.into(),
        strong: false,
    }
}

fn strong(text: impl Into<String>) -> Span {
    Span {
        text: text.into(),
        strong: true,
    }
}

fn hm(t: OffsetDateTime, offset: UtcOffset) -> String {
    let t = t.to_offset(offset);
    format!("{:02}:{:02}", t.hour(), t.minute())
}

fn words(n: usize) -> String {
    const SMALL: [&str; 11] = [
        "no", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
    ];
    SMALL
        .get(n)
        .map_or_else(|| thousands(n as u64), |w| (*w).to_owned())
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{} {}", words(n), if n == 1 { one } else { many })
}

/// `Folder / Title` for a note path, from the Brain rows when known.
fn note_label(path: &Path, notes: &[NoteRow]) -> String {
    let title = notes
        .iter()
        .find(|n| n.path == path)
        .map(|n| n.title.clone())
        .unwrap_or_else(|| {
            path.file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        });
    let folder = path
        .parent()
        .and_then(|p| p.components().nth(1))
        .map(|c| c.as_os_str().to_string_lossy().into_owned());
    match folder {
        Some(f) => format!("{f} / {title}"),
        None => title,
    }
}

/// Markdown as plain text on one line, cut to whole sentences, else at a
/// word, within `max` characters: what a quote on Home shows.
fn phrase(text: &str, max: usize) -> String {
    crate::edition::sentences(&crate::edition::plain(text), max)
}

/// The Review row: applied, unreverted, unmarked changes of the last week.
fn review(inp: &Inputs<'_>) -> Option<Need> {
    let (ws, history) = (inp.ws?, inp.history?);
    let since = inp.now - Duration::days(REVIEW_DAYS);
    let mut items = Vec::new();
    let mut newest: Option<OffsetDateTime> = None;
    let mut notes: HashSet<PathBuf> = HashSet::new();
    for change in &history.changes {
        let Ok(proposal) = change.proposal.record.as_ref() else {
            continue;
        };
        let reverted = change.receipts.iter().any(|r| r.verdict == Verdict::Revert);
        let Some(confirm) = change
            .receipts
            .iter()
            .find(|r| r.verdict == Verdict::Confirm && r.applied.is_some())
        else {
            continue;
        };
        let applied = confirm.applied.unwrap_or(confirm.at);
        if reverted || applied < since || inp.marked.contains(&confirm.id) {
            continue;
        }
        newest = Some(newest.map_or(applied, |n| n.max(applied)));
        notes.insert(proposal.note.clone());
        let state = hippo_core::dream::revert_state_in(ws, history, confirm.id).ok();
        let (edited_since, edited_by) = if matches!(state, Some(RevertState::NoteChanged)) {
            let modified = std::fs::metadata(ws.root.join(&proposal.note))
                .and_then(|m| m.modified())
                .ok()
                .map(OffsetDateTime::from);
            let (text, by) = edited_since(inp, &proposal.note, applied, modified);
            (Some(text), by)
        } else {
            (None, None)
        };
        let undo = match state {
            Some(RevertState::Revertible) => Some(vec!["revert".into(), confirm.id.to_string()]),
            Some(RevertState::NoteChanged) => Some(vec![
                "revert".into(),
                "--force".into(),
                confirm.id.to_string(),
            ]),
            _ => None,
        };
        let old = (!proposal.old.trim().is_empty()).then(|| phrase(&proposal.old, OLD_CHARS));
        let new = phrase(&proposal.new, NEW_CHARS);
        let change_text = match &old {
            None => format!("added “{new}”"),
            Some(old) => format!("“{old}” → {new}"),
        };
        let diff = old.is_some().then(|| {
            WordDiff::new(
                &crate::edition::plain(&proposal.old),
                &crate::edition::plain(&proposal.new),
            )
        });
        let full = match &old {
            None => format!("Added: {}", phrase(&proposal.new, FULL_CHARS)),
            Some(_) => format!(
                "Was: {}\n\nNow: {}",
                phrase(&proposal.old, FULL_CHARS),
                phrase(&proposal.new, FULL_CHARS)
            ),
        };
        items.push(ReviewItem {
            receipt: confirm.id,
            proposal: change.proposal_id,
            note: proposal.note.clone(),
            label: note_label(&proposal.note, inp.notes),
            change: change_text,
            old,
            new,
            diff,
            full,
            edited_since,
            edited_by,
            undo,
        });
    }
    if items.is_empty() {
        return None;
    }
    let when = newest.map(|t| hm(t, inp.offset)).unwrap_or_default();
    let lead = vec![
        plain("Dreaming fixed "),
        strong(plural(notes.len(), "note", "notes")),
        plain(format!(
            " at {when}: {}, each confirmed by a fresh reviewer, not yet seen by you.",
            plural(items.len(), "change", "changes")
        )),
    ];
    let report = inp.runs.first().map(|r| r.report.path.clone());
    Some(Need::Review {
        lead,
        items,
        report,
    })
}

/// `an agent edited it since, 09:37` and the session's title when one
/// session that edited the note (as the rollup saw in its transcript) was
/// running when the file last changed; else `edited since, 09:37`. hippo
/// cannot tell a person's edit from an agent's, so it never says "you".
fn edited_since(
    inp: &Inputs<'_>,
    note: &Path,
    applied: OffsetDateTime,
    modified: Option<OffsetDateTime>,
) -> (String, Option<String>) {
    let Some(t) = modified else {
        return ("edited since".to_owned(), None);
    };
    let at = when(t, inp.now, inp.offset);
    match editor(inp, note, applied, t) {
        Some(by) => (format!("an agent edited it since, {at}"), Some(by)),
        None => (format!("edited since, {at}"), None),
    }
}

/// The title of the session that edited `note` after `applied` and was
/// running when the file changed at `modified`; `None` when no session, or
/// more than one, fits.
fn editor(
    inp: &Inputs<'_>,
    note: &Path,
    applied: OffsetDateTime,
    modified: OffsetDateTime,
) -> Option<String> {
    let rollup = inp.rollup?;
    // A session's row carries the first turn that named the note, which
    // can be well before its edit.
    let seen = rollup
        .notes_in(applied - Duration::days(REVIEW_DAYS), inp.now)
        .ok()?;
    let mut ids: Vec<&str> = seen
        .iter()
        .filter(|s| s.kind == "edit" && s.note == note)
        .map(|s| s.session_id.as_str())
        .collect();
    ids.sort_unstable();
    ids.dedup();
    let fits: Vec<SessionFacts> = ids
        .into_iter()
        .filter_map(|id| rollup.session(id).ok().flatten())
        .filter(|f| {
            let (Some(first), Some(last)) = (f.first_turn, f.last_turn) else {
                return false;
            };
            last > applied
                && first - Duration::minutes(1) <= modified
                && modified <= last + Duration::minutes(5)
        })
        .collect();
    match fits.as_slice() {
        [one] => Some(title(one)),
        _ => None,
    }
}

/// Whether a last reply asks the person something.
fn asks(reply: &str) -> Option<String> {
    let text = reply.trim();
    let tail: String = {
        let chars: Vec<char> = text.chars().collect();
        chars[chars.len().saturating_sub(300)..].iter().collect()
    };
    let lower = tail.to_lowercase();
    let question = text.ends_with('?')
        || lower.contains("tell me")
        || lower.contains("let me know")
        || lower.contains("which do you want")
        || lower.contains("should i ");
    if !question {
        return None;
    }
    // The last sentence, as it is quoted on the row.
    let sentence = text
        .rsplit(['.', '\n'])
        .map(str::trim)
        .find(|s| !s.is_empty())
        .unwrap_or(text);
    Some(phrase(sentence, 120))
}

/// The Answer rows: top-level threads of the last day whose last reply asks
/// the person something, newest first.
fn answers(inp: &Inputs<'_>) -> Vec<Need> {
    let Some(rollup) = inp.rollup else {
        return Vec::new();
    };
    let Ok(mut recent) = rollup.sessions_in(inp.now - Duration::hours(24), inp.now) else {
        return Vec::new();
    };
    recent.sort_by_key(|s| std::cmp::Reverse(s.facts.last_turn));
    recent
        .iter()
        .filter(|s| s.facts.parent.is_none() && !s.facts.automated)
        .filter(|s| s.facts.ending(inp.now) == Ending::Completed)
        .filter_map(|s| {
            let quote = asks(s.facts.outcome.as_deref()?)?;
            let title = s.facts.title.clone().unwrap_or_else(|| s.facts.id.clone());
            let at = s
                .facts
                .last_turn
                .map(|t| hm(t, inp.offset))
                .unwrap_or_default();
            let mut text = vec![strong(title)];
            if let Some(p) = &s.facts.project {
                text.push(plain(format!(" · {p}")));
            }
            text.push(plain(format!(", asked at {at}: “{quote}”")));
            Some(Need::Answer {
                text,
                session: s.facts.id.clone(),
            })
        })
        .take(ANSWERS)
        .collect()
}

// ---------------------------------------------------------------------------
// Stops and failures, kept only while they are true

/// A session's title: the rollup's, else its request's first line.
pub(crate) fn title(f: &SessionFacts) -> String {
    // A slash command (`/compact`) names no work: the last reply does.
    let asked = f
        .asked
        .as_deref()
        .filter(|a| !crate::fleet::is_slash_command(a) || f.outcome.is_none());
    f.title
        .clone()
        .or_else(|| asked.map(hippo_core::session_meta::headline))
        .or_else(|| f.outcome.as_deref().map(hippo_core::session_meta::headline))
        .filter(|t| !t.trim().is_empty())
        .map(|t| visible(&t))
        .unwrap_or_else(|| f.id.chars().take(8).collect())
}

/// Words too common in titles to say two are about the same work.
const COMMON: &[&str] = &[
    "a", "an", "the", "and", "or", "of", "to", "for", "in", "on", "at", "with", "by", "from",
    "into", "is", "it", "its", "this", "that", "do", "make", "fix", "fixes", "fixed", "add",
    "update", "new", "work", "task", "finish", "continue", "resume", "review",
];

/// A title's words that say what the work is: lower case, plural `s`
/// dropped, common words and the project's own name left out.
fn title_words(title: &str, project: Option<&str>) -> BTreeSet<String> {
    let project: Vec<String> = project
        .map(|p| {
            p.split(|c: char| !c.is_alphanumeric())
                .map(str::to_lowercase)
                .collect()
        })
        .unwrap_or_default();
    title
        .split(|c: char| !c.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|w| !w.is_empty() && !COMMON.contains(&w.as_str()))
        .map(|w| match w.strip_suffix('s') {
            Some(stem) if stem.chars().count() >= 3 && !stem.ends_with('s') => stem.to_owned(),
            _ => w,
        })
        .filter(|w| !COMMON.contains(&w.as_str()) && !project.contains(w))
        .collect()
}

/// Whether two titles name the same work: at least two telling words in
/// common, and at least half of the shorter title's.
fn same_work(a: &str, b: &str, project: Option<&str>) -> bool {
    let (a, b) = (title_words(a, project), title_words(b, project));
    let shared = a.intersection(&b).count();
    let shorter = a.len().min(b.len());
    shorter >= 2 && shared >= 2 && shared * 2 >= shorter
}

/// A request that says it carries on stopped work.
fn says_it_resumes(asked: &str) -> bool {
    let lower = asked.to_lowercase();
    [
        "resum",
        "continue",
        "carry on",
        "pick up",
        "picking up",
        "left off",
        "interrupted",
        "previous agent",
        "rate limit",
        "usage limit",
    ]
    .iter()
    .any(|w| lower.contains(w))
}

/// Why `later` carries on the work of `stop`, which stopped at `at`; `None`
/// when nothing says it does. `pulls` are each session's pull requests.
pub(crate) fn continues(
    stop: &SessionFacts,
    at: OffsetDateTime,
    later: &SessionFacts,
    pulls: &HashMap<&str, Vec<String>>,
) -> Option<&'static str> {
    if later.id == stop.id
        || later.parent.as_deref() == Some(stop.id.as_str())
        || later.automated
        || later.last_turn.is_none_or(|t| t <= at)
        || later.first_turn < stop.first_turn
    {
        return None;
    }
    let (stop_title, later_title) = (title(stop), title(later));
    if stop_title.eq_ignore_ascii_case(&later_title) {
        return Some("the same title");
    }
    // The same request: a fork of the thread, or the same ask again.
    let words_of = |t: &str| {
        t.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    };
    if let (Some(a), Some(b)) = (stop.asked.as_deref(), later.asked.as_deref()) {
        let a = words_of(a);
        if a.chars().count() >= 12 && a == words_of(b) {
            return Some("the same request");
        }
    }
    if let Some(asked) = later.asked.as_deref() {
        let lower = asked.to_lowercase();
        let short: String = stop.id.chars().take(8).collect();
        if lower.contains(&short.to_lowercase())
            || (stop_title.chars().count() >= 12 && lower.contains(&stop_title.to_lowercase()))
        {
            return Some("its request names the stopped thread");
        }
    }
    let project = match (&stop.project, &later.project) {
        (Some(a), Some(b)) if a == b => Some(a.as_str()),
        _ => return None,
    };
    if let (Some(a), Some(b)) = (pulls.get(stop.id.as_str()), pulls.get(later.id.as_str())) {
        if a.iter().any(|u| b.contains(u)) {
            return Some("the same pull request");
        }
    }
    if same_work(&stop_title, &later_title, project) {
        return Some("a continuing title");
    }
    // A request that says it resumes stopped work, with a title near this
    // one's: one telling word in common in place of two.
    if later.asked.as_deref().is_some_and(says_it_resumes) {
        let (a, b) = (
            title_words(&stop_title, project),
            title_words(&later_title, project),
        );
        if a.intersection(&b).count() * 3 >= a.len().min(b.len()).max(1) {
            return Some("a request that resumes it");
        }
    }
    None
}

/// Each session's pull requests, by id.
pub(crate) fn pulls_by_session<'a>(
    rollup: &Rollup,
    sessions: &'a [rollup::InRange],
) -> HashMap<&'a str, Vec<String>> {
    sessions
        .iter()
        .map(|s| {
            let urls = rollup
                .pulls_of(&s.facts.id)
                .map(|v| v.into_iter().map(|p| p.url).collect())
                .unwrap_or_default();
            (s.facts.id.as_str(), urls)
        })
        .collect()
}

/// Whether the work of `stop`, stopped at `at` by a limit, carried on.
///
/// A stop is carried on by a later session, a sub-agent's included, that
/// has its title or a continuing one in the same project, names it in its
/// request, or shares a pull request with it ([`continues`]). A stop more
/// than [`STALE_HOURS`] old also counts once any later session worked in
/// its project. `sessions` must hold every session active since the stop's
/// first turn; Home and the editions use this one rule, so they agree.
pub(crate) fn carried_on(
    stop: &SessionFacts,
    at: OffsetDateTime,
    sessions: &[rollup::InRange],
    pulls: &HashMap<&str, Vec<String>>,
    now: OffsetDateTime,
) -> bool {
    if sessions
        .iter()
        .any(|l| continues(stop, at, &l.facts, pulls).is_some())
    {
        return true;
    }
    now - at > Duration::hours(STALE_HOURS)
        && sessions.iter().any(|l| {
            l.facts.id != stop.id
                && !l.facts.automated
                && l.facts.project.is_some()
                && l.facts.project == stop.project
                && l.facts.first_turn > Some(at)
        })
}

/// The Resume rows: top-level threads of the last week stopped at the usage
/// limit (or rate-limited) whose work nothing later [`carried_on`], newest
/// first.
fn resumes(inp: &Inputs<'_>) -> Vec<Need> {
    let Some(rollup) = inp.rollup else {
        return Vec::new();
    };
    let Ok(recent) = rollup.sessions_in(inp.now - Duration::days(LOOKBACK_DAYS), inp.now) else {
        return Vec::new();
    };
    let stops: Vec<(&SessionFacts, OffsetDateTime)> = recent
        .iter()
        .map(|s| &s.facts)
        .filter(|f| f.parent.is_none() && !f.automated)
        .filter(|f| matches!(f.ending(inp.now), Ending::UsageLimit | Ending::RateLimited))
        .filter_map(|f| Some((f, f.ending_at.or(f.last_turn)?)))
        .collect();
    if stops.is_empty() {
        return Vec::new();
    }
    let pulls = pulls_by_session(rollup, &recent);
    let mut open: Vec<(&SessionFacts, OffsetDateTime)> = stops
        .into_iter()
        .filter(|(stop, at)| !carried_on(stop, *at, &recent, &pulls, inp.now))
        .collect();
    open.sort_by_key(|(_, at)| std::cmp::Reverse(*at));
    let rows = open
        .iter()
        .take(RESUMES)
        .map(|(f, at)| {
            let how = if f.ending(inp.now) == Ending::RateLimited {
                " was rate-limited"
            } else {
                " stopped at the usage limit"
            };
            let mut detail = vec![when(*at, inp.now, inp.offset)];
            if let Some(d) = f
                .ending_detail
                .as_deref()
                .filter(|d| d.starts_with("resets"))
            {
                // A reset already past is not news.
                if inp.now - *at < Duration::hours(5) {
                    detail.push(crate::text::twenty_four(d));
                }
            }
            if let Some(p) = &f.project {
                detail.push(p.clone());
            }
            detail.push("nothing has carried it on".to_owned());
            Need::Resume {
                text: vec![
                    strong(title(f)),
                    plain(format!("{how} · {}", detail.join(" · "))),
                ],
                session: Some(f.id.clone()),
            }
        })
        .collect::<Vec<_>>();
    let mut rows = rows;
    if open.len() > RESUMES {
        rows.push(Need::Resume {
            text: vec![plain(format!(
                "{} more stopped and not carried on; the editions list them",
                words(open.len() - RESUMES)
            ))],
            session: None,
        });
    }
    rows
}

/// `14:02` today, `yesterday 13:14`, `Fri 13:14` within the week.
fn when(t: OffsetDateTime, now: OffsetDateTime, offset: UtcOffset) -> String {
    let (local, today) = (t.to_offset(offset), now.to_offset(offset).date());
    let clock = format!("{:02}:{:02}", local.hour(), local.minute());
    if local.date() == today {
        clock
    } else if today.previous_day() == Some(local.date()) {
        format!("yesterday {clock}")
    } else {
        let day = local.weekday().to_string();
        format!("{} {clock}", &day[..3])
    }
}

/// A failure reason that passes on its own: a rate or usage limit, a lost
/// connection, an overloaded API.
fn transient(reason: &str) -> bool {
    let lower = reason.to_lowercase();
    [
        "rate limit",
        "usage limit",
        "try again",
        "overloaded",
        "connection",
        "reach the api",
        "timed out",
        "timeout",
    ]
    .iter()
    .any(|w| lower.contains(w))
}

/// The Failure rows, still unresolved: fixable ones first (a dreaming run
/// that failed, an ingest that stopped, a profile whose sign-in failed),
/// then delegated tasks and API errors of the last [`FAILURE_HOURS`] that
/// nothing later redid.
///
/// A sign-in failure is resolved when a later session on the same profile
/// ran; a delegated task's, when a later session has its title or a
/// continuing one; a transient one (a limit, a lost connection), when any
/// later session ran a turn.
fn failures(inp: &Inputs<'_>) -> Vec<Need> {
    let mut out = Vec::new();
    if let Some(r) = inp
        .status
        .and_then(|s| s.report.as_ref())
        .filter(|r| r.failed)
    {
        out.push(Need::Failure {
            text: vec![
                strong("The last dreaming run failed"),
                plain(match &r.error {
                    Some(e) => format!(" · {e}"),
                    None => " · the notes were not checked".to_owned(),
                }),
            ],
            session: None,
            note: None,
            fix: true,
        });
    }
    if let Some(t) = inp
        .status
        .filter(|s| s.ingest_stale)
        .and_then(|s| s.last_ingest)
    {
        out.push(Need::Failure {
            text: vec![
                strong(format!(
                    "hippo has not read new sessions since {}",
                    when(t, inp.now, inp.offset)
                )),
                plain(" · run hippo doctor"),
            ],
            session: None,
            note: None,
            fix: true,
        });
    }
    let Some(rollup) = inp.rollup else {
        return out;
    };
    let since = inp.now - Duration::hours(FAILURE_HOURS);
    let Ok(recent) = rollup.sessions_in(since, inp.now) else {
        return out;
    };
    let ran_after = |at: OffsetDateTime, keep: &dyn Fn(&SessionFacts) -> bool| {
        recent
            .iter()
            .any(|s| s.facts.last_turn.is_some_and(|t| t > at) && keep(&s.facts))
    };
    let mut signed_out: Vec<&SessionFacts> = recent
        .iter()
        .map(|s| &s.facts)
        .filter(|f| f.parent.is_none())
        .filter(|f| f.ending_detail.as_deref() == Some(rollup::SIGNED_OUT))
        .filter(|f| f.ending(inp.now) == Ending::Interrupted)
        .collect();
    signed_out.sort_by_key(|f| std::cmp::Reverse(f.ending_at));
    for f in signed_out {
        let Some(at) = f.ending_at.or(f.last_turn) else {
            continue;
        };
        let profile = f.profile();
        let signed_in_again = ran_after(at, &|s| {
            s.id != f.id
                && s.profile() == profile
                && s.ending_detail.as_deref() != Some(rollup::SIGNED_OUT)
        });
        if signed_in_again {
            continue;
        }
        let which = match profile.as_deref() {
            None | Some("default") => String::new(),
            Some(p) => format!(" (profile {p})"),
        };
        out.push(Need::Failure {
            text: vec![
                strong(title(f)),
                plain(format!(
                    " stopped: its sign-in failed{which} · {} · run claude auth login, then open it to resume",
                    when(at, inp.now, inp.offset)
                )),
            ],
            session: Some(f.id.clone()),
            note: None,
            fix: true,
        });
    }
    let Ok(events) = rollup.events_in(since, inp.now) else {
        return out;
    };
    // By what they say: (times, the newest's session, its task's name).
    let mut grouped: BTreeMap<String, (Vec<OffsetDateTime>, String, Option<String>)> =
        BTreeMap::new();
    for e in &events {
        let Ok(at) = OffsetDateTime::parse(&e.ts, &time::format_description::well_known::Rfc3339)
        else {
            continue;
        };
        let (text, task) = match e.kind.as_str() {
            "delegate_failed" => {
                // `<task name>: <reason>`; a name can hold `: ` too.
                let (task, reason) = e
                    .detail
                    .rsplit_once(": ")
                    .map_or((None, e.detail.as_str()), |(t, r)| (Some(t.to_owned()), r));
                (
                    format!("Delegated task failed: {}", reason.trim_end_matches('.')),
                    task,
                )
            }
            _ => (e.detail.clone(), None),
        };
        let slot = grouped
            .entry(text)
            .or_insert_with(|| (Vec::new(), e.session_id.clone(), None));
        slot.0.push(at);
        slot.1 = e.session_id.clone();
        if task.is_some() {
            slot.2 = task;
        }
    }
    let mut rows: Vec<(OffsetDateTime, Need)> = Vec::new();
    for (text, (mut at, session, task)) in grouped {
        at.sort();
        let (Some(first), Some(last)) = (at.first().copied(), at.last().copied()) else {
            continue;
        };
        let redone = task.as_deref().is_some_and(|task| {
            ran_after(last, &|s| {
                let t = title(s);
                t.eq_ignore_ascii_case(task) || same_work(&t, task, s.project.as_deref())
            })
        });
        let passed = transient(&text) && ran_after(last, &|s| s.id != session);
        if redone || passed {
            continue;
        }
        let times = if at.len() > 1 {
            format!(
                "{} times from {}",
                words(at.len()),
                when(first, inp.now, inp.offset)
            )
        } else {
            when(first, inp.now, inp.offset)
        };
        rows.push((
            last,
            Need::Failure {
                text: vec![plain(text), plain(format!(" · {times}"))],
                session: Some(session),
                note: None,
                fix: false,
            },
        ));
    }
    rows.sort_by_key(|(t, _)| std::cmp::Reverse(*t));
    out.extend(rows.into_iter().map(|(_, n)| n));
    out
}

/// Builds the Home.
pub fn build(inp: &Inputs<'_>) -> Home {
    let local = inp.now.to_offset(inp.offset);
    let kicker = format!(
        "Home · {} {} {} · {:02}:{:02}",
        local.weekday(),
        local.day(),
        local.month(),
        local.hour(),
        local.minute()
    );

    // Needs you, in the order of what matters: failures the person can fix,
    // fixes to review, real resumes, questions, then other failures.
    let failures = failures(inp);
    let (fixable, other): (Vec<Need>, Vec<Need>) = failures
        .into_iter()
        .partition(|n| matches!(n, Need::Failure { fix: true, .. }));
    let review = review(inp);
    let resume = resumes(inp);
    let answers = answers(inp);
    let headline = headline(inp, &fixable, review.as_ref(), &resume, &answers, &other);
    let mut needs = fixable;
    needs.extend(review);
    needs.extend(resume);
    needs.extend(answers);
    needs.extend(other);

    // Status line and health.
    let mut status = Vec::new();
    let claude = inp
        .profiles
        .iter()
        .filter(|p| !matches!(p.as_str(), "codex" | "dsh" | "opencode"))
        .count();
    let others: Vec<&str> = [("codex", "Codex"), ("dsh", "dsh"), ("opencode", "OpenCode")]
        .into_iter()
        .filter(|(p, _)| inp.profiles.iter().any(|q| q == p))
        .map(|(_, name)| name)
        .collect();
    let tools = match (claude, others.as_slice()) {
        (0, []) => "your agents".to_owned(),
        (0, others) => crate::text::and_list(others),
        (1, others) => crate::text::and_list(&[&["Claude Code"], others].concat()),
        (n, []) => format!("{n} Claude Code profiles"),
        (n, others) => {
            let profiles = format!("{n} Claude Code profiles");
            crate::text::and_list(&[&[profiles.as_str()], others].concat())
        }
    };
    status.push(plain("Reading "));
    status.push(strong(tools));
    let mut tail = format!(" every {} minutes", inp.ingest_interval_minutes);
    if let Some(t) = inp.status.and_then(|s| s.last_ingest) {
        tail.push_str(&format!(", last at {}", hm(t, inp.offset)));
    }
    status.push(plain(tail));
    match inp.ws {
        None => status.push(plain(
            " · no notes folder yet, so no dreaming and no briefing",
        )),
        Some(ws) => {
            status.push(plain(" · "));
            status.push(strong(format!(
                "{} notes",
                thousands(inp.notes.len() as u64)
            )));
            status.push(plain(format!(
                " in {}",
                ws.notes_dir.file_name().map_or_else(
                    || ws.notes_dir.display().to_string(),
                    |n| n.to_string_lossy().into_owned()
                )
            )));
            let last = inp
                .runs
                .first()
                .and_then(|r| r.written)
                .map(|t| hm(OffsetDateTime::from(t), inp.offset));
            status.push(plain(match last {
                Some(t) => format!(" · dreaming nightly at {}, last run {t}", inp.dream_at),
                None => format!(" · dreaming nightly at {}, no run yet", inp.dream_at),
            }));
        }
    }
    let health_warnings: Vec<String> = inp.status.map_or_else(Vec::new, |s| s.warnings.clone());
    let mut health_notes = Vec::new();
    if let Some(t) = inp
        .status
        .filter(|s| s.ingest_stale)
        .and_then(|s| s.last_ingest)
    {
        health_notes.push(format!(
            "hippo has not read new sessions since {}: the ingest job may not be running.",
            when(t, inp.now, inp.offset)
        ));
    }
    if let Some(s) = inp.status {
        health_notes.extend(s.errors.iter().map(|e| format!("Could not read {e}.")));
    }
    if let Some(e) = inp.today {
        health_notes.extend(e.health.iter().map(|h| format!("Today: {h}.")));
    }
    let warnings = health_warnings.len() + health_notes.len();
    let (health, health_ok) = if warnings == 0 {
        ("health ok".to_owned(), true)
    } else {
        (
            format!(
                "health: {} {}",
                words(warnings),
                if warnings == 1 { "warning" } else { "warnings" }
            ),
            false,
        )
    };

    // Today, so far.
    let today = match inp.today {
        Some(e) if e.sessions > 0 => {
            let mut spans = e.deck.clone();
            if e.in_flight > 0 {
                spans.push(plain(format!(
                    " {} still open.",
                    capitalise(&plural(e.in_flight, "thread is", "threads are"))
                )));
            }
            spans
        }
        _ => vec![plain("Nothing started yet today.")],
    };

    let memory = inp.ws.map(|ws| memory(inp, ws));
    let open = needs.len();
    let mut dock_line = if open == 0 {
        "nothing needs you".to_owned()
    } else {
        format!(
            "{} need{} you",
            plural(open, "thing", "things"),
            if open == 1 { "s" } else { "" }
        )
    };
    if let Some(t) = inp.status.and_then(|s| s.last_ingest) {
        dock_line.push_str(&format!(" · read at {}", hm(t, inp.offset)));
    }
    Home {
        kicker,
        headline,
        status,
        health,
        health_ok,
        health_warnings,
        health_notes,
        needs,
        today,
        today_date: inp.today.map(|e| e.date),
        yesterday: inp.yesterday.cloned(),
        shipped: inp.yesterday.map(|e| e.shipped.clone()).unwrap_or_default(),
        memory,
        dock_line,
    }
}

/// The text of a row's spans.
fn said(text: &[Span]) -> String {
    text.iter().map(|s| s.text.as_str()).collect()
}

/// The headline: the single most important true item, in the order of
/// "Needs you", else a calm line about yesterday.
fn headline(
    inp: &Inputs<'_>,
    fixable: &[Need],
    review: Option<&Need>,
    resume: &[Need],
    answers: &[Need],
    other: &[Need],
) -> String {
    if let Some(Need::Failure { text, session, .. }) = fixable.first() {
        let first = text.first().map(|s| s.text.as_str()).unwrap_or_default();
        return if first == "The last dreaming run failed" {
            "Last night's dreaming run failed; the notes were not checked.".to_owned()
        } else if first.starts_with("hippo has not read") {
            format!("{first}; new sessions are missing.")
        } else if session.is_some() {
            format!("{first} could not sign in; run claude auth login to resume it.")
        } else {
            format!("{}.", said(text))
        };
    }
    if let Some(Need::Review { items, .. }) = review {
        let notes: HashSet<&Path> = items.iter().map(|i| i.note.as_path()).collect();
        return format!(
            "Dreaming fixed {}; {} for your look.",
            plural(notes.len(), "note", "notes"),
            if notes.len() == 1 {
                "it waits"
            } else {
                "they wait"
            }
        );
    }
    let stopped: Vec<&Need> = resume
        .iter()
        .filter(|n| {
            matches!(
                n,
                Need::Resume {
                    session: Some(_),
                    ..
                }
            )
        })
        .collect();
    match stopped.as_slice() {
        [] => {}
        [Need::Resume { text, .. }] => {
            let what = text.first().map(|s| s.text.as_str()).unwrap_or_default();
            return format!("{what} stopped and nothing has carried it on.");
        }
        _ => {
            return format!(
                "{} stopped and nothing has carried them on.",
                capitalise(&plural(stopped.len(), "thread", "threads"))
            );
        }
    }
    match answers {
        [] => {}
        [Need::Answer { text, .. }] => {
            let what = text.first().map(|s| s.text.as_str()).unwrap_or_default();
            return format!("{what} asked you something.");
        }
        _ => {
            return format!(
                "{} asked you something.",
                capitalise(&plural(answers.len(), "thread", "threads"))
            );
        }
    }
    if let Some(Need::Failure { text, .. }) = other.first() {
        let what = text.first().map(|s| s.text.as_str()).unwrap_or_default();
        return format!("{what}.");
    }
    let y_sessions = inp.yesterday.map_or(0, |e| e.sessions);
    if y_sessions > 0 {
        format!(
            "Nothing needs you. Your agents ran {} yesterday.",
            plural(y_sessions, "session", "sessions")
        )
    } else {
        "Nothing needs you. Quiet night.".to_owned()
    }
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// The memory's week: notes in all, changed and new; what agents read and
/// edited (from the rollup); what dreaming did.
fn memory(inp: &Inputs<'_>, ws: &Workspace) -> Memory {
    let week = inp.now - Duration::days(7);
    let mut changed = 0;
    let mut new_notes: Vec<(PathBuf, String, OffsetDateTime)> = Vec::new();
    for n in inp.notes {
        let Ok(meta) = std::fs::metadata(ws.root.join(&n.path)) else {
            continue;
        };
        if meta
            .modified()
            .ok()
            .is_some_and(|t| OffsetDateTime::from(t) >= week)
        {
            changed += 1;
        }
        if let Some(born) = meta.created().ok().map(OffsetDateTime::from) {
            if born >= week {
                new_notes.push((n.path.clone(), n.title.clone(), born));
            }
        }
    }
    new_notes.sort_by_key(|n| std::cmp::Reverse(n.2));
    let mut lines = vec![vec![
        strong(format!("{} notes", thousands(inp.notes.len() as u64))),
        plain(format!(
            " · {} changed this week, {} new.",
            thousands(changed as u64),
            thousands(new_notes.len() as u64)
        )),
    ]];
    if let Some(rollup) = inp.rollup {
        if let Ok(seen) = rollup.notes_in(week, inp.now) {
            let mut reads: BTreeMap<PathBuf, usize> = BTreeMap::new();
            let mut edits: BTreeMap<PathBuf, usize> = BTreeMap::new();
            for s in &seen {
                let bucket = if s.kind == "edit" {
                    &mut edits
                } else {
                    &mut reads
                };
                *bucket.entry(s.note.clone()).or_default() += 1;
            }
            let top = |m: &BTreeMap<PathBuf, usize>, k: usize| {
                let mut v: Vec<(&PathBuf, &usize)> = m.iter().collect();
                v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
                v.into_iter()
                    .take(k)
                    .map(|(p, n)| format!("{} {n}", note_label(p, inp.notes)))
                    .collect::<Vec<_>>()
                    .join(" · ")
            };
            if !reads.is_empty() {
                let times: usize = reads.values().sum();
                lines.push(vec![
                    plain("Agents read "),
                    strong(format!(
                        "{} notes, {} times",
                        thousands(reads.len() as u64),
                        thousands(times as u64)
                    )),
                    plain(format!(". Most: {}.", top(&reads, 3))),
                ]);
            }
            if !edits.is_empty() {
                lines.push(vec![
                    plain("Agents edited "),
                    strong(format!("{} notes", thousands(edits.len() as u64))),
                    plain(format!(". Most: {}.", top(&edits, 2))),
                ]);
            }
        }
    }
    let runs: Vec<&Run> = inp
        .runs
        .iter()
        .filter(|r| r.written.is_some_and(|t| OffsetDateTime::from(t) >= week))
        .collect();
    if !runs.is_empty() {
        let applied: usize = inp.history.map_or(0, |h| {
            h.changes
                .iter()
                .filter(|c| {
                    c.receipts.iter().any(|r| {
                        r.verdict == Verdict::Confirm && r.applied.is_some_and(|t| t >= week)
                    })
                })
                .count()
        });
        let undone: usize = inp.history.map_or(0, |h| {
            h.changes
                .iter()
                .filter(|c| {
                    c.receipts
                        .iter()
                        .any(|r| r.verdict == Verdict::Revert && r.at >= week)
                })
                .count()
        });
        lines.push(vec![
            plain("Dreaming ran "),
            strong(plural(runs.len(), "night", "nights")),
            plain(", applied "),
            strong(plural(applied, "fix", "fixes")),
            plain(if undone == 0 {
                ", none undone.".to_owned()
            } else {
                format!(", {} undone.", words(undone))
            }),
        ]);
    }
    Memory {
        lines,
        new_notes: new_notes
            .into_iter()
            .take(5)
            .map(|(p, t, _)| (p, t))
            .collect(),
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)] // the tests build a synthetic home
mod tests;
