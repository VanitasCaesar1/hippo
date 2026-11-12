//! `hippo doctor`: one row per check, printed as a
//! `check | status | detail` table. Any `problem` makes the command fail;
//! a `warn` is reported and never fails it.
//!
//! Problems: the configuration fails to load; `notes_dir` is missing; the
//! linter finds errors; the index is missing, or, with the ingest job
//! installed, the last ingest is older than three ingest intervals; a
//! sample task's required context is over budget; the newest dreaming
//! report failed; a dreamed note matches neither hash of its newest
//! applied receipt within 24 hours of that receipt being applied; with the
//! dream job installed, the last dreaming success is older than 36 hours;
//! no success once a real (not dry) dreaming run has reported; a snapshot
//! directory has no receipt.
//!
//! A new setup passes: no schedule installed and
//! no dreaming run yet are `ok` rows that start `not set up yet:` and name
//! the command to run.
//!
//! Warnings, never problems: linter warnings, `BadFrontmatter` among them;
//! transcripts that did not parse; transcript files the index does not
//! hold; a dreamed note edited more than 24 hours after its newest receipt
//! was applied; a proposal without a receipt (its review failed); session
//! records that cannot be read and derived records without an indexed
//! session; the dream job runs another
//! binary than this one while a pack other than `core` is installed (macOS
//! only); one of the two launchd jobs missing; an old index or dreaming
//! success with no job to refresh it; the result of a one-word
//! `model_command` run, sent only with `hippo doctor --model`. The size of
//! `snapshots/` is reported.

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::io::{Read, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use time::format_description::well_known::Rfc3339;
use time::{Duration, OffsetDateTime};
use ulid::Ulid;
use wait_timeout::ChildExt;
use walkdir::WalkDir;

use crate::adapters;
use crate::config::{Config, Workspace};
use crate::context::{self, PartSelection};
use crate::dream::report;
use crate::error::{Error, Result};
use crate::index::Index;
use crate::notes::{LintFinding, Notes};
use crate::packs::install::schedule_warning;
use crate::records::{
    list_receipts, list_sessions, DreamState, IngestState, Listed, Proposal, ProposalKind, Receipt,
    Session,
};
use crate::util::{hex, json_files, sha256};

/// A check's outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    Ok,
    Warn,
    Problem,
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Status::Ok => "ok",
            Status::Warn => "warn",
            Status::Problem => "problem",
        })
    }
}

/// One row of the report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub name: &'static str,
    pub status: Status,
    pub detail: String,
}

/// Every check's row, in a fixed order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub checks: Vec<Check>,
}

impl Report {
    /// Whether any row is a problem: `hippo doctor` then exits non-zero.
    pub fn has_problem(&self) -> bool {
        self.checks
            .iter()
            .any(|check| check.status == Status::Problem)
    }

    /// The row of the check called `name`.
    pub fn check(&self, name: &str) -> Option<&Check> {
        self.checks.iter().find(|check| check.name == name)
    }
}

impl fmt::Display for Report {
    /// `check | status | detail`, then one line per row.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "check | status | detail")?;
        for check in &self.checks {
            writeln!(f, "{} | {} | {}", check.name, check.status, check.detail)?;
        }
        Ok(())
    }
}

/// Loads the configuration from `home` and the workspace it names, then
/// runs every check. When either fails to load, the report is that one
/// `config` problem.
/// The model ping runs only with `model` (`hippo doctor --model`).
pub fn run_in(home: &Path, model: bool) -> Report {
    let loaded =
        Config::load_in(home).and_then(|cfg| Workspace::load_optional(&cfg).map(|ws| (cfg, ws)));
    let env = Env {
        check_model: model,
        ..Env::current()
    };
    match loaded {
        Ok((cfg, Some(ws))) => run_with(&cfg, &ws, &env),
        Ok((cfg, None)) => run_history(&cfg, &env),
        Err(err) => Report {
            checks: vec![row("config", Status::Problem, err.to_string())],
        },
    }
}

/// The checks a history-only configuration (no notes folder yet) can
/// pass: the index, its parse failures, unindexed transcripts and the
/// ingest job. The notes rows are one `ok` line saying there is no notes
/// folder, which is a choice, not a problem.
fn run_history(cfg: &Config, env: &Env) -> Report {
    let mut checks = vec![row(
        "config",
        Status::Ok,
        format!(
            "{}; no notes folder yet: hippo reads history only",
            cfg.home.join("config.toml").display()
        ),
    )];
    let (index_row, index) = index_check(cfg, env);
    checks.push(index_row);
    checks.push(parse_failures_check(index.as_ref()));
    checks.extend(dsh_logs_check(index.as_ref()));
    checks.push(unindexed_check(cfg, index.as_ref()));
    if let Some(dir) = &env.launch_agents {
        checks.push(launchd_jobs_check(dir, &["dev.hippo.ingest"], cfg));
    }
    Report { checks }
}

/// Runs every check for a loaded configuration and workspace.
pub fn run(cfg: &Config, ws: &Workspace) -> Report {
    run_with(cfg, ws, &Env::current())
}

/// What the checks read from outside the configuration, so tests can fix
/// it.
struct Env {
    now: OffsetDateTime,
    /// `~/Library/LaunchAgents` on macOS; `None` skips the launchd check.
    launch_agents: Option<PathBuf>,
    /// This binary, for the dream job binary check; `None` skips it.
    exe: Option<PathBuf>,
    /// How long the one-word model run may take.
    model_timeout: std::time::Duration,
    /// Whether to send the one-word model run (`hippo doctor --model`).
    check_model: bool,
}

impl Env {
    fn current() -> Env {
        let launch_agents = if cfg!(target_os = "macos") {
            home::home_dir().map(|home| home.join("Library").join("LaunchAgents"))
        } else {
            None
        };
        Env {
            now: OffsetDateTime::now_utc(),
            launch_agents,
            exe: std::env::current_exe().ok(),
            model_timeout: std::time::Duration::from_secs(60),
            check_model: false,
        }
    }

    /// Whether the launchd job `label` is installed: `None` off macOS,
    /// where hippo cannot see cron or systemd.
    fn job_installed(&self, label: &str) -> Option<bool> {
        self.launch_agents
            .as_ref()
            .map(|dir| dir.join(format!("{label}.plist")).is_file())
    }
}

fn run_with(cfg: &Config, ws: &Workspace, env: &Env) -> Report {
    let mut checks = vec![
        row(
            "config",
            Status::Ok,
            format!(
                "{}; workspace {}",
                cfg.home.join("config.toml").display(),
                ws.root.display()
            ),
        ),
        notes_dir_check(ws),
        linter_check(ws),
    ];
    let (index_row, index) = index_check(cfg, env);
    checks.push(index_row);
    checks.push(parse_failures_check(index.as_ref()));
    checks.extend(dsh_logs_check(index.as_ref()));
    checks.push(unindexed_check(cfg, index.as_ref()));
    checks.push(sample_tasks_check(ws));
    checks.push(report_check(ws));
    checks.push(last_success_check(ws, env, report::any_real_run(ws)));
    match load_receipts(ws) {
        Ok(receipts) => {
            checks.push(receipts_check(ws, &receipts, env.now));
            checks.push(snapshots_check(ws, &receipts));
            checks.push(proposals_check(ws, &receipts));
        }
        Err(err) => {
            checks.push(row("receipts", Status::Problem, err.to_string()));
            let detail = "cannot tell which snapshots have receipts".to_owned();
            checks.push(row("snapshots", Status::Problem, detail));
        }
    }
    if let Some(dir) = &env.launch_agents {
        checks.push(launchd_check(dir, cfg));
        if let Some(exe) = &env.exe {
            if has_other_pack(ws) {
                if let Some(text) = schedule_warning(dir, exe) {
                    checks.push(row("dream job binary", Status::Warn, text));
                }
            }
        }
    }
    let (records, unreadable) = load_records(ws);
    checks.push(sessions_check(ws, &records, index.as_ref(), env));
    checks.push(records_check(&records, &unreadable, index.as_ref()));
    checks.push(git_check(ws));
    checks.push(lessons_check(cfg));
    checks.push(model_check(cfg, env));
    Report { checks }
}

