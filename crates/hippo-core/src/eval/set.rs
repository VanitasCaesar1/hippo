//! The eval set: `set.toml`, the questions and the answer key, and the
//! checks that run before anything is built.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use walkdir::WalkDir;

use crate::config::Workspace;
use crate::dream::inputs::{self, ChunkInputs};
use crate::dream::window;
use crate::error::{Error, Result};
use crate::index::Index;
use crate::notes::Notes;
use crate::packs;
use crate::records::Session;
use crate::redact::redact;
use crate::util::{hex, sha256};

/// `set.toml`. A misspelt key is an error, not an ignored field, because
/// it would silently empty a question.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Set {
    pub protocol: String,
    /// The workspace's entry note, relative to the workspace.
    pub entry_note: PathBuf,
    #[serde(default)]
    pub context: Vec<ContextQuestion>,
    #[serde(default)]
    pub recall: Vec<RecallQuestion>,
    /// The dreaming suite's answer key; the suite is skipped without it.
    #[serde(default)]
    pub dreaming: Option<Dreaming>,
}

/// A task and the notes a person says the context block should list.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextQuestion {
    pub task: String,
    /// Workspace-relative note paths; may be empty.
    pub expected: Vec<PathBuf>,
}

/// A query and the turns it should find; any one of them counts.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecallQuestion {
    pub query: String,
    /// What the query is. The report counts hits by it.
    pub kind: RecallKind,
    pub expected: Vec<ExpectedTurn>,
}

/// One turn of one session.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedTurn {
    pub session: String,
    pub ordinal: u32,
}

/// What a recall query is, in the order the report lists the kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecallKind {
    /// Words copied from one turn.
    Verbatim,
    /// Words from a tool call or result.
    Tool,
    /// Words from the Codex rollout.
    Codex,
    /// The meaning of a turn in other words.
    Paraphrase,
}

impl RecallKind {
    /// The kinds in the order the report lists them.
    pub const ALL: [RecallKind; 4] = [
        RecallKind::Verbatim,
        RecallKind::Tool,
        RecallKind::Codex,
        RecallKind::Paraphrase,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            RecallKind::Verbatim => "verbatim",
            RecallKind::Tool => "tool",
            RecallKind::Codex => "codex",
            RecallKind::Paraphrase => "paraphrase",
        }
    }
}

/// The recall question kinds, in the order the report lists them:
/// `verbatim` (words copied from one turn), `tool` (from a tool call or
/// result), `codex` (from the Codex rollout) and `paraphrase` (the meaning
/// of a turn in other words).
pub const KINDS: [&str; 4] = ["verbatim", "tool", "codex", "paraphrase"];
/// The dreaming suite's clock and answer key.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dreaming {
    /// Written to `dream-state.json` before the run.
    #[serde(with = "time::serde::rfc3339")]
    pub last_success: OffsetDateTime,
    /// The dreaming run's clock.
    #[serde(with = "time::serde::rfc3339")]
    pub now: OffsetDateTime,
    #[serde(default)]
    pub error: Vec<PlantedError>,
    #[serde(default)]
    pub keep: Vec<KeptClaim>,
}

/// What makes a planted error easy or hard for `verify`, so the report can
/// count corrections by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ErrorKind {
    /// The evidence turn holds the note's title, a heading or a `sources`
    /// path, so `verify`'s search gathers it, and it is short enough to quote.
    Findable,
    /// The evidence turn holds neither.
    Unfindable,
    /// The evidence turn is too long to quote whole in a `replace`.
    Long,
}

impl ErrorKind {
    /// The kinds in the order the report lists them.
    pub const ALL: [ErrorKind; 3] = [ErrorKind::Findable, ErrorKind::Unfindable, ErrorKind::Long];

    pub fn as_str(self) -> &'static str {
        match self {
            ErrorKind::Findable => "findable",
            ErrorKind::Unfindable => "unfindable",
            ErrorKind::Long => "long",
        }
    }
}

/// A wrong claim a note holds before the run.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlantedError {
    pub note: PathBuf,
    pub kind: ErrorKind,
    /// The wrong text.
    pub stale: String,
    /// After the run the note must hold one of these.
    pub fixed: Vec<String>,
}

/// Correct text that must survive the run.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeptClaim {
    pub note: PathBuf,
    pub text: String,
}

