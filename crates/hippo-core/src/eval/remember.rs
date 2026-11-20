//! The `remember` suite (`docs/designs/remember.md`, "Eval"): a synthetic
//! history with rules said in other words weeks apart, a reversal, a rule
//! only in an account memory, a long session that changes topic and a
//! session outside the workspace, measured for this release and for 0.7.2's
//! behaviour reproduced in the same harness.
//!
//! The model is an oracle that answers from the set's answer key: the match
//! call groups quotes of one planted rule and names a planted reversal; the
//! drafter writes a planted rule only for an item seen in two sessions or
//! more (a conservative drafter, as hippo's own declines one-off
//! corrections); reviewers confirm planted rules. So the suite measures
//! hippo's pipeline (detection, grouping, delivery), not a model's
//! judgement.
//!
//! - capture: planted rules (not later reversed) that became a pending or
//!   kept lesson;
//! - delivery: (session, rule it needs) pairs where the session was given
//!   the rule, end to end, so a rule never captured is never delivered;
//! - stale: pairs where a session was given a rule that a newer one
//!   reversed;
//! - calls: model calls this release's backfill made on the set.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use time::{Duration, OffsetDateTime};
use ulid::Ulid;

use crate::adapters::{RawTurn, Role};
use crate::config::Workspace;
use crate::context::{self, topic, PartSelection};
use crate::error::{Error, Result};
use crate::index::{Index, SessionSource, Turn};
use crate::lessons::{self, detect, memory, Status};
use crate::records::Tool;
use crate::util::copy_dir;

/// One planted rule.
#[derive(Debug, Clone, Deserialize)]
pub struct PlantedRule {
    pub id: String,
    /// The rule as the oracle drafts it.
    pub rule: String,
    /// `machine` or `workspace`.
    pub scope: String,
    /// Every correction and memory line that states it.
    pub quotes: Vec<String>,
    /// The planted rule this one takes back.
    #[serde(default)]
    pub reverses: Option<String>,
}

/// One turn of a planted session.
#[derive(Debug, Clone, Deserialize)]
pub struct PlantedTurn {
    /// `user`, `assistant` or `bash` (an agent's shell command).
    pub role: String,
    pub text: String,
}

/// One planted session.
#[derive(Debug, Clone, Deserialize)]
pub struct PlantedSession {
    pub id: String,
    pub days_ago: i64,
    /// Started outside the workspace.
    #[serde(default)]
    pub outside: bool,
    /// Every turn at one time: a replayed transcript.
    #[serde(default)]
    pub replayed: bool,
    pub turns: Vec<PlantedTurn>,
    /// The planted rules this session should be given.
    #[serde(default)]
    pub needs: Vec<String>,
}

/// A planted agent memory file, under a synthetic user home.
#[derive(Debug, Clone, Deserialize)]
pub struct PlantedMemory {
    /// Relative to the user home, as `.claude-work/CLAUDE.md`.
    pub path: String,
    /// Its source id, turned on for the run.
    pub source: String,
    pub text: String,
}

/// `remember.toml`: the answer key and the history.
#[derive(Debug, Clone, Deserialize)]
pub struct Key {
    pub rules: Vec<PlantedRule>,
    pub sessions: Vec<PlantedSession>,
    #[serde(default)]
    pub memory: Vec<PlantedMemory>,
}

impl Key {
    pub fn load(path: &Path) -> Result<Key> {
        let text = fs::read_to_string(path).map_err(|e| Error::io_at(path, e))?;
        toml::from_str(&text)
            .map_err(|e| Error::InvalidProposal(format!("{}: {e}", path.display())))
    }

    fn norm(text: &str) -> String {
        text.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .replace("< < <", "<<<")
            .to_lowercase()
    }

    /// The planted rule a quote states.
    pub fn rule_of(&self, quote: &str) -> Option<&PlantedRule> {
        let q = Key::norm(quote);
        self.rules
            .iter()
            .find(|r| r.quotes.iter().any(|x| Key::norm(x) == q) || Key::norm(&r.rule) == q)
    }

    /// Planted rules a later one reverses.
    fn reversed(&self) -> BTreeSet<&str> {
        self.rules
            .iter()
            .filter_map(|r| r.reverses.as_deref())
            .collect()
    }
}

// ---------------------------------------------------------------------------
// The oracle

