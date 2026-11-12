//! Adapters: one tool's transcript files into sessions and turns.
//!
//! Both adapters walk every transcript root and claim files by name, so a
//! root may hold either tool's files: Codex takes `rollout-*.jsonl`, and
//! Claude Code takes every other `*.jsonl`. Every transcript belongs to
//! the one configured workspace.

pub mod claude_code;
pub mod codex;

use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde_json::Value;
use time::format_description::well_known::Rfc3339;
use time::{OffsetDateTime, UtcOffset};

use crate::config::{expand_tilde, Config};
use crate::error::{Error, Result};
pub use crate::records::Tool;

/// Who produced a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    /// A tool call or a tool result.
    Tool,
}

impl Role {
    /// `user`, `assistant` or `tool`, as the index stores it.
    pub fn as_str(self) -> &'static str {
        match self {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        }
    }
}

/// One content block of a transcript, before redaction: a text block, a
/// tool call or a tool result. Reasoning blocks are never turns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawTurn {
    /// Counts emitted turns from 0.
    pub ordinal: u32,
    pub role: Role,
    /// The containing record's timestamp.
    pub ts: OffsetDateTime,
    /// Set for `tool` turns.
    pub tool_name: Option<String>,
    pub text: String,
}

/// One transcript file, parsed as one session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    /// The session id per Conventions.
    pub id: String,
    pub tool: Tool,
    pub started: OffsetDateTime,
    pub turns: Vec<RawTurn>,
}

/// Reads one tool's transcripts.
pub trait Adapter {
    fn tool(&self) -> Tool;
    /// The directories to walk for this tool's files.
    fn roots(&self, cfg: &Config) -> Vec<PathBuf>;
    /// Whether a file under a root is this tool's transcript.
    fn file_matches(&self, path: &Path) -> bool;
    /// Parses a matching file found under `root`.
    fn parse(&self, root: &Path, path: &Path) -> Result<Parsed>;
    /// A revision of the parser that this file's bytes need, when they hold
    /// records an earlier parser skipped: the ingest stores the file's hash
    /// with it, so an older index reads the file again once.
    fn revision(&self, _bytes: &[u8]) -> Option<&'static str> {
        None
    }
    /// The directory session `id`, parsed from `path`, ran in, when its
    /// transcript names one (`session_meta::origin`): which workspace it
    /// belongs to.
    fn cwd(&self, id: &str, path: &Path) -> Option<PathBuf> {
        crate::session_meta::origin(id, self.tool().as_str(), path).cwd
    }
}

/// The adapters ingest runs, in order.
pub fn all() -> Vec<Box<dyn Adapter>> {
    vec![Box::new(claude_code::ClaudeCode), Box::new(codex::Codex)]
}

/// The configured `transcript_roots` with `~` expanded and globs matched:
/// every existing directory they name, sorted. A pattern that is not a
/// valid glob is skipped with a warning.
pub fn transcript_roots(cfg: &Config) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for pattern in &cfg.transcript_roots {
        let pattern = expand_tilde(pattern);
        match glob::glob(&pattern) {
            Ok(paths) => roots.extend(paths.filter_map(|path| path.ok()).filter(|p| p.is_dir())),
            Err(err) => tracing::warn!("skipping transcript root {pattern:?}: {err}"),
        }
    }
    let excluded: Vec<PathBuf> = cfg
        .exclude_roots
        .iter()
        .map(|e| PathBuf::from(expand_tilde(e)))
        .collect();
    roots.retain(|r| !is_excluded(r, &excluded));
    roots.sort();
    roots.dedup();
    roots
}

/// Whether `dir` equals or lies below one of `excluded`.
pub fn is_excluded(dir: &Path, excluded: &[PathBuf]) -> bool {
    excluded
        .iter()
        .any(|e| !e.as_os_str().is_empty() && dir.starts_with(e))
}

fn has_extension_jsonl(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "jsonl")
}

/// A Claude Code Workflow's journal, `…/workflows/<run>/journal.jsonl`:
/// a log of the run's agents, not a transcript. The agents' own
/// transcripts beside it are.
fn is_workflow_journal(path: &Path) -> bool {
    path.file_name().is_some_and(|name| name == "journal.jsonl")
        && path
            .parent()
            .and_then(Path::parent)
            .and_then(Path::file_name)
            .is_some_and(|dir| dir == "workflows")
}

fn is_rollout(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name.as_encoded_bytes().starts_with(b"rollout-"))
}

/// The error for a file that opened but is not a transcript its adapter can
/// use. There is no parse-specific `Error` variant, so it is an
/// `InvalidData` I/O error that names `path`.
fn invalid_data(path: &Path, what: &str) -> Error {
    Error::io_at(path, io::Error::new(io::ErrorKind::InvalidData, what))
}

