//! The context x-ray (`hippocamp xray`): what context hippocamp's hooks gave a
//! session, whether the session used it, what it searched the notes for,
//! and whether its claims have evidence.
//!
//! Everything here reads transcripts after the fact. "Read" and "cited"
//! are a proxy: what the model attended to inside its context cannot be
//! known from a transcript.

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

use aho_corasick::AhoCorasick;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::{Duration, OffsetDateTime};

use crate::config::Workspace;
use crate::dream::observed::{self, Kind, Resolver};
use crate::error::{Error, Result};
use crate::index::{Index, StoredTurn};
use crate::notes::{normalise, Notes};
use crate::records::Session;
use crate::rollup::Rollup;
use crate::sessions;

pub mod claims;
pub mod searches;
pub(crate) mod shell;

pub use claims::{Claim, ClaimKind, Status};
pub use searches::Search;

/// How far into a transcript its hook output is looked for.
const HEAD_BYTES: u64 = 1024 * 1024;

/// A quote is at least this many characters in a row (whitespace runs compare as one
/// space).
pub const QUOTE_CHARS: usize = 32;

/// Shorter injected text is matched whole, and text under this not at all.
const MIN_WHOLE: usize = 12;

// ---------------------------------------------------------------------------
// The block

/// Where a session's context block was read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Source {
    /// The declared session record's `context_start`/`context_prompt`.
    Record,
    /// The client's own copy of the hook output in the transcript.
    Transcript,
    /// Neither: the block was not kept.
    NotRecorded,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Record => "record",
            Source::Transcript => "transcript",
            Source::NotRecorded => "not-recorded",
        }
    }

    pub fn parse(s: &str) -> Source {
        match s {
            "record" => Source::Record,
            "transcript" => Source::Transcript,
            _ => Source::NotRecorded,
        }
    }
}

/// The text hippocamp's hooks printed for a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub source: Source,
    pub start: Option<String>,
    pub prompt: Option<String>,
}

/// The block of a session: its record's fields, else (Claude Code) the
/// transcript's `hook_success` attachments for `hippocamp hook`, else not
/// recorded. Never recompiled.
pub fn block(record: Option<&Session>, tool: &str, transcript: &Path) -> Block {
    if let Some(r) = record.filter(|r| !r.derived) {
        if r.context_start.is_some() || r.context_prompt.is_some() {
            return Block {
                source: Source::Record,
                start: r.context_start.clone(),
                prompt: r.context_prompt.clone(),
            };
        }
    }
    if tool == "claude-code" {
        let (start, prompt) = transcript_block(transcript);
        if start.is_some() || prompt.is_some() {
            return Block {
                source: Source::Transcript,
                start,
                prompt,
            };
        }
    }
    Block {
        source: Source::NotRecorded,
        start: None,
        prompt: None,
    }
}

/// The `hippocamp hook session-start` and `hippocamp hook prompt` output a Claude
/// Code transcript's head records, read until the first assistant record
/// after the prompt hook (or [`HEAD_BYTES`]).
pub fn transcript_block(path: &Path) -> (Option<String>, Option<String>) {
    let Ok(file) = File::open(path) else {
        return (None, None);
    };
    let mut reader = BufReader::new(file.take(HEAD_BYTES));
    let mut line = String::new();
    let (mut start, mut prompt) = (None, None);
    let mut assistant_seen = false;
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if line.contains("\"type\":\"assistant\"") || line.contains("\"type\": \"assistant\"") {
            assistant_seen = true;
            if prompt.is_some() || start.is_some() {
                break;
            }
            continue;
        }
        if !line.contains("hook_success") || !line.contains("hippocamp") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(a) = v.get("attachment") else {
            continue;
        };
        if a.get("type").and_then(Value::as_str) != Some("hook_success") {
            continue;
        }
        let command = a.get("command").and_then(Value::as_str).unwrap_or("");
        let text = a
            .get("stdout")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .or_else(|| a.get("content").and_then(Value::as_str))
            .unwrap_or("");
        if text.trim().is_empty() {
            continue;
        }
        if is_hippocamp_hook(command, "session-start") && start.is_none() {
            start = Some(text.to_owned());
        } else if is_hippocamp_hook(command, "prompt") && prompt.is_none() {
            prompt = Some(text.to_owned());
            if assistant_seen {
                break;
            }
        }
    }
    (start, prompt)
}

