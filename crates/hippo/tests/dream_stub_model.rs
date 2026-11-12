//! `hippo dream --pass verify` with
//! `fixtures/stub-model/model.sh` as the model and the reviewer, over the
//! synthetic transcripts in `fixtures/`, scenario by scenario.
//!
//! Each test has its own `HIPPO_HOME` and workspace, with `fixtures/notes/`
//! copied to `notes/`. Before the run, session `stub-1` is declared as
//! touching `notes/Topic A.md`, the note every stub proposal names, and
//! `last_success` is set before 1 November 2026, the transcripts' day, so
//! that the window covers them.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::Value;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures");
/// The note the stub's proposals change.
const TARGET: &str = "notes/Topic A.md";
/// The text of turn `stub-1` 1, which the stub's proposals quote as `new`.
const TURN_1: &str = "I'll ask a sub-agent where the sender sleeps between retries.";
/// `dream-state.json` with the window starting before the transcripts.
const STATE: &str = "{\"protocol\":\"HIPPO_V1\",\"last_success\":\"2026-11-01T00:00:00Z\"}\n";

/// A hippo home and a workspace, set up and ingested.
struct Stub {
    home: tempfile::TempDir,
    _dir: tempfile::TempDir,
    workspace: PathBuf,
}

impl Stub {
    fn new() -> Stub {
        let home = tempfile::tempdir().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let workspace = fs::canonicalize(dir.path()).unwrap();
        copy_dir(&Path::new(FIXTURES).join("notes"), &workspace.join("notes"));
        let stub = Stub {
            home,
            _dir: dir,
            workspace,
        };
        let workspace = stub.workspace.display().to_string();
        stub.ok(&[
            "init",
            "--workspace",
            &workspace,
            "--notes-dir",
            "notes",
            "--entry-note",
            "notes/START HERE.md",
        ]);
        let model = Path::new(FIXTURES).join("stub-model/model.sh");
        let model = model.display().to_string();
        let config = format!(
            "workspace = {workspace:?}\n\
             transcript_roots = [\"{FIXTURES}/claude-code\", \"{FIXTURES}/codex\"]\n\
             model_command = [{model:?}, \"--system-prompt\", \"{{system_prompt}}\"]\n\
             reviewer_command_override = [\"env\", \"HIPPO_STUB_REVIEW=1\", {model:?}, \
             \"--system-prompt\", \"{{system_prompt}}\"]\n"
        );
        fs::write(stub.home.path().join("config.toml"), config).unwrap();
        stub.ok(&[
            "session",
            "start",
            "--tool",
            "claude-code",
            "--session",
            "stub-1",
        ]);
        stub.ok(&["session", "end", "--session", "stub-1", "--note", TARGET]);
        let out = stub.ok(&["ingest"]);
        // The fixtures ran in another folder and name no note: indexed,
        // with no record in this workspace.
        assert_eq!(
            stdout(&out),
            "indexed 3 files, skipped 0, derived 0 sessions\n"
        );
        fs::write(stub.state_path(), STATE).unwrap();
        stub
    }

