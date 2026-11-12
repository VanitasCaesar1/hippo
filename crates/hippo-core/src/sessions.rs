//! Session records, `.hippo/sessions/<id>.json`.
//!
//! [`start`] and [`end`] write a declared record (`hippo session`, and
//! `hippo hook` with [`set_task_and_seen`] and [`set_context_bytes`]);
//! [`derive()`] writes the record of a session that has no declared one;
//! [`list`] reads them all and [`render_log`] makes the session log.
//! Session ids must pass [`is_valid_id`]: they become file names.
//!
//! A **declared** record is written by `hippo session` and `hippo hook`; a
//! **derived** record, by ingest alone. [`derive()`] never touches a declared
//! one, so a record has one writer.

use std::collections::{BTreeSet, HashMap};
use std::io;
use std::path::{Path, PathBuf};

use time::{Date, OffsetDateTime, UtcOffset};

use crate::adapters::Role;
use crate::config::{stays_inside, Workspace};
use crate::dream::history;
use crate::error::{Error, Result};
use crate::index::{Index, Turn};
use crate::notes;
use crate::records::{list_sessions, load_error, Listed, Session, Tool, Verdict};
use crate::util::write_atomic;

pub use crate::session_meta::{
    call_summary, classify, openings_of, origin, origins, request_text, run_facts, title,
    Injection, Origin, RunFacts, TurnKind,
};

/// The longest session id: ids become file names.
const MAX_ID_LEN: usize = 128;