/// Whether a hook command runs `hippocamp hook <sub>`.
fn is_hippocamp_hook(command: &str, sub: &str) -> bool {
    let words: Vec<&str> = command.split_whitespace().collect();
    words.windows(3).any(|w| {
        Path::new(w[0]).file_name().and_then(|n| n.to_str()) == Some("hippocamp")
            && w[1] == "hook"
            && w[2] == sub
    })
}

// ---------------------------------------------------------------------------
// Parts

/// A part's kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PartKind {
    Required,
    Relevant,
    Housekeeping,
}

impl PartKind {
    pub fn as_str(self) -> &'static str {
        match self {
            PartKind::Required => "required",
            PartKind::Relevant => "relevant",
            PartKind::Housekeeping => "housekeeping",
        }
    }

    pub(crate) fn parse(s: &str) -> PartKind {
        match s {
            "required" => PartKind::Required,
            "relevant" => PartKind::Relevant,
            _ => PartKind::Housekeeping,
        }
    }
}

/// What a session did with a part.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Use {
    Read,
    Cited,
    Ignored,
}

impl Use {
    pub fn as_str(self) -> &'static str {
        match self {
            Use::Read => "read",
            Use::Cited => "cited",
            Use::Ignored => "ignored",
        }
    }

    pub(crate) fn parse(s: &str) -> Use {
        match s {
            "read" => Use::Read,
            "cited" => Use::Cited,
            _ => Use::Ignored,
        }
    }
}

/// One part of a context block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Part {
    pub kind: PartKind,
    /// The note's title, else `entry section`, `session line`,
    /// `linter counts`, `warning` or `other`.
    pub label: String,
    /// The note it names, relative to the workspace.
    pub note: Option<PathBuf>,
    /// The injected text.
    #[serde(skip)]
    pub text: String,
    /// The bytes the part took in the block.
    pub bytes: usize,
    /// The note's own size, when it exists now.
    pub note_bytes: Option<u64>,
    #[serde(rename = "use")]
    pub used: Use,
    /// The first turn that read it.
    pub read_at: Option<u32>,
    /// The first assistant turn that cited it.
    pub cited_at: Option<u32>,
    /// How many assistant turns cited it.
    pub cited: u32,
}

impl Part {
    fn new(kind: PartKind, label: &str, note: Option<PathBuf>) -> Part {
        Part {
            kind,
            label: label.to_owned(),
            note,
            text: String::new(),
            bytes: 0,
            note_bytes: None,
            used: Use::Ignored,
            read_at: None,
            cited_at: None,
            cited: 0,
        }
    }

    fn push(&mut self, line: &str) {
        self.bytes += line.len() + 1;
        if !self.text.is_empty() {
            self.text.push('\n');
        }
        self.text.push_str(line);
    }
}