/// Whether git tracks, or would not ignore, a file each person keeps to
/// themselves. Read-only: `git rev-parse`, `git
/// ls-files` and `git check-ignore`; never a problem.
fn git_check(ws: &Workspace) -> Check {
    let git = |args: &[&str]| {
        Command::new("git")
            .arg("-C")
            .arg(&ws.root)
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
    };
    let Ok(top) = git(&["rev-parse", "--show-toplevel"]) else {
        return row("git", Status::Ok, "git not found");
    };
    if !top.status.success() {
        return row("git", Status::Ok, "not a git work tree");
    }
    let log = ws.log_path.to_string_lossy().into_owned();
    let paths: Vec<String> = crate::config::PER_PERSON
        .iter()
        .map(|p| format!(".hippo/{}", p.trim_end_matches('/')))
        .chain([log.clone()])
        .collect();
    let mut args = vec!["ls-files", "-z", "--"];
    args.extend(paths.iter().map(String::as_str));
    let tracked: Vec<String> = match git(&args) {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
            .split('\0')
            .filter(|f| !f.is_empty())
            .map(str::to_owned)
            .collect(),
        _ => return row("git", Status::Ok, "git could not list the tracked files"),
    };
    if !tracked.is_empty() {
        let mut holders: Vec<&str> = paths
            .iter()
            .map(String::as_str)
            .filter(|p| {
                tracked
                    .iter()
                    .any(|f| f == p || f.starts_with(&format!("{p}/")))
            })
            .collect();
        holders.dedup();
        let names: Vec<&str> = tracked.iter().map(String::as_str).collect();
        let quoted: Vec<String> = holders.iter().map(|p| shell_word(p)).collect();
        let detail = format!(
            "{} per-person files are tracked by git: {}; stop tracking them with `git rm -r --cached -- {}` then `git commit -m \"Stop tracking per-person hippo files\"`",
            tracked.len(),
            sample(&names),
            quoted.join(" ")
        );
        return row("git", Status::Warn, detail);
    }
    // A probe inside each folder: check-ignore tests the rules, not files.
    let not_ignored: Vec<&str> = paths
        .iter()
        .filter(|p| {
            let probe = if p.ends_with(".json") || **p == log {
                (*p).clone()
            } else {
                format!("{p}/probe.json")
            };
            !git(&["check-ignore", "-q", "--no-index", "--", &probe])
                .is_ok_and(|out| out.status.success())
        })
        .map(String::as_str)
        .collect();
    if !not_ignored.is_empty() {
        let detail = format!(
            "not ignored by git: {}; `hippo init` in the workspace adds the ignore rules",
            sample(&not_ignored)
        );
        return row("git", Status::Warn, detail);
    }
    row("git", Status::Ok, "per-person files ignored")
}

