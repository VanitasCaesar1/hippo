//! `hippo setup`, the first run the plugins drive, in a scratch home with
//! synthetic history. After `--yes`, a stub agent starts a session the way
//! Claude Code and Codex do: it reads the hooks file setup wrote and runs
//! the session-start command with the hook's JSON on stdin. The session
//! must get the start block, with nothing else run by hand.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::Value;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures");
const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");

struct Machine {
    _dir: tempfile::TempDir,
    home: PathBuf,
    project: PathBuf,
}

/// A home with one synthetic Claude Code project and a Git project with no
/// notes yet.
fn machine() -> Machine {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().canonicalize().unwrap();
    let history = home.join(".claude/projects/-work-tern");
    fs::create_dir_all(&history).unwrap();
    fs::copy(
        format!("{FIXTURES}/claude-code/synthetic.jsonl"),
        history.join("synthetic.jsonl"),
    )
    .unwrap();
    let project = home.join("code/tern");
    fs::create_dir_all(project.join(".git")).unwrap();
    fs::create_dir_all(project.join("src")).unwrap();
    Machine {
        _dir: dir,
        home,
        project,
    }
}

fn env(cmd: &mut Command, home: &Path) {
    cmd.env_clear()
        .env("HOME", home)
        .env("HIPPO_HOME", home.join(".hippo"))
        .env("CODEX_HOME", home.join(".codex"))
        .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("PATH", "/usr/bin:/bin")
        .env("TZ", "UTC");
}

/// `hippo <args>` from `dir`.
fn hippo(m: &Machine, dir: &Path, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_hippo"));
    env(&mut cmd, &m.home);
    cmd.args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// The stub agent: runs `command` as the agent runs a hook (`sh -c`), in
/// `dir`, with `input` on stdin, and returns what it printed.
fn run_hook(m: &Machine, dir: &Path, command: &str, input: &Value) -> String {
    let mut cmd = Command::new("/bin/sh");
    env(&mut cmd, &m.home);
    let mut child = cmd
        .arg("-c")
        .arg(command)
        .current_dir(dir)
        .env("CLAUDE_PROJECT_DIR", dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.to_string().as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{}", text(&out));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// The session-start commands in a hooks file (`hooks.SessionStart[].hooks[].command`).
fn session_start(file: &Path) -> Vec<String> {
    let v: Value = serde_json::from_str(&fs::read_to_string(file).unwrap()).unwrap();
    v["hooks"]["SessionStart"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|g| g["hooks"].as_array().unwrap().iter())
        .map(|h| h["command"].as_str().unwrap().to_owned())
        .collect()
}

const ENTRY: &str = "The entry note: every new agent session in this workspace reads";

#[test]
fn without_yes_it_prints_the_steps_and_writes_nothing() {
    let m = machine();
    let out = hippo(&m, &m.project.join("src"), &["setup", "--agent", "codex"]);
    let said = text(&out);
    assert_eq!(out.status.code(), Some(0), "{said}");
    let root = m.project.display().to_string();
    assert!(
        said.contains(&format!(
            "hippo init --workspace {root} --notes-dir notes --entry-note 'notes/START HERE.md'"
        )),
        "{said}"
    );
    assert!(said.contains("hippo hooks install --agent codex"), "{said}");
    assert!(said.contains("hippo ingest"), "{said}");
    assert!(said.contains("It schedules nothing"), "{said}");
    assert!(!m.home.join(".hippo").exists());
    assert!(!m.home.join(".codex").exists());
    assert!(!m.project.join("notes").exists());
}

#[test]
fn codex_a_new_session_starts_with_the_block() {
    let m = machine();
    let out = hippo(&m, &m.project, &["setup", "--agent", "codex", "--yes"]);
    let said = text(&out);
    assert_eq!(out.status.code(), Some(0), "{said}");
    assert!(said.contains("Ready. Start a new Codex session"), "{said}");
    assert!(said.contains("trust them"), "{said}");
    assert!(said.contains("hippo schedule install"), "{said}");
    assert!(m.project.join("notes/START HERE.md").is_file());
    assert!(m.home.join(".hippo/index.sqlite").is_file(), "ingest ran");
    // Nothing was scheduled.
    assert!(!m.home.join("Library/LaunchAgents").exists());

    let commands = session_start(&m.home.join(".codex/hooks.json"));
    assert_eq!(commands.len(), 1, "{commands:?}");
    let input = serde_json::json!({
        "session_id": "019a0000-0000-7000-8000-000000000001",
        "hook_event_name": "SessionStart",
        "source": "startup",
        "cwd": m.project.join("src"),
        "model": "stub",
    });
    let printed = run_hook(&m, &m.project.join("src"), &commands[0], &input);
    assert!(printed.contains(ENTRY), "{printed}");

    // A second run changes nothing and says hippo is set up.
    let again = hippo(&m, &m.project, &["setup", "--agent", "codex", "--yes"]);
    assert_eq!(again.status.code(), Some(0), "{}", text(&again));
    assert!(!text(&again).contains("hippo init"), "{}", text(&again));
    assert_eq!(session_start(&m.home.join(".codex/hooks.json")).len(), 1);
}

#[test]
fn claude_code_a_new_session_starts_with_the_block() {
    let m = machine();
    let out = hippo(&m, &m.project, &["setup", "--yes"]);
    let said = text(&out);
    assert_eq!(out.status.code(), Some(0), "{said}");
    assert!(
        said.contains("Ready. Start a new Claude Code session"),
        "{said}"
    );
    let settings = m.project.join(".claude/settings.local.json");
    let commands = session_start(&settings);
    assert_eq!(commands.len(), 1, "{commands:?}");
    let input = serde_json::json!({
        "session_id": "11111111-2222-3333-4444-555555555555",
        "hook_event_name": "SessionStart",
        "source": "startup",
        "cwd": m.project,
    });
    let printed = run_hook(&m, &m.project, &commands[0], &input);
    assert!(printed.contains(ENTRY), "{printed}");

    // The plugin's own hook stays silent: the workspace's hook gives it.
    let plugin = session_start(&Path::new(ROOT).join("plugin/hooks/hooks.json"));
    let quiet = run_hook(&m, &m.project, &plugin[0], &input);
    assert!(quiet.trim().is_empty(), "{quiet}");
}

#[test]
fn another_workspace_is_named_not_replaced() {
    let m = machine();
    assert!(hippo(&m, &m.project, &["setup", "--yes"]).status.success());
    let other = m.home.join("code/other");
    fs::create_dir_all(other.join(".git")).unwrap();
    let out = hippo(&m, &other, &["setup", "--agent", "codex", "--yes"]);
    let said = text(&out);
    assert_eq!(out.status.code(), Some(1), "{said}");
    assert!(said.contains("hippo's workspace is"), "{said}");
    assert!(said.contains("--force"), "{said}");
    assert!(!other.join("notes").exists());
    assert!(!m.home.join(".codex/hooks.json").exists());
}
