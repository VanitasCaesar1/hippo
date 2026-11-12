//! Drives `hippo packs check` over pack directories made in temporary
//! directories. The command reads no configuration, so `HIPPO_HOME` and
//! `HOME` point at empty temporary directories.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

const CLOSING: &str = "Respond with only a JSON array of objects `{note, kind, old, new, evidence: [{session, ordinal}], reason}` as your final message; an empty array means no proposals.";

fn write(dir: &Path, path: &str, text: &str) {
    let path = dir.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn minimal_pack(dir: &Path) {
    write(
        dir,
        "pack.toml",
        "protocol = \"HIPPO_V1\"\nname = \"glossary-terms\"\nversion = \"1.0.0\"\n\
         description = \"Adds terms to the glossary.\"\nauthors = [\"Ada\"]\nlicense = \"MIT\"\n",
    );
    write(dir, "README.md", "Glossary pack.\n");
    write(
        dir,
        "passes/glossary-terms.toml",
        "protocol = \"HIPPO_V1\"\nprompt = \"prompts/glossary-terms.md\"\nreads = [\"notes\"]\n\
         reviewer = \"personas/glossary-terms.md\"\nper_note = true\n",
    );
    write(
        dir,
        "prompts/glossary-terms.md",
        &format!("Notes: {{{{notes}}}}\n\n{CLOSING}\n"),
    );
    write(
        dir,
        "personas/glossary-terms.md",
        "## Role\nr\n## What you receive\nw\n## Rules\nr\n## Reply\n\
         Reply with `{\"verdict\": \"confirm\"|\"reject\", \"notes\": \"n\"}`.\n",
    );
}

fn check(dir: &Path) -> Output {
    let home = tempfile::tempdir().unwrap();
    let user = tempfile::tempdir().unwrap();
    Command::new(env!("CARGO_BIN_EXE_hippo"))
        .args(["packs", "check"])
        .arg(dir)
        .env("HIPPO_HOME", home.path())
        .env("HOME", user.path())
        .output()
        .unwrap()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn a_valid_minimal_pack_prints_ok() {
    let dir = tempfile::tempdir().unwrap();
    minimal_pack(dir.path());
    write(dir.path(), ".DS_Store", "x");
    let out = check(dir.path());
    assert!(out.status.success(), "{}", stdout(&out));
    assert_eq!(stdout(&out), "skipped .DS_Store\nok glossary-terms 1.0.0\n");
}

#[test]
fn a_persona_with_a_hidden_code_point_fails_with_its_position() {
    let dir = tempfile::tempdir().unwrap();
    minimal_pack(dir.path());
    write(
        dir.path(),
        "personas/glossary-terms.md",
        "## Role\nr\n## What you receive\nw\n## Rules\nr\n\n## Reply \u{202E}\n",
    );
    let out = check(dir.path());
    assert_eq!(out.status.code(), Some(1));
    let text = stdout(&out);
    assert!(
        text.contains("error personas/glossary-terms.md:8:10: U+202E\n"),
        "{text}"
    );
    assert!(!text.contains("ok "), "{text}");
}

#[test]
fn warnings_do_not_fail_the_check() {
    let dir = tempfile::tempdir().unwrap();
    minimal_pack(dir.path());
    write(dir.path(), "templates/a.md", "see https://example.com\n");
    let out = check(dir.path());
    assert!(out.status.success());
    assert_eq!(
        stdout(&out),
        "warning templates/a.md: a URL outside README.md\nok glossary-terms 1.0.0\n"
    );
}

#[cfg(unix)]
#[test]
fn a_link_in_the_pack_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    minimal_pack(dir.path());
    std::os::unix::fs::symlink("/etc/hosts", dir.path().join("x.md")).unwrap();
    let out = check(dir.path());
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        stdout(&out),
        "error x.md: a symbolic link; packs may not contain links\n"
    );
}

