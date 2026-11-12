//! Resume that acts: open a session where it lives.
//!
//! - A T3 Code session opens its exact thread through
//!   `t3code://threads/<environment>/<thread>`; the thread is found in T3
//!   Code's own state, opened read-only.
//! - A terminal session whose agent still runs brings the app hosting it
//!   forward (a second `claude --resume` would fork the session).
//! - Otherwise the terminal set in Manage › Settings (Ghostty or Terminal)
//!   runs the resume command from a one-shot script that deletes
//!   itself, the way Brief's Start in Claude Code does.
//!
//! Nothing here writes into hippocamp's or an agent's files; the script lives
//! in a new private directory under the system's temporary directory.

use std::path::{Path, PathBuf};
use std::process::Command;

use rusqlite::{Connection, OpenFlags, OptionalExtension as _};

/// T3 Code's state directory: `~/.t3/userdata`.
fn t3_userdata() -> Option<PathBuf> {
    Some(hippocamp_core::config::user_home()?.join(".t3/userdata"))
}

/// The native id T3 Code keeps for a hippocamp session id: Claude Code's
/// session id as is, Codex's uuid without `codex-`.
pub fn native_id(session: &str) -> &str {
    session.strip_prefix("codex-").unwrap_or(session)
}

/// The thread of `session` in T3 Code's state at `db`: the newest
/// provider-thread row whose native id is the session's, on a thread that
/// is not deleted. Opened read-only (`mode=ro`, never `immutable`, which
/// would skip the write-ahead log the running app keeps).
pub fn thread_in(db: &Path, session: &str) -> Option<String> {
    let uri = format!("file:{}?mode=ro", db.to_string_lossy());
    let conn = Connection::open_with_flags(
        uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    conn.busy_timeout(std::time::Duration::from_millis(500))
        .ok()?;
    conn.query_row(
        "SELECT p.thread_id FROM orchestration_v2_projection_provider_threads p \
         JOIN orchestration_v2_projection_threads t ON t.thread_id = p.thread_id \
         WHERE json_extract(p.payload_json, '$.nativeThreadRef.nativeId') = ?1 \
           AND t.deleted_at IS NULL \
         ORDER BY p.updated_at DESC LIMIT 1",
        [native_id(session)],
        |r| r.get::<_, Option<String>>(0),
    )
    .optional()
    .ok()
    .flatten()
    .flatten()
}

/// The deep link to T3 Code's thread for `session`, when its state names
/// one: `t3code://threads/<environment>/<thread>`.
pub fn t3_thread_link(session: &str) -> Option<String> {
    let dir = t3_userdata()?;
    let env = std::fs::read_to_string(dir.join("environment-id")).ok()?;
    let env = env.trim();
    let thread = thread_in(&dir.join("statev2.sqlite"), session)?;
    thread_link(env, &thread)
}

/// `t3code://threads/<env>/<thread>`, both percent-encoded; `None` for an
/// empty part.
pub fn thread_link(env: &str, thread: &str) -> Option<String> {
    if env.is_empty() || thread.is_empty() {
        return None;
    }
    Some(format!(
        "t3code://threads/{}/{}",
        encode(env),
        encode(thread)
    ))
}

fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Opens a URL or a file with `open`.
pub fn open(target: &str) -> Result<(), String> {
    match Command::new("/usr/bin/open").arg(target).status() {
        Ok(s) if s.success() => Ok(()),
        Ok(_) => Err(format!("open did not accept {target}")),
        Err(e) => Err(format!("cannot run open: {e}")),
    }
}

/// Brings an application forward by its bundle path.
pub fn activate_app(app: &Path) -> Result<(), String> {
    match Command::new("/usr/bin/open").arg("-a").arg(app).status() {
        Ok(s) if s.success() => Ok(()),
        Ok(_) => Err(format!("could not bring {} forward", app_name(app))),
        Err(e) => Err(format!("cannot run open: {e}")),
    }
}

/// `Terminal` for `/System/Applications/Utilities/Terminal.app`.
pub fn app_name(app: &Path) -> String {
    app.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| app.display().to_string())
}

/// The `.app` bundle an executable path lies in (the outermost one):
/// `/Applications/Ghostty.app` for `/Applications/Ghostty.app/Contents/MacOS/ghostty`.
pub fn bundle_of(exe: &str) -> Option<PathBuf> {
    let i = exe.find(".app/")?;
    Some(PathBuf::from(&exe[..i + 4]))
}

