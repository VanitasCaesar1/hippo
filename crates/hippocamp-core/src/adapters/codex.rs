//! Codex transcripts: every `rollout-*.jsonl` under a transcript root.
//!
//! Each line is `{"timestamp": ..., "type": ..., "payload": {...}}`; other
//! top-level keys and record types are ignored. The first `session_meta`
//! line with a `payload.id` names the session, `codex-<payload.id>`, and
//! gives its start: `payload.timestamp`, or the line's own `timestamp` when
//! the payload has none. A file without one is an error. `response_item`
//! payloads make turns, each stamped with its line's `timestamp`:
//!
//! - `message` with role `user` or `assistant`: one turn per `content[]`
//!   item that has a `text`, whatever the item's `type` (`input_text`,
//!   `output_text`); any other role, `developer` included, is skipped;
//! - `function_call`: one `tool` turn named by `name`, whose text is
//!   `<name> <arguments>` with `arguments` as written;
//! - `function_call_output`: one `tool` turn named by the `function_call`
//!   with the same `call_id` (or unnamed), whose text is `output`;
//! - `custom_tool_call` and `local_shell_call`, unconfirmed against a real
//!   rollout: mapped like `function_call` when the payload has a `name`
//!   (with `arguments`, or else `input`), skipped otherwise;
//!   `custom_tool_call_output`, likewise unconfirmed: mapped like
//!   `function_call_output` when it has an `output`, skipped otherwise;
//! - `reasoning` and every other payload type are never indexed.
//!
//! `event_msg` and every other record type are skipped.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;
use time::OffsetDateTime;

use super::{
    each_json_line, has_extension_jsonl, invalid_data, is_rollout, push_turn, timestamp,
    transcript_roots, Adapter, Parsed, RawTurn, Role, Tool,
};
use crate::config::Config;
use crate::error::Result;

/// The Codex adapter.
#[derive(Debug, Clone, Copy, Default)]
pub struct Codex;

impl Adapter for Codex {
    fn tool(&self) -> Tool {
        Tool::Codex
    }

    fn roots(&self, cfg: &Config) -> Vec<PathBuf> {
        transcript_roots(cfg)
    }

    fn file_matches(&self, path: &Path) -> bool {
        has_extension_jsonl(path) && is_rollout(path)
    }

    fn parse(&self, _root: &Path, path: &Path) -> Result<Parsed> {
        let mut meta = None;
        let mut call_names = HashMap::new();
        let mut turns = Vec::new();
        each_json_line(path, |line| {
            match line.get("type").and_then(Value::as_str) {
                // The first one that names a session wins.
                Some("session_meta") if meta.is_none() => meta = session_meta(&line),
                Some("response_item") => {
                    let Some(ts) = timestamp(line.get("timestamp")) else {
                        tracing::warn!(
                            "{}: skipping a response_item without a usable timestamp",
                            path.display()
                        );
                        return;
                    };
                    if let Some(item) = line.get("payload") {
                        add_item(&mut turns, &mut call_names, ts, item);
                    }
                }
                _ => {}
            }
        })?;

        let Some((id, started)) = meta else {
            return Err(invalid_data(
                path,
                "no session_meta record with a payload.id",
            ));
        };
        Ok(Parsed {
            id: format!("codex-{id}"),
            tool: Tool::Codex,
            started,
            turns,
        })
    }
}

/// The session id and start from a `session_meta` line. The start is
/// `payload.timestamp`, or the line's own `timestamp` when the payload has
/// none. `None` when there is no id or no usable start.
fn session_meta(line: &Value) -> Option<(String, OffsetDateTime)> {
    let payload = line.get("payload")?;
    let id = payload.get("id")?.as_str().filter(|id| !id.is_empty())?;
    let started =
        timestamp(payload.get("timestamp")).or_else(|| timestamp(line.get("timestamp")))?;
    Some((id.to_owned(), started))
}

