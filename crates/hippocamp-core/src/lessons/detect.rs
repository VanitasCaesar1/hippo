//! Finding lesson candidates in the index, without a model: a person's corrections right after an agent action, and
//! failures that repeat across sessions. Candidates cluster by key terms
//! (corrections) or by a normalised error signature (failures).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;
use time::format_description::well_known::Rfc3339;
use time::{OffsetDateTime, UtcOffset};

use crate::error::Result;
use crate::index::{Index, SessionTurn};
use crate::rollup::{Rollup, SessionFacts};
use crate::session_meta::{self, TurnKind};

use super::{Evidence, Kind};

/// The longest person's turn that can be a correction, in characters.
pub const MAX_PERSON_CHARS: usize = 400;
/// The longest quote or agent action kept, in characters.
pub const QUOTE_CHARS: usize = 160;
/// How many evidence items a cluster keeps.
pub const KEEP_EVIDENCE: usize = 6;
/// How many earlier turns are read to find the agent action.
const LOOK_BACK: u32 = 12;
/// A failure qualifies at this many sessions in one local day…
pub const FAILURE_DAY: usize = 3;
/// …or this many in the window.
pub const FAILURE_WINDOW: usize = 4;

/// A cluster of evidence: one candidate lesson.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cluster {
    pub kind: Kind,
    /// Its 4 most frequent terms, sorted and joined by spaces (a
    /// correction), or its signature (a failure).
    pub key: String,
    /// Every key term of its evidence (corrections only).
    pub terms: BTreeSet<String>,
    /// Newest first, at most [`KEEP_EVIDENCE`].
    pub evidence: Vec<Evidence>,
    pub sessions: BTreeSet<String>,
    pub profiles: BTreeSet<String>,
    pub first: OffsetDateTime,
    pub last: OffsetDateTime,
    /// The most sessions on one local day.
    pub most_in_a_day: usize,
}

/// What a detection read and found.
#[derive(Debug, Clone, Default)]
pub struct Found {
    /// Top-level sessions a person started, with a turn in the window.
    pub sessions: usize,
    pub corrections: Vec<Cluster>,
    pub failures: Vec<Cluster>,
}

/// The sessions the pass reads: top-level, started by a person.
fn people_sessions(
    rollup: &Rollup,
    since: OffsetDateTime,
) -> Result<HashMap<String, SessionFacts>> {
    let now = OffsetDateTime::now_utc() + time::Duration::minutes(1);
    Ok(rollup
        .sessions_in(since, now.max(since + time::Duration::seconds(1)))?
        .into_iter()
        .map(|r| r.facts)
        .filter(|f| f.parent.is_none() && !f.automated && !f.id.contains("-agent-"))
        .map(|f| (f.id.clone(), f))
        .collect())
}

/// Finds corrections and repeated failures since `since`; `offset` is the
/// local offset that days are counted in.
pub fn find(
    index: &Index,
    rollup: &Rollup,
    since: OffsetDateTime,
    offset: UtcOffset,
) -> Result<Found> {
    let people = people_sessions(rollup, since)?;
    let mut found = Found {
        sessions: people.len(),
        ..Found::default()
    };
    let mut corrections = Vec::new();
    for turn in index.user_turns_since(since, MAX_PERSON_CHARS * 4)? {
        let Some(facts) = people.get(&turn.session) else {
            continue;
        };
        // Cheap tests first: most user turns are not corrections.
        if !person_text(&turn.text).is_some_and(|t| is_correction(&t)) {
            continue;
        }
        let before = index.turns_before(&turn.session, turn.ordinal, LOOK_BACK, 2000)?;
        let Some(c) = correction_at(&turn, &before) else {
            continue;
        };
        corrections.push((evidence(&turn, facts, c.quote, Some(c.action)), c.terms));
    }
    found.corrections = cluster_corrections(corrections, offset);

    let mut failures: Vec<(Evidence, String)> = Vec::new();
    for turn in index.failed_results_since(since, 4000)? {
        let root = root_of(&turn.session);
        let Some(facts) = people.get(root) else {
            continue;
        };
        let Some(sig) = signature(&turn.text) else {
            continue;
        };
        let before = index.turns_before(&turn.session, turn.ordinal, 1, 600)?;
        let action = before
            .first()
            .filter(|t| kind_of(t) == TurnKind::ToolCall)
            .map(call_line);
        let mut ev = evidence(&turn, facts, sig.clone(), action);
        ev.session = turn.session.clone();
        failures.push((ev, sig));
    }
    found.failures = cluster_failures(failures, offset);
    Ok(found)
}

