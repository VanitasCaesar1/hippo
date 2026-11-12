//! Ingests the synthetic transcripts in `fixtures/` and searches them:
//! three sessions, every turn indexed, no
//! leak from `fixtures/redaction/leaks.txt` in the index or in a derived
//! record, and a second ingest that skips every file.

use std::fs;
use std::path::{Path, PathBuf};

use hippo_core::config::{Config, Workspace};
use hippo_core::index::Index;
use hippo_core::ingest::{ingest, IngestSummary};
use hippo_core::records::{IngestState, Session, Tool};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures");

const CLAUDE: &str = "stub-1";
const SUBAGENT: &str = "stub-1-agent-a1";
const CODEX: &str = "codex-01900000-c0de-7000-8000-000000000001";

/// The redaction kinds, in the order they are applied.
const KINDS: [&str; 10] = [
    "pem", "aws", "github", "provider", "jwt", "auth", "kv", "email", "aadhaar", "phone",
];

fn leaks() -> Vec<String> {
    let text = fs::read_to_string(Path::new(FIXTURES).join("redaction/leaks.txt")).unwrap();
    text.lines().map(str::to_owned).collect()
}

/// Copies the fixtures' Claude Code and Codex transcripts under `to`,
/// moving their working directory (`/work/example`) to `cwd`, so that their
/// sessions ran in that workspace and get records there.
fn fixtures_in(to: &Path, cwd: &Path) {
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
    let cwd = cwd.display().to_string();
    for tool in ["claude-code", "codex"] {
        copy(&Path::new(FIXTURES).join(tool), &to.join(tool), &cwd);
    }
}

/// A configuration whose only transcript roots are a copy of `fixtures/`
/// in the home, run in the workspace, with the hippo home and the
/// workspace in temporary directories.
fn setup(home: &Path, workspace: &Path) -> (Config, Workspace) {
    let copies = home.join("transcripts");
    fixtures_in(&copies, workspace);
    let config = Config {
        home: home.to_path_buf(),
        workspace: Some(workspace.to_path_buf()),
        // Not `fixtures/eval`, whose transcripts belong to the eval set.
        transcript_roots: vec![
            copies.join("claude-code").display().to_string(),
            copies.join("codex").display().to_string(),
        ],
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

fn record(ws: &Workspace, id: &str) -> Session {
    let path: PathBuf = ws.hippo_dir().join("sessions").join(format!("{id}.json"));
    Session::load(&path).unwrap()
}

#[test]
fn ingest_indexes_the_fixtures_and_recall_finds_them_without_leaks() {
    // Integration tests keep HIPPO_HOME in a temporary directory; ingest
    // itself uses the configuration's home.
    let home = tempfile::tempdir().unwrap();
    std::env::set_var("HIPPO_HOME", home.path());
    let workspace = tempfile::tempdir().unwrap();
    let (config, ws) = setup(home.path(), workspace.path());

    let first = ingest(&config, Some(&ws), false).unwrap();
    assert_eq!(
        first,
        IngestSummary::Done {
            indexed: 3,
            skipped: 0,
            derived: 3,
            removed: 0
        }
    );
    assert_eq!(
        first.to_string(),
        "indexed 3 files, skipped 0, derived 3 sessions"
    );

    // Three sessions, two Claude Code (one a nested sub-agent) and one
    // Codex, and 22 turns: 13 + 5 + 4.
    let index = Index::open(&config.home, false).unwrap();
    for (id, turns) in [(CLAUDE, 13), (SUBAGENT, 5), (CODEX, 4)] {
        assert_eq!(index.turns_for_session(id).unwrap().len(), turns, "{id}");
    }
    assert!(index.failed_files().unwrap().is_empty());
    let db = rusqlite::Connection::open(config.home.join("index.sqlite")).unwrap();
    let count = |sql: &str| db.query_row(sql, [], |row| row.get::<_, i64>(0)).unwrap();
    assert_eq!(count("SELECT COUNT(*) FROM sessions"), 3);
    assert_eq!(count("SELECT COUNT(*) FROM turns"), 22);
    let tools: Vec<(String, String)> = db
        .prepare("SELECT id, tool FROM sessions ORDER BY id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        tools,
        [
            (CODEX.to_owned(), Tool::Codex.as_str().to_owned()),
            (CLAUDE.to_owned(), Tool::ClaudeCode.as_str().to_owned()),
            (SUBAGENT.to_owned(), Tool::ClaudeCode.as_str().to_owned()),
        ]
    );

    // A phrase from the transcripts hits; a leaked value does not.
    let hits = index.recall("retry backoff", 20, false).unwrap();
    assert!(hits.iter().any(|hit| hit.session == CODEX), "{hits:?}");
    assert!(index
        .recall("AKIAFAKEFAKEFAKE0000", 20, false)
        .unwrap()
        .is_empty());

    // No leaks.txt line is found by recall or present in any indexed text,
    // and every kind of redaction marker is.
    let texts: Vec<String> = db
        .prepare("SELECT text FROM turns")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let all = texts.join("\n");
    for leak in leaks() {
        let found = index.recall(&leak, 200, false).unwrap();
        assert!(found.is_empty(), "recall found {leak:?}: {found:?}");
        assert!(!all.contains(&leak), "indexed text holds {leak:?}");
    }
    for kind in KINDS {
        assert!(
            all.contains(&format!("[redacted:{kind}]")),
            "no [redacted:{kind}]"
        );
    }

    // The derived records: the Codex handoff is its last assistant turn,
    // redacted like the index (it carries leak lines 11 and 12).
    for id in [CLAUDE, SUBAGENT] {
        let session = record(&ws, id);
        assert!(session.derived && session.handoff.is_none(), "{id}");
    }
    let codex = record(&ws, CODEX);
    assert!(codex.derived && codex.handoff_derived);
    assert!(codex.ended.is_some());
    let handoff = codex.handoff.unwrap();
    assert!(handoff.contains("[redacted:auth]") && handoff.contains("[redacted:kv]"));
    for leak in leaks() {
        assert!(!handoff.contains(&leak), "handoff holds {leak:?}");
    }

    let state = IngestState::load(&config.home.join("ingest-state.json")).unwrap();
    assert_eq!((state.files_seen, state.files_indexed), (3, 3));

    // Nothing changed, so a second ingest skips every file.
    let second = ingest(&config, Some(&ws), false).unwrap();
    assert_eq!(
        second,
        IngestSummary::Done {
            indexed: 0,
            skipped: 3,
            derived: 0,
            removed: 0
        }
    );
}
