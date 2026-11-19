//! Model invocation (Invariant 4, in `docs/site/src/architecture.md`):
//! hippo runs a model only through an argv such as `model_command`.
//!
//! The element `{system_prompt}` is replaced by the system prompt, and the
//! user prompt is written to the child's stdin. Each call runs in its own
//! new folder, `$HIPPO_HOME/model-runs/<ulid>/` (0700), so it starts with
//! nothing to read, and two calls at once never see or remove each other's
//! files; the folder is removed when the call ends, whatever the outcome.
//! A call that starts removes sibling folders older than a day, which only
//! a crashed call leaves (`docs/designs/common.md`, rule 1). Its reply is the `result` string of the JSON it prints; a non-zero
//! exit, output without a string `result`, or 600 seconds without exit is
//! [`Error::Model`].

use std::fs;
use std::io::{self, Read, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Duration;

use serde_json::Value;

use crate::config::Config;
use crate::error::{Error, Result};
use crate::runners::{self, sandbox, Preset, ReplyMode, Role, Runner, RunnerRecord, SystemPrompt};

/// The argv element that is replaced by the system prompt.
pub const SYSTEM_PROMPT: &str = "{system_prompt}";

/// How long a model may run before it is killed.
pub const TIMEOUT: Duration = Duration::from_secs(600);

/// How old a folder under `model-runs/` must be before a starting call
/// removes it as a crashed call's leftover.
const STALE_RUN: Duration = Duration::from_secs(24 * 60 * 60);

/// How long to wait for the output pipes to close once the child exited. A
/// grandchild that keeps them open does not hold the run up for longer.
const DRAIN_GRACE: Duration = Duration::from_secs(5);

/// A model's reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelReply {
    /// The reply text: the `result` of the JSON the command printed, its
    /// whole stdout, or its reply file, as the runner's `reply` says.
    pub text: String,
    /// The argv joined by spaces with the system prompt replaced by
    /// `<system prompt: N bytes>`: the form a receipt's `invocation` takes.
    pub invocation: String,
    /// The runner that answered, for a receipt's `runner`.
    pub runner: Option<RunnerRecord>,
}

/// Runs the runner `role` resolves to (`docs/designs/runners.md`), moving
/// to the role's fallbacks, inside the runner's data domain, when the
/// runner's account is at 95% or more of a window before the call, or the
/// call fails with the CLI's usage-limit message. Any other error stops
/// the call. Which runner answered, and any fallback, is in the reply.
pub fn run_role(
    cfg: &Config,
    role: Role,
    system_prompt: &str,
    user_prompt: &str,
) -> Result<ModelReply> {
    let plan = runners::plan(cfg)?;
    let chain = plan.chain(role);
    let Some(first) = chain.first() else {
        return Err(Error::Runners(format!(
            "no runner for the {} role",
            role.key()
        )));
    };
    if let Some(why) = plan.refusal(&cfg.home, first.runner, role) {
        return Err(Error::Runners(why));
    }
    let usable: Vec<&Runner> = chain
        .iter()
        .filter(|step| step.refused.is_none())
        .map(|step| step.runner)
        .filter(|r| r.name == first.runner.name || plan.refusal(&cfg.home, r, role).is_none())
        .collect();
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let mut skipped: Option<String> = None;
    for (i, runner) in usable.iter().enumerate() {
        let last = i + 1 == usable.len();
        if !last {
            if let Some(why) = near_limit(runner, now) {
                skipped.get_or_insert_with(|| format!("{}: {why}", runner.name));
                continue;
            }
        }
        match run_runner(cfg, runner, system_prompt, user_prompt) {
            Err(Error::ModelLimit(why)) if !last => {
                skipped.get_or_insert_with(|| format!("{}: {why}", runner.name));
            }
            Ok(mut reply) => {
                if let Some(record) = reply.runner.as_mut() {
                    record.fallback_from = skipped.take();
                }
                return Ok(reply);
            }
            Err(e) => return Err(e),
        }
    }
    Err(Error::Model(format!(
        "no runner for the {} role could answer",
        role.key()
    )))
}

/// Why `runner`'s account should be skipped at `now`: a 5-hour or 7-day
/// window at 95% or more and not yet reset, as Fleet reads the account's
/// meter. `None` when it may be used, or when hippo cannot tell.
pub fn near_limit(runner: &Runner, now: i64) -> Option<String> {
    let dir = runner.account_dir()?;
    let meter = crate::usage::meter::read(&crate::usage::meter::settings_file(&dir))?;
    for (label, window) in [("5-hour", meter.five_hour), ("7-day", meter.seven_day)] {
        if let Some(w) = window {
            if w.used_percent >= 95.0 && !w.reset_passed(now) {
                return Some(format!("{label} window at {:.0}%", w.used_percent));
            }
        }
    }
    None
}

/// `text` with the inline system markers broken, so text inside the user
/// prompt cannot open or close the system part early.
pub fn escape_system_markers(text: &str) -> String {
    text.replace(INLINE_START, "<<\\<SYSTEM>>>")
        .replace(INLINE_END, "<<\\<END SYSTEM>>>")
}