/// Splits a block into its parts. `notes` resolves the
/// relevant notes' titles; the entry note's `required:` line joins the
/// entry section.
pub fn parts(block: &Block, ws: &Workspace, notes: &Notes) -> Vec<Part> {
    let mut by_title: BTreeMap<String, PathBuf> = BTreeMap::new();
    for (path, note) in &notes.notes {
        by_title
            .entry(note.title())
            .or_insert_with(|| normalise(&ws.notes_dir.join(path)));
    }
    let size = |note: &Path| -> Option<u64> {
        let below = note.strip_prefix(&ws.notes_dir).ok()?;
        notes
            .notes
            .get(below)
            .map(|n| (n.raw_frontmatter.len() + n.body.len()) as u64)
    };
    let title_of = |note: &Path| -> String {
        note.strip_prefix(&ws.notes_dir)
            .ok()
            .and_then(|b| notes.notes.get(b))
            .map(|n| n.title())
            .unwrap_or_else(|| {
                note.file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
            })
    };
    let entry = normalise(&ws.entry_note);
    let mut out: Vec<Part> = Vec::new();
    for (text, is_start) in [(&block.start, true), (&block.prompt, false)] {
        let Some(text) = text else { continue };
        let mut entry_part: Option<usize> = None;
        let mut current: Option<usize> = None;
        let mut in_entry = is_start;
        let mut relevant_header: Option<String> = None;
        for line in text.lines() {
            let trimmed = line.trim_end();
            if trimmed.starts_with("warning: ") {
                out.push(Part::new(PartKind::Housekeeping, "warning", None));
                current = Some(out.len() - 1);
                out[current.unwrap()].push(line);
                continue;
            }
            if let Some(path) = trimmed.strip_prefix("required: ") {
                in_entry = false;
                let note = normalise(Path::new(path.trim()));
                if note == entry {
                    if let Some(i) = entry_part {
                        out[i].push(line);
                        current = Some(i);
                        continue;
                    }
                }
                let mut p = Part::new(PartKind::Required, &title_of(&note), Some(note));
                p.push(line);
                out.push(p);
                current = Some(out.len() - 1);
                continue;
            }
            if trimmed.starts_with("session: ") {
                in_entry = false;
                let mut p = Part::new(PartKind::Housekeeping, "session line", None);
                p.push(line);
                out.push(p);
                current = Some(out.len() - 1);
                continue;
            }
            if trimmed == "relevant:" {
                in_entry = false;
                relevant_header = Some(line.to_owned());
                current = None;
                continue;
            }
            if relevant_header.is_some()
                || current.is_some_and(|i| out[i].kind == PartKind::Relevant)
            {
                if let Some(item) = trimmed.strip_prefix("- ") {
                    let (title, note) = relevant_title(item, &by_title);
                    let mut p = Part::new(PartKind::Relevant, &title, note);
                    if let Some(h) = relevant_header.take() {
                        p.push(&h);
                    }
                    p.push(line);
                    out.push(p);
                    current = Some(out.len() - 1);
                    continue;
                }
            }
            if trimmed.starts_with("linter: ") {
                in_entry = false;
                let mut p = Part::new(PartKind::Housekeeping, "linter counts", None);
                p.push(line);
                out.push(p);
                current = Some(out.len() - 1);
                continue;
            }
            if in_entry {
                let i = *entry_part.get_or_insert_with(|| {
                    out.push(Part::new(
                        PartKind::Required,
                        &title_of(&entry),
                        Some(entry.clone()),
                    ));
                    out.len() - 1
                });
                out[i].push(line);
                current = Some(i);
                continue;
            }
            match current {
                Some(i) if trimmed.is_empty() => {
                    out[i].bytes += line.len() + 1;
                }
                _ if trimmed.is_empty() => {}
                _ => {
                    let mut p = Part::new(PartKind::Housekeeping, "other", None);
                    p.push(line);
                    out.push(p);
                    current = Some(out.len() - 1);
                }
            }
        }
        if let Some(h) = relevant_header {
            let mut p = Part::new(PartKind::Housekeeping, "other", None);
            p.push(&h);
            out.push(p);
        }
    }
    for p in &mut out {
        if let Some(note) = &p.note {
            p.note_bytes = size(note);
        }
    }
    out
}

/// A relevant line's title and note: the longest `<title>: ` prefix that is
/// a note's title, else the text before the first `: `.
fn relevant_title(item: &str, by_title: &BTreeMap<String, PathBuf>) -> (String, Option<PathBuf>) {
    let mut cuts: Vec<usize> = item.match_indices(": ").map(|(i, _)| i).collect();
    cuts.reverse();
    for i in &cuts {
        if let Some(note) = by_title.get(&item[..*i]) {
            return (item[..*i].to_owned(), Some(note.clone()));
        }
    }
    let title = cuts.last().map_or(item, |i| &item[..*i]).to_owned();
    let note = by_title.get(&title).cloned();
    (title, note)
}

