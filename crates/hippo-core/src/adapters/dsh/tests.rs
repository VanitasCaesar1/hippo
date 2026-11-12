use super::*;
use std::fs;

use serde_json::json;
use time::macros::datetime;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/dsh");
const SHOP: &str = "--home-synthetic-shop--";
const S1: &str = "11111111-1111-4111-8111-111111111111";
const S2: &str = "22222222-2222-4222-8222-222222222222";
const S3: &str = "33333333-3333-4333-8333-333333333333";
const S4: &str = "44444444-4444-4444-8444-444444444444";
const S5: &str = "55555555-5555-4555-8555-555555555555";
const S6: &str = "66666666-6666-4666-8666-666666666666";

fn root() -> PathBuf {
    PathBuf::from(FIXTURES)
}

fn log(session: &str, name: &str) -> PathBuf {
    root().join(SHOP).join(session).join(name)
}

fn parse(path: &Path) -> Parsed {
    Dsh.parse(&root(), path).unwrap()
}

/// `(role, tool name, text)` of each turn.
fn shape(p: &Parsed) -> Vec<(Role, Option<&str>, &str)> {
    p.turns
        .iter()
        .map(|t| (t.role, t.tool_name.as_deref(), t.text.as_str()))
        .collect()
}

/// Writes `lines` as a log at `<dir>/--p--/<id>/<name>`, after the
/// synthetic marker.
fn write_log(dir: &Path, id: &str, name: &str, lines: &[Value]) -> PathBuf {
    let path = dir.join("--p--").join(id).join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut text = String::from("{\"synthetic\": true}\n");
    for line in lines {
        text.push_str(&line.to_string());
        text.push('\n');
    }
    fs::write(&path, text).unwrap();
    path
}

fn head(id: &str, version: u64) -> Value {
    json!({"type": "session", "version": version, "id": id, "createdAt": 1_793_527_200_000_i64,
           "cwd": "/p", "isSeeded": false, "delegationDepth": 0})
}

#[test]
fn the_normal_session_parses_into_its_turns() {
    let p = parse(&log(S1, "session.v4.jsonl"));
    assert_eq!(p.id, format!("dsh-{S1}"));
    assert_eq!(p.tool, Tool::Dsh);
    assert_eq!(p.started, datetime!(2026-11-01 10:00:00 UTC));
    let turns = shape(&p);
    // The person's words only: dsh's runtime context and sub-agent notices
    // are not turns, nor are reasoning, the system prompt or tool-call
    // blocks inside a reply.
    let users: Vec<_> = turns.iter().filter(|t| t.0 == Role::User).collect();
    assert_eq!(users.len(), 1);
    assert!(users[0].2.starts_with("Fix the failing checkout test"));
    assert!(!turns.iter().any(|t| t.2.contains("never indexed")));
    assert!(!turns.iter().any(|t| t.2.contains("system-reminder")));
    assert!(!turns.iter().any(|t| t.2.contains("Background subagent")));
    assert_eq!(
        turns[1],
        (Role::Assistant, None, "I'll read the test first.")
    );
    assert_eq!(
        turns[2],
        (
            Role::Tool,
            Some("Read"),
            r#"Read {"file_path": "tests/checkout.test.ts"}"#
        )
    );
    assert_eq!(turns[3].0, Role::Tool);
    assert_eq!(turns[3].1, Some("Read"));
    // Calls under the names hippo's readers know, results named by their
    // call.
    let names: Vec<_> = turns.iter().filter_map(|t| t.1).collect();
    for name in ["Bash", "Edit", "apply_patch", "Write", "Agent"] {
        assert!(names.contains(&name), "{name}: {names:?}");
    }
    assert!(!names
        .iter()
        .any(|n| ["bash", "edit", "write", "subagent", "subagent_fork"].contains(n)));
    // Every turn is stamped with its event's time.
    assert!(p.turns.windows(2).all(|w| w[0].ts <= w[1].ts));
    assert!(p.turns.iter().all(|t| t.ts > p.started));
    let ordinals: Vec<u32> = p.turns.iter().map(|t| t.ordinal).collect();
    assert_eq!(ordinals, (0..p.turns.len() as u32).collect::<Vec<_>>());
}

