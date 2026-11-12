//! Incidents' pure parts: what a reload reads,
//! the list's grouping and order, the counts, the scoreboard, the lesson a
//! correction already has, and the words each row and page use. No window
//! code; `ui/incidents_ui.rs` draws it.

use std::collections::BTreeMap;
use std::path::Path;

pub use hippocamp_core::incidents::subject::{Candidate, Subject, SubjectKind};
use hippocamp_core::incidents::{Cause, Hook, Incident, Kind, Step, hook_for, human_span};
use hippocamp_core::lessons::{Lesson, Status};
use time::{Date, Duration, OffsetDateTime, UtcOffset};

/// How far back the list reaches, in days: this week.
pub const WEEK_DAYS: i64 = 7;
/// A group with fewer top-level sessions than this shows its count only.
pub const MIN_SESSIONS: usize = 20;
/// How long a "Not an incident" mark is kept, in seconds.
pub const DISMISSED_FOR: i64 = 30 * 24 * 3600;

/// What a reload reads: this week's incidents, quiet ones too, and each
/// account's top-level person-started sessions of the week.
#[derive(Debug, Clone, Default)]
pub struct Data {
    pub list: Vec<Incident>,
    /// Sessions per account (profile) in the last [`WEEK_DAYS`] days.
    pub sessions: BTreeMap<String, usize>,
}

/// Reads the week's incidents and session counts from the rollup. Writes
/// nothing; an unreadable rollup is no incidents.
pub fn load(home: &Path, now: OffsetDateTime) -> Option<Data> {
    let rollup = hippocamp_core::rollup::Rollup::open(home).ok().flatten()?;
    let since = now - Duration::days(WEEK_DAYS);
    let list = rollup.incidents(since).ok()?;
    let mut sessions: BTreeMap<String, usize> = BTreeMap::new();
    if let Ok(in_range) = rollup.sessions_in(since, now) {
        for s in in_range {
            if s.facts.parent.is_some() || s.facts.automated {
                continue;
            }
            if let Some(p) = s.facts.profile() {
                *sessions.entry(p).or_default() += 1;
            }
        }
    }
    Some(Data { list, sessions })
}

/// The incidents not marked "Not an incident".
pub fn shown<'a>(list: &'a [Incident], dismissed: &BTreeMap<String, i64>) -> Vec<&'a Incident> {
    list.iter()
        .filter(|i| !dismissed.contains_key(&i.id))
        .collect()
}

/// `dismissed` with `id` marked at `now`, marks older than 30 days
/// dropped.
pub fn dismiss(dismissed: &BTreeMap<String, i64>, id: &str, now: i64) -> BTreeMap<String, i64> {
    let mut out: BTreeMap<String, i64> = dismissed
        .iter()
        .filter(|(_, t)| now - **t <= DISMISSED_FOR)
        .map(|(k, v)| (k.clone(), *v))
        .collect();
    out.insert(id.to_owned(), now);
    out
}

/// `dismissed` without `id` (Undo), old marks dropped.
pub fn restore(dismissed: &BTreeMap<String, i64>, id: &str, now: i64) -> BTreeMap<String, i64> {
    let mut out = dismiss(dismissed, id, now);
    out.remove(id);
    out
}

/// The sidebar's count: incidents (not quiet, not dismissed) newer than
/// `seen_at`; all of them when the page was never opened.
pub fn count_new(
    list: &[Incident],
    dismissed: &BTreeMap<String, i64>,
    seen_at: Option<i64>,
) -> usize {
    shown(list, dismissed)
        .into_iter()
        .filter(|i| !i.quiet && !i.kind.is_hint())
        .filter(|i| seen_at.is_none_or(|s| i.at.unix_timestamp() > s))
        .count()
}

/// Home's `n incidents today` (local day, not quiet, not dismissed).
pub fn count_today(
    list: &[Incident],
    dismissed: &BTreeMap<String, i64>,
    now: OffsetDateTime,
    offset: UtcOffset,
) -> usize {
    let today = now.to_offset(offset).date();
    shown(list, dismissed)
        .into_iter()
        .filter(|i| !i.quiet && !i.kind.is_hint() && i.at.to_offset(offset).date() == today)
        .count()
}

/// Manage's row: `(marked, of)`: incidents this week marked "Not an
/// incident", and all of the week's (quiet ones left out).
pub fn false_alarms(list: &[Incident], dismissed: &BTreeMap<String, i64>) -> (usize, usize) {
    let all = list.iter().filter(|i| !i.quiet);
    let m = all.clone().count();
    let n = all.filter(|i| dismissed.contains_key(&i.id)).count();
    (n, m)
}

/// The list's filters (title bar menus); `None` is `All`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    pub cause: Option<Cause>,
    pub account: Option<String>,
    pub project: Option<String>,
}

impl Filter {
    pub fn is_none(&self) -> bool {
        *self == Filter::default()
    }

    pub fn keeps(&self, i: &Incident) -> bool {
        self.cause.is_none_or(|c| i.cause == c)
            && self
                .account
                .as_ref()
                .is_none_or(|a| i.body.chain.profile.as_ref() == Some(a))
            && self
                .project
                .as_ref()
                .is_none_or(|p| i.body.chain.project.as_ref() == Some(p))
    }
}

