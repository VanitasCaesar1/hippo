//! `hippo mcp` as a client drives it: one JSON-RPC conversation over stdio
//! (initialize, tools/list, then tools/call for every tool) against a
//! synthetic workspace and an index of `fixtures/`, with every home in a
//! temporary directory. Each read tool's text is checked against the CLI
//! command it stands for, and the returned text against redaction.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::{json, Value};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures");

/// A fake secret from `fixtures/redaction/leaks.txt` (`DB_PASSWORD=…`), put
/// in a note; it must never leave the server.
fn secret() -> String {
    let leaks = fs::read_to_string(Path::new(FIXTURES).join("redaction/leaks.txt")).unwrap();
    leaks
        .lines()
        .find_map(|l| l.strip_prefix("kv DB_PASSWORD="))
        .map(|v| format!("DB_PASSWORD={v}"))
        .unwrap()
}

/// `hippo ask`'s text without the seconds the search took, which differ
/// between two runs.
fn steady(text: &str) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    if let Some(first) = lines.iter_mut().find(|l| l.starts_with("searched ")) {
        if let (Some(a), Some(b)) = (first.find(" in "), first.find(" s for: ")) {
            first.replace_range(a..b + 2, " in _ s");
        }
    }
    lines.join("\n")
}

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
        copy_dir(&Path::new(FIXTURES).join("notes"), &workspace.join("notes"));
        fs::write(
            workspace.join("notes/Deploy keys.md"),
            format!(
                "# Deploy keys\n\nThe staging deploy uses {} from the vault.\n",
                secret()
            ),
        )
        .unwrap();
        let setup = Setup {
            home,
            user_home,
            _dir: dir,
            workspace,
        };
        let ws = setup.workspace.display().to_string();
        setup.ok(&[
            "init",
            "--workspace",
            &ws,
            "--notes-dir",
            "notes",
            "--entry-note",
            "notes/START HERE.md",
        ]);
        let model = Path::new(FIXTURES).join("stub-model/model.sh");
        let model = model.display().to_string();
        fs::write(
            setup.home.path().join("config.toml"),
            format!(
                "workspace = {ws:?}\n\
                 transcript_roots = [\"{FIXTURES}/claude-code\", \"{FIXTURES}/codex\"]\n\
                 model_command = [{model:?}, \"--system-prompt\", \"{{system_prompt}}\"]\n\
                 reviewer_command_override = [\"env\", \"HIPPO_STUB_REVIEW=1\", {model:?}, \
                 \"--system-prompt\", \"{{system_prompt}}\"]\n"
            ),
        )
        .unwrap();
        setup.ok(&["ingest"]);
        setup
    }

    fn command(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_hippo"));
        c.env("HIPPO_HOME", self.home.path())
            .env("HOME", self.user_home.path())
            .env("XDG_CONFIG_HOME", self.user_home.path().join(".config"))
            .env("HIPPO_STUB_SCENARIO", "lessons")
            .env_remove("HIPPO_STUB_REVIEW")
            .env_remove("CODEX_HOME")
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("COPILOT_HOME")
            .env_remove("RUST_LOG")
            .current_dir(&self.workspace);
        c
    }

    fn ok(&self, args: &[&str]) -> Output {
        let out = self
            .command()
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "hippo {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }

    fn stdout(&self, args: &[&str]) -> String {
        String::from_utf8(self.ok(args).stdout).unwrap()
    }

    /// Sends `messages` to `hippo mcp` with `extra` arguments, one per line,
    /// closes stdin and returns the replies by id.
    fn converse(&self, extra: &[&str], messages: &[Value]) -> Vec<Value> {
        let mut child = self
            .command()
            .arg("mcp")
            .args(extra)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        for m in messages {
            writeln!(stdin, "{m}").unwrap();
        }
        drop(stdin);
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("{e}: {l}")))
            .collect()
    }
}

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

