//! Dreaming step 2: what a pass reads, gathered for one day chunk.
//!
//! - `sessions`: the session records with a turn in the chunk, each with
//!   the ordinal of its last turn; a line shows at most
//!   [`SESSION_TASK_BYTES`] of the task and [`SESSION_HANDOFF_BYTES`] of the
//!   handoff.
//! - `notes`: the notes those sessions touched (`notes_touched`) or cited
//!   (`[[...]]` in the handoff). A `per_note` pass runs once per note and
//!   then reads only that note and the sessions that touched or cited it.
//! - `turns`: for a `per_note` pass, the matches of a full-text query made
//!   of the note's title, each heading and each `sources` path as quoted
//!   phrases joined by `OR`, best first; otherwise every turn of the
//!   sessions. Either way only as many as fit the pass's budget.
//! - `human_confirmed`: the `old` text of each revert receipt's proposal
//!   that its note still contains.
//!
//! Each input renders as one JSON object per line. A note's bytes are read
//! when its pass gathers them, and are what `before_hash` hashes.
//!
//! Besides the notes session records declare, a chunk holds the notes its
//! sessions' transcripts show them editing, discussing or reading
//! ([`observed`](crate::dream::observed)). A pass that reads
//! `turns` takes both; a pass that does not has no evidence for an observed
//! note beyond the records, and takes the declared ones only. A note over
//! budget is read as an excerpt of whole sections
//! ([`sections`](crate::dream::sections)).

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};

use serde_json::json;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::config::Workspace;
use crate::dream::observed::{self, Kind, Resolver, Seen};
use crate::dream::sections::Excerpt;
use crate::dream::window::Chunk;
use crate::error::{Error, Result};
use crate::index::{Index, SessionTurn};
use crate::notes::{links, normalise, Link, Note, Notes};
use crate::records::{list_receipts, Input, Proposal, Session, Verdict};
use crate::sessions;

/// A note as it goes into a prompt.
#[derive(Debug, Clone)]
pub struct NoteInput {
    /// Relative to the workspace, as a proposal names it.
    pub path: PathBuf,
    /// The note's bytes when it was read for the prompt.
    pub bytes: Vec<u8>,
    pub note: Note,
    /// When the prompt shows only part of the note.
    pub excerpt: Option<Excerpt>,
}

impl NoteInput {
    /// Reads the note at `path`, relative to the workspace.
    pub fn read(ws: &Workspace, path: &Path) -> Result<NoteInput> {
        let note = Note::read(&ws.root.join(path))?;
        let bytes = [&note.raw_frontmatter[..], &note.body[..]].concat();
        Ok(NoteInput {
            path: path.to_path_buf(),
            bytes,
            note,
            excerpt: None,
        })
    }

    /// The note's text, invalid UTF-8 replaced.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.bytes).into_owned()
    }

    /// The text the prompt shows: the excerpt, or the whole note.
    pub fn shown(&self) -> String {
        match &self.excerpt {
            Some(excerpt) => {
                String::from_utf8_lossy(&self.bytes[excerpt.bytes.clone()]).into_owned()
            }
            None => self.text(),
        }
    }

    /// This note shown only at `range` of its bytes.
    pub fn excerpted(&self, range: Range<usize>) -> NoteInput {
        NoteInput {
            excerpt: Some(Excerpt::of(&self.bytes, range)),
            ..self.clone()
        }
    }

    /// The reviewer's view: the shown text, led by a line naming the lines
    /// shown when it is an excerpt.
    pub fn for_reviewer(&self) -> String {
        match &self.excerpt {
            Some(excerpt) => format!(
                "(An excerpt: {}. The rest of the note is not shown.)\n\n{}",
                excerpt.label(),
                self.shown()
            ),
            None => self.text(),
        }
    }
}

/// A session record with the ordinal of its last indexed turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInput {
    pub record: Session,
    pub last_ordinal: u32,
}