/// Text as quotes compare it: `\n`, `\r`, `\t` as
/// whitespace, `\"` as `"`, whitespace runs as one space.
pub(crate) fn squash(text: &str) -> String {
    crate::dream::validate::squash(text).into_iter().collect()
}

/// Classifies each part's use over the session's own turns.
/// `dir` is the session's directory, as [`Resolver::session_dir`] gives it.
pub fn classify(
    parts: &mut [Part],
    turns: &[StoredTurn],
    resolver: &Resolver<'_>,
    dir: Option<&Path>,
    session_id: &str,
) {
    // Probes: every 32-character window of each part's text, or the whole
    // text when shorter.
    let mut probes: Vec<String> = Vec::new();
    let mut owner: Vec<usize> = Vec::new();
    for (i, p) in parts.iter().enumerate() {
        // The session line is matched by its id alone (`names_id`): the
        // client puts the id in its own paths.
        if p.label == "session line" {
            continue;
        }
        let text: Vec<char> = squash(&p.text).chars().collect();
        if text.len() < MIN_WHOLE {
            continue;
        }
        if text.len() <= QUOTE_CHARS {
            probes.push(text.iter().collect());
            owner.push(i);
            continue;
        }
        for w in text.windows(QUOTE_CHARS) {
            probes.push(w.iter().collect());
            owner.push(i);
        }
    }
    let matcher = AhoCorasick::new(&probes).ok();
    let titles: Vec<Option<regex::Regex>> = parts
        .iter()
        .map(|p| {
            (p.note.is_some() && p.label.split_whitespace().count() >= 2).then(|| {
                regex::Regex::new(&format!(r"(^|[^\w]){}($|[^\w])", regex::escape(&p.label)))
                    .expect("escaped")
            })
        })
        .collect();
    let session_part = parts.iter().position(|p| p.label == "session line");
    let mut cited_turns: Vec<Vec<u32>> = vec![Vec::new(); parts.len()];
    for t in turns {
        let call = crate::rollup::is_call(t);
        let assistant = t.role == "assistant";
        if !call && !assistant {
            continue;
        }
        let mut hit: Vec<usize> = Vec::new();
        if let Some(m) = &matcher {
            let text = squash(&t.text);
            for found in m.find_overlapping_iter(&text) {
                let i = owner[found.pattern().as_usize()];
                if !hit.contains(&i) {
                    hit.push(i);
                }
            }
        }
        let observed = observed::observe(resolver, dir, t);
        for (i, p) in parts.iter_mut().enumerate() {
            let quoted = hit.contains(&i);
            let named = p.note.as_ref().is_some_and(|note| {
                observed.iter().any(|o| {
                    &o.note == note
                        && if call {
                            matches!(o.kind, Kind::Read | Kind::Edit)
                        } else {
                            o.kind == Kind::Discuss
                        }
                })
            });
            if call {
                let id_used = Some(i) == session_part && names_id(&t.text, session_id);
                if (quoted || named || id_used) && p.read_at.is_none() {
                    p.read_at = Some(t.ordinal);
                }
            } else {
                let by_title = titles[i].as_ref().is_some_and(|re| re.is_match(&t.text));
                if quoted || named || by_title {
                    p.cited_at.get_or_insert(t.ordinal);
                    if !cited_turns[i].contains(&t.ordinal) {
                        cited_turns[i].push(t.ordinal);
                    }
                }
            }
        }
    }
    for (p, cited) in parts.iter_mut().zip(cited_turns) {
        p.cited = cited.len() as u32;
        p.used = if p.read_at.is_some() {
            Use::Read
        } else if p.cited > 0 {
            Use::Cited
        } else {
            Use::Ignored
        };
    }
}

