//! `hippo config set` of a security setting and `hippo accept`, in scratch
//! homes (`docs/designs/common.md`, rule 3): a tightening applies at once,
//! a loosening waits and `doctor` names it, and `accept` refuses an agent
//! and a missing terminal.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const MARKERS: [&str; 12] = [
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CODEX_SANDBOX",
    "CODEX_THREAD_ID",
    "OPENCODE",
    "OPENCODE_SESSION_ID",
    "AI_AGENT",
    "AGENT",
    "GEMINI_CLI",
    "CURSOR_AGENT",
    "COPILOT_CLI",
    "JUNIE_DATA",
];

struct Setup {
    _dir: tempfile::TempDir,
    base: PathBuf,
    ws: PathBuf,
}

impl Setup {
    fn new() -> Setup {
        let dir = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(dir.path()).unwrap();
        let ws = base.join("ws");
        fs::create_dir_all(ws.join("notes")).unwrap();
        fs::write(ws.join("notes/START HERE.md"), "# Start here\n").unwrap();
        let s = Setup {
            _dir: dir,
            base,
            ws,
        };
        let ws = s.ws.display().to_string();
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
            &[],
        );
        assert!(out.status.success(), "{}", text(&out.stderr));
        s
    }

    fn hippo_toml(&self) -> PathBuf {
        self.ws.join(".hippo/hippo.toml")
    }

    /// Runs hippo with every home in the scratch folder and no agent
    /// marker but those in `env`.
    fn run(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_hippo"));
        cmd.args(args)
            .current_dir(&self.ws)
            .env("HOME", self.base.join("home"))
            .env("HIPPO_HOME", self.base.join("hippo"))
            .env("CODEX_HOME", self.base.join("codex"))
            .env("CLAUDE_CONFIG_DIR", self.base.join("claude"))
            .env("XDG_CONFIG_HOME", self.base.join("xdg/config"))
            .env("XDG_DATA_HOME", self.base.join("xdg/data"))
            .env_remove("RUST_LOG")
            .stdin(Stdio::null());
        for marker in MARKERS {
            cmd.env_remove(marker);
        }
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd.output().unwrap()
    }

    fn doctor_row(&self, name: &str) -> String {
        let out = self.run(&["doctor"], &[]);
        text(&out.stdout)
            .lines()
            .find(|l| l.starts_with(&format!("{name} |")))
            .unwrap_or_default()
            .to_owned()
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}

#[test]
fn init_writes_a_workspace_id() {
    let s = Setup::new();
    let toml = read(&s.hippo_toml());
    let line = toml
        .lines()
        .find(|l| l.starts_with("workspace_id = "))
        .expect("an id");
    assert_eq!(line.len(), "workspace_id = \"\"".len() + 26, "{line}");
}

#[test]
fn a_tightening_applies_at_once_and_raises_the_floor() {
    let s = Setup::new();
    let out = s.run(&["config", "set", "guard.fail", "closed"], &[]);
    let shown = text(&out.stdout);
    assert!(shown.contains("+ fail = \"closed\""), "{shown}");
    assert!(shown.contains("+ min_hippo_version = \"0.6.0\""), "{shown}");
    assert!(shown.contains("nothing written"), "{shown}");
    let out = s.run(&["config", "set", "guard.fail", "closed", "--yes"], &[]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(!text(&out.stdout).contains("loosening waiting"));
    let toml = read(&s.hippo_toml());
    assert!(toml.contains("[guard]\nfail = \"closed\"\n"), "{toml}");
    assert!(toml.contains("min_hippo_version = \"0.6.0\""), "{toml}");
    assert!(s.doctor_row("accept").contains("| ok |"));
    // Hand-edited back to open: closed stays in force and waits.
    fs::write(
        s.hippo_toml(),
        toml.replace("fail = \"closed\"", "fail = \"open\""),
    )
    .unwrap();
    let row = s.doctor_row("accept");
    assert!(row.contains("| warn |"), "{row}");
    assert!(
        row.contains("loosening waiting: [guard] fail: closed in force, open in the file"),
        "{row}"
    );
}

#[test]
fn a_loosening_waits_and_accept_needs_the_person() {
    let s = Setup::new();
    let out = s.run(&["config", "set", "team.inbox", "shared", "--yes"], &[]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(
        text(&out.stdout)
            .contains("loosening waiting: [team] inbox: local in force, shared in the file"),
        "{}",
        text(&out.stdout)
    );
    let toml = read(&s.hippo_toml());
    assert!(toml.contains("[team]\ninbox = \"shared\""), "{toml}");
    let list = s.run(&["accept", "--list"], &[]);
    assert!(list.status.success());
    assert!(text(&list.stdout).contains("[team] inbox"));
    // No terminal: refused, nothing accepted.
    let out = s.run(&["accept"], &[]);
    assert!(!out.status.success());
    assert!(
        text(&out.stderr).contains("needs a terminal"),
        "{}",
        text(&out.stderr)
    );
    // Each agent's marker: refused, naming the agent.
    for (var, agent) in [
        ("CLAUDECODE", "Claude Code"),
        ("CODEX_THREAD_ID", "Codex"),
        ("OPENCODE", "OpenCode"),
        ("GEMINI_CLI", "Gemini CLI"),
        ("CURSOR_AGENT", "Cursor"),
    ] {
        let out = s.run(&["accept"], &[(var, "1")]);
        assert!(!out.status.success());
        let err = text(&out.stderr);
        assert!(err.contains(var) && err.contains(agent), "{err}");
    }
    // There is no --yes.
    assert!(!s.run(&["accept", "--yes"], &[]).status.success());
    assert!(s.doctor_row("accept").contains("[team] inbox"));
}

#[test]
fn an_unknown_setting_in_a_known_table_refuses_writes() {
    let s = Setup::new();
    let toml = read(&s.hippo_toml());
    fs::write(
        s.hippo_toml(),
        format!("{toml}\n[team]\ndreamer = \"alice\"\n"),
    )
    .unwrap();
    let row = s.doctor_row("workspace settings");
    assert!(
        row.contains("| problem |") && row.contains("team.dreamer"),
        "{row}"
    );
}