/// The sessions with a turn in one chunk and the notes they touched or
/// cited.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChunkInputs {
    /// By id.
    pub sessions: Vec<SessionInput>,
    /// Each note, relative to the workspace, with the ids of the sessions
    /// that touched or cited it.
    pub notes: BTreeMap<PathBuf, BTreeSet<String>>,
    /// What ranks each of `notes`: a record's `notes_touched` counts as an
    /// edit, a handoff's link as a discussion.
    pub seen: BTreeMap<PathBuf, Seen>,
    /// The notes the sessions' transcripts name, declared or not.
    pub observed: BTreeMap<PathBuf, Seen>,
}

impl ChunkInputs {
    /// These inputs with the observed notes among `notes`, their sessions
    /// and their evidence added.
    pub fn with_observed(&self) -> ChunkInputs {
        let mut merged = self.clone();
        for (note, seen) in &self.observed {
            merged
                .notes
                .entry(note.clone())
                .or_default()
                .extend(seen.sessions.iter().cloned());
            merged.seen.entry(note.clone()).or_default().merge(seen);
        }
        merged.observed.clear();
        merged
    }

    /// The notes, best first ([`observed::rank`]), with the log notes
    /// (`log_notes`, relative to the workspace) after
    /// every other note: a write-once record of a session has nothing to
    /// correct, so it takes a slot only when one is left over.
    pub fn ranked(&self, log_notes: &BTreeSet<PathBuf>) -> Vec<PathBuf> {
        let seen: BTreeMap<PathBuf, Seen> = self
            .notes
            .keys()
            .map(|note| {
                (
                    note.clone(),
                    self.seen.get(note).cloned().unwrap_or_default(),
                )
            })
            .collect();
        let (logs, rest): (Vec<PathBuf>, Vec<PathBuf>) = observed::rank(&seen)
            .into_iter()
            .partition(|note| log_notes.contains(note));
        rest.into_iter().chain(logs).collect()
    }

    /// `declared <n> notes; observed <n> notes (<e> edited, <d> discussed,
    /// <r> read) in <s> sessions`.
    pub fn summary(&self) -> String {
        let count = |kind: Kind| {
            self.observed
                .values()
                .filter(|seen| seen.kind == Some(kind))
                .count()
        };
        let sessions: BTreeSet<&String> = self
            .observed
            .values()
            .flat_map(|seen| seen.sessions.iter())
            .collect();
        format!(
            "declared {} notes; observed {} notes ({} edited, {} discussed, {} read) in {} sessions",
            self.notes.len(),
            self.observed.len(),
            count(Kind::Edit),
            count(Kind::Discuss),
            count(Kind::Read),
            sessions.len()
        )
    }
}

/// The sessions with a turn in `chunk` that have a record, and their notes.
/// Only notes that exist under `notes_dir` count.
pub fn for_chunk(
    ws: &Workspace,
    index: &Index,
    last_ordinals: &HashMap<String, u32>,
    chunk: Chunk,
) -> Result<ChunkInputs> {
    let scanned = Notes::scan(&ws.notes_root())?;
    let notes_dir = normalise(&ws.notes_dir);
    let log = normalise(&ws.log_path);
    let resolver = Resolver::new(&ws.root, &ws.notes_dir, &scanned, Some(&log));
    let mut inputs = ChunkInputs::default();
    for id in index.sessions_between(chunk.start, chunk.end)? {
        if !sessions::is_valid_id(&id) {
            continue;
        }
        let Some(record) = sessions::get(ws, &id)? else {
            continue;
        };
        for (note, kind) in session_notes(&notes_dir, &scanned, &record) {
            if note == log {
                continue;
            }
            inputs
                .notes
                .entry(note.clone())
                .or_default()
                .insert(id.clone());
            inputs.seen.entry(note).or_default().add(&id, kind, &[]);
        }
        for seen in observed_in(index, &resolver, &id)? {
            inputs
                .observed
                .entry(seen.note)
                .or_default()
                .add(&id, seen.kind, &seen.written);
        }
        let last_ordinal = last_ordinals.get(&id).copied().unwrap_or_default();
        inputs.sessions.push(SessionInput {
            record,
            last_ordinal,
        });
    }
    Ok(inputs)
}