fn request(id: u64, method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

fn tool(id: u64, name: &str, arguments: Value) -> Value {
    request(
        id,
        "tools/call",
        json!({ "name": name, "arguments": arguments }),
    )
}

fn reply(replies: &[Value], id: u64) -> &Value {
    replies
        .iter()
        .find(|r| r["id"] == id)
        .unwrap_or_else(|| panic!("no reply {id} in {replies:?}"))
}

/// A tool result's text, asserting it is not an error.
fn text(replies: &[Value], id: u64) -> String {
    let r = &reply(replies, id)["result"];
    assert_eq!(r["isError"], false, "{r}");
    assert_eq!(r["content"][0]["type"], "text", "{r}");
    r["content"][0]["text"].as_str().unwrap().to_owned()
}

#[test]
fn a_client_conversation_covers_every_tool() {
    let s = Setup::new();
    let ws = s.workspace.display().to_string();
    let replies = s.converse(
        &["--allow-write"],
        &[
            request(
                1,
                "initialize",
                json!({
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": { "name": "test-client", "version": "1.0" }
                }),
            ),
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
            request(2, "tools/list", json!({})),
            tool(
                3,
                "hippo_context",
                json!({ "task": "fix the retry backoff", "cwd": ws }),
            ),
            tool(
                4,
                "hippo_recall",
                json!({ "query": "retry backoff", "limit": 5 }),
            ),
            tool(
                5,
                "hippo_ask",
                json!({ "question": "how was the webhook retry backoff fixed?" }),
            ),
            tool(6, "hippo_day", json!({ "date": "2026-11-01" })),
            tool(7, "hippo_incidents", json!({ "days": 14 })),
            tool(
                8,
                "hippo_context",
                json!({ "task": "the staging deploy keys" }),
            ),
            tool(9, "hippo_recall", json!({})),
            tool(
                10,
                "hippo_note_correction",
                json!({
                    "said": "no, previews run against staging, never production",
                    "rule": "Run previews against the staging database, never production.",
                    "title": "Previews against staging only"
                }),
            ),
            request(11, "ping", json!({})),
        ],
    );
    // One reply per request; none for the notification.
    assert_eq!(replies.len(), 11, "{replies:?}");

    let init = &reply(&replies, 1)["result"];
    assert_eq!(init["protocolVersion"], "2025-06-18");
    assert_eq!(init["serverInfo"]["name"], "hippo");
    assert_eq!(init["serverInfo"]["version"], env!("CARGO_PKG_VERSION"));
    assert!(init["capabilities"]["tools"].is_object());

    let names: Vec<&str> = reply(&replies, 2)["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "hippo_context",
            "hippo_recall",
            "hippo_ask",
            "hippo_day",
            "hippo_incidents",
            "hippo_note_correction"
        ]
    );

    // The read tools return what the CLI prints.
    assert_eq!(
        text(&replies, 3),
        s.stdout(&["context", "--task", "fix the retry backoff"])
    );
    let recall = text(&replies, 4);
    assert_eq!(
        recall,
        s.stdout(&["recall", "retry backoff", "--limit", "5"])
    );
    assert!(recall.contains("stub-1"), "{recall}");
    let ask = text(&replies, 5);
    assert_eq!(
        steady(&ask),
        steady(&s.stdout(&[
            "ask",
            "--sources-only",
            "how was the webhook retry backoff fixed?"
        ]))
    );
    assert!(ask.contains("searched "), "{ask}");
    let day = text(&replies, 6);
    assert_eq!(day, s.stdout(&["show", "day", "2026-11-01"]));
    assert!(day.starts_with("2026-11-01: "), "{day}");
    assert_eq!(text(&replies, 7), s.stdout(&["incidents", "--days", "14"]));

    // A note's secret is redacted on the way out; the CLI prints the note
    // as written.
    let keys = text(&replies, 8);
    assert!(keys.contains("Deploy keys"), "{keys}");
    let value = secret().split_once('=').unwrap().1.to_owned();
    assert!(!keys.contains(&value), "{keys}");
    assert!(keys.contains("[redacted:"), "{keys}");
    assert!(s
        .stdout(&["context", "--task", "the staging deploy keys"])
        .contains(&secret()));

    // A bad argument is a tool error the model can read.
    let bad = &reply(&replies, 9)["result"];
    assert_eq!(bad["isError"], true);
    assert!(bad["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("query is required"));

    // The correction is reviewed and waits in the Inbox; no note changed.
    let queued = text(&replies, 10);
    assert!(
        queued.starts_with("queued to the Inbox as lesson "),
        "{queued}"
    );
    let lessons: Vec<_> = fs::read_dir(s.home.path().join("lessons"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    assert_eq!(lessons.len(), 1);
    let lesson: Value = serde_json::from_str(&fs::read_to_string(&lessons[0]).unwrap()).unwrap();
    assert_eq!(lesson["status"], "pending");
    assert_eq!(lesson["kind"], "correction");
    assert_eq!(lesson["review"]["verdict"], "confirm");
    for note in ["START HERE.md", "Topic A.md"] {
        assert_eq!(
            fs::read(s.workspace.join("notes").join(note)).unwrap(),
            fs::read(Path::new(FIXTURES).join("notes").join(note)).unwrap()
        );
    }

    assert_eq!(reply(&replies, 11)["result"], json!({}));
}

#[test]
fn without_allow_write_the_write_tool_is_unknown() {
    let s = Setup::new();
    let replies = s.converse(
        &[],
        &[
            request(1, "initialize", json!({ "protocolVersion": "2099-01-01" })),
            tool(
                2,
                "hippo_note_correction",
                json!({ "said": "x", "rule": "y" }),
            ),
        ],
    );
    assert_eq!(
        reply(&replies, 1)["result"]["protocolVersion"],
        "2025-11-25"
    );
    assert_eq!(reply(&replies, 2)["error"]["code"], -32602);
    assert!(!s.home.path().join("lessons").exists());
}

#[test]
fn install_prints_by_default_and_writes_only_with_write() {
    let s = Setup::new();
    let cursor = s.user_home.path().join(".cursor/mcp.json");
    let printed = s.stdout(&["mcp", "install", "--client", "cursor"]);
    assert!(printed.contains("\"mcpServers\""), "{printed}");
    assert!(
        printed.contains(env!("CARGO_BIN_EXE_hippo")) || printed.contains("/hippo\""),
        "{printed}"
    );
    assert!(!cursor.exists());

    fs::create_dir_all(cursor.parent().unwrap()).unwrap();
    fs::write(&cursor, "{\"mcpServers\":{\"other\":{\"command\":\"o\"}}}").unwrap();
    let wrote = s.stdout(&["mcp", "install", "--client", "cursor", "--write"]);
    assert!(wrote.starts_with("added \"hippo\" to "), "{wrote}");
    let now: Value = serde_json::from_str(&fs::read_to_string(&cursor).unwrap()).unwrap();
    assert_eq!(now["mcpServers"]["other"]["command"], "o");
    assert_eq!(now["mcpServers"]["hippo"]["args"], json!(["mcp"]));
    let backups = fs::read_dir(cursor.parent().unwrap())
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("mcp.json.hippo-backup-")
        })
        .count();
    assert_eq!(backups, 1);

    let code = s.stdout(&["mcp", "install", "--client", "claude-code"]);
    assert!(
        code.contains("claude mcp add --transport stdio --scope user hippo -- "),
        "{code}"
    );
    let refused = s
        .command()
        .args(["mcp", "install", "--client", "claude-code", "--write"])
        .output()
        .unwrap();
    assert!(!refused.status.success());
}
