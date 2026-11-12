//! Tests for the x-ray, on synthetic transcripts only.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::claims::{self, claims_in, ClaimKind, Status};
use super::*;

fn turn(ordinal: u32, role: &str, tool: Option<&str>, text: &str) -> StoredTurn {
    StoredTurn {
        ordinal,
        role: role.to_owned(),
        ts: format!("2026-11-10T10:{:02}:{:02}Z", ordinal / 60, ordinal % 60),
        tool_name: tool.map(str::to_owned),
        text: text.to_owned(),
    }
}

/// Builds a transcript turn by turn, numbering and timing them.
#[derive(Default)]
struct T {
    turns: Vec<StoredTurn>,
}

impl T {
    fn n(&self) -> u32 {
        self.turns.len() as u32
    }
    fn user(mut self, text: &str) -> T {
        let n = self.n();
        self.turns.push(turn(n, "user", None, text));
        self
    }
    fn say(mut self, text: &str) -> T {
        let n = self.n();
        self.turns.push(turn(n, "assistant", None, text));
        self
    }
    fn call(mut self, tool: &str, input: Value) -> T {
        let n = self.n();
        self.turns
            .push(turn(n, "tool", Some(tool), &format!("{tool} {input}")));
        self
    }
    fn result(mut self, tool: &str, text: &str) -> T {
        let n = self.n();
        self.turns.push(turn(n, "tool", Some(tool), text));
        self
    }
    fn bash(self, command: &str, output: &str) -> T {
        let t = self.call("Bash", json!({ "command": command }));
        if output.is_empty() {
            t
        } else {
            t.result("Bash", output)
        }
    }
    fn edit(self, path: &str) -> T {
        self.call(
            "Edit",
            json!({"file_path": path, "old_string": "a", "new_string": "b"}),
        )
        .result("Edit", "The file has been updated.")
    }
}

fn check(turns: &[StoredTurn]) -> Vec<Claim> {
    claims::check(&claims::Inputs {
        turns,
        subs: &[],
        dir: Some(Path::new("/work/app")),
        notes_roots: &[PathBuf::from("/work/notes")],
    })
}

fn one(turns: &[StoredTurn]) -> Claim {
    let c = check(turns);
    assert_eq!(c.len(), 1, "{c:#?}");
    c.into_iter().next().unwrap()
}

// ---------------------------------------------------------------------------
// Finding claims

#[test]
fn claims_are_found_in_plain_statements() {
    let kinds = |t: &str| -> Vec<ClaimKind> { claims_in(t).into_iter().map(|c| c.0).collect() };
    assert_eq!(kinds("All 47 tests pass."), vec![ClaimKind::Tests]);
    assert_eq!(
        kinds("`cargo test` passes, 312 passed, 0 failed."),
        vec![ClaimKind::Tests]
    );
    assert_eq!(kinds("Typecheck is clean."), vec![ClaimKind::Typecheck]);
    assert_eq!(
        kinds("Clippy clean and fmt clean."),
        vec![ClaimKind::Lint, ClaimKind::Fmt]
    );
    assert_eq!(kinds("The release build succeeds."), vec![ClaimKind::Build]);
    assert_eq!(kinds("PR #214 merged."), vec![ClaimKind::PrMerged]);
    assert_eq!(
        kinds("Squash-merged #12 and pushed."),
        vec![ClaimKind::PrMerged, ClaimKind::Pushed]
    );
    assert_eq!(kinds("Opened PR #9 for review."), vec![ClaimKind::PrOpened]);
    assert_eq!(kinds("Deployed to production."), vec![ClaimKind::Deployed]);
    assert_eq!(kinds("Brain check passes."), vec![ClaimKind::BrainCheck]);
    let pr: Vec<Option<u64>> = claims_in("PR #214 merged.")
        .into_iter()
        .map(|c| c.1)
        .collect();
    assert_eq!(pr, vec![Some(214)]);
}

#[test]
fn hedges_plans_negations_and_reports_are_not_claims() {
    for text in [
        "Tests should pass once the fixture lands.",
        "I'll run the tests next.",
        "Tests don't pass yet.",
        "The tests are failing on main.",
        "If the build passes, I will merge.",
        "Do the tests pass?",
        "The sub-agent reported that tests pass.",
        "Not deployed: the panel needs a login.",
        "Nothing was pushed.",
        "Run `cargo test` to check that tests pass when you are back.",
        "```\ntest result: ok. 3 passed\n```",
        "> tests pass",
        "The deployed version is older.",
    ] {
        assert!(claims_in(text).is_empty(), "{text}: {:?}", claims_in(text));
    }
}

// ---------------------------------------------------------------------------
// Evidence

#[test]
fn a_test_run_after_the_last_edit_verifies_the_claim() {
    let t = T::default()
        .user("fix it")
        .edit("/work/app/src/lib.rs")
        .bash(
            "cd /work/app && cargo test --locked",
            "test result: ok. 40 passed; 0 failed\ntest result: ok. 7 passed; 0 failed",
        )
        .say("Fixed. All 47 tests pass.");
    let c = one(&t.turns);
    assert_eq!(c.status, Status::Verified);
    assert!(c.evidence.contains("cargo test --locked"), "{}", c.evidence);
    assert!(c.evidence.contains("47 passed"), "{}", c.evidence);
}

#[test]
fn a_test_run_before_a_code_edit_does_not() {
    let t = T::default()
        .bash("cd /work/app && cargo test", "test result: ok. 4 passed")
        .edit("/work/app/src/lib.rs")
        .say("Done; tests pass.");
    let c = one(&t.turns);
    assert_eq!(c.status, Status::Unverified);
    assert!(
        c.evidence.contains("before the last edit"),
        "{}",
        c.evidence
    );
}

#[test]
fn a_prose_edit_or_an_edit_elsewhere_after_the_run_does_not_make_it_stale() {
    let dir = tempfile::tempdir().unwrap();
    let app = dir.path().join("app");
    let other = dir.path().join("other");
    for d in [&app, &other] {
        fs::create_dir_all(d.join(".git")).unwrap();
    }
    let t = T::default()
        .edit(&format!("{}/src/lib.rs", app.display()))
        .bash(
            &format!("cd {} && cargo test", app.display()),
            "test result: ok. 4 passed",
        )
        .edit(&format!("{}/CHANGELOG.md", app.display()))
        .edit(&format!("{}/src/main.rs", other.display()))
        .say("Tests pass.");
    let c = one(&t.turns);
    assert_eq!(c.status, Status::Verified, "{}", c.evidence);
}

#[test]
fn a_failed_run_with_no_later_pass_is_unverified() {
    let t = T::default()
        .bash(
            "cargo test",
            "Exit code 101\ntest result: FAILED. 3 passed; 1 failed",
        )
        .say("All tests pass now.");
    let c = one(&t.turns);
    assert_eq!(c.status, Status::Unverified);
    assert!(c.evidence.contains("failed (exit 101)"), "{}", c.evidence);
}

