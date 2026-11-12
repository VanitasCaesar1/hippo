//! Markdown notes: frontmatter and body kept byte-exact, titles, headings,
//! links, the linter, snapshots, and [`apply`], the only path by which
//! dreaming writes a note.
//!
//! Conventions from the design: a note's title is the text of its first
//! body line beginning `# `, or the file stem; its headings are the body
//! lines beginning with one to six `#` and a space; its first body line is
//! the first non-blank body line that is not a heading. A link `[[X]]`,
//! after stripping a `|alias` and then a `#heading` suffix, resolves to
//! `<notes_dir>/X.md` if that note exists, else, when `X` has no `/`, to
//! the unique note named `X.md`; otherwise it dangles.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde::de::DeserializeOwned;
use serde_yaml_ng::{Mapping, Value};
use time::{Date, OffsetDateTime};
use ulid::Ulid;
use walkdir::WalkDir;

use crate::error::{Error, Result};
use crate::records::{Confirmed, ProposalKind};
use crate::util::{hex, sha256, write_atomic, write_new};

/// The note types a frontmatter `type` may have, and a pass's `note_types`
/// may name.
pub const NOTE_TYPES: &[&str] = &["policy", "decision", "log", "evidence", "topic"];

/// The five frontmatter keys hippo reads. Other keys stay in the raw bytes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Frontmatter {
    /// `policy`, `decision`, `log`, `evidence` or `topic`, as written.
    pub r#type: Option<String>,
    /// `current`, `planned` or `superseded`, as written.
    pub status: Option<String>,
    /// `YYYY-MM-DD`.
    pub updated: Option<Date>,
    /// `YYYY-MM-DD`; the note is overdue once this is before today.
    pub review_after: Option<Date>,
    /// Paths the note's claims come from.
    pub sources: Option<Vec<String>>,
}

/// A Markdown note, read byte-exact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    /// The path the note was read from.
    pub path: PathBuf,
    /// The frontmatter block, both `---` lines included; empty without one.
    pub raw_frontmatter: Vec<u8>,
    /// Everything after the frontmatter block.
    pub body: Vec<u8>,
    /// The known keys; `None` when absent, of the wrong shape, or when the
    /// block does not parse.
    pub frontmatter: Frontmatter,
    /// Why the frontmatter block does not parse as YAML, if it does not.
    pub frontmatter_error: Option<String>,
    /// One message per known key of the wrong shape, or for a block that
    /// parses but is not a mapping.
    pub frontmatter_warnings: Vec<String>,
}

impl Note {
    /// Reads a note. A first line `---` (ending `\n` or `\r\n`) opens a
    /// frontmatter block that ends at the next line that is exactly `---`;
    /// without that closing line the whole file is body.
    pub fn read(path: &Path) -> Result<Note> {
        let bytes = fs::read(path).map_err(|e| Error::io_at(path, e))?;
        Ok(Note::parse(path, bytes))
    }

    fn parse(path: &Path, mut bytes: Vec<u8>) -> Note {
        let mut note = Note {
            path: path.to_path_buf(),
            raw_frontmatter: Vec::new(),
            body: Vec::new(),
            frontmatter: Frontmatter::default(),
            frontmatter_error: None,
            frontmatter_warnings: Vec::new(),
        };
        match split_frontmatter(&bytes) {
            Some((yaml, end)) => {
                match parse_frontmatter(&bytes[yaml]) {
                    Ok((frontmatter, warnings)) => {
                        note.frontmatter = frontmatter;
                        note.frontmatter_warnings = warnings;
                    }
                    Err(message) => note.frontmatter_error = Some(message),
                }
                note.body = bytes.split_off(end);
                note.raw_frontmatter = bytes;
            }
            None => note.body = bytes,
        }
        note
    }

    /// Writes `raw_frontmatter` then `body`, byte for byte, atomically.
    /// Tests only: [`apply`] is the one path that writes a note.
    #[cfg(test)]
    pub fn write(&self, path: &Path) -> Result<()> {
        write_atomic(path, &[&self.raw_frontmatter[..], &self.body[..]].concat())
    }

    /// The text of the first body line beginning `# `, else the file stem.
    pub fn title(&self) -> String {
        let body = String::from_utf8_lossy(&self.body);
        match body.lines().find_map(|line| line.strip_prefix("# ")) {
            Some(title) => title.trim().to_owned(),
            None => self
                .path
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_default(),
        }
    }

    /// The text of every body line beginning with one to six `#` and a space.
    pub fn headings(&self) -> Vec<String> {
        let body = String::from_utf8_lossy(&self.body);
        body.lines()
            .filter_map(heading)
            .map(str::to_owned)
            .collect()
    }

    /// The first non-blank body line that is not a heading, trimmed.
    pub fn first_body_line(&self) -> Option<String> {
        let body = String::from_utf8_lossy(&self.body);
        body.lines()
            .find(|line| !line.trim().is_empty() && heading(line).is_none())
            .map(|line| line.trim().to_owned())
    }
}

