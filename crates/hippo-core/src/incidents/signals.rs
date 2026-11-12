//! What one session holds that incidents are built from, read from its turns at refresh: the person's corrections, calls
//! a hook, the safety check or the person refused, destructive commands,
//! and the lines each edit call added and removed. No transcript is read
//! and no model is called.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use crate::index::{SessionTurn, StoredTurn};
use crate::lessons::detect::{self as lessons, correction_at, CORRECTION_LOOK_BACK};
use crate::rollup::changes::patch_lines;
use crate::rollup::work::{command_of, Repos};
use crate::rollup::{is_call, is_edit_tool};
use crate::util::{hex, sha256};
use crate::xray::shell::{self, Simple};

/// The longest text a signal keeps, in characters.
pub const SIGNAL_CHARS: usize = 300;

/// What kind of thing a signal saw.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SignalKind {
    /// The person corrected the agent (the lessons detector).
    Correction,
    /// A hook, the client's safety check or the person refused a call.
    Blocked,
    /// A destructive command ran.
    Destructive,
}

impl SignalKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SignalKind::Correction => "correction",
            SignalKind::Blocked => "blocked",
            SignalKind::Destructive => "destructive",
        }
    }

    pub fn parse(s: &str) -> Option<SignalKind> {
        match s {
            "correction" => Some(SignalKind::Correction),
            "blocked" => Some(SignalKind::Blocked),
            "destructive" => Some(SignalKind::Destructive),
            _ => None,
        }
    }
}

/// One signal in a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signal {
    pub kind: SignalKind,
    /// The turn that shows it: the person's words, the refusal, the
    /// destructive call.
    pub ordinal: u32,
    pub ts: String,
    /// The step it answers or is: the agent action, the refused call, the
    /// command.
    pub step_ordinal: Option<u32>,
    pub step_ts: Option<String>,
    pub step: String,
    /// The evidence in its own words: the person's, the hook's message, the
    /// command.
    pub text: String,
    /// A correction's key terms (space-separated); who refused a call
    /// (`hook:<name>`, `check`, `person`); a destructive command's pattern.
    pub extra: String,
}

/// One edit call's lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditLines {
    pub ordinal: u32,
    pub ts: String,
    /// The file as the call wrote it.
    pub path: String,
    /// `<repository>:<path inside it>` when the repository is found, else
    /// the path: two worktrees of one repository are one file.
    pub file: String,
    /// Hashes of the lines it added and removed, sorted.
    pub added: Vec<String>,
    pub removed: Vec<String>,
}

fn session_turn(id: &str, t: &StoredTurn, max: usize) -> SessionTurn {
    SessionTurn {
        session: id.to_owned(),
        ordinal: t.ordinal,
        role: t.role.clone(),
        ts: t.ts.clone(),
        tool_name: t.tool_name.clone(),
        text: t.text.chars().take(max).collect(),
    }
}

/// `text` on one line, at most `max` characters.
pub fn one_line(text: &str, max: usize) -> String {
    lessons::one_line(text, max)
}