/// The notes session `id`'s indexed turns name ([`observed::observe`]).
pub fn observed_in(
    index: &Index,
    resolver: &Resolver<'_>,
    id: &str,
) -> Result<Vec<observed::Observation>> {
    let dir = index
        .source(id)?
        .and_then(|(tool, transcript)| resolver.session_dir(&tool, &transcript));
    let mut found = Vec::new();
    for turn in index.note_mentions(id)? {
        found.extend(observed::observe(resolver, dir.as_deref(), &turn));
    }
    Ok(found)
}

/// The notes each of the sessions `ids` edited, as their transcripts show.
/// Empty for a session the index does not hold.
pub fn edited_by(
    ws: &Workspace,
    index: &Index,
    ids: &[&str],
) -> Result<BTreeMap<String, BTreeSet<PathBuf>>> {
    let scanned = Notes::scan(&ws.notes_root())?;
    let log = normalise(&ws.log_path);
    let resolver = Resolver::new(&ws.root, &ws.notes_dir, &scanned, Some(&log));
    let mut out = BTreeMap::new();
    for id in ids {
        let edited: BTreeSet<PathBuf> = observed_in(index, &resolver, id)?
            .into_iter()
            .filter(|seen| seen.kind == Kind::Edit)
            .map(|seen| seen.note)
            .collect();
        out.insert((*id).to_owned(), edited);
    }
    Ok(out)
}

/// What a session's transcript shows of how it left its work.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Trace {
    /// It ran a session-end command, through another tool or hippo
    /// ([`observed::ends_session`]).
    pub ended: bool,
    /// It edited a note.
    pub edited: bool,
}

impl Trace {
    /// Whether it left anything at all.
    pub fn any(self) -> bool {
        self.ended || self.edited
    }
}

/// The trace of each of the sessions `ids`: a session-end
/// command run in a shell call, or a note edit. Empty for a session the
/// index does not hold.
pub fn traces(ws: &Workspace, index: &Index, ids: &[&str]) -> Result<BTreeMap<String, Trace>> {
    let edited = edited_by(ws, index, ids)?;
    let mut out = BTreeMap::new();
    for id in ids {
        let ended = index
            .calls_holding(id, "session")?
            .iter()
            .any(observed::ends_session);
        let edited = edited.get(*id).is_some_and(|notes| !notes.is_empty());
        out.insert((*id).to_owned(), Trace { ended, edited });
    }
    Ok(out)
}

/// What one model call of a pass reads.
#[derive(Debug, Clone, Default)]
pub struct Gathered {
    pub sessions: Vec<SessionInput>,
    pub notes: Vec<NoteInput>,
    /// Best-ranked first, so the budget drops from the end.
    pub turns: Vec<SessionTurn>,
    /// `(note, text)` pairs.
    pub human_confirmed: Vec<(PathBuf, String)>,
}

impl Gathered {
    /// The rendered text of each input in `reads` but `turns`, and the
    /// rendered turns, one line each.
    pub fn rendered(&self, reads: &[Input]) -> (Vec<(Input, String)>, Vec<String>) {
        let mut blocks = Vec::new();
        for input in reads {
            let lines: Vec<String> = match input {
                Input::Sessions => self.sessions.iter().map(session_line).collect(),
                Input::Notes => self.notes.iter().map(note_line).collect(),
                Input::HumanConfirmed => self
                    .human_confirmed
                    .iter()
                    .map(|(note, text)| confirmed_line(note, text))
                    .collect(),
                Input::Turns => continue,
            };
            blocks.push((*input, lines.join("\n")));
        }
        let turns = if reads.contains(&Input::Turns) {
            self.turns.iter().map(turn_line).collect()
        } else {
            Vec::new()
        };
        (blocks, turns)
    }

