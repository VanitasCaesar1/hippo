//! First open: where a new user is in the onboarding flow, and the commands
//! it runs.
//!
//! The pages are Welcome, Found (or Empty) and Reading; then the first home
//! with its banner, and the step-two sheet over it. Every change to hippo's
//! files is a `hippo` command shown on the page before it runs: `init`,
//! `ingest --progress` and `schedule install` for the first read, and
//! `init`, `schedule install` and `hooks install` for step two. The app
//! itself writes only its preferences file.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use hippo_core::detect::Detected;
use hippo_core::ingest::Progress;
use hippo_view::onboarding::{self, Found, Looked, NotesFolder, SoFar};

use crate::cli::{self, Limits, Stream};

/// A full-window onboarding page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Welcome,
    Found,
    Empty,
    Reading,
}

/// What a running sequence of commands is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// `init`, `ingest --progress`, `schedule install`.
    Read,
    /// `init --add-root <path>`, then look again.
    AddRoot,
    /// Step two's commands.
    StepTwo,
    /// `hippo review <receipts> --ok` from the Home's Review row.
    Review,
    /// `hippo lessons keep|dismiss <id>` from the Inbox or the Home.
    Lesson,
    /// `hippo lessons propose` from Ask's Make it a rule or Save as note.
    Propose,
    /// `hippo lessons propose --miss` from the x-ray's Add to inbox.
    Miss,
    /// `hippo xray unrequire --yes` from the x-ray's tune tip, confirmed.
    Tune,
    /// A change or a run confirmed in Manage.
    Manage,
    /// `hippo doctor` from Manage's Health card.
    Doctor,
}

/// Commands running in order on a background thread, the first failure
/// ending the sequence.
#[derive(Debug)]
pub struct Job {
    pub purpose: Purpose,
    pub stream: Arc<Mutex<Stream>>,
    /// Set by the background thread when the sequence ends: `Ok` when every
    /// command succeeded, else what failed.
    pub result: Arc<Mutex<Option<Result<(), String>>>>,
}

/// The step-two sheet.
#[derive(Debug, Clone)]
pub struct Sheet {
    pub folder: Option<NotesFolder>,
    pub dreaming: bool,
    pub briefing: bool,
    /// The output of the commands, once Set up was pressed.
    pub output: Vec<String>,
    pub running: bool,
    pub done: bool,
    pub error: Option<String>,
}

impl Sheet {
    pub fn new() -> Sheet {
        Sheet {
            folder: None,
            dreaming: true,
            briefing: true,
            output: Vec::new(),
            running: false,
            done: false,
            error: None,
        }
    }

    /// The commands Set up runs, when a usable folder is chosen.
    pub fn commands(&self) -> Option<Vec<Vec<String>>> {
        let folder = self.folder.as_ref().filter(|f| f.exists)?;
        onboarding::step_two_commands(folder, self.dreaming, self.briefing)
    }
}

/// Where the new user is.
#[derive(Debug, Default)]
pub struct Flow {
    /// The full-window page, or `None` for the shell.
    pub page: Option<Page>,
    /// The first reload decided whether onboarding starts.
    pub decided: bool,
    pub detected: Option<Detected>,
    pub found: Option<Found>,
    pub looked: Vec<Looked>,
    pub looking: bool,
    pub job: Option<Job>,
    pub progress: Option<Progress>,
    pub read_started: Option<Instant>,
    pub so_far: Option<SoFar>,
    /// When the numbers were last read from the rollup.
    pub so_far_at: Option<Instant>,
    /// When the home was last rebuilt while the read runs.
    pub home_at: Option<Instant>,
    /// Lines of the running command's output for the log box: the command
    /// and what it printed, without the progress JSON.
    pub log: Vec<String>,
    /// The first read has ended.
    pub read_done: bool,
    /// The first home's banner shows.
    pub banner: bool,
    pub sheet: Option<Sheet>,
    /// A command failed; the page says so.
    pub error: Option<String>,
    /// The Found page shows the commands the read runs.
    pub show_commands: bool,
    /// Folders switched off on the Connect page (`~` kept).
    pub off: Vec<String>,
    /// The window is short (under 680 points): pages use a tighter rhythm.
    pub compact: bool,
    /// The window is narrow (under 900 points): the Ask step stacks.
    pub compact_width: bool,
    /// The last `hippo review` the Home ran: its command line and result.
    pub review: Option<(String, Result<(), String>)>,
}