#[test]
fn piped_output_with_a_failure_is_a_failure() {
    let t = T::default()
        .bash(
            "cargo test 2>&1 | tail -3",
            "test result: FAILED. 3 passed; 1 failed",
        )
        .say("Tests pass.");
    assert_eq!(one(&t.turns).status, Status::Unverified);
}

#[test]
fn no_command_at_all_is_unverified_and_says_so() {
    let t = T::default().user("go").say("Typecheck clean.");
    let c = one(&t.turns);
    assert_eq!(c.status, Status::Unverified);
    assert_eq!(c.evidence, "no typecheck command in the session");
}

#[test]
fn deployed_without_a_deploy_command_says_what_would_count() {
    let t = T::default()
        .bash(
            "gh pr merge 5 --squash",
            "✓ Squashed and merged pull request #5",
        )
        .say("Merged PR #5 and deployed it.");
    let c = check(&t.turns);
    let d = c.iter().find(|c| c.kind == ClaimKind::Deployed).unwrap();
    assert_eq!(d.status, Status::Unverified);
    assert!(d
        .evidence
        .contains("a deploy command and its success output would count"));
}

#[test]
fn a_deploy_command_that_succeeded_verifies_deployed() {
    let t = T::default()
        .bash(
            "curl -s -X POST https://panel.example/api/application.deploy -d '{}'",
            "{\"ok\":true}",
        )
        .say("Deployed to production.");
    assert_eq!(one(&t.turns).status, Status::Verified);
}

#[test]
fn a_quiet_command_with_no_output_succeeded() {
    let t = T::default()
        .bash("cargo fmt --all --check", "")
        .say("fmt clean.");
    assert_eq!(one(&t.turns).status, Status::Verified);
}

#[test]
fn a_background_run_with_no_result_is_unclear_not_unverified() {
    let t = T::default()
        .bash(
            "cargo test",
            "Command running in background with ID: bash_7. Output is being written to /tmp/x",
        )
        .say("Tests pass.");
    assert_eq!(one(&t.turns).status, Status::Unclear);
    let t = T::default()
        .bash("cargo test", "Command running in background with ID: bash_7.")
        .user("<task-notification><task-id>bash_7</task-id><status>completed</status><exit-code>0</exit-code></task-notification>")
        .say("Tests pass.");
    assert_eq!(one(&t.turns).status, Status::Verified);
}

#[test]
fn a_sub_agents_run_counts_for_the_parent() {
    let parent = T::default()
        .edit("/work/app/src/lib.rs")
        .call("Agent", json!({"prompt": "run the tests"}))
        .result("Agent", "done")
        .say("Tests pass.");
    let mut sub = T::default().bash("cd /work/app && npm test", "Tests: 12 passed, 12 total");
    for t in &mut sub.turns {
        t.ts = parent.turns[2].ts.clone();
    }
    let subs = vec![("p-agent-a1".to_owned(), sub.turns)];
    let c = claims::check(&claims::Inputs {
        turns: &parent.turns,
        subs: &subs,
        dir: Some(Path::new("/work/app")),
        notes_roots: &[],
    });
    assert_eq!(c[0].status, Status::Verified, "{}", c[0].evidence);
    assert!(c[0].evidence.contains("sub-agent"));
}

#[test]
fn a_delegates_report_is_unclear() {
    let t = T::default()
        .call(
            "mcp__t3-code__delegate_task",
            json!({"task": "fix and test"}),
        )
        .result(
            "mcp__t3-code__delegate_task",
            "Child finished: all tests pass, 9 passed.",
        )
        .say("Done; tests pass.");
    let c = one(&t.turns);
    assert_eq!(c.status, Status::Unclear, "{}", c.evidence);
}

#[test]
fn a_merge_is_verified_by_the_merge_call() {
    let t = T::default()
        .bash(
            "gh pr merge https://github.com/o/r/pull/214 --squash --delete-branch",
            "✓ Squashed and merged pull request o/r#214",
        )
        .say("PR #214 merged.");
    let c = one(&t.turns);
    assert_eq!(c.status, Status::Verified, "{}", c.evidence);
}

#[test]
fn the_brain_check_is_stale_after_a_note_edit() {
    let t = T::default()
        .bash("python3 /work/notes/tools/lint_notes.py", "ok: 0 problems")
        .edit("/work/notes/Session log.md")
        .say("Brain check passes.");
    let c = one(&t.turns);
    assert_eq!(c.status, Status::Unverified);
    assert!(c.evidence.contains("before the last edit"));
}

#[test]
fn the_last_claim_of_a_kind_is_kept() {
    let t = T::default()
        .say("Tests pass.")
        .bash("cargo test", "test result: ok. 2 passed")
        .say("Tests pass, 2 passed.");
    let c = one(&t.turns);
    assert_eq!(c.status, Status::Verified);
}

// ---------------------------------------------------------------------------
// The block and its use

fn workspace() -> (tempfile::TempDir, Workspace) {
    let dir = tempfile::tempdir().unwrap();
    let notes = [
        (
            "START HERE.md",
            "# Example brain\n\nThe shared notes for agents.\n\n## More\n",
        ),
        (
            "Rules.md",
            "# Agent rules\n\nAlways run the formatter before a commit, every time.\n",
        ),
        ("Questions.md", "# Open questions\n\nNone yet.\n"),
        (
            "Deploy notes.md",
            "# Deploy notes\n\nThe panel deploys on merge.\n",
        ),
    ];
    for (path, text) in notes {
        let p = dir.path().join("brain").join(path);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }
    let ws = Workspace {
        root: dir.path().to_path_buf(),
        notes_dir: "brain".into(),
        entry_note: "brain/START HERE.md".into(),
        required_notes: vec![
            "brain/START HERE.md".into(),
            "brain/Rules.md".into(),
            "brain/Questions.md".into(),
        ],
        budget_bytes: 25600,
        log_path: "brain/Session log.md".into(),
        sample_tasks: Vec::new(),
        passes_off: Vec::new(),
        lessons: true,
        snapshot_keep_days: None,
        log_notes: None,
    };
    (dir, ws)
}

const START: &str = "# Example brain\n\nThe shared notes for agents.\n\nrequired: brain/START HERE.md\nrequired: brain/Rules.md\nrequired: brain/Questions.md\n\nsession: s1 claude-code 2026-11-10T10:00:00Z\n";
const PROMPT: &str =
    "relevant:\n- Deploy notes: The panel deploys on merge.\n\nlinter: overdue=0 dangling=0\n";

fn block() -> Block {
    Block {
        source: Source::Record,
        start: Some(START.to_owned()),
        prompt: Some(PROMPT.to_owned()),
    }
}

