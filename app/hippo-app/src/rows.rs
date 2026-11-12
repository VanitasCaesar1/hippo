//! Rows as text, with no window code: Memory's notes list, filtered, and
//! what each row opens; and the facts of dreaming runs and changes that the
//! Inbox, the run tab and the change tab show.

use std::path::{Path, PathBuf};

use hippo_core::dream::{Change, History};
use hippo_core::records::Verdict;
use hippo_core::sessions::{Handoff, LogRow};
use hippo_view::brain::Brain;
use hippo_view::dreaming::Dreaming;
use hippo_view::sessions::Sessions;
use hippo_view::text::visible;
use time::Date;
use time::macros::format_description;

use crate::state::Tab;

/// What a row stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowKey {
    Note(PathBuf),
    /// A line that only says something (an error, an empty list).
    Text(String),
}

impl RowKey {
    /// The id kept in `Selections`.
    pub fn id(&self) -> String {
        match self {
            RowKey::Note(p) => p.display().to_string(),
            RowKey::Text(t) => format!("text:{t}"),
        }
    }

    /// Whether the arrow keys stop on the row.
    pub fn selectable(&self) -> bool {
        !matches!(self, RowKey::Text(_))
    }

    /// The view the row opens, if any.
    pub fn tab(&self) -> Option<Tab> {
        match self {
            RowKey::Note(p) => Some(Tab::Note(p.clone())),
            RowKey::Text(_) => None,
        }
    }
}

/// How a row is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// One line, 24 points.
    Line,
    /// A line in secondary text, or in the error colour.
    Message { error: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub key: RowKey,
    pub style: Style,
    pub title: String,
    /// The secondary line, facts joined by ` · `.
    pub meta: Option<String>,
    /// Short secondary text at the right: a folder, a lint count.
    pub right: Option<String>,
}

impl Row {
    fn message(text: impl Into<String>, error: bool) -> Row {
        let text = text.into();
        Row {
            key: RowKey::Text(text.clone()),
            style: Style::Message { error },
            title: text,
            meta: None,
            right: None,
        }
    }
}

/// What a reload read for the panels.
#[derive(Debug, Clone)]
pub struct Panels {
    pub brain: Brain,
    pub sessions: Result<Sessions, String>,
    #[allow(dead_code)] // the note and proposal tabs read it
    pub history: Result<History, String>,
    pub dreaming: Result<Dreaming, String>,
    /// The notes that need a person, from the linter; notes with no
    /// frontmatter newest first by modification time.
    pub attention: hippo_view::brain::Attention,
    /// The notes directory, relative to the workspace's root.
    pub notes_dir: PathBuf,
    pub dream_at: String,
}

/// The message when there is no index.
pub const NO_INDEX: &str = "No index yet: run hippo ingest";

fn lower_contains(haystack: &str, needle: &str) -> bool {
    needle.is_empty() || haystack.to_lowercase().contains(&needle.to_lowercase())
}

/// A note's folder below the notes directory, with a trailing `/`.
fn folder(path: &Path, notes_dir: &Path) -> Option<String> {
    let below = path.strip_prefix(notes_dir).unwrap_or(path);
    let parent = below.parent()?;
    if parent.as_os_str().is_empty() {
        None
    } else {
        Some(visible(&format!("{}/", parent.display())))
    }
}

/// A lint finding's kind as a row shows it.
pub fn lint_word(kind: &str) -> &str {
    match kind {
        "missing-frontmatter" => "no frontmatter",
        "bad-frontmatter" => "bad frontmatter",
        "dangling-link" => "broken link",
        "overdue" => "review due",
        other => other,
    }
}