/// `word` as one shell word: as is when it is safe, else in single quotes.
fn shell_word(word: &str) -> String {
    if word
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-'))
    {
        word.to_owned()
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

/// The last lessons scan: a warning when it failed,
/// never a problem; `not run yet` before the first.
fn lessons_check(cfg: &Config) -> Check {
    match crate::lessons::State::load(&cfg.home) {
        None => row("lessons", Status::Ok, "not run yet"),
        Some(s) => match s.error {
            Some(e) => row(
                "lessons",
                Status::Warn,
                format!("the last scan failed: {e}"),
            ),
            None => {
                let waiting = crate::lessons::pending(&cfg.home).map_or(0, |p| p.len());
                row(
                    "lessons",
                    Status::Ok,
                    format!(
                        "{} sessions read, {} waiting in the Inbox",
                        s.sessions, waiting
                    ),
                )
            }
        },
    }
}

fn row(name: &'static str, status: Status, detail: impl Into<String>) -> Check {
    Check {
        name,
        status,
        detail: detail.into(),
    }
}

fn notes_dir_check(ws: &Workspace) -> Check {
    let dir = ws.notes_root();
    if dir.is_dir() {
        row("notes_dir", Status::Ok, dir.display().to_string())
    } else {
        let detail = format!("{} is not a directory", dir.display());
        row("notes_dir", Status::Problem, detail)
    }
}

/// Errors are problems; warnings, `BadFrontmatter` among them, are not.
fn linter_check(ws: &Workspace) -> Check {
    let notes = match Notes::scan(&ws.notes_root()) {
        Ok(notes) => notes,
        Err(err) => return row("linter", Status::Problem, err.to_string()),
    };
    let findings = notes.lint();
    let count = |wanted: fn(&LintFinding) -> bool| findings.iter().filter(|f| wanted(f)).count();
    let errors = count(LintFinding::is_error);
    let warnings = findings.len() - errors;
    let mut detail = format!(
        "{} notes; {errors} errors, {warnings} warnings ({} missing frontmatter, {} bad frontmatter, \
         {} dangling links, {} overdue)",
        notes.notes.len(),
        count(|f| matches!(f, LintFinding::MissingFrontmatter { .. })),
        count(|f| matches!(f, LintFinding::BadFrontmatter { .. })),
        count(|f| matches!(f, LintFinding::DanglingLink { .. })),
        count(|f| matches!(f, LintFinding::Overdue { .. })),
    );
    let status = if errors > 0 {
        if let Some(first) = findings.iter().find(|f| f.is_error()) {
            detail.push_str(&format!("; first: {first}"));
        }
        Status::Problem
    } else if warnings > 0 {
        Status::Warn
    } else {
        Status::Ok
    };
    row("linter", status, detail)
}

/// Appended to a stale index's detail on macOS. A launchd job that reads a
/// workspace on an external volume asks once for "files on a removable
/// volume"; until someone answers, the job waits and nothing else runs.
/// The grant is kept for the binary's path, so a rebuild in place did not
/// ask again.
#[cfg(target_os = "macos")]
const STALE_HINT: &str =
    "; if `launchctl print gui/$UID/dev.hippo.ingest` says `state = running`, \
     the job is probably waiting on a macOS permission prompt: answer it at the Mac";
#[cfg(not(target_os = "macos"))]
const STALE_HINT: &str = "";

/// The index must exist, and the last ingest be no older than three ingest
/// intervals.
fn index_check(cfg: &Config, env: &Env) -> (Check, Option<Index>) {
    let index = match Index::open(&cfg.home, false) {
        Ok(index) => index,
        Err(err) => return (row("index", Status::Problem, err.to_string()), None),
    };
    let state_path = cfg.home.join("ingest-state.json");
    let limit_minutes = cfg.ingest_interval_minutes.saturating_mul(3);
    let check = match IngestState::load(&state_path) {
        Err(err) => row("index", Status::Problem, format!("no ingest state: {err}")),
        Ok(state) if state.is_stale(cfg.ingest_interval_minutes, env.now) => {
            let last = rfc3339(state.last_ingest);
            match env.job_installed("dev.hippo.ingest") {
                Some(true) => row(
                    "index",
                    Status::Problem,
                    format!(
                        "last ingest {last}, over three ingest intervals ({limit_minutes} minutes) ago{STALE_HINT}"
                    ),
                ),
                Some(false) => row(
                    "index",
                    Status::Ok,
                    format!(
                        "last ingest {last}; with no schedule, running hippo or hippo ingest brings it up to date"
                    ),
                ),
                None => row(
                    "index",
                    Status::Warn,
                    format!(
                        "last ingest {last}, over three ingest intervals ({limit_minutes} minutes) ago; \
                         if a cron job or systemd timer runs hippo ingest, check it (hippo schedule install prints both), \
                         else run hippo ingest"
                    ),
                ),
            }
        }
        Ok(state) => row(
            "index",
            Status::Ok,
            format!(
                "last ingest {}: {} files seen, {} indexed",
                rfc3339(state.last_ingest),
                state.files_seen,
                state.files_indexed
            ),
        ),
    };
    (check, Some(index))
}

fn parse_failures_check(index: Option<&Index>) -> Check {
    let Some(index) = index else {
        return row("parse failures", Status::Warn, "skipped: no index");
    };
    match index.failed_files() {
        Ok(failed) if failed.is_empty() => row("parse failures", Status::Ok, "none"),
        Ok(failed) => {
            let paths: Vec<&str> = failed
                .iter()
                .map(|file| file.transcript_path.as_str())
                .collect();
            let detail = format!(
                "{} transcripts did not parse and wait for a change: {}",
                failed.len(),
                sample(&paths)
            );
            row("parse failures", Status::Warn, detail)
        }
        Err(err) => row("parse failures", Status::Warn, err.to_string()),
    }
}

/// dsh logs in a format version hippo does not read, from the index's
/// parse failures: a warning naming the versions and what to do. With
/// none, an `ok` row when the index holds dsh sessions, else no row.
fn dsh_logs_check(index: Option<&Index>) -> Option<Check> {
    let index = index?;
    let failed = index.failed_files().ok()?;
    let mut versions: std::collections::BTreeMap<u64, Vec<&str>> = Default::default();
    for file in &failed {
        let Some(rest) = file.error.split("dsh log format v").nth(1) else {
            continue;
        };
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(v) = digits.parse() {
            versions
                .entry(v)
                .or_default()
                .push(file.transcript_path.as_str());
        }
    }
    if versions.is_empty() {
        let any = index
            .sources()
            .ok()?
            .values()
            .any(|(tool, _)| tool == "dsh");
        return any.then(|| {
            row(
                "dsh logs",
                Status::Ok,
                "every dsh log is in a format hippo reads (v3 or v4)",
            )
        });
    }
    let total: usize = versions.values().map(Vec::len).sum();
    let counts: Vec<String> = versions
        .iter()
        .map(|(v, paths)| format!("v{v} ({})", paths.len()))
        .collect();
    let mut advice = Vec::new();
    if versions.keys().any(|v| *v < 3) {
        advice.push("resuming an older session in a current dsh upgrades its log");
    }
    if versions.keys().any(|v| *v > 4) {
        advice.push("a newer log format needs a newer hippo");
    }
    let paths: Vec<&str> = versions.values().flatten().copied().collect();
    Some(row(
        "dsh logs",
        Status::Warn,
        format!(
            "{total} dsh logs are in a format hippo does not read (it reads v3 and v4): {}; {}: {}",
            counts.join(", "),
            advice.join("; "),
            sample(&paths)
        ),
    ))
}

/// Transcript files an adapter claims that the index has no row for: new
/// since the last ingest, or sharing a session id with another file (the
/// index keeps one file per session).
fn unindexed_check(cfg: &Config, index: Option<&Index>) -> Check {
    let Some(index) = index else {
        return row("unindexed files", Status::Warn, "skipped: no index");
    };
    let known = match index.file_hashes() {
        Ok(known) => known,
        Err(err) => return row("unindexed files", Status::Warn, err.to_string()),
    };
    let model_runs = cfg.home.join("model-runs");
    let mut missing: Vec<String> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    for adapter in adapters::all() {
        for root in adapter.roots(cfg) {
            let entries = WalkDir::new(&root)
                .into_iter()
                .filter_entry(|entry| !entry.path().starts_with(&model_runs))
                .filter_map(|entry| entry.ok());
            for entry in entries {
                let path = entry.path();
                if entry.file_type().is_file()
                    && adapter.file_matches(path)
                    && seen.insert(path.to_path_buf())
                    && !known.contains_key(path.to_string_lossy().as_ref())
                {
                    missing.push(path.display().to_string());
                }
            }
        }
    }
    if missing.is_empty() {
        let detail = format!("{} transcript files, all indexed", seen.len());
        return row("unindexed files", Status::Ok, detail);
    }
    missing.sort();
    let paths: Vec<&str> = missing.iter().map(String::as_str).collect();
    let detail = format!(
        "{} of {} transcript files are not in the index (new since the last ingest, \
         or sharing a session id with another file): {}",
        missing.len(),
        seen.len(),
        sample(&paths)
    );
    row("unindexed files", Status::Warn, detail)
}

/// Each sample task's context must compile within `budget_bytes`.
fn sample_tasks_check(ws: &Workspace) -> Check {
    if ws.sample_tasks.is_empty() {
        return row("sample tasks", Status::Ok, "none configured");
    }
    let mut over = Vec::new();
    let mut largest = 0;
    for task in &ws.sample_tasks {
        match context::compile(
            ws,
            None,
            Some(task),
            None,
            None,
            ws.budget_bytes,
            PartSelection::All,
        ) {
            Ok(compiled) => largest = largest.max(compiled.bytes),
            Err(Error::OverBudget { required_bytes, .. }) => {
                over.push(format!("{task:?} needs {required_bytes} bytes"));
            }
            Err(err) => over.push(format!("{task:?}: {err}")),
        }
    }
    if over.is_empty() {
        let detail = format!(
            "{} tasks within {} bytes; largest {largest}",
            ws.sample_tasks.len(),
            ws.budget_bytes
        );
        row("sample tasks", Status::Ok, detail)
    } else {
        let detail = format!(
            "over a budget of {} bytes: {}",
            ws.budget_bytes,
            over.join("; ")
        );
        row("sample tasks", Status::Problem, detail)
    }
}

/// The newest dreaming report: a failed one is a problem, and so is one
/// that cannot be read.
fn report_check(ws: &Workspace) -> Check {
    match report::newest(ws) {
        Ok(None) => row("last report", Status::Ok, "no dreaming report yet"),
        Ok(Some(status)) => {
            let name = file_name(&status.path);
            if status.failed {
                let error = status
                    .error
                    .map(|error| format!("error: {error}"))
                    .unwrap_or_default();
                row(
                    "last report",
                    Status::Problem,
                    format!("{name} failed; {error}"),
                )
            } else {
                let big = fs::read_to_string(&status.path)
                    .map(|text| too_big(&text))
                    .unwrap_or(0);
                match big {
                    0 => row("last report", Status::Ok, name),
                    n => row(
                        "last report",
                        Status::Warn,
                        format!(
                            "{name}; {n} {} too big for dreaming to read whole (split {})",
                            if n == 1 { "note was" } else { "notes were" },
                            if n == 1 { "it" } else { "them" },
                        ),
                    ),
                }
            }
        }
        Err(err) => {
            // The error names the path; this row names the file.
            let path = report::newest_path(ws).unwrap_or_default();
            let text = err.to_string();
            let text = text
                .strip_prefix(&format!("{}: ", path.display()))
                .unwrap_or(&text);
            row(
                "last report",
                Status::Problem,
                format!("{}: {text}", file_name(&path)),
            )
        }
    }
}

/// The notes a dreaming report's Omissions left out for their size: a
/// pass skipped (`skipped: the note alone makes a …-byte prompt`) or left
/// out (`omitted: note <path> (<n> bytes)`). Notes left out by rank are not
/// counted.
fn too_big(report: &str) -> usize {
    let mut notes = std::collections::BTreeSet::new();
    let mut inside = false;
    for line in report.lines() {
        if let Some(h) = line.strip_prefix("## ") {
            inside = h.trim() == "Omissions";
            continue;
        }
        let Some(bullet) = line.strip_prefix("- ").filter(|_| inside) else {
            continue;
        };
        if let Some((head, _)) = bullet.split_once(": skipped: the note alone makes a ") {
            // `[<from> to <to> ]<pass> <note>`
            let words: Vec<&str> = head.splitn(4, ' ').collect();
            let body = match words.as_slice() {
                [_, "to", _, rest] => *rest,
                _ => head,
            };
            if let Some((_, note)) = body.split_once(' ') {
                notes.insert(note.to_owned());
            }
        } else if let Some((_, rest)) = bullet.split_once(": omitted: note ") {
            if let Some((path, size)) = rest.rsplit_once(" (") {
                if size.trim_end_matches(')').ends_with(" bytes")
                    && size
                        .split(' ')
                        .next()
                        .is_some_and(|n| n.parse::<u64>().is_ok())
                {
                    notes.insert(path.to_owned());
                }
            }
        }
    }
    notes.len()
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `last_success` no older than 36 hours when the dream job is installed
/// (a warning without it); absent counts only once a real (not dry)
/// dreaming run has reported.
fn last_success_check(ws: &Workspace, env: &Env, any_real_run: bool) -> Check {
    let state = DreamState::load(&ws.hippo_dir().join("dream-state.json")).ok();
    match state.and_then(|state| state.last_success) {
        Some(at) if env.now - at > Duration::hours(36) => {
            if env.job_installed("dev.hippo.dream") == Some(true) {
                row(
                    "last success",
                    Status::Problem,
                    format!("{}, over 36 hours ago", rfc3339(at)),
                )
            } else {
                row(
                    "last success",
                    Status::Warn,
                    format!(
                        "{}, over 36 hours ago; no dream job runs it: run hippo dream, or hippo schedule install",
                        rfc3339(at)
                    ),
                )
            }
        }
        Some(at) => row("last success", Status::Ok, rfc3339(at)),
        None if any_real_run => row(
            "last success",
            Status::Problem,
            "no successful dreaming run recorded",
        ),
        None => row(
            "last success",
            Status::Ok,
            "not set up yet: no dreaming run yet; hippo dream runs one (it calls the model)",
        ),
    }
}

fn load_receipts(ws: &Workspace) -> Result<Vec<Receipt>> {
    list_receipts(ws)?
        .into_iter()
        .map(Listed::require)
        .collect()
}

/// For every dreamed note, the note should still match a hash of its newest
/// applied receipt: `before_hash` (a revert, or the change undone) or
/// `after_hash` (as hippo left it). A mismatch within 24 hours of that
/// receipt being applied is a problem: a partial apply or an immediate
/// overwrite. An older one is a warning, since agents edit notes at any
/// time. So is a fresh one whose change is still in the note: an agent
/// edited elsewhere in it.
fn receipts_check(ws: &Workspace, receipts: &[Receipt], now: OffsetDateTime) -> Check {
    let proposals = ws.hippo_dir().join("proposals");
    let mut notes: HashMap<Ulid, Proposal> = HashMap::new();
    let mut newest: HashMap<PathBuf, &Receipt> = HashMap::new();
    for receipt in receipts.iter().filter(|receipt| receipt.applied.is_some()) {
        let proposal = match notes.entry(receipt.proposal) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let path = proposals.join(format!("{}.json", receipt.proposal));
                match Proposal::load(&path) {
                    Ok(proposal) => entry.insert(proposal),
                    Err(err) => return row("receipts", Status::Problem, err.to_string()),
                }
            }
        };
        let note = proposal.note.clone();
        let entry = newest.entry(note).or_insert(receipt);
        if receipt.applied > entry.applied {
            *entry = receipt;
        }
    }
    if newest.is_empty() {
        return row("receipts", Status::Ok, "no applied receipt");
    }
    let mut fresh: Vec<String> = Vec::new();
    let mut older: Vec<String> = Vec::new();
    let mut kept = 0usize;
    for (note, receipt) in &newest {
        let bytes = fs::read(ws.root.join(note)).ok();
        let hash = bytes
            .as_deref()
            .map(|bytes| hex(&sha256(bytes)))
            .unwrap_or_default();
        if hash == receipt.before_hash || hash == receipt.after_hash {
            continue;
        }
        let intact = bytes.as_deref().is_some_and(|bytes| {
            notes
                .get(&receipt.proposal)
                .is_some_and(|proposal| change_intact(proposal, &String::from_utf8_lossy(bytes)))
        });
        // A note edited since, or merged with a teammate's change, that
        // still holds the applied change is consistent at any age:
        // comparing whole-file hashes warned for ever.
        if intact {
            kept += 1;
            continue;
        }
        let recent = receipt
            .applied
            .is_some_and(|applied| now - applied <= Duration::hours(24));
        let list = if recent { &mut fresh } else { &mut older };
        list.push(note.display().to_string());
    }
    if fresh.is_empty() && older.is_empty() {
        let detail = if kept == 0 {
            format!("{} dreamed notes match their newest receipt", newest.len())
        } else {
            format!(
                "{} dreamed notes keep their change ({kept} edited since)",
                newest.len()
            )
        };
        return row("receipts", Status::Ok, detail);
    }
    fresh.sort();
    older.sort();
    let mut parts = Vec::new();
    if !fresh.is_empty() {
        let paths: Vec<&str> = fresh.iter().map(String::as_str).collect();
        parts.push(format!(
            "{} changed within 24 hours of being dreamed: {}",
            fresh.len(),
            sample(&paths)
        ));
    }
    if !older.is_empty() {
        let paths: Vec<&str> = older.iter().map(String::as_str).collect();
        parts.push(format!(
            "{} whose change an edit since removed: {}",
            older.len(),
            sample(&paths)
        ));
    }
    let status = if fresh.is_empty() {
        Status::Warn
    } else {
        Status::Problem
    };
    let detail = format!(
        "of {} dreamed notes, {} match neither hash of their newest receipt ({})",
        newest.len(),
        fresh.len() + older.len(),
        parts.join("; ")
    );
    row("receipts", status, detail)
}

/// The applied change is still in the note: its `new` text is there and,
/// for a replacement that did not keep it, its `old` text is gone.
fn change_intact(proposal: &Proposal, text: &str) -> bool {
    if proposal.new.is_empty() && proposal.old.is_empty() {
        return false;
    }
    let new_there = proposal.new.is_empty() || text.contains(&proposal.new);
    let old_gone = proposal.kind != ProposalKind::Replace
        || proposal.old.is_empty()
        || proposal.new.contains(&proposal.old)
        || !text.contains(&proposal.old);
    new_there && old_gone
}

/// Every snapshot directory needs a receipt for its proposal; the size of
/// `snapshots/` is reported.
fn snapshots_check(ws: &Workspace, receipts: &[Receipt]) -> Check {
    let dir = ws.hippo_dir().join("snapshots");
    let Ok(entries) = fs::read_dir(&dir) else {
        return row("snapshots", Status::Ok, "none");
    };
    let with_receipt: HashSet<String> = receipts
        .iter()
        .map(|receipt| receipt.proposal.to_string())
        .collect();
    let mut orphans: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| !with_receipt.contains(name))
        .collect();
    let bytes: u64 = WalkDir::new(&dir)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.metadata().ok())
        .filter(|metadata| metadata.is_file())
        .map(|metadata| metadata.len())
        .sum();
    if orphans.is_empty() {
        let keep = match ws.snapshot_keep_days {
            Some(d) => format!("kept {d} days (snapshot_keep_days)"),
            None => "kept until removed (snapshot_keep_days unset)".to_owned(),
        };
        return row("snapshots", Status::Ok, format!("{bytes} bytes; {keep}"));
    }
    orphans.sort();
    let names: Vec<&str> = orphans.iter().map(String::as_str).collect();
    let detail = format!(
        "{bytes} bytes; {} without a receipt: {}",
        orphans.len(),
        sample(&names)
    );
    row("snapshots", Status::Problem, detail)
}

