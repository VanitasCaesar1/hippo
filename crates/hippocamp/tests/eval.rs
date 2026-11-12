//! `hippocamp eval fixtures/eval` through the built binary, with `HIPPOCAMP_HOME`
//! in an empty temporary directory. The pinned lines are measurements of
//! the set, not targets: a change that moves them updates them here and
//! says why in its pull request.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

const SET: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/eval");

/// Context retrieval over the set's 20 tasks.
const CONTEXT_LINE: &str = "context: found 15/18, precision 15/25, complete 13/16, quiet 4/4";
/// Phrase search over the set's 20 queries.
const RECALL_LINE: &str = "recall: hit@1 16/20, hit@5 16/20, hit@20 16/20, mrr 0.80 \
     (verbatim 10/10, tool 4/4, codex 2/2, paraphrase 0/4)";
/// One confirmed correction of the planted error in Backups, nothing damaged.
const DREAMING_STUB_LINE: &str = "dreaming: corrected 1/7, damaged 0/8, lines lost 0, clean notes changed 0/2; proposals 1, validation failures 0, confirmed 1, rejected 0, applied 1";
const DREAMING_LINE: &str = "dreaming: skipped: needs --with-model";

#[test]
fn eval_without_a_model_prints_the_pinned_summary_and_writes_nothing() {
    let home = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_hippocamp"))
        .args(["eval", SET])
        .env("HIPPOCAMP_HOME", home.path())
        .env_remove("RUST_LOG")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stdout}\n{stderr}");

    let line = |prefix: &str| {
        stdout
            .lines()
            .find(|line| line.starts_with(prefix))
            .unwrap_or_else(|| panic!("no {prefix} line in:\n{stdout}"))
    };
    assert_eq!(line("context:"), CONTEXT_LINE);
    assert_eq!(line("recall:"), RECALL_LINE);
    assert_eq!(line("dreaming:"), DREAMING_LINE);

    // Eval built its own home and workspace; the given one is untouched.
    assert_eq!(fs::read_dir(home.path()).unwrap().count(), 0);
}

/// The summary line starting `prefix`, or a panic showing the output.
fn line(out: &Output, prefix: &str) -> String {
    let stdout = String::from_utf8_lossy(&out.stdout);
    stdout
        .lines()
        .find(|line| line.starts_with(prefix))
        .unwrap_or_else(|| panic!("no {prefix} line in:\n{stdout}"))
        .to_owned()
}

/// Writes `config.toml` in `home` naming `workspace`, which `--with-model`
/// loads but eval never touches.
fn config_toml(home: &Path, workspace: &Path, model: &str, extra: &str) {
    let text = format!("workspace = {workspace:?}\nmodel_command = {model}\n{extra}");
    fs::write(home.join("config.toml"), text).unwrap();
}