/// The signals in a session's turns. `person` is whether a person started
/// it at the top level: only there are corrections read.
pub fn signals(id: &str, turns: &[StoredTurn], person: bool) -> Vec<Signal> {
    let mut out = Vec::new();
    let by_ordinal: HashMap<u32, &StoredTurn> = turns.iter().map(|t| (t.ordinal, t)).collect();
    if person {
        for (i, t) in turns.iter().enumerate() {
            if t.role != "user"
                || t.text.chars().count() > lessons::MAX_PERSON_CHARS * 4
                || !lessons::person_text(&t.text).is_some_and(|p| lessons::is_correction(&p))
            {
                continue;
            }
            let back = CORRECTION_LOOK_BACK as usize;
            let before: Vec<SessionTurn> = turns[i.saturating_sub(back)..i]
                .iter()
                .rev()
                .map(|b| session_turn(id, b, 2000))
                .collect();
            let turn = session_turn(id, t, usize::MAX);
            let Some(c) = correction_at(&turn, &before) else {
                continue;
            };
            let step = c.action_ordinal.and_then(|o| by_ordinal.get(&o));
            out.push(Signal {
                kind: SignalKind::Correction,
                ordinal: t.ordinal,
                ts: t.ts.clone(),
                step_ordinal: step.map(|s| s.ordinal),
                step_ts: step.map(|s| s.ts.clone()),
                step: c.action,
                text: c.quote,
                extra: c.terms.join(" "),
            });
        }
    }
    let pairs = shell::pairs(turns);
    let call_of: HashMap<usize, usize> = pairs
        .iter()
        .filter_map(|&(c, r)| r.map(|r| (r, c)))
        .collect();
    for (i, t) in turns.iter().enumerate() {
        if t.role != "tool" {
            continue;
        }
        if is_call(t) {
            if let Some(s) = destructive(t) {
                out.push(s);
            }
            continue;
        }
        let Some(by) = refused_by(&t.text) else {
            continue;
        };
        let call = call_of.get(&i).map(|&c| &turns[c]);
        out.push(Signal {
            kind: SignalKind::Blocked,
            ordinal: t.ordinal,
            ts: t.ts.clone(),
            step_ordinal: call.map(|c| c.ordinal),
            step_ts: call.map(|c| c.ts.clone()),
            step: call.map_or_else(String::new, |c| {
                lessons::call_line(&session_turn(id, c, 2000))
            }),
            text: one_line(refusal_message(&t.text), SIGNAL_CHARS),
            extra: by,
        });
    }
    out
}

// ---------------------------------------------------------------------------
// Refusals

static HOOK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^PreToolUse:\S+ hook error: \[([^\]]+)\]").expect("valid regex"));

/// Who refused the call whose result is `text`: `hook:<name>`, `check`
/// (the client's removal safety check) or `person`; `None` when nothing
/// did. The client's pacing rule (`Blocked: sleep N followed by …`) is not
/// a refusal of harm.
pub fn refused_by(text: &str) -> Option<String> {
    let t = text.trim_start();
    let t = t.strip_prefix("<tool_use_error>").unwrap_or(t);
    if let Some(m) = HOOK.captures(t) {
        let cmd = m[1].trim();
        let name = cmd
            .split_whitespace()
            .next()
            .map(|w| {
                Path::new(w)
                    .file_name()
                    .map_or(w.to_owned(), |n| n.to_string_lossy().into_owned())
            })
            .unwrap_or_default();
        return Some(format!("hook:{name}"));
    }
    if t.starts_with("PreToolUse:") && t.contains("hook returned blocking error") {
        return Some("hook:hook".to_owned());
    }
    if t.starts_with("Claude Code's removal safety check denied this command") {
        return Some("check".to_owned());
    }
    if t.starts_with("The user doesn't want to proceed with this tool use") {
        return Some("person".to_owned());
    }
    None
}

/// The message of a refusal, without the client's wrapping.
fn refusal_message(text: &str) -> &str {
    let t = text.trim_start();
    let t = t.strip_prefix("<tool_use_error>").unwrap_or(t);
    match HOOK.find(t) {
        Some(m) => t[m.end()..].trim_start_matches([':', ' ']),
        None => t,
    }
}

// ---------------------------------------------------------------------------
// Destructive commands

/// Folders whose removal is housekeeping: build output.
const DISPOSABLE: &[&str] = &[
    "target",
    "node_modules",
    "dist",
    "build",
    ".next",
    "deriveddata",
    "__pycache__",
    ".turbo",
    "coverage",
    ".expo",
    ".pytest_cache",
    ".gradle",
    ".output",
    "out",
    ".out",
    "shots",
    "screenshots",
    "frames",
];