/// The Brain list, filtered by path or title, ignoring case.
pub fn brain(p: &Panels, filter: &str) -> Vec<Row> {
    if let Some(e) = &p.brain.error {
        return vec![Row::message(format!("The notes cannot be read: {e}"), true)];
    }
    if p.brain.rows.is_empty() {
        return vec![Row::message(
            format!("No notes in {}.", p.notes_dir.display()),
            false,
        )];
    }
    // A finding most notes share says something about the brain, not about
    // a note: rows leave it out (the note's tab still lists it).
    let common = |kind: &str| {
        let with = p
            .brain
            .rows
            .iter()
            .filter(|r| r.lint_kinds.contains(&kind))
            .count();
        with * 4 > p.brain.rows.len()
    };
    let skip_missing = common("missing-frontmatter");
    let rows: Vec<Row> = p
        .brain
        .rows
        .iter()
        .filter(|r| {
            lower_contains(&r.title, filter) || lower_contains(&r.path.to_string_lossy(), filter)
        })
        .map(|r| {
            let mut right = Vec::new();
            if let Some(f) = folder(&r.path, &p.notes_dir) {
                right.push(f);
            }
            let kinds: Vec<&str> = r
                .lint_kinds
                .iter()
                .copied()
                .filter(|k| !(skip_missing && *k == "missing-frontmatter"))
                .collect();
            match kinds.as_slice() {
                [] => {}
                [one] => right.push(lint_word(one).to_string()),
                many => right.push(format!("{} lint", many.len())),
            }
            Row {
                key: RowKey::Note(r.path.clone()),
                style: Style::Line,
                title: r.title.clone(),
                meta: None,
                right: (!right.is_empty()).then(|| right.join(" · ")),
            }
        })
        .collect();
    if rows.is_empty() {
        return vec![Row::message("No note matches the filter.", false)];
    }
    rows
}

/// A day as headings show it: `Sun 08 Nov 2026`.
pub fn day_label(d: Date) -> String {
    d.format(format_description!(
        "[weekday repr:short] [day] [month repr:short] [year]"
    ))
    .unwrap_or_default()
}

/// A session's facts as `hippo log` gives them, for its tab's header:
/// `claude-code · 34 turns · 2 notes · ended · handoff`.
pub fn session_meta(row: &LogRow, turns: Option<u64>) -> String {
    let mut facts = vec![row.tool.as_str().to_string()];
    match turns.or(row.turns) {
        Some(1) => facts.push("1 turn".into()),
        Some(n) => facts.push(format!("{n} turns")),
        None => {}
    }
    match row.notes.len() {
        0 => {}
        1 => facts.push("1 note".into()),
        n => facts.push(format!("{n} notes")),
    }
    facts.push(if row.ended.is_some() { "ended" } else { "open" }.into());
    match row.handoff {
        Handoff::Yes => facts.push("handoff".into()),
        Handoff::Derived => facts.push("handoff derived".into()),
        Handoff::No => {}
    }
    if row.confirmed + row.rejected > 0 {
        facts.push(format!(
            "dreaming {} confirmed, {} rejected",
            row.confirmed, row.rejected
        ));
    }
    facts.join(" · ")
}

/// A session id as a title: its first 8 characters, and for a sub-agent's
/// session (`<parent>-agent-<id>`) the agent's first 8 too, since sub-agents
/// of one session share the parent's prefix.
pub fn id_title(id: &str) -> String {
    // A UUID's first 8 characters tell it apart; a shorter, readable id is
    // shown whole.
    let uuid_like = |s: &str| {
        s.len() > 8
            && s.as_bytes().get(8) == Some(&b'-')
            && s[..8].bytes().all(|b| b.is_ascii_hexdigit())
    };
    let short = |s: &str| {
        if uuid_like(s) || s.chars().count() > 24 {
            s.chars().take(8).collect::<String>()
        } else {
            s.to_string()
        }
    };
    match id.split_once("-agent-") {
        Some((parent, agent)) => {
            let agent: String = agent.chars().take(8).collect();
            visible(&format!("{} › agent {agent}", short(parent)))
        }
        None => visible(&short(id)),
    }
}

