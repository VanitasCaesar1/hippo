//! Where a pack comes from: a directory on this machine or a git
//! repository fetched over https, and the `git` runner both the fetch and
//! the later items use.
//!
//! Git is an external program, never a library and never a checkout: hippocamp
//! fetches one commit without files over 64 KiB into a bare repository in a
//! temporary directory, cuts the remote off, and reads the pack's files
//! with `ls-tree` and `cat-file`. No link is created on disk and no
//! checkout filter runs.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;
use std::sync::LazyLock;
use std::thread;
use std::time::{Duration, Instant};

use regex::Regex;
use wait_timeout::ChildExt;
use walkdir::WalkDir;

use super::escape;
use super::files::{self, PackFiles, MAX_FILES, MAX_FILE_BYTES, MAX_PACK_BYTES};
use super::manifest::is_pack_name;
use crate::error::{Error, Result};
use crate::model::{clip, drain_capped};

/// The filter that leaves out every blob of 65,537 bytes or more.
const FILTER: &str = "blob:limit=65537";

/// How long to wait for the output threads after git has exited.
const DRAIN_GRACE: Duration = Duration::from_secs(5);

/// The most `skipped` paths [`read_tree`] lists; the rest are counted.
const MAX_SKIPPED: usize = 100;

/// The most standard output a git call may print: an `ls-tree` of a huge
/// tree is the largest answer hippocamp asks for.
const MAX_STDOUT: usize = 8 * 1024 * 1024;

/// The most standard error a git call may print.
const MAX_STDERR: usize = 64 * 1024;

/// The most the temporary repository may grow to during the fetch.
const MAX_FETCH_BYTES: u64 = 16 * 1024 * 1024;

/// How often the size of the temporary repository is read.
const SIZE_POLL: Duration = Duration::from_millis(200);

/// How often a running git process is looked at.
const TICK: Duration = Duration::from_millis(50);

/// A parsed `hippocamp add` argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A directory on this machine, absolute and canonical.
    Path(PathBuf),
    /// A directory of a git repository at an https URL.
    Git {
        /// The repository URL.
        url: String,
        /// The pack's directory in the repository; empty for the root.
        path: String,
        /// A branch, tag or 40-character commit; `None` for the remote's
        /// default branch.
        git_ref: Option<String>,
    },
    /// A pack listed in a registry index, by exact name; the index comes
    /// from `--registry`.
    Registry { name: String },
}

/// A pack read from a source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fetched {
    /// The pack's files.
    pub files: PackFiles,
    /// The commit read, for a git source.
    pub commit: Option<String>,
    /// Hidden entries and a root `origin.json`, skipped before any check.
    pub skipped: Vec<String>,
}

/// How git is run.
#[derive(Debug, Clone)]
pub struct GitOptions {
    /// Also allow `file://` URLs. For tests only; the CLI passes `false`.
    pub allow_file: bool,
    /// After this long a git process is killed.
    pub timeout: Duration,
}

/// What a git process printed and how it exited.
#[derive(Debug, Clone)]
pub struct GitOutput {
    /// The exit status.
    pub status: i32,
    /// Everything it wrote to standard output.
    pub stdout: Vec<u8>,
}

static GIT_URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^https://[A-Za-z0-9.-]+(:[0-9]+)?(/[A-Za-z0-9._~-]+)+$")
        .expect("the git URL pattern is valid")
});

static GIT_REF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._/-]{0,99}$").expect("the git ref pattern is valid")
});

/// An error whose text may hold a name or a line that came from outside:
/// escaped, so it cannot move a terminal's cursor.
fn pack_err(message: impl Into<String>) -> Error {
    Error::Pack(escape(&message.into()))
}

/// Reads a `hippocamp add` argument as a path, a git source or a registry name. A path starts
/// with `/`, `./`, `../` or `~/` (expanded with `user_home`), is joined to
/// `cwd` when relative, and is canonicalised. A git source is
/// `https://<repository>[//<directory>][@<ref>]`.
pub fn parse(arg: &str, cwd: &Path, user_home: Option<&Path>) -> Result<Source> {
    if arg.starts_with('/') || arg.starts_with("./") || arg.starts_with("../") {
        return parse_path(arg, PathBuf::from(arg), cwd);
    }
    if let Some(rest) = arg.strip_prefix("~/") {
        let Some(home) = user_home else {
            return Err(pack_err(format!(
                "{arg}: cannot find the home directory; set HOME"
            )));
        };
        return parse_path(arg, home.join(rest), cwd);
    }
    if arg.starts_with("https://") {
        return parse_git(arg);
    }
    if is_pack_name(arg) {
        return Ok(Source::Registry {
            name: arg.to_owned(),
        });
    }
    Err(pack_err(format!(
        "not a pack source: {arg}; use a path (./dir), an https git URL (https://host/repo[//dir][@ref]) or a registry name with --registry <path to index.json>"
    )))
}

fn parse_path(arg: &str, path: PathBuf, cwd: &Path) -> Result<Source> {
    let joined = if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    };
    let canonical = std::fs::canonicalize(&joined).map_err(|e| pack_err(format!("{arg}: {e}")))?;
    Ok(Source::Path(canonical))
}

fn parse_git(arg: &str) -> Result<Source> {
    let bad = |why: &str| pack_err(format!("{arg}: {why}"));
    let mut at = arg.split('@');
    let head = at.next().unwrap_or_default();
    let git_ref = at.next();
    if at.next().is_some() {
        return Err(bad("more than one @"));
    }
    let after_scheme = &head["https://".len()..];
    let (location, path) = match after_scheme.find("//") {
        Some(i) if i + 2 == after_scheme.len() => {
            return Err(bad("the directory after // is empty"));
        }
        Some(i) => (&after_scheme[..i], &after_scheme[i + 2..]),
        None => (after_scheme, ""),
    };
    let url = format!("https://{location}");
    check_git_source(&url, path, git_ref, false).map_err(|why| bad(&why))?;
    Ok(Source::Git {
        url,
        path: path.to_owned(),
        git_ref: git_ref.map(str::to_owned),
    })
}