/// `lsof -Fpn` output (`p<pid>`, `n<path>` lines) as `(pid, cwd)` pairs.
pub fn parse_lsof(out: &str) -> Vec<(u32, PathBuf)> {
    let mut pairs = Vec::new();
    let mut pid = None;
    for line in out.lines() {
        if let Some(p) = line.strip_prefix('p') {
            pid = p.trim().parse().ok();
        } else if let (Some(n), Some(p)) = (line.strip_prefix('n'), pid) {
            pairs.push((p, PathBuf::from(n)));
        }
    }
    pairs
}

/// `ps -axo pid=,ppid=,comm=` output as `(pid, ppid, executable)`.
pub fn parse_ps(out: &str) -> Vec<(u32, u32, String)> {
    out.lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let pid = it.next()?.parse().ok()?;
            let ppid = it.next()?.parse().ok()?;
            let rest: Vec<&str> = it.collect();
            Some((pid, ppid, rest.join(" ")))
        })
        .collect()
}

/// The app hosting `pid`: its nearest ancestor (itself included) that runs
/// from an `.app` bundle.
pub fn host_app(pid: u32, procs: &[(u32, u32, String)]) -> Option<PathBuf> {
    let mut at = pid;
    for _ in 0..64 {
        let (_, ppid, exe) = procs.iter().find(|p| p.0 == at)?;
        if let Some(app) = bundle_of(exe) {
            return Some(app);
        }
        if *ppid <= 1 {
            return None;
        }
        at = *ppid;
    }
    None
}

/// The pid of the running Claude Code process holding `session`: the
/// client keeps `<config dir>/sessions/<pid>.json` naming its `sessionId`
/// while it runs. A file whose process is gone is left out.
pub fn claude_pid(config_dir: &Path, session: &str, procs: &[(u32, u32, String)]) -> Option<u32> {
    let rd = std::fs::read_dir(config_dir.join("sessions")).ok()?;
    rd.flatten().find_map(|e| {
        let text = std::fs::read_to_string(e.path()).ok()?;
        let v: serde_json::Value = serde_json::from_str(&text).ok()?;
        let pid = u32::try_from(v.get("pid")?.as_u64()?).ok()?;
        (v.get("sessionId")?.as_str()? == session && procs.iter().any(|p| p.0 == pid))
            .then_some(pid)
    })
}

fn processes() -> Vec<(u32, u32, String)> {
    Command::new("/bin/ps")
        .args(["-axo", "pid=,ppid=,comm="])
        .output()
        .map(|o| parse_ps(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_default()
}

/// The app hosting the agent of `row` while it still runs: for Claude
/// Code the process its profile names for the session; for Codex (which
/// keeps no such file) a `codex` process working in the session's folder.
pub fn running_host(row: &hippocamp_view::fleet::Row) -> Option<PathBuf> {
    let procs = processes();
    if row.tool == "codex" {
        let cwd = row.cwd.as_deref()?;
        let lsof = Command::new("/usr/sbin/lsof")
            .args(["-a", "-d", "cwd", "-c", "codex", "-Fpn"])
            .output()
            .ok()?;
        return parse_lsof(&String::from_utf8_lossy(&lsof.stdout))
            .into_iter()
            .filter(|(_, d)| d == cwd)
            .find_map(|(pid, _)| host_app(pid, &procs));
    }
    let dir = hippocamp_core::usage::config_dir_of(&row.transcript)?;
    host_app(claude_pid(&dir, &row.id, &procs)?, &procs)
}

/// The terminal a one-shot script runs in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Term {
    Terminal,
    /// Ghostty, at its bundle's path.
    Ghostty(PathBuf),
}

impl Term {
    pub fn name(&self) -> &'static str {
        match self {
            Term::Terminal => "Terminal",
            Term::Ghostty(_) => "Ghostty",
        }
    }
}

/// Ghostty's bundle, when it is installed in `/Applications` or
/// `~/Applications`.
pub fn ghostty_app() -> Option<PathBuf> {
    let mut places = vec![PathBuf::from("/Applications/Ghostty.app")];
    if let Some(h) = hippocamp_core::config::user_home() {
        places.push(h.join("Applications/Ghostty.app"));
    }
    places.into_iter().find(|p| p.is_dir())
}

/// The terminal for the setting `pref` (`ghostty` or `terminal`): unset is
/// Ghostty when `ghostty` is installed, else Terminal; `ghostty` without
/// Ghostty installed falls back to Terminal.
pub fn term_for(pref: Option<&str>, ghostty: Option<PathBuf>) -> Term {
    match (pref, ghostty) {
        (Some("terminal"), _) | (_, None) => Term::Terminal,
        (_, Some(app)) => Term::Ghostty(app),
    }
}

