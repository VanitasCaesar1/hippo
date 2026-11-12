//! Which hippocamp installation the app uses, and running its commands.
//!
//! An app opened from the Finder does not see the shell's `HIPPOCAMP_HOME` or
//! `PATH`, so it reads both from the launchd job that `hippocamp schedule install`
//! wrote. The commands it runs (`revert`, `context --json`, `--version`) run
//! on a background thread, with a limit, and their output is captured whole.

#![allow(dead_code)] // the panels of items 7 and 8 use what this one does not yet

use std::ffi::OsString;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;

/// The property list of the ingest job, under the user's home.
pub const LAUNCH_AGENT: &str = "Library/LaunchAgents/dev.hippocamp.ingest.plist";
/// The program that turns a property list into JSON.
pub const PLUTIL: &str = "/usr/bin/plutil";
/// How long `revert` and `context` may run.
pub const COMMAND_LIMIT: Duration = Duration::from_secs(60);
/// How long `--version` may run.
pub const VERSION_LIMIT: Duration = Duration::from_secs(10);
/// How long after SIGTERM a command gets before it is killed.
pub const KILL_GRACE: Duration = Duration::from_secs(5);

/// Where the installation was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// The launchd job named both.
    LaunchAgent,
    /// The app's own environment had `HIPPOCAMP_HOME` (development, synthetic
    /// data), and `hippocamp` came from `PATH`.
    Environment,
    /// A default, for the reason given.
    Fallback(String),
}

/// The `hippocamp` binary and the home it runs with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installation {
    pub binary: PathBuf,
    pub home: PathBuf,
    pub source: Source,
}

impl Installation {
    /// The line the status bar shows when the installation is not the one the
    /// launchd job names; `None` when it is.
    pub fn status_line(&self) -> Option<String> {
        match &self.source {
            Source::LaunchAgent => None,
            Source::Environment => Some(format!(
                "HIPPOCAMP_HOME from the environment: {}",
                self.home.display()
            )),
            Source::Fallback(reason) => Some(format!("default hippocamp: {reason}")),
        }
    }

    /// The status bar's short note about the installation: two words, with
    /// the full line as its tooltip.
    /// `None` when the launchd job named both.
    pub fn status_note(&self) -> Option<InstallNote> {
        self.status_note_in(cfg!(debug_assertions))
    }

    /// [`Installation::status_note`] in a development build (`dev`) or a
    /// release build: a home from the environment is a developer's choice,
    /// so a release build does not say `dev home`.
    pub fn status_note_in(&self, dev: bool) -> Option<InstallNote> {
        let detail = self.status_line()?;
        Some(match self.source {
            Source::Environment if !dev => return None,
            Source::Environment => InstallNote {
                label: "dev home",
                detail,
                problem: false,
            },
            _ => InstallNote {
                label: "default hippocamp",
                detail,
                problem: true,
            },
        })
    }

    /// Whether the binary exists as a file.
    pub fn binary_exists(&self) -> bool {
        self.binary.is_file()
    }
}

/// The status bar's note about the installation in use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallNote {
    /// What the status bar shows: `dev home` or `default hippocamp`.
    pub label: &'static str,
    /// The full line, shown as the label's tooltip.
    pub detail: String,
    /// A fallback is a problem (drawn in the warning colour); a home from the
    /// environment is a choice.
    pub problem: bool,
}

/// What the launchd job says. A key it lacks is `None`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LaunchJob {
    pub program: Option<PathBuf>,
    pub home: Option<PathBuf>,
}

#[derive(Deserialize)]
struct PlistJson {
    #[serde(rename = "ProgramArguments", default)]
    program_arguments: Vec<String>,
    #[serde(rename = "EnvironmentVariables", default)]
    environment: std::collections::BTreeMap<String, String>,
}

/// The two keys of a property list's JSON form, as `plutil` prints it.
pub fn parse_job(json: &str) -> Result<LaunchJob, String> {
    let parsed: PlistJson = serde_json::from_str(json).map_err(|e| e.to_string())?;
    Ok(LaunchJob {
        program: parsed.program_arguments.first().map(PathBuf::from),
        home: parsed.environment.get("HIPPOCAMP_HOME").map(PathBuf::from),
    })
}