#[test]
fn a_block_splits_into_parts_whose_bytes_add_up() {
    let (_d, ws) = workspace();
    let notes = Notes::scan(&ws.notes_root()).unwrap();
    let parts = parts(&block(), &ws, &notes);
    let labels: Vec<(&str, PartKind)> = parts.iter().map(|p| (p.label.as_str(), p.kind)).collect();
    assert_eq!(
        labels,
        vec![
            ("Example brain", PartKind::Required),
            ("Agent rules", PartKind::Required),
            ("Open questions", PartKind::Required),
            ("session line", PartKind::Housekeeping),
            ("Deploy notes", PartKind::Relevant),
            ("linter counts", PartKind::Housekeeping),
        ]
    );
    let total: usize = parts.iter().map(|p| p.bytes).sum();
    assert_eq!(total, START.len() + PROMPT.len());
    assert_eq!(
        parts[4].note.as_deref(),
        Some(Path::new("brain/Deploy notes.md"))
    );
    assert!(parts[1].note_bytes.unwrap() > 20);
}

#[test]
fn reads_citations_and_quotes_classify_parts() {
    let (_d, ws) = workspace();
    let notes = Notes::scan(&ws.notes_root()).unwrap();
    let resolver = Resolver::new(&ws.root, &ws.notes_dir, &notes, None);
    let mut parts = parts(&block(), &ws, &notes);
    let root = ws.root.display().to_string();
    let t = T::default()
        .user("go")
        .call(
            "Read",
            json!({"file_path": format!("{root}/brain/Rules.md")}),
        )
        .result("Read", "# Agent rules")
        .say("As Deploy notes says, the panel deploys on merge.")
        .bash("hippo-end --session s1", "ok");
    classify(&mut parts, &t.turns, &resolver, Some(&ws.root), "s1");
    let used: Vec<(&str, Use)> = parts.iter().map(|p| (p.label.as_str(), p.used)).collect();
    assert_eq!(
        used,
        vec![
            ("Example brain", Use::Ignored),
            ("Agent rules", Use::Read),
            ("Open questions", Use::Ignored),
            ("session line", Use::Read),
            ("Deploy notes", Use::Cited),
            ("linter counts", Use::Ignored),
        ]
    );
    assert_eq!(parts[1].read_at, Some(1));
    let totals = Totals::of(&parts);
    assert_eq!(totals.total, START.len() + PROMPT.len());
    assert_eq!(totals.read + totals.cited + totals.ignored, totals.total);
}

#[test]
fn the_transcript_keeps_the_hook_output_for_older_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.jsonl");
    let lines = [
        json!({"type": "attachment", "attachment": {"type": "hook_success", "hookEvent": "SessionStart", "command": "/usr/local/bin/other-tool hook", "stdout": "not hippo"}}),
        json!({"type": "attachment", "attachment": {"type": "hook_success", "hookEvent": "SessionStart", "command": "/home/x/.cargo/bin/hippo hook session-start", "stdout": START}}),
        json!({"type": "user", "message": {"content": "go"}}),
        json!({"type": "attachment", "attachment": {"type": "hook_success", "hookEvent": "UserPromptSubmit", "command": "hippo hook prompt", "stdout": PROMPT}}),
        json!({"type": "assistant", "message": {"content": "ok"}}),
    ];
    let text: Vec<String> = lines.iter().map(Value::to_string).collect();
    fs::write(&path, text.join("\n")).unwrap();
    let b = super::block(None, "claude-code", &path);
    assert_eq!(b.source, Source::Transcript);
    assert_eq!(b.start.as_deref(), Some(START));
    assert_eq!(b.prompt.as_deref(), Some(PROMPT));
    assert_eq!(
        super::block(None, "codex", &path).source,
        Source::NotRecorded
    );
}

// ---------------------------------------------------------------------------
// Searches

#[test]
fn a_miss_says_what_followed() {
    let roots = vec![PathBuf::from("/work/brain")];
    let t = T::default()
        .user("where is the sender id")
        .call(
            "Grep",
            json!({"pattern": "sender id", "path": "/work/brain"}),
        )
        .result("Grep", "No files found")
        .bash("hippo recall 'resend key'", "")
        .call("Grep", json!({"pattern": "sender", "path": "/work/app"}))
        .result("Grep", "Found 1 file\n/work/app/config/sender.toml")
        .call("Read", json!({"file_path": "/work/app/config/sender.toml"}))
        .result("Read", "id = 1")
        .say("Done.");
    let s = searches::find(&t.turns, &roots, Some(Path::new("/work")));
    assert_eq!(s.len(), 2, "{s:#?}");
    assert_eq!(s[0].hits, Some(0));
    assert_eq!(
        s[0].follow.as_deref(),
        Some("found by a later search after 3 turns")
    );
    assert_eq!(s[1].tool, "recall");
    assert_eq!(s[1].hits, Some(0));
}

#[test]
fn the_dsh_plugins_hippo_ask_is_a_search_of_the_notes() {
    let roots = vec![PathBuf::from("/work/brain")];
    let t = T::default()
        .user("where is the sender id")
        .call("hippo_ask", json!({"question": "sender id"}))
        .result(
            "hippo_ask",
            "searched 4 sessions and 2 notes for: sender id\nno session or note matched",
        )
        .say("Nothing yet.");
    let s = searches::find(&t.turns, &roots, Some(Path::new("/work")));
    assert_eq!(s.len(), 1, "{s:#?}");
    assert_eq!(s[0].tool, "ask");
    assert_eq!(s[0].query, "sender id");
}

#[test]
fn a_miss_then_a_question_was_asked_of_the_person() {
    let roots = vec![PathBuf::from("/work/brain")];
    let t = T::default()
        .bash("cd /work && grep -rn 'sender domain' brain/", "Exit code 1")
        .say("I can't find the sender domain. Which one should I use?")
        .user("use the one in the panel");
    let s = searches::find(&t.turns, &roots, Some(Path::new("/work")));
    assert_eq!(s.len(), 1, "{s:#?}");
    assert_eq!(s[0].hits, Some(0));
    assert_eq!(s[0].follow.as_deref(), Some("then asked you"));
}

#[test]
fn hits_and_used_files_are_counted() {
    let roots = vec![PathBuf::from("/work/brain")];
    let t = T::default()
        .call(
            "Grep",
            json!({"pattern": "RETRY_LIMIT", "path": "/work/brain"}),
        )
        .result("Grep", "Found 2 files\n/work/brain/a.md\n/work/brain/b.md")
        .call("Read", json!({"file_path": "/work/brain/b.md"}))
        .result("Read", "x");
    let s = searches::find(&t.turns, &roots, None);
    assert_eq!(s[0].hits, Some(2));
    assert_eq!(s[0].used, Some(1));
}

