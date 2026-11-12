//! Replay: one local day as lanes
//!
//! One lane per top-level session with a turn that day, its sub-agents and
//! delegated sessions as child tracks under it. A track is the stretches a
//! session worked, waited on the person or stood at a usage limit, read
//! from the index's turns of the day; marks come from the rollup (pull
//! requests, incidents) and from the same turns (questions, limits), and
//! a carry-on from Home's rule ([`crate::home::continues`]). No model call,
//! nothing written.

use std::collections::{HashMap, HashSet};

use hippo_core::index::{Index, TurnMark};
use hippo_core::rollup::{self, parse_ts, InRange, PullDone, Rollup};
use time::{Date, Duration, OffsetDateTime, UtcOffset};

use crate::correction::{command_head, is_request, IDLE_GAP, RUN_CAP};
use crate::fleet::tail::ends_with_question;

/// How far beyond the day the turns are read, so a wait or a working
/// stretch that crosses midnight is whole.
pub const MARGIN: Duration = Duration::hours(12);
/// The width of a strength bucket, the rollup's `activity` bucket.
pub const BUCKET: Duration = Duration::minutes(15);
/// A lone turn is drawn this wide.
const LONE: Duration = Duration::seconds(30);
/// Turns and edits (an edit counts three) in a bucket for full strength.
const FULL: f32 = 40.;

/// What a stretch of a track was.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SpanKind {
    /// The session was working: turns close together, or a call running.
    Working,
    /// It waited on the person: a question or a plan to approve.
    Waiting,
    /// It stood at a usage limit.
    Limit,
}

/// One stretch of a track.
#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    pub start: OffsetDateTime,
    pub end: OffsetDateTime,
    pub kind: SpanKind,
    /// Working: 0 to 1, from the turns and edits of its bucket.
    pub strength: f32,
    /// A wait's question turn, or a limit's message turn.
    pub ordinal: Option<u32>,
    /// A wait's question, as asked.
    pub asked: Option<String>,
    /// Still open now (from Fleet's live state).
    pub open: bool,
}

impl Span {
    pub fn len(&self) -> Duration {
        self.end - self.start
    }

    pub fn holds(&self, t: OffsetDateTime) -> bool {
        self.start <= t && t < self.end
    }
}

/// A tool call, as the scrubber names the step a session was on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepAt {
    pub at: OffsetDateTime,
    pub ordinal: u32,
    /// Its description, else its command's first words, else its file.
    pub words: String,
}

/// One session's drawing.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Track {
    pub id: String,
    pub title: Option<String>,
    /// In time order, clipped to the day.
    pub spans: Vec<Span>,
    /// Its calls of the day, in time order.
    pub steps: Vec<StepAt>,
    /// Its first and last turn inside the day.
    pub first: Option<OffsetDateTime>,
    pub last: Option<OffsetDateTime>,
}

impl Track {
    /// Its span of `kind` holding `t`.
    pub fn at(&self, t: OffsetDateTime, kind: SpanKind) -> Option<&Span> {
        self.spans.iter().find(|s| s.kind == kind && s.holds(t))
    }

    /// Its newest step at or before `t`.
    pub fn step_at(&self, t: OffsetDateTime) -> Option<&StepAt> {
        let k = self.steps.partition_point(|s| s.at <= t);
        k.checked_sub(1).map(|k| &self.steps[k])
    }

    /// The time of its spans of `kind`.
    pub fn total(&self, kind: SpanKind) -> Duration {
        self.spans
            .iter()
            .filter(|s| s.kind == kind)
            .map(Span::len)
            .sum()
    }
}

/// What a mark on a lane is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MarkKind {
    PullOpened,
    PullMerged,
    Incident,
    Question,
    Limit,
    CarryOn,
}

/// What a click on a mark opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Url(String),
    Incident(String),
    Turn { session: String, ordinal: u32 },
    Session(String),
}

/// A mark on a lane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mark {
    pub at: OffsetDateTime,
    pub kind: MarkKind,
    /// Its tooltip.
    pub label: String,
    pub target: Option<Target>,
}

/// A top-level session's lane.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Lane {
    pub track: Track,
    pub project: Option<String>,
    pub profile: Option<String>,
    pub marks: Vec<Mark>,
    /// Its sub-agents, workflow agents and delegated sessions active that
    /// day, by first activity.
    pub children: Vec<Track>,
    /// How many children worked in each minute of the day.
    pub running: Vec<u16>,
}

