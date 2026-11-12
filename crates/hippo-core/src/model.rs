//! Model invocation (Invariant 4, in `docs/site/src/architecture.md`):
//! hippo runs a model only through an argv such as `model_command`.
//!
//! The element `{system_prompt}` is replaced by the system prompt, and the
//! user prompt is written to the child's stdin. The child runs in
//! `$HIPPO_HOME/model-runs/`, emptied first, so it starts with nothing to
//! read. Its reply is the `result` string of the JSON it prints; a non-zero
//! exit, output without a string `result`, or 600 seconds without exit is
//! [`Error::Model`].

use std::fs;
use std::io::{self, Read, Write as _};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Duration;

use serde_json::Value;

use crate::config::Config;
use crate::error::{Error, Result};

/// The argv element that is replaced by the system prompt.
pub const SYSTEM_PROMPT: &str = "{system_prompt}";

/// How long a model may run before it is killed.
pub const TIMEOUT: Duration = Duration::from_secs(600);

/// How long to wait for the output pipes to close once the child exited. A
/// grandchild that keeps them open does not hold the run up for longer.
const DRAIN_GRACE: Duration = Duration::from_secs(5);

/// A model's reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelReply {
    /// The `result` string of the JSON the command printed.
    pub text: String,
    /// The argv joined by spaces with the system prompt replaced by
    /// `<system prompt: N bytes>`: the form a receipt's `invocation` takes.
    pub invocation: String,
}

/// Runs `cfg.model_command` with `system_prompt` and `user_prompt`.
pub fn run(cfg: &Config, system_prompt: &str, user_prompt: &str) -> Result<ModelReply> {
    run_command(cfg, &cfg.model_command, system_prompt, user_prompt)
}

/// Runs `command`, an argv in the form of `model_command`, as [`run`] runs
/// `model_command`. The dreaming reviewer passes
/// `reviewer_command_override` here when it is set.
pub fn run_command(
    cfg: &Config,
    command: &[String],
    system_prompt: &str,
    user_prompt: &str,
) -> Result<ModelReply> {
    run_with_timeout(&cfg.home, command, system_prompt, user_prompt, TIMEOUT)
}

/// Runs `argv`, an argv in the form of `model_command`, in `workdir` as it
/// stands: the directory is neither emptied nor created. The pack review
/// passes its own `review-runs/<ulid>/` here.
pub fn run_argv(
    workdir: &Path,
    argv: &[String],
    system_prompt: &str,
    user_prompt: &str,
) -> Result<ModelReply> {
    run_in(workdir, argv, system_prompt, user_prompt, TIMEOUT)
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

fn run_with_timeout(
    home: &Path,
    command: &[String],
    system_prompt: &str,
    user_prompt: &str,
    timeout: Duration,
) -> Result<ModelReply> {
    let runs = home.join("model-runs");
    empty_dir(&runs)?;
    run_in(&runs, command, system_prompt, user_prompt, timeout)
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

fn run_in(
    workdir: &Path,
    command: &[String],
    system_prompt: &str,
    user_prompt: &str,
    timeout: Duration,
) -> Result<ModelReply> {
    // An argument cannot hold a NUL byte, which a note or a transcript
    // turn can: each becomes U+FFFD.
    let argv: Vec<String> = command
        .iter()
        .map(|arg| {
            let arg = if arg == SYSTEM_PROMPT {
                system_prompt
            } else {
                arg.as_str()
            };
            arg.replace('\0', "\u{FFFD}")
        })
        .collect();
    let Some((program, args)) = argv.split_first() else {
        return Err(Error::Model("the model command is empty".into()));
    };
    let mut child = Command::new(program)
        .args(args)
        .current_dir(workdir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Error::Model(spawn_error(program, &e)))?;

    // Feed stdin and drain both outputs on their own threads, so a child
    // that writes a lot before it reads, or the reverse, cannot deadlock.
    if let Some(mut stdin) = child.stdin.take() {
        let input = user_prompt.as_bytes().to_vec();
        // A child that exits without reading stdin closes the pipe; the
        // write then fails, which is not an error of the run.
        thread::spawn(move || {
            let _ = stdin.write_all(&input);
        });
    }
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());

    let status = match wait_for(&mut child, timeout)
        .map_err(|e| Error::Model(format!("waiting for {program}: {e}")))?
    {
        Some(status) => status,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::Model(format!(
                "{program} did not exit within {} seconds",
                timeout.as_secs()
            )));
        }
    };
    let stdout = stdout.recv_timeout(DRAIN_GRACE).unwrap_or_default();
    let stderr = stderr.recv_timeout(DRAIN_GRACE).unwrap_or_default();
    if !status.success() {
        let detail = failure_reason(&stdout, &stderr)
            .map(|why| format!(": {why}"))
            .unwrap_or_default();
        return Err(Error::Model(format!(
            "{program} exited with {status}{detail}"
        )));
    }
    let stdout = String::from_utf8_lossy(&stdout);
    let reply: Value = serde_json::from_str(stdout.trim())
        .map_err(|_| Error::Model(format!("{program} did not print JSON")))?;
    let Some(text) = reply.get("result").and_then(Value::as_str) else {
        return Err(Error::Model(format!(
            "{program} printed JSON without a result string"
        )));
    };
    Ok(ModelReply {
        text: text.to_owned(),
        invocation: invocation(command, system_prompt),
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
             or point model_command at another model CLI: \
             hippo config set model_command '[\"<program>\", \"<arg>\", \"{{system_prompt}}\"]' --yes"
        )
    } else {
        format!("cannot run {program}: {e}")
    }
}

/// Removes `dir` with everything in it, then creates it empty. A symlink in
/// its place is removed, not followed.
fn empty_dir(dir: &Path) -> Result<()> {
    match fs::remove_dir_all(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(Error::io_at(dir, e)),
    }
    fs::create_dir_all(dir).map_err(|e| Error::io_at(dir, e))
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
        assert!(err.contains("hippo config set model_command"), "{err}");
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
    fn runs_in_an_emptied_model_runs_directory() {
        let home = tempfile::tempdir().unwrap();
        let runs = home.path().join("model-runs");
        fs::create_dir_all(runs.join("old")).unwrap();
        fs::write(runs.join("stale.txt"), "left over").unwrap();
        let command = sh(
            r#"cat >/dev/null; printf '{"result":"%s %s"}' "$(basename "$PWD")" "$(ls -A | wc -l | tr -d ' ')""#,
        );
        let reply = call(home.path(), &command, "s", "u").unwrap();
        assert_eq!(reply.text, "model-runs 0");
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
}