/// The markers around an inline system prompt.
pub const INLINE_START: &str = "<<<SYSTEM>>>";
pub const INLINE_END: &str = "<<<END SYSTEM>>>";

/// Runs `runner` once, in its own `$HIPPO_HOME/model-runs/<ulid>/` (0700,
/// removed afterwards): placeholders filled, the system prompt passed as
/// the runner's argv says, the user prompt on stdin, the environment
/// limited and the call sandboxed (except an implicit runner of the older
/// keys), and the reply read as the runner's `reply` says.
pub fn run_runner(
    cfg: &Config,
    runner: &Runner,
    system_prompt: &str,
    user_prompt: &str,
) -> Result<ModelReply> {
    in_run_dir(&cfg.home, |dir| {
        call_in(cfg, dir, runner, system_prompt, user_prompt)
    })
}

/// What a canary run found (`docs/designs/runners.md`, The sandbox).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Canary {
    /// The reply held the word planted in the run folder: the model can
    /// read files there, so it has a read tool.
    pub read_run_folder: bool,
    /// The reply held the word planted in `$HIPPO_HOME`, outside the run
    /// folder: the runner is not sandboxed, whatever its profile says.
    pub read_outside: bool,
    /// The reply, cut short.
    pub reply: String,
}

impl Canary {
    /// Neither word came back.
    pub fn passed(&self) -> bool {
        !self.read_run_folder && !self.read_outside
    }
}

/// Plants two files holding random words, one in the call's run folder and
/// one in `$HIPPO_HOME` outside it, asks `runner` to print both, and says
/// which came back. One model call. The outside file is removed after.
pub fn canary(cfg: &Config, runner: &Runner) -> Result<Canary> {
    let inside = format!("CANARY-{}", ulid::Ulid::generate());
    let outside = format!("CANARY-{}", ulid::Ulid::generate());
    let outside_file = cfg
        .home
        .join(format!("canary-{}.txt", ulid::Ulid::generate()));
    crate::util::write_atomic(&outside_file, format!("{outside}\n").as_bytes())?;
    let question = format!(
        "Read the file canary.txt in your current working directory and the file {}, \
         then reply with their exact contents and nothing else.",
        outside_file.display()
    );
    let reply = in_run_dir(&cfg.home, |dir| {
        crate::util::write_atomic(&dir.join("canary.txt"), format!("{inside}\n").as_bytes())?;
        call_in(cfg, dir, runner, "You answer in one short line.", &question)
    });
    let _ = fs::remove_file(&outside_file);
    let reply = reply?;
    let canary = Canary {
        read_run_folder: reply.text.contains(&inside),
        read_outside: reply.text.contains(&outside),
        reply: clip(&reply.text),
    };
    let record = runners::canary_file(&cfg.home, &runner.name);
    if let Some(dir) = record.parent() {
        crate::perms::create_private_dir(dir)?;
    }
    let verdict = if canary.passed() { "passed" } else { "failed" };
    crate::util::write_atomic(&record, format!("{verdict}\n").as_bytes())?;
    Ok(canary)
}

/// Runs `f` in a new `<home>/model-runs/<ulid>/`, removed afterwards,
/// after removing leftovers older than a day.
fn in_run_dir<T>(home: &Path, f: impl FnOnce(&Path) -> Result<T>) -> Result<T> {
    let runs = home.join("model-runs");
    crate::perms::create_private_dir(&runs)?;
    sweep_stale(&runs, STALE_RUN);
    let dir = runs.join(ulid::Ulid::generate().to_string());
    // `create_dir`, not `create_dir_all`: a folder that exists already is
    // another call's, never shared.
    fs::create_dir(&dir).map_err(|e| Error::io_at(&dir, e))?;
    crate::perms::create_private_dir(&dir)?;
    let out = f(&dir);
    let _ = fs::remove_dir_all(&dir);
    out
}

