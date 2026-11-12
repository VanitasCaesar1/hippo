//! Drives the `hippo` binary with `HIPPO_HOME` in a temporary directory:
//! init, sessions, context, the hooks, ingest and recall over `fixtures/`,
//! the log and doctor.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::Value;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures");

const ENTRY: &str = "---\ntype: policy\n---\n# Start here\n\nRead the protocol first.\n\n\
                     ## Later\n\nNot part of the entry section.\n";

/// A hippo home and an initialised workspace, in temporary directories.
/// `HOME` is a temporary directory too, where both launchd jobs look
/// installed, so `hippo doctor` never reads the real user's
/// `~/Library/LaunchAgents`.
struct Hippo {
    home: tempfile::TempDir,
    _dir: tempfile::TempDir,
    user_home: tempfile::TempDir,
    workspace: PathBuf,
}

impl Hippo {
    fn init() -> Hippo {
        let home = tempfile::tempdir().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let workspace = fs::canonicalize(dir.path()).unwrap();
        fs::create_dir(workspace.join("notes")).unwrap();
        fs::write(workspace.join("notes/START HERE.md"), ENTRY).unwrap();
        let user_home = tempfile::tempdir().unwrap();
        let agents = user_home.path().join("Library/LaunchAgents");
        fs::create_dir_all(&agents).unwrap();
        for label in ["dev.hippo.ingest", "dev.hippo.dream"] {
            fs::write(agents.join(format!("{label}.plist")), "<plist/>").unwrap();
        }
        let hippo = Hippo {
            home,
            _dir: dir,
            user_home,
            workspace,
        };
        let workspace = hippo.workspace.display().to_string();
        let out = hippo.run(
            &[
                "init",
                "--workspace",
                &workspace,
                "--notes-dir",
                "notes",
                "--entry-note",
                "notes/START HERE.md",
            ],
            None,
        );
        assert!(out.status.success(), "{}", stderr(&out));
        let written = stdout(&out);
        for file in [
            "config.toml",
            ".hippo/hippo.toml",
            ".hippo/packs/core/passes/verify.toml",
        ] {
            assert!(written.contains(file), "{written}");
        }
        hippo
    }

    /// Runs `hippo` with `args`, writing `stdin` to it.
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

    fn session(&self, id: &str) -> Value {
        let path = self.workspace.join(format!(".hippo/sessions/{id}.json"));
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    }
}

fn stdout(out: &Output) -> String {
    String::from_utf8(out.stdout.clone()).unwrap()
}

fn stderr(out: &Output) -> String {
    String::from_utf8(out.stderr.clone()).unwrap()
}

#[test]
fn init_then_session_start_then_context_prints_the_parts() {
    let hippo = Hippo::init();
    let start = [
        "session",
        "start",
        "--tool",
        "claude-code",
        "--session",
        "s1",
    ];
    let out = hippo.run(&[&start[..], &["--task", "fix the retry"]].concat(), None);
    assert!(out.status.success(), "{}", stderr(&out));
    let started = hippo.session("s1")["started"].as_str().unwrap().to_owned();

    let out = hippo.run(&["context", "--session", "s1"], None);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        format!(
            "# Start here\n\nRead the protocol first.\n\n\
             session: s1 claude-code {started}\n\nlinter: overdue=0 dangling=0\n"
        )
    );

    // Required parts over the budget fail; optional ones are omitted.
    let out = hippo.run(&["context", "--budget", "10"], None);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).starts_with("context over budget: "),
        "{}",
        stderr(&out)
    );
    assert!(stderr(&out).ends_with(" > 10\n"), "{}", stderr(&out));
    fs::write(
        hippo.workspace.join("notes/Retry policy.md"),
        "# Retry policy\n\nRetries back off.\n",
    )
    .unwrap();
    let optional = [
        "context",
        "--parts",
        "optional",
        "--task",
        "retry policy",
        "--budget",
        "5",
    ];
    let out = hippo.run(&optional, None);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "");
    let omitted = stderr(&out);
    assert!(omitted.contains("omitted: relevant (") && omitted.contains("omitted: linter ("));

    // A session that does not exist, or an id that cannot be one, is an error.
    assert_eq!(
        hippo
            .run(&["context", "--session", "nobody"], None)
            .status
            .code(),
        Some(1)
    );
    let bad = ["session", "start", "--tool", "codex", "--session", "../x"];
    assert_eq!(hippo.run(&bad, None).status.code(), Some(1));

    // Ending with a handoff from stdin.
    let end = [
        "session",
        "end",
        "--session",
        "s1",
        "--note",
        "notes/START HERE.md",
    ];
    let out = hippo.run(
        &[&end[..], &["--handoff", "-"]].concat(),
        Some("Done; see [[START HERE]].\n"),
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let record = hippo.session("s1");
    assert_eq!(record["handoff"], "Done; see [[START HERE]].\n");
    assert_eq!(record["notes_touched"][0], "notes/START HERE.md");
    assert!(record["no_change_reason"].is_null());
}