    /// `<n> sessions, <n> notes, <n> turns, <n> human-confirmed`.
    pub fn counts(&self) -> String {
        format!(
            "{} sessions, {} notes, {} turns, {} human-confirmed",
            self.sessions.len(),
            self.notes.len(),
            self.turns.len(),
            self.human_confirmed.len()
        )
    }
}

/// The inputs of a `per_note` pass for `note`: that note, the sessions that
/// touched or cited it, the turns its query finds and its human-confirmed
/// text, as far as `reads` asks. `Ok(Err(why))` when the note is skipped:
/// it vanished, or the pass reads turns and the note gives nothing to
/// search for.
pub fn for_note(
    ws: &Workspace,
    index: &Index,
    chunk: &ChunkInputs,
    note: &Path,
    reads: &[Input],
    budget: usize,
    reverted: &[Proposal],
) -> Result<std::result::Result<Gathered, String>> {
    let Some(note) = read_present(ws, note)? else {
        return Ok(Err("the note no longer exists".to_owned()));
    };
    for_read_note(index, chunk, note, reads, budget, reverted)
}

/// [`for_note`] for a note already read, which may be an excerpt: then the
/// turns' query takes the headings of the excerpt only.
pub fn for_read_note(
    index: &Index,
    chunk: &ChunkInputs,
    note: NoteInput,
    reads: &[Input],
    budget: usize,
    reverted: &[Proposal],
) -> Result<std::result::Result<Gathered, String>> {
    let cited_by = chunk.notes.get(&note.path).cloned().unwrap_or_default();
    let mut gathered = Gathered {
        sessions: chunk
            .sessions
            .iter()
            .filter(|session| cited_by.contains(&session.record.id))
            .cloned()
            .collect(),
        ..Gathered::default()
    };
    if reads.contains(&Input::Turns) {
        let query = match &note.excerpt {
            Some(_) => excerpt_query(&note.note, &note.shown()),
            None => note_query(&note.note),
        };
        if query.is_empty() {
            return Ok(Err("no title, heading or source to search for".to_owned()));
        }
        gathered.turns = index.search_turns(&query, within(budget))?;
    }
    if reads.contains(&Input::HumanConfirmed) {
        gathered.human_confirmed = human_confirmed(reverted, std::slice::from_ref(&note));
    }
    gathered.notes.push(note);
    Ok(Ok(gathered))
}

/// The inputs of a pass that is not `per_note`: every session and note of
/// the chunk, every turn of those sessions that fits the budget, and the
/// notes' human-confirmed text, as far as `reads` asks.
pub fn for_all(
    ws: &Workspace,
    index: &Index,
    chunk: &ChunkInputs,
    reads: &[Input],
    budget: usize,
    reverted: &[Proposal],
) -> Result<Gathered> {
    let mut gathered = Gathered {
        sessions: chunk.sessions.clone(),
        ..Gathered::default()
    };
    for path in chunk.notes.keys() {
        if let Some(note) = read_present(ws, path)? {
            gathered.notes.push(note);
        }
    }
    if reads.contains(&Input::Turns) {
        let mut keep = within(budget);
        'sessions: for session in &chunk.sessions {
            for stored in index.turns_for_session(&session.record.id)? {
                let turn = SessionTurn {
                    session: session.record.id.clone(),
                    ordinal: stored.ordinal,
                    role: stored.role,
                    ts: stored.ts,
                    tool_name: stored.tool_name,
                    text: stored.text,
                };
                if !keep(&turn) {
                    break 'sessions;
                }
                gathered.turns.push(turn);
            }
        }
    }
    if reads.contains(&Input::HumanConfirmed) {
        gathered.human_confirmed = human_confirmed(reverted, &gathered.notes);
    }
    Ok(gathered)
}