impl Lane {
    /// Whether it (or a child) worked at `t`.
    pub fn working_at(&self, t: OffsetDateTime) -> bool {
        self.track.at(t, SpanKind::Working).is_some()
            || self
                .children
                .iter()
                .any(|c| c.at(t, SpanKind::Working).is_some())
    }
}

/// The day's line: counts only.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DayLine {
    pub threads: usize,
    pub subs: usize,
    /// Working time of every session, sub-agents included.
    pub work: Duration,
    /// Time lanes waited on the person, and in how many waits.
    pub waited: Duration,
    pub questions: usize,
    pub limits: usize,
    pub merged: usize,
    pub incidents: usize,
    /// The longest wait: its lane's id and the span.
    pub longest: Option<(String, Span)>,
}

/// One day.
#[derive(Debug, Clone, PartialEq)]
pub struct Day {
    pub date: Date,
    pub start: OffsetDateTime,
    pub end: OffsetDateTime,
    /// When it was read.
    pub read_at: OffsetDateTime,
    /// By first activity that day.
    pub lanes: Vec<Lane>,
    pub line: DayLine,
}

/// What Fleet says of a session now.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Live {
    pub live: bool,
    pub waiting: Option<(OffsetDateTime, Option<String>)>,
    pub stopped: Option<OffsetDateTime>,
}

/// A working lane at a moment: the step it was on (its own, or a child's
/// with the child's title), and the session and turn that step is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnStep {
    pub lane: usize,
    pub session: String,
    pub ordinal: Option<u32>,
    pub words: Option<String>,
}

/// What stood at one minute: the scrubber's panel.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Moment {
    /// Working lanes (by index) with the step they were on.
    pub working: Vec<OnStep>,
    /// Lanes waiting on the person, with how long they had waited.
    pub waiting: Vec<(usize, Duration)>,
    /// Lanes at a usage limit.
    pub limited: Vec<usize>,
    pub subs_running: usize,
}

/// What stood at `t`.
pub fn moment(day: &Day, t: OffsetDateTime) -> Moment {
    let mut m = Moment::default();
    for (i, lane) in day.lanes.iter().enumerate() {
        if let Some(w) = lane.track.at(t, SpanKind::Waiting) {
            m.waiting.push((i, t - w.start));
        } else if lane.track.at(t, SpanKind::Limit).is_some() {
            m.limited.push(i);
        } else if lane.track.at(t, SpanKind::Working).is_some() {
            let step = lane.track.step_at(t);
            m.working.push(OnStep {
                lane: i,
                session: lane.track.id.clone(),
                ordinal: step.map(|s| s.ordinal),
                words: step.map(|s| s.words.clone()),
            });
        } else if let Some(c) = lane
            .children
            .iter()
            .find(|c| c.at(t, SpanKind::Working).is_some())
        {
            let step = c.step_at(t);
            let title = c.title.clone().unwrap_or_else(|| "a sub-agent".to_owned());
            m.working.push(OnStep {
                lane: i,
                session: c.id.clone(),
                ordinal: step.map(|s| s.ordinal),
                words: Some(match step {
                    Some(s) => format!("{title}: {}", s.words),
                    None => title,
                }),
            });
        }
        m.subs_running += lane
            .children
            .iter()
            .filter(|c| c.at(t, SpanKind::Working).is_some())
            .count();
    }
    m
}

fn ts(t: &TurnMark) -> Option<OffsetDateTime> {
    parse_ts(&t.ts)
}

/// A call's words for the scrubber.
fn step_words(t: &TurnMark) -> String {
    if let Some(a) = t.about.as_deref().filter(|a| !a.trim().is_empty()) {
        return one_line(a);
    }
    if let Some(c) = t.command.as_deref() {
        let h = command_head(c);
        if !h.is_empty() {
            return h;
        }
    }
    if let Some(f) = t.file.as_deref() {
        let name = f.rsplit('/').next().unwrap_or(f);
        return format!("{} {name}", t.tool_name.as_deref().unwrap_or("Edit"));
    }
    t.tool_name.clone().unwrap_or_default()
}

fn one_line(s: &str) -> String {
    let line = s.lines().next().unwrap_or("").trim();
    if line.chars().count() > 90 {
        let cut: String = line.chars().take(89).collect();
        format!("{cut}…")
    } else {
        line.to_owned()
    }
}

/// An `AskUserQuestion` call's first question, from the head of its input.
fn question_of(t: &TurnMark) -> Option<String> {
    let k = t.head.find("\"question\":\"")? + 12;
    let rest = &t.head[k..];
    let end = rest.find("\",\"").or_else(|| rest.find("\"}"))?;
    Some(one_line(&rest[..end].replace("\\n", " ")))
}