impl Flow {
    /// Whether the first read is running.
    pub fn reading(&self) -> bool {
        self.job
            .as_ref()
            .is_some_and(|j| j.purpose == Purpose::Read)
            && !self.read_done
    }

    /// Takes what the running job printed since the last call: progress
    /// lines update [`Flow::progress`], the rest go to the log. Returns
    /// whether a progress line said the rollup was just refreshed, and the
    /// job's result once it has ended.
    pub fn poll(&mut self) -> (bool, Option<Result<(), String>>) {
        let Some(job) = self.job.as_ref() else {
            return (false, None);
        };
        let lines = job
            .stream
            .lock()
            .map(|mut s| s.take_new())
            .unwrap_or_default();
        let result = job.result.lock().ok().and_then(|r| r.clone());
        let mut refreshed = false;
        for (err, line) in lines {
            if err
                && line.starts_with('{')
                && let Ok(p) = serde_json::from_str::<Progress>(&line)
            {
                refreshed |= p.rollup;
                self.progress = Some(p);
                continue;
            }
            self.log.push(line);
        }
        let keep = 200;
        if self.log.len() > keep {
            self.log.drain(..self.log.len() - keep);
        }
        (refreshed, result)
    }
}

/// How long since `t`, as a `time::Duration`.
pub fn elapsed(t: Instant) -> time::Duration {
    time::Duration::try_from(t.elapsed()).unwrap_or(time::Duration::ZERO)
}

/// The commands of the first read, as the Connect page shows them: the
/// folders switched off (`~` kept) go to `exclude_roots` before the read.
pub fn read_commands(off: &[String]) -> Vec<Vec<String>> {
    let mut out = vec![vec!["init".to_owned()]];
    for dir in off {
        out.push(vec![
            "config".into(),
            "add".into(),
            "exclude_roots".into(),
            dir.clone(),
            "--yes".into(),
        ]);
    }
    out.push(vec!["ingest".into(), "--progress".into()]);
    out.push(vec!["schedule".into(), "install".into()]);
    out
}

/// What each first-read command does, beside it on the Found page.
pub fn read_comment(args: &[String]) -> &'static str {
    match args.first().map(String::as_str) {
        Some("init") => "# writes ~/.hippo/config.toml naming these folders; no notes yet",
        Some("ingest") => "# builds the index in ~/.hippo; reads, never changes, the transcripts",
        Some("schedule") => "# a launchd job reads new sessions every 10 minutes",
        Some("config") => "# this folder is switched off: hippo skips it",
        _ => "",
    }
}

/// Runs `commands` in order with `binary`, stopping at the first that
/// fails; every line goes to `stream`, each command first as `$ hippo …`.
/// `ingest` has no time limit; the others have the usual 60 seconds.
/// Blocks; call it on a background thread.
pub fn run_job(
    binary: &Path,
    home: &Path,
    cwd: &Path,
    commands: &[Vec<String>],
    stream: Arc<Mutex<Stream>>,
) -> Result<(), String> {
    for args in commands {
        let line = onboarding::shell_line(args);
        if let Ok(mut s) = stream.lock() {
            s.lines.push((false, format!("$ {line}")));
        }
        // Reading and dreaming take as long as they take.
        let long = matches!(
            args.first().map(String::as_str),
            Some("ingest" | "dream" | "lessons")
        );
        let limits = (!long).then(Limits::commands);
        match cli::run_streaming(binary, home, cwd, args, limits, stream.clone()) {
            Ok(out) if out.success() => {}
            Ok(out) if out.timed_out => {
                return Err(format!("{line} did not finish in 60 seconds"));
            }
            Ok(out) => {
                let why = out
                    .stderr
                    .lines()
                    .rev()
                    .find(|l| !l.trim().is_empty() && !l.starts_with('{'))
                    .unwrap_or("")
                    .to_owned();
                let code = out
                    .code
                    .map_or("a signal".to_owned(), |c| format!("exit status {c}"));
                return Err(format!("{line} failed ({code}): {why}"));
            }
            Err(e) => return Err(format!("cannot run {}: {e}", binary.display())),
        }
    }
    Ok(())
}