#[test]
fn eval_without_the_flag_never_runs_the_configured_model() {
    let home = tempfile::tempdir().unwrap();
    let marker = home.path().join("ran");
    let workspace = tempfile::tempdir().unwrap();
    config_toml(
        home.path(),
        workspace.path(),
        &format!("[\"sh\", \"-c\", \"touch {}\"]", marker.display()),
        "",
    );
    let out = Command::new(env!("CARGO_BIN_EXE_hippocamp"))
        .args(["eval", SET])
        .env("HIPPOCAMP_HOME", home.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(line(&out, "dreaming:"), DREAMING_LINE);
    assert!(!marker.exists());
}

#[test]
fn eval_with_the_stub_model_scores_one_correction() {
    let home = tempfile::tempdir().unwrap();
    let model = Path::new(SET).join("../stub-model/model.sh");
    let model = fs::canonicalize(model).unwrap().display().to_string();
    // The configured workspace is never touched: it keeps its one file.
    let workspace = tempfile::tempdir().unwrap();
    fs::write(workspace.path().join("sentinel.md"), "# Sentinel\n").unwrap();
    config_toml(
        home.path(),
        workspace.path(),
        &format!("[{model:?}, \"--system-prompt\", \"{{system_prompt}}\"]"),
        &format!(
            "reviewer_command_override = [\"env\", \"HIPPOCAMP_STUB_REVIEW=1\", {model:?}, \
             \"--system-prompt\", \"{{system_prompt}}\"]\n"
        ),
    );
    let out = Command::new(env!("CARGO_BIN_EXE_hippocamp"))
        .args(["eval", SET, "--with-model"])
        .env("HIPPOCAMP_HOME", home.path())
        .env("HIPPOCAMP_STUB_SCENARIO", "eval")
        .env("HIPPOCAMP_STUB_VERDICT", "confirm")
        .env_remove("RUST_LOG")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(line(&out, "context:"), CONTEXT_LINE);
    assert_eq!(line(&out, "recall:"), RECALL_LINE);
    assert_eq!(line(&out, "dreaming:"), DREAMING_STUB_LINE, "{stdout}");
    // The run used the set's clock: its last window ends at `now`.
    assert!(stdout.contains("to 2026-11-08T00:00:00Z: done"), "{stdout}");

    // Only the configuration the test wrote is there.
    let left: Vec<_> = fs::read_dir(home.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(left, ["config.toml"]);
    let kept: Vec<_> = fs::read_dir(workspace.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(kept, ["sentinel.md"]);
    assert_eq!(
        fs::read_to_string(workspace.path().join("sentinel.md")).unwrap(),
        "# Sentinel\n"
    );
}

#[test]
fn a_failed_dreaming_run_prints_its_own_report_and_exits_1() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    // A model that prints no result: the verify pass fails the run.
    config_toml(
        home.path(),
        workspace.path(),
        "[\"sh\", \"-c\", \"cat > /dev/null; echo not-json\"]",
        "",
    );
    let out = Command::new(env!("CARGO_BIN_EXE_hippocamp"))
        .args(["eval", SET, "--with-model"])
        .env("HIPPOCAMP_HOME", home.path())
        .env_remove("RUST_LOG")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(
        line(&out, "dreaming:").starts_with("dreaming: failed: "),
        "{stdout}"
    );
    assert_eq!(line(&out, "context:"), CONTEXT_LINE);
    assert!(stdout.contains("The failed run's own report:"), "{stdout}");
    assert!(
        stdout.contains("```markdown\nstatus: failed\nerror: "),
        "{stdout}"
    );
}

#[test]
fn a_recall_answer_that_does_not_quote_its_query_fails_the_set() {
    // Question 1 quotes turn eval-01 7; turn 6 is a real turn without
    // that text, the kind of slip check 8 exists to catch.
    let dir = tempfile::tempdir().unwrap();
    let set = dir.path().join("eval");
    copy_dir(Path::new(SET), &set);
    let toml = fs::read_to_string(set.join("set.toml")).unwrap();
    let wrong = toml.replacen(
        "expected = [{ session = \"eval-01\", ordinal = 7 }]",
        "expected = [{ session = \"eval-01\", ordinal = 6 }]",
        1,
    );
    assert_ne!(wrong, toml, "the set's first recall answer moved");
    fs::write(set.join("set.toml"), wrong).unwrap();

    let home = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_hippocamp"))
        .arg("eval")
        .arg(&set)
        .env("HIPPOCAMP_HOME", home.path())
        .env_remove("RUST_LOG")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("eval set: recall question 1 (kept for 30 days after the retention change): no expected turn contains the query"),
        "{stderr}"
    );
    assert!(out.stdout.is_empty());
}

/// Runs `hippocamp eval` on `set` and returns the exit code and stderr.
fn eval_fails(set: &Path) -> String {
    let home = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_hippocamp"))
        .arg("eval")
        .arg(set)
        .env("HIPPOCAMP_HOME", home.path())
        .env_remove("RUST_LOG")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(out.stdout.is_empty());
    stderr
}

#[test]
fn a_query_that_is_only_part_of_a_word_in_its_turn_fails_the_set() {
    // "kept for 30 day" is inside "kept for 30 days" but ends mid-word.
    let dir = tempfile::tempdir().unwrap();
    let set = dir.path().join("eval");
    copy_dir(Path::new(SET), &set);
    let toml = fs::read_to_string(set.join("set.toml")).unwrap();
    let cut = toml.replacen(
        "query = \"kept for 30 days after the retention change\"",
        "query = \"kept for 30 day\"",
        1,
    );
    assert_ne!(cut, toml);
    fs::write(set.join("set.toml"), cut).unwrap();
    let stderr = eval_fails(&set);
    assert!(
        stderr.contains("recall question 1 (kept for 30 day): no expected turn contains the query"),
        "{stderr}"
    );
}

#[test]
fn a_transcript_turn_that_redacts_fails_the_set() {
    let dir = tempfile::tempdir().unwrap();
    let set = dir.path().join("eval");
    copy_dir(Path::new(SET), &set);
    let path = set.join("transcripts/claude-code/eval-01.jsonl");
    let text = fs::read_to_string(&path).unwrap();
    // A made-up key in the shape of a GitHub token, in turn 0.
    let key = format!("ghp_{}", "A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q7r8");
    let marked = text.replacen("the cron job", &format!("the key {key}"), 1);
    assert_ne!(marked, text);
    fs::write(&path, marked).unwrap();
    let stderr = eval_fails(&set);
    assert!(
        stderr.contains("eval set: redaction changes transcripts/claude-code/eval-01.jsonl:2"),
        "{stderr}"
    );
}

#[test]
fn a_transcript_that_does_not_parse_fails_the_set() {
    let dir = tempfile::tempdir().unwrap();
    let set = dir.path().join("eval");
    copy_dir(Path::new(SET), &set);
    // Without its session id the file cannot be indexed.
    let path = set.join("transcripts/claude-code/eval-01.jsonl");
    let text = fs::read_to_string(&path).unwrap();
    fs::write(
        &path,
        text.replace("\"sessionId\":\"eval-01\"", "\"x\":\"eval-01\""),
    )
    .unwrap();
    let stderr = eval_fails(&set);
    assert!(stderr.contains("eval-01.jsonl did not parse: "), "{stderr}");
}

#[test]
fn eval_without_the_flag_ignores_a_broken_config() {
    let home = tempfile::tempdir().unwrap();
    fs::write(home.path().join("config.toml"), "this is not toml [").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_hippocamp"))
        .args(["eval", SET])
        .env("HIPPOCAMP_HOME", home.path())
        .env_remove("RUST_LOG")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.lines().any(|l| l == CONTEXT_LINE), "{stdout}");
    assert!(stdout.lines().any(|l| l == RECALL_LINE), "{stdout}");
}

