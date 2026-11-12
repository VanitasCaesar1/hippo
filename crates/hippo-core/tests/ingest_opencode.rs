//! Ingests a database built from the synthetic SQL in `fixtures/opencode`
//! and reads it back: every session, sub-agents under their session, a
//! fork without its copied history, no leak from
//! `fixtures/redaction/leaks.txt` in the index or a derived record, a
//! second ingest that reads nothing, a third that reads only the session
//! that changed, and a database of another schema as a parse failure.

use std::fs;
use std::path::{Path, PathBuf};

use hippo_core::adapters::opencode;
use hippo_core::config::{Config, Workspace};
use hippo_core::index::Index;
use hippo_core::ingest::{ingest, IngestSummary};
use hippo_core::records::Session;
use hippo_core::session_meta;
use rusqlite::Connection;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures");

const S1: &str = "ses_1111synthetic0000000001";
const S2: &str = "ses_2222synthetic0000000002";
const S3: &str = "ses_3333synthetic0000000003";
const S6: &str = "ses_6666synthetic0000000006";

/// The redaction kinds, in the order they are applied.
const KINDS: [&str; 10] = [
    "pem", "aws", "github", "provider", "jwt", "auth", "kv", "email", "aadhaar", "phone",
];

fn leaks() -> Vec<String> {
    let text = fs::read_to_string(Path::new(FIXTURES).join("redaction/leaks.txt")).unwrap();
    text.lines().map(str::to_owned).collect()
}

fn hid(sid: &str) -> String {
    format!("opencode-{sid}")
}

fn sub(sid: &str) -> String {
    format!("opencode-{S1}-agent-{sid}")
}

/// Builds `<data>/opencode.db` from the fixture SQL, with the shop's
/// working directory moved to `cwd`.
fn build(data: &Path, cwd: &str, files: &[&str]) -> PathBuf {
    fs::create_dir_all(data).unwrap();
    let db = data.join("opencode.db");
    let conn = Connection::open(&db).unwrap();
    for f in files {
        let sql = fs::read_to_string(Path::new(FIXTURES).join("opencode").join(f)).unwrap();
        conn.execute_batch(&sql.replace("/home/synthetic/shop", cwd))
            .unwrap();
    }
    db
}

fn setup(home: &Path, workspace: &Path, roots: Vec<String>) -> (Config, Workspace) {
    let config = Config {
        home: home.to_path_buf(),
        workspace: Some(workspace.to_path_buf()),
        transcript_roots: roots,
        exclude_roots: Vec::new(),
        model_command: Vec::new(),
        ingest_interval_minutes: 10,
        dream_at: "03:30".into(),
        reviewer_command_override: None,
    };
    let ws = Workspace {
        root: workspace.to_path_buf(),
        notes_dir: "brain".into(),
        entry_note: "brain/START HERE.md".into(),
        required_notes: Vec::new(),
        budget_bytes: hippo_core::DEFAULT_CONTEXT_BUDGET,
        log_path: "brain/Session log.md".into(),
        sample_tasks: Vec::new(),
        passes_off: Vec::new(),
        lessons: true,
        snapshot_keep_days: None,
        log_notes: None,
    };
    (config, ws)
}

