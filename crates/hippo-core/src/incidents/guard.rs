//! The guard's incident kinds, read from transcripts after the fact
//! (`docs/designs/guard.md`, Part A): `remote-script`, `fresh-clone-exec`,
//! `credential-read` and `guard-bypass`. Rules only, no model call; each
//! prefers precision to recall, so a case it cannot read with confidence is
//! left out. They read what the transcript records, a command's text, not
//! what it did.
//!
//! Every adapter's tool calls are read: shell calls by any of the names the
//! agents use (`Bash`, `shell`, `exec_command`, `run_shell_command`,
//! `Shell`, …), file reads and writes by their tool names and the path
//! keys each agent uses (`file_path`, `path`, `absolute_path`, …).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use super::signals::{one_line, Signal, SignalKind, SIGNAL_CHARS};
use crate::index::StoredTurn;
use crate::rollup::is_call;
use crate::xray::shell::{self, Simple};

/// Hosts (with an optional path prefix) whose installers do not count as
/// `remote-script`: each vendor's documented one-line installer.
pub const ALLOWED_HOSTS: &[&str] = &[
    // Homebrew's installer.
    "raw.githubusercontent.com/Homebrew/",
    // rustup.
    "sh.rustup.rs",
    "static.rust-lang.org/rustup/",
    // hippo's own installer.
    "raw.githubusercontent.com/VanitasCaesar1/hippo/",
    // uv, Bun, Deno, nvm, Claude Code: their documented install lines.
    "astral.sh/uv/",
    "bun.sh/install",
    "deno.land/install.sh",
    "raw.githubusercontent.com/nvm-sh/nvm/",
    "claude.ai/install.sh",
];

/// Clone origins whose setup does not count as `fresh-clone-exec`.
pub const ALLOWED_ORIGINS: &[&str] = &["github.com/VanitasCaesar1/hippo"];

/// Text a reason may quote: a host or path of plain characters, else
/// nothing (`docs/designs/guard.md`, Text in reasons).
pub fn shown(text: &str) -> String {
    static PLAIN: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^[A-Za-z0-9._:/~-]{1,200}$").expect("valid regex"));
    if PLAIN.is_match(text) {
        text.to_owned()
    } else {
        "(host not shown)".to_owned()
    }
}

/// The script a shell call runs, for every agent's shell tool.
pub(crate) fn shell_script(name: &str, input: &str) -> Option<String> {
    if let Some(s) = crate::rollup::work::command_of(name, input) {
        return Some(s);
    }
    let lower = name.to_ascii_lowercase();
    if !matches!(
        lower.as_str(),
        "run_shell_command"
            | "run_shell"
            | "shell_command"
            | "bash_command"
            | "shell"
            | "bash"
            | "run_command"
            | "execute_command"
            | "run_terminal_cmd"
            | "terminal"
            | "exec"
            | "tool.shell"
            | "tool.run_shell"
    ) {
        return None;
    }
    let v: Value = serde_json::from_str(input).ok()?;
    match v.get("command").or_else(|| v.get("cmd"))? {
        Value::String(s) => Some(s.clone()),
        Value::Array(a) => Some(
            a.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" "),
        ),
        _ => None,
    }
}

/// The working directory a shell call names for itself, if any (Codex's
/// `workdir`, Gemini's `directory`).
fn call_dir(input: &str) -> Option<String> {
    let v: Value = serde_json::from_str(input).ok()?;
    ["workdir", "cwd", "directory", "dir"]
        .iter()
        .find_map(|k| v.get(k).and_then(Value::as_str))
        .map(str::to_owned)
}

/// What a file tool does with its path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileOp {
    Read,
    Write,
}

/// A file tool's operation and the paths its input names.
pub(crate) fn file_call(name: &str, input: &str) -> Option<(FileOp, Vec<String>, String)> {
    let lower = name.to_ascii_lowercase();
    let op = match lower.as_str() {
        "read" | "read_file" | "readfile" | "view" | "view_file" | "open_file" | "cat"
        | "read_many_files" | "tool.read_file" | "notebookread" => FileOp::Read,
        "write" | "edit" | "multiedit" | "notebookedit" | "write_file" | "writefile"
        | "replace" | "edit_file" | "str_replace" | "str_replace_editor" | "create_file"
        | "apply_patch" | "tool.write_file" | "tool.edit_file" | "search_replace" => FileOp::Write,
        _ => return None,
    };
    if lower == "apply_patch" {
        let paths = crate::rollup::edited_paths("apply_patch", input);
        return Some((op, paths, input.to_owned()));
    }
    let v: Value = serde_json::from_str(input).ok()?;
    let mut paths = Vec::new();
    for key in [
        "file_path",
        "path",
        "absolute_path",
        "filePath",
        "target_file",
        "filename",
        "file",
        "notebook_path",
    ] {
        if let Some(p) = v.get(key).and_then(Value::as_str) {
            paths.push(p.to_owned());
        }
    }
    if let Some(Value::Array(items)) = v.get("paths") {
        paths.extend(items.iter().filter_map(Value::as_str).map(str::to_owned));
    }
    // The text a write puts in, for the rules that look at it.
    let text = [
        "content",
        "new_string",
        "old_string",
        "edits",
        "new_str",
        "old_str",
    ]
    .iter()
    .filter_map(|k| v.get(k))
    .map(|v| v.to_string())
    .collect::<Vec<_>>()
    .join("\n");
    (!paths.is_empty()).then_some((op, paths, text))
}