/// The oracle's reply to one call, as `claude -p --output-format json`
/// prints it: `{"result": …}`.
pub fn oracle(key: &Key, system: &str, prompt: &str) -> String {
    let reply = if system.contains("correction matcher") {
        oracle_match(key, prompt)
    } else if system.contains("memory reviewer") {
        let ok = block(prompt, "line").iter().any(|v| {
            v.get("line")
                .and_then(Value::as_str)
                .is_some_and(|l| key.rule_of(l).is_some())
        });
        json!({"verdict": if ok { "confirm" } else { "reject" }, "notes": "oracle"}).to_string()
    } else if system.contains("lesson reviewer") {
        json!({"verdict": "confirm", "notes": "oracle"}).to_string()
    } else if system.contains("lessons pass") {
        oracle_draft(key, prompt)
    } else {
        "[]".to_owned()
    };
    json!({ "result": reply }).to_string()
}

/// The JSON lines between `<<<UNTRUSTED name>>>` and `<<<END name>>>`.
fn block(prompt: &str, name: &str) -> Vec<Value> {
    let open = format!("<<<UNTRUSTED {name}>>>");
    let close = format!("<<<END {name}>>>");
    let Some(start) = prompt.find(&open) else {
        return Vec::new();
    };
    let rest = &prompt[start + open.len()..];
    let end = rest.find(&close).unwrap_or(rest.len());
    rest[..end]
        .lines()
        .filter_map(|l| serde_json::from_str(l.trim()).ok())
        .collect()
}

fn oracle_match(key: &Key, prompt: &str) -> String {
    let mut out = Vec::new();
    for item in block(prompt, "items") {
        let n = item.get("n").cloned().unwrap_or(Value::Null);
        let mine = item
            .get("correction")
            .and_then(Value::as_str)
            .and_then(|q| key.rule_of(q));
        let mut same = Vec::new();
        let mut reverses = Value::Null;
        for c in item
            .get("candidates")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let id = c.get("id").cloned().unwrap_or(Value::Null);
            let theirs: BTreeSet<&str> = c
                .get("said")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter_map(|q| key.rule_of(q))
                .map(|r| r.id.as_str())
                .collect();
            let Some(mine) = mine else { continue };
            if theirs.contains(mine.id.as_str()) {
                same.push(id);
            } else if mine.reverses.as_deref().is_some_and(|r| theirs.contains(r)) {
                reverses = id;
            }
        }
        out.push(json!({"n": n, "same": same, "reverses": reverses}));
    }
    Value::Array(out).to_string()
}

fn oracle_draft(key: &Key, prompt: &str) -> String {
    let mut out = Vec::new();
    for item in block(prompt, "lessons") {
        let n = item.get("n").cloned().unwrap_or(Value::Null);
        let sessions = item.get("sessions").and_then(Value::as_u64).unwrap_or(0);
        let rule = item
            .get("evidence")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|e| e.get("quote").and_then(Value::as_str))
            .find_map(|q| key.rule_of(q));
        match rule {
            Some(r) if sessions >= 2 => out.push(json!({
                "n": n, "lesson": true, "title": detect::one_line(&r.rule, 80),
                "rule": r.rule, "scope": r.scope, "note": null, "section": null,
            })),
            _ => out.push(json!({"n": n, "lesson": false})),
        }
    }
    Value::Array(out).to_string()
}

// ---------------------------------------------------------------------------
// The run

/// One mode's figures.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Figures {
    pub captured: usize,
    pub rules: usize,
    pub delivered: usize,
    pub needed: usize,
    pub stale: usize,
}

/// The suite's result.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RememberResult {
    /// 0.7.2's behaviour, reproduced.
    pub before: Figures,
    /// This release.
    pub after: Figures,
    /// This release with a Rules budget that holds one rule, so later
    /// rules reach a session only on a new topic.
    pub tight: Figures,
    /// Model calls this release's backfill made.
    pub calls: usize,
    /// One line per (session, rule) pair and per rule, for the report.
    pub lines: Vec<String>,
}

impl RememberResult {
    pub fn summary(&self) -> String {
        let pct = |a: usize, b: usize| {
            if b == 0 {
                "–".to_owned()
            } else {
                format!("{a}/{b} ({:.0} %)", a as f64 * 100.0 / b as f64)
            }
        };
        let mut out = String::from("| | 0.7.2 (reproduced) | this release |\n|---|---|---|\n");
        out.push_str(&format!(
            "| capture | {} | {} |\n| delivery | {} | {} |\n| stale rules given | {} | {} |\n",
            pct(self.before.captured, self.before.rules),
            pct(self.after.captured, self.after.rules),
            pct(self.before.delivered, self.before.needed),
            pct(self.after.delivered, self.after.needed),
            self.before.stale,
            self.after.stale,
        ));
        out.push_str(&format!(
            "\nWith a Rules budget of {TIGHT_BUDGET} bytes (one rule in the start block), this release delivers {}: the rest comes with a new topic or not at all.\n",
            pct(self.tight.delivered, self.tight.needed),
        ));
        out.push_str(&format!(
            "\nModel calls of this release's backfill on the set: {}.\n",
            self.calls
        ));
        for l in &self.lines {
            out.push_str(&format!("\n- {l}"));
        }
        out.push('\n');
        out
    }
}

