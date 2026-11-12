//! Test workspaces, built with hippocamp-core's public functions and `std::fs`.
// Tests build temporary workspaces, so they write; the code that ships does not.
#![allow(clippy::disallowed_methods, dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

use hippocamp_core::config::{self, InitOptions};
use tempfile::TempDir;

/// A hippocamp home and a workspace, both in a temporary directory.
pub struct Env {
    pub dir: TempDir,
    pub home: PathBuf,
    pub workspace: PathBuf,
}

/// `config::init_in` for a fresh home and workspace (notes in `brain/`).
pub fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let workspace = dir.path().join("work");
    fs::create_dir_all(workspace.join("brain")).unwrap();
    config::init_in(
        &home,
        InitOptions {
            workspace: Some(workspace.clone()),
            notes_dir: Some("brain".into()),
            entry_note: Some("brain/index.md".into()),
            add_roots: Vec::new(),
            force: false,
        },
    )
    .unwrap();
    let workspace = fs::canonicalize(workspace).unwrap();
    Env {
        dir,
        home,
        workspace,
    }
}

/// Every file's path, size and modification time, and every directory's
/// path, under `root`, sorted: a snapshot to compare before and after a read.
/// A directory's own modification time is left out: SQLite makes and removes
/// its `-wal` and `-shm` files in the home when a reader opens the index,
/// which touches the directory but no file that stays.
pub fn snapshot(root: &Path) -> Vec<(PathBuf, u64, std::time::SystemTime)> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let meta = fs::symlink_metadata(&path).unwrap();
            if meta.is_dir() {
                out.push((path.clone(), 0, std::time::UNIX_EPOCH));
            } else {
                out.push((path.clone(), meta.len(), meta.modified().unwrap()));
            }
            if meta.is_dir() {
                stack.push(path);
            }
        }
    }
    out.sort();
    out
}

use hippocamp_core::adapters::Role;
use hippocamp_core::config::Workspace;
use hippocamp_core::index::{Index, SessionSource, Turn};
use hippocamp_core::records::{Evidence, Proposal, ProposalKind, Receipt, Session, Tool, Verdict};
use hippocamp_core::redact::redact;
use time::OffsetDateTime;
use ulid::Ulid;

/// The loaded workspace of `env`.
pub fn workspace(env: &Env) -> Workspace {
    Workspace::load(&hippocamp_core::config::Config::load_in(&env.home).unwrap()).unwrap()
}

/// Writes a note at `path` (relative to the workspace) and returns it.
pub fn note(env: &Env, path: &str, text: &str) -> PathBuf {
    let file = env.workspace.join(path);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, text).unwrap();
    PathBuf::from(path)
}

/// Saves a session record at `.hippocamp/sessions/<id>.json`.
pub fn session(ws: &Workspace, id: &str, task: &str, started: OffsetDateTime) -> Session {
    let record = Session {
        id: id.to_owned(),
        tool: Tool::ClaudeCode,
        task: task.to_owned(),
        task_seen: true,
        context_bytes_at_start: 0,
        started,
        ended: None,
        notes_touched: Vec::new(),
        handoff: None,
        handoff_derived: false,
        no_change_reason: None,
        context_start: None,
        context_prompt: None,
        derived: false,
    };
    record
        .save(
            &ws.hippocamp_dir()
                .join("sessions")
                .join(format!("{id}.json")),
        )
        .unwrap();
    record
}

/// Saves a proposal at `.hippocamp/proposals/<id>.json`.
pub fn proposal(
    ws: &Workspace,
    id: Ulid,
    note: &str,
    old: &str,
    new: &str,
    evidence: &[(&str, u32)],
) -> Proposal {
    let record = Proposal {
        id,
        pass: "verify".to_owned(),
        note: PathBuf::from(note),
        kind: ProposalKind::Replace,
        before_hash: hippocamp_core::hash(old.as_bytes()),
        after_hash: hippocamp_core::hash(new.as_bytes()),
        old: old.to_owned(),
        new: new.to_owned(),
        evidence: evidence
            .iter()
            .map(|(session, ordinal)| Evidence {
                session: (*session).to_owned(),
                ordinal: *ordinal,
            })
            .collect(),
        reason: "because".to_owned(),
    };
    record
        .save(
            &ws.hippocamp_dir()
                .join("proposals")
                .join(format!("{id}.json")),
        )
        .unwrap();
    record
}

/// Saves a receipt at `.hippocamp/receipts/<id>.json`. `applied` gives the
/// before and after hashes and the snapshot of an applied confirm receipt.
pub fn receipt(
    ws: &Workspace,
    id: Ulid,
    proposal: Ulid,
    verdict: Verdict,
    applied: Option<(&str, &str, PathBuf)>,
) -> Receipt {
    let at = OffsetDateTime::from_unix_timestamp(1_794_000_000).unwrap();
    let (before_hash, after_hash, snapshot, applied) = match applied {
        Some((before, after, snapshot)) => (
            before.to_owned(),
            after.to_owned(),
            Some(snapshot),
            Some(at),
        ),
        None => (String::new(), String::new(), None, None),
    };
    let record = Receipt {
        id,
        proposal,
        proposal_hash: String::new(),
        before_hash,
        after_hash,
        reviewer: "reviewer".to_owned(),
        invocation: "model".to_owned(),
        verdict,
        notes: "looks right".to_owned(),
        at,
        applied,
        snapshot,
    };
    record
        .save(
            &ws.hippocamp_dir()
                .join("receipts")
                .join(format!("{id}.json")),
        )
        .unwrap();
    record
}

/// Writes `.hippocamp/reports/<date>-dream.md` naming `proposals`.
pub fn report(ws: &Workspace, date: &str, proposals: &[Ulid]) {
    let dir = ws.hippocamp_dir().join("reports");
    fs::create_dir_all(&dir).unwrap();
    let mut text = String::from("status: ok\n\n## Proposals\n\n");
    for id in proposals {
        text.push_str(&format!("- {id} brain/a.md\n"));
    }
    text.push_str("\n## Applied\n");
    fs::write(dir.join(format!("{date}-dream.md")), text).unwrap();
}

/// A ULID made at `unix_ms`, with `n` as its random part.
pub fn ulid_at(unix_ms: u64, n: u128) -> Ulid {
    Ulid::from_parts(unix_ms, n)
}

/// An index in `home` holding `turns` (role, text) for session `id`.
pub fn index_with(home: &Path, id: &str, turns: &[(Role, &str)]) -> Index {
    let mut index = Index::open(home, true).unwrap();
    let ts = OffsetDateTime::from_unix_timestamp(1_794_000_000).unwrap();
    let turns: Vec<Turn> = turns
        .iter()
        .enumerate()
        .map(|(ordinal, (role, text))| Turn {
            ordinal: ordinal as u32,
            role: *role,
            ts,
            tool_name: (*role == Role::Tool).then(|| "Bash".to_owned()),
            text: redact(text),
        })
        .collect();
    let path = PathBuf::from(format!("/transcripts/{id}.jsonl"));
    index
        .replace_session(
            &SessionSource {
                id,
                tool: Tool::ClaudeCode,
                transcript_path: &path,
                file_hash: &hippocamp_core::hash(id.as_bytes()),
            },
            &turns,
        )
        .unwrap();
    index
}
