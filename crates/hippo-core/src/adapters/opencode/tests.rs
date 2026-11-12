use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::Connection;

use super::*;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/opencode");

const S1: &str = "ses_1111synthetic0000000001";
const S2: &str = "ses_2222synthetic0000000002";
const S3: &str = "ses_3333synthetic0000000003";
const S4: &str = "ses_4444synthetic0000000004";
const S5: &str = "ses_5555synthetic0000000005";
const S6: &str = "ses_6666synthetic0000000006";
const S7: &str = "ses_7777synthetic0000000007";

/// Loads `files` (under `fixtures/opencode`) into a new `opencode.db` in
/// `dir`, a folder named `opencode` like OpenCode's own.
pub(crate) fn build(dir: &Path, files: &[&str]) -> PathBuf {
    let data = dir.join("opencode");
    fs::create_dir_all(&data).unwrap();
    let db = data.join("opencode.db");
    let conn = Connection::open(&db).unwrap();
    for f in files {
        let sql = fs::read_to_string(Path::new(FIXTURES).join(f)).unwrap();
        conn.execute_batch(&sql).unwrap();
    }
    db
}

/// The fixture database with its seven sessions.
pub(crate) fn sessions(dir: &Path) -> PathBuf {
    build(dir, &["schema.sql", "sessions.sql"])
}

fn parse(db: &Path, sid: &str) -> Parsed {
    OpenCode
        .parse(db.parent().unwrap(), &session_path(db, sid))
        .unwrap()
}

fn shape(p: &Parsed) -> Vec<(Role, Option<&str>, &str)> {
    p.turns
        .iter()
        .map(|t| {
            let head = t.text.lines().next().unwrap_or("");
            (t.role, t.tool_name.as_deref(), head)
        })
        .collect()
}

#[test]
fn the_normal_session_parses_into_its_turns() {
    let dir = tempfile::tempdir().unwrap();
    let db = sessions(dir.path());
    let p = parse(&db, S1);
    assert_eq!(p.id, format!("opencode-{S1}"));
    assert_eq!(p.tool, Tool::OpenCode);
    assert_eq!(p.started, time::macros::datetime!(2026-11-01 10:00:00 UTC));
    let got = shape(&p);
    assert_eq!(got[0].0, Role::User);
    assert!(got[0].2.starts_with("The checkout total is wrong"));
    assert_eq!(
        got[1..],
        [
            (
                Role::Assistant,
                None,
                "I'll run the checkout tests first."
            ),
            (
                Role::Tool,
                Some("Bash"),
                r#"Bash {"command":"npm test -- checkout","description":"Run the checkout tests"}"#
            ),
            (Role::Tool, Some("Bash"), "Exit code 1"),
            (
                Role::Tool,
                Some("Read"),
                r#"Read {"file_path":"/home/synthetic/shop/src/cart.ts"}"#
            ),
            (
                Role::Tool,
                Some("Read"),
                "export const discount = 0.1 // AKIAFAKEFAKEFAKE0000"
            ),
            (
                Role::Tool,
                Some("Edit"),
                r#"Edit {"file_path":"/etc/hosts","new_string":"b","old_string":"a"}"#
            ),
            (
                Role::Tool,
                Some("Edit"),
                "<tool_use_error>Error: Cannot write /etc/hosts: outside the project</tool_use_error>"
            ),
            (
                Role::Tool,
                Some("Agent"),
                r#"Agent {"description":"Check the cart totals","prompt":"Find where the discount is applied twice.","subagent_type":"explore"}"#
            ),
            (
                Role::Tool,
                Some("Agent"),
                r#"<subagent sessionID="ses_2222synthetic0000000002" state="completed">"#
            ),
            (
                Role::Tool,
                Some("Edit"),
                r#"Edit {"file_path":"/home/synthetic/shop/src/total.ts","new_string":"price","old_string":"price * (1 - discount)","replace_all":false}"#
            ),
            (Role::Tool, Some("Edit"), "Edit applied successfully."),
            (
                Role::Tool,
                Some("apply_patch"),
                r#"apply_patch {"input":"*** Begin Patch\n*** Update File: src/cart.ts\n@@\n-old\n+new\n*** End Patch"}"#
            ),
            (
                Role::Tool,
                Some("apply_patch"),
                "Success. Updated the following files:"
            ),
            (
                Role::Assistant,
                None,
                "Fixed: the checkout total applies the discount once. The test client used Authorization: Bearer FAKEfake0000FAKEfake0000FAKE, which you should rotate; ask fake.person@example.com."
            ),
        ]
    );
    // System text and reasoning are never turns.
    assert!(!p.turns.iter().any(|t| t.text.contains("NOT A TURN")));
    // Each turn has its own time: a result when it completed.
    let exit = p
        .turns
        .iter()
        .find(|t| t.text.starts_with("Exit code"))
        .unwrap();
    assert_eq!(exit.ts, time::macros::datetime!(2026-11-01 10:00:03.1 UTC));
    let ordinals: Vec<u32> = p.turns.iter().map(|t| t.ordinal).collect();
    assert_eq!(ordinals, (0..p.turns.len() as u32).collect::<Vec<_>>());
}

