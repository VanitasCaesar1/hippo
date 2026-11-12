//! Tests of the rollup. Every transcript here is synthetic.

use super::*;
use crate::adapters::Role;
use crate::index::{SessionSource, Turn};
use crate::records::Tool;
use crate::redact::redact;

fn at(ts: &str) -> OffsetDateTime {
    OffsetDateTime::parse(ts, &Rfc3339).unwrap()
}

fn stored(role: &str, ts: &str, tool: Option<&str>, text: &str) -> StoredTurn {
    StoredTurn {
        ordinal: 0,
        role: role.to_owned(),
        ts: ts.to_owned(),
        tool_name: tool.map(str::to_owned),
        text: text.to_owned(),
    }
}

fn turn(ordinal: u32, role: Role, ts: &str, tool: Option<&str>, text: &str) -> Turn {
    Turn {
        ordinal,
        role,
        ts: at(ts),
        tool_name: tool.map(str::to_owned),
        text: redact(text),
    }
}

fn put(index: &mut Index, id: &str, path: &str, hash: &str, turns: &[Turn]) {
    let source = SessionSource {
        id,
        tool: Tool::ClaudeCode,
        transcript_path: Path::new(path),
        file_hash: hash,
    };
    index.replace_session(&source, turns).unwrap();
}

const T: &str = "2026-11-08T08:00:00Z";

#[test]
fn a_usage_limit_reply_is_a_usage_limit_with_its_reset() {
    let turns = [
        stored("user", T, None, "build the scheduler"),
        stored("tool", T, Some("Bash"), "Bash {\"command\":\"ls\"}"),
        stored(
            "assistant",
            "2026-11-08T09:44:00Z",
            None,
            "You've hit your 5-hour limit · resets 6:10pm (Australia/Darwin)",
        ),
    ];
    let c = classify(&turns);
    assert_eq!(c.stored, Stored::UsageLimit);
    assert_eq!(c.detail.as_deref(), Some("resets 6:10pm"));
    assert_eq!(c.at.as_deref(), Some("2026-11-08T09:44:00Z"));
}

#[test]
fn api_errors_are_rate_limits_or_interruptions() {
    let ended = |text: &str| {
        classify(&[
            stored("user", T, None, "go"),
            stored("assistant", T, None, text),
        ])
    };
    let rate = ended("API Error: 429 rate_limit_error: too many requests");
    assert_eq!(rate.stored, Stored::RateLimited);
    assert_eq!(
        ended("API Error: 529 Overloaded").stored,
        Stored::RateLimited
    );
    let other = ended("API Error: Can't reach the API server (ENOTFOUND)\nmore");
    assert_eq!(other.stored, Stored::Interrupted);
    assert_eq!(
        other.detail.as_deref(),
        Some("API Error: Can't reach the API server (ENOTFOUND)")
    );
}

#[test]
fn a_sign_in_failure_is_an_interruption_and_no_outcome() {
    let turns = [
        stored("user", T, None, "post about the launch"),
        stored("assistant", T, None, "Drafting it now."),
        stored(
            "assistant",
            "2026-11-08T09:44:00Z",
            None,
            "Failed to authenticate: this login reached its time limit and cannot be renewed. Run `claude auth login` to sign in again.",
        ),
    ];
    let c = classify(&turns);
    assert_eq!(c.stored, Stored::Interrupted);
    assert_eq!(c.detail.as_deref(), Some(SIGNED_OUT));
    assert_eq!(compute(&turns).outcome.as_deref(), Some("Drafting it now."));
}

#[test]
fn an_interruption_a_reply_and_a_mid_step_end() {
    let stopped = classify(&[
        stored("user", T, None, "go"),
        stored("tool", T, Some("Bash"), "Bash {\"command\":\"make\"}"),
        stored(
            "user",
            T,
            None,
            "[Request interrupted by user for tool use]",
        ),
    ]);
    assert_eq!(stopped.stored, Stored::Interrupted);
    assert_eq!(stopped.detail.as_deref(), Some("stopped by you"));

    let done = classify(&[
        stored("user", T, None, "go"),
        stored("assistant", T, None, "Done: merged."),
    ]);
    assert_eq!(done.stored, Stored::Completed);

    // A reply, then more calls: the transcript ends mid-step.
    let mid = classify(&[
        stored("user", T, None, "go"),
        stored("assistant", T, None, "Running the tests now."),
        stored("tool", T, Some("Bash"), "Bash {\"command\":\"cargo test\"}"),
    ]);
    assert_eq!(mid.stored, Stored::Unfinished);
    assert_eq!(classify(&[]).stored, Stored::Unfinished);
}