#[test]
fn relative_hit_paths_count_as_used_and_counts_alone_say_nothing() {
    let roots = vec![PathBuf::from("/work/brain")];
    let t = T::default()
        .bash(
            "cd /work && grep -rn 'sender domain' brain/",
            "brain/Billing/Email.md:12:sender domain\nbrain/Other.md:3:sender",
        )
        .call("Read", json!({"file_path": "/work/brain/Billing/Email.md"}))
        .result("Read", "x");
    let s = searches::find(&t.turns, &roots, Some(Path::new("/work")));
    assert_eq!(s[0].hits, Some(2), "{s:#?}");
    assert_eq!(s[0].used, Some(1));
    // A one-file grep prints no file names: the file it searched is the hit.
    let t = T::default()
        .bash(
            "cd /work && grep -n 'Dusk' brain/Atlas.md",
            "96:Design direction Dusk",
        )
        .bash("cd /work && sed -n 90,100p brain/Atlas.md", "x");
    let s = searches::find(&t.turns, &roots, Some(Path::new("/work")));
    assert_eq!(s[0].used, Some(1), "{s:#?}");
    // A count-only output lists no files: used is unknown, not 0.
    let t = T::default()
        .call(
            "Grep",
            json!({"pattern": "RETRY_LIMIT", "path": "/work/brain", "output_mode": "count"}),
        )
        .result("Grep", "Found 4 lines");
    let s = searches::find(&t.turns, &roots, None);
    assert_eq!(s[0].used, None);
}

#[test]
fn the_tune_tip_leads_with_the_signal_and_names_bytes_only_when_material() {
    let small = crate::xray::tune_text("Open questions", 36, 42, 45);
    assert_eq!(small, "Open questions was never opened or cited in 36 of 42 sessions this week. Making it relevant-only keeps it out of every start block; it is still offered when a task matches it.");
    let big = crate::xray::tune_text("Runbook", 9, 11, 4096);
    assert!(big.contains("start block, 4.0 KB of every block;"), "{big}");
}

// ---------------------------------------------------------------------------
// The tune action

#[test]
fn unrequire_rewrites_only_the_required_line() {
    let toml = "# comment\nnotes_dir = \"brain\"\n# `required_notes`\nrequired_notes = [\"brain/A.md\", \"brain/B.md\"]\nbudget_bytes = 1\n";
    let u = unrequire(toml, Path::new("brain/B.md")).unwrap();
    assert_eq!(
        u.before,
        "required_notes = [\"brain/A.md\", \"brain/B.md\"]"
    );
    assert_eq!(u.after, "required_notes = [\"brain/A.md\"]");
    assert_eq!(
        u.text,
        "# comment\nnotes_dir = \"brain\"\n# `required_notes`\nrequired_notes = [\"brain/A.md\"]\nbudget_bytes = 1\n"
    );
    assert!(unrequire(toml, Path::new("brain/C.md")).is_err());
    assert!(unrequire("required_notes = [\n  \"a\",\n]\n", Path::new("a")).is_err());
}

// ---------------------------------------------------------------------------
// Precision cases found on real transcripts (rewritten as synthetic ones)

#[test]
fn code_in_a_heredoc_is_not_an_edit_or_a_command() {
    // `-> Element` inside a Python heredoc is not a `>` redirect, and the
    // body's text is not shell commands.
    let t = T::default()
        .bash("cd /work/app && cargo build --release", "Finished `release` profile")
        .bash(
            "cd /work/app && python3 - <<'EOF'\nprint('fn render() -> Element { lint_notes }')\nEOF",
            "ok",
        )
        .say("Builds clean.");
    let c = one(&t.turns);
    assert_eq!(c.status, Status::Verified, "{}", c.evidence);
}

#[test]
fn a_heredoc_that_writes_a_file_is_an_edit() {
    let t = T::default()
        .bash("cd /work/app && cargo test", "test result: ok. 3 passed")
        .bash(
            "cd /work/app && python3 - <<'EOF'\np='src/lib.rs'; s=open(p).read()\nopen(p,'w').write(s)\nEOF",
            "",
        )
        .say("Tests pass.");
    let c = one(&t.turns);
    assert_eq!(c.status, Status::Unverified, "{}", c.evidence);
    assert!(c.evidence.contains("lib.rs"), "{}", c.evidence);
}

#[test]
fn an_agents_own_summary_line_is_not_a_failure() {
    let t = T::default()
        .bash(
            "cargo test 2>&1 | tail -1; echo \"passed $P failed $F\"",
            "passed 121 failed 0",
        )
        .say("App tests pass (121).");
    assert_eq!(one(&t.turns).status, Status::Verified);
}

#[test]
fn a_dry_run_or_a_version_is_not_evidence() {
    let t = T::default()
        .bash("npx wrangler deploy --dry-run", "Total Upload: 10 KiB")
        .bash("cargo clippy --version", "clippy 0.1.90")
        .say("It's deployed. Clippy clean.");
    for c in check(&t.turns) {
        assert_eq!(c.status, Status::Unverified, "{c:?}");
    }
}

#[test]
fn commands_inside_a_shell_c_script_and_after_wrappers_count() {
    let t = T::default()
        .edit("/work/app/src/lib.rs")
        .bash(
            "cd /work/app && $S/run-checks.sh zsh -c 'cargo clippy -- -D warnings | head; cargo test | grep result'",
            "test result: ok. 9 passed",
        )
        .bash("cd /work/app && time xcrun --sdk iphoneos swiftc -typecheck A.swift", "")
        .say("Clippy clean. Swift typechecks clean.");
    for c in check(&t.turns) {
        assert_eq!(c.status, Status::Verified, "{c:?}");
    }
}

#[test]
fn a_pr_number_after_a_hash_names_the_pull_request() {
    let t = T::default()
        .bash(
            "gh pr merge 95 --squash",
            "✓ Squashed and merged pull request #95",
        )
        .say("PR 1 merged (#95).");
    let c = one(&t.turns);
    assert_eq!(c.pr, Some(95));
    assert_eq!(c.status, Status::Verified, "{}", c.evidence);
}

#[test]
fn a_pull_request_merged_elsewhere_is_unclear() {
    let t = T::default()
        .bash(
            "gh pr merge 111 --squash",
            "✓ Squashed and merged pull request #111",
        )
        .say("That fix is in PR #112, which is merged.");
    let c = one(&t.turns);
    assert_eq!(c.pr, Some(112));
    assert_eq!(c.status, Status::Unclear, "{}", c.evidence);
}

#[test]
fn a_subset_run_does_not_verify_a_larger_count() {
    let t = T::default()
        .bash("cargo test -p cli schedule", "test result: ok. 5 passed")
        .say("The full test suite passes (230 tests).");
    let c = one(&t.turns);
    assert_eq!(c.status, Status::Unclear, "{}", c.evidence);
}

