//! The shared Inbox (`docs/designs/team-brain.md`): lesson identities and
//! decisions, shared through the workspace's git repository.
//!
//! With `[team] inbox = "shared"` in force (accepted on this machine,
//! `docs/designs/common.md` rule 3), a correction lesson whose every
//! evidence session ran inside the workspace or one of its worktrees is
//! written to `.hippo/inbox/L-<ulid>/`, one file per person and kind, once
//! the person has said `share_lessons = "rules"`:
//!
//! ```text
//! .hippo/inbox/L-01JC7Q…/
//!   lesson-alice.json          Alice's wording; written once
//!   seen-alice.json            Alice's counts; rewritten only by Alice
//!   decision-bob-01JE….json    Bob kept it
//!   same-as-L-01JD…-bob.json   Bob linked it to another lesson
//! ```
//!
//! Nobody edits a file another person wrote, and no file name depends on
//! the lesson's text. No quotes and no session ids are shared. A scan
//! compares each correction cluster's terms with every shared lesson's, in
//! code: a match with a decision is not drafted, a match without one only
//! counts. Kept bullets carry `<!-- hippo:lesson L-… -->`, which Obsidian
//! and GitHub hide.
//!
//! Names and verdicts written by others are labels, not identities: they
//! are shown as written, beside the git commit author of the file.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use ulid::Ulid;

use super::{detect, Kind, Lesson, Target};
use crate::accept::Inbox;
use crate::config::{Config, Workspace};
use crate::error::{Error, Result};
use crate::records;

/// `<workspace>/.hippo/inbox/`.
pub fn inbox_dir(ws: &Workspace) -> PathBuf {
    ws.hippo_dir().join("inbox")
}

/// Whether the shared Inbox is in force for this workspace on this
/// machine.
pub fn enabled(ws: &Workspace) -> bool {
    ws.team.inbox == Some(Inbox::Shared)
}

/// The directory name of the lesson with id `id`: `L-<id>`.
pub fn dir_name(id: Ulid) -> String {
    format!("L-{id}")
}

/// The marker a kept lesson's bullet ends with.
pub fn marker(dir: &str) -> String {
    format!("<!-- hippo:lesson {dir} -->")
}

/// The person's choice of what of theirs may be written to the shared
/// Inbox (`share_lessons` in `config.toml`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Share {
    Off,
    Rules,
}

impl Share {
    /// The setting as `config.toml` holds it; anything but `rules` is off
    /// (`rules+quotes` ships later).
    pub fn of(cfg: &Config) -> Share {
        match cfg.share_lessons.as_deref() {
            Some("rules") => Share::Rules,
            _ => Share::Off,
        }
    }
}

/// `lesson-<member>.json`: one person's wording of a lesson.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wording {
    pub kind: Kind,
    /// The terms a scan matches against.
    pub terms: Vec<String>,
    pub title: String,
    pub rule: String,
    #[serde(default)]
    pub target: Option<Target>,
    /// The reviewer's verdict, as the writer's hippo recorded it.
    pub verdict: String,
    /// The runner that reviewed, by name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner: Option<String>,
}

/// `seen-<member>.json`: one person's counts. No quotes, no session ids.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    pub sessions: usize,
    #[serde(with = "time::serde::rfc3339")]
    pub first: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub last: OffsetDateTime,
    pub most_in_a_day: usize,
    pub agents: Vec<String>,
}

/// `keep` or `dismiss`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Keep,
    Dismiss,
}

/// `decision-<member>-<ulid>.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    pub verdict: Verdict,
    pub by: String,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt: Option<Ulid>,
    #[serde(default)]
    pub edited: bool,
    /// For a dismissal: `wrong`, `duplicate` or `not-for-us`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The sessions everyone had seen when it was dismissed: it reopens
    /// after [`super::REOPEN_AFTER`] more.
    #[serde(default)]
    pub sessions_at: usize,
}

