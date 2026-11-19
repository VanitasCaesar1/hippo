use super::*;
use std::fs;

use time::macros::datetime;

const PROJECT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/cursor/projects/work-example/agent-transcripts"
);
const MAIN: &str = "aa000000-0000-4000-8000-000000000001";
const SUB: &str = "bb000000-0000-4000-8000-000000000002";
const FLAT: &str = "cc000000-0000-4000-8000-000000000003";

fn main_file() -> PathBuf {
    Path::new(PROJECT).join(MAIN).join(format!("{MAIN}.jsonl"))
}

/// `(role, tool name, text)` of each turn.
fn shape(p: &Parsed) -> Vec<(Role, Option<&str>, &str)> {
    p.turns
        .iter()
        .map(|t| (t.role, t.tool_name.as_deref(), t.text.as_str()))
        .collect()
}

#[test]
fn the_fixture_parses_into_its_turns() {
    let p = Cursor.parse(Path::new(PROJECT), &main_file()).unwrap();
    assert_eq!(p.id, format!("cursor-{MAIN}"));
    assert_eq!(p.tool, Tool::Cursor);
    // 9:00 AM at UTC-7.
    assert_eq!(p.started, datetime!(2026-11-01 16:00:00 UTC));
    let turns = shape(&p);
    assert_eq!(turns.len(), 8, "{turns:#?}");
    assert_eq!(turns[0].0, Role::User);
    assert!(turns[0]
        .2
        .starts_with("Fix the retry backoff in the example webhook sender."));
    assert!(!turns[0].2.contains("<timestamp>"));
    assert_eq!(
        turns[1],
        (Role::Assistant, None, "I'll read the sender first.")
    );
    assert_eq!(
        turns[2],
        (
            Role::Tool,
            Some("Read"),
            r#"Read {"path":"/work/example/src/sender.ts"}"#
        )
    );
    assert_eq!(
        turns[3],
        (
            Role::Tool,
            Some("Shell"),
            r#"Shell {"command":"export DB_PASSWORD=\"FAKEfake0000FAKEfake\""}"#
        )
    );
    assert_eq!(turns[4].0, Role::Assistant);
    // Only the prompt inside <user_query> of the second message.
    assert_eq!(turns[5], (Role::User, None, "Now add a test for the cap."));
    assert_eq!(turns[6].1, Some("Task"));
    assert_eq!(turns[7].2, "Added a test for the 30 second cap.");
    // Each turn has the time of the prompt it follows.
    let times: Vec<_> = p.turns.iter().map(|t| t.ts).collect();
    assert!(times[..5]
        .iter()
        .all(|t| *t == datetime!(2026-11-01 16:00:00 UTC)));
    assert!(times[5..]
        .iter()
        .all(|t| *t == datetime!(2026-11-01 16:20:00 UTC)));
}

#[test]
fn a_sub_agent_and_the_flat_layout_are_sessions() {
    let sub = Path::new(PROJECT)
        .join(MAIN)
        .join("subagents")
        .join(format!("{SUB}.jsonl"));
    let p = Cursor.parse(Path::new(PROJECT), &sub).unwrap();
    assert_eq!(p.id, format!("cursor-{MAIN}-agent-{SUB}"));
    assert_eq!(
        shape(&p),
        [
            (
                Role::User,
                None,
                "Find where the sender sleeps between retries."
            ),
            (
                Role::Tool,
                Some("Grep"),
                r#"Grep {"path":"/work/example/src","pattern":"sleep"}"#
            ),
            (Role::Assistant, None, "It sleeps in retry.ts, line 12."),
        ]
    );
    // No prompt has a time: the file's modification time.
    let modified = OffsetDateTime::from(fs::metadata(&sub).unwrap().modified().unwrap());
    assert_eq!(p.started, modified);

    let flat = Path::new(PROJECT).join(format!("{FLAT}.jsonl"));
    let p = Cursor.parse(Path::new(PROJECT), &flat).unwrap();
    assert_eq!(p.id, format!("cursor-{FLAT}"));
    assert_eq!(p.turns[0].text, "Which port does the example server use?");
    assert_eq!(p.turns[1].role, Role::Assistant);
}