impl Set {
    /// Reads `<dir>/set.toml` and runs the checks that need no index:
    /// the protocol and unknown keys, that every note path names a note
    /// hippo's own scan finds in `<dir>/notes`, and that every recall
    /// question expects a turn and a kind. Before any of that:
    /// no symlink in the set, every transcript starts `{"synthetic": true}`,
    /// and redaction leaves every note, task and query as it is. An error
    /// names the entry.
    pub fn load(dir: &Path) -> Result<Set> {
        check_no_symlinks(dir)?;
        check_synthetic_transcripts(dir)?;
        let path = dir.join("set.toml");
        let text = fs::read_to_string(&path).map_err(|e| Error::io_at(&path, e))?;
        let set: Set = toml::from_str(&text).map_err(|e| bad(format!("set.toml: {e}")))?;
        if set.protocol != crate::PROTOCOL {
            return Err(bad(format!(
                "set.toml: protocol must be {}, found {:?}",
                crate::PROTOCOL,
                set.protocol
            )));
        }

        let notes_dir = dir.join("notes");
        if !notes_dir.is_dir() {
            return Err(bad(format!("{} is not a directory", notes_dir.display())));
        }
        let found: BTreeSet<PathBuf> = Notes::scan(&notes_dir)?
            .notes
            .into_keys()
            .map(|key| Path::new("notes").join(key))
            .collect();
        let check = |entry: String, path: &Path| -> Result<()> {
            if found.contains(path) {
                Ok(())
            } else {
                Err(bad(format!(
                    "{entry}: {} is not a note in the set's notes/",
                    path.display()
                )))
            }
        };
        check("entry_note".into(), &set.entry_note)?;
        if let Some(dreaming) = &set.dreaming {
            for (i, error) in dreaming.error.iter().enumerate() {
                check(
                    format!("dreaming error {} ({})", i + 1, error.stale),
                    &error.note,
                )?;
            }
            for (i, keep) in dreaming.keep.iter().enumerate() {
                check(
                    format!("dreaming keep {} ({})", i + 1, keep.text),
                    &keep.note,
                )?;
            }
        }
        for (i, question) in set.context.iter().enumerate() {
            for path in &question.expected {
                check(
                    format!("context question {} ({})", i + 1, question.task),
                    path,
                )?;
            }
        }
        for (i, question) in set.recall.iter().enumerate() {
            if question.expected.is_empty() {
                return Err(bad(format!(
                    "recall question {} ({}): no expected turn",
                    i + 1,
                    question.query
                )));
            }
        }
        check_redaction(dir, &set)?;
        if let Some(dreaming) = &set.dreaming {
            check_dreaming(dir, dreaming)?;
        }
        Ok(set)
    }

    /// The set's hash: SHA-256 over every file under `dir` in path order,
    /// each as its relative path and its bytes' SHA-256, shortened to 12
    /// hex digits. Hidden files, such as `.DS_Store`, are left out so that
    /// a file manager cannot change it.
    pub fn hash(dir: &Path) -> Result<String> {
        let mut files = Vec::new();
        let entries = WalkDir::new(dir)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| {
                entry.depth() == 0 || !entry.file_name().as_encoded_bytes().starts_with(b".")
            });
        for entry in entries {
            let entry = entry.map_err(std::io::Error::from)?;
            if entry.file_type().is_file() {
                let relative = entry.path().strip_prefix(dir).expect("under its root");
                let relative: Vec<String> = relative
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect();
                files.push((relative.join("/"), entry.into_path()));
            }
        }
        files.sort();
        let mut lines = Vec::new();
        for (relative, path) in files {
            let bytes = fs::read(&path).map_err(|e| Error::io_at(&path, e))?;
            lines.push(format!("{relative} {}\n", hex(&sha256(&bytes))));
        }
        Ok(short_digest(&lines))
    }
}

/// SHA-256 over `lines`, shortened to 12 hex digits.
pub(super) fn short_digest(lines: &[String]) -> String {
    let mut digest = Sha256::new();
    for line in lines {
        digest.update(line);
    }
    hex(&digest.finalize())[..12].to_owned()
}

/// Whether `text` holds `query` as a whole phrase, ignoring case: the
/// character before it and the one after it, where there is one, are not
/// letters or digits. So `queue drain` is not in `queue drained`.
pub fn holds_phrase(text: &str, query: &str) -> bool {
    let (text, query) = (text.to_lowercase(), query.to_lowercase());
    if query.is_empty() {
        return false;
    }
    text.match_indices(&query).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + query.len()..].chars().next();
        !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
    })
}

/// The set directory holds no symlink: a link could lead a copy, or the
/// hash, outside the set.
fn check_no_symlinks(dir: &Path) -> Result<()> {
    for entry in WalkDir::new(dir).follow_links(false) {
        let entry = entry.map_err(std::io::Error::from)?;
        if entry.path_is_symlink() {
            return Err(bad(format!("{} is a symlink", entry.path().display())));
        }
    }
    Ok(())
}