/// `same-as-<other>-<member>.json`: this lesson is `other`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SameAs {
    pub other: String,
    pub by: String,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
}

/// A file someone wrote, with what it says about its writer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written<T> {
    /// The member named in the file name.
    pub member: String,
    pub record: T,
    /// The `hippo_version` the file carries.
    pub hippo_version: Option<String>,
    pub path: PathBuf,
}

/// One shared lesson directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shared {
    /// `L-<ulid>`.
    pub dir: String,
    pub path: PathBuf,
    /// By file name order: the first is the one shown.
    pub wordings: Vec<Written<Wording>>,
    pub seen: Vec<Written<Counts>>,
    pub decisions: Vec<Written<Decision>>,
    pub same_as: Vec<Written<SameAs>>,
}

impl Shared {
    /// Every wording's terms.
    pub fn terms(&self) -> BTreeSet<String> {
        self.wordings
            .iter()
            .flat_map(|w| w.record.terms.iter().cloned())
            .collect()
    }
}

fn read_one<T: serde::de::DeserializeOwned>(path: &Path) -> Option<(T, Option<String>)> {
    let bytes = fs::read(path).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let version = value
        .get("hippo_version")
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let record = records::decode(&bytes).ok()?;
    Some((record, version))
}

/// A member handle in a file name is letters, digits, `-` and `_`.
fn handle_ok(text: &str) -> bool {
    crate::settings::valid_handle(text)
}

/// Every shared lesson in the workspace, by directory name. Files that do
/// not parse, or whose names do not follow the layout, are left out.
pub fn read(ws: &Workspace) -> Vec<Shared> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(inbox_dir(ws)) else {
        return out;
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_dir()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .and_then(|n| n.strip_prefix("L-"))
                    .is_some_and(|id| id.parse::<Ulid>().is_ok())
        })
        .collect();
    dirs.sort();
    for path in dirs {
        let dir = path.file_name().unwrap().to_string_lossy().into_owned();
        let mut shared = Shared {
            dir,
            path: path.clone(),
            wordings: Vec::new(),
            seen: Vec::new(),
            decisions: Vec::new(),
            same_as: Vec::new(),
        };
        let mut files: Vec<PathBuf> = fs::read_dir(&path)
            .map(|e| e.flatten().map(|e| e.path()).collect())
            .unwrap_or_default();
        files.sort();
        for file in files {
            let Some(name) = file
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_suffix(".json"))
            else {
                continue;
            };
            if let Some(member) = name.strip_prefix("lesson-") {
                if let (true, Some((record, v))) = (handle_ok(member), read_one(&file)) {
                    shared.wordings.push(Written {
                        member: member.to_owned(),
                        record,
                        hippo_version: v,
                        path: file.clone(),
                    });
                }
            } else if let Some(member) = name.strip_prefix("seen-") {
                if let (true, Some((record, v))) = (handle_ok(member), read_one(&file)) {
                    shared.seen.push(Written {
                        member: member.to_owned(),
                        record,
                        hippo_version: v,
                        path: file.clone(),
                    });
                }
            } else if let Some(rest) = name.strip_prefix("decision-") {
                if let Some((member, id)) = rest.rsplit_once('-') {
                    if handle_ok(member) && id.parse::<Ulid>().is_ok() {
                        if let Some((record, v)) = read_one::<Decision>(&file) {
                            shared.decisions.push(Written {
                                member: member.to_owned(),
                                record,
                                hippo_version: v,
                                path: file.clone(),
                            });
                        }
                    }
                }
            } else if let Some(rest) = name.strip_prefix("same-as-") {
                if let Some((_, member)) = rest.rsplit_once('-') {
                    if let (true, Some((record, v))) =
                        (handle_ok(member), read_one::<SameAs>(&file))
                    {
                        shared.same_as.push(Written {
                            member: member.to_owned(),
                            record,
                            hippo_version: v,
                            path: file.clone(),
                        });
                    }
                }
            }
        }
        if !shared.wordings.is_empty() || !shared.decisions.is_empty() {
            out.push(shared);
        }
    }
    out
}