#[test]
fn hooks_print_context_and_never_fail_the_session() {
    let hippo = Hippo::init();
    let start = r#"{"session_id":"h1","hook_event_name":"SessionStart","source":"startup"}"#;
    let out = hippo.run(&["hook", "session-start"], Some(start));
    assert!(out.status.success(), "{}", stderr(&out));
    let record = hippo.session("h1");
    let started = record["started"].as_str().unwrap();
    let printed = stdout(&out);
    assert_eq!(
        printed,
        format!("# Start here\n\nRead the protocol first.\n\nsession: h1 claude-code {started}\n")
    );
    assert_eq!(record["context_bytes_at_start"], printed.len());
    assert_eq!(record["derived"], false);

    // The first prompt gets the relevant notes (never the entry note,
    // which the block already has) and the linter counts and becomes the
    // task; later prompts get nothing.
    fs::write(
        hippo.workspace.join("notes/Retry policy.md"),
        "# Retry policy\n\nRetries back off.\n",
    )
    .unwrap();
    let prompt = r#"{"session_id":"h1","hook_event_name":"UserPromptSubmit","prompt":"the retry policy, please"}"#;
    let out = hippo.run(&["hook", "prompt"], Some(prompt));
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        "relevant:\n- Retry policy: Retries back off.\n\nlinter: overdue=0 dangling=0\n"
    );
    let record = hippo.session("h1");
    assert_eq!(record["task"], "the retry policy, please");
    assert_eq!(record["task_seen"], true);
    let out = hippo.run(&["hook", "prompt"], Some(prompt));
    assert!(out.status.success());
    assert_eq!(stdout(&out), "");

    let end = r#"{"session_id":"h1","hook_event_name":"SessionEnd"}"#;
    assert!(hippo
        .run(&["hook", "session-end"], Some(end))
        .status
        .success());
    let record = hippo.session("h1");
    assert!(record["ended"].is_string());
    assert_eq!(record["no_change_reason"], "unspecified");

    // Errors print one line and still exit 0.
    for (hook, input) in [
        ("prompt", r#"{"session_id":"nobody","prompt":"hi"}"#),
        ("session-start", "not json"),
        ("session-end", r#"{"hook_event_name":"SessionEnd"}"#),
        ("session-start", r#"{"session_id":"../escape"}"#),
    ] {
        let out = hippo.run(&["hook", hook], Some(input));
        assert!(out.status.success(), "{hook} {input}");
        let err = stderr(&out);
        assert!(err.starts_with("hippo hook: "), "{err}");
        assert_eq!(err.lines().count(), 1, "{err}");
    }
}

#[test]
fn ingest_recall_log_and_doctor() {
    let hippo = Hippo::init();
    // Index the synthetic transcripts only (not the eval set's under
    // `eval/`), run in the workspace, and never run a model.
    let copies = hippo.home.path().join("transcripts");
    fixtures_in(&copies, &hippo.workspace);
    let config = format!(
        "workspace = \"{}\"\ntranscript_roots = [\"{}/claude-code\", \"{}/codex\"]\n\
         model_command = []\n",
        hippo.workspace.display(),
        copies.display(),
        copies.display(),
    );
    fs::write(hippo.home.path().join("config.toml"), config).unwrap();

    let out = hippo.run(&["ingest"], None);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        "indexed 3 files, skipped 0, derived 3 sessions\n"
    );
    let out = hippo.run(&["ingest"], None);
    assert_eq!(
        stdout(&out),
        "indexed 0 files, skipped 3, derived 0 sessions\n"
    );

    let codex = "codex-01900000-c0de-7000-8000-000000000001";
    let out = hippo.run(&["recall", "retry backoff"], None);
    assert!(out.status.success(), "{}", stderr(&out));
    let lines = stdout(&out);
    assert!(
        lines
            .lines()
            .any(|line| line.starts_with(&format!("{codex} codex "))),
        "{lines}"
    );
    let out = hippo.run(
        &["recall", "--json", "--limit", "500", "retry backoff"],
        None,
    );
    assert!(out.status.success(), "{}", stderr(&out));
    for line in stdout(&out).lines() {
        let hit: Value = serde_json::from_str(line).unwrap();
        for key in [
            "session",
            "tool",
            "ordinal",
            "ts",
            "role",
            "tool_name",
            "snippet",
            "score",
        ] {
            assert!(hit.get(key).is_some(), "{line} lacks {key}");
        }
        assert!(hit["ordinal"].is_u64(), "{line}");
        assert!(!hit["snippet"].as_str().unwrap().contains('\n'));
    }
    // The key is redacted, so nothing matches: the empty result names the
    // next step, and JSON stays empty.
    let out = hippo.run(&["recall", "AKIAFAKEFAKEFAKE0000"], None);
    assert!(
        stdout(&out).starts_with("no turn matches \"AKIAFAKEFAKEFAKE0000\"\n"),
        "{}",
        stdout(&out)
    );
    let out = hippo.run(&["recall", "AKIAFAKEFAKEFAKE0000", "--json"], None);
    assert_eq!(stdout(&out), "");
    let out = hippo.run(&["recall", "--raw", "AND AND"], None);
    assert_eq!(out.status.code(), Some(1));
    assert!(!stderr(&out).is_empty());

    let out = hippo.run(&["log"], None);
    assert!(out.status.success(), "{}", stderr(&out));
    let path = PathBuf::from(stdout(&out).trim_end());
    assert_eq!(path, hippo.workspace.join("notes/Session log.md"));
    assert!(fs::read_to_string(&path)
        .unwrap()
        .starts_with("# Session log\n"));
    let out = hippo.run(&["log", "--stdout"], None);
    assert!(stdout(&out).contains(&format!("codex {codex} — (no task)")));

    // Nothing is wrong yet; a snapshot without a receipt is a problem.
    let out = hippo.run(&["doctor"], None);
    assert!(out.status.success(), "{}", stdout(&out));
    assert!(stdout(&out).starts_with("check | status | detail\n"));
    let snapshot = hippo
        .workspace
        .join(".hippo/snapshots/01JC9X4Q3A8M2K7N5P6R8S9T0V");
    fs::create_dir_all(&snapshot).unwrap();
    let out = hippo.run(&["doctor"], None);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stdout(&out).contains("snapshots | problem"),
        "{}",
        stdout(&out)
    );
}

