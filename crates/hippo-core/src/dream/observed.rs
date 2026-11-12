//! Dreaming step 2, observed notes: the notes a session's
//! own transcript shows it editing, discussing or reading, found in its
//! indexed turns rather than in its session record.
//!
//! A turn names a note when a path in it resolves to an existing note
//! under `notes_dir`:
//!
//! - **edit**: the `file_path` of a Claude Code `Edit`, `MultiEdit`,
//!   `Write` or `NotebookEdit` call; a Codex `apply_patch` file (`*** Update
//!   File:` or `*** Add File:`); in a shell command, the target of `>` or
//!   `>>`, the files of `tee`, of `sed -i` and `perl -i`, and the last
//!   argument of `cp`, `mv` and `install`.
//! - **read**: the `file_path` of `Read`, the `path` of `Grep` and `Glob`,
//!   and every other path in a shell command (Claude Code `Bash`, Codex
//!   `shell`, `exec_command`, `local_shell`).
//! - **discuss**: a path or a resolving `[[link]]` in a user or assistant
//!   turn, or in the input of any other tool call (a sub-agent's prompt).
//!
//! Tool results are never read: a listing or a file's contents names paths
//! the session did not choose.
//!
//! An absolute path counts when it is under the workspace root or the root
//! of one of the workspace's git worktrees. A relative path in a tool call
//! resolves against the `cd` before it in the same shell command, else the
//! call's `workdir`, else the session's directory: for Claude Code, the
//! workspace root, a worktree root or a directory directly under the
//! workspace root whose encoded name is the transcript's project directory
//! (`/` and every other character that is not a letter or digit become
//! `-`); for Codex, the `cwd` of the rollout's `session_meta`. A relative
//! path whose directory is unknown does not count, so another project's own
//! `brain/` is not taken for the workspace's. In user, assistant and other
//! prose, a relative path also counts when it names a note relative to the
//! workspace root or to `notes_dir`. The generated session log (`log_path`)
//! is never a candidate.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use crate::index::StoredTurn;
use crate::notes::{links, normalise, Link, Notes};

/// How a turn names a note, weakest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Read,
    Discuss,
    Edit,
}

impl Kind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Kind::Read => "read",
            Kind::Discuss => "discuss",
            Kind::Edit => "edit",
        }
    }
}

/// One note named by one turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observation {
    /// Relative to the workspace: `<notes_dir>/<path below it>`.
    pub note: PathBuf,
    pub kind: Kind,
    pub ordinal: u32,
    /// For an edit, the text it wrote where the call says so (the
    /// `new_string` of an `Edit`, each of a `MultiEdit`); otherwise empty.
    pub written: Vec<String>,
}

/// What dreaming knows about one candidate note in a chunk.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Seen {
    /// The strongest kind seen.
    pub kind: Option<Kind>,
    /// Distinct turns (or session records) that name the note.
    pub evidence: usize,
    /// The sessions that named it.
    pub sessions: BTreeSet<String>,
    /// Text edits wrote into it.
    pub written: Vec<String>,
}

impl Seen {
    /// Counts one more piece of evidence of `kind` from `session`.
    pub fn add(&mut self, session: &str, kind: Kind, written: &[String]) {
        self.kind = self.kind.max(Some(kind));
        self.evidence += 1;
        self.sessions.insert(session.to_owned());
        for text in written {
            if !self.written.contains(text) {
                self.written.push(text.clone());
            }
        }
    }

    /// Adds everything `other` holds.
    pub fn merge(&mut self, other: &Seen) {
        self.kind = self.kind.max(other.kind);
        self.evidence += other.evidence;
        self.sessions.extend(other.sessions.iter().cloned());
        for text in &other.written {
            if !self.written.contains(text) {
                self.written.push(text.clone());
            }
        }
    }
}

/// Orders candidate notes best first: the strongest kind (edit, then
/// discuss, then read), then the most evidence, then the path.
pub fn rank(candidates: &BTreeMap<PathBuf, Seen>) -> Vec<PathBuf> {
    let mut ordered: Vec<(&PathBuf, &Seen)> = candidates.iter().collect();
    ordered.sort_by(|(a_path, a), (b_path, b)| {
        b.kind
            .cmp(&a.kind)
            .then(b.evidence.cmp(&a.evidence))
            .then(a_path.cmp(b_path))
    });
    ordered.into_iter().map(|(path, _)| path.clone()).collect()
}

/// Resolves paths as written in turns to the workspace's notes.
#[derive(Debug, Clone)]
pub struct Resolver<'a> {
    /// The workspace root, as configured and canonicalised, then the roots
    /// of the workspace's other git worktrees.
    roots: Vec<PathBuf>,
    /// Directories a Claude Code session may start in, by encoded name.
    encoded: HashMap<String, PathBuf>,
    notes_dir: PathBuf,
    notes: &'a Notes,
    /// Relative to the workspace; never a candidate.
    exclude: Option<PathBuf>,
}