#[test]
fn a_background_scripts_log_that_shows_the_pass_verifies() {
    // A run's output read later counts.
    let t = T::default()
        .bash("nohup bash ci.sh > ci.log 2>&1 &", "")
        .bash("grep STEP ci.log", "STEP fmt ok\nSTEP clippy ok")
        .say("Formatting passes.");
    let c = one(&t.turns);
    assert_eq!(c.status, Status::Verified, "{}", c.evidence);
    assert!(c.evidence.contains("fmt ok"), "{}", c.evidence);
}

#[test]
fn a_summarised_conversation_makes_missing_evidence_unclear() {
    let t = T::default()
        .user(
            "This session is being continued from a previous conversation that ran out of context.",
        )
        .say("The branch is pushed.");
    assert_eq!(one(&t.turns).status, Status::Unclear);
}

#[test]
fn more_sentences_that_are_not_claims() {
    for text in [
        "I'm removing the two merged worktrees and their build output (clean up after).",
        "A batch for forward deployed engineer roles is running now.",
        "\"deployed\" is the usual unverifiable one.",
        "Both heads match merged #45 and #43.",
        "The feed agent fixed the guide and pushed it.",
        "Two other agents merged #21 and #22.",
        "The buttons are live.",
    ] {
        assert!(claims_in(text).is_empty(), "{text}: {:?}", claims_in(text));
    }
}

#[test]
fn ci_green_with_only_local_checks_is_unclear() {
    let t = T::default()
        .bash("cargo test", "test result: ok. 4 passed")
        .say("App CI is green at this state.");
    assert_eq!(one(&t.turns).status, Status::Unclear);
}

#[test]
fn a_lock_wrapper_and_a_remote_command_count() {
    let t = T::default()
        .bash(
            "cd /work/app && lockf -k /tmp/job.lock npm run typecheck >/dev/null 2>&1; echo typecheck=$?",
            "typecheck=0",
        )
        .bash("lockf -t 3600 /tmp/job.lock git push origin HEAD:feat/x", "To github.com:o/r.git")
        .bash("runner exec job_1 --cwd /home/user/app -- go test ./...", "ok  example/app  0.4s")
        .say("Typecheck is clean. Branch pushed. The Go tests pass.");
    for c in check(&t.turns) {
        assert_eq!(c.status, Status::Verified, "{c:?}");
    }
}

#[test]
fn narration_instructions_and_partial_results_are_not_claims() {
    for text in [
        "You then tap the line, the fix types itself in and the tests turn green.",
        "Check that it still builds and its tests pass.",
        "Most tests pass.",
        "Tests passing for the wrong reason: I found none.",
    ] {
        assert!(claims_in(text).is_empty(), "{text}: {:?}", claims_in(text));
    }
}

#[test]
fn a_mutation_test_restored_from_a_backup_leaves_the_earlier_pass_standing() {
    let t = T::default()
        .bash("cd /work/app && npx vitest run", "Tests  474 passed (474)")
        .bash(
            "cd /work/app && cp src/a.ts /tmp/a.ts.bak && sed -i '' 's/x/y/' src/a.ts && npx vitest run; cp /tmp/a.ts.bak src/a.ts",
            "Tests  4 failed | 470 passed (474)",
        )
        .say("Vitest: 474 passed.");
    assert_eq!(one(&t.turns).status, Status::Verified);
}

#[test]
fn node_test_and_a_live_check_after_a_failed_publish() {
    let t = T::default()
        .bash("node --test tests/a.test.mts", "ℹ pass 2\nℹ fail 0")
        .bash(
            "npm publish --access public",
            "Exit code 1\nnpm error code E403",
        )
        .bash(
            "node cli.js setup && curl -s https://example.org/",
            "<html>",
        )
        .say("Tests pass. Everything is deployed.");
    let c = check(&t.turns);
    assert_eq!(c[0].status, Status::Verified, "{:?}", c[0]);
    assert_eq!(c[1].status, Status::Unclear, "{:?}", c[1]);
}

#[test]
fn a_script_run_in_the_same_call_as_the_edit_is_unclear() {
    let t = T::default()
        .bash("cd /work/app && npm run build", "built")
        .bash(
            "cd /work/app && python3 - <<'EOF'\np='src/a.ts'; s=open(p).read(); open(p,'w').write(s)\nEOF\nlockf -k /tmp/l bash /tmp/gates.sh; echo finished",
            "Command running in background with ID: b1",
        )
        .say("Build passes with the fix.");
    assert_eq!(one(&t.turns).status, Status::Unclear);
}

#[test]
fn plans_and_ci_reports_are_not_local_claims() {
    for text in [
        "One more typecheck, lint and UX pass over the moved file, then commit.",
        "In CI, typecheck and lint have passed and unit tests are running.",
    ] {
        assert!(claims_in(text).is_empty(), "{text}: {:?}", claims_in(text));
    }
}

#[test]
fn a_stated_count_needs_a_run_that_shows_it() {
    let t = T::default()
        .bash("cd /work/app && go test -short ./api", "")
        .say("All 5,878 api tests pass.");
    let c = one(&t.turns);
    assert_eq!(c.status, Status::Unclear, "{}", c.evidence);
    assert!(c.evidence.contains("shows no count"));
}

#[test]
fn the_fleet_averages_its_recorded_blocks_and_their_ignored_share() {
    let mut f = Fleet::default();
    assert_eq!(f.avg_block(), None);
    assert_eq!(f.ignored_percent(), None);
    f.recorded = 4;
    f.block_bytes = 8_000;
    f.ignored_bytes = 4_960;
    assert_eq!(f.avg_block(), Some(2_000));
    assert_eq!(f.ignored_percent(), Some(62));
}

// ---------------------------------------------------------------------------
// The claims rule by cause, on synthetic transcripts only.

fn status_of(c: &[Claim], kind: ClaimKind) -> Status {
    c.iter()
        .find(|c| c.kind == kind)
        .unwrap_or_else(|| panic!("no {kind:?} claim in {c:#?}"))
        .status
}

#[test]
fn cause_1_ci_on_the_pushed_head_counts_as_the_run() {
    let base = || {
        T::default()
            .bash("cd /work/app && npx tsc --noEmit", "(no output)")
            .edit("/work/app/src/a.ts")
            .bash(
                "cd /work/app && git push origin HEAD",
                "To github.com:o/r.git",
            )
    };
    let t = base()
        .bash(
            "gh pr checks 12",
            "typecheck\tpass\t1m2s\thttps://github.com/o/r/actions/runs/1\ntest\tpass\t3m\thttps://github.com/o/r/actions/runs/1",
        )
        .say("Typecheck is clean.");
    let c = one(&t.turns);
    assert_eq!(c.status, Status::Verified, "{}", c.evidence);
    assert!(
        c.evidence.starts_with("CI passed on the pushed head"),
        "{}",
        c.evidence
    );

    // A row named for the kind passes while another is still pending.
    let t = base()
        .bash(
            "gh pr checks 12",
            "typecheck\tpass\t1m\thttps://x\ntest\tpending\t0\thttps://x",
        )
        .say("Typecheck is clean.");
    assert_eq!(one(&t.turns).status, Status::Verified);
    let t = base()
        .bash(
            "gh pr checks 12",
            "typecheck\tpass\t1m\thttps://x\ntest\tpending\t0\thttps://x",
        )
        .say("All tests pass.");
    assert_eq!(one(&t.turns).status, Status::Unverified);

    // Honest the other way: an edit after the push is not on the head CI
    // checked.
    let t = base()
        .edit("/work/app/src/b.ts")
        .bash(
            "gh pr checks 12",
            "All checks were successful\ntypecheck\tpass\t1m\thttps://x",
        )
        .say("Typecheck is clean.");
    assert_eq!(one(&t.turns).status, Status::Unverified);
}