/// The lessons linked by `same-as` files, as groups of indices into
/// `lessons`: a decision on any member applies to all.
pub fn groups(lessons: &[Shared]) -> Vec<Vec<usize>> {
    let mut parent: Vec<usize> = (0..lessons.len()).collect();
    fn find(p: &mut [usize], i: usize) -> usize {
        let mut r = i;
        while p[r] != r {
            r = p[r];
        }
        p[i] = r;
        r
    }
    for (i, l) in lessons.iter().enumerate() {
        for s in &l.same_as {
            if let Some(j) = lessons.iter().position(|o| o.dir == s.record.other) {
                let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                parent[a.max(b)] = a.min(b);
            }
        }
    }
    let mut by_root: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in 0..lessons.len() {
        let r = find(&mut parent, i);
        by_root.entry(r).or_default().push(i);
    }
    by_root.into_values().collect()
}

/// A group's state from its decisions; it never depends on timestamps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// No decision yet.
    Open,
    /// Some keep and no dismissal (or only `duplicate` dismissals).
    Kept,
    /// Only dismissals.
    Dismissed,
    /// A keep and a dismissal: listed together so the two can talk.
    Crossed,
}

/// The decisions of every lesson in a group.
pub fn decisions<'a>(lessons: &'a [Shared], group: &[usize]) -> Vec<&'a Written<Decision>> {
    group
        .iter()
        .flat_map(|&i| lessons[i].decisions.iter())
        .collect()
}

pub fn state(lessons: &[Shared], group: &[usize]) -> State {
    let d = decisions(lessons, group);
    let keep = d.iter().any(|d| d.record.verdict == Verdict::Keep);
    // A duplicate dismissal links two lessons; it is no verdict on the rule.
    let dismiss = d.iter().any(|d| {
        d.record.verdict == Verdict::Dismiss && d.record.reason.as_deref() != Some("duplicate")
    });
    match (keep, dismiss) {
        (false, false) if d.is_empty() => State::Open,
        (true, false) => State::Kept,
        (true, true) => State::Crossed,
        (false, true) => State::Dismissed,
        // Only duplicate links: open.
        (false, false) => State::Open,
    }
}

/// Sessions seen across every person's `seen` file in the group.
pub fn sessions(lessons: &[Shared], group: &[usize]) -> usize {
    group
        .iter()
        .flat_map(|&i| lessons[i].seen.iter())
        .map(|s| s.record.sessions)
        .sum()
}

/// The group whose terms match `terms` by the scan's rule
/// ([`detect::same_terms`]).
pub fn find_group(
    lessons: &[Shared],
    groups: &[Vec<usize>],
    terms: &BTreeSet<String>,
) -> Option<usize> {
    groups.iter().position(|g| {
        g.iter()
            .any(|&i| detect::same_terms(&lessons[i].terms(), terms))
    })
}

/// The group holding directory `dir`.
pub fn group_of(lessons: &[Shared], groups: &[Vec<usize>], dir: &str) -> Option<usize> {
    groups
        .iter()
        .position(|g| g.iter().any(|&i| lessons[i].dir == dir))
}

fn write_record<T: Serialize>(path: &Path, record: &T) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| Error::io_at(dir, e))?;
    }
    crate::util::write_atomic(path, &records::encode_shared(record)?)
}

/// The wording a local lesson shares.
pub fn wording_of(lesson: &Lesson) -> Wording {
    Wording {
        kind: lesson.kind,
        terms: lesson.terms.clone(),
        title: lesson.title.clone(),
        rule: lesson.rule.clone(),
        target: lesson.target.clone(),
        verdict: match &lesson.review {
            Some(r) if r.verdict == records::Verdict::Confirm => "confirmed".to_owned(),
            Some(_) => "rejected".to_owned(),
            None => "not reviewed".to_owned(),
        },
        runner: lesson
            .review
            .as_ref()
            .and_then(|r| r.runner.as_ref())
            .map(|r| r.name.clone()),
    }
}