/// The full-text query for a note: its title, each heading and each
/// `sources` path as a quoted phrase (inner `"` doubled), joined by ` OR `.
/// Text without a letter or digit is left out, and so are repeats. Empty
/// when nothing is left.
pub fn note_query(note: &Note) -> String {
    query_of(note, note.headings())
}

/// [`note_query`] with the headings of `shown`, an excerpt of the note, in
/// place of all the note's headings.
pub fn excerpt_query(note: &Note, shown: &str) -> String {
    let headings = shown
        .lines()
        .filter_map(|line| {
            let hashes = line.bytes().take_while(|&b| b == b'#').count();
            let text = line[hashes..].strip_prefix(' ')?;
            (1..=6).contains(&hashes).then(|| text.trim().to_owned())
        })
        .collect();
    query_of(note, headings)
}

fn query_of(note: &Note, headings: Vec<String>) -> String {
    let sources = note.frontmatter.sources.clone().unwrap_or_default();
    let mut phrases: Vec<String> = Vec::new();
    for text in std::iter::once(note.title()).chain(headings).chain(sources) {
        let text = text.trim();
        if !text.chars().any(char::is_alphanumeric) {
            continue;
        }
        let phrase = format!("\"{}\"", text.replace('"', "\"\""));
        if !phrases.contains(&phrase) {
            phrases.push(phrase);
        }
    }
    phrases.join(" OR ")
}

/// The proposal of every revert receipt, in receipt order.
pub fn reverted_proposals(ws: &Workspace) -> Result<Vec<Proposal>> {
    let hippo = ws.hippo_dir();
    let mut proposals = Vec::new();
    for listed in list_receipts(ws)? {
        let receipt = listed.require()?;
        if receipt.verdict == Verdict::Revert {
            let file = hippo
                .join("proposals")
                .join(format!("{}.json", receipt.proposal));
            proposals.push(Proposal::load(&file)?);
        }
    }
    Ok(proposals)
}

/// For each of `notes`, the `old` text of the reverted proposals on it that
/// the note still contains, without repeats.
pub fn human_confirmed(reverted: &[Proposal], notes: &[NoteInput]) -> Vec<(PathBuf, String)> {
    let mut found: Vec<(PathBuf, String)> = Vec::new();
    for note in notes {
        for proposal in reverted {
            let present = proposal.note == note.path
                && !proposal.old.is_empty()
                && contains(&note.bytes, proposal.old.as_bytes());
            let pair = (note.path.clone(), proposal.old.clone());
            if present && !found.contains(&pair) {
                found.push(pair);
            }
        }
    }
    found
}

/// The most bytes of a session's `task` a `sessions` line shows: a record's task can hold a whole pasted brief.
pub const SESSION_TASK_BYTES: usize = 600;

/// The most bytes of a session's `handoff` a `sessions` line shows.
pub const SESSION_HANDOFF_BYTES: usize = 2_000;

/// `text` when it is at most `max` bytes, else its first `max` bytes cut
/// back to a character boundary and followed by `…`.
pub fn bounded(text: &str, max: usize) -> Cow<'_, str> {
    if text.len() <= max {
        return Cow::Borrowed(text);
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    Cow::Owned(format!("{}…", &text[..end]))
}

/// One `sessions` line, its `task` and `handoff` bounded.
pub fn session_line(session: &SessionInput) -> String {
    let record = &session.record;
    json!({
        "id": record.id,
        "tool": record.tool.as_str(),
        "task": bounded(&record.task, SESSION_TASK_BYTES),
        "started": rfc3339(record.started),
        "ended": record.ended.map(rfc3339),
        "last_ordinal": session.last_ordinal,
        "notes_touched": record
            .notes_touched
            .iter()
            .map(|path| path.to_string_lossy())
            .collect::<Vec<_>>(),
        "handoff": record
            .handoff
            .as_deref()
            .map(|handoff| bounded(handoff, SESSION_HANDOFF_BYTES)),
        "derived": record.derived,
    })
    .to_string()
}