// `hippo show` and `hippo context --json`.

use hippo_core::adapters::Role;
use hippo_core::index::{Index, SessionSource, Turn};
use hippo_core::records::{
    Evidence, Proposal, ProposalKind, Receipt, Session, Tool as RecordTool, Verdict,
};
use hippo_core::redact::redact;
use time::macros::datetime;

const PROPOSAL: &str = "01JC9X4Q3A8M2K7N5P6R8S9T0V";
const CONFIRM: &str = "01JC9X4Q3A8M2K7N5P6R8S9T1A";
const REJECT: &str = "01JC9X4Q3A8M2K7N5P6R8S9T1B";
const OTHER: &str = "01JC9X4Q3A8M2K7N5P6R8S9T0W";

impl Hippo {
    /// Writes the index turns of session `id`: user, then a tool turn.
    fn index_turns(&self, id: &str) {
        let mut index = Index::open(self.home.path(), true).unwrap();
        let ts = datetime!(2026-11-08 03:00:00 UTC);
        let turns = [
            Turn {
                ordinal: 0,
                role: Role::User,
                ts,
                tool_name: None,
                text: redact("fix the\nretry backoff"),
            },
            Turn {
                ordinal: 1,
                role: Role::Tool,
                ts,
                tool_name: Some("Bash".into()),
                text: redact("cargo test"),
            },
        ];
        let path = PathBuf::from(format!("/synthetic/{id}.jsonl"));
        index
            .replace_session(
                &SessionSource {
                    id,
                    tool: RecordTool::ClaudeCode,
                    transcript_path: &path,
                    file_hash: "0",
                },
                &turns,
            )
            .unwrap();
    }