/// One local day's rows.
#[derive(Debug, Clone)]
pub struct Day<'a> {
    pub date: Date,
    /// Costliest first.
    pub rows: Vec<&'a Incident>,
    /// The folded `ran a destructive command · n` row's incidents.
    pub quiet: Vec<&'a Incident>,
}

/// The rows grouped by local day, newest day first; within a day costliest
/// first (minutes, lines, tokens), the newer first on a tie. Quiet ones go
/// to their day's folded row. Hints to check are not among them
/// ([`hints`]).
pub fn days<'a>(rows: &[&'a Incident], offset: UtcOffset) -> Vec<Day<'a>> {
    let mut out: Vec<Day<'a>> = Vec::new();
    let mut sorted: Vec<&Incident> = rows.iter().copied().filter(|i| !i.kind.is_hint()).collect();
    sorted.sort_by(|a, b| {
        let day = |i: &Incident| i.at.to_offset(offset).date();
        day(b)
            .cmp(&day(a))
            .then(b.cost_key().cmp(&a.cost_key()))
            .then(b.at.cmp(&a.at))
    });
    for i in sorted {
        let date = i.at.to_offset(offset).date();
        if out.last().is_none_or(|d| d.date != date) {
            out.push(Day {
                date,
                rows: Vec::new(),
                quiet: Vec::new(),
            });
        }
        let day = out.last_mut().expect("pushed");
        if i.quiet {
            day.quiet.push(i);
        } else {
            day.rows.push(i);
        }
    }
    out
}

/// The hints to check among the rows, newest first: listed after the
/// days under [`hippocamp_core::incidents::HINTS_HEADING`], and counted
/// nowhere.
pub fn hints<'a>(rows: &[&'a Incident]) -> Vec<&'a Incident> {
    let mut out: Vec<&Incident> = rows.iter().copied().filter(|i| i.kind.is_hint()).collect();
    out.sort_by_key(|i| std::cmp::Reverse(i.at));
    out
}

/// The folded row's words (after a ten-second reader could not
/// tell what "quiet" meant): `Not counted: 2 blocked before they ran ·
/// 3 failed and left · 1 destructive command, no harm seen`, only the
/// non-zero parts.
pub fn quiet_label(quiet: &[&Incident]) -> String {
    let n = |k: Kind| quiet.iter().filter(|i| i.kind == k).count();
    let mut parts = Vec::new();
    if n(Kind::Blocked) > 0 {
        parts.push(format!("{} blocked before they ran", n(Kind::Blocked)));
    }
    if n(Kind::Failed) > 0 {
        parts.push(format!("{} failed and left", n(Kind::Failed)));
    }
    match n(Kind::Destructive) {
        0 => {}
        1 => parts.push("1 destructive command, no harm seen".to_owned()),
        d => parts.push(format!("{d} destructive commands, no harm seen")),
    }
    format!("Not counted: {}", parts.join(" · "))
}

/// A day's heading: `Today`, `Yesterday`, else `Sun 8 Nov`.
pub fn day_label(date: Date, today: Date) -> String {
    if date == today {
        "Today".to_owned()
    } else if today.previous_day() == Some(date) {
        "Yesterday".to_owned()
    } else {
        crate::state::edition_title(date)
    }
}

/// One scoreboard entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Score {
    pub who: Who,
    pub count: usize,
    /// The sessions beside the count: only for an account with at least
    /// [`MIN_SESSIONS`] of them. Never a rate.
    pub sessions: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Who {
    Account(String),
    Model(String),
}

/// This week's incidents per account and per model (where known), most
/// first. `rows` are the incidents on screen (not quiet).
pub fn scoreboard(rows: &[&Incident], sessions: &BTreeMap<String, usize>) -> Vec<Score> {
    let mut accounts: BTreeMap<String, usize> = BTreeMap::new();
    let mut models: BTreeMap<String, usize> = BTreeMap::new();
    for i in rows.iter().filter(|i| !i.quiet) {
        if let Some(p) = &i.body.chain.profile {
            *accounts.entry(p.clone()).or_default() += 1;
        }
        if let Some(m) = &i.body.chain.model {
            *models.entry(m.clone()).or_default() += 1;
        }
    }
    let mut out: Vec<Score> = accounts
        .into_iter()
        .map(|(p, count)| Score {
            sessions: sessions.get(&p).copied().filter(|n| *n >= MIN_SESSIONS),
            who: Who::Account(p),
            count,
        })
        .collect();
    out.sort_by_key(|a| std::cmp::Reverse(a.count));
    let mut m: Vec<Score> = models
        .into_iter()
        .map(|(name, count)| Score {
            who: Who::Model(name),
            count,
            sessions: None,
        })
        .collect();
    m.sort_by_key(|a| std::cmp::Reverse(a.count));
    out.extend(m);
    out
}