/// Reads a note, or `None` when it vanished after the scan listed it.
fn read_present(path: &Path) -> Result<Option<Note>> {
    match Note::read(path) {
        Ok(note) => Ok(Some(note)),
        Err(Error::Io(err)) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

/// The YAML byte range inside a leading frontmatter block, and the offset
/// where the block, closing line included, ends.
fn split_frontmatter(bytes: &[u8]) -> Option<(std::ops::Range<usize>, usize)> {
    let open = if bytes.starts_with(b"---\n") {
        4
    } else if bytes.starts_with(b"---\r\n") {
        5
    } else {
        return None;
    };
    let mut start = open;
    while start < bytes.len() {
        let end = bytes[start..]
            .iter()
            .position(|&b| b == b'\n')
            .map_or(bytes.len(), |i| start + i + 1);
        let line = &bytes[start..end];
        let line = line.strip_suffix(b"\n").unwrap_or(line);
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line == b"---" {
            return Some((open..start, end));
        }
        start = end;
    }
    None
}

/// Reads the known keys from a frontmatter block's YAML. Only YAML that
/// does not parse is an error. A known key of the wrong shape reads as
/// absent and adds a warning, as does a block that is not a mapping. A
/// blank or comment-only block has no keys.
fn parse_frontmatter(yaml: &[u8]) -> std::result::Result<(Frontmatter, Vec<String>), String> {
    let text = std::str::from_utf8(yaml).map_err(|e| format!("not UTF-8: {e}"))?;
    let mut frontmatter = Frontmatter::default();
    let mut warnings = Vec::new();
    let blank = text.lines().all(|line| {
        let line = line.trim();
        line.is_empty() || line.starts_with('#')
    });
    if blank {
        return Ok((frontmatter, warnings));
    }
    let mapping = match serde_yaml_ng::from_str::<Value>(text).map_err(|e| e.to_string())? {
        Value::Mapping(mapping) => mapping,
        Value::Null => return Ok((frontmatter, warnings)),
        _ => {
            warnings.push("not a mapping of keys".to_owned());
            return Ok((frontmatter, warnings));
        }
    };
    frontmatter.r#type = known(&mapping, "type", &mut warnings);
    frontmatter.status = known(&mapping, "status", &mut warnings);
    frontmatter.updated = known_date(&mapping, "updated", &mut warnings);
    frontmatter.review_after = known_date(&mapping, "review_after", &mut warnings);
    frontmatter.sources = known(&mapping, "sources", &mut warnings);
    Ok((frontmatter, warnings))
}

/// A known key's value; `None` when it is absent, null or of the wrong
/// shape, and the wrong shape adds a warning.
fn known<T: DeserializeOwned>(
    mapping: &Mapping,
    key: &str,
    warnings: &mut Vec<String>,
) -> Option<T> {
    let value = mapping.get(key).filter(|value| !value.is_null())?;
    match serde_yaml_ng::from_value(value.clone()) {
        Ok(value) => Some(value),
        Err(e) => {
            warnings.push(format!("{key}: {e}"));
            None
        }
    }
}

/// A known date key, written `YYYY-MM-DD`.
fn known_date(mapping: &Mapping, key: &str, warnings: &mut Vec<String>) -> Option<Date> {
    let text: String = known(mapping, key, warnings)?;
    let format = time::macros::format_description!("[year]-[month]-[day]");
    match Date::parse(&text, format) {
        Ok(date) => Some(date),
        Err(_) => {
            warnings.push(format!(
                "{key}: expected a date as YYYY-MM-DD, found {text:?}"
            ));
            None
        }
    }
}

/// A heading line's text: one to six `#`, a space, then the text.
fn heading(line: &str) -> Option<&str> {
    let hashes = line.bytes().take_while(|&b| b == b'#').count();
    let text = line[hashes..].strip_prefix(' ')?;
    (1..=6).contains(&hashes).then_some(text.trim())
}

static LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[\[([^\[\]\n]+)\]\]").expect("the link pattern is valid"));

/// The inner text of every `[[...]]` link in `text`.
pub fn links(text: &str) -> impl Iterator<Item = &str> + '_ {
    LINK.captures_iter(text)
        .filter_map(|captures| captures.get(1))
        .map(|inner| inner.as_str())
}

/// Every `[[...]]` link in `text`, in text order: its byte range, brackets
/// included, and its inner text. Found by the same pattern as [`links`].
pub fn link_spans(text: &str) -> Vec<(std::ops::Range<usize>, &str)> {
    LINK.captures_iter(text)
        .filter_map(|captures| {
            let whole = captures.get(0)?;
            let inner = captures.get(1)?;
            Some((whole.range(), inner.as_str()))
        })
        .collect()
}

/// `path` without `.` components, so that `./brain/a.md` and `brain/a.md`
/// compare equal.
pub fn normalise(path: &Path) -> PathBuf {
    path.components()
        .filter(|c| *c != Component::CurDir)
        .collect()
}

/// `X` from `X`, `X|alias`, `X#heading` or `X#heading|alias`.
fn link_target(text: &str) -> &str {
    let text = text.split_once('|').map_or(text, |(target, _)| target);
    text.split_once('#').map_or(text, |(target, _)| target)
}

/// Where a link points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Link {
    /// The note's path, relative to the notes directory.
    Resolved(PathBuf),
    /// No note, or more than one candidate.
    Dangling,
}

/// Every note under a notes directory.
#[derive(Debug, Clone)]
pub struct Notes {
    /// The notes directory as given to [`Notes::scan`].
    pub root: PathBuf,
    /// The notes by path relative to `root`.
    pub notes: BTreeMap<PathBuf, Note>,
}