/// The rules every git source obeys, whether it was typed, read from an
/// `origin.json` or given with `--ref`: an https URL with no user name,
/// query, fragment or percent sign and no `.` or `..` segment, a ref that is
/// a branch, tag or commit name, and a directory that obeys the path rules.
/// With `allow_file` (tests only) a `file://` URL passes. The error is the
/// reason, without the source.
pub fn check_git_source(
    url: &str,
    path: &str,
    git_ref: Option<&str>,
    allow_file: bool,
) -> std::result::Result<(), String> {
    if let Some(r) = git_ref {
        if !GIT_REF.is_match(r) || r.contains("..") {
            return Err("the ref is not a branch, tag or commit name".to_owned());
        }
    }
    let plain_file = allow_file
        && url.starts_with("file://")
        && !url.chars().any(char::is_control)
        && !url.contains(['?', '#', '%']);
    if !plain_file {
        if !GIT_URL.is_match(url) {
            return Err(
                "the repository URL must be https://host/path with no user name, query, fragment or percent sign"
                    .to_owned(),
            );
        }
        let location = &url["https://".len()..];
        let after_host = location.split_once('/').map_or("", |(_, p)| p);
        if after_host.split('/').any(|s| s == "." || s == "..") {
            return Err("the repository URL has a . or .. segment".to_owned());
        }
    }
    if !path.is_empty() {
        files::check_path(path)?;
    }
    Ok(())
}

/// Reads the pack in a directory on this machine: [`files::read_dir`]
/// with no commit.
pub fn read_path(dir: &Path) -> Result<Fetched> {
    let (files, skipped) = files::read_dir(dir)?;
    Ok(Fetched {
        files,
        commit: None,
        skipped,
    })
}

/// Runs `git` with `args` (in `dir` when given) and returns any exit
/// status. Every call carries the transport restrictions and environment of
/// [`git_command`]; standard input is null unless
/// `stdin` is given. A timeout kills git, and so does output past the caps
/// (8 MiB of standard output, 64 KiB of standard error).
pub fn run_git(
    dir: Option<&Path>,
    args: &[&str],
    stdin: Option<&[u8]>,
    options: &GitOptions,
) -> Result<GitOutput> {
    run(dir, args, stdin, options, &Limits::default()).map(|(output, _)| output)
}

/// What a git call may produce before it is killed.
struct Limits {
    stdout: usize,
    stderr: usize,
    /// A directory that may not grow past the given bytes, and the message
    /// when it does.
    watch: Option<(PathBuf, u64, String)>,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            stdout: MAX_STDOUT,
            stderr: MAX_STDERR,
            watch: None,
        }
    }
}

/// The bytes of the files below `dir`, links not followed.
fn dir_bytes(dir: &Path) -> u64 {
    WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .filter_map(|entry| entry.metadata().ok())
        .map(|meta| meta.len())
        .sum()
}

/// The command `git` with the transport restrictions, a clean environment
/// and no hook, template or file-system monitor of the user's.
///
/// Every variable named `GIT_*` is removed first, so a hippocamp started from a
/// git hook, `git rebase --exec` or `git submodule foreach` cannot be pointed
/// at the caller's repository (`GIT_DIR`, `GIT_WORK_TREE`), at other
/// programs (`GIT_EXEC_PATH`, `GIT_SSH_COMMAND`, `GIT_PROXY_COMMAND`) or at
/// injected configuration (`GIT_CONFIG_*`); the user's own git configuration
/// files, and with them the credential helpers, still apply.
fn git_command(options: &GitOptions) -> Command {
    let mut command = Command::new("git");
    for (name, _) in std::env::vars_os() {
        if name
            .to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("GIT_")
        {
            command.env_remove(name);
        }
    }
    command.args([
        "-c",
        "protocol.allow=never",
        "-c",
        "protocol.https.allow=always",
        "-c",
        "core.hooksPath=/dev/null",
        "-c",
        "core.fsmonitor=false",
    ]);
    if options.allow_file {
        command.args(["-c", "protocol.file.allow=always"]);
    }
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .env(
            "GIT_ALLOW_PROTOCOL",
            if options.allow_file {
                "https:file"
            } else {
                "https"
            },
        )
        .env("GIT_NO_LAZY_FETCH", "1");
    command
}

/// [`run_git`], also returning what git wrote to standard error.
fn run(
    dir: Option<&Path>,
    args: &[&str],
    stdin: Option<&[u8]>,
    options: &GitOptions,
    limits: &Limits,
) -> Result<(GitOutput, Vec<u8>)> {
    let subcommand = args.first().copied().unwrap_or("git");
    let mut command = git_command(options);
    if let Some(dir) = dir {
        command.arg("-C").arg(dir);
    }
    command
        .args(args)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|e| pack_err(format!("cannot run git: {e}")))?;
    if let (Some(mut pipe), Some(input)) = (child.stdin.take(), stdin) {
        let input = input.to_vec();
        thread::spawn(move || {
            let _ = pipe.write_all(&input);
        });
    }
    let (stdout, stdout_over) = drain_capped(child.stdout.take(), limits.stdout);
    let (stderr, stderr_over) = drain_capped(child.stderr.take(), limits.stderr);
    let started = Instant::now();
    let mut last_poll = started;
    let kill = |child: &mut std::process::Child| {
        let _ = child.kill();
        let _ = child.wait();
    };
    let status = loop {
        if let Some(status) = child
            .wait_timeout(TICK)
            .map_err(|e| pack_err(format!("waiting for git {subcommand}: {e}")))?
        {
            break status;
        }
        for (over, cap) in [(&stdout_over, limits.stdout), (&stderr_over, limits.stderr)] {
            if over.load(Ordering::SeqCst) {
                kill(&mut child);
                return Err(pack_err(format!(
                    "git {subcommand} printed more than {cap} bytes"
                )));
            }
        }
        if let Some((watched, max, message)) = &limits.watch {
            if last_poll.elapsed() >= SIZE_POLL {
                last_poll = Instant::now();
                if dir_bytes(watched) > *max {
                    kill(&mut child);
                    return Err(pack_err(message.clone()));
                }
            }
        }
        if started.elapsed() >= options.timeout {
            kill(&mut child);
            return Err(pack_err(format!(
                "git {subcommand} timed out after {} s",
                options.timeout.as_secs()
            )));
        }
    };
    // Output past a cap that arrived before git exited is as much a failure.
    for (over, cap) in [(&stdout_over, limits.stdout), (&stderr_over, limits.stderr)] {
        if over.load(Ordering::SeqCst) {
            return Err(pack_err(format!(
                "git {subcommand} printed more than {cap} bytes"
            )));
        }
    }
    let stdout = stdout.recv_timeout(DRAIN_GRACE).unwrap_or_default();
    let stderr = stderr.recv_timeout(DRAIN_GRACE).unwrap_or_default();
    let output = GitOutput {
        status: status.code().unwrap_or(-1),
        stdout,
    };
    Ok((output, stderr))
}