/// One `notes` line: the note's path and its whole text, or for an excerpt
/// its path, the lines shown and their text.
pub fn note_line(note: &NoteInput) -> String {
    match &note.excerpt {
        Some(excerpt) => json!({
            "note": note.path.to_string_lossy(),
            "lines": excerpt.label(),
            "text": note.shown(),
        })
        .to_string(),
        None => json!({"note": note.path.to_string_lossy(), "text": note.text()}).to_string(),
    }
}

/// One `turns` line, and one evidence line for the reviewer.
pub fn turn_line(turn: &SessionTurn) -> String {
    json!({
        "session": turn.session,
        "ordinal": turn.ordinal,
        "role": turn.role,
        "ts": turn.ts,
        "tool_name": turn.tool_name,
        "text": turn.text,
    })
    .to_string()
}

/// One `human_confirmed` line.
pub fn confirmed_line(note: &Path, text: &str) -> String {
    json!({"note": note.to_string_lossy(), "text": text}).to_string()
}

/// A `keep` for [`Index::search_turns`] that accepts turns while their
/// rendered lines fit in `budget` bytes.
fn within(budget: usize) -> impl FnMut(&SessionTurn) -> bool {
    let mut used = 0usize;
    move |turn| {
        used = used.saturating_add(turn_line(turn).len() + 1);
        used <= budget
    }
}

/// The notes `record` touched (an edit) or cited (a discussion) that the
/// scan found, relative to the workspace, each once with its strongest
/// kind.
fn session_notes(notes_dir: &Path, scanned: &Notes, record: &Session) -> BTreeMap<PathBuf, Kind> {
    let mut found = BTreeMap::new();
    if let Some(handoff) = &record.handoff {
        for link in links(handoff) {
            if let Link::Resolved(below) = scanned.resolve_link(link) {
                found.insert(notes_dir.join(below), Kind::Discuss);
            }
        }
    }
    for touched in &record.notes_touched {
        if let Ok(below) = normalise(touched).strip_prefix(notes_dir) {
            if scanned.notes.contains_key(below) {
                found.insert(notes_dir.join(below), Kind::Edit);
            }
        }
    }
    found
}