fn call_in(
    cfg: &Config,
    dir: &Path,
    runner: &Runner,
    system_prompt: &str,
    user_prompt: &str,
) -> Result<ModelReply> {
    let template = runner.argv();
    let system_file = dir.join("system-prompt.txt");
    let reply_file = dir.join("reply.txt");
    let mode = runner.system_prompt();
    if mode == SystemPrompt::File {
        crate::util::write_atomic(&system_file, system_prompt.as_bytes())?;
    }
    let dir_text = dir.display().to_string();
    // An argument cannot hold a NUL byte, which a note or a transcript
    // turn can: each becomes U+FFFD.
    let mut argv: Vec<String> = template
        .iter()
        .map(|arg| {
            let arg = if arg == runners::SYSTEM_PROMPT {
                system_prompt.to_owned()
            } else {
                arg.replace(
                    runners::SYSTEM_PROMPT_FILE,
                    &system_file.display().to_string(),
                )
                .replace(runners::REPLY_FILE, &reply_file.display().to_string())
                .replace(runners::RUN_DIR, &dir_text)
            };
            arg.replace('\0', "\u{FFFD}")
        })
        .collect();
    let stdin = match mode {
        SystemPrompt::Inline => format!(
            "{INLINE_START}\n{system_prompt}\n{INLINE_END}\n\n{}",
            escape_system_markers(user_prompt)
        ),
        _ => user_prompt.to_owned(),
    };
    let Some(program) = argv.first().cloned() else {
        return Err(Error::Model("the model command is empty".into()));
    };
    let mut sandboxed = false;
    let env = if runner.legacy {
        None
    } else {
        let vars: Vec<(String, String)> = std::env::vars().collect();
        let env = runners::environment(runner, &vars, dir);
        let path = env
            .iter()
            .find(|(k, _)| k == "PATH")
            .map(|(_, v)| v.clone())
            .unwrap_or_default();
        let found = find_on(&program, &path)
            .ok_or_else(|| Error::Model(spawn_error(&program, &io::ErrorKind::NotFound.into())))?;
        argv[0] = found.display().to_string();
        let policy = policy_for(cfg, runner, dir, &found, &env);
        if let Some(wrapped) = sandbox::wrap(&argv, &policy) {
            argv = wrapped;
            sandboxed = true;
        }
        Some(env)
    };
    let out = spawn(dir, &argv, env.as_deref(), &stdin, runner.timeout, &program)?;
    let shown = || {
        let mut name = runner.name.clone();
        if !runner.legacy {
            name = format!("runner {name} ({program})");
        } else {
            name = program.clone();
        }
        name
    };
    if !out.status.success() {
        let detail = failure_reason(&out.stdout, &out.stderr);
        let limit = match runner.preset {
            Preset::Claude => {
                detail
                    .as_deref()
                    .is_some_and(crate::usage::is_limit_message)
                    || crate::usage::is_limit_message(&String::from_utf8_lossy(&out.stdout))
            }
            Preset::Command => out
                .status
                .code()
                .is_some_and(|c| runner.limit_exit_codes.contains(&c)),
        };
        let detail = detail.map(|why| format!(": {why}")).unwrap_or_default();
        if limit {
            return Err(Error::ModelLimit(format!("usage limit{detail}")));
        }
        return Err(Error::Model(format!(
            "{} exited with {}{detail}",
            shown(),
            out.status
        )));
    }
    let text = match runner.reply {
        ReplyMode::JsonResult => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let reply: Value = serde_json::from_str(stdout.trim())
                .map_err(|_| Error::Model(format!("{} did not print JSON", shown())))?;
            let Some(text) = reply.get("result").and_then(Value::as_str) else {
                return Err(Error::Model(format!(
                    "{} printed JSON without a result string",
                    shown()
                )));
            };
            text.to_owned()
        }
        ReplyMode::Stdout => String::from_utf8_lossy(&out.stdout).trim().to_owned(),
        ReplyMode::File => fs::read_to_string(&reply_file)
            .map_err(|_| Error::Model(format!("{} wrote no reply file", shown())))?,
    };
    Ok(ModelReply {
        text,
        invocation: invocation(&template, system_prompt),
        runner: Some(RunnerRecord::of(&cfg.home, runner, sandboxed)),
    })
}

/// The sandbox policy for one call of `runner` in `dir`, its program found
/// at `found`, with the environment `env` it will get.
fn policy_for(
    cfg: &Config,
    runner: &Runner,
    dir: &Path,
    found: &Path,
    env: &[(String, String)],
) -> sandbox::Policy {
    let home = crate::config::user_home();
    let mut policy = sandbox::Policy {
        run_dir: dir.to_path_buf(),
        home: home.clone(),
        hippo_home: Some(cfg.home.clone()),
        workspace: cfg.workspace.clone(),
        read: sandbox::install_paths(found, home.as_deref()),
        write: Vec::new(),
    };
    if runner.preset == Preset::Claude {
        let account = env
            .iter()
            .find(|(k, _)| k == "CLAUDE_CONFIG_DIR")
            .map(|(_, v)| PathBuf::from(v))
            .or_else(|| runner.account_dir());
        if let Some(account) = account {
            policy.write = sandbox::claude_paths(&account, home.as_deref());
        }
    }
    policy
}

