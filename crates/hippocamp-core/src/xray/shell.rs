//! Shell calls as the x-ray reads them: calls paired with their results,
//! scripts split into simple commands, wrappers taken off, and the shell's
//! directory followed from call to call.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use crate::dream::observed::{call, split_commands};
use crate::index::StoredTurn;
use crate::rollup::is_call;
use crate::rollup::work::command_of;

/// Each call's index in `turns` with its result's: the next tool turn of
/// the same tool that is not a call, before the next words of the person
/// or the assistant (the rollup's rule).
pub(crate) fn pairs(turns: &[StoredTurn]) -> Vec<(usize, Option<usize>)> {
    let mut pairs: Vec<(usize, Option<usize>)> = Vec::new();
    let mut waiting: Vec<usize> = Vec::new();
    for (i, t) in turns.iter().enumerate() {
        if t.role != "tool" {
            waiting.clear();
        } else if is_call(t) {
            waiting.push(pairs.len());
            pairs.push((i, None));
        } else {
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

/// A call's tool name and parsed input.
pub(crate) fn input(turn: &StoredTurn) -> Option<(&str, Value)> {
    let (name, raw) = call(turn)?;
    let v = serde_json::from_str(raw).ok()?;
    Some((name, v))
}

/// The script a shell call runs.
pub(crate) fn script(turn: &StoredTurn) -> Option<String> {
    let (name, raw) = call(turn)?;
    command_of(name, raw)
}

/// One simple command: its words, and the words that are `>`/`>>`
/// targets.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Simple {
    pub words: Vec<String>,
    pub written: Vec<String>,
}

impl Simple {
    /// The program's base name and its arguments, past environment
    /// assignments and wrappers (`env`, `time`, `npx`, `timeout 60`,
    /// `uv run`, `python -m`, a `+toolchain`).
    pub fn program(&self) -> Option<(String, Vec<&str>)> {
        let mut w: Vec<&str> = self.words.iter().map(String::as_str).collect();
        loop {
            while w.first().is_some_and(|x| is_assignment(x)) {
                w.remove(0);
            }
            let first = *w.first()?;
            let base = base(first);
            match base.as_str() {
                "env" | "time" | "nice" | "nohup" | "npx" | "bunx" | "caffeinate" | "sudo"
                | "command" | "exec" => {
                    w.remove(0);
                    while let Some(x) = w.first().copied() {
                        if !x.starts_with('-') {
                            break;
                        }
                        w.remove(0);
                        // `nice -n 10`
                        if x == "-n" && base == "nice" && !w.is_empty() {
                            w.remove(0);
                        }
                    }
                }
                "timeout" | "gtimeout" => {
                    w.remove(0);
                    while w.first().is_some_and(|x| x.starts_with('-')) {
                        w.remove(0);
                    }
                    if !w.is_empty() {
                        w.remove(0);
                    }
                }
                "xcrun" => {
                    w.remove(0);
                    while let Some(x) = w.first().copied() {
                        if !x.starts_with('-') {
                            break;
                        }
                        w.remove(0);
                        if matches!(x, "--sdk" | "-sdk" | "--toolchain" | "-toolchain")
                            && !w.is_empty()
                        {
                            w.remove(0);
                        }
                    }
                }
                // `lockf [-k] [-t secs] <lock file> <command>`, `flock`.
                "lockf" | "flock" => {
                    w.remove(0);
                    while let Some(x) = w.first().copied() {
                        if !x.starts_with('-') {
                            break;
                        }
                        w.remove(0);
                        if matches!(x, "-t" | "-w" | "--timeout" | "-E" | "--conflict-exit-code")
                            && !w.is_empty()
                        {
                            w.remove(0);
                        }
                    }
                    if !w.is_empty() {
                        w.remove(0);
                    }
                }
                "uv" | "poetry" | "pipenv" if w.get(1) == Some(&"run") => {
                    w.drain(..2);
                }
                "python" | "python3" if w.get(1) == Some(&"-m") && w.len() > 2 => {
                    w.drain(..2);
                }
                _ => break,
            }
        }
        let prog = base(w.first()?);
        let args: Vec<&str> = w[1..]
            .iter()
            .copied()
            .filter(|a| !(a.starts_with('+') && prog == "cargo"))
            .collect();
        Some((prog, args))
    }
}

fn base(word: &str) -> String {
    Path::new(word)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| word.to_owned())
}

fn is_assignment(word: &str) -> bool {
    match word.split_once('=') {
        Some((name, _)) => {
            !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
        None => false,
    }
}

/// A script's heredoc bodies taken out (`<<EOF` … `EOF`, `<<'EOF'`,
/// `<<-EOF`), each with the command line that fed it: a body is a program's
/// input, not shell commands.
pub(crate) fn heredocs(script: &str) -> (String, Vec<String>) {
    let mut out = String::with_capacity(script.len());
    let mut bodies = Vec::new();
    let mut lines = script.lines();
    while let Some(line) = lines.next() {
        out.push_str(line);
        out.push('\n');
        let Some(delim) = heredoc_delimiter(line) else {
            continue;
        };
        let mut body = String::new();
        for l in lines.by_ref() {
            if l.trim() == delim {
                break;
            }
            body.push_str(l);
            body.push('\n');
        }
        bodies.push(body);
    }
    (out, bodies)
}

/// Each heredoc of a script with the file it is saved to, when the line
/// that opens it writes it with `cat > <file>` or `tee <file>` (a script
/// kept to run later), or `None` when a program reads it (`python3 -
/// <<EOF`).
pub(crate) fn heredoc_targets(script: &str) -> Vec<(Option<String>, String)> {
    static OPENER: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"<<-?\s*['"]?\w+['"]?"#).expect("valid regex"));
    let mut out = Vec::new();
    let mut lines = script.lines();
    while let Some(line) = lines.next() {
        let Some(delim) = heredoc_delimiter(line) else {
            continue;
        };
        let mut body = String::new();
        for l in lines.by_ref() {
            if l.trim() == delim {
                break;
            }
            body.push_str(l);
            body.push('\n');
        }
        let opener = OPENER.replace(line, " ");
        let target = plain_commands(&opener).into_iter().find_map(|c| {
            match c.words.first().map(|w| base(w)).as_deref() {
                Some("cat") => c.written.first().cloned(),
                Some("tee") => c.words[1..].iter().find(|w| !w.starts_with('-')).cloned(),
                _ => None,
            }
        });
        out.push((target, body));
    }
    out
}

/// The delimiter of the heredoc a line opens, if it opens one.
fn heredoc_delimiter(line: &str) -> Option<String> {
    let i = line.find("<<")?;
    let rest = &line[i + 2..];
    if rest.starts_with('<') {
        return None; // a here-string, `<<<`
    }
    let rest = rest.trim_start_matches('-').trim_start();
    let word: String = rest
        .chars()
        .take_while(|c| !c.is_whitespace() && !matches!(c, ';' | '|' | '&' | ')'))
        .collect();
    let word = word.trim_matches(|c| c == '\'' || c == '"').to_owned();
    (!word.is_empty() && word.chars().all(|c| c.is_alphanumeric() || c == '_')).then_some(word)
}

/// A script's simple commands, heredoc bodies left out, and the script of
/// a `bash -c '…'` (`sh`, `zsh`, after any wrapper) read in its place.
pub(crate) fn simple_commands(script: &str) -> Vec<Simple> {
    simple_commands_at(script, 0)
}

fn simple_commands_at(script: &str, depth: u8) -> Vec<Simple> {
    let (script, _) = heredocs(script);
    let mut out = Vec::new();
    for s in plain_commands(&script) {
        let inner = s.words.windows(3).find_map(|w| {
            (matches!(base(&w[0]).as_str(), "bash" | "sh" | "zsh") && w[1] == "-c")
                .then(|| w[2].clone())
        });
        match inner {
            Some(inner) if depth < 3 => out.extend(simple_commands_at(&inner, depth + 1)),
            _ => out.push(s),
        }
    }
    out
}

fn plain_commands(script: &str) -> Vec<Simple> {
    split_commands(script)
        .into_iter()
        .map(|words| {
            let mut s = Simple::default();
            for w in words {
                if w.written {
                    s.written.push(w.text);
                } else {
                    s.words.push(w.text);
                }
            }
            s
        })
        .filter(|s| !s.words.is_empty())
        .collect()
}

/// The shell's directory, followed across calls: the session's, moved by
/// a call whose script starts `cd <absolute dir>`, reset by a result that
/// says `Shell cwd was reset to <dir>`.
#[derive(Debug, Clone)]
pub(crate) struct Cwd {
    pub dir: Option<PathBuf>,
}

impl Cwd {
    /// The simple commands of `script`, each with the directory it runs
    /// in (a `cd <dir>` earlier in the script moves it), and moves the
    /// shell's directory to where the script left it, or to where the
    /// result says the client reset it.
    pub fn step(&mut self, script: &str, result: Option<&str>) -> Vec<(Simple, Option<PathBuf>)> {
        let mut dir = self.dir.clone();
        let mut out = Vec::new();
        for c in simple_commands(script) {
            if c.words.first().map(String::as_str) == Some("cd") {
                dir = match c.words.get(1).map(String::as_str) {
                    Some(d) if Path::new(d).is_absolute() => Some(lexical(Path::new(d))),
                    Some(d) if !d.starts_with('~') && !d.starts_with('$') && d != "-" => {
                        dir.as_ref().map(|base| lexical(&base.join(d)))
                    }
                    _ => None,
                };
            }
            out.push((c, dir.clone()));
        }
        self.dir = dir;
        if let Some(r) = result {
            if let Some(pos) = r.rfind("Shell cwd was reset to ") {
                let rest = &r[pos + "Shell cwd was reset to ".len()..];
                let d = rest.lines().next().unwrap_or("").trim();
                if !d.is_empty() {
                    self.dir = Some(PathBuf::from(d));
                }
            }
        }
        out
    }
}

/// `path` with `.` and `..` resolved by text.
pub(crate) fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// A result's text without the client's own trailing notes.
pub(crate) fn output_lines(result: &str) -> Vec<&str> {
    result
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty())
        .filter(|l| !l.starts_with("Shell cwd was reset to "))
        .collect()
}

/// The exit code a result starts with (`Exit code 2`), when it does.
pub(crate) fn exit_code(result: &str) -> Option<i64> {
    let t = result.trim_start();
    let rest = t
        .strip_prefix("Exit code: ")
        .or_else(|| t.strip_prefix("Exit code "))?;
    let n: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    n.parse().ok()
}