/// Every proposal should have a receipt. One without was written but never
/// reviewed: its review failed, and the run with it. A warning.
fn proposals_check(ws: &Workspace, receipts: &[Receipt]) -> Check {
    let reviewed: HashSet<String> = receipts
        .iter()
        .map(|receipt| receipt.proposal.to_string())
        .collect();
    let files = match json_files(&ws.hippo_dir().join("proposals")) {
        Ok(files) => files,
        Err(err) => return row("proposals", Status::Warn, err.to_string()),
    };
    let mut unreviewed: Vec<String> = files
        .iter()
        .filter_map(|path| path.file_stem())
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|id| !reviewed.contains(id))
        .collect();
    if unreviewed.is_empty() {
        let detail = format!("{} proposals, each with a receipt", files.len());
        return row("proposals", Status::Ok, detail);
    }
    unreviewed.sort();
    let ids: Vec<&str> = unreviewed.iter().map(String::as_str).collect();
    let detail = format!(
        "{} of {} proposals have no receipt (their review failed): {}",
        unreviewed.len(),
        files.len(),
        sample(&ids)
    );
    row("proposals", Status::Warn, detail)
}

/// Both launchd jobs' property lists, as `hippo schedule install` writes
/// them. None installed is `not set up yet`; one
/// of them missing is a warning.
fn launchd_check(dir: &Path, cfg: &Config) -> Check {
    launchd_jobs_check(dir, &["dev.hippo.ingest", "dev.hippo.dream"], cfg)
}

fn launchd_jobs_check(dir: &Path, labels: &[&str], cfg: &Config) -> Check {
    let missing: Vec<&str> = labels
        .iter()
        .copied()
        .filter(|label| !dir.join(format!("{label}.plist")).is_file())
        .collect();
    if missing.is_empty() {
        row(
            "launchd",
            Status::Ok,
            format!("{} installed", labels.join(" and ")),
        )
    } else if missing.len() == labels.len() {
        let dream = if labels.contains(&"dev.hippo.dream") {
            format!(" and dreams at {}", cfg.dream_at)
        } else {
            String::new()
        };
        let detail = format!(
            "not set up yet: no schedule, so the index updates when you run hippo or hippo ingest; \
             hippo schedule install ingests every {} minutes{dream}",
            cfg.ingest_interval_minutes
        );
        row("launchd", Status::Ok, detail)
    } else {
        let detail = format!(
            "schedule partly installed: {} missing from {}; run hippo schedule install",
            missing.join(", "),
            dir.display()
        );
        row("launchd", Status::Warn, detail)
    }
}

/// Whether `.hippo/packs/` holds a pack other than `core`.
fn has_other_pack(ws: &Workspace) -> bool {
    let Ok(entries) = fs::read_dir(ws.hippo_dir().join("packs")) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let name = entry.file_name();
        let visible = !name.as_encoded_bytes().starts_with(b".");
        visible && name != "core" && entry.path().is_dir()
    })
}

/// Every session record, and the names of those that cannot be read.
fn load_records(ws: &Workspace) -> (Vec<Session>, Vec<String>) {
    let mut records = Vec::new();
    let mut unreadable = Vec::new();
    for listed in list_sessions(ws).unwrap_or_default() {
        match listed.record {
            Ok(record) => records.push(record),
            Err(_) => unreadable.push(listed.path.display().to_string()),
        }
    }
    (records, unreadable)
}

/// The declared sessions that ended in the last seven days: how many ended
/// `unspecified` (no notes, no handoff, no reason through hippo), how many
/// of those ran a session-end command, which a workspace that ends its
/// sessions with its own tool shows, how many others edited
/// notes as their transcripts show, which dreaming reads,
/// and how many left no trace at all. Never a problem:
/// agents may end sessions with another tool. A warning when the sessions
/// that left no trace are more than half.
fn sessions_check(ws: &Workspace, records: &[Session], index: Option<&Index>, env: &Env) -> Check {
    let since = env.now - Duration::days(7);
    let ended: Vec<&Session> = records
        .iter()
        .filter(|record| !record.derived && record.ended.is_some_and(|ended| ended >= since))
        .collect();
    let unspecified: Vec<&str> = ended
        .iter()
        .filter(|record| record.no_change_reason.as_deref() == Some("unspecified"))
        .map(|record| record.id.as_str())
        .collect();
    let traces = index.and_then(|index| crate::dream::inputs::traces(ws, index, &unspecified).ok());
    let (traced, no_trace) = match &traces {
        Some(traces) => {
            let ran = traces.values().filter(|t| t.ended).count();
            let edited = traces.values().filter(|t| t.edited && !t.ended).count();
            (
                format!(
                    "{ran} of them ran a session-end command, {edited} others edited notes their transcripts show"
                ),
                unspecified.len() - ran - edited,
            )
        }
        None => (
            "their transcripts could not be read".to_owned(),
            unspecified.len(),
        ),
    };
    let detail = format!(
        "{} declared sessions ended in the last 7 days: {} ended unspecified, {traced}, {no_trace} left no trace",
        ended.len(),
        unspecified.len(),
    );
    let status = if no_trace * 2 > ended.len() {
        Status::Warn
    } else {
        Status::Ok
    };
    row("sessions", status, detail)
}