#[test]
fn failed_results_are_written_as_hippos_readers_know_them() {
    assert_eq!(
        result_text("Bash", "FAIL x\n  expected 1\nExit code: 1", false),
        "Exit code 1\nFAIL x\n  expected 1"
    );
    assert_eq!(
        result_text("Bash", "killed\nExit code: 137 (SIGKILL)", false),
        "Exit code 137\nkilled"
    );
    assert_eq!(result_text("Bash", "Exit code: 2", false), "Exit code 2\n");
    // Only a shell result's last line, and only that wording.
    assert_eq!(result_text("Read", "Exit code: 1", false), "Exit code: 1");
    assert_eq!(
        result_text("Bash", "Exit code: 1\nmore", false),
        "Exit code: 1\nmore"
    );
    assert_eq!(
        result_text("Edit", "denied", true),
        "<tool_use_error>denied</tool_use_error>"
    );
}

#[test]
fn core_tools_take_claude_codes_names_and_argument_names() {
    let input = |name: &str, v: Value| stored_input(name, v.as_object().unwrap().clone());
    use serde_json::json;
    for (from, to) in [
        ("shell", "Bash"),
        ("bash", "Bash"),
        ("read", "Read"),
        ("write", "Write"),
        ("edit", "Edit"),
        ("patch", "apply_patch"),
        ("apply_patch", "apply_patch"),
        ("glob", "Glob"),
        ("grep", "Grep"),
        ("webfetch", "WebFetch"),
        ("websearch", "WebSearch"),
        ("todowrite", "TodoWrite"),
        ("question", "AskUserQuestion"),
        ("skill", "Skill"),
        ("subagent", "Agent"),
        ("task", "Agent"),
        ("github_create_issue", "github_create_issue"),
    ] {
        assert_eq!(stored_name(from), to, "{from}");
    }
    assert_eq!(
        input("write", json!({"path": "/a", "content": "x"})),
        json!({"file_path": "/a", "content": "x"})
    );
    assert_eq!(
        input("read", json!({"filePath": "/a", "limit": 5})),
        json!({"file_path": "/a", "limit": 5})
    );
    assert_eq!(
        input("skill", json!({"id": "pdf"})),
        json!({"skill": "pdf"})
    );
    assert_eq!(
        input(
            "question",
            json!({"questions": [{"question": "q", "multiple": true}]})
        ),
        json!({"questions": [{"question": "q", "multiSelect": true}]})
    );
    // A name Claude Code's arguments already use is not overwritten.
    assert_eq!(
        input("edit", json!({"path": "/p", "file_path": "/f"})),
        json!({"path": "/p", "file_path": "/f"})
    );
    // Other tools keep their arguments as written.
    assert_eq!(
        input("mcp_thing", json!({"filePath": "/a"})),
        json!({"filePath": "/a"})
    );
}

#[test]
fn a_sub_agent_is_named_under_the_top_of_its_chain() {
    let dir = tempfile::tempdir().unwrap();
    let db = sessions(dir.path());
    let top = format!("opencode-{S1}");
    assert_eq!(parse(&db, S2).id, format!("{top}-agent-{S2}"));
    assert_eq!(parse(&db, S6).id, format!("{top}-agent-{S6}"));
    assert_eq!(
        ids(&session_path(&db, S6)),
        Some((format!("{top}-agent-{S6}"), Some(top.clone())))
    );
    assert_eq!(ids(&session_path(&db, S1)), Some((top, None)));
    // A sub-agent's first message is the task it was given.
    assert!(parse(&db, S2).turns[0]
        .text
        .starts_with("You are a subagent spawned"));
}