#[test]
fn ingest_reads_opencode_sessions_incrementally_without_leaks() {
    let home = tempfile::tempdir().unwrap();
    std::env::set_var("HIPPO_HOME", home.path());
    let workspace = tempfile::tempdir().unwrap();
    let data = home.path().join("share/opencode");
    let db = build(
        &data,
        &workspace.path().display().to_string(),
        &["schema.sql", "sessions.sql"],
    );
    // OpenCode's snapshots and cloned repositories are not walked: the
    // Claude Code adapter would claim this file.
    fs::create_dir_all(data.join("repos/r")).unwrap();
    fs::write(data.join("repos/r/notes.jsonl"), "{}\n").unwrap();
    // A database of another schema, in its own data folder.
    let future = build(
        &home.path().join("future/opencode"),
        "/home/synthetic/shop",
        &["newer.sql"],
    );
    let (config, ws) = setup(
        home.path(),
        workspace.path(),
        vec![
            data.display().to_string(),
            future.parent().unwrap().display().to_string(),
        ],
    );

    // Seven sessions; the future database is skipped. The three in the
    // workspace and their sub-agents get records; the two in
    // /home/synthetic/garden do not.
    let first = ingest(&config, Some(&ws), false).unwrap();
    assert_eq!(
        first,
        IngestSummary::Done {
            indexed: 7,
            skipped: 1,
            derived: 5,
            removed: 0
        }
    );

    let index = Index::open(&config.home, false).unwrap();
    let conn = Connection::open(config.home.join("index.sqlite")).unwrap();
    let rows: Vec<(String, String, String)> = conn
        .prepare("SELECT id, tool, transcript_path FROM sessions WHERE error IS NULL ORDER BY id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(rows.len(), 7);
    assert!(rows.iter().all(|(_, tool, _)| tool == "opencode"));
    let path_of = |id: &str| {
        rows.iter()
            .find(|(i, _, _)| i == id)
            .map(|(_, _, p)| p.clone())
            .unwrap()
    };
    assert_eq!(
        path_of(&hid(S1)),
        opencode::session_path(&db, S1).display().to_string()
    );
    assert_eq!(index.sub_agents(&hid(S1)).unwrap(), [sub(S2), sub(S6)]);
    let fork = index.turns_for_session(&hid(S3)).unwrap();
    assert_eq!(fork.len(), 2);
    assert!(!fork.iter().any(|t| t.text.contains("INHERITED")));

    // The future database is a parse failure that names its schema.
    let failed = index.failed_files().unwrap();
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].transcript_path, future.display().to_string());
    assert!(
        failed[0].error.contains("20991231000000_synthetic_future"),
        "{}",
        failed[0].error
    );

    // Recall finds a session by its words; no leaked value is indexed.
    let hits = index.recall("checkout total", 20, false).unwrap();
    assert!(hits.iter().any(|h| h.session == hid(S1)), "{hits:?}");
    let texts: Vec<String> = conn
        .prepare("SELECT text FROM turns")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let all = texts.join("\n");
    for leak in leaks() {
        assert!(
            index.recall(&leak, 200, false).unwrap().is_empty(),
            "{leak}"
        );
        assert!(!all.contains(&leak), "indexed text holds {leak:?}");
    }
    for kind in KINDS {
        assert!(
            all.contains(&format!("[redacted:{kind}]")),
            "no [redacted:{kind}]"
        );
    }
    assert!(all.contains("Exit code 1\nFAIL tests/checkout.test.ts"));
    assert!(all.contains("<tool_use_error>Error: Cannot write /etc/hosts"));
    assert!(!all.contains("notes.jsonl"));

    // Origins: the working directory, the parent and the title.
    let sources = index.sources().unwrap();
    let origin = |id: &str| {
        let (tool, path) = &sources[id];
        session_meta::origin(id, tool, path)
    };
    let o1 = origin(&hid(S1));
    assert_eq!(o1.cwd.as_deref(), Some(workspace.path()));
    assert_eq!(o1.parent, None);
    assert_eq!(o1.named.as_deref(), Some("Fix the failing checkout test"));
    let o2 = origin(&sub(S2));
    assert_eq!(o2.parent.as_deref(), Some(hid(S1).as_str()));
    assert_eq!(o2.description.as_deref(), Some("Check the cart totals"));
    assert_eq!(o2.named, None);
    let facts = session_meta::run_facts("opencode", &sources[&hid(S1)].1);
    assert_eq!(facts.model.as_deref(), Some("synthetic-coder-1"));
    assert_eq!(facts.effort.as_deref(), Some("high"));

    // A derived record's handoff is the last reply, redacted.
    let path = ws
        .hippo_dir()
        .join("sessions")
        .join(format!("{}.json", hid(S1)));
    let record = Session::load(&path).unwrap();
    assert!(record.derived && record.handoff_derived);
    let handoff = record.handoff.unwrap();
    assert!(handoff.starts_with("Fixed: the checkout total"));
    assert!(handoff.contains("[redacted:auth]"));
    for leak in leaks() {
        assert!(!handoff.contains(&leak), "handoff holds {leak:?}");
    }

    // Nothing changed: a second ingest reads no session.
    let second = ingest(&config, Some(&ws), false).unwrap();
    assert_eq!(
        second,
        IngestSummary::Done {
            indexed: 0,
            skipped: 8,
            derived: 0,
            removed: 0
        }
    );

    // OpenCode adds a message to one session: only that one is read again.
    let writer = Connection::open(&db).unwrap();
    writer
        .execute(
            "INSERT INTO session_message (id, session_id, type, seq, time_created, time_updated, data) \
             VALUES ('msg_more', ?1, 'user', 41, 1793527900000, 1793527900000, \
             '{\"text\":\"One more thing: the receipt rounding.\"}')",
            [S1],
        )
        .unwrap();
    writer
        .execute(
            "UPDATE event_sequence SET seq = 41 WHERE aggregate_id = ?1",
            [S1],
        )
        .unwrap();
    drop(writer);
    let third = ingest(&config, Some(&ws), false).unwrap();
    assert_eq!(
        third,
        IngestSummary::Done {
            indexed: 1,
            skipped: 7,
            derived: 1,
            removed: 0
        }
    );
    let index = Index::open(&config.home, false).unwrap();
    assert!(index
        .turns_for_session(&hid(S1))
        .unwrap()
        .iter()
        .any(|t| t.text.contains("receipt rounding")));
}