/// Calls `visit` with each line of the JSON Lines file at `path` that holds
/// a JSON value, in file order.
///
/// Blank lines are passed over. A line that is not valid JSON, such as a
/// partly written last line, is skipped with a warning that names the file
/// and the line number and quotes none of the content. Only a failure to
/// open or read the file is an error.
fn each_json_line(path: &Path, mut visit: impl FnMut(Value)) -> Result<()> {
    let at = |err| Error::io_at(path, err);
    let mut reader = BufReader::with_capacity(1 << 18, File::open(path).map_err(at)?);
    let mut line = Vec::new();
    for number in 1usize.. {
        line.clear();
        if reader.read_until(b'\n', &mut line).map_err(at)? == 0 {
            break;
        }
        let text = line.trim_ascii();
        if text.is_empty() {
            continue;
        }
        match serde_json::from_slice(text) {
            Ok(value) => visit(value),
            Err(err) => tracing::warn!(
                "{}:{number}: skipping a line that is not valid JSON: {err}",
                path.display()
            ),
        }
    }
    Ok(())
}

/// The RFC 3339 timestamp in a JSON field, as UTC. `None` when the field is
/// missing, is not a string, or is not RFC 3339.
fn timestamp(field: Option<&Value>) -> Option<OffsetDateTime> {
    let ts = OffsetDateTime::parse(field?.as_str()?, &Rfc3339).ok()?;
    Some(ts.to_offset(UtcOffset::UTC))
}

