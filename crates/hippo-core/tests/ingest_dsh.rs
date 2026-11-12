//! Ingests the synthetic dsh logs in `fixtures/dsh` and reads them back:
//! the newest generation of each session, sub-agents under their session,
//! a fork without its inherited history, the unread format version as a
//! parse failure, no leak from `fixtures/redaction/leaks.txt` in the
//! index or a derived record, and a second ingest that skips every file.

use std::fs;
use std::path::Path;

use hippo_core::config::{Config, Workspace};
use hippo_core::index::Index;
use hippo_core::ingest::{ingest, IngestSummary};
use hippo_core::records::Session;
use hippo_core::session_meta;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures");

const S1: &str = "dsh-11111111-1111-4111-8111-111111111111";
const S2: &str =
    "dsh-11111111-1111-4111-8111-111111111111-agent-22222222-2222-4222-8222-222222222222";
const S3: &str =
    "dsh-11111111-1111-4111-8111-111111111111-agent-33333333-3333-4333-8333-333333333333";
const S4: &str = "dsh-44444444-4444-4444-8444-444444444444";
const S6: &str =
    "dsh-11111111-1111-4111-8111-111111111111-agent-66666666-6666-4666-8666-666666666666";

/// The redaction kinds, in the order they are applied.
const KINDS: [&str; 10] = [
    "pem", "aws", "github", "provider", "jwt", "auth", "kv", "email", "aadhaar", "phone",
];

fn leaks() -> Vec<String> {
    let text = fs::read_to_string(Path::new(FIXTURES).join("redaction/leaks.txt")).unwrap();
    text.lines().map(str::to_owned).collect()
}

/// Copies `fixtures/dsh` to `to`, moving the plain logs' working directory
/// (`/home/synthetic/shop`) to `cwd`; the compressed fork keeps its own.
fn copy(from: &Path, to: &Path, cwd: &str) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy(&entry.path(), &target, cwd);
        } else if entry.path().extension().is_some_and(|e| e == "zstd") {
            fs::copy(entry.path(), target).unwrap();
        } else {
            let text = fs::read_to_string(entry.path()).unwrap();
            fs::write(target, text.replace("/home/synthetic/shop", cwd)).unwrap();
        }
    }
}

fn setup(home: &Path, workspace: &Path) -> (Config, Workspace) {
    let root = home.join("dsh-sessions");
    copy(
        &Path::new(FIXTURES).join("dsh"),
        &root,
        &workspace.display().to_string(),
    );
    let config = Config {
        home: home.to_path_buf(),
        workspace: Some(workspace.to_path_buf()),
        transcript_roots: vec![root.display().to_string()],
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
fn ingest_reads_dsh_logs_into_sessions_and_sub_agents_without_leaks() {
    let home = tempfile::tempdir().unwrap();
    std::env::set_var("HIPPO_HOME", home.path());
    let workspace = tempfile::tempdir().unwrap();
    let (config, ws) = setup(home.path(), workspace.path());

    // Five sessions; the v9 log is a parse failure, counted as skipped.
    // The fork ran elsewhere, so it has no derived record here.
    let first = ingest(&config, Some(&ws), false).unwrap();
    assert_eq!(
        first,
        IngestSummary::Done {
            indexed: 5,
            skipped: 1,
            derived: 4,
            removed: 0
        }
    );

    let index = Index::open(&config.home, false).unwrap();
    let db = rusqlite::Connection::open(config.home.join("index.sqlite")).unwrap();
    let rows: Vec<(String, String)> = db
        .prepare("SELECT id, tool FROM sessions WHERE error IS NULL ORDER BY id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let ids: Vec<&str> = rows.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(ids, [S1, S2, S3, S6, S4]);
    assert!(rows.iter().all(|(_, tool)| tool == "dsh"));

    // The newest generation only: the v3 log beside session 1 is not read.
    let s1 = index.turns_for_session(S1).unwrap();
    assert!(!s1.iter().any(|t| t.text.contains("OLD GENERATION")));
    assert_eq!(s1.first().map(|t| t.role.as_str()), Some("user"));
    // A fork holds its own turns, not the history it inherited.
    let fork = index.turns_for_session(S3).unwrap();
    assert_eq!(fork.len(), 2);
    assert!(!fork.iter().any(|t| t.text.contains("INHERITED")));
    // Sub-agents, a sub-agent's sub-agent included, are under session 1.
    assert_eq!(index.sub_agents(S1).unwrap(), [S2, S3, S6]);

    // The unread format is a parse failure that names its version.
    let failed = index.failed_files().unwrap();
    assert_eq!(failed.len(), 1);
    assert!(failed[0].transcript_path.ends_with("session.v9.jsonl"));
    assert!(
        failed[0]
            .error
            .contains("dsh log format v9 is not one hippo reads"),
        "{}",
        failed[0].error
    );

    // Recall finds the session by its words; a leaked value is not there.
    let hits = index.recall("checkout total", 20, false).unwrap();
    assert!(hits.iter().any(|h| h.session == S1), "{hits:?}");
    let texts: Vec<String> = db
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
    // The failed check reads as a failure the way hippo's readers know it.
    assert!(all.contains("Exit code 1\nFAIL tests/checkout.test.ts"));
    assert!(all.contains("<tool_use_error>Error: Cannot write /etc/hosts"));

    // Origins: the working directory, the parent and the title.
    let sources = index.sources().unwrap();
    let origin = |id: &str| {
        let (tool, path) = &sources[id];
        session_meta::origin(id, tool, path)
    };
    let o1 = origin(S1);
    assert_eq!(o1.cwd.as_deref(), Some(workspace.path()));
    assert_eq!(o1.parent, None);
    assert_eq!(o1.named.as_deref(), Some("Fix the failing checkout test"));
    let o2 = origin(S2);
    assert_eq!(o2.parent.as_deref(), Some(S1));
    assert_eq!(o2.description.as_deref(), Some("Check the cart totals"));
    assert_eq!(origin(S6).parent.as_deref(), Some(S1));
    let facts = session_meta::run_facts("dsh", &sources[S1].1);
    assert_eq!(facts.model.as_deref(), Some("deepseek-v4-pro"));
    assert_eq!(facts.effort.as_deref(), Some("high"));

    // A derived record's handoff is the last reply, redacted.
    let path = ws.hippo_dir().join("sessions").join(format!("{S1}.json"));
    let record = Session::load(&path).unwrap();
    assert!(record.derived && record.handoff_derived);
    let handoff = record.handoff.unwrap();
    assert!(handoff.starts_with("Fixed: the checkout total"));
    assert!(handoff.contains("[redacted:auth]"));
    for leak in leaks() {
        assert!(!handoff.contains(&leak), "handoff holds {leak:?}");
    }

    // Nothing changed: a second ingest skips every file.
    let second = ingest(&config, Some(&ws), false).unwrap();
    assert_eq!(
        second,
        IngestSummary::Done {
            indexed: 0,
            skipped: 6,
            derived: 0,
            removed: 0
        }
    );
}