/// The tight Rules budget of [`RememberResult::tight`].
pub const TIGHT_BUDGET: usize = 140;

fn day(now: OffsetDateTime, days_ago: i64) -> OffsetDateTime {
    now - Duration::days(days_ago)
}

/// Writes the planted sessions into `index`.
fn plant(index: &mut Index, key: &Key, now: OffsetDateTime) -> Result<()> {
    for s in &key.sessions {
        let start = day(now, s.days_ago);
        let turns: Vec<Turn> = s
            .turns
            .iter()
            .enumerate()
            .map(|(k, t)| {
                let (role, tool, text) = match t.role.as_str() {
                    "bash" => (
                        Role::Tool,
                        Some("Bash".to_owned()),
                        format!("Bash {}", json!({ "command": t.text })),
                    ),
                    "assistant" => (Role::Assistant, None, t.text.clone()),
                    _ => (Role::User, None, t.text.clone()),
                };
                Turn::from_raw(RawTurn {
                    ordinal: k as u32,
                    role,
                    ts: if s.replayed {
                        start
                    } else {
                        start + Duration::minutes(k as i64)
                    },
                    tool_name: tool,
                    text,
                })
            })
            .collect();
        let path = format!("/remember/{}.jsonl", s.id);
        index.replace_session(
            &SessionSource {
                id: &s.id,
                tool: Tool::ClaudeCode,
                transcript_path: Path::new(&path),
                file_hash: &s.id,
            },
            &turns,
        )?;
    }
    Ok(())
}

/// The planted rule a lesson holds: its rule text, or a memory line.
fn planted_of<'k>(key: &'k Key, l: &lessons::Lesson) -> Option<&'k PlantedRule> {
    key.rule_of(&l.rule)
}