fn is_edit(t: &TurnMark) -> bool {
    t.call
        && matches!(
            t.tool_name.as_deref(),
            Some("Edit" | "MultiEdit" | "Write" | "NotebookEdit" | "apply_patch")
        )
}

/// The day a track is drawn over, and the time now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    pub start: OffsetDateTime,
    pub end: OffsetDateTime,
    pub now: OffsetDateTime,
}

/// A session's track over the frame's day from its turns (read with
/// [`MARGIN`] on each side). `person`: a session a person started, whose
/// questions are waits.
pub fn track_of(
    id: &str,
    title: Option<String>,
    marks: &[TurnMark],
    person: bool,
    frame: Frame,
    live: Option<&Live>,
) -> Track {
    let Frame { start, end, now } = frame;
    let mut turns: Vec<(OffsetDateTime, &TurnMark)> =
        marks.iter().filter_map(|t| Some((ts(t)?, t))).collect();
    turns.sort_by_key(|(at, t)| (*at, t.ordinal));
    let mut held: Vec<Span> = Vec::new();
    let wait = |s: OffsetDateTime, e: OffsetDateTime, t: &TurnMark, asked: Option<String>| Span {
        start: s,
        end: e,
        kind: SpanKind::Waiting,
        strength: 0.,
        ordinal: Some(t.ordinal),
        asked,
        open: false,
    };
    for (i, (at, t)) in turns.iter().enumerate() {
        let rest = &turns[i + 1..];
        if t.call
            && matches!(
                t.tool_name.as_deref(),
                Some("AskUserQuestion" | "ExitPlanMode")
            )
        {
            let answer = rest
                .iter()
                .find(|(_, u)| u.role == "tool" && !u.call && u.tool_name == t.tool_name);
            if let (true, Some((e, _))) = (person, answer) {
                let asked = if t.tool_name.as_deref() == Some("ExitPlanMode") {
                    Some("A plan to approve".to_owned())
                } else {
                    question_of(t)
                };
                held.push(wait(*at, *e, t, asked));
            }
            continue;
        }
        if t.role != "assistant" {
            continue;
        }
        if rollup::is_usage_limit(&t.head) {
            let next = rest.first().map(|(e, _)| *e);
            let e = match (next, live.and_then(|l| l.stopped)) {
                (Some(e), _) => Some(e),
                (None, Some(_)) => Some(now),
                (None, None) => None,
            };
            held.push(Span {
                start: *at,
                end: e.unwrap_or(*at),
                kind: SpanKind::Limit,
                strength: 0.,
                ordinal: Some(t.ordinal),
                asked: None,
                open: next.is_none() && e.is_some(),
            });
            continue;
        }
        let text = t.tail.as_deref().unwrap_or(&t.head);
        if !person || !ends_with_question(text) {
            continue;
        }
        // The next turn that is not an injection must be the person's.
        let next = rest.iter().find(|(_, u)| u.role != "user" || is_request(u));
        if let Some((e, u)) = next {
            if u.role == "user" && is_request(u) {
                let last = text.trim_end();
                let q = last
                    .rsplit(['.', '!', '\n'])
                    .find(|s| s.trim().ends_with('?'))
                    .map(one_line);
                held.push(wait(*at, *e, t, q));
            }
        }
    }
    if let Some((w, asked)) = live.and_then(|l| l.waiting.clone()) {
        if !held
            .iter()
            .any(|s| s.kind == SpanKind::Waiting && s.end >= w)
        {
            held.push(Span {
                start: w,
                end: now,
                kind: SpanKind::Waiting,
                strength: 0.,
                ordinal: None,
                asked,
                open: true,
            });
        }
    }
    held.retain(|s| s.end > s.start || s.kind == SpanKind::Limit);
    let inside_held = |t: OffsetDateTime| held.iter().any(|s| s.start <= t && t < s.end);
    // Working: turns close together, or a call running; never inside a
    // wait or a limit.
    let mut work: Vec<(OffsetDateTime, OffsetDateTime)> = Vec::new();
    let mut push = |s: OffsetDateTime, e: OffsetDateTime| match work.last_mut() {
        Some(last) if s <= last.1 + Duration::seconds(1) => last.1 = last.1.max(e),
        _ => work.push((s, e)),
    };
    for (i, (at, t)) in turns.iter().enumerate() {
        // A limit's own message is not work, even where it has no stretch
        // (a sub-agent stopped by its parent's limit writes one and ends).
        if inside_held(*at) || (t.role == "assistant" && rollup::is_usage_limit(&t.head)) {
            continue;
        }
        let next = turns.get(i + 1).map(|(e, _)| *e);
        match next {
            Some(e) if e - *at <= IDLE_GAP || (t.call && e - *at <= RUN_CAP) => {
                push(*at, e);
            }
            _ => push(*at, *at + LONE),
        }
    }
    if live.is_some_and(|l| l.live) {
        if let Some((at, _)) = turns.last() {
            push(*at, now);
        }
    }
    // Strength per bucket: turns, and edits three times.
    let mut weight: HashMap<i64, f32> = HashMap::new();
    let bucket = |t: OffsetDateTime| t.unix_timestamp().div_euclid(BUCKET.whole_seconds());
    for (at, t) in &turns {
        *weight.entry(bucket(*at)).or_default() += if is_edit(t) { 3. } else { 1. };
    }
    let mut spans: Vec<Span> = Vec::new();
    for (s, e) in work {
        // Split at bucket edges so the strength follows the stretch.
        let mut a = s;
        while a < e {
            let b = bucket(a);
            let edge = OffsetDateTime::from_unix_timestamp((b + 1) * BUCKET.whole_seconds())
                .unwrap_or(e)
                .min(e);
            let w = weight.get(&b).copied().unwrap_or(1.);
            spans.push(Span {
                start: a,
                end: edge,
                kind: SpanKind::Working,
                strength: (w / FULL).clamp(0.15, 1.),
                ordinal: None,
                asked: None,
                open: false,
            });
            a = edge;
        }
    }
    spans.extend(held);
    // Clip to the day.
    let mut spans: Vec<Span> = spans
        .into_iter()
        .filter(|s| (s.end > start || s.start >= start) && s.start < end)
        .map(|mut s| {
            s.start = s.start.max(start);
            s.end = s.end.min(end);
            s
        })
        .collect();
    spans.sort_by_key(|s| (s.start, s.kind as u8));
    let inside = |t: &OffsetDateTime| *t >= start && *t < end;
    let steps = turns
        .iter()
        .filter(|(at, t)| t.call && inside(at))
        .map(|(at, t)| StepAt {
            at: *at,
            ordinal: t.ordinal,
            words: step_words(t),
        })
        .collect();
    Track {
        id: id.to_owned(),
        title,
        spans,
        steps,
        first: turns.iter().map(|(a, _)| *a).find(inside),
        last: turns.iter().rev().map(|(a, _)| *a).find(inside),
    }
}

