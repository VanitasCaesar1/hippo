//! The OS sandbox every runner starts in (`docs/designs/runners.md`, The
//! sandbox).
//!
//! On macOS, `sandbox-exec` with a profile built per call; on Linux,
//! bubblewrap (`bwrap`) when it is on `PATH` and works. Elsewhere there is
//! no sandbox: `claude` still runs (its isolation is no tools, as before),
//! a `command` runner in a role that writes is refused unless it sets
//! `allow_unsandboxed`, and `doctor` says which.
//!
//! What the profile allows:
//!
//! - **read** the run folder, the CLI's executable and its install folder,
//!   the account folder (for Claude Code also `~/.claude.json` and, on
//!   macOS, the login keychain, which it reads with `/usr/bin/security`),
//!   and everything outside the home folder, `$HIPPO_HOME` and the
//!   workspace (system libraries, certificate stores);
//! - **write** the run folder (the child's `TMPDIR`), the account folder
//!   and those Claude Code files, and the system's temporary folders;
//! - **network**: allowed, since the CLI must reach its provider.
//!
//! Everything else under `$HOME`, `$HIPPO_HOME` outside the run folder,
//! and the workspace is denied. This is a limit on what an injected
//! instruction can make the CLI read, not a security boundary: the
//! account folder holds the CLI's own credentials and stays readable.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

/// What a sandboxed call may touch.
#[derive(Debug, Clone, Default)]
pub struct Policy {
    /// The call's own folder: read and write.
    pub run_dir: PathBuf,
    /// The person's home folder: denied except what is allowed below.
    pub home: Option<PathBuf>,
    /// `$HIPPO_HOME`: denied except the run folder.
    pub hippo_home: Option<PathBuf>,
    /// The workspace: denied.
    pub workspace: Option<PathBuf>,
    /// Folders and files the CLI may read.
    pub read: Vec<PathBuf>,
    /// Folders and files the CLI may read and write.
    pub write: Vec<PathBuf>,
}

/// Which sandbox this machine has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    SandboxExec,
    Bwrap,
    None,
}

const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// The sandbox this machine has, probed once: `sandbox-exec` on macOS;
/// on Linux, a `bwrap` on `PATH` that can start `true` in a new mount
/// namespace (some distributions forbid that without privileges).
pub fn kind() -> Kind {
    static KIND: OnceLock<Kind> = OnceLock::new();
    *KIND.get_or_init(|| {
        if cfg!(target_os = "macos") && Path::new(SANDBOX_EXEC).is_file() {
            return Kind::SandboxExec;
        }
        if cfg!(target_os = "linux") {
            let works = Command::new("bwrap")
                .args(["--ro-bind", "/", "/", "--dev", "/dev", "--", "true"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success());
            if works {
                return Kind::Bwrap;
            }
        }
        Kind::None
    })
}

/// Whether calls can be sandboxed here.
pub fn available() -> bool {
    kind() != Kind::None
}

/// The sandbox's name for `runners` and `doctor`.
pub fn kind_name() -> &'static str {
    match kind() {
        Kind::SandboxExec => "sandbox-exec",
        Kind::Bwrap => "bwrap",
        Kind::None if cfg!(target_os = "linux") => "no working bwrap; install bubblewrap",
        Kind::None => "no sandbox on this system",
    }
}