/// A model id as a person says it: `claude-sonnet-5-5` is `Sonnet 5.5`.
pub fn model_name(id: &str) -> String {
    let id = id.strip_prefix("claude-").unwrap_or(id);
    let id = id.split('[').next().unwrap_or(id);
    let parts: Vec<&str> = id.split('-').collect();
    let Some(first) = parts.first() else {
        return id.to_owned();
    };
    let mut name = String::new();
    let mut chars = first.chars();
    if let Some(c) = chars.next() {
        name.extend(c.to_uppercase());
        name.push_str(chars.as_str());
    }
    let nums: Vec<&&str> = parts[1..]
        .iter()
        .take_while(|p| p.chars().all(|c| c.is_ascii_digit()) && p.len() <= 2)
        .collect();
    if !nums.is_empty() {
        name.push(' ');
        name.push_str(&nums.iter().map(|p| **p).collect::<Vec<_>>().join("."));
    }
    name
}

/// The account filter's choices, the project filter's and the cause
/// filter's, from the incidents on offer.
pub fn accounts(list: &[Incident]) -> Vec<String> {
    let mut v: Vec<String> = list
        .iter()
        .filter_map(|i| i.body.chain.profile.clone())
        .collect();
    v.sort();
    v.dedup();
    v
}

pub fn projects(list: &[Incident]) -> Vec<String> {
    let mut v: Vec<String> = list
        .iter()
        .filter_map(|i| i.body.chain.project.clone())
        .collect();
    v.sort();
    v.dedup();
    v
}

/// The causes that occur, in the engine's order.
pub fn causes(list: &[Incident]) -> Vec<Cause> {
    Cause::ALL
        .into_iter()
        .filter(|c| list.iter().any(|i| i.cause == *c))
        .collect()
}

/// The faint word for the kind on a row's second line.
pub fn kind_word(k: Kind) -> &'static str {
    match k {
        Kind::Corrected => "corrected",
        Kind::Unverified => "unverified claim",
        Kind::Blocked => "blocked",
        Kind::Failed => "failed",
        Kind::Redone => "redone",
        Kind::Collision => "collision",
        Kind::Destructive => "destructive",
    }
}

// ---------------------------------------------------------------------------
// What a correction is about

/// A correction's subject, as the engine stored it.
pub fn subject(i: &Incident) -> Option<&Subject> {
    if i.kind != Kind::Corrected {
        return None;
    }
    i.body.subject.as_ref()
}

/// Whether the page shows a step (`What it did`): every incident but a
/// correction whose subject is not a step.
pub fn has_step(i: &Incident) -> bool {
    i.kind != Kind::Corrected || subject(i).is_some_and(|s| s.kind == SubjectKind::Step)
}

/// The title-bar button: `Open the step`, or `Open the session` when there
/// is no step (it opens the session at the correction).
pub fn open_label(i: &Incident) -> &'static str {
    if has_step(i) {
        "Open the step"
    } else {
        "Open the session"
    }
}

/// A row's short form of what a correction is about: the step's tool line,
/// or `about <words>` with the first candidate (its account and title);
/// nothing for what it said or not established.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Short {
    Step(String),
    About {
        words: String,
        profile: Option<String>,
        title: String,
    },
}

pub fn short_form(i: &Incident) -> Option<Short> {
    let s = subject(i)?;
    match s.kind {
        SubjectKind::Step => {
            let step = s.step.as_ref()?;
            let line = match parse_tool(&step.text) {
                // A command reads as its first words, plumbing left out.
                Some((verb, target)) if verb == "Bash" => format!(
                    "{verb} {}",
                    hippocamp_core::incidents::subject::command_head(&target)
                ),
                Some((verb, target)) => format!("{verb} {}", target.trim()),
                None => step.text.trim().to_owned(),
            };
            Some(Short::Step(line))
        }
        SubjectKind::Earlier => {
            let c = s.candidates.first()?;
            // The correction's own session is already the row's title.
            let own = c.relation == hippocamp_core::incidents::subject::Relation::Own;
            Some(Short::About {
                words: format!("about {}", c.words.join(", ")),
                profile: (!own).then(|| c.profile.clone()).flatten(),
                title: if own {
                    "this session".to_owned()
                } else {
                    candidate_title(c)
                },
            })
        }
        SubjectKind::Said | SubjectKind::None => None,
    }
}

/// A candidate's title: its own, else its session's short id.
pub fn candidate_title(c: &Candidate) -> String {
    c.title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(|t| t.lines().next().unwrap_or(t).to_owned())
        .unwrap_or_else(|| crate::rows::id_title(&c.session))
}

/// A candidate's second line: what it is to the correction, when, the
/// files under the matched path, the pull request.
pub fn candidate_facts(c: &Candidate, now: OffsetDateTime, offset: UtcOffset) -> Vec<String> {
    let mut v = vec![c.relation.label().to_owned()];
    v.push(when_words(c.at, now, offset));
    if let Some(under) = &c.under {
        v.push(if is_file(under) {
            format!("changed {under}")
        } else {
            format!(
                "{} file{} under {under}",
                c.files,
                if c.files == 1 { "" } else { "s" }
            )
        });
    }
    if let Some(p) = &c.pull {
        let n = p.url.rsplit('/').next().unwrap_or("");
        v.push(match &p.title {
            Some(t) => format!("#{n} {t}"),
            None => format!("pull request #{n}"),
        });
    }
    v
}