/// `path` with the home folder written `~`: this machine's home,
/// `$HOME`, `${HOME}`, `/Users/<name>`, `/home/<name>` and `/root` all
/// become `~`.
pub fn home_relative(path: &str) -> String {
    static HOME: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^(?:\$HOME|\$\{HOME\}|/Users/[^/]+|/home/[^/]+|/root|/var/root)(/|$)")
            .expect("valid regex")
    });
    let p = path.trim_matches(|c| c == '"' || c == '\'');
    if let Some(rest) = p.strip_prefix("~/") {
        return format!("~/{rest}");
    }
    if p == "~" {
        return "~".into();
    }
    // This machine's own home, wherever it is.
    if let Some(home) = crate::config::user_home() {
        let home = home.to_string_lossy();
        let home = home.trim_end_matches('/');
        if !home.is_empty() {
            if let Some(rest) = p
                .strip_prefix(home)
                .filter(|r| r.is_empty() || r.starts_with('/'))
            {
                return format!("~{rest}");
            }
        }
    }
    match HOME.captures(p) {
        Some(c) => format!("~{}{}", &c[1], &p[c[0].len()..]),
        None => p.to_owned(),
    }
}

// ---------------------------------------------------------------------------
// credential-read

/// The credential location `path` is in, or `None`.
pub fn credential_place(path: &str) -> Option<&'static str> {
    let p = home_relative(path);
    let lower = p.to_ascii_lowercase();
    let file = lower.rsplit('/').next().unwrap_or("");
    if let Some(rest) = lower.strip_prefix("~/.ssh/") {
        // Public keys, the hosts file and the client config hold no secret.
        if rest.is_empty()
            || file.ends_with(".pub")
            || file.starts_with("known_hosts")
            || file == "config"
            || file == "authorized_keys"
            || rest.contains('*')
        {
            return None;
        }
        return Some("ssh private key");
    }
    const FILES: &[(&str, &str)] = &[
        ("~/.aws/credentials", "AWS credentials"),
        ("~/.kube/config", "Kubernetes config"),
        ("~/.docker/config.json", "Docker credentials"),
        ("~/.netrc", ".netrc"),
        ("~/.npmrc", ".npmrc"),
        ("~/.pypirc", ".pypirc"),
        ("~/.git-credentials", "git credentials"),
        ("~/.config/gh/hosts.yml", "GitHub CLI token"),
    ];
    for (place, name) in FILES {
        if lower == *place {
            return Some(name);
        }
    }
    const FOLDERS: &[(&str, &str)] = &[
        ("~/.config/gcloud/", "gcloud credentials"),
        ("~/.azure/", "Azure credentials"),
        ("~/library/keychains/", "keychain"),
        ("~/.electrum/", "wallet"),
        ("~/library/application support/electrum/", "wallet"),
        ("~/library/application support/exodus/", "wallet"),
        ("~/.config/exodus/", "wallet"),
        ("~/library/application support/ledger live/", "wallet"),
        ("~/.config/ledger live/", "wallet"),
        (
            "~/library/application support/google/chrome/",
            "browser profile",
        ),
        (
            "~/library/application support/bravesoftware/",
            "browser profile",
        ),
        (
            "~/library/application support/microsoft edge/",
            "browser profile",
        ),
        (
            "~/library/application support/firefox/profiles/",
            "browser profile",
        ),
        (
            "~/library/application support/arc/user data/",
            "browser profile",
        ),
        ("~/.mozilla/firefox/", "browser profile"),
        ("~/.config/google-chrome/", "browser profile"),
        ("~/.config/chromium/", "browser profile"),
        ("~/.config/bravesoftware/", "browser profile"),
    ];
    for (place, name) in FOLDERS {
        if lower.starts_with(place) && lower.len() > place.len() {
            return Some(name);
        }
    }
    if lower.contains("/keystore/utc--") {
        return Some("wallet");
    }
    None
}