/// Whether `text` holds the session id as a value, not inside a path (the
/// client puts the id in its own paths: transcripts, tool results,
/// scratch folders).
fn names_id(text: &str, id: &str) -> bool {
    text.match_indices(id).any(|(i, _)| {
        let before = text[..i].chars().next_back();
        let after = text[i + id.len()..].chars().next();
        !matches!(before, Some('/' | '-' | '_')) && !matches!(after, Some('/' | '.' | '-' | '_'))
    })
}

/// Bytes by use.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Totals {
    pub read: usize,
    pub cited: usize,
    pub ignored: usize,
    pub total: usize,
}

impl Totals {
    pub fn of(parts: &[Part]) -> Totals {
        let mut t = Totals::default();
        for p in parts {
            match p.used {
                Use::Read => t.read += p.bytes,
                Use::Cited => t.cited += p.bytes,
                Use::Ignored => t.ignored += p.bytes,
            }
            t.total += p.bytes;
        }
        t
    }
}

// ---------------------------------------------------------------------------
// One session

/// A session's x-ray.
#[derive(Debug, Clone, Serialize)]
pub struct Xray {
    pub session: String,
    pub tool: String,
    pub block: Source,
    pub parts: Vec<Part>,
    pub totals: Totals,
    pub searches: Vec<Search>,
    pub claims: Vec<Claim>,
}

/// What [`analyse`] reads for one session.
pub struct Inputs<'a> {
    pub id: &'a str,
    pub tool: &'a str,
    pub transcript: &'a Path,
    pub turns: &'a [StoredTurn],
    /// Its sub-agents' turns, each with the sub-agent's id.
    pub subs: &'a [(String, Vec<StoredTurn>)],
    pub record: Option<&'a Session>,
    /// The workspace and its notes, when there is one.
    pub notes: Option<(&'a Workspace, &'a Notes, &'a Resolver<'a>)>,
}

/// The x-ray of one session from what it was given.
pub fn analyse(inp: &Inputs<'_>) -> Xray {
    let dir = inp
        .notes
        .and_then(|(_, _, r)| r.session_dir(inp.tool, inp.transcript))
        .or_else(|| crate::session_meta::origin(inp.id, inp.tool, inp.transcript).cwd);
    let block = block(inp.record, inp.tool, inp.transcript);
    let (parts, searches) = match inp.notes {
        Some((ws, notes, resolver)) => {
            let mut parts = parts(&block, ws, notes);
            classify(&mut parts, inp.turns, resolver, dir.as_deref(), inp.id);
            let searches = searches::find(inp.turns, &resolver.notes_roots(), dir.as_deref());
            (parts, searches)
        }
        None => (Vec::new(), Vec::new()),
    };
    let notes_roots = inp
        .notes
        .map(|(_, _, r)| r.notes_roots())
        .unwrap_or_default();
    let claims = claims::check(&claims::Inputs {
        turns: inp.turns,
        subs: inp.subs,
        dir: dir.as_deref(),
        notes_roots: &notes_roots,
    });
    Xray {
        session: inp.id.to_owned(),
        tool: inp.tool.to_owned(),
        block: block.source,
        totals: Totals::of(&parts),
        parts,
        searches,
        claims,
    }
}

/// The x-ray of session `id` from the index (and the workspace's notes,
/// with `ws`).
pub fn of(ws: Option<&Workspace>, index: &Index, id: &str) -> Result<Xray> {
    let (tool, transcript) = index
        .source(id)?
        .ok_or_else(|| Error::InvalidSessionId(format!("{id} is not in the index")))?;
    let turns = index.turns_for_session(id)?;
    let subs: Vec<(String, Vec<StoredTurn>)> = index
        .sub_agents(id)?
        .into_iter()
        .map(|s| {
            let t = index.turns_for_session(&s)?;
            Ok((s, t))
        })
        .collect::<Result<_>>()?;
    let record = match ws {
        Some(ws) if sessions::is_valid_id(id) => sessions::get(ws, id).ok().flatten(),
        _ => None,
    };
    let scanned = match ws {
        Some(ws) => Some((ws, Notes::scan(&ws.notes_root())?)),
        None => None,
    };
    let log = ws.map(|ws| normalise(&ws.log_path));
    let resolver = scanned
        .as_ref()
        .map(|(ws, notes)| Resolver::new(&ws.root, &ws.notes_dir, notes, log.as_deref()));
    let notes = match (&scanned, &resolver) {
        (Some((ws, notes)), Some(r)) => Some((*ws, notes, r)),
        _ => None,
    };
    Ok(analyse(&Inputs {
        id,
        tool: &tool,
        transcript: &transcript,
        turns: &turns,
        subs: &subs,
        record: record.as_ref(),
        notes,
    }))
}

