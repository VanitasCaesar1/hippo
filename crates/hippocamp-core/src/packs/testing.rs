//! The fixture-test case format of a pack: `tests/<case>/case.toml`
//! and the runner behind
//! `hippocamp packs test`: [`run_case`] runs one case in a temporary hippocamp home
//! against a stub model, and [`check_expectations`] compares the run with
//! the case's `[expect]`.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use time::format_description::well_known::Rfc3339;
use time::{Duration, OffsetDateTime};

use super::files::PackFiles;
use super::manifest;
use crate::config::Workspace;
use crate::dream::{self, RunSummary};
use crate::error::{Error, Result};
use crate::eval::{scratch, Models};
use crate::index::Index;
use crate::records::{check_protocol, Tool, Verdict};
use crate::sessions;

/// A parsed `case.toml`; `protocol` is checked and not kept.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Case {
    /// The pass under test.
    pub pass: String,
    /// What the case shows.
    pub about: String,
    /// `[[session]]`, one or more.
    pub session: Vec<CaseSession>,
    /// `[model]`: what the stub model and reviewer reply.
    pub model: CaseModel,
    #[serde(default)]
    pub expect: CaseExpect,
}

/// A `[[session]]` of a case.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaseSession {
    /// The `sessionId` in the case's transcript.
    pub id: String,
    #[serde(default = "claude_code")]
    pub tool: Tool,
    /// Paths under `notes/` the session touched.
    #[serde(default)]
    pub notes: Vec<String>,
    pub handoff: Option<String>,
}

fn claude_code() -> Tool {
    Tool::ClaudeCode
}

/// The `[model]` table of a case.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaseModel {
    /// The text the stub model returns to every pass call.
    pub reply: String,
    /// What the stub reviewer replies.
    #[serde(default = "confirm")]
    pub verdict: Verdict,
}

fn confirm() -> Verdict {
    Verdict::Confirm
}

/// The `[expect]` table of a case; an absent key is not checked.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaseExpect {
    pub calls: Option<u64>,
    pub proposals: Option<u64>,
    pub validation_failures: Option<u64>,
    pub confirmed: Option<u64>,
    pub rejected: Option<u64>,
    pub applied: Option<u64>,
    #[serde(default)]
    pub prompt_contains: Vec<String>,
    #[serde(default)]
    pub prompt_excludes: Vec<String>,
    #[serde(default)]
    pub note: Vec<CaseNote>,
}

/// A `[[expect.note]]`: what a note holds after the run.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaseNote {
    pub path: String,
    #[serde(default)]
    pub contains: Vec<String>,
    #[serde(default)]
    pub excludes: Vec<String>,
}

impl Case {
    /// Parses a `case.toml`: `protocol` as a record's, every table with
    /// unknown keys refused, and a `verdict` of `confirm` or `reject`.
    pub fn parse(text: &str) -> std::result::Result<Case, String> {
        let mut table: toml::Table = toml::from_str(text).map_err(first_line)?;
        check_protocol(table.remove("protocol").map(|found| match found {
            toml::Value::String(text) => text,
            other => other.to_string(),
        }))
        .map_err(|e| e.to_string())?;
        let case: Case = table.try_into().map_err(first_line)?;
        if case.session.is_empty() {
            return Err("session: needs at least one [[session]]".to_owned());
        }
        if case.model.verdict == Verdict::Revert {
            return Err("verdict must be confirm or reject".to_owned());
        }
        Ok(case)
    }

    /// Whether the registry accepts the case: it sets `calls`, and either
    /// `applied` or at least one `[[expect.note]]`.
    pub fn listable(&self) -> bool {
        self.expect.calls.is_some()
            && (self.expect.applied.is_some() || !self.expect.note.is_empty())
    }
}

/// The longest span the transcripts of one case may cover, so that one
/// dreaming window holds them all.
const MAX_SPAN: Duration = Duration::hours(23);

/// What running one case showed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaseOutcome {
    /// The case's directory name.
    pub name: String,
    /// One line per failed expectation; empty when the case passed.
    pub failures: Vec<String>,
}