#[test]
fn a_planted_error_verify_would_never_take_fails_the_set() {
    // With the window starting after every session, no note is a candidate.
    let dir = tempfile::tempdir().unwrap();
    let set = dir.path().join("eval");
    copy_dir(Path::new(SET), &set);
    let toml = fs::read_to_string(set.join("set.toml")).unwrap();
    let late = toml.replacen(
        "last_success = \"2026-10-31T00:00:00Z\"",
        "last_success = \"2026-11-07T23:00:00Z\"",
        1,
    );
    assert_ne!(late, toml, "the set's window moved");
    fs::write(set.join("set.toml"), late).unwrap();
    let stderr = eval_fails(&set);
    assert!(
        stderr.contains(
            "eval set: dreaming error 1 (kept for 14 days): notes/Backups.md is a candidate in no chunk"
        ),
        "{stderr}"
    );
}

/// Copies the set, lets `edit` change it, and returns the error `hippocamp
/// eval` exits with.
fn edited_set_fails(edit: impl FnOnce(&Path)) -> String {
    let dir = tempfile::tempdir().unwrap();
    let set = dir.path().join("eval");
    copy_dir(Path::new(SET), &set);
    edit(&set);
    eval_fails(&set)
}

/// Replaces `from` by `to` once in `file` of the set, which must change.
fn replace_in(set: &Path, file: &str, from: &str, to: &str) {
    let path = set.join(file);
    let text = fs::read_to_string(&path).unwrap();
    let new = text.replacen(from, to, 1);
    assert_ne!(new, text, "{file} has no {from:?}");
    fs::write(path, new).unwrap();
}

#[test]
fn a_planted_error_note_that_is_a_candidate_on_two_days_fails_the_set() {
    // eval-02 (2 November) also touches Backups, which eval-01 (1 November)
    // touches: two chunks.
    let stderr = edited_set_fails(|set| {
        replace_in(
            set,
            "sessions/eval-02.json",
            "\"notes_touched\":[\"notes/Thumbnail pipeline.md\"]",
            "\"notes_touched\":[\"notes/Thumbnail pipeline.md\",\"notes/Backups.md\"]",
        )
    });
    assert!(
        stderr.contains(
            "dreaming error 1 (kept for 14 days): notes/Backups.md is a candidate in 2 chunks"
        ),
        "{stderr}"
    );
}

#[test]
fn a_kept_claim_note_no_record_touches_in_the_window_fails_the_set() {
    // Sharing links is touched only by eval-07 (6 November); end the window
    // before it.
    let stderr = edited_set_fails(|set| {
        replace_in(
            set,
            "set.toml",
            "now = \"2026-11-08T00:00:00Z\"",
            "now = \"2026-11-06T12:00:00Z\"",
        )
    });
    assert!(stderr.contains("dreaming keep 7 (Share links expire after 30 days by default.): notes/Sharing links.md is a candidate in no chunk"), "{stderr}");
}

#[test]
fn a_planted_error_nobody_corrects_in_a_transcript_fails_the_set() {
    let stderr = edited_set_fails(|set| {
        replace_in(
            set,
            "set.toml",
            "fixed = [\"kept for 30 days\"]",
            "fixed = [\"kept for 31 days\"]",
        )
    });
    assert!(
        stderr.contains(
            "eval set: dreaming error 1 (kept for 14 days): no indexed turn holds any fixed string"
        ),
        "{stderr}"
    );
}

#[test]
fn an_error_kind_that_does_not_match_what_verify_gathers_fails_the_set() {
    // Backups' evidence is findable, not unfindable.
    let stderr = edited_set_fails(|set| {
        replace_in(
            set,
            "set.toml",
            "kind = \"findable\"",
            "kind = \"unfindable\"",
        )
    });
    assert!(
        stderr.contains(
            "dreaming error 1 (kept for 14 days): its kind is unfindable, but verify gathers \
             a turn that holds a fixed string and is short enough to quote"
        ),
        "{stderr}"
    );
    // Database migrations' evidence is not gathered at all, so not findable.
    let stderr = edited_set_fails(|set| {
        replace_in(
            set,
            "set.toml",
            "kind = \"unfindable\"",
            "kind = \"findable\"",
        )
    });
    assert!(
        stderr.contains(
            "dreaming error 4 (Migrations run automatically on deploy): its kind is findable, \
             but verify gathers no turn that holds a fixed string"
        ),
        "{stderr}"
    );
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