#[test]
fn failed_results_are_written_as_hippos_readers_know_them() {
    let p = parse(&log(S1, "session.v4.jsonl"));
    let texts: Vec<&str> = p.turns.iter().map(|t| t.text.as_str()).collect();
    assert!(
        texts.contains(&"Exit code 1\nFAIL tests/checkout.test.ts\n  expected 3, received 2"),
        "{texts:#?}"
    );
    assert!(texts.contains(
        &"<tool_use_error>Error: Cannot write /etc/hosts: the path is outside the workspace.</tool_use_error>"
    ));
    // A passing run is left as written.
    assert!(texts.contains(&"PASS tests/checkout.test.ts\nTests: 12 passed\n"));

    assert_eq!(result_text(Some("Bash"), "ok\n".into(), false), "ok\n");
    assert_eq!(
        result_text(Some("Bash"), "[exit code: 127]".into(), false),
        "Exit code 127\n"
    );
    assert_eq!(
        result_text(
            Some("Bash"),
            "a\n[stderr]\nb\n[exit code: 2]\n".into(),
            false
        ),
        "Exit code 2\na\n[stderr]\nb"
    );
    // Only a shell result's last line counts.
    assert_eq!(
        result_text(Some("Read"), "x\n[exit code: 1]".into(), false),
        "x\n[exit code: 1]"
    );
    assert_eq!(
        result_text(Some("Bash"), "[exit code: 1] then more".into(), false),
        "[exit code: 1] then more"
    );
}

#[test]
fn only_the_newest_generation_is_the_session() {
    let v3 = log(S1, "session.v3.jsonl");
    let v4 = log(S1, "session.v4.jsonl");
    assert!(Dsh.file_matches(&v4));
    assert!(!Dsh.file_matches(&v3));
    let claimed = logs_under(&root());
    assert!(claimed.contains(&v4));
    assert!(!claimed.contains(&v3));
    // Each session folder gives one log: six sessions, the unread v9 one
    // included.
    assert_eq!(claimed.len(), 6, "{claimed:#?}");
}

#[test]
fn file_names_are_read_as_dsh_names_them() {
    for (name, version) in [
        ("session.jsonl", Some(0)),
        ("session.jsonl.zstd", Some(0)),
        ("session.v1.jsonl", Some(1)),
        ("session.v4.jsonl.zstd", Some(4)),
        ("session.v12.jsonl", Some(12)),
        ("session.v0.jsonl", None),
        ("session.v04.jsonl", None),
        ("session.V4.jsonl", None),
        ("session.v4.jsonl.tmp", None),
        ("session.lock", None),
        ("other.jsonl", None),
    ] {
        assert_eq!(name_version(Path::new(name)), version, "{name}");
    }
    for (path, is) in [
        ("r/--home-a--/id/session.v4.jsonl", true),
        ("r/_no-cwd/id/session.v4.jsonl.zstd", true),
        ("r/home-a/id/session.v4.jsonl", false),
        ("r/----/id/session.v4.jsonl", false),
        ("r/--home-a--/session.v4.jsonl", false),
        ("r/--home-a--/id/abc.jsonl", false),
    ] {
        assert_eq!(is_log(Path::new(path)), is, "{path}");
    }
    // Claude Code leaves dsh logs alone, and still takes its own files.
    let claude = super::super::claude_code::ClaudeCode;
    assert!(!claude.file_matches(Path::new("r/--home-a--/id/session.v4.jsonl")));
    assert!(claude.file_matches(Path::new("p/-home-a/abc.jsonl")));
}

#[test]
fn a_sub_agent_is_named_under_the_top_of_its_chain() {
    let child = parse(&log(S2, "session.v4.jsonl"));
    assert_eq!(child.id, format!("dsh-{S1}-agent-{S2}"));
    assert_eq!(
        shape(&child),
        [
            (Role::User, None, "Check that the cart totals add up."),
            (
                Role::Tool,
                Some("Read"),
                r#"Read {"file_path": "src/cart.ts"}"#
            ),
            (
                Role::Tool,
                Some("Read"),
                "<content>\n1: export const tax = 0.18\n</content>"
            ),
            (Role::Assistant, None, "The totals add up."),
        ]
    );
    // A sub-agent of a sub-agent is still under the top session.
    let grandchild = parse(&log(S6, "session.v4.jsonl"));
    assert_eq!(grandchild.id, format!("dsh-{S1}-agent-{S6}"));
}