/// The lane each session of the day draws in: itself when it is a lane,
/// else the top session above it that is active that day.
fn roots(
    days: &HashMap<&str, &InRange>,
    delegations: &HashMap<String, String>,
) -> HashMap<String, String> {
    let parent_of = |id: &str| -> Option<String> {
        let f = &days.get(id)?.facts;
        f.parent
            .clone()
            .or_else(|| id.split_once("-agent-").map(|(p, _)| p.to_owned()))
            .or_else(|| delegations.get(id).cloned())
    };
    let mut out = HashMap::new();
    for id in days.keys() {
        let mut cur = (*id).to_owned();
        let mut seen = HashSet::new();
        while let Some(p) = parent_of(&cur) {
            if !days.contains_key(p.as_str()) || !seen.insert(p.clone()) {
                break;
            }
            cur = p;
        }
        out.insert((*id).to_owned(), cur);
    }
    out
}

/// A pull request's short name: `owner/repo#12`.
pub fn pull_name(url: &str) -> String {
    let parts: Vec<&str> = url.trim_end_matches('/').split('/').collect();
    match parts.as_slice() {
        [.., owner, repo, "pull", n] => format!("{owner}/{repo}#{n}"),
        _ => url.to_owned(),
    }
}

/// Reads one local day. `live` is Fleet's state of today's sessions, by
/// id (empty for another day). Writes nothing.
pub fn load(
    rollup: &Rollup,
    index: &Index,
    date: Date,
    offset: UtcOffset,
    now: OffsetDateTime,
    live: &HashMap<String, Live>,
) -> hippo_core::error::Result<Day> {
    let (start, end) = rollup::day_range(date, offset);
    let frame = Frame { start, end, now };
    let ranged = rollup.sessions_in(start, end)?;
    let days: HashMap<&str, &InRange> = ranged.iter().map(|s| (s.facts.id.as_str(), s)).collect();
    let delegations = rollup.delegations().unwrap_or_default();
    let root = roots(&days, &delegations);
    let read = |id: &str| -> Vec<TurnMark> {
        index
            .turn_marks(id, start - MARGIN, end + MARGIN)
            .unwrap_or_default()
    };
    let mut lanes: Vec<Lane> = Vec::new();
    let mut lane_of: HashMap<String, usize> = HashMap::new();
    let mut tops: Vec<&InRange> = ranged
        .iter()
        .filter(|s| root.get(&s.facts.id) == Some(&s.facts.id) && !s.facts.automated)
        .collect();
    tops.sort_by_key(|s| (s.first, s.facts.id.clone()));
    for s in tops {
        let f = &s.facts;
        let track = track_of(
            &f.id,
            f.title.clone(),
            &read(&f.id),
            true,
            frame,
            live.get(&f.id),
        );
        if track.first.is_none() && !live.get(&f.id).is_some_and(|l| l.live) {
            continue;
        }
        lane_of.insert(f.id.clone(), lanes.len());
        lanes.push(Lane {
            track,
            project: f.project.clone(),
            profile: f.profile(),
            ..Lane::default()
        });
    }
    // Children, under their lane.
    let mut kids: Vec<&InRange> = ranged
        .iter()
        .filter(|s| root.get(&s.facts.id).is_some_and(|r| *r != s.facts.id))
        .collect();
    kids.sort_by_key(|s| (s.first, s.facts.id.clone()));
    for s in kids {
        let Some(&k) = root.get(&s.facts.id).and_then(|r| lane_of.get(r)) else {
            continue;
        };
        let f = &s.facts;
        let track = track_of(
            &f.id,
            f.title.clone(),
            &read(&f.id),
            false,
            frame,
            live.get(&f.id),
        );
        if track.first.is_some() {
            lanes[k].children.push(track);
        }
    }
    // Lanes in order of first activity that day.
    lanes.sort_by_key(|l| (l.track.first, l.track.id.clone()));
    let lane_of: HashMap<String, usize> = lanes
        .iter()
        .enumerate()
        .map(|(i, l)| (l.track.id.clone(), i))
        .collect();
    let lane_for =
        |id: &str| -> Option<usize> { root.get(id).and_then(|r| lane_of.get(r)).copied() };
    // Sub-agents running per minute.
    for lane in &mut lanes {
        // Each child counts once per minute however many spans it has.
        let mut once = vec![0u16; 24 * 60 + 60];
        for c in &lane.children {
            let mut minutes: HashSet<usize> = HashSet::new();
            for s in c.spans.iter().filter(|s| s.kind == SpanKind::Working) {
                let a = ((s.start - start).whole_minutes()).max(0) as usize;
                let b = ((s.end - start).whole_minutes()).max(0) as usize;
                minutes.extend(a..=b.min(once.len() - 1));
            }
            for m in minutes {
                once[m] += 1;
            }
        }
        lane.running = once;
    }
    // Marks: questions and limits from the tracks.
    for lane in &mut lanes {
        let id = lane.track.id.clone();
        for s in &lane.track.spans {
            let target = s.ordinal.map(|o| Target::Turn {
                session: id.clone(),
                ordinal: o,
            });
            match s.kind {
                SpanKind::Waiting => lane.marks.push(Mark {
                    at: s.start,
                    kind: MarkKind::Question,
                    label: match &s.asked {
                        Some(q) => format!("Asked: {q}"),
                        None => "Asked you".to_owned(),
                    },
                    target,
                }),
                SpanKind::Limit => lane.marks.push(Mark {
                    at: s.start,
                    kind: MarkKind::Limit,
                    label: "Stopped at a usage limit".to_owned(),
                    target,
                }),
                SpanKind::Working => {}
            }
        }
    }
    // Pull requests.
    if let Ok(pulls) = rollup.pulls_in(start, end) {
        for p in pulls {
            let (Some(k), Some(at)) = (lane_for(&p.session_id), parse_ts(&p.ts)) else {
                continue;
            };
            let (kind, word) = match p.done {
                PullDone::Merged => (MarkKind::PullMerged, "Merged"),
                _ => (MarkKind::PullOpened, "Opened"),
            };
            lanes[k].marks.push(Mark {
                at,
                kind,
                label: match p.title.as_deref().filter(|t| !t.is_empty()) {
                    Some(t) => format!("{word} {}: {t}", pull_name(&p.url)),
                    None => format!("{word} {}", pull_name(&p.url)),
                },
                target: Some(Target::Url(p.url)),
            });
        }
    }
    // Counted incidents.
    if let Ok(list) = rollup.incidents(start) {
        for i in list.into_iter().filter(|i| !i.quiet && i.at < end) {
            let Some(k) = lane_for(&i.body.chain.root).or_else(|| lane_for(&i.body.chain.session))
            else {
                continue;
            };
            let words: String = i.body.showed.text.chars().take(80).collect();
            lanes[k].marks.push(Mark {
                at: i.at,
                kind: MarkKind::Incident,
                label: format!("Incident: {}", words.trim()),
                target: Some(Target::Incident(i.id)),
            });
        }
    }
    // Carry-ons: a later session that carries on a lane's limit (Home's
    // rule), marked on the stopped lane when it started.
    let pulls = crate::home::pulls_by_session(rollup, &ranged);
    let stops: Vec<(usize, OffsetDateTime)> = lanes
        .iter()
        .enumerate()
        .flat_map(|(i, l)| {
            l.track
                .spans
                .iter()
                .filter(|s| s.kind == SpanKind::Limit)
                .map(move |s| (i, s.start))
        })
        .collect();
    for (i, at) in stops {
        let Some(stop) = days.get(lanes[i].track.id.as_str()).map(|s| &s.facts) else {
            continue;
        };
        let later = ranged
            .iter()
            .filter(|l| root.get(&l.facts.id) == Some(&l.facts.id))
            .filter(|l| l.facts.first_turn.is_some_and(|t| t > at))
            .filter(|l| crate::home::continues(stop, at, &l.facts, &pulls).is_some())
            .min_by_key(|l| l.facts.first_turn);
        if let Some(l) = later {
            let who = l
                .facts
                .profile()
                .map(|p| format!(" · {p}"))
                .unwrap_or_default();
            let title = l
                .facts
                .title
                .clone()
                .unwrap_or_else(|| "another session".to_owned());
            if let Some(t) = l.facts.first_turn.filter(|t| *t < end) {
                lanes[i].marks.push(Mark {
                    at: t,
                    kind: MarkKind::CarryOn,
                    label: format!("Carried on in {title}{who}"),
                    target: Some(Target::Session(l.facts.id.clone())),
                });
            }
        }
    }
    for lane in &mut lanes {
        lane.marks.retain(|m| m.at >= start && m.at < end);
        lane.marks.sort_by_key(|m| m.at);
    }
    let line = day_line(&lanes);
    Ok(Day {
        date,
        start,
        end,
        read_at: now,
        lanes,
        line,
    })
}