/// Where the commands run: the user's home directory, outside any
/// workspace, so a bare `hippo init` writes a history-only configuration.
pub fn job_cwd() -> PathBuf {
    hippo_core::config::user_home().unwrap_or_else(|| PathBuf::from("/"))
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)] // a stub hippo script in a temporary directory
mod tests {

    #[test]
    fn folders_switched_off_are_excluded_before_the_read() {
        let c = read_commands(&["~/.claude-work/projects".to_owned()]);
        assert_eq!(c[0], ["init"]);
        assert_eq!(
            c[1],
            [
                "config",
                "add",
                "exclude_roots",
                "~/.claude-work/projects",
                "--yes"
            ]
        );
        assert_eq!(c[2], ["ingest", "--progress"]);
        assert_eq!(read_commands(&[]).len(), 3);
    }

    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn a_job_runs_in_order_streams_progress_and_stops_at_a_failure() {
        let dir = tempfile::tempdir().unwrap();
        let stub = dir.path().join("hippo");
        std::fs::write(
            &stub,
            "#!/bin/sh\nif [ \"$1\" = ingest ]; then\n  echo '{\"files_done\":1,\"files_total\":2,\"bytes_done\":5,\"bytes_total\":9,\"indexed\":1,\"skipped\":0,\"path\":\"/x\",\"modified\":null,\"rollup\":true}' >&2\n  echo 'indexed 2 files'\nfi\nif [ \"$1\" = fail ]; then echo 'no notes folder yet' >&2; exit 1; fi\necho \"ran $*\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        let stream = Arc::new(Mutex::new(Stream::default()));
        let commands = vec![
            vec!["init".to_string()],
            vec!["ingest".to_string(), "--progress".to_string()],
            vec!["fail".to_string()],
            vec!["never".to_string()],
        ];
        let result = run_job(&stub, dir.path(), dir.path(), &commands, stream.clone());
        let err = result.unwrap_err();
        assert!(
            err.contains("hippo fail failed (exit status 1): no notes folder yet"),
            "{err}"
        );
        let mut flow = Flow {
            job: Some(Job {
                purpose: Purpose::Read,
                stream,
                result: Arc::new(Mutex::new(Some(Err(err)))),
            }),
            ..Flow::default()
        };
        let (refreshed, result) = flow.poll();
        assert!(refreshed);
        assert!(result.unwrap().is_err());
        assert_eq!(flow.progress.as_ref().unwrap().files_done, 1);
        assert_eq!(flow.log.first().map(String::as_str), Some("$ hippo init"));
        assert!(flow.log.iter().any(|l| l == "indexed 2 files"));
        assert!(!flow.log.iter().any(|l| l.contains("never")));
    }

    #[test]
    fn the_sheet_runs_nothing_without_a_folder() {
        let mut sheet = Sheet::new();
        assert_eq!(sheet.commands(), None);
        let dir = tempfile::tempdir().unwrap();
        let brain = dir.path().join("brain");
        std::fs::create_dir(&brain).unwrap();
        std::fs::write(brain.join("START HERE.md"), "x").unwrap();
        sheet.folder = Some(onboarding::inspect_folder(&brain));
        sheet.briefing = false;
        let cmds = sheet.commands().unwrap();
        assert_eq!(cmds.len(), 2);
        assert_eq!(cmds[1], ["schedule", "install"]);
    }
}
