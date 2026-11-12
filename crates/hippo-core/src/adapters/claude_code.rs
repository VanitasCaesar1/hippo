//! Claude Code transcripts: every `*.jsonl` other than a Codex rollout under
//! a transcript root, nested sub-agent transcripts included, each its own
//! session. A Workflow's `workflows/<run>/journal.jsonl` is not a
//! transcript and is not read.
//!
//! Each line is a JSON record. Records of type `user` and `assistant` carry
//! `sessionId`, `timestamp` and `message.content`; every other type is
//! skipped, and so is a `user` or `assistant` record without a usable
//! timestamp. Those are the kept records, and `message.content` becomes
//! turns:
//!
//! - a string is one turn with the record's role;
//! - a `text` block is one turn with the record's role;
//! - a `tool_use` block is one `tool` turn named by its `name`, whose text
//!   is `<name> <input as compact JSON>`;
//! - a `tool_result` block is one `tool` turn named by the `tool_use` with
//!   the same id earlier in the file (or unnamed), whose text is `content`
//!   if that is a string, else its `text` blocks joined by newlines;
//! - `thinking`, `redacted_thinking`, images and any other block are not
//!   turns.
//!
//! One `attachment` record is kept too: a
//! `queued_command` whose `prompt` is a string starting
//! `<task-notification>` is a `user` turn with the prompt as its text, as
//! older clients wrote a background task's end. Every other attachment is
//! skipped.
//!
//! A file directly under `projects/<p>/` (or under the root itself) is a
//! session named by its records' `sessionId`; a file deeper than that is a
//! sub-agent transcript named `<sessionId>-<file stem>`. Should a file's
//! records carry several `sessionId`s, as the replayed history of a resumed
//! session may, the one equal to the file's stem names it, else the first
//! one. A file with no `sessionId` in any kept record is an error.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;
use time::OffsetDateTime;

use super::{
    each_json_line, has_extension_jsonl, invalid_data, is_rollout, is_workflow_journal, push_turn,
    timestamp, transcript_roots, Adapter, Parsed, RawTurn, Role, Tool,
};
use crate::config::Config;
use crate::error::Result;

/// The Claude Code adapter.
#[derive(Debug, Clone, Copy, Default)]
pub struct ClaudeCode;

impl Adapter for ClaudeCode {
    fn tool(&self) -> Tool {
        Tool::ClaudeCode
    }

    fn roots(&self, cfg: &Config) -> Vec<PathBuf> {
        transcript_roots(cfg)
    }

    fn file_matches(&self, path: &Path) -> bool {
        has_extension_jsonl(path) && !is_rollout(path) && !is_workflow_journal(path)
    }

    fn revision(&self, bytes: &[u8]) -> Option<&'static str> {
        revision(bytes)
    }

    fn parse(&self, root: &Path, path: &Path) -> Result<Parsed> {
        // `<p>/<file>.jsonl` has two components under the root; anything
        // deeper is a sub-agent transcript.
        let relative = path
            .strip_prefix(root)
            .map_err(|_| invalid_data(path, "not under the transcript root"))?;
        let nested = relative.components().count() > 2;
        let stem = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();

        let mut started = None;
        let mut session_id: Option<String> = None;
        let mut stem_is_a_session_id = false;
        let mut tool_names = HashMap::new();
        let mut turns = Vec::new();
        each_json_line(path, |record| {
            let role = match record.get("type").and_then(Value::as_str) {
                Some("user") => Role::User,
                Some("assistant") => Role::Assistant,
                Some("attachment") => {
                    if let (Some(text), Some(ts)) =
                        (notification(&record), timestamp(record.get("timestamp")))
                    {
                        started.get_or_insert(ts);
                        push_turn(&mut turns, Role::User, ts, None, text.to_owned());
                    }
                    return;
                }
                _ => return,
            };
            let Some(ts) = timestamp(record.get("timestamp")) else {
                tracing::warn!(
                    "{}: skipping a {} record without a usable timestamp",
                    path.display(),
                    role.as_str()
                );
                return;
            };
            started.get_or_insert(ts);
            let id = record.get("sessionId").and_then(Value::as_str);
            if let Some(id) = id.filter(|id| !id.is_empty()) {
                session_id.get_or_insert_with(|| id.to_owned());
                stem_is_a_session_id |= id == stem;
            }
            match record.pointer("/message/content") {
                Some(Value::String(text)) => push_turn(&mut turns, role, ts, None, text.clone()),
                Some(Value::Array(blocks)) => {
                    for block in blocks {
                        add_block(&mut turns, &mut tool_names, role, ts, block);
                    }
                }
                _ => {}
            }
        })?;

        let (Some(started), Some(session_id)) = (started, session_id) else {
            return Err(invalid_data(
                path,
                "no sessionId in any user or assistant record",
            ));
        };
        // A file named for one of the sessionIds in its records is that
        // session, however many others it carries (a resumed session may
        // replay older records); otherwise the first sessionId names it.
        let id = if nested {
            format!("{session_id}-{stem}")
        } else if stem_is_a_session_id {
            stem
        } else {
            session_id
        };
        Ok(Parsed {
            id,
            tool: Tool::ClaudeCode,
            started,
            turns,
        })
    }
}