/// Whether a matched path is a file (its last part has an extension).
fn is_file(path: &str) -> bool {
    path.rsplit('/')
        .next()
        .is_some_and(|last| last.contains('.') && !last.starts_with('.'))
}

/// `today 14:02`, `yesterday 09:10`, `Mon 9 Nov 09:10`.
pub fn when_words(t: OffsetDateTime, now: OffsetDateTime, offset: UtcOffset) -> String {
    let (d, today) = (t.to_offset(offset).date(), now.to_offset(offset).date());
    let day = if d == today {
        "today".to_owned()
    } else if Some(d) == today.previous_day() {
        "yesterday".to_owned()
    } else {
        let wd = &format!("{:?}", d.weekday())[..3];
        let mon = &format!("{:?}", d.month())[..3];
        format!("{wd} {} {mon}", d.day())
    };
    format!("{day} {}", clock(t, offset))
}

/// What the agent said, as the page shows it: a Markdown image link reads
/// as the image's file name, not as `![…](…)`.
pub fn said_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(k) = rest.find("![") {
        let after = &rest[k + 2..];
        let Some(close) = after.find("](") else { break };
        let target = &after[close + 2..];
        let Some(end) = target.find(')') else { break };
        let path = target[..end].split_whitespace().next().unwrap_or("");
        let name = path
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or(path);
        out.push_str(&rest[..k]);
        out.push_str(if name.is_empty() {
            &after[..close]
        } else {
            name
        });
        rest = &target[end + 1..];
    }
    out.push_str(rest);
    out
}

/// A row's first line: the evidence in its own words; the person's words
/// in curly quotes for a correction.
pub fn evidence(i: &Incident) -> String {
    // A claim reads as the claim in quotes; why it is unverified is on
    // its page (the reason alone, often a command, said nothing).
    if i.kind == Kind::Unverified {
        return i.body.did.text.trim().to_owned();
    }
    let t = i.body.showed.text.trim();
    if i.kind == Kind::Corrected && !t.starts_with('\u{201c}') {
        format!("\u{201c}{t}\u{201d}")
    } else {
        t.to_owned()
    }
}

/// The crumb: the evidence, cut at a word to at most `max` characters and
/// ended with an ellipsis when cut.
pub fn short_evidence(i: &Incident, max: usize) -> String {
    let t = evidence(i);
    if t.chars().count() <= max {
        return t;
    }
    let cut: String = t.chars().take(max).collect();
    let cut = match cut.rfind(' ') {
        Some(k) if k > max / 2 => cut[..k].to_owned(),
        _ => cut,
    };
    format!("{}…", cut.trim_end_matches([',', ';', ':', ' ']))
}

/// The cost on a row: the first known of minutes, lines, tokens.
pub fn cost_text(i: &Incident) -> Option<String> {
    if let Some(m) = i.minutes.filter(|m| *m > 0) {
        return Some(human_span(Duration::minutes(m)));
    }
    if let Some(l) = i.lines.filter(|l| *l > 0) {
        return Some(format!("{l} lines"));
    }
    i.tokens
        .filter(|t| *t > 0)
        .map(|t| format!("{}k tokens", (t + 500) / 1000))
}

/// Every known part of the cost, for the page's block.
pub fn cost_parts(i: &Incident) -> Vec<String> {
    let mut v = Vec::new();
    if let Some(m) = i.minutes.filter(|m| *m > 0) {
        v.push(if i.kind == Kind::Redone {
            format!(
                "{} from the step to the redo",
                human_span(Duration::minutes(m))
            )
        } else {
            // Working time only, idle and waiting left out.
            format!(
                "{} of work from the step to the correction",
                human_span(Duration::minutes(m))
            )
        });
    }
    if let Some(l) = i.lines.filter(|l| *l > 0) {
        v.push(format!("{l} lines redone"));
    }
    if let Some(t) = i.tokens.filter(|t| *t > 0) {
        v.push(format!("{}k tokens", (t + 500) / 1000));
    }
    v
}

/// A short age: `now`, `5m`, `2h`, `3d`.
pub fn age(d: Duration) -> String {
    let s = d.whole_seconds().max(0);
    if s < 60 {
        "now".to_owned()
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else if s < 86_400 {
        format!("{}h", s / 3600)
    } else {
        format!("{}d", s / 86_400)
    }
}

/// A tool line's verb and target from a step's text: `Tool: input`, or
/// `Tool {json}` as the engine writes an edit (`Edit {"file_path": …}`),
/// whose file path or command is the target. `None` for any other text.
/// The verb is a tool's name (one capitalised word).
/// A string field of a call's JSON input, read leniently: the excerpt the
/// engine keeps is cut at 300 characters, so the JSON may not close. A
/// field cut short returns what it has.
pub fn json_field(text: &str, key: &str) -> Option<String> {
    json_field_closed(text, key).map(|(v, _)| v)
}

/// [`json_field`], and whether the field closed (was not cut short).
fn json_field_closed(text: &str, key: &str) -> Option<(String, bool)> {
    let start = text.find(&format!("\"{key}\":\""))? + key.len() + 4;
    let mut out = String::new();
    let mut chars = text[start..].chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some((out, true)),
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(other) => out.push(other),
                None => break,
            },
            c => out.push(c),
        }
    }
    Some((out.trim_end_matches('…').to_owned(), false))
}