/// A change's receipt state, in words.
pub fn receipt_state(change: &Change) -> String {
    let reverted = change.receipts.iter().any(|r| r.verdict == Verdict::Revert);
    let last = change
        .receipts
        .iter()
        .rev()
        .find(|r| r.verdict != Verdict::Revert);
    match last {
        None => "no receipt".into(),
        Some(r) => {
            let verdict = match r.verdict {
                Verdict::Confirm => "confirmed",
                Verdict::Reject => "rejected",
                Verdict::Revert => "reverted",
            };
            let applied = if r.applied.is_some() {
                ", applied"
            } else if r.verdict == Verdict::Confirm {
                ", not applied"
            } else {
                ""
            };
            if reverted {
                format!("{verdict}{applied}, reverted")
            } else {
                format!("{verdict}{applied}")
            }
        }
    }
}

/// A change's title (its note's file name without `.md`) and secondary line
/// (pass, kind, receipt state).
pub fn change_text(change: &Change) -> (String, String) {
    match &change.proposal.record {
        Ok(p) => {
            let title = p
                .note
                .file_stem()
                .map(|s| visible(&s.to_string_lossy()))
                .unwrap_or_else(|| visible(&p.note.display().to_string()));
            let kind = p.kind.as_str();
            (
                title,
                format!("{} · {kind} · {}", visible(&p.pass), receipt_state(change)),
            )
        }
        Err(e) => (
            change.proposal_id.to_string(),
            format!("proposal unreadable: {e} · {}", receipt_state(change)),
        ),
    }
}

/// A note's name from its path: the file stem.
pub fn note_name(path: &str) -> String {
    let p = std::path::Path::new(path);
    visible(
        &p.file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_owned()),
    )
}

/// What one dreaming run did, counted for its row, the overview and its
/// tab.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunFacts {
    /// When the run's window ended: the run's time.
    pub when: Option<time::OffsetDateTime>,
    pub date: Option<Date>,
    pub failed: bool,
    pub error: Option<String>,
    pub dry_run: bool,
    /// Changes written to notes (confirmed and applied) and still in place.
    pub applied: usize,
    /// Changes written, then reverted by a person.
    pub reverted: usize,
    /// Proposals the reviewer rejected.
    pub rejected: usize,
    /// Proposals with any other state (confirmed but not applied, no
    /// receipt).
    pub other: usize,
    /// Proposals that failed validation: never reviewed, never written.
    pub invalid: usize,
    /// Notes the run checked.
    pub checked: usize,
    /// Whole notes left out.
    pub skipped: usize,
}

/// Counts a run's changes and its report's digest.
pub fn run_facts(run: &hippo_view::dreaming::Run) -> RunFacts {
    let mut f = RunFacts::default();
    if let Ok(r) = &run.report.record {
        f.date = Some(r.status.date);
        f.failed = r.status.failed;
        f.error = r.status.error.clone();
        f.dry_run = r.status.dry_run;
    } else {
        f.failed = true;
    }
    for change in &run.changes {
        let last = change
            .receipts
            .iter()
            .rev()
            .find(|r| r.verdict != Verdict::Revert);
        match last {
            Some(r) if r.verdict == Verdict::Confirm && r.applied.is_some() => {
                if change.receipts.iter().any(|r| r.verdict == Verdict::Revert) {
                    f.reverted += 1;
                } else {
                    f.applied += 1;
                }
            }
            Some(r) if r.verdict == Verdict::Reject => f.rejected += 1,
            _ => f.other += 1,
        }
    }
    if let Some(d) = &run.digest {
        f.invalid = d.failures.len();
        f.checked = d.notes_checked().len();
        let mut left: Vec<&str> = d.skipped.iter().map(|s| s.note.as_str()).collect();
        left.sort_unstable();
        left.dedup();
        f.skipped = left.len();
        // `<from> to <to>: done`: the last chunk's end, when the report's
        // time is unknown (a failed run's last chunk is the failed one).
        f.when = d.window.last().and_then(|w| {
            let (end, _) = w.split(" to ").nth(1)?.rsplit_once(": ")?;
            time::OffsetDateTime::parse(end, &time::format_description::well_known::Rfc3339).ok()
        });
        if let Some(t) = run.written {
            f.when = Some(time::OffsetDateTime::from(t));
        }
        // A dry run proposes without writing: count what it proposed.
        if f.dry_run {
            f.other += run.missing.len();
        }
    }
    f
}