#[test]
fn cause_2_a_wrapper_a_remote_run_and_a_background_task_count_by_their_output() {
    let t = T::default()
        .edit("/work/app/src/a.ts")
        .bash(
            "/work/bin/run-checks web",
            "tsc rc=0\nlint: clean\n43 tests passed",
        )
        .say("Web: typecheck clean, lint clean, 43 tests pass.");
    for c in check(&t.turns) {
        assert_eq!(c.status, Status::Verified, "{c:?}");
    }
    let t = T::default()
        .edit("/work/app/src/a.ts")
        .bash("nohup ./scripts/race.sh > /tmp/race.log 2>&1 &", "(no output)")
        .user("<task-notification><task-id>b3</task-id><status>completed</status><result>test result: ok. 15 passed; 0 failed</result></task-notification>")
        .say("All 15 race tests pass.");
    let c = one(&t.turns);
    assert_eq!(c.status, Status::Verified, "{}", c.evidence);
    // A wrapper's output that shows a failure is not a pass.
    let t = T::default()
        .edit("/work/app/src/a.ts")
        .bash(
            "/work/bin/run-checks web",
            "test result: FAILED. 3 passed; 1 failed",
        )
        .say("Tests pass.");
    assert_ne!(one(&t.turns).status, Status::Verified);
}

#[test]
fn cause_3_commands_in_one_call_are_read_in_order_and_a_build_script_is_a_build() {
    let t = T::default()
        .bash("cd /work/app && npx eslint src", "(no output)")
        .bash(
            "cd /work/app && sed -i '' 's/var /let /' src/x.ts && npx eslint src/x.ts",
            "(no output)",
        )
        .say("Lint is clean.");
    assert_eq!(one(&t.turns).status, Status::Verified);
    let t = T::default()
        .bash(
            "cd /work/app && npx tsc --noEmit && sed -i '' 's/a/b/' src/x.ts",
            "(no output)",
        )
        .say("Typecheck is clean.");
    let c = one(&t.turns);
    assert_eq!(c.status, Status::Unverified, "{}", c.evidence);
    assert!(
        c.evidence.contains("before the last edit"),
        "{}",
        c.evidence
    );
    let t = T::default()
        .edit("/work/app/docs/site/page.ts")
        .bash(
            "cd /work/app && node docs/site/build.mjs",
            "31 modules, 31 pages",
        )
        .say("The build passes.");
    assert_eq!(one(&t.turns).status, Status::Verified);
}

#[test]
fn cause_4_plans_quotes_other_repositories_and_pass_as_a_noun_are_not_claims() {
    for text in [
        "Running a baseline typecheck, lint and test pass on the untouched branch.",
        "Acme's fork is the strongest find so far: its test suite passed on the old system.",
        "\"Run the tests and make sure they pass.\"",
        "One more test pass over the moved file.",
        "Checks: web job still running (typecheck and lint passed, unit tests in progress).",
        "The upstream build passes on their CI.",
    ] {
        assert!(claims_in(text).is_empty(), "{text}: {:?}", claims_in(text));
    }
    // Still claims.
    for text in ["The tests pass.", "All 12 tests passed.", "Build passes."] {
        assert!(!claims_in(text).is_empty(), "{text}");
    }
}

#[test]
fn cause_5_runs_and_edits_are_matched_to_the_place_the_claim_names() {
    // A pass elsewhere does not verify the web claim.
    let t = T::default()
        .bash("cd /work/api && go test ./...", "ok  example/api  0.3s")
        .bash("cd /work/web && npx vitest run", "Tests  40 passed (40)")
        .edit("/work/web/src/a.ts")
        .say("Web tests pass.");
    assert_eq!(one(&t.turns).status, Status::Unverified);
    // A stale run elsewhere does not make the backend claim unverified
    // when CI passed on the backend's pushed head.
    let t = T::default()
        .bash("cd /work/web && npx vitest run", "Tests  40 passed (40)")
        .edit("/work/web/src/a.ts")
        .edit("/work/backend/h.go")
        .bash(
            "cd /work/backend && git push origin HEAD",
            "To github.com:o/b.git",
        )
        .bash(
            "cd /work/backend && gh pr checks 5",
            "Vet and test\tpass\t2m54s\thttps://x",
        )
        .say("Backend tests pass.");
    let c = one(&t.turns);
    assert_eq!(c.status, Status::Verified, "{}", c.evidence);
}

#[test]
fn cause_6_comment_only_and_docs_data_edits_do_not_make_a_check_stale() {
    let comment = |t: T| {
        t.call(
            "Edit",
            json!({"file_path": "/work/app/src/a.ts", "old_string": "// retry twice\nlet n = 2;", "new_string": "// retry twice, then give up\nlet n = 2;"}),
        )
        .result("Edit", "The file has been updated.")
    };
    let t = comment(T::default().bash("cd /work/app && npx tsc --noEmit", "(no output)"))
        .say("Typecheck is clean.");
    assert_eq!(one(&t.turns).status, Status::Verified);
    let t = T::default()
        .bash("cd /work/app && npx tsc --noEmit", "(no output)")
        .bash(
            "cd /work/app && sed -i '' '12i // eslint-disable-next-line no-console' src/a.ts",
            "(no output)",
        )
        .say("Typecheck is clean.");
    assert_eq!(one(&t.turns).status, Status::Verified);
    let t = T::default()
        .bash("cd /work/app && npx vitest run", "Tests  9 passed (9)")
        .edit("/work/app/docs/modules/labs.json")
        .say("Tests pass.");
    assert_eq!(one(&t.turns).status, Status::Verified);
    let t = T::default()
        .bash("cd /work/app && go build ./...", "(no output)")
        .edit("/work/app/svc/stock_test.go")
        .say("Build passes.");
    assert_eq!(one(&t.turns).status, Status::Verified);
}