    /// Runs `hippo` with `args`, the stub's scenario and verdict in its
    /// environment.
    fn run(&self, args: &[&str], scenario: &str, verdict: &str) -> Output {
        Command::new(env!("CARGO_BIN_EXE_hippo"))
            .args(args)
            .env("HIPPO_HOME", self.home.path())
            .env("HIPPO_STUB_SCENARIO", scenario)
            .env("HIPPO_STUB_VERDICT", verdict)
            .env_remove("HIPPO_STUB_REVIEW")
            .env_remove("RUST_LOG")
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    /// Runs `hippo` with `args` and asserts it succeeded.
    fn ok(&self, args: &[&str]) -> Output {
        let out = self.run(args, "", "");
        assert!(out.status.success(), "hippo {args:?}: {}", stderr(&out));
        out
    }

    /// `hippo dream --pass verify` and `extra`, with the stub playing
    /// `scenario` and its reviewer saying `verdict`.
    fn dream(&self, extra: &[&str], scenario: &str, verdict: &str) -> Output {
        let mut args = vec!["dream", "--pass", "verify"];
        args.extend_from_slice(extra);
        self.run(&args, scenario, verdict)
    }

    fn note(&self) -> Vec<u8> {
        fs::read(self.workspace.join(TARGET)).unwrap()
    }

    fn state_path(&self) -> PathBuf {
        self.workspace.join(".hippo/dream-state.json")
    }

    /// The files in `.hippo/<dir>`, sorted.
    fn files(&self, dir: &str) -> Vec<PathBuf> {
        let Ok(entries) = fs::read_dir(self.workspace.join(".hippo").join(dir)) else {
            return Vec::new();
        };
        let mut files: Vec<PathBuf> = entries.map(|entry| entry.unwrap().path()).collect();
        files.sort();
        files
    }

    fn receipts(&self) -> Vec<Value> {
        self.files("receipts")
            .iter()
            .map(|path| serde_json::from_slice(&fs::read(path).unwrap()).unwrap())
            .collect()
    }

    /// The one dreaming report.
    fn report(&self) -> String {
        let reports = self.files("reports");
        assert_eq!(reports.len(), 1, "{reports:?}");
        fs::read_to_string(&reports[0]).unwrap()
    }
}

#[test]
fn a_confirmed_replace_is_applied_and_reverted() {
    let stub = Stub::new();
    let original = stub.note();
    let out = stub.dream(&[], "replace", "confirm");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out)
        .contains("1 proposals, 0 validation failures, 1 confirmed, 0 rejected, 1 applied"));

    assert_eq!(stub.files("proposals").len(), 1);
    let receipts = stub.receipts();
    assert_eq!(receipts.len(), 1);
    let receipt = &receipts[0];
    assert_eq!(receipt["verdict"], "confirm");
    assert_eq!(receipt["reviewer"], "verify-reviewer");
    assert!(receipt["applied"].is_string(), "{receipt}");
    let snapshot = stub.workspace.join(receipt["snapshot"].as_str().unwrap());
    assert_eq!(fs::read(&snapshot).unwrap(), original);
    assert!(stub.report().starts_with("status: ok\n"));
    let expected = String::from_utf8(original.clone())
        .unwrap()
        .replace("other notes link to", TURN_1);
    assert_eq!(String::from_utf8(stub.note()).unwrap(), expected);

    let id = receipt["id"].as_str().unwrap();
    let out = stub.ok(&["revert", id]);
    assert!(stdout(&out).starts_with(&format!("restored {TARGET} from ")));
    assert_eq!(stub.note(), original);
    let mut verdicts: Vec<String> = stub
        .receipts()
        .iter()
        .map(|receipt| receipt["verdict"].as_str().unwrap().to_owned())
        .collect();
    verdicts.sort();
    assert_eq!(verdicts, ["confirm", "revert"]);
}

#[test]
fn a_rejected_replace_gets_a_receipt_and_leaves_the_note() {
    let stub = Stub::new();
    let original = stub.note();
    let out = stub.dream(&[], "replace", "reject");
    assert!(out.status.success(), "{}", stderr(&out));
    let receipts = stub.receipts();
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0]["verdict"], "reject");
    assert!(receipts[0]["applied"].is_null() && receipts[0]["snapshot"].is_null());
    assert_eq!(stub.note(), original);
    assert!(stub.report().starts_with("status: ok\n"));
}

#[test]
fn a_note_outside_the_notes_or_an_old_that_occurs_twice_is_not_reviewed() {
    for (scenario, why) in [
        (
            "outside",
            "verify ../etc/passwd: not a note inside notes_dir",
        ),
        ("dup", "verify notes/Topic A.md: old text occurs 2 times"),
    ] {
        let stub = Stub::new();
        let original = stub.note();
        let out = stub.dream(&[], scenario, "confirm");
        assert!(out.status.success(), "{scenario}: {}", stderr(&out));
        assert!(stub.receipts().is_empty(), "{scenario}");
        assert!(stub.files("proposals").is_empty(), "{scenario}");
        let report = stub.report();
        let failures = report.split("## Validation failures\n\n").nth(1).unwrap();
        assert!(
            failures.starts_with(&format!("- {why}")),
            "{scenario}: {report}"
        );
        assert_eq!(stub.note(), original, "{scenario}");
    }
}

#[test]
fn an_unparsable_reply_fails_the_run_with_exit_code_2() {
    let stub = Stub::new();
    let out = stub.dream(&[], "unparsable", "confirm");
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(stderr(&out).contains("the reply has no JSON array"));
    assert!(stub.report().starts_with("status: failed\nerror: "));
    assert_eq!(fs::read_to_string(stub.state_path()).unwrap(), STATE);
}