/// The prompt of a `queued_command` attachment that is a background
/// task's notification.
fn notification(record: &Value) -> Option<&str> {
    let a = record.get("attachment")?;
    if a.get("type").and_then(Value::as_str) != Some("queued_command") {
        return None;
    }
    a.get("prompt")
        .and_then(Value::as_str)
        .filter(|p| p.starts_with("<task-notification>"))
}

/// What a file's bytes need of the adapter beyond the first one:
/// a file holding a `queued_command` attachment is read
/// again once by an ingest that knows them.
pub fn revision(bytes: &[u8]) -> Option<&'static str> {
    const NEEDLE: &[u8] = b"\"queued_command\"";
    bytes
        .windows(NEEDLE.len())
        .any(|w| w == NEEDLE)
        .then_some("queued-1")
}

/// Adds the turn for one `message.content` block, if it makes one.
/// `tool_names` maps each `tool_use` id seen so far to its tool's name.
fn add_block(
    turns: &mut Vec<RawTurn>,
    tool_names: &mut HashMap<String, String>,
    role: Role,
    ts: OffsetDateTime,
    block: &Value,
) {
    match block.get("type").and_then(Value::as_str) {
        Some("text") => {
            if let Some(text) = block.get("text").and_then(Value::as_str) {
                push_turn(turns, role, ts, None, text.to_owned());
            }
        }
        Some("tool_use") => {
            let Some(name) = block.get("name").and_then(Value::as_str) else {
                return;
            };
            if let Some(id) = block.get("id").and_then(Value::as_str) {
                tool_names.insert(id.to_owned(), name.to_owned());
            }
            // `Value` displays as compact JSON.
            let text = match block.get("input") {
                Some(input) => format!("{name} {input}"),
                None => name.to_owned(),
            };
            push_turn(turns, Role::Tool, ts, Some(name.to_owned()), text);
        }
        Some("tool_result") => {
            let tool_name = block
                .get("tool_use_id")
                .and_then(Value::as_str)
                .and_then(|id| tool_names.get(id))
                .cloned();
            let text = result_text(block.get("content"));
            push_turn(turns, Role::Tool, ts, tool_name, text);
        }
        _ => {}
    }
}