impl<'a> Resolver<'a> {
    /// A resolver for the workspace at `root` whose notes, `notes`, are
    /// under `notes_dir` (relative to `root`). The worktrees are found from
    /// the workspace's git directory, if it has one.
    pub fn new(
        root: &Path,
        notes_dir: &Path,
        notes: &'a Notes,
        exclude: Option<&Path>,
    ) -> Resolver<'a> {
        let mut roots = vec![root.to_path_buf()];
        if let Ok(canonical) = root.canonicalize() {
            if !roots.contains(&canonical) {
                roots.push(canonical);
            }
        }
        for worktree in worktrees(root) {
            if !roots.contains(&worktree) {
                roots.push(worktree);
            }
        }
        let mut encoded = HashMap::new();
        for dir in &roots {
            encoded.insert(encode(dir), dir.clone());
        }
        if let Ok(entries) = fs::read_dir(root) {
            for entry in entries.flatten() {
                if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                    let dir = entry.path();
                    encoded.entry(encode(&dir)).or_insert(dir);
                }
            }
        }
        Resolver {
            roots,
            encoded,
            notes_dir: normalise(notes_dir),
            notes,
            exclude: exclude.map(normalise),
        }
    }

    /// Whether `dir` is a root (the workspace or one of its worktrees) or
    /// inside one.
    pub fn holds(&self, dir: &Path) -> bool {
        let dir = lexical(dir);
        let canonical = dir.canonicalize().ok();
        self.roots.iter().any(|root| {
            dir.starts_with(root) || canonical.as_ref().is_some_and(|c| c.starts_with(root))
        })
    }

    /// The notes directory under each root, absolute.
    pub fn notes_roots(&self) -> Vec<PathBuf> {
        self.roots.iter().map(|r| r.join(&self.notes_dir)).collect()
    }

    /// The directory session paths resolve against: for `claude-code`, the
    /// directory whose encoded name is the transcript's project directory
    /// (the one after `projects/`); for `codex`, the rollout's `cwd`.
    pub fn session_dir(&self, tool: &str, transcript: &Path) -> Option<PathBuf> {
        match tool {
            "claude-code" => {
                let mut parts = transcript.components().map(|c| c.as_os_str());
                parts.find(|part| *part == "projects")?;
                let project = parts.next()?.to_string_lossy();
                self.encoded.get(project.as_ref()).cloned()
            }
            "codex" => codex_cwd(transcript),
            _ => None,
        }
    }

    /// The note `written` names, relative to the workspace: an absolute
    /// path under a root, or a relative one under `dir`.
    pub fn note_at(&self, written: &str, dir: Option<&Path>) -> Option<PathBuf> {
        let written = written.trim();
        if written.is_empty() {
            return None;
        }
        let path = Path::new(written);
        let absolute = if path.is_absolute() {
            lexical(path)
        } else {
            lexical(&dir?.join(path))
        };
        self.roots
            .iter()
            .find_map(|root| absolute.strip_prefix(root).ok())
            .and_then(|relative| self.note_in_workspace(relative))
    }

    /// For prose: [`Resolver::note_at`], else a relative path naming a note
    /// from the workspace root or from `notes_dir`.
    fn note_in_prose(&self, written: &str, dir: Option<&Path>) -> Option<PathBuf> {
        if let Some(note) = self.note_at(written, dir) {
            return Some(note);
        }
        let path = Path::new(written.trim());
        if path.is_absolute() || path.components().any(|c| c == Component::ParentDir) {
            return None;
        }
        let path = normalise(path);
        self.note_in_workspace(&path)
            .or_else(|| self.note_in_workspace(&self.notes_dir.join(&path)))
    }

    /// `relative`, a path relative to the workspace, when it names an
    /// existing note that may be a candidate.
    fn note_in_workspace(&self, relative: &Path) -> Option<PathBuf> {
        let below = relative.strip_prefix(&self.notes_dir).ok()?;
        if !self.notes.notes.contains_key(below) {
            return None;
        }
        let note = self.notes_dir.join(below);
        (self.exclude.as_ref() != Some(&note)).then_some(note)
    }

    /// The note a `[[link]]` resolves to, relative to the workspace.
    fn note_linked(&self, inner: &str) -> Option<PathBuf> {
        match self.notes.resolve_link(inner) {
            Link::Resolved(below) => self.note_in_workspace(&self.notes_dir.join(below)),
            Link::Dangling => None,
        }
    }
}

/// The notes `turn` names, each once per kind, with `dir` the session's
/// directory ([`Resolver::session_dir`]).
pub fn observe(resolver: &Resolver<'_>, dir: Option<&Path>, turn: &StoredTurn) -> Vec<Observation> {
    let mut found: Vec<(PathBuf, Kind, Vec<String>)> = Vec::new();
    match turn.role.as_str() {
        "user" | "assistant" => prose(resolver, dir, &turn.text, &mut found),
        "tool" => {
            if let Some((name, input)) = call(turn) {
                tool_call(resolver, dir, name, input, &mut found);
            }
        }
        _ => {}
    }
    let mut seen: BTreeMap<(PathBuf, Kind), Vec<String>> = BTreeMap::new();
    for (note, kind, written) in found {
        let entry = seen.entry((note, kind)).or_default();
        for text in written {
            if !entry.contains(&text) {
                entry.push(text);
            }
        }
    }
    seen.into_iter()
        .map(|((note, kind), written)| Observation {
            note,
            kind,
            ordinal: turn.ordinal,
            written,
        })
        .collect()
}

/// A tool turn's name and input, when it is a call: its text starts with
/// its tool name and a space. A result's text does not.
pub(crate) fn call(turn: &StoredTurn) -> Option<(&str, &str)> {
    let name = turn.tool_name.as_deref()?;
    let input = turn.text.strip_prefix(name)?.strip_prefix(' ')?;
    Some((name, input))
}