/// Records that cannot be read, and derived records without an indexed
/// session. An indexed session without a record is not a gap: sessions of
/// other projects get none.
fn records_check(records: &[Session], unreadable: &[String], index: Option<&Index>) -> Check {
    let mut gaps = Vec::new();
    if !unreadable.is_empty() {
        let names: Vec<&str> = unreadable.iter().map(String::as_str).collect();
        gaps.push(format!(
            "{} unreadable: {}",
            unreadable.len(),
            sample(&names)
        ));
    }
    if let Some(index) = index {
        match index.turn_counts() {
            Ok(indexed) => {
                let mut stale: Vec<&str> = records
                    .iter()
                    .filter(|record| record.derived && !indexed.contains_key(&record.id))
                    .map(|record| record.id.as_str())
                    .collect();
                stale.sort_unstable();
                if !stale.is_empty() {
                    gaps.push(format!(
                        "{} derived records without an indexed session: {}",
                        stale.len(),
                        sample(&stale)
                    ));
                }
            }
            Err(err) => gaps.push(err.to_string()),
        }
    }
    if gaps.is_empty() {
        let detail = format!("{} records", records.len());
        row("session records", Status::Ok, detail)
    } else {
        row("session records", Status::Warn, gaps.join("; "))
    }
}

/// With `hippo doctor --model`, runs `model_command` with a one-word
/// prompt when its program is on `PATH`. A reply is `ok`; anything else is
/// `warn`, never a problem. Without the flag nothing is sent.
fn model_check(cfg: &Config, env: &Env) -> Check {
    let Some(program) = cfg.model_command.first() else {
        return row("model", Status::Warn, "model_command is empty");
    };
    if find_program(program).is_none() {
        let not_found = std::io::Error::from(std::io::ErrorKind::NotFound);
        let detail = crate::model::spawn_error(program, &not_found);
        return row("model", Status::Warn, detail);
    }
    if !env.check_model {
        let detail = format!("not checked (hippo doctor --model sends one word through {program})");
        return row("model", Status::Ok, detail);
    }
    match ask_one_word(cfg, env.model_timeout) {
        Ok(reply) => row("model", Status::Ok, format!("replied {reply:?}")),
        Err(err) => row("model", Status::Warn, err),
    }
}

/// `program` when it is a path to a file, else the first file of that name
/// in a `PATH` directory.
fn find_program(program: &str) -> Option<PathBuf> {
    let path = Path::new(program);
    if path.components().count() > 1 {
        return path.is_file().then(|| path.to_path_buf());
    }
    let dirs = std::env::var_os("PATH")?;
    std::env::split_paths(&dirs)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

/// Runs `model_command` in an emptied `$HIPPO_HOME/model-runs/`, with
/// `{system_prompt}` asking for one word and `ping` on stdin, and returns
/// the `result` of the JSON it prints.
fn ask_one_word(cfg: &Config, timeout: std::time::Duration) -> std::result::Result<String, String> {
    let runs = cfg.home.join("model-runs");
    let _ = fs::remove_dir_all(&runs);
    fs::create_dir_all(&runs).map_err(|e| format!("{}: {e}", runs.display()))?;
    let argv: Vec<String> = cfg
        .model_command
        .iter()
        .map(|arg| match arg.as_str() {
            "{system_prompt}" => "Reply with exactly one word.".to_owned(),
            other => other.to_owned(),
        })
        .collect();
    let mut child = Command::new(&argv[0])
        .args(&argv[1..])
        .current_dir(&runs)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| crate::model::spawn_error(&argv[0], &e))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(b"ping");
    }
    let status = match child.wait_timeout(timeout).map_err(|e| e.to_string())? {
        Some(status) => status,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("no reply within {} seconds", timeout.as_secs()));
        }
    };
    let mut stdout = String::new();
    let mut stderr = Vec::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = out.read_to_string(&mut stdout);
    }
    if let Some(mut err) = child.stderr.take() {
        let _ = err.read_to_end(&mut stderr);
    }
    if !status.success() {
        let why = crate::model::failure_reason(stdout.as_bytes(), &stderr).unwrap_or_default();
        return Err(format!("exited with {status}: {why}"));
    }
    let reply: serde_json::Value =
        serde_json::from_str(&stdout).map_err(|_| "the reply is not JSON".to_owned())?;
    match reply.get("result").and_then(|result| result.as_str()) {
        Some(result) => Ok(clip(result)),
        None => Err("the reply has no result".to_owned()),
    }
}

/// `text` on one line, cut to 60 characters.
fn clip(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match line.char_indices().nth(60) {
        Some((at, _)) => format!("{}…", &line[..at]),
        None => line,
    }
}

/// The first three items, and how many more there are.
fn sample(items: &[&str]) -> String {
    let shown = items.iter().take(3).copied().collect::<Vec<_>>().join(", ");
    match items.len() {
        n if n > 3 => format!("{shown} and {} more", n - 3),
        _ => shown,
    }
}

