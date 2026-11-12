//! Ingests the synthetic Muse Code log in `fixtures/muse` and reads it
//! back: one session with its turns, the sub-agent log left unread, the
//! working directory from the log, no leak from
//! `fixtures/redaction/leaks.txt` in the index or the derived record, and a
//! second ingest that skips the file.

use std::fs;
use std::path::Path;

use hippo_core::config::{Config, Workspace};
use hippo_core::index::Index;
use hippo_core::ingest::{ingest, IngestSummary};
use hippo_core::records::Session;
use hippo_core::session_meta;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures");
const S1: &str = "muse-01a1d0f0-0000-7000-8000-000000000001";

const KINDS: [&str; 10] = [
    "pem", "aws", "github", "provider", "jwt", "auth", "kv", "email", "aadhaar", "phone",
];

fn leaks() -> Vec<String> {
    let text = fs::read_to_string(Path::new(FIXTURES).join("redaction/leaks.txt")).unwrap();
    text.lines().map(str::to_owned).collect()
}

/// Copies `fixtures/muse` to `to`, moving the session's working directory
/// (`/work/example`) to `cwd`.
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
fn ingest_reads_a_muse_log_into_a_session_without_leaks() {
    let home = tempfile::tempdir().unwrap();
    std::env::set_var("HIPPO_HOME", home.path());
    let workspace = tempfile::tempdir().unwrap();
    let workspace = fs::canonicalize(workspace.path()).unwrap();
    let root = home.path().join("muse/sessions");
    copy(
        &Path::new(FIXTURES).join("muse"),
        &root,
        &workspace.display().to_string(),
    );
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
    };

    let first = ingest(&config, Some(&ws), false).unwrap();
    assert_eq!(
        first,
        IngestSummary::Done {
            indexed: 1,
            skipped: 0,
            derived: 1,
            removed: 0
        }
    );
    let index = Index::open(&config.home, false).unwrap();
    let turns = index.turns_for_session(S1).unwrap();
    assert_eq!(turns.len(), 6);
    assert_eq!(turns[0].role.as_str(), "user");
    assert_eq!(turns[1].tool_name.as_deref(), Some("read_file"));
    let hits = index.recall("backoff", 20, false).unwrap();
    assert!(hits.iter().any(|h| h.session == S1), "{hits:?}");

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
    let (tool, path) = &sources[S1];
    assert_eq!(tool, "muse");
    assert_eq!(
        session_meta::origin(S1, tool, path).cwd.as_deref(),
        Some(workspace.as_path())
    );

    let record =
        Session::load(&ws.hippo_dir().join("sessions").join(format!("{S1}.json"))).unwrap();
    assert!(record.derived && record.handoff_derived);
    assert!(record
        .handoff
        .unwrap()
        .starts_with("The backoff starts at 10 ms."));

    let again = ingest(&config, Some(&ws), false).unwrap();
    assert_eq!(
        again,
        IngestSummary::Done {
            indexed: 0,
            skipped: 1,
            derived: 0,
            removed: 0
        }
    );
}
