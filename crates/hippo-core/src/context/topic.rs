//! What a long session was given, and when its topic changes
//! (`docs/designs/remember.md`, "Topic change").
//!
//! `$HIPPO_HOME/hook-state/<session>.json` holds, per session, the prompt
//! count, the key terms of the prompts since the last injection, and the
//! notes and rules already given. [`Topic::changed`] decides from the
//! prompt's terms alone, so a prompt that is not a new topic costs no notes
//! scan. `$HIPPO_HOME/logs/injections.jsonl` gets one line per hook output.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use ulid::Ulid;

use crate::error::{Error, Result};
use crate::util::write_atomic;

/// The fewest key terms a prompt needs to be a new topic.
pub const MIN_TERMS: usize = 3;
/// A new topic shares less than this Jaccard index with the terms since
/// the last injection.
pub const MAX_OVERLAP: f64 = 0.2;
/// State files older than this many days are removed at a session start.
pub const KEEP_DAYS: i64 = 14;

/// One session's hook state.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Topic {
    pub prompts: usize,
    /// Prompts since the last injection (the first prompt's counts).
    pub since: usize,
    /// Key terms of the prompts since the last injection.
    pub terms: BTreeSet<String>,
    /// Notes already given, relative to the workspace.
    pub notes: Vec<PathBuf>,
    /// Rules already given.
    pub rules: Vec<Ulid>,
}

/// `$HIPPO_HOME/hook-state/`.
pub fn dir(home: &Path) -> PathBuf {
    home.join("hook-state")
}

fn path(home: &Path, session: &str) -> PathBuf {
    // Session ids are `[A-Za-z0-9_-]`; anything else is replaced.
    let safe: String = session
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    dir(home).join(format!("{safe}.json"))
}

impl Topic {
    /// The session's state; `None` when the hooks never started it.
    pub fn load(home: &Path, session: &str) -> Option<Topic> {
        let bytes = fs::read(path(home, session)).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    pub fn save(&self, home: &Path, session: &str) -> Result<()> {
        let d = dir(home);
        fs::create_dir_all(&d).map_err(|e| Error::io_at(&d, e))?;
        write_atomic(&path(home, session), &serde_json::to_vec(self)?)
    }

    /// Counts a prompt with `terms` and says whether it is a new topic:
    /// at least [`MIN_TERMS`] terms, at least `every` prompts since the
    /// last injection, and an overlap under [`MAX_OVERLAP`] with the terms
    /// since then. When it is not, its terms join those.
    pub fn changed(&mut self, terms: &[String], every: usize) -> bool {
        self.prompts += 1;
        self.since += 1;
        let set: BTreeSet<String> = terms.iter().cloned().collect();
        let shared = set.intersection(&self.terms).count();
        let union = set.union(&self.terms).count();
        let overlap = if union == 0 {
            1.0
        } else {
            shared as f64 / union as f64
        };
        let new = set.len() >= MIN_TERMS && self.since > every && overlap < MAX_OVERLAP;
        if new {
            self.since = 0;
            self.terms = set;
        } else {
            self.terms.extend(set);
        }
        new
    }
}

/// Removes state files not changed for [`KEEP_DAYS`] days. Errors are
/// ignored: this runs inside a hook.
pub fn sweep(home: &Path, now: std::time::SystemTime) {
    let Ok(entries) = fs::read_dir(dir(home)) else {
        return;
    };
    let limit = std::time::Duration::from_secs(u64::try_from(KEEP_DAYS).unwrap_or(14) * 86_400);
    for e in entries.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|age| age > limit);
        if old {
            let _ = fs::remove_file(e.path());
        }
    }
}

/// What a later prompt on a new topic gets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Topical {
    pub text: String,
    pub rules: Vec<Ulid>,
    pub notes: Vec<PathBuf>,
}