/// `1 change`, `3 changes`, `no changes`.
pub fn count(n: usize, one: &str, many: &str) -> String {
    match n {
        0 => format!("no {many}"),
        1 => format!("1 {one}"),
        n => format!("{n} {many}"),
    }
}

impl RunFacts {
    /// `Sun 22:53` within a week, else `8 Nov 22:53`, local time; the
    /// report's date without a time.
    pub fn when_label(&self, offset: time::UtcOffset) -> String {
        match (self.when, self.date) {
            // The app's one date form: `Sun 22:53` within a week, else
            // `8 Nov 22:53`.
            (Some(t), _) => {
                hippo_view::fleet::when(t.unix_timestamp(), time::OffsetDateTime::now_utc(), offset)
            }
            (None, Some(d)) => day_label(d),
            (None, None) => "unknown date".into(),
        }
    }

    /// The headline: `failed`, `dry run, wrote nothing`, `2 changes`.
    pub fn outcome(&self) -> String {
        if self.failed {
            "failed".into()
        } else if self.dry_run {
            "dry run, wrote nothing".into()
        } else {
            count(self.applied, "change", "changes")
        }
    }

    /// The facts after the headline, joined by ` · `: what was rejected,
    /// failed validation, checked and left out.
    pub fn details(&self) -> String {
        let mut parts = Vec::new();
        if self.reverted > 0 {
            parts.push(format!("{} reverted", self.reverted));
        }
        if self.rejected > 0 {
            parts.push(format!("{} rejected", self.rejected));
        }
        if self.other > 0 {
            parts.push(if self.dry_run {
                count(self.other, "proposal", "proposals")
            } else {
                format!("{} not applied", self.other)
            });
        }
        if self.invalid > 0 {
            parts.push(format!("{} failed validation", self.invalid));
        }
        parts.push(format!("{} checked", count(self.checked, "note", "notes")));
        if self.skipped > 0 {
            parts.push(format!("{} left out", count(self.skipped, "note", "notes")));
        }
        parts.join(" · ")
    }
}

/// The index of the next selectable row after `from` (or the first), moving
/// by `step` (1 or -1); `None` when there is none.
pub fn step(rows: &[Row], from: Option<usize>, step: isize) -> Option<usize> {
    let n = rows.len() as isize;
    let mut i = match from {
        Some(i) => i as isize + step,
        None if step > 0 => 0,
        None => n - 1,
    };
    while (0..n).contains(&i) {
        if rows[i as usize].key.selectable() {
            return Some(i as usize);
        }
        i += step;
    }
    from
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)] // the tests build a temporary hippo home
mod tests {
    use super::*;
    use hippo_core::config::{InitOptions, init_in};