/// [`run_git`], with a non-zero exit status turned into
/// `git <subcommand> failed: <last non-empty stderr line>`. The line is
/// escaped: git echoes what a server or a file name told it.
pub fn run_git_ok(
    dir: Option<&Path>,
    args: &[&str],
    stdin: Option<&[u8]>,
    options: &GitOptions,
) -> Result<Vec<u8>> {
    run_git_ok_limited(dir, args, stdin, options, &Limits::default())
}

fn run_git_ok_limited(
    dir: Option<&Path>,
    args: &[&str],
    stdin: Option<&[u8]>,
    options: &GitOptions,
    limits: &Limits,
) -> Result<Vec<u8>> {
    let (output, stderr) = run(dir, args, stdin, options, limits)?;
    if output.status == 0 {
        return Ok(output.stdout);
    }
    let text = String::from_utf8_lossy(&stderr);
    let last = text
        .lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .unwrap_or("no message");
    Err(pack_err(format!(
        "git {} failed: {}",
        args.first().copied().unwrap_or("git"),
        escape(&clip(last))
    )))
}

/// Fetches `git_ref` (or the default branch) of `url` into a new bare
/// repository in a temporary directory and reads `path` from it.
pub fn fetch_git(
    url: &str,
    path: &str,
    git_ref: Option<&str>,
    options: &GitOptions,
) -> Result<Fetched> {
    let tmp = tempfile::tempdir().map_err(Error::Io)?;
    let repo = tmp.path().join("repo.git");
    let commit = fetch_into(&repo, url, git_ref, options)?;
    read_tree(&repo, &commit, path, options)
}

/// Steps 1 to 4 of the fetch: a bare repository at `repo`, one commit
/// without files over 64 KiB, the remote removed, and the commit's id. The
/// repository is made without a template (no hooks are copied in) and git is
/// killed if the repository passes 16 MiB, which a server that ignores the
/// size filter would cause.
pub(crate) fn fetch_into(
    repo: &Path,
    url: &str,
    git_ref: Option<&str>,
    options: &GitOptions,
) -> Result<String> {
    fetch_into_limited(repo, url, git_ref, options, MAX_FETCH_BYTES)
}

/// [`fetch_into`] with the repository's size limit as a parameter.
fn fetch_into_limited(
    repo: &Path,
    url: &str,
    git_ref: Option<&str>,
    options: &GitOptions,
    max_bytes: u64,
) -> Result<String> {
    if url.starts_with('-') || git_ref.is_some_and(|r| r.starts_with('-')) {
        return Err(pack_err("a URL or ref may not start with -"));
    }
    let repo_arg = repo.to_string_lossy();
    run_git_ok(
        None,
        &["init", "--bare", "--quiet", "--template=", &repo_arg],
        None,
        options,
    )?;
    let git = |args: &[&str]| run_git_ok(Some(repo), args, None, options);
    git(&["remote", "add", "origin", url])?;
    git(&["config", "remote.origin.promisor", "true"])?;
    git(&["config", "remote.origin.partialclonefilter", FILTER])?;
    let filter = format!("--filter={FILTER}");
    let limits = Limits {
        watch: Some((
            repo.to_path_buf(),
            max_bytes,
            format!(
                "the fetch passed {}; the server may not honour the size filter",
                size_text(max_bytes)
            ),
        )),
        ..Limits::default()
    };
    run_git_ok_limited(
        Some(repo),
        &[
            "fetch",
            "--quiet",
            "--depth",
            "1",
            "--no-tags",
            &filter,
            "origin",
            git_ref.unwrap_or("HEAD"),
        ],
        None,
        options,
        &limits,
    )?;
    git(&["remote", "remove", "origin"])?;
    let out = git(&["rev-parse", "--verify", "FETCH_HEAD^{commit}"])?;
    Ok(String::from_utf8_lossy(&out).trim().to_owned())
}

/// `16 MiB` for a whole number of mebibytes, else `<n> bytes`.
fn size_text(bytes: u64) -> String {
    if bytes > 0 && bytes.is_multiple_of(1024 * 1024) {
        format!("{} MiB", bytes / (1024 * 1024))
    } else {
        format!("{bytes} bytes")
    }
}

/// One `ls-tree` entry that passed rules 1 to 3.
struct Entry {
    path: String,
    oid: String,
}

