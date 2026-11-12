//! Where a session worked and which pull requests it opened or merged, read
//! from its calls and their results rather than from what its replies name.
//!
//! A session's working directory is often not where it worked: an agent
//! started in one checkout edits another, or a worktree of it. So the
//! project is the repository that most of its file edits and `git` and `gh`
//! commands point at, and the working directory only when there are none.
//!
//! A pull request counts only when the session opened it (a `gh pr create`
//! whose output is the new pull request's URL, or a tool that creates one
//! and returns its URL) or merged it (a `gh pr merge` that did not fail).
//! Pull requests a reply or a command merely names are not counted.

use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

use crate::index::StoredTurn;

use super::changes;
use super::{is_call, is_edit_tool, pull_urls};

/// A repository found on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    /// Its name: the last part of its `origin` remote, else the main
    /// checkout's directory name (a worktree's is its main checkout's).
    pub name: String,
    /// `owner/name`, from a GitHub `origin` remote.
    pub github: Option<String>,
}

/// Finds the repository a path is in, by looking for `.git` in it and its
/// parents. Remembers each directory it looked at; reads, never writes.
#[derive(Debug, Default)]
pub struct Repos {
    roots: HashMap<PathBuf, Option<Repo>>,
    home: Option<PathBuf>,
}

impl Repos {
    /// With the user's home directory, which is never a project even when
    /// it holds a `.git` (a dotfiles checkout).
    pub fn new() -> Repos {
        Repos {
            roots: HashMap::new(),
            home: std::env::var_os("HOME").map(PathBuf::from),
        }
    }

    /// The repository holding `path`, a file or directory that need not
    /// exist any more. A path under `.claude/worktrees/` belongs to the
    /// checkout above it, a path under `.codex/worktrees/<id>/<name>` to
    /// `<name>`, even when the worktree is gone.
    pub fn of(&mut self, path: &Path) -> Option<Repo> {
        if !path.is_absolute() {
            return None;
        }
        for dir in path.ancestors() {
            if dir.parent().is_none() || Some(dir) == self.home.as_deref() {
                break;
            }
            if let Some(found) = self.root(dir) {
                return Some(found);
            }
        }
        let text = path.to_string_lossy();
        if let Some(at) = text.find("/.claude/worktrees/") {
            return self.of(Path::new(&text[..at]));
        }
        if let Some(at) = text.find("/.codex/worktrees/") {
            let name = text[at..].split('/').nth(4).filter(|n| !n.is_empty())?;
            return Some(Repo {
                name: name.to_owned(),
                github: None,
            });
        }
        None
    }

    /// The repository holding `path` and the path inside it, when a
    /// checkout on disk holds it (two worktrees of one repository give the
    /// same name): incidents compare files by it.
    pub fn relative(&mut self, path: &Path) -> Option<(String, String)> {
        if !path.is_absolute() {
            return None;
        }
        for dir in path.ancestors().skip(1) {
            if dir.parent().is_none() || Some(dir) == self.home.as_deref() {
                break;
            }
            if let Some(found) = self.root(dir) {
                let rel = path.strip_prefix(dir).ok()?;
                return Some((found.name, rel.to_string_lossy().into_owned()));
            }
        }
        None
    }

    /// `dir`'s repository when `dir` holds a `.git`.
    fn root(&mut self, dir: &Path) -> Option<Repo> {
        if let Some(hit) = self.roots.get(dir) {
            return hit.clone();
        }
        let dot = dir.join(".git");
        let found = match fs::symlink_metadata(&dot) {
            Ok(m) if m.is_dir() => Some(repo_at(dir)),
            Ok(m) if m.is_file() => Some(match worktree_main(dir, &dot) {
                Some(main) => repo_at(&main),
                None => repo_at(dir),
            }),
            _ => None,
        };
        self.roots.insert(dir.to_owned(), found.clone());
        found
    }
}

/// A worktree's main checkout, from its `.git` file
/// (`gitdir: <main>/.git/worktrees/<name>`).
fn worktree_main(dir: &Path, dot: &Path) -> Option<PathBuf> {
    let text = fs::read_to_string(dot).ok()?;
    let gitdir = text.lines().find_map(|l| l.strip_prefix("gitdir:"))?.trim();
    let gitdir = dir.join(gitdir);
    let s = gitdir.to_string_lossy();
    let at = s.find("/.git/worktrees/")?;
    Some(PathBuf::from(&s[..at]))
}

