//! Ingest: walks the transcript roots, redacts every turn, indexes it and
//! derives session records.
//!
//! [`ingest`] takes `$HIPPO_HOME/ingest.lock`; when another ingest holds it
//! the result is [`IngestSummary::AlreadyRunning`] and nothing else is
//! done. Then it lists, for each adapter and each of its roots, every
//! regular file under the root that the adapter claims (a symlink is not
//! followed), and takes them newest first (by modification time, then
//! path), so that a first ingest of months of history has the recent days
//! early. For each file:
//!
//! 1. Files under `$HIPPO_HOME/model-runs/` are never read.
//! 2. The file is hashed (SHA-256, followed by `+<revision>` when the
//!    adapter needs a newer parser for its bytes). Unless
//!    `full`, a file whose hash is the one the index holds for its path is
//!    skipped.
//! 3. Otherwise the adapter parses it, every turn goes through
//!    [`redact`] ([`Turn::from_raw`]),
//!    [`Index::replace_session`] stores the session and
//!    [`sessions::derive`] writes its record when it needs one.
//!
//! A file that cannot be read is skipped with a warning and counted as
//! skipped. A file that cannot be parsed is skipped with a warning too, and
//! counted as skipped, and the index records its hash and the error
//! ([`Index::record_failure`]), so it is not parsed again until it changes
//! or a `full` ingest tries every file; the rows the index already has for
//! it stay. One bad file does not stop an ingest of gigabytes. A failed
//! `derive` is a warning as well. An index error ends the ingest.
//!
//! With `full` the sessions are written into a new index that
//! [`Index::rebuild`] swaps in at the end, and every file is parsed. (That
//! index starts empty, so a file that fails to parse has no old rows in it.)
//! Then [`crate::rollup::refresh`] brings the rollup up to date, a warning
//! when it fails. Without `full` it is also refreshed every
//! [`ROLLUP_EVERY`] files, so a reader can build a home from what is read so
//! far. The last step is `ingest-state.json`. [`ingest_with_progress`]
//! reports each file as it goes ([`Progress`]).

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs::{self, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};

use time::OffsetDateTime;
use walkdir::WalkDir;

use crate::adapters::{self, Adapter, Parsed};
use crate::config::{Config, Workspace};
use crate::error::{Error, Result};
use crate::index::{Index, SessionSource, Turn};
use crate::records::IngestState;
use crate::redact::redact;
use crate::sessions;
use crate::util::{hex, sha256};

/// Held while an ingest runs, in `$HIPPO_HOME`.
const LOCK_FILE: &str = "ingest.lock";
/// The last ingest's time and counts, in `$HIPPO_HOME`.
const STATE_FILE: &str = "ingest-state.json";
/// The working directory of model runs, in `$HIPPO_HOME`; never read.
const MODEL_RUNS_DIR: &str = "model-runs";
/// Files between two refreshes of the rollup during an ingest.
pub const ROLLUP_EVERY: u64 = 250;

/// Where an ingest is, after a file: what the reading screen shows.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Progress {
    /// Files handled so far (indexed or skipped), and in all.
    pub files_done: u64,
    pub files_total: u64,
    /// Their bytes so far, and in all.
    pub bytes_done: u64,
    pub bytes_total: u64,
    /// Files indexed and skipped so far.
    pub indexed: u64,
    pub skipped: u64,
    /// The file just handled, and its modification time.
    pub path: PathBuf,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub modified: Option<OffsetDateTime>,
    /// Whether the rollup was refreshed after this file.
    pub rollup: bool,
}

/// What an ingest did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngestSummary {
    /// Another ingest holds the lock; nothing was done. The CLI prints
    /// `ingest already running` and exits 0.
    AlreadyRunning,
    Done {
        /// Files parsed and written to the index.
        indexed: u64,
        /// Matching files not indexed: unchanged, or unreadable, or not
        /// parseable.
        skipped: u64,
        /// Session records written or updated.
        derived: u64,
        /// Derived records removed because their session does not belong
        /// to the workspace.
        removed: u64,
    },
}

impl fmt::Display for IngestSummary {
    /// `ingest already running`, or
    /// `indexed <n> files, skipped <m>, derived <k> sessions`, then, when
    /// records were removed, a second line saying how many and why.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IngestSummary::AlreadyRunning => f.write_str("ingest already running"),
            IngestSummary::Done {
                indexed,
                skipped,
                derived,
                removed,
            } => {
                write!(
                    f,
                    "indexed {indexed} files, skipped {skipped}, derived {derived} sessions"
                )?;
                if *removed > 0 {
                    write!(
                        f,
                        "\nremoved {removed} session {} of other projects from .hippo/sessions/ (they did not run in this workspace or touch its notes)",
                        if *removed == 1 { "record" } else { "records" }
                    )?;
                }
                Ok(())
            }
        }
    }
}

/// Ingests the transcripts under `cfg.transcript_roots` with the real
/// adapters; see [`ingest_with`]. Without a workspace (a history-only
/// configuration) the index and the rollup are built and no session record
/// is derived.
pub fn ingest(cfg: &Config, ws: Option<&Workspace>, full: bool) -> Result<IngestSummary> {
    ingest_with(cfg, ws, full, &adapters::all())
}

/// [`ingest`], calling `on` after each file.
pub fn ingest_with_progress(
    cfg: &Config,
    ws: Option<&Workspace>,
    full: bool,
    on: &mut dyn FnMut(&Progress),
) -> Result<IngestSummary> {
    run(cfg, ws, full, &adapters::all(), Some(on))
}

/// [`ingest`] with the adapters given, so a test can pass its own. `full`
/// parses every file and builds the index anew; otherwise files whose hash
/// the index already holds are skipped.
pub fn ingest_with(
    cfg: &Config,
    ws: Option<&Workspace>,
    full: bool,
    adapters: &[Box<dyn Adapter>],
) -> Result<IngestSummary> {
    run(cfg, ws, full, adapters, None)
}