/// The note at `path`, or `None` when it no longer exists.
fn read_present(ws: &Workspace, path: &Path) -> Result<Option<NoteInput>> {
    match NoteInput::read(ws, path) {
        Ok(note) => Ok(Some(note)),
        Err(Error::Io(err)) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

fn rfc3339(at: OffsetDateTime) -> String {
    at.format(&Rfc3339).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dream::tests::{at, Setup, NOTE, NOTE_PATH, TURN_1};
    use crate::records::{Evidence, ProposalKind};
    use ulid::Ulid;

    fn note_with(dir: &Path, text: &str) -> Note {
        let path = dir.join("Webhooks.md");
        std::fs::write(&path, text).unwrap();
        Note::read(&path).unwrap()
    }

    #[test]
    fn the_query_quotes_title_headings_and_sources_once_each() {
        let dir = tempfile::tempdir().unwrap();
        let note = note_with(
            dir.path(),
            "---\nsources: [\"brain/Sources/Say \\\"hi\\\".md\"]\n---\n# Webhooks\n\n## Retry\n\n## Retry\n\n## ---\n",
        );
        assert_eq!(
            note_query(&note),
            "\"Webhooks\" OR \"Retry\" OR \"brain/Sources/Say \"\"hi\"\".md\""
        );
        let untitled = note_with(dir.path(), "no headings at all\n");
        assert_eq!(note_query(&untitled), "\"Webhooks\"");
    }

    #[test]
    fn a_session_line_cuts_task_and_handoff_on_a_character_boundary() {
        // `a` and then three-byte characters: byte 600 falls inside one.
        let task = format!("a{}", "€".repeat(300));
        let handoff = "ü".repeat(1_500);
        let short = "Done; see [[Topic]].";
        let line = |handoff: &str| {
            let session = SessionInput {
                record: Session {
                    id: "s9".to_owned(),
                    tool: crate::records::Tool::ClaudeCode,
                    task: task.clone(),
                    task_seen: true,
                    context_bytes_at_start: 0,
                    started: at("2026-11-08T01:00:00Z"),
                    ended: Some(at("2026-11-08T02:00:00Z")),
                    notes_touched: vec![PathBuf::from(NOTE_PATH)],
                    handoff: Some(handoff.to_owned()),
                    handoff_derived: false,
                    no_change_reason: None,
                    context_start: None,
                    context_prompt: None,
                    derived: true,
                },
                last_ordinal: 7,
            };
            serde_json::from_str::<serde_json::Value>(&session_line(&session)).unwrap()
        };
        let cut = line(&handoff);
        let shown = cut["task"].as_str().unwrap();
        assert_eq!(shown, format!("a{}…", "€".repeat(199)));
        assert_eq!(shown.len(), 598 + "…".len());
        let shown = cut["handoff"].as_str().unwrap();
        assert_eq!(shown, format!("{}…", "ü".repeat(1_000)));
        assert_eq!(
            (
                cut["id"].as_str(),
                cut["tool"].as_str(),
                cut["last_ordinal"].as_u64(),
                cut["derived"].as_bool(),
                cut["notes_touched"][0].as_str(),
                cut["ended"].as_str(),
            ),
            (
                Some("s9"),
                Some("claude-code"),
                Some(7),
                Some(true),
                Some(NOTE_PATH),
                Some("2026-11-08T02:00:00Z")
            )
        );
        // Text within the bound is shown as it is, with no mark.
        assert_eq!(line(short)["handoff"].as_str(), Some(short));
        assert_eq!(bounded("€€", 6), "€€");
        assert_eq!(bounded("€€", 5), "€…");
    }

    #[test]
    fn a_chunk_takes_sessions_with_turns_in_it_and_their_touched_and_cited_notes() {
        let s = Setup::new();
        s.add_note("notes/Other.md", "# Other\n\nText.\n");
        s.add_session(
            "s2",
            at("2026-11-08T01:30:00Z"),
            &[],
            Some("See [[Other]] and [[Missing]]."),
        );
        let index = s.index();
        let chunk = Chunk {
            start: at("2026-11-07T03:30:00Z"),
            end: s.now,
        };
        let inputs = for_chunk(&s.ws, &index, &index.last_ordinals().unwrap(), chunk).unwrap();
        let ids: Vec<&str> = inputs
            .sessions
            .iter()
            .map(|s| s.record.id.as_str())
            .collect();
        assert_eq!(ids, ["s1", "s2"]);
        assert_eq!(inputs.sessions[0].last_ordinal, 1);
        let notes: Vec<(&Path, Vec<&str>)> = inputs
            .notes
            .iter()
            .map(|(path, ids)| (path.as_path(), ids.iter().map(String::as_str).collect()))
            .collect();
        assert_eq!(
            notes,
            [
                (Path::new("notes/Other.md"), vec!["s2"]),
                (Path::new(NOTE_PATH), vec!["s1"]),
            ]
        );
        let later = Chunk {
            start: s.now,
            end: at("2026-11-09T03:30:00Z"),
        };
        let none = for_chunk(&s.ws, &index, &HashMap::new(), later).unwrap();
        assert_eq!(none, ChunkInputs::default());
    }

    #[test]
    fn a_note_gathers_its_sessions_its_query_turns_and_human_confirmed_text() {
        let s = Setup::new();
        let index = s.index();
        let chunk = for_chunk(
            &s.ws,
            &index,
            &index.last_ordinals().unwrap(),
            Chunk {
                start: at("2026-11-07T03:30:00Z"),
                end: s.now,
            },
        )
        .unwrap();
        let reverted = vec![
            proposal_on(NOTE_PATH, "ran on 3 November"),
            proposal_on(NOTE_PATH, "absent text"),
        ];
        let reads = [
            Input::Sessions,
            Input::Notes,
            Input::Turns,
            Input::HumanConfirmed,
        ];
        let gathered = for_note(
            &s.ws,
            &index,
            &chunk,
            Path::new(NOTE_PATH),
            &reads,
            10_000,
            &reverted,
        )
        .unwrap()
        .unwrap();
        assert_eq!(gathered.notes[0].bytes, NOTE.as_bytes());
        assert_eq!(gathered.sessions.len(), 1);
        assert_eq!(gathered.turns.len(), 1);
        assert_eq!(gathered.turns[0].text, TURN_1);
        assert_eq!(
            gathered.human_confirmed,
            [(PathBuf::from(NOTE_PATH), "ran on 3 November".to_owned())]
        );
        assert_eq!(
            gathered.counts(),
            "1 sessions, 1 notes, 1 turns, 1 human-confirmed"
        );
        let (blocks, turns) = gathered.rendered(&reads);
        assert_eq!(blocks.len(), 3);
        assert!(turns[0].contains("\"ordinal\":1"));
        // A budget too small for the turn keeps none.
        let small = for_note(
            &s.ws,
            &index,
            &chunk,
            Path::new(NOTE_PATH),
            &reads,
            10,
            &reverted,
        )
        .unwrap()
        .unwrap();
        assert!(small.turns.is_empty());
        let gone = for_note(
            &s.ws,
            &index,
            &chunk,
            Path::new("notes/Gone.md"),
            &reads,
            10_000,
            &[],
        )
        .unwrap();
        assert_eq!(gone.unwrap_err(), "the note no longer exists");
    }

    #[test]
    fn a_pass_over_everything_takes_every_turn_that_fits() {
        let s = Setup::new();
        let index = s.index();
        let chunk = for_chunk(
            &s.ws,
            &index,
            &index.last_ordinals().unwrap(),
            Chunk {
                start: at("2026-11-07T03:30:00Z"),
                end: s.now,
            },
        )
        .unwrap();
        let reads = [Input::Sessions, Input::Notes, Input::Turns];
        let all = for_all(&s.ws, &index, &chunk, &reads, 10_000, &[]).unwrap();
        assert_eq!(
            (all.sessions.len(), all.notes.len(), all.turns.len()),
            (1, 1, 2)
        );
        let first_only = turn_line(&all.turns[0]).len() + 1;
        let one = for_all(&s.ws, &index, &chunk, &reads, first_only, &[]).unwrap();
        assert_eq!(one.turns.len(), 1);
    }

    #[test]
    fn reverted_proposals_come_from_revert_receipts_only() {
        let s = Setup::new();
        assert!(reverted_proposals(&s.ws).unwrap().is_empty());
        let proposal = proposal_on(NOTE_PATH, "old");
        s.write_proposal(&proposal);
        s.write_receipt(proposal.id, Verdict::Reject);
        assert!(reverted_proposals(&s.ws).unwrap().is_empty());
        s.write_receipt(proposal.id, Verdict::Revert);
        assert_eq!(reverted_proposals(&s.ws).unwrap(), [proposal]);
    }

    fn proposal_on(note: &str, old: &str) -> Proposal {
        Proposal {
            id: Ulid::generate(),
            pass: "verify".into(),
            note: note.into(),
            kind: ProposalKind::Replace,
            before_hash: String::new(),
            after_hash: String::new(),
            old: old.into(),
            new: "new".into(),
            evidence: vec![Evidence {
                session: "s1".into(),
                ordinal: 1,
            }],
            reason: String::new(),
        }
    }
}