type Found = Vec<(PathBuf, Kind, Vec<String>)>;

/// Programs that only print, search or edit text: a `session-end` among
/// their arguments is text, not a command run.
const TEXT_TOOLS: [&str; 16] = [
    "echo", "printf", "grep", "egrep", "fgrep", "rg", "ag", "cat", "less", "head", "tail", "sed",
    "awk", "perl", "git", "jq",
];

/// Whether `turn` is a shell call that runs a session-end command:
/// a simple command whose arguments hold the word
/// `session-end`, or the words `session end`, and whose program is not one
/// of [`TEXT_TOOLS`]. A workspace that ends its sessions with its own tool
/// (`mytool session-end`, `hippo session end`) shows it this way.
pub fn ends_session(turn: &StoredTurn) -> bool {
    let Some((name, input)) = call(turn) else {
        return false;
    };
    let value: Option<Value> = serde_json::from_str(input).ok();
    let command = |key: &str| value.as_ref().and_then(|v| v.get(key));
    let commands = match name {
        "Bash" => command("command")
            .and_then(Value::as_str)
            .map(split_commands)
            .unwrap_or_default(),
        "shell" | "local_shell" | "container.exec" | "exec_command" => {
            match command("command").or_else(|| command("cmd")) {
                Some(Value::String(script)) => split_commands(script),
                Some(Value::Array(argv)) => {
                    let argv: Vec<&str> = argv.iter().filter_map(Value::as_str).collect();
                    match argv.as_slice() {
                        [program, flag, script]
                            if is_shell(program) && flag.starts_with('-') && flag.contains('c') =>
                        {
                            split_commands(script)
                        }
                        words => vec![words.iter().map(|w| Word::plain((*w).to_owned())).collect()],
                    }
                }
                _ => Vec::new(),
            }
        }
        _ => Vec::new(),
    };
    commands.iter().any(|words| runs_session_end(words))
}

/// Whether one simple command runs a session end: see [`ends_session`].
fn runs_session_end(words: &[Word]) -> bool {
    let words: Vec<&str> = words
        .iter()
        .map(|w| w.text.as_str())
        .skip_while(|w| is_assignment(w))
        .collect();
    let Some((program, args)) = words.split_first() else {
        return false;
    };
    let name = program.rsplit('/').next().unwrap_or(program);
    if TEXT_TOOLS.contains(&name) {
        return false;
    }
    args.contains(&"session-end") || args.windows(2).any(|w| w == ["session", "end"])
}

fn tool_call(
    resolver: &Resolver<'_>,
    dir: Option<&Path>,
    name: &str,
    input: &str,
    out: &mut Found,
) {
    let value: Option<Value> = serde_json::from_str(input).ok();
    let field = |key: &str| -> Option<String> {
        value
            .as_ref()
            .and_then(|v| v.get(key))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    let workdir = field("workdir").map(PathBuf::from);
    let dir = workdir.as_deref().or(dir);
    let file = |key: &str, kind: Kind, written: Vec<String>, out: &mut Found| {
        if let Some(path) = field(key) {
            if let Some(note) = resolver.note_at(&path, dir) {
                out.push((note, kind, written));
            }
        }
    };
    match name {
        "Read" => file("file_path", Kind::Read, Vec::new(), out),
        "Grep" | "Glob" => file("path", Kind::Read, Vec::new(), out),
        "Write" => file("file_path", Kind::Edit, Vec::new(), out),
        "NotebookEdit" => file("notebook_path", Kind::Edit, Vec::new(), out),
        "Edit" => {
            let written = field("new_string").into_iter().collect();
            file("file_path", Kind::Edit, written, out);
        }
        "MultiEdit" => {
            let written = value
                .as_ref()
                .and_then(|v| v.get("edits"))
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|edit| edit.get("new_string").and_then(Value::as_str))
                .map(str::to_owned)
                .collect();
            file("file_path", Kind::Edit, written, out);
        }
        "Bash" => {
            if let Some(command) = field("command") {
                shell(resolver, dir, &command, out);
            }
        }
        "shell" | "local_shell" | "container.exec" | "exec_command" => {
            let command = value
                .as_ref()
                .and_then(|v| v.get("command").or_else(|| v.get("cmd")));
            match command {
                Some(Value::String(script)) => shell(resolver, dir, script, out),
                Some(Value::Array(argv)) => {
                    let argv: Vec<&str> = argv.iter().filter_map(Value::as_str).collect();
                    match argv.as_slice() {
                        [program, flag, script]
                            if is_shell(program) && flag.starts_with('-') && flag.contains('c') =>
                        {
                            shell(resolver, dir, script, out)
                        }
                        words => {
                            let words: Vec<Word> =
                                words.iter().map(|w| Word::plain((*w).to_owned())).collect();
                            let mut cd = dir.map(Path::to_path_buf);
                            command_paths(resolver, &mut cd, &words, out);
                        }
                    }
                }
                _ => {}
            }
        }
        "apply_patch" => {
            let patch = match &value {
                Some(Value::Object(map)) => map
                    .get("input")
                    .or_else(|| map.get("patch"))
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                Some(Value::String(text)) => Some(text.clone()),
                _ => Some(input.to_owned()),
            };
            for line in patch.unwrap_or_default().lines() {
                let path = line
                    .strip_prefix("*** Update File: ")
                    .or_else(|| line.strip_prefix("*** Add File: "));
                if let Some(note) = path.and_then(|path| resolver.note_at(path, dir)) {
                    out.push((note, Kind::Edit, Vec::new()));
                }
            }
        }
        _ => match &value {
            Some(value) => {
                let mut strings = Vec::new();
                collect_strings(value, &mut strings);
                for text in strings {
                    prose(resolver, dir, text, out);
                }
            }
            None => prose(resolver, dir, input, out),
        },
    }
}