#[test]
fn a_parent_not_under_the_root_names_the_sub_agent_itself() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = head("kid", 4);
    h["parentSession"] = json!("gone");
    h["origin"] = json!("subagent");
    let path = write_log(dir.path(), "kid", "session.v4.jsonl", &[h]);
    assert_eq!(
        Dsh.parse(dir.path(), &path).unwrap().id,
        "dsh-gone-agent-kid"
    );
    // A fork the person made has no origin: it is a session of its own.
    let mut h = head("own", 4);
    h["parentSession"] = json!("gone");
    let path = write_log(dir.path(), "own", "session.v4.jsonl", &[h]);
    assert_eq!(Dsh.parse(dir.path(), &path).unwrap().id, "dsh-own");
}

#[test]
fn a_compressed_fork_reads_its_own_turns_only() {
    let path = log(S3, "session.v4.jsonl.zstd");
    assert!(Dsh.file_matches(&path));
    let fork = parse(&path);
    assert_eq!(fork.id, format!("dsh-{S1}-agent-{S3}"));
    assert_eq!(
        shape(&fork),
        [
            (Role::User, None, "Which test failed?"),
            (Role::Assistant, None, "tests/checkout.test.ts failed."),
        ]
    );
    let h = header(&path).unwrap();
    assert!(h.seeded && h.sub_agent);
    assert_eq!(h.parent.as_deref(), Some(S1));
}

#[test]
fn a_torn_last_frame_is_dropped_and_the_rest_is_read() {
    let bytes = fs::read(log(S3, "session.v4.jsonl.zstd")).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir
        .path()
        .join("--p--")
        .join(S3)
        .join("session.v4.jsonl.zstd");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    // Cut into the last frame.
    fs::write(&path, &bytes[..bytes.len() - 20]).unwrap();
    let log = read(&path).unwrap();
    assert_eq!(log.header.id, S3);
    let whole = read(&self::log(S3, "session.v4.jsonl.zstd")).unwrap();
    assert!(!log.events.is_empty());
    assert!(log.events.len() < whole.events.len());
    assert_eq!(log.events[..], whole.events[..log.events.len()]);
    // Garbage after the frames ends the stream too.
    let mut extra = bytes.clone();
    extra.extend_from_slice(b"not a frame at all");
    fs::write(&path, &extra).unwrap();
    assert_eq!(read(&path).unwrap().events.len(), whole.events.len());
}

#[test]
fn a_session_stopped_mid_call_ends_on_its_call() {
    let p = parse(&log(S4, "session.v4.jsonl"));
    assert_eq!(
        shape(&p),
        [
            (Role::User, None, "Run the end-to-end suite."),
            (
                Role::Tool,
                Some("Bash"),
                r#"Bash {"command": "npm run test:e2e", "description": "Run the end-to-end suite"}"#
            ),
        ]
    );
}

#[test]
fn an_unknown_format_version_is_refused_with_its_version() {
    let path = root()
        .join("--home-synthetic-later--")
        .join(S5)
        .join("session.v9.jsonl");
    assert!(Dsh.file_matches(&path));
    let err = Dsh.parse(&root(), &path).unwrap_err().to_string();
    assert!(
        err.contains("dsh log format v9 is not one hippo reads"),
        "{err}"
    );
    assert!(
        !err.contains("FUTURE"),
        "the message quotes no content: {err}"
    );
    assert_eq!(header(&path).unwrap().version, 9);
    for version in [0, 1, 2, 5] {
        let dir = tempfile::tempdir().unwrap();
        let path = write_log(dir.path(), "s", "session.v4.jsonl", &[head("s", version)]);
        assert!(Dsh.parse(dir.path(), &path).is_err(), "v{version}");
    }
}