const S8: &str = "ses_8888synthetic0000000008";

#[test]
fn endings_tokens_and_attachments_reach_the_rollup_redacted() {
    use hippo_core::rollup::{Rollup, Stored};
    let home = tempfile::tempdir().unwrap();
    std::env::set_var("HIPPO_HOME", home.path());
    let workspace = tempfile::tempdir().unwrap();
    let data = home.path().join("share/opencode");
    build(
        &data,
        &workspace.path().display().to_string(),
        &["schema.sql", "sessions.sql", "endings.sql"],
    );
    let (config, ws) = setup(
        home.path(),
        workspace.path(),
        vec![data.display().to_string()],
    );
    ingest(&config, Some(&ws), false).unwrap();

    // The session ended on an API error: stored as interrupted, the
    // error's first line as the detail, as for Claude Code.
    let rollup = Rollup::open(&config.home).unwrap().unwrap();
    let facts = rollup.session(&hid(S8)).unwrap().unwrap();
    assert_eq!(facts.stored, Stored::Interrupted);
    assert_eq!(
        facts.ending_detail.as_deref(),
        Some("API Error: 503 provider.internal: Upstream connection reset.")
    );
    // Its work counts in the day the card and the overview read.
    let start = facts.first_turn.unwrap();
    let day = rollup
        .sessions_in(start, start + time::Duration::hours(1))
        .unwrap();
    let s8 = day.iter().find(|s| s.facts.id == hid(S8)).unwrap();
    assert!(s8.last.unwrap() > s8.first.unwrap());

    // The sub-agent run's tool calls reach Fleet's task line.
    let tasks = rollup.tasks_of(&hid(S1)).unwrap();
    let agent = tasks.iter().find(|t| t.tools.is_some()).unwrap();
    assert_eq!((agent.tokens, agent.tools), (None, Some(2)));

    // An attachment's name and path are indexed, redacted; its bytes are not.
    let index = Index::open(&config.home, false).unwrap();
    let turns = index.turns_for_session(&hid(S8)).unwrap();
    let first = &turns[0].text;
    assert!(
        first.contains("[Image: cart-broken.png, image/png, "),
        "{first}"
    );
    assert!(first.contains("[redacted:email]"), "{first}");
    assert!(!first.contains("fake.person@example.com"), "{first}");
    assert!(turns.iter().all(|t| !t.text.contains("base64")));
}
