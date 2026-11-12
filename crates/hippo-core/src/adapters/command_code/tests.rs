use super::*;
use std::fs;

use serde_json::json;
use time::macros::datetime;

const PROJECT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/command-code/.commandcode/projects/work-example"
);
const S1: &str = "ee000000-0000-4000-8000-000000000001";
const OLD: &str = "ee000000-0000-4000-8000-000000000002";

fn file(id: &str) -> PathBuf {
    Path::new(PROJECT).join(format!("{id}.jsonl"))
}

/// `(role, tool name, text)` of each turn.
fn shape(p: &Parsed) -> Vec<(Role, Option<&str>, &str)> {
    p.turns
        .iter()
        .map(|t| (t.role, t.tool_name.as_deref(), t.text.as_str()))
        .collect()
}

fn write(dir: &Path, lines: &[Value]) -> PathBuf {
    let path = dir.join(".commandcode/projects/p/s.jsonl");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    fs::write(&path, text).unwrap();
    path
}

#[test]
fn the_fixture_parses_into_its_turns() {
    let p = CommandCode.parse(Path::new(PROJECT), &file(S1)).unwrap();
    assert_eq!(p.id, format!("command-code-{S1}"));
    assert_eq!(p.tool, Tool::CommandCode);
    assert_eq!(p.started, datetime!(2026-11-01 12:00:00 UTC));
    let turns = shape(&p);
    assert_eq!(turns.len(), 5, "{turns:#?}");
    assert_eq!(turns[0].0, Role::User);
    assert!(turns[0].2.starts_with("Fix the retry backoff"));
    assert_eq!(turns[1], (Role::Assistant, None, "Let me read the sender."));
    assert_eq!(
        turns[2],
        (
            Role::Tool,
            Some("read_file"),
            r#"read_file {"file_path":"/work/example/src/retry.ts"}"#
        )
    );
    assert_eq!(turns[3].1, Some("read_file"));
    assert!(turns[3]
        .2
        .starts_with("Read 1/1 file\n1: const backoffMs = 10;"));
    assert_eq!(
        turns[4],
        (
            Role::Assistant,
            None,
            "The backoff starts at 10 ms. Raise it to 500 ms and double it each retry."
        )
    );
    assert_eq!(p.turns[4].ts, datetime!(2026-11-01 12:00:06 UTC));
    assert!(p.turns.iter().all(|t| !t.text.contains("Never indexed")));
}

#[test]
fn only_session_files_under_commandcode_match() {
    assert!(CommandCode.file_matches(&file(S1)));
    let sidecar = Path::new(PROJECT).join(format!("{S1}.checkpoints.jsonl"));
    assert!(sidecar.exists());
    assert!(!CommandCode.file_matches(&sidecar));
    for path in [
        "/u/.claude/projects/p/s.jsonl",
        "/u/.commandcode/s.jsonl",
        "/u/.commandcode/projects/s.jsonl",
        "/u/.commandcode/projects/p/s.prompts.jsonl",
        "/u/.commandcode/projects/p/mcp.json",
    ] {
        assert!(!CommandCode.file_matches(Path::new(path)), "{path}");
    }
    // None of them is a Claude Code transcript.
    for path in [file(S1), sidecar] {
        assert!(is_under_commandcode(&path));
        assert!(!super::super::claude_code::ClaudeCode.file_matches(&path));
    }
}

#[test]
fn the_header_names_the_working_directory() {
    let h = header(&file(S1)).unwrap();
    assert_eq!(h.id, S1);
    assert_eq!(h.cwd.as_deref(), Some(Path::new("/work/example")));
    assert_eq!(header(&file(OLD)), None);
}

#[test]
fn another_version_or_no_header_is_not_read() {
    let err = CommandCode
        .parse(Path::new(PROJECT), &file(OLD))
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("Command Code session format v2 is not one hippo reads"),
        "{err}"
    );
    let dir = tempfile::tempdir().unwrap();
    let path = write(
        dir.path(),
        &[
            json!({"type": "message", "timestamp": "2026-11-01T12:00:00Z",
                 "message": {"role": "user", "content": "hi"}}),
        ],
    );
    let err = CommandCode.parse(dir.path(), &path).unwrap_err();
    assert!(err.to_string().contains("no session header"), "{err}");
}

#[test]
fn string_content_and_unknown_tool_results_are_read() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(
        dir.path(),
        &[
            json!({"type": "session", "version": 3, "id": "s", "timestamp": "2026-11-01T12:00:00Z"}),
            json!({"type": "message", "timestamp": "2026-11-01T12:00:01Z",
                   "message": {"role": "user", "content": "plain"}}),
            json!({"type": "message", "timestamp": "2026-11-01T12:00:02Z",
                   "message": {"role": "user", "content": [
                       {"type": "tool_result", "tool_use_id": "nope", "content": "out"}]}}),
            json!({"type": "message", "message": {"role": "user", "content": "no timestamp"}}),
            json!({"type": "message", "timestamp": "2026-11-01T12:00:03Z",
                   "message": {"role": "system", "content": "skipped"}}),
        ],
    );
    let p = CommandCode.parse(dir.path(), &path).unwrap();
    assert_eq!(
        shape(&p),
        [(Role::User, None, "plain"), (Role::Tool, None, "out")]
    );
    assert_eq!(header(&path).unwrap().cwd, None);
}