/// The notes prose names: resolving `[[links]]`, backticked paths ending
/// `.md` (which may hold spaces), and other paths ending `.md`.
fn prose(resolver: &Resolver<'_>, dir: Option<&Path>, text: &str, out: &mut Found) {
    for inner in links(text) {
        if let Some(note) = resolver.note_linked(inner) {
            out.push((note, Kind::Discuss, Vec::new()));
        }
    }
    let ticked = BACKTICKED
        .captures_iter(text)
        .filter_map(|captures| captures.get(1))
        .map(|m| m.as_str());
    for written in ticked.chain(md_paths(text)) {
        if let Some(note) = resolver.note_in_prose(written, dir) {
            out.push((note, Kind::Discuss, Vec::new()));
        }
    }
}

static BACKTICKED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"`([^`\n]+\.md)`").expect("the backticked path pattern is valid"));

/// A path ending `.md`, without spaces, quotes, brackets or shell
/// operators.
static MD_PATH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"[^\s"'`(){}\[\],;=<>|&*?]*\.md\b"#).expect("the path pattern is valid")
});

fn md_paths(text: &str) -> impl Iterator<Item = &str> + '_ {
    MD_PATH
        .find_iter(text)
        .map(|m| m.as_str().trim_end_matches(':'))
        .filter(|path| path.len() > 3)
}

fn collect_strings<'v>(value: &'v Value, out: &mut Vec<&'v str>) {
    match value {
        Value::String(text) => out.push(text),
        Value::Array(items) => items.iter().for_each(|item| collect_strings(item, out)),
        Value::Object(map) => map.values().for_each(|item| collect_strings(item, out)),
        _ => {}
    }
}

fn is_shell(program: &str) -> bool {
    let name = program.rsplit('/').next().unwrap_or(program);
    matches!(name, "bash" | "sh" | "zsh" | "dash")
}

/// One shell word, unquoted, and how a redirection used it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Word {
    pub(crate) text: String,
    /// The target of `>` or `>>`.
    pub(crate) written: bool,
}

impl Word {
    fn plain(text: String) -> Word {
        Word {
            text,
            written: false,
        }
    }
}

/// The notes a shell script names. `cd` changes the directory for the
/// commands after it in the same script.
fn shell(resolver: &Resolver<'_>, dir: Option<&Path>, script: &str, out: &mut Found) {
    let mut cd = dir.map(Path::to_path_buf);
    for command in split_commands(script) {
        command_paths(resolver, &mut cd, &command, out);
    }
}

/// The notes one simple command names, and the effect of a `cd`.
fn command_paths(
    resolver: &Resolver<'_>,
    cd: &mut Option<PathBuf>,
    words: &[Word],
    out: &mut Found,
) {
    let start = words
        .iter()
        .position(|word| !is_assignment(&word.text))
        .unwrap_or(words.len());
    let words = &words[start..];
    let Some(program) = words.first() else {
        return;
    };
    let program = program.text.rsplit('/').next().unwrap_or(&program.text);
    let args = &words[1..];
    if program == "cd" {
        if let Some(target) = args.iter().find(|w| !w.text.starts_with('-')) {
            let target = Path::new(&target.text);
            *cd = if target.is_absolute() {
                Some(lexical(target))
            } else {
                cd.as_ref().map(|dir| lexical(&dir.join(target)))
            };
        }
        return;
    }
    let operands: Vec<usize> = (0..args.len())
        .filter(|&i| !args[i].text.starts_with('-') || args[i].written)
        .collect();
    let in_place = args
        .iter()
        .any(|w| w.text.starts_with("-i") || w.text == "--in-place");
    let perl_in_place = args
        .iter()
        .any(|w| w.text.starts_with('-') && !w.text.starts_with("--") && w.text[1..].contains('i'));
    let written_at = |i: usize| -> bool {
        args[i].written
            || match program {
                "tee" => true,
                "sed" | "gsed" => in_place,
                "perl" => perl_in_place,
                "cp" | "mv" | "install" => operands.last() == Some(&i),
                _ => false,
            }
    };
    // The program word too: a heredoc's lines, read as commands, may start
    // with a path.
    for (i, word) in words.iter().enumerate() {
        let written = if i == 0 {
            word.written
        } else {
            written_at(i - 1)
        };
        let kind = if written { Kind::Edit } else { Kind::Read };
        let text = &word.text;
        let mut candidates: Vec<&str> = md_paths(text).collect();
        let whole = text.trim_end_matches([':', ',', ')']);
        if whole.ends_with(".md") && !candidates.contains(&whole) {
            candidates.push(whole);
        }
        for written in candidates {
            if let Some(note) = resolver.note_at(written, cd.as_deref()) {
                out.push((note, kind, Vec::new()));
            }
        }
    }
}