fn rfc3339(at: OffsetDateTime) -> String {
    at.format(&Rfc3339).unwrap_or_else(|_| format!("{at:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::Role;
    use crate::index::{SessionSource, Turn};
    use crate::records::{Evidence, Tool, Verdict};
    use crate::redact::redact;
    use time::macros::datetime;

    const NOW: OffsetDateTime = datetime!(2026-11-08 12:00:00 UTC);

    /// A hippo home and a workspace with one entry note, in temporary
    /// directories, and a configuration with no model command.
    struct Setup {
        _home: tempfile::TempDir,
        _root: tempfile::TempDir,
        cfg: Config,
        ws: Workspace,
    }

    fn setup() -> Setup {
        let home = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let entry = "---\ntype: policy\n---\n# Start\n\nRead this first.\n";
        write(&root.path().join("notes/START HERE.md"), entry);
        let cfg = Config {
            home: home.path().to_path_buf(),
            workspace: Some(root.path().to_path_buf()),
            transcript_roots: Vec::new(),
            exclude_roots: Vec::new(),
            model_command: Vec::new(),
            ingest_interval_minutes: 10,
            dream_at: "03:30".into(),
            reviewer_command_override: None,
        };
        let ws = Workspace {
            root: root.path().to_path_buf(),
            notes_dir: "notes".into(),
            entry_note: "notes/START HERE.md".into(),
            required_notes: Vec::new(),
            budget_bytes: crate::DEFAULT_CONTEXT_BUDGET,
            log_path: "notes/Session log.md".into(),
            sample_tasks: Vec::new(),
            passes_off: Vec::new(),
            lessons: true,
            snapshot_keep_days: None,
            log_notes: None,
        };
        Setup {
            _home: home,
            _root: root,
            cfg,
            ws,
        }
    }

    fn env() -> Env {
        Env {
            now: NOW,
            launch_agents: None,
            exe: None,
            model_timeout: std::time::Duration::from_secs(10),
            check_model: true,
        }
    }

    fn report(s: &Setup) -> Report {
        run_with(&s.cfg, &s.ws, &env())
    }

    /// The report on macOS with the launchd jobs in `labels` installed.
    fn report_with_jobs(s: &Setup, labels: &[&str]) -> Report {
        let agents = s._home.path().join("LaunchAgents");
        let _ = fs::remove_dir_all(&agents);
        fs::create_dir_all(&agents).unwrap();
        for label in labels {
            write(&agents.join(format!("{label}.plist")), "<plist/>");
        }
        let env = Env {
            launch_agents: Some(agents),
            ..env()
        };
        run_with(&s.cfg, &s.ws, &env)
    }

    fn status(report: &Report, name: &str) -> Status {
        report
            .check(name)
            .unwrap_or_else(|| panic!("no {name} row in\n{report}"))
            .status
    }

    fn detail<'a>(report: &'a Report, name: &str) -> &'a str {
        &report.check(name).unwrap().detail
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    /// An index, and an ingest state as of `at`.
    fn ingested(s: &Setup, at: OffsetDateTime) -> Index {
        let index = Index::open(&s.cfg.home, true).unwrap();
        let state = IngestState {
            last_ingest: at,
            files_seen: 0,
            files_indexed: 0,
            scoped_workspace: None,
        };
        state.save(&s.cfg.home.join("ingest-state.json")).unwrap();
        index
    }

    fn proposal(n: u128, note: &str, before: &[u8], after: &[u8]) -> Proposal {
        Proposal {
            id: Ulid::from_parts(1, n),
            pass: "verify".into(),
            note: note.into(),
            kind: ProposalKind::Replace,
            before_hash: hex(&sha256(before)),
            after_hash: hex(&sha256(after)),
            old: "before".into(),
            new: "after".into(),
            evidence: vec![Evidence {
                session: "s1".into(),
                ordinal: 0,
            }],
            reason: "a test".into(),
        }
    }

    fn receipt(n: u128, proposal: &Proposal, verdict: Verdict, applied: bool) -> Receipt {
        Receipt {
            id: Ulid::from_parts(2, n),
            proposal: proposal.id,
            proposal_hash: "0".repeat(64),
            before_hash: proposal.before_hash.clone(),
            after_hash: proposal.after_hash.clone(),
            reviewer: "verify-reviewer".into(),
            invocation: "a test".into(),
            verdict,
            notes: String::new(),
            at: datetime!(2026-11-08 03:31:04 UTC),
            applied: applied.then_some(datetime!(2026-11-08 03:31:05 UTC)),
            snapshot: None,
        }
    }

    fn save_dreaming(s: &Setup, proposal: &Proposal, receipt: &Receipt) {
        let hippo = s.ws.hippo_dir();
        proposal
            .save(
                &hippo
                    .join("proposals")
                    .join(format!("{}.json", proposal.id)),
            )
            .unwrap();
        receipt
            .save(&hippo.join("receipts").join(format!("{}.json", receipt.id)))
            .unwrap();
    }

    fn session(id: &str, ended: Option<OffsetDateTime>, reason: Option<&str>) -> Session {
        Session {
            id: id.into(),
            tool: Tool::ClaudeCode,
            task: String::new(),
            task_seen: false,
            context_bytes_at_start: 0,
            started: datetime!(2026-11-01 03:00:00 UTC),
            ended,
            notes_touched: Vec::new(),
            handoff: None,
            handoff_derived: false,
            no_change_reason: reason.map(str::to_owned),
            context_start: None,
            context_prompt: None,
            derived: false,
        }
    }

    fn save_session(s: &Setup, record: &Session) {
        let path =
            s.ws.hippo_dir()
                .join("sessions")
                .join(format!("{}.json", record.id));
        record.save(&path).unwrap();
    }

    #[test]
    fn a_snapshot_directory_without_a_receipt_is_a_problem() {
        let s = setup();
        let id = "01JC9X4Q3A8M2K7N5P6R8S9T0V";
        let snapshot =
            s.ws.hippo_dir()
                .join("snapshots")
                .join(id)
                .join("notes/START HERE.md");
        write(&snapshot, "the note before a change");
        let r = report(&s);
        assert_eq!(status(&r, "snapshots"), Status::Problem);
        assert!(detail(&r, "snapshots").contains(id), "{r}");
        assert!(detail(&r, "snapshots").starts_with("24 bytes"), "{r}");
        assert!(r.has_problem());

        // A receipt for that proposal accounts for the directory.
        let mut rejected = proposal(1, "notes/START HERE.md", b"a", b"b");
        rejected.id = id.parse().unwrap();
        save_dreaming(
            &s,
            &rejected,
            &receipt(1, &rejected, Verdict::Reject, false),
        );
        let r = report(&s);
        assert_eq!(status(&r, "snapshots"), Status::Ok, "{r}");
    }

    #[test]
    fn a_fresh_workspace_has_no_last_success_problem() {
        let s = setup();
        let r = report(&s);
        assert_eq!(status(&r, "last success"), Status::Ok);
        assert!(
            detail(&r, "last success").starts_with("not set up yet: no dreaming run yet"),
            "{r}"
        );
        assert_eq!(status(&r, "last report"), Status::Ok);

        // A dry run alone is still "not set up yet".
        write(
            &s.ws.hippo_dir().join("reports/2026-11-07-dream.md"),
            &format!(
                "status: ok\n\n## Window\n\n- {}\n",
                crate::dream::report::DRY_RUN_LINE
            ),
        );
        assert_eq!(status(&report(&s), "last success"), Status::Ok);

        // Once a real dreaming report exists, a missing success counts.
        write(
            &s.ws.hippo_dir().join("reports/2026-11-08-dream.md"),
            "status: ok\n",
        );
        assert_eq!(status(&report(&s), "last success"), Status::Problem);
        let state = |at| DreamState {
            last_success: Some(at),
        };
        let path = s.ws.hippo_dir().join("dream-state.json");
        state(datetime!(2026-11-08 03:35:00 UTC))
            .save(&path)
            .unwrap();
        assert_eq!(status(&report(&s), "last success"), Status::Ok);
        state(datetime!(2026-11-06 23:59:59 UTC))
            .save(&path)
            .unwrap();
        // Old: a problem only when the dream job should have run it.
        let jobs = ["dev.hippo.ingest", "dev.hippo.dream"];
        assert_eq!(
            status(&report_with_jobs(&s, &jobs), "last success"),
            Status::Problem
        );
        assert_eq!(
            status(&report_with_jobs(&s, &[]), "last success"),
            Status::Warn
        );
        assert_eq!(status(&report(&s), "last success"), Status::Warn);
    }

    #[test]
    fn the_index_must_exist_and_be_recent() {
        let s = setup();
        let r = report(&s);
        assert_eq!(status(&r, "index"), Status::Problem);
        assert_eq!(detail(&r, "index"), "no index; run hippo ingest");
        assert_eq!(status(&r, "parse failures"), Status::Warn);

        ingested(&s, NOW - Duration::minutes(30));
        assert_eq!(status(&report(&s), "index"), Status::Ok);
        ingested(&s, NOW - Duration::minutes(31));
        // Stale with the ingest job installed: a problem.
        let r = report_with_jobs(&s, &["dev.hippo.ingest"]);
        assert_eq!(status(&r, "index"), Status::Problem);
        assert!(detail(&r, "index").contains("30 minutes"), "{r}");
        assert_eq!(
            detail(&r, "index").contains("permission prompt"),
            cfg!(target_os = "macos"),
            "{r}"
        );
        // A manual ingest with no schedule never turns into a problem.
        let r = report_with_jobs(&s, &[]);
        assert_eq!(status(&r, "index"), Status::Ok, "{r}");
        assert!(
            detail(&r, "index").contains("running hippo or hippo ingest"),
            "{r}"
        );
        // Where hippo cannot see the schedule (off macOS): a warning.
        let r = report(&s);
        assert_eq!(status(&r, "index"), Status::Warn, "{r}");
        assert!(
            detail(&r, "index").contains("cron job or systemd timer"),
            "{r}"
        );
    }

    #[test]
    fn linter_errors_are_problems_and_warnings_are_not() {
        let s = setup();
        assert_eq!(status(&report(&s), "linter"), Status::Ok);
        let bad = "---\nreview_after: soon\n---\n# Bad\n\nSee [[Missing]].\n";
        write(&s.ws.notes_root().join("Bad.md"), bad);
        let r = report(&s);
        assert_eq!(status(&r, "linter"), Status::Warn);
        assert!(
            detail(&r, "linter").contains("1 bad frontmatter, 1 dangling links"),
            "{r}"
        );
        write(
            &s.ws.notes_root().join("Broken.md"),
            "---\ntype: [unclosed\n---\n",
        );
        let r = report(&s);
        assert_eq!(status(&r, "linter"), Status::Problem);
        assert!(detail(&r, "linter").contains("Broken.md"), "{r}");
    }

    #[test]
    fn a_sample_task_over_budget_is_a_problem() {
        let mut s = setup();
        assert_eq!(status(&report(&s), "sample tasks"), Status::Ok);
        s.ws.sample_tasks = vec!["read the protocol".into()];
        assert_eq!(status(&report(&s), "sample tasks"), Status::Ok);
        s.ws.budget_bytes = 10;
        let r = report(&s);
        assert_eq!(status(&r, "sample tasks"), Status::Problem);
        assert!(
            detail(&r, "sample tasks").contains("read the protocol"),
            "{r}"
        );
    }

    #[test]
    fn a_failed_newest_report_is_a_problem() {
        let s = setup();
        let reports = s.ws.hippo_dir().join("reports");
        write(
            &reports.join("2026-11-07-dream.md"),
            "status: failed\nerror: old\n",
        );
        write(&reports.join("2026-11-08-dream.md"), "status: ok\n");
        write(&reports.join("2026-11-09-eval.md"), "status: failed\n");
        assert_eq!(status(&report(&s), "last report"), Status::Ok);
        let failed = "status: failed\nerror: model unreachable\n";
        write(&reports.join("2026-11-09-dream.md"), failed);
        let r = report(&s);
        assert_eq!(status(&r, "last report"), Status::Problem);
        assert!(
            detail(&r, "last report").contains("model unreachable"),
            "{r}"
        );
    }

    #[test]
    fn notes_too_big_for_dreaming_are_a_warning_of_the_last_report() {
        let s = setup();
        let reports = s.ws.hippo_dir().join("reports");
        write(
            &reports.join("2026-11-08-dream.md"),
            "status: ok\n\n## Omissions\n\
             - 2026-11-07T00:00:00Z to 2026-11-08T00:00:00Z verify notes/Big log.md: skipped: the note alone makes a 463886-byte prompt, over 200000\n\
             - 2026-11-07T00:00:00Z to 2026-11-08T00:00:00Z consolidate: omitted: note notes/Big log.md (350374 bytes)\n\
             - 2026-11-07T00:00:00Z to 2026-11-08T00:00:00Z consolidate: omitted: note notes/Other.md (13244 bytes)\n\
             - 2026-11-07T00:00:00Z to 2026-11-08T00:00:00Z verify: omitted: note notes/Small.md (rank 11 of 45, over max_notes 10)\n\
             \n## Proposals\n",
        );
        let r = report(&s);
        assert_eq!(status(&r, "last report"), Status::Warn);
        assert_eq!(
            detail(&r, "last report"),
            "2026-11-08-dream.md; 2 notes were too big for dreaming to read whole (split them)"
        );
    }

    #[test]
    fn a_failed_report_row_keeps_its_text_and_an_unreadable_one_names_the_file() {
        let s = setup();
        let reports = s.ws.hippo_dir().join("reports");
        write(
            &reports.join("2026-11-08-dream.md"),
            "status: failed\nerror: model unreachable\n",
        );
        assert_eq!(
            detail(&report(&s), "last report"),
            "2026-11-08-dream.md failed; error: model unreachable"
        );
        fs::create_dir_all(reports.join("2026-11-09-dream.md")).unwrap();
        let r = report(&s);
        assert_eq!(status(&r, "last report"), Status::Problem);
        let text = detail(&r, "last report");
        assert!(text.starts_with("2026-11-09-dream.md: "), "{text}");
        assert!(!text.contains(&reports.display().to_string()), "{text}");
    }

    #[test]
    fn a_dreamed_note_matching_neither_hash_is_a_problem() {
        let s = setup();
        let note = s.ws.root.join("notes/Webhooks.md");
        write(&note, "after\n");
        let changed = proposal(1, "notes/Webhooks.md", b"before\n", b"after\n");
        save_dreaming(&s, &changed, &receipt(1, &changed, Verdict::Confirm, true));
        assert_eq!(status(&report(&s), "receipts"), Status::Ok);

        // Applied this morning, then changed: a problem.
        fs::write(&note, "edited by an agent\n").unwrap();
        let r = report(&s);
        assert_eq!(status(&r, "receipts"), Status::Problem);
        assert!(detail(&r, "receipts").contains("notes/Webhooks.md"), "{r}");

        // Edited elsewhere with the change kept, as a merge leaves it: ok.
        fs::write(&note, "after\na line an agent added later\n").unwrap();
        let r = report(&s);
        assert_eq!(status(&r, "receipts"), Status::Ok, "{r}");
        assert!(detail(&r, "receipts").contains("keep their change"), "{r}");

        // The change overwritten by the old text: a problem again.
        fs::write(&note, "before\na line an agent added later\n").unwrap();
        assert_eq!(status(&report(&s), "receipts"), Status::Problem);

        // Back to the bytes before the change, as a revert leaves them.
        fs::write(&note, "before\n").unwrap();
        assert_eq!(status(&report(&s), "receipts"), Status::Ok);

        // A note dreamed two days ago whose change an edit removed: a
        // warning.
        let older_note = s.ws.root.join("notes/Retries.md");
        write(&older_note, "edited later\n");
        let older = proposal(2, "notes/Retries.md", b"old\n", b"new\n");
        let older_receipt = Receipt {
            applied: Some(datetime!(2026-11-06 03:31:05 UTC)),
            ..receipt(2, &older, Verdict::Confirm, true)
        };
        save_dreaming(&s, &older, &older_receipt);
        let r = report(&s);
        assert_eq!(status(&r, "receipts"), Status::Warn, "{r}");
        assert!(detail(&r, "receipts").contains("notes/Retries.md"), "{r}");

        // A fresh mismatch alongside it is still a problem.
        fs::write(&note, "edited again\n").unwrap();
        let r = report(&s);
        assert_eq!(status(&r, "receipts"), Status::Problem);
        let both = detail(&r, "receipts");
        assert!(
            both.contains("Webhooks.md") && both.contains("Retries.md"),
            "{r}"
        );
    }

    #[test]
    fn sessions_ending_unspecified_with_no_trace_are_at_most_a_warning() {
        let s = setup();
        let recent = Some(datetime!(2026-11-07 10:00:00 UTC));
        let old = Some(datetime!(2026-10-30 10:00:00 UTC));
        save_session(&s, &session("a", recent, Some("unspecified")));
        save_session(&s, &session("b", recent, None));
        // Not counted: ended over seven days ago, still open, or derived.
        save_session(&s, &session("c", old, Some("unspecified")));
        save_session(&s, &session("d", None, None));
        let derived = Session {
            derived: true,
            ..session("e", recent, Some("unspecified"))
        };
        save_session(&s, &derived);
        let r = report(&s);
        assert_eq!(status(&r, "sessions"), Status::Ok, "{r}");
        assert!(
            detail(&r, "sessions")
                .starts_with("2 declared sessions ended in the last 7 days: 1 ended unspecified"),
            "{r}"
        );

        save_session(&s, &session("f", recent, Some("unspecified")));
        let r = report(&s);
        assert_eq!(status(&r, "sessions"), Status::Warn, "{r}");
        assert!(detail(&r, "sessions").ends_with(", 2 left no trace"), "{r}");
    }

    #[test]
    fn a_session_end_command_run_by_another_tool_is_a_trace() {
        let s = setup();
        let mut index = ingested(&s, NOW);
        let recent = Some(datetime!(2026-11-07 10:00:00 UTC));
        let mut call = |id: &str, command: &str| {
            let turn = Turn {
                ordinal: 0,
                role: Role::Tool,
                ts: NOW,
                tool_name: Some("Bash".to_owned()),
                text: redact(&format!(
                    "Bash {}",
                    serde_json::json!({ "command": command })
                )),
            };
            let path = PathBuf::from(format!("/t/{id}.jsonl"));
            let source = SessionSource {
                id,
                tool: Tool::ClaudeCode,
                transcript_path: &path,
                file_hash: id,
            };
            index.replace_session(&source, &[turn]).unwrap();
        };
        call(
            "ran",
            "cd /w && python3 tools/notesctl.py session-end --no-change \"reading only\"",
        );
        call("searched", "grep -rn session-end tools/");
        call("hippo", "hippo session end");
        for id in ["ran", "searched", "hippo"] {
            save_session(&s, &session(id, recent, Some("unspecified")));
        }
        let r = report(&s);
        assert_eq!(status(&r, "sessions"), Status::Ok, "{r}");
        assert_eq!(
            detail(&r, "sessions"),
            "3 declared sessions ended in the last 7 days: 3 ended unspecified, \
             2 of them ran a session-end command, 0 others edited notes their transcripts show, \
             1 left no trace"
        );
    }

    /// A LaunchAgents directory whose dream job runs `program`.
    #[cfg(target_os = "macos")]
    fn dream_plist(dir: &Path, program: &Path) {
        write(
            &dir.join("dev.hippo.dream.plist"),
            &format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict>\
                 <key>ProgramArguments</key><array><string>{}</string><string>dream</string></array>\
                 </dict></plist>\n",
                program.display()
            ),
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_dream_job_binary_row_needs_another_pack_and_another_binary() {
        let _guard = crate::util::serial();
        let s = setup();
        let agents = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let exe = bin.path().join("hippo");
        write(&exe, "x");
        let other = bin.path().join("old-hippo");
        write(&other, "x");
        write(&agents.path().join("dev.hippo.ingest.plist"), "<plist/>");
        let with = |program: &Path| {
            dream_plist(agents.path(), program);
            let env = Env {
                launch_agents: Some(agents.path().to_path_buf()),
                exe: Some(exe.clone()),
                ..env()
            };
            run_with(&s.cfg, &s.ws, &env)
        };

        // Another binary, but only the core pack: no row.
        write(&s.ws.hippo_dir().join("packs/core/pack.toml"), "x");
        assert!(with(&other).check("dream job binary").is_none());

        // Another pack: a warning naming both binaries.
        write(&s.ws.hippo_dir().join("packs/glossary/pack.toml"), "x");
        let r = with(&other);
        assert_eq!(status(&r, "dream job binary"), Status::Warn, "{r}");
        assert!(detail(&r, "dream job binary").contains("old-hippo"), "{r}");

        // The same binary: no row.
        assert!(with(&exe).check("dream job binary").is_none());
    }

    #[test]
    fn no_schedule_is_not_set_up_yet_and_half_a_schedule_a_warning() {
        let cfg = setup().cfg;
        let dir = tempfile::tempdir().unwrap();
        let none = launchd_check(dir.path(), &cfg);
        assert_eq!(none.status, Status::Ok);
        assert!(
            none.detail.starts_with("not set up yet: no schedule")
                && none.detail.contains("hippo schedule install"),
            "{none:?}"
        );
        write(&dir.path().join("dev.hippo.ingest.plist"), "<plist/>");
        let check = launchd_check(dir.path(), &cfg);
        assert_eq!(check.status, Status::Warn);
        assert!(check.detail.contains("dev.hippo.dream") && !check.detail.contains("ingest,"));
        write(&dir.path().join("dev.hippo.dream.plist"), "<plist/>");
        assert_eq!(launchd_check(dir.path(), &cfg).status, Status::Ok);
        // Off macOS (no LaunchAgents directory given) there is no row.
        assert!(report(&setup()).check("launchd").is_none());
    }

    #[test]
    fn the_model_check_never_fails_the_report() {
        let mut s = setup();
        assert_eq!(status(&report(&s), "model"), Status::Warn);
        s.cfg.model_command = vec!["hippo-no-such-program".into()];
        let r = report(&s);
        assert_eq!(status(&r, "model"), Status::Warn);
        assert!(detail(&r, "model").contains("not on PATH"), "{r}");
        assert!(detail(&r, "model").contains("install Claude Code"), "{r}");
        #[cfg(unix)]
        {
            // Without --model nothing is sent.
            s.cfg.model_command = vec!["sh".into()];
            let env = Env {
                check_model: false,
                ..env()
            };
            let r = run_with(&s.cfg, &s.ws, &env);
            assert_eq!(status(&r, "model"), Status::Ok, "{r}");
            assert!(
                detail(&r, "model").starts_with("not checked (hippo doctor --model"),
                "{r}"
            );
        }
        #[cfg(unix)]
        {
            let _serial = crate::util::serial();
            let script = |body: &str| vec!["sh".into(), "-c".into(), body.to_owned()];
            s.cfg.model_command = script("cat > /dev/null; printf '{\"result\":\"pong\"}'");
            let r = report(&s);
            assert_eq!(status(&r, "model"), Status::Ok, "{r}");
            assert_eq!(detail(&r, "model"), "replied \"pong\"");
            s.cfg.model_command = script("echo not logged in >&2; exit 3");
            let r = report(&s);
            assert_eq!(status(&r, "model"), Status::Warn);
            assert!(detail(&r, "model").contains("not logged in"), "{r}");
            s.cfg.model_command = script("echo hello");
            assert_eq!(detail(&report(&s), "model"), "the reply is not JSON");
        }
    }

    #[test]
    fn parse_failures_and_record_gaps_are_warnings() {
        let s = setup();
        let mut index = ingested(&s, NOW);
        assert_eq!(status(&report(&s), "parse failures"), Status::Ok);
        assert_eq!(status(&report(&s), "session records"), Status::Ok);

        let failed = Path::new("/t/rollout-broken.jsonl");
        index
            .record_failure(Tool::Codex, failed, "h1", &redact("not JSON at line 3"))
            .unwrap();
        // An indexed session without a record, and a derived record whose
        // session the index lacks.
        let turn = Turn {
            ordinal: 0,
            role: Role::User,
            ts: NOW,
            tool_name: None,
            text: redact("hello"),
        };
        let source = SessionSource {
            id: "indexed-only",
            tool: Tool::ClaudeCode,
            transcript_path: Path::new("/t/indexed-only.jsonl"),
            file_hash: "h2",
        };
        index.replace_session(&source, &[turn]).unwrap();
        let stale = Session {
            derived: true,
            ..session("record-only", Some(NOW), None)
        };
        save_session(&s, &stale);
        write(
            &s.ws.hippo_dir().join("sessions/unreadable.json"),
            "{not json",
        );

        let r = report(&s);
        assert_eq!(status(&r, "parse failures"), Status::Warn);
        assert!(
            detail(&r, "parse failures").contains("rollout-broken.jsonl"),
            "{r}"
        );
        assert_eq!(status(&r, "session records"), Status::Warn);
        let gaps = detail(&r, "session records");
        for expected in ["unreadable.json", "record-only"] {
            assert!(gaps.contains(expected), "{gaps}");
        }
        // Another project's session has no record, and that is no gap.
        assert!(!gaps.contains("indexed-only"), "{gaps}");
        // Warnings only: the report still passes.
        assert!(!r.has_problem(), "{r}");
    }

    #[test]
    fn dsh_logs_in_a_format_hippo_does_not_read_are_named_with_their_versions() {
        let s = setup();
        let mut index = ingested(&s, NOW);
        // No dsh session, no row.
        assert!(report(&s).check("dsh logs").is_none());
        let turn = Turn {
            ordinal: 0,
            role: Role::User,
            ts: NOW,
            tool_name: None,
            text: redact("hello"),
        };
        let source = SessionSource {
            id: "dsh-s1",
            tool: Tool::Dsh,
            transcript_path: Path::new("/d/--p--/s1/session.v4.jsonl"),
            file_hash: "h1",
        };
        index.replace_session(&source, &[turn]).unwrap();
        assert_eq!(status(&report(&s), "dsh logs"), Status::Ok);
        for (path, version) in [
            ("/d/--p--/s2/session.v9.jsonl", 9),
            ("/d/--p--/s3/session.jsonl", 0),
        ] {
            let message = format!("{path}: {}", crate::adapters::dsh::unsupported(version));
            index
                .record_failure(Tool::Dsh, Path::new(path), "h", &redact(&message))
                .unwrap();
        }
        let r = report(&s);
        assert_eq!(status(&r, "dsh logs"), Status::Warn);
        let d = detail(&r, "dsh logs");
        for expected in [
            "2 dsh logs",
            "v0 (1), v9 (1)",
            "resuming",
            "newer hippo",
            "s2/session.v9.jsonl",
        ] {
            assert!(d.contains(expected), "{expected}: {d}");
        }
        assert!(!r.has_problem(), "{r}");
    }

    #[test]
    fn transcript_files_missing_from_the_index_are_warnings() {
        let mut s = setup();
        let transcripts = tempfile::tempdir().unwrap();
        write(&transcripts.path().join("p/one.jsonl"), "{}\n");
        s.cfg.transcript_roots = vec![transcripts.path().display().to_string()];
        ingested(&s, NOW);
        let r = report(&s);
        assert_eq!(status(&r, "unindexed files"), Status::Warn);
        assert!(detail(&r, "unindexed files").contains("one.jsonl"), "{r}");
    }

    #[test]
    fn a_configuration_that_fails_to_load_is_the_only_row() {
        let home = tempfile::tempdir().unwrap();
        let r = run_in(home.path(), false);
        assert_eq!(r.checks.len(), 1);
        assert_eq!(status(&r, "config"), Status::Problem);
        assert!(r.has_problem());
    }

    #[test]
    fn the_report_prints_as_a_table() {
        let r = Report {
            checks: vec![
                row("index", Status::Ok, "last ingest now"),
                row("snapshots", Status::Problem, "1 without a receipt"),
            ],
        };
        assert_eq!(
            r.to_string(),
            "check | status | detail\nindex | ok | last ingest now\n\
             snapshots | problem | 1 without a receipt\n"
        );
    }

    #[test]
    fn a_proposal_without_a_receipt_is_a_warning() {
        let s = setup();
        assert_eq!(
            detail(&report(&s), "proposals"),
            "0 proposals, each with a receipt"
        );
        let reviewed = Ulid::generate();
        let unreviewed = Ulid::generate();
        for id in [reviewed, unreviewed] {
            write(&s.ws.hippo_dir().join(format!("proposals/{id}.json")), "{}");
        }
        let receipt = Receipt {
            id: Ulid::generate(),
            proposal: reviewed,
            proposal_hash: String::new(),
            before_hash: String::new(),
            after_hash: String::new(),
            reviewer: "verify-reviewer".into(),
            invocation: String::new(),
            verdict: Verdict::Reject,
            notes: String::new(),
            at: NOW,
            applied: None,
            snapshot: None,
        };
        receipt
            .save(
                &s.ws
                    .hippo_dir()
                    .join(format!("receipts/{}.json", receipt.id)),
            )
            .unwrap();
        let r = report(&s);
        assert_eq!(status(&r, "proposals"), Status::Warn);
        assert_eq!(
            detail(&r, "proposals"),
            format!("1 of 2 proposals have no receipt (their review failed): {unreviewed}")
        );
    }
}