/// `program` when it is a path, else the first executable file of that
/// name in the folders of `path`.
fn find_on(program: &str, path: &str) -> Option<PathBuf> {
    let p = Path::new(program);
    if p.components().count() > 1 {
        return p.is_file().then(|| p.to_path_buf());
    }
    std::env::split_paths(path)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

/// `command` joined by spaces with each `{system_prompt}` element replaced
/// by `<system prompt: N bytes>`, `N` being the system prompt's length.
pub fn invocation(command: &[String], system_prompt: &str) -> String {
    command
        .iter()
        .map(|arg| {
            if arg == SYSTEM_PROMPT {
                format!("<system prompt: {} bytes>", system_prompt.len())
            } else {
                arg.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Runs `command` as an implicit runner of the older keys does, in its own
/// run folder: the tests' way in.
#[cfg(test)]
fn run_with_timeout(
    home: &Path,
    command: &[String],
    system_prompt: &str,
    user_prompt: &str,
    timeout: Duration,
) -> Result<ModelReply> {
    let runner = Runner {
        name: "custom".into(),
        preset: Preset::Command,
        data_domain: "custom".into(),
        argv: command.to_vec(),
        legacy: true,
        timeout,
        ..Runner::default_claude()
    };
    let mut cfg: Config = toml::from_str("").expect("an empty configuration loads");
    cfg.home = home.to_path_buf();
    run_runner(&cfg, &runner, system_prompt, user_prompt)
}

/// Removes what sits in `runs` unchanged for longer than `age`: folders a
/// crashed call left, and files an older hippo left when every call shared
/// the folder. Links are removed, not followed. Failures are ignored; the
/// next call tries again.
fn sweep_stale(runs: &Path, age: Duration) {
    let Ok(entries) = fs::read_dir(runs) else {
        return;
    };
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        let Ok(meta) = fs::symlink_metadata(entry.path()) else {
            continue;
        };
        let old = meta
            .modified()
            .ok()
            .and_then(|m| now.duration_since(m).ok())
            .is_some_and(|d| d > age);
        if !old {
            continue;
        }
        let _ = if meta.is_dir() {
            fs::remove_dir_all(entry.path())
        } else {
            fs::remove_file(entry.path())
        };
    }
}

/// Waits for `child` to exit, at most `timeout`, by polling: a SIGCHLD
/// handler (as `wait_timeout` uses) never fired when hippo was started by
/// the desktop app, which left `hippo ask` waiting out the whole timeout
/// for a model that had long exited.
fn wait_for(
    child: &mut std::process::Child,
    timeout: Duration,
) -> io::Result<Option<std::process::ExitStatus>> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if std::time::Instant::now() >= deadline {
            return Ok(None);
        }
        thread::sleep(Duration::from_millis(50));
    }
}

/// What a finished child printed.
struct Output {
    status: std::process::ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

/// Starts `argv` in `workdir` with `env` (the whole inherited environment
/// when `None`), writes `stdin`, and waits at most `timeout`. `shown` names
/// the program in errors.
fn spawn(
    workdir: &Path,
    argv: &[String],
    env: Option<&[(String, String)]>,
    stdin: &str,
    timeout: Duration,
    shown: &str,
) -> Result<Output> {
    let Some((program, args)) = argv.split_first() else {
        return Err(Error::Model("the model command is empty".into()));
    };
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(workdir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(env) = env {
        command.env_clear();
        command.envs(env.iter().map(|(k, v)| (k, v)));
    }
    let mut child = command
        .spawn()
        .map_err(|e| Error::Model(spawn_error(shown, &e)))?;

    // Feed stdin and drain both outputs on their own threads, so a child
    // that writes a lot before it reads, or the reverse, cannot deadlock.
    if let Some(mut pipe) = child.stdin.take() {
        let input = stdin.as_bytes().to_vec();
        // A child that exits without reading stdin closes the pipe; the
        // write then fails, which is not an error of the run.
        thread::spawn(move || {
            let _ = pipe.write_all(&input);
        });
    }
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());

    let status = match wait_for(&mut child, timeout)
        .map_err(|e| Error::Model(format!("waiting for {shown}: {e}")))?
    {
        Some(status) => status,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::Model(format!(
                "{shown} did not exit within {} seconds",
                timeout.as_secs()
            )));
        }
    };
    Ok(Output {
        status,
        stdout: stdout.recv_timeout(DRAIN_GRACE).unwrap_or_default(),
        stderr: stderr.recv_timeout(DRAIN_GRACE).unwrap_or_default(),
    })
}

/// Why a model command that exited non-zero failed: the `result` of the JSON it printed on stdout (the `claude` CLI
/// writes `{"is_error":true,"result":"Not logged in · Please run /login"}`
/// there), else the first non-empty line of stderr, else of stdout.
pub(crate) fn failure_reason(stdout: &[u8], stderr: &[u8]) -> Option<String> {
    let out = String::from_utf8_lossy(stdout);
    if let Ok(reply) = serde_json::from_str::<Value>(out.trim()) {
        if let Some(result) = reply.get("result").and_then(Value::as_str) {
            if !result.trim().is_empty() {
                return Some(clip(result.trim()));
            }
        }
    }
    let first = |bytes: &[u8]| {
        String::from_utf8_lossy(bytes)
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(clip)
    };
    first(stderr).or_else(|| first(stdout))
}

/// The error for a model command that could not start: when it is not on
/// `PATH`, the two ways out.
pub(crate) fn spawn_error(program: &str, e: &io::Error) -> String {
    if e.kind() == io::ErrorKind::NotFound {
        format!(
            "{program} is not on PATH: install Claude Code (it provides the claude command) and sign in, \
             or add a runner for another model CLI: \
             hippo runners add <name> --preset command --argv '[\"<program>\", \"{{system_prompt}}\"]' --domain <label> --yes"
        )
    } else {
        format!("cannot run {program}: {e}")
    }
}

/// Reads `pipe` to its end on a thread and sends the bytes.
pub(crate) fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> mpsc::Receiver<Vec<u8>> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut bytes);
        }
        let _ = sender.send(bytes);
    });
    receiver
}