fn is_assignment(word: &str) -> bool {
    match word.split_once('=') {
        Some((name, _)) => {
            !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
        None => false,
    }
}

/// Splits a script into simple commands of unquoted words. Quotes and
/// backslashes work as in a POSIX shell; `;`, `&`, `|`, newlines and
/// parentheses end a command; `>`/`>>` mark the next word as written, a
/// redirection to a file descriptor (`2>&1`) names no file, a `<<`
/// delimiter is not a word, and `#` at the start of a word comments out the
/// rest of the line. Heredoc bodies are read as commands: an approximation.
pub(crate) fn split_commands(script: &str) -> Vec<Vec<Word>> {
    let mut commands = Vec::new();
    let mut words: Vec<Word> = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut pending_written = false;
    let mut skip_next = false;
    let mut chars = script.chars().peekable();

    let finish = |word: &mut String,
                  in_word: &mut bool,
                  words: &mut Vec<Word>,
                  pending_written: &mut bool,
                  skip_next: &mut bool| {
        if *in_word {
            if *skip_next {
                *skip_next = false;
            } else {
                words.push(Word {
                    text: std::mem::take(word),
                    written: std::mem::take(pending_written),
                });
            }
            word.clear();
            *in_word = false;
        }
    };

    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                for c in chars.by_ref() {
                    if c == '\'' {
                        break;
                    }
                    word.push(c);
                }
            }
            '"' => {
                in_word = true;
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => match chars.peek() {
                            Some(&next @ ('"' | '\\' | '$' | '`')) => {
                                word.push(next);
                                chars.next();
                            }
                            Some('\n') => {
                                chars.next();
                            }
                            _ => word.push('\\'),
                        },
                        other => word.push(other),
                    }
                }
            }
            '\\' => match chars.next() {
                Some('\n') | None => {}
                Some(next) => {
                    in_word = true;
                    word.push(next);
                }
            },
            '#' if !in_word => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
                finish(
                    &mut word,
                    &mut in_word,
                    &mut words,
                    &mut pending_written,
                    &mut skip_next,
                );
                if !words.is_empty() {
                    commands.push(std::mem::take(&mut words));
                }
            }
            '>' => {
                // `2>` names a file descriptor, not a word.
                if in_word && word.chars().all(|c| c.is_ascii_digit()) {
                    word.clear();
                    in_word = false;
                } else {
                    finish(
                        &mut word,
                        &mut in_word,
                        &mut words,
                        &mut pending_written,
                        &mut skip_next,
                    );
                }
                if chars.peek() == Some(&'>') {
                    chars.next();
                }
                if chars.peek() == Some(&'&') {
                    chars.next();
                    while chars
                        .peek()
                        .is_some_and(|c| c.is_ascii_digit() || *c == '-')
                    {
                        chars.next();
                    }
                } else {
                    pending_written = true;
                }
            }
            '<' => {
                finish(
                    &mut word,
                    &mut in_word,
                    &mut words,
                    &mut pending_written,
                    &mut skip_next,
                );
                if chars.peek() == Some(&'<') {
                    chars.next();
                    if chars.peek() == Some(&'-') {
                        chars.next();
                    }
                    skip_next = true;
                }
            }
            ';' | '&' | '|' | '\n' | '(' | ')' | '`' => {
                finish(
                    &mut word,
                    &mut in_word,
                    &mut words,
                    &mut pending_written,
                    &mut skip_next,
                );
                pending_written = false;
                if !words.is_empty() {
                    commands.push(std::mem::take(&mut words));
                }
            }
            c if c.is_whitespace() => {
                finish(
                    &mut word,
                    &mut in_word,
                    &mut words,
                    &mut pending_written,
                    &mut skip_next,
                );
            }
            other => {
                in_word = true;
                word.push(other);
            }
        }
    }
    finish(
        &mut word,
        &mut in_word,
        &mut words,
        &mut pending_written,
        &mut skip_next,
    );
    if !words.is_empty() {
        commands.push(words);
    }
    commands
}