    fn write_session(&self, id: &str) {
        let session = Session {
            id: id.to_owned(),
            tool: RecordTool::ClaudeCode,
            task: "retry".into(),
            task_seen: true,
            context_bytes_at_start: 0,
            started: datetime!(2026-11-08 03:00:00 UTC),
            ended: Some(datetime!(2026-11-08 03:30:00 UTC)),
            notes_touched: vec![],
            handoff: None,
            handoff_derived: false,
            no_change_reason: None,
            context_start: None,
            context_prompt: None,
            derived: false,
        };
        session
            .save(&self.workspace.join(format!(".hippo/sessions/{id}.json")))
            .unwrap();
    }

    fn write_proposal(&self, id: &str, note: &str, evidence: &[(&str, u32)]) {
        let proposal = Proposal {
            id: id.parse().unwrap(),
            pass: "verify".into(),
            note: note.into(),
            kind: ProposalKind::Replace,
            before_hash: "a".repeat(64),
            after_hash: "b".repeat(64),
            old: "Read the\nprotocol".into(),
            new: "Read the protocol twice".into(),
            evidence: evidence
                .iter()
                .map(|(session, ordinal)| Evidence {
                    session: (*session).into(),
                    ordinal: *ordinal,
                })
                .collect(),
            reason: "the log says so".into(),
        };
        proposal
            .save(&self.workspace.join(format!(".hippo/proposals/{id}.json")))
            .unwrap();
    }

    fn write_receipt(&self, id: &str, proposal: &str, verdict: Verdict, applied: bool) {
        let receipt = Receipt {
            id: id.parse().unwrap(),
            proposal: proposal.parse().unwrap(),
            proposal_hash: "c".repeat(64),
            before_hash: "a".repeat(64),
            after_hash: "b".repeat(64),
            reviewer: "verify-reviewer".into(),
            invocation: "claude".into(),
            verdict,
            notes: "looks\nright".into(),
            at: datetime!(2026-11-08 04:00:00 UTC),
            applied: applied.then_some(datetime!(2026-11-08 04:00:01 UTC)),
            snapshot: None,
        };
        receipt
            .save(&self.workspace.join(format!(".hippo/receipts/{id}.json")))
            .unwrap();
    }

    fn fail(&self, args: &[&str]) -> String {
        let out = self.run(args, None);
        assert_eq!(
            out.status.code(),
            Some(1),
            "{}{}",
            stdout(&out),
            stderr(&out)
        );
        assert_eq!(stdout(&out), "");
        stderr(&out)
    }

    fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args, None);
        assert!(out.status.success(), "{}", stderr(&out));
        stdout(&out)
    }
}

fn json_lines(text: &str) -> Vec<Value> {
    text.lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn show_day_prints_the_rollup_of_a_day() {
    let hippo = Hippo::init();
    assert_eq!(
        hippo.fail(&["show", "day", "2026-11-08"]),
        format!(
            "no rollup in {}: run hippo ingest, which writes it\n",
            hippo.home.path().display()
        )
    );
    hippo.index_turns("s1");
    let index = Index::open(hippo.home.path(), false).unwrap();
    hippo_core::rollup::refresh(hippo.home.path(), None, &index).unwrap();

    let text = hippo.ok(&["show", "day", "2026-11-08"]);
    assert_eq!(
        text,
        "2026-11-08: 1 sessions, 0 agents, 2 turns, 0 edits, 0 pull requests opened, 0 merged\n\
         fix the · - · 2 turns · 0 edits · interrupted · s1\n"
    );
    // 03:00 UTC is the day before at -05:00.
    let text = hippo.ok(&["show", "day", "2026-11-08", "--offset", "-05:00"]);
    assert!(text.starts_with("2026-11-08: 0 sessions"), "{text}");
    let value: Value =
        serde_json::from_str(&hippo.ok(&["show", "day", "2026-11-08", "--json"])).unwrap();
    assert_eq!(value["rows"][0]["id"], "s1");
    assert_eq!(value["rows"][0]["ending"], "interrupted");
    assert!(hippo.fail(&["show", "day", "8 Nov"]).contains("YYYY-MM-DD"));
}

#[test]
fn show_session_prints_the_log_line_and_the_turns() {
    let hippo = Hippo::init();
    hippo.write_session("s1");
    // No index yet.
    assert_eq!(
        hippo.fail(&["show", "session", "s1"]),
        "no index; run hippo ingest\n"
    );
    hippo.index_turns("s1");
    hippo.index_turns("orphan");

    let log = hippo.ok(&["log", "--stdout"]);
    let line = log.lines().find(|l| l.starts_with("- ")).unwrap();
    let text = hippo.ok(&["show", "session", "s1"]);
    assert_eq!(
        text,
        format!("{line}\ntitle: fix the\n0 user fix the retry backoff\n1 tool Bash cargo test\n")
    );
    assert!(line.contains("turns 2"));

    let lines = json_lines(&hippo.ok(&["show", "session", "s1", "--json"]));
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0], hippo.session("s1"));
    assert_eq!(lines[1]["ordinal"], 0);
    assert_eq!(lines[1]["text"], "fix the\nretry backoff");
    assert_eq!(lines[1]["tool_name"], Value::Null);
    assert_eq!(lines[1]["ts"], "2026-11-08T03:00:00.000000Z");
    assert_eq!(lines[2]["tool_name"], "Bash");
    assert!(hippo
        .ok(&["show", "session", "s1", "--json"])
        .lines()
        .all(|l| !l.contains('\n')));

    // Turns without a record.
    let text = hippo.ok(&["show", "session", "orphan"]);
    assert!(text.starts_with("(no session record)\ntitle: "), "{text}");
    let lines = json_lines(&hippo.ok(&["show", "session", "orphan", "--json"]));
    assert_eq!(lines[0], Value::Null);
    assert_eq!(lines.len(), 3);

    // A record without turns.
    hippo.write_session("quiet");
    let text = hippo.ok(&["show", "session", "quiet"]);
    assert!(text.ends_with("\nno indexed turns\n"), "{text}");
    assert!(text.starts_with("- 03:00 claude-code quiet"), "{text}");
    assert_eq!(
        json_lines(&hippo.ok(&["show", "session", "quiet", "--json"])).len(),
        1
    );

    assert_eq!(
        hippo.fail(&["show", "session", "nope"]),
        "no indexed session nope (hippo fleet lists today's sessions, hippo show day <date> a day's)\n"
    );
    assert_eq!(
        hippo.fail(&["show", "session", "bad/id"]),
        "invalid session id \"bad/id\"\n"
    );
}

