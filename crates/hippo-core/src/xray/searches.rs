//! What a session searched the notes for: `hippo recall`
//! and `hippo ask`, `Grep` and `Glob` in the notes folder, and shell
//! searches with a path in it; their hits, which hit files it then read,
//! and what followed a search that found nothing.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use super::shell::{self, exit_code, output_lines, Cwd};
use crate::index::StoredTurn;
use crate::lessons::detect::key_terms;
use crate::session_meta::{classify, TurnKind};

/// How far after a miss the x-ray looks for what followed.
const FOLLOW_TURNS: u32 = 60;

/// The longest query kept, in characters.
const QUERY_CHARS: usize = 80;

/// One search.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Search {
    pub ordinal: u32,
    /// `recall`, `ask`, `Grep`, `Glob`, `grep`, `rg`, `find`, …
    pub tool: String,
    pub query: String,
    /// `None` when the result does not say: an error, or a call whose
    /// output mixes other commands' with the search's.
    pub hits: Option<u64>,
    /// Hit files the session then read; `None` when the result lists no
    /// files (a count-only, mixed or failed output).
    pub used: Option<u32>,
    /// After a miss: `then asked you`, `found it in <file> after n turns`,
    /// `found by a later search after n turns`.
    pub follow: Option<String>,
    /// The turn `follow` points at.
    pub follow_ordinal: Option<u32>,
}

impl Search {
    pub fn is_miss(&self) -> bool {
        self.hits == Some(0)
    }
}

/// A search call before its scope is checked.
struct Found {
    tool: String,
    query: String,
    /// Paths it searched, absolute where known.
    paths: Vec<PathBuf>,
    /// Searches anywhere count (hippo's own search).
    anywhere: bool,
}

/// The searches in a session's own turns. `notes_roots` are the notes
/// folder's absolute paths; `dir` is the session's directory.
pub fn find(turns: &[StoredTurn], notes_roots: &[PathBuf], dir: Option<&Path>) -> Vec<Search> {
    let pairs = shell::pairs(turns);
    let mut cwd = Cwd {
        dir: dir.map(Path::to_path_buf),
    };
    // Every search, in the notes or not: a later one may answer a miss.
    let mut all: Vec<(usize, Search, bool)> = Vec::new();
    // The directory each search ran in, for its relative hit paths, and
    // the files it searched when they are files (a one-file grep prints no
    // file names).
    let mut dirs: std::collections::HashMap<usize, PathBuf> = Default::default();
    let mut searched_files: std::collections::HashMap<usize, Vec<String>> = Default::default();
    for (ci, ri) in &pairs {
        let t = &turns[*ci];
        let result = ri.map(|r| turns[r].text.as_str());
        let later = *ci + 1 < turns.len();
        let mut separable = true;
        let (found, run_dir) = if let Some(script) = shell::script(t) {
            let commands = cwd.step(&script, result);
            separable = one_producer(&commands);
            commands
                .into_iter()
                .find_map(|(c, d)| shell_search(&c).map(|f| (Some(f), d)))
                .unwrap_or((None, None))
        } else {
            (tool_search(t), cwd.dir.clone())
        };
        let Some(found) = found else { continue };
        let in_notes = found.anywhere
            || found.paths.iter().any(|p| {
                let p = if p.is_absolute() {
                    p.clone()
                } else {
                    match &run_dir {
                        Some(d) => shell::lexical(&d.join(p)),
                        None => return false,
                    }
                };
                notes_roots.iter().any(|r| p.starts_with(r))
            });
        // A call whose output mixes other commands' output with the
        // search's has no hit count of its own.
        let hits = if separable {
            hits(&found.tool, result, later)
        } else {
            None
        };
        if let Some(d) = &run_dir {
            dirs.insert(*ci, d.clone());
        }
        let files: Vec<String> = found
            .paths
            .iter()
            .filter(|p| p.extension().is_some())
            .map(|p| {
                let p = match (&run_dir, p.is_absolute()) {
                    (Some(d), false) => shell::lexical(&d.join(p)),
                    _ => p.clone(),
                };
                p.to_string_lossy().into_owned()
            })
            .collect();
        if !files.is_empty() && files.len() == found.paths.len() {
            searched_files.insert(*ci, files);
        }
        all.push((
            *ci,
            Search {
                ordinal: t.ordinal,
                tool: found.tool,
                query: cut(&found.query, QUERY_CHARS),
                hits,
                used: None,
                follow: None,
                follow_ordinal: None,
            },
            in_notes,
        ));
    }
    // Used: hit files later read, with `Read` or named in a shell command.
    let mut reads: Vec<(usize, String)> = Vec::new();
    for (i, t) in turns.iter().enumerate() {
        if let Some((name, v)) = shell::input(t) {
            if name == "Read" {
                if let Some(p) = v.get("file_path").and_then(Value::as_str) {
                    reads.push((i, p.to_owned()));
                }
                continue;
            }
        }
        if let Some(script) = shell::script(t) {
            for c in shell::simple_commands(&script) {
                for w in c
                    .words
                    .iter()
                    .skip(1)
                    .filter(|w| !w.starts_with('-') && w.contains('/'))
                {
                    reads.push((i, w.clone()));
                }
            }
        }
    }
    let result_of = |ci: usize| -> Option<&str> {
        pairs
            .iter()
            .find(|(c, _)| *c == ci)
            .and_then(|(_, r)| r.map(|r| turns[r].text.as_str()))
    };
    let mut out = Vec::new();
    for k in 0..all.len() {
        if !all[k].2 {
            continue;
        }
        let ci = all[k].0;
        let mut s = all[k].1.clone();
        if let (Some(result), Some(_)) = (result_of(ci), s.hits) {
            let mut files = hit_files(result, dirs.get(&ci).map(PathBuf::as_path));
            if files.is_empty() && s.hits.unwrap_or(0) > 0 {
                files = searched_files.get(&ci).cloned().unwrap_or_default();
            }
            if !files.is_empty() {
                s.used = Some(
                    files
                        .iter()
                        .filter(|f| reads.iter().any(|(i, p)| *i > ci && same_file(p, f)))
                        .count() as u32,
                );
            }
        }
        if s.is_miss() {
            follow(&mut s, ci, turns, &all, &reads);
        }
        out.push(s);
    }
    out
}