/// Runs case `case_name` of `pack` in a new temporary directory, removed
/// afterwards, with `stub` as the model and the reviewer: the command and
/// its leading arguments, to which `--reply`, `--log` and `--system-prompt`
/// are added. A broken setup is an `Err`; a failed expectation is in the
/// outcome.
pub fn run_case(
    pack: &PackFiles,
    case_name: &str,
    case: &Case,
    stub: &[String],
) -> Result<CaseOutcome> {
    let outcome = |failures: Vec<String>| CaseOutcome {
        name: case_name.to_owned(),
        failures,
    };
    let prefix = format!("tests/{case_name}/");
    let manifest_text = pack
        .get("pack.toml")
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .ok_or_else(|| Error::Pack("pack.toml is missing".to_owned()))?;
    let pack_name = manifest::parse(manifest_text).map_err(Error::Pack)?.name;

    let tmp = tempfile::Builder::new()
        .prefix("hippocamp-pack-test-")
        .tempdir()?;
    let home = tmp.path().join("home");
    let workspace = tmp.path().join("workspace");
    let transcripts = tmp.path().join("transcripts");
    fs::create_dir_all(workspace.join("notes")).map_err(|e| Error::io_at(&workspace, e))?;
    let case_transcripts = transcripts.join(case_name);
    fs::create_dir_all(&case_transcripts).map_err(|e| Error::io_at(&case_transcripts, e))?;

    // The case's files: notes below `notes/`, transcripts by file name.
    let mut stamps = Vec::new();
    for (path, bytes) in pack.iter() {
        let Some(rest) = path.strip_prefix(&prefix) else {
            continue;
        };
        if let Some(below) = rest.strip_prefix("notes/") {
            write_file(&workspace.join("notes").join(below), bytes)?;
        } else if let Some(name) = rest.strip_prefix("transcripts/") {
            write_file(&case_transcripts.join(name), bytes)?;
            if let Err(why) = collect_timestamps(path, bytes, &mut stamps) {
                return Ok(outcome(vec![why]));
            }
        }
    }
    let (Some(&first), Some(&last)) = (stamps.iter().min(), stamps.iter().max()) else {
        return Ok(outcome(vec![
            "no transcript record has a timestamp".to_owned()
        ]));
    };
    if last - first > MAX_SPAN {
        return Ok(outcome(vec![
            "the transcripts span more than 23 hours".to_owned()
        ]));
    }

    let pass_reply = tmp.path().join("pass-reply.txt");
    let review_reply = tmp.path().join("review-reply.txt");
    let pass_log = tmp.path().join("pass.log");
    let review_log = tmp.path().join("review.log");
    write_file(&pass_reply, case.model.reply.as_bytes())?;
    let verdict = match case.model.verdict {
        Verdict::Reject => "reject",
        _ => "confirm",
    };
    let review_text = serde_json::json!({"verdict": verdict, "notes": "stub"}).to_string();
    write_file(&review_reply, review_text.as_bytes())?;
    let command = |reply: &Path, log: &Path| {
        let mut argv = stub.to_vec();
        for part in [
            "--reply",
            &reply.to_string_lossy(),
            "--log",
            &log.to_string_lossy(),
            "--system-prompt",
            "{system_prompt}",
        ] {
            argv.push(part.to_owned());
        }
        argv
    };
    let models = Models {
        model_command: command(&pass_reply, &pass_log),
        reviewer_command_override: Some(command(&review_reply, &review_log)),
    };
    let (cfg, ws) = scratch(
        &home,
        &workspace,
        &transcripts,
        "notes/START HERE.md",
        Some(&models),
    )?;

    // Only the pack under test is installed.
    let packs_dir = ws.hippocamp_dir().join("packs");
    let core = packs_dir.join("core");
    if core.exists() {
        fs::remove_dir_all(&core).map_err(|e| Error::io_at(&core, e))?;
    }
    for (path, bytes) in pack.iter() {
        if !path.starts_with("tests/") {
            write_file(&packs_dir.join(&pack_name).join(path), bytes)?;
        }
    }

    for session in &case.session {
        sessions::start(&ws, session.tool, &session.id, None)?;
        let notes: Vec<PathBuf> = session.notes.iter().map(PathBuf::from).collect();
        sessions::end(&ws, &session.id, &notes, session.handoff.as_deref(), None)?;
    }

    let now = last + Duration::hours(1);
    let summary = dream::run_at(&cfg, &ws, Some(&case.pass), false, now)?;

    let mut failures = Vec::new();
    let index = Index::open(&cfg.home, false)?;
    let indexed = index.last_ordinals()?;
    for session in &case.session {
        if !indexed.contains_key(&session.id) {
            failures.push(format!(
                "session {} has no turns in the transcripts",
                session.id
            ));
        }
    }
    if !failures.is_empty() {
        return Ok(outcome(failures));
    }
    let log = fs::read_to_string(&pass_log).unwrap_or_default();
    let read_note = |path: &str| read_note(&ws, path);
    failures.extend(check_expectations(&case.expect, &summary, &log, &read_note));
    Ok(outcome(failures))
}