/// Every `*.jsonl` under `transcripts/` starts with `{"synthetic": true}`.
fn check_synthetic_transcripts(dir: &Path) -> Result<()> {
    for entry in WalkDir::new(dir.join("transcripts")).follow_links(false) {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if !entry.file_type().is_file() || path.extension().is_none_or(|ext| ext != "jsonl") {
            continue;
        }
        let text = fs::read_to_string(path).map_err(|e| Error::io_at(path, e))?;
        if text.lines().next() != Some("{\"synthetic\": true}") {
            return Err(bad(format!(
                "{} does not start with {{\"synthetic\": true}}",
                path.strip_prefix(dir).unwrap_or(path).display()
            )));
        }
    }
    Ok(())
}

/// Redaction leaves every note file, context task and recall query as it
/// is, so the text the index and the notes hold is the text the key names.
/// It also leaves every line of every transcript file as it is, the raw
/// text, so what the index does not keep (thinking blocks, directories) is
/// covered too.
fn check_redaction(dir: &Path, set: &Set) -> Result<()> {
    let unchanged = |text: &str| redact(text).as_str() == text;
    for key in Notes::scan(&dir.join("notes"))?.notes.into_keys() {
        let path = dir.join("notes").join(&key);
        let text = fs::read_to_string(&path).map_err(|e| Error::io_at(&path, e))?;
        if !unchanged(&text) {
            return Err(bad(format!("redaction changes notes/{}", key.display())));
        }
    }
    for entry in WalkDir::new(dir.join("transcripts")).follow_links(false) {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let text = fs::read_to_string(path).map_err(|e| Error::io_at(path, e))?;
        if let Some(n) = text.lines().position(|line| !unchanged(line)) {
            return Err(bad(format!(
                "redaction changes {}:{}",
                path.strip_prefix(dir).unwrap_or(path).display(),
                n + 1
            )));
        }
    }
    for (i, question) in set.context.iter().enumerate() {
        if !unchanged(&question.task) {
            return Err(bad(format!(
                "context question {} ({}): redaction changes the task",
                i + 1,
                question.task
            )));
        }
    }
    for (i, question) in set.recall.iter().enumerate() {
        if !unchanged(&question.query) {
            return Err(bad(format!(
                "recall question {} ({}): redaction changes the query",
                i + 1,
                question.query
            )));
        }
    }
    Ok(())
}

/// The checks of the dreaming key that need only the files: the planted
/// errors and kept claims occur as the set says, and every session record
/// loads as a declared record.
fn check_dreaming(dir: &Path, dreaming: &Dreaming) -> Result<()> {
    let text_of = |note: &Path| -> Result<String> {
        let path = dir.join(note);
        fs::read_to_string(&path).map_err(|e| Error::io_at(&path, e))
    };
    // Redaction leaves every string of the key as it is, or the text the
    // key names is not the text the index and the notes hold.
    for (i, error) in dreaming.error.iter().enumerate() {
        let entry = format!("dreaming error {} ({})", i + 1, error.stale);
        if redact(&error.stale).as_str() != error.stale {
            return Err(bad(format!("{entry}: redaction changes the stale text")));
        }
        if error.fixed.iter().any(|f| redact(f).as_str() != f) {
            return Err(bad(format!("{entry}: redaction changes a fixed string")));
        }
    }
    for (i, keep) in dreaming.keep.iter().enumerate() {
        if redact(&keep.text).as_str() != keep.text {
            return Err(bad(format!(
                "dreaming keep {} ({}): redaction changes the text",
                i + 1,
                keep.text
            )));
        }
    }
    // Check 4.
    for (i, error) in dreaming.error.iter().enumerate() {
        let entry = format!("dreaming error {} ({})", i + 1, error.stale);
        let text = text_of(&error.note)?;
        // Counted as `replace` counts them, overlapping matches separately.
        match crate::notes::occurrences(text.as_bytes(), error.stale.as_bytes()).0 {
            1 => {}
            0 => {
                return Err(bad(format!(
                    "{entry}: stale text does not occur in {}",
                    error.note.display()
                )))
            }
            n => {
                return Err(bad(format!(
                "{entry}: stale text occurs {n} times in {}, so no one correction could remove it",
                error.note.display()
            )))
            }
        }
        if error.fixed.is_empty() {
            return Err(bad(format!(
                "{entry}: fixed is empty, so the error could never count as corrected"
            )));
        }
        for fixed in &error.fixed {
            if text.contains(fixed) {
                return Err(bad(format!(
                    "{entry}: fixed text {fixed:?} already occurs in {}",
                    error.note.display()
                )));
            }
        }
    }
    for (i, keep) in dreaming.keep.iter().enumerate() {
        let entry = format!("dreaming keep {} ({})", i + 1, keep.text);
        let text = text_of(&keep.note)?;
        if !text.contains(&keep.text) {
            return Err(bad(format!(
                "{entry}: text does not occur in {}",
                keep.note.display()
            )));
        }
        for error in dreaming.error.iter().filter(|e| e.note == keep.note) {
            if text
                .lines()
                .any(|line| line.contains(&error.stale) && line.contains(&keep.text))
            {
                return Err(bad(format!(
                    "{entry}: a line of {} holds both it and the stale text {:?}",
                    keep.note.display(),
                    error.stale
                )));
            }
        }
    }

    // Every file in sessions/ is a declared session record.
    let sessions = dir.join("sessions");
    if sessions.is_dir() {
        let mut files: Vec<PathBuf> = fs::read_dir(&sessions)
            .map_err(|e| Error::io_at(&sessions, e))?
            .map(|entry| entry.map(|e| e.path()))
            .collect::<std::io::Result<_>>()?;
        files.sort();
        for file in files {
            let name = file
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let record = Session::load(&file)
                .map_err(|e| bad(format!("sessions/{name}: not a session record: {e}")))?;
            if record.derived {
                return Err(bad(format!(
                    "sessions/{name}: must be a declared record, not derived"
                )));
            }
            // Records are looked up as `<id>.json`; a misnamed one would be
            // silently ignored.
            if name != format!("{}.json", record.id) {
                return Err(bad(format!(
                    "sessions/{name}: must be named {}.json after its id",
                    record.id
                )));
            }
            let texts = std::iter::once(&record.task).chain(record.handoff.as_ref());
            if texts.into_iter().any(|text| redact(text).as_str() != text) {
                return Err(bad(format!(
                    "sessions/{name}: redaction changes its task or handoff"
                )));
            }
        }
    }
    Ok(())
}