// ---------------------------------------------------------------------------
// The fleet

/// One note's use across the fleet's sessions.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct NoteUse {
    pub note: PathBuf,
    pub label: String,
    pub kind: Option<PartKind>,
    pub sessions: u32,
    pub read: u32,
    pub cited: u32,
    pub ignored: u32,
    /// The bytes its part takes in a block (the most seen).
    pub bytes: usize,
}

/// A tune tip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Tip {
    pub note: PathBuf,
    pub label: String,
    pub kind: PartKind,
    pub ignored: u32,
    pub sessions: u32,
    /// The bytes it would free from every start block.
    pub frees: usize,
    /// `unrequire` when `hippocamp xray unrequire` would act on it; `None`
    /// for information only.
    pub action: Option<String>,
    pub text: String,
}

/// The claims line.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ClaimStats {
    pub claims: u32,
    pub verified: u32,
    pub unverified: u32,
    pub unclear: u32,
    /// The kind most often unverified, and how often.
    pub usual: Option<(String, u32)>,
}

impl ClaimStats {
    /// `61 claims, 7 unverified · "deployed" is the usual one (5)`.
    pub fn line(&self) -> String {
        let mut s = format!(
            "{} claim{}, {} unverified",
            self.claims,
            if self.claims == 1 { "" } else { "s" },
            self.unverified
        );
        if self.unclear > 0 {
            s.push_str(&format!(", {} unclear", self.unclear));
        }
        if let Some((kind, n)) = &self.usual {
            if *n > 1 {
                s.push_str(&format!(" · \"{kind}\" is the usual one ({n})"));
            }
        }
        s
    }
}

/// The fleet's use of its context over a window.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Fleet {
    pub days: u32,
    /// Top-level sessions in the window.
    pub sessions: u32,
    /// Of them, with a recorded block.
    pub recorded: u32,
    /// Every recorded block's bytes, summed.
    pub block_bytes: usize,
    /// Of those, the bytes of parts nobody opened or cited.
    pub ignored_bytes: usize,
    pub notes: Vec<NoteUse>,
    pub tips: Vec<Tip>,
    pub claims: ClaimStats,
}