/// Whether removing `target` is housekeeping: a temporary, cache or build
/// folder, or under one.
fn disposable(target: &str) -> bool {
    let t = target.to_lowercase();
    if t.starts_with("/var/folders") || t.starts_with("/private/var/folders") {
        return true;
    }
    t.split('/').any(|part| {
        DISPOSABLE.contains(&part)
            || part.contains("tmp")
            || part.contains("temp")
            || part.contains("cache")
            || part.contains("scratch")
            || part.starts_with(".target")
            || part.starts_with("target-")
    })
}

/// The destructive simple command in a script and its pattern, if any.
/// Follows the script's `cd` and its `NAME=value` assignments, so `cd /tmp
/// && rm -rf x` and `D=/tmp/x; rm -rf $D` are housekeeping; a target held
/// in a variable it does not set is not counted (precision over recall),
/// and neither is a folder the script makes again (`rm -rf fr && mkdir
/// fr`) or one `git check-ignore` vouched for.
pub(crate) fn destructive_in(commands: &[Simple]) -> Option<(&'static str, String)> {
    let mut vars: HashMap<String, String> = HashMap::new();
    let mut cwd_disposable = false;
    for (i, s) in commands.iter().enumerate() {
        for w in &s.words {
            if let Some((k, v)) = w.split_once('=') {
                if !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    vars.insert(k.to_owned(), v.to_owned());
                }
            }
        }
        let Some((p, args)) = s.program() else {
            continue;
        };
        let words = || one_line(&s.words.join(" "), SIGNAL_CHARS);
        match p.as_str() {
            "cd" | "pushd" => {
                if let Some(d) = args.first() {
                    let d = expand(d, &vars);
                    cwd_disposable = match d {
                        Some(d) if d.starts_with('/') || d.starts_with('~') => disposable(&d),
                        Some(d) => cwd_disposable || disposable(&d),
                        None => true,
                    };
                }
            }
            "rm" => {
                let mut r = false;
                let mut f = false;
                let mut targets = Vec::new();
                let mut flags_done = false;
                for a in &args {
                    if !flags_done && *a == "--" {
                        flags_done = true;
                    } else if !flags_done && a.starts_with("--") {
                        r |= *a == "--recursive";
                        f |= *a == "--force";
                    } else if !flags_done && a.starts_with('-') && a.len() > 1 {
                        r |= a.contains('r') || a.contains('R');
                        f |= a.contains('f');
                    } else {
                        targets.push(*a);
                    }
                }
                if !(r && f) {
                    continue;
                }
                let later = &commands[i + 1..];
                let earlier = &commands[..i];
                let harmful = targets.iter().any(|raw| {
                    let Some(t) = expand(raw, &vars) else {
                        return false;
                    };
                    let relative = !(t.starts_with('/') || t.starts_with('~'));
                    let remade = later.iter().any(|c| {
                        c.program()
                            .is_some_and(|(p, a)| p == "mkdir" && a.contains(raw))
                    });
                    let vouched = earlier.iter().any(|c| {
                        c.words.iter().any(|w| w == "check-ignore")
                            && c.words.iter().any(|w| w == raw)
                    });
                    !(disposable(&t) || (relative && cwd_disposable) || remade || vouched)
                });
                if harmful {
                    return Some(("rm -rf", words()));
                }
            }
            "git" => {
                let mut rest: &[&str] = &args;
                // `git -C <dir>`, `git -c key=value`.
                while let [flag, _, tail @ ..] = rest {
                    if *flag == "-C" || *flag == "-c" {
                        rest = tail;
                    } else {
                        break;
                    }
                }
                let Some((sub, rest)) = rest.split_first() else {
                    continue;
                };
                let pattern = match *sub {
                    "reset" if rest.contains(&"--hard") => Some("git reset --hard"),
                    // `--force-with-lease` is the safe form a rebase needs;
                    // a plain force is not.
                    "push"
                        if rest.iter().any(|a| {
                            *a == "--force"
                                || *a == "-f"
                                || (a.starts_with('-') && !a.starts_with("--") && a.contains('f'))
                                || (a.starts_with('+') && a.len() > 1)
                        }) =>
                    {
                        Some("force push")
                    }
                    "clean"
                        if rest.iter().any(|a| {
                            a.starts_with('-') && !a.starts_with("--") && a.contains('f')
                        }) || rest.contains(&"--force") =>
                    {
                        Some("git clean -f")
                    }
                    "checkout" if rest == ["--", "."] || rest == ["."] => Some("git checkout ."),
                    "restore"
                        if rest.contains(&".")
                            && (!rest.contains(&"--staged") || rest.contains(&"--worktree")) =>
                    {
                        Some("git restore .")
                    }
                    _ => None,
                };
                if let Some(p) = pattern {
                    return Some((p, words()));
                }
            }
            _ => {}
        }
    }
    None
}