/// Reads a property list with `plutil -convert json -o -`. `-o -` prints to
/// standard output; without it `plutil` rewrites the file in place.
pub fn read_job(plutil: &Path, plist: &Path) -> Result<LaunchJob, String> {
    let output = Command::new(plutil)
        .args(["-convert", "json", "-o", "-"])
        .arg(plist)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("cannot run {}: {e}", plutil.display()))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(format!("plutil failed: {}", err.trim()));
    }
    parse_job(&String::from_utf8_lossy(&output.stdout))
}

/// The first executable file called `hippocamp` in the directories of `path`.
pub fn find_on_path(path: &OsString) -> Option<PathBuf> {
    std::env::split_paths(path)
        .map(|dir| dir.join("hippocamp"))
        .find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// What [`find`] looks at, so that tests can supply all of it.
pub struct Environment {
    /// The user's home directory.
    pub user_home: PathBuf,
    /// `HIPPOCAMP_HOME` in the app's own environment.
    pub hippocamp_home: Option<PathBuf>,
    /// `PATH` in the app's own environment.
    pub path: Option<OsString>,
    /// `plutil`.
    pub plutil: PathBuf,
}

impl Environment {
    /// The process's own environment.
    pub fn current() -> Environment {
        Environment {
            user_home: hippocamp_core::config::user_home().unwrap_or_else(|| PathBuf::from("/")),
            hippocamp_home: std::env::var_os("HIPPOCAMP_HOME")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from),
            path: std::env::var_os("PATH"),
            plutil: PathBuf::from(PLUTIL),
        }
    }
}

/// Finds the installation, with these fallbacks: the environment's
/// `HIPPOCAMP_HOME` first, then the launchd job's two keys, then
/// `~/.cargo/bin/hippocamp` and `~/.hippocamp`, each with its reason.
pub fn find(env: &Environment) -> Installation {
    let cargo_binary = env.user_home.join(".cargo/bin/hippocamp");
    let default_home = env.user_home.join(".hippocamp");

    if let Some(home) = &env.hippocamp_home {
        return match env.path.as_ref().and_then(find_on_path) {
            Some(binary) => Installation {
                binary,
                home: home.clone(),
                source: Source::Environment,
            },
            None => Installation {
                binary: cargo_binary,
                home: home.clone(),
                source: Source::Fallback(
                    "HIPPOCAMP_HOME is set but no hippocamp is on PATH; using ~/.cargo/bin/hippocamp".into(),
                ),
            },
        };
    }

    let plist = env.user_home.join(LAUNCH_AGENT);
    if !plist.exists() {
        return Installation {
            binary: cargo_binary,
            home: default_home,
            source: Source::Fallback(
                "no launchd job (run hippocamp schedule install); using ~/.cargo/bin/hippocamp and ~/.hippocamp"
                    .into(),
            ),
        };
    }
    match read_job(&env.plutil, &plist) {
        Err(error) => Installation {
            binary: cargo_binary,
            home: default_home,
            source: Source::Fallback(format!("the launchd job cannot be read ({error})")),
        },
        Ok(job) => match job.program {
            Some(binary) => Installation {
                binary,
                // The job sets no HIPPOCAMP_HOME when hippocamp ran with the default.
                home: job.home.unwrap_or(default_home),
                source: Source::LaunchAgent,
            },
            None => Installation {
                binary: cargo_binary,
                home: job.home.unwrap_or(default_home),
                source: Source::Fallback(
                    "the launchd job has no program; using ~/.cargo/bin/hippocamp".into(),
                ),
            },
        },
    }
}

/// A finished command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    pub stdout: String,
    pub stderr: String,
    /// `None` when a signal ended the command.
    pub code: Option<i32>,
    /// The limit passed and the command was stopped.
    pub timed_out: bool,
}

impl Output {
    pub fn success(&self) -> bool {
        self.code == Some(0) && !self.timed_out
    }
}

/// Limits for one run.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub run: Duration,
    pub grace: Duration,
}

impl Limits {
    pub fn commands() -> Limits {
        Limits {
            run: COMMAND_LIMIT,
            grace: KILL_GRACE,
        }
    }

    pub fn version() -> Limits {
        Limits {
            run: VERSION_LIMIT,
            grace: KILL_GRACE,
        }
    }
}

fn drain<R: Read + Send + 'static>(mut stream: R) -> thread::JoinHandle<String> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stream.read_to_end(&mut bytes);
        String::from_utf8_lossy(&bytes).into_owned()
    })
}