/// The arguments for `open` that run `script` in a new Ghostty process
/// (checked on Ghostty 1.3.1): one window, the script run through
/// the person's login shell so their `PATH` holds `claude` and `codex`,
/// and the process quits when the agent exits. The script's path is never
/// a bare argument: AppKit would hand it to Ghostty as a document, and
/// Ghostty asks "Allow Ghostty to execute …?" for it.
pub fn ghostty_args(app: &Path, script: &Path, shell: &str) -> Vec<std::ffi::OsString> {
    let quoted = format!("'{}'", script.to_string_lossy().replace('\'', "'\\''"));
    vec![
        "-na".into(),
        app.as_os_str().to_owned(),
        "--args".into(),
        "--quit-after-last-window-closed=true".into(),
        // The window stays after the agent exits (or a `cd` fails), as
        // Terminal's does, until a key closes it.
        "--wait-after-command=true".into(),
        format!("--command={shell} -lic {quoted}").into(),
    ]
}

/// Opens `script` (an executable one-shot `.command` file) in `term`.
pub fn open_script(term: &Term, script: &Path) -> Result<(), String> {
    let mut cmd = Command::new("/usr/bin/open");
    match term {
        Term::Terminal => {
            cmd.args(["-a", "Terminal"]).arg(script);
        }
        Term::Ghostty(app) => {
            let shell = std::env::var("SHELL")
                .ok()
                .filter(|s| s.starts_with('/'))
                .unwrap_or_else(|| "/bin/zsh".into());
            cmd.args(ghostty_args(app, script, &shell));
        }
    }
    match cmd.status() {
        Ok(s) if s.success() => Ok(()),
        Ok(_) => Err(format!("open could not start {}", term.name())),
        Err(e) => Err(format!("cannot run open: {e}")),
    }
}

/// What opening a session did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Opened {
    /// Done; the words say what.
    Said(String),
    /// A T3 Code session whose thread was not found: the app brings T3
    /// Code forward and copies the title.
    T3Fallback,
}

/// Opens `row` where it lives (see the module's comment): T3 Code's
/// thread, the app still running it, or its resume command in `term`.
pub fn open_session(row: &hippocamp_view::fleet::Row, term: &Term) -> Result<Opened, String> {
    if row.client == Some("T3 Code") {
        return match t3_thread_link(&row.id) {
            Some(link) => {
                open(&link)?;
                Ok(Opened::Said("opened its thread in T3 Code".into()))
            }
            None => Ok(Opened::T3Fallback),
        };
    }
    if let Some(app) = running_host(row) {
        activate_app(&app)?;
        return Ok(Opened::Said(format!(
            "brought {} forward: the agent is still running there",
            app_name(&app)
        )));
    }
    let target = hippocamp_core::resume::Target {
        id: &row.id,
        tool: &row.tool,
        transcript: &row.transcript,
        cwd: row.cwd.as_deref(),
    };
    let cmd =
        hippocamp_core::resume::command(&target).ok_or("this session has no resume command")?;
    let on = row
        .profile
        .as_deref()
        .map(hippocamp_view::fleet::profile_label)
        .unwrap_or(&row.tool);
    run_in_terminal(&cmd, &resume_header(&row.title, on), term)?;
    Ok(Opened::Said(format!("resumed in {}", term.name())))
}

/// The resume script's one line: `hippocamp: resuming "<title>" on
/// <profile>`.
pub fn resume_header(title: &str, profile: &str) -> String {
    let t: String = title
        .trim()
        .trim_end_matches('.')
        .chars()
        .take(80)
        .collect();
    format!("hippocamp: resuming \"{t}\" on {profile}")
}

/// The one-shot script a terminal runs: it deletes itself (and its private
/// directory), clears the screen, says `header` in one line, then
/// runs `command` (hippocamp-core's resume quoted its paths).
pub fn resume_script(command: &str, header: &str) -> String {
    // The header is printed through a quoted heredoc (no expansion); it is
    // one line, so it never holds the delimiter.
    let header = header.replace('\n', " ");
    format!(
        "#!/bin/sh\n# Written by Hippocamp.app for one run; it deletes itself.\n\
         dir=$(dirname \"$0\")\nrm -f \"$0\"\nrmdir \"$dir\" 2>/dev/null\n\
         clear\ncat <<'HIPPOCAMP_RESUME'\n{header}\nHIPPOCAMP_RESUME\n{command}\n"
    )
}