/// The counts a local lesson shares.
pub fn counts_of(lesson: &Lesson) -> Counts {
    Counts {
        sessions: lesson.seen.sessions,
        first: lesson.seen.first,
        last: lesson.seen.last,
        most_in_a_day: lesson.seen.most_in_a_day,
        agents: lesson.seen.profiles.clone(),
    }
}

/// The files sharing `lesson` writes for `member`: its wording (only when
/// this person has none there yet) and its counts.
pub fn planned_files(ws: &Workspace, member: &str, lesson: &Lesson) -> Vec<PathBuf> {
    let dir = inbox_dir(ws).join(dir_name(lesson.id));
    let mut out = Vec::new();
    let wording = dir.join(format!("lesson-{member}.json"));
    if !wording.exists() {
        out.push(wording);
    }
    out.push(dir.join(format!("seen-{member}.json")));
    out
}

/// Writes `lesson` to the shared Inbox as `member`: its directory is
/// `L-<lesson id>`. Never replaces another person's file.
pub fn share(ws: &Workspace, member: &str, lesson: &Lesson) -> Result<String> {
    let dir = dir_name(lesson.id);
    let path = inbox_dir(ws).join(&dir);
    let wording = path.join(format!("lesson-{member}.json"));
    if !wording.exists() {
        write_record(&wording, &wording_of(lesson))?;
    }
    write_seen(ws, member, &dir, &counts_of(lesson))?;
    Ok(dir)
}

/// Rewrites `member`'s counts for `dir`.
pub fn write_seen(ws: &Workspace, member: &str, dir: &str, counts: &Counts) -> Result<()> {
    write_record(
        &inbox_dir(ws).join(dir).join(format!("seen-{member}.json")),
        counts,
    )
}

/// Writes `member`'s decision on `dir`.
pub fn decide(ws: &Workspace, member: &str, dir: &str, decision: &Decision) -> Result<PathBuf> {
    let path = inbox_dir(ws)
        .join(dir)
        .join(format!("decision-{member}-{}.json", Ulid::generate()));
    write_record(&path, decision)?;
    Ok(path)
}

/// Links `dir` to `other` as the same lesson, as `member`.
pub fn link(
    ws: &Workspace,
    member: &str,
    dir: &str,
    other: &str,
    now: OffsetDateTime,
) -> Result<()> {
    let path = inbox_dir(ws)
        .join(dir)
        .join(format!("same-as-{other}-{member}.json"));
    write_record(
        &path,
        &SameAs {
            other: other.to_owned(),
            by: member.to_owned(),
            at: now,
        },
    )
}

/// The worktrees of the workspace's git repository and the workspace:
/// where a session must have run for its lesson to be shared.
pub fn workspace_roots(ws: &Workspace) -> Vec<PathBuf> {
    let mut roots = vec![fs::canonicalize(&ws.root).unwrap_or_else(|_| ws.root.clone())];
    if let Ok(entries) = fs::read_dir(ws.root.join(".git").join("worktrees")) {
        for e in entries.flatten() {
            if let Ok(text) = fs::read_to_string(e.path().join("gitdir")) {
                if let Some(tree) = Path::new(text.trim()).parent() {
                    roots.push(fs::canonicalize(tree).unwrap_or_else(|_| tree.to_path_buf()));
                }
            }
        }
    }
    roots
}

