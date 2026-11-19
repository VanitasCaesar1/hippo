//! `hippo hook session-start --agent cursor` and `hippo hooks install
//! --agent cursor`, with `HIPPO_HOME` and `HOME` in temporary directories.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::Value;

struct Setup {
    home: tempfile::TempDir,
    user_home: tempfile::TempDir,
    _dir: tempfile::TempDir,
    workspace: PathBuf,
}

impl Setup {
    fn new() -> Setup {
        let home = tempfile::tempdir().unwrap();
        let user_home = tempfile::tempdir().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let workspace = fs::canonicalize(dir.path()).unwrap();
        fs::create_dir_all(workspace.join("notes")).unwrap();
        fs::write(
            workspace.join("notes/START HERE.md"),
            "# Start here\n\nRead the protocol first.\n",
        )
        .unwrap();
        let s = Setup {
            home,
            user_home,
            _dir: dir,
            workspace,
        };
        let ws = s.workspace.display().to_string();
        let out = s.run(
            &[
                "init",
                "--workspace",
                &ws,
                "--notes-dir",
                "notes",
                "--entry-note",
                "notes/START HERE.md",
            ],
            None,
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        s
    }

    fn run(&self, args: &[&str], stdin: Option<&str>) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_hippo"))
            .args(args)
            .env("HIPPO_HOME", self.home.path())
            .env("HOME", self.user_home.path())
            .env_remove("RUST_LOG")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        if let Some(text) = stdin {
            input.write_all(text.as_bytes()).unwrap();
        }
        drop(input);
        child.wait_with_output().unwrap()
    }

    fn start(&self, roots: &[&Path]) -> String {
        let input = serde_json::json!({
            "conversation_id": "c1",
            "generation_id": "g1",
            "session_id": "c1",
            "hook_event_name": "sessionStart",
            "workspace_roots": roots.iter().map(|r| r.display().to_string()).collect::<Vec<_>>(),
            "transcript_path": null,
        });
        let out = self.run(
            &["hook", "session-start", "--agent", "cursor"],
            Some(&input.to_string()),
        );
        assert!(out.status.success());
        assert!(
            out.stderr.is_empty(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }
}

#[test]
fn the_start_block_goes_only_to_a_session_in_the_workspace() {
    let s = Setup::new();
    let sub = s.workspace.join("app");
    fs::create_dir_all(&sub).unwrap();
    let out = s.start(&[&sub]);
    let value: Value = serde_json::from_str(out.trim()).unwrap();
    assert!(value["additional_context"]
        .as_str()
        .unwrap()
        .contains("Read the protocol first."));
    let elsewhere = tempfile::tempdir().unwrap();
    assert_eq!(s.start(&[elsewhere.path()]), "");
    assert_eq!(s.start(&[]), "");
    // Read-only: no session record.
    assert!(
        !s.workspace.join(".hippo/sessions").exists()
            || fs::read_dir(s.workspace.join(".hippo/sessions"))
                .unwrap()
                .count()
                == 0
    );
}

#[test]
fn install_writes_cursors_hooks_file_in_home() {
    let s = Setup::new();
    let out = s.run(&["hooks", "install", "--agent", "cursor"], None);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let path = s.user_home.path().join(".cursor/hooks.json");
    let value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(value["version"], 1);
    assert_eq!(
        value["hooks"]["sessionStart"][0]["command"],
        "hippo hook session-start --agent cursor"
    );
    let status = s.run(&["hooks", "status", "--agent", "cursor"], None);
    assert_eq!(
        String::from_utf8_lossy(&status.stdout),
        "sessionStart: installed\n"
    );
    let out = s.run(&["hooks", "uninstall", "--agent", "cursor"], None);
    assert!(out.status.success());
    let status = s.run(&["hooks", "status", "--agent", "cursor"], None);
    assert_eq!(
        String::from_utf8_lossy(&status.stdout),
        "sessionStart: missing\n"
    );
}