/// The text of a `tool_result`'s `content`: the string itself, or the `text`
/// blocks of an array joined by newlines.
fn result_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, io};

    use serde_json::json;
    use time::macros::datetime;
    use time::UtcOffset;

    use crate::error::Error;

    const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures");

    /// Parses a fixture with `fixtures/` as the transcript root.
    fn fixture(name: &str) -> Parsed {
        let root = Path::new(FIXTURES);
        ClaudeCode.parse(root, &root.join(name)).unwrap()
    }

    /// Line `n` (from 1) of `fixtures/redaction/leaks.txt`.
    fn leak(n: usize) -> String {
        let leaks = fs::read_to_string(format!("{FIXTURES}/redaction/leaks.txt")).unwrap();
        leaks.lines().nth(n - 1).unwrap().to_owned()
    }

    /// Writes `lines` as a transcript at `<root>/<relative>`.
    fn write(root: &Path, relative: &str, lines: &[Value]) -> PathBuf {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let text: String = lines.iter().map(|line| format!("{line}\n")).collect();
        fs::write(&path, text).unwrap();
        path
    }

    fn user(session: &str, ts: &str, content: Value) -> Value {
        json!({
            "type": "user",
            "sessionId": session,
            "timestamp": ts,
            "message": {"role": "user", "content": content},
        })
    }

    fn assistant(session: &str, ts: &str, content: Value) -> Value {
        json!({
            "type": "assistant",
            "sessionId": session,
            "timestamp": ts,
            "message": {"role": "assistant", "content": content},
        })
    }

    fn is_invalid_data(err: &Error) -> bool {
        matches!(err, Error::Io(e) if e.kind() == io::ErrorKind::InvalidData)
    }

    #[test]
    fn a_queued_task_notification_is_a_user_turn_and_is_redacted() {
        let dir = tempfile::tempdir().unwrap();
        let key = leak(1);
        let prompt = format!(
            "<task-notification>\n<task-id>a1</task-id>\n<status>completed</status>\n<summary>Agent \"Audit\" finished</summary>\n<result>Found {key} in the log.</result>\n</task-notification>"
        );
        let path = write(
            dir.path(),
            "p/s1.jsonl",
            &[
                user("s1", "2026-11-08T08:00:00Z", json!("Audit the logs")),
                json!({"type": "attachment", "sessionId": "s1", "timestamp": "2026-11-08T08:05:00Z",
                       "attachment": {"type": "queued_command", "prompt": prompt}}),
                json!({"type": "attachment", "sessionId": "s1", "timestamp": "2026-11-08T08:06:00Z",
                       "attachment": {"type": "queued_command", "prompt": "a queued message"}}),
                json!({"type": "attachment", "sessionId": "s1", "timestamp": "2026-11-08T08:07:00Z",
                       "attachment": {"type": "hook_success", "stdout": "<task-notification>"}}),
            ],
        );
        let parsed = ClaudeCode.parse(dir.path(), &path).unwrap();
        assert_eq!(parsed.turns.len(), 2);
        let t = &parsed.turns[1];
        assert_eq!(t.role, Role::User);
        assert_eq!(t.ts, datetime!(2026-11-08 08:05:00 UTC));
        assert!(t.text.starts_with("<task-notification>"));
        let stored = crate::index::Turn::from_raw(t.clone());
        assert!(
            !stored.text.as_str().contains(&key),
            "the key was not redacted"
        );
        assert!(stored
            .text
            .as_str()
            .contains("<summary>Agent \"Audit\" finished</summary>"));

        let bytes = fs::read(&path).unwrap();
        assert_eq!(revision(&bytes), Some("queued-1"));
        assert_eq!(revision(b"{\"type\":\"user\"}"), None);
    }

    #[test]
    fn the_fixture_parses_into_its_turns() {
        let parsed = fixture("claude-code/synthetic.jsonl");
        assert_eq!(parsed.id, "stub-1");
        assert_eq!(parsed.tool, Tool::ClaudeCode);
        // The summary and the system record before it are not kept records.
        assert_eq!(parsed.started, datetime!(2026-11-01 10:00:00 UTC));

        // Role, tool name, timestamp, the start of the text, and the
        // leaks.txt lines the text must hold. A turn that holds none is
        // compared whole.
        let expected = [
            (
                Role::User,
                None,
                datetime!(2026-11-01 10:00:00 UTC),
                "Fix the retry backoff in the example webhook sender. It retries too fast.\n\
                 I pasted some config by mistake:\n",
                vec![1, 2, 3],
            ),
            (
                Role::Assistant,
                None,
                datetime!(2026-11-01 10:00:04 UTC),
                "I'll ask a sub-agent where the sender sleeps between retries.",
                vec![],
            ),
            (
                Role::Tool,
                Some("Task"),
                datetime!(2026-11-01 10:00:06 UTC),
                r#"Task {"description":"Find the sleep between retries","prompt":"Find every place the sender sleeps between retries and report the file and line.","subagent_type":"general-purpose"}"#,
                vec![],
            ),
            (
                Role::Tool,
                Some("Task"),
                datetime!(2026-11-01 10:00:41 UTC),
                "The only sleep is in src/sender.py line 12, driven by RETRY_DELAY.",
                vec![],
            ),
            (
                Role::Assistant,
                None,
                datetime!(2026-11-01 10:00:45 UTC),
                "The file header lists the owner's contact details, which I will leave out of the fix:\n",
                vec![13, 14, 15],
            ),
            (
                Role::Tool,
                Some("Bash"),
                datetime!(2026-11-01 10:00:45 UTC),
                r#"Bash {"command":"grep -rn '"#,
                vec![4],
            ),
            (
                Role::Tool,
                Some("Bash"),
                datetime!(2026-11-01 10:00:47 UTC),
                "src/sender.py:3:RETRY_DELAY = 0\n",
                vec![6, 7, 8, 9, 10],
            ),
            (
                Role::User,
                None,
                datetime!(2026-11-01 10:01:30 UTC),
                "Here is the command I ran:\n",
                vec![16],
            ),
            (
                Role::Assistant,
                None,
                datetime!(2026-11-01 10:01:33 UTC),
                "Thanks. I will echo the key shape once more to compare.",
                vec![],
            ),
            (
                Role::Tool,
                Some("Bash"),
                datetime!(2026-11-01 10:01:35 UTC),
                r#"Bash {"command":"echo '"#,
                vec![5],
            ),
            (
                Role::Tool,
                Some("Bash"),
                datetime!(2026-11-01 10:01:36 UTC),
                "provider sk-ant-api03-",
                vec![5, 12, 11],
            ),
            (
                Role::Assistant,
                None,
                datetime!(2026-11-01 10:01:40 UTC),
                "Backoff now doubles from 2 seconds. Done.",
                vec![],
            ),
            (
                Role::User,
                None,
                datetime!(2026-11-01 10:02:00 UTC),
                "Thanks, that works.",
                vec![],
            ),
        ];
        assert_eq!(parsed.turns.len(), expected.len());
        for (i, (turn, (role, tool_name, ts, text, leaks))) in
            parsed.turns.iter().zip(expected).enumerate()
        {
            assert_eq!(
                usize::try_from(turn.ordinal).unwrap(),
                i,
                "ordinal of turn {i}"
            );
            assert_eq!(turn.role, role, "role of turn {i}");
            assert_eq!(
                turn.tool_name.as_deref(),
                tool_name,
                "tool name of turn {i}"
            );
            assert_eq!(turn.ts, ts, "timestamp of turn {i}");
            if leaks.is_empty() {
                assert_eq!(turn.text, text, "text of turn {i}");
            }
            assert!(turn.text.starts_with(text), "turn {i}: {}", turn.text);
            for n in leaks {
                assert!(turn.text.contains(&leak(n)), "turn {i} lacks leak {n}");
            }
        }

        let count = |role| parsed.turns.iter().filter(|turn| turn.role == role).count();
        assert_eq!(count(Role::User), 3);
        assert_eq!(count(Role::Assistant), 4);
        assert_eq!(count(Role::Tool), 6);
        let calls = parsed.turns.iter().filter(|turn| {
            let name = turn.tool_name.as_deref().unwrap_or_default();
            turn.role == Role::Tool && turn.text.starts_with(&format!("{name} {{"))
        });
        assert_eq!(
            calls.count(),
            3,
            "three tool_use turns, three tool_result turns"
        );

        // A tool_result with array content joins its text blocks with a
        // newline and drops the image between them.
        let joined = (6..=10).map(leak).collect::<Vec<_>>().join("\n");
        assert_eq!(
            parsed.turns[6].text,
            format!("src/sender.py:3:RETRY_DELAY = 0\n{joined}")
        );

        // Nothing from a skipped record or block reaches a turn.
        let all = parsed
            .turns
            .iter()
            .map(|turn| turn.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        for never in [
            "probably retries",
            "Webhook retry backoff",
            "Session started",
            "Structured copy",
            "iVBORw0KGgo",
            "c3ludGhldGlj",
        ] {
            assert!(!all.contains(never), "{never}");
        }
    }

    #[test]
    fn the_nested_fixture_is_a_session_of_its_own() {
        let parsed = fixture("claude-code/stub-1/subagents/agent-a1.jsonl");
        assert_eq!(parsed.id, "stub-1-agent-a1");
        assert_eq!(parsed.tool, Tool::ClaudeCode);
        assert_eq!(parsed.started, datetime!(2026-11-01 10:00:07 UTC));
        let turns: Vec<_> = parsed
            .turns
            .iter()
            .map(|turn| {
                (
                    turn.ordinal,
                    turn.role,
                    turn.tool_name.as_deref(),
                    &*turn.text,
                )
            })
            .collect();
        assert_eq!(
            turns,
            [
                (
                    0,
                    Role::User,
                    None,
                    "Find every place the sender sleeps between retries and report the file and line."
                ),
                (1, Role::Assistant, None, "Searching the sender module for sleep calls."),
                (
                    2,
                    Role::Tool,
                    Some("Grep"),
                    r#"Grep {"output_mode":"content","path":"/work/example/src","pattern":"sleep"}"#
                ),
                (
                    3,
                    Role::Tool,
                    Some("Grep"),
                    "src/sender.py:12:    time.sleep(RETRY_DELAY)"
                ),
                (
                    4,
                    Role::Assistant,
                    None,
                    "The only sleep is in src/sender.py line 12, driven by RETRY_DELAY."
                ),
            ]
        );
    }

    #[test]
    fn the_path_under_the_root_decides_between_a_session_and_a_sub_agent() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let hello = |session| [user(session, "2026-11-01T10:00:00Z", json!("hello"))];
        let top = write(root, "proj/abc.jsonl", &hello("abc"));
        let nested = write(root, "proj/abc/subagents/agent-1.jsonl", &hello("abc"));
        let shallow = write(root, "proj/abc/agent-3.jsonl", &hello("abc"));
        let deeper = write(root, "proj/abc/a/b/c/agent-2.jsonl", &hello("abc"));
        let loose = write(root, "loose.jsonl", &hello("loose"));
        let id = |path| ClaudeCode.parse(root, path).unwrap().id;
        assert_eq!(id(&top), "abc");
        assert_eq!(id(&nested), "abc-agent-1");
        // One level below `<p>/` is already deeper than `<p>/<file>.jsonl`.
        assert_eq!(id(&shallow), "abc-agent-3");
        assert_eq!(id(&deeper), "abc-agent-2");
        // Not deeper than `<p>/<file>.jsonl`, so not nested.
        assert_eq!(id(&loose), "loose");

        // The root only says how deep a file is; a file elsewhere is refused.
        let elsewhere = tempfile::tempdir().unwrap();
        let outside = write(elsewhere.path(), "proj/abc.jsonl", &hello("abc"));
        let err = ClaudeCode.parse(root, &outside).unwrap_err();
        assert!(is_invalid_data(&err), "{err}");
    }

    #[test]
    fn a_file_named_for_one_of_its_session_ids_is_that_session() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let replayed = [
            user("old", "2026-11-01T10:00:00Z", json!("replayed")),
            user("new", "2026-11-01T10:00:05Z", json!("fresh")),
        ];
        let resumed = write(root, "proj/new.jsonl", &replayed);
        assert_eq!(ClaudeCode.parse(root, &resumed).unwrap().id, "new");
        // No record names the file's stem: the first sessionId does.
        let other = write(root, "proj/other.jsonl", &replayed);
        assert_eq!(ClaudeCode.parse(root, &other).unwrap().id, "old");
        // A nested file is `<sessionId>-<stem>` whatever its stem is.
        let nested = write(root, "proj/old/subagents/new.jsonl", &replayed);
        assert_eq!(ClaudeCode.parse(root, &nested).unwrap().id, "old-new");
    }

    #[test]
    fn a_file_without_a_session_id_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let at = "2026-11-01T10:00:00Z";
        let cases = [
            ("empty", vec![]),
            (
                "other types only",
                vec![
                    json!({"type": "summary", "summary": "x", "sessionId": "s"}),
                    json!({"type": "system", "sessionId": "s", "timestamp": at}),
                ],
            ),
            (
                "no sessionId",
                vec![
                    json!({"type": "system", "sessionId": "ignored", "timestamp": at}),
                    json!({"type": "user", "timestamp": at, "message": {"content": "hi"}}),
                ],
            ),
            ("empty sessionId", vec![user("", at, json!("hi"))]),
            (
                "not a string",
                vec![
                    json!({"type": "user", "sessionId": 7, "timestamp": at, "message": {"content": "hi"}}),
                ],
            ),
            (
                "only on a record without a timestamp",
                vec![json!({"type": "user", "sessionId": "s", "message": {"content": "hi"}})],
            ),
        ];
        for (name, lines) in cases {
            let path = write(
                root,
                &format!("proj/{}.jsonl", name.replace(' ', "-")),
                &lines,
            );
            let err = ClaudeCode.parse(root, &path).unwrap_err();
            assert!(is_invalid_data(&err), "{name}: {err}");
            assert!(err.to_string().contains("no sessionId"), "{name}: {err}");
            assert!(
                err.to_string().contains(path.to_str().unwrap()),
                "{name}: {err}"
            );
        }
    }

    #[test]
    fn a_missing_file_is_an_io_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("proj/gone.jsonl");
        let err = ClaudeCode.parse(dir.path(), &path).unwrap_err();
        assert!(
            matches!(&err, Error::Io(e) if e.kind() == io::ErrorKind::NotFound),
            "{err}"
        );
    }

    #[test]
    fn bad_lines_and_bad_records_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("proj/messy.jsonl");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let first = user("messy", "2026-11-01T10:00:00Z", json!("first"));
        let second = assistant(
            "messy",
            "2026-11-01T10:00:05Z",
            json!([{"type": "text", "text": "second"}]),
        );
        let no_ts =
            json!({"type": "user", "sessionId": "messy", "message": {"content": "no timestamp"}});
        let bad_ts = user("messy", "yesterday", json!("bad timestamp"));
        let number_ts = user("messy", "1762000000", json!("string number"));
        let mut bytes = Vec::new();
        // CRLF line ends, blank lines, the marker line, a line of text, a
        // line that is not UTF-8, a cut-off record, and a partly written
        // last line with no newline.
        bytes.extend_from_slice(b"{\"synthetic\": true}\r\n\r\n   \n");
        bytes.extend_from_slice(format!("{first}\r\n").as_bytes());
        bytes.extend_from_slice(b"this is not json\n\xff\xfe{\"type\":\"user\"}\n");
        bytes.extend_from_slice(
            b"{\"type\":\"user\",\"sessionId\":\"messy\",\"message\":{\"content\":\"cut\n",
        );
        for record in [&no_ts, &bad_ts, &number_ts, &second] {
            bytes.extend_from_slice(format!("{record}\n").as_bytes());
        }
        bytes.extend_from_slice(b"{\"type\":\"assistant\",\"sessionId\":\"messy\",\"message\":{\"content\":\"partly wri");
        fs::write(&path, bytes).unwrap();

        let parsed = ClaudeCode.parse(dir.path(), &path).unwrap();
        assert_eq!(parsed.id, "messy");
        assert_eq!(parsed.started, datetime!(2026-11-01 10:00:00 UTC));
        let turns: Vec<_> = parsed
            .turns
            .iter()
            .map(|turn| (turn.ordinal, turn.role, &*turn.text))
            .collect();
        assert_eq!(
            turns,
            [(0, Role::User, "first"), (1, Role::Assistant, "second")]
        );
    }

    #[test]
    fn tool_results_take_the_name_of_the_tool_use_before_them() {
        let dir = tempfile::tempdir().unwrap();
        let at = "2026-11-01T10:00:00Z";
        let path = write(
            dir.path(),
            "proj/tools.jsonl",
            &[
                // A result before its call is unnamed.
                user(
                    "tools",
                    at,
                    json!([{"type": "tool_result", "tool_use_id": "c1", "content": "early"}]),
                ),
                assistant(
                    "tools",
                    at,
                    json!([
                        {"type": "tool_use", "id": "c1", "name": "Read", "input": {"file_path": "/a", "limit": 5}},
                        {"type": "tool_use", "id": "c2", "name": "Bash", "input": {"command": "ls"}},
                        {"type": "tool_use", "id": "c3", "name": "Ping"},
                        {"type": "tool_use", "id": "c4", "input": {"no": "name"}},
                        {"type": "text"},
                    ]),
                ),
                user(
                    "tools",
                    at,
                    json!([
                        {"type": "tool_result", "tool_use_id": "c2", "content": "two"},
                        {"type": "tool_result", "tool_use_id": "c1", "content": [
                            {"type": "text", "text": "a"},
                            {"type": "image", "source": {}},
                            {"type": "text", "text": "b"},
                        ]},
                        {"type": "tool_result", "tool_use_id": "nobody", "content": "orphan"},
                        {"type": "tool_result", "content": "no id"},
                        {"type": "tool_result", "tool_use_id": "c1"},
                        {"type": "tool_result", "tool_use_id": "c4", "content": "its call had no name"},
                    ]),
                ),
            ],
        );
        let parsed = ClaudeCode.parse(dir.path(), &path).unwrap();
        let turns: Vec<_> = parsed
            .turns
            .iter()
            .map(|turn| {
                (
                    turn.ordinal,
                    turn.role,
                    turn.tool_name.as_deref(),
                    &*turn.text,
                )
            })
            .collect();
        assert_eq!(
            turns,
            [
                (0, Role::Tool, None, "early"),
                (
                    1,
                    Role::Tool,
                    Some("Read"),
                    r#"Read {"file_path":"/a","limit":5}"#
                ),
                (2, Role::Tool, Some("Bash"), r#"Bash {"command":"ls"}"#),
                (3, Role::Tool, Some("Ping"), "Ping"),
                (4, Role::Tool, Some("Bash"), "two"),
                (5, Role::Tool, Some("Read"), "a\nb"),
                (6, Role::Tool, None, "orphan"),
                (7, Role::Tool, None, "no id"),
                (8, Role::Tool, Some("Read"), ""),
                (9, Role::Tool, None, "its call had no name"),
            ]
        );
    }

    #[test]
    fn a_kept_record_without_turns_still_names_and_starts_the_session() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            dir.path(),
            "proj/quiet.jsonl",
            &[
                assistant(
                    "quiet",
                    "2026-11-01T10:00:00Z",
                    json!([{"type": "thinking", "thinking": "only thoughts"}]),
                ),
                json!({"type": "user", "sessionId": "quiet", "timestamp": "2026-11-01T10:00:01Z"}),
                user("quiet", "2026-11-01T10:00:02Z", json!(null)),
                user("quiet", "2026-11-01T10:00:03Z", json!({"not": "a list"})),
            ],
        );
        let parsed = ClaudeCode.parse(dir.path(), &path).unwrap();
        assert_eq!(parsed.id, "quiet");
        assert_eq!(parsed.started, datetime!(2026-11-01 10:00:00 UTC));
        assert!(parsed.turns.is_empty());
    }

    #[test]
    fn timestamps_are_read_as_utc() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            dir.path(),
            "proj/zone.jsonl",
            &[user("zone", "2026-11-01T19:30:00.250+09:30", json!("hi"))],
        );
        let parsed = ClaudeCode.parse(dir.path(), &path).unwrap();
        let ten = datetime!(2026-11-01 10:00:00.25 UTC);
        assert_eq!(parsed.started, ten);
        assert_eq!(parsed.started.offset(), UtcOffset::UTC);
        assert_eq!(parsed.turns[0].ts, ten);
        assert_eq!(parsed.turns[0].ts.offset(), UtcOffset::UTC);
    }
}