/// Whether every evidence session of `lesson` ran with its working
/// directory inside the workspace or one of its worktrees, by the
/// sessions' recorded folders (`cwd`). A session with no known folder
/// counts as outside.
pub fn from_workspace(
    lesson: &Lesson,
    folders: &std::collections::HashMap<String, crate::session_meta::Origin>,
    roots: &[PathBuf],
) -> bool {
    !lesson.evidence.is_empty()
        && lesson.evidence.iter().all(|e| {
            folders
                .get(&e.session)
                .and_then(|o| o.cwd.as_ref())
                .is_some_and(|cwd| {
                    let cwd = fs::canonicalize(cwd).unwrap_or_else(|_| cwd.clone());
                    roots.iter().any(|r| cwd.starts_with(r))
                })
        })
}

/// Where in the notes a marker for any of `dirs` is: (note, line).
pub fn markers_in_notes(ws: &Workspace, dirs: &[&str]) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    for entry in walkdir::WalkDir::new(ws.notes_root())
        .into_iter()
        .flatten()
        .filter(|e| e.file_type().is_file() && e.path().extension().is_some_and(|x| x == "md"))
    {
        let Ok(text) = fs::read_to_string(entry.path()) else {
            continue;
        };
        for line in text.lines() {
            if dirs.iter().any(|d| line.contains(&marker(d))) {
                let rel = entry
                    .path()
                    .strip_prefix(&ws.root)
                    .unwrap_or(entry.path())
                    .to_path_buf();
                out.push((rel, line.to_owned()));
            }
        }
    }
    out
}

/// The commit author of `path` in the workspace's git history, if git
/// knows one: the evidence of who pushed a file.
pub fn committer(ws: &Workspace, path: &Path) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(&ws.root)
        .args(["log", "-1", "--format=%an", "--"])
        .arg(path)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let name = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (out.status.success() && !name.is_empty()).then_some(name)
}

/// What the `doctor` team row reports.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Health {
    pub lessons: usize,
    /// A lesson's marker in more than one bullet: (dir, places).
    pub duplicate_markers: Vec<(String, Vec<PathBuf>)>,
    /// Groups with a keep and a dismissal: their first directory.
    pub crossed: Vec<String>,
    /// Members whose latest file was written by a hippo below the
    /// workspace's `min_hippo_version`: (member, version).
    pub below_floor: Vec<(String, String)>,
}

/// The team's health for `doctor`.
pub fn health(ws: &Workspace) -> Health {
    let lessons = read(ws);
    let groups = groups(&lessons);
    let mut h = Health {
        lessons: groups.len(),
        ..Health::default()
    };
    for g in &groups {
        if state(&lessons, g) == State::Crossed {
            h.crossed.push(lessons[g[0]].dir.clone());
        }
        let dirs: Vec<&str> = g.iter().map(|&i| lessons[i].dir.as_str()).collect();
        let found = markers_in_notes(ws, &dirs);
        if found.len() > 1 {
            h.duplicate_markers.push((
                lessons[g[0]].dir.clone(),
                found.into_iter().map(|(p, _)| p).collect(),
            ));
        }
    }
    if let Some(floor) = ws
        .min_hippo_version
        .as_deref()
        .and_then(crate::config::parse_version)
    {
        let mut latest: BTreeMap<String, (OffsetDateTime, Option<String>)> = BTreeMap::new();
        for l in &lessons {
            for d in &l.decisions {
                let e = latest
                    .entry(d.member.clone())
                    .or_insert((d.record.at, d.hippo_version.clone()));
                if d.record.at >= e.0 {
                    *e = (d.record.at, d.hippo_version.clone());
                }
            }
            for s in &l.seen {
                let e = latest
                    .entry(s.member.clone())
                    .or_insert((s.record.last, s.hippo_version.clone()));
                if s.record.last >= e.0 {
                    *e = (s.record.last, s.hippo_version.clone());
                }
            }
        }
        for (member, (_, version)) in latest {
            let below = match version.as_deref().and_then(crate::config::parse_version) {
                Some(v) => v < floor,
                None => true,
            };
            if below {
                h.below_floor
                    .push((member, version.unwrap_or_else(|| "unknown".to_owned())));
            }
        }
    }
    h
}