/// `path` with `.` and `..` components resolved without the file system.
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// A directory's name as Claude Code writes its project directory: every
/// character that is not an ASCII letter or digit becomes `-`.
pub fn encode(dir: &Path) -> String {
    dir.to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// The roots of the other worktrees of the git repository at `root`, from
/// its common git directory: the main worktree, when `root` is a linked
/// one, and each `worktrees/<name>/gitdir`.
fn worktrees(root: &Path) -> Vec<PathBuf> {
    let Some(common) = common_dir(root) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    if common.file_name().is_some_and(|name| name == ".git") {
        if let Some(main) = common.parent() {
            found.push(main.to_path_buf());
        }
    }
    if let Ok(entries) = fs::read_dir(common.join("worktrees")) {
        for entry in entries.flatten() {
            let Ok(gitdir) = fs::read_to_string(entry.path().join("gitdir")) else {
                continue;
            };
            let gitdir = PathBuf::from(gitdir.trim());
            if let Some(worktree) = gitdir.parent() {
                found.push(worktree.to_path_buf());
            }
        }
    }
    found.retain(|dir| dir != root);
    found
}

/// The common git directory of the repository whose worktree is `root`.
pub(crate) fn common_dir(root: &Path) -> Option<PathBuf> {
    let dot_git = root.join(".git");
    let meta = fs::symlink_metadata(&dot_git).ok()?;
    if meta.is_dir() {
        return dot_git.canonicalize().ok();
    }
    let text = fs::read_to_string(&dot_git).ok()?;
    let gitdir = text.trim().strip_prefix("gitdir:")?.trim();
    let gitdir = root.join(gitdir);
    let common = match fs::read_to_string(gitdir.join("commondir")) {
        Ok(common) => gitdir.join(common.trim()),
        Err(_) => gitdir,
    };
    common.canonicalize().ok()
}

/// The `cwd` of a Codex rollout's first `session_meta`, read from at most
/// its first 20 lines.
pub(crate) fn codex_cwd(transcript: &Path) -> Option<PathBuf> {
    let file = fs::File::open(transcript).ok()?;
    for line in BufReader::new(file).lines().take(20) {
        let line = line.ok()?;
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if value.get("type").and_then(Value::as_str) == Some("session_meta") {
            let cwd = value.get("payload")?.get("cwd")?.as_str()?;
            return Some(PathBuf::from(cwd));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A workspace at `<tmp>/ws` with notes under `brain/`, among them
    /// `brain/Ops/Session log.md` and `brain/START HERE.md`, and a git
    /// repository with one linked worktree at `<tmp>/ws-wt/feature`.
    struct Ws {
        _dir: tempfile::TempDir,
        root: PathBuf,
        worktree: PathBuf,
        notes: Notes,
    }

    impl Ws {
        fn new() -> Ws {
            let dir = tempfile::tempdir().unwrap();
            let base = dir.path().canonicalize().unwrap();
            let root = base.join("ws");
            for note in [
                "brain/START HERE.md",
                "brain/Ops/Session log.md",
                "brain/Ops/Runbook.md",
                "brain/Ops/Log (hippo).md",
                "brain/Topic.md",
            ] {
                let path = root.join(note);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, "# Note\n").unwrap();
            }
            fs::create_dir_all(root.join("app")).unwrap();
            // A linked worktree, as `git worktree add` lays it out.
            let worktree = base.join("ws-wt").join("feature");
            let gitdir = root.join(".git").join("worktrees").join("feature");
            fs::create_dir_all(&gitdir).unwrap();
            fs::create_dir_all(&worktree).unwrap();
            fs::write(
                gitdir.join("gitdir"),
                format!("{}\n", worktree.join(".git").display()),
            )
            .unwrap();
            fs::write(gitdir.join("commondir"), "../..\n").unwrap();
            fs::write(
                worktree.join(".git"),
                format!("gitdir: {}\n", gitdir.display()),
            )
            .unwrap();
            let notes = Notes::scan(&root.join("brain")).unwrap();
            Ws {
                _dir: dir,
                root,
                worktree,
                notes,
            }
        }

        fn resolver(&self) -> Resolver<'_> {
            Resolver::new(
                &self.root,
                Path::new("brain"),
                &self.notes,
                Some(Path::new("brain/Ops/Log (hippo).md")),
            )
        }
    }

    fn turn(ordinal: u32, role: &str, tool: Option<&str>, text: &str) -> StoredTurn {
        StoredTurn {
            ordinal,
            role: role.into(),
            ts: "2026-11-08T01:00:00.000000Z".into(),
            tool_name: tool.map(str::to_owned),
            text: text.into(),
        }
    }

    fn call_turn(tool: &str, input: Value) -> StoredTurn {
        turn(3, "tool", Some(tool), &format!("{tool} {input}"))
    }

    fn seen(ws: &Ws, dir: Option<&Path>, turn: &StoredTurn) -> Vec<(String, &'static str)> {
        observe(&ws.resolver(), dir, turn)
            .into_iter()
            .map(|o| (o.note.display().to_string(), o.kind.as_str()))
            .collect()
    }

    #[test]
    fn claude_code_file_tools_name_notes_by_absolute_path() {
        let ws = Ws::new();
        let note = ws.root.join("brain/Topic.md").display().to_string();
        let read = call_turn("Read", json!({"file_path": note, "limit": 40}));
        assert_eq!(seen(&ws, None, &read), [("brain/Topic.md".into(), "read")]);
        let edit = call_turn(
            "Edit",
            json!({"file_path": note, "old_string": "a", "new_string": "the new text"}),
        );
        let observed = observe(&ws.resolver(), None, &edit);
        assert_eq!(observed[0].kind, Kind::Edit);
        assert_eq!(observed[0].written, ["the new text"]);
        let multi = call_turn(
            "MultiEdit",
            json!({"file_path": note, "edits": [{"old_string": "a", "new_string": "one"}, {"old_string": "b", "new_string": "two"}]}),
        );
        assert_eq!(
            observe(&ws.resolver(), None, &multi)[0].written,
            ["one", "two"]
        );
        let write = call_turn("Write", json!({"file_path": note, "content": "x"}));
        assert_eq!(seen(&ws, None, &write), [("brain/Topic.md".into(), "edit")]);
        // A file outside the notes, a note that does not exist, and a path
        // outside the workspace name nothing.
        for path in [
            ws.root.join("app/README.md"),
            ws.root.join("brain/Missing.md"),
            PathBuf::from("/elsewhere/brain/Topic.md"),
        ] {
            let read = call_turn("Read", json!({"file_path": path.display().to_string()}));
            assert!(seen(&ws, None, &read).is_empty(), "{}", path.display());
        }
    }

    #[test]
    fn a_worktree_path_names_the_workspace_note() {
        let ws = Ws::new();
        let in_worktree = ws.worktree.join("brain/Ops/Runbook.md");
        let edit = call_turn(
            "Edit",
            json!({"file_path": in_worktree.display().to_string(), "new_string": "x"}),
        );
        assert_eq!(
            seen(&ws, None, &edit),
            [("brain/Ops/Runbook.md".into(), "edit")]
        );
    }

    #[test]
    fn tool_results_and_the_generated_log_are_never_candidates() {
        let ws = Ws::new();
        let note = ws.root.join("brain/Topic.md").display().to_string();
        // A result: its text does not start with the tool's name.
        let result = turn(4, "tool", Some("Bash"), &format!("{note}:3: a match"));
        assert!(seen(&ws, None, &result).is_empty());
        let log = ws
            .root
            .join("brain/Ops/Log (hippo).md")
            .display()
            .to_string();
        let read = call_turn("Read", json!({ "file_path": log }));
        assert!(seen(&ws, None, &read).is_empty());
    }

    #[test]
    fn relative_shell_paths_need_a_known_directory() {
        let ws = Ws::new();
        let bash = call_turn(
            "Bash",
            json!({"command": "cat \"brain/START HERE.md\" brain/Topic.md 2>&1 | head -50"}),
        );
        assert!(seen(&ws, None, &bash).is_empty(), "no directory, no note");
        assert_eq!(
            seen(&ws, Some(&ws.root), &bash),
            [
                ("brain/START HERE.md".into(), "read"),
                ("brain/Topic.md".into(), "read")
            ]
        );
        // Another project's directory: its own brain/ is not the workspace's.
        assert!(seen(&ws, Some(Path::new("/other/project")), &bash).is_empty());
    }

    #[test]
    fn cd_moves_later_commands_and_writes_are_edits() {
        let ws = Ws::new();
        let script = format!(
            "cd {}/brain && sed -n 1,80p Ops/Runbook.md; printf 'x' >> 'Ops/Session log.md' && sed -i '' 's/a/b/' Topic.md",
            ws.root.display()
        );
        let bash = call_turn("Bash", json!({ "command": script }));
        assert_eq!(
            seen(&ws, Some(Path::new("/elsewhere")), &bash),
            [
                ("brain/Ops/Runbook.md".into(), "read"),
                ("brain/Ops/Session log.md".into(), "edit"),
                ("brain/Topic.md".into(), "edit"),
            ]
        );
        let relative = call_turn(
            "Bash",
            json!({"command": "cd brain && cat Topic.md | tee -a Ops/Runbook.md >/dev/null"}),
        );
        assert_eq!(
            seen(&ws, Some(&ws.root), &relative),
            [
                ("brain/Ops/Runbook.md".into(), "edit"),
                ("brain/Topic.md".into(), "read"),
            ]
        );
    }

    #[test]
    fn heredocs_and_comments_do_not_hide_or_invent_notes() {
        let ws = Ws::new();
        let bash = call_turn(
            "Bash",
            json!({"command": "python3 - <<'EOF'\nprint(open(\"brain/Topic.md\").read())\nEOF\n# cat brain/Ops/Runbook.md"}),
        );
        assert_eq!(
            seen(&ws, Some(&ws.root), &bash),
            [("brain/Topic.md".into(), "read")]
        );
    }

    #[test]
    fn codex_shell_reads_use_workdir_and_patches_are_edits() {
        let ws = Ws::new();
        let root = ws.root.display().to_string();
        let shell = call_turn(
            "shell",
            json!({"command": ["bash", "-lc", "sed -n '1,40p' 'brain/Ops/Runbook.md'"], "workdir": root}),
        );
        assert_eq!(
            seen(&ws, None, &shell),
            [("brain/Ops/Runbook.md".into(), "read")]
        );
        let argv = call_turn(
            "shell",
            json!({"command": ["cat", "brain/Topic.md"], "workdir": root}),
        );
        assert_eq!(seen(&ws, None, &argv), [("brain/Topic.md".into(), "read")]);
        let exec = call_turn(
            "exec_command",
            json!({"cmd": "rg -n retry brain/Topic.md", "workdir": root}),
        );
        assert_eq!(seen(&ws, None, &exec), [("brain/Topic.md".into(), "read")]);
        let patch = turn(
            5,
            "tool",
            Some("apply_patch"),
            "apply_patch *** Begin Patch\n*** Update File: brain/Topic.md\n@@\n-a\n+b\n*** End Patch",
        );
        assert_eq!(
            seen(&ws, Some(&ws.root), &patch),
            [("brain/Topic.md".into(), "edit")]
        );
    }

    #[test]
    fn prose_names_notes_by_link_backticks_and_relative_paths() {
        let ws = Ws::new();
        let text =
            "Updated `brain/Ops/Session log.md` and [[Topic]]; see Ops/Runbook.md and [[Missing]].";
        let said = turn(2, "assistant", None, text);
        assert_eq!(
            seen(&ws, None, &said),
            [
                ("brain/Ops/Runbook.md".into(), "discuss"),
                ("brain/Ops/Session log.md".into(), "discuss"),
                ("brain/Topic.md".into(), "discuss"),
            ]
        );
        // A sub-agent's prompt is prose too.
        let agent = call_turn(
            "Agent",
            json!({"prompt": "Read brain/Topic.md first.", "description": "x"}),
        );
        assert_eq!(
            seen(&ws, None, &agent),
            [("brain/Topic.md".into(), "discuss")]
        );
    }

    #[test]
    fn a_claude_code_session_starts_in_its_encoded_project_directory() {
        let ws = Ws::new();
        let resolver = ws.resolver();
        let project = encode(&ws.root);
        let main = PathBuf::from(format!("/h/.claude/projects/{project}/abc.jsonl"));
        assert_eq!(
            resolver.session_dir("claude-code", &main),
            Some(ws.root.clone())
        );
        // A sub-agent's transcript is nested under its session.
        let nested = PathBuf::from(format!(
            "/h/.claude-2/projects/{project}/abc/subagents/agent-1.jsonl"
        ));
        assert_eq!(
            resolver.session_dir("claude-code", &nested),
            Some(ws.root.clone())
        );
        let child = PathBuf::from(format!(
            "/h/.claude/projects/{}/abc.jsonl",
            encode(&ws.root.join("app"))
        ));
        assert_eq!(
            resolver.session_dir("claude-code", &child),
            Some(ws.root.join("app"))
        );
        let worktree = PathBuf::from(format!(
            "/h/.claude/projects/{}/abc.jsonl",
            encode(&ws.worktree)
        ));
        assert_eq!(
            resolver.session_dir("claude-code", &worktree),
            Some(ws.worktree.clone())
        );
        let other = PathBuf::from("/h/.claude/projects/-other-project/abc.jsonl");
        assert_eq!(resolver.session_dir("claude-code", &other), None);
    }

    #[test]
    fn a_codex_session_starts_in_its_session_meta_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let rollout = dir.path().join("rollout-1.jsonl");
        fs::write(
            &rollout,
            format!(
                "{}\n{}\n",
                json!({"type": "response_item", "payload": {}}),
                json!({"type": "session_meta", "payload": {"id": "x", "cwd": "/work/ws"}})
            ),
        )
        .unwrap();
        let ws = Ws::new();
        assert_eq!(
            ws.resolver().session_dir("codex", &rollout),
            Some(PathBuf::from("/work/ws"))
        );
    }

    #[test]
    fn candidates_rank_by_kind_then_evidence_then_path() {
        let mut candidates = BTreeMap::new();
        let mut add = |path: &str, kind: Kind, times: usize| {
            let seen: &mut Seen = candidates.entry(PathBuf::from(path)).or_default();
            for _ in 0..times {
                seen.add("s", kind, &[]);
            }
        };
        add("brain/b.md", Kind::Read, 9);
        add("brain/a.md", Kind::Discuss, 1);
        add("brain/c.md", Kind::Edit, 1);
        add("brain/d.md", Kind::Edit, 3);
        add("brain/e.md", Kind::Discuss, 1);
        add("brain/e.md", Kind::Read, 1);
        let ranked: Vec<String> = rank(&candidates)
            .iter()
            .map(|p| p.display().to_string())
            .collect();
        assert_eq!(
            ranked,
            [
                "brain/d.md",
                "brain/c.md",
                "brain/e.md",
                "brain/a.md",
                "brain/b.md"
            ]
        );
    }

    #[test]
    fn a_session_end_command_is_seen_in_shell_calls_only() {
        let ends = |tool: &str, input: Value| ends_session(&call_turn(tool, input));
        assert!(ends(
            "Bash",
            json!({ "command": "python3 tools/notesctl.py --workspace /w session-end --note a.md" })
        ));
        assert!(ends(
            "Bash",
            json!({ "command": "for i in 1 2; do out=$(FOO=1 /usr/bin/notesctl session-end); done" })
        ));
        assert!(ends(
            "Bash",
            json!({ "command": "hippo session end --note a.md" })
        ));
        assert!(ends(
            "exec_command",
            json!({ "cmd": ["bash", "-lc", "notesctl session-end"] })
        ));
        assert!(ends(
            "shell",
            json!({ "command": ["notesctl", "session-end"] })
        ));
        // Text that names the command, a result, another tool's input.
        assert!(!ends(
            "Bash",
            json!({ "command": "grep -rn session-end tools/" })
        ));
        assert!(!ends(
            "Bash",
            json!({ "command": "echo notesctl session-end" })
        ));
        assert!(!ends(
            "Bash",
            json!({ "command": "notesctl session-ender" })
        ));
        assert!(!ends(
            "Write",
            json!({ "file_path": "a.md", "content": "notesctl session-end" })
        ));
        assert!(!ends_session(&turn(
            4,
            "tool",
            Some("Bash"),
            "notesctl session-end: done"
        )));
    }

    #[test]
    fn the_shell_splitter_keeps_quoted_words_and_drops_descriptors() {
        let commands = split_commands("a 'b c' \"d \\\"e\\\"\" 2>&1 > out.md; f | g");
        let words: Vec<Vec<(String, bool)>> = commands
            .iter()
            .map(|c| c.iter().map(|w| (w.text.clone(), w.written)).collect())
            .collect();
        assert_eq!(
            words,
            [
                vec![
                    ("a".into(), false),
                    ("b c".into(), false),
                    ("d \"e\"".into(), false),
                    ("out.md".into(), true)
                ],
                vec![("f".into(), false)],
                vec![("g".into(), false)],
            ]
        );
    }
}