/// Runs the suite: the key at `key_path`, the notes of `notes_dir`, and the
/// oracle run as `oracle_argv` (its `{system_prompt}` element replaced).
pub fn run(
    key_path: &Path,
    notes_dir: &Path,
    entry_note: &str,
    oracle_argv: Vec<String>,
) -> Result<RememberResult> {
    let key = Key::load(key_path)?;
    let now = OffsetDateTime::now_utc();
    let tmp = tempfile::Builder::new()
        .prefix("hippo-remember-")
        .tempdir()?;
    let reversed = key.reversed();
    let mut out = RememberResult::default();

    // This release.
    let home = tmp.path().join("home");
    let workspace = tmp.path().join("workspace");
    let user_home = tmp.path().join("user");
    let transcripts = tmp.path().join("transcripts");
    fs::create_dir_all(&transcripts)?;
    copy_dir(notes_dir, &workspace.join("notes"))?;
    let models = super::Models {
        model_command: oracle_argv.clone(),
        reviewer_command_override: Some(oracle_argv),
    };
    let (mut cfg, ws) = super::scratch(&home, &workspace, &transcripts, entry_note, Some(&models))?;
    for m in &key.memory {
        let p = user_home.join(&m.path);
        if let Some(d) = p.parent() {
            fs::create_dir_all(d)?;
        }
        fs::write(&p, &m.text)?;
        cfg.remember.memory_sources.push(m.source.clone());
    }
    let mut index = Index::open(&cfg.home, true)?;
    plant(&mut index, &key, now)?;
    crate::rollup::refresh(&cfg.home, Some(&ws), &index)?;
    let scanned = lessons::scan_with(
        &cfg,
        &ws,
        &lessons::ScanOptions {
            days: 30,
            dry_run: false,
            backfill: true,
            max_calls: 200,
            places: memory::Places {
                user_home: Some(user_home.clone()),
                claude_config_dir: None,
                codex_home: None,
            },
        },
    )?;
    out.calls = scanned.state.model_calls;
    // The person keeps every pending rule, oldest first.
    let mut pending = lessons::pending(&cfg.home)?;
    pending.sort_by_key(|l| l.created);
    for l in pending {
        if let Err(err) = lessons::keep_rule(&cfg, Some(&ws), l.id, None, None, false, now) {
            out.lines
                .push(format!("this release: keeping {} failed: {err}", l.title));
        }
    }
    let all = lessons::list(&cfg.home)?;
    let mut captured: BTreeSet<&str> = BTreeSet::new();
    for l in &all {
        let live = matches!(l.status, Status::Pending | Status::Kept);
        if let (true, Some(r)) = (live, planted_of(&key, l)) {
            captured.insert(r.id.as_str());
        }
    }
    let lesson_rule: BTreeMap<Ulid, &str> = all
        .iter()
        .filter_map(|l| Some((l.id, planted_of(&key, l)?.id.as_str())))
        .collect();
    let counted: Vec<&PlantedRule> = key
        .rules
        .iter()
        .filter(|r| !reversed.contains(r.id.as_str()))
        .collect();
    out.after.rules = counted.len();
    out.after.captured = counted
        .iter()
        .filter(|r| captured.contains(r.id.as_str()))
        .count();
    for r in &counted {
        out.lines.push(format!(
            "{}: this release {}",
            r.id,
            if captured.contains(r.id.as_str()) {
                "captured"
            } else {
                "missed"
            }
        ));
    }
    // Delivery: what each session that needs a rule is given.
    let kept = lessons::kept_rules(&cfg.home, Some(&ws));
    for s in key.sessions.iter().filter(|s| !s.needs.is_empty()) {
        let given = given_now(&cfg, &ws, &kept, s)?;
        let given: BTreeSet<&str> = given
            .iter()
            .filter_map(|id| lesson_rule.get(id).copied())
            .collect();
        for need in &s.needs {
            out.after.needed += 1;
            if given.contains(need.as_str()) {
                out.after.delivered += 1;
            }
        }
        out.after.stale += given.iter().filter(|g| reversed.contains(*g)).count();
        out.lines.push(format!(
            "{}: this release gave {:?}, needed {:?}",
            s.id, given, s.needs
        ));
    }

    // Again with one rule's room in the start block.
    let mut tight = cfg.clone();
    tight.remember.rules_budget_bytes = Some(TIGHT_BUDGET);
    for s in key.sessions.iter().filter(|s| !s.needs.is_empty()) {
        let renamed = PlantedSession {
            id: format!("{}-tight", s.id),
            ..s.clone()
        };
        let given = given_now(&tight, &ws, &kept, &renamed)?;
        let given: BTreeSet<&str> = given
            .iter()
            .filter_map(|id| lesson_rule.get(id).copied())
            .collect();
        for need in &s.needs {
            out.tight.needed += 1;
            if given.contains(need.as_str()) {
                out.tight.delivered += 1;
            }
        }
        out.tight.stale += given.iter().filter(|g| reversed.contains(*g)).count();
        out.lines.push(format!(
            "{}: this release, tight budget, gave {:?}, needed {:?}",
            s.id, given, s.needs
        ));
    }

    // 0.7.2's behaviour, reproduced.
    let before = before(&key, &cfg, &index, notes_dir, entry_note, tmp.path(), now)?;
    out.before = before.0;
    out.lines.extend(before.1);
    Ok(out)
}

/// The rule ids session `s` is given now: its start block, and for a
/// session with later prompts, what each new topic brings.
fn given_now(
    cfg: &crate::config::Config,
    ws: &Workspace,
    kept: &[lessons::Rule],
    s: &PlantedSession,
) -> Result<Vec<Ulid>> {
    if s.outside {
        let refs: Vec<&lessons::Rule> = kept.iter().filter(|r| r.machine).collect();
        return Ok(context::rules_text(&refs, cfg.remember.rules_budget()).1);
    }
    let mut compiled = context::compile(
        ws,
        None,
        None,
        None,
        None,
        ws.budget_bytes,
        PartSelection::Required,
    )?;
    let mut given = compiled.add_rules(kept, true, cfg.remember.rules_budget(), ws.budget_bytes);
    let mut state = topic::Topic {
        rules: given.clone(),
        ..topic::Topic::default()
    };
    let prompts: Vec<&str> = s
        .turns
        .iter()
        .filter(|t| t.role == "user")
        .map(|t| t.text.as_str())
        .collect();
    if let Some(first) = prompts.first() {
        state.changed(&detect::key_terms(first), cfg.remember.reinject_every());
        state.save(&cfg.home, &s.id)?;
        for p in &prompts[1..] {
            if let Some(t) = topic::next(cfg, ws, &s.id, p)? {
                given.extend(t.rules);
            }
        }
    }
    Ok(given)
}