/// A later prompt of `session`: when its topic is new ([`Topic::changed`]),
/// the kept rules that share its words and were not given yet (at most 3)
/// and the relevant notes not given yet, within the workspace budget.
/// `None` when the topic is the same, the session has no state, or there is
/// nothing new. The state is saved either way.
pub fn next(
    cfg: &crate::config::Config,
    ws: &crate::config::Workspace,
    session: &str,
    prompt: &str,
) -> Result<Option<Topical>> {
    let Some(mut state) = Topic::load(&cfg.home, session) else {
        return Ok(None);
    };
    let terms = crate::lessons::detect::key_terms(prompt);
    if !state.changed(&terms, cfg.remember.reinject_every()) {
        state.save(&cfg.home, session)?;
        return Ok(None);
    }
    let compiled = super::compile(
        ws,
        None,
        Some(prompt),
        None,
        None,
        ws.budget_bytes,
        super::PartSelection::Optional,
    )?;
    let notes = super::new_relevant_lines(&compiled, &state.notes);
    let rules: Vec<crate::lessons::Rule> = crate::lessons::kept_rules(&cfg.home, Some(ws))
        .into_iter()
        .filter(|r| !state.rules.contains(&r.id))
        .filter(|r| r.terms.iter().any(|t| terms.contains(t)))
        .take(3)
        .collect();
    if notes.is_empty() && rules.is_empty() {
        state.save(&cfg.home, session)?;
        return Ok(None);
    }
    let budget = ws.budget_bytes;
    let mut text = String::from("For the new topic:");
    let mut given_rules = Vec::new();
    for r in &rules {
        let line = format!("\n- {}", r.text);
        if text.len() + line.len() < budget {
            text.push_str(&line);
            given_rules.push(r.id);
        }
    }
    let mut given_notes = Vec::new();
    if !notes.is_empty() && text.len() + 10 < budget {
        text.push_str("\nrelevant:");
        for (path, line) in notes {
            if text.len() + line.len() + 1 < budget {
                text.push('\n');
                text.push_str(&line);
                given_notes.push(path);
            }
        }
    }
    text.push('\n');
    state.rules.extend(given_rules.iter().copied());
    state.notes.extend(given_notes.iter().cloned());
    state.save(&cfg.home, session)?;
    Ok(Some(Topical {
        text,
        rules: given_rules,
        notes: given_notes,
    }))
}

/// One hook output, as `injections.jsonl` records it: never the prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Injection {
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    pub session: String,
    /// `claude-code`, `codex`, `gemini`, `cursor`, `plugin`.
    pub agent: String,
    /// `start`, `rules-only`, `prompt` or `topic`.
    pub event: String,
    #[serde(default)]
    pub rules: Vec<Ulid>,
    #[serde(default)]
    pub notes: Vec<PathBuf>,
    pub bytes: usize,
}

/// `$HIPPO_HOME/logs/injections.jsonl`.
pub fn log_path(home: &Path) -> PathBuf {
    home.join("logs").join("injections.jsonl")
}

/// Appends `i` to the delivery log.
pub fn log(home: &Path, i: &Injection) -> Result<()> {
    use std::io::Write as _;
    let p = log_path(home);
    if let Some(d) = p.parent() {
        fs::create_dir_all(d).map_err(|e| Error::io_at(d, e))?;
    }
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&p)
        .map_err(|e| Error::io_at(&p, e))?;
    writeln!(f, "{}", serde_json::to_string(i)?).map_err(|e| Error::io_at(&p, e))
}

/// Every line of the delivery log that reads; a missing log is none.
pub fn read_log(home: &Path) -> Vec<Injection> {
    fs::read_to_string(log_path(home))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| (*w).to_owned()).collect()
    }

    #[test]
    fn a_new_topic_needs_enough_words_enough_prompts_and_little_overlap() {
        let mut s = Topic::default();
        s.changed(&t(&["deploy", "staging", "docker"]), 2);
        // Too soon after the last injection.
        assert!(!s.changed(&t(&["invoice", "pdf", "layout"]), 2));
        assert!(!s.changed(&t(&["fix"]), 2), "too few words");
        // Overlaps what was said since (the invoice words joined).
        assert!(!s.changed(&t(&["invoice", "pdf", "footer"]), 2));
        assert!(s.changed(&t(&["android", "build", "gradle"]), 2));
        assert_eq!(s.since, 0);
        assert_eq!(s.prompts, 5);
    }

    #[test]
    fn state_and_log_round_trip_and_old_state_is_swept() {
        let home = tempfile::tempdir().unwrap();
        let mut s = Topic::default();
        s.notes.push(PathBuf::from("notes/A.md"));
        s.save(home.path(), "codex-a/b").unwrap();
        assert_eq!(Topic::load(home.path(), "codex-a/b"), Some(s));
        assert!(dir(home.path()).join("codex-a_b.json").exists());
        sweep(
            home.path(),
            std::time::SystemTime::now() + std::time::Duration::from_secs(15 * 86_400),
        );
        assert_eq!(Topic::load(home.path(), "codex-a/b"), None);
        let i = Injection {
            at: OffsetDateTime::UNIX_EPOCH,
            session: "s1".into(),
            agent: "claude-code".into(),
            event: "start".into(),
            rules: vec![Ulid::nil()],
            notes: vec![],
            bytes: 10,
        };
        log(home.path(), &i).unwrap();
        log(home.path(), &i).unwrap();
        assert_eq!(read_log(home.path()), vec![i.clone(), i]);
    }
}