/// A correction the person gave: what the lessons pass and incidents
/// both read from one turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Correction {
    /// The person's words, one line, at most [`QUOTE_CHARS`].
    pub quote: String,
    /// Its key terms.
    pub terms: Vec<String>,
    /// The agent action it answers, as [`agent_action`] says it.
    pub action: String,
    /// The turn of that action, when it is a turn (not a T3 Code quote).
    pub action_ordinal: Option<u32>,
}

/// How many earlier turns [`correction_at`] needs, nearest first.
pub const CORRECTION_LOOK_BACK: u32 = LOOK_BACK;

/// The correction in `turn`, a person's turn, given the turns before it
/// (nearest first, at most [`CORRECTION_LOOK_BACK`]): `None` when it is not
/// one.
pub fn correction_at(turn: &SessionTurn, before: &[SessionTurn]) -> Option<Correction> {
    let text = person_text(&turn.text)?;
    if !is_correction(&text) {
        return None;
    }
    let terms = key_terms(&text);
    if terms.len() < 2 && !(terms.len() == 1 && short_rule(&text)) {
        return None;
    }
    let quoted = t3_quote(&turn.text)
        .map(|(_, q)| q)
        .filter(|q| !q.trim().is_empty());
    let (action, action_ordinal) = match quoted {
        Some(q) => (format!("agent: {}", one_line(&q, QUOTE_CHARS - 7)), None),
        None => {
            let action = agent_action(before)?;
            (action, agent_action_turn(before).map(|t| t.ordinal))
        }
    };
    Some(Correction {
        quote: one_line(&text, QUOTE_CHARS),
        terms,
        action,
        action_ordinal,
    })
}

fn root_of(id: &str) -> &str {
    id.split_once("-agent-").map_or(id, |(p, _)| p)
}

fn evidence(
    turn: &SessionTurn,
    facts: &SessionFacts,
    quote: String,
    action: Option<String>,
) -> Evidence {
    Evidence {
        session: turn.session.clone(),
        ordinal: turn.ordinal,
        profile: facts.profile(),
        project: facts.project.clone(),
        at: OffsetDateTime::parse(&turn.ts, &Rfc3339).unwrap_or(OffsetDateTime::UNIX_EPOCH),
        quote,
        action,
    }
}

// ---------------------------------------------------------------------------
// A person's turn

/// Prefixes of text an agent or client relays in the person's place.
const RELAYED: &[&str] = &[
    "Delegated task node:",
    "Stop hook feedback",
    "A session-scoped Stop hook",
    "Note: the T3 server",
    "Continue where you left off.",
];

/// Whether a person's turn is text an agent or client relays in the
/// person's place (a delegated task's report, a hook's feedback).
pub fn relayed(text: &str) -> bool {
    RELAYED.iter().any(|p| text.starts_with(p))
}

static THIRD_PERSON: LazyLock<Regex> = LazyLock::new(|| {
    // A person named in the third person: an agent's words about them.
    // Generic roles, and any `the <noun>` that reports what someone said,
    // not one project's titles.
    Regex::new(r"(?i)\bthe (user|human|person|parent)\b|\bthe [a-z]+ (said|says|asked|asks|wants|told|decided|prefers|approved|confirmed)\b").expect("valid regex")
});

static CITATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[Assistant quote\]\(t3-citation://[^)?]*\?([^)\s]*)\)").expect("valid regex")
});

/// `%XX` and `+` decoded.
fn url_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => match u8::from_str_radix(&text[i + 1..i + 3], 16) {
                Ok(b) => {
                    out.push(b);
                    i += 2;
                }
                Err(_) => out.push(b'%'),
            },
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A T3 Code quote of the agent's words (`[Assistant quote](t3-citation://…?text=…&comment=…)`):
/// the person's comment on it, in place of the link, and the quoted text.
pub fn t3_quote(raw: &str) -> Option<(String, String)> {
    let c = CITATION.captures(raw)?;
    let mut quoted = String::new();
    let mut comment = String::new();
    for pair in c[1].split('&') {
        match pair.split_once('=') {
            Some(("text", v)) => quoted = url_decode(v),
            Some(("comment", v)) => comment = url_decode(v),
            _ => {}
        }
    }
    let whole = c.get(0).expect("match");
    let rest = format!("{} {}", &raw[..whole.start()], &raw[whole.end()..]);
    let person = format!("{} {}", comment.trim(), rest.trim())
        .trim()
        .to_owned();
    Some((person, quoted))
}

/// The person's own words in a `user` turn's text, or `None` when the
/// turn is injected, relayed or too long to be a correction. A T3 Code
/// quote of the agent's words counts as the person's comment on it.
pub fn person_text(raw: &str) -> Option<String> {
    if session_meta::classify("user", None, raw) != TurnKind::Request {
        return None;
    }
    let owned;
    let raw = match t3_quote(raw) {
        Some((person, _)) => {
            owned = person;
            owned.as_str()
        }
        None => raw,
    };
    let body = session_meta::request_text(raw)?;
    let kept: Vec<&str> = body
        .lines()
        .filter(|l| {
            let t = l.trim_start();
            !(t.starts_with("[Attached image")
                || t.starts_with("[Image")
                || t.starts_with("[Attached file"))
        })
        .collect();
    let text = kept.join("\n").trim().to_owned();
    if text.is_empty() || text.chars().count() > MAX_PERSON_CHARS {
        return None;
    }
    // Markdown code and bold marks: agents write them, a person's quick
    // correction rarely does.
    let marked = text.contains("**") || text.contains('`');
    if RELAYED.iter().any(|p| text.starts_with(p)) || THIRD_PERSON.is_match(&text) || marked {
        return None;
    }
    Some(text)
}

static LEADING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\W*(no|nope|nah|not|stop|never|don'?t|dont|do not|wrong|why)\b")
        .expect("valid regex")
});

static ANYWHERE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?ix)
        \bhow\s+many\s+times\b
        | \bi\s+(said|told\s+you|asked)\b
        | \bwtf\b
        | \bwhy\b[^.?!]*\bagain\b
        | \bagain\s*[?!]
        | \bnever\s+\w+
        | \b(don'?t|dont|do\s+not)\s+\w+
        | \bstop\s+\w+ing\b
        | \binstead\s+of\b
        | \buse\s+\S+\s+not\b
        ",
    )
    .expect("valid regex")
});

/// Whether `text`, a person's turn, carries a corrective marker.
pub fn is_correction(text: &str) -> bool {
    LEADING.is_match(text) || ANYWHERE.is_match(text) || shouting(text)
}

static OBJECTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\?|\b(no|not|dont|don't|isnt|isn't|never|why|wtf|wrong)\b")
        .expect("valid regex")
});

static RULE_LIKE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\W*(no|never|don'?t|dont|do not|stop|use)\b").expect("valid regex")
});

/// A short turn shaped like a rule ("dont use emojis", "no fast mode"):
/// at most 6 words, starting with a negation or `use`. One key term is
/// enough for it.
fn short_rule(text: &str) -> bool {
    text.split_whitespace().count() <= 6 && RULE_LIKE.is_match(text)
}