#[test]
fn a_dry_run_reviews_and_writes_only_the_report() {
    let stub = Stub::new();
    let original = stub.note();
    let out = stub.dream(&["--dry-run"], "replace", "confirm");
    assert!(out.status.success(), "{}", stderr(&out));
    let report = stub.report();
    assert!(report.starts_with("status: ok\n"), "{report}");
    assert!(report.contains(" confirm: stub\n"), "{report}");
    assert!(stub.receipts().is_empty());
    assert!(stub.files("proposals").is_empty());
    assert!(stub.files("snapshots").is_empty());
    assert_eq!(stub.note(), original);
    // The file is normally absent; this test wrote it to open the window, and
    // the dry run leaves it as it was.
    assert_eq!(fs::read_to_string(stub.state_path()).unwrap(), STATE);
}

#[test]
fn revert_refuses_a_note_edited_since_unless_forced() {
    let stub = Stub::new();
    let original = stub.note();
    let out = stub.dream(&[], "replace", "confirm");
    assert!(out.status.success(), "{}", stderr(&out));
    let id = stub.receipts()[0]["id"].as_str().unwrap().to_owned();
    fs::write(stub.workspace.join(TARGET), "Edited by hand.\n").unwrap();
    let out = stub.run(&["revert", &id], "", "");
    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
    assert!(stderr(&out).contains("--force"), "{}", stderr(&out));
    assert_eq!(stub.note(), b"Edited by hand.\n");
    stub.ok(&["revert", "--force", &id]);
    assert_eq!(stub.note(), original);
}

#[test]
fn revert_run_twice_fails_naming_the_first_revert_receipt() {
    let stub = Stub::new();
    let original = stub.note();
    let out = stub.dream(&[], "replace", "confirm");
    assert!(out.status.success(), "{}", stderr(&out));
    let id = stub.receipts()[0]["id"].as_str().unwrap().to_owned();
    stub.ok(&["revert", &id]);
    let revert = stub
        .receipts()
        .into_iter()
        .find(|receipt| receipt["verdict"] == "revert")
        .unwrap();
    let revert_id = revert["id"].as_str().unwrap().to_owned();
    for args in [vec!["revert", &id], vec!["revert", "--force", &id]] {
        let out = stub.run(&args, "", "");
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        assert!(
            stderr(&out).contains(&format!("receipt {id} was already reverted by {revert_id}")),
            "{}",
            stderr(&out)
        );
        assert_eq!(stub.note(), original);
        assert_eq!(stub.receipts().len(), 2);
    }
}

#[test]
fn reverting_the_older_of_two_changes_to_one_note_fails_naming_the_newer() {
    let stub = Stub::new();
    let out = stub.dream(&[], "replace", "confirm");
    assert!(out.status.success(), "{}", stderr(&out));
    let older = stub.receipts().remove(0);
    let older_id = older["id"].as_str().unwrap().to_owned();
    let proposal_path = |id: &str| stub.workspace.join(format!(".hippo/proposals/{id}.json"));
    let receipt_path = |id: &str| stub.workspace.join(format!(".hippo/receipts/{id}.json"));

    // A later applied change to the same note: its own proposal and a
    // confirm receipt applied after the first.
    let newer_proposal = "01JC9X4Q3A8M2K7N5P6R8S9T0V";
    let newer_receipt = "01JC9X4Q3A8M2K7N5P6R8S9T0W";
    let mut proposal: Value = serde_json::from_slice(
        &fs::read(proposal_path(older["proposal"].as_str().unwrap())).unwrap(),
    )
    .unwrap();
    proposal["id"] = newer_proposal.into();
    fs::write(
        proposal_path(newer_proposal),
        serde_json::to_vec(&proposal).unwrap(),
    )
    .unwrap();
    let mut receipt = older.clone();
    receipt["id"] = newer_receipt.into();
    receipt["proposal"] = newer_proposal.into();
    receipt["applied"] = "2999-01-01T00:00:00Z".into();
    fs::write(
        receipt_path(newer_receipt),
        serde_json::to_vec(&receipt).unwrap(),
    )
    .unwrap();

    let before = stub.note();
    for args in [
        vec!["revert", &older_id],
        vec!["revert", "--force", &older_id],
    ] {
        let out = stub.run(&args, "", "");
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        assert!(
            stderr(&out).contains(&format!(
                "revert {newer_receipt} first: it changed {TARGET} later"
            )),
            "{}",
            stderr(&out)
        );
        assert_eq!(stub.note(), before);
    }
}

/// Copies the directory `from` to `to`, byte for byte.
fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}
