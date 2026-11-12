use super::*;
use std::fs;

use serde_json::json;
use time::macros::datetime;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/muse");
const S1: &str = "01a1d0f0-0000-7000-8000-000000000001";

fn log() -> PathBuf {
    PathBuf::from(FIXTURES)
        .join("2026/11/01")
        .join(S1)
        .join("session.jsonl")
}

/// `(role, tool name, text)` of each turn.
fn shape(p: &Parsed) -> Vec<(Role, Option<&str>, &str)> {
    p.turns
        .iter()
        .map(|t| (t.role, t.tool_name.as_deref(), t.text.as_str()))
        .collect()
}

/// Writes `lines` as `<dir>/2026/11/02/<id>/session.jsonl`.
fn write_log(dir: &Path, id: &str, lines: &[Value]) -> PathBuf {
    let path = dir.join("2026/11/02").join(id).join("session.jsonl");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    fs::write(&path, text).unwrap();
    path
}

fn envelope(id: &str, micros: i64, payload_type: &str, payload: Value) -> Value {
    json!({"schema_version": 1, "stream": {"kind": "session", "id": id}, "recorded_at": micros,
           "record_type": "event", "payload_type": payload_type, "payload_schema_version": 1,
           "payload": payload})
}

fn run_event(id: &str, micros: i64, event: Value) -> Value {
    envelope(
        id,
        micros,
        "runtime.session",
        json!({"kind": "run", "run_id": "r", "event": event}),
    )
}

#[test]
fn the_fixture_parses_into_its_turns() {
    let p = Muse.parse(Path::new(FIXTURES), &log()).unwrap();
    assert_eq!(p.id, format!("muse-{S1}"));
    assert_eq!(p.tool, Tool::Muse);
    assert_eq!(p.started, datetime!(2026-11-01 11:00:00 UTC));
    let turns = shape(&p);
    assert_eq!(turns.len(), 6, "{turns:#?}");
    assert_eq!(turns[0].0, Role::User);
    assert!(turns[0].2.starts_with("Fix the retry backoff"));
    assert_eq!(
        turns[1],
        (
            Role::Tool,
            Some("read_file"),
            r#"read_file {"path": "src/retry.ts"}"#
        )
    );
    assert_eq!(turns[2].1, Some("run_shell"));
    assert_eq!(turns[3].0, Role::Tool);
    assert_eq!(turns[3].1, Some("read_file"));
    assert!(turns[3].2.starts_with("Read text file `src/retry.ts`."));
    assert_eq!(
        turns[4],
        (Role::Tool, Some("run_shell"), "1:const backoffMs = 10;")
    );
    assert_eq!(
        turns[5],
        (
            Role::Assistant,
            None,
            "The backoff starts at 10 ms. Raise it to 500 ms and double it each retry."
        )
    );
    assert_eq!(p.turns[1].ts, datetime!(2026-11-01 11:00:00.2 UTC));
    // Nothing from the system prompt, tasks, or the future payload version.
    assert!(p.turns.iter().all(|t| !t.text.contains("Never indexed")));
}

#[test]
fn only_top_level_logs_in_dated_folders_match() {
    assert!(Muse.file_matches(&log()));
    let sub = log()
        .parent()
        .unwrap()
        .join("subagent/01a1d0f0-0000-7000-8000-000000000002/session.jsonl");
    assert!(sub.exists());
    assert!(!Muse.file_matches(&sub));
    // Neither log is the Claude Code adapter's.
    assert!(is_muse_file(&log()) && is_muse_file(&sub));
    assert!(!super::super::claude_code::ClaudeCode.file_matches(&log()));
    assert!(!super::super::claude_code::ClaudeCode.file_matches(&sub));
    for path in [
        "/s/2026/11/01/x/other.jsonl",
        "/s/2026/11/x/session.jsonl",
        "/s/26/11/01/x/session.jsonl",
        "/s/.msp-view-g12/x/session.jsonl",
    ] {
        assert!(!Muse.file_matches(Path::new(path)), "{path}");
    }
}

#[test]
fn the_workspace_root_comes_from_the_metadata_record() {
    assert_eq!(workspace_root(&log()), Some(PathBuf::from("/work/example")));
    let dir = tempfile::tempdir().unwrap();
    let path = write_log(
        dir.path(),
        "s",
        &[run_event(
            "s",
            1,
            json!({"kind": "started", "prompt": "hi"}),
        )],
    );
    assert_eq!(workspace_root(&path), None);
    assert_eq!(workspace_root(&dir.path().join("gone.jsonl")), None);
}

#[test]
fn other_versions_are_skipped_and_a_log_without_a_session_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let mut future = run_event(
        "s",
        2_000_000,
        json!({"kind": "started", "prompt": "future"}),
    );
    future["schema_version"] = json!(2);
    let frame = json!({"retained_frame": "x", "frame_schema_version": 9, "children": [
        {"child_index": 0, "record_json": run_event("s", 3_000_000, json!({"kind": "started", "prompt": "framed"})).to_string()}]});
    let path = write_log(
        dir.path(),
        "s",
        &[
            json!({"synthetic": true}),
            future,
            frame,
            run_event("s", 1_000_000, json!({"kind": "started", "prompt": "kept"})),
        ],
    );
    let p = Muse.parse(dir.path(), &path).unwrap();
    assert_eq!(p.id, "muse-s");
    assert_eq!(p.started, datetime!(1970-01-01 00:00:01 UTC));
    assert_eq!(shape(&p), [(Role::User, None, "kept")]);

    // A tool result whose call was never seen is unnamed.
    let path = write_log(
        dir.path(),
        "t",
        &[run_event(
            "t",
            1,
            json!({"kind": "tool_result_batch_committed",
            "results": [{"tool_call_id": "nope", "text": "out"}]}),
        )],
    );
    assert_eq!(
        shape(&Muse.parse(dir.path(), &path).unwrap()),
        [(Role::Tool, None, "out")]
    );

    let path = write_log(
        dir.path(),
        "u",
        &[json!({"synthetic": true}), json!({"schema_version": 1})],
    );
    let err = Muse.parse(dir.path(), &path).unwrap_err();
    assert!(err.to_string().contains("names a session"), "{err}");
}

#[test]
fn the_default_root_follows_xdg_data_home() {
    // Read only: the test does not set the variable, which other tests
    // share.
    let expected = match std::env::var_os("XDG_DATA_HOME").map(PathBuf::from) {
        Some(dir) if dir.is_absolute() => dir.join("muse/sessions").display().to_string(),
        _ => DEFAULT_ROOT.to_owned(),
    };
    assert_eq!(default_root(), expected);
}