/// Appends a turn, numbered after the turns already emitted.
fn push_turn(
    turns: &mut Vec<RawTurn>,
    role: Role,
    ts: OffsetDateTime,
    tool_name: Option<String>,
    text: String,
) {
    let ordinal = u32::try_from(turns.len()).unwrap_or(u32::MAX);
    turns.push(RawTurn {
        ordinal,
        role,
        ts,
        tool_name,
        text,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use serde_json::json;

    use crate::redact::redact;

    const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures");

    /// The redaction kinds, in the order they are applied.
    const KINDS: [&str; 10] = [
        "pem", "aws", "github", "provider", "jwt", "auth", "kv", "email", "aadhaar", "phone",
    ];

    /// The lines of `fixtures/redaction/leaks.txt`.
    fn leaks() -> Vec<String> {
        let text = fs::read_to_string(format!("{FIXTURES}/redaction/leaks.txt")).unwrap();
        text.lines().map(str::to_owned).collect()
    }

    /// The turns of each adapter's synthetic transcripts, parsed with
    /// `fixtures/` as the root: Claude Code's two files, then Codex's.
    fn fixture_turns() -> [(&'static str, Vec<RawTurn>); 2] {
        let root = Path::new(FIXTURES);
        let claude_code = [
            "claude-code/synthetic.jsonl",
            "claude-code/stub-1/subagents/agent-a1.jsonl",
        ]
        .into_iter()
        .flat_map(|name| {
            let path = root.join(name);
            assert!(claude_code::ClaudeCode.file_matches(&path), "{name}");
            claude_code::ClaudeCode.parse(root, &path).unwrap().turns
        })
        .collect();
        let path = root.join("codex/rollout-synthetic.jsonl");
        assert!(codex::Codex.file_matches(&path));
        let codex = codex::Codex.parse(root, &path).unwrap().turns;
        [("claude-code", claude_code), ("codex", codex)]
    }

    #[test]
    fn every_leak_is_embedded_verbatim_in_each_adapters_fixture() {
        let leaks = leaks();
        assert_eq!(leaks.len(), 17);
        for (adapter, turns) in fixture_turns() {
            for leak in &leaks {
                assert!(
                    turns.iter().any(|turn| turn.text.contains(leak.as_str())),
                    "no {adapter} turn holds {leak:?}"
                );
            }
        }
    }

    #[test]
    fn redaction_leaves_no_leak_and_every_kind_in_each_adapters_fixture() {
        let leaks = leaks();
        for kind in KINDS {
            let label = format!("{kind} ");
            assert!(
                leaks.iter().any(|leak| leak.starts_with(&label)),
                "leaks.txt has no {kind} line"
            );
        }
        for (adapter, turns) in fixture_turns() {
            let redacted: Vec<String> = turns
                .iter()
                .map(|turn| redact(&turn.text).as_str().to_owned())
                .collect();
            for text in &redacted {
                for leak in &leaks {
                    // The whole line, and the secret without its label.
                    let (_, secret) = leak.split_once(' ').unwrap();
                    assert!(
                        !text.contains(leak.as_str()),
                        "{adapter}: {leak:?} survived"
                    );
                    assert!(!text.contains(secret), "{adapter}: {secret:?} survived");
                }
            }
            for kind in KINDS {
                let marker = format!("[redacted:{kind}]");
                assert!(
                    redacted.iter().any(|text| text.contains(&marker)),
                    "{adapter}: no turn holds {marker}"
                );
            }
        }
    }

    #[test]
    fn json_lines_skip_blank_and_bad_lines() {
        let dir = tempfile::tempdir().unwrap();
        let read = |name: &str, bytes: &[u8]| {
            let path = dir.path().join(name);
            fs::write(&path, bytes).unwrap();
            let mut seen = Vec::new();
            each_json_line(&path, |value| seen.push(value)).unwrap();
            seen
        };
        // CRLF, blank lines, text, invalid UTF-8, a cut-off object and a
        // last line with no newline.
        let whole = read(
            "whole.jsonl",
            b"{\"n\":1}\r\n\r\n  \t\n\
              not json\n\xff\xfe{\"n\":2}\n{\"n\":\n{\"n\":3}\n[4]",
        );
        assert_eq!(whole, [json!({"n": 1}), json!({"n": 3}), json!([4])]);
        // A partly written last line is skipped, not an error.
        let cut = read("cut.jsonl", b"{\"n\":1}\n{\"n\":2,\"text\":\"par");
        assert_eq!(cut, [json!({"n": 1})]);
        assert!(read("empty.jsonl", b"").is_empty());

        let err = each_json_line(&dir.path().join("gone.jsonl"), |_| {}).unwrap_err();
        assert!(
            matches!(&err, Error::Io(e) if e.kind() == io::ErrorKind::NotFound),
            "{err}"
        );
        assert!(err.to_string().contains("gone.jsonl"), "{err}");
    }

    #[test]
    fn timestamps_are_rfc_3339_in_utc() {
        let ts = |text: &str| timestamp(Some(&json!(text)));
        let utc = ts("2026-11-01T10:00:00.5Z").unwrap();
        assert_eq!(utc, time::macros::datetime!(2026-11-01 10:00:00.5 UTC));
        let shifted = ts("2026-11-01T19:30:00.5+09:30").unwrap();
        assert_eq!(shifted, utc);
        assert_eq!(shifted.offset(), UtcOffset::UTC);
        // The offset is required.
        for bad in [
            "",
            "yesterday",
            "2026-11-01",
            "2026-11-01T10:00:00",
            "1762000000",
        ] {
            assert_eq!(ts(bad), None, "{bad:?}");
        }
        assert_eq!(timestamp(Some(&json!(1_762_000_000))), None);
        assert_eq!(timestamp(Some(&Value::Null)), None);
        assert_eq!(timestamp(None), None);
    }

    #[test]
    fn turns_are_numbered_in_the_order_they_are_pushed() {
        let ts = time::macros::datetime!(2026-11-01 10:00:00 UTC);
        let mut turns = Vec::new();
        push_turn(&mut turns, Role::User, ts, None, "a".into());
        push_turn(&mut turns, Role::Tool, ts, Some("Read".into()), "b".into());
        let numbered: Vec<_> = turns
            .iter()
            .map(|turn| (turn.ordinal, turn.role, turn.tool_name.as_deref()))
            .collect();
        assert_eq!(
            numbered,
            [(0, Role::User, None), (1, Role::Tool, Some("Read"))]
        );
    }

    #[test]
    fn roots_expand_globs_to_existing_directories() {
        let dir = tempfile::tempdir().unwrap();
        for profile in [".claude", ".claude-work"] {
            fs::create_dir_all(dir.path().join(profile).join("projects")).unwrap();
        }
        fs::write(dir.path().join(".claude-file"), "").unwrap();
        let home = dir.path().to_str().unwrap();
        let cfg = Config {
            home: dir.path().join(".hippocamp"),
            workspace: Some(dir.path().to_path_buf()),
            transcript_roots: vec![
                format!("{home}/.claude*/projects"),
                format!("{home}/missing"),
            ],
            exclude_roots: Vec::new(),
            model_command: Vec::new(),
            ingest_interval_minutes: 10,
            dream_at: "03:30".into(),
            reviewer_command_override: None,
        };
        let roots = transcript_roots(&cfg);
        assert_eq!(
            roots,
            [
                dir.path().join(".claude/projects"),
                dir.path().join(".claude-work/projects")
            ]
        );
        // A root turned off is skipped.
        let cfg = Config {
            exclude_roots: vec![format!("{home}/.claude-work")],
            ..cfg
        };
        assert_eq!(
            transcript_roots(&cfg),
            [dir.path().join(".claude/projects")]
        );
    }

    #[test]
    fn files_are_claimed_by_name() {
        let claude = claude_code::ClaudeCode;
        let codex = codex::Codex;
        for (path, claude_takes, codex_takes) in [
            ("p/abc.jsonl", true, false),
            ("p/abc/subagents/agent-1.jsonl", true, false),
            ("p/abc/subagents/workflows/wf_1/agent-a1.jsonl", true, false),
            ("p/abc/subagents/workflows/wf_1/journal.jsonl", false, false),
            ("p/abc/journal.jsonl", true, false),
            (
                "2026/11/01/rollout-2026-11-01T03-36-53-x.jsonl",
                false,
                true,
            ),
            ("p/notes.json", false, false),
            ("p/rollout-notes.md", false, false),
        ] {
            let path = Path::new(path);
            assert_eq!(
                claude.file_matches(path),
                claude_takes,
                "{}",
                path.display()
            );
            assert_eq!(codex.file_matches(path), codex_takes, "{}", path.display());
        }
    }
}