/// The repository whose main checkout is `main`.
fn repo_at(main: &Path) -> Repo {
    let remote = fs::read_to_string(main.join(".git").join("config"))
        .ok()
        .and_then(|c| origin_url(&c));
    let from_remote = remote.as_deref().and_then(remote_parts);
    let name = from_remote
        .as_ref()
        .map(|(_, name)| name.clone())
        .or_else(|| main.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_default();
    let github = match (&remote, &from_remote) {
        (Some(url), Some((owner, name))) if url.contains("github.com") => {
            Some(format!("{owner}/{name}"))
        }
        _ => None,
    };
    Repo { name, github }
}

/// The `url` of `[remote "origin"]` in a git config.
fn origin_url(config: &str) -> Option<String> {
    let mut inside = false;
    for line in config.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            inside = t == "[remote \"origin\"]";
        } else if inside {
            if let Some((k, v)) = t.split_once('=') {
                if k.trim() == "url" {
                    return Some(v.trim().to_owned());
                }
            }
        }
    }
    None
}

/// `(owner, name)` of a remote URL: `git@host:owner/name.git`,
/// `https://host/owner/name`, `ssh://git@host/owner/name.git`.
fn remote_parts(url: &str) -> Option<(String, String)> {
    let path = match url.split_once("://") {
        Some((_, rest)) => rest.split_once('/')?.1,
        None => url.split_once(':')?.1,
    };
    let mut parts = path.trim_end_matches('/').rsplit('/');
    let name = parts.next()?.trim_end_matches(".git");
    let owner = parts.next()?;
    (!name.is_empty() && !owner.is_empty()).then(|| (owner.to_owned(), name.to_owned()))
}

// ---------------------------------------------------------------------------
// Commands

/// `path` without `.` and `..` parts, joined to `base` when relative; `~`
/// is the home directory.
fn resolve(base: Option<&Path>, path: &str, home: Option<&Path>) -> Option<PathBuf> {
    let joined = if let Some(rest) = path.strip_prefix("~/") {
        home?.join(rest)
    } else if path == "~" {
        home?.to_owned()
    } else if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        base?.join(path)
    };
    let mut out = PathBuf::new();
    for c in joined.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            c => out.push(c),
        }
    }
    Some(out)
}

/// A shell command as simple commands, each its words: split at `&&`,
/// `||`, `;`, `|`, `&`, parentheses, `$(` and line breaks outside quotes,
/// so that the command inside `url=$(gh pr create …)` is one too. Quotes
/// are removed. Good enough to find `cd`, `git` and `gh` and their
/// arguments; not a shell.
pub(crate) fn simple_commands(command: &str) -> Vec<Vec<String>> {
    let command = without_heredocs(command);
    let mut out: Vec<Vec<String>> = Vec::new();
    let mut words: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut quoted = false; // the word had quotes, so "" is a word
    let mut quote: Option<char> = None;
    let mut chars = command.chars().peekable();
    let end_word = |word: &mut String, quoted: &mut bool, words: &mut Vec<String>| {
        if !word.is_empty() || *quoted {
            words.push(std::mem::take(word));
        }
        *quoted = false;
    };
    while let Some(c) = chars.next() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else if c == '\\' && q == '"' {
                if let Some(n) = chars.next() {
                    word.push(n);
                }
            } else {
                word.push(c);
            }
            continue;
        }
        let split = match c {
            '\'' | '"' => {
                quote = Some(c);
                quoted = true;
                false
            }
            '\\' => {
                if let Some(n) = chars.next().filter(|n| *n != '\n') {
                    word.push(n);
                }
                false
            }
            '$' if chars.peek() == Some(&'(') => {
                chars.next();
                true
            }
            ' ' | '\t' => {
                end_word(&mut word, &mut quoted, &mut words);
                false
            }
            '&' | '|' | ';' | '\n' | '(' | ')' | '`' => true,
            _ => {
                word.push(c);
                false
            }
        };
        if split {
            end_word(&mut word, &mut quoted, &mut words);
            if !words.is_empty() {
                out.push(std::mem::take(&mut words));
            }
        }
    }
    end_word(&mut word, &mut quoted, &mut words);
    if !words.is_empty() {
        out.push(words);
    }
    out
}