fn run(
    cfg: &Config,
    ws: Option<&Workspace>,
    full: bool,
    adapters: &[Box<dyn Adapter>],
    mut on: Option<&mut dyn FnMut(&Progress)>,
) -> Result<IngestSummary> {
    fs::create_dir_all(&cfg.home).map_err(|e| Error::io_at(&cfg.home, e))?;
    let lock_path = cfg.home.join(LOCK_FILE);
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|e| Error::io_at(&lock_path, e))?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => return Ok(IngestSummary::AlreadyRunning),
        Err(TryLockError::Error(err)) => return Err(Error::io_at(&lock_path, err)),
    }
    // From here the lock is held until `lock` drops, at the end.

    let counts = if full {
        Index::rebuild(&cfg.home, |index| {
            walk(cfg, ws, adapters, full, index, &HashMap::new(), on.take())
        })?
    } else {
        let mut index = Index::open(&cfg.home, true)?;
        let known = index.file_hashes()?;
        let counts = walk(cfg, ws, adapters, full, &mut index, &known, on.take())?;
        // A failure recorded for a file no adapter reads any more (such as
        // a Workflow journal, before adapters left it out) is forgotten.
        for failed in index.failed_files()? {
            let path = Path::new(&failed.transcript_path);
            if !adapters.iter().any(|a| a.file_matches(path)) {
                index.forget_failure(&failed.transcript_path)?;
            }
        }
        counts
    };

    // A workspace whose records were written before records were limited to
    // the workspace's own sessions is cleaned once (and again by a full
    // ingest).
    let state_path = cfg.home.join(STATE_FILE);
    let previous = IngestState::load(&state_path).ok();
    let mut counts = counts;
    let mut scoped = previous.as_ref().and_then(|s| s.scoped_workspace.clone());
    if let Some(ws) = ws {
        let root = ws.root.canonicalize().unwrap_or_else(|_| ws.root.clone());
        let root = root.to_string_lossy().into_owned();
        if full || scoped.as_deref() != Some(root.as_str()) {
            let index = Index::open(&cfg.home, false)?;
            counts.removed += sweep(ws, &index, adapters)?;
            scoped = Some(root);
        }
    }

    // The rollup is a cache: a failure to refresh it is a warning, and the
    // next ingest tries again.
    match Index::open(&cfg.home, false)
        .and_then(|index| crate::rollup::refresh(&cfg.home, ws, &index))
    {
        Ok(done) => tracing::debug!(
            updated = done.updated,
            removed = done.removed,
            "rollup refreshed"
        ),
        Err(err) => tracing::warn!("cannot refresh the rollup: {err}"),
    }
    // The usage cache likewise (`usage`): numbers only, for headroom.
    let now = OffsetDateTime::now_utc();
    let offset = crate::usage::local_offset_at(now);
    match crate::usage::refresh(cfg, now, offset) {
        Ok(done) => tracing::debug!(
            read = done.read,
            messages = done.messages,
            "usage refreshed"
        ),
        Err(err) => tracing::warn!("cannot refresh the usage cache: {err}"),
    }

    let state = IngestState {
        last_ingest: OffsetDateTime::now_utc(),
        files_seen: counts.indexed + counts.skipped,
        files_indexed: counts.indexed,
        scoped_workspace: scoped,
    };
    state.save(&state_path)?;
    drop(lock);
    Ok(IngestSummary::Done {
        indexed: counts.indexed,
        skipped: counts.skipped,
        derived: counts.derived,
        removed: counts.removed,
    })
}

#[derive(Debug, Default)]
struct Counts {
    indexed: u64,
    skipped: u64,
    derived: u64,
    removed: u64,
}

/// Removes every derived record in `ws` whose session the index holds and
/// that does not belong to the workspace. A record
/// whose session the index does not hold is kept. Returns how many went.
fn sweep(ws: &Workspace, index: &Index, adapters: &[Box<dyn Adapter>]) -> Result<u64> {
    let scope = sessions::Scope::new(ws)?;
    let mut removed = 0;
    for listed in crate::records::list_sessions(ws)? {
        let Ok(record) = listed.record else {
            continue;
        };
        if !record.derived {
            continue;
        }
        let Some((tool, transcript)) = index.source(&record.id)? else {
            continue;
        };
        let cwd = match adapters.iter().find(|a| a.file_matches(&transcript)) {
            Some(adapter) => adapter.cwd(&record.id, &transcript),
            None => crate::session_meta::origin(&record.id, &tool, &transcript).cwd,
        };
        if !scope.belongs(index, &record.id, cwd.as_deref())?
            && sessions::remove_derived(ws, &record.id)?
        {
            removed += 1;
        }
    }
    Ok(removed)
}

/// Lists every file an adapter claims under its roots, then ingests each,
/// newest first, into `index`. `known` maps the transcript paths the index
/// holds to their hashes. Without `full`, the rollup is refreshed every
/// [`ROLLUP_EVERY`] files.
fn walk(
    cfg: &Config,
    ws: Option<&Workspace>,
    adapters: &[Box<dyn Adapter>],
    full: bool,
    index: &mut Index,
    known: &HashMap<String, String>,
    mut on: Option<&mut dyn FnMut(&Progress)>,
) -> Result<Counts> {
    let model_runs = cfg.home.join(MODEL_RUNS_DIR);
    // A file reached through two overlapping roots is ingested once.
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut files: Vec<Found> = Vec::new();
    for (which, adapter) in adapters.iter().enumerate() {
        for root in adapter.roots(cfg) {
            let entries = WalkDir::new(&root)
                .sort_by_file_name()
                .into_iter()
                .filter_entry(|entry| !entry.path().starts_with(&model_runs));
            for entry in entries {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(err) => {
                        tracing::warn!(root = %root.display(), "cannot walk part of the transcript root: {err}");
                        continue;
                    }
                };
                let path = entry.path();
                if entry.file_type().is_file()
                    && adapter.file_matches(path)
                    && seen.insert(path.to_path_buf())
                {
                    let meta = entry.metadata().ok();
                    files.push(Found {
                        adapter: which,
                        root: root.clone(),
                        path: path.to_path_buf(),
                        bytes: meta.as_ref().map_or(0, |m| m.len()),
                        modified: meta
                            .and_then(|m| m.modified().ok())
                            .map(OffsetDateTime::from),
                    });
                }
            }
        }
    }
    files.sort_by(|a, b| {
        b.modified
            .cmp(&a.modified)
            .then_with(|| a.path.cmp(&b.path))
    });
    let mut progress = Progress {
        files_done: 0,
        files_total: files.len() as u64,
        bytes_done: 0,
        bytes_total: files.iter().map(|f| f.bytes).sum(),
        indexed: 0,
        skipped: 0,
        path: PathBuf::new(),
        modified: None,
        rollup: false,
    };
    let scope = ws.map(sessions::Scope::new).transpose()?;
    let mut run = Run {
        ws,
        scope: scope.as_ref(),
        full,
        known,
        index,
        counts: Counts::default(),
    };
    for found in files {
        run.file(adapters[found.adapter].as_ref(), &found.root, &found.path)?;
        progress.files_done += 1;
        progress.bytes_done += found.bytes;
        progress.indexed = run.counts.indexed;
        progress.skipped = run.counts.skipped;
        progress.path = found.path;
        progress.modified = found.modified;
        progress.rollup = false;
        let last = progress.files_done == progress.files_total;
        if !full && !last && progress.files_done.is_multiple_of(ROLLUP_EVERY) {
            // A cache: a failure here is a warning, as at the end.
            match crate::rollup::refresh(&cfg.home, ws, run.index) {
                Ok(_) => progress.rollup = true,
                Err(err) => tracing::warn!("cannot refresh the rollup: {err}"),
            }
        }
        if let Some(on) = on.as_mut() {
            on(&progress);
        }
    }
    Ok(run.counts)
}

