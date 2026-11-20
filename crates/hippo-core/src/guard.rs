//! The guard, Part B (`docs/designs/guard.md`): a check each agent's
//! pre-tool hook runs before a tool call, warn-only in this release.
//!
//! Rules come from four layers, compiled into one rule set in
//! `$HIPPO_HOME/guard/compiled.json`:
//!
//! 1. built-ins in this file and in `incidents::guard` (the kinds, the
//!    built-in allow list, the self-protection rules);
//! 2. machine rules, `$HIPPO_HOME/guard/machine.toml`, for every session;
//! 3. the workspace's guard note (`guard_note`, `<notes_dir>/Guard.md` by
//!    default), for a session whose folder is inside the workspace or one
//!    of its worktrees;
//! 4. packs' `Guard.md`, which can add rules and raise actions but never
//!    add allow entries.
//!
//! A new rule set is compared with the one in force, whatever layer
//! changed (`docs/designs/common.md`, rule 3): a rule added, an action
//! raised or an allow entry removed applies at once; a rule removed, an
//! action lowered or an allow entry added waits for `hippo accept`. An
//! edited `match` rule is a removal and an addition. The accepted file
//! holds the hash of the set in force, so a `compiled.json` changed by
//! anything but hippo is noticed and set aside.
//!
//! In this release every kind and `match` rule acts as `warn`, whatever
//! its configured action; only the self-protection rules block.

mod check;
pub mod install;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::accept::{self, Fail};
use crate::config::{Config, Workspace};
use crate::error::{Error, Result};

pub use check::{
    decide, effective, evaluate, log, parse_call, payload_agent, respond, test_history, Call,
    Decision, Hit, TestCount, AGENTS,
};

/// The release where `ask` and `block` act; until then they warn.
pub const WARN_ONLY: bool = true;

/// The kinds a rule can name that the guard checks in this release.
pub const KINDS: [&str; 4] = [
    "remote-script",
    "fresh-clone-exec",
    "credential-read",
    "guard-bypass",
];

/// Kinds the design names for later: a rule may name them, and they do
/// nothing yet.
pub const LATER_KINDS: [&str; 2] = ["unknown-egress", "command-from-fetched"];

/// What a rule does when it matches.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    /// The call runs; the agent (and where the agent allows it, the
    /// person) gets a note.
    #[default]
    Warn,
    /// The person is asked.
    Ask,
    /// The call is refused.
    Block,
}

impl std::fmt::Display for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Action::Warn => "warn",
            Action::Ask => "ask",
            Action::Block => "block",
        })
    }
}

/// One rule in a compiled set.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Rule {
    /// `machine`, `workspace` or `pack:<name>`.
    pub layer: String,
    /// A built-in kind this rule sets the action of.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// A regular expression over a shell call's command.
    #[serde(rename = "match", default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    /// A path glob over a file tool's paths (`~` is the home folder).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub action: Action,
}

type RuleId = (
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

impl Rule {
    /// Everything but the action: two rules with one id are the same rule.
    fn id(&self) -> RuleId {
        (
            self.layer.clone(),
            self.kind.clone(),
            self.name.clone(),
            self.pattern.clone(),
            self.path.clone(),
            self.reason.clone(),
        )
    }

    /// How the rule is named to people: its name, kind or pattern, and
    /// its layer.
    pub fn label(&self) -> String {
        let what = self
            .name
            .clone()
            .or_else(|| self.kind.clone())
            .or_else(|| self.pattern.as_ref().map(|p| format!("match {p}")))
            .or_else(|| self.path.as_ref().map(|p| format!("path {p}")))
            .unwrap_or_default();
        format!("{what} ({})", self.layer)
    }
}

/// One allow entry: a host (with an optional path prefix) for
/// `remote-script`, a clone origin for `fresh-clone-exec`, or a keychain
/// service or credential path glob for `credential-read`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Allow {
    pub layer: String,
    /// `host`, `origin`, `keychain` or `path`.
    pub what: String,
    pub value: String,
}

/// A compiled rule set: rules and allow entries, sorted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleSet {
    pub rules: Vec<Rule>,
    pub allow: Vec<Allow>,
}