/// Steps 5 to 7 on a repository that already holds `commit`: the files of
/// the directory `path` ("" for the root). Hidden entries and a root
/// `origin.json` are skipped; every other entry is checked in turn against
/// the path rules, the mode, the size and then read.
///
/// A component that is exactly `.` or `..` is not a hidden entry: the path
/// rules refuse it. Paths that differ only in case are refused.
pub fn read_tree(repo: &Path, commit: &str, path: &str, options: &GitOptions) -> Result<Fetched> {
    if commit.starts_with('-') {
        return Err(pack_err("a commit may not start with -"));
    }
    if !path.is_empty() {
        files::check_path(path).map_err(pack_err)?;
    }
    let treeish = if path.is_empty() {
        format!("{commit}^{{tree}}")
    } else {
        format!("{commit}:{path}")
    };
    let listing = run_git(
        Some(repo),
        &["ls-tree", "-r", "-z", &treeish],
        None,
        options,
    )?;
    if listing.status != 0 {
        let what = if path.is_empty() {
            "the repository root"
        } else {
            path
        };
        return Err(pack_err(format!(
            "{what}: not a directory in the repository at {commit}"
        )));
    }

    // Rules 1 to 3, in entry order, up to the first entry that fails.
    let mut skipped = Vec::new();
    let mut more = 0usize;
    let mut candidates: Vec<Entry> = Vec::new();
    let mut first_error: Option<Error> = None;
    for record in listing.stdout.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let Some(tab) = record.iter().position(|b| *b == b'\t') else {
            return Err(pack_err("git ls-tree printed an entry without a path"));
        };
        let (meta, name) = (&record[..tab], &record[tab + 1..]);
        let meta = String::from_utf8_lossy(meta);
        let mut fields = meta.split(' ');
        let (mode, oid) = (
            fields.next().unwrap_or_default(),
            fields.nth(1).unwrap_or_default(),
        );
        let hidden = name
            .split(|b| *b == b'/')
            .any(|c| c.starts_with(b".") && c != b"." && c != b"..");
        if hidden || name == b"origin.json" {
            if skipped.len() < MAX_SKIPPED {
                skipped.push(String::from_utf8_lossy(name).into_owned());
            } else {
                more += 1;
            }
            continue;
        }
        let Ok(name) = std::str::from_utf8(name) else {
            first_error = Some(pack_err(format!(
                "{}: not UTF-8",
                escape(&String::from_utf8_lossy(name))
            )));
            break;
        };
        if let Err(message) = files::check_path(name) {
            first_error = Some(pack_err(escape(&message)));
            break;
        }
        match mode {
            "100644" | "100755" => {}
            "120000" => {
                first_error = Some(pack_err(format!(
                    "{name}: a symbolic link; packs may not contain links"
                )));
                break;
            }
            "160000" => {
                first_error = Some(pack_err(format!("{name}: a submodule")));
                break;
            }
            other => {
                first_error = Some(pack_err(format!("{name}: mode {other} is not a file")));
                break;
            }
        }
        candidates.push(Entry {
            path: name.to_owned(),
            oid: oid.to_owned(),
        });
        // One past the limit is enough to refuse the pack: no need to read
        // the rest of a huge listing.
        if candidates.len() > MAX_FILES {
            break;
        }
    }
    if more > 0 {
        skipped.push(format!("… and {more} more"));
    }

    // Rule 4: sizes, in one call; an earlier entry's size outranks a later
    // entry's failure.
    let mut sizes = Vec::new();
    if !candidates.is_empty() {
        let input: String = candidates.iter().map(|e| format!("{}\n", e.oid)).collect();
        let out = run_git_ok(
            Some(repo),
            &["cat-file", "--batch-check=%(objectname) %(objectsize)"],
            Some(input.as_bytes()),
            options,
        )?;
        let text = String::from_utf8_lossy(&out);
        sizes = text.lines().map(str::to_owned).collect();
        if sizes.len() != candidates.len() {
            return Err(pack_err(
                "git cat-file answered a different number of lines",
            ));
        }
    }
    let mut total = 0usize;
    for (n, (entry, line)) in candidates.iter().zip(&sizes).enumerate() {
        let size = line.split(' ').nth(1).unwrap_or_default();
        if size == "missing" {
            return Err(pack_err(format!(
                "{}: over the {MAX_FILE_BYTES}-byte limit (the fetch left it out)",
                entry.path
            )));
        }
        let size: u64 = size
            .parse()
            .map_err(|_| pack_err(format!("{}: git printed no size", entry.path)))?;
        if size > MAX_FILE_BYTES {
            return Err(pack_err(format!(
                "{}: {size} bytes, over the {MAX_FILE_BYTES}-byte limit",
                entry.path
            )));
        }
        if n >= MAX_FILES {
            return Err(pack_err(format!("pack: more than {MAX_FILES} files")));
        }
        total += size as usize;
        if total > MAX_PACK_BYTES {
            return Err(pack_err(format!(
                "pack: {total} bytes, over the {MAX_PACK_BYTES}-byte limit"
            )));
        }
    }
    if let Some(error) = first_error {
        return Err(error);
    }

    // Paths that differ only in case would collide on some disks.
    let mut names = PackFiles::default();
    for entry in &candidates {
        names.insert(entry.path.clone(), Vec::new());
    }
    if let Some(collision) = files::check_layout(&names)
        .into_iter()
        .find(|m| m.ends_with("differ only in case"))
    {
        return Err(pack_err(collision));
    }

    // Rule 5: the content.
    let mut pack = PackFiles::default();
    for entry in candidates {
        let bytes = run_git_ok(Some(repo), &["cat-file", "blob", &entry.oid], None, options)?;
        pack.insert(entry.path, bytes);
    }
    Ok(Fetched {
        files: pack,
        commit: Some(commit.to_owned()),
        skipped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::serial;
    use std::fs;

    fn have_git() -> bool {
        let found = Command::new("git").arg("--version").output().is_ok();
        if !found {
            eprintln!("git is not on PATH; skipping");
        }
        found
    }

    fn options() -> GitOptions {
        GitOptions {
            allow_file: true,
            timeout: Duration::from_secs(60),
        }
    }

    /// Runs git in `dir` with a fixed identity and no user configuration.
    fn git(dir: &Path, args: &[&str]) -> String {
        git_in(dir, args, None)
    }

    fn git_in(dir: &Path, args: &[&str], stdin: Option<&str>) -> String {
        let mut child = Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "tag.gpgsign=false",
                "-C",
            ])
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("git runs");
        let mut pipe = child.stdin.take().expect("stdin");
        if let Some(text) = stdin {
            pipe.write_all(text.as_bytes()).expect("write stdin");
        }
        drop(pipe);
        let out = child.wait_with_output().expect("git finishes");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    /// A repository on branch `main` that others may fetch from, filtered.
    fn new_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        git(dir.path(), &["init", "--quiet", "-b", "main"]);
        git(dir.path(), &["config", "uploadpack.allowFilter", "true"]);
        git(
            dir.path(),
            &["config", "uploadpack.allowAnySHA1InWant", "true"],
        );
        dir
    }

    fn write(dir: &Path, path: &str, bytes: &[u8]) {
        let full = dir.join(path);
        fs::create_dir_all(full.parent().expect("parent")).expect("mkdir");
        fs::write(full, bytes).expect("write");
    }

    fn commit_all(dir: &Path, message: &str) -> String {
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "--quiet", "-m", message]);
        git(dir, &["rev-parse", "HEAD"])
    }

    fn url(dir: &Path) -> String {
        format!("file://{}", dir.display())
    }

    fn blob(dir: &Path, text: &str) -> String {
        git_in(dir, &["hash-object", "-w", "--stdin"], Some(text))
    }

    /// `git mktree --missing` over `(mode, type, oid, name)` entries.
    fn tree(dir: &Path, entries: &[(&str, &str, &str, &str)]) -> String {
        let input: String = entries
            .iter()
            .map(|(m, t, o, n)| format!("{m} {t} {o}\t{n}\n"))
            .collect();
        git_in(dir, &["mktree", "--missing"], Some(&input))
    }

    /// Commits `tree` as the only commit of `main`.
    fn commit_tree(dir: &Path, tree: &str) -> String {
        let commit = git(dir, &["commit-tree", tree, "-m", "x"]);
        git(dir, &["update-ref", "refs/heads/main", &commit]);
        commit
    }

    fn fetch_err(repo: &Path, path: &str) -> String {
        match fetch_git(&url(repo), path, None, &options()) {
            Err(e) => e.to_string(),
            Ok(f) => panic!("expected a refusal, got {} files", f.files.len()),
        }
    }

    #[test]
    fn fetches_a_tag_a_branch_a_commit_the_default_branch_and_a_directory() {
        if !have_git() {
            return;
        }
        let _guard = serial();
        let repo = new_repo();
        write(repo.path(), "pack.toml", b"one");
        write(repo.path(), "sub/pack.toml", b"sub one");
        let first = commit_all(repo.path(), "one");
        git(repo.path(), &["tag", "v1"]);
        git(repo.path(), &["checkout", "--quiet", "-b", "feat"]);
        write(repo.path(), "pack.toml", b"feature");
        let feat = commit_all(repo.path(), "feat");
        git(repo.path(), &["checkout", "--quiet", "main"]);
        write(repo.path(), "pack.toml", b"two");
        write(repo.path(), "sub/pack.toml", b"sub two");
        let second = commit_all(repo.path(), "two");
        let u = url(repo.path());
        let o = options();

        let read = |git_ref: Option<&str>, path: &str| {
            let f = fetch_git(&u, path, git_ref, &o).expect("fetches");
            let bytes = f.files.get("pack.toml").expect("pack.toml").to_vec();
            (String::from_utf8(bytes).expect("utf8"), f.commit)
        };
        assert_eq!(read(Some("v1"), ""), ("one".into(), Some(first.clone())));
        assert_eq!(read(Some("feat"), ""), ("feature".into(), Some(feat)));
        assert_eq!(read(Some(&first), ""), ("one".into(), Some(first)));
        assert_eq!(read(None, ""), ("two".into(), Some(second.clone())));
        assert_eq!(read(None, "sub"), ("sub two".into(), Some(second)));
        let err = fetch_err(repo.path(), "nope");
        assert!(err.starts_with("nope: "), "{err}");
    }

    #[test]
    fn refuses_a_link_and_a_submodule() {
        if !have_git() {
            return;
        }
        let _guard = serial();
        let repo = new_repo();
        let target = blob(repo.path(), "../../etc/passwd");
        let file = blob(repo.path(), "x");
        let root = tree(
            repo.path(),
            &[
                ("100644", "blob", &file, "pack.toml"),
                ("120000", "blob", &target, "link"),
            ],
        );
        commit_tree(repo.path(), &root);
        assert_eq!(
            fetch_err(repo.path(), ""),
            "link: a symbolic link; packs may not contain links"
        );

        let gitlink = "1".repeat(40);
        let root = tree(
            repo.path(),
            &[
                ("100644", "blob", &file, "pack.toml"),
                ("160000", "commit", &gitlink, "sub"),
            ],
        );
        commit_tree(repo.path(), &root);
        assert_eq!(fetch_err(repo.path(), ""), "sub: a submodule");
    }

    #[test]
    fn the_size_limit_is_65536_bytes() {
        if !have_git() {
            return;
        }
        let _guard = serial();
        let repo = new_repo();
        write(repo.path(), "ok/pack.toml", &vec![b'a'; 65_536]);
        write(repo.path(), "big/pack.toml", &vec![b'a'; 65_537]);
        let commit = commit_all(repo.path(), "sizes");

        let f = fetch_git(&url(repo.path()), "ok", None, &options()).expect("fits");
        assert_eq!(f.files.get("pack.toml").map(<[u8]>::len), Some(65_536));
        assert_eq!(
            fetch_err(repo.path(), "big"),
            "pack.toml: over the 65536-byte limit (the fetch left it out)"
        );

        // Read straight from a repository that holds the blob, the size is known.
        let err = read_tree(repo.path(), &commit, "big", &options()).expect_err("refused");
        assert_eq!(
            err.to_string(),
            "pack.toml: 65537 bytes, over the 65536-byte limit"
        );
    }

    #[test]
    fn the_fetch_leaves_out_big_files_outside_the_pack() {
        if !have_git() {
            return;
        }
        let _guard = serial();
        let repo = new_repo();
        write(repo.path(), "pack/pack.toml", b"x");
        let mut state = 0x9e37_79b9_u32;
        let noise: Vec<u8> = (0..200_000)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state >> 24) as u8
            })
            .collect();
        write(repo.path(), "other/random.bin", &noise);
        commit_all(repo.path(), "mixed");
        let big = git(repo.path(), &["rev-parse", "HEAD:other/random.bin"]);

        let tmp = tempfile::tempdir().expect("tempdir");
        let fetched = tmp.path().join("repo.git");
        fetch_into(&fetched, &url(repo.path()), None, &options()).expect("fetches");
        let listing = run_git_ok(
            Some(&fetched),
            &["cat-file", "--batch-all-objects", "--batch-check"],
            None,
            &options(),
        )
        .expect("lists");
        let listing = String::from_utf8_lossy(&listing);
        assert!(listing.lines().count() >= 3, "{listing}");
        assert!(!listing.contains(&big), "{listing}");
        for line in listing.lines() {
            let size: u64 = line
                .split(' ')
                .nth(2)
                .expect("size")
                .parse()
                .expect("number");
            assert!(size < 65_537, "{line}");
        }
    }

    #[test]
    fn refuses_paths_that_cannot_exist_on_a_disk() {
        if !have_git() {
            return;
        }
        let _guard = serial();
        let repo = new_repo();
        let file = blob(repo.path(), "x");
        let other = blob(repo.path(), "y");
        let pack = ("100644", "blob", file.as_str(), "pack.toml");

        let inner = tree(repo.path(), &[("100644", "blob", &file, "x.md")]);
        let root = tree(repo.path(), &[pack, ("040000", "tree", &inner, "..")]);
        commit_tree(repo.path(), &root);
        let err = fetch_err(repo.path(), "");
        assert!(err.starts_with("../x.md: breaks the path rules"), "{err}");

        let root = tree(repo.path(), &[pack, ("100644", "blob", &file, "a\\b")]);
        commit_tree(repo.path(), &root);
        let err = fetch_err(repo.path(), "");
        assert!(err.starts_with("a\\b: breaks the path rules"), "{err}");

        let root = tree(
            repo.path(),
            &[
                pack,
                ("100644", "blob", &file, "A.md"),
                ("100644", "blob", &other, "a.md"),
            ],
        );
        commit_tree(repo.path(), &root);
        assert_eq!(
            fetch_err(repo.path(), ""),
            "A.md and a.md differ only in case"
        );
    }

    #[test]
    fn skips_hidden_entries_and_a_root_origin_record() {
        if !have_git() {
            return;
        }
        let _guard = serial();
        let repo = new_repo();
        let file = blob(repo.path(), "x");
        let target = blob(repo.path(), "/etc/passwd");
        let dir = tree(repo.path(), &[("100644", "blob", &file, "inner.md")]);
        let root = tree(
            repo.path(),
            &[
                ("100644", "blob", &file, "pack.toml"),
                ("120000", "blob", &target, ".hidden-link"),
                ("040000", "tree", &dir, ".git"),
                ("100644", "blob", &file, "origin.json"),
            ],
        );
        commit_tree(repo.path(), &root);
        let f = fetch_git(&url(repo.path()), "", None, &options()).expect("fetches");
        assert_eq!(f.files.paths().collect::<Vec<_>>(), ["pack.toml"]);
        let mut skipped = f.skipped.clone();
        skipped.sort();
        assert_eq!(skipped, [".git/inner.md", ".hidden-link", "origin.json"]);
    }

    #[test]
    fn a_failed_git_command_names_its_subcommand_and_a_hung_one_is_killed() {
        if !have_git() {
            return;
        }
        let _guard = serial();
        let dir = tempfile::tempdir().expect("tempdir");
        let err = run_git_ok(
            Some(dir.path()),
            &["rev-parse", "--verify", "nope"],
            None,
            &options(),
        )
        .expect_err("fails");
        assert!(
            err.to_string().starts_with("git rev-parse failed: "),
            "{err}"
        );
        let out = run_git(
            Some(dir.path()),
            &["rev-parse", "--verify", "nope"],
            None,
            &options(),
        )
        .expect("runs");
        assert_ne!(out.status, 0);
        let short = GitOptions {
            allow_file: true,
            timeout: Duration::from_secs(1),
        };
        let err = run_git(None, &["-c", "alias.slow=!sleep 5", "slow"], None, &short)
            .expect_err("killed");
        assert_eq!(err.to_string(), "git -c timed out after 1 s");
    }

    #[test]
    fn parse_reads_paths_and_git_urls() {
        let _guard = serial();
        let cwd = tempfile::tempdir().expect("tempdir");
        let home = tempfile::tempdir().expect("tempdir");
        fs::create_dir(cwd.path().join("x")).expect("mkdir");
        fs::create_dir(home.path().join("packs")).expect("mkdir");
        let cwd_path = cwd.path();

        let canon = |p: PathBuf| fs::canonicalize(p).expect("canonical");
        assert_eq!(
            parse("./x", cwd_path, None).expect("path"),
            Source::Path(canon(cwd_path.join("x")))
        );
        assert_eq!(
            parse("~/packs", cwd_path, Some(home.path())).expect("home"),
            Source::Path(canon(home.path().join("packs")))
        );
        let absolute = cwd_path.join("x");
        assert_eq!(
            parse(absolute.to_str().expect("utf8"), Path::new("/"), None).expect("abs"),
            Source::Path(canon(absolute))
        );

        assert_eq!(
            parse("https://github.com/alice/hippocamp-packs", cwd_path, None).expect("plain"),
            Source::Git {
                url: "https://github.com/alice/hippocamp-packs".into(),
                path: String::new(),
                git_ref: None,
            }
        );
        assert_eq!(
            parse(
                "https://github.com:443/alice/hippocamp-packs//glossary-terms@v1.0.0",
                cwd_path,
                None
            )
            .expect("full"),
            Source::Git {
                url: "https://github.com:443/alice/hippocamp-packs".into(),
                path: "glossary-terms".into(),
                git_ref: Some("v1.0.0".into()),
            }
        );

        for bad in [
            "http://h/x",
            "https://user@h/x",
            "https://h/x?y",
            "https://h/x#f",
            "https://h/x%20y",
            "https://h/../x",
            "https://h/./x",
            "https://h/x@a@b",
            "https://h/x@-v",
            "https://h/x@a..b",
            "https://h/x@",
            "https://h/x//",
            "https://h/x//../y",
            "https://h",
            "Glossary",
            "a_b",
            "",
        ] {
            let err = parse(bad, cwd_path, None).expect_err(bad);
            assert!(matches!(err, Error::Pack(_)), "{bad}: {err}");
        }
        assert_eq!(
            parse("glossary-terms", cwd_path, None).expect("name"),
            Source::Registry {
                name: "glossary-terms".into()
            }
        );
        assert_eq!(
            parse("Glossary", cwd_path, None).expect_err("bad name").to_string(),
            "not a pack source: Glossary; use a path (./dir), an https git URL (https://host/repo[//dir][@ref]) or a registry name with --registry <path to index.json>"
        );
        assert!(parse("./missing", cwd_path, None).is_err());
        assert!(parse("~/packs", cwd_path, None).is_err());
    }

    #[test]
    fn read_path_reads_a_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(dir.path(), "pack.toml", b"x");
        write(dir.path(), ".DS_Store", b"x");
        let f = read_path(dir.path()).expect("reads");
        assert_eq!(f.commit, None);
        assert_eq!(f.files.paths().collect::<Vec<_>>(), ["pack.toml"]);
        assert_eq!(f.skipped, [".DS_Store"]);
    }

    /// Sets an environment variable for the length of a scope.
    struct EnvVar(&'static str, Option<std::ffi::OsString>);

    impl EnvVar {
        fn set(name: &'static str, value: impl AsRef<std::ffi::OsStr>) -> EnvVar {
            let old = std::env::var_os(name);
            std::env::set_var(name, value);
            EnvVar(name, old)
        }
    }

    impl Drop for EnvVar {
        fn drop(&mut self) {
            match &self.1 {
                Some(old) => std::env::set_var(self.0, old),
                None => std::env::remove_var(self.0),
            }
        }
    }

    #[test]
    fn git_runs_with_a_clean_environment_and_leaves_the_callers_repository_alone() {
        if !have_git() {
            return;
        }
        let _guard = serial();
        let source = new_repo();
        write(source.path(), "pack.toml", b"x");
        commit_all(source.path(), "pack");

        // The caller's repository, as a git hook would leave it in GIT_DIR.
        let theirs = new_repo();
        write(theirs.path(), "mine.txt", b"mine");
        commit_all(theirs.path(), "mine");
        let config = fs::read(theirs.path().join(".git/config")).expect("config");
        let objects = git(theirs.path(), &["count-objects", "-v"]);

        let fetched = {
            let _dir = EnvVar::set("GIT_DIR", theirs.path().join(".git"));
            let _tree = EnvVar::set("GIT_WORK_TREE", theirs.path());
            let _exec = EnvVar::set("GIT_EXEC_PATH", "/nonexistent/exec-path");
            let _count = EnvVar::set("GIT_CONFIG_COUNT", "1");
            let _key = EnvVar::set("GIT_CONFIG_KEY_0", "core.sshCommand");
            let _value = EnvVar::set("GIT_CONFIG_VALUE_0", "false");
            fetch_git(&url(source.path()), "", None, &options()).expect("fetches")
        };
        assert_eq!(fetched.files.paths().collect::<Vec<_>>(), ["pack.toml"]);

        assert_eq!(
            fs::read(theirs.path().join(".git/config")).expect("config"),
            config
        );
        assert!(!theirs.path().join(".git/FETCH_HEAD").exists());
        assert_eq!(git(theirs.path(), &["count-objects", "-v"]), objects);
        assert_eq!(git(theirs.path(), &["remote"]), "");
    }

    #[cfg(unix)]
    #[test]
    fn the_temporary_repository_is_made_without_the_users_templates_and_hooks() {
        if !have_git() {
            return;
        }
        use std::os::unix::fs::PermissionsExt;
        let _guard = serial();
        let templates = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(templates.path().join("hooks")).expect("mkdir");
        let hook = templates.path().join("hooks/reference-transaction");
        fs::write(&hook, "#!/bin/sh\ntouch \"$0.ran\"\n").expect("write");
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).expect("chmod");
        let home = tempfile::tempdir().expect("tempdir");
        fs::write(
            home.path().join(".gitconfig"),
            format!(
                "[init]\n\ttemplateDir = {}\n[core]\n\thooksPath = {}\n",
                templates.path().display(),
                templates.path().join("hooks").display()
            ),
        )
        .expect("config");
        let _home = EnvVar::set("HOME", home.path());
        let _xdg = EnvVar::set("XDG_CONFIG_HOME", home.path().join("xdg"));

        let source = new_repo();
        write(source.path(), "pack.toml", b"x");
        commit_all(source.path(), "pack");
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo.git");
        fetch_into(&repo, &url(source.path()), None, &options()).expect("fetches");
        assert!(!repo.join("hooks").join("reference-transaction").exists());
        assert!(!templates
            .path()
            .join("hooks/reference-transaction.ran")
            .exists());
    }

    #[test]
    fn git_that_prints_past_the_cap_is_killed() {
        if !have_git() {
            return;
        }
        let _guard = serial();
        let dir = tempfile::tempdir().expect("tempdir");
        let limits = Limits {
            stdout: 1000,
            ..Limits::default()
        };
        let started = Instant::now();
        let err = run(
            Some(dir.path()),
            &["-c", "alias.noisy=!yes", "noisy"],
            None,
            &options(),
            &limits,
        )
        .expect_err("killed");
        assert_eq!(err.to_string(), "git -c printed more than 1000 bytes");
        assert!(started.elapsed() < Duration::from_secs(30));

        let limits = Limits {
            stderr: 500,
            ..Limits::default()
        };
        let err = run(
            Some(dir.path()),
            &["-c", "alias.noisy=!yes >&2", "noisy"],
            None,
            &options(),
            &limits,
        )
        .expect_err("killed");
        assert_eq!(err.to_string(), "git -c printed more than 500 bytes");
    }

    #[test]
    fn a_repository_that_grows_past_the_limit_kills_the_fetch() {
        if !have_git() {
            return;
        }
        let _guard = serial();
        let dir = tempfile::tempdir().expect("tempdir");
        let limits = Limits {
            watch: Some((
                dir.path().to_path_buf(),
                50_000,
                "the fetch passed 50000 bytes; the server may not honour the size filter".into(),
            )),
            ..Limits::default()
        };
        let started = Instant::now();
        let err = run(
            Some(dir.path()),
            &[
                "-c",
                "alias.grow=!head -c 100000 /dev/zero > big; sleep 30",
                "grow",
            ],
            None,
            &options(),
            &limits,
        )
        .expect_err("killed");
        assert_eq!(
            err.to_string(),
            "the fetch passed 50000 bytes; the server may not honour the size filter"
        );
        assert!(started.elapsed() < Duration::from_secs(20));
        assert_eq!(size_text(16 * 1024 * 1024), "16 MiB");
        assert_eq!(size_text(50_000), "50000 bytes");
    }

    #[test]
    fn the_size_poll_counts_the_files_of_the_repository() {
        if !have_git() {
            return;
        }
        let _guard = serial();
        let source = new_repo();
        write(source.path(), "pack.toml", b"x");
        commit_all(source.path(), "pack");
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo.git");
        let commit = fetch_into(&repo, &url(source.path()), None, &options()).expect("fetches");
        assert_eq!(commit.len(), 40);
        assert!(dir_bytes(&repo) > 1);
        assert!(dir_bytes(&repo) < MAX_FETCH_BYTES);
        write(tmp.path(), "repo.git/extra", &[0u8; 1234]);
        assert!(dir_bytes(&repo) >= 1234);
    }

    #[test]
    fn more_than_the_file_limit_is_refused_without_reading_the_rest() {
        if !have_git() {
            return;
        }
        let _guard = serial();
        let repo = new_repo();
        for n in 0..=MAX_FILES + 5 {
            write(repo.path(), &format!("templates/t{n}.md"), b"x");
        }
        commit_all(repo.path(), "many");
        assert_eq!(
            fetch_err(repo.path(), ""),
            format!("pack: more than {MAX_FILES} files")
        );
    }

    #[test]
    fn text_from_git_is_escaped_in_errors() {
        if !have_git() {
            return;
        }
        let _guard = serial();
        let repo = new_repo();
        let file = blob(repo.path(), "x");
        let root = tree(
            repo.path(),
            &[
                ("100644", "blob", &file, "pack.toml"),
                ("100644", "blob", &file, "\u{1b}]0;pwned\u{7}"),
            ],
        );
        commit_tree(repo.path(), &root);
        let err = fetch_err(repo.path(), "");
        assert!(!err.contains('\u{1b}') && !err.contains('\u{7}'), "{err:?}");
        assert!(err.contains("\\u{1b}]0;pwned\\u{7}"), "{err}");

        // The last line git printed to standard error is escaped too.
        let dir = tempfile::tempdir().expect("tempdir");
        let err = run_git_ok(
            Some(dir.path()),
            &[
                "-c",
                "alias.bad=!printf '\\033[2Jboom\\n' >&2; exit 1",
                "bad",
            ],
            None,
            &options(),
        )
        .expect_err("fails");
        let err = err.to_string();
        assert!(!err.contains('\u{1b}'), "{err:?}");
        assert!(err.ends_with("failed: \\u{1b}[2Jboom"), "{err}");
    }

    #[test]
    fn every_git_source_obeys_the_url_ref_and_path_rules() {
        let ok = |url: &str, path: &str, r: Option<&str>| check_git_source(url, path, r, false);
        assert!(ok("https://github.com/a/b", "dir", Some("v1")).is_ok());
        assert!(ok("https://github.com/a/b", "", None).is_ok());
        assert!(ok("https://u@github.com/a/b", "", None).is_err());
        assert!(ok("https://github.com/a/b?x", "", None).is_err());
        assert!(ok("https://github.com/a/../b", "", None).is_err());
        assert!(ok("http://github.com/a/b", "", None).is_err());
        assert!(ok("file:///tmp/x", "", None).is_err());
        assert!(ok("https://github.com/a/b", "../x", None).is_err());
        for bad in ["-x", "--upload-pack=x", "a..b", "a b", "a\nb", "a:b", ""] {
            assert!(
                ok("https://github.com/a/b", "", Some(bad)).is_err(),
                "{bad:?}"
            );
        }
        // A file URL passes only for the tests' option.
        assert!(check_git_source("file:///tmp/x", "", None, true).is_ok());
        assert!(check_git_source("file:///tmp/x\n", "", None, true).is_err());
    }
}