/// An edit call's file and the first lines it removed and added, for the
/// page's diff: `None` when `text` is not an edit call.
pub fn edit_preview(text: &str) -> Option<(String, Vec<String>, Vec<String>)> {
    let (verb, _) = text.split_once(' ')?;
    if !matches!(verb, "Edit" | "Write" | "MultiEdit") {
        return None;
    }
    let path = json_field(text, "file_path")?;
    let lines = |key: &str| -> Vec<String> {
        json_field_closed(text, key)
            .map(|(s, closed)| {
                let mut all: Vec<&str> = s.lines().collect();
                // A line the excerpt cut short is not shown as a line.
                if !closed && all.len() > 1 {
                    all.pop();
                }
                all.into_iter()
                    .map(str::trim_end)
                    .filter(|l| !l.trim().is_empty())
                    .take(4)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    };
    let added = if verb == "Write" {
        lines("content")
    } else {
        lines("new_string")
    };
    Some((path, lines("old_string"), added))
}

pub fn parse_tool(text: &str) -> Option<(String, String)> {
    let word = |verb: &str| {
        verb.chars().next().is_some_and(|c| c.is_ascii_uppercase())
            && verb.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    };
    if let Some((verb, rest)) = text.split_once(' ')
        && word(verb)
        && rest.trim_start().starts_with('{')
    {
        // The excerpt may be cut mid-JSON: read the fields leniently then.
        let keys = ["file_path", "command", "path", "pattern", "description"];
        let target = match serde_json::from_str::<serde_json::Value>(rest.trim()) {
            Ok(v) => keys
                .iter()
                .find_map(|k| v.get(*k).and_then(|t| t.as_str()).map(str::to_owned)),
            Err(_) => keys.iter().find_map(|k| json_field(rest, k)),
        }?;
        return Some((verb.to_owned(), target));
    }
    let (verb, rest) = text.split_once(": ")?;
    (word(verb) && !rest.trim().is_empty()).then(|| (verb.to_owned(), rest.trim().to_owned()))
}

/// What a click on Never again does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NeverAgain {
    /// A correction with a lesson already in the Inbox: open it.
    Open(String),
    /// A lesson that was kept, rejected or closed: say so.
    Said(&'static str),
    /// No lesson: offer to draft one.
    Draft,
    /// A destructive command a hook can block.
    Hook(Hook),
}

/// Whether Never again shows for this incident at all.
pub fn offers_never_again(i: &Incident) -> bool {
    match i.kind {
        Kind::Corrected => true,
        Kind::Destructive => i.body.detail.as_deref().and_then(hook_for).is_some(),
        _ => false,
    }
}

/// The lesson whose evidence is the same step as `showed` (the same
/// session and ordinal).
pub fn lesson_for<'a>(lessons: &'a [Lesson], showed: &Step) -> Option<&'a Lesson> {
    lessons.iter().find(|l| {
        l.evidence
            .iter()
            .any(|e| e.session == showed.session && e.ordinal == showed.ordinal)
    })
}

/// What Never again does for `i`, given the lessons on disk.
pub fn never_again(i: &Incident, lessons: &[Lesson]) -> Option<NeverAgain> {
    match i.kind {
        Kind::Corrected => Some(match lesson_for(lessons, &i.body.showed) {
            Some(l) if l.status == Status::Pending => NeverAgain::Open(l.id.to_string()),
            Some(l) => NeverAgain::Said(match l.status {
                Status::Kept => "Hippocamp's lessons pass already kept a rule for this",
                Status::Rejected => {
                    "Hippocamp's lessons pass: the reviewer rejected a rule for this"
                }
                Status::Declined => "Hippocamp's lessons pass found no lasting rule in this",
                _ => "You dismissed a rule for this earlier; it comes back after 3 more sessions",
            }),
            None => NeverAgain::Draft,
        }),
        Kind::Destructive => i
            .body
            .detail
            .as_deref()
            .and_then(hook_for)
            .map(NeverAgain::Hook),
        _ => None,
    }
}