/// Shouting that objects: at least 3 words and 10 letters, 70 % of the
/// letters capitals, and a question mark or a negation. Capitals alone are
/// often emphasis of a request, not a correction.
fn shouting(text: &str) -> bool {
    let letters: Vec<char> = text.chars().filter(|c| c.is_alphabetic()).collect();
    let upper = letters.iter().filter(|c| c.is_uppercase()).count();
    text.split_whitespace().count() >= 3
        && letters.len() >= 10
        && upper * 10 >= letters.len() * 7
        && OBJECTION.is_match(text)
}

const STOP: &[&str] = &[
    "the",
    "and",
    "for",
    "you",
    "are",
    "was",
    "were",
    "not",
    "but",
    "with",
    "this",
    "that",
    "these",
    "those",
    "there",
    "their",
    "they",
    "them",
    "then",
    "than",
    "what",
    "when",
    "where",
    "which",
    "who",
    "why",
    "how",
    "its",
    "it's",
    "have",
    "has",
    "had",
    "does",
    "did",
    "doing",
    "done",
    "can",
    "cant",
    "can't",
    "could",
    "should",
    "would",
    "will",
    "won't",
    "just",
    "only",
    "also",
    "again",
    "still",
    "even",
    "any",
    "all",
    "some",
    "more",
    "most",
    "very",
    "too",
    "into",
    "from",
    "about",
    "out",
    "our",
    "your",
    "yours",
    "mine",
    "here",
    "now",
    "then",
    "never",
    "dont",
    "don't",
    "stop",
    "said",
    "told",
    "asked",
    "times",
    "many",
    "much",
    "use",
    "using",
    "used",
    "make",
    "made",
    "get",
    "got",
    "need",
    "want",
    "like",
    "look",
    "looks",
    "see",
    "say",
    "one",
    "thing",
    "things",
    "way",
    "yes",
    "nope",
    "nah",
    "instead",
    "wrong",
    "right",
    "okay",
    "it",
    "is",
    "be",
    "been",
    "being",
    "let",
    "lets",
    "let's",
    "know",
    "think",
    "should",
    "shouldn't",
    "isnt",
    "isn't",
    "arent",
    "aren't",
    "didnt",
    "didn't",
    "doesnt",
    "doesn't",
    "ever",
    "every",
    "each",
    "other",
    "same",
    "such",
    "own",
    "off",
    "over",
    "under",
    "again",
    "ive",
    "i've",
    "im",
    "i'm",
    "u",
    "ur",
    "pls",
    "please",
    "plz",
    "thanks",
    "thank",
    "wtf",
    "bruh",
    "man",
    "dude",
    "bro",
    "shit",
    "shitty",
    "damn",
    "fucking",
    "fuck",
    "idiot",
    "stupid",
    "lol",
    "lmao",
    "ugh",
    "omg",
    "hell",
    "crap",
    "seriously",
    "literally",
    "actually",
    "really",
    "bad",
    "good",
    "worse",
    "better",
    "garbage",
    "useless",
    "work",
    "works",
    "working",
    "thats",
    "that's",
    "whats",
    "what's",
    "go",
    "going",
    "put",
    "keep",
    "come",
    "take",
    "give",
    "show",
];