/// The fleet stats over the `days` before `now`, from the rollup.
pub fn fleet(
    ws: Option<&Workspace>,
    rollup: &Rollup,
    days: u32,
    now: OffsetDateTime,
) -> Result<Fleet> {
    let start = now - Duration::days(i64::from(days));
    let sessions = rollup.top_level_in(start, now + Duration::minutes(1))?;
    let mut out = Fleet {
        days,
        sessions: sessions.len() as u32,
        ..Fleet::default()
    };
    let mut uses: HashMap<PathBuf, NoteUse> = HashMap::new();
    let mut unverified_kinds: BTreeMap<String, u32> = BTreeMap::new();
    for id in &sessions {
        let parts = rollup.context_of(id)?;
        if !parts.is_empty() {
            out.recorded += 1;
        }
        let t = Totals::of(&parts);
        out.block_bytes += t.total;
        out.ignored_bytes += t.ignored;
        // A note is counted once per session, however many of its blocks
        // (the start block, then prompt blocks) carried it: its strongest
        // use and kind.
        let mut here: BTreeMap<PathBuf, (&Part, Use, PartKind, usize)> = BTreeMap::new();
        for p in &parts {
            let Some(note) = p.note.clone() else { continue };
            if p.kind == PartKind::Housekeeping {
                continue;
            }
            let rank = |u: Use| match u {
                Use::Read => 2,
                Use::Cited => 1,
                Use::Ignored => 0,
            };
            here.entry(note)
                .and_modify(|(_, used, kind, bytes)| {
                    if rank(p.used) > rank(*used) {
                        *used = p.used;
                    }
                    *kind = (*kind).min(p.kind);
                    *bytes = (*bytes).max(p.bytes);
                })
                .or_insert((p, p.used, p.kind, p.bytes));
        }
        for (note, (p, used, kind, bytes)) in here {
            let u = uses.entry(note.clone()).or_insert_with(|| NoteUse {
                note,
                label: p.label.clone(),
                ..NoteUse::default()
            });
            // Required wins over relevant when a note was both.
            u.kind = Some(u.kind.map_or(kind, |k| k.min(kind)));
            u.sessions += 1;
            match used {
                Use::Read => u.read += 1,
                Use::Cited => u.cited += 1,
                Use::Ignored => u.ignored += 1,
            }
            u.bytes = u.bytes.max(bytes);
        }
        for c in rollup.claims_of(id)? {
            out.claims.claims += 1;
            match c.status {
                Status::Verified => out.claims.verified += 1,
                Status::Unclear => out.claims.unclear += 1,
                Status::Unverified => {
                    out.claims.unverified += 1;
                    *unverified_kinds
                        .entry(c.kind.label().to_owned())
                        .or_default() += 1;
                }
            }
        }
    }
    out.claims.usual = unverified_kinds
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)));
    let mut notes: Vec<NoteUse> = uses.into_values().collect();
    notes.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            .then(b.sessions.cmp(&a.sessions))
            .then(a.note.cmp(&b.note))
    });
    let entry = ws.map(|w| normalise(&w.entry_note));
    let required: Vec<PathBuf> = ws
        .map(|w| w.required_notes.iter().map(|p| normalise(p)).collect())
        .unwrap_or_default();
    for n in &notes {
        if n.sessions < 5 {
            continue;
        }
        let most = n.ignored * 3 >= n.sessions * 2;
        match n.kind {
            Some(PartKind::Required) if most && Some(&n.note) != entry.as_ref() => {
                let configured = required.contains(&n.note);
                out.tips.push(Tip {
                    note: n.note.clone(),
                    label: n.label.clone(),
                    kind: PartKind::Required,
                    ignored: n.ignored,
                    sessions: n.sessions,
                    frees: n.bytes,
                    action: configured.then(|| "unrequire".to_owned()),
                    text: tune_text(&n.label, n.ignored, n.sessions, n.bytes),
                });
            }
            Some(PartKind::Relevant) if n.ignored == n.sessions => {
                out.tips.push(Tip {
                    note: n.note.clone(),
                    label: n.label.clone(),
                    kind: PartKind::Relevant,
                    ignored: n.ignored,
                    sessions: n.sessions,
                    frees: n.bytes,
                    action: None,
                    text: format!(
                        "{} was offered as relevant in {} sessions and never opened or cited; its first line may not say what it holds.",
                        n.label, n.sessions
                    ),
                });
            }
            _ => {}
        }
    }
    // Actions first, then the note ignored in the largest share of sessions.
    out.tips.sort_by(|a, b| {
        b.action
            .is_some()
            .cmp(&a.action.is_some())
            .then(
                (u64::from(b.ignored) * u64::from(a.sessions))
                    .cmp(&(u64::from(a.ignored) * u64::from(b.sessions))),
            )
            .then(a.note.cmp(&b.note))
    });
    out.notes = notes;
    Ok(out)
}

impl Fleet {
    /// The average recorded block's bytes; `None` without one.
    pub fn avg_block(&self) -> Option<usize> {
        (self.recorded > 0).then(|| self.block_bytes / self.recorded as usize)
    }