impl RuleSet {
    fn normalise(&mut self) {
        self.rules.sort();
        self.rules.dedup_by(|a, b| {
            a.id() == b.id() && {
                // Two copies of one rule: the stricter action stays.
                b.action = b.action.max(a.action);
                true
            }
        });
        self.allow.sort();
        self.allow.dedup();
    }

    /// The SHA-256 of the set's JSON, as the accepted file records it.
    pub fn hash(&self) -> String {
        let json = serde_json::to_vec(self).expect("a rule set serialises");
        crate::util::hex(&crate::util::sha256(&json))
    }
}

/// The set in force for an accepted set and a new one, and what waits:
/// rules in either (a removal waits), each at the stricter action (a
/// lowering waits), and only the allow entries in both (an addition
/// waits).
pub fn merge(accepted: &RuleSet, candidate: &RuleSet) -> (RuleSet, Vec<String>) {
    let mut in_force = RuleSet::default();
    let mut waiting = Vec::new();
    for rule in &accepted.rules {
        match candidate.rules.iter().find(|c| c.id() == rule.id()) {
            None => {
                waiting.push(format!("rule removed: {}", rule.label()));
                in_force.rules.push(rule.clone());
            }
            Some(c) => {
                if c.action < rule.action {
                    waiting.push(format!(
                        "action lowered: {} {} → {}",
                        rule.label(),
                        rule.action,
                        c.action
                    ));
                }
                let mut kept = rule.clone();
                kept.action = rule.action.max(c.action);
                in_force.rules.push(kept);
            }
        }
    }
    for c in &candidate.rules {
        if !accepted.rules.iter().any(|r| r.id() == c.id()) {
            in_force.rules.push(c.clone());
        }
    }
    for a in &candidate.allow {
        if accepted.allow.contains(a) {
            in_force.allow.push(a.clone());
        } else {
            waiting.push(format!(
                "allow entry added: {} {} ({})",
                a.what, a.value, a.layer
            ));
        }
    }
    in_force.normalise();
    (in_force, waiting)
}

