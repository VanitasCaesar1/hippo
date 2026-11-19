use super::*;
use std::fs;

use time::macros::datetime;

const PROJECT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/gemini/.gemini/tmp/work-example"
);
const S1: &str = "5e550000-0000-4000-8000-000000000001";

fn chats() -> PathBuf {
    Path::new(PROJECT).join("chats")
}

/// `(role, tool name, text)` of each turn.
fn shape(p: &Parsed) -> Vec<(Role, Option<&str>, &str)> {
    p.turns
        .iter()
        .map(|t| (t.role, t.tool_name.as_deref(), t.text.as_str()))
        .collect()
}

#[test]
fn the_json_lines_session_parses_into_its_turns() {
    let path = chats().join("session-2026-11-01T09-00-5e550000.jsonl");
    let p = Gemini.parse(Path::new(PROJECT), &path).unwrap();
    assert_eq!(p.id, format!("gemini-{S1}"));
    assert_eq!(p.tool, Tool::Gemini);
    assert_eq!(p.started, datetime!(2026-11-01 09:00:00 UTC));
    let turns = shape(&p);
    assert_eq!(turns.len(), 7, "{turns:#?}");
    assert_eq!(turns[0].0, Role::User);
    assert!(turns[0]
        .2
        .starts_with("Fix the retry backoff in the example webhook sender."));
    assert_eq!(
        turns[1],
        (Role::Assistant, None, "I'll read the sender first.")
    );
    // The message was written twice as its calls finished: read once, as
    // last written.
    assert_eq!(
        turns[2],
        (
            Role::Tool,
            Some("read_file"),
            r#"read_file {"absolute_path":"/work/example/src/retry.ts"}"#
        )
    );
    assert_eq!(
        turns[3],
        (Role::Tool, Some("read_file"), "const backoffMs = 10;")
    );
    assert_eq!(turns[4].1, Some("run_shell_command"));
    assert_eq!(
        turns[5],
        (Role::Tool, Some("run_shell_command"), "Exit Code: 1")
    );
    // The info message is skipped.
    assert_eq!(
        turns[6],
        (
            Role::Assistant,
            None,
            "The backoff now doubles up to 30 seconds."
        )
    );
    assert_eq!(p.turns[0].ts, datetime!(2026-11-01 09:00:01 UTC));
    assert_eq!(p.turns[5].ts, datetime!(2026-11-01 09:00:09 UTC));
}

#[test]
fn an_older_json_session_and_a_sub_agent_are_sessions() {
    let old = chats().join("session-2026-10-30T10-00-5e550000.json");
    let p = Gemini.parse(Path::new(PROJECT), &old).unwrap();
    assert_eq!(p.id, "gemini-5e550000-0000-4000-8000-000000000003");
    assert_eq!(p.started, datetime!(2026-10-30 10:00:00 UTC));
    assert_eq!(
        shape(&p),
        [
            (Role::User, None, "Which port does the example server use?"),
            (Role::Assistant, None, "Port 8080, set in server.ts."),
        ]
    );
    let sub = chats()
        .join(S1)
        .join("5e550000-0000-4000-8000-000000000002.jsonl");
    let p = Gemini.parse(Path::new(PROJECT), &sub).unwrap();
    assert_eq!(
        p.id,
        format!("gemini-{S1}-agent-5e550000-0000-4000-8000-000000000002")
    );
    assert_eq!(
        shape(&p)[2..],
        [
            (
                Role::Tool,
                Some("search_file_content"),
                r#"search_file_content {"pattern":"sleep"}"#
            ),
            (Role::Tool, Some("search_file_content"), "Found 1 match"),
        ]
    );
}

#[test]
fn files_are_claimed_by_their_place() {
    for (path, takes) in [
        (
            "h/.gemini/tmp/p/chats/session-2026-11-01T09-00-abcd.jsonl",
            true,
        ),
        (
            "h/.gemini/tmp/p/chats/session-2026-11-01T09-00-abcd.json",
            true,
        ),
        ("h/.gemini/tmp/p/chats/abcd/efgh.jsonl", true),
        ("h/.gemini/tmp/p/chats/notes.json", false),
        ("h/.gemini/tmp/p/logs.json", false),
        ("h/.gemini/tmp/p/checkpoint-tag.json", false),
        ("h/.gemini/tmp/p/checkpoints/x.json", false),
        ("h/other/tmp/p/chats/session-1.jsonl", false),
    ] {
        assert_eq!(is_session(Path::new(path)), takes, "{path}");
    }
    let claude = super::super::claude_code::ClaudeCode;
    assert!(!claude.file_matches(&chats().join("session-2026-11-01T09-00-5e550000.jsonl")));
    assert!(!claude.file_matches(
        &chats()
            .join(S1)
            .join("5e550000-0000-4000-8000-000000000002.jsonl")
    ));
}

#[test]
fn the_directory_comes_from_project_root() {
    let path = chats().join("session-2026-11-01T09-00-5e550000.jsonl");
    assert_eq!(project_root(&path), Some(PathBuf::from("/work/example")));
    let dir = tempfile::tempdir().unwrap();
    let hashed = dir
        .path()
        .join(".gemini/tmp/0123abcd/chats/session-1.jsonl");
    fs::create_dir_all(hashed.parent().unwrap()).unwrap();
    assert_eq!(project_root(&hashed), None);
}

#[test]
fn a_set_with_messages_replaces_them_and_bad_files_are_errors() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".gemini/tmp/p/chats/session-x.jsonl");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        "{\"sessionId\":\"s\",\"startTime\":\"2026-11-01T00:00:00Z\"}\n\
         {\"id\":\"a\",\"type\":\"user\",\"content\":\"first\"}\n\
         {\"$set\":{\"messages\":[{\"id\":\"b\",\"type\":\"user\",\"content\":\"kept\"}]}}\n\
         {\"$rewindTo\":\"b\"}\n",
    )
    .unwrap();
    let p = Gemini.parse(dir.path(), &path).unwrap();
    assert_eq!(shape(&p), [(Role::User, None, "kept")]);
    fs::write(&path, "{\"other\": 1}\n").unwrap();
    assert!(Gemini.parse(dir.path(), &path).is_err());
}