/// `command` with the bodies of its here-documents (`<<EOF` … `EOF`,
/// `<<-'EOF'`, `<< "END"`) removed: a commit message or a script there is
/// text, not commands, and its quotes would unbalance the rest.
fn without_heredocs(command: &str) -> String {
    static MARK: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"<<-?\s*(?:'([^']+)'|"([^"]+)"|\\?([A-Za-z_][A-Za-z0-9_]*))"#)
            .expect("valid regex")
    });
    let mut out = String::with_capacity(command.len());
    let mut until: Vec<String> = Vec::new();
    for line in command.split_inclusive('\n') {
        if let Some(end) = until.first() {
            if line.trim() == end {
                until.remove(0);
            }
            continue;
        }
        for c in MARK.captures_iter(line) {
            if let Some(m) = c.get(1).or_else(|| c.get(2)).or_else(|| c.get(3)) {
                until.push(m.as_str().to_owned());
            }
        }
        out.push_str(&MARK.replace_all(line, ""));
    }
    out
}

/// A simple command's words after variable assignments and wrappers such
/// as `env`, `timeout 60` and `command`.
fn program(words: &[String]) -> &[String] {
    let mut i = 0;
    while i < words.len() {
        let w = words[i].as_str();
        let assignment = w.contains('=') && !w.starts_with('-') && !w.starts_with('=');
        if assignment
            || matches!(
                w,
                "env"
                    | "command"
                    | "exec"
                    | "time"
                    | "nohup"
                    | "sudo"
                    | "if"
                    | "then"
                    | "else"
                    | "elif"
                    | "do"
                    | "while"
                    | "until"
                    | "!"
                    | "{"
            )
        {
            i += 1;
        } else if w == "timeout" {
            i += 1;
            while words.get(i).is_some_and(|w| w.starts_with('-')) {
                i += 1;
            }
            i += 1; // the duration
        } else {
            break;
        }
    }
    &words[i.min(words.len())..]
}

/// `gh` flags that take a value, of the subcommands read here.
const GH_VALUE_FLAGS: &[&str] = &[
    "-R",
    "--repo",
    "-b",
    "--body",
    "-F",
    "--body-file",
    "-t",
    "--title",
    "--subject",
    "-B",
    "--base",
    "-H",
    "--head",
    "-A",
    "--author-email",
    "--match-head-commit",
    "-a",
    "--assignee",
    "-l",
    "--label",
    "-m",
    "--milestone",
    "-p",
    "--project",
    "-r",
    "--reviewer",
    "-T",
    "--template",
    "-X",
    "--method",
    "-f",
    "--raw-field",
    "--field",
    "-H",
    "--header",
    "-q",
    "--jq",
];

/// The value of flag `names` among `words`: `-R x`, `--repo x`, `--repo=x`.
/// A pull request's title as kept: one line, at most 200 characters.
fn pull_title(title: &str) -> String {
    let line = title.split_whitespace().collect::<Vec<_>>().join(" ");
    line.chars().take(200).collect()
}

fn flag_value(words: &[String], names: &[&str]) -> Option<String> {
    let mut it = words.iter();
    while let Some(w) = it.next() {
        if names.contains(&w.as_str()) {
            return it.next().cloned();
        }
        for n in names {
            if let Some(v) = w.strip_prefix(&format!("{n}=")) {
                return Some(v.to_owned());
            }
        }
    }
    None
}

/// The words of `words` that are not flags or flag values.
fn positionals<'a>(words: &'a [String], value_flags: &[&str]) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut skip = false;
    for w in words {
        if skip {
            skip = false;
            continue;
        }
        if w.starts_with('-') && w.len() > 1 {
            skip = value_flags.contains(&w.as_str());
            continue;
        }
        out.push(w.as_str());
    }
    out
}

/// What a session's calls did, as [`read`] finds it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Work {
    /// The repository most of its edits and `git`/`gh` commands point at,
    /// and how many did; `(cwd's repository or directory name, 0)` when
    /// none did.
    pub project: Option<(String, u64)>,
    /// `(url, opened, merged, ts)`: the pull requests it opened or merged,
    /// in the order of the first such call.
    pub pulls: Vec<(String, PullDone, String)>,
    /// The title each opened pull request was given (`gh pr create
    /// --title`, an MCP call's `title`), by URL.
    pub pull_titles: Vec<(String, String)>,
    /// Lines changed per file its edit calls named, in the order first
    /// named; `calls` and `uncounted` per file.
    pub changes: Vec<Changed>,
    /// The newest branch its `git` and `gh` commands named.
    pub branch: Option<String>,
    /// Its calls with their results, as [`pairs_of`] finds them.
    pub pairs: Vec<(usize, Option<usize>)>,
}