#[test]
fn unfinished_is_open_while_recent_and_interrupted_after() {
    let last = at("2026-11-08T10:00:00Z");
    assert_eq!(
        Ending::of(Stored::Unfinished, Some(last), last + Duration::minutes(10)),
        Ending::Open
    );
    assert_eq!(
        Ending::of(Stored::Unfinished, Some(last), last + Duration::hours(3)),
        Ending::Interrupted
    );
    assert_eq!(
        Ending::of(Stored::Unfinished, None, last),
        Ending::Interrupted
    );
    assert_eq!(
        Ending::of(Stored::Completed, Some(last), last),
        Ending::Completed
    );
    assert!(Ending::UsageLimit.is_problem());
    assert!(!Ending::Open.is_problem());
}

#[test]
fn profiles_come_from_the_transcript_root() {
    let p = |path: &str| profile_of("claude-code", Path::new(path));
    assert_eq!(
        p("/Users/a/.claude/projects/x/1.jsonl").as_deref(),
        Some("default")
    );
    assert_eq!(
        p("/Users/a/.claude-work/projects/x/1.jsonl").as_deref(),
        Some("work")
    );
    assert_eq!(p("/elsewhere/1.jsonl"), None);
    assert_eq!(
        profile_of("codex", Path::new("/Users/a/.codex/sessions/r.jsonl")).as_deref(),
        Some("codex")
    );
}