/// Programs whose path arguments are files they read.
const READERS: &[&str] = &[
    "cat", "less", "more", "head", "tail", "bat", "cp", "scp", "rsync", "tar", "zip", "base64",
    "xxd", "od", "hexdump", "strings", "grep", "rg", "egrep", "fgrep", "awk", "sqlite3", "openssl",
    "gpg", "curl", "nc", "jq", "yq", "source", ".",
];

/// The credential reads in one shell script: (place, the path or the
/// command, the words).
fn credential_reads(script: &str, commands: &[Simple]) -> Vec<(&'static str, String)> {
    static SUB: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\$\(([^()]*)\)").expect("valid regex"));
    // Command substitutions inside quotes too: a header built from
    // `"$(security find-generic-password -s … -w)"`.
    let inner: Vec<Simple> = SUB
        .captures_iter(script)
        .flat_map(|c| shell::simple_commands(&c[1]))
        .collect();
    let mut out: Vec<(&'static str, String)> = Vec::new();
    for c in commands.iter().chain(inner.iter()) {
        let Some((program, args)) = c.program() else {
            continue;
        };
        if program == "security"
            && args
                .first()
                .is_some_and(|a| a.starts_with("find-") && a.ends_with("-password"))
            && args.contains(&"-w")
        {
            // The item, by the service it is stored under (`-s`), else
            // its account (`-a`): what `hippo guard allow keychain` takes.
            let item = args
                .windows(2)
                .find(|w| w[0] == "-s")
                .or_else(|| args.windows(2).find(|w| w[0] == "-a"))
                .map(|w| w[1].trim_matches(|c| c == '"' || c == '\''))
                .unwrap_or("(no service)");
            out.push((
                "keychain password",
                format!("security {} -s {item}", args[0]),
            ));
            continue;
        }
        if !READERS.contains(&program.as_str()) {
            continue;
        }
        for a in &args {
            // `curl -T file`, `--data-binary @file`.
            let a = a.trim_start_matches('@');
            if a.starts_with('-') {
                continue;
            }
            if let Some(place) = credential_place(a) {
                out.push((place, home_relative(a)));
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// remote-script

static URL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"https?://([^\s'"`)|;&<>]+)"#).expect("valid regex"));

/// A download piped or substituted into an interpreter.
static PIPED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)
        \b(?:curl|wget|fetch|http)\b[^|;&\n]*\|\s*
          (?:sudo\s+(?:-\S+\s+)*)?(?:env\s+(?:\S+=\S*\s+)*)?
          (?:/usr/bin/|/bin/|/usr/local/bin/)?
          (?:sh|bash|zsh|dash|ksh|fish|python[0-9.]*|node|perl|ruby|php|pwsh|iex)\b
        | \b(?:sh|bash|zsh|dash|ksh|python[0-9.]*|source|\.)\s+(?:-\S+\s+)*<\(\s*(?:curl|wget)\b
        | \b(?:sh|bash|zsh|dash|ksh)\s+-c\s+[\x22']?\$\(\s*(?:curl|wget)\b
        | \b(?:python[0-9.]*|node|perl|ruby)\s+-\s*<\s*<\(\s*(?:curl|wget)\b
        | \b(?:iex|Invoke-Expression)\b[^\n]*\b(?:irm|iwr|Invoke-RestMethod|Invoke-WebRequest)\b
        | \b(?:irm|iwr|Invoke-RestMethod|Invoke-WebRequest)\b[^|\n]*\|\s*(?:iex|Invoke-Expression)\b
        ",
    )
    .expect("valid regex")
});

/// Whether an interpreter whose arguments start `rest` reads its script
/// from standard input: no arguments, `-s`, `-`, or only option flags,
/// and none of `-c`, `-e`, `-m`, `-E`, `-n`, `-p` or a script file.
fn reads_script_from_stdin(rest: &str) -> bool {
    // A closing quote ends the script of `bash -c '…'` too.
    let segment = rest
        .split(['|', ';', '&', ')', '\n', '\'', '"', '`'])
        .next()
        .unwrap_or("");
    for word in segment.split_whitespace() {
        if word == "--" || word == "-s" || word == "-" {
            return true;
        }
        if let Some(flags) = word.strip_prefix('-') {
            if flags.starts_with('-') {
                continue;
            }
            if flags
                .chars()
                .any(|c| matches!(c, 'c' | 'e' | 'm' | 'E' | 'n' | 'p'))
            {
                return false;
            }
            continue;
        }
        // A script file or a command for `iex`.
        return false;
    }
    true
}

/// Whether `url` (without its scheme) is on the built-in allow list.
fn allowed_host(url: &str) -> bool {
    ALLOWED_HOSTS.iter().any(|a| url.starts_with(a))
}

/// The remote scripts in one shell script: (host or pattern, the words).
fn remote_scripts(script: &str, commands: &[Simple]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let (body, _) = shell::heredocs(script);
    let literal = literal_mask(&body);
    let mut offset = 0;
    for line in body.split_inclusive('\n') {
        let at = offset;
        offset += line.len();
        let line = line.trim_end_matches('\n');
        let Some(m) = PIPED.find(line) else {
            continue;
        };
        // Text inside quotes is an argument, not a command: release notes
        // that quote an install line (`gh release create --notes "…"`).
        if literal.get(at + m.start()).copied().unwrap_or(false) {
            continue;
        }
        // A pipe into an interpreter that has its own script (`-c`, `-e`,
        // `-m`, a file) gives it data, not code: `| python3 -m json.tool`.
        if m.as_str().contains('|') && !reads_script_from_stdin(&line[m.end()..]) {
            continue;
        }
        // The download's URL: the first in the matched part, else in the
        // line (a variable holds it otherwise).
        let url = URL
            .captures(m.as_str())
            .or_else(|| URL.captures(line))
            .map(|c| c[1].to_owned());
        if url.as_deref().is_some_and(allowed_host) {
            continue;
        }
        // A comment or an echo of the line is not a run.
        let before = &line[..m.start()];
        if before.contains('#') || before.trim_start().starts_with("echo ") {
            continue;
        }
        let host = url
            .as_deref()
            .map(|u| shown(u.split('/').next().unwrap_or(u)))
            .unwrap_or_else(|| "(host not shown)".into());
        out.push((host, one_line(m.as_str(), SIGNAL_CHARS)));
    }
    for c in commands {
        let Some((program, args)) = c.program() else {
            continue;
        };
        let url_arg = |a: &str| {
            a.starts_with("http://")
                || a.starts_with("https://")
                || a.starts_with("git+")
                || a.starts_with("git://")
                || a.starts_with("git@")
        };
        let found = match (program.as_str(), args.first().copied()) {
            ("brew", Some("tap")) => args
                .get(1)
                .filter(|t| {
                    let t = t.to_ascii_lowercase();
                    t.contains('/') && !t.starts_with("homebrew/")
                })
                .map(|t| format!("tap {t}")),
            ("brew", Some("install" | "reinstall")) => args
                .iter()
                .skip(1)
                .find(|a| url_arg(a))
                .map(|u| (*u).to_owned()),
            ("npm" | "pnpm" | "yarn" | "bun", Some("i" | "install" | "add"))
                if args
                    .iter()
                    .any(|a| matches!(*a, "-g" | "--global" | "global")) =>
            {
                args.iter()
                    .skip(1)
                    .find(|a| url_arg(a))
                    .map(|u| (*u).to_owned())
            }
            ("pip" | "pip3" | "pipx", Some("install")) => args
                .iter()
                .skip(1)
                .find(|a| url_arg(a))
                .map(|u| (*u).to_owned()),
            _ => None,
        };
        if let Some(what) = found {
            let host = URL
                .captures(&what)
                .map(|c| c[1].to_owned())
                .or_else(|| what.split_once('@').map(|(_, h)| h.to_owned()));
            if host.as_deref().is_some_and(allowed_host) {
                continue;
            }
            let host = match (host, what.strip_prefix("tap ")) {
                (_, Some(tap)) => format!("tap {}", shown(tap)),
                (Some(h), None) => shown(h.split(['/', ':']).next().unwrap_or(&h)),
                (None, None) => shown(&what),
            };
            out.push((host, one_line(&c.words.join(" "), SIGNAL_CHARS)));
        }
    }
    out
}

/// For each byte of a script, whether it is literal text inside quotes:
/// single quotes, or double quotes outside a `$(…)` or backticks. The
/// quoted script of `sh -c '…'`, `bash -c "…"` or `eval "…"` runs, so it
/// is not literal.
pub(crate) fn literal_mask(script: &str) -> Vec<bool> {
    static RUNS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?:\b(?:sh|bash|zsh|dash|ksh)(?:\s+-[A-Za-z]+)*\s+-[A-Za-z]*c|\beval)\s*$")
            .expect("valid regex")
    });
    #[derive(Clone, Copy, PartialEq)]
    enum Ctx {
        Code,
        Single { runs: bool },
        Double { runs: bool },
        Sub,
        Tick,
    }
    let bytes = script.as_bytes();
    let mut mask = vec![false; bytes.len()];
    let mut stack = vec![Ctx::Code];
    let mut i = 0;
    while i < bytes.len() {
        let top = *stack.last().unwrap_or(&Ctx::Code);
        let b = bytes[i];
        mask[i] = matches!(
            top,
            Ctx::Single { runs: false } | Ctx::Double { runs: false }
        );
        match top {
            Ctx::Code | Ctx::Sub | Ctx::Tick => match b {
                b'\\' => i += 1,
                b'\'' | b'"' => {
                    let runs = RUNS.is_match(&script[..i]);
                    stack.push(if b == b'\'' {
                        Ctx::Single { runs }
                    } else {
                        Ctx::Double { runs }
                    });
                }
                b'$' if bytes.get(i + 1) == Some(&b'(') => {
                    stack.push(Ctx::Sub);
                    i += 1;
                }
                b'(' if top == Ctx::Sub => stack.push(Ctx::Sub),
                b')' if top == Ctx::Sub => {
                    stack.pop();
                }
                b'`' if top == Ctx::Tick => {
                    stack.pop();
                }
                b'`' => stack.push(Ctx::Tick),
                _ => {}
            },
            Ctx::Single { .. } => {
                if b == b'\'' {
                    stack.pop();
                }
            }
            Ctx::Double { .. } => match b {
                b'\\' => i += 1,
                b'"' => {
                    stack.pop();
                }
                b'$' if bytes.get(i + 1) == Some(&b'(') => {
                    stack.push(Ctx::Sub);
                    i += 1;
                }
                b'`' => stack.push(Ctx::Tick),
                _ => {}
            },
        }
        i += 1;
    }
    mask
}

// ---------------------------------------------------------------------------
// fresh-clone-exec

/// Whether a clone's source is another machine: a URL with a host
/// (`https://`, `ssh://`, `git://`), `user@host:path`, or `owner/repo`
/// for `gh repo clone`. A local path or `file://` is not a download.
pub(crate) fn remote_source(url: &str) -> bool {
    let u = url.trim_matches(|c| c == '"' || c == '\'');
    if u.starts_with("file://")
        || u.starts_with('/')
        || u.starts_with('.')
        || u.starts_with('~')
        || u.starts_with('$')
    {
        return false;
    }
    ["https://", "http://", "ssh://", "git://", "git+"]
        .iter()
        .any(|p| u.starts_with(p))
        || u.split_once('@')
            .is_some_and(|(_, rest)| rest.contains(':'))
}

/// `dir` joined to `base` lexically; `dir` as it is when absolute or `~`.
fn join(base: &str, dir: &str) -> String {
    let dir = dir.trim_matches(|c| c == '"' || c == '\'');
    if dir.starts_with('/') || dir.starts_with('~') {
        return normal(dir);
    }
    normal(&format!("{}/{dir}", base.trim_end_matches('/')))
}

/// `path` with `.` and `..` worked out, without a trailing slash.
fn normal(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for p in path.split('/') {
        match p {
            "" | "." if !parts.is_empty() => {}
            ".." if parts.len() > 1 => {
                parts.pop();
            }
            _ => parts.push(p),
        }
    }
    let s = parts.join("/");
    if s.is_empty() {
        "/".into()
    } else {
        s
    }
}

/// The folder a clone or an unpacked download lands in, relative to where
/// it ran, and its origin, from one simple command.
fn clone_of(program: &str, args: &[&str]) -> Option<(String, String)> {
    let plain: Vec<&str> = args
        .iter()
        .copied()
        .filter(|a| !a.starts_with('-'))
        .collect();
    match (program, plain.as_slice()) {
        ("git", ["clone", url, rest @ ..]) if remote_source(url) => {
            let dest = rest
                .first()
                .map(|d| (*d).to_owned())
                .unwrap_or_else(|| repo_name(url));
            Some((dest, origin(url)))
        }
        ("gh", ["repo", "clone", repo, rest @ ..]) if !repo.starts_with(['/', '.', '~', '$']) => {
            let dest = rest
                .first()
                .map(|d| (*d).to_owned())
                .unwrap_or_else(|| repo_name(repo));
            let origin = if repo.contains("://") || repo.contains('@') {
                origin(repo)
            } else {
                format!("github.com/{repo}")
            };
            Some((dest, origin))
        }
        _ => None,
    }
}

/// `owner/repo` from a clone URL, without `.git`.
fn repo_name(url: &str) -> String {
    let last = url
        .trim_end_matches('/')
        .rsplit(['/', ':'])
        .next()
        .unwrap_or(url);
    last.trim_end_matches(".git").to_owned()
}

/// `host/owner/repo` of a clone URL.
fn origin(url: &str) -> String {
    let u = url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("ssh://")
        .trim_start_matches("git://");
    let u = match u.split_once('@') {
        Some((_, rest)) if !u.starts_with("github.com") => rest.replacen(':', "/", 1),
        _ => u.to_owned(),
    };
    u.trim_end_matches('/').trim_end_matches(".git").to_owned()
}

/// What a simple command runs of the code in the folder it runs in, or
/// in a folder it names: (the folder it names, if any, the words).
pub(crate) fn executes_repo_code(program: &str, args: &[&str]) -> Option<Option<String>> {
    let plain: Vec<&str> = args
        .iter()
        .copied()
        .filter(|a| !a.starts_with('-'))
        .collect();
    if args.contains(&"--ignore-scripts") {
        return None;
    }
    let flag_value = |flag: &str| {
        args.windows(2)
            .find(|w| w[0] == flag)
            .map(|w| w[1].to_owned())
    };
    match program {
        "npm" | "pnpm" | "yarn" | "bun" => match plain.first() {
            // A bare `yarn` installs.
            None if program == "yarn" => Some(flag_value("--cwd")),
            Some(&("install" | "i" | "ci")) if plain.len() == 1 => Some(
                flag_value("--prefix")
                    .or_else(|| flag_value("--cwd"))
                    .or_else(|| flag_value("-C")),
            ),
            _ => None,
        },
        "pip" | "pip3" => {
            let target = plain
                .get(1)
                .copied()
                .filter(|_| plain.first() == Some(&"install"))?;
            let local = target == "."
                || target.starts_with("./")
                || args.windows(2).any(|w| w[0] == "-e" && w[1] == target);
            local.then(|| (target != ".").then(|| target.to_owned()))
        }
        "python" | "python3" => (plain.first() == Some(&"setup.py")).then_some(None),
        "make" | "gmake" | "cmake" => Some(flag_value("-C")),
        "configure" => Some(None),
        "cargo" => {
            matches!(plain.first(), Some(&("build" | "run" | "test" | "install"))).then(|| {
                flag_value("--manifest-path").map(|m| m.trim_end_matches("/Cargo.toml").to_owned())
            })
        }
        "go" => (plain.first() == Some(&"generate")).then_some(None),
        "docker" => {
            (plain.first() == Some(&"build")).then(|| plain.last().map(|c| (*c).to_owned()))
        }
        "sh" | "bash" | "zsh" => {
            let file = plain.first()?;
            file.ends_with(".sh").then(|| {
                let dir = file.rsplit_once('/').map(|(d, _)| d.to_owned());
                dir
            })
        }
        p if p.ends_with(".sh") || p == "install" || p == "setup" => Some(None),
        _ => None,
    }
}

/// One clone the session made.
#[derive(Debug, Clone)]
struct Clone {
    dir: String,
    origin: String,
    words: String,
    ordinal: u32,
    ts: String,
}

// ---------------------------------------------------------------------------
// guard-bypass

/// The homes of this install, as the running process sees them: the
/// person's and hippo's. Writes count only when they land in these real
/// places, never because a path merely names `$HIPPO_HOME` or a
/// `settings.json` (a test's scratch home is not the person's).
#[derive(Debug, Clone, Default)]
pub(crate) struct Real {
    pub home: Option<PathBuf>,
    pub hippo: Option<PathBuf>,
}

impl Real {
    pub(crate) fn now() -> Real {
        Real {
            home: crate::config::user_home().map(|h| real_path(&h)),
            hippo: Some(real_path(&crate::config::home())),
        }
    }

    /// `path` as an absolute path on this machine; `None` when it depends
    /// on a folder the text does not fix: a relative path (its folder is
    /// not known here), any variable but `$HOME`, or `~` and `$HOME` in a
    /// script that sets `HOME` itself.
    pub(crate) fn place(&self, path: &str, home_moved: bool) -> Option<PathBuf> {
        let p = path.trim_matches(|c| c == '"' || c == '\'');
        let home_rest = p
            .strip_prefix("~/")
            .or_else(|| p.strip_prefix("$HOME/"))
            .or_else(|| p.strip_prefix("${HOME}/"));
        let abs = match home_rest {
            Some(_) if home_moved => return None,
            Some(rest) => self.home.as_ref()?.join(rest),
            None if p.starts_with('/') => PathBuf::from(p),
            None => return None,
        };
        Some(real_path(&abs))
    }
}

/// Whether a script sets `HOME` for itself or a command
/// (`HOME=/tmp/x cmd`, `export HOME=…`), so its `~` is not the person's.
pub(crate) fn moves_home(script: &str) -> bool {
    static SETS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?:^|[\s;&|(])(?:export\s+)?HOME=").expect("valid regex"));
    SETS.is_match(script)
}

/// `path` with `.` and `..` removed and the longest part that exists
/// resolved through links (`/var` is `/private/var` on macOS).
pub(crate) fn real_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            c => out.push(c),
        }
    }
    let mut existing = out.as_path();
    let mut rest = Vec::new();
    while !existing.exists() {
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_owned());
                existing = parent;
            }
            _ => return out,
        }
    }
    let mut real = std::fs::canonicalize(existing).unwrap_or_else(|_| existing.to_path_buf());
    for name in rest.into_iter().rev() {
        real.push(name);
    }
    real
}