fn read_note(ws: &Workspace, path: &str) -> Option<String> {
    fs::read_to_string(ws.root.join(path)).ok()
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| Error::io_at(parent, e))?;
    }
    fs::write(path, bytes).map_err(|e| Error::io_at(path, e))
}

/// Adds the top-level `timestamp` of every record in a transcript to
/// `stamps`; the error names the file and line of a timestamp that is not
/// RFC 3339.
fn collect_timestamps(
    path: &str,
    bytes: &[u8],
    stamps: &mut Vec<OffsetDateTime>,
) -> std::result::Result<(), String> {
    let text = String::from_utf8_lossy(bytes);
    for (n, line) in text.lines().enumerate() {
        let Ok(serde_json::Value::Object(record)) = serde_json::from_str(line) else {
            continue;
        };
        let Some(stamp) = record.get("timestamp") else {
            continue;
        };
        let parsed = stamp
            .as_str()
            .and_then(|text| OffsetDateTime::parse(text, &Rfc3339).ok());
        match parsed {
            Some(at) => stamps.push(at),
            None => return Err(format!("{path}:{}: the timestamp is not RFC 3339", n + 1)),
        }
    }
    Ok(())
}

/// Compares a run with `expect`. `pass_log` is the stub's JSON-lines log of
/// the pass's model calls; `note` returns the final text of a note by its
/// workspace-relative path. Returns one line per failed expectation.
pub fn check_expectations(
    expect: &CaseExpect,
    summary: &RunSummary,
    pass_log: &str,
    note: &dyn Fn(&str) -> Option<String>,
) -> Vec<String> {
    let mut failures = Vec::new();
    let calls = pass_log.lines().filter(|l| !l.trim().is_empty()).count();
    let counts: [(&str, Option<u64>, usize); 6] = [
        ("calls", expect.calls, calls),
        ("proposals", expect.proposals, summary.proposals),
        (
            "validation_failures",
            expect.validation_failures,
            summary.validation_failures,
        ),
        ("confirmed", expect.confirmed, summary.confirmed),
        ("rejected", expect.rejected, summary.rejected),
        ("applied", expect.applied, summary.applied),
    ];
    for (key, wanted, got) in counts {
        if let Some(wanted) = wanted {
            if wanted != got as u64 {
                failures.push(format!("{key}: expected {wanted}, got {got}"));
            }
        }
    }
    let inputs: Vec<String> = pass_log
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter_map(|value| value.get("input")?.as_str().map(str::to_owned))
        .collect();
    for text in &expect.prompt_contains {
        if !inputs.iter().any(|input| input.contains(text.as_str())) {
            failures.push(format!("prompt_contains: \"{text}\" not found"));
        }
    }
    for text in &expect.prompt_excludes {
        if inputs.iter().any(|input| input.contains(text.as_str())) {
            failures.push(format!("prompt_excludes: \"{text}\" found"));
        }
    }
    for expected in &expect.note {
        let Some(body) = note(&expected.path) else {
            failures.push(format!("note {}: no such note", expected.path));
            continue;
        };
        for text in &expected.contains {
            if !body.contains(text.as_str()) {
                failures.push(format!("note {}: \"{text}\" not found", expected.path));
            }
        }
        for text in &expected.excludes {
            if body.contains(text.as_str()) {
                failures.push(format!("note {}: \"{text}\" found", expected.path));
            }
        }
    }
    failures
}

/// The stub model behind `hippocamp packs stub-model`: appends one JSON line
/// `{"system_prompt": …, "input": …}` to `log`, so no text of a prompt can
/// be mistaken for a delimiter, and returns `{"result": <the reply file's
/// text>}`, as `claude -p --output-format json` does.
pub fn stub_model(reply: &Path, log: &Path, system_prompt: &str, stdin: &str) -> Result<String> {
    let text = fs::read_to_string(reply).map_err(|e| Error::io_at(reply, e))?;
    let mut line = serde_json::json!({"system_prompt": system_prompt, "input": stdin}).to_string();
    line.push('\n');
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
        .and_then(|mut file| file.write_all(line.as_bytes()))
        .map_err(|e| Error::io_at(log, e))?;
    Ok(serde_json::json!({"result": text}).to_string())
}