/// `word` with `$NAME`/`${NAME}` replaced from `vars`; `None` when it uses
/// a variable the script does not set.
fn expand(word: &str, vars: &HashMap<String, String>) -> Option<String> {
    if !word.contains('$') {
        return Some(word.to_owned());
    }
    static VAR: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\$\{?([A-Za-z_][A-Za-z0-9_]*)\}?").expect("valid regex"));
    let mut missing = false;
    let out = VAR.replace_all(word, |c: &regex::Captures<'_>| match vars.get(&c[1]) {
        Some(v) => v.clone(),
        None => {
            missing = true;
            String::new()
        }
    });
    (!missing).then(|| out.into_owned())
}

/// A destructive call's signal: its first destructive simple command.
fn destructive(t: &StoredTurn) -> Option<Signal> {
    let (name, input) = crate::dream::observed::call(t)?;
    let script = command_of(name, input)?;
    // An SQL `DROP` is not read: nearly every one was a scratch test
    // database or a word in a query's text.
    let (pattern, words) = destructive_in(&shell::simple_commands(&script))?;
    Some(Signal {
        kind: SignalKind::Destructive,
        ordinal: t.ordinal,
        ts: t.ts.clone(),
        step_ordinal: Some(t.ordinal),
        step_ts: Some(t.ts.clone()),
        step: format!("{name}: {}", one_line(&script, SIGNAL_CHARS)),
        text: words,
        extra: pattern.to_owned(),
    })
}

// ---------------------------------------------------------------------------
// Edit lines

/// A line's hash for comparing edits, or `None` for a line
/// too short or with no letter or digit.
pub fn line_hash(line: &str) -> Option<String> {
    let squashed = line.split_whitespace().collect::<Vec<_>>().join(" ");
    if squashed.chars().count() < 8 || !squashed.chars().any(char::is_alphanumeric) {
        return None;
    }
    Some(hex(&sha256(squashed.as_bytes()))[..12].to_owned())
}

fn hashes<'a>(lines: impl Iterator<Item = &'a str>) -> BTreeSet<String> {
    lines.filter_map(line_hash).collect()
}

/// Added and removed line hashes of an old and a new text.
fn diff_sets(old: &str, new: &str) -> (BTreeSet<String>, BTreeSet<String>) {
    let o = hashes(old.lines());
    let n = hashes(new.lines());
    (
        n.difference(&o).cloned().collect(),
        o.difference(&n).cloned().collect(),
    )
}