/// The checks of the dreaming key that need the index, run after ingest.
/// An error names the entry. It returns the clean candidate notes: the
/// notes `verify` takes as candidates in the window that have no planted
/// error.
///
/// 1. Every note a planted error or kept claim names is a candidate in
///    exactly one chunk of the window, by dreaming's own code (`chunks` and
///    `for_chunk`), so `verify` takes it once.
/// 2. Every planted error has an indexed turn that holds a `fixed` string.
/// 3. Each error's `kind` matches what `verify` gathers for its note.
pub(super) fn check_dreaming_index(
    ws: &Workspace,
    index: &Index,
    dreaming: &Dreaming,
) -> Result<BTreeSet<PathBuf>> {
    let last_ordinals = index.last_ordinals()?;
    let mut chunks_of: BTreeMap<PathBuf, usize> = BTreeMap::new();
    for chunk in window::chunks(Some(dreaming.last_success), dreaming.now) {
        for note in inputs::for_chunk(ws, index, &last_ordinals, chunk)?
            .with_observed()
            .notes
            .into_keys()
        {
            *chunks_of.entry(note).or_default() += 1;
        }
    }
    let named = dreaming
        .error
        .iter()
        .enumerate()
        .map(|(i, e)| (format!("dreaming error {} ({})", i + 1, e.stale), &e.note))
        .chain(
            dreaming
                .keep
                .iter()
                .enumerate()
                .map(|(i, k)| (format!("dreaming keep {} ({})", i + 1, k.text), &k.note)),
        );
    for (entry, note) in named {
        match chunks_of.get(note).copied().unwrap_or(0) {
            1 => {}
            0 => {
                return Err(bad(format!(
                    "{entry}: {} is a candidate in no chunk, so verify would never take it",
                    note.display()
                )))
            }
            n => {
                return Err(bad(format!(
                    "{entry}: {} is a candidate in {n} chunks, so verify would take it {n} times",
                    note.display()
                )))
            }
        }
    }

    // An exact scan of every indexed turn, not a search.
    let mut texts = Vec::new();
    for id in last_ordinals.keys() {
        texts.extend(
            index
                .turns_for_session(id)?
                .into_iter()
                .map(|turn| turn.text),
        );
    }
    for (i, error) in dreaming.error.iter().enumerate() {
        let held = error
            .fixed
            .iter()
            .any(|fixed| texts.iter().any(|text| text.contains(fixed)));
        if !held {
            return Err(bad(format!(
                "dreaming error {} ({}): no indexed turn holds any fixed string",
                i + 1,
                error.stale
            )));
        }
    }

    // The declared kind against what verify gathers for the note.
    let passes = packs::load_passes(&ws.root)?;
    let (_, verify) = passes
        .iter()
        .find(|(path, _)| path.file_stem().is_some_and(|stem| stem == "verify"))
        .ok_or_else(|| bad("the core pack has no verify pass".into()))?;
    for (i, error) in dreaming.error.iter().enumerate() {
        let path = ws.root.join(&error.note);
        let note_bytes = fs::metadata(&path)
            .map_err(|e| Error::io_at(&path, e))?
            .len() as usize;
        let gathered = inputs::for_note(
            ws,
            index,
            &ChunkInputs::default(),
            &error.note,
            &verify.reads,
            verify.input_budget_bytes,
            &[],
        )?;
        let holding: Vec<usize> = match gathered {
            Ok(gathered) => gathered
                .turns
                .iter()
                .filter(|turn| error.fixed.iter().any(|fixed| turn.text.contains(fixed)))
                .map(|turn| turn.text.len())
                .collect(),
            Err(_) => Vec::new(),
        };
        let quotable = quotable(&error.stale, &holding, note_bytes);
        let (agrees, found) = match (error.kind, quotable) {
            (ErrorKind::Unfindable, None) | (ErrorKind::Long, Some(false)) => (true, ""),
            (ErrorKind::Findable, Some(true)) => (true, ""),
            (_, None) => (false, "no turn that holds a fixed string"),
            (_, Some(true)) => (
                false,
                "a turn that holds a fixed string and is short enough to quote",
            ),
            (_, Some(false)) => (
                false,
                "a turn that holds a fixed string but is too long to quote",
            ),
        };
        if !agrees {
            return Err(bad(format!(
                "dreaming error {} ({}): its kind is {}, but verify gathers {found}",
                i + 1,
                error.stale,
                error.kind.as_str()
            )));
        }
    }

    Ok(chunks_of
        .into_keys()
        .filter(|note| !dreaming.error.iter().any(|error| error.note == *note))
        .collect())
}