#[test]
fn control_characters_in_names_cannot_start_a_line() {
    let dir = tempfile::tempdir().unwrap();
    minimal_pack(dir.path());
    write(dir.path(), "templates/a\nok x 1.0.0", "x\n");
    write(dir.path(), ".hidden\nok y 2.0.0\u{1b}[2J", "x\n");
    let out = check(dir.path());
    assert_eq!(out.status.code(), Some(1));
    let text = stdout(&out);
    assert!(text.contains("\\u{a}ok x 1.0.0"), "{text}");
    assert!(
        text.contains("skipped .hidden\\u{a}ok y 2.0.0\\u{1b}[2J"),
        "{text}"
    );
    assert!(!text.contains('\u{1b}'), "{text}");
    assert!(text.lines().all(|line| !line.starts_with("ok ")), "{text}");
}

/// A pack with one note filter and one case: the session touches the
/// glossary and an unrelated note, and the stub model proposes one entry.
fn case_pack(dir: &Path, expect: &str) {
    minimal_pack(dir);
    write(
        dir,
        "passes/glossary-terms.toml",
        "protocol = \"HIPPO_V1\"\nprompt = \"prompts/glossary-terms.md\"\n\
         reads = [\"sessions\", \"notes\", \"turns\"]\nreviewer = \"personas/glossary-terms.md\"\n\
         per_note = true\nnote_paths = [\"**/Glossary*.md\"]\n",
    );
    write(
        dir,
        "prompts/glossary-terms.md",
        &format!(
            "Sessions: {{{{sessions}}}}\nNotes: {{{{notes}}}}\nTurns: {{{{turns}}}}\n\n{CLOSING}\n"
        ),
    );
    write(
        dir,
        "tests/one/case.toml",
        &format!(
            r#"protocol = "HIPPO_V1"
pass = "glossary-terms"
about = "A term defined in a session is appended to the glossary"

[[session]]
id = "glossary-1"
notes = ["notes/Glossary.md", "notes/Misc.md"]
handoff = "Defined the retry budget; see [[Glossary]]."

[model]
reply = '''[{{"note": "notes/Glossary.md", "kind": "append", "old": "", "new": "\n- Retry budget: the retries a sender may make.\n", "evidence": [{{"session": "glossary-1", "ordinal": 1}}], "reason": "Turn 1 defines it."}}]'''

[expect]
{expect}
"#
        ),
    );
    write(
        dir,
        "tests/one/notes/Glossary.md",
        "# Glossary\n\nTerms of the example sender.\n",
    );
    write(
        dir,
        "tests/one/notes/Misc.md",
        "# Misc\n\nUnrelated-llama-text.\n",
    );
    write(
        dir,
        "tests/one/transcripts/t.jsonl",
        concat!(
            "{\"synthetic\": true}\n",
            "{\"type\":\"user\",\"sessionId\":\"glossary-1\",\"timestamp\":\"2026-11-01T10:00:00.000Z\",",
            "\"message\":{\"role\":\"user\",\"content\":\"Add the retry budget to the Glossary note.\"}}\n",
            "{\"type\":\"assistant\",\"sessionId\":\"glossary-1\",\"timestamp\":\"2026-11-01T10:00:05.000Z\",",
            "\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",",
            "\"text\":\"The retry budget is the number of retries a sender may make.\"}]}}\n",
        ),
    );
}

const PASSING: &str = "calls = 1\nproposals = 1\nconfirmed = 1\napplied = 1\n\
    prompt_contains = [\"Glossary\"]\nprompt_excludes = [\"Unrelated-llama-text\"]\n\
    \n[[expect.note]]\npath = \"notes/Glossary.md\"\ncontains = [\"Retry budget\"]\n";

fn run_test(dir: &Path, extra: &[&str], decoy: bool) -> Output {
    let home = tempfile::tempdir().unwrap();
    let user = tempfile::tempdir().unwrap();
    if decoy {
        write(
            user.path(),
            ".claude/projects/-decoy/decoy-1.jsonl",
            concat!(
                "{\"type\":\"user\",\"sessionId\":\"decoy-1\",\"timestamp\":\"2026-11-01T10:00:00.000Z\",",
                "\"message\":{\"role\":\"user\",\"content\":\"Glossary decoy-zebra-text\"}}\n"
            ),
        );
    }
    Command::new(env!("CARGO_BIN_EXE_hippo"))
        .args(["packs", "test"])
        .arg(dir)
        .args(extra)
        .env("HIPPO_HOME", home.path())
        .env("HOME", user.path())
        .output()
        .unwrap()
}