/// Adds the turns for one `response_item` payload, if it makes any.
/// `call_names` maps each `function_call` id seen so far to its function's
/// name.
fn add_item(
    turns: &mut Vec<RawTurn>,
    call_names: &mut HashMap<String, String>,
    ts: OffsetDateTime,
    item: &Value,
) {
    match item.get("type").and_then(Value::as_str) {
        Some("message") => {
            let role = match item.get("role").and_then(Value::as_str) {
                Some("user") => Role::User,
                Some("assistant") => Role::Assistant,
                _ => return,
            };
            let parts = item
                .get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten();
            for text in parts.filter_map(|part| part.get("text").and_then(Value::as_str)) {
                push_turn(turns, role, ts, None, text.to_owned());
            }
        }
        Some("function_call" | "custom_tool_call" | "local_shell_call") => {
            let Some(name) = item.get("name").and_then(Value::as_str) else {
                return;
            };
            if let Some(id) = item.get("call_id").and_then(Value::as_str) {
                call_names.insert(id.to_owned(), name.to_owned());
            }
            let text = match item.get("arguments").or_else(|| item.get("input")) {
                Some(arguments) => format!("{name} {}", as_written(arguments)),
                None => name.to_owned(),
            };
            push_turn(turns, Role::Tool, ts, Some(name.to_owned()), text);
        }
        Some("custom_tool_call_output") => {
            let Some(output) = item.get("output") else {
                return;
            };
            let tool_name = item
                .get("call_id")
                .and_then(Value::as_str)
                .and_then(|id| call_names.get(id))
                .cloned();
            push_turn(turns, Role::Tool, ts, tool_name, as_written(output));
        }
        Some("function_call_output") => {
            let tool_name = item
                .get("call_id")
                .and_then(Value::as_str)
                .and_then(|id| call_names.get(id))
                .cloned();
            let text = item.get("output").map(as_written).unwrap_or_default();
            push_turn(turns, Role::Tool, ts, tool_name, text);
        }
        _ => {}
    }
}