/// The file a write lands in, if it is one of this install's real files
/// that turn hooks off or hold hippo's state; and whether only a write
/// that mentions hooks or hippo counts there (the settings files, which
/// hold much else).
fn guard_file(path: &str, real: &Real, home_moved: bool) -> Option<(&'static str, bool)> {
    let abs = real.place(path, home_moved)?;
    let state = |rest: &str| {
        rest == "config.toml"
            || rest == "bin-path"
            || rest.starts_with("guard/")
            || rest.starts_with("accepted/")
    };
    if let Some(rest) = real
        .hippo
        .as_ref()
        .and_then(|h| abs.strip_prefix(h).ok())
        .map(|r| r.to_string_lossy().into_owned())
    {
        if state(&rest) {
            return Some(("hippo's state", false));
        }
    }
    let rel = abs
        .strip_prefix(real.home.as_ref()?)
        .ok()?
        .to_string_lossy()
        .to_ascii_lowercase();
    let parts: Vec<&str> = rel.split('/').collect();
    match parts.as_slice() {
        [account, "settings.json" | "settings.local.json"] if account.starts_with(".claude") => {
            Some(("Claude Code settings", true))
        }
        [".codex", "hooks.json"] => Some(("Codex hooks", false)),
        [".codex", "config.toml"] => Some(("Codex config", true)),
        [".cursor", "hooks.json"] => Some(("Cursor hooks", false)),
        [".gemini", "settings.json"] => Some(("Gemini CLI settings", true)),
        [".dsh", "profiles", _, "cordis.patch.yml"] => Some(("dsh profile", true)),
        [".config", "opencode", "opencode.json" | "opencode.jsonc"] => {
            Some(("OpenCode config", true))
        }
        [".hippo", rest @ ..] if state(&rest.join("/")) => Some(("hippo's state", false)),
        _ => None,
    }
}