#[test]
fn a_case_that_passes_prints_ok() {
    let dir = tempfile::tempdir().unwrap();
    case_pack(dir.path(), PASSING);
    let out = run_test(dir.path(), &[], false);
    assert!(out.status.success(), "{}", stdout(&out));
    assert_eq!(stdout(&out), "ok glossary-terms/one\n");
    let out = run_test(dir.path(), &["--case", "one"], false);
    assert_eq!(stdout(&out), "ok glossary-terms/one\n");
    let out = run_test(dir.path(), &["--case", "none"], false);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stdout(&out), "error no case named none\n");
}

#[test]
fn a_failed_expectation_prints_one_fail_line() {
    let dir = tempfile::tempdir().unwrap();
    case_pack(dir.path(), &PASSING.replace("applied = 1", "applied = 0"));
    let out = run_test(dir.path(), &[], false);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        stdout(&out),
        "FAIL glossary-terms/one: applied: expected 0, got 1\n"
    );
}

#[test]
fn a_pack_with_an_error_is_not_run() {
    let dir = tempfile::tempdir().unwrap();
    case_pack(dir.path(), PASSING);
    write(dir.path(), "extra.txt", "x");
    let out = run_test(dir.path(), &[], false);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stdout(&out), "error unexpected file extra.txt\n");
}

#[test]
fn the_run_does_not_index_the_users_transcripts() {
    let dir = tempfile::tempdir().unwrap();
    case_pack(
        dir.path(),
        &PASSING.replace(
            "[\"Unrelated-llama-text\"]",
            "[\"Unrelated-llama-text\", \"decoy-zebra-text\"]",
        ),
    );
    let out = run_test(dir.path(), &[], true);
    assert!(out.status.success(), "{}", stdout(&out));
    assert_eq!(stdout(&out), "ok glossary-terms/one\n");
}

#[test]
fn the_stub_model_logs_the_prompt_and_replies_with_the_file() {
    use std::io::Write as _;
    use std::process::Stdio;
    let dir = tempfile::tempdir().unwrap();
    let reply = dir.path().join("reply.txt");
    let log = dir.path().join("log");
    fs::write(&reply, "[]").unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_hippo"))
        .args(["packs", "stub-model", "--reply"])
        .arg(&reply)
        .arg("--log")
        .arg(&log)
        .args(["--system-prompt", "sys"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"hello").unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(stdout(&out), "{\"result\":\"[]\"}\n");
    assert_eq!(
        fs::read_to_string(&log).unwrap(),
        "{\"input\":\"hello\",\"system_prompt\":\"sys\"}\n"
    );
}

// ---------------------------------------------------------------------
// hippo add and hippo packs list, diff, update and remove
// ---------------------------------------------------------------------

const STUB: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/stub-model/model.sh"
);

/// A hippo home, a workspace and a directory to run in, each temporary.
struct Setup {
    home: tempfile::TempDir,
    user: tempfile::TempDir,
    work: tempfile::TempDir,
    workspace: tempfile::TempDir,
}

fn stub(verdict: &str) -> Vec<String> {
    [
        "env",
        "HIPPO_STUB_REVIEW=1",
        &format!("HIPPO_STUB_VERDICT={verdict}"),
        STUB,
    ]
    .map(String::from)
    .to_vec()
}

impl Setup {
    /// `hippo init` over a new workspace, with `model` as `model_command`.
    fn new(model: &[String]) -> Setup {
        let s = Setup {
            home: tempfile::tempdir().unwrap(),
            user: tempfile::tempdir().unwrap(),
            work: tempfile::tempdir().unwrap(),
            workspace: tempfile::tempdir().unwrap(),
        };
        let workspace = fs::canonicalize(s.workspace.path()).unwrap();
        write(&workspace, "notes/START HERE.md", "# Start\n");
        let out = s.run(&[
            "init",
            "--workspace",
            workspace.to_str().unwrap(),
            "--notes-dir",
            "notes",
            "--entry-note",
            "notes/START HERE.md",
        ]);
        assert!(out.status.success(), "{}", stderr(&out));
        s.model(model);
        s
    }