/// The files an edit call changed, with the hashes of the lines it added
/// and removed: [`crate::rollup::changes::of_call`]'s reading, line by line.
pub fn edit_sets(
    name: &str,
    input: &str,
    result: Option<&str>,
) -> Vec<(String, BTreeSet<String>, BTreeSet<String>)> {
    if name == "apply_patch" || (name != "Edit" && name != "MultiEdit" && name != "Write") {
        let text = match name {
            "apply_patch" => patch_text(input),
            _ => match command_of(name, input) {
                Some(c) if crate::rollup::changes::has_patch(&c) => c,
                _ => return Vec::new(),
            },
        };
        return patch_sets(&text);
    }
    let Ok(v) = serde_json::from_str::<Value>(input) else {
        return Vec::new();
    };
    let s = |v: &Value, k: &str| {
        v.get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let path = s(&v, "file_path");
    if path.is_empty() {
        return Vec::new();
    }
    let (added, removed) = match name {
        "Edit" => diff_sets(&s(&v, "old_string"), &s(&v, "new_string")),
        "MultiEdit" => {
            let mut a = BTreeSet::new();
            let mut r = BTreeSet::new();
            for e in v
                .get("edits")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let (ea, er) = diff_sets(&s(e, "old_string"), &s(e, "new_string"));
                a.extend(ea);
                r.extend(er);
            }
            (a, r)
        }
        _ => {
            // A Write that replaced a file: its old text is unknown, so
            // nothing it added or removed is known.
            let created = result.is_some_and(|r| {
                let r = r.to_lowercase();
                r.contains("created") && !r.contains("updated")
            });
            if !created {
                return Vec::new();
            }
            (hashes(s(&v, "content").lines()), BTreeSet::new())
        }
    };
    vec![(path, added, removed)]
}

fn patch_text(input: &str) -> String {
    match serde_json::from_str::<Value>(input) {
        Ok(Value::String(s)) => s,
        Ok(v) => ["input", "patch"]
            .iter()
            .find_map(|k| v.get(*k).and_then(Value::as_str))
            .unwrap_or_default()
            .to_owned(),
        Err(_) => input.to_owned(),
    }
}

/// A Codex patch's files with the hashes of their `+` and `-` lines.
fn patch_sets(patch: &str) -> Vec<(String, BTreeSet<String>, BTreeSet<String>)> {
    let files = patch_lines(patch);
    let mut out: Vec<(String, BTreeSet<String>, BTreeSet<String>)> = files
        .iter()
        .map(|f| (f.path.clone(), BTreeSet::new(), BTreeSet::new()))
        .collect();
    let mut at: Option<usize> = None;
    for line in patch.lines() {
        if let Some(rest) = line.strip_prefix("*** ") {
            let p = ["Add File: ", "Update File: ", "Delete File: "]
                .iter()
                .find_map(|k| rest.strip_prefix(k))
                .map(str::trim);
            at = p.and_then(|p| out.iter().position(|f| f.0 == p));
            continue;
        }
        let Some(i) = at else { continue };
        if let Some(l) = line.strip_prefix('+') {
            out[i].1.extend(line_hash(l));
        } else if let Some(l) = line.strip_prefix('-') {
            out[i].2.extend(line_hash(l));
        }
    }
    out.retain(|f| !f.1.is_empty() || !f.2.is_empty());
    out
}

/// Every edit call's lines in a session's turns.
pub fn edits(turns: &[StoredTurn], repos: &mut Repos) -> Vec<EditLines> {
    let mut out = Vec::new();
    for (call, result) in shell::pairs(turns) {
        let t = &turns[call];
        let Some((name, input)) = crate::dream::observed::call(t) else {
            continue;
        };
        if !(is_edit_tool(name)
            || command_of(name, input).is_some_and(|c| crate::rollup::changes::has_patch(&c)))
        {
            continue;
        }
        let result = result.map(|r| turns[r].text.as_str());
        for (path, added, removed) in edit_sets(name, input, result) {
            if added.is_empty() && removed.is_empty() {
                continue;
            }
            let file = repos
                .relative(Path::new(&path))
                .map_or_else(|| path.clone(), |(repo, rel)| format!("{repo}:{rel}"));
            out.push(EditLines {
                ordinal: t.ordinal,
                ts: t.ts.clone(),
                path,
                file,
                added: added.into_iter().collect(),
                removed: removed.into_iter().collect(),
            });
        }
    }
    out
}
