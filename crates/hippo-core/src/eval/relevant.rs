//! `hippo eval relevant`: how well the relevant notes match what agents
//! went on to use, measured on the user's own sessions. Read-only; nothing it reads leaves the machine.
//!
//! For each session that started in the last `days` (top-level, person
//! started; sub-agents too with `subagents`), the ground truth is the set
//! of notes the session opened, discussed or edited (the rollup's notes),
//! less the notes the relevant part never picks (entry,
//! required, session log) and notes that no longer exist. A session with
//! no such note is left out. Its task is its first request: the declared
//! record's `task`, else its first user turn's request text from the
//! index. Its repository is the one its working directory lies in.
//!
//! Each case is ranked by the current ranking and by the earlier
//! ranking ([`rank::rank_v1`]) and scored at five: recall (the share
//! of a session's notes listed, averaged over sessions), precision (listed
//! notes that were used, over all listed), MRR (one over the rank of the
//! first used note) and hit rate (sessions with at least one used note
//! listed). The most recent `holdout` share of sessions is reported apart,
//! so that tuning on the rest can be checked on sessions it did not see.
//!
//! The ground truth is a proxy: an agent may open a note for a reason
//! unrelated to its first request, and the block it was given (the old
//! ranking's picks among it) may have steered what it opened.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;
use time::format_description::well_known::Rfc3339;
use time::{Duration, OffsetDateTime};

use crate::config::Workspace;
use crate::context::{self, rank};
use crate::error::Result;
use crate::index::Index;
use crate::notes::{normalise, Notes};
use crate::rollup::Rollup;
use crate::{session_meta, sessions};

/// How many of a session's first user turns are searched for its request.
const REQUEST_TURNS: usize = 60;

/// A note used by more than this share of the tuning sessions is a hub:
/// the "without hubs" scores leave it out of what sessions expect, so they
/// measure task-specific picks.
pub const HUB_SHARE: f64 = 0.1;

/// The five notes most used in `used`, most first, then by path.
fn popular(used: &[Vec<PathBuf>]) -> Vec<PathBuf> {
    let mut uses: HashMap<&PathBuf, usize> = HashMap::new();
    for p in used.iter().flatten() {
        *uses.entry(p).or_default() += 1;
    }
    let mut ranked: Vec<(&PathBuf, usize)> = uses.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    ranked
        .into_iter()
        .take(rank::LIMIT)
        .map(|(p, _)| p.clone())
        .collect()
}

/// What to evaluate.
#[derive(Debug, Clone)]
pub struct Options {
    pub days: i64,
    /// The most recent share of sessions held out, 0 to 1.
    pub holdout: f64,
    pub subagents: bool,
    pub params: rank::Params,
    pub now: OffsetDateTime,
}

/// One session.
#[derive(Debug, Clone, Serialize)]
pub struct Case {
    pub session: String,
    pub started: String,
    pub held_out: bool,
    pub subagent: bool,
    pub task: String,
    pub repo: Option<String>,
    /// The notes it used, relative to the workspace.
    pub expected: Vec<PathBuf>,
    /// The baseline's picks.
    pub v1: Vec<PathBuf>,
    /// The current ranking's picks, and why.
    pub picked: Vec<PathBuf>,
    pub why: Vec<String>,
    /// The reference: the notes the earlier sessions used most.
    pub popular: Vec<PathBuf>,
}

/// Scores over a set of cases.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Metrics {
    pub sessions: usize,
    pub recall: f64,
    pub precision: f64,
    pub mrr: f64,
    pub hit_rate: f64,
    /// Notes listed per session, on average.
    pub listed: f64,
    /// Sessions with nothing listed.
    pub empty: usize,
}

impl Metrics {
    /// Scores `(expected, listed)` pairs.
    pub fn of<'a>(cases: impl Iterator<Item = (&'a [PathBuf], &'a [PathBuf])>) -> Metrics {
        let mut m = Metrics::default();
        let (mut hits, mut listed) = (0usize, 0usize);
        for (expected, picked) in cases {
            m.sessions += 1;
            let found = picked.iter().filter(|p| expected.contains(p)).count();
            hits += found;
            listed += picked.len();
            if !expected.is_empty() {
                m.recall += found as f64 / expected.len() as f64;
            }
            if let Some(i) = picked.iter().position(|p| expected.contains(p)) {
                m.mrr += 1.0 / (i + 1) as f64;
                m.hit_rate += 1.0;
            }
            if picked.is_empty() {
                m.empty += 1;
            }
        }
        if m.sessions > 0 {
            let n = m.sessions as f64;
            m.recall /= n;
            m.mrr /= n;
            m.hit_rate /= n;
            m.listed = listed as f64 / n;
        }
        if listed > 0 {
            m.precision = hits as f64 / listed as f64;
        }
        m
    }

    /// `recall@5 0.21  precision@5 0.18  mrr 0.30  hit 0.41  listed 4.2  empty 3`.
    pub fn line(&self) -> String {
        format!(
            "recall@5 {:.3}  precision@5 {:.3}  mrr {:.3}  hit {:.3}  listed {:.2}  empty {}",
            self.recall, self.precision, self.mrr, self.hit_rate, self.listed, self.empty
        )
    }
}