/// A file to ingest, found under one of an adapter's roots.
struct Found {
    adapter: usize,
    root: PathBuf,
    path: PathBuf,
    bytes: u64,
    modified: Option<OffsetDateTime>,
}

struct Run<'a> {
    ws: Option<&'a Workspace>,
    /// Which sessions belong to `ws`.
    scope: Option<&'a sessions::Scope>,
    full: bool,
    known: &'a HashMap<String, String>,
    index: &'a mut Index,
    counts: Counts,
}

impl Run<'_> {
    /// Ingests one transcript file, or skips it.
    fn file(&mut self, adapter: &dyn Adapter, root: &Path, path: &Path) -> Result<()> {
        let hash = match fs::read(path) {
            // A file that needs a newer parser is stored
            // under its hash and that revision, so an index that holds the
            // plain hash reads it again once.
            Ok(bytes) => match adapter.revision(&bytes) {
                Some(rev) => format!("{}+{rev}", hex(&sha256(&bytes))),
                None => hex(&sha256(&bytes)),
            },
            Err(err) => {
                tracing::warn!(path = %path.display(), "skipping unreadable transcript: {err}");
                self.counts.skipped += 1;
                return Ok(());
            }
        };
        let stored = path.to_string_lossy();
        if !self.full && self.known.get(stored.as_ref()) == Some(&hash) {
            self.counts.skipped += 1;
            return Ok(());
        }
        let Parsed {
            id,
            tool,
            started,
            turns,
        } = match adapter.parse(root, path) {
            Ok(parsed) => parsed,
            Err(err) => {
                // A parser's message can quote the transcript: redact it
                // before it goes anywhere.
                let message = redact(&err.to_string());
                tracing::warn!(
                    path = %path.display(),
                    "skipping unparseable transcript: {}",
                    message.as_str()
                );
                self.index
                    .record_failure(adapter.tool(), path, &hash, &message)?;
                self.counts.skipped += 1;
                return Ok(());
            }
        };
        let turns: Vec<Turn> = turns.into_iter().map(Turn::from_raw).collect();
        let source = SessionSource {
            id: &id,
            tool,
            transcript_path: path,
            file_hash: &hash,
        };
        self.index.replace_session(&source, &turns)?;
        self.counts.indexed += 1;
        let (Some(ws), Some(scope)) = (self.ws, self.scope) else {
            return Ok(());
        };
        // Only a session that belongs gets a record in the workspace; one
        // that does not loses a derived record it had.
        let cwd = adapter.cwd(&id, path);
        if !scope.belongs(self.index, &id, cwd.as_deref())? {
            if sessions::is_valid_id(&id) && sessions::remove_derived(ws, &id)? {
                self.counts.removed += 1;
            }
            return Ok(());
        }
        match sessions::derive(ws, &id, tool, started, &turns) {
            Ok(true) => self.counts.derived += 1,
            Ok(false) => {}
            Err(err) => {
                tracing::warn!(session = %id, path = %path.display(), "no record derived: {err}");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::{RawTurn, Role};
    use crate::records::{Session, Tool};
    use std::cell::Cell;
    use std::fs::File;
    use std::io;
    use std::rc::Rc;
    use time::format_description::well_known::Rfc3339;

    /// A transcript format of the test's own. Line 1 is
    /// `<id>\t<tool>\t<started>`, then one `<role>\t<ts>\t<tool_name>\t<text>`
    /// line per turn; a `.fake` file is claimed, anything else is not. Its
    /// errors quote the line they are about, as a JSON parser's can.
    struct Fake {
        roots: Vec<PathBuf>,
        /// How many files `parse` has been called on.
        parses: Rc<Cell<usize>>,
        /// While set, a file named `flaky.fake` does not parse.
        broken: Rc<Cell<bool>>,
        /// While set, a file named `flaky.fake` is not claimed.
        ignored: Rc<Cell<bool>>,
        /// The directory a transcript ran in when its header names none.
        ws: Option<PathBuf>,
    }

    impl Fake {
        fn new(roots: Vec<PathBuf>) -> Fake {
            Fake {
                roots,
                parses: Rc::default(),
                broken: Rc::default(),
                ignored: Rc::default(),
                ws: None,
            }
        }
    }

    impl Adapter for Fake {
        /// The header's fourth field, else the workspace: a test session
        /// runs in the workspace unless it says otherwise.
        fn cwd(&self, _id: &str, path: &Path) -> Option<PathBuf> {
            let text = fs::read_to_string(path).ok()?;
            let header = text.lines().next()?;
            match header.split('\t').nth(3) {
                Some(dir) => Some(PathBuf::from(dir)),
                None => self.ws.clone(),
            }
        }

        fn tool(&self) -> Tool {
            Tool::ClaudeCode
        }

        fn roots(&self, _cfg: &Config) -> Vec<PathBuf> {
            self.roots.clone()
        }

        fn file_matches(&self, path: &Path) -> bool {
            path.extension().is_some_and(|ext| ext == "fake")
                && !(self.ignored.get() && path.file_name().is_some_and(|n| n == "flaky.fake"))
        }

        fn parse(&self, _root: &Path, path: &Path) -> Result<Parsed> {
            let bad = |what: &str| {
                Error::io_at(
                    path,
                    io::Error::new(io::ErrorKind::InvalidData, what.to_owned()),
                )
            };
            self.parses.set(self.parses.get() + 1);
            if self.broken.get() && path.file_name().is_some_and(|name| name == "flaky.fake") {
                return Err(bad("the adapter is broken"));
            }
            let text = fs::read_to_string(path).map_err(|e| Error::io_at(path, e))?;
            let mut lines = text.lines();
            let header = lines.next().ok_or_else(|| bad("empty"))?;
            let mut head = header.split('\t');
            let id = head
                .next()
                .filter(|id| !id.is_empty())
                .ok_or_else(|| bad("no id"))?;
            let tool = match head.next() {
                Some("claude-code") => Tool::ClaudeCode,
                Some("codex") => Tool::Codex,
                Some("dsh") => Tool::Dsh,
                _ => return Err(bad(&format!("no tool in the header {header:?}"))),
            };
            let started = head.next().ok_or_else(|| bad("no start"))?;
            let started =
                OffsetDateTime::parse(started, &Rfc3339).map_err(|e| bad(&e.to_string()))?;
            let mut turns = Vec::new();
            for (ordinal, line) in lines.enumerate() {
                let mut parts = line.splitn(4, '\t');
                let (Some(role), Some(ts), Some(tool_name), Some(text)) =
                    (parts.next(), parts.next(), parts.next(), parts.next())
                else {
                    return Err(bad(&format!("not a turn: {line:?}")));
                };
                turns.push(RawTurn {
                    ordinal: u32::try_from(ordinal).unwrap(),
                    role: match role {
                        "user" => Role::User,
                        "assistant" => Role::Assistant,
                        "tool" => Role::Tool,
                        _ => return Err(bad("no role")),
                    },
                    ts: OffsetDateTime::parse(ts, &Rfc3339).map_err(|e| bad(&e.to_string()))?,
                    tool_name: (!tool_name.is_empty()).then(|| tool_name.to_owned()),
                    text: text.to_owned(),
                });
            }
            Ok(Parsed {
                id: id.to_owned(),
                tool,
                started,
                turns,
            })
        }
    }

    struct Env {
        /// Held for the whole test: see `util::serial`.
        _serial: std::sync::MutexGuard<'static, ()>,
        dir: tempfile::TempDir,
        /// Where the transcripts are.
        root: PathBuf,
        cfg: Config,
        ws: Workspace,
        adapters: Vec<Box<dyn Adapter>>,
        /// The `parses` and `broken` of the adapter in `adapters`.
        parses: Rc<Cell<usize>>,
        broken: Rc<Cell<bool>>,
        ignored: Rc<Cell<bool>>,
    }

    impl Env {
        fn new() -> Env {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().join("transcripts");
            let ws_root = dir.path().join("workspace");
            fs::create_dir_all(&root).unwrap();
            fs::create_dir_all(&ws_root).unwrap();
            let cfg = Config {
                home: dir.path().join("home"),
                workspace: Some(ws_root.clone()),
                transcript_roots: Vec::new(),
                exclude_roots: Vec::new(),
                model_command: Vec::new(),
                ingest_interval_minutes: 10,
                dream_at: "03:30".into(),
                reviewer_command_override: None,
            };
            let ws = Workspace {
                root: ws_root,
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
            };
            let fake = Fake {
                ws: Some(ws.root.clone()),
                ..Fake::new(vec![root.clone()])
            };
            let (parses, broken, ignored) = (
                fake.parses.clone(),
                fake.broken.clone(),
                fake.ignored.clone(),
            );
            let adapters: Vec<Box<dyn Adapter>> = vec![Box::new(fake)];
            Env {
                _serial: crate::util::serial(),
                dir,
                root,
                cfg,
                ws,
                adapters,
                parses,
                broken,
                ignored,
            }
        }

        fn ingest(&self, full: bool) -> IngestSummary {
            ingest_with(&self.cfg, Some(&self.ws), full, &self.adapters).unwrap()
        }

        fn index(&self) -> Index {
            Index::open(&self.cfg.home, false).unwrap()
        }

        fn record_path(&self, id: &str) -> PathBuf {
            self.ws
                .root
                .join(".hippo/sessions")
                .join(format!("{id}.json"))
        }

        /// The three transcripts most tests start from.
        fn three_files(&self) {
            write(
                &self.root.join("a.fake"),
                "sess-a",
                "claude-code",
                &[
                    ("user", "2026-11-08T03:00:00Z", "", "alpha migration plan"),
                    (
                        "assistant",
                        "2026-11-08T03:00:05Z",
                        "",
                        "ok alpha migration",
                    ),
                ],
            );
            write(
                &self.root.join("nested/dir/b.fake"),
                "sess-b",
                "claude-code",
                &[
                    ("user", "2026-11-08T04:00:00Z", "", "beta webhook retry"),
                    (
                        "tool",
                        "2026-11-08T04:00:02Z",
                        "Bash",
                        "curl the beta webhook",
                    ),
                ],
            );
            write(
                &self.root.join("c.fake"),
                "codex-c",
                "codex",
                &[
                    ("user", "2026-11-08T05:00:00Z", "", "gamma question"),
                    (
                        "assistant",
                        "2026-11-08T05:00:09Z",
                        "",
                        "gamma answer, all done",
                    ),
                ],
            );
            fs::write(self.root.join("notes.txt"), "not a transcript").unwrap();
        }

        fn home_files(&self) -> Vec<String> {
            let mut names: Vec<String> = fs::read_dir(&self.cfg.home)
                .unwrap()
                .map(|e| e.unwrap().file_name().into_string().unwrap())
                .collect();
            names.sort();
            names
        }
    }

    fn write(path: &Path, id: &str, tool: &str, turns: &[(&str, &str, &str, &str)]) {
        let mut text = format!("{id}\t{tool}\t2026-11-08T02:59:00Z\n");
        for (role, ts, tool_name, body) in turns {
            text.push_str(&format!("{role}\t{ts}\t{tool_name}\t{body}\n"));
        }
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn done(indexed: u64, skipped: u64, derived: u64) -> IngestSummary {
        IngestSummary::Done {
            indexed,
            skipped,
            derived,
            removed: 0,
        }
    }

    fn sessions_of(hits: &[crate::index::Hit]) -> Vec<(&str, u32)> {
        hits.iter()
            .map(|h| (h.session.as_str(), h.ordinal))
            .collect()
    }

    #[test]
    fn the_summary_prints_as_the_cli_does() {
        assert_eq!(
            IngestSummary::AlreadyRunning.to_string(),
            "ingest already running"
        );
        assert_eq!(
            done(3, 12, 2).to_string(),
            "indexed 3 files, skipped 12, derived 2 sessions"
        );
    }

    #[test]
    fn the_first_ingest_indexes_every_matching_file_and_derives_sessions() {
        let env = Env::new();
        env.three_files();
        assert_eq!(env.ingest(false), done(3, 0, 3));
        // Nothing but the index, the lock, the state and the rollup is left
        // in the home.
        assert_eq!(
            env.home_files(),
            [
                "index.sqlite",
                "ingest-state.json",
                "ingest.lock",
                "rollup.sqlite",
                "usage.sqlite"
            ]
        );
        // The rollup holds every session ingest indexed.
        let rollup = crate::rollup::Rollup::open(&env.cfg.home).unwrap().unwrap();
        let days = rollup.days(time::UtcOffset::UTC).unwrap();
        assert_eq!(days.len(), 1);
        assert_eq!(days[0].sessions, 3);
        assert_eq!(days[0].turns, 6);

        let index = env.index();
        // Both turns match as well as each other, so the newer comes first.
        let hits = index.recall("alpha migration", 20, false).unwrap();
        assert_eq!(sessions_of(&hits), [("sess-a", 1), ("sess-a", 0)]);
        assert_eq!(hits[0].tool, "claude-code");
        let hits = index.recall("beta webhook", 20, false).unwrap();
        assert_eq!(sessions_of(&hits), [("sess-b", 0), ("sess-b", 1)]);
        assert_eq!(hits[1].tool_name.as_deref(), Some("Bash"));
        assert_eq!(index.recall("gamma", 20, false).unwrap()[0].tool, "codex");
        assert_eq!(index.file_hashes().unwrap().len(), 3);

        let a = Session::load(&env.record_path("sess-a")).unwrap();
        assert!(a.derived && a.handoff.is_none());
        // `started` is the adapter's (the header's 02:59:00), not the first
        // turn's; `ended` is the last turn's.
        assert_eq!(
            a.started,
            OffsetDateTime::parse("2026-11-08T02:59:00Z", &Rfc3339).unwrap()
        );
        assert_eq!(
            a.ended,
            Some(OffsetDateTime::parse("2026-11-08T03:00:05Z", &Rfc3339).unwrap())
        );
        let c = Session::load(&env.record_path("codex-c")).unwrap();
        assert_eq!(c.tool, Tool::Codex);
        assert_eq!(c.handoff.as_deref(), Some("gamma answer, all done"));
        assert!(c.handoff_derived);
        assert!(env.record_path("sess-b").is_file());
    }

    #[test]
    fn a_second_ingest_skips_every_file() {
        let env = Env::new();
        env.three_files();
        env.ingest(false);
        assert_eq!(env.ingest(false), done(0, 3, 0));
        assert_eq!(env.index().recall("alpha", 20, false).unwrap().len(), 2);
    }

    #[test]
    fn a_changed_file_is_indexed_again() {
        let env = Env::new();
        env.three_files();
        env.ingest(false);

        // A longer transcript: a new turn and a later end.
        write(
            &env.root.join("a.fake"),
            "sess-a",
            "claude-code",
            &[
                ("user", "2026-11-08T03:00:00Z", "", "alpha migration plan"),
                (
                    "assistant",
                    "2026-11-08T03:00:05Z",
                    "",
                    "ok alpha migration",
                ),
                ("user", "2026-11-08T06:00:00Z", "", "and now delta"),
            ],
        );
        assert_eq!(env.ingest(false), done(1, 2, 1));
        let index = env.index();
        assert_eq!(index.recall("delta", 20, false).unwrap().len(), 1);
        assert_eq!(index.recall("alpha", 20, false).unwrap().len(), 2);
        assert_eq!(index.turns_for_session("sess-a").unwrap().len(), 3);
        assert_eq!(
            Session::load(&env.record_path("sess-a")).unwrap().ended,
            Some(OffsetDateTime::parse("2026-11-08T06:00:00Z", &Rfc3339).unwrap())
        );
        drop(index);

        // Different words, same turns and times: indexed again, but the
        // record it would derive is the one it has.
        write(
            &env.root.join("a.fake"),
            "sess-a",
            "claude-code",
            &[
                ("user", "2026-11-08T03:00:00Z", "", "epsilon migration plan"),
                (
                    "assistant",
                    "2026-11-08T03:00:05Z",
                    "",
                    "ok epsilon migration",
                ),
                ("user", "2026-11-08T06:00:00Z", "", "and now zeta"),
            ],
        );
        assert_eq!(env.ingest(false), done(1, 2, 0));
        let index = env.index();
        assert!(index.recall("alpha", 20, false).unwrap().is_empty());
        assert_eq!(index.recall("epsilon", 20, false).unwrap().len(), 2);
        assert_eq!(index.turns_for_session("sess-a").unwrap().len(), 3);
        assert_eq!(env.ingest(false), done(0, 3, 0));
    }

    #[test]
    fn a_full_ingest_rebuilds_the_index_and_leaves_no_sidecars() {
        let env = Env::new();
        env.three_files();
        env.ingest(false);
        // A transcript that is gone is gone from a rebuilt index; its
        // record stays.
        fs::remove_file(env.root.join("c.fake")).unwrap();

        // Every file is parsed again, and the records they derive are the
        // ones already written.
        assert_eq!(env.ingest(true), done(2, 0, 0));
        assert_eq!(
            env.home_files(),
            [
                "index.sqlite",
                "ingest-state.json",
                "ingest.lock",
                "rollup.sqlite",
                "usage.sqlite"
            ]
        );
        // The rebuilt index lost a session; so did the rollup.
        let rollup = crate::rollup::Rollup::open(&env.cfg.home).unwrap().unwrap();
        assert!(rollup.session("codex-c").unwrap().is_none());
        assert!(rollup.session("sess-a").unwrap().is_some());
        let index = env.index();
        assert_eq!(index.recall("alpha", 20, false).unwrap().len(), 2);
        assert_eq!(index.recall("beta", 20, false).unwrap().len(), 2);
        assert!(index.recall("gamma", 20, false).unwrap().is_empty());
        assert!(!index.exists("codex-c", 0).unwrap());
        assert_eq!(index.file_hashes().unwrap().len(), 2);
        assert!(env.record_path("codex-c").is_file());
        drop(index);
        drop(rollup);

        // The rebuilt index is a working one: an ingest skips what it holds.
        assert_eq!(env.ingest(false), done(0, 2, 0));
        assert_eq!(
            env.home_files(),
            [
                "index.sqlite",
                "ingest-state.json",
                "ingest.lock",
                "rollup.sqlite",
                "usage.sqlite"
            ]
        );
    }

    #[test]
    fn a_full_ingest_builds_an_index_where_there_is_none() {
        let env = Env::new();
        env.three_files();
        assert_eq!(env.ingest(true), done(3, 0, 3));
        assert_eq!(env.index().recall("gamma", 20, false).unwrap().len(), 2);
    }

    #[test]
    fn a_full_ingest_repairs_an_index_of_another_format() {
        let env = Env::new();
        env.three_files();
        env.ingest(false);
        let conn = rusqlite::Connection::open(env.cfg.home.join("index.sqlite")).unwrap();
        conn.pragma_update(None, "user_version", 3).unwrap();
        drop(conn);

        let err = ingest_with(&env.cfg, Some(&env.ws), false, &env.adapters).unwrap_err();
        assert!(err.to_string().contains("run hippo ingest --full"), "{err}");
        assert_eq!(env.ingest(true), done(3, 0, 0));
        assert_eq!(env.ingest(false), done(0, 3, 0));
    }

    #[test]
    fn a_held_lock_means_already_running() {
        let env = Env::new();
        env.three_files();
        fs::create_dir_all(&env.cfg.home).unwrap();
        let held = File::create(env.cfg.home.join("ingest.lock")).unwrap();
        held.try_lock().unwrap();

        for full in [false, true] {
            assert_eq!(env.ingest(full), IngestSummary::AlreadyRunning);
        }
        // Nothing was done: no index, no state, no records.
        assert_eq!(env.home_files(), ["ingest.lock"]);
        assert!(!env.ws.root.join(".hippo").exists());

        held.unlock().unwrap();
        drop(held);
        assert_eq!(env.ingest(false), done(3, 0, 3));
    }

    #[test]
    fn files_under_model_runs_are_skipped() {
        let env = Env::new();
        env.three_files();
        write(
            &env.cfg.home.join("model-runs/nested/run.fake"),
            "model-run",
            "claude-code",
            &[("user", "2026-11-08T07:00:00Z", "", "from a model run")],
        );
        // The root holds both the transcripts and the hippo home.
        let adapters: Vec<Box<dyn Adapter>> = vec![Box::new(Fake {
            ws: Some(env.ws.root.clone()),
            ..Fake::new(vec![env.dir.path().to_path_buf()])
        })];
        for full in [false, true] {
            let summary = ingest_with(&env.cfg, Some(&env.ws), full, &adapters).unwrap();
            assert_eq!(summary, if full { done(3, 0, 0) } else { done(3, 0, 3) });
            let index = env.index();
            assert!(!index.exists("model-run", 0).unwrap());
            assert!(index.recall("model run", 20, false).unwrap().is_empty());
            assert!(!env.record_path("model-run").exists());
        }
        // The same file anywhere else is read.
        fs::rename(
            env.cfg.home.join("model-runs/nested/run.fake"),
            env.root.join("run.fake"),
        )
        .unwrap();
        assert_eq!(env.ingest(false), done(1, 3, 1));
        assert!(env.index().exists("model-run", 0).unwrap());
    }

    #[test]
    fn a_secret_reaches_the_index_only_as_a_redaction() {
        let env = Env::new();
        write(
            &env.root.join("leak.fake"),
            "leaky",
            "codex",
            &[
                (
                    "user",
                    "2026-11-08T03:00:00Z",
                    "",
                    "use aws AKIAFAKEFAKEFAKE0000 please",
                ),
                (
                    "tool",
                    "2026-11-08T03:00:01Z",
                    "Bash",
                    "export AWS=AKIAFAKEFAKEFAKE0000",
                ),
                (
                    "assistant",
                    "2026-11-08T03:00:02Z",
                    "",
                    "done with AKIAFAKEFAKEFAKE0000",
                ),
            ],
        );
        for full in [false, true] {
            env.ingest(full);
            let index = env.index();
            assert!(index
                .recall("AKIAFAKEFAKEFAKE0000", 20, false)
                .unwrap()
                .is_empty());
            assert_eq!(index.recall("redacted aws", 20, false).unwrap().len(), 3);
            let texts: Vec<String> = index
                .turns_for_session("leaky")
                .unwrap()
                .into_iter()
                .map(|t| t.text)
                .collect();
            assert_eq!(
                texts,
                [
                    "use aws [redacted:aws] please",
                    "export AWS=[redacted:aws]",
                    "done with [redacted:aws]"
                ]
            );
            // Not in the file either, nor in the derived handoff.
            let file = fs::read(env.cfg.home.join("index.sqlite")).unwrap();
            assert!(!file.windows(8).any(|w| w == b"AKIAFAKE"));
            let record = fs::read_to_string(env.record_path("leaky")).unwrap();
            assert!(record.contains("done with [redacted:aws]"), "{record}");
            assert!(!record.contains("AKIAFAKE"), "{record}");
        }
    }

    #[test]
    fn ingest_never_overwrites_a_declared_record() {
        let env = Env::new();
        env.three_files();
        let declared = Session {
            id: "sess-a".into(),
            tool: Tool::ClaudeCode,
            task: "fix the webhook retry".into(),
            task_seen: true,
            context_bytes_at_start: 4120,
            started: OffsetDateTime::parse("2026-11-08T02:00:00Z", &Rfc3339).unwrap(),
            ended: None,
            notes_touched: vec!["brain/Engineering/Webhooks.md".into()],
            handoff: None,
            handoff_derived: false,
            no_change_reason: None,
            context_start: None,
            context_prompt: None,
            derived: false,
        };
        declared.save(&env.record_path("sess-a")).unwrap();
        let before = fs::read(env.record_path("sess-a")).unwrap();

        // Two records are derived; sess-a is indexed all the same.
        assert_eq!(env.ingest(false), done(3, 0, 2));
        assert_eq!(fs::read(env.record_path("sess-a")).unwrap(), before);
        assert_eq!(env.index().recall("alpha", 20, false).unwrap().len(), 2);

        // Neither a changed transcript nor a full ingest touches it.
        write(
            &env.root.join("a.fake"),
            "sess-a",
            "claude-code",
            &[("user", "2026-11-08T09:00:00Z", "", "alpha again")],
        );
        assert_eq!(env.ingest(false), done(1, 2, 0));
        assert_eq!(env.ingest(true), done(3, 0, 0));
        assert_eq!(fs::read(env.record_path("sess-a")).unwrap(), before);
        assert_eq!(Session::load(&env.record_path("sess-a")).unwrap(), declared);
    }

    /// A transcript of a session that ran in `/elsewhere`.
    fn write_elsewhere(path: &Path, id: &str, text: &str) {
        let body = format!(
            "{id}\tclaude-code\t2026-11-08T02:59:00Z\t/elsewhere\nuser\t2026-11-08T03:00:00Z\t\t{text}\n"
        );
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    #[test]
    fn only_sessions_that_belong_get_a_record() {
        // Run in the workspace, or naming one of its
        // notes; another project's session is indexed and searchable, and
        // leaves nothing in the workspace.
        let env = Env::new();
        let note = env.ws.root.join("notes/Alpha.md");
        fs::create_dir_all(note.parent().unwrap()).unwrap();
        fs::write(&note, "# Alpha\n").unwrap();
        write(
            &env.root.join("here.fake"),
            "here",
            "claude-code",
            &[("user", "2026-11-08T03:00:00Z", "", "fix the gamma parser")],
        );
        write_elsewhere(&env.root.join("cites.fake"), "cites", "see [[Alpha]] first");
        write_elsewhere(&env.root.join("other.fake"), "other", "zeta deploy script");
        assert_eq!(env.ingest(false), done(3, 0, 2));
        assert!(env.record_path("here").is_file());
        assert!(env.record_path("cites").is_file());
        assert!(!env.record_path("other").exists());
        let counts = env.index().turn_counts().unwrap();
        assert!(counts.contains_key("other"), "{counts:?}");
        let hits = env.index().recall("zeta", 20, false).unwrap();
        assert_eq!(sessions_of(&hits), vec![("other", 0)]);
    }

    #[test]
    fn a_workspace_made_before_is_swept_once_and_declared_records_stay() {
        let env = Env::new();
        write_elsewhere(&env.root.join("old.fake"), "old", "zeta deploy script");
        write_elsewhere(&env.root.join("kept.fake"), "kept", "eta build");
        write(
            &env.root.join("here.fake"),
            "here",
            "claude-code",
            &[("user", "2026-11-08T03:00:00Z", "", "fix the gamma parser")],
        );
        assert_eq!(env.ingest(false), done(3, 0, 1));
        // As an older ingest left it: a derived record for every session,
        // and a declared one for a session elsewhere that ran the hooks.
        let old = Session {
            id: "old".into(),
            tool: Tool::ClaudeCode,
            task: String::new(),
            task_seen: false,
            context_bytes_at_start: 0,
            started: OffsetDateTime::parse("2026-11-08T02:59:00Z", &Rfc3339).unwrap(),
            ended: None,
            notes_touched: Vec::new(),
            handoff: None,
            handoff_derived: false,
            no_change_reason: None,
            context_start: None,
            context_prompt: None,
            derived: true,
        };
        old.save(&env.record_path("old")).unwrap();
        crate::sessions::start(&env.ws, Tool::ClaudeCode, "kept", None).unwrap();
        let state = env.cfg.home.join(STATE_FILE);
        let mut before = IngestState::load(&state).unwrap();
        assert!(before.scoped_workspace.is_some());
        before.scoped_workspace = None;
        before.save(&state).unwrap();

        let summary = env.ingest(false);
        assert_eq!(
            summary,
            IngestSummary::Done {
                indexed: 0,
                skipped: 3,
                derived: 0,
                removed: 1
            }
        );
        assert_eq!(
            summary.to_string(),
            "indexed 0 files, skipped 3, derived 0 sessions\nremoved 1 session record of other projects from .hippo/sessions/ (they did not run in this workspace or touch its notes)"
        );
        assert!(!env.record_path("old").exists());
        assert!(env.record_path("kept").is_file(), "a declared record stays");
        assert!(env.record_path("here").is_file());

        // Once swept, the next ingest does not look again.
        old.save(&env.record_path("old")).unwrap();
        assert_eq!(env.ingest(false), done(0, 3, 0));
        assert!(env.record_path("old").is_file());
        // A full ingest does, and a changed file is judged as it is read.
        assert_eq!(
            env.ingest(true),
            IngestSummary::Done {
                indexed: 3,
                skipped: 0,
                derived: 0,
                removed: 1
            }
        );
        assert!(!env.record_path("old").exists());
    }

    #[test]
    fn the_state_file_holds_the_counts_of_the_last_ingest() {
        let env = Env::new();
        env.three_files();
        let state_path = env.cfg.home.join("ingest-state.json");
        let before = OffsetDateTime::now_utc();
        env.ingest(false);
        let state = IngestState::load(&state_path).unwrap();
        assert_eq!((state.files_seen, state.files_indexed), (3, 3));
        assert!(state.last_ingest >= before - time::Duration::seconds(1));
        assert!(state.last_ingest <= OffsetDateTime::now_utc());

        env.ingest(false);
        let state = IngestState::load(&state_path).unwrap();
        assert_eq!((state.files_seen, state.files_indexed), (3, 0));

        // The raw file: protocol first, the three keys.
        let json: serde_json::Value =
            serde_json::from_slice(&fs::read(&state_path).unwrap()).unwrap();
        assert_eq!(json["protocol"], "HIPPO_V1");
        assert_eq!(json["files_seen"], 3);
        assert_eq!(json["files_indexed"], 0);
        assert!(json["last_ingest"].as_str().unwrap().ends_with('Z'));

        // A run that did nothing because another holds the lock leaves it be.
        let bytes = fs::read(&state_path).unwrap();
        let held = File::open(env.cfg.home.join("ingest.lock")).unwrap();
        held.try_lock().unwrap();
        assert_eq!(env.ingest(false), IngestSummary::AlreadyRunning);
        assert_eq!(fs::read(&state_path).unwrap(), bytes);
    }

    /// The `file_hash` a transcript with these bytes is recorded under.
    fn hash_of(bytes: &[u8]) -> String {
        hex(&sha256(bytes))
    }

    #[test]
    fn a_failing_file_is_recorded_with_its_error_and_skipped_on_the_next_ingest() {
        let env = Env::new();
        env.three_files();
        let bad = env.root.join("a.fake");
        fs::write(&bad, "not a transcript at all").unwrap();

        // b and c are indexed; a fails, counts as skipped, and is recorded.
        assert_eq!(env.ingest(false), done(2, 1, 2));
        assert_eq!(env.parses.get(), 3);
        let index = env.index();
        assert!(!index.exists("sess-a", 0).unwrap());
        let failed = index.failed_files().unwrap();
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].transcript_path, bad.to_string_lossy());
        assert_eq!(failed[0].file_hash, hash_of(b"not a transcript at all"));
        assert!(
            failed[0].error.contains("no tool in the header"),
            "{failed:?}"
        );
        // Its hash is held like any other file's: that is what skips it.
        let hashes = index.file_hashes().unwrap();
        assert_eq!(hashes.len(), 3);
        assert_eq!(hashes[bad.to_str().unwrap()], failed[0].file_hash);
        drop(index);

        // Nothing has changed: no file is parsed again, the bad one included.
        assert_eq!(env.ingest(false), done(0, 3, 0));
        assert_eq!(env.parses.get(), 3);
        let state = IngestState::load(&env.cfg.home.join("ingest-state.json")).unwrap();
        assert_eq!((state.files_seen, state.files_indexed), (3, 0));
        assert_eq!(env.index().failed_files().unwrap().len(), 1);
        assert!(!env.record_path("sess-a").exists());
    }

    #[test]
    fn a_fixed_file_is_indexed_and_its_error_row_is_gone() {
        let env = Env::new();
        env.three_files();
        fs::write(env.root.join("a.fake"), "not a transcript at all").unwrap();
        assert_eq!(env.ingest(false), done(2, 1, 2));

        // The file changes into a transcript: parsed, indexed, derived.
        write(
            &env.root.join("a.fake"),
            "sess-a",
            "claude-code",
            &[("user", "2026-11-08T03:00:00Z", "", "alpha migration plan")],
        );
        assert_eq!(env.ingest(false), done(1, 2, 1));
        assert_eq!(env.parses.get(), 4);
        let index = env.index();
        assert!(index.failed_files().unwrap().is_empty());
        assert_eq!(index.recall("alpha migration", 20, false).unwrap().len(), 1);
        // One row for the file now: the session's.
        let hashes = index.file_hashes().unwrap();
        assert_eq!(hashes.len(), 3);
        let error_rows: i64 = rusqlite::Connection::open(env.cfg.home.join("index.sqlite"))
            .unwrap()
            .query_row(
                "SELECT count(*) FROM sessions WHERE id LIKE 'error:%' OR error IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(error_rows, 0);
        assert!(env.record_path("sess-a").is_file());
        drop(index);
        assert_eq!(env.ingest(false), done(0, 3, 0));
    }

    #[test]
    fn a_file_that_stops_parsing_keeps_its_old_rows() {
        let env = Env::new();
        env.three_files();
        env.ingest(false);

        fs::write(env.root.join("a.fake"), "not a transcript at all").unwrap();
        write(
            &env.root.join("c.fake"),
            "codex-c",
            "codex",
            &[("user", "2026-11-08T05:00:00Z", "", "gamma changed")],
        );
        // a changed and fails; c changed and is indexed; b is as it was.
        assert_eq!(env.ingest(false), done(1, 2, 1));
        let index = env.index();
        assert_eq!(index.recall("alpha migration", 20, false).unwrap().len(), 2);
        assert_eq!(index.recall("gamma changed", 20, false).unwrap().len(), 1);
        assert_eq!(index.turns_for_session("sess-a").unwrap().len(), 2);
        let failed = index.failed_files().unwrap();
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].file_hash, hash_of(b"not a transcript at all"));
        // The failure marks the file's row; it does not add a second one.
        assert_eq!(index.file_hashes().unwrap().len(), 3);
        drop(index);

        // The failure is held: nothing is parsed again.
        let parsed = env.parses.get();
        assert_eq!(env.ingest(false), done(0, 3, 0));
        assert_eq!(env.parses.get(), parsed);
    }

    #[test]
    fn a_failure_is_forgotten_once_no_adapter_claims_the_file() {
        let env = Env::new();
        env.three_files();
        write(
            &env.root.join("flaky.fake"),
            "sess-flaky",
            "claude-code",
            &[("user", "2026-11-08T06:00:00Z", "", "flaky words")],
        );
        env.broken.set(true);
        env.ingest(false);
        assert_eq!(env.index().failed_files().unwrap().len(), 1);
        // The adapter no longer reads such files: the failure goes, and the
        // other sessions stay.
        env.ignored.set(true);
        env.ingest(false);
        let index = env.index();
        assert!(index.failed_files().unwrap().is_empty());
        assert_eq!(index.file_hashes().unwrap().len(), 3);
    }

    #[test]
    fn a_full_ingest_parses_a_failed_file_again() {
        let env = Env::new();
        env.three_files();
        write(
            &env.root.join("flaky.fake"),
            "sess-flaky",
            "claude-code",
            &[("user", "2026-11-08T06:00:00Z", "", "flaky words")],
        );
        env.broken.set(true);
        assert_eq!(env.ingest(false), done(3, 1, 3));
        assert_eq!(env.index().failed_files().unwrap().len(), 1);

        // The adapter is fixed, but the file's bytes are as they were.
        env.broken.set(false);
        assert_eq!(env.ingest(false), done(0, 4, 0));
        assert!(!env.index().exists("sess-flaky", 0).unwrap());

        // A full ingest tries every file, and now it parses.
        assert_eq!(env.ingest(true), done(4, 0, 1));
        let index = env.index();
        assert!(index.failed_files().unwrap().is_empty());
        assert_eq!(index.recall("flaky words", 20, false).unwrap().len(), 1);

        // Still broken, a full ingest records the failure in the new index.
        drop(index);
        env.broken.set(true);
        assert_eq!(env.ingest(true), done(3, 1, 0));
        let index = env.index();
        assert_eq!(index.failed_files().unwrap().len(), 1);
        assert!(!index.exists("sess-flaky", 0).unwrap());
    }

    #[test]
    fn a_failure_is_recorded_redacted() {
        let env = Env::new();
        // The error quotes the line it is about, and the line is a secret.
        fs::write(env.root.join("leaky.fake"), "aws AKIAFAKEFAKEFAKE0000\n").unwrap();
        assert_eq!(env.ingest(false), done(0, 1, 0));
        let failed = env.index().failed_files().unwrap();
        assert_eq!(failed.len(), 1);
        assert!(failed[0].error.contains("[redacted:aws]"), "{failed:?}");
        assert!(!failed[0].error.contains("AKIAFAKE"), "{failed:?}");
        let file = fs::read(env.cfg.home.join("index.sqlite")).unwrap();
        assert!(!file.windows(8).any(|w| w == b"AKIAFAKE"));
    }

    #[test]
    fn a_bad_file_does_not_stop_the_rest_of_a_first_ingest() {
        let env = Env::new();
        env.three_files();
        fs::write(env.root.join("a.fake"), "\tx").unwrap();
        assert_eq!(env.ingest(false), done(2, 1, 2));
        assert!(!env.index().exists("sess-a", 0).unwrap());
        assert!(env.index().exists("sess-b", 0).unwrap());
        assert!(env.index().exists("codex-c", 0).unwrap());
    }

    #[test]
    fn a_file_reached_through_two_roots_is_ingested_once() {
        let env = Env::new();
        env.three_files();
        let adapters: Vec<Box<dyn Adapter>> = vec![
            Box::new(Fake {
                ws: Some(env.ws.root.clone()),
                ..Fake::new(vec![env.root.clone(), env.root.join("nested")])
            }),
            Box::new(Fake {
                ws: Some(env.ws.root.clone()),
                ..Fake::new(vec![env.root.clone()])
            }),
        ];
        for full in [false, true] {
            let summary = ingest_with(&env.cfg, Some(&env.ws), full, &adapters).unwrap();
            assert_eq!(summary, if full { done(3, 0, 0) } else { done(3, 0, 3) });
            assert_eq!(env.index().turns_for_session("sess-b").unwrap().len(), 2);
        }
    }

    #[test]
    fn ingest_without_any_transcripts_still_writes_an_index_and_state() {
        let env = Env::new();
        assert_eq!(env.ingest(false), done(0, 0, 0));
        assert_eq!(env.ingest(true), done(0, 0, 0));
        let state = IngestState::load(&env.cfg.home.join("ingest-state.json")).unwrap();
        assert_eq!((state.files_seen, state.files_indexed), (0, 0));
        assert!(env
            .index()
            .recall("anything", 20, false)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn the_real_adapters_are_the_default() {
        // No transcript roots are configured, so there is nothing to read;
        // this only checks that `ingest` runs end to end with `all()`.
        let env = Env::new();
        let summary = ingest(&env.cfg, Some(&env.ws), false).unwrap();
        assert_eq!(summary, done(0, 0, 0));
    }

    #[test]
    fn progress_reports_each_file_newest_first_and_history_needs_no_workspace() {
        let env = Env::new();
        env.three_files();
        // a.fake is the newest, then b, then c.
        let now = std::time::SystemTime::now();
        for (name, age) in [("a.fake", 0), ("nested/dir/b.fake", 60), ("c.fake", 120)] {
            let path = env.root.join(name);
            if !path.exists() {
                continue;
            }
            let file = File::options().write(true).open(&path).unwrap();
            file.set_modified(now - std::time::Duration::from_secs(age))
                .unwrap();
        }
        let mut seen: Vec<Progress> = Vec::new();
        let summary = run(
            &env.cfg,
            None,
            false,
            &env.adapters,
            Some(&mut |p: &Progress| seen.push(p.clone())),
        )
        .unwrap();
        let IngestSummary::Done {
            indexed, derived, ..
        } = summary
        else {
            panic!("{summary:?}");
        };
        assert_eq!(derived, 0, "no workspace, no records");
        assert_eq!(seen.len() as u64, indexed);
        let last = seen.last().unwrap();
        assert_eq!(last.files_done, last.files_total);
        assert_eq!(last.bytes_done, last.bytes_total);
        assert!(seen[0].path.ends_with("a.fake"), "{:?}", seen[0].path);
        assert!(seen.windows(2).all(|w| w[0].modified >= w[1].modified));
        assert!(!env.ws.root.join(".hippo").exists());
        assert!(env.cfg.home.join("rollup.sqlite").exists());
    }
}