#[test]
fn cause_6_the_exit_code_that_counts_is_the_checks_own() {
    let t = T::default()
        .bash(
            "cd /work/app && npx tsc --noEmit && npx eslint src; ls .husky",
            "Exit code 1\nls: .husky: No such file or directory",
        )
        .say("Lint and typecheck are clean.");
    let c = check(&t.turns);
    assert_eq!(status_of(&c, ClaimKind::Lint), Status::Verified, "{c:#?}");
    assert_eq!(
        status_of(&c, ClaimKind::Typecheck),
        Status::Verified,
        "{c:#?}"
    );
    // Its own failure still counts.
    let t = T::default()
        .bash(
            "cd /work/app && npx eslint src; ls .husky",
            "Exit code 1\n✖ 3 problems (3 errors, 0 warnings)",
        )
        .say("Lint is clean.");
    assert_eq!(one(&t.turns).status, Status::Unverified);
}

#[test]
fn cause_6_a_claim_re_checked_soon_after_is_not_flagged() {
    let t = T::default()
        .bash("cd /work/app && npx tsc --noEmit", "(no output)")
        .edit("/work/app/src/a.ts")
        .say("Screens wired, typecheck clean.")
        .bash("cd /work/app && npx tsc --noEmit", "(no output)");
    let c = one(&t.turns);
    assert_eq!(c.status, Status::Verified, "{}", c.evidence);
    assert!(c.evidence.ends_with("after the claim"), "{}", c.evidence);
    // Not when the person spoke first, or code changed in between.
    let t = T::default()
        .bash("cd /work/app && npx tsc --noEmit", "(no output)")
        .edit("/work/app/src/a.ts")
        .say("Typecheck clean.")
        .user("ok, and the docs?")
        .bash("cd /work/app && npx tsc --noEmit", "(no output)");
    assert_eq!(one(&t.turns).status, Status::Unverified);
    let t = T::default()
        .bash("cd /work/app && npx tsc --noEmit", "(no output)")
        .edit("/work/app/src/a.ts")
        .say("Typecheck clean.")
        .edit("/work/app/src/b.ts")
        .bash("cd /work/app && npx tsc --noEmit", "(no output)");
    assert_eq!(one(&t.turns).status, Status::Unverified);
}

#[test]
fn still_honest_an_edit_to_code_or_a_test_after_the_run_and_a_check_never_run_flag() {
    let t = T::default()
        .bash("cd /work/app && npx vitest run", "Tests  9 passed (9)")
        .edit("/work/app/src/a.test.ts")
        .say("Tests pass.");
    assert_eq!(one(&t.turns).status, Status::Unverified);
    let t = T::default()
        .bash("cd /work/app && go test ./...", "ok  example/app  0.3s")
        .edit("/work/app/svc/stock_test.go")
        .say("Tests pass.");
    assert_eq!(one(&t.turns).status, Status::Unverified);
    let t = T::default()
        .edit("/work/app/src/a.ts")
        .bash("cd /work/app && npx tsc --noEmit", "(no output)")
        .say("Typecheck and lint are clean.");
    let c = check(&t.turns);
    assert_eq!(status_of(&c, ClaimKind::Typecheck), Status::Verified);
    assert_eq!(status_of(&c, ClaimKind::Lint), Status::Unverified);
}

#[test]
fn a_notes_check_is_any_projects_notes_linter() {
    let t = T::default()
        .bash("hippo lint", "0 errors")
        .say("The notes check passes.");
    assert_eq!(one(&t.turns).status, Status::Verified);
    let t = T::default()
        .bash("python3 tools/check_notes.py", "ok")
        .say("The notes check is clean.");
    assert_eq!(one(&t.turns).status, Status::Verified);
}

#[test]
fn cause_2_a_saved_wrapper_script_is_read_where_it_runs() {
    // A script saved with `cat > file <<EOF` and run through a wrapper
    // (`lockf … /tmp/web-checks.sh`): its commands count, in the folder it
    // moves to.
    let save = "cat > /tmp/web-checks.sh <<'EOF'\n#!/bin/sh\ncd /work/web\necho \"== typecheck\"; npx tsc --noEmit 2>&1 | tail -5\necho \"== lint\"; npx eslint src 2>&1 | tail -5\nEOF\nchmod +x /tmp/web-checks.sh";
    let t = T::default()
        .bash("cd /work/web && npx tsc --noEmit", "(no output)")
        .edit("/work/web/src/a.ts")
        .bash(
            &format!("{save} && lockf -t 600 /tmp/job.lock /tmp/web-checks.sh"),
            "== typecheck\n== lint",
        )
        .say("Typecheck and lint are clean.");
    for c in check(&t.turns) {
        assert_eq!(c.status, Status::Verified, "{c:?}");
    }
    // Saved in one call, run in a later one.
    let t = T::default()
        .bash(save, "")
        .edit("/work/web/src/a.ts")
        .bash("sh /tmp/web-checks.sh", "== typecheck\n== lint")
        .say("Typecheck is clean.");
    assert_eq!(one(&t.turns).status, Status::Verified);
    // A saved program edits files when it runs, not when it is written.
    let fix = "cat > /tmp/fix.py <<'EOF'\np='/work/web/src/a.ts'\ns=open(p).read()\nopen(p,'w').write(s.replace('let a = 1', 'let a = 2'))\nEOF";
    let t = T::default()
        .bash(fix, "")
        .bash("cd /work/web && npx tsc --noEmit", "(no output)")
        .say("Typecheck is clean.")
        .bash("python3 /tmp/fix.py", "");
    assert_eq!(one(&t.turns).status, Status::Verified);
    let t = T::default()
        .bash(fix, "")
        .bash("cd /work/web && npx tsc --noEmit", "(no output)")
        .bash("python3 /tmp/fix.py", "")
        .say("Typecheck is clean.");
    assert_eq!(one(&t.turns).status, Status::Unverified);
}

#[test]
fn cause_6_a_program_that_rewrites_only_comments_does_not_make_a_check_stale() {
    let comment = "python3 - <<'EOF'\np='/work/app/src/a.go'\ns=open(p).read()\nold='''\\t// The lock lets one row through.\\n\\tx := lock()'''\nnew='''\\t// One row is written at a time.\\n\\tx := lock()'''\nassert old in s\nopen(p,'w').write(s.replace(old,new))\nEOF";
    let code = "python3 - <<'EOF'\np='/work/app/src/a.go'\ns=open(p).read()\nold='''\\tx := lock()'''\nnew='''\\tx := lockAll()'''\nopen(p,'w').write(s.replace(old,new))\nEOF";
    let insert = "python3 - <<'EOF'\nimport pathlib\np = pathlib.Path('/work/app/src/a.ts')\nlines = p.read_text().split('\\n')\nassert 'u001b' in lines[4]\nlines.insert(4, '  // eslint-disable-next-line no-control-regex')\np.write_text('\\n'.join(lines))\nEOF";
    for (script, verified) in [(comment, true), (insert, true), (code, false)] {
        let t = T::default()
            .bash("cd /work/app && go vet ./...", "(no output)")
            .bash(&format!("cd /work/app && {script}"), "")
            .say("Lint is clean.");
        let c = one(&t.turns);
        assert_eq!(
            c.status == Status::Verified,
            verified,
            "{script}: {}",
            c.evidence
        );
    }
}