/// What followed a miss at `ci`.
fn follow(
    s: &mut Search,
    ci: usize,
    turns: &[StoredTurn],
    all: &[(usize, Search, bool)],
    reads: &[(usize, String)],
) {
    let terms = key_terms(
        &s.query
            .replace(['*', '.', '|', '\\', '(', ')', '[', ']'], " "),
    );
    if terms.is_empty() {
        return;
    }
    let limit = s.ordinal + FOLLOW_TURNS;
    let mut best: Option<(u32, String)> = None;
    let mut consider = |ord: u32, text: String| {
        if ord <= limit && best.as_ref().is_none_or(|(b, _)| ord < *b) {
            best = Some((ord, text));
        }
    };
    for (i, later, _) in all {
        if *i <= ci || later.hits.unwrap_or(0) == 0 {
            continue;
        }
        let theirs = key_terms(
            &later
                .query
                .replace(['*', '.', '|', '\\', '(', ')', '[', ']'], " "),
        );
        if theirs.iter().any(|t| terms.contains(t)) {
            let n = later.ordinal - s.ordinal;
            consider(
                later.ordinal,
                format!("found by a later search after {n} turns"),
            );
            break;
        }
    }
    for (i, path) in reads {
        if *i <= ci {
            continue;
        }
        let name = Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if terms.iter().any(|t| name.contains(t.as_str())) {
            let ord = turns[*i].ordinal;
            let shown = Path::new(path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            consider(
                ord,
                format!("found it in {shown} after {} turns", ord - s.ordinal),
            );
            break;
        }
    }
    // The next assistant words end in a question, answered by the person.
    let mut iter = turns[ci + 1..].iter();
    if let Some(reply) = iter
        .by_ref()
        .find(|t| t.role == "assistant" || t.role == "user")
    {
        if reply.role == "assistant" && reply.text.trim_end().ends_with('?') {
            let next = iter.find(|t| {
                !matches!(
                    classify(&t.role, t.tool_name.as_deref(), &t.text),
                    TurnKind::Injection(_)
                )
            });
            if let Some(n) = next {
                if classify(&n.role, n.tool_name.as_deref(), &n.text) == TurnKind::Request {
                    consider(n.ordinal, "then asked you".to_owned());
                }
            }
        }
    }
    if let Some((ord, text)) = best {
        s.follow = Some(text);
        s.follow_ordinal = Some(ord);
    }
}

/// Whether the search is the only command of a script whose output reaches
/// the result: the others only change directory, set variables, or filter
/// a pipe.
fn one_producer(commands: &[(shell::Simple, Option<PathBuf>)]) -> bool {
    let mut producers = 0;
    for (c, _) in commands {
        let Some((prog, args)) = c.program() else {
            continue;
        };
        let quiet =
            matches!(
                prog.as_str(),
                "cd" | "export"
                    | "set"
                    | "source"
                    | "."
                    | "true"
                    | ":"
                    | "head"
                    | "tail"
                    | "sort"
                    | "uniq"
                    | "cut"
                    | "wc"
                    | "tr"
                    | "column"
                    | "awk"
                    | "nl"
                    | "fold"
            ) || (prog == "sed" && !args.iter().any(|a| a.starts_with("-i")) && args.len() <= 2)
                || (prog == "grep" && producers > 0 && args.iter().all(|a| !a.contains('/')));
        if !quiet {
            producers += 1;
        }
    }
    producers <= 1
}

/// A `Grep` or `Glob` call, or the dsh or OpenCode plugin's `hippo_ask`.
fn tool_search(t: &StoredTurn) -> Option<Found> {
    let (name, v) = shell::input(t)?;
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
    match name {
        "hippo_ask" => Some(Found {
            tool: "ask".to_owned(),
            query: s("question")?,
            paths: Vec::new(),
            anywhere: true,
        }),
        "Grep" | "Glob" => {
            let pattern = s("pattern")?;
            let mut paths: Vec<PathBuf> = s("path").map(PathBuf::from).into_iter().collect();
            if name == "Glob" && paths.is_empty() && pattern.starts_with('/') {
                paths.push(PathBuf::from(&pattern));
            }
            Some(Found {
                tool: name.to_owned(),
                query: pattern,
                paths,
                anywhere: false,
            })
        }
        _ => None,
    }
}

/// A shell search: hippo's, or a grep/rg/ag/find/fd with path arguments.
fn shell_search(c: &shell::Simple) -> Option<Found> {
    let (prog, args) = c.program()?;
    {
        match prog.as_str() {
            "hippo" if matches!(args.first(), Some(&"recall") | Some(&"ask")) => {
                let query: Vec<&str> = args[1..]
                    .iter()
                    .copied()
                    .filter(|a| !a.starts_with('-'))
                    .collect();
                if query.is_empty() {
                    return None;
                }
                return Some(Found {
                    tool: args[0].to_owned(),
                    query: query.join(" "),
                    paths: Vec::new(),
                    anywhere: true,
                });
            }
            "grep" | "egrep" | "fgrep" | "rg" | "ag" => {
                let mut pattern: Option<String> = None;
                let mut paths = Vec::new();
                let mut i = 0;
                let value_flags = [
                    "-e",
                    "-f",
                    "-m",
                    "-A",
                    "-B",
                    "-C",
                    "-g",
                    "--glob",
                    "-t",
                    "--type",
                    "--include",
                    "--exclude",
                    "--max-count",
                    "-M",
                ];
                while i < args.len() {
                    let a = args[i];
                    if a == "-e" || a == "--regexp" {
                        pattern = args.get(i + 1).map(|s| (*s).to_owned());
                        i += 2;
                        continue;
                    }
                    if value_flags.contains(&a) {
                        i += 2;
                        continue;
                    }
                    if a.starts_with('-') {
                        i += 1;
                        continue;
                    }
                    if pattern.is_none() {
                        pattern = Some(a.to_owned());
                    } else {
                        paths.push(PathBuf::from(a));
                    }
                    i += 1;
                }
                if let Some(p) = pattern {
                    if !paths.is_empty() {
                        return Some(Found {
                            tool: prog,
                            query: p,
                            paths,
                            anywhere: false,
                        });
                    }
                }
            }
            "find" => {
                let paths: Vec<PathBuf> = args
                    .iter()
                    .take_while(|a| !a.starts_with('-'))
                    .map(PathBuf::from)
                    .collect();
                let name = args
                    .iter()
                    .position(|a| matches!(*a, "-name" | "-iname" | "-path" | "-ipath"))
                    .and_then(|i| args.get(i + 1));
                if let Some(n) = name {
                    if !paths.is_empty() {
                        return Some(Found {
                            tool: prog,
                            query: (*n).to_owned(),
                            paths,
                            anywhere: false,
                        });
                    }
                }
            }
            "fd" | "fdfind" => {
                let plain: Vec<&str> = args
                    .iter()
                    .copied()
                    .filter(|a| !a.starts_with('-'))
                    .collect();
                if plain.len() >= 2 {
                    return Some(Found {
                        tool: prog,
                        query: plain[0].to_owned(),
                        paths: plain[1..].iter().map(PathBuf::from).collect(),
                        anywhere: false,
                    });
                }
            }
            _ => {}
        }
    }
    None
}

/// A search result's hit count; with no result turn and a later turn, the
/// output was empty: 0.
fn hits(tool: &str, result: Option<&str>, later: bool) -> Option<u64> {
    let Some(r) = result else {
        return later.then_some(0);
    };
    let t = r.trim();
    if t.starts_with("<tool_use_error>") {
        return None;
    }
    if matches!(tool, "Grep" | "Glob") {
        if t.starts_with("No files found") || t.starts_with("No matches found") {
            return Some(0);
        }
        if let Some(rest) = t.strip_prefix("Found ") {
            let n: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(n) = n.parse() {
                return Some(n);
            }
        }
        let n = output_lines(t)
            .into_iter()
            .filter(|l| !l.starts_with("(Results are truncated"))
            .count();
        return Some(n as u64);
    }
    match exit_code(t) {
        Some(0) | None => {}
        Some(1) => {
            let rest: Vec<&str> = output_lines(t).into_iter().skip(1).collect();
            return rest.is_empty().then_some(0);
        }
        Some(_) => return None,
    }
    Some(output_lines(t).len() as u64)
}

/// Absolute file paths a search result lists (`/a/b.md`, `/a/b.md:12:…`).
/// The files a search's result lists: absolute paths, and relative ones
/// (`brain/x.md:12:…`, `./x.md`) resolved against `dir` when known.
/// A line counts only when its path part looks
/// like a file: it has an extension and no spaces.
fn hit_files(result: &str, dir: Option<&Path>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for l in result.lines() {
        let l = l.trim();
        let path = match l.find(':') {
            Some(i) => &l[..i],
            None => l,
        };
        if path.is_empty() || path.contains(' ') || Path::new(path).extension().is_none() {
            continue;
        }
        let path = if path.starts_with('/') {
            path.to_owned()
        } else {
            let rel = path.trim_start_matches("./");
            match dir {
                Some(d) => shell::lexical(&d.join(rel)).to_string_lossy().into_owned(),
                None => rel.to_owned(),
            }
        };
        if !out.contains(&path) {
            out.push(path);
        }
    }
    out
}

/// Whether a read path and a hit path name the same file: equal, or one
/// ends with the other at a `/`.
fn same_file(read: &str, hit: &str) -> bool {
    let read = read.trim_start_matches("./");
    read == hit || hit.ends_with(&format!("/{read}")) || read.ends_with(&format!("/{hit}"))
}

fn cut(text: &str, max: usize) -> String {
    let one: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= max {
        return one;
    }
    let mut s: String = one.chars().take(max - 1).collect();
    s.push('…');
    s
}