/// One file's changes over a session's edit calls.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Changed {
    pub path: String,
    pub lines: changes::Lines,
    /// Edit calls that named it.
    pub calls: u64,
    /// Of those, calls whose lines are not known.
    pub uncounted: u64,
}

fn note_changes(out: &mut Vec<Changed>, files: Vec<changes::FileLines>) {
    for f in files {
        let at = match out.iter().position(|c| c.path == f.path) {
            Some(i) => i,
            None => {
                out.push(Changed {
                    path: f.path,
                    ..Changed::default()
                });
                out.len() - 1
            }
        };
        let c = &mut out[at];
        c.calls += 1;
        match f.lines {
            Some(l) => c.lines.add(l),
            None => c.uncounted += 1,
        }
    }
}

/// What a session did to a pull request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PullDone {
    Opened,
    Merged,
}

impl PullDone {
    pub fn as_str(self) -> &'static str {
        match self {
            PullDone::Opened => "opened",
            PullDone::Merged => "merged",
        }
    }

    pub(crate) fn parse(s: &str) -> Option<PullDone> {
        match s {
            "opened" => Some(PullDone::Opened),
            "merged" => Some(PullDone::Merged),
            _ => None,
        }
    }
}

/// A merge a call asked for, before its result is read.
#[derive(Debug, Clone)]
enum Target {
    Url(String),
    /// A number in a repository named by `owner/name`, or only by name.
    Number {
        github: Option<String>,
        name: Option<String>,
        n: u64,
    },
}

impl Target {
    fn number(&self) -> Option<u64> {
        match self {
            Target::Url(u) => url_parts(u).map(|p| p.2),
            Target::Number { n, .. } => Some(*n),
        }
    }
}

/// Whether a merge call's `result` shows a merge of pull request `n`: when
/// it shows one, yes; else, unless `strict`, when it shows no failure (gh
/// prints nothing on success when its output is piped away, and a call's
/// empty output is not indexed).
fn merged(result: Option<&str>, n: Option<u64>, strict: bool, last: bool) -> bool {
    let text = result.unwrap_or_default();
    // `Exit code` is the whole command's: it speaks for the merge only when
    // the merge ran last.
    let exit_failed = last && text.trim_start().starts_with("Exit code");
    merge_shown(text, n) || (!strict && !merge_failed(text) && !exit_failed)
}

/// One call's command, if it runs a shell command: Claude Code's `Bash`
/// `command`, or a Codex `shell`/`exec_command` `command` or `cmd`.
pub(crate) fn command_of(name: &str, input: &str) -> Option<String> {
    if !matches!(
        name,
        "Bash" | "shell" | "exec_command" | "local_shell" | "container.exec"
    ) {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(input).ok()?;
    let c = v.get("command").or_else(|| v.get("cmd"))?;
    match c {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Array(a) => {
            let words: Vec<&str> = a.iter().filter_map(|w| w.as_str()).collect();
            // `["bash", "-lc", "<script>"]` runs its script.
            match words.as_slice() {
                [_, flag, script] if flag.ends_with('c') && flag.starts_with('-') => {
                    Some((*script).to_owned())
                }
                _ => Some(words.join(" ")),
            }
        }
        _ => None,
    }
}

/// The merge failed or only queued: the result says so.
fn merge_failed(result: &str) -> bool {
    let lower = result.to_lowercase();
    lower.contains("will be automatically merged")
        || lower.contains("auto-merge")
        || lower.lines().any(|l| {
            (l.contains("merge") || l.contains("pull request") || l.contains("graphql"))
                && [
                    "not mergeable",
                    "failed",
                    "error",
                    "could not",
                    "cannot",
                    "already merged",
                    "graphql:",
                    "conflict",
                    "denied",
                    "not allowed",
                    "prohibit",
                ]
                .iter()
                .any(|bad| l.contains(bad))
        })
}

/// The result shows a merge: gh's `Merged pull request …`, a state of
/// `MERGED`, or a squash commit naming the number.
fn merge_shown(result: &str, n: Option<u64>) -> bool {
    let lower = result.to_lowercase();
    lower.contains("merged pull request")
        || result
            .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            .any(|w| w == "MERGED")
        || n.is_some_and(|n| result.contains(&format!("(#{n})")))
}

/// The pull request URLs a `gh pr create` printed: lines that are a URL
/// alone. Nothing when it says the pull request already exists.
fn created_urls(result: &str) -> Vec<String> {
    if result.contains("already exists") {
        return Vec::new();
    }
    result
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("https://github.com/"))
        .flat_map(pull_urls)
        .take(1)
        .collect()
}