/// Whether the evidence could be quoted in a `replace` of the stale text:
/// `holding` are the sizes of the gathered turns that hold a fixed string.
/// `None` without any. `replace` allows its old and new text half of the
/// note at most, so the stale text and the turn together must fit that.
fn quotable(stale: &str, holding: &[usize], note_bytes: usize) -> Option<bool> {
    (!holding.is_empty()).then(|| {
        holding
            .iter()
            .any(|len| stale.len() + len <= note_bytes / 2)
    })
}

fn bad(message: String) -> Error {
    Error::EvalSet(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPO_SET: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/eval");

    /// A set directory with the notes `notes/A.md` and `notes/.hidden.md`,
    /// and `set.toml` holding `toml`.
    fn set_dir(toml: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("notes")).unwrap();
        fs::write(dir.path().join("notes/A.md"), "# A\n\nText.\n").unwrap();
        fs::write(dir.path().join("notes/.hidden.md"), "# Hidden\n").unwrap();
        fs::write(dir.path().join("set.toml"), toml).unwrap();
        dir
    }

    fn error_of(toml: &str) -> String {
        let dir = set_dir(toml);
        Set::load(dir.path()).unwrap_err().to_string()
    }

    const HEAD: &str = "protocol = \"HIPPO_V1\"\nentry_note = \"notes/A.md\"\n";

    #[test]
    fn a_good_set_loads() {
        let dir = set_dir(&format!(
            "{HEAD}[[context]]\ntask = \"t\"\nexpected = [\"notes/A.md\"]\n\
             [[recall]]\nquery = \"q\"\nkind = \"verbatim\"\n\
             expected = [{{ session = \"s\", ordinal = 2 }}]\n"
        ));
        let set = Set::load(dir.path()).unwrap();
        assert_eq!(set.context[0].expected, [PathBuf::from("notes/A.md")]);
        assert_eq!(set.recall[0].expected[0].ordinal, 2);
    }

    #[test]
    fn check_1_protocol_and_unknown_keys() {
        let wrong = "protocol = \"HIPPO_V0\"\nentry_note = \"notes/A.md\"\n";
        assert!(error_of(wrong).starts_with("eval set: set.toml: protocol must be HIPPO_V1"));
        let key = format!("{HEAD}[[context]]\ntask = \"t\"\nexpect = []\n");
        let message = error_of(&key);
        assert!(message.starts_with("eval set: set.toml:"), "{message}");
        assert!(message.contains("expect"), "{message}");
    }

    #[test]
    fn check_2_paths_name_notes_the_scan_finds() {
        let entry = "protocol = \"HIPPO_V1\"\nentry_note = \"notes/Nope.md\"\n";
        assert_eq!(
            error_of(entry),
            "eval set: entry_note: notes/Nope.md is not a note in the set's notes/"
        );
        // A hidden note exists on disk but is not one the scan finds.
        let hidden = format!(
            "{HEAD}[[context]]\ntask = \"x\"\nexpected = [\"notes/A.md\"]\n\
             [[context]]\ntask = \"find me\"\nexpected = [\"notes/.hidden.md\"]\n"
        );
        assert_eq!(
            error_of(&hidden),
            "eval set: context question 2 (find me): notes/.hidden.md is not a note in the set's notes/"
        );
    }

    #[test]
    fn check_3_a_recall_question_needs_an_expected_turn() {
        let none = format!("{HEAD}[[recall]]\nquery = \"lost\"\nkind = \"tool\"\nexpected = []\n");
        assert_eq!(
            error_of(&none),
            "eval set: recall question 1 (lost): no expected turn"
        );
    }

    /// A set with one note `notes/A.md`, whose first line holds the stale
    /// text "Old text here" beside the claim "Keep both"; `dreaming_toml`
    /// as its `[dreaming]` table; and one declared session record touching
    /// the notes in `touched`.
    fn dream_error(
        dreaming_toml: &str,
        touched: &[&str],
        extra_file: Option<(&str, &str)>,
    ) -> String {
        let dir = set_dir(&format!("{HEAD}{dreaming_toml}"));
        fs::write(
            dir.path().join("notes/A.md"),
            "# A\n\nOld text here and Keep both.\nKeep this one.\n",
        )
        .unwrap();
        fs::create_dir(dir.path().join("sessions")).unwrap();
        let notes: Vec<String> = touched.iter().map(|n| format!("{n:?}")).collect();
        let record = format!(
            "{{\"protocol\":\"HIPPO_V1\",\"id\":\"s1\",\"tool\":\"claude-code\",\"task\":\"\",\
             \"task_seen\":true,\"context_bytes_at_start\":0,\"started\":\"2026-11-01T00:00:00Z\",\
             \"ended\":null,\"notes_touched\":[{}],\"handoff\":null,\"handoff_derived\":false,\
             \"no_change_reason\":null,\"derived\":false}}",
            notes.join(",")
        );
        fs::write(dir.path().join("sessions/s1.json"), record).unwrap();
        if let Some((name, text)) = extra_file {
            fs::write(dir.path().join("sessions").join(name), text).unwrap();
        }
        match Set::load(dir.path()) {
            Ok(_) => "ok".to_owned(),
            Err(e) => e.to_string(),
        }
    }

    const CLOCK: &str =
        "[dreaming]\nlast_success = \"2026-10-31T00:00:00Z\"\nnow = \"2026-11-08T00:00:00Z\"\n";
    const ERROR: &str = "[[dreaming.error]]\nnote = \"notes/A.md\"\nkind = \"findable\"\nstale = \"Old text here\"\nfixed = [\"New text\"]\n";

    #[test]
    fn a_good_dreaming_table_loads() {
        let toml = format!(
            "{CLOCK}{ERROR}[[dreaming.keep]]\nnote = \"notes/A.md\"\ntext = \"Keep this one\"\n"
        );
        assert_eq!(dream_error(&toml, &["notes/A.md"], None), "ok");
    }

    #[test]
    fn check_2_covers_dreaming_notes() {
        let toml = format!("{CLOCK}[[dreaming.keep]]\nnote = \"notes/Nope.md\"\ntext = \"x\"\n");
        assert_eq!(
            dream_error(&toml, &[], None),
            "eval set: dreaming keep 1 (x): notes/Nope.md is not a note in the set's notes/"
        );
        let toml = format!(
            "{CLOCK}[[dreaming.error]]\nnote = \"notes/Nope.md\"\nkind = \"findable\"\nstale = \"s\"\nfixed = []\n"
        );
        assert_eq!(
            dream_error(&toml, &[], None),
            "eval set: dreaming error 1 (s): notes/Nope.md is not a note in the set's notes/"
        );
    }

    #[test]
    fn check_4_stale_keep_and_fixed_text() {
        let touched = ["notes/A.md"];
        let missing = format!(
            "{CLOCK}[[dreaming.error]]\nnote = \"notes/A.md\"\nkind = \"findable\"\nstale = \"Absent\"\nfixed = [\"New\"]\n"
        );
        assert_eq!(
            dream_error(&missing, &touched, None),
            "eval set: dreaming error 1 (Absent): stale text does not occur in notes/A.md"
        );
        let present = format!(
            "{CLOCK}[[dreaming.error]]\nnote = \"notes/A.md\"\nkind = \"findable\"\nstale = \"Old text here\"\nfixed = [\"Keep this\"]\n"
        );
        assert_eq!(
            dream_error(&present, &touched, None),
            "eval set: dreaming error 1 (Old text here): fixed text \"Keep this\" already occurs in notes/A.md"
        );
        let twice = format!(
            "{CLOCK}[[dreaming.error]]\nnote = \"notes/A.md\"\nkind = \"findable\"\nstale = \"Keep\"\nfixed = [\"New\"]\n"
        );
        assert_eq!(
            dream_error(&twice, &touched, None),
            "eval set: dreaming error 1 (Keep): stale text occurs 2 times in notes/A.md, so no one correction could remove it"
        );
        let unfixable = format!(
            "{CLOCK}[[dreaming.error]]\nnote = \"notes/A.md\"\nkind = \"findable\"\nstale = \"Old text here\"\nfixed = []\n"
        );
        assert_eq!(
            dream_error(&unfixable, &touched, None),
            "eval set: dreaming error 1 (Old text here): fixed is empty, so the error could never count as corrected"
        );
        let lost = format!("{CLOCK}[[dreaming.keep]]\nnote = \"notes/A.md\"\ntext = \"Gone\"\n");
        assert_eq!(
            dream_error(&lost, &touched, None),
            "eval set: dreaming keep 1 (Gone): text does not occur in notes/A.md"
        );
        let both = format!(
            "{CLOCK}{ERROR}[[dreaming.keep]]\nnote = \"notes/A.md\"\ntext = \"Keep both\"\n"
        );
        let message = dream_error(&both, &touched, None);
        assert!(
            message.starts_with(
                "eval set: dreaming keep 1 (Keep both): a line of notes/A.md holds both"
            ),
            "{message}"
        );
    }

    #[test]
    fn an_error_kind_is_required_and_one_of_three() {
        let no_kind = format!(
            "{HEAD}{CLOCK}[[dreaming.error]]\nnote = \"notes/A.md\"\nstale = \"x\"\nfixed = [\"y\"]\n"
        );
        assert!(error_of(&no_kind).contains("missing field `kind`"));
        let odd = no_kind.replace("stale", "kind = \"easy\"\nstale");
        assert!(error_of(&odd).contains("unknown variant `easy`"));
    }

    #[test]
    fn redaction_must_leave_the_dreaming_strings_alone() {
        let secret = format!("ghp_{}", "A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q7r8");
        let toml = format!(
            "{CLOCK}[[dreaming.error]]\nnote = \"notes/A.md\"\nkind = \"findable\"\n\
             stale = \"Old text here\"\nfixed = [\"{secret}\"]\n"
        );
        assert_eq!(
            dream_error(&toml, &["notes/A.md"], None),
            "eval set: dreaming error 1 (Old text here): redaction changes a fixed string"
        );
    }

    #[test]
    fn a_session_record_is_named_after_its_id_and_survives_redaction() {
        let record = |id: &str, task: &str, handoff: &str| {
            format!(
                "{{\"protocol\":\"HIPPO_V1\",\"id\":\"{id}\",\"tool\":\"claude-code\",\"task\":\"{task}\",\
                 \"task_seen\":true,\"context_bytes_at_start\":0,\"started\":\"2026-11-01T00:00:00Z\",\
                 \"ended\":null,\"notes_touched\":[],\"handoff\":{handoff},\"handoff_derived\":false,\
                 \"no_change_reason\":null,\"derived\":false}}"
            )
        };
        assert_eq!(
            dream_error(CLOCK, &[], Some(("wrong.json", &record("s9", "", "null")))),
            "eval set: sessions/wrong.json: must be named s9.json after its id"
        );
        let secret = format!("ghp_{}", "A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q7r8");
        assert_eq!(
            dream_error(
                CLOCK,
                &[],
                Some(("s2.json", &record("s2", &secret, "null")))
            ),
            "eval set: sessions/s2.json: redaction changes its task or handoff"
        );
        assert_eq!(
            dream_error(
                CLOCK,
                &[],
                Some(("s2.json", &record("s2", "", &format!("\"{secret}\""))))
            ),
            "eval set: sessions/s2.json: redaction changes its task or handoff"
        );
    }

    #[test]
    fn check_5_session_files_are_declared_records() {
        let message = dream_error(CLOCK, &[], Some(("junk.json", "{}")));
        assert!(
            message.starts_with("eval set: sessions/junk.json: not a session record"),
            "{message}"
        );
        let derived =
            "{\"protocol\":\"HIPPO_V1\",\"id\":\"d1\",\"tool\":\"claude-code\",\"task\":\"\",\
            \"task_seen\":false,\"context_bytes_at_start\":0,\"started\":\"2026-11-01T00:00:00Z\",\
            \"ended\":null,\"notes_touched\":[],\"handoff\":null,\"handoff_derived\":false,\
            \"no_change_reason\":null,\"derived\":true}";
        assert_eq!(
            dream_error(CLOCK, &[], Some(("d1.json", derived))),
            "eval set: sessions/d1.json: must be a declared record, not derived"
        );
    }

    #[test]
    fn evidence_is_quotable_when_it_fits_half_the_note() {
        let stale = "0123456789"; // 10 bytes
        assert_eq!(quotable(stale, &[], 1000), None);
        assert_eq!(quotable(stale, &[490], 1000), Some(true));
        assert_eq!(quotable(stale, &[491], 1000), Some(false));
        // Any one short turn is enough.
        assert_eq!(quotable(stale, &[900, 100], 1000), Some(true));
    }

    #[test]
    fn a_recall_kind_is_one_of_four() {
        let toml = format!(
            "{HEAD}[[recall]]\nquery = \"q\"\nkind = \"verbatum\"\n\
             expected = [{{ session = \"s\", ordinal = 2 }}]\n"
        );
        let message = error_of(&toml);
        assert!(message.starts_with("eval set: set.toml:"), "{message}");
        assert!(message.contains("unknown variant `verbatum`"), "{message}");
    }

    #[test]
    fn a_phrase_must_stand_on_its_own() {
        assert!(holds_phrase("The Queue drained at 14:20", "queue DRAINED"));
        assert!(holds_phrase("queue drained", "queue drained"));
        assert!(holds_phrase("a (queue drain) b", "queue drain"));
        // Letters or digits touch the match on either side.
        assert!(!holds_phrase("the queue drained", "queue drain"));
        assert!(!holds_phrase("the requeue drain", "queue drain"));
        assert!(!holds_phrase("x 3000 MB", "000 MB"));
        // A later whole match still counts.
        assert!(holds_phrase(
            "queue drained, then queue drain.",
            "queue drain"
        ));
        assert!(!holds_phrase("anything", ""));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_in_the_set_is_an_error() {
        let dir = set_dir(HEAD);
        std::os::unix::fs::symlink("A.md", dir.path().join("notes/Link.md")).unwrap();
        let message = Set::load(dir.path()).unwrap_err().to_string();
        assert!(
            message.starts_with("eval set: ") && message.ends_with("Link.md is a symlink"),
            "{message}"
        );
    }

    #[test]
    fn a_transcript_must_start_synthetic() {
        let dir = set_dir(HEAD);
        fs::create_dir_all(dir.path().join("transcripts/codex")).unwrap();
        fs::write(
            dir.path().join("transcripts/codex/ok.jsonl"),
            "{\"synthetic\": true}\n{}\n",
        )
        .unwrap();
        assert!(Set::load(dir.path()).is_ok());
        fs::write(dir.path().join("transcripts/codex/bad.jsonl"), "{}\n").unwrap();
        assert_eq!(
            Set::load(dir.path()).unwrap_err().to_string(),
            "eval set: transcripts/codex/bad.jsonl does not start with {\"synthetic\": true}"
        );
    }

    #[test]
    fn redaction_must_leave_notes_tasks_and_queries_alone() {
        // A made-up key in the shape of a GitHub token.
        let secret = format!("ghp_{}", "A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q7r8");
        let dir = set_dir(HEAD);
        fs::write(
            dir.path().join("notes/A.md"),
            format!("# A\n\ntoken {secret}\n"),
        )
        .unwrap();
        assert_eq!(
            Set::load(dir.path()).unwrap_err().to_string(),
            "eval set: redaction changes notes/A.md"
        );
        fs::write(dir.path().join("notes/A.md"), "# A\n").unwrap();
        // Any line of a transcript, not only the indexed text.
        fs::create_dir_all(dir.path().join("transcripts/x")).unwrap();
        fs::write(
            dir.path().join("transcripts/x/t.jsonl"),
            format!("{{\"synthetic\": true}}\n{{\"cwd\":\"{secret}\"}}\n"),
        )
        .unwrap();
        assert_eq!(
            Set::load(dir.path()).unwrap_err().to_string(),
            "eval set: redaction changes transcripts/x/t.jsonl:2"
        );
        fs::remove_dir_all(dir.path().join("transcripts")).unwrap();
        let task = format!("{HEAD}[[context]]\ntask = \"use {secret}\"\nexpected = []\n");
        assert!(error_of(&task).ends_with(": redaction changes the task"));
        let query = format!(
            "{HEAD}[[recall]]\nquery = \"{secret}\"\nkind = \"tool\"\nexpected = [{{ session = \"s\", ordinal = 1 }}]\n"
        );
        assert!(error_of(&query).ends_with(": redaction changes the query"));
    }

    #[test]
    fn the_hash_follows_every_file_and_skips_hidden_ones() {
        let dir = set_dir(HEAD);
        let before = Set::hash(dir.path()).unwrap();
        assert_eq!(before.len(), 12);
        fs::write(dir.path().join(".DS_Store"), "noise").unwrap();
        assert_eq!(Set::hash(dir.path()).unwrap(), before);
        fs::write(dir.path().join("notes/A.md"), "# A\n\nChanged.\n").unwrap();
        assert_ne!(Set::hash(dir.path()).unwrap(), before);
    }

    #[test]
    fn the_repository_set_loads() {
        let set = Set::load(Path::new(REPO_SET)).unwrap();
        assert_eq!(set.context.len(), 20);
        assert_eq!(set.recall.len(), 20);
        let dreaming = set.dreaming.unwrap();
        assert_eq!((dreaming.error.len(), dreaming.keep.len()), (7, 8));
    }
}