/// Whether text written into a settings file touches hooks or hippo.
fn mentions_hooks(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    t.contains("hook") || t.contains("hippo")
}

/// The guard bypasses in one shell script: (what, the words).
fn shell_bypasses(script: &str, commands: &[Simple], real: &Real) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    let home_moved = moves_home(script);
    for c in commands {
        let words = || one_line(&c.words.join(" "), SIGNAL_CHARS);
        let Some((program, args)) = c.program() else {
            continue;
        };
        if program == "hippo" {
            let sub: Vec<&str> = args
                .iter()
                .copied()
                .filter(|a| !a.starts_with('-'))
                .collect();
            match sub.as_slice() {
                ["hooks", "uninstall", ..] => out.push(("hippo hooks uninstall", words())),
                ["accept", ..] => out.push(("hippo accept", words())),
                ["guard", "accept" | "allow" | "uninstall", ..] => {
                    out.push(("hippo guard", words()))
                }
                _ => {}
            }
            continue;
        }
        let mut targets: Vec<String> = c.written.clone();
        match program.as_str() {
            "rm" | "unlink" | "truncate" => targets.extend(args.iter().map(|a| (*a).to_owned())),
            "mv" | "cp" | "install" | "ln" => {
                let plain: Vec<&str> = args
                    .iter()
                    .copied()
                    .filter(|a| !a.starts_with('-'))
                    .collect();
                if program == "mv" {
                    targets.extend(plain.iter().map(|a| (*a).to_owned()));
                } else if let Some(last) = plain.last() {
                    targets.push((*last).to_owned());
                }
            }
            "tee" => targets.extend(
                args.iter()
                    .filter(|a| !a.starts_with('-'))
                    .map(|a| (*a).to_owned()),
            ),
            "sed" | "perl" if args.iter().any(|a| a.starts_with("-i")) => targets.extend(
                args.iter()
                    .filter(|a| !a.starts_with('-'))
                    .map(|a| (*a).to_owned()),
            ),
            _ => {}
        }
        for t in targets {
            let Some((what, needs_mention)) = guard_file(&t, real, home_moved) else {
                continue;
            };
            let removes = matches!(program.as_str(), "rm" | "unlink" | "truncate" | "mv");
            if needs_mention && !removes && !mentions_hooks(script) {
                continue;
            }
            out.push((what, words()));
            break;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// The signals

fn signal(kind: SignalKind, t: &StoredTurn, step: String, text: String, extra: String) -> Signal {
    Signal {
        kind,
        ordinal: t.ordinal,
        ts: t.ts.clone(),
        step_ordinal: Some(t.ordinal),
        step_ts: Some(t.ts.clone()),
        step,
        text,
        extra,
    }
}

/// The guard's signals in a session's turns.
pub(crate) fn signals(turns: &[StoredTurn]) -> Vec<Signal> {
    let mut out = Vec::new();
    let real = Real::now();
    let mut clones: Vec<Clone> = Vec::new();
    // The shell's folder, followed from call to call; `.` until a `cd`.
    let mut cwd = String::from(".");
    let mut fired: HashMap<(SignalKind, String), ()> = HashMap::new();
    let mut push = |out: &mut Vec<Signal>, s: Signal| {
        let key = (s.kind, format!("{}{}", s.ordinal, s.extra));
        if fired.insert(key, ()).is_none() {
            out.push(s);
        }
    };
    for t in turns {
        if t.role != "tool" || !is_call(t) {
            continue;
        }
        let Some((name, input)) = crate::dream::observed::call(t) else {
            continue;
        };
        if let Some((op, paths, text)) = file_call(name, input) {
            for path in &paths {
                match op {
                    FileOp::Read => {
                        if let Some(place) = credential_place(path) {
                            push(
                                &mut out,
                                signal(
                                    SignalKind::CredentialRead,
                                    t,
                                    format!("{name}: {}", home_relative(path)),
                                    format!("{name} {}", home_relative(path)),
                                    place.to_owned(),
                                ),
                            );
                        }
                    }
                    FileOp::Write => {
                        if let Some((what, needs_mention)) = guard_file(path, &real, false) {
                            if !needs_mention || mentions_hooks(&text) {
                                push(
                                    &mut out,
                                    signal(
                                        SignalKind::GuardBypass,
                                        t,
                                        format!("{name}: {}", home_relative(path)),
                                        format!("{name} {}", home_relative(path)),
                                        what.to_owned(),
                                    ),
                                );
                            }
                        }
                    }
                }
            }
            continue;
        }
        let Some(script) = shell_script(name, input) else {
            continue;
        };
        let step = format!("{name}: {}", one_line(&script, SIGNAL_CHARS));
        let commands = shell::simple_commands(&script);
        for (host, words) in remote_scripts(&script, &commands) {
            push(
                &mut out,
                signal(SignalKind::RemoteScript, t, step.clone(), words, host),
            );
        }
        let mut reads = credential_reads(&script, &commands);
        reads.dedup();
        for (place, what) in reads {
            push(
                &mut out,
                signal(
                    SignalKind::CredentialRead,
                    t,
                    step.clone(),
                    what,
                    place.to_owned(),
                ),
            );
        }
        for (what, words) in shell_bypasses(&script, &commands, &real) {
            push(
                &mut out,
                signal(
                    SignalKind::GuardBypass,
                    t,
                    step.clone(),
                    words,
                    what.to_owned(),
                ),
            );
        }
        // Clones and what runs in them, following the script's `cd`s.
        let mut dir = call_dir(input).map_or_else(|| cwd.clone(), |d| join(&cwd, &d));
        for c in &commands {
            let Some((program, args)) = c.program() else {
                continue;
            };
            if program == "cd" || program == "pushd" {
                if let Some(d) = args.first() {
                    dir = join(&dir, d);
                }
                continue;
            }
            if let Some((dest, origin)) = clone_of(&program, &args) {
                if !ALLOWED_ORIGINS.iter().any(|a| origin.starts_with(a)) {
                    clones.push(Clone {
                        dir: join(&dir, &dest),
                        origin,
                        words: one_line(&c.words.join(" "), SIGNAL_CHARS),
                        ordinal: t.ordinal,
                        ts: t.ts.clone(),
                    });
                }
                continue;
            }
            let program_path = c
                .words
                .iter()
                .find(|w| !w.contains('='))
                .map(String::as_str)
                .unwrap_or("");
            let Some(named) = executes_repo_code(&program, &args) else {
                continue;
            };
            // Where it runs: the folder it names, or a script's own path
            // (`./repo/install.sh`), or the current folder.
            let at = match named {
                Some(d) => join(&dir, &d),
                None if program_path.contains('/') && !program_path.starts_with('/') => {
                    let folder = program_path.rsplit_once('/').map_or(".", |(d, _)| d);
                    join(&dir, folder)
                }
                None => dir.clone(),
            };
            let Some(clone) = clones
                .iter()
                .rev()
                .find(|k| at == k.dir || at.starts_with(&format!("{}/", k.dir)))
            else {
                continue;
            };
            let words = one_line(&c.words.join(" "), SIGNAL_CHARS);
            push(
                &mut out,
                Signal {
                    kind: SignalKind::FreshCloneExec,
                    ordinal: t.ordinal,
                    ts: t.ts.clone(),
                    step_ordinal: Some(clone.ordinal),
                    step_ts: Some(clone.ts.clone()),
                    step: clone.words.clone(),
                    text: words,
                    extra: shown(&clone.origin),
                },
            );
        }
        // A shell that keeps its folder between calls (Claude Code's)
        // stays where the script left it.
        if call_dir(input).is_none() {
            cwd = dir;
        }
    }
    out
}

#[cfg(test)]
mod tests;