    /// Sets `model_command` in `config.toml`.
    fn model(&self, model: &[String]) {
        let workspace = fs::canonicalize(self.workspace.path()).unwrap();
        let argv: Vec<String> = model.iter().map(|a| format!("{a:?}")).collect();
        let config = format!(
            "workspace = {:?}\nmodel_command = [{}]\n",
            workspace.display().to_string(),
            argv.join(", ")
        );
        fs::write(self.home.path().join("config.toml"), config).unwrap();
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_hippo"))
            .args(args)
            .current_dir(self.work.path())
            .env("HIPPO_HOME", self.home.path())
            .env("HOME", self.user.path())
            .env_remove("RUST_LOG")
            .output()
            .unwrap()
    }

    /// The pack at `<work>/pack`, a minimal valid one.
    fn source(&self) -> std::path::PathBuf {
        let dir = self.work.path().join("pack");
        minimal_pack(&dir);
        dir
    }

    fn installed(&self, path: &str) -> std::path::PathBuf {
        self.workspace
            .path()
            .join(".hippo/packs/glossary-terms")
            .join(path)
    }

    fn packs(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self.workspace.path().join(".hippo/packs"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// A `model_command` that records that it ran and fails.
fn marker_model(marker: &Path) -> Vec<String> {
    vec![
        "sh".into(),
        "-c".into(),
        format!("touch {}; exit 1", marker.display()),
    ]
}

fn add_ok(s: &Setup) {
    let out = s.run(&["add", "./pack"]);
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
}

#[test]
fn add_copies_a_pack_with_its_origin_after_a_confirming_review() {
    let s = Setup::new(&stub("confirm"));
    s.source();
    let out = s.run(&["add", "./pack"]);
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("pack glossary-terms 1.0.0: Adds terms to the glossary.\n"),
        "{text}"
    );
    assert!(
        text.contains(
            "pass glossary-terms: reads notes; one call per note; works on every touched note\n"
        ),
        "{text}"
    );
    assert!(text.contains("files 5, "), "{text}");
    assert!(text.contains(" confirm: stub\n"), "{text}");
    assert!(
        text.ends_with(
            "wrote .hippo/packs/glossary-terms/\n\
             its passes first run at the next hippo dream (03:30 local time on the schedule)\n"
        ),
        "{text}"
    );
    assert_eq!(
        fs::read_to_string(s.installed("README.md")).unwrap(),
        "Glossary pack.\n"
    );
    assert!(s.installed("passes/glossary-terms.toml").is_file());
    let origin: serde_json::Value =
        serde_json::from_slice(&fs::read(s.installed("origin.json")).unwrap()).unwrap();
    assert_eq!(origin["protocol"], "HIPPO_V1");
    assert_eq!(origin["review"]["verdict"], "confirm");
    assert_eq!(origin["review"]["record"]["reviewer"], "pack-reviewer");
    assert_eq!(origin["source"]["kind"], "path");
    assert_eq!(origin["files"].as_object().unwrap().len(), 5);
}

#[test]
fn a_rejecting_review_writes_nothing() {
    let s = Setup::new(&stub("reject"));
    s.source();
    let out = s.run(&["add", "./pack"]);
    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
    assert!(stdout(&out).contains(" reject: stub\n"));
    assert_eq!(s.packs(), ["core"]);
}

#[test]
fn a_dry_run_writes_nothing_and_calls_no_model() {
    let marker_dir = tempfile::tempdir().unwrap();
    let marker = marker_dir.path().join("ran");
    let s = Setup::new(&marker_model(&marker));
    s.source();
    let out = s.run(&["add", "./pack", "--dry-run"]);
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    assert!(stdout(&out).ends_with("dry run: nothing written\n"));
    assert_eq!(s.packs(), ["core"]);
    assert!(!marker.exists());
}

#[test]
fn list_shows_clean_then_edited_and_ignores_hidden_files() {
    let s = Setup::new(&stub("confirm"));
    s.source();
    add_ok(&s);
    let line = |s: &Setup| {
        let out = s.run(&["packs", "list"]);
        assert!(out.status.success(), "{}", stderr(&out));
        stdout(&out)
            .lines()
            .find(|l| l.starts_with("glossary-terms "))
            .unwrap()
            .to_owned()
    };
    let source = fs::canonicalize(s.work.path().join("pack")).unwrap();
    let wanted = |state: &str| format!("glossary-terms 1.0.0 {state} path:{}", source.display());
    assert_eq!(line(&s), wanted("clean"));
    assert!(stdout(&s.run(&["packs", "list"])).starts_with("core - builtin -\n"));
    fs::write(s.installed("README.md"), "edited\n").unwrap();
    assert_eq!(line(&s), wanted("edited:1"));
    fs::write(s.installed(".DS_Store"), "x").unwrap();
    assert_eq!(line(&s), wanted("edited:1"));
}

#[test]
fn update_takes_upstream_changes_and_keeps_local_ones() {
    let s = Setup::new(&stub("confirm"));
    let source = s.source();
    add_ok(&s);
    fs::write(s.installed("prompts/glossary-terms.md"), "Mine {{notes}}\n").unwrap();
    write(
        &source,
        "personas/glossary-terms.md",
        "## Role\nr2\n## What you receive\nw\n## Rules\nr\n## Reply\n\
         Reply with `{\"verdict\": \"confirm\"|\"reject\", \"notes\": \"n\"}`.\n",
    );
    let out = s.run(&["packs", "update", "glossary-terms"]);
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    let text = stdout(&out);
    assert!(
        text.starts_with("glossary-terms 1.0.0 -> 1.0.0, commit - -> -\n"),
        "{text}"
    );
    assert!(
        text.contains("\ntake personas/glossary-terms.md\n"),
        "{text}"
    );
    assert!(
        text.contains("\nkeep prompts/glossary-terms.md\n"),
        "{text}"
    );
    assert!(
        text.contains("--- local/personas/glossary-terms.md"),
        "{text}"
    );
    assert!(
        text.ends_with("updated .hippo/packs/glossary-terms/\n"),
        "{text}"
    );
    assert!(
        fs::read_to_string(s.installed("personas/glossary-terms.md"))
            .unwrap()
            .contains("r2")
    );
    assert_eq!(
        fs::read_to_string(s.installed("prompts/glossary-terms.md")).unwrap(),
        "Mine {{notes}}\n"
    );
    assert_eq!(s.packs(), ["core", "glossary-terms"]);
}

#[test]
fn a_widening_update_needs_accept_reads() {
    let s = Setup::new(&stub("confirm"));
    let source = s.source();
    add_ok(&s);
    write(
        &source,
        "passes/glossary-terms.toml",
        "protocol = \"HIPPO_V1\"\nprompt = \"prompts/glossary-terms.md\"\n\
         reads = [\"notes\", \"turns\"]\nreviewer = \"personas/glossary-terms.md\"\nper_note = true\n",
    );
    let out = s.run(&["packs", "update", "glossary-terms"]);
    assert_eq!(out.status.code(), Some(1));
    let text = stdout(&out);
    assert!(
        text.contains("pass glossary-terms: reads notes -> notes, turns\n"),
        "{text}"
    );
    assert!(text.contains("--accept-reads"), "{text}");
    assert!(
        !fs::read_to_string(s.installed("passes/glossary-terms.toml"))
            .unwrap()
            .contains("turns")
    );
    let out = s.run(&["packs", "update", "glossary-terms", "--accept-reads"]);
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    assert!(
        fs::read_to_string(s.installed("passes/glossary-terms.toml"))
            .unwrap()
            .contains("turns")
    );
}

#[test]
fn a_conflict_needs_keep_local() {
    let s = Setup::new(&stub("confirm"));
    let source = s.source();
    add_ok(&s);
    fs::write(s.installed("README.md"), "mine\n").unwrap();
    write(&source, "README.md", "theirs\n");
    let out = s.run(&["packs", "update", "glossary-terms"]);
    assert_eq!(out.status.code(), Some(1));
    let text = stdout(&out);
    assert!(text.contains("\nconflict README.md\n"), "{text}");
    assert!(text.contains("--keep-local"), "{text}");
    assert_eq!(
        fs::read_to_string(s.installed("README.md")).unwrap(),
        "mine\n"
    );
    let out = s.run(&["packs", "update", "glossary-terms", "--keep-local"]);
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    assert_eq!(
        fs::read_to_string(s.installed("README.md")).unwrap(),
        "mine\n"
    );
    let list = stdout(&s.run(&["packs", "list"]));
    assert!(list.contains(" edited:1 "), "{list}");
}

#[test]
fn a_source_that_is_gone_cannot_update() {
    let s = Setup::new(&stub("confirm"));
    let source = s.source();
    add_ok(&s);
    fs::remove_dir_all(source).unwrap();
    let out = s.run(&["packs", "update", "glossary-terms"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("is not a directory here"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn diff_prints_the_plan_and_changes_nothing() {
    let s = Setup::new(&stub("confirm"));
    let source = s.source();
    add_ok(&s);
    // From here on a model call would leave the marker.
    let marker_dir = tempfile::tempdir().unwrap();
    let marker = marker_dir.path().join("ran");
    s.model(&marker_model(&marker));
    write(&source, "README.md", "changed upstream\n");
    let out = s.run(&["packs", "diff", "glossary-terms"]);
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("\ntake README.md\n"), "{text}");
    assert!(text.contains("--- local/README.md"), "{text}");
    assert!(text.contains("+++ upstream/README.md"), "{text}");
    assert_eq!(
        fs::read_to_string(s.installed("README.md")).unwrap(),
        "Glossary pack.\n"
    );
    assert!(!marker.exists());
    assert_eq!(s.packs(), ["core", "glossary-terms"]);
}

#[test]
fn remove_deletes_an_added_pack_and_a_local_one_only_with_force() {
    let s = Setup::new(&stub("confirm"));
    s.source();
    add_ok(&s);
    let out = s.run(&["packs", "remove", "glossary-terms"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "removed .hippo/packs/glossary-terms/\n");
    assert_eq!(s.packs(), ["core"]);

    minimal_pack(&s.workspace.path().join(".hippo/packs/glossary-terms"));
    let out = s.run(&["packs", "remove", "glossary-terms"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("has no origin.json"),
        "{}",
        stderr(&out)
    );
    assert_eq!(s.packs(), ["core", "glossary-terms"]);
    let out = s.run(&["packs", "remove", "glossary-terms", "--force"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(s.packs(), ["core"]);

    let out = s.run(&["packs", "remove", "core", "--force"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("core is hippo's own pack"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn hostile_packs_are_refused_before_any_model_call() {
    let marker_dir = tempfile::tempdir().unwrap();
    let marker = marker_dir.path().join("ran");
    let s = Setup::new(&marker_model(&marker));
    let dir = s.work.path().join("pack");

    #[cfg(unix)]
    {
        minimal_pack(&dir);
        std::os::unix::fs::symlink("/etc/hosts", dir.join("x.md")).unwrap();
        let out = s.run(&["add", "./pack"]);
        assert_eq!(out.status.code(), Some(1));
        assert!(stderr(&out).contains("a symbolic link"), "{}", stderr(&out));
        fs::remove_dir_all(&dir).unwrap();
    }

    minimal_pack(&dir);
    write(
        &dir,
        "personas/glossary-terms.md",
        "## Role\nr \u{202E}\n## What you receive\nw\n## Rules\nr\n## Reply\nr\n",
    );
    let out = s.run(&["add", "./pack"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stdout(&out).contains("U+202E"), "{}", stdout(&out));
    fs::remove_dir_all(&dir).unwrap();

    minimal_pack(&dir);
    write(&dir, "templates/big.md", &"x".repeat(70_000));
    let out = s.run(&["add", "./pack"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("over the 65536-byte limit"),
        "{}",
        stderr(&out)
    );

    assert_eq!(s.packs(), ["core"]);
    assert!(!marker.exists());
}

/// An index.json that lists `name` at a repository the CLI never reaches.
fn write_index(path: &Path, name: &str) {
    let index = serde_json::json!({
        "protocol": "HIPPO_V1",
        "generated": "2026-11-08T11:00:00Z",
        "counts": {"listed": 1, "first_party": 0, "other_owners": 1},
        "packs": [{
            "name": name, "version": "1.0.0", "description": "Adds terms.",
            "authors": ["Ada"], "owner": "alice", "maintainers": ["alice"],
            "first_party": false, "license": "MIT",
            "repo": "https://github.com/alice/hippo-packs", "repo_id": 1, "owner_id": 2,
            "path": name, "commit": "3f2c1a9e0b4d7c6e5f8a9b0c1d2e3f4a5b6c7d8e",
            "pack_hash": "b".repeat(64), "kind": "passes", "passes": [], "files": [],
            "review": {"id": "01JCA5ZB3K9V7Q2M4N6P8R0T1W", "verdict": "confirm",
                       "at": "2026-11-08T11:00:00Z"}
        }]
    });
    fs::write(path, serde_json::to_vec_pretty(&index).unwrap()).unwrap();
}

#[test]
fn a_registry_name_needs_registry_and_an_unknown_name_gets_suggestions() {
    let marker_dir = tempfile::tempdir().unwrap();
    let marker = marker_dir.path().join("ran");
    let s = Setup::new(&marker_model(&marker));

    let out = s.run(&["add", "glossary-terms"]);
    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
    assert!(
        stderr(&out).contains("a registry name needs --registry <path to index.json>"),
        "{}",
        stderr(&out)
    );

    let index = s.work.path().join("index.json");
    write_index(&index, "glossary-term");
    let out = s.run(&["add", "glossary-terms", "--registry", "index.json"]);
    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
    let text = stderr(&out);
    assert!(text.contains("no pack glossary-terms in "), "{text}");
    assert!(text.contains("did you mean glossary-term?"), "{text}");
    assert_eq!(s.packs(), ["core"]);
    assert!(!marker.exists());
}

#[test]
fn a_hostile_index_entry_is_refused_before_git_runs_and_its_text_is_escaped() {
    let marker_dir = tempfile::tempdir().unwrap();
    let marker = marker_dir.path().join("ran");
    let s = Setup::new(&marker_model(&marker));
    let index = s.work.path().join("index.json");

    // A commit that is a refspec, not 40 hex characters.
    write_index(&index, "glossary-terms");
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&index).unwrap()).unwrap();
    value["packs"][0]["commit"] = serde_json::json!("refs/heads/a:refs/heads/b");
    fs::write(&index, serde_json::to_vec(&value).unwrap()).unwrap();
    let out = s.run(&["add", "glossary-terms", "--registry", "index.json"]);
    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
    let text = stderr(&out);
    assert!(
        text.contains("the index's entry for it is refused (commit:"),
        "{text}"
    );

    // A repository that is not a GitHub repository, with a terminal escape.
    value["packs"][0]["commit"] = serde_json::json!("3f2c1a9e0b4d7c6e5f8a9b0c1d2e3f4a5b6c7d8e");
    value["packs"][0]["repo"] = serde_json::json!("https://evil.example/\u{1b}]0;pwned\u{7}");
    fs::write(&index, serde_json::to_vec(&value).unwrap()).unwrap();
    let out = s.run(&["add", "glossary-terms", "--registry", "index.json"]);
    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
    let text = stderr(&out);
    assert!(text.contains("refused (repo:"), "{text}");
    assert!(
        !text.contains('\u{1b}') && !text.contains('\u{7}'),
        "{text:?}"
    );
    assert_eq!(s.packs(), ["core"]);
    assert!(!marker.exists());
}

#[cfg(unix)]
#[test]
fn an_error_naming_a_directory_entry_prints_no_terminal_escape() {
    let marker_dir = tempfile::tempdir().unwrap();
    let marker = marker_dir.path().join("ran");
    let s = Setup::new(&marker_model(&marker));
    let dir = s.work.path().join("pack");
    minimal_pack(&dir);
    std::os::unix::fs::symlink("/etc/hosts", dir.join("\u{1b}[2J\u{202e}x")).unwrap();
    let out = s.run(&["add", "./pack"]);
    assert_eq!(out.status.code(), Some(1));
    let text = stderr(&out);
    assert!(
        text.contains("\\u{1b}[2J\\u{202e}x: a symbolic link"),
        "{text:?}"
    );
    assert!(
        !text.contains('\u{1b}') && !text.contains('\u{202e}'),
        "{text:?}"
    );
    assert!(!marker.exists());
}

#[test]
fn bidirectional_marks_in_names_are_escaped_by_check() {
    let dir = tempfile::tempdir().unwrap();
    minimal_pack(dir.path());
    write(dir.path(), "templates/a\u{202e}b.md", "x\n");
    let out = check(dir.path());
    assert_eq!(out.status.code(), Some(1));
    let text = stdout(&out);
    assert!(text.contains("a\\u{202e}b.md"), "{text}");
    assert!(!text.contains('\u{202e}'), "{text}");
}