/// `path` with links resolved, or as given when it does not exist.
fn real(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The folders to allow for reading a CLI found at `program` (as found on
/// `PATH`): its install folder, two levels above the real executable (a
/// versioned binary's folder, or a Node package's prefix), but never the
/// home folder or a folder directly in it, which would open far more than
/// the CLI; and the link itself when `program` is one.
pub fn install_paths(program: &Path, home: Option<&Path>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let home = home.map(real);
    let mut add = |file: &Path| {
        let mut dir = file.parent().map(Path::to_path_buf);
        if let Some(up) = dir.as_deref().and_then(Path::parent) {
            let too_broad = home
                .as_deref()
                .is_some_and(|h| up == h || up.parent() == Some(h));
            if !too_broad {
                dir = Some(up.to_path_buf());
            }
        }
        if let Some(dir) = dir {
            let too_broad = home.as_deref().is_some_and(|h| dir == h);
            if !too_broad && !out.contains(&dir) {
                out.push(dir);
            }
        }
    };
    let canonical = real(program);
    add(&canonical);
    if fs::symlink_metadata(program).is_ok_and(|m| m.file_type().is_symlink()) {
        out.push(program.to_path_buf());
    } else if canonical != program {
        add(program);
    }
    out
}

/// A string as an SBPL string literal.
fn sbpl(path: &Path) -> String {
    let text = path.display().to_string();
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The `sandbox-exec` profile for `policy`.
pub fn macos_profile(policy: &Policy) -> String {
    let mut p = String::from("(version 1)\n(allow default)\n");
    let mut denied = Vec::new();
    for dir in [&policy.home, &policy.hippo_home, &policy.workspace]
        .into_iter()
        .flatten()
    {
        denied.push(real(dir));
    }
    for dir in &denied {
        p.push_str(&format!("(deny file-read-data (subpath {}))\n", sbpl(dir)));
    }
    // Writes: nowhere but the system's temporary folders and devices, then
    // never in the denied folders (which may sit in a temporary folder),
    // then the run folder and the allowed places. The last matching rule
    // wins.
    p.push_str("(deny file-write* (subpath \"/\"))\n");
    p.push_str(
        "(allow file-write* (subpath \"/private/var/folders\") (subpath \"/private/tmp\") (subpath \"/dev\"))\n",
    );
    for dir in &denied {
        p.push_str(&format!("(deny file-write* (subpath {}))\n", sbpl(dir)));
    }
    let run = real(&policy.run_dir);
    let mut allow_read = vec![run.clone()];
    allow_read.extend(policy.read.iter().map(|p| real(p)));
    allow_read.extend(policy.write.iter().map(|p| real(p)));
    for path in &allow_read {
        p.push_str(&format!(
            "(allow file-read-data (subpath {}))\n",
            sbpl(path)
        ));
    }
    let mut allow_write = vec![run];
    allow_write.extend(policy.write.iter().map(|p| real(p)));
    for path in &allow_write {
        p.push_str(&format!("(allow file-write* (subpath {}))\n", sbpl(path)));
    }
    p
}

/// The `bwrap` arguments for `policy`, before the command: the whole
/// system read-only, the home folder, `$HIPPO_HOME` and the workspace
/// hidden under empty folders, then the allowed paths bound back.
pub fn bwrap_args(policy: &Policy) -> Vec<String> {
    let mut a: Vec<String> = [
        "--ro-bind",
        "/",
        "/",
        "--dev",
        "/dev",
        "--proc",
        "/proc",
        "--tmpfs",
        "/tmp",
        "--die-with-parent",
    ]
    .map(String::from)
    .to_vec();
    for dir in [&policy.home, &policy.hippo_home, &policy.workspace]
        .into_iter()
        .flatten()
    {
        let dir = real(dir);
        if dir.is_dir() {
            a.extend(["--tmpfs".into(), dir.display().to_string()]);
        }
    }
    for path in &policy.read {
        let path = real(path);
        if path.exists() {
            let shown = path.display().to_string();
            a.extend(["--ro-bind".into(), shown.clone(), shown]);
        }
    }
    let mut writable = vec![real(&policy.run_dir)];
    writable.extend(policy.write.iter().map(|p| real(p)));
    for path in writable {
        if path.exists() {
            let shown = path.display().to_string();
            a.extend(["--bind".into(), shown.clone(), shown]);
        }
    }
    a
}

/// `argv` wrapped in this machine's sandbox, with `program` (the first
/// element, already resolved to a path) run directly. `None` when there is
/// no sandbox here.
pub fn wrap(argv: &[String], policy: &Policy) -> Option<Vec<String>> {
    match kind() {
        Kind::SandboxExec => {
            let mut out = vec![
                SANDBOX_EXEC.to_owned(),
                "-p".to_owned(),
                macos_profile(policy),
            ];
            out.extend(argv.iter().cloned());
            Some(out)
        }
        Kind::Bwrap => {
            let mut out = vec!["bwrap".to_owned()];
            out.extend(bwrap_args(policy));
            out.push("--".to_owned());
            out.extend(argv.iter().cloned());
            Some(out)
        }
        Kind::None => None,
    }
}

/// The paths Claude Code reads and writes for `account` (its config
/// folder; `None` is `~/.claude`) beyond the folder itself: the client
/// settings file (`~/.claude.json` for the default account), and on macOS
/// the login keychain, where it keeps its sign-in and which it reads and
/// refreshes with `/usr/bin/security`.
pub fn claude_paths(account: &Path, home: Option<&Path>) -> Vec<PathBuf> {
    let mut out = vec![
        account.to_path_buf(),
        crate::usage::meter::settings_file(account),
    ];
    if let Some(home) = home {
        if cfg!(target_os = "macos") {
            out.push(home.join("Library/Keychains"));
        }
        // The native installer's state and caches.
        out.push(home.join(".local/state/claude"));
        out.push(home.join(".cache/claude"));
        out.push(home.join("Library/Caches/claude-cli-nodejs"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_paths_never_open_the_home_folder() {
        let dir = tempfile::tempdir().unwrap();
        let home = real(dir.path());
        let versions = home.join(".local/share/claude/versions");
        fs::create_dir_all(&versions).unwrap();
        fs::create_dir_all(home.join(".local/bin")).unwrap();
        let binary = versions.join("2.1.0");
        fs::write(&binary, "x").unwrap();
        let link = home.join(".local/bin/claude");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&binary, &link).unwrap();
        let paths = install_paths(&link, Some(&home));
        assert!(
            paths.contains(&home.join(".local/share/claude")),
            "{paths:?}"
        );
        assert!(paths.contains(&link), "{paths:?}");
        assert!(!paths.contains(&home.join(".local")), "{paths:?}");
        assert!(!paths.contains(&home), "{paths:?}");

        // A binary directly in a folder of the home folder gets that
        // folder only.
        fs::create_dir_all(home.join("bin")).unwrap();
        fs::write(home.join("bin/tool"), "x").unwrap();
        assert_eq!(
            install_paths(&home.join("bin/tool"), Some(&home)),
            [home.join("bin")]
        );
    }

    #[test]
    fn the_macos_profile_denies_home_and_allows_the_run_folder() {
        let policy = Policy {
            run_dir: "/h/.hippo/model-runs/01X".into(),
            home: Some("/h".into()),
            hippo_home: Some("/h/.hippo".into()),
            workspace: Some("/w".into()),
            read: vec!["/h/.local/share/claude".into()],
            write: vec!["/h/.claude".into()],
        };
        let p = macos_profile(&policy);
        assert!(p.contains("(deny file-read-data (subpath \"/h\"))"));
        assert!(p.contains("(deny file-read-data (subpath \"/w\"))"));
        assert!(p.contains("(allow file-read-data (subpath \"/h/.hippo/model-runs/01X\"))"));
        assert!(p.contains("(allow file-write* (subpath \"/h/.claude\"))"));
        assert!(p.contains("(allow file-read-data (subpath \"/h/.local/share/claude\"))"));
        // Allows come after the denies they open.
        assert!(p.find("deny file-read-data").unwrap() < p.find("allow file-read-data").unwrap());
        assert_eq!(sbpl(Path::new("/a \"b\"")), "\"/a \\\"b\\\"\"");
    }

    /// The test plan's sandbox check: a stub CLI tries to read a file in
    /// the home folder, in `$HIPPO_HOME` and in the workspace, and to
    /// write outside its run folder; all four fail, and reading and
    /// writing its run folder work. Runs where a sandbox is available.
    #[test]
    fn a_sandboxed_stub_reaches_only_its_run_folder() {
        if !available() {
            eprintln!("no sandbox here: {}", kind_name());
            return;
        }
        let _serial = crate::util::serial();
        let dir = tempfile::tempdir().unwrap();
        let root = real(dir.path());
        let home = root.join("home");
        let hippo_home = home.join(".hippo");
        let run = hippo_home.join("model-runs/01RUN");
        let workspace = root.join("work");
        for d in [&run, &workspace] {
            fs::create_dir_all(d).unwrap();
        }
        fs::write(home.join("secret.txt"), "home-canary").unwrap();
        fs::write(hippo_home.join("canary.txt"), "hippo-canary").unwrap();
        fs::write(workspace.join("canary.txt"), "workspace-canary").unwrap();
        fs::write(run.join("prompt.txt"), "run-ok").unwrap();
        let policy = Policy {
            run_dir: run.clone(),
            home: Some(home.clone()),
            hippo_home: Some(hippo_home.clone()),
            workspace: Some(workspace.clone()),
            read: Vec::new(),
            write: Vec::new(),
        };
        let script = format!(
            "cat {h} 2>/dev/null; cat {hh} 2>/dev/null; cat {w} 2>/dev/null; \
             echo out > {home_out} 2>/dev/null; cat {run}/prompt.txt; echo wrote > {run}/reply.txt",
            h = home.join("secret.txt").display(),
            hh = hippo_home.join("canary.txt").display(),
            w = workspace.join("canary.txt").display(),
            home_out = home.join("written.txt").display(),
            run = run.display(),
        );
        let argv = vec!["/bin/sh".to_owned(), "-c".to_owned(), script];
        let wrapped = wrap(&argv, &policy).unwrap();
        let out = Command::new(&wrapped[0])
            .args(&wrapped[1..])
            .current_dir(&run)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("run-ok"),
            "{stdout} {}",
            String::from_utf8_lossy(&out.stderr)
        );
        for canary in ["home-canary", "hippo-canary", "workspace-canary"] {
            assert!(!stdout.contains(canary), "read {canary}: {stdout}");
        }
        assert!(!home.join("written.txt").exists());
        assert_eq!(
            fs::read_to_string(run.join("reply.txt")).unwrap(),
            "wrote\n"
        );
    }
}