/// The baseline and the current ranking over one set of cases.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Pair {
    pub v1: Metrics,
    pub current: Metrics,
    /// A reference, not a ranking: the five notes most used by the
    /// sessions before each one, whatever its task.
    pub popular: Metrics,
}

impl Pair {
    /// Scores `cases`, each expecting its notes less `hubs`; a case left
    /// expecting nothing is not counted.
    fn of<'a>(cases: impl Iterator<Item = &'a Case>, hubs: &[PathBuf]) -> Pair {
        let kept: Vec<(&Case, Vec<PathBuf>)> = cases
            .map(|c| {
                let expected = c
                    .expected
                    .iter()
                    .filter(|p| !hubs.contains(p))
                    .cloned()
                    .collect();
                (c, expected)
            })
            .filter(|(_, e): &(&Case, Vec<PathBuf>)| !e.is_empty())
            .collect();
        let of = |pick: fn(&Case) -> &[PathBuf]| {
            Metrics::of(kept.iter().map(|(c, e)| (&e[..], pick(c))))
        };
        Pair {
            v1: of(|c| &c.v1),
            current: of(|c| &c.picked),
            popular: of(|c| &c.popular),
        }
    }
}

/// The eval's outcome.
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub days: i64,
    pub holdout: f64,
    pub subagents: bool,
    pub params: rank::Params,
    pub all: Pair,
    pub tuning: Pair,
    pub held_out: Pair,
    /// Notes used by more than [`HUB_SHARE`] of the tuning sessions.
    pub hubs: Vec<PathBuf>,
    /// The same, with the hubs taken out of what each session expects.
    pub all_specific: Pair,
    pub tuning_specific: Pair,
    pub held_out_specific: Pair,
    pub cases: Vec<Case>,
}

impl Report {
    /// The aggregate numbers, without any task text or note name.
    pub fn render(&self) -> String {
        let mut out = format!(
            "relevant notes, last {} days, {} sessions ({} held out){}\n",
            self.days,
            self.all.current.sessions,
            self.held_out.current.sessions,
            if self.subagents {
                ", sub-agents included"
            } else {
                ""
            }
        );
        let hubs = format!("without the {} hub notes", self.hubs.len());
        for (name, pair) in [
            ("all".to_owned(), &self.all),
            ("tuning".to_owned(), &self.tuning),
            ("held out".to_owned(), &self.held_out),
            (format!("all, {hubs}"), &self.all_specific),
            (format!("tuning, {hubs}"), &self.tuning_specific),
            (format!("held out, {hubs}"), &self.held_out_specific),
        ] {
            out.push_str(&format!(
                "{name} ({} sessions):\n  v1       {}\n  current  {}\n  popular  {}\n",
                pair.current.sessions,
                pair.v1.line(),
                pair.current.line(),
                pair.popular.line()
            ));
        }
        out
    }
}