#[test]
fn a_fork_reads_its_own_turns_only() {
    let dir = tempfile::tempdir().unwrap();
    let db = sessions(dir.path());
    let p = parse(&db, S3);
    assert_eq!(p.id, format!("opencode-{S3}"));
    let texts: Vec<&str> = p.turns.iter().map(|t| t.text.as_str()).collect();
    assert_eq!(
        texts,
        [
            "Try keeping the discount in total.ts instead.",
            "Kept it in total.ts and removed it from cart.ts."
        ]
    );
    // Without the session it was forked from, the copied messages are
    // known by their age.
    let conn = Connection::open(&db).unwrap();
    conn.execute(
        "UPDATE session_message SET id = 'gone' WHERE id = 'msg_1111synthetic0000000001_003'",
        [],
    )
    .unwrap();
    drop(conn);
    assert_eq!(parse(&db, S3).turns.len(), 2);
}

#[test]
fn a_live_session_ends_on_its_call_and_is_running() {
    let dir = tempfile::tempdir().unwrap();
    let db = sessions(dir.path());
    let p = parse(&db, S4);
    let last = p.turns.last().unwrap();
    assert_eq!(last.tool_name.as_deref(), Some("AskUserQuestion"));
    assert!(
        last.text.contains(r#""multiSelect":false"#),
        "{}",
        last.text
    );
    assert!(info(&session_path(&db, S4)).unwrap().running());
    let p = parse(&db, S5);
    assert_eq!(
        shape(&p).last().copied(),
        Some((
            Role::Tool,
            Some("Bash"),
            r#"Bash {"command":"npm run test:slow"}"#
        ))
    );
    assert!(info(&session_path(&db, S5)).unwrap().running());
    assert!(!info(&session_path(&db, S1)).unwrap().running());
    // Fleet's tail sees the question with no answer.
    let lines = as_claude_lines(&session_path(&db, S4), 50);
    assert_eq!(lines.len(), 2);
    assert!(
        lines[1].contains(r#""name":"AskUserQuestion""#),
        "{}",
        lines[1]
    );
    assert!(!lines[1].contains("tool_result"));
    assert!(lines[1].contains(r#""stop_reason":null"#));
    // A finished reply without a call ends its turn; results follow calls.
    let lines = as_claude_lines(&session_path(&db, S1), 50);
    assert!(lines
        .last()
        .unwrap()
        .contains(r#""stop_reason":"end_turn""#));
    assert!(lines
        .iter()
        .any(|l| l.contains(r#""tool_use_id":"call_s1_1""#) && l.contains("Exit code 1")));
    assert_eq!(
        last_turn_at(&session_path(&db, S1)),
        Some(time::macros::datetime!(2026-11-01 10:00:40 UTC))
    );
}

#[test]
fn part_rows_come_before_the_inline_content() {
    let dir = tempfile::tempdir().unwrap();
    let db = sessions(dir.path());
    let p = parse(&db, S7);
    assert_eq!(
        shape(&p),
        [
            (Role::User, None, "List the routes."),
            (
                Role::Tool,
                Some("Glob"),
                r#"Glob {"pattern":"src/routes/*.ts"}"#
            ),
            (Role::Tool, Some("Glob"), "src/routes/cart.ts"),
            (Role::Assistant, None, "Two route files."),
            (Role::Assistant, None, "Those are all the routes."),
        ]
    );
    // OpenCode's placeholder title is not a name.
    assert!(is_placeholder_title(
        info(&session_path(&db, S7))
            .unwrap()
            .title
            .as_deref()
            .unwrap()
    ));
}

#[test]
fn sessions_are_listed_with_stamps_that_change_with_them() {
    let dir = tempfile::tempdir().unwrap();
    let db = sessions(dir.path());
    let listed = OpenCode.sessions_in(&db).unwrap().unwrap();
    let paths: Vec<PathBuf> = listed.iter().map(|l| l.path.clone()).collect();
    let mut want: Vec<PathBuf> = [S1, S2, S3, S4, S5, S6, S7]
        .iter()
        .map(|s| session_path(&db, s))
        .collect();
    want.sort();
    assert_eq!(paths, want);
    let stamp = |listed: &[Listed], sid: &str| {
        listed
            .iter()
            .find(|l| l.path == session_path(&db, sid))
            .unwrap()
            .stamp
            .clone()
    };
    assert_eq!(stamp(&listed, S1), "opencode2:40:1793527200000:6");
    // A new message, or a change OpenCode commits, changes the stamp; the
    // other sessions keep theirs.
    let conn = Connection::open(&db).unwrap();
    conn.execute(
        "UPDATE event_sequence SET seq = 41 WHERE aggregate_id = ?1",
        [S1],
    )
    .unwrap();
    drop(conn);
    let again = OpenCode.sessions_in(&db).unwrap().unwrap();
    assert_ne!(stamp(&again, S1), stamp(&listed, S1));
    assert_eq!(stamp(&again, S2), stamp(&listed, S2));
    // The newest of the session's times is when it changed: here, its
    // newest message, after the run started.
    let s4 = listed
        .iter()
        .find(|l| l.path == session_path(&db, S4))
        .unwrap();
    assert_eq!(
        s4.modified,
        Some(time::macros::datetime!(2026-11-01 10:03:22 UTC))
    );
    // Other files are not databases.
    assert!(OpenCode.sessions_in(Path::new("/x/a.jsonl")).is_none());
}

#[test]
fn a_database_of_another_schema_is_refused_with_its_version() {
    let dir = tempfile::tempdir().unwrap();
    let db = build(dir.path(), &["newer.sql"]);
    let err = OpenCode.sessions_in(&db).unwrap().unwrap_err().to_string();
    assert!(err.contains("20991231000000_synthetic_future"), "{err}");
    assert!(err.contains("session_message.seq"), "{err}");
    assert!(err.contains("a newer hippo is needed"), "{err}");

    // OpenCode 1's tables only: older than hippo reads.
    let old = tempfile::tempdir().unwrap();
    let data = old.path().join("opencode");
    fs::create_dir_all(&data).unwrap();
    let db = data.join("opencode.db");
    let conn = Connection::open(&db).unwrap();
    conn.execute_batch(
        "CREATE TABLE migration (id TEXT PRIMARY KEY, time_completed INTEGER NOT NULL);
         INSERT INTO migration VALUES ('20260510033149_session_usage', 0);
         CREATE TABLE session (id TEXT PRIMARY KEY);",
    )
    .unwrap();
    drop(conn);
    let err = check_path(&db).unwrap_err();
    assert!(err.contains("older than hippo reads"), "{err}");
    assert!(err.contains("20260510033149_session_usage"), "{err}");
}

#[test]
fn a_newer_schema_with_the_same_tables_is_read() {
    let dir = tempfile::tempdir().unwrap();
    let db = sessions(dir.path());
    assert_eq!(check_path(&db), Ok(Schema::Known(NEWEST_KNOWN.into())));
    let conn = Connection::open(&db).unwrap();
    conn.execute_batch(
        "INSERT INTO migration VALUES ('20991231000000_synthetic_future', 0);
         ALTER TABLE session_v2 ADD COLUMN synthetic_new integer;",
    )
    .unwrap();
    drop(conn);
    assert_eq!(
        check_path(&db),
        Ok(Schema::Newer("20991231000000_synthetic_future".into()))
    );
    assert_eq!(parse(&db, S1).turns.len(), 15);
}

#[test]
fn the_database_is_read_while_opencode_writes_to_it_and_is_never_written() {
    let dir = tempfile::tempdir().unwrap();
    let db = sessions(dir.path());
    let writer = Connection::open(&db).unwrap();
    let mode: String = writer
        .query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode, "wal");
    // OpenCode holds a write transaction open: hippo still reads the last
    // committed state.
    writer.execute_batch("BEGIN IMMEDIATE").unwrap();
    writer
        .execute(
            "INSERT INTO session_message (id, session_id, type, seq, time_created, time_updated, data) \
             VALUES ('msg_new', ?1, 'user', 99, 1793527900000, 1793527900000, '{\"text\":\"UNCOMMITTED\"}')",
            [S1],
        )
        .unwrap();
    let p = parse(&db, S1);
    assert!(!p.turns.iter().any(|t| t.text.contains("UNCOMMITTED")));
    writer.execute_batch("COMMIT").unwrap();
    assert!(parse(&db, S1)
        .turns
        .iter()
        .any(|t| t.text.contains("UNCOMMITTED")));
    drop(writer);

    // A connection hippo opens cannot write.
    let conn = open(&db).unwrap();
    let err = conn
        .execute("DELETE FROM session_message", [])
        .unwrap_err()
        .to_string();
    assert!(err.contains("readonly"), "{err}");
}

#[test]
fn database_names_and_session_paths() {
    for (name, yes) in [
        ("opencode.db", true),
        ("opencode-local.db", true),
        ("opencode-beta.db", true),
        ("opencode-next.db", false),
        ("opencode-.db", false),
        ("opencode.db-wal", false),
        ("opencode.db-shm", false),
        ("other.db", false),
        ("opencode-a b.db", false),
    ] {
        assert_eq!(
            is_database(Path::new("/d").join(name).as_path()),
            yes,
            "{name}"
        );
    }
    let db = Path::new("/home/x/.local/share/opencode/opencode.db");
    let path = session_path(db, "ses_abc");
    assert_eq!(
        path,
        Path::new("/home/x/.local/share/opencode/opencode.db#ses_abc")
    );
    assert_eq!(split(&path), Some((db.to_path_buf(), "ses_abc".into())));
    assert!(OpenCode.file_matches(&path) && OpenCode.file_matches(db));
    assert_eq!(split(Path::new("/x/a.jsonl#ses_1")), None);
    assert_eq!(split(db), None);
}

#[test]
fn walks_do_not_go_below_opencodes_data_folder() {
    let dir = tempfile::tempdir().unwrap();
    let db = sessions(dir.path());
    let data = db.parent().unwrap();
    let repo = data.join("repos/some-repo");
    fs::create_dir_all(&repo).unwrap();
    fs::write(repo.join("events.jsonl"), "{}\n").unwrap();
    // Another folder named opencode, without a database, is walked.
    let other = dir.path().join("work/opencode/notes");
    fs::create_dir_all(&other).unwrap();
    fs::write(other.join("a.jsonl"), "{}\n").unwrap();
    let files: Vec<PathBuf> = walkdir::WalkDir::new(dir.path())
        .into_iter()
        .filter_entry(crate::adapters::descend)
        .flatten()
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        .collect();
    assert!(files.contains(&db));
    assert!(files.contains(&other.join("a.jsonl")));
    assert!(!files.iter().any(|f| f.starts_with(data.join("repos"))));
    assert_eq!(databases(&[dir.path().to_path_buf()]), [db]);
}

#[test]
fn the_working_directory_and_model_come_from_the_database() {
    let dir = tempfile::tempdir().unwrap();
    let db = sessions(dir.path());
    let path = session_path(&db, S1);
    assert_eq!(
        OpenCode.cwd("x", &path),
        Some(PathBuf::from("/home/synthetic/shop"))
    );
    assert_eq!(
        model(&path),
        (Some("synthetic-coder-1".into()), Some("high".into()))
    );
    assert_eq!(model(&session_path(&db, S2)).1.as_deref(), Some("low"));
}

#[test]
fn a_missing_session_or_a_bad_message_is_handled() {
    let dir = tempfile::tempdir().unwrap();
    let db = sessions(dir.path());
    let err = OpenCode
        .parse(db.parent().unwrap(), &session_path(&db, "ses_gone"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("no such OpenCode session"), "{err}");
    let conn = Connection::open(&db).unwrap();
    conn.execute(
        "UPDATE session_message SET data = 'not json' WHERE session_id = ?1 AND seq = 1",
        [S2],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO session_message (id, session_id, type, seq, time_created, time_updated, data) \
         VALUES ('msg_odd', ?1, 'some-future-type', 50, 1793527900000, 1793527900000, '{\"text\":\"ODD\"}')",
        [S2],
    )
    .unwrap();
    drop(conn);
    let p = parse(&db, S2);
    assert!(p.turns[0].text.starts_with("Grep"), "{:?}", p.turns[0]);
    assert!(!p.turns.iter().any(|t| t.text.contains("ODD")));
}

const S8: &str = "ses_8888synthetic0000000008";

/// The fixture database with the endings session added.
fn with_endings(dir: &Path) -> PathBuf {
    build(dir, &["schema.sql", "sessions.sql", "endings.sql"])
}

#[test]
fn endings_attachments_and_a_compaction_become_turns() {
    let dir = tempfile::tempdir().unwrap();
    let db = with_endings(dir.path());
    let p = parse(&db, S8);
    let texts: Vec<(Role, &str)> = p.turns.iter().map(|t| (t.role, t.text.as_str())).collect();
    let png = base64_len(&b64(b"\x89PNG synthetic image bytes, never indexed"));
    let pdf = base64_len(&b64(b"%PDF synthetic spec bytes"));
    assert_eq!(
        texts[0],
        (
            Role::User,
            format!(
                "Why does the cart page look broken?\n\
                 [Image: cart-broken.png, image/png, {png} bytes]\n\
                 [File: fake.person@example.com spec.pdf, application/pdf, {pdf} bytes, /home/synthetic/shop/docs/fake.person@example.com spec.pdf]"
            )
            .as_str()
        )
    );
    // The read's result: its text, then one line per file, the data URL's
    // size only.
    let result = p
        .turns
        .iter()
        .find(|t| t.role == Role::Tool && t.text.starts_with("Image read."))
        .unwrap();
    assert_eq!(
        result.text,
        format!(
            "Image read.\n[Image: cart.png, image/png, {png} bytes]\n\
             [File: cart.css, text/css, /home/synthetic/shop/public/cart.css]"
        )
    );
    assert!(p.turns.iter().all(|t| !t.text.contains("base64,")));
    let rest: Vec<(Role, &str)> = texts[4..].to_vec();
    assert_eq!(
        rest,
        [
            // The 429 step; the failed idle after it adds nothing.
            (
                Role::Assistant,
                "API Error: 429 provider.free-limit: Free usage limit reached for synthetic-coder-1."
            ),
            (
                Role::User,
                "[Compaction summary, auto]\nSYNTHETIC SUMMARY: the cart page lost its grid styles."
            ),
            (Role::User, "Carry on with the grid."),
            (Role::Assistant, "Restoring the grid"),
            // Aborted; the interrupted idle adds nothing.
            (Role::User, "[Request interrupted by user]"),
            (Role::User, "Try again."),
            (
                Role::Assistant,
                "Failed to authenticate: 401: Invalid API key for provider synthetic."
            ),
            (Role::User, "One last try."),
            (Role::Assistant, "The grid is back."),
            // A failed run whose steps had no error.
            (
                Role::Assistant,
                "API Error: 503 provider.internal: Upstream connection reset."
            ),
        ]
    );
    // An error turn is stamped when its step ended.
    assert_eq!(p.turns[4].ts, ms(1_793_528_002_100).unwrap());
    // The compaction reads as an earlier conversation's summary.
    assert_eq!(
        crate::session_meta::classify("user", None, texts[5].1),
        crate::session_meta::TurnKind::Injection(crate::session_meta::Injection::Summary)
    );
}

fn b64(bytes: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= c.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[test]
fn base64_sizes_come_from_the_length_alone() {
    for n in 0..12usize {
        let bytes = vec![7u8; n];
        assert_eq!(base64_len(&b64(&bytes)), n as u64, "{n}");
    }
    assert_eq!(data_url_len("text/plain,hello"), Some(5));
    assert_eq!(data_url_len("image/png;base64,AAAA"), Some(3));
    assert_eq!(data_url_len("no comma"), None);
    assert_eq!(attachment_line(None, None, None, None), "[File]");
    assert_eq!(
        attachment_line(Some("a.png"), Some("image/png"), Some(3), None),
        "[Image: a.png, image/png, 3 bytes]"
    );
    assert_eq!(
        path_of_uri("file:///a/b%20c.txt").as_deref(),
        Some("/a/b c.txt")
    );
    assert_eq!(path_of_uri("file://localhost/a").as_deref(), Some("/a"));
    assert_eq!(path_of_uri("/abs").as_deref(), Some("/abs"));
    assert_eq!(path_of_uri("media:00"), None);
    assert_eq!(path_of_uri("https://x/y"), None);
    assert_eq!(percent_decode("100%"), "100%");
}

#[test]
fn a_sub_agent_result_carries_its_token_use() {
    let dir = tempfile::tempdir().unwrap();
    let db = with_endings(dir.path());
    let conn = open(&db).unwrap();
    // Three steps of 1 950 tokens each, one tool call.
    assert_eq!(sub_agent_usage(&conn, S8), Some((Some(5850), 1)));
    // A session with no token counts: tool calls only.
    assert_eq!(sub_agent_usage(&conn, S2), Some((None, 2)));
    assert_eq!(sub_agent_usage(&conn, "ses_gone"), None);
    let p = parse(&db, S1);
    let agent = p
        .turns
        .iter()
        .find(|t| t.tool_name.as_deref() == Some("Agent") && t.text.starts_with("<subagent"))
        .unwrap();
    assert!(
        agent
            .text
            .ends_with("</subagent>\n<usage>tool_uses: 2</usage>"),
        "{}",
        agent.text
    );
}