impl Notes {
    /// Reads every `*.md` regular file under `notes_dir`. Symlinks are not
    /// followed, and hidden entries, `.hippo/` among them, are skipped. A
    /// file or directory that vanishes during the scan is skipped, since
    /// agents edit notes at any time; a missing `notes_dir` is an error.
    pub fn scan(notes_dir: &Path) -> Result<Notes> {
        let mut notes = BTreeMap::new();
        let entries = WalkDir::new(notes_dir)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| {
                entry.depth() == 0 || !entry.file_name().as_encoded_bytes().starts_with(b".")
            });
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(err)
                    if err.depth() > 0
                        && err
                            .io_error()
                            .is_some_and(|e| e.kind() == io::ErrorKind::NotFound) =>
                {
                    continue
                }
                Err(err) => return Err(io::Error::from(err).into()),
            };
            let path = entry.path();
            if !entry.file_type().is_file() || path.extension().is_none_or(|ext| ext != "md") {
                continue;
            }
            let relative = path
                .strip_prefix(notes_dir)
                .expect("walkdir yields paths under its root");
            if let Some(note) = read_present(path)? {
                notes.insert(relative.to_path_buf(), note);
            }
        }
        Ok(Notes {
            root: notes_dir.to_path_buf(),
            notes,
        })
    }

    /// Resolves a link's inner text, such as `Sub/Topic B|alias`.
    pub fn resolve_link(&self, text: &str) -> Link {
        let target = link_target(text);
        let direct = PathBuf::from(format!("{target}.md"));
        if self.notes.contains_key(&direct) {
            return Link::Resolved(direct);
        }
        if !target.contains('/') {
            let name = format!("{target}.md");
            let mut named = self
                .notes
                .keys()
                .filter(|path| path.file_name().is_some_and(|n| n == name.as_str()));
            if let (Some(only), None) = (named.next(), named.next()) {
                return Link::Resolved(only.clone());
            }
        }
        Link::Dangling
    }

    /// The linter's findings in note path order: an error for frontmatter
    /// that does not parse as YAML, and warnings for missing frontmatter,
    /// each known key of the wrong shape, each dangling link and a
    /// `review_after` before today (UTC).
    pub fn lint(&self) -> Vec<LintFinding> {
        self.lint_on(OffsetDateTime::now_utc().date())
    }

    fn lint_on(&self, today: Date) -> Vec<LintFinding> {
        let mut findings = Vec::new();
        for (path, note) in &self.notes {
            if let Some(message) = &note.frontmatter_error {
                findings.push(LintFinding::Error {
                    note: path.clone(),
                    message: message.clone(),
                });
            } else if note.raw_frontmatter.is_empty() {
                findings.push(LintFinding::MissingFrontmatter { note: path.clone() });
            }
            for message in &note.frontmatter_warnings {
                findings.push(LintFinding::BadFrontmatter {
                    note: path.clone(),
                    message: message.clone(),
                });
            }
            let body = String::from_utf8_lossy(&note.body);
            for link in links(&body) {
                if self.resolve_link(link) == Link::Dangling {
                    findings.push(LintFinding::DanglingLink {
                        note: path.clone(),
                        link: link.to_owned(),
                    });
                }
            }
            if let Some(review_after) = note.frontmatter.review_after.filter(|d| *d < today) {
                findings.push(LintFinding::Overdue {
                    note: path.clone(),
                    review_after,
                });
            }
        }
        findings
    }
}

/// One linter finding. Only [`LintFinding::Error`] is an error; the rest
/// are warnings. Paths are relative to the notes directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LintFinding {
    /// The frontmatter block does not parse as YAML.
    Error { note: PathBuf, message: String },
    /// `missing frontmatter`.
    MissingFrontmatter { note: PathBuf },
    /// The frontmatter parses, but a known key has the wrong shape (it
    /// reads as absent) or the block is not a mapping.
    BadFrontmatter { note: PathBuf, message: String },
    /// `dangling link`: no note, or more than one candidate.
    DanglingLink { note: PathBuf, link: String },
    /// `overdue`: `review_after` is before today.
    Overdue { note: PathBuf, review_after: Date },
}

impl LintFinding {
    /// The note the finding is about, relative to the notes directory.
    pub fn note(&self) -> &Path {
        match self {
            Self::Error { note, .. }
            | Self::MissingFrontmatter { note }
            | Self::BadFrontmatter { note, .. }
            | Self::DanglingLink { note, .. }
            | Self::Overdue { note, .. } => note,
        }
    }

    /// The finding's kind: `error`, `missing-frontmatter`,
    /// `bad-frontmatter`, `dangling-link` or `overdue`.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Error { .. } => "error",
            Self::MissingFrontmatter { .. } => "missing-frontmatter",
            Self::BadFrontmatter { .. } => "bad-frontmatter",
            Self::DanglingLink { .. } => "dangling-link",
            Self::Overdue { .. } => "overdue",
        }
    }

    /// The finding's detail: the message, the link as `[[<link>]]`, the
    /// `review_after` date, or nothing for a missing frontmatter block.
    pub fn detail(&self) -> String {
        match self {
            Self::Error { message, .. } | Self::BadFrontmatter { message, .. } => message.clone(),
            Self::MissingFrontmatter { .. } => String::new(),
            Self::DanglingLink { link, .. } => format!("[[{link}]]"),
            Self::Overdue { review_after, .. } => review_after.to_string(),
        }
    }

    /// Whether this finding is an error rather than a warning.
    pub fn is_error(&self) -> bool {
        matches!(self, LintFinding::Error { .. })
    }
}

impl fmt::Display for LintFinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Error { note, message } => {
                write!(
                    f,
                    "error: {}: frontmatter is not valid YAML: {message}",
                    note.display()
                )
            }
            Self::MissingFrontmatter { note } => {
                write!(f, "warning: {}: missing frontmatter", note.display())
            }
            Self::BadFrontmatter { note, message } => {
                write!(f, "warning: {}: bad frontmatter: {message}", note.display())
            }
            Self::DanglingLink { note, link } => {
                write!(f, "warning: {}: dangling link [[{link}]]", note.display())
            }
            Self::Overdue { note, review_after } => write!(
                f,
                "warning: {}: overdue (review_after {review_after})",
                note.display()
            ),
        }
    }
}

/// Invariant 3, with the rules [`Notes::scan`] applies to notes. `path`,
/// relative to `workspace`, must be relative without `..` and name a
/// visible `*.md` file below `notes_dir` (no component below it starts
/// with `.`). After symlink resolution it must still be such a file: a
/// regular file inside `notes_dir` and not under `.hippo/`.
pub fn validate_proposal_path(workspace: &Path, notes_dir: &Path, path: &Path) -> Result<()> {
    let outside = || Error::PathOutsideNotes(path.to_path_buf());
    if !names_a_note(notes_dir, path) {
        return Err(outside());
    }
    let notes_root = workspace.join(notes_dir);
    let notes_root = fs::canonicalize(&notes_root).map_err(|e| Error::io_at(&notes_root, e))?;
    let hippo_dir = workspace.join(".hippo");
    let hippo_dir = fs::canonicalize(&hippo_dir).unwrap_or(hippo_dir);
    let inside = fs::canonicalize(workspace.join(path)).is_ok_and(|resolved| {
        resolved
            .strip_prefix(&notes_root)
            .is_ok_and(is_visible_markdown)
            && !resolved.starts_with(&hippo_dir)
            && resolved.is_file()
    });
    if inside {
        Ok(())
    } else {
        Err(outside())
    }
}