/// Runs `binary args` with `HIPPOCAMP_HOME` set to `home`, `HOME` inherited and
/// `cwd` as the working directory. Blocks; call it on a background thread.
/// Past the limit it sends SIGTERM with `/bin/kill`, then SIGKILL after the
/// grace period.
pub fn run(
    binary: &Path,
    home: &Path,
    cwd: &Path,
    args: &[String],
    limits: Limits,
) -> io::Result<Output> {
    let mut child = Command::new(binary)
        .args(args)
        .env("HIPPOCAMP_HOME", home)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = drain(child.stdout.take().expect("piped"));
    let stderr = drain(child.stderr.take().expect("piped"));

    let started = Instant::now();
    let mut timed_out = false;
    let mut terminated_at: Option<Instant> = None;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        let elapsed = started.elapsed();
        if elapsed >= limits.run && terminated_at.is_none() {
            timed_out = true;
            terminated_at = Some(Instant::now());
            let _ = Command::new("/bin/kill")
                .args(["-TERM", &child.id().to_string()])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        if terminated_at.is_some_and(|t| t.elapsed() >= limits.grace) {
            let _ = child.kill();
            break child.wait()?;
        }
        thread::sleep(Duration::from_millis(10));
    };
    Ok(Output {
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
        code: status.code(),
        timed_out,
    })
}

/// What a running command has printed so far, line by line, shared with
/// the window while [`run_streaming`] runs on a background thread.
#[derive(Debug, Default)]
pub struct Stream {
    /// `(from standard error, line)`, in the order read (the two streams
    /// interleave as their lines arrive).
    pub lines: Vec<(bool, String)>,
    /// How many of `lines` the window has taken.
    pub taken: usize,
    /// Set when the command ends.
    pub done: Option<Output>,
}

impl Stream {
    /// The lines printed since the last call.
    pub fn take_new(&mut self) -> Vec<(bool, String)> {
        let new = self.lines[self.taken..].to_vec();
        self.taken = self.lines.len();
        new
    }
}

fn pump<R: Read + Send + 'static>(
    stream: R,
    err: bool,
    sink: std::sync::Arc<std::sync::Mutex<Stream>>,
) -> thread::JoinHandle<String> {
    thread::spawn(move || {
        use std::io::BufRead;
        let mut all = String::new();
        let reader = io::BufReader::new(stream);
        for line in reader.split(b'\n').map_while(Result::ok) {
            let line = String::from_utf8_lossy(&line).into_owned();
            all.push_str(&line);
            all.push('\n');
            if let Ok(mut s) = sink.lock() {
                s.lines.push((err, line));
            }
        }
        all
    })
}

/// [`run`], with each line of output put into `sink` as it is printed, and
/// no time limit when `limits` is `None` (the first `hippocamp ingest`, which
/// can take many minutes). Blocks; call it on a background thread. The
/// finished [`Output`] is also put into `sink`.
pub fn run_streaming(
    binary: &Path,
    home: &Path,
    cwd: &Path,
    args: &[String],
    limits: Option<Limits>,
    sink: std::sync::Arc<std::sync::Mutex<Stream>>,
) -> io::Result<Output> {
    let mut child = Command::new(binary)
        .args(args)
        .env("HIPPOCAMP_HOME", home)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = pump(child.stdout.take().expect("piped"), false, sink.clone());
    let stderr = pump(child.stderr.take().expect("piped"), true, sink.clone());
    let started = Instant::now();
    let mut timed_out = false;
    let mut terminated_at: Option<Instant> = None;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if let Some(limits) = limits {
            if started.elapsed() >= limits.run && terminated_at.is_none() {
                timed_out = true;
                terminated_at = Some(Instant::now());
                let _ = Command::new("/bin/kill")
                    .args(["-TERM", &child.id().to_string()])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
            if terminated_at.is_some_and(|t| t.elapsed() >= limits.grace) {
                let _ = child.kill();
                break child.wait()?;
            }
        }
        thread::sleep(Duration::from_millis(20));
    };
    let output = Output {
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
        code: status.code(),
        timed_out,
    };
    if let Ok(mut s) = sink.lock() {
        s.done = Some(output.clone());
    }
    Ok(output)
}

/// The arguments of `hippocamp revert`: `--force` only for Revert anyway.
pub fn revert_args(receipt: &str, force: bool) -> Vec<String> {
    let mut args = vec!["revert".to_string()];
    if force {
        args.push("--force".into());
    }
    args.push(receipt.to_string());
    args
}

/// The arguments of `hippocamp context`, with `--task=` so that a task starting
/// with `-` is not read as a flag.
pub fn context_args(task: &str) -> Vec<String> {
    vec!["context".into(), format!("--task={task}"), "--json".into()]
}