// ---------------------------------------------------------------------------
// Reading rules

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct BlockSpec {
    #[serde(default)]
    allow: AllowSpec,
    #[serde(default)]
    rule: Vec<RuleSpec>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct AllowSpec {
    #[serde(default)]
    hosts: Vec<String>,
    #[serde(default)]
    clone_origins: Vec<String>,
    /// Keychain items `credential-read` may read, by service.
    #[serde(default)]
    keychain: Vec<String>,
    /// Credential files `credential-read` may read: globs over the path,
    /// `~` for the home folder.
    #[serde(default)]
    paths: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleSpec {
    kind: Option<String>,
    name: Option<String>,
    #[serde(rename = "match")]
    pattern: Option<String>,
    path: Option<String>,
    action: Action,
    reason: Option<String>,
}

/// The fenced `hippo-guard` blocks of a note, in order.
pub fn fenced_blocks(note: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut open: Option<(String, Vec<&str>)> = None;
    for line in note.lines() {
        let t = line.trim_start();
        match &mut open {
            None => {
                for fence in ["```", "~~~"] {
                    if let Some(info) = t.strip_prefix(fence) {
                        if info
                            .trim_start_matches(fence.chars().next().unwrap())
                            .trim()
                            == "hippo-guard"
                        {
                            let run: String = t
                                .chars()
                                .take_while(|c| *c == fence.chars().next().unwrap())
                                .collect();
                            open = Some((run, Vec::new()));
                        }
                    }
                }
            }
            Some((run, lines)) => {
                if t.starts_with(run.as_str())
                    && t.trim_start_matches(run.chars().next().unwrap())
                        .trim()
                        .is_empty()
                {
                    out.push(lines.join("\n"));
                    open = None;
                } else {
                    lines.push(line);
                }
            }
        }
    }
    out
}

/// Reads one block's TOML into rules and allow entries for `layer`,
/// adding a line to `problems` for anything it cannot use.
fn read_block(text: &str, layer: &str, from: &str, set: &mut RuleSet, problems: &mut Vec<String>) {
    let spec: BlockSpec = match toml::from_str(text) {
        Ok(spec) => spec,
        Err(e) => {
            problems.push(format!(
                "{from}: a hippo-guard block does not parse: {}",
                one_line(&e.to_string())
            ));
            return;
        }
    };
    let packs = layer.starts_with("pack:");
    if packs
        && !(spec.allow.hosts.is_empty()
            && spec.allow.clone_origins.is_empty()
            && spec.allow.keychain.is_empty()
            && spec.allow.paths.is_empty())
    {
        problems.push(format!(
            "{from}: a pack cannot add allow entries; they are ignored"
        ));
    }
    if !packs {
        for (what, values) in [
            ("host", &spec.allow.hosts),
            ("origin", &spec.allow.clone_origins),
            ("keychain", &spec.allow.keychain),
            ("path", &spec.allow.paths),
        ] {
            for v in values {
                let v = v
                    .trim()
                    .trim_start_matches("https://")
                    .trim_start_matches("http://");
                if v.is_empty() {
                    continue;
                }
                set.allow.push(Allow {
                    layer: layer.to_owned(),
                    what: what.to_owned(),
                    value: v.to_owned(),
                });
            }
        }
    }
    for r in spec.rule {
        let given = [r.kind.is_some(), r.pattern.is_some(), r.path.is_some()]
            .iter()
            .filter(|b| **b)
            .count();
        if given != 1 {
            problems.push(format!(
                "{from}: a rule needs exactly one of kind, match or path"
            ));
            continue;
        }
        if let Some(kind) = &r.kind {
            if !KINDS.contains(&kind.as_str()) && !LATER_KINDS.contains(&kind.as_str()) {
                problems.push(format!("{from}: unknown kind {kind:?}"));
                continue;
            }
        }
        if let Some(p) = &r.pattern {
            if let Err(e) = regex::Regex::new(p) {
                problems.push(format!(
                    "{from}: match {p:?} is not a regular expression: {}",
                    one_line(&e.to_string())
                ));
                continue;
            }
        }
        if let Some(p) = &r.path {
            if glob::Pattern::new(p).is_err() {
                problems.push(format!("{from}: path {p:?} is not a glob"));
                continue;
            }
        }
        set.rules.push(Rule {
            layer: layer.to_owned(),
            kind: r.kind,
            name: r.name,
            pattern: r.pattern,
            path: r.path,
            reason: r.reason,
            action: r.action,
        });
    }
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `$HIPPO_HOME/guard/`.
pub fn dir(home: &Path) -> PathBuf {
    home.join("guard")
}

/// `$HIPPO_HOME/guard/machine.toml`: the person's own rules, in the
/// format of a `hippo-guard` block.
pub fn machine_path(home: &Path) -> PathBuf {
    dir(home).join("machine.toml")
}

/// `$HIPPO_HOME/guard/compiled.json`.
pub fn compiled_path(home: &Path) -> PathBuf {
    dir(home).join("compiled.json")
}

/// The packs' guard notes: `<workspace>/.hippo/packs/<name>/Guard.md`.
fn pack_notes(ws: &Workspace) -> Vec<(String, PathBuf)> {
    let packs = ws.hippo_dir().join("packs");
    let mut out = Vec::new();
    if let Ok(entries) = fs::read_dir(&packs) {
        for e in entries.flatten() {
            let note = e.path().join("Guard.md");
            if note.is_file() {
                out.push((e.file_name().to_string_lossy().into_owned(), note));
            }
        }
    }
    out.sort();
    out
}

/// The rules every layer gives now, and what could not be read.
pub fn candidate(home: &Path, ws: Option<&Workspace>) -> (RuleSet, Vec<String>) {
    let mut set = RuleSet::default();
    let mut problems = Vec::new();
    let machine = machine_path(home);
    match fs::read_to_string(&machine) {
        Ok(text) => read_block(
            &text,
            "machine",
            &machine.display().to_string(),
            &mut set,
            &mut problems,
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => problems.push(format!("{}: {e}", machine.display())),
    }
    if let Some(ws) = ws {
        let note = ws.guard_note_path();
        match fs::read_to_string(&note) {
            Ok(text) => {
                for block in fenced_blocks(&text) {
                    read_block(
                        &block,
                        "workspace",
                        &note.display().to_string(),
                        &mut set,
                        &mut problems,
                    );
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => problems.push(format!("{}: {e}", note.display())),
        }
        for (name, note) in pack_notes(ws) {
            if let Ok(text) = fs::read_to_string(&note) {
                for block in fenced_blocks(&text) {
                    read_block(
                        &block,
                        &format!("pack:{name}"),
                        &note.display().to_string(),
                        &mut set,
                        &mut problems,
                    );
                }
            }
        }
    }
    set.normalise();
    (set, problems)
}

// ---------------------------------------------------------------------------
// The compiled set

/// A file the compiled set was read from, and its size and change time
/// then: when any differs, the set is compiled again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    pub path: PathBuf,
    pub stamp: Option<(u64, i128)>,
}

fn stamp(path: &Path) -> Option<(u64, i128)> {
    let m = fs::metadata(path).ok()?;
    let modified = m.modified().ok()?;
    let ns = match modified.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_nanos() as i128,
        Err(e) => -(e.duration().as_nanos() as i128),
    };
    Some((m.len(), ns))
}

/// `$HIPPO_HOME/guard/compiled.json`: what `hippo guard check` reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Compiled {
    /// The hippo that compiled it; another one compiles again.
    pub hippo_version: String,
    /// The accepted file's key.
    pub key: String,
    /// The rules in force.
    pub in_force: RuleSet,
    /// The rules the layers give now.
    pub candidate: RuleSet,
    /// What waits for `hippo accept`.
    pub waiting: Vec<String>,
    /// What could not be read.
    pub problems: Vec<String>,
    /// The workspace, and its worktrees: where the workspace layer applies.
    pub workspace: Option<PathBuf>,
    pub worktrees: Vec<PathBuf>,
    /// `[guard] fail` and `timeout_ms` in force.
    pub fail: Fail,
    pub timeout_ms: u64,
    /// `$HIPPO_HOME`, resolved, for the self-protection rules.
    pub home: PathBuf,
    pub sources: Vec<Source>,
}

impl Compiled {
    /// Whether every source is as it was when this was compiled.
    pub fn is_fresh(&self) -> bool {
        self.hippo_version == crate::config::HIPPO_VERSION
            && self.sources.iter().all(|s| stamp(&s.path) == s.stamp)
    }

    /// Whether `dir` is inside the workspace or one of its worktrees.
    pub fn in_workspace(&self, dir: &Path) -> bool {
        self.workspace
            .iter()
            .chain(&self.worktrees)
            .any(|root| dir.starts_with(root))
    }
}

/// The worktrees of the git repository at `root`, read from
/// `.git/worktrees/*/gitdir` without running git.
fn worktrees(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = fs::read_dir(root.join(".git").join("worktrees")) {
        for e in entries.flatten() {
            if let Ok(text) = fs::read_to_string(e.path().join("gitdir")) {
                if let Some(tree) = Path::new(text.trim()).parent() {
                    out.push(fs::canonicalize(tree).unwrap_or_else(|_| tree.to_path_buf()));
                }
            }
        }
    }
    out.sort();
    out
}

/// Reads `compiled.json`; `None` when it is missing or does not parse.
pub fn read_compiled(home: &Path) -> Option<Compiled> {
    let text = fs::read(compiled_path(home)).ok()?;
    serde_json::from_slice(&text).ok()
}

/// Compiles the rule set for the configuration and its workspace, applies
/// rule 3 against the set in force, records the result as accepted, and
/// writes `compiled.json`.
pub fn compile(cfg: &Config, ws: Option<&Workspace>) -> Result<Compiled> {
    compile_with(cfg, ws, false)
}

/// Accepts the rule set the layers give now: `hippo accept`.
pub fn accept(cfg: &Config, ws: Option<&Workspace>) -> Result<Compiled> {
    compile_with(cfg, ws, true)
}

fn compile_with(cfg: &Config, ws: Option<&Workspace>, accept_all: bool) -> Result<Compiled> {
    let home = &cfg.home;
    let key = accept::key(ws);
    let mut accepted = accept::load(home, &key).ok().flatten().unwrap_or_default();
    let (candidate, mut problems) = candidate(home, ws);
    // The set in force before: the last compiled one, if the accepted
    // file vouches for it; else the built-ins alone.
    let previous = read_compiled(home).filter(|c| c.key == key);
    let baseline = match (&previous, &accepted.guard_rules) {
        (Some(prev), Some(hash)) if prev.in_force.hash() == *hash => prev.in_force.clone(),
        (_, Some(_)) => {
            problems.push(
                "the compiled rules did not match the accepted hash; the guard started again from its built-in rules".into(),
            );
            RuleSet::default()
        }
        _ => RuleSet::default(),
    };
    let (in_force, waiting) = if accept_all {
        (candidate.clone(), Vec::new())
    } else {
        merge(&baseline, &candidate)
    };
    if accepted.guard_rules.as_deref() != Some(in_force.hash().as_str()) {
        accepted.guard_rules = Some(in_force.hash());
        accept::store(home, &key, &accepted)?;
    }
    let (fail, timeout_ms) = match ws {
        Some(ws) => (
            ws.guard.fail.unwrap_or_default(),
            ws.guard.timeout_ms.unwrap_or(accept::DEFAULT_TIMEOUT_MS),
        ),
        None => (Fail::Open, accept::DEFAULT_TIMEOUT_MS),
    };
    let mut paths = vec![
        home.join("config.toml"),
        machine_path(home),
        accept::path(home, &key),
    ];
    let mut workspace = None;
    let mut trees = Vec::new();
    if let Some(ws) = ws {
        paths.push(ws.hippo_dir().join("hippo.toml"));
        paths.push(ws.guard_note_path());
        paths.push(ws.hippo_dir().join("packs"));
        paths.extend(pack_notes(ws).into_iter().map(|(_, p)| p));
        paths.push(ws.root.join(".git").join("worktrees"));
        workspace = Some(fs::canonicalize(&ws.root).unwrap_or_else(|_| ws.root.clone()));
        trees = worktrees(&ws.root);
    }
    let compiled = Compiled {
        hippo_version: crate::config::HIPPO_VERSION.to_owned(),
        key,
        in_force,
        candidate,
        waiting,
        problems,
        workspace,
        worktrees: trees,
        fail,
        timeout_ms,
        home: fs::canonicalize(home).unwrap_or_else(|_| home.clone()),
        sources: paths
            .into_iter()
            .map(|path| Source {
                stamp: stamp(&path),
                path,
            })
            .collect(),
    };
    crate::perms::create_private_dir(&dir(home))?;
    let json = serde_json::to_vec_pretty(&compiled)?;
    crate::util::write_atomic(&compiled_path(home), &json)?;
    Ok(compiled)
}

/// The compiled set, compiled again first when a source changed.
pub fn current(cfg: &Config, ws: Option<&Workspace>) -> Result<Compiled> {
    match read_compiled(&cfg.home) {
        Some(c) if c.is_fresh() && c.key == accept::key(ws) => Ok(c),
        _ => compile(cfg, ws),
    }
}

/// The compiled set for `hippo guard check`: read as it is when fresh,
/// else compiled again from the configuration under `home`.
pub fn for_check(home: &Path) -> Result<Compiled> {
    if let Some(c) = read_compiled(home) {
        if c.is_fresh() {
            return Ok(c);
        }
    }
    // The guard is machine-wide: it needs no index and no workspace, so a
    // machine where hippo was never run gets the built-in and machine
    // rules rather than failing open on every call.
    let cfg = match Config::load_in(home) {
        Err(crate::error::Error::NotSetUp(_)) => Config::defaults_in(home),
        other => other?,
    };
    let ws = Workspace::load_optional(&cfg)?;
    compile(&cfg, ws.as_ref())
}

/// What the guard has logged, for `doctor` and `guard status`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Activity {
    /// Calls checked today.
    pub checked_today: u64,
    /// Calls a rule matched in the last day.
    pub matched_day: usize,
    /// Failures (timeouts, errors) in the last day.
    pub failures_day: usize,
}

/// `$HIPPO_HOME/guard/log.jsonl`: one line per call a rule matched.
pub fn log_path(home: &Path) -> PathBuf {
    dir(home).join("log.jsonl")
}

/// `$HIPPO_HOME/guard/failures.jsonl`: one line per failed check.
pub fn failures_path(home: &Path) -> PathBuf {
    dir(home).join("failures.jsonl")
}

/// `$HIPPO_HOME/guard/calls/<day>`: one byte per call checked that day.
pub fn calls_path(home: &Path, day: &str) -> PathBuf {
    dir(home).join("calls").join(day)
}

fn today() -> String {
    let now = time::OffsetDateTime::now_utc();
    format!(
        "{:04}-{:02}-{:02}",
        now.year(),
        u8::from(now.month()),
        now.day()
    )
}

fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

/// Appends `line` to `path`, creating it 0600 in a 0700 folder.
fn append(path: &Path, line: &[u8]) -> Result<()> {
    use std::io::Write as _;
    if let Some(parent) = path.parent() {
        crate::perms::create_private_dir(parent)?;
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| Error::io_at(path, e))?;
    file.write_all(line).map_err(|e| Error::io_at(path, e))
}

/// Counts one checked call for today.
pub fn count_call(home: &Path) {
    let _ = append(&calls_path(home, &today()), b".");
}

/// Logs a failed check: what went wrong and what the call did.
pub fn log_failure(home: &Path, agent: &str, what: &str, fail: Fail) {
    let line = serde_json::json!({
        "at": now_rfc3339(),
        "agent": agent,
        "failure": what,
        "call": if fail == Fail::Closed { "refused (fail = closed)" } else { "proceeded (fail = open)" },
    });
    let _ = append(&failures_path(home), format!("{line}\n").as_bytes());
}

/// What the log and the counts say.
pub fn activity(home: &Path) -> Activity {
    let since = time::OffsetDateTime::now_utc() - time::Duration::days(1);
    let since = since
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();
    let recent = |path: PathBuf| {
        fs::read_to_string(path)
            .map(|text| {
                text.lines()
                    .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
                    .filter(|v| {
                        v.get("at")
                            .and_then(|a| a.as_str())
                            .is_some_and(|a| a >= since.as_str())
                    })
                    .count()
            })
            .unwrap_or(0)
    };
    Activity {
        checked_today: fs::metadata(calls_path(home, &today())).map_or(0, |m| m.len()),
        matched_day: recent(log_path(home)),
        failures_day: recent(failures_path(home)),
    }
}

/// Removes call counts older than 14 days.
pub fn prune_counts(home: &Path) {
    let cutoff = time::OffsetDateTime::now_utc() - time::Duration::days(14);
    let cutoff = format!(
        "{:04}-{:02}-{:02}",
        cutoff.year(),
        u8::from(cutoff.month()),
        cutoff.day()
    );
    if let Ok(entries) = fs::read_dir(dir(home).join("calls")) {
        for e in entries.flatten() {
            if e.file_name().to_string_lossy().as_ref() < cutoff.as_str() {
                let _ = fs::remove_file(e.path());
            }
        }
    }
}

/// The waiting lines and problems of the compiled set, for `accept` and
/// `doctor`, compiling first when a source changed.
pub fn waiting(cfg: &Config, ws: Option<&Workspace>) -> Result<Vec<String>> {
    Ok(current(cfg, ws)?.waiting)
}

/// The allow entries of `set` active for a call in or out of the
/// workspace.
fn active_layer(layer: &str, in_ws: bool) -> bool {
    layer != "workspace" || in_ws
}

/// The kinds' default actions, before any rule: what they become once
/// `ask` and `block` act.
pub fn default_action(kind: &str, detail: &str) -> Action {
    match kind {
        "remote-script" => Action::Ask,
        "credential-read"
            if matches!(
                detail,
                "ssh private key" | "keychain" | "keychain password" | "wallet"
            ) =>
        {
            Action::Ask
        }
        _ => Action::Warn,
    }
}

/// A line for a person: `<n> allow entries, <n> rules`.
pub fn summary(set: &RuleSet) -> String {
    let layers: BTreeSet<&str> = set.rules.iter().map(|r| r.layer.as_str()).collect();
    format!(
        "{} rules, {} allow entries{}",
        set.rules.len(),
        set.allow.len(),
        if layers.is_empty() {
            String::new()
        } else {
            format!(
                " (from {})",
                layers.into_iter().collect::<Vec<_>>().join(", ")
            )
        }
    )
}

#[cfg(test)]
mod tests;