    /// The share of the recorded blocks' bytes nobody opened or cited, in
    /// percent; `None` without one.
    pub fn ignored_percent(&self) -> Option<u32> {
        (self.block_bytes > 0)
            .then(|| (self.ignored_bytes as f64 * 100.0 / self.block_bytes as f64).round() as u32)
    }
}

/// `38 B`, `1.3 KB`.
pub fn human_bytes(n: usize) -> String {
    if n < 1000 {
        format!("{n} B")
    } else {
        format!("{:.1} KB", n as f64 / 1024.0)
    }
}

/// The tune tip's words: the signal first; the
/// bytes only when they are at least 1 KB.
pub fn tune_text(label: &str, ignored: u32, sessions: u32, bytes: usize) -> String {
    let freed = if bytes >= 1024 {
        format!(", {} of every block", human_bytes(bytes))
    } else {
        String::new()
    };
    format!(
        "{label} was never opened or cited in {ignored} of {sessions} sessions this week. Making it relevant-only keeps it out of every start block{freed}; it is still offered when a task matches it."
    )
}

// ---------------------------------------------------------------------------
// `hippocamp xray unrequire`

/// The `required_notes` change that drops `note`: the line before and
/// after, and the whole new file text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unrequire {
    pub before: String,
    pub after: String,
    pub text: String,
}

/// Plans dropping `note` from `required_notes` in `hippocamp_toml` (its text):
/// the one `required_notes = [...]` line is rewritten, nothing else.
pub fn unrequire(hippocamp_toml: &str, note: &Path) -> std::result::Result<Unrequire, String> {
    let note = normalise(note);
    let mut found = None;
    for (i, line) in hippocamp_toml.lines().enumerate() {
        let t = line.trim_start();
        if t.starts_with('#') {
            continue;
        }
        if let Some(rest) = t.strip_prefix("required_notes") {
            if rest.trim_start().starts_with('=') {
                found = Some((i, line.to_owned()));
                break;
            }
        }
    }
    let (i, line) = found.ok_or("hippocamp.toml has no required_notes line")?;
    let value = line.split_once('=').map(|(_, v)| v.trim()).unwrap_or("");
    if !(value.starts_with('[') && value.ends_with(']')) {
        return Err(
            "required_notes is not a one-line array; edit hippocamp.toml by hand".to_owned(),
        );
    }
    let parsed: toml::Value = toml::from_str(&format!("v = {value}"))
        .map_err(|e| format!("required_notes does not parse: {e}"))?;
    let items: Vec<String> = parsed
        .get("v")
        .and_then(|v| v.as_array())
        .ok_or("required_notes is not an array")?
        .iter()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect();
    let kept: Vec<&String> = items
        .iter()
        .filter(|p| normalise(Path::new(p)) != note)
        .collect();
    if kept.len() == items.len() {
        return Err(format!("{} is not in required_notes", note.display()));
    }
    let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
    let after = format!(
        "{indent}required_notes = [{}]",
        kept.iter()
            .map(|p| serde_json::to_string(p).expect("a string serialises"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let mut lines: Vec<&str> = hippocamp_toml.lines().collect();
    lines[i] = &after;
    let mut text = lines.join("\n");
    if hippocamp_toml.ends_with('\n') {
        text.push('\n');
    }
    Ok(Unrequire {
        before: line,
        after: after.clone(),
        text,
    })
}

/// `hippocamp xray unrequire <note> --yes`: writes the change [`unrequire`]
/// plans to `.hippocamp/hippocamp.toml`, atomically.
pub fn unrequire_write(ws: &Workspace, note: &Path) -> Result<Unrequire> {
    let path = ws.hippocamp_dir().join("hippocamp.toml");
    let text = std::fs::read_to_string(&path)?;
    let change = unrequire(&text, note).map_err(Error::InvalidProposal)?;
    crate::util::write_atomic(&path, change.text.as_bytes())?;
    Ok(change)
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests;
