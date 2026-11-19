//! Ingests the synthetic Gemini CLI sessions in `fixtures/gemini` and reads
//! them back: a JSON Lines session, its sub-agent and an older JSON one with
//! their turns, `logs.json` and the checkpoint unread, the working
//! directory from `.project_root`, no leak from
//! `fixtures/redaction/leaks.txt` in the index, and a second ingest that
//! skips every file.

use std::fs;
use std::path::Path;

use hippo_core::config::{Config, Workspace};
use hippo_core::index::Index;
use hippo_core::ingest::{ingest, IngestSummary};
use hippo_core::records::Session;
use hippo_core::session_meta;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures");
const MAIN: &str = "gemini-5e550000-0000-4000-8000-000000000001";
const SUB: &str =
    "gemini-5e550000-0000-4000-8000-000000000001-agent-5e550000-0000-4000-8000-000000000002";

const KINDS: [&str; 10] = [
    "pem", "aws", "github", "provider", "jwt", "auth", "kv", "email", "aadhaar", "phone",
];

fn leaks() -> Vec<String> {
    let text = fs::read_to_string(Path::new(FIXTURES).join("redaction/leaks.txt")).unwrap();
    text.lines().map(str::to_owned).collect()
}

/// Copies `from` to `to`, moving the session's directory (`/work/example`)
/// to `cwd`.
fn copy(from: &Path, to: &Path, cwd: &str) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy(&entry.path(), &target, cwd);
        } else {
            let text = fs::read_to_string(entry.path()).unwrap();
            fs::write(target, text.replace("/work/example", cwd)).unwrap();
        }
    }
}

#[test]
fn ingest_reads_gemini_sessions_without_leaks() {
    let home = tempfile::tempdir().unwrap();
    std::env::set_var("HIPPO_HOME", home.path());
    let workspace = tempfile::tempdir().unwrap();
    let workspace = fs::canonicalize(workspace.path()).unwrap();
    let base = home.path().join("g");
    copy(
        &Path::new(FIXTURES).join("gemini"),
        &base,
        &workspace.display().to_string(),
    );
    let root = base.join(".gemini/tmp");
    let config = Config {
        home: home.path().to_path_buf(),
        workspace: Some(workspace.clone()),
        transcript_roots: vec![root.display().to_string()],
        exclude_roots: Vec::new(),
        model_command: Vec::new(),
        ingest_interval_minutes: 10,
        dream_at: "03:30".into(),
        reviewer_command_override: None,
    };
    let ws = Workspace {
        root: workspace.clone(),
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
        min_hippo_version: None,
        unknown_keys: Vec::new(),
    };

    let first = ingest(&config, Some(&ws), false).unwrap();
    let IngestSummary::Done {
        indexed, skipped, ..
    } = first
    else {
        panic!("{first:?}");
    };
    assert_eq!((indexed, skipped), (3, 0), "{first:?}");
    let index = Index::open(&config.home, false).unwrap();
    let turns = index.turns_for_session(MAIN).unwrap();
    assert_eq!(turns.len(), 7);
    assert_eq!(turns[0].role.as_str(), "user");
    assert_eq!(turns[2].tool_name.as_deref(), Some("read_file"));
    assert_eq!(index.turns_for_session(SUB).unwrap().len(), 4);
    assert!(index.failed_files().unwrap().is_empty());
    let hits = index.recall("backoff", 20, false).unwrap();
    assert!(hits.iter().any(|h| h.session == MAIN), "{hits:?}");

    let all: String = turns
        .iter()
        .map(|t| t.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    for leak in leaks() {
        assert!(!all.contains(&leak), "indexed text holds {leak:?}");
    }
    for kind in KINDS {
        assert!(
            all.contains(&format!("[redacted:{kind}]")),
            "no [redacted:{kind}]"
        );
    }

    let sources = index.sources().unwrap();
    let (tool, path) = &sources[MAIN];
    assert_eq!(tool, "gemini");
    assert_eq!(
        session_meta::origin(MAIN, tool, path)
            .cwd
            .map(|p| fs::canonicalize(p).unwrap()),
        Some(workspace.clone())
    );

    let record =
        Session::load(&ws.hippo_dir().join("sessions").join(format!("{MAIN}.json"))).unwrap();
    assert!(record.derived && record.handoff_derived);
    assert_eq!(
        record.handoff.as_deref(),
        Some("The backoff now doubles up to 30 seconds.")
    );

    let again = ingest(&config, Some(&ws), false).unwrap();
    let IngestSummary::Done {
        indexed, skipped, ..
    } = again
    else {
        panic!("{again:?}");
    };
    assert_eq!((indexed, skipped), (0, 3), "{again:?}");
}