    fn panels_with_notes(notes: &[(&str, &str)]) -> (tempfile::TempDir, Panels) {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let ws = dir.path().join("ws");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(ws.join("notes")).unwrap();
        init_in(
            &home,
            InitOptions {
                workspace: Some(ws.clone()),
                notes_dir: Some("notes".into()),
                entry_note: Some("notes/index.md".into()),
                add_roots: Vec::new(),
                force: false,
            },
        )
        .unwrap();
        for (path, text) in notes {
            let p = ws.join("notes").join(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        let loaded = crate::state::load(&home, time::OffsetDateTime::now_utc());
        let panels = loaded.panels.expect("ready");
        (dir, panels)
    }

    #[test]
    fn brain_rows_show_folders_and_filter_by_title_or_path() {
        let (_dir, p) = panels_with_notes(&[
            ("index.md", "# Start here\n"),
            ("ops/Deploy.md", "# Deploy notes\n"),
        ]);
        let all = brain(&p, "");
        let titles: Vec<&str> = all.iter().map(|r| r.title.as_str()).collect();
        assert!(titles.contains(&"Deploy notes"), "{titles:?}");
        let deploy = all.iter().find(|r| r.title == "Deploy notes").unwrap();
        // Every note here lacks frontmatter: the rows leave that out.
        assert_eq!(deploy.right.as_deref(), Some("ops/"));
        assert_eq!(brain(&p, "OPS/").len(), 1, "path, ignoring case");
        assert_eq!(brain(&p, "start").len(), 1, "title");
        assert_eq!(
            brain(&p, "nothing like this")[0].style,
            Style::Message { error: false }
        );
    }

    #[test]
    fn a_finding_few_notes_have_is_named_in_the_row() {
        let fm = "---\ntype: decision\nstatus: current\nupdated: 2026-11-01\nreview_after: 2099-01-01\n---\n";
        let notes: Vec<(String, String)> = (0..5)
            .map(|i| (format!("n{i}.md"), format!("{fm}# Note {i}\n[[index]]\n")))
            .chain([("index.md".to_string(), format!("{fm}# Start\n"))])
            .chain([("ops/Deploy.md".to_string(), "# Deploy notes\n".to_string())])
            .collect();
        let refs: Vec<(&str, &str)> = notes
            .iter()
            .map(|(a, b)| (a.as_str(), b.as_str()))
            .collect();
        let (_dir, p) = panels_with_notes(&refs);
        let deploy = brain(&p, "")
            .into_iter()
            .find(|r| r.title == "Deploy notes")
            .unwrap();
        assert_eq!(deploy.right.as_deref(), Some("ops/ · no frontmatter"));
        assert_eq!(lint_word("dangling-link"), "broken link");
    }

    #[test]
    fn an_empty_home_says_what_each_list_lacks() {
        let (_dir, p) = panels_with_notes(&[]);
        assert!(brain(&p, "")[0].title.starts_with("No notes in notes"));
    }

    #[test]
    fn the_arrow_keys_pass_over_headings_and_messages() {
        let row = |key: RowKey| Row {
            key,
            style: Style::Line,
            title: String::new(),
            meta: None,
            right: None,
        };
        let rows = vec![
            row(RowKey::Text("heading".into())),
            row(RowKey::Note("a".into())),
            row(RowKey::Text("heading".into())),
            row(RowKey::Note("b".into())),
            row(RowKey::Text("x".into())),
        ];
        assert_eq!(step(&rows, None, 1), Some(1));
        assert_eq!(step(&rows, Some(1), 1), Some(3));
        assert_eq!(step(&rows, Some(3), 1), Some(3), "stays at the end");
        assert_eq!(step(&rows, Some(3), -1), Some(1));
        assert_eq!(step(&rows, None, -1), Some(3));
    }

    #[test]
    fn a_sub_agent_session_is_titled_by_parent_and_agent() {
        assert_eq!(
            id_title("c93ae326-d55b-42b4-8b4d-c2c01ab50490-agent-ad45cab426f361768"),
            "c93ae326 › agent ad45cab4"
        );
        assert_eq!(id_title("eval-01"), "eval-01");
        assert_eq!(
            id_title("demo-live"),
            "demo-live",
            "a short id is shown whole"
        );
    }

    #[test]
    fn row_ids_are_note_paths() {
        assert_eq!(RowKey::Note("notes/a.md".into()).id(), "notes/a.md");
        assert!(!RowKey::Text("x".into()).selectable());
        assert!(RowKey::Note("a".into()).selectable());
    }
}