/// The reason of a TOML error, without its source excerpt.
fn first_line(err: toml::de::Error) -> String {
    err.message().lines().next().unwrap_or_default().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CASE: &str = r#"protocol = "HIPPOCAMP_V1"
pass = "glossary-terms"
about = "A term defined in a session is appended to the glossary"

[[session]]
id = "glossary-1"
notes = ["notes/Glossary.md", "notes/Unrelated.md"]
handoff = "Defined the retry budget; see [[Glossary]]."

[model]
reply = '''[]'''
verdict = "confirm"

[expect]
calls = 1
proposals = 1
validation_failures = 0
confirmed = 1
rejected = 0
applied = 1
prompt_contains = ["Glossary"]
prompt_excludes = ["Unrelated"]

[[expect.note]]
path = "notes/Glossary.md"
contains = ["Retry budget"]
excludes = []
"#;

    #[test]
    fn the_design_example_parses() {
        let case = Case::parse(CASE).unwrap();
        assert_eq!(case.pass, "glossary-terms");
        assert_eq!(case.session.len(), 1);
        assert_eq!(case.session[0].tool, Tool::ClaudeCode);
        assert_eq!(case.session[0].notes.len(), 2);
        assert_eq!(case.model.verdict, Verdict::Confirm);
        assert_eq!(case.expect.calls, Some(1));
        assert_eq!(case.expect.applied, Some(1));
        assert_eq!(case.expect.note[0].contains, ["Retry budget"]);
        assert!(case.listable());
    }

    #[test]
    fn defaults_and_required_tables() {
        let minimal = "protocol = \"HIPPOCAMP_V1\"\npass = \"p\"\nabout = \"a\"\n\
                       [[session]]\nid = \"s\"\ntool = \"codex\"\n[model]\nreply = \"[]\"\n";
        let case = Case::parse(minimal).unwrap();
        assert_eq!(case.session[0].tool, Tool::Codex);
        assert_eq!(case.model.verdict, Verdict::Confirm);
        assert_eq!(case.expect, CaseExpect::default());
        assert!(!case.listable());
        for missing in ["[[session]]\nid = \"s\"\n", "[model]\nreply = \"[]\"\n"] {
            let text = minimal
                .replace("tool = \"codex\"\n", "")
                .replace(missing, "");
            let err = Case::parse(&text).unwrap_err();
            assert!(err.contains("missing field"), "{err}");
        }
        let err = Case::parse(&minimal.replace("HIPPOCAMP_V1", "HIPPOCAMP_V0")).unwrap_err();
        assert_eq!(err, "expected protocol HIPPOCAMP_V1, found HIPPOCAMP_V0");
    }

    #[test]
    fn unknown_keys_are_refused_in_every_table() {
        for (anchor, extra) in [
            ("about = ", "extra = 1\nabout = "),
            ("id = \"glossary-1\"", "id = \"glossary-1\"\nextra = 1"),
            ("verdict = ", "extra = 1\nverdict = "),
            ("calls = 1", "calls = 1\nextra = 1"),
            ("excludes = []", "excludes = []\nextra = 1"),
        ] {
            let text = CASE.replacen(anchor, extra, 1);
            let err = Case::parse(&text).unwrap_err();
            assert!(err.contains("unknown field `extra`"), "{anchor}: {err}");
        }
    }

    #[test]
    fn revert_is_not_a_verdict_of_a_case() {
        let text = CASE.replace("verdict = \"confirm\"", "verdict = \"revert\"");
        assert_eq!(
            Case::parse(&text).unwrap_err(),
            "verdict must be confirm or reject"
        );
        let text = CASE.replace("verdict = \"confirm\"", "verdict = \"reject\"");
        assert_eq!(Case::parse(&text).unwrap().model.verdict, Verdict::Reject);
    }

    #[test]
    fn listable_needs_calls_and_a_check_of_the_result() {
        let mut case = Case::parse(CASE).unwrap();
        case.expect.note.clear();
        assert!(case.listable());
        case.expect.applied = None;
        assert!(!case.listable());
        case.expect.note.push(CaseNote {
            path: "notes/a.md".into(),
            contains: vec![],
            excludes: vec![],
        });
        assert!(case.listable());
        case.expect.calls = None;
        assert!(!case.listable());
    }

    fn expect() -> CaseExpect {
        CaseExpect {
            calls: Some(1),
            proposals: Some(1),
            validation_failures: Some(0),
            confirmed: Some(1),
            rejected: Some(0),
            applied: Some(1),
            prompt_contains: vec!["Glossary".into()],
            prompt_excludes: vec!["Unrelated".into()],
            note: vec![CaseNote {
                path: "notes/Glossary.md".into(),
                contains: vec!["Retry".into()],
                excludes: vec!["Gone".into()],
            }],
        }
    }

    fn summary() -> RunSummary {
        RunSummary {
            proposals: 1,
            confirmed: 1,
            applied: 1,
            ..RunSummary::default()
        }
    }

    const LOG: &str = "{\"system_prompt\":\"s\",\"input\":\"the Glossary note\"}\n";

    fn note(_: &str) -> Option<String> {
        Some("Retry budget".to_owned())
    }

    #[test]
    fn a_matching_run_has_no_failure() {
        assert_eq!(
            check_expectations(&expect(), &summary(), LOG, &note),
            Vec::<String>::new()
        );
        assert!(check_expectations(&CaseExpect::default(), &summary(), "", &note).is_empty());
    }

    #[test]
    fn every_key_reports_its_mismatch() {
        let mut got = summary();
        got.proposals = 2;
        got.validation_failures = 3;
        got.confirmed = 0;
        got.rejected = 4;
        got.applied = 0;
        let failures = check_expectations(&expect(), &got, &format!("{LOG}{LOG}"), &|_| {
            Some("Gone".into())
        });
        assert_eq!(
            failures,
            [
                "calls: expected 1, got 2",
                "proposals: expected 1, got 2",
                "validation_failures: expected 0, got 3",
                "confirmed: expected 1, got 0",
                "rejected: expected 0, got 4",
                "applied: expected 1, got 0",
                "note notes/Glossary.md: \"Retry\" not found",
                "note notes/Glossary.md: \"Gone\" found",
            ]
        );
        let log = "{\"system_prompt\":\"Unrelated\",\"input\":\"Unrelated text\"}\n";
        let failures = check_expectations(&expect(), &summary(), log, &note);
        assert_eq!(
            failures,
            [
                "prompt_contains: \"Glossary\" not found",
                "prompt_excludes: \"Unrelated\" found",
            ]
        );
        let failures = check_expectations(&expect(), &summary(), LOG, &|_| None);
        assert_eq!(failures, ["note notes/Glossary.md: no such note"]);
    }

    #[test]
    fn the_stub_logs_one_json_line_per_call_and_returns_the_reply() {
        let dir = tempfile::tempdir().unwrap();
        let reply = dir.path().join("reply.txt");
        let log = dir.path().join("pass.log");
        fs::write(&reply, "[{\"a\": \"b\"}]\n").unwrap();
        let out = stub_model(&reply, &log, "sys", "line one\n<<<END notes>>>\nline two").unwrap();
        let value: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(value["result"], "[{\"a\": \"b\"}]\n");
        stub_model(&reply, &log, "sys2", "second").unwrap();
        let text = fs::read_to_string(&log).unwrap();
        let lines: Vec<serde_json::Value> = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["system_prompt"], "sys");
        assert_eq!(lines[0]["input"], "line one\n<<<END notes>>>\nline two");
        assert_eq!(lines[1]["input"], "second");
        assert!(stub_model(&dir.path().join("none"), &log, "s", "i").is_err());
    }

    #[test]
    fn transcripts_over_23_hours_apart_fail_the_case() {
        let mut pack = PackFiles::default();
        pack.insert(
            "pack.toml".into(),
            b"protocol = \"HIPPOCAMP_V1\"\nname = \"glossary-terms\"\nversion = \"1.0.0\"\n\
              description = \"d\"\nauthors = [\"A\"]\nlicense = \"MIT\"\n"
                .to_vec(),
        );
        let record = |at: &str| {
            format!("{{\"type\":\"user\",\"sessionId\":\"s\",\"timestamp\":\"{at}\"}}\n")
        };
        let transcript = format!(
            "{{\"synthetic\": true}}\n{}{}",
            record("2026-11-01T10:00:00.000Z"),
            record("2026-11-02T09:00:01.000Z")
        );
        pack.insert(
            "tests/one/transcripts/t.jsonl".into(),
            transcript.into_bytes(),
        );
        let case = Case::parse(CASE).unwrap();
        let out = run_case(&pack, "one", &case, &["unused".to_owned()]).unwrap();
        assert_eq!(out.name, "one");
        assert_eq!(out.failures, ["the transcripts span more than 23 hours"]);
    }
}