/// A string as written; any other value as compact JSON.
fn as_written(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, io};

    use serde_json::json;
    use time::macros::datetime;

    use crate::error::Error;

    const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures");

    fn fixture() -> Parsed {
        let root = Path::new(FIXTURES);
        Codex
            .parse(root, &root.join("codex/rollout-synthetic.jsonl"))
            .unwrap()
    }

    /// Line `n` (from 1) of `fixtures/redaction/leaks.txt`.
    fn leak(n: usize) -> String {
        let leaks = fs::read_to_string(format!("{FIXTURES}/redaction/leaks.txt")).unwrap();
        leaks.lines().nth(n - 1).unwrap().to_owned()
    }

    /// Writes `lines` as a rollout at `<dir>/rollout-<name>.jsonl`.
    fn write(dir: &Path, name: &str, lines: &[Value]) -> PathBuf {
        let path = dir.join(format!("rollout-{name}.jsonl"));
        let text: String = lines.iter().map(|line| format!("{line}\n")).collect();
        fs::write(&path, text).unwrap();
        path
    }

    const AT: &str = "2026-11-01T10:00:00Z";

    fn meta(id: &str) -> Value {
        json!({"timestamp": AT, "type": "session_meta", "payload": {"id": id, "timestamp": AT}})
    }

    fn item(ts: &str, payload: Value) -> Value {
        json!({"timestamp": ts, "type": "response_item", "payload": payload})
    }

    fn is_invalid_data(err: &Error) -> bool {
        matches!(err, Error::Io(e) if e.kind() == io::ErrorKind::InvalidData)
    }

    #[test]
    fn the_fixture_parses_into_its_turns() {
        let parsed = fixture();
        assert_eq!(parsed.id, "codex-01900000-c0de-7000-8000-000000000001");
        assert_eq!(parsed.tool, Tool::Codex);
        // payload.timestamp, not the session_meta line's own timestamp.
        assert_eq!(parsed.started, datetime!(2026-11-01 11:00:00 UTC));

        // Role, tool name, timestamp (the line's), the start of the text,
        // and the leaks.txt lines the text must hold.
        let expected = [
            (
                Role::User,
                None,
                datetime!(2026-11-01 11:00:03 UTC),
                "Fix the retry backoff in the example webhook sender. It retries too fast.\n\
                 I pasted some config by mistake:\n",
                vec![1, 2, 3, 13, 14, 15],
            ),
            (
                Role::Tool,
                Some("shell"),
                datetime!(2026-11-01 11:00:08 UTC),
                r#"shell {"command":["bash","-lc","grep -rn '"#,
                vec![4],
            ),
            (
                Role::Tool,
                Some("shell"),
                datetime!(2026-11-01 11:00:09 UTC),
                "Exit code: 0\nWall time: 0.1 seconds\nOutput:\nsrc/sender.py:3:RETRY_DELAY = 0\n",
                vec![5, 6, 7, 8, 9, 10, 16],
            ),
            (
                Role::Assistant,
                None,
                datetime!(2026-11-01 11:00:12 UTC),
                "The retry loop never sleeps, so I will patch it to back off. The check output also showed:\n",
                vec![11, 12],
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
            assert!(turn.text.starts_with(text), "turn {i}: {}", turn.text);
            for n in leaks {
                assert!(turn.text.contains(&leak(n)), "turn {i} lacks leak {n}");
            }
        }

        // Two messages (one per role), one call and one output.
        let count = |role| parsed.turns.iter().filter(|turn| turn.role == role).count();
        assert_eq!(count(Role::User), 1);
        assert_eq!(count(Role::Assistant), 1);
        assert_eq!(count(Role::Tool), 2);

        // The call's text is its name and its arguments, as written.
        assert_eq!(
            parsed.turns[1].text,
            format!(
                r#"shell {{"command":["bash","-lc","grep -rn '{}' /work/example"],"workdir":"/work/example"}}"#,
                leak(4)
            )
        );

        // Nothing from the developer message, the reasoning item or the
        // event_msg reaches a turn.
        for turn in &parsed.turns {
            for never in [
                "Synthetic developer instructions",
                "Reasoning that is never indexed",
                "c3ludGhldGlj",
            ] {
                assert!(!turn.text.contains(never), "{never}");
            }
        }
    }

    #[test]
    fn other_records_other_keys_and_other_roles_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            dir.path(),
            "ignored",
            &[
                json!({"timestamp": AT, "ordinal": 0, "type": "session_meta",
                       "payload": {"id": "abc", "timestamp": AT}, "metadata": {"k": "v"}}),
                json!({"timestamp": AT, "type": "world_state", "payload": {"cwd": "/x"}}),
                json!({"timestamp": AT, "type": "turn_context", "payload": {}}),
                json!({"timestamp": AT, "type": "event_msg",
                       "payload": {"type": "agent_message", "message": "never"}}),
                item(
                    AT,
                    json!({"type": "message", "role": "system",
                                "content": [{"type": "input_text", "text": "never"}]}),
                ),
                item(
                    AT,
                    json!({"type": "message", "role": "developer",
                                "content": [{"type": "input_text", "text": "never"}]}),
                ),
                item(
                    AT,
                    json!({"type": "reasoning",
                                "summary": [{"type": "summary_text", "text": "never"}]}),
                ),
                item(
                    AT,
                    // Without a name, a custom tool call is skipped.
                    json!({"type": "custom_tool_call", "input": "never"}),
                ),
                item(
                    AT,
                    json!({"type": "message", "role": "user", "content": [
                        {"type": "input_image", "image_url": "never"},
                        {"type": "input_text", "text": "a"},
                        {"type": "output_text", "text": "b"},
                        {"type": "something_else", "text": "c"},
                    ]}),
                ),
                item(
                    AT,
                    json!({"type": "message", "role": "user", "content": "not a list of parts"}),
                ),
                item(
                    AT,
                    json!({"type": "message", "role": "assistant", "content": [
                        {"type": "output_text", "text": "d"},
                    ]}),
                ),
                json!({"timestamp": AT, "type": "response_item"}),
            ],
        );
        let parsed = Codex.parse(dir.path(), &path).unwrap();
        assert_eq!(parsed.id, "codex-abc");
        let turns: Vec<_> = parsed
            .turns
            .iter()
            .map(|turn| (turn.ordinal, turn.role, &*turn.text))
            .collect();
        assert_eq!(
            turns,
            [
                (0, Role::User, "a"),
                (1, Role::User, "b"),
                (2, Role::User, "c"),
                (3, Role::Assistant, "d"),
            ]
        );
    }

    #[test]
    fn a_rollout_without_a_usable_session_meta_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let message = item(
            AT,
            json!({"type": "message", "role": "user", "content": [{"type": "input_text", "text": "hi"}]}),
        );
        let cases = [
            ("empty", vec![]),
            ("no session_meta", vec![message.clone()]),
            (
                "no id",
                vec![
                    json!({"timestamp": AT, "type": "session_meta", "payload": {"timestamp": AT}}),
                    message.clone(),
                ],
            ),
            (
                "empty id",
                vec![
                    json!({"timestamp": AT, "type": "session_meta", "payload": {"id": "", "timestamp": AT}}),
                ],
            ),
            (
                "id is not a string",
                vec![
                    json!({"timestamp": AT, "type": "session_meta", "payload": {"id": 7, "timestamp": AT}}),
                ],
            ),
            (
                "no payload",
                vec![json!({"timestamp": AT, "type": "session_meta"})],
            ),
            (
                "no start",
                vec![json!({"type": "session_meta", "payload": {"id": "abc"}})],
            ),
        ];
        for (name, lines) in cases {
            let path = write(dir.path(), &name.replace(' ', "-"), &lines);
            let err = Codex.parse(dir.path(), &path).unwrap_err();
            assert!(is_invalid_data(&err), "{name}: {err}");
            assert!(err.to_string().contains("session_meta"), "{name}: {err}");
            assert!(
                err.to_string().contains(path.to_str().unwrap()),
                "{name}: {err}"
            );
        }
    }

    #[test]
    fn the_first_usable_session_meta_names_the_session() {
        let dir = tempfile::tempdir().unwrap();
        let no_id = json!({"timestamp": AT, "type": "session_meta", "payload": {"timestamp": AT}});
        let path = write(dir.path(), "metas", &[no_id, meta("first"), meta("second")]);
        assert_eq!(Codex.parse(dir.path(), &path).unwrap().id, "codex-first");
    }

    #[test]
    fn a_session_meta_without_a_start_of_its_own_uses_its_line_timestamp() {
        let dir = tempfile::tempdir().unwrap();
        for (name, payload) in [
            ("absent", json!({"id": "abc"})),
            (
                "not rfc 3339",
                json!({"id": "abc", "timestamp": "yesterday"}),
            ),
            (
                "not a string",
                json!({"id": "abc", "timestamp": 1762000000}),
            ),
        ] {
            let line = json!({"timestamp": "2026-11-01T12:00:00+02:00", "type": "session_meta", "payload": payload});
            let path = write(dir.path(), name, &[line]);
            let parsed = Codex.parse(dir.path(), &path).unwrap();
            assert_eq!(parsed.started, datetime!(2026-11-01 10:00:00 UTC), "{name}");
            assert_eq!(parsed.started.offset(), time::UtcOffset::UTC, "{name}");
        }
    }

    #[test]
    fn custom_and_shell_calls_map_like_function_calls_when_shaped_like_them() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            dir.path(),
            "custom",
            &[
                meta("abc"),
                item(
                    AT,
                    json!({"type": "custom_tool_call", "name": "apply_patch", "call_id": "k1",
                           "input": "*** Begin Patch"}),
                ),
                item(
                    AT,
                    json!({"type": "custom_tool_call_output", "call_id": "k1", "output": "Done"}),
                ),
                // No output, or no name: skipped.
                item(
                    AT,
                    json!({"type": "custom_tool_call_output", "call_id": "k1"}),
                ),
                item(
                    AT,
                    json!({"type": "local_shell_call", "call_id": "k2",
                           "action": {"type": "exec", "command": ["ls"]}}),
                ),
                item(
                    AT,
                    json!({"type": "local_shell_call", "name": "shell", "call_id": "k3",
                           "arguments": "{\"command\":[\"pwd\"]}"}),
                ),
            ],
        );
        let parsed = Codex.parse(dir.path(), &path).unwrap();
        let turns: Vec<_> = parsed
            .turns
            .iter()
            .map(|turn| (turn.role, turn.tool_name.as_deref(), &*turn.text))
            .collect();
        assert_eq!(
            turns,
            [
                (
                    Role::Tool,
                    Some("apply_patch"),
                    "apply_patch *** Begin Patch"
                ),
                (Role::Tool, Some("apply_patch"), "Done"),
                (Role::Tool, Some("shell"), r#"shell {"command":["pwd"]}"#),
            ]
        );
    }

    #[test]
    fn arguments_and_output_are_written_as_given_or_as_compact_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            dir.path(),
            "calls",
            &[
                meta("abc"),
                item(
                    AT,
                    json!({"type": "function_call", "name": "shell", "call_id": "c1",
                                "arguments": "{ \"command\": [\"ls\"] }"}),
                ),
                item(
                    AT,
                    json!({"type": "function_call", "name": "edit", "call_id": "c2",
                                "arguments": {"path": "/a", "line": 3, "flags": [true, null]}}),
                ),
                item(
                    AT,
                    json!({"type": "function_call", "name": "ping", "call_id": "c3"}),
                ),
                item(
                    AT,
                    json!({"type": "function_call", "call_id": "c4", "arguments": "{}"}),
                ),
                item(
                    AT,
                    json!({"type": "function_call_output", "call_id": "c2",
                                "output": {"exit": 0, "text": "ok"}}),
                ),
                item(
                    AT,
                    json!({"type": "function_call_output", "call_id": "c1", "output": "line one\nline two"}),
                ),
                item(AT, json!({"type": "function_call_output", "call_id": "c1"})),
                item(
                    AT,
                    json!({"type": "function_call_output", "call_id": "nobody", "output": "orphan"}),
                ),
                item(
                    AT,
                    json!({"type": "function_call_output", "output": "no id"}),
                ),
                item(
                    AT,
                    json!({"type": "function_call_output", "call_id": "c4", "output": "its call had no name"}),
                ),
            ],
        );
        let parsed = Codex.parse(dir.path(), &path).unwrap();
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
                // As written: the string's own spacing is kept.
                (
                    0,
                    Role::Tool,
                    Some("shell"),
                    r#"shell { "command": ["ls"] }"#
                ),
                (
                    1,
                    Role::Tool,
                    Some("edit"),
                    r#"edit {"flags":[true,null],"line":3,"path":"/a"}"#
                ),
                (2, Role::Tool, Some("ping"), "ping"),
                (3, Role::Tool, Some("edit"), r#"{"exit":0,"text":"ok"}"#),
                (4, Role::Tool, Some("shell"), "line one\nline two"),
                (5, Role::Tool, Some("shell"), ""),
                (6, Role::Tool, None, "orphan"),
                (7, Role::Tool, None, "no id"),
                (8, Role::Tool, None, "its call had no name"),
            ]
        );
    }

    #[test]
    fn bad_lines_and_lines_without_a_timestamp_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rollout-messy.jsonl");
        let hello = |ts: Option<&str>, text: &str| {
            let payload = json!({"type": "message", "role": "user", "content": [{"type": "input_text", "text": text}]});
            match ts {
                Some(ts) => item(ts, payload),
                None => json!({"type": "response_item", "payload": payload}),
            }
        };
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"{\"synthetic\": true}\r\n\r\n");
        bytes.extend_from_slice(format!("{}\r\n", meta("messy")).as_bytes());
        bytes.extend_from_slice(b"this is not json\n\xff\xfe{\"type\":\"response_item\"}\n");
        for line in [
            hello(Some("2026-11-01T10:00:01Z"), "kept"),
            hello(None, "no timestamp"),
            hello(Some("yesterday"), "bad timestamp"),
            hello(Some("2026-11-01T10:00:02Z"), "kept too"),
        ] {
            bytes.extend_from_slice(format!("{line}\n").as_bytes());
        }
        bytes.extend_from_slice(b"{\"timestamp\":\"2026-11-01T10:00:03Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"mess");
        fs::write(&path, bytes).unwrap();

        let parsed = Codex.parse(dir.path(), &path).unwrap();
        assert_eq!(parsed.id, "codex-messy");
        let turns: Vec<_> = parsed
            .turns
            .iter()
            .map(|turn| (turn.ordinal, turn.ts, &*turn.text))
            .collect();
        assert_eq!(
            turns,
            [
                (0, datetime!(2026-11-01 10:00:01 UTC), "kept"),
                (1, datetime!(2026-11-01 10:00:02 UTC), "kept too"),
            ]
        );
    }

    #[test]
    fn a_missing_file_is_an_io_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = Codex
            .parse(dir.path(), &dir.path().join("rollout-gone.jsonl"))
            .unwrap_err();
        assert!(
            matches!(&err, Error::Io(e) if e.kind() == io::ErrorKind::NotFound),
            "{err}"
        );
    }
}