#[test]
fn format_3_results_name_their_call_in_their_source() {
    let dir = tempfile::tempdir().unwrap();
    let t = 1_793_527_201_000_i64;
    let path = write_log(
        dir.path(),
        "old",
        "session.v3.jsonl",
        &[
            head("old", 3),
            json!({"type": "tool/call", "seq": 0, "time": t,
                   "data": {"turn": 1, "step": 1, "callId": "c1", "name": "bash", "arguments": "{\"command\":\"ls\"}"}}),
            json!({"type": "tool/result", "seq": 1, "time": t + 1,
                   "data": {"turn": 1, "step": 1, "message": {"role": "tool", "source": {"kind": "tool", "callId": "c1"},
                            "content": [{"type": "text", "text": "a.txt\n"}], "id": "r"}}}),
        ],
    );
    let p = Dsh.parse(dir.path(), &path).unwrap();
    assert_eq!(
        shape(&p),
        [
            (Role::Tool, Some("Bash"), r#"Bash {"command":"ls"}"#),
            (Role::Tool, Some("Bash"), "a.txt\n"),
        ]
    );
}

#[test]
fn bad_events_are_skipped_and_a_log_without_a_header_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let t = 1_793_527_201_000_i64;
    let path = write_log(
        dir.path(),
        "s",
        "session.v4.jsonl",
        &[
            head("s", 4),
            // No time.
            json!({"type": "user/message", "seq": 0, "data": {"content": [{"type": "text", "text": "lost"}], "source": {"kind": "user"}}}),
            // An event type hippo does not know.
            json!({"type": "future/thing", "seq": 1, "time": t, "data": {"text": "skipped"}}),
            // An image and an empty reply are not turns.
            json!({"type": "user/message", "seq": 2, "time": t, "data": {"content": [{"type": "image"}, {"type": "text", "text": "kept"}], "source": {"kind": "user"}}}),
            json!({"type": "assistant/message", "seq": 3, "time": t, "data": {"message": {"content": [{"type": "text", "text": "  "}]}}}),
            // A result whose call is not in the log has no name.
            json!({"type": "tool/result", "seq": 4, "time": t, "data": {"message": {"toolCallId": "x", "content": [{"type": "text", "text": "orphan"}]}}}),
        ],
    );
    fs::write(
        &path,
        fs::read_to_string(&path).unwrap() + "{\"type\":\"user/message\",\"par",
    )
    .unwrap();
    let p = Dsh.parse(dir.path(), &path).unwrap();
    assert_eq!(
        shape(&p),
        [(Role::User, None, "kept"), (Role::Tool, None, "orphan")]
    );

    let none = write_log(
        dir.path(),
        "n",
        "session.v4.jsonl",
        &[json!({"type": "turn/start"})],
    );
    let err = Dsh.parse(dir.path(), &none).unwrap_err().to_string();
    assert!(err.contains("no dsh session header"), "{err}");
    let gone = Dsh
        .parse(dir.path(), &dir.path().join("gone.jsonl"))
        .unwrap_err();
    assert!(
        matches!(&gone, Error::Io(e) if e.kind() == io::ErrorKind::NotFound),
        "{gone}"
    );
}

#[test]
fn ids_with_other_characters_are_hashed() {
    assert_eq!(id_part("abc-DEF_1"), "abc-DEF_1");
    let odd = id_part("a/b");
    assert!(odd.starts_with('h') && odd.len() == 33, "{odd}");
    assert_eq!(odd, id_part("a/b"));
    assert_ne!(odd, id_part("a.b"));
    assert!(id_part(&"x".repeat(60)).starts_with('h'));
    let h = Header {
        version: 4,
        id: "a.b".into(),
        created: datetime!(2026-11-01 10:00:00 UTC),
        cwd: None,
        parent: None,
        sub_agent: false,
        seeded: false,
    };
    let id = session_id(Path::new("r/_no-cwd/a.b/session.v4.jsonl"), &h);
    assert!(id.len() <= 128 && crate::sessions::is_valid_id(&id), "{id}");
}

#[test]
fn segments_are_escaped_as_dsh_escapes_them() {
    assert_eq!(encode_segment(S1), S1);
    assert_eq!(encode_segment("a.b_c-d"), "a.b_c-d");
    assert_eq!(encode_segment("a/b~c"), "a~002Fb~007Ec");
    assert_eq!(encode_segment("é"), "~00E9");
    assert_eq!(encode_segment("."), "~002E");
    assert_eq!(encode_segment(".."), "~002E~002E");
    assert_eq!(find(&root(), S2), Some(log(S2, "session.v4.jsonl")));
    assert_eq!(find(&root(), S1), Some(log(S1, "session.v4.jsonl")));
    assert_eq!(find(&root(), "nobody"), None);
}

#[test]
fn the_working_directory_comes_from_the_header() {
    assert_eq!(
        Dsh.cwd("x", &log(S1, "session.v4.jsonl")),
        Some(PathBuf::from("/home/synthetic/shop"))
    );
}