/// Invariant 3 for a `create`: `path` passes the
/// path rules as written, does not exist, and its parent directory exists
/// inside `notes_dir` after symlink resolution and not under `.hippo/`.
pub fn validate_new_note_path(workspace: &Path, notes_dir: &Path, path: &Path) -> Result<()> {
    let outside = || Error::PathOutsideNotes(path.to_path_buf());
    if !names_a_note(notes_dir, path) {
        return Err(outside());
    }
    let full = workspace.join(path);
    if full.symlink_metadata().is_ok() {
        return Err(Error::InvalidProposal(format!(
            "{} exists already",
            path.display()
        )));
    }
    let notes_root = workspace.join(notes_dir);
    let notes_root = fs::canonicalize(&notes_root).map_err(|e| Error::io_at(&notes_root, e))?;
    let hippo_dir = workspace.join(".hippo");
    let hippo_dir = fs::canonicalize(&hippo_dir).unwrap_or(hippo_dir);
    let parent = full.parent().ok_or_else(outside)?;
    let inside = fs::canonicalize(parent).is_ok_and(|resolved| {
        resolved.starts_with(&notes_root) && !resolved.starts_with(&hippo_dir) && resolved.is_dir()
    });
    if inside {
        Ok(())
    } else {
        Err(outside())
    }
}

/// A snapshot for a `create`: an empty file at the place [`snapshot`]
/// would copy the note to, since the note did not exist.
pub fn snapshot_empty(workspace: &Path, proposal_id: Ulid, note_path: &Path) -> Result<PathBuf> {
    if !is_plain_relative(note_path) {
        return Err(Error::PathOutsideNotes(note_path.to_path_buf()));
    }
    let relative = Path::new(".hippo")
        .join("snapshots")
        .join(proposal_id.to_string())
        .join(note_path);
    let target = workspace.join(&relative);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|e| Error::io_at(parent, e))?;
    }
    write_new(&target, b"")?;
    Ok(relative)
}

/// The path part of [`validate_proposal_path`] that needs no file system:
/// `path` is relative without `..` and names a visible `*.md` file below
/// `notes_dir`.
pub(crate) fn names_a_note(notes_dir: &Path, path: &Path) -> bool {
    is_plain_relative(path)
        && path
            .strip_prefix(normalise(notes_dir))
            .is_ok_and(is_visible_markdown)
}

/// A path whose components are all visible (none starts with `.`) and
/// whose file name ends `.md`.
fn is_visible_markdown(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "md")
        && path.components().all(|c| match c {
            Component::Normal(name) => !name.as_encoded_bytes().starts_with(b"."),
            _ => false,
        })
}

/// Copies the note at `note_path` (relative to `workspace`) to
/// `<workspace>/.hippo/snapshots/<proposal id>/<note path>`, creating
/// parents, and returns that path relative to the workspace, the form a
/// receipt's `snapshot` takes. An existing snapshot is never overwritten.
pub fn snapshot(workspace: &Path, proposal_id: Ulid, note_path: &Path) -> Result<PathBuf> {
    if !is_plain_relative(note_path) {
        return Err(Error::PathOutsideNotes(note_path.to_path_buf()));
    }
    let relative = Path::new(".hippo")
        .join("snapshots")
        .join(proposal_id.to_string())
        .join(note_path);
    let target = workspace.join(&relative);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|e| Error::io_at(parent, e))?;
    }
    let source = workspace.join(note_path);
    let bytes = fs::read(&source).map_err(|e| Error::io_at(&source, e))?;
    write_new(&target, &bytes)?;
    Ok(relative)
}

/// Applies a confirmed proposal to its note. This is the only path by which
/// dreaming writes a note, and it takes a [`Confirmed`], which only a
/// confirming receipt yields.
///
/// Refuses with `PathOutsideNotes` when the note path is absolute or holds
/// `..`, `NoteChanged` when the note no longer hashes to the
/// proposal's `before_hash`, `OldNotUnique` when a `replace`'s `old` does
/// not occur exactly once, and `ReplaceOverCap` when `old` plus `new`
/// exceed half the note's bytes. `append` adds a final newline to a
/// non-empty note that lacks one, then `new`. The write is atomic; returns
/// the new SHA-256.
pub fn apply(workspace: &Path, confirmed: &Confirmed) -> Result<[u8; 32]> {
    let note = confirmed.note();
    if !is_plain_relative(note) {
        return Err(Error::PathOutsideNotes(note.to_path_buf()));
    }
    let path = workspace.join(note);
    if confirmed.kind() == ProposalKind::Create {
        let actual = hex(&sha256(b""));
        if path.exists() || actual != confirmed.before_hash() {
            return Err(Error::NoteChanged {
                expected: confirmed.before_hash().to_owned(),
                actual: fs::read(&path).map(|b| hex(&sha256(&b))).unwrap_or(actual),
            });
        }
        let bytes = confirmed.new_text().as_bytes();
        write_new(&path, bytes)?;
        return Ok(sha256(bytes));
    }
    let current = fs::read(&path).map_err(|e| Error::io_at(&path, e))?;
    let actual = hex(&sha256(&current));
    if actual != confirmed.before_hash() {
        return Err(Error::NoteChanged {
            expected: confirmed.before_hash().to_owned(),
            actual,
        });
    }
    let changed = change(
        &current,
        confirmed.kind(),
        confirmed.old_text().as_bytes(),
        confirmed.new_text().as_bytes(),
    )?;
    write_atomic(&path, &changed)?;
    Ok(sha256(&changed))
}