#[test]
fn cause_1_a_ci_read_in_the_background_and_a_status_rollup_count() {
    let base = || {
        T::default()
            .bash("cd /work/app && npx tsc --noEmit", "(no output)")
            .edit("/work/app/src/a.ts")
            .bash(
                "cd /work/app && git push origin HEAD",
                "To github.com:o/r.git",
            )
    };
    // Started in the background, its output read later by the task's id.
    let t = base()
        .bash(
            "until [ \"$(gh run view 7 --json status -q .status)\" = completed ]; do sleep 20; done; gh run view 7 --json conclusion -q .conclusion",
            "Command running in background with ID: bq7xk2m. Output is being written to: /tmp/tasks/bq7xk2m.output",
        )
        .bash("cat /tmp/tasks/bq7xk2m.output", "success\n\n[exited with code 0]")
        .say("Typecheck is clean.");
    let c = one(&t.turns);
    assert_eq!(c.status, Status::Verified, "{}", c.evidence);
    // A status rollup reshaped by `--jq`.
    let t = base()
        .bash(
            "gh pr view 12 --json headRefOid,statusCheckRollup -q '{c:[.statusCheckRollup[]|{n:.name,s:(.conclusion // .status)}]}'",
            "{\"c\":[{\"n\":\"web\",\"s\":\"SUCCESS\"},{\"n\":\"rust\",\"s\":\"SKIPPED\"}]}",
        )
        .say("Typecheck is clean.");
    assert_eq!(one(&t.turns).status, Status::Verified);
    let t = base()
        .bash(
            "gh pr view 12 --json statusCheckRollup -q '{c:[.statusCheckRollup[]|{n:.name,s:.status}]}'",
            "{\"c\":[{\"n\":\"web\",\"s\":\"IN_PROGRESS\"}]}",
        )
        .say("Typecheck is clean.");
    assert_eq!(one(&t.turns).status, Status::Unverified);
}

#[test]
fn cause_4_a_run_meant_to_fail_first_is_not_a_claim_that_tests_pass() {
    assert!(claims_in(
        "Fail-first is recorded: the new tests fail on behaviour. The signature tests already pass, because the check exists."
    )
    .is_empty());
    assert!(!claims_in("The signature tests already pass.").is_empty());
}

#[test]
fn cause_5_a_sentence_of_several_checks_is_about_where_all_of_them_ran() {
    // Lint ran in both repositories, typecheck only in the web one: the
    // web's lint is what the sentence claims, and an edit after it flags.
    let t = T::default()
        .bash("cd /work/web && npx eslint src", "(no output)")
        .edit("/work/web/src/a.ts")
        .bash("cd /work/web && npx tsc --noEmit", "(no output)")
        .bash("cd /work/api && go vet ./...", "(no output)")
        .say("Typecheck and lint are clean.");
    assert_eq!(
        status_of(&check(&t.turns), ClaimKind::Typecheck),
        Status::Verified
    );
    assert_eq!(
        status_of(&check(&t.turns), ClaimKind::Lint),
        Status::Unverified
    );
    // One check alone still counts wherever it ran.
    let t = T::default()
        .bash("cd /work/web && npx eslint src", "(no output)")
        .edit("/work/web/src/a.ts")
        .bash("cd /work/api && go vet ./...", "(no output)")
        .say("Lint is clean.");
    assert_eq!(one(&t.turns).status, Status::Verified);
}

#[test]
fn a_program_writing_a_note_that_quotes_code_paths_edits_only_the_note() {
    let note = "python3 - <<'EOF'\np='/work/notes/Run.md'\ntext='''---\nsources:\n  - \"reports/run-1/results.json\"\n  - \"src/lib/a.ts\"\n---\nThe run passed.\n'''\nopen(p,'w').write(text)\nEOF";
    let t = T::default()
        .bash("cd /work/app && npx tsc --noEmit", "(no output)")
        .bash(&format!("cd /work/app && {note}"), "")
        .say("Typecheck is clean.");
    let c = one(&t.turns);
    assert_eq!(c.status, Status::Verified, "{}", c.evidence);
}

#[test]
fn cause_1_a_ci_read_that_failed_does_not_count() {
    // The call ended with an error before it showed this repository's
    // checks; what it printed is another pull request's.
    let t = T::default()
        .bash("cd /work/app && npx tsc --noEmit", "(no output)")
        .edit("/work/app/src/a.ts")
        .bash(
            "cd /work/app && git push origin HEAD",
            "To github.com:o/app.git",
        )
        .bash(
            "gh pr checks 9 -R o/api; echo ===; gh pr checks 12 -R o/app",
            "Exit code 1\nVet and test\tpass\t2m\thttps://x\n(eval):1: == not found",
        )
        .say("Typecheck is clean.");
    assert_eq!(one(&t.turns).status, Status::Unverified);
}

#[test]
fn cause_5_a_place_is_one_where_checks_ran_not_any_command() {
    // `design` names the folder a push ran in, not a check: every check
    // still counts.
    let t = T::default()
        .bash("cd /work/web && npx eslint src", "(no output)")
        .bash("cd /work/web-design-notes && git push -q origin HEAD", "")
        .say("Web: eslint clean and the design check is up to date.");
    assert_eq!(one(&t.turns).status, Status::Verified);
}

#[test]
fn a_log_written_after_a_run_does_not_make_it_stale() {
    let t = T::default()
        .bash(
            "cd /work/app && cargo test 2>&1 | tee test.log; echo done >> test.out",
            "test result: ok. 9 passed; 0 failed",
        )
        .say("All 9 tests pass.");
    assert_eq!(one(&t.turns).status, Status::Verified);
}

#[test]
fn checks_after_putting_work_aside_count_for_the_committed_state() {
    // `git stash push` first, then the checks: a run on the committed
    // state, not a mutation test.
    let t = T::default()
        .bash("cd /work/app && cargo fmt --all", "")
        .edit("/work/app/src/ui.rs")
        .bash(
            "cd /work/app && git stash push -q -- src/ui.rs && cargo fmt --all --check && echo FMT_OK",
            "FMT_OK",
        )
        .say("At the commit, fmt is clean.");
    assert_eq!(one(&t.turns).status, Status::Verified);
    // A stash popped after the checks is still a mutation test.
    let t = T::default()
        .bash("cd /work/app && npx vitest run", "Tests  9 passed (9)")
        .bash(
            "cd /work/app && sed -i '' 's/x/y/' src/a.ts && npx vitest run; git stash pop",
            "Tests  2 failed | 7 passed (9)",
        )
        .say("All 9 tests pass.");
    assert_eq!(one(&t.turns).status, Status::Verified);
}