#[test]
fn facts_count_calls_edits_agents_and_failures() {
    let turns = [
        stored("user", T, None, "Act as the implementation sub-agent for this task.\n\nAdd a retry queue to the webhook worker"),
        stored("tool", T, Some("Edit"), r#"Edit {"file_path":"/w/src/a.rs","old_string":"a","new_string":"b"}"#),
        stored("tool", T, Some("Edit"), "The file /w/src/a.rs has been updated."),
        stored("tool", T, Some("Write"), r#"Write {"file_path":"/w/src/b.rs","content":"x"}"#),
        stored("tool", T, Some("Edit"), r#"Edit {"file_path":"/w/src/a.rs","old_string":"b","new_string":"c"}"#),
        stored("tool", T, Some("Agent"), r#"Agent {"description":"Review the queue"}"#),
        stored("tool", T, Some("Bash"), r#"Bash {"command":"gh pr merge https://github.com/acme/widgets/pull/12 --squash"}"#),
        stored("user", T, None, "Delegated task node:delegated-task:abc (Widgets phase 2) [failed]: Claude API rate limit reached. Try again later."),
        stored("assistant", T, None, "Opened https://github.com/acme/widgets/pull/13 for review.\nMerged https://github.com/acme/widgets/pull/12."),
    ];
    let c = compute(&turns);
    assert_eq!(c.turns, 9);
    assert_eq!(c.tool_calls, 5);
    assert_eq!(c.edits, 3);
    assert_eq!(c.files, 2);
    assert_eq!(c.agents, 1);
    assert_eq!(c.events.len(), 1);
    assert_eq!(c.events[0].1, "delegate_failed");
    assert_eq!(
        c.events[0].2,
        "Widgets phase 2: Claude API rate limit reached. Try again later."
    );
    assert_eq!(
        c.asked.as_deref(),
        Some("Add a retry queue to the webhook worker")
    );
    assert!(c
        .outcome
        .as_deref()
        .unwrap()
        .starts_with("Opened https://github.com/acme/widgets/pull/13"));
    // One bucket: every turn is at the same time.
    assert_eq!(c.activity.len(), 1);
    assert_eq!(c.activity.values().next(), Some(&(9, 3)));
}

#[test]
fn the_outcome_skips_the_limit_message() {
    let c = compute(&[
        stored("user", T, None, "go"),
        stored(
            "assistant",
            T,
            None,
            "Phase two is built;\n\n- the tests pass.",
        ),
        stored(
            "assistant",
            T,
            None,
            "You've hit your 5-hour limit · resets 2:50pm",
        ),
    ]);
    assert_eq!(
        c.outcome.as_deref(),
        Some("Phase two is built;\n\n- the tests pass.")
    );
}

fn two_sessions(index: &mut Index) {
    // A parent across local midnight at +09:30 (14:20Z is 23:50 local).
    put(
        index,
        "parent-1",
        "/nowhere/.claude-work/projects/p/parent-1.jsonl",
        "h1",
        &[
            turn(
                0,
                Role::User,
                "2026-11-08T14:20:00Z",
                None,
                "Ship the invoice export",
            ),
            turn(
                1,
                Role::Tool,
                "2026-11-08T14:21:00Z",
                Some("Write"),
                r#"Write {"file_path":"/w/x.rs","content":"1"}"#,
            ),
            turn(
                2,
                Role::Assistant,
                "2026-11-08T14:35:00Z",
                None,
                "Exported. https://github.com/acme/books/pull/4 is merged.",
            ),
            turn(3, Role::User, "2026-11-08T14:40:00Z", None, "now the PDF"),
            turn(
                4,
                Role::Assistant,
                "2026-11-08T14:45:00Z",
                None,
                "You've hit your 5-hour limit · resets 6:10pm",
            ),
        ],
    );
    put(
        index,
        "other-2",
        "/nowhere/.claude/projects/q/other-2.jsonl",
        "h2",
        &[
            turn(
                0,
                Role::User,
                "2026-11-08T00:00:00Z",
                None,
                "Tidy the readme",
            ),
            turn(1, Role::Assistant, "2026-11-08T00:05:00Z", None, "Tidied."),
        ],
    );
}

#[test]
fn refresh_builds_days_hours_and_sessions() {
    let home = tempfile::tempdir().unwrap();
    let mut index = Index::open(home.path(), true).unwrap();
    two_sessions(&mut index);
    let done = refresh(home.path(), None, &index).unwrap();
    assert_eq!(
        done,
        Refreshed {
            updated: 2,
            removed: 0
        }
    );

    let rollup = Rollup::open(home.path()).unwrap().unwrap();
    let acst = UtcOffset::from_hms(9, 30, 0).unwrap();
    let days = rollup.days(acst).unwrap();
    let dates: Vec<String> = days.iter().map(|d| d.date.unwrap().to_string()).collect();
    // 14:20Z and 14:21Z are 8 Nov local; 14:35Z on are 9 Nov local.
    assert_eq!(dates, ["2026-11-09", "2026-11-08"]);
    assert_eq!(days[0].sessions, 1);
    assert_eq!(days[0].turns, 3);
    // The reply names a pull request; no call opened or merged it.
    assert_eq!(days[0].pulls, 0);
    assert_eq!(days[1].sessions, 2);
    assert_eq!(days[1].turns, 4);
    assert_eq!(days[1].edits, 1);

    let (start, end) = day_range(time::macros::date!(2026 - 11 - 08), acst);
    let in_day = rollup.sessions_in(start, end).unwrap();
    let ids: Vec<(&str, u64)> = in_day
        .iter()
        .map(|s| (s.facts.id.as_str(), s.turns))
        .collect();
    assert_eq!(ids, [("other-2", 2), ("parent-1", 2)]);
    // parent-1 starts on the 8th (local) and goes on into the 9th.
    let parent = in_day.iter().find(|s| s.facts.id == "parent-1").unwrap();
    assert_eq!(parent.first, Some(at("2026-11-08T14:20:00Z")));
    assert_eq!(parent.last, Some(at("2026-11-08T14:30:00Z")));
    let next = day_range(time::macros::date!(2026 - 11 - 09), acst);
    let tomorrow = rollup.sessions_in(next.0, next.1).unwrap();
    assert_eq!(tomorrow[0].first, Some(at("2026-11-08T14:30:00Z")));
    assert_eq!(tomorrow[0].last, Some(at("2026-11-08T14:45:00Z")));
    let hours = rollup.activity(start, end).unwrap();
    assert_eq!(hours.len(), 2);
    assert_eq!(hours[0].0, at("2026-11-08T00:00:00Z"));

    assert_eq!(
        rollup.titles().unwrap().get("other-2").map(String::as_str),
        Some("Tidy the readme")
    );
    let summary = rollup.summary().unwrap();
    assert_eq!(summary.sessions, 2);
    assert_eq!(summary.profiles, ["default", "work"]);
    assert_eq!(summary.first, Some(at("2026-11-08T00:00:00Z")));

    let facts = rollup.session("parent-1").unwrap().unwrap();
    assert_eq!(facts.title.as_deref(), Some("Ship the invoice export"));
    assert_eq!(facts.profile().as_deref(), Some("work"));
    assert_eq!(facts.stored, Stored::UsageLimit);
    assert_eq!(facts.ending_detail.as_deref(), Some("resets 6:10pm"));
    assert_eq!(facts.ending(at("2026-11-09T00:00:00Z")), Ending::UsageLimit);
    assert_eq!(
        facts.outcome.as_deref(),
        Some("Exported. https://github.com/acme/books/pull/4 is merged.")
    );
    assert_eq!(facts.edits, 1);
    assert!(rollup.pulls_of("parent-1").unwrap().is_empty());
}

#[test]
fn refresh_recomputes_only_changed_sessions_and_drops_removed_ones() {
    let home = tempfile::tempdir().unwrap();
    let mut index = Index::open(home.path(), true).unwrap();
    two_sessions(&mut index);
    refresh(home.path(), None, &index).unwrap();
    assert_eq!(
        refresh(home.path(), None, &index).unwrap(),
        Refreshed::default()
    );
    put(
        &mut index,
        "other-2",
        "/nowhere/.claude/projects/q/other-2.jsonl",
        "h2b",
        &[turn(
            0,
            Role::User,
            "2026-11-08T00:00:00Z",
            None,
            "Tidy the readme",
        )],
    );
    assert_eq!(
        refresh(home.path(), None, &index).unwrap(),
        Refreshed {
            updated: 1,
            removed: 0
        }
    );
    let rollup = Rollup::open(home.path()).unwrap().unwrap();
    assert_eq!(rollup.session("other-2").unwrap().unwrap().turns, 1);
    drop(rollup);

    // A new index without parent-1 (as `hippocamp ingest --full` would leave).
    let fresh = tempfile::tempdir().unwrap();
    let mut other = Index::open(fresh.path(), true).unwrap();
    put(
        &mut other,
        "other-2",
        "/nowhere/.claude/projects/q/other-2.jsonl",
        "h2b",
        &[turn(
            0,
            Role::User,
            "2026-11-08T00:00:00Z",
            None,
            "Tidy the readme",
        )],
    );
    assert_eq!(
        refresh(home.path(), None, &other).unwrap(),
        Refreshed {
            updated: 0,
            removed: 1
        }
    );
    let rollup = Rollup::open(home.path()).unwrap().unwrap();
    assert!(rollup.session("parent-1").unwrap().is_none());
    assert!(rollup.pulls_of("parent-1").unwrap().is_empty());
}

#[test]
fn a_rollup_of_another_format_is_rebuilt_and_a_reader_ignores_it() {
    let home = tempfile::tempdir().unwrap();
    let mut index = Index::open(home.path(), true).unwrap();
    two_sessions(&mut index);
    {
        let conn = Connection::open(home.path().join(FILE_NAME)).unwrap();
        conn.execute_batch("CREATE TABLE other(x); PRAGMA user_version=8;")
            .unwrap();
    }
    assert!(Rollup::open(home.path()).unwrap().is_none());
    assert_eq!(refresh(home.path(), None, &index).unwrap().updated, 2);
    assert!(Rollup::open(home.path()).unwrap().is_some());

    // Not a database at all.
    remove(home.path()).unwrap();
    fs::write(home.path().join(FILE_NAME), "not sqlite").unwrap();
    assert_eq!(refresh(home.path(), None, &index).unwrap().updated, 2);
}

#[test]
fn no_file_is_no_rollup_and_in_memory_writes_nothing() {
    let home = tempfile::tempdir().unwrap();
    let mut index = Index::open(home.path(), true).unwrap();
    two_sessions(&mut index);
    assert!(Rollup::open(home.path()).unwrap().is_none());
    let memory = Rollup::in_memory(None, &index, &["other-2".to_owned()]).unwrap();
    assert_eq!(memory.session("other-2").unwrap().unwrap().turns, 2);
    assert!(memory.session("parent-1").unwrap().is_none());
    assert!(!home.path().join(FILE_NAME).exists());
}

#[test]
fn secrets_stay_redacted_in_the_rollup() {
    let home = tempfile::tempdir().unwrap();
    let mut index = Index::open(home.path(), true).unwrap();
    let key = "sk-ant-api03-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    put(
        &mut index,
        "s",
        "/nowhere/.claude/projects/q/s.jsonl",
        "h",
        &[
            turn(
                0,
                Role::User,
                T,
                None,
                &format!("Rotate the key {key} today"),
            ),
            turn(1, Role::Assistant, T, None, &format!("Rotated {key}.")),
        ],
    );
    refresh(home.path(), None, &index).unwrap();
    let facts = Rollup::open(home.path())
        .unwrap()
        .unwrap()
        .session("s")
        .unwrap()
        .unwrap();
    for text in [
        facts.asked.unwrap(),
        facts.outcome.unwrap(),
        facts.title.unwrap(),
    ] {
        assert!(!text.contains(key), "{text}");
    }
}

#[test]
fn notes_a_session_read_and_edited_are_kept() {
    let home = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let root_path = root.path().canonicalize().unwrap();
    fs::create_dir_all(root_path.join("notes/ops")).unwrap();
    fs::write(root_path.join("notes/ops/Deploy.md"), "# Deploy\n").unwrap();
    fs::write(root_path.join("notes/Billing.md"), "# Billing\n").unwrap();
    let ws = Workspace {
        root: root_path.clone(),
        notes_dir: "notes".into(),
        entry_note: "notes/Billing.md".into(),
        required_notes: Vec::new(),
        budget_bytes: crate::DEFAULT_CONTEXT_BUDGET,
        log_path: "notes/Session log.md".into(),
        sample_tasks: Vec::new(),
        passes_off: Vec::new(),
        lessons: true,
        snapshot_keep_days: None,
        log_notes: None,
    };
    let deploy = root_path.join("notes/ops/Deploy.md");
    let billing = root_path.join("notes/Billing.md");
    let mut index = Index::open(home.path(), true).unwrap();
    put(
        &mut index,
        "s",
        "/nowhere/.claude/projects/q/s.jsonl",
        "h",
        &[
            turn(0, Role::User, T, None, "update the deploy note"),
            turn(
                1,
                Role::Tool,
                T,
                Some("Read"),
                &format!(r#"Read {{"file_path":"{}"}}"#, billing.display()),
            ),
            turn(
                2,
                Role::Tool,
                "2026-11-08T08:05:00Z",
                Some("Edit"),
                &format!(
                    r#"Edit {{"file_path":"{}","old_string":"a","new_string":"b"}}"#,
                    deploy.display()
                ),
            ),
        ],
    );
    refresh(home.path(), Some(&ws), &index).unwrap();
    let rollup = Rollup::open(home.path()).unwrap().unwrap();
    let mut seen: Vec<(String, String)> = rollup
        .notes_of("s")
        .unwrap()
        .into_iter()
        .map(|n| (n.note.display().to_string(), n.kind))
        .collect();
    seen.sort();
    assert_eq!(
        seen,
        [
            ("notes/Billing.md".to_owned(), "read".to_owned()),
            ("notes/ops/Deploy.md".to_owned(), "edit".to_owned()),
        ]
    );
    let readers = rollup
        .sessions_of_note(Path::new("notes/ops/Deploy.md"))
        .unwrap();
    assert_eq!(readers.len(), 1);
    assert_eq!(readers[0].session_id, "s");
}

#[test]
fn a_sub_agent_names_its_parent_by_id_when_its_head_is_gone() {
    let home = tempfile::tempdir().unwrap();
    let mut index = Index::open(home.path(), true).unwrap();
    put(
        &mut index,
        "p1-agent-a7",
        "/nowhere/.claude/projects/q/p1/subagents/agent-a7.jsonl",
        "h",
        &[turn(0, Role::User, T, None, "Review the queue")],
    );
    refresh(home.path(), None, &index).unwrap();
    let facts = Rollup::open(home.path())
        .unwrap()
        .unwrap()
        .session("p1-agent-a7")
        .unwrap()
        .unwrap();
    assert_eq!(facts.parent.as_deref(), Some("p1"));
}

#[test]
fn changes_branch_and_tasks_are_kept_per_session() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join(".claude/projects/q");
    fs::create_dir_all(&dir).unwrap();
    // The transcript head names a branch; the session's commands name a
    // newer one.
    let head = dir.join("c1.jsonl");
    fs::write(
        &head,
        "{\"type\":\"user\",\"sessionId\":\"c1\",\"cwd\":\"/w\",\"gitBranch\":\"main\",\"message\":{\"content\":\"go\"}}\n",
    )
    .unwrap();
    let mut index = Index::open(home.path(), true).unwrap();
    let ts = "2026-11-08T08:01:00Z";
    put(
        &mut index,
        "c1",
        head.to_str().unwrap(),
        "h1",
        &[
            turn(0, Role::User, T, None, "fix the parser"),
            turn(
                1,
                Role::Tool,
                ts,
                Some("Edit"),
                r#"Edit {"file_path":"/w/src/a.rs","old_string":"a\nb","new_string":"a\nc\nd"}"#,
            ),
            turn(
                2,
                Role::Tool,
                ts,
                Some("Edit"),
                "The file /w/src/a.rs has been updated successfully.",
            ),
            turn(
                3,
                Role::Tool,
                ts,
                Some("Write"),
                r#"Write {"file_path":"/w/new.md","content":"x\ny\n"}"#,
            ),
            turn(
                4,
                Role::Tool,
                ts,
                Some("Write"),
                "File created successfully at: /w/new.md",
            ),
            turn(
                5,
                Role::Tool,
                ts,
                Some("Write"),
                r#"Write {"file_path":"/w/src/a.rs","content":"whole\n"}"#,
            ),
            turn(
                6,
                Role::Tool,
                ts,
                Some("Write"),
                "The file /w/src/a.rs has been updated successfully.",
            ),
            turn(
                7,
                Role::Tool,
                ts,
                Some("Bash"),
                r#"Bash {"command":"cd /w && git checkout -b fix/parser","description":"Branch"}"#,
            ),
            turn(
                8,
                Role::Tool,
                ts,
                Some("Agent"),
                r#"Agent {"description":"Check the tests","prompt":"p"}"#,
            ),
            turn(
                9,
                Role::Tool,
                "2026-11-08T08:04:00Z",
                Some("Agent"),
                "All pass.",
            ),
            turn(10, Role::Assistant, "2026-11-08T08:05:00Z", None, "Done."),
        ],
    );
    put(
        &mut index,
        "c2",
        dir.join("c2.jsonl").to_str().unwrap(),
        "h2",
        &[turn(0, Role::User, T, None, "hi")],
    );
    let other = dir.join("c2.jsonl");
    fs::write(
        &other,
        "{\"type\":\"user\",\"sessionId\":\"c2\",\"gitBranch\":\"docs/x\",\"message\":{\"content\":\"hi\"}}\n",
    )
    .unwrap();
    refresh(home.path(), None, &index).unwrap();
    let r = Rollup::open(home.path()).unwrap().unwrap();
    let facts = r.session("c1").unwrap().unwrap();
    assert_eq!(facts.branch.as_deref(), Some("fix/parser"));
    assert_eq!(
        r.session("c2").unwrap().unwrap().branch.as_deref(),
        Some("docs/x"),
        "no branch command: the transcript head's"
    );
    let changes = r.changes_of("c1").unwrap();
    let view: Vec<(&str, u64, u64, u64, u64)> = changes
        .iter()
        .map(|c| {
            (
                c.path.as_str(),
                c.lines.added,
                c.lines.removed,
                c.calls,
                c.uncounted,
            )
        })
        .collect();
    assert_eq!(
        view,
        vec![("/w/src/a.rs", 2, 1, 2, 1), ("/w/new.md", 2, 0, 1, 0)]
    );
    let tasks = r.tasks_of("c1").unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].title, "Check the tests");
    assert_eq!(
        tasks[0].ended.as_deref(),
        Some("2026-11-08T08:04:00.000000Z")
    );
    assert_eq!(tasks[0].status.as_deref(), Some("completed"));
    assert!(!r.merged_by_any("https://github.com/o/r/pull/1").unwrap());
}

#[test]
fn a_sub_task_knows_its_own_session_and_a_delegate_is_linked_through_t3_code() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join(".claude/projects/q");
    fs::create_dir_all(dir.join("p1/subagents")).unwrap();
    let mut index = Index::open(home.path(), true).unwrap();
    let ts = "2026-11-09T08:01:00Z";
    let key = "sk-ant-api03-BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB";
    put(
        &mut index,
        "p1",
        dir.join("p1.jsonl").to_str().unwrap(),
        "h1",
        &[
            turn(0, Role::User, ts, None, "split the work"),
            turn(
                1,
                Role::Tool,
                ts,
                Some("Agent"),
                &format!(r#"Agent {{"description":"Fix the docs","prompt":"Fix the docs; the key is {key}"}}"#),
            ),
            turn(
                2,
                Role::Tool,
                "2026-11-09T08:04:00Z",
                Some("Agent"),
                &format!("Fixed, and rotated {key}.\nagentId: a1 (use SendMessage)\n<usage>subagent_tokens: 500\ntool_uses: 4</usage>"),
            ),
            turn(
                3,
                Role::Tool,
                ts,
                Some("mcp__t3-code__delegate_task"),
                r#"mcp__t3-code__delegate_task {"title":"Lane B","task":"Build lane B."}"#,
            ),
            turn(
                4,
                Role::Tool,
                ts,
                Some("mcp__t3-code__delegate_task"),
                r#"{"taskId":"node:t-b","childThreadId":"thread:child-b","status":"running"}"#,
            ),
        ],
    );
    // The sub-agent's own transcript, and the delegated child's.
    put(
        &mut index,
        "p1-agent-a1",
        dir.join("p1/subagents/agent-a1.jsonl").to_str().unwrap(),
        "h2",
        &[
            turn(0, Role::User, ts, None, "Fix the docs"),
            turn(
                1,
                Role::Tool,
                ts,
                Some("Edit"),
                r#"Edit {"file_path":"/w/README.md","old_string":"a","new_string":"b\nc"}"#,
            ),
        ],
    );
    put(
        &mut index,
        "c9",
        dir.join("c9.jsonl").to_str().unwrap(),
        "h3",
        &[turn(0, Role::User, ts, None, "Build lane B.")],
    );
    put(
        &mut index,
        "c9-agent-n1",
        dir.join("c9/subagents/agent-n1.jsonl").to_str().unwrap(),
        "h4",
        &[turn(0, Role::User, ts, None, "nested")],
    );
    // Before T3 Code's state says where the thread ran, the delegate has
    // no session; a missing state file links nothing.
    refresh_with(home.path(), None, &index, None).unwrap();
    let r = Rollup::open(home.path()).unwrap().unwrap();
    let tasks = r.tasks_of("p1").unwrap();
    assert_eq!(tasks.len(), 2);
    assert_eq!(tasks[0].child.as_deref(), Some("p1-agent-a1"));
    assert_eq!((tasks[0].tokens, tasks[0].tools), (Some(500), Some(4)));
    for text in [
        tasks[0].input.clone().unwrap(),
        tasks[0].report.clone().unwrap(),
    ] {
        assert!(!text.contains(key), "redacted on ingest: {text}");
    }
    assert_eq!(tasks[1].kind, TaskKind::Delegate);
    assert_eq!(tasks[1].child, None);
    assert_eq!(r.children_of("p1").unwrap(), ["p1-agent-a1"]);
    drop(r);
    let t3 = home.path().join("statev2.sqlite");
    let conn = Connection::open(&t3).unwrap();
    conn.execute_batch(
        "CREATE TABLE orchestration_v2_projection_provider_threads(provider_thread_id TEXT PRIMARY KEY, thread_id TEXT, updated_at TEXT NOT NULL, payload_json TEXT NOT NULL);
         INSERT INTO orchestration_v2_projection_provider_threads VALUES ('x', 'thread:child-b', '2026-11-09T08:02:00Z', '{\"nativeThreadRef\":{\"nativeId\":\"c9\"}}');",
    )
    .unwrap();
    drop(conn);
    refresh_with(home.path(), None, &index, Some(&t3)).unwrap();
    let r = Rollup::open(home.path()).unwrap().unwrap();
    assert_eq!(r.tasks_of("p1").unwrap()[1].child.as_deref(), Some("c9"));
    assert_eq!(r.children_of("p1").unwrap(), ["c9", "p1-agent-a1"]);
    assert_eq!(
        r.descendants_of("p1").unwrap().len(),
        3,
        "the delegate's own sub-agent too"
    );
    let (parent, task) = r.delegated_by("c9").unwrap().unwrap();
    assert_eq!((parent.as_str(), task.title.as_str()), ("p1", "Lane B"));
    assert_eq!(
        r.delegations().unwrap().get("c9").map(String::as_str),
        Some("p1")
    );
    assert_eq!(r.changes_of("p1-agent-a1").unwrap()[0].lines.added, 2);
}

#[test]
fn the_ddl_writes_the_format_this_code_reads() {
    assert!(DDL.contains(&format!("PRAGMA user_version={FORMAT};")));
}