#[test]
fn files_are_claimed_by_their_place() {
    for (path, id) in [
        ("x/agent-transcripts/a1/a1.jsonl", Some("cursor-a1")),
        ("x/agent-transcripts/a1.jsonl", Some("cursor-a1")),
        (
            "x/agent-transcripts/a1/subagents/b2.jsonl",
            Some("cursor-a1-agent-b2"),
        ),
        ("x/agent-transcripts/a1/other.jsonl", None),
        ("x/agent-transcripts/a1/a1.txt", None),
        ("x/agent-transcripts/a1/a1.meta.jsonl", None),
        ("x/agent-tools/a1.txt", None),
        ("x/projects/p/s.jsonl", None),
    ] {
        assert_eq!(session_id(Path::new(path)).as_deref(), id, "{path}");
    }
    // The Claude Code adapter leaves every file under agent-transcripts.
    let claude = super::super::claude_code::ClaudeCode;
    assert!(!claude.file_matches(&main_file()));
    assert!(claude.file_matches(Path::new("x/projects/p/s.jsonl")));
}

#[test]
fn prompt_times_read_cursors_format() {
    let at = |t: &str| prompt_time(&format!("<timestamp>{t}</timestamp> hi"));
    assert_eq!(
        at("Friday, Nov 6, 2026, 9:58 PM (UTC-7:00)"),
        Some(datetime!(2026-11-07 04:58:00 UTC))
    );
    assert_eq!(
        at("Monday, Nov 9, 2026, 12:05 AM (UTC+5:30)"),
        Some(datetime!(2026-11-08 18:35:00 UTC))
    );
    assert_eq!(
        at("Monday, November 9, 2026, 12:05 PM (UTC)"),
        Some(datetime!(2026-11-09 12:05:00 UTC))
    );
    assert_eq!(
        at("Monday, Nov 9, 2026, 1:05 PM"),
        Some(datetime!(2026-11-09 13:05:00 UTC))
    );
    for bad in [
        "yesterday",
        "Monday, Nov 9, 2026",
        "Monday, Foo 9, 2026, 1:05 PM",
    ] {
        assert_eq!(at(bad), None, "{bad}");
    }
    assert_eq!(prompt_time("no tag"), None);
}

#[test]
fn a_persons_message_keeps_only_what_they_typed() {
    assert_eq!(
        person_text(
            "<timestamp>x</timestamp>\n<user_query>\n  do it \n</user_query>\n<extra>y</extra>"
        ),
        "do it"
    );
    assert_eq!(person_text("<timestamp>x</timestamp>\nplain"), "plain");
    assert_eq!(person_text("plain"), "plain");
}

#[test]
fn a_slug_is_read_back_against_the_file_system() {
    assert_eq!(slug("/Users/a.b/My Code/x_y"), "Users-a-b-My-Code-x-y");
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path();
    for d in ["work/my-app/web", "work/my/app", "work/other"] {
        fs::create_dir_all(base.join(d)).unwrap();
    }
    // `my-app` is tried before `my`.
    assert_eq!(
        resolve_slug(base, "work-my-app-web"),
        Some(base.join("work/my-app/web"))
    );
    assert_eq!(
        resolve_slug(base, "work-my-app"),
        Some(base.join("work/my-app"))
    );
    assert_eq!(resolve_slug(base, "work-gone"), None);
    assert_eq!(resolve_slug(base, ""), None);
    // From the root, as ingest reads it: the temporary directory itself.
    let real = fs::canonicalize(base.join("work/other")).unwrap();
    let name = slug(real.to_str().unwrap());
    assert_eq!(
        resolve_slug(Path::new("/"), &name).map(|p| fs::canonicalize(p).unwrap()),
        Some(real)
    );
    assert_eq!(slug_of(&main_file()), Some("work-example"));
}