/// Writes [`resume_script`] to a new private directory (0700) under the
/// system's temporary directory as an executable `.command` file (0700)
/// and opens it in `term`. The app's one other write besides its
/// preferences and Brief's start files.
#[allow(clippy::disallowed_methods)]
pub fn run_in_terminal(command: &str, header: &str, term: &Term) -> Result<(), String> {
    use std::io::Write as _;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let go = || -> std::io::Result<PathBuf> {
        let base =
            std::env::temp_dir().join(format!("hippocamp-resume-{}", ulid::Ulid::generate()));
        std::fs::DirBuilder::new().mode(0o700).create(&base)?;
        let script = base.join("resume.command");
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o700)
            .open(&script)?;
        f.write_all(resume_script(command, header).as_bytes())?;
        Ok(script)
    };
    let script = go().map_err(|e| format!("could not write the resume script: {e}"))?;
    open_script(term, &script)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_methods)] // tests build a temporary database
    use super::*;

    #[test]
    fn ghostty_runs_the_script_through_a_login_shell_never_as_a_document() {
        let app = Path::new("/Applications/Ghostty.app");
        let args = ghostty_args(app, Path::new("/tmp/x y/it's.command"), "/bin/zsh");
        let args: Vec<String> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args[..3], ["-na", "/Applications/Ghostty.app", "--args"]);
        assert_eq!(args[3], "--quit-after-last-window-closed=true");
        assert_eq!(args[4], "--wait-after-command=true");
        assert_eq!(
            args[5],
            "--command=/bin/zsh -lic '/tmp/x y/it'\\''s.command'"
        );
        // No argument is the bare script path (AppKit would open it as a
        // document and Ghostty would ask to execute it).
        assert!(
            args.iter()
                .all(|a| !a.ends_with(".command") || a.starts_with("--"))
        );
        assert_eq!(
            term_for(None, Some(app.to_path_buf())),
            Term::Ghostty(app.to_path_buf())
        );
        assert_eq!(
            term_for(Some("terminal"), Some(app.to_path_buf())),
            Term::Terminal
        );
        assert_eq!(term_for(Some("ghostty"), None), Term::Terminal);
        assert_eq!(term_for(None, None), Term::Terminal);
    }

    #[test]
    fn ids_with_percent_and_colon_are_encoded_once() {
        // T3 Code decodes path parameters once: `%` must arrive as `%25`,
        // or an id with `%3A` in it decodes to a different id.
        assert_eq!(
            thread_link("env-1", "a%3Ab:c").as_deref(),
            Some("t3code://threads/env-1/a%253Ab%3Ac")
        );
        assert_eq!(
            thread_link("e:1", "t/2").as_deref(),
            Some("t3code://threads/e%3A1/t%2F2")
        );
    }

    #[test]
    fn a_thread_link_names_the_environment_and_the_thread() {
        assert_eq!(
            thread_link("env-1", "thread-a b").as_deref(),
            Some("t3code://threads/env-1/thread-a%20b")
        );
        assert_eq!(thread_link("", "t"), None);
        assert_eq!(native_id("codex-0199"), "0199");
        assert_eq!(native_id("abc"), "abc");
    }

    #[test]
    fn the_newest_live_thread_of_a_session_is_found_read_only() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("state.sqlite");
        let c = Connection::open(&db).unwrap();
        c.execute_batch(
            "CREATE TABLE orchestration_v2_projection_provider_threads (
               provider_thread_id TEXT PRIMARY KEY, thread_id TEXT,
               updated_at TEXT NOT NULL, payload_json TEXT NOT NULL);
             CREATE TABLE orchestration_v2_projection_threads (
               thread_id TEXT PRIMARY KEY, deleted_at TEXT);
             INSERT INTO orchestration_v2_projection_threads VALUES
               ('thread-old', NULL), ('thread-new', NULL), ('thread-gone', '2026-11-01');
             INSERT INTO orchestration_v2_projection_provider_threads VALUES
               ('p1', 'thread-old', '2026-11-01', '{\"nativeThreadRef\":{\"nativeId\":\"s-1\"}}'),
               ('p2', 'thread-new', '2026-11-02', '{\"nativeThreadRef\":{\"nativeId\":\"s-1\"}}'),
               ('p3', 'thread-gone', '2026-11-03', '{\"nativeThreadRef\":{\"nativeId\":\"s-1\"}}'),
               ('p4', 'thread-cx', '2026-11-03', '{\"nativeThreadRef\":{\"nativeId\":\"0199\"}}');
             INSERT INTO orchestration_v2_projection_threads VALUES ('thread-cx', NULL);",
        )
        .unwrap();
        drop(c);
        assert_eq!(thread_in(&db, "s-1").as_deref(), Some("thread-new"));
        assert_eq!(thread_in(&db, "codex-0199").as_deref(), Some("thread-cx"));
        assert_eq!(thread_in(&db, "nobody"), None);
        assert_eq!(thread_in(&dir.path().join("missing.sqlite"), "s-1"), None);
    }

    #[test]
    fn a_running_agent_is_traced_to_the_app_that_hosts_it() {
        let lsof = "p100\nfcwd\nn/work/demo\np200\nfcwd\nn/work/other\n";
        assert_eq!(
            parse_lsof(lsof),
            vec![
                (100, PathBuf::from("/work/demo")),
                (200, PathBuf::from("/work/other"))
            ]
        );
        let ps = parse_ps(
            "  1     0 /sbin/launchd\n 50     1 /Applications/Ghostty.app/Contents/MacOS/ghostty\n 60    50 /usr/bin/login\n 70    60 -zsh\n100    70 claude\n200     1 claude\n",
        );
        assert_eq!(
            host_app(100, &ps),
            Some(PathBuf::from("/Applications/Ghostty.app"))
        );
        assert_eq!(host_app(200, &ps), None);
        assert_eq!(
            bundle_of("/Applications/T3 Code (Nightly).app/Contents/Frameworks/T3 Code Helper.app/Contents/MacOS/x")
                .as_deref(),
            Some(Path::new("/Applications/T3 Code (Nightly).app"))
        );
        assert_eq!(app_name(Path::new("/Applications/Ghostty.app")), "Ghostty");
    }

    #[test]
    fn a_running_session_is_found_by_its_profile_s_pid_file() {
        let dir = tempfile::tempdir().unwrap();
        let sessions = dir.path().join("sessions");
        std::fs::create_dir(&sessions).unwrap();
        std::fs::write(
            sessions.join("100.json"),
            r#"{"pid":100,"sessionId":"s-1"}"#,
        )
        .unwrap();
        std::fs::write(
            sessions.join("300.json"),
            r#"{"pid":300,"sessionId":"s-2"}"#,
        )
        .unwrap();
        let procs = vec![(100, 1, "claude".to_owned())];
        assert_eq!(claude_pid(dir.path(), "s-1", &procs), Some(100));
        // s-2's process is gone.
        assert_eq!(claude_pid(dir.path(), "s-2", &procs), None);
        assert_eq!(claude_pid(dir.path(), "s-3", &procs), None);
    }

    /// The script deletes itself and its directory, then runs the command
    /// verbatim (a stub `claude` records its arguments and directory).
    #[test]
    fn the_resume_script_deletes_itself_and_runs_the_command() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let log = dir.path().join("log");
        let stub = bin.join("claude");
        std::fs::write(
            &stub,
            format!(
                "#!/bin/sh\necho \"$(pwd -P) $CLAUDE_CONFIG_DIR $*\" > '{}'\n",
                log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        let work = dir.path().join("my repo");
        std::fs::create_dir(&work).unwrap();
        let once = dir.path().join("once");
        std::fs::create_dir(&once).unwrap();
        let script = once.join("resume.command");
        let target = hippocamp_core::resume::Target {
            id: "s-1",
            tool: "claude-code",
            transcript: Path::new("/h/.claude-work/projects/-w/s-1.jsonl"),
            cwd: Some(&work),
        };
        let cmd = hippocamp_core::resume::command(&target).unwrap();
        std::fs::write(
            &script,
            resume_script(&cmd, &resume_header("Tidy it.", "work")),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let st = Command::new(&script)
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .status()
            .unwrap();
        assert!(st.success());
        assert!(!script.exists() && !once.exists());
        let text = resume_script("true", &resume_header("Tidy it.", "work"));
        assert!(
            text.contains(
                "clear\ncat <<'HIPPOCAMP_RESUME'\nhippocamp: resuming \"Tidy it\" on work\nHIPPOCAMP_RESUME\n"
            ),
            "{text}"
        );
        let got = std::fs::read_to_string(&log).unwrap();
        let work_real = work.canonicalize().unwrap();
        assert_eq!(
            got.trim(),
            format!("{} /h/.claude-work --resume s-1", work_real.display())
        );
    }
}