/// A proposal's change, applied to note bytes in memory. Dreaming's
/// validation uses it to compute `after_hash`.
pub(crate) fn change(note: &[u8], kind: ProposalKind, old: &[u8], new: &[u8]) -> Result<Vec<u8>> {
    match kind {
        ProposalKind::Replace => {
            let (count, first) = occurrences(note, old);
            let at = match (count, first) {
                (1, Some(at)) => at,
                _ => return Err(Error::OldNotUnique(count)),
            };
            let change = old.len() + new.len();
            if change > note.len() / 2 {
                return Err(Error::ReplaceOverCap {
                    change,
                    note: note.len(),
                });
            }
            Ok([&note[..at], new, &note[at + old.len()..]].concat())
        }
        ProposalKind::Create => {
            if !note.is_empty() {
                return Err(Error::InvalidProposal(
                    "a create needs a note that does not exist".to_owned(),
                ));
            }
            Ok(new.to_vec())
        }
        ProposalKind::Append => {
            let mut out = note.to_vec();
            if !out.is_empty() && !out.ends_with(b"\n") {
                out.push(b'\n');
            }
            out.extend_from_slice(new);
            Ok(out)
        }
    }
}

/// How often `old` occurs in `note`, counting a match at every offset so
/// overlapping matches count separately (`aa` occurs twice in `aaa`), and
/// where the first occurrence starts.
pub(crate) fn occurrences(note: &[u8], old: &[u8]) -> (usize, Option<usize>) {
    if old.is_empty() {
        // An empty pattern matches at every offset.
        return (note.len() + 1, Some(0));
    }
    let (mut count, mut first, mut from) = (0, None, 0);
    while let Some(at) = note[from..].windows(old.len()).position(|w| w == old) {
        first.get_or_insert(from + at);
        count += 1;
        from += at + 1;
    }
    (count, first)
}