/// `hippocamp --version`.
pub fn version_args() -> Vec<String> {
    vec!["--version".into()]
}

/// One part of the context that did not fit.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Omitted {
    pub part: String,
    pub bytes: usize,
}

/// The output of `hippocamp context --task <task> --json`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ContextOutput {
    pub text: String,
    pub bytes: usize,
    pub budget: usize,
    pub omitted: Vec<Omitted>,
    pub required: Vec<PathBuf>,
    pub relevant: Vec<PathBuf>,
}

/// Why a context preview is not there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextError {
    /// The installed CLI does not know `--json`.
    TooOld,
    /// The command failed; its standard error as it printed it.
    Failed(String),
    /// The command succeeded but its output is not what the app expects.
    Unreadable(String),
    /// The command did not finish in time.
    TimedOut,
}

impl ContextError {
    /// What the preview says.
    pub fn message(&self) -> String {
        match self {
            ContextError::TooOld => "The installed hippocamp is too old for the context preview. \
                 Update hippocamp: cargo install --path crates/hippocamp"
                .into(),
            ContextError::Failed(stderr) => stderr.trim().to_string(),
            ContextError::Unreadable(why) => {
                format!("hippocamp context printed output the app cannot read: {why}")
            }
            ContextError::TimedOut => "hippocamp context did not finish in 60 seconds".into(),
        }
    }
}

/// Reads a finished `hippocamp context --json` run.
pub fn parse_context(output: &Output) -> Result<ContextOutput, ContextError> {
    if output.timed_out {
        return Err(ContextError::TimedOut);
    }
    if output.success() {
        return serde_json::from_str(&output.stdout)
            .map_err(|e| ContextError::Unreadable(e.to_string()));
    }
    // Clap exits 2 for a flag it does not know, and says which.
    if output.code == Some(2) || output.stderr.contains("unexpected argument '--json'") {
        return Err(ContextError::TooOld);
    }
    Err(ContextError::Failed(output.stderr.clone()))
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)] // tests build temporary stub scripts and property lists
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    /// A `hippocamp` that prints its arguments and environment, and sleeps, fails
    /// or prints JSON when its first argument says so.
    fn stub(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("hippocamp");
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    const ECHO: &str = r#"
echo "args:$*"
echo "HIPPOCAMP_HOME=$HIPPOCAMP_HOME"
echo "HOME=$HOME"
echo "pwd=$(pwd -P)"
case "$1" in
  sleep) exec sleep 30 ;;
  fail) echo "boom" >&2; exit 3 ;;