/// `owner/name` and number of a pull request URL.
fn url_parts(url: &str) -> Option<(String, String, u64)> {
    let rest = url.strip_prefix("https://github.com/")?;
    let mut parts = rest.split('/');
    let owner = parts.next()?.to_owned();
    let name = parts.next()?.to_owned();
    parts.next()?;
    let n = parts.next()?.parse().ok()?;
    Some((owner, name, n))
}

/// A session's calls in order, each with its result: indexes into `turns`.
pub fn pairs_of(turns: &[StoredTurn]) -> Vec<(usize, Option<usize>)> {
    // A call's result is the next tool turn of the same tool that is not itself a call, taken in call order,
    // before the next words of the person or the assistant (a call's
    // results come back before the assistant speaks again; a call whose
    // output was empty has none in the index).
    let mut pairs: Vec<(usize, Option<usize>)> = Vec::new();
    let mut waiting: Vec<usize> = Vec::new(); // indexes into `pairs`
    for (i, t) in turns.iter().enumerate() {
        if t.role != "tool" {
            waiting.clear();
        } else if is_call(t) {
            waiting.push(pairs.len());
            pairs.push((i, None));
        } else if t.role == "tool" {
            let name = t.tool_name.as_deref();
            if let Some(w) = waiting
                .iter()
                .position(|&p| turns[pairs[p].0].tool_name.as_deref() == name)
            {
                pairs[waiting.remove(w)].1 = Some(i);
            }
        }
    }

    pairs
}