#[test]
fn show_proposal_prints_receipts_and_evidence() {
    let hippo = Hippo::init();
    hippo.index_turns("s1");
    hippo.write_proposal(PROPOSAL, "notes/START HERE.md", &[("s1", 1), ("s2", 7)]);
    hippo.write_receipt(REJECT, PROPOSAL, Verdict::Reject, false);
    hippo.write_receipt(CONFIRM, PROPOSAL, Verdict::Confirm, true);
    hippo.write_receipt("01JC9X4Q3A8M2K7N5P6R8S9T1C", OTHER, Verdict::Confirm, true);

    let text = hippo.ok(&["show", "proposal", PROPOSAL]);
    assert_eq!(
        text,
        format!(
            "proposal {PROPOSAL} verify notes/START HERE.md replace\n\
             old: Read the protocol\nnew: Read the protocol twice\nreason: the log says so\n\
             receipt {CONFIRM} confirm verify-reviewer applied 2026-11-08T04:00:01Z looks right\n\
             receipt {REJECT} reject verify-reviewer applied no looks right\n\
             evidence s1 1 tool cargo test\n\
             evidence s2 7 (not in the index)\n"
        )
    );
    let value = json_lines(&hippo.ok(&["show", "proposal", PROPOSAL, "--json"]));
    assert_eq!(value.len(), 1);
    let value = &value[0];
    let on_disk: Value = serde_json::from_str(
        &fs::read_to_string(
            hippo
                .workspace
                .join(format!(".hippo/proposals/{PROPOSAL}.json")),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(value["proposal"], on_disk);
    assert_eq!(value["receipts"].as_array().unwrap().len(), 2);
    assert_eq!(value["receipts"][0]["id"], CONFIRM);
    assert_eq!(value["evidence"][0]["session"], "s1");
    assert_eq!(value["evidence"][0]["ordinal"], 1);
    assert_eq!(value["evidence"][0]["tool_name"], "Bash");
    assert_eq!(
        value["evidence"][1],
        serde_json::json!({"session": "s2", "ordinal": 7, "missing": true})
    );

    // A receipt whose proposal is missing.
    assert_eq!(
        hippo.fail(&["show", "proposal", OTHER]),
        format!("no proposal {OTHER}\n")
    );
    let err = hippo.fail(&["show", "proposal", "nope"]);
    assert!(err.starts_with("not a proposal id: nope"), "{err}");
}

#[test]
fn show_note_prints_lint_findings_and_receipts() {
    let hippo = Hippo::init();
    fs::write(
        hippo.workspace.join("notes/Topic.md"),
        "---\ntype: policy\n---\n# Topic\n\nSee [[Missing note]].\n",
    )
    .unwrap();
    hippo.write_proposal(PROPOSAL, "./notes/Topic.md", &[]);
    hippo.write_receipt(CONFIRM, PROPOSAL, Verdict::Confirm, true);
    hippo.write_proposal(OTHER, "notes/START HERE.md", &[]);
    hippo.write_receipt(REJECT, OTHER, Verdict::Reject, false);

    let text = hippo.ok(&["show", "note", "notes/Topic.md"]);
    assert_eq!(
        text,
        format!(
            "lint dangling-link [[Missing note]]\n\
             receipt {CONFIRM} confirm {PROPOSAL} 2026-11-08T04:00:01Z\n"
        )
    );
    let same = hippo.ok(&["show", "note", "./notes/Topic.md"]);
    assert_eq!(same, text);
    let value = json_lines(&hippo.ok(&["show", "note", "notes/Topic.md", "--json"]));
    assert_eq!(value.len(), 1);
    assert_eq!(value[0]["note"], "notes/Topic.md");
    assert_eq!(
        value[0]["lint"],
        serde_json::json!([{"kind": "dangling-link", "detail": "[[Missing note]]"}])
    );
    assert_eq!(value[0]["receipts"].as_array().unwrap().len(), 1);
    assert_eq!(value[0]["receipts"][0]["id"], CONFIRM);

    // A clean note prints nothing.
    assert_eq!(
        hippo.ok(&["show", "note", "notes/START HERE.md"]),
        format!("receipt {REJECT} reject {OTHER} no\n")
    );

    assert_eq!(
        hippo.fail(&["show", "note", "notes/Gone.md"]),
        "no note notes/Gone.md\n"
    );
    for outside in ["../x.md", ".hippo/hippo.toml", "README.md"] {
        let err = hippo.fail(&["show", "note", outside]);
        assert!(err.starts_with("not a note inside notes_dir"), "{err}");
    }
}

#[test]
fn context_json_matches_the_text_and_the_omitted_lines() {
    let hippo = Hippo::init();
    let settings = hippo.workspace.join(".hippo/hippo.toml");
    let toml = fs::read_to_string(&settings).unwrap();
    let toml = toml.replace(
        "required_notes = []",
        "required_notes = [\"notes/START HERE.md\"]",
    );
    assert!(toml.contains("START HERE"), "{toml}");
    fs::write(&settings, toml).unwrap();
    let text = hippo.ok(&["context", "--task", "protocol"]);
    let out = hippo.run(&["context", "--task", "protocol", "--json"], None);
    assert!(out.status.success(), "{}", stderr(&out));
    let lines = json_lines(&stdout(&out));
    assert_eq!(lines.len(), 1);
    let value = &lines[0];
    assert_eq!(value["text"], text);
    assert_eq!(value["bytes"], text.len());
    assert_eq!(
        value["required"],
        serde_json::json!(["notes/START HERE.md"])
    );
    assert!(value["relevant"].is_array());
    assert!(value["budget"].as_u64().unwrap() > 0);
    assert_eq!(value["omitted"], serde_json::json!([]));

    // A budget that fits only the required parts omits the rest, as the
    // text form reports on stderr.
    let plain = hippo.run(&["context", "--task", "protocol", "--budget", "100"], None);
    let json = hippo.run(
        &["context", "--task", "protocol", "--budget", "100", "--json"],
        None,
    );
    assert!(plain.status.success() && json.status.success());
    let value = &json_lines(&stdout(&json))[0];
    assert_eq!(value["budget"], 100);
    let omitted: Vec<String> = value["omitted"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| format!("omitted: {} ({})", o["part"].as_str().unwrap(), o["bytes"]))
        .collect();
    assert_eq!(omitted.join("\n"), stderr(&plain).trim_end());
    assert_eq!(value["text"], stdout(&plain));

    // Errors are as without --json.
    let err = hippo.fail(&["context", "--budget", "10", "--json"]);
    assert!(err.starts_with("context over budget: "), "{err}");
    assert_eq!(
        hippo.fail(&["context", "--session", "nope", "--json"]),
        "no session nope\n"
    );
}

/// Runs `hippo hook session-start --plugin` with `input` on stdin, the
/// user's Claude settings in `claude_dir`, in `cwd`, with `HIPPO_HOME`
/// `home`.
fn plugin_hook(
    home: &std::path::Path,
    user_home: &std::path::Path,
    claude_dir: &std::path::Path,
    cwd: &std::path::Path,
    input: &str,
) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_hippo"))
        .args(["hook", "session-start", "--plugin"])
        .current_dir(cwd)
        .env("HIPPO_HOME", home)
        .env("HOME", user_home)
        .env("CLAUDE_CONFIG_DIR", claude_dir)
        .env_remove("CLAUDE_PROJECT_DIR")
        .env_remove("RUST_LOG")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(input.as_bytes()).unwrap();
    drop(stdin);
    child.wait_with_output().unwrap()
}

#[test]
fn the_plugin_hook_is_silent_unless_it_has_notes_to_give_and_nobody_gave_them() {
    let claude = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let elsewhere = fs::canonicalize(elsewhere.path()).unwrap();
    let input = |cwd: &std::path::Path| {
        format!(
            r#"{{"session_id":"p1","hook_event_name":"SessionStart","source":"startup","cwd":{}}}"#,
            Value::from(cwd.display().to_string())
        )
    };
    let silent = |out: &Output, what: &str| {
        assert!(out.status.success(), "{what}");
        assert_eq!(stdout(out), "", "{what}");
        assert_eq!(stderr(out), "", "{what}");
    };

    // Not set up: nothing, not even the not-set-up line.
    let empty = tempfile::tempdir().unwrap();
    let out = plugin_hook(
        empty.path(),
        empty.path(),
        claude.path(),
        &elsewhere,
        &input(&elsewhere),
    );
    silent(&out, "not set up");

    // History only: no notes to give.
    let history = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_hippo"))
        .arg("init")
        .current_dir(&elsewhere)
        .env("HIPPO_HOME", history.path())
        .env("HOME", history.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let out = plugin_hook(
        history.path(),
        history.path(),
        claude.path(),
        &elsewhere,
        &input(&elsewhere),
    );
    silent(&out, "history only");

    // A workspace: outside it nothing, inside it the required parts, with
    // no session line and no session record.
    let hippo = Hippo::init();
    let run = |cwd: &std::path::Path, stdin: &str| {
        plugin_hook(
            hippo.home.path(),
            hippo.user_home.path(),
            claude.path(),
            cwd,
            stdin,
        )
    };
    silent(
        &run(&elsewhere, &input(&elsewhere)),
        "outside the workspace",
    );
    let block = "# Start here\n\nRead the protocol first.\n";
    let ws = hippo.workspace.clone();
    let sub = ws.join("app/src");
    fs::create_dir_all(&sub).unwrap();
    for cwd in [&ws, &sub] {
        let out = run(&elsewhere, &input(cwd));
        assert!(out.status.success());
        assert_eq!(
            stdout(&out).trim_end(),
            block.trim_end(),
            "{}",
            cwd.display()
        );
        assert_eq!(stderr(&out), "");
    }
    assert!(!ws.join(".hippo/sessions/p1.json").exists());
    // Without a cwd in the input, the process's directory counts.
    let out = run(&ws, r#"{"session_id":"p1"}"#);
    assert_eq!(stdout(&out).trim_end(), block.trim_end());
    // Input that is not JSON is the same as no cwd.
    silent(&run(&elsewhere, "not json"), "not json, outside");

    // A settings file that already runs the workspace hook silences it.
    let hook = r#"{"hooks": {"SessionStart": [{"hooks": [{"type": "command", "command": "hippo hook session-start"}]}]}}"#;
    for file in [
        ws.join(".claude/settings.json"),
        ws.join(".claude/settings.local.json"),
        sub.join(".claude/settings.json"),
        claude.path().join("settings.json"),
    ] {
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, hook).unwrap();
        silent(&run(&elsewhere, &input(&sub)), &file.display().to_string());
        // An unreadable file is the same as no hook.
        fs::write(&file, "not json").unwrap();
        let out = run(&elsewhere, &input(&sub));
        assert_eq!(
            stdout(&out).trim_end(),
            block.trim_end(),
            "{}",
            file.display()
        );
        fs::remove_file(&file).unwrap();
    }
    // `hippo hooks install` writes what the check looks for.
    let out = hippo.run(
        &["hooks", "install", "--workspace", &ws.display().to_string()],
        None,
    );
    assert!(out.status.success(), "{}", stderr(&out));
    silent(&run(&elsewhere, &input(&ws)), "after hooks install");
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