/// [`drain`] with a bound: reads `pipe` on a thread and keeps at most `cap`
/// bytes. At the first byte past `cap` the thread stops reading, sets the
/// returned flag and sends what it holds, so the caller can kill the child
/// that writes more than it may; memory never holds more than `cap` + one
/// read.
pub(crate) fn drain_capped<R: Read + Send + 'static>(
    pipe: Option<R>,
    cap: usize,
) -> (mpsc::Receiver<Vec<u8>>, Arc<AtomicBool>) {
    let (sender, receiver) = mpsc::channel();
    let exceeded = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&exceeded);
    thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut pipe) = pipe {
            let mut chunk = [0u8; 8192];
            loop {
                match pipe.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let room = cap.saturating_sub(bytes.len());
                        bytes.extend_from_slice(&chunk[..n.min(room)]);
                        if n > room {
                            flag.store(true, Ordering::SeqCst);
                            break;
                        }
                    }
                }
            }
        }
        let _ = sender.send(bytes);
    });
    (receiver, exceeded)
}

/// At most 200 characters of `text`.
pub(crate) fn clip(text: &str) -> String {
    match text.char_indices().nth(200) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text.to_owned(),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::Instant;

    /// `sh -c <body> sh {system_prompt}`: in `body`, `$1` is the system
    /// prompt and stdin is the user prompt.
    fn sh(body: &str) -> Vec<String> {
        ["sh", "-c", body, "sh", SYSTEM_PROMPT]
            .map(String::from)
            .to_vec()
    }

    /// Runs `command` with a 20-second timeout, one spawning test at a
    /// time (see `util::serial`).
    fn call(home: &Path, command: &[String], system: &str, user: &str) -> Result<ModelReply> {
        let _serial = crate::util::serial();
        run_with_timeout(home, command, system, user, Duration::from_secs(20))
    }

    #[test]
    fn a_capped_drain_stops_at_the_cap_on_an_endless_pipe() {
        let (receiver, exceeded) = drain_capped(Some(io::repeat(b'a')), 1000);
        let bytes = receiver.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(bytes.len(), 1000);
        assert!(exceeded.load(Ordering::SeqCst));
        let (receiver, exceeded) = drain_capped(Some(io::Cursor::new(vec![b'b'; 1000])), 1000);
        assert_eq!(
            receiver
                .recv_timeout(Duration::from_secs(10))
                .unwrap()
                .len(),
            1000
        );
        assert!(
            !exceeded.load(Ordering::SeqCst),
            "exactly the cap is within it"
        );
    }

    #[test]
    fn substitutes_the_system_prompt_and_writes_the_user_prompt_to_stdin() {
        let home = tempfile::tempdir().unwrap();
        let command = sh(r#"input=$(cat); printf '{"result":"%s|%s"}' "$1" "$input""#);
        let reply = call(home.path(), &command, "be brief", "hello there").unwrap();
        assert_eq!(reply.text, "be brief|hello there");
        assert_eq!(
            reply.invocation,
            format!("sh -c {} sh <system prompt: 8 bytes>", command[2])
        );
    }

    #[test]
    fn a_failure_shows_the_models_own_reason() {
        let home = tempfile::tempdir().unwrap();
        // The claude CLI signed out: the reason is in the JSON on stdout.
        let command = sh(
            r#"cat >/dev/null; printf '{"is_error":true,"result":"Not logged in · Please run /login"}'; exit 1"#,
        );
        let err = call(home.path(), &command, "s", "u")
            .unwrap_err()
            .to_string();
        assert!(
            err.ends_with(": Not logged in · Please run /login"),
            "{err}"
        );
        // Else stderr's first line, else stdout's.
        let command = sh("cat >/dev/null; echo; echo 'bad flag' >&2; exit 2");
        let err = call(home.path(), &command, "s", "u")
            .unwrap_err()
            .to_string();
        assert!(err.ends_with(": bad flag"), "{err}");
        let command = sh("cat >/dev/null; echo 'quota reached'; exit 3");
        let err = call(home.path(), &command, "s", "u")
            .unwrap_err()
            .to_string();
        assert!(err.ends_with(": quota reached"), "{err}");
    }

    #[test]
    fn a_missing_program_names_the_two_ways_out() {
        let home = tempfile::tempdir().unwrap();
        let command = vec!["hippo-no-such-model-cli".to_owned()];
        let err = call(home.path(), &command, "s", "u")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("hippo-no-such-model-cli is not on PATH"),
            "{err}"
        );
        assert!(err.contains("install Claude Code"), "{err}");
        assert!(err.contains("hippo runners add"), "{err}");
    }

    #[test]
    fn invocation_keeps_empty_arguments_and_elides_every_system_prompt() {
        let command = [
            "claude",
            "-p",
            "--tools",
            "",
            "--system-prompt",
            SYSTEM_PROMPT,
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(
            invocation(&command, "abcd"),
            "claude -p --tools  --system-prompt <system prompt: 4 bytes>"
        );
    }

    #[test]
    fn runs_in_its_own_empty_folder_removed_afterwards() {
        let home = tempfile::tempdir().unwrap();
        let runs = home.path().join("model-runs");
        fs::create_dir_all(runs.join("recent")).unwrap();
        fs::write(runs.join("recent.txt"), "another call's").unwrap();
        let command = sh(
            r#"cat >/dev/null; printf '{"result":"%s %s %s"}' "$(basename "$(dirname "$PWD")")" "$(basename "$PWD" | wc -c | tr -d ' ')" "$(ls -A | wc -l | tr -d ' ')""#,
        );
        let reply = call(home.path(), &command, "s", "u").unwrap();
        // In model-runs/<26-character ulid>/, empty.
        assert_eq!(reply.text, "model-runs 27 0");
        // Its folder is gone; a recent sibling is another call's and stays.
        let mut left: Vec<_> = fs::read_dir(&runs)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(left, ["recent", "recent.txt"]);
        #[cfg(unix)]
        assert_eq!(crate::perms::mode(&runs), Some(0o700));
        // A failed call removes its folder too.
        let failing = sh("cat >/dev/null; exit 3");
        assert!(call(home.path(), &failing, "s", "u").is_err());
        assert_eq!(fs::read_dir(&runs).unwrap().count(), 2);
    }

    #[test]
    fn a_starting_call_sweeps_only_leftovers_older_than_a_day() {
        let home = tempfile::tempdir().unwrap();
        let runs = home.path().join("model-runs");
        fs::create_dir_all(runs.join("crashed")).unwrap();
        fs::create_dir_all(runs.join("running")).unwrap();
        let old = std::time::SystemTime::now() - Duration::from_secs(25 * 60 * 60);
        let dir = fs::File::open(runs.join("crashed")).unwrap();
        dir.set_modified(old).unwrap();
        sweep_stale(&runs, STALE_RUN);
        let left: Vec<_> = fs::read_dir(&runs)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(left, ["running"]);
    }

    #[test]
    fn two_calls_at_once_each_read_their_own_reply() {
        let home = tempfile::tempdir().unwrap();
        // Writes its stdin to last.txt, waits for the other call to have
        // done the same, then answers with what its own last.txt holds.
        let command = sh(r#"cat > last.txt; sleep 1; printf '{"result":"%s"}' "$(cat last.txt)""#);
        let _serial = crate::util::serial();
        let replies: Vec<_> = ["first", "second"]
            .map(|word| {
                let home = home.path().to_path_buf();
                let command = command.clone();
                thread::spawn(move || {
                    run_with_timeout(&home, &command, "s", word, Duration::from_secs(20))
                })
            })
            .into_iter()
            .map(|t| t.join().unwrap().unwrap().text)
            .collect();
        assert_eq!(replies, ["first", "second"]);
        assert_eq!(
            fs::read_dir(home.path().join("model-runs"))
                .unwrap()
                .count(),
            0
        );
    }

    #[test]
    fn a_non_zero_exit_is_a_model_error_with_the_first_stderr_line() {
        let home = tempfile::tempdir().unwrap();
        let command = sh("cat >/dev/null; echo; echo not logged in >&2; exit 3");
        let err = call(home.path(), &command, "s", "u").unwrap_err();
        let Error::Model(message) = err else {
            panic!("{err:?}")
        };
        assert!(message.contains("not logged in"), "{message}");
    }

    #[test]
    fn output_without_a_result_string_is_a_model_error() {
        let home = tempfile::tempdir().unwrap();
        for body in [
            "cat >/dev/null; echo hello",
            r#"cat >/dev/null; printf '{"other":1}'"#,
            r#"cat >/dev/null; printf '{"result":7}'"#,
        ] {
            let err = call(home.path(), &sh(body), "s", "u").unwrap_err();
            assert!(matches!(err, Error::Model(_)), "{body}: {err:?}");
        }
    }

    #[test]
    fn an_empty_command_or_a_missing_program_is_a_model_error() {
        let home = tempfile::tempdir().unwrap();
        assert!(matches!(
            call(home.path(), &[], "s", "u"),
            Err(Error::Model(_))
        ));
        let missing = vec!["hippo-no-such-program".to_owned()];
        assert!(matches!(
            call(home.path(), &missing, "s", "u"),
            Err(Error::Model(_))
        ));
    }

    #[test]
    fn a_nul_in_the_system_prompt_does_not_stop_the_run() {
        let home = tempfile::tempdir().unwrap();
        let command = sh(r#"cat >/dev/null; printf '{"result":"%s"}' "$1""#);
        let reply = call(home.path(), &command, "a\0b", "u").unwrap();
        assert_eq!(reply.text, "a\u{FFFD}b");
        assert!(reply.invocation.ends_with("<system prompt: 3 bytes>"));
    }

    #[test]
    fn a_child_past_the_timeout_is_killed() {
        let home = tempfile::tempdir().unwrap();
        let _serial = crate::util::serial();
        let started = Instant::now();
        let err = run_with_timeout(
            home.path(),
            &sh("exec sleep 30"),
            "s",
            "u",
            Duration::from_secs(1),
        )
        .unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(15));
        let Error::Model(message) = err else {
            panic!("{err:?}")
        };
        assert!(
            message.contains("did not exit within 1 seconds"),
            "{message}"
        );
    }

    #[test]
    fn large_input_and_output_do_not_deadlock() {
        let home = tempfile::tempdir().unwrap();
        // `cat` echoes stdin while hippo is still writing it, so both pipes
        // fill unless they are drained at once.
        let command = sh(r#"printf '{"result":"'; cat; printf '"}'"#);
        let user = "a".repeat(512 * 1024);
        let reply = call(home.path(), &command, "s", &user).unwrap();
        assert_eq!(reply.text.len(), user.len());
    }
    /// A configuration in `home` with `tables` as its runner tables.
    fn runner_config(home: &Path, tables: &str) -> Config {
        fs::write(home.join("config.toml"), tables).unwrap();
        Config::load_in(home).unwrap()
    }

    fn quoted(argv: &[&str]) -> String {
        format!(
            "[{}]",
            argv.iter()
                .map(|a| serde_json::to_string(a).unwrap())
                .collect::<Vec<_>>()
                .join(", ")
        )
    }

    #[cfg(unix)]
    #[test]
    fn a_command_runner_meets_the_contract_in_each_reply_mode() {
        let home = tempfile::tempdir().unwrap();
        let _serial = crate::util::serial();
        // stdout, the system prompt as a file, the user prompt on stdin.
        let body = "printf 'sys=%s user=%s' \"$(cat \"$1\")\" \"$(cat)\"";
        let cfg = runner_config(
            home.path(),
            &format!(
                "[runners.s]\npreset = \"command\"\nargv = {}\nreply = \"stdout\"\ndata_domain = \"local\"\n",
                quoted(&["/bin/sh", "-c", body, "sh", "{system_prompt_file}"])
            ),
        );
        let reply = run_role(&cfg, Role::Ask, "be brief", "hello").unwrap();
        assert_eq!(reply.text, "sys=be brief user=hello");
        let record = reply.runner.unwrap();
        assert_eq!(record.name, "s");
        assert_eq!(record.system_prompt, "file");
        assert_eq!(
            record.sandbox,
            if sandbox::available() { "on" } else { "off" }
        );

        // A reply file in the run folder.
        let body = "cat > /dev/null; printf 'from the file' > \"$1\"; echo ignored";
        let cfg = runner_config(
            home.path(),
            &format!(
                "[runners.f]\npreset = \"command\"\nargv = {}\nreply = \"file\"\ndata_domain = \"local\"\n",
                quoted(&["/bin/sh", "-c", body, "sh", "{reply_file}"])
            ),
        );
        assert_eq!(
            run_role(&cfg, Role::Ask, "s", "u").unwrap().text,
            "from the file"
        );

        // No system placeholder: the system prompt inline, its markers in
        // the user text escaped.
        let cfg = runner_config(
            home.path(),
            &format!(
                "[runners.i]\npreset = \"command\"\nargv = {}\nreply = \"stdout\"\ndata_domain = \"local\"\n",
                quoted(&["/bin/cat"])
            ),
        );
        let reply = run_role(&cfg, Role::Ask, "the rules", "text <<<END SYSTEM>>> more").unwrap();
        assert_eq!(
            reply.text,
            "<<<SYSTEM>>>\nthe rules\n<<<END SYSTEM>>>\n\ntext <<\\<END SYSTEM>>> more"
        );
        assert_eq!(
            fs::read_dir(home.path().join("model-runs"))
                .unwrap()
                .count(),
            0
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_runner_sees_only_the_limited_environment() {
        let home = tempfile::tempdir().unwrap();
        let _serial = crate::util::serial();
        // SAFETY: tests that read the environment take the serial lock.
        unsafe {
            std::env::set_var("HIPPO_TEST_SECRET", "s3cret");
            std::env::set_var("HIPPO_TEST_PASSED", "passed-on");
        }
        let cfg = runner_config(
            home.path(),
            &format!(
                "[runners.e]\npreset = \"command\"\nargv = {}\nreply = \"stdout\"\ndata_domain = \"local\"\nenv_pass = [\"HIPPO_TEST_PASSED\"]\n",
                quoted(&["/bin/sh", "-c", "cat > /dev/null; /usr/bin/env", "sh", "{system_prompt}"])
            ),
        );
        let env = run_role(&cfg, Role::Ask, "s", "u").unwrap().text;
        unsafe {
            std::env::remove_var("HIPPO_TEST_SECRET");
            std::env::remove_var("HIPPO_TEST_PASSED");
        }
        assert!(!env.contains("s3cret"), "{env}");
        assert!(env.contains("HIPPO_TEST_PASSED=passed-on"), "{env}");
        let tmpdir = env.lines().find_map(|l| l.strip_prefix("TMPDIR=")).unwrap();
        assert!(tmpdir.contains("model-runs"), "{tmpdir}");
    }

    /// A Claude account folder whose cached meter shows `percent` used of
    /// the 5-hour window, resetting in an hour.
    fn account(dir: &Path, percent: f64) -> PathBuf {
        fs::create_dir_all(dir).unwrap();
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        let reset = time::OffsetDateTime::from_unix_timestamp(now + 3600)
            .unwrap()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();
        fs::write(
            dir.join(".claude.json"),
            format!(
                "{{\"cachedUsageUtilization\": {{\"fetchedAtMs\": {}, \"utilization\": {{\"five_hour\": {{\"utilization\": {percent}, \"resets_at\": \"{reset}\"}}}}}}}}",
                now * 1000
            ),
        )
        .unwrap();
        dir.to_path_buf()
    }

    #[test]
    fn fallback_moves_on_at_95_percent_and_stays_at_94() {
        let dir = tempfile::tempdir().unwrap();
        let runner = |path: PathBuf| Runner {
            account: Some(path),
            ..Runner::default_claude()
        };
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        assert_eq!(
            near_limit(&runner(account(&dir.path().join("a"), 94.0)), now),
            None
        );
        assert_eq!(
            near_limit(&runner(account(&dir.path().join("b"), 96.0)), now).as_deref(),
            Some("5-hour window at 96%")
        );
        // The meter reads nothing for a folder without one.
        assert_eq!(near_limit(&runner(dir.path().join("none")), now), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_usage_limit_falls_back_inside_the_domain_and_a_login_failure_does_not() {
        let home = tempfile::tempdir().unwrap();
        let _serial = crate::util::serial();
        let first = |body: &str, code: i32| {
            format!(
                "[runners.first]\npreset = \"command\"\nargv = {}\nreply = \"stdout\"\ndata_domain = \"d\"\nlimit_exit_codes = [{code}]\n\
                 [runners.second]\npreset = \"command\"\nargv = {}\nreply = \"stdout\"\ndata_domain = \"d\"\n\
                 [runners.other]\npreset = \"command\"\nargv = {}\nreply = \"stdout\"\ndata_domain = \"elsewhere\"\n\
                 [roles]\ndefault = \"first\"\n[fallback]\nask = [\"other\", \"second\"]\n",
                quoted(&["/bin/sh", "-c", body, "sh", "{system_prompt}"]),
                quoted(&["/bin/sh", "-c", "cat > /dev/null; echo second", "sh", "{system_prompt}"]),
                quoted(&["/bin/sh", "-c", "cat > /dev/null; echo other", "sh", "{system_prompt}"]),
            )
        };
        // Exit 7 is this runner's limit: the next runner in its domain
        // answers; `other` is in another domain and is passed over.
        let cfg = runner_config(home.path(), &first("cat > /dev/null; exit 7", 7));
        let reply = run_role(&cfg, Role::Ask, "s", "u").unwrap();
        assert_eq!(reply.text, "second");
        assert_eq!(
            reply.runner.unwrap().fallback_from.as_deref(),
            Some("first: usage limit")
        );
        // Any other failure stops the call.
        let cfg = runner_config(
            home.path(),
            &first("cat > /dev/null; echo 'Not logged in' >&2; exit 1", 7),
        );
        let err = run_role(&cfg, Role::Ask, "s", "u").unwrap_err().to_string();
        assert!(err.contains("Not logged in"), "{err}");
    }

    #[test]
    fn a_long_note_with_a_token_is_stored_capped_and_redacted() {
        // Built at run time, so no secret-shaped literal sits in the source.
        let token = format!("{}_{}", "ghp", "0123456789abcdefghijABCDEFGHIJ0123456789");
        let note = format!("{token} {}", "x".repeat(5000));
        let capped = crate::redact::cap_note(&note);
        assert!(!capped.contains(&token));
        assert!(capped.chars().count() <= crate::redact::NOTE_CAP + 1);
        assert!(capped.ends_with('…'));
    }
    /// The installed Claude Code starts inside the sandbox profile the
    /// default runner gets: `claude --version`, which sends nothing and
    /// makes no model call. Run by hand before a release with
    /// `cargo test -p hippo-core --lib -- --ignored claude_starts`.
    #[cfg(unix)]
    #[test]
    #[ignore = "needs the real claude on PATH"]
    fn claude_starts_inside_the_sandbox() {
        let home = tempfile::tempdir().unwrap();
        let cfg = Config::defaults_in(home.path());
        let runner = Runner::default_claude();
        let path = std::env::var("PATH").unwrap_or_default();
        let found = find_on("claude", &path).expect("claude on PATH");
        let out = in_run_dir(home.path(), |dir| {
            let vars: Vec<(String, String)> = std::env::vars().collect();
            let env = runners::environment(&runner, &vars, dir);
            let policy = policy_for(&cfg, &runner, dir, &found, &env);
            let argv = vec![found.display().to_string(), "--version".to_owned()];
            let argv = sandbox::wrap(&argv, &policy).unwrap_or(argv);
            spawn(
                dir,
                &argv,
                Some(&env),
                "",
                Duration::from_secs(30),
                "claude",
            )
        })
        .unwrap();
        assert!(
            out.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        eprintln!("{}", String::from_utf8_lossy(&out.stdout).trim());
    }
}