/// Builds the cases and scores them.
pub fn run(ws: &Workspace, index: &Index, rollup: &Rollup, opts: &Options) -> Result<Report> {
    let notes = Notes::scan(&ws.notes_root())?;
    let notes_dir = normalise(&ws.notes_dir);
    let skipped = context::skipped(ws, &notes);
    let start = opts.now - Duration::days(opts.days);
    let mut rows = rollup.sessions_in(start, opts.now)?;
    rows.retain(|r| {
        let f = &r.facts;
        f.first_turn.is_some_and(|t| t >= start)
            && !f.automated
            && (opts.subagents || f.parent.is_none())
    });
    rows.sort_by(|a, b| {
        a.facts
            .first_turn
            .cmp(&b.facts.first_turn)
            .then_with(|| a.facts.id.cmp(&b.facts.id))
    });

    let mut cases = Vec::new();
    // The notes each earlier case used, for the popularity reference.
    let mut used: Vec<Vec<PathBuf>> = Vec::new();
    for row in rows {
        let f = &row.facts;
        let mut expected: Vec<PathBuf> = Vec::new();
        for seen in rollup.notes_of(&f.id)? {
            let note = normalise(&seen.note);
            let Ok(key) = note.strip_prefix(&notes_dir) else {
                continue;
            };
            if notes.notes.contains_key(key)
                && !skipped.iter().any(|s| s == key)
                && !expected.contains(&note)
            {
                expected.push(note);
            }
        }
        if expected.is_empty() {
            continue;
        }
        let Some(task) = first_request(ws, index, &f.id)? else {
            continue;
        };
        let cwd = match f.tool.as_str() {
            "codex" => crate::dream::observed::codex_cwd(&f.transcript_path),
            "dsh" => crate::adapters::dsh::header(&f.transcript_path).and_then(|h| h.cwd),
            "opencode" => {
                crate::adapters::opencode::info(&f.transcript_path).and_then(|i| i.directory)
            }
            "muse" => crate::adapters::muse::workspace_root(&f.transcript_path),
            _ => transcript_cwd(&f.transcript_path),
        };
        let repo = cwd.and_then(|dir| rank::repo_name(&dir, &ws.root));
        let to_ws = |key: &Path| normalise(&ws.notes_dir.join(key));
        let v1 = rank::rank_v1(&notes, &task)
            .iter()
            .map(|k| to_ws(k))
            .collect();
        let picks = rank::rank(&notes, &task, repo.as_deref(), &skipped, &opts.params);
        cases.push(Case {
            session: f.id.clone(),
            started: f
                .first_turn
                .and_then(|t| t.format(&Rfc3339).ok())
                .unwrap_or_default(),
            held_out: false,
            subagent: f.parent.is_some(),
            picked: picks.iter().map(|p| to_ws(&p.path)).collect(),
            why: picks.iter().map(|p| p.reason(repo.as_deref())).collect(),
            popular: popular(&used),
            task,
            repo,
            expected: expected.clone(),
            v1,
        });
        used.push(expected);
    }
    let held = (cases.len() as f64 * opts.holdout.clamp(0.0, 1.0)).round() as usize;
    let first_held = cases.len() - held;
    for c in &mut cases[first_held..] {
        c.held_out = true;
    }
    let tuning: Vec<&Case> = cases.iter().filter(|c| !c.held_out).collect();
    let mut uses: HashMap<&PathBuf, usize> = HashMap::new();
    for c in &tuning {
        for p in &c.expected {
            *uses.entry(p).or_default() += 1;
        }
    }
    let mut hubs: Vec<PathBuf> = uses
        .into_iter()
        .filter(|(_, n)| *n as f64 > HUB_SHARE * tuning.len() as f64)
        .map(|(p, _)| p.clone())
        .collect();
    hubs.sort();
    let held = |c: &&Case| c.held_out;
    let tune = |c: &&Case| !c.held_out;
    Ok(Report {
        days: opts.days,
        holdout: opts.holdout,
        subagents: opts.subagents,
        params: opts.params.clone(),
        all: Pair::of(cases.iter(), &[]),
        tuning: Pair::of(cases.iter().filter(tune), &[]),
        held_out: Pair::of(cases.iter().filter(held), &[]),
        all_specific: Pair::of(cases.iter(), &hubs),
        tuning_specific: Pair::of(cases.iter().filter(tune), &hubs),
        held_out_specific: Pair::of(cases.iter().filter(held), &hubs),
        hubs,
        cases,
    })
}

/// A session's first request: its declared record's task, else the
/// request text of its first user turns in the index.
fn first_request(ws: &Workspace, index: &Index, id: &str) -> Result<Option<String>> {
    if sessions::is_valid_id(id) {
        if let Some(record) = sessions::get(ws, id)? {
            if !record.derived && !record.task.trim().is_empty() {
                return Ok(Some(record.task));
            }
        }
    }
    Ok(index
        .turns_for_session(id)?
        .iter()
        .filter(|t| t.role == "user")
        .take(REQUEST_TURNS)
        .find_map(|t| session_meta::request_text(&t.text)))
}

/// The `cwd` a Claude Code transcript's first records name.
fn transcript_cwd(path: &Path) -> Option<PathBuf> {
    let file = File::open(path).ok()?;
    for line in BufReader::new(file).lines().take(50) {
        let line = line.ok()?;
        if !line.contains("\"cwd\"") {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<Value>(&line) {
            if let Some(cwd) = v.get("cwd").and_then(Value::as_str) {
                return Some(PathBuf::from(cwd));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn metrics_score_recall_precision_and_rank() {
        let a = [p("a"), p("b")];
        let a_picked = [p("x"), p("a")];
        let b = [p("c")];
        let b_picked: Vec<PathBuf> = Vec::new();
        let m = Metrics::of([(&a[..], &a_picked[..]), (&b[..], &b_picked[..])].into_iter());
        assert_eq!(m.sessions, 2);
        assert!((m.recall - 0.25).abs() < 1e-9); // (1/2 + 0) / 2
        assert!((m.precision - 0.5).abs() < 1e-9); // 1 of 2 listed
        assert!((m.mrr - 0.25).abs() < 1e-9); // (1/2 + 0) / 2
        assert!((m.hit_rate - 0.5).abs() < 1e-9);
        assert!((m.listed - 1.0).abs() < 1e-9);
        assert_eq!(m.empty, 1);
    }

    #[test]
    fn a_transcript_names_its_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        std::fs::write(
            &path,
            "{\"type\":\"summary\"}\n{\"type\":\"user\",\"cwd\":\"/work/tern\"}\n",
        )
        .unwrap();
        assert_eq!(transcript_cwd(&path), Some(p("/work/tern")));
    }
}