/// A relative path of normal components only, so it stays inside whatever
/// directory it is joined to.
fn is_plain_relative(path: &Path) -> bool {
    path.components().next().is_some()
        && path.components().all(|c| matches!(c, Component::Normal(_)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::records::{self, Proposal, Receipt, Verdict};
    use time::macros::date;

    const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/notes");

    /// Where `workspace_with` puts its note.
    const NOTE: &str = "brain/Note.md";

    fn fixture(name: &str) -> PathBuf {
        Path::new(FIXTURES).join(name)
    }

    /// A temporary workspace whose only note is `NOTE`.
    fn workspace_with(text: &[u8]) -> tempfile::TempDir {
        let workspace = tempfile::tempdir().unwrap();
        fs::create_dir(workspace.path().join("brain")).unwrap();
        fs::write(workspace.path().join(NOTE), text).unwrap();
        workspace
    }

    /// A `Confirmed` for `note`, made the only way there is: a proposal
    /// file, a receipt confirming it, and `Confirmed::check`.
    fn confirmed_for(
        note: &str,
        kind: ProposalKind,
        old: &str,
        new: &str,
        made_against: &[u8],
    ) -> Confirmed {
        let proposal = Proposal {
            id: Ulid::from_parts(1, 1),
            pass: "verify".into(),
            note: note.into(),
            kind,
            before_hash: hex(&sha256(made_against)),
            after_hash: String::new(),
            old: old.into(),
            new: new.into(),
            evidence: Vec::new(),
            reason: "a test".into(),
        };
        let file = records::encode(&proposal).unwrap();
        let receipt = Receipt {
            id: Ulid::from_parts(1, 2),
            proposal: proposal.id,
            proposal_hash: hex(&sha256(&file)),
            before_hash: proposal.before_hash.clone(),
            after_hash: String::new(),
            reviewer: "verify-reviewer".into(),
            invocation: "a test".into(),
            verdict: Verdict::Confirm,
            notes: String::new(),
            at: OffsetDateTime::UNIX_EPOCH,
            applied: None,
            snapshot: None,
        };
        Confirmed::check(&file, &proposal, &receipt, made_against).unwrap()
    }

    fn confirmed(kind: ProposalKind, old: &str, new: &str, made_against: &[u8]) -> Confirmed {
        confirmed_for(NOTE, kind, old, new, made_against)
    }

    #[test]
    fn read_then_write_is_byte_identity_for_every_fixture() {
        let out = tempfile::tempdir().unwrap();
        let mut checked = 0;
        for entry in WalkDir::new(FIXTURES) {
            let entry = entry.unwrap();
            if !entry.file_type().is_file() {
                continue;
            }
            let copy = out.path().join(format!("{checked}.md"));
            Note::read(entry.path()).unwrap().write(&copy).unwrap();
            let original = fs::read(entry.path()).unwrap();
            assert_eq!(
                fs::read(&copy).unwrap(),
                original,
                "{}",
                entry.path().display()
            );
            checked += 1;
        }
        assert!(checked >= 8, "only {checked} fixtures");
    }

    #[test]
    fn lint_counts_on_the_fixtures() {
        let findings = Notes::scan(Path::new(FIXTURES)).unwrap().lint();
        let count =
            |wanted: fn(&LintFinding) -> bool| findings.iter().filter(|f| wanted(f)).count();
        assert_eq!(
            count(|f| matches!(f, LintFinding::MissingFrontmatter { .. })),
            1
        );
        assert_eq!(count(|f| matches!(f, LintFinding::Overdue { .. })), 1);
        assert_eq!(count(LintFinding::is_error), 0);
        let mut dangling: Vec<(String, &str)> = findings
            .iter()
            .filter_map(|f| match f {
                LintFinding::DanglingLink { note, link } => {
                    Some((note.display().to_string(), link.as_str()))
                }
                _ => None,
            })
            .collect();
        dangling.sort();
        assert_eq!(
            dangling,
            [
                ("START HERE.md".to_owned(), "Missing"),
                ("Topic A.md".to_owned(), "Dup")
            ]
        );
        assert_eq!(findings.len(), 4, "{findings:?}");
    }

    #[test]
    fn reads_the_known_frontmatter_keys_and_conventions() {
        let note = Note::read(&fixture("START HERE.md")).unwrap();
        let expected = Frontmatter {
            r#type: Some("policy".into()),
            status: Some("current".into()),
            updated: Some(date!(2026 - 11 - 07)),
            review_after: Some(date!(2099 - 12 - 31)),
            sources: Some(vec!["Sources/Register.md".into()]),
        };
        assert_eq!(note.frontmatter, expected);
        assert_eq!(note.frontmatter_error, None);
        assert!(note
            .raw_frontmatter
            .ends_with(b"owner: fixture-maintainer\n---\n"));
        assert_eq!(note.title(), "Start here");
        assert_eq!(note.headings(), ["Start here", "Reading order", "Links"]);
        assert_eq!(
            note.first_body_line().as_deref(),
            Some("A synthetic entry note for the parser and linter tests.")
        );

        let bare = Note::read(&fixture("No frontmatter.md")).unwrap();
        assert!(bare.raw_frontmatter.is_empty());
        assert_eq!(bare.frontmatter, Frontmatter::default());
        assert_eq!(bare.title(), "No frontmatter");
    }

    #[test]
    fn crlf_notes_keep_their_line_endings() {
        let bytes = fs::read(fixture("CRLF.md")).unwrap();
        // Fails if git or an editor turned the fixture's CRLF into LF.
        let newlines = bytes.iter().filter(|&&b| b == b'\n').count();
        assert_eq!(bytes.windows(2).filter(|w| w == b"\r\n").count(), newlines);
        let note = Note::read(&fixture("CRLF.md")).unwrap();
        assert!(note.raw_frontmatter.ends_with(b"---\r\n"));
        assert_eq!(note.frontmatter.r#type.as_deref(), Some("log"));
        assert_eq!(note.title(), "CRLF");
        assert_eq!(
            note.first_body_line().as_deref(),
            Some("A synthetic note whose lines all end with CRLF.")
        );
    }

    #[test]
    fn frontmatter_edge_cases() {
        let dir = tempfile::tempdir().unwrap();
        for (name, text) in [
            ("Bad.md", "---\ntype: [unclosed\n---\n# Bad\n"),
            (
                "Date.md",
                "---\nreview_after: soon\nstatus: current\n---\nbody\n",
            ),
            ("Sources.md", "---\nsources: \"x\"\n---\nbody\n"),
            ("Scalar.md", "---\njust text\n---\nbody\n"),
            ("Empty.md", "---\n# a comment only\n---\nbody\n"),
            ("Open.md", "---\nno closing line\n"),
            ("Plain.md", "no heading here\n"),
        ] {
            fs::write(dir.path().join(name), text).unwrap();
        }
        let notes = Notes::scan(dir.path()).unwrap();
        let note = |name: &str| &notes.notes[Path::new(name)];

        assert!(note("Bad.md").frontmatter_error.is_some());
        // A key of the wrong shape reads as absent; the others still parse.
        let date = note("Date.md");
        assert_eq!(date.frontmatter_error, None);
        assert_eq!(date.frontmatter.review_after, None);
        assert_eq!(date.frontmatter.status.as_deref(), Some("current"));
        assert_eq!(note("Sources.md").frontmatter.sources, None);
        assert_eq!(note("Empty.md").frontmatter_error, None);
        assert!(note("Empty.md").frontmatter_warnings.is_empty());
        assert_eq!(note("Empty.md").title(), "Empty");
        assert!(note("Open.md").raw_frontmatter.is_empty());
        assert_eq!(note("Open.md").body, b"---\nno closing line\n");
        assert_eq!(note("Plain.md").title(), "Plain");

        // Only YAML that does not parse is an error; wrong shapes warn.
        let lint = notes.lint();
        let errors: Vec<String> = lint
            .iter()
            .filter(|f| f.is_error())
            .map(LintFinding::to_string)
            .collect();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].starts_with("error: Bad.md: frontmatter is not valid YAML: "));
        let warnings: Vec<String> = lint
            .iter()
            .filter(|f| matches!(f, LintFinding::BadFrontmatter { .. }))
            .map(LintFinding::to_string)
            .collect();
        assert_eq!(warnings.len(), 3, "{warnings:?}");
        assert_eq!(
            warnings[0],
            "warning: Date.md: bad frontmatter: review_after: \
             expected a date as YYYY-MM-DD, found \"soon\""
        );
        assert_eq!(
            warnings[1],
            "warning: Scalar.md: bad frontmatter: not a mapping of keys"
        );
        assert!(
            warnings[2].starts_with("warning: Sources.md: bad frontmatter: sources: "),
            "{}",
            warnings[2]
        );
    }

    #[test]
    fn overdue_means_before_today() {
        let dir = tempfile::tempdir().unwrap();
        let text = "---\nreview_after: 2026-11-08\n---\n";
        fs::write(dir.path().join("Due.md"), text).unwrap();
        let notes = Notes::scan(dir.path()).unwrap();
        assert!(notes.lint_on(date!(2026 - 11 - 08)).is_empty());
        assert_eq!(
            notes.lint_on(date!(2026 - 11 - 09)),
            [LintFinding::Overdue {
                note: "Due.md".into(),
                review_after: date!(2026 - 11 - 08)
            }]
        );
    }

    #[test]
    fn resolves_links_per_the_conventions() {
        let notes = Notes::scan(Path::new(FIXTURES)).unwrap();
        let resolved = |path: &str| Link::Resolved(path.into());
        assert_eq!(notes.resolve_link("Topic A"), resolved("Topic A.md"));
        assert_eq!(
            notes.resolve_link("Sub/Topic B|alias"),
            resolved("Sub/Topic B.md")
        );
        assert_eq!(
            notes.resolve_link("Topic A#Heading"),
            resolved("Topic A.md")
        );
        assert_eq!(
            notes.resolve_link("Topic A#Heading|alias"),
            resolved("Topic A.md")
        );
        assert_eq!(notes.resolve_link("Topic B"), resolved("Sub/Topic B.md"));
        assert_eq!(notes.resolve_link("Sub/Dup"), resolved("Sub/Dup.md"));
        assert_eq!(notes.resolve_link("Dup"), Link::Dangling);
        assert_eq!(notes.resolve_link("Missing"), Link::Dangling);
        assert_eq!(notes.resolve_link("Other/Topic B"), Link::Dangling);

        // A note directly under the notes directory wins over same-named ones.
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("Sub")).unwrap();
        fs::write(dir.path().join("Dup.md"), "# Dup\n").unwrap();
        fs::write(dir.path().join("Sub").join("Dup.md"), "# Dup\n").unwrap();
        let notes = Notes::scan(dir.path()).unwrap();
        assert_eq!(notes.resolve_link("Dup"), resolved("Dup.md"));
    }

    #[test]
    fn scan_reads_visible_markdown_files_only() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "A.md",
            "Sub/B.md",
            ".hidden.md",
            ".hippo/packs/P.md",
            ".git/C.md",
            "notes.txt",
        ] {
            let path = dir.path().join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "# x\n").unwrap();
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            symlink(dir.path().join("A.md"), dir.path().join("Link.md")).unwrap();
            symlink(dir.path().join("Sub"), dir.path().join("Linked")).unwrap();
        }
        let notes = Notes::scan(dir.path()).unwrap();
        let paths: Vec<&Path> = notes.notes.keys().map(PathBuf::as_path).collect();
        assert_eq!(paths, [Path::new("A.md"), Path::new("Sub/B.md")]);
    }

    #[test]
    fn scan_skips_vanished_notes_but_needs_its_root() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_present(&dir.path().join("Gone.md")).unwrap().is_none());
        let missing = Notes::scan(&dir.path().join("missing"));
        assert!(
            matches!(&missing, Err(Error::Io(e)) if e.kind() == io::ErrorKind::NotFound),
            "{missing:?}"
        );
    }

    #[test]
    fn proposal_paths_must_be_notes_inside_notes_dir() {
        let workspace = workspace_with(b"# Note\n");
        let root = workspace.path();
        fs::create_dir(root.join("brain").join("Sub")).unwrap();
        fs::write(root.join("outside.md"), "# Outside\n").unwrap();
        fs::create_dir(root.join(".hippo")).unwrap();
        fs::write(root.join(".hippo").join("x.md"), "# X\n").unwrap();
        fs::create_dir(root.join("brain").join(".obsidian")).unwrap();
        for hidden_or_not_markdown in [".hidden.md", ".obsidian/x.md", "notes.txt"] {
            fs::write(root.join("brain").join(hidden_or_not_markdown), "x\n").unwrap();
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            symlink(root.join("outside.md"), root.join("brain/Escape.md")).unwrap();
            symlink(root.join("brain/.hidden.md"), root.join("brain/Alias.md")).unwrap();
            symlink(root.join(NOTE), root.join("brain/Inside.md")).unwrap();
        }

        let check = |notes_dir: &str, path: &str| {
            validate_proposal_path(root, Path::new(notes_dir), Path::new(path))
        };
        assert!(check("brain", NOTE).is_ok());
        assert!(check(".", NOTE).is_ok());
        assert!(check("./brain", NOTE).is_ok());
        #[cfg(unix)]
        assert!(check("brain", "brain/Inside.md").is_ok());
        for (notes_dir, path) in [
            // Not relative, or climbing out with `..`.
            ("brain", "/etc/hosts"),
            ("brain", "brain/../outside.md"),
            ("brain", "./brain/Note.md"),
            // Not below notes_dir, or not a visible Markdown file there.
            ("brain", "outside.md"),
            ("brain", "brain/.hidden.md"),
            ("brain", "brain/.obsidian/x.md"),
            ("brain", "brain/notes.txt"),
            ("brain", "brain/Sub"),
            (".", ".hippo/x.md"),
            // Missing, or resolving outside notes_dir or to a hidden file.
            ("brain", "brain/Missing.md"),
            ("brain", "brain/Escape.md"),
            ("brain", "brain/Alias.md"),
        ] {
            let result = check(notes_dir, path);
            assert!(
                matches!(result, Err(Error::PathOutsideNotes(_))),
                "{notes_dir} {path}: {result:?}"
            );
        }
    }

    #[test]
    fn snapshot_copies_the_note_under_its_proposal_id() {
        let workspace = workspace_with(b"before\n");
        let id: Ulid = "01JC9X4Q3A8M2K7N5P6R8S9T0V".parse().unwrap();
        let relative = snapshot(workspace.path(), id, Path::new(NOTE)).unwrap();
        assert_eq!(
            relative,
            Path::new(".hippo/snapshots/01JC9X4Q3A8M2K7N5P6R8S9T0V/brain/Note.md")
        );
        assert_eq!(
            fs::read(workspace.path().join(&relative)).unwrap(),
            b"before\n"
        );
        // A second snapshot for the same proposal is refused, not overwritten.
        fs::write(workspace.path().join(NOTE), "after\n").unwrap();
        let again = snapshot(workspace.path(), id, Path::new(NOTE));
        assert!(
            matches!(&again, Err(Error::Io(e)) if e.kind() == io::ErrorKind::AlreadyExists),
            "{again:?}"
        );
        let kept = fs::read(workspace.path().join(&relative)).unwrap();
        assert_eq!(kept, b"before\n");
        let outside = snapshot(workspace.path(), id, Path::new("../x.md"));
        assert!(matches!(outside, Err(Error::PathOutsideNotes(_))));
    }

    #[test]
    fn apply_replaces_or_appends_and_returns_the_new_hash() {
        let text = b"# Webhooks\n\nThe migration ran on 3 November.\n";
        let workspace = workspace_with(text);
        let replace = confirmed(ProposalKind::Replace, "3 November", "7 November", text);
        let hash = apply(workspace.path(), &replace).unwrap();
        let after = fs::read(workspace.path().join(NOTE)).unwrap();
        assert_eq!(after, b"# Webhooks\n\nThe migration ran on 7 November.\n");
        assert_eq!(hash, sha256(&after));
        // The temporary file was renamed away.
        let entries = fs::read_dir(workspace.path().join("brain")).unwrap();
        assert_eq!(entries.count(), 1);

        for before in [&b"# A\nlast line"[..], b"# A\nlast line\n"] {
            let workspace = workspace_with(before);
            let append = confirmed(ProposalKind::Append, "", "## More\n", before);
            apply(workspace.path(), &append).unwrap();
            let after = fs::read(workspace.path().join(NOTE)).unwrap();
            assert_eq!(after, b"# A\nlast line\n## More\n");
        }
        // An empty note gets no leading newline.
        let workspace = workspace_with(b"");
        let append = confirmed(ProposalKind::Append, "", "## More\n", b"");
        apply(workspace.path(), &append).unwrap();
        let after = fs::read(workspace.path().join(NOTE)).unwrap();
        assert_eq!(after, b"## More\n");
    }

    #[test]
    fn apply_refuses_each_error_variant_and_leaves_the_note() {
        let text = b"alpha beta alpha gamma delta epsilon nanana theta\n";
        let workspace = workspace_with(text);
        let root = workspace.path();
        let replace = |old: &str, new: &str| confirmed(ProposalKind::Replace, old, new, text);

        for escape in ["../outside.md", "/etc/hosts"] {
            let outside = confirmed_for(escape, ProposalKind::Replace, "beta", "BETA", text);
            let refused = apply(root, &outside);
            assert!(
                matches!(refused, Err(Error::PathOutsideNotes(_))),
                "{refused:?}"
            );
        }
        let stale = confirmed(ProposalKind::Replace, "beta", "BETA", b"older bytes");
        match apply(root, &stale) {
            Err(Error::NoteChanged { expected, actual }) => {
                assert_eq!(expected, hex(&sha256(b"older bytes")));
                assert_eq!(actual, hex(&sha256(text)));
            }
            other => panic!("expected NoteChanged, got {other:?}"),
        }
        let absent = apply(root, &replace("omega", "OMEGA"));
        assert!(matches!(absent, Err(Error::OldNotUnique(0))), "{absent:?}");
        let twice = apply(root, &replace("alpha", "ALPHA"));
        assert!(matches!(twice, Err(Error::OldNotUnique(2))), "{twice:?}");
        // "nana" overlaps itself in "nanana": two occurrences, not one.
        let overlapping = apply(root, &replace("nana", "NANA"));
        assert!(
            matches!(overlapping, Err(Error::OldNotUnique(2))),
            "{overlapping:?}"
        );
        let empty = apply(root, &replace("", "x"));
        assert!(matches!(empty, Err(Error::OldNotUnique(_))), "{empty:?}");
        // old + new = 11 + 25 = 36 bytes, over half of the note's 50.
        let large = apply(root, &replace("gamma delta", "a much longer replacement"));
        assert!(
            matches!(
                large,
                Err(Error::ReplaceOverCap {
                    change: 36,
                    note: 50
                })
            ),
            "{large:?}"
        );
        assert_eq!(fs::read(root.join(NOTE)).unwrap(), text);

        // Exactly half is allowed: 5 + 20 = 25.
        apply(root, &replace("gamma", "GAMMA, then a little")).unwrap();
    }

    #[test]
    fn link_spans_give_ranges_and_agree_with_links() {
        let text = "see [[A]] and [[B#h|alias]]\n[[not\nlink]] [[C]]";
        let spans = link_spans(text);
        let inner: Vec<&str> = spans.iter().map(|(_, inner)| *inner).collect();
        assert_eq!(inner, ["A", "B#h|alias", "C"]);
        assert_eq!(inner, links(text).collect::<Vec<_>>());
        for (range, inner) in &spans {
            assert_eq!(&text[range.clone()], format!("[[{inner}]]"));
        }
        assert!(link_spans("no links [here]").is_empty());
    }

    #[test]
    fn normalise_drops_current_directory_components() {
        assert_eq!(
            normalise(Path::new("./brain/a.md")),
            Path::new("brain/a.md")
        );
        assert_eq!(
            normalise(Path::new("brain/./a.md")),
            Path::new("brain/a.md")
        );
        assert_eq!(normalise(Path::new(".")), Path::new(""));
        assert_eq!(normalise(Path::new("./a.md")), Path::new("a.md"));
        assert_eq!(normalise(Path::new("../a.md")), Path::new("../a.md"));
    }

    #[test]
    fn lint_findings_name_their_note_kind_and_detail() {
        let note = PathBuf::from("a.md");
        let review_after = time::macros::date!(2026 - 10 - 01);
        let cases = [
            (
                LintFinding::Error {
                    note: note.clone(),
                    message: "bad yaml".into(),
                },
                "error",
                "bad yaml",
            ),
            (
                LintFinding::MissingFrontmatter { note: note.clone() },
                "missing-frontmatter",
                "",
            ),
            (
                LintFinding::BadFrontmatter {
                    note: note.clone(),
                    message: "updated: expected a date".into(),
                },
                "bad-frontmatter",
                "updated: expected a date",
            ),
            (
                LintFinding::DanglingLink {
                    note: note.clone(),
                    link: "Missing".into(),
                },
                "dangling-link",
                "[[Missing]]",
            ),
            (
                LintFinding::Overdue {
                    note: note.clone(),
                    review_after,
                },
                "overdue",
                "2026-10-01",
            ),
        ];
        for (finding, kind, detail) in cases {
            assert_eq!(finding.note(), note);
            assert_eq!(finding.kind(), kind);
            assert_eq!(finding.detail(), detail);
        }
    }
}