/// The key terms of `text`: words of 3 or more letters that are not stop
/// or frustration words, lower-cased, each cut to its first 6 letters,
/// in order of first appearance, without repeats.
pub fn key_terms(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for word in text.split(|c: char| !(c.is_alphanumeric() || c == '\'')) {
        let w = word.trim_matches('\'').to_lowercase();
        if w.chars().count() < 3
            || w.chars().all(|c| c.is_ascii_digit())
            || STOP.contains(&w.as_str())
        {
            continue;
        }
        let t: String = w.chars().take(6).collect();
        if !out.contains(&t) {
            out.push(t);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// The agent action before it

fn kind_of(t: &SessionTurn) -> TurnKind {
    session_meta::classify(&t.role, t.tool_name.as_deref(), &t.text)
}

/// The agent action a person's turn answers, from the turns before it
/// (nearest first): `None` when the nearest non-injected turn is the
/// person's own; else the nearest tool call of the agent's last stretch,
/// else its last sentence.
pub fn agent_action(before: &[SessionTurn]) -> Option<String> {
    let stretch = stretch(before);
    if stretch.is_empty() {
        return None;
    }
    if let Some(call) = stretch.iter().find(|t| kind_of(t) == TurnKind::ToolCall) {
        return Some(call_line(call));
    }
    stretch
        .iter()
        .find(|t| t.role == "assistant" && !t.text.trim().is_empty())
        .map(|t| format!("agent: {}", last_sentence(&t.text)))
        .or_else(|| Some("agent ran a tool".to_owned()))
}

/// The turn [`agent_action`] names: the nearest tool call of the agent's
/// last stretch, else its last words.
pub fn agent_action_turn(before: &[SessionTurn]) -> Option<&SessionTurn> {
    let stretch = stretch(before);
    stretch
        .iter()
        .find(|t| kind_of(t) == TurnKind::ToolCall)
        .or_else(|| {
            stretch
                .iter()
                .find(|t| t.role == "assistant" && !t.text.trim().is_empty())
        })
        .copied()
}

/// The agent's turns since the person's last request, nearest first.
fn stretch(before: &[SessionTurn]) -> Vec<&SessionTurn> {
    let mut stretch = Vec::new();
    for t in before {
        match kind_of(t) {
            TurnKind::Injection(_) => continue,
            TurnKind::Request => break,
            _ => stretch.push(t),
        }
    }
    stretch
}

/// `<Tool>: <command | file_path | path | pattern | description | input>`,
/// one line, at most [`QUOTE_CHARS`] characters.
pub fn call_line(t: &SessionTurn) -> String {
    let name = t.tool_name.clone().unwrap_or_else(|| "tool".to_owned());
    let input = t.text.strip_prefix(&name).unwrap_or(&t.text).trim();
    let shown = serde_json::from_str::<Value>(input)
        .ok()
        .and_then(|v| {
            [
                "command",
                "cmd",
                "file_path",
                "path",
                "pattern",
                "url",
                "description",
                "prompt",
            ]
            .iter()
            .find_map(|k| match v.get(*k) {
                Some(Value::String(s)) => Some(s.clone()),
                Some(Value::Array(a)) => Some(
                    a.iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(" "),
                ),
                _ => None,
            })
        })
        .unwrap_or_else(|| input.to_owned());
    one_line(&format!("{name}: {shown}"), QUOTE_CHARS)
}

fn last_sentence(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = flat.trim_end_matches(['.', '!', ' ']);
    let start = trimmed.rfind(['.', '!', '?', '\n']).map_or(0, |i| i + 1);
    one_line(trimmed[start..].trim(), QUOTE_CHARS)
}

/// `text` on one line, cut at `max` characters with an ellipsis.
pub fn one_line(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let cut: String = flat.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", cut.trim_end())
}

// ---------------------------------------------------------------------------
// Failures

static ERROR_WORD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(error|not found|no such file|no matches found|denied|failed|fatal|panicked|traceback|cannot|could not|unable to|refused|timed out|invalid|unknown|unexpected)")
        .expect("valid regex")
});
static PROGRAM_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[\w.()\[\]-]+:\s").expect("valid regex"));
static EXIT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^Exit code:? (-?\d+)").expect("valid regex"));
static GLOB: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\*\.\w+").expect("valid regex"));
static PATH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:~|\.{1,2}|[\w.@+-]+)?(?:/[\w.@+-]+)+/?").expect("valid regex")
});
static RUN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"={2,}").expect("valid regex"));
static HEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b[0-9a-f]{7,}\b").expect("valid regex"));
static NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d+").expect("valid regex"));
static QUOTED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"'[^']*'|"[^"]*"|`[^`]*`|‘[^’]*’|“[^”]*”"#).expect("valid regex")
});

/// A failed tool result's signature, or `None` when it is
/// not a failure: exit code 0, or no error line to name.
pub fn signature(result: &str) -> Option<String> {
    let first = result.lines().next().unwrap_or_default();
    if let Some(c) = EXIT.captures(first) {
        if &c[1] == "0" {
            return None;
        }
    }
    if let Some(rest) = first.strip_prefix("<tool_use_error>") {
        let text = rest.replace("</tool_use_error>", "");
        let text = text.trim();
        return (!text.is_empty()).then(|| normalise(text));
    }
    let code_like = |l: &str| {
        ["return", "if ", "}", "//", "#", "*", "-", "+", ">"]
            .iter()
            .any(|p| l.starts_with(p))
    };
    let hits: Vec<&str> = result
        .lines()
        .skip(1)
        .take(60)
        .map(str::trim)
        .filter(|l| !l.is_empty() && l.len() < 300 && !code_like(l) && ERROR_WORD.is_match(l))
        .collect();
    let line = hits
        .iter()
        .find(|l| {
            PROGRAM_LINE.is_match(l)
                || l.to_lowercase().starts_with("error")
                || l.to_lowercase().starts_with("fatal")
        })
        .or(hits.first())?;
    Some(normalise(line))
}

fn normalise(line: &str) -> String {
    let s = GLOB.replace_all(line, "*.<ext>");
    let s = PATH.replace_all(&s, "<path>");
    let s = HEX.replace_all(&s, "<id>");
    let s = QUOTED.replace_all(&s, "<q>");
    let s = NUMBER.replace_all(&s, "N");
    let s = RUN.replace_all(&s, "==");
    one_line(&s, QUOTE_CHARS)
}

// ---------------------------------------------------------------------------
// Clustering

/// Whether two term sets say the same thing: at least 2 shared terms and a
/// Jaccard index of at least 0.34.
pub fn same_terms(a: &BTreeSet<String>, b: &BTreeSet<String>) -> bool {
    let shared = a.intersection(b).count();
    let union = a.union(b).count();
    shared >= 2 && union > 0 && shared * 100 >= union * 34
}

/// A cluster being built: its terms and its members with their terms.
type Group = (BTreeSet<String>, Vec<(Evidence, Vec<String>)>);

fn cluster_corrections(mut items: Vec<(Evidence, Vec<String>)>, offset: UtcOffset) -> Vec<Cluster> {
    items.sort_by_key(|i| i.0.at);
    let mut groups: Vec<Group> = Vec::new();
    for (ev, terms) in items {
        let set: BTreeSet<String> = terms.iter().cloned().collect();
        match groups.iter_mut().find(|(g, _)| same_terms(g, &set)) {
            Some((g, members)) => {
                g.extend(set);
                members.push((ev, terms));
            }
            None => groups.push((set, vec![(ev, terms)])),
        }
    }
    groups
        .into_iter()
        .map(|(terms, members)| {
            let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
            for (_, ts) in &members {
                for t in ts {
                    *counts.entry(t.as_str()).or_default() += 1;
                }
            }
            let mut ranked: Vec<(&str, usize)> = counts.into_iter().collect();
            ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
            let mut key: Vec<&str> = ranked.iter().take(4).map(|(t, _)| *t).collect();
            key.sort_unstable();
            let key = key.join(" ");
            make_cluster(
                Kind::Correction,
                key,
                terms,
                members.into_iter().map(|(e, _)| e).collect(),
                offset,
            )
        })
        .collect()
}

/// Whether two failure signatures are the same error: their key terms
/// overlap by a Jaccard index of at least 0.6 (two terms at least), or 0.4
/// when they start with the same four words; or, both longer than five
/// words, they start with the same five.
pub(crate) fn same_error(a: &str, b: &str) -> bool {
    let head = |s: &str, n: usize| s.split_whitespace().take(n).collect::<Vec<_>>().join(" ");
    let long = |s: &str, n: usize| s.split_whitespace().count() >= n;
    if long(a, 6) && long(b, 6) && head(a, 5) == head(b, 5) {
        return true;
    }
    let same_start = long(a, 4) && head(a, 4) == head(b, 4);
    let terms = |s: &str| -> BTreeSet<String> {
        key_terms(&s.replace(['<', '>'], " ")).into_iter().collect()
    };
    let (a, b) = (terms(a), terms(b));
    let shared = a.intersection(&b).count();
    let union = a.union(&b).count();
    shared >= 2 && (shared * 10 >= union * 6 || (same_start && shared * 10 >= union * 4))
}

fn cluster_failures(items: Vec<(Evidence, String)>, offset: UtcOffset) -> Vec<Cluster> {
    // The most frequent signature of a group names it.
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for (_, sig) in &items {
        *counts.entry(sig.clone()).or_default() += 1;
    }
    let mut order: Vec<(String, usize)> = counts.into_iter().collect();
    order.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let mut name: HashMap<String, String> = HashMap::new();
    let mut heads: Vec<String> = Vec::new();
    for (sig, _) in order {
        let head = heads.iter().find(|h| same_error(h, &sig)).cloned();
        match head {
            Some(h) => {
                name.insert(sig, h);
            }
            None => {
                heads.push(sig.clone());
                name.insert(sig.clone(), sig);
            }
        }
    }
    let mut by_sig: BTreeMap<String, Vec<Evidence>> = BTreeMap::new();
    for (ev, sig) in items {
        let head = name.get(&sig).cloned().unwrap_or(sig);
        by_sig.entry(head).or_default().push(ev);
    }
    by_sig
        .into_iter()
        .filter_map(|(sig, evs)| {
            // One evidence item per top-level session: its newest hit.
            let mut newest: BTreeMap<String, Evidence> = BTreeMap::new();
            for ev in evs {
                let root = root_of(&ev.session).to_owned();
                match newest.get(&root) {
                    Some(old) if old.at >= ev.at => {}
                    _ => {
                        newest.insert(root, ev);
                    }
                }
            }
            let c = make_cluster(
                Kind::Failure,
                sig,
                BTreeSet::new(),
                newest.into_values().collect(),
                offset,
            );
            (c.most_in_a_day >= FAILURE_DAY || c.sessions.len() >= FAILURE_WINDOW).then_some(c)
        })
        .collect()
}

fn make_cluster(
    kind: Kind,
    key: String,
    terms: BTreeSet<String>,
    mut evidence: Vec<Evidence>,
    offset: UtcOffset,
) -> Cluster {
    evidence.sort_by_key(|e| std::cmp::Reverse(e.at));
    let sessions: BTreeSet<String> = evidence
        .iter()
        .map(|e| root_of(&e.session).to_owned())
        .collect();
    let profiles: BTreeSet<String> = evidence.iter().filter_map(|e| e.profile.clone()).collect();
    let mut days: BTreeMap<time::Date, BTreeSet<&str>> = BTreeMap::new();
    for e in &evidence {
        days.entry(e.at.to_offset(offset).date())
            .or_default()
            .insert(root_of(&e.session));
    }
    let most_in_a_day = days.values().map(BTreeSet::len).max().unwrap_or(0);
    let first = evidence.last().map_or(OffsetDateTime::UNIX_EPOCH, |e| e.at);
    let last = evidence
        .first()
        .map_or(OffsetDateTime::UNIX_EPOCH, |e| e.at);
    evidence.truncate(KEEP_EVIDENCE);
    Cluster {
        kind,
        key,
        terms,
        evidence,
        sessions,
        profiles,
        first,
        last,
        most_in_a_day,
    }
}