/// Whether `id` matches `[A-Za-z0-9_-]{1,128}`, the ids that are safe to
/// use as a file name.
pub fn is_valid_id(id: &str) -> bool {
    (1..=MAX_ID_LEN).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// `<workspace>/.hippo/sessions/<id>.json`. The id must be valid.
pub(crate) fn record_path(ws: &Workspace, id: &str) -> PathBuf {
    ws.hippo_dir().join("sessions").join(format!("{id}.json"))
}

/// Which sessions belong to a workspace: those
/// whose working directory is the workspace root or inside it (or inside
/// one of its git worktrees), and those whose indexed turns read, edit or
/// discuss one of its notes. Only those get a derived record
/// in `.hippo/sessions/`; a declared record belongs by being declared.
pub struct Scope {
    root: PathBuf,
    notes_dir: PathBuf,
    log: PathBuf,
    notes: notes::Notes,
}

impl Scope {
    /// Scans `ws`'s notes once; the scope is then reused for every session.
    pub fn new(ws: &Workspace) -> Result<Scope> {
        Ok(Scope {
            root: ws.root.clone(),
            notes_dir: ws.notes_dir.clone(),
            log: notes::normalise(&ws.log_path),
            // A missing notes folder (`doctor` reports it) has no notes.
            notes: if ws.notes_root().is_dir() {
                notes::Notes::scan(&ws.notes_root())?
            } else {
                notes::Notes {
                    root: ws.notes_root(),
                    notes: Default::default(),
                }
            },
        })
    }

    /// Whether session `id`, which ran in `cwd` (when its transcript says)
    /// and whose turns the index holds, belongs.
    pub fn belongs(&self, index: &Index, id: &str, cwd: Option<&Path>) -> Result<bool> {
        let resolver = crate::dream::observed::Resolver::new(
            &self.root,
            &self.notes_dir,
            &self.notes,
            Some(&self.log),
        );
        if cwd.is_some_and(|dir| resolver.holds(dir)) {
            return Ok(true);
        }
        Ok(!crate::dream::inputs::observed_in(index, &resolver, id)?.is_empty())
    }
}

/// Removes session `id`'s record when it is a derived one, and says whether
/// it did. A declared record, or none, is left alone.
pub fn remove_derived(ws: &Workspace, id: &str) -> Result<bool> {
    if !is_valid_id(id) {
        return Ok(false);
    }
    let path = record_path(ws, id);
    match read_record(&path) {
        Ok(Some(record)) if record.derived => {
            std::fs::remove_file(&path).map_err(|e| Error::io_at(&path, e))?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// Writes the derived record of session `id`, found in a transcript of
/// `tool`, and says whether it wrote one. `started` is when the adapter
/// says the session began (`Parsed.started`: the `session_meta` timestamp
/// for Codex, the first kept record's timestamp for Claude Code), and
/// `turns` are the session's turns as indexed.
///
/// - An invalid id is skipped with a warning (`Ok(false)`).
/// - A declared record (`derived` false) is never touched.
/// - Otherwise the record is built from the transcript: empty `task`,
///   `task_seen` false, `started` as given and `ended` the last turn's
///   timestamp, or `started` when there are no turns (a derived session is
///   never open), both in UTC, no notes touched and no handoff. For Codex,
///   dsh and OpenCode, `handoff` is the last assistant turn's text, redacted like everything
///   else in the index, with `handoff_derived` true.
/// - It is written, with [`Session::save`], only when no record exists or
///   the existing derived record differs, so an unchanged transcript never
///   rewrites it and a full ingest is idempotent.
///
/// A record that exists but cannot be read is an error and is left alone:
/// it may be a declared record.
pub fn derive(
    ws: &Workspace,
    id: &str,
    tool: Tool,
    started: OffsetDateTime,
    turns: &[Turn],
) -> Result<bool> {
    if !is_valid_id(id) {
        tracing::warn!(
            "not deriving a record for session {id:?}: ids must match [A-Za-z0-9_-]{{1,128}}"
        );
        return Ok(false);
    }
    let path = record_path(ws, id);
    let existing = read_record(&path)?;
    if existing.as_ref().is_some_and(|record| !record.derived) {
        return Ok(false);
    }
    let record = derived_record(id, tool, started, turns);
    if existing.as_ref() == Some(&record) {
        return Ok(false);
    }
    record.save(&path)?;
    Ok(true)
}

fn derived_record(id: &str, tool: Tool, started: OffsetDateTime, turns: &[Turn]) -> Session {
    let ended = turns.last().map_or(started, |turn| turn.ts);
    let handoff = match tool {
        Tool::Codex | Tool::Dsh | Tool::OpenCode | Tool::Muse | Tool::CommandCode => turns
            .iter()
            .rev()
            .find(|turn| turn.role == Role::Assistant)
            .map(|turn| turn.text.as_str().to_owned()),
        Tool::ClaudeCode => None,
    };
    Session {
        id: id.to_owned(),
        tool,
        task: String::new(),
        task_seen: false,
        context_bytes_at_start: 0,
        started: utc(started),
        ended: Some(utc(ended)),
        notes_touched: Vec::new(),
        handoff_derived: handoff.is_some(),
        handoff,
        no_change_reason: None,
        context_start: None,
        context_prompt: None,
        derived: true,
    }
}

/// `ts` in UTC, the zone every timestamp in a record is written in; `ts`
/// itself in the one case it cannot be converted.
fn utc(ts: OffsetDateTime) -> OffsetDateTime {
    ts.checked_to_offset(UtcOffset::UTC).unwrap_or(ts)
}

/// Starts session `id` (`hippo session start`). With no record yet, it
/// writes a declared one: the task given, `task_seen` false, started now
/// and open. An existing declared record is reopened: `ended` becomes null
/// and everything else, `task` and `handoff` included, is kept. A derived
/// record is replaced by a declared one, which [`derive()`] then never
/// writes. An id that fails [`is_valid_id`] is `InvalidSessionId`.
pub fn start(ws: &Workspace, tool: Tool, id: &str, task: Option<&str>) -> Result<Session> {
    start_at(ws, tool, id, task, OffsetDateTime::now_utc())
}

fn start_at(
    ws: &Workspace,
    tool: Tool,
    id: &str,
    task: Option<&str>,
    now: OffsetDateTime,
) -> Result<Session> {
    if !is_valid_id(id) {
        return Err(Error::InvalidSessionId(id.to_owned()));
    }
    let path = record_path(ws, id);
    let record = match read_record(&path)? {
        Some(mut declared) if !declared.derived => {
            declared.ended = None;
            declared
        }
        _ => Session {
            id: id.to_owned(),
            tool,
            task: task.unwrap_or_default().to_owned(),
            task_seen: false,
            context_bytes_at_start: 0,
            started: utc(now),
            ended: None,
            notes_touched: Vec::new(),
            handoff: None,
            handoff_derived: false,
            no_change_reason: None,
            context_start: None,
            context_prompt: None,
            derived: false,
        },
    };
    record.save(&path)?;
    Ok(record)
}

/// Ends declared session `id` (`hippo session end`): `ended` is now;
/// `notes` are appended to `notes_touched`, skipping duplicates; a
/// `handoff` (Markdown naming notes with `[[...]]` links) replaces the
/// record's, with `handoff_derived` false. `no_change_reason` becomes
/// `no_change`, else `unspecified` when neither notes nor a handoff were
/// given, else null. A missing or derived record is `NoDeclaredSession`.
pub fn end(
    ws: &Workspace,
    id: &str,
    notes: &[PathBuf],
    handoff: Option<&str>,
    no_change: Option<&str>,
) -> Result<Session> {
    end_at(ws, id, notes, handoff, no_change, OffsetDateTime::now_utc())
}

fn end_at(
    ws: &Workspace,
    id: &str,
    notes: &[PathBuf],
    handoff: Option<&str>,
    no_change: Option<&str>,
    now: OffsetDateTime,
) -> Result<Session> {
    let (path, mut record) = declared(ws, id)?;
    record.ended = Some(utc(now));
    for note in notes {
        if !record.notes_touched.contains(note) {
            record.notes_touched.push(note.clone());
        }
    }
    if let Some(handoff) = handoff {
        record.handoff = Some(handoff.to_owned());
        record.handoff_derived = false;
    }
    record.no_change_reason = match no_change {
        Some(reason) => Some(reason.to_owned()),
        None if notes.is_empty() && handoff.is_none() => Some("unspecified".to_owned()),
        None => None,
    };
    record.save(&path)?;
    Ok(record)
}

/// Ends declared session `id` as a Codex hook does: `ended` is now, and
/// `handoff`, when given, replaces the record's with `handoff_derived`
/// true and no `no_change_reason`, as ingest would have derived it; with
/// none it ends as [`end`] does with nothing given (`unspecified`).
pub fn end_with_derived_handoff(
    ws: &Workspace,
    id: &str,
    handoff: Option<&str>,
) -> Result<Session> {
    let Some(handoff) = handoff else {
        return end(ws, id, &[], None, None);
    };
    let (path, mut record) = declared(ws, id)?;
    record.ended = Some(utc(OffsetDateTime::now_utc()));
    record.handoff = Some(handoff.to_owned());
    record.handoff_derived = true;
    record.no_change_reason = None;
    record.save(&path)?;
    Ok(record)
}

/// Sets a declared session's `task` and `task_seen` (`hippo hook prompt`,
/// on the first prompt).
pub fn set_task_and_seen(ws: &Workspace, id: &str, task: &str) -> Result<Session> {
    set_task_seen_and_context(ws, id, task, None)
}

/// [`set_task_and_seen`], also recording the text the prompt hook printed
/// as `context_prompt`.
pub fn set_task_seen_and_context(
    ws: &Workspace,
    id: &str,
    task: &str,
    context: Option<&str>,
) -> Result<Session> {
    let (path, mut record) = declared(ws, id)?;
    record.task = task.to_owned();
    record.task_seen = true;
    if let Some(text) = context {
        record.context_prompt = Some(text.to_owned());
    }
    record.save(&path)?;
    Ok(record)
}

/// Sets a declared session's `context_bytes_at_start` (`hippo hook
/// session-start`).
pub fn set_context_bytes(ws: &Workspace, id: &str, bytes: usize) -> Result<Session> {
    set_context_start(ws, id, bytes, None)
}

/// [`set_context_bytes`], also recording the text the session-start hook
/// printed as `context_start`.
pub fn set_context_start(
    ws: &Workspace,
    id: &str,
    bytes: usize,
    text: Option<&str>,
) -> Result<Session> {
    let (path, mut record) = declared(ws, id)?;
    record.context_bytes_at_start = bytes;
    // A resumed session's hook prints the block again; the x-ray shows the
    // one it started with.
    if let (Some(text), None) = (text, &record.context_start) {
        record.context_start = Some(text.to_owned());
    }
    record.save(&path)?;
    Ok(record)
}

/// The record of session `id`, declared or derived; `None` when there is
/// none. An id that fails [`is_valid_id`] is `InvalidSessionId`.
pub fn get(ws: &Workspace, id: &str) -> Result<Option<Session>> {
    if !is_valid_id(id) {
        return Err(Error::InvalidSessionId(id.to_owned()));
    }
    read_record(&record_path(ws, id))
}

/// Every session record in `.hippo/sessions/`, newest `started` first and
/// then by id. A record that cannot be read is an error.
pub fn list(ws: &Workspace) -> Result<Vec<Session>> {
    let mut records = list_sessions(ws)?
        .into_iter()
        .map(Listed::require)
        .collect::<Result<Vec<_>>>()?;
    sort_sessions(&mut records);
    Ok(records)
}

/// Newest `started` first, then by id.
fn sort_sessions(records: &mut [Session]) {
    records.sort_by(|a, b| b.started.cmp(&a.started).then_with(|| a.id.cmp(&b.id)));
}

/// The session log (`hippo log`): `# Session log`, then `## <YYYY-MM-DD>`
/// for each UTC day sessions started on, newest first, then one line per
/// session, newest first:
///
/// `- <HH:MM> <tool> <id> — <task or (no task)> — ended <time|open> —
/// turns <count|?> — notes: [[a]], [[b]] — handoff: yes|derived|no
/// (<no_change_reason>) — dreaming: <n> confirmed, <m> rejected`
///
/// Times are UTC; an end on a later day carries its date. The turn count
/// comes from `index`, `?` when it has no turns for the session or there
/// is no index. A note is linked by its path below `notes_dir` without
/// `.md`; `none` when no note was touched. `n` and `m` count the confirm
/// and reject receipts whose proposal's `evidence` names the session.
///
/// Fails on the first session record, receipt or proposal that cannot be
/// read; [`log_rows`] lists them instead.
pub fn render_log(ws: &Workspace, index: Option<&Index>) -> Result<String> {
    let turns = match index {
        Some(index) => index.turn_counts()?,
        None => HashMap::new(),
    };
    let dreaming = dreaming_counts(ws, true)?;
    let mut out = String::from("# Session log\n");
    let mut day: Option<Date> = None;
    for session in list(ws)? {
        let row = row_of(&session, &turns, &dreaming);
        let started = utc(row.started);
        if day != Some(started.date()) {
            day = Some(started.date());
            out.push_str(&format!("\n## {}\n\n", started.date()));
        }
        out.push_str(&log_line(ws, &row));
        out.push('\n');
    }
    Ok(out)
}

/// Whether a session handed off, as a `hippo log` line says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handoff {
    /// The session wrote no handoff.
    No,
    /// Ingest derived the handoff from the transcript.
    Derived,
    /// The agent declared the handoff.
    Yes,
}

/// The facts of one `hippo log` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogRow {
    pub id: String,
    pub tool: Tool,
    pub started: OffsetDateTime,
    /// `None` while the session is open.
    pub ended: Option<OffsetDateTime>,
    /// May be empty.
    pub task: String,
    /// The indexed turns; `None` without an index, or when it has none.
    pub turns: Option<u64>,
    /// Paths relative to the workspace.
    pub notes: Vec<PathBuf>,
    pub handoff: Handoff,
    pub no_change_reason: Option<String>,
    /// Ingest made the record from a transcript: hippo's hooks did not
    /// see the session start.
    pub derived: bool,
    /// Confirm receipts whose proposal's evidence names the session.
    pub confirmed: usize,
    /// Reject receipts whose proposal's evidence names the session.
    pub rejected: usize,
}

fn row_of(
    session: &Session,
    turns: &HashMap<String, u64>,
    dreaming: &HashMap<String, (usize, usize)>,
) -> LogRow {
    let (confirmed, rejected) = dreaming.get(&session.id).copied().unwrap_or_default();
    LogRow {
        id: session.id.clone(),
        tool: session.tool,
        started: session.started,
        ended: session.ended,
        task: session.task.clone(),
        turns: turns.get(&session.id).copied(),
        notes: session.notes_touched.clone(),
        handoff: match (&session.handoff, session.handoff_derived) {
            (None, _) => Handoff::No,
            (Some(_), true) => Handoff::Derived,
            (Some(_), false) => Handoff::Yes,
        },
        no_change_reason: session.no_change_reason.clone(),
        derived: session.derived,
        confirmed,
        rejected,
    }
}

/// One row per session record: the readable ones in the order of [`list`]
/// (newest `started` first, then id), then the unreadable ones as errors in
/// file-name order. `turns` come from `index` when given. A receipt or
/// proposal that cannot be read is passed over in the dreaming counts, so
/// that one half-written by a running dreaming pass does not empty the
/// list.
pub fn log_rows(ws: &Workspace, index: Option<&Index>) -> Result<Vec<Listed<LogRow>>> {
    let turns = match index {
        Some(index) => index.turn_counts()?,
        None => HashMap::new(),
    };
    let dreaming = dreaming_counts(ws, false)?;
    let mut sessions = Vec::new();
    let mut unreadable = Vec::new();
    for listed in list_sessions(ws)? {
        match listed.record {
            Ok(session) => sessions.push(session),
            Err(err) => unreadable.push(Listed {
                path: listed.path,
                record: Err(err),
            }),
        }
    }
    sort_sessions(&mut sessions);
    let mut rows: Vec<Listed<LogRow>> = sessions
        .iter()
        .map(|session| Listed {
            path: record_path(ws, &session.id),
            record: Ok(row_of(session, &turns, &dreaming)),
        })
        .collect();
    rows.extend(unreadable);
    Ok(rows)
}

/// The row of session `id`, with `turns` as given: `None` when it has no
/// record, an error when its record cannot be read.
pub fn log_row(ws: &Workspace, id: &str, turns: Option<u64>) -> Result<Option<LogRow>> {
    let Some(session) = get(ws, id)? else {
        return Ok(None);
    };
    let dreaming = dreaming_counts(ws, false)?;
    let mut row = row_of(&session, &HashMap::new(), &dreaming);
    row.turns = turns;
    Ok(Some(row))
}

/// Writes [`render_log`] to the workspace's `log_path` atomically and
/// returns the path written. `log_path` must be a file path relative to the
/// workspace, without `..`.
pub fn write_log(ws: &Workspace, index: Option<&Index>) -> Result<PathBuf> {
    if ws.log_path.file_name().is_none() || !stays_inside(&ws.log_path) {
        let why = format!(
            "log_path must be a file inside the workspace (relative, without ..), found {}",
            ws.log_path.display()
        );
        return Err(Error::Io(io::Error::new(io::ErrorKind::InvalidInput, why)));
    }
    let text = render_log(ws, index)?;
    let path = ws.root.join(&ws.log_path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| Error::io_at(parent, e))?;
    }
    write_atomic(&path, text.as_bytes())?;
    Ok(path)
}

/// The `hippo log` line of `row`, as [`render_log`] writes it.
pub fn log_line(ws: &Workspace, row: &LogRow) -> String {
    let started = utc(row.started);
    let ended = match row.ended.map(utc) {
        None => "open".to_owned(),
        Some(ended) if ended.date() == started.date() => hh_mm(ended),
        Some(ended) => format!("{} {}", ended.date(), hh_mm(ended)),
    };
    let task = match one_line(&row.task) {
        task if task.is_empty() => "(no task)".to_owned(),
        task => task,
    };
    let turns = row
        .turns
        .map_or_else(|| "?".to_owned(), |count| count.to_string());
    let notes = if row.notes.is_empty() {
        "none".to_owned()
    } else {
        let links: Vec<String> = row
            .notes
            .iter()
            .map(|note| format!("[[{}]]", link_target(ws, note)))
            .collect();
        links.join(", ")
    };
    let mut handoff = match row.handoff {
        Handoff::No => "no",
        Handoff::Derived => "derived",
        Handoff::Yes => "yes",
    }
    .to_owned();
    if let Some(reason) = &row.no_change_reason {
        handoff.push_str(&format!(" ({})", one_line(reason)));
    }
    format!(
        "- {} {} {} — {task} — ended {ended} — turns {turns} — notes: {notes} — \
         handoff: {handoff} — dreaming: {} confirmed, {} rejected",
        hh_mm(started),
        row.tool.as_str(),
        row.id,
        row.confirmed,
        row.rejected,
    )
}

fn hh_mm(at: OffsetDateTime) -> String {
    format!("{:02}:{:02}", at.hour(), at.minute())
}

/// `text` with every run of whitespace, line breaks included, as one space.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The `[[...]]` target naming a note path: the path below `notes_dir`
/// without `.md`, which a link resolves back to.
fn link_target(ws: &Workspace, note: &Path) -> String {
    let notes_dir = notes::normalise(&ws.notes_dir);
    let below = note.strip_prefix(&notes_dir).unwrap_or(note);
    let text = below.to_string_lossy();
    text.strip_suffix(".md").unwrap_or(&text).to_owned()
}

/// For each session, the confirm and reject receipts whose proposal's
/// `evidence` names it, from [`crate::dream::history`]. Revert receipts are
/// not counted. With `strict`, a receipt that cannot be read, or a confirm
/// or reject receipt whose proposal cannot be, is an error; without it,
/// such a receipt is passed over.
fn dreaming_counts(ws: &Workspace, strict: bool) -> Result<HashMap<String, (usize, usize)>> {
    let history = history(ws)?;
    if strict {
        let receipts = ws.hippo_dir().join("receipts");
        if let Some((_, text)) = history
            .unreadable
            .iter()
            .find(|(path, _)| path.starts_with(&receipts))
        {
            return Err(load_error(text.clone()));
        }
    }
    let mut counts: HashMap<String, (usize, usize)> = HashMap::new();
    for change in &history.changes {
        let counted: Vec<bool> = change
            .receipts
            .iter()
            .filter_map(|receipt| match receipt.verdict {
                Verdict::Confirm => Some(true),
                Verdict::Reject => Some(false),
                Verdict::Revert => None,
            })
            .collect();
        if counted.is_empty() {
            continue;
        }
        let proposal = match &change.proposal.record {
            Ok(proposal) => proposal,
            Err(_) if !strict => continue,
            Err(text) => return Err(load_error(text.clone())),
        };
        let sessions: BTreeSet<&str> = proposal
            .evidence
            .iter()
            .map(|evidence| evidence.session.as_str())
            .collect();
        for confirmed in counted {
            for session in &sessions {
                let count = counts.entry((*session).to_owned()).or_default();
                if confirmed {
                    count.0 += 1;
                } else {
                    count.1 += 1;
                }
            }
        }
    }
    Ok(counts)
}

/// The declared record of `id` and its path; a missing or derived record is
/// `NoDeclaredSession`.
fn declared(ws: &Workspace, id: &str) -> Result<(PathBuf, Session)> {
    if !is_valid_id(id) {
        return Err(Error::InvalidSessionId(id.to_owned()));
    }
    let path = record_path(ws, id);
    match read_record(&path)? {
        Some(record) if !record.derived => Ok((path, record)),
        _ => Err(Error::NoDeclaredSession(id.to_owned())),
    }
}

/// The record at `path`; `None` when there is none.
fn read_record(path: &Path) -> Result<Option<Session>> {
    match Session::load(path) {
        Ok(record) => Ok(Some(record)),
        Err(Error::Io(err)) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::records::{Proposal, Receipt};
    use crate::redact::redact;
    use std::fs;
    use time::format_description::well_known::Rfc3339;
    use ulid::Ulid;

    fn at(ts: &str) -> OffsetDateTime {
        OffsetDateTime::parse(ts, &Rfc3339).unwrap()
    }

    fn turn(ordinal: u32, role: Role, ts: &str, text: &str) -> Turn {
        Turn {
            ordinal,
            role,
            ts: at(ts),
            tool_name: None,
            text: redact(text),
        }
    }

    fn workspace(root: &std::path::Path) -> Workspace {
        Workspace {
            root: root.to_path_buf(),
            notes_dir: "notes".into(),
            entry_note: "notes/START HERE.md".into(),
            required_notes: Vec::new(),
            budget_bytes: crate::DEFAULT_CONTEXT_BUDGET,
            log_path: "notes/Session log.md".into(),
            sample_tasks: Vec::new(),
            passes_off: Vec::new(),
            lessons: true,
            snapshot_keep_days: None,
            log_notes: None,
        }
    }

    fn conversation() -> Vec<Turn> {
        vec![
            turn(0, Role::User, "2026-11-01T03:36:53Z", "ship it"),
            turn(1, Role::Assistant, "2026-11-01T03:37:10Z", "working on it"),
            turn(2, Role::Tool, "2026-11-01T03:38:00Z", "ls"),
            turn(
                3,
                Role::Assistant,
                "2026-11-01T03:40:10Z",
                "Finished the migration.",
            ),
        ]
    }

    #[test]
    fn ids_are_file_name_safe() {
        for id in [
            "a",
            "stub-1",
            "codex-01a1952e-796c-73b1-b951-1fb376387e9a",
            "A_b-9",
            &"x".repeat(128),
        ] {
            assert!(is_valid_id(id), "{id}");
        }
        for id in [
            "",
            &"x".repeat(129),
            "a/b",
            "..",
            "a.b",
            "a b",
            "é",
            "a\n",
            "a\0",
            "../etc/passwd",
        ] {
            assert!(!is_valid_id(id), "{id:?}");
        }
    }

    #[test]
    fn derive_writes_the_record_of_a_claude_code_transcript() {
        let dir = tempfile::tempdir().unwrap();
        let ws = workspace(dir.path());
        // The adapter's start is before the first turn (03:36:53): it is
        // the record's `started`; the last turn is its `ended`.
        let wrote = derive(
            &ws,
            "stub-1",
            Tool::ClaudeCode,
            at("2026-11-01T03:00:00Z"),
            &conversation(),
        )
        .unwrap();
        assert!(wrote);
        let path = dir.path().join(".hippo/sessions/stub-1.json");
        assert_eq!(
            Session::load(&path).unwrap(),
            Session {
                id: "stub-1".into(),
                tool: Tool::ClaudeCode,
                task: String::new(),
                task_seen: false,
                context_bytes_at_start: 0,
                started: at("2026-11-01T03:00:00Z"),
                ended: Some(at("2026-11-01T03:40:10Z")),
                notes_touched: Vec::new(),
                handoff: None,
                handoff_derived: false,
                no_change_reason: None,
                context_start: None,
                context_prompt: None,
                derived: true,
            }
        );
        let json: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(json["protocol"], "HIPPO_V1");
        assert_eq!(json["started"], "2026-11-01T03:00:00Z");
        assert_eq!(json["ended"], "2026-11-01T03:40:10Z");
    }

    #[test]
    fn derive_takes_a_codex_handoff_from_the_last_assistant_turn_redacted() {
        let dir = tempfile::tempdir().unwrap();
        let ws = workspace(dir.path());
        let mut turns = conversation();
        turns.push(turn(
            4,
            Role::Assistant,
            "2026-11-01T03:41:00Z",
            "Done. The key was AKIAFAKEFAKEFAKE0000, see [[Topic A]].",
        ));
        turns.push(turn(5, Role::Tool, "2026-11-01T03:41:05Z", "echo done"));
        derive(
            &ws,
            "codex-01a1",
            Tool::Codex,
            at("2026-11-01T03:00:00Z"),
            &turns,
        )
        .unwrap();
        let record = Session::load(&dir.path().join(".hippo/sessions/codex-01a1.json")).unwrap();
        assert_eq!(record.tool, Tool::Codex);
        assert_eq!(
            record.handoff.as_deref(),
            Some("Done. The key was [redacted:aws], see [[Topic A]].")
        );
        assert!(record.handoff_derived);
        assert!(record.derived);
        assert_eq!(record.ended, Some(at("2026-11-01T03:41:05Z")));
        let bytes = fs::read_to_string(dir.path().join(".hippo/sessions/codex-01a1.json")).unwrap();
        assert!(!bytes.contains("AKIAFAKE"), "{bytes}");

        // No assistant turn, no handoff, and nothing derived.
        let only_user = [turn(0, Role::User, "2026-11-01T03:36:53Z", "hello")];
        derive(
            &ws,
            "codex-02b2",
            Tool::Codex,
            at("2026-11-01T03:00:00Z"),
            &only_user,
        )
        .unwrap();
        let record = Session::load(&dir.path().join(".hippo/sessions/codex-02b2.json")).unwrap();
        assert_eq!(record.handoff, None);
        assert!(!record.handoff_derived);
    }

    #[test]
    fn derive_without_turns_starts_and_ends_at_the_parsers_start() {
        let dir = tempfile::tempdir().unwrap();
        let ws = workspace(dir.path());
        derive(
            &ws,
            "empty",
            Tool::ClaudeCode,
            at("2026-11-01T03:00:00Z"),
            &[],
        )
        .unwrap();
        let record = Session::load(&dir.path().join(".hippo/sessions/empty.json")).unwrap();
        assert_eq!(record.started, at("2026-11-01T03:00:00Z"));
        assert_eq!(record.ended, Some(at("2026-11-01T03:00:00Z")));
    }

    #[test]
    fn derive_writes_timestamps_in_utc() {
        let dir = tempfile::tempdir().unwrap();
        let ws = workspace(dir.path());
        let turns = [
            turn(0, Role::User, "2026-11-01T13:06:53+09:30", "hi"),
            turn(1, Role::Assistant, "2026-11-01T13:10:10+09:30", "hello"),
        ];
        derive(
            &ws,
            "acst",
            Tool::ClaudeCode,
            at("2026-11-01T13:00:00+09:30"),
            &turns,
        )
        .unwrap();
        let text = fs::read_to_string(dir.path().join(".hippo/sessions/acst.json")).unwrap();
        // The adapter's start (13:00 +09:30) and the last turn (13:10:10 +09:30).
        assert!(
            text.contains("\"started\": \"2026-11-01T03:30:00Z\""),
            "{text}"
        );
        assert!(
            text.contains("\"ended\": \"2026-11-01T03:40:10Z\""),
            "{text}"
        );
    }

    #[test]
    fn derive_skips_an_invalid_id_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let ws = workspace(dir.path());
        for id in ["", "../escape", "a/b", "has space"] {
            let wrote = derive(
                &ws,
                id,
                Tool::ClaudeCode,
                at("2026-11-01T03:00:00Z"),
                &conversation(),
            )
            .unwrap();
            assert!(!wrote, "{id:?}");
        }
        assert!(!dir.path().join(".hippo").exists());
        assert!(!dir.path().join("escape.json").exists());
    }

    #[test]
    fn derive_never_touches_a_declared_record() {
        let dir = tempfile::tempdir().unwrap();
        let ws = workspace(dir.path());
        let path = dir.path().join(".hippo/sessions/ab12.json");
        let declared = Session {
            id: "ab12".into(),
            tool: Tool::ClaudeCode,
            task: "fix the webhook retry".into(),
            task_seen: true,
            context_bytes_at_start: 4120,
            started: at("2026-11-08T03:00:00Z"),
            ended: None,
            notes_touched: vec!["brain/Engineering/Webhooks.md".into()],
            handoff: None,
            handoff_derived: false,
            no_change_reason: None,
            context_start: None,
            context_prompt: None,
            derived: false,
        };
        declared.save(&path).unwrap();
        let before = fs::read(&path).unwrap();
        let wrote = derive(
            &ws,
            "ab12",
            Tool::ClaudeCode,
            at("2026-11-08T03:00:00Z"),
            &conversation(),
        )
        .unwrap();
        assert!(!wrote);
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(Session::load(&path).unwrap(), declared);
    }

    #[test]
    fn derive_rewrites_a_derived_record_only_when_it_differs() {
        let dir = tempfile::tempdir().unwrap();
        let ws = workspace(dir.path());
        let path = dir.path().join(".hippo/sessions/stub-1.json");
        let start = at("2026-11-01T03:00:00Z");
        assert!(derive(&ws, "stub-1", Tool::ClaudeCode, start, &conversation()).unwrap());

        // The same record in another layout: equal, so it is left as it is.
        let compact = serde_json::to_vec(
            &serde_json::from_slice::<serde_json::Value>(&fs::read(&path).unwrap()).unwrap(),
        )
        .unwrap();
        fs::write(&path, &compact).unwrap();
        assert!(!derive(&ws, "stub-1", Tool::ClaudeCode, start, &conversation()).unwrap());
        assert_eq!(fs::read(&path).unwrap(), compact);

        // The transcript grew: the record follows it.
        let mut longer = conversation();
        longer.push(turn(
            4,
            Role::Assistant,
            "2026-11-01T04:00:00Z",
            "one more thing",
        ));
        assert!(derive(&ws, "stub-1", Tool::ClaudeCode, start, &longer).unwrap());
        assert_eq!(
            Session::load(&path).unwrap().ended,
            Some(at("2026-11-01T04:00:00Z"))
        );
        assert_ne!(fs::read(&path).unwrap(), compact);
        assert!(!derive(&ws, "stub-1", Tool::ClaudeCode, start, &longer).unwrap());

        // The adapter dates the session differently: a different record.
        let later_start = at("2026-11-01T03:10:00Z");
        assert!(derive(&ws, "stub-1", Tool::ClaudeCode, later_start, &longer).unwrap());
        assert_eq!(Session::load(&path).unwrap().started, later_start);
    }

    #[test]
    fn derive_leaves_a_record_it_cannot_read_alone() {
        let dir = tempfile::tempdir().unwrap();
        let ws = workspace(dir.path());
        let path = dir.path().join(".hippo/sessions/odd.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        for contents in ["{not json", "{\"protocol\":\"HIPPO_V9\",\"id\":\"odd\"}"] {
            fs::write(&path, contents).unwrap();
            let result = derive(
                &ws,
                "odd",
                Tool::ClaudeCode,
                at("2026-11-01T03:00:00Z"),
                &conversation(),
            );
            assert!(result.is_err(), "{contents}: {result:?}");
            assert_eq!(fs::read_to_string(&path).unwrap(), contents);
        }
    }

    const START: &str = "2026-11-08T03:00:00Z";

    fn declared_record(id: &str, tool: Tool, task: &str) -> Session {
        Session {
            id: id.into(),
            tool,
            task: task.into(),
            task_seen: false,
            context_bytes_at_start: 0,
            started: at(START),
            ended: None,
            notes_touched: Vec::new(),
            handoff: None,
            handoff_derived: false,
            no_change_reason: None,
            context_start: None,
            context_prompt: None,
            derived: false,
        }
    }

    #[test]
    fn start_writes_a_declared_record_and_refuses_a_bad_id() {
        let dir = tempfile::tempdir().unwrap();
        let ws = workspace(dir.path());
        let record = start_at(
            &ws,
            Tool::ClaudeCode,
            "ab12cd34",
            Some("fix the webhook retry"),
            at(START),
        )
        .unwrap();
        let expected = declared_record("ab12cd34", Tool::ClaudeCode, "fix the webhook retry");
        assert_eq!(record, expected);
        assert_eq!(
            Session::load(&record_path(&ws, "ab12cd34")).unwrap(),
            expected
        );
        for id in ["", "../escape", "a b"] {
            let err = start(&ws, Tool::Codex, id, None).unwrap_err();
            assert!(
                matches!(&err, Error::InvalidSessionId(bad) if bad == id),
                "{err}"
            );
        }
        assert!(!dir.path().join(".hippo/escape.json").exists());
    }

    #[test]
    fn reopening_a_declared_session_keeps_its_task_and_handoff() {
        let dir = tempfile::tempdir().unwrap();
        let ws = workspace(dir.path());
        start_at(&ws, Tool::ClaudeCode, "ab12", Some("first task"), at(START)).unwrap();
        let handoff = Some("See [[Webhooks]].");
        end_at(&ws, "ab12", &[], handoff, None, at("2026-11-08T04:00:00Z")).unwrap();
        let reopened = start_at(
            &ws,
            Tool::ClaudeCode,
            "ab12",
            Some("second task"),
            at("2026-11-08T05:00:00Z"),
        )
        .unwrap();
        assert_eq!(reopened.task, "first task");
        assert_eq!(reopened.handoff.as_deref(), handoff);
        assert_eq!(reopened.ended, None);
        assert_eq!(reopened.started, at(START));
        assert!(!reopened.derived);
    }

    #[test]
    fn starting_a_derived_session_replaces_it_and_ingest_leaves_it_alone() {
        let dir = tempfile::tempdir().unwrap();
        let ws = workspace(dir.path());
        let parsed_start = at("2026-11-01T03:00:00Z");
        assert!(derive(
            &ws,
            "stub-1",
            Tool::ClaudeCode,
            parsed_start,
            &conversation()
        )
        .unwrap());
        assert!(Session::load(&record_path(&ws, "stub-1")).unwrap().derived);

        let declared =
            start_at(&ws, Tool::ClaudeCode, "stub-1", Some("pick up"), at(START)).unwrap();
        assert_eq!(
            declared,
            declared_record("stub-1", Tool::ClaudeCode, "pick up")
        );
        // From then on ingest does not write it.
        assert!(!derive(
            &ws,
            "stub-1",
            Tool::ClaudeCode,
            parsed_start,
            &conversation()
        )
        .unwrap());
        assert_eq!(
            Session::load(&record_path(&ws, "stub-1")).unwrap(),
            declared
        );
    }

    #[test]
    fn end_follows_its_flags() {
        let dir = tempfile::tempdir().unwrap();
        let ws = workspace(dir.path());
        let ended_at = at("2026-11-08T04:15:00Z");

        // Neither notes nor a handoff: unspecified.
        start_at(&ws, Tool::ClaudeCode, "plain", None, at(START)).unwrap();
        let plain = end_at(&ws, "plain", &[], None, None, ended_at).unwrap();
        assert_eq!(plain.ended, Some(ended_at));
        assert_eq!(plain.no_change_reason.as_deref(), Some("unspecified"));

        // Notes, each kept once, and a handoff: no reason.
        start_at(&ws, Tool::ClaudeCode, "noted", None, at(START)).unwrap();
        let webhooks = PathBuf::from("notes/Webhooks.md");
        let topic = PathBuf::from("notes/Topic A.md");
        end_at(
            &ws,
            "noted",
            std::slice::from_ref(&webhooks),
            None,
            None,
            ended_at,
        )
        .unwrap();
        let notes = [topic.clone(), webhooks.clone()];
        let handoff = Some("Done; see [[Webhooks]].");
        let noted = end_at(&ws, "noted", &notes, handoff, None, ended_at).unwrap();
        assert_eq!(noted.notes_touched, [webhooks, topic]);
        assert_eq!(noted.handoff.as_deref(), handoff);
        assert!(!noted.handoff_derived);
        assert_eq!(noted.no_change_reason, None);

        // A no-change reason is kept as given.
        start_at(&ws, Tool::Codex, "quiet", None, at(START)).unwrap();
        let quiet = end_at(&ws, "quiet", &[], None, Some("read only"), ended_at).unwrap();
        assert_eq!(quiet.no_change_reason.as_deref(), Some("read only"));
        assert_eq!(Session::load(&record_path(&ws, "quiet")).unwrap(), quiet);
    }

    #[test]
    fn end_needs_a_declared_session() {
        let dir = tempfile::tempdir().unwrap();
        let ws = workspace(dir.path());
        derive(&ws, "derived-1", Tool::Codex, at(START), &conversation()).unwrap();
        for id in ["missing", "derived-1"] {
            let err = end(&ws, id, &[], None, None).unwrap_err();
            assert!(matches!(&err, Error::NoDeclaredSession(_)), "{err}");
            assert_eq!(err.to_string(), format!("no declared session {id}"));
        }
        assert!(
            Session::load(&record_path(&ws, "derived-1"))
                .unwrap()
                .derived
        );
    }

    #[test]
    fn the_hook_setters_change_their_fields_only() {
        let dir = tempfile::tempdir().unwrap();
        let ws = workspace(dir.path());
        start_at(&ws, Tool::ClaudeCode, "hooked", None, at(START)).unwrap();
        set_context_bytes(&ws, "hooked", 4120).unwrap();
        let record = set_task_and_seen(&ws, "hooked", "fix the webhook retry").unwrap();
        let expected = Session {
            context_bytes_at_start: 4120,
            task_seen: true,
            ..declared_record("hooked", Tool::ClaudeCode, "fix the webhook retry")
        };
        assert_eq!(record, expected);
        assert_eq!(
            Session::load(&record_path(&ws, "hooked")).unwrap(),
            expected
        );
        let missing = set_task_and_seen(&ws, "nobody", "x").unwrap_err();
        assert!(matches!(missing, Error::NoDeclaredSession(_)), "{missing}");
    }

    #[test]
    fn list_reads_every_record_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let ws = workspace(dir.path());
        assert!(list(&ws).unwrap().is_empty());
        start_at(&ws, Tool::ClaudeCode, "a", None, at(START)).unwrap();
        start_at(&ws, Tool::Codex, "b", None, at("2026-11-08T05:00:00Z")).unwrap();
        derive(
            &ws,
            "c",
            Tool::Codex,
            at("2026-11-01T03:00:00Z"),
            &conversation(),
        )
        .unwrap();
        let sessions = ws.hippo_dir().join("sessions");
        fs::write(sessions.join(".d.json.tmp"), "partial").unwrap();
        fs::write(sessions.join("notes.txt"), "not a record").unwrap();
        let ids: Vec<String> = list(&ws).unwrap().into_iter().map(|s| s.id).collect();
        assert_eq!(ids, ["b", "a", "c"]);
    }

    #[test]
    fn render_log_matches_the_golden_text_for_two_sessions() {
        use crate::index::SessionSource;
        use crate::records::{Evidence, ProposalKind};

        let dir = tempfile::tempdir().unwrap();
        let ws = workspace(dir.path());
        // A declared Claude Code session, ended with a note and a handoff.
        start_at(
            &ws,
            Tool::ClaudeCode,
            "ab12cd34",
            Some("fix the webhook retry"),
            at(START),
        )
        .unwrap();
        end_at(
            &ws,
            "ab12cd34",
            &[PathBuf::from("notes/Engineering/Webhooks.md")],
            Some("Retry backs off; see [[Engineering/Webhooks]]."),
            None,
            at("2026-11-08T04:15:00Z"),
        )
        .unwrap();
        // A derived Codex session the day before, which the index lacks.
        let last = turn(
            0,
            Role::Assistant,
            "2026-11-07T22:41:00Z",
            "Finished the migration.",
        );
        derive(
            &ws,
            "codex-01a1",
            Tool::Codex,
            at("2026-11-07T22:30:00Z"),
            &[last],
        )
        .unwrap();

        // The index holds three turns of the first session.
        let home = tempfile::tempdir().unwrap();
        let mut index = Index::open(home.path(), true).unwrap();
        let source = SessionSource {
            id: "ab12cd34",
            tool: Tool::ClaudeCode,
            transcript_path: Path::new("/t/ab12cd34.jsonl"),
            file_hash: "h",
        };
        let turns = [
            turn(
                0,
                Role::User,
                "2026-11-08T03:00:05Z",
                "the retry is too fast",
            ),
            turn(1, Role::Assistant, "2026-11-08T03:01:00Z", "backing it off"),
            turn(2, Role::User, "2026-11-08T03:02:00Z", "thanks"),
        ];
        index.replace_session(&source, &turns).unwrap();

        // Dreaming: a proposal citing both sessions, confirmed and later
        // reverted (a revert is not counted); one citing the first, rejected.
        let hippo = ws.hippo_dir();
        let proposal = |n: u128, sessions: &[&str]| Proposal {
            id: Ulid::from_parts(1, n),
            pass: "verify".into(),
            note: "notes/Engineering/Webhooks.md".into(),
            kind: ProposalKind::Replace,
            before_hash: "0".repeat(64),
            after_hash: "1".repeat(64),
            old: "ran on 3 November".into(),
            new: "ran on 7 November".into(),
            evidence: sessions
                .iter()
                .map(|session| Evidence {
                    session: (*session).into(),
                    ordinal: 1,
                })
                .collect(),
            reason: "a test".into(),
        };
        let both = proposal(1, &["ab12cd34", "codex-01a1", "ab12cd34"]);
        let first = proposal(2, &["ab12cd34"]);
        for proposal in [&both, &first] {
            let path = hippo
                .join("proposals")
                .join(format!("{}.json", proposal.id));
            proposal.save(&path).unwrap();
        }
        let verdicts = [
            (1, &both, Verdict::Confirm),
            (2, &first, Verdict::Reject),
            (3, &both, Verdict::Revert),
        ];
        for (n, proposal, verdict) in verdicts {
            let receipt = Receipt {
                id: Ulid::from_parts(2, n),
                proposal: proposal.id,
                proposal_hash: "2".repeat(64),
                before_hash: proposal.before_hash.clone(),
                after_hash: proposal.after_hash.clone(),
                reviewer: "verify-reviewer".into(),
                invocation: "a test".into(),
                verdict,
                notes: String::new(),
                at: at("2026-11-08T03:31:04Z"),
                applied: None,
                snapshot: None,
            };
            let path = hippo.join("receipts").join(format!("{}.json", receipt.id));
            receipt.save(&path).unwrap();
        }

        let expected = "# Session log\n\
            \n\
            ## 2026-11-08\n\
            \n\
            - 03:00 claude-code ab12cd34 — fix the webhook retry — ended 04:15 — turns 3 — \
            notes: [[Engineering/Webhooks]] — handoff: yes — dreaming: 1 confirmed, 1 rejected\n\
            \n\
            ## 2026-11-07\n\
            \n\
            - 22:30 codex codex-01a1 — (no task) — ended 22:41 — turns ? — notes: none — \
            handoff: derived — dreaming: 1 confirmed, 0 rejected\n";
        assert_eq!(render_log(&ws, Some(&index)).unwrap(), expected);
    }

    #[test]
    fn log_lines_show_open_sessions_reasons_and_later_end_days() {
        let dir = tempfile::tempdir().unwrap();
        let ws = workspace(dir.path());
        let task = Some("a task\nover two lines");
        start_at(
            &ws,
            Tool::ClaudeCode,
            "open-1",
            task,
            at("2026-11-08T23:50:00Z"),
        )
        .unwrap();
        start_at(&ws, Tool::Codex, "late-1", None, at("2026-11-08T23:30:00Z")).unwrap();
        end_at(&ws, "late-1", &[], None, None, at("2026-11-09T00:10:00Z")).unwrap();
        let expected = "# Session log\n\
            \n\
            ## 2026-11-08\n\
            \n\
            - 23:50 claude-code open-1 — a task over two lines — ended open — turns ? — \
            notes: none — handoff: no — dreaming: 0 confirmed, 0 rejected\n\
            - 23:30 codex late-1 — (no task) — ended 2026-11-09 00:10 — turns ? — \
            notes: none — handoff: no (unspecified) — dreaming: 0 confirmed, 0 rejected\n";
        assert_eq!(render_log(&ws, None).unwrap(), expected);
    }

    #[test]
    fn get_reads_any_record_by_id() {
        let dir = tempfile::tempdir().unwrap();
        let ws = workspace(dir.path());
        assert_eq!(get(&ws, "ab12").unwrap(), None);
        let started = start_at(&ws, Tool::ClaudeCode, "ab12", None, at(START)).unwrap();
        assert_eq!(get(&ws, "ab12").unwrap(), Some(started));
        derive(&ws, "stub-1", Tool::ClaudeCode, at(START), &conversation()).unwrap();
        assert!(get(&ws, "stub-1").unwrap().unwrap().derived);
        let err = get(&ws, "../x").unwrap_err();
        assert!(matches!(err, Error::InvalidSessionId(_)), "{err}");
    }

    #[test]
    fn write_log_writes_inside_the_workspace_only() {
        let dir = tempfile::tempdir().unwrap();
        let mut ws = workspace(dir.path());
        start_at(&ws, Tool::ClaudeCode, "ab12", None, at(START)).unwrap();
        let path = write_log(&ws, None).unwrap();
        assert_eq!(path, dir.path().join("notes/Session log.md"));
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            render_log(&ws, None).unwrap()
        );
        for outside in ["../Session log.md", "/tmp/Session log.md", ""] {
            ws.log_path = outside.into();
            let err = write_log(&ws, None).unwrap_err();
            assert!(err.to_string().contains("log_path"), "{outside:?}: {err}");
        }
    }

    #[test]
    fn log_rows_hold_the_facts_of_each_line_and_keep_unreadable_records() {
        let dir = tempfile::tempdir().unwrap();
        let ws = workspace(dir.path());
        start_at(
            &ws,
            Tool::ClaudeCode,
            "new-1",
            Some("a task"),
            at("2026-11-08T03:00:00Z"),
        )
        .unwrap();
        end_at(
            &ws,
            "new-1",
            &[PathBuf::from("notes/Topic A.md")],
            Some("see [[Topic A]]"),
            None,
            at("2026-11-08T04:00:00Z"),
        )
        .unwrap();
        start_at(&ws, Tool::Codex, "old-1", None, at("2026-11-07T03:00:00Z")).unwrap();
        end_at(&ws, "old-1", &[], None, None, at("2026-11-07T04:00:00Z")).unwrap();
        let sessions = ws.hippo_dir().join("sessions");
        fs::write(sessions.join("bad.json"), "{").unwrap();

        // One receipt that does not load, and one that does.
        let receipts = ws.hippo_dir().join("receipts");
        fs::create_dir_all(&receipts).unwrap();
        fs::write(receipts.join("half.json"), "").unwrap();

        let rows = log_rows(&ws, None).unwrap();
        let ids: Vec<Option<&str>> = rows
            .iter()
            .map(|l| l.record.as_ref().ok().map(|r| r.id.as_str()))
            .collect();
        assert_eq!(ids, [Some("new-1"), Some("old-1"), None]);
        assert!(rows[2].path.ends_with("bad.json"));
        let first = rows[0].record.as_ref().unwrap();
        assert_eq!(first.tool, Tool::ClaudeCode);
        assert_eq!(first.task, "a task");
        assert_eq!(first.turns, None);
        assert_eq!(first.notes, [PathBuf::from("notes/Topic A.md")]);
        assert_eq!(first.handoff, Handoff::Yes);
        assert_eq!((first.confirmed, first.rejected), (0, 0));
        assert_eq!(
            rows[1].record.as_ref().unwrap().no_change_reason.as_deref(),
            Some("unspecified")
        );

        // render_log still fails on the unreadable session, and on the
        // unreadable receipt once the session is gone.
        assert!(render_log(&ws, None).is_err());
        fs::remove_file(sessions.join("bad.json")).unwrap();
        let err = render_log(&ws, None).unwrap_err();
        assert!(
            err.to_string().contains("EOF") || err.to_string().contains("expected"),
            "{err}"
        );
        assert_eq!(log_rows(&ws, None).unwrap().len(), 2);
    }

    #[test]
    fn log_line_formats_a_row() {
        let dir = tempfile::tempdir().unwrap();
        let ws = workspace(dir.path());
        let row = LogRow {
            id: "ab12".into(),
            tool: Tool::Codex,
            started: at("2026-11-08T03:00:00Z"),
            ended: None,
            task: "one\ntwo".into(),
            turns: Some(7),
            notes: vec![PathBuf::from("notes/Sub/Topic.md")],
            handoff: Handoff::Derived,
            no_change_reason: None,
            derived: false,
            confirmed: 2,
            rejected: 1,
        };
        assert_eq!(
            log_line(&ws, &row),
            "- 03:00 codex ab12 — one two — ended open — turns 7 — notes: [[Sub/Topic]] — \
             handoff: derived — dreaming: 2 confirmed, 1 rejected"
        );
    }

    #[test]
    fn log_row_gives_one_session_or_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let ws = workspace(dir.path());
        assert_eq!(log_row(&ws, "none", Some(3)).unwrap(), None);
        start_at(&ws, Tool::ClaudeCode, "ab12", None, at(START)).unwrap();
        let row = log_row(&ws, "ab12", Some(3)).unwrap().unwrap();
        assert_eq!((row.id.as_str(), row.turns), ("ab12", Some(3)));
        fs::write(record_path(&ws, "bad"), "{").unwrap();
        assert!(log_row(&ws, "bad", None).is_err());
        assert!(matches!(
            log_row(&ws, "../x", None),
            Err(Error::InvalidSessionId(_))
        ));
    }
}