/// The day's line from its lanes.
pub fn day_line(lanes: &[Lane]) -> DayLine {
    let mut line = DayLine {
        threads: lanes.len(),
        ..DayLine::default()
    };
    for l in lanes {
        line.subs += l.children.len();
        line.work += l.track.total(SpanKind::Working);
        line.work += l
            .children
            .iter()
            .map(|c| c.total(SpanKind::Working))
            .sum::<Duration>();
        for s in l.track.spans.iter().filter(|s| s.kind == SpanKind::Waiting) {
            line.waited += s.len();
            line.questions += 1;
            if line.longest.as_ref().is_none_or(|(_, w)| s.len() > w.len()) {
                line.longest = Some((l.track.id.clone(), s.clone()));
            }
        }
        for m in &l.marks {
            match m.kind {
                MarkKind::Limit => line.limits += 1,
                MarkKind::PullMerged => line.merged += 1,
                MarkKind::Incident => line.incidents += 1,
                _ => {}
            }
        }
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> OffsetDateTime {
        parse_ts(s).unwrap()
    }

    fn t(ord: u32, role: &str, ts: &str, tool: Option<&str>, head: &str, call: bool) -> TurnMark {
        TurnMark {
            ordinal: ord,
            role: role.to_owned(),
            ts: ts.to_owned(),
            tool_name: tool.map(str::to_owned),
            head: head.to_owned(),
            tail: None,
            file: None,
            about: None,
            command: None,
            call,
        }
    }

    const D0: &str = "2026-11-10T00:00:00Z";
    const D1: &str = "2026-11-11T00:00:00Z";

    fn kinds(tr: &Track) -> Vec<(SpanKind, String, String)> {
        let hm = |t: OffsetDateTime| format!("{:02}:{:02}", t.hour(), t.minute());
        // Working pieces merged for the test: consecutive pieces of one kind.
        let mut out: Vec<(SpanKind, OffsetDateTime, OffsetDateTime)> = Vec::new();
        for s in &tr.spans {
            match out.last_mut() {
                Some(l) if l.0 == s.kind && l.2 >= s.start => l.2 = s.end,
                _ => out.push((s.kind, s.start, s.end)),
            }
        }
        out.into_iter().map(|(k, a, b)| (k, hm(a), hm(b))).collect()
    }

    #[test]
    fn work_waits_and_limits_from_turns() {
        let mut call = t(
            1,
            "tool",
            "2026-11-10T09:01:00Z",
            Some("Bash"),
            "Bash {}",
            true,
        );
        call.about = Some("Run the tests".to_owned());
        let turns = vec![
            t(
                0,
                "user",
                "2026-11-10T09:00:00Z",
                None,
                "crop the covers",
                false,
            ),
            call,
            // A 20-minute command counts as work.
            t(2, "tool", "2026-11-10T09:21:00Z", Some("Bash"), "ok", false),
            t(
                3,
                "assistant",
                "2026-11-10T09:22:00Z",
                None,
                "Done. Shall I merge it?",
                false,
            ),
            // The person answers 30 minutes later: a wait.
            t(4, "user", "2026-11-10T09:52:00Z", None, "yes", false),
            t(
                5,
                "assistant",
                "2026-11-10T09:53:00Z",
                None,
                "Merging.",
                false,
            ),
            // An hour idle, then a limit, then the resume.
            t(
                6,
                "assistant",
                "2026-11-10T11:00:00Z",
                None,
                "You've hit your 5-hour limit · resets 1pm",
                false,
            ),
            t(
                7,
                "user",
                "2026-11-10T13:05:00Z",
                None,
                "Continue where you left off.",
                false,
            ),
            t(8, "assistant", "2026-11-10T13:06:00Z", None, "Back.", false),
        ];
        let tr = track_of(
            "s",
            None,
            &turns,
            true,
            Frame {
                start: at(D0),
                end: at(D1),
                now: at(D1),
            },
            None,
        );
        assert_eq!(
            kinds(&tr),
            vec![
                (SpanKind::Working, "09:00".into(), "09:22".into()),
                (SpanKind::Waiting, "09:22".into(), "09:52".into()),
                (SpanKind::Working, "09:52".into(), "09:53".into()),
                (SpanKind::Limit, "11:00".into(), "13:05".into()),
                (SpanKind::Working, "13:05".into(), "13:06".into()),
            ]
        );
        assert_eq!(tr.total(SpanKind::Waiting), Duration::minutes(30));
        assert_eq!(
            tr.step_at(at("2026-11-10T09:10:00Z")).unwrap().words,
            "Run the tests"
        );
        assert_eq!(tr.first, Some(at("2026-11-10T09:00:00Z")));
    }

    #[test]
    fn an_ask_call_waits_until_its_answer_and_a_sub_agent_never_waits() {
        let turns = vec![
            t(
                0,
                "tool",
                "2026-11-10T10:00:00Z",
                Some("AskUserQuestion"),
                r#"AskUserQuestion {"questions":[{"question":"Which one?","header":"x"}]}"#,
                true,
            ),
            t(
                1,
                "tool",
                "2026-11-10T10:12:00Z",
                Some("AskUserQuestion"),
                "User has answered",
                false,
            ),
            t(
                2,
                "assistant",
                "2026-11-10T10:13:00Z",
                None,
                "Going with A.",
                false,
            ),
        ];
        let tr = track_of(
            "s",
            None,
            &turns,
            true,
            Frame {
                start: at(D0),
                end: at(D1),
                now: at(D1),
            },
            None,
        );
        let w = tr
            .spans
            .iter()
            .find(|s| s.kind == SpanKind::Waiting)
            .unwrap();
        assert_eq!(
            (w.start, w.end),
            (at("2026-11-10T10:00:00Z"), at("2026-11-10T10:12:00Z"))
        );
        assert_eq!(w.asked.as_deref(), Some("Which one?"));
        let sub = track_of(
            "s-agent-a",
            None,
            &turns,
            false,
            Frame {
                start: at(D0),
                end: at(D1),
                now: at(D1),
            },
            None,
        );
        assert!(sub.spans.iter().all(|s| s.kind == SpanKind::Working));
    }

    #[test]
    fn a_lone_limit_message_is_not_work() {
        let turns = vec![t(
            0,
            "assistant",
            "2026-11-10T03:19:55Z",
            None,
            "You've hit your 5-hour limit · resets 5:20am",
            false,
        )];
        let tr = track_of(
            "s-agent-w1",
            None,
            &turns,
            false,
            Frame {
                start: at(D0),
                end: at(D1),
                now: at(D1),
            },
            None,
        );
        assert!(tr.spans.iter().all(|s| s.kind != SpanKind::Working));
    }

    #[test]
    fn a_question_answered_by_a_notification_is_not_a_wait() {
        let turns = vec![
            t(
                0,
                "assistant",
                "2026-11-10T10:00:00Z",
                None,
                "Shall I go on?",
                false,
            ),
            t(
                1,
                "user",
                "2026-11-10T10:30:00Z",
                None,
                "<task-notification> done",
                false,
            ),
            t(
                2,
                "assistant",
                "2026-11-10T10:31:00Z",
                None,
                "It finished.",
                false,
            ),
        ];
        let tr = track_of(
            "s",
            None,
            &turns,
            true,
            Frame {
                start: at(D0),
                end: at(D1),
                now: at(D1),
            },
            None,
        );
        assert!(tr.spans.iter().all(|s| s.kind != SpanKind::Waiting));
    }

    #[test]
    fn spans_are_clipped_to_the_day_and_live_state_reaches_now() {
        let turns = vec![
            t(
                0,
                "assistant",
                "2026-11-09T23:50:00Z",
                None,
                "Shall I?",
                false,
            ),
            t(1, "user", "2026-11-10T00:20:00Z", None, "go", false),
            t(2, "assistant", "2026-11-10T00:21:00Z", None, "ok", false),
        ];
        let now = at("2026-11-10T01:00:00Z");
        let live = Live {
            live: true,
            ..Live::default()
        };
        let tr = track_of(
            "s",
            None,
            &turns,
            true,
            Frame {
                start: at(D0),
                end: at(D1),
                now,
            },
            Some(&live),
        );
        let k = kinds(&tr);
        assert_eq!(k[0], (SpanKind::Waiting, "00:00".into(), "00:20".into()));
        assert_eq!(
            k.last().unwrap(),
            &(SpanKind::Working, "00:20".into(), "01:00".into())
        );
    }

    #[test]
    fn the_moment_and_the_day_line() {
        let w = |a: &str, b: &str, kind| Span {
            start: at(a),
            end: at(b),
            kind,
            strength: 0.5,
            ordinal: None,
            asked: None,
            open: false,
        };
        let lane = |id: &str, spans: Vec<Span>, kids: Vec<Track>| Lane {
            track: Track {
                id: id.into(),
                spans,
                steps: vec![StepAt {
                    at: at("2026-11-10T09:00:00Z"),
                    ordinal: 3,
                    words: "Run the tests".into(),
                }],
                ..Track::default()
            },
            children: kids,
            ..Lane::default()
        };
        let kid = Track {
            id: "a-agent-1".into(),
            title: Some("Crop".into()),
            spans: vec![w(
                "2026-11-10T10:00:00Z",
                "2026-11-10T11:00:00Z",
                SpanKind::Working,
            )],
            ..Track::default()
        };
        let lanes = vec![
            lane(
                "a",
                vec![w(
                    "2026-11-10T09:00:00Z",
                    "2026-11-10T09:30:00Z",
                    SpanKind::Working,
                )],
                vec![kid],
            ),
            lane(
                "b",
                vec![w(
                    "2026-11-10T09:00:00Z",
                    "2026-11-10T10:30:00Z",
                    SpanKind::Waiting,
                )],
                vec![],
            ),
        ];
        let day = Day {
            date: at(D0).date(),
            start: at(D0),
            end: at(D1),
            read_at: at(D1),
            line: day_line(&lanes),
            lanes,
        };
        let m = moment(&day, at("2026-11-10T09:10:00Z"));
        assert_eq!(m.working[0].words.as_deref(), Some("Run the tests"));
        assert_eq!((m.working[0].lane, m.working[0].ordinal), (0, Some(3)));
        assert_eq!(m.waiting, vec![(1, Duration::minutes(10))]);
        let m = moment(&day, at("2026-11-10T10:15:00Z"));
        assert_eq!(m.working[0].words.as_deref(), Some("Crop"));
        assert_eq!(m.working[0].session, "a-agent-1");
        assert_eq!(m.subs_running, 1);
        assert_eq!(day.line.waited, Duration::minutes(90));
        assert_eq!(day.line.work, Duration::minutes(90));
        assert_eq!(day.line.questions, 1);
        assert_eq!(day.line.longest.as_ref().unwrap().0, "b");
    }

    #[test]
    fn pull_names_are_short() {
        assert_eq!(pull_name("https://github.com/o/r/pull/12"), "o/r#12");
    }
}