/// The step's time as the page's cards show it: `14:07`.
pub fn clock(t: OffsetDateTime, offset: UtcOffset) -> String {
    let l = t.to_offset(offset);
    format!("{:02}:{:02}", l.hour(), l.minute())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cut_edit_call_still_shows_its_file_and_lines() {
        let text = r#"Edit {"file_path":"/w/src/covers.ts","old_string":"let a = 1;\nlet b = 2;","new_string":"fn cover_1() { render(1) }\nfn cover_2() { rend…"#;
        assert_eq!(
            parse_tool(text),
            Some(("Edit".to_owned(), "/w/src/covers.ts".to_owned()))
        );
        let (path, removed, added) = edit_preview(text).unwrap();
        assert_eq!(path, "/w/src/covers.ts");
        assert_eq!(removed, ["let a = 1;", "let b = 2;"]);
        assert_eq!(added, ["fn cover_1() { render(1) }"]);
        assert!(edit_preview("Bash: ls").is_none());
    }
    use hippocamp_core::incidents::{Body, Chain, Told};
    use time::macros::datetime;

    fn step(session: &str, ordinal: u32, at: OffsetDateTime, text: &str) -> Step {
        Step {
            session: session.into(),
            ordinal,
            at,
            text: text.into(),
        }
    }

    fn inc(id: &str, kind: Kind, at: OffsetDateTime) -> Incident {
        Incident {
            id: id.into(),
            kind,
            quiet: false,
            at,
            cause: Cause::NoRule,
            minutes: None,
            lines: None,
            tokens: None,
            body: Body {
                chain: Chain {
                    root: "s1".into(),
                    session: "s1".into(),
                    profile: Some("work".into()),
                    project: Some("tern".into()),
                    tool: "claude-code".into(),
                    ..Chain::default()
                },
                told: None::<Told>,
                rule: None,
                did: step("s1", 3, at, "Bash: cargo test"),
                showed: step("s1", 5, at, "no, run the tests first"),
                fixed: None,
                other: None,
                read: vec![],
                detail: None,
                subject: None,
            },
        }
    }

    const NOW: OffsetDateTime = datetime!(2026-11-10 12:00 UTC);

    #[test]
    fn days_are_newest_first_costliest_first_with_quiet_folded() {
        let mut a = inc("a", Kind::Corrected, NOW - Duration::hours(1));
        a.minutes = Some(5);
        let mut b = inc("b", Kind::Blocked, NOW - Duration::hours(2));
        b.minutes = Some(22);
        let mut c = inc("c", Kind::Redone, NOW - Duration::hours(3));
        c.lines = Some(120);
        let mut q = inc("q", Kind::Destructive, NOW - Duration::hours(4));
        q.quiet = true;
        let old = inc("old", Kind::Failed, NOW - Duration::days(1));
        let list = [a, b, c, q, old];
        let rows: Vec<&Incident> = list.iter().collect();
        let d = days(&rows, UtcOffset::UTC);
        assert_eq!(d.len(), 2);
        let ids: Vec<&str> = d[0].rows.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["b", "a", "c"], "minutes first, then lines");
        assert_eq!(d[0].quiet.len(), 1);
        assert_eq!(d[1].rows[0].id, "old");
        assert!(d[0].date > d[1].date);
        assert_eq!(day_label(d[0].date, NOW.date()), "Today");
        assert_eq!(day_label(d[1].date, NOW.date()), "Yesterday");
    }

    #[test]
    fn the_folded_row_names_only_its_non_zero_parts() {
        let mk = |id: &str, k: Kind| {
            let mut i = inc(id, k, NOW);
            i.quiet = true;
            i
        };
        let v = [
            mk("a", Kind::Blocked),
            mk("b", Kind::Blocked),
            mk("c", Kind::Destructive),
        ];
        let r: Vec<&Incident> = v.iter().collect();
        assert_eq!(
            quiet_label(&r),
            "Not counted: 2 blocked before they ran · 1 destructive command, no harm seen"
        );
        let f = [mk("d", Kind::Failed)];
        let r: Vec<&Incident> = f.iter().collect();
        assert_eq!(quiet_label(&r), "Not counted: 1 failed and left");
    }

    #[test]
    fn counts_leave_out_quiet_and_dismissed_and_what_was_seen() {
        let a = inc("a", Kind::Corrected, NOW - Duration::hours(1));
        let b = inc("b", Kind::Blocked, NOW - Duration::hours(5));
        let mut q = inc("q", Kind::Destructive, NOW - Duration::minutes(5));
        q.quiet = true;
        let yesterday = inc("y", Kind::Failed, NOW - Duration::days(1));
        let list = [a, b, q, yesterday];
        let none = BTreeMap::new();
        assert_eq!(count_new(&list, &none, None), 3, "quiet ones never count");
        let seen = (NOW - Duration::hours(2)).unix_timestamp();
        assert_eq!(count_new(&list, &none, Some(seen)), 1);
        let mut dismissed = BTreeMap::new();
        dismissed.insert("a".to_owned(), 0);
        assert_eq!(count_new(&list, &dismissed, Some(seen)), 0);
        assert_eq!(count_today(&list, &none, NOW, UtcOffset::UTC), 2);
        assert_eq!(count_today(&list, &dismissed, NOW, UtcOffset::UTC), 1);
        assert_eq!(false_alarms(&list, &dismissed), (1, 3));
        assert_eq!(false_alarms(&[], &none), (0, 0));
    }

    #[test]
    fn a_hint_to_check_is_counted_nowhere_and_listed_apart() {
        let a = inc("a", Kind::Corrected, NOW - Duration::hours(1));
        let h = inc("h", Kind::Unverified, NOW - Duration::hours(2));
        let list = [a, h];
        let none = BTreeMap::new();
        assert_eq!(count_new(&list, &none, None), 1);
        assert_eq!(count_today(&list, &none, NOW, UtcOffset::UTC), 1);
        let rows: Vec<&Incident> = list.iter().collect();
        let d = days(&rows, UtcOffset::UTC);
        assert_eq!(d.len(), 1);
        assert_eq!(
            d[0].rows.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
            ["a"]
        );
        assert_eq!(
            hints(&rows)
                .iter()
                .map(|i| i.id.as_str())
                .collect::<Vec<_>>(),
            ["h"]
        );
    }

    #[test]
    fn dismissals_keep_thirty_days_and_undo_removes_one() {
        let mut m = BTreeMap::new();
        m.insert("old".to_owned(), 0);
        m.insert("new".to_owned(), 2_000_000);
        let now = DISMISSED_FOR + 1_000_000;
        let out = dismiss(&m, "x", now);
        assert_eq!(out.keys().collect::<Vec<_>>(), ["new", "x"]);
        let back = restore(&out, "x", now);
        assert_eq!(back.keys().collect::<Vec<_>>(), ["new"]);
    }

    #[test]
    fn the_scoreboard_shows_sessions_only_from_twenty() {
        let mut list = Vec::new();
        for n in 0..4 {
            let mut i = inc(&format!("w{n}"), Kind::Corrected, NOW);
            i.body.chain.model = Some("claude-sonnet-5-5".into());
            list.push(i);
        }
        let mut p = inc("p", Kind::Blocked, NOW);
        p.body.chain.profile = Some("personal".into());
        list.push(p);
        let mut quiet = inc("q", Kind::Destructive, NOW);
        quiet.quiet = true;
        list.push(quiet);
        let rows: Vec<&Incident> = list.iter().collect();
        let sessions: BTreeMap<String, usize> =
            [("work".to_owned(), 31), ("personal".to_owned(), 12)].into();
        let s = scoreboard(&rows, &sessions);
        assert_eq!(
            s[0],
            Score {
                who: Who::Account("work".into()),
                count: 4,
                sessions: Some(31)
            }
        );
        assert_eq!(
            s[1],
            Score {
                who: Who::Account("personal".into()),
                count: 1,
                sessions: None
            },
            "12 sessions is under 20: the count only"
        );
        assert_eq!(
            s[2],
            Score {
                who: Who::Model("claude-sonnet-5-5".into()),
                count: 4,
                sessions: None
            }
        );
        assert_eq!(s.len(), 3, "the model is counted only where known");
        assert_eq!(model_name("claude-sonnet-5-5"), "Sonnet 5.5");
        assert_eq!(model_name("claude-opus-4-1-20250805"), "Opus 4.1");
        assert_eq!(model_name("claude-fable-5-2[1m]"), "Fable 5.2");
    }

    #[test]
    fn filters_keep_what_every_menu_allows() {
        let i = inc("a", Kind::Corrected, NOW);
        assert!(Filter::default().keeps(&i));
        let f = Filter {
            cause: Some(Cause::NoRule),
            account: Some("work".into()),
            project: Some("tern".into()),
        };
        assert!(f.keeps(&i));
        assert!(
            !Filter {
                cause: Some(Cause::Hook),
                ..f.clone()
            }
            .keeps(&i)
        );
        assert!(
            !Filter {
                account: Some("other".into()),
                ..f
            }
            .keeps(&i)
        );
    }

    #[test]
    fn words_for_rows() {
        let mut i = inc("a", Kind::Corrected, NOW);
        assert_eq!(evidence(&i), "\u{201c}no, run the tests first\u{201d}");
        i.kind = Kind::Blocked;
        assert_eq!(evidence(&i), "no, run the tests first");
        i.body.showed.text =
            "Blocked: a deploy to production needs a release tag on this branch".into();
        let short = short_evidence(&i, 30);
        assert!(short.ends_with('…'), "{short}");
        assert!(short.chars().count() <= 31);
        assert!(!short.contains("effort must"), "cut at a word: {short}");
        i.minutes = Some(22);
        assert_eq!(cost_text(&i).as_deref(), Some("22m"));
        i.minutes = None;
        i.lines = Some(120);
        assert_eq!(cost_text(&i).as_deref(), Some("120 lines"));
        i.lines = None;
        i.tokens = Some(39_200);
        assert_eq!(cost_text(&i).as_deref(), Some("39k tokens"));
        i.tokens = None;
        assert_eq!(cost_text(&i), None, "absent when unknown");
        assert_eq!(age(Duration::minutes(5)), "5m");
        assert_eq!(age(Duration::hours(26)), "1d");
        assert_eq!(kind_word(Kind::Unverified), "unverified claim");
        assert_eq!(
            parse_tool("Bash: git push --force origin main"),
            Some(("Bash".into(), "git push --force origin main".into()))
        );
        assert_eq!(
            parse_tool(r#"Edit {"file_path":"/w/src/covers.ts","new_string":"x"}"#),
            Some(("Edit".into(), "/w/src/covers.ts".into()))
        );
        assert_eq!(parse_tool("Edit {not json"), None);
        assert_eq!(parse_tool("\u{201c}it passes\u{201d}"), None);
        assert_eq!(parse_tool("no, why did you: stop"), None);
    }

    fn lesson(session: &str, ordinal: u32, status: Status) -> Lesson {
        let json = serde_json::json!({
            "id": ulid::Ulid::generate().to_string(),
            "kind": "correction",
            "key": "k",
            "title": "t",
            "rule": "r",
            "target": null,
            "evidence": [{
                "session": session, "ordinal": ordinal,
                "at": "2026-11-10T10:00:00Z", "quote": "q"
            }],
            "seen": {"sessions": 1, "profiles": [], "first": "2026-11-10T10:00:00Z", "last": "2026-11-10T10:00:00Z"},
            "status": match status {
                Status::Pending => "pending",
                Status::Kept => "kept",
                Status::Dismissed => "dismissed",
                Status::Declined => "declined",
                Status::Rejected => "rejected",
            },
            "created": "2026-11-10T10:00:00Z",
            "updated": "2026-11-10T10:00:00Z"
        });
        serde_json::from_value(json).expect("a lesson")
    }

    #[test]
    fn never_again_finds_the_lesson_by_session_and_ordinal() {
        let i = inc("a", Kind::Corrected, NOW);
        let pending = lesson("s1", 5, Status::Pending);
        let id = pending.id.to_string();
        let other_step = lesson("s1", 6, Status::Kept);
        let other_session = lesson("s2", 5, Status::Kept);
        assert_eq!(
            never_again(&i, &[other_step.clone(), other_session.clone(), pending]),
            Some(NeverAgain::Open(id))
        );
        assert_eq!(
            never_again(&i, &[other_step.clone(), other_session.clone()]),
            Some(NeverAgain::Draft),
            "a lesson on another step or session is not this one"
        );
        let kept = lesson("s1", 5, Status::Kept);
        assert_eq!(
            never_again(&i, &[kept]),
            Some(NeverAgain::Said(
                "Hippocamp's lessons pass already kept a rule for this"
            ))
        );
        let rejected = lesson("s1", 5, Status::Rejected);
        assert!(matches!(
            never_again(&i, &[rejected]),
            Some(NeverAgain::Said(s)) if s.contains("rejected")
        ));
        // Other kinds: nothing, except a destructive pattern with a hook.
        assert!(!offers_never_again(&inc("b", Kind::Blocked, NOW)));
        let mut d = inc("d", Kind::Destructive, NOW);
        assert!(!offers_never_again(&d), "no pattern: no hook");
        d.body.detail = Some("force push".into());
        assert!(offers_never_again(&d));
        assert!(matches!(
            never_again(&d, &[]),
            Some(NeverAgain::Hook(h)) if h.file == "block-force-push.sh"
        ));
    }

    #[test]
    fn what_it_said_shows_an_image_link_as_its_file_name() {
        assert_eq!(
            said_text("A new user sees this: ![new user](/tmp/x/first/new-user.png) and more."),
            "A new user sees this: new-user.png and more."
        );
        assert_eq!(said_text("no images here"), "no images here");
        assert_eq!(said_text("broken ![a](b"), "broken ![a](b");
    }

    #[test]
    fn a_row_says_the_step_or_what_it_may_be_about() {
        use hippocamp_core::incidents::subject::{Candidate, Relation};
        let at = OffsetDateTime::UNIX_EPOCH + Duration::days(20_000);
        let mut i = inc("c1", Kind::Corrected, at);
        assert!(!has_step(&i), "a correction with no subject has no step");
        let none = Subject {
            kind: SubjectKind::None,
            step: None,
            sub: None,
            matched: vec![],
            minutes: None,
            candidates: vec![],
            said: None,
        };
        i.body.subject = Some(Subject {
            kind: SubjectKind::Earlier,
            candidates: vec![Candidate {
                session: "s9".into(),
                title: Some("Settings polish".into()),
                relation: Relation::Other,
                profile: Some("work".into()),
                terms: vec!["bill".into()],
                words: vec!["billing".into()],
                names: vec!["src/billing/Invoice.tsx".into()],
                at,
                ordinal: Some(3),
                files: 2,
                under: Some("src/billing".into()),
                pull: None,
            }],
            ..none.clone()
        });
        assert!(!has_step(&i));
        assert_eq!(open_label(&i), "Open the session");
        assert_eq!(
            short_form(&i),
            Some(Short::About {
                words: "about billing".into(),
                profile: Some("work".into()),
                title: "Settings polish".into()
            })
        );
        let facts = candidate_facts(
            &i.body.subject.as_ref().unwrap().candidates[0],
            at,
            UtcOffset::UTC,
        );
        assert_eq!(
            facts,
            [
                "another session",
                "today 00:00",
                "2 files under src/billing"
            ]
        );
        i.body.subject = Some(Subject {
            kind: SubjectKind::Said,
            ..none
        });
        assert_eq!(short_form(&i), None);
        assert_eq!(open_label(&i), "Open the session");
    }
}