esac
"#;

    fn limits() -> Limits {
        Limits {
            run: Duration::from_secs(20),
            grace: Duration::from_millis(200),
        }
    }

    #[test]
    fn revert_arguments_use_force_only_for_revert_anyway() {
        assert_eq!(revert_args("01ABC", false), ["revert", "01ABC"]);
        assert_eq!(revert_args("01ABC", true), ["revert", "--force", "01ABC"]);
    }

    #[test]
    fn context_task_is_one_argument_even_when_it_starts_with_a_dash() {
        assert_eq!(
            context_args("-fix --things"),
            ["context", "--task=-fix --things", "--json"]
        );
    }

    #[test]
    fn run_sets_home_inherits_home_and_uses_the_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let bin = stub(dir.path(), ECHO);
        let home = dir.path().join("hippocamp-home");
        let ws = dir.path().join("ws");
        fs::create_dir(&ws).unwrap();
        let out = run(&bin, &home, &ws, &revert_args("R1", true), limits()).unwrap();
        assert!(out.success(), "{out:?}");
        assert!(out.stdout.contains("args:revert --force R1"));
        assert!(
            out.stdout
                .contains(&format!("HIPPOCAMP_HOME={}", home.display()))
        );
        let user_home = std::env::var("HOME").unwrap();
        assert!(out.stdout.contains(&format!("HOME={user_home}")));
        let ws_real = fs::canonicalize(&ws).unwrap();
        assert!(out.stdout.contains(&format!("pwd={}", ws_real.display())));
    }

    #[test]
    fn run_captures_output_and_exit_code() {
        let dir = tempfile::tempdir().unwrap();
        let bin = stub(dir.path(), ECHO);
        let out = run(
            &bin,
            dir.path(),
            dir.path(),
            &["fail".to_string()],
            limits(),
        )
        .unwrap();
        assert_eq!(out.code, Some(3));
        assert_eq!(out.stderr, "boom\n");
        assert!(!out.timed_out);
        assert!(!out.success());
    }

    #[test]
    fn run_stops_a_command_past_its_limit() {
        let dir = tempfile::tempdir().unwrap();
        let bin = stub(dir.path(), ECHO);
        let started = Instant::now();
        let out = run(
            &bin,
            dir.path(),
            dir.path(),
            &["sleep".to_string()],
            Limits {
                run: Duration::from_millis(200),
                grace: Duration::from_millis(200),
            },
        )
        .unwrap();
        assert!(out.timed_out);
        assert_eq!(out.code, None, "ended by a signal");
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn run_reports_a_missing_binary() {
        let dir = tempfile::tempdir().unwrap();
        let err = run(
            &dir.path().join("nope"),
            dir.path(),
            dir.path(),
            &[],
            limits(),
        )
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn job_keys_are_read_and_missing_keys_are_none() {
        let job = parse_job(
            r#"{"Label":"dev.hippocamp.ingest","ProgramArguments":["/x/hippocamp","ingest"],
                "EnvironmentVariables":{"HIPPOCAMP_HOME":"/x/home","PATH":"/usr/bin"}}"#,
        )
        .unwrap();
        assert_eq!(job.program, Some(PathBuf::from("/x/hippocamp")));
        assert_eq!(job.home, Some(PathBuf::from("/x/home")));
        assert_eq!(parse_job("{}").unwrap(), LaunchJob::default());
        assert!(parse_job("not json").is_err());
    }

    fn env(dir: &Path) -> Environment {
        Environment {
            user_home: dir.to_path_buf(),
            hippocamp_home: None,
            path: None,
            plutil: PathBuf::from(PLUTIL),
        }
    }

    fn write_plist(user_home: &Path, program: Option<&str>, hippocamp_home: Option<&str>) {
        let path = user_home.join(LAUNCH_AGENT);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut xml = String::from(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict>\n\
             <key>Label</key><string>dev.hippocamp.ingest</string>\n",
        );
        if let Some(p) = program {
            xml += &format!(
                "<key>ProgramArguments</key><array><string>{p}</string><string>ingest</string></array>\n"
            );
        }
        if let Some(h) = hippocamp_home {
            xml += &format!(
                "<key>EnvironmentVariables</key><dict><key>HIPPOCAMP_HOME</key><string>{h}</string></dict>\n"
            );
        }
        xml += "</dict></plist>\n";
        fs::write(path, xml).unwrap();
    }

    #[test]
    fn without_a_launchd_job_the_defaults_are_used_and_the_reason_is_named() {
        let dir = tempfile::tempdir().unwrap();
        let found = find(&env(dir.path()));
        assert_eq!(found.binary, dir.path().join(".cargo/bin/hippocamp"));
        assert_eq!(found.home, dir.path().join(".hippocamp"));
        assert!(matches!(found.source, Source::Fallback(_)));
        assert!(found.status_line().unwrap().contains("no launchd job"));
        let note = found.status_note().unwrap();
        assert_eq!((note.label, note.problem), ("default hippocamp", true));
    }

    #[test]
    fn the_environment_wins_and_hippocamp_comes_from_path() {
        let dir = tempfile::tempdir().unwrap();
        let bin_dir = dir.path().join("bin");
        fs::create_dir(&bin_dir).unwrap();
        let bin = stub(&bin_dir, "exit 0");
        let mut e = env(dir.path());
        e.hippocamp_home = Some(dir.path().join("synthetic"));
        e.path = Some(std::env::join_paths([dir.path().join("empty"), bin_dir]).unwrap());
        let found = find(&e);
        assert_eq!(found.binary, bin);
        assert_eq!(found.home, dir.path().join("synthetic"));
        assert_eq!(found.source, Source::Environment);
        assert!(found.status_line().unwrap().contains("environment"));
        // The status bar says two calm words; the path is the tooltip.
        let note = found.status_note_in(true).unwrap();
        assert_eq!((note.label, note.problem), ("dev home", false));
        assert!(note.detail.contains("synthetic"));
        // A release build says nothing about it.
        assert_eq!(found.status_note_in(false), None);
    }

    #[test]
    fn a_set_home_with_no_hippocamp_on_path_falls_back_for_the_binary_only() {
        let dir = tempfile::tempdir().unwrap();
        let mut e = env(dir.path());
        e.hippocamp_home = Some(dir.path().join("synthetic"));
        e.path = Some(OsString::from(dir.path().join("empty")));
        let found = find(&e);
        assert_eq!(found.home, dir.path().join("synthetic"));
        assert_eq!(found.binary, dir.path().join(".cargo/bin/hippocamp"));
        assert!(matches!(found.source, Source::Fallback(_)));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_launchd_job_names_the_binary_and_the_home() {
        let dir = tempfile::tempdir().unwrap();
        write_plist(dir.path(), Some("/opt/hippocamp"), Some("/opt/home"));
        let before = fs::read(dir.path().join(LAUNCH_AGENT)).unwrap();
        let found = find(&env(dir.path()));
        assert_eq!(found.binary, PathBuf::from("/opt/hippocamp"));
        assert_eq!(found.home, PathBuf::from("/opt/home"));
        assert_eq!(found.source, Source::LaunchAgent);
        assert_eq!(found.status_line(), None);
        assert_eq!(found.status_note(), None);
        // Reading never rewrites the property list.
        assert_eq!(fs::read(dir.path().join(LAUNCH_AGENT)).unwrap(), before);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_job_without_a_home_uses_the_default_home_and_without_a_program_the_default_binary() {
        let dir = tempfile::tempdir().unwrap();
        write_plist(dir.path(), Some("/opt/hippocamp"), None);
        let found = find(&env(dir.path()));
        assert_eq!(found.binary, PathBuf::from("/opt/hippocamp"));
        assert_eq!(found.home, dir.path().join(".hippocamp"));
        assert_eq!(found.source, Source::LaunchAgent);

        write_plist(dir.path(), None, Some("/opt/home"));
        let found = find(&env(dir.path()));
        assert_eq!(found.binary, dir.path().join(".cargo/bin/hippocamp"));
        assert_eq!(found.home, PathBuf::from("/opt/home"));
        assert!(matches!(found.source, Source::Fallback(_)));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_property_list_plutil_cannot_read_is_a_fallback_with_its_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(LAUNCH_AGENT);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "this is not a property list").unwrap();
        let found = find(&env(dir.path()));
        assert!(matches!(found.source, Source::Fallback(_)));
        assert!(found.status_line().unwrap().contains("cannot be read"));
    }

    fn output(code: Option<i32>, stdout: &str, stderr: &str) -> Output {
        Output {
            stdout: stdout.into(),
            stderr: stderr.into(),
            code,
            timed_out: false,
        }
    }

    #[test]
    fn context_output_is_parsed_from_a_stubs_json() {
        let dir = tempfile::tempdir().unwrap();
        let json = r#"{"text":"hello","bytes":5,"budget":100,
            "omitted":[{"part":"relevant notes","bytes":40}],
            "required":["notes/a.md"],"relevant":["notes/b.md","notes/c.md"]}"#;
        let bin = stub(dir.path(), &format!("cat <<'EOF'\n{json}\nEOF"));
        let out = run(
            &bin,
            dir.path(),
            dir.path(),
            &context_args("task"),
            limits(),
        )
        .unwrap();
        let parsed = parse_context(&out).unwrap();
        assert_eq!(parsed.text, "hello");
        assert_eq!(parsed.budget, 100);
        assert_eq!(parsed.omitted[0].part, "relevant notes");
        assert_eq!(parsed.required, [PathBuf::from("notes/a.md")]);
        assert_eq!(parsed.relevant.len(), 2);
    }

    #[test]
    fn context_json_failures_are_told_apart() {
        let too_old = parse_context(&output(
            Some(2),
            "",
            "error: unexpected argument '--json' found\n",
        ));
        assert_eq!(too_old, Err(ContextError::TooOld));
        assert!(too_old.unwrap_err().message().contains("Update hippocamp"));

        let by_message = parse_context(&output(
            Some(1),
            "",
            "error: unexpected argument '--json' found",
        ));
        assert_eq!(by_message, Err(ContextError::TooOld));

        let other = parse_context(&output(Some(1), "", "context is over budget\n"));
        assert_eq!(
            other,
            Err(ContextError::Failed("context is over budget\n".into()))
        );
        assert_eq!(other.unwrap_err().message(), "context is over budget");

        assert!(matches!(
            parse_context(&output(Some(0), "not json", "")),
            Err(ContextError::Unreadable(_))
        ));
        let mut timed = output(None, "", "");
        timed.timed_out = true;
        assert_eq!(parse_context(&timed), Err(ContextError::TimedOut));
    }
}