/// 0.7.2 reproduced: nightly scans of 7-day windows clustering by key
/// terms; a cluster drafted (by the same oracle) when it spans two
/// sessions; no memories, no reversal; a kept rule a bullet in a note of its
/// own, given only when that note ranks among the first prompt's relevant
/// notes, and only inside the workspace.
fn before(
    key: &Key,
    cfg: &crate::config::Config,
    index: &Index,
    notes_dir: &Path,
    entry_note: &str,
    tmp: &Path,
    now: OffsetDateTime,
) -> Result<(Figures, Vec<String>)> {
    let rollup = crate::rollup::Rollup::open(&cfg.home)?.ok_or(Error::MissingIndex)?;
    let (_, items) = detect::corrections(index, &rollup, OffsetDateTime::UNIX_EPOCH)?;
    let oldest = key.sessions.iter().map(|s| s.days_ago).max().unwrap_or(0);
    let mut captured: BTreeSet<String> = BTreeSet::new();
    for d in (0..=oldest).rev() {
        let end = day(now, d);
        let start = end - Duration::days(7);
        let window: Vec<detect::Item> = items
            .iter()
            .filter(|(e, _)| e.at > start && e.at <= end)
            .cloned()
            .collect();
        for c in detect::cluster_by_terms(window, time::UtcOffset::UTC) {
            if c.sessions.len() < 2 {
                continue;
            }
            if let Some(r) = c.evidence.iter().find_map(|e| key.rule_of(&e.quote)) {
                captured.insert(r.id.clone());
            }
        }
    }
    let reversed = key.reversed();
    let mut f = Figures::default();
    let mut lines = Vec::new();
    let counted: Vec<&PlantedRule> = key
        .rules
        .iter()
        .filter(|r| !reversed.contains(r.id.as_str()))
        .collect();
    f.rules = counted.len();
    f.captured = counted.iter().filter(|r| captured.contains(&r.id)).count();
    for r in &counted {
        lines.push(format!(
            "{}: 0.7.2 {}",
            r.id,
            if captured.contains(&r.id) {
                "captured"
            } else {
                "missed"
            }
        ));
    }
    // Each captured rule kept as a bullet in a note of its own.
    let home = tmp.join("before-home");
    let workspace = tmp.join("before-workspace");
    let transcripts = tmp.join("before-transcripts");
    fs::create_dir_all(&transcripts)?;
    copy_dir(notes_dir, &workspace.join("notes"))?;
    let (_, ws) = super::scratch(&home, &workspace, &transcripts, entry_note, None)?;
    // By file name: the relevant part names notes by their full path.
    let mut note_of: BTreeMap<std::ffi::OsString, String> = BTreeMap::new();
    for r in key.rules.iter().filter(|r| captured.contains(&r.id)) {
        let title = detect::one_line(&r.rule, 60);
        let path = lessons::new_note_path(&ws, &title);
        let full = ws.root.join(&path);
        if let Some(d) = full.parent() {
            fs::create_dir_all(d)?;
        }
        fs::write(
            &full,
            format!(
                "---\ntype: policy\nstatus: current\n---\n\n# {title}\n\n{}\n",
                lessons::bullet(&title, &r.rule, now.date())
            ),
        )?;
        if let Some(name) = path.file_name() {
            note_of.insert(name.to_owned(), r.id.clone());
        }
    }
    for s in key.sessions.iter().filter(|s| !s.needs.is_empty()) {
        let mut given: BTreeSet<String> = BTreeSet::new();
        let first = s
            .turns
            .iter()
            .find(|t| t.role == "user")
            .map(|t| t.text.as_str());
        if let (false, Some(first)) = (s.outside, first) {
            let compiled = context::compile(
                &ws,
                None,
                Some(first),
                None,
                None,
                ws.budget_bytes,
                PartSelection::Optional,
            )?;
            for p in &compiled.relevant {
                if let Some(id) = p.file_name().and_then(|n| note_of.get(n)) {
                    given.insert(id.clone());
                }
            }
        }
        for need in &s.needs {
            f.needed += 1;
            if given.contains(need) {
                f.delivered += 1;
            }
        }
        f.stale += given
            .iter()
            .filter(|g| reversed.contains(g.as_str()))
            .count();
        lines.push(format!(
            "{}: 0.7.2 gave {:?}, needed {:?}",
            s.id, given, s.needs
        ));
    }
    Ok((f, lines))
}