/// Reads what a session's calls did. `cwd` is where it started; `repos`
/// looks repositories up on disk and may be shared across sessions.
pub fn read(turns: &[StoredTurn], cwd: Option<&Path>, repos: &mut Repos) -> Work {
    let home = repos.home.clone();
    let pairs = pairs_of(turns);

    let mut weights: Vec<(String, u64)> = Vec::new();
    let add = |name: String, weights: &mut Vec<(String, u64)>| match weights
        .iter_mut()
        .find(|(n, _)| *n == name)
    {
        Some(w) => w.1 += 1,
        None => weights.push((name, 1)),
    };
    // Worktrees the session made, by path: their repository.
    let mut made: Vec<(PathBuf, Repo)> = Vec::new();
    let repo_of = |path: &Path, repos: &mut Repos, made: &[(PathBuf, Repo)]| -> Option<Repo> {
        made.iter()
            .filter(|(p, _)| path.starts_with(p))
            .max_by_key(|(p, _)| p.as_os_str().len())
            .map(|(_, r)| r.clone())
            .or_else(|| repos.of(path))
    };
    // The working directory's name, when it is no repository's.
    let cwd_name = cwd
        .filter(|c| repos.of(c).is_none())
        .and_then(|c| c.file_name())
        .map(|n| n.to_string_lossy().into_owned());
    let mut opened: Vec<(String, String)> = Vec::new();
    let mut pull_titles: Vec<(String, String)> = Vec::new();
    let mut merges: Vec<(Target, String)> = Vec::new();
    let mut dir: Option<PathBuf> = cwd.map(Path::to_owned);
    let mut changed: Vec<Changed> = Vec::new();
    let mut branch: Option<String> = None;

    for &(call, result) in &pairs {
        let t = &turns[call];
        let name = t.tool_name.as_deref().unwrap_or_default();
        let input = &t.text[name.len() + 1..];
        let result = result.map(|r| turns[r].text.as_str());
        if is_edit_tool(name) {
            note_changes(&mut changed, changes::of_call(name, input, result));
            for p in super::edited_paths(name, input) {
                let Some(path) = resolve(dir.as_deref(), &p, home.as_deref()) else {
                    continue;
                };
                match repo_of(&path, repos, &made) {
                    Some(r) => add(r.name, &mut weights),
                    // A file under a working directory that is no
                    // repository counts for that directory.
                    None => {
                        if let Some(name) = cwd_name
                            .as_ref()
                            .filter(|_| cwd.is_some_and(|c| path.starts_with(c)))
                        {
                            add(name.clone(), &mut weights);
                        }
                    }
                }
            }
            continue;
        }
        let lower = name.to_lowercase();
        if lower.contains("create_pull_request") {
            for url in result
                .map(pull_urls)
                .unwrap_or_default()
                .into_iter()
                .take(1)
            {
                let v: serde_json::Value = serde_json::from_str(input).unwrap_or_default();
                if let Some(title) = v.get("title").and_then(|x| x.as_str()) {
                    pull_titles.push((url.clone(), pull_title(title)));
                }
                opened.push((url, t.ts.clone()));
            }
            continue;
        }
        if lower.contains("merge_pull_request") {
            let v: serde_json::Value = serde_json::from_str(input).unwrap_or_default();
            let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_owned);
            let n = ["pullNumber", "pull_number", "number"]
                .iter()
                .find_map(|k| v.get(*k).and_then(serde_json::Value::as_u64));
            if let (Some(owner), Some(repo), Some(n)) = (s("owner"), s("repo"), n) {
                let ok = result.is_some_and(|r| {
                    let l = r.to_lowercase().replace(' ', "");
                    l.contains("\"merged\":true") || l.contains("successfullymerged")
                });
                if ok {
                    merges.push((
                        Target::Url(format!("https://github.com/{owner}/{repo}/pull/{n}")),
                        t.ts.clone(),
                    ));
                }
            }
            continue;
        }
        let Some(command) = command_of(name, input) else {
            continue;
        };
        if changes::has_patch(&command) {
            note_changes(&mut changed, changes::patch_lines(&command));
        }
        let mut here = dir.clone();
        let commands = simple_commands(&command);
        let count = commands.len();
        for (k, words) in commands.iter().enumerate() {
            let last = k + 1 == count;
            let words = program(words);
            let Some(first) = words.first().map(String::as_str) else {
                continue;
            };
            match first {
                "cd" | "pushd" => {
                    if let Some(to) = words.get(1).filter(|w| !w.starts_with('-')) {
                        here = resolve(here.as_deref(), to, home.as_deref());
                    }
                }
                "git" => {
                    if let Some(b) = changes::branch_of_git(words) {
                        branch = Some(b);
                    }
                    let at = match flag_value(&words[1..], &["-C"]) {
                        Some(c) => resolve(here.as_deref(), &c, home.as_deref()),
                        None => here.clone(),
                    };
                    let Some(at) = at else { continue };
                    let repo = repo_of(&at, repos, &made);
                    if let Some(r) = &repo {
                        add(r.name.clone(), &mut weights);
                    }
                    let args = positionals(&words[1..], &["-C", "-c", "-b", "-B", "--reason"]);
                    if args.first() == Some(&"worktree") && args.get(1) == Some(&"add") {
                        if let (Some(path), Some(r)) = (args.get(2), repo) {
                            if let Some(p) = resolve(Some(&at), path, home.as_deref()) {
                                made.push((p, r));
                            }
                        }
                    }
                }
                "gh" => {
                    if let Some(b) = changes::branch_of_gh(words) {
                        branch = Some(b);
                    }
                    let flagged = flag_value(&words[1..], &["-R", "--repo"]);
                    let repo = here.as_deref().and_then(|h| repo_of(h, repos, &made));
                    match (&flagged, &repo) {
                        (Some(f), _) => {
                            add(f.rsplit('/').next().unwrap_or(f).to_owned(), &mut weights)
                        }
                        (None, Some(r)) => add(r.name.clone(), &mut weights),
                        _ => {}
                    }
                    let args = positionals(&words[1..], GH_VALUE_FLAGS);
                    let github = flagged
                        .clone()
                        .or_else(|| repo.as_ref().and_then(|r| r.github.clone()));
                    let repo_name = flagged
                        .as_deref()
                        .map(|f| f.rsplit('/').next().unwrap_or(f).to_owned())
                        .or_else(|| repo.as_ref().map(|r| r.name.clone()));
                    match args.as_slice() {
                        ["pr", "create", ..] => {
                            let title = flag_value(&words[1..], &["--title", "-t"]);
                            for url in result.map(created_urls).unwrap_or_default() {
                                if let Some(title) = &title {
                                    pull_titles.push((url.clone(), pull_title(title)));
                                }
                                opened.push((url, t.ts.clone()));
                            }
                        }
                        ["pr", "merge", target, ..] => {
                            let target = if target.starts_with("https://") {
                                pull_urls(target).into_iter().next().map(Target::Url)
                            } else {
                                target.trim_start_matches('#').parse().ok().map(|n| {
                                    Target::Number {
                                        github: github.clone(),
                                        name: repo_name.clone(),
                                        n,
                                    }
                                })
                            };
                            if let Some(target) = target {
                                // `--auto` only queues the merge: it counts
                                // when the result shows it done.
                                let strict = words.iter().any(|w| w == "--auto");
                                if merged(result, target.number(), strict, last) {
                                    merges.push((target, t.ts.clone()));
                                }
                            }
                        }
                        ["api", path, ..]
                            if path.contains("/pulls/") && path.ends_with("/merge") =>
                        {
                            // `repos/<owner>/<name>/pulls/<n>/merge`
                            let p: Vec<&str> = path.trim_start_matches('/').split('/').collect();
                            if let [_, owner, name, _, n, _] = p.as_slice() {
                                if let Ok(n) = n.parse::<u64>() {
                                    if merged(result, Some(n), false, last) {
                                        merges.push((
                                            Target::Url(format!(
                                                "https://github.com/{owner}/{name}/pull/{n}"
                                            )),
                                            t.ts.clone(),
                                        ));
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
                _ => {
                    // Another tool that merges a pull request by its URL,
                    // such as a project's merge script: counted only when
                    // its output shows the merge.
                    if words.iter().any(|w| w == "merge") {
                        for w in words.iter().skip(1) {
                            if let Some(url) = pull_urls(w).into_iter().next() {
                                let text = result.unwrap_or_default();
                                let says = text.to_lowercase().contains("merged");
                                if says && !merge_failed(text) {
                                    merges.push((Target::Url(url), t.ts.clone()));
                                }
                            }
                        }
                    }
                }
            }
        }
        dir = match result.and_then(|r| {
            r.lines()
                .rev()
                .find_map(|l| l.trim().strip_prefix("Shell cwd was reset to "))
        }) {
            Some(reset) => Some(PathBuf::from(reset.trim())),
            None => here,
        };
    }

    // Numbers become URLs by their repository: from the remote, else from
    // a URL the session saw for a repository of that name.
    let seen: Vec<String> = turns.iter().flat_map(|t| pull_urls(&t.text)).collect();
    let mut pulls: Vec<(String, PullDone, String)> = Vec::new();
    let push =
        |url: String, done: PullDone, ts: String, pulls: &mut Vec<(String, PullDone, String)>| {
            if !pulls.iter().any(|(u, d, _)| *u == url && *d == done) {
                pulls.push((url, done, ts));
            }
        };
    for (url, ts) in opened {
        if let Some((_, name, _)) = url_parts(&url) {
            add(name, &mut weights);
        }
        push(url, PullDone::Opened, ts, &mut pulls);
    }
    for (target, ts) in merges {
        let url = match target {
            Target::Url(u) => Some(u),
            Target::Number { github, name, n } => github
                .map(|g| format!("https://github.com/{g}/pull/{n}"))
                .or_else(|| {
                    let name = name?;
                    seen.iter()
                        .filter_map(|u| url_parts(u))
                        .find(|(_, nm, _)| *nm == name)
                        .map(|(owner, nm, _)| format!("https://github.com/{owner}/{nm}/pull/{n}"))
                }),
        };
        if let Some(url) = url {
            push(url, PullDone::Merged, ts, &mut pulls);
        }
    }

    let project = weights
        .iter()
        .enumerate()
        .max_by(|(i, a), (j, b)| a.1.cmp(&b.1).then(j.cmp(i)))
        .map(|(_, (n, w))| (n.clone(), *w))
        .or_else(|| {
            // The home folder is never a project.
            let cwd = cwd.filter(|c| repos.home.as_deref() != Some(*c))?;
            let name = repos
                .of(cwd)
                .map(|r| r.name)
                .or_else(|| cwd.file_name().map(|n| n.to_string_lossy().into_owned()))?;
            Some((name, 0))
        });
    Work {
        project,
        pulls,
        pull_titles,
        changes: changed,
        branch,
        pairs,
    }
}

#[cfg(test)]
mod tests;
