//! Log notes: notes that record what happened rather
//! than what is true, so verifying them finds nothing to correct and
//! offering them for a task is noise.
//!
//! A note is a log note when it is the workspace's `log_path`, or, with no
//! `log_notes` setting in `hippocamp.toml`, when either holds:
//!
//! - **dated file**: its file name starts with a date `YYYY-MM-DD`, and at
//!   least [`DATED_SHARE`] of the at least [`DATED_FOLDER_MIN`] notes
//!   directly in its folder do too (a folder of session write-ups);
//! - **journal**: at least [`JOURNAL_MIN`] of its level-2 and level-3
//!   headings, and at least [`DATED_SHARE`] of them, start with a date
//!   `YYYY-MM-DD` (an append-only log another tool keeps).
//!
//! A `log_notes` setting (globs relative to `notes_dir`, matched as a
//! pass's `note_paths`) replaces both rules; `log_notes = []` turns them
//! off. `log_path` always counts.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::config::Workspace;
use crate::notes::{normalise, Notes};

/// A folder's dated share, and a journal's dated-heading share, at or over
/// which its notes count as log notes: 4 in 5.
pub const DATED_SHARE: (usize, usize) = (4, 5);
/// The fewest notes directly in a folder for the dated-file rule.
pub const DATED_FOLDER_MIN: usize = 5;
/// The fewest dated level-2 and level-3 headings for the journal rule.
pub const JOURNAL_MIN: usize = 10;

/// The workspace's log notes among `notes`, relative to the notes
/// directory (the keys of [`Notes::notes`]).
pub fn below_notes_dir(ws: &Workspace, notes: &Notes) -> BTreeSet<PathBuf> {
    let notes_dir = normalise(&ws.notes_dir);
    let mut found = BTreeSet::new();
    if let Ok(log) = normalise(&ws.log_path).strip_prefix(&notes_dir) {
        found.insert(log.to_path_buf());
    }
    match &ws.log_notes {
        Some(patterns) => {
            for path in notes.notes.keys() {
                if matches_any(patterns, path) {
                    found.insert(path.clone());
                }
            }
        }
        None => {
            found.extend(dated_files(notes));
            for (path, note) in &notes.notes {
                if is_journal(&String::from_utf8_lossy(&note.body)) {
                    found.insert(path.clone());
                }
            }
        }
    }
    found
}

/// [`below_notes_dir`], relative to the workspace (`<notes_dir>/<path>`),
/// as dreaming names notes.
pub fn in_workspace(ws: &Workspace, notes: &Notes) -> BTreeSet<PathBuf> {
    let notes_dir = normalise(&ws.notes_dir);
    below_notes_dir(ws, notes)
        .into_iter()
        .map(|path| normalise(&notes_dir.join(path)))
        .collect()
}

/// Notes whose file name starts with a date, in folders that are mostly
/// such notes.
fn dated_files(notes: &Notes) -> Vec<PathBuf> {
    let mut folders: BTreeMap<PathBuf, (usize, Vec<PathBuf>)> = BTreeMap::new();
    for path in notes.notes.keys() {
        let folder = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let entry = folders.entry(folder).or_default();
        entry.0 += 1;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default();
        if starts_with_date(&name) {
            entry.1.push(path.clone());
        }
    }
    folders
        .into_values()
        .filter(|(all, dated)| *all >= DATED_FOLDER_MIN && mostly(dated.len(), *all))
        .flat_map(|(_, dated)| dated)
        .collect()
}

/// Whether a note body is a journal: most of its level-2 and level-3
/// headings, and at least [`JOURNAL_MIN`], start with a date. Headings in
/// fenced code blocks do not count.
pub fn is_journal(body: &str) -> bool {
    let mut fenced = false;
    let (mut all, mut dated) = (0usize, 0usize);
    for line in body.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        let Some(text) = line
            .strip_prefix("## ")
            .or_else(|| line.strip_prefix("### "))
        else {
            continue;
        };
        all += 1;
        if starts_with_date(text.trim_start()) {
            dated += 1;
        }
    }
    dated >= JOURNAL_MIN && mostly(dated, all)
}

/// `YYYY-MM-DD` with a month 01–12 and a day 01–31, at the start of `text`,
/// not followed by another digit.
pub fn starts_with_date(text: &str) -> bool {
    let b = text.as_bytes();
    if b.len() < 10 || b.get(10).is_some_and(u8::is_ascii_digit) {
        return false;
    }
    let digits = |r: std::ops::Range<usize>| b[r].iter().all(u8::is_ascii_digit);
    if !(digits(0..4) && b[4] == b'-' && digits(5..7) && b[7] == b'-' && digits(8..10)) {
        return false;
    }
    let num = |r: std::ops::Range<usize>| {
        std::str::from_utf8(&b[r])
            .ok()
            .and_then(|s| s.parse::<u8>().ok())
    };
    num(5..7).is_some_and(|m| (1..=12).contains(&m))
        && num(8..10).is_some_and(|d| (1..=31).contains(&d))
}

fn mostly(part: usize, all: usize) -> bool {
    all > 0 && part * DATED_SHARE.1 >= all * DATED_SHARE.0
}

/// Whether `path` (relative to `notes_dir`) matches one of `patterns`:
/// case-insensitive, `*` does not cross `/`, as a pass's `note_paths`.
pub fn matches_any(patterns: &[String], path: &Path) -> bool {
    let path = path
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    let options = glob::MatchOptions {
        case_sensitive: false,
        require_literal_separator: true,
        require_literal_leading_dot: true,
    };
    patterns
        .iter()
        .any(|pattern| glob::Pattern::new(pattern).is_ok_and(|p| p.matches_with(&path, options)))
}

/// Checks a `log_notes` setting: at most 50 globs, each under 200 bytes,
/// relative, without `..` and parsing.
pub fn check_patterns(patterns: &[String]) -> std::result::Result<(), String> {
    if patterns.len() > 50 {
        return Err("log_notes: at most 50 patterns".to_owned());
    }
    for pattern in patterns {
        if pattern.chars().any(char::is_control) {
            return Err("log_notes: an entry has a control character".to_owned());
        }
        if pattern.len() > 200 {
            return Err(format!("log_notes: {pattern} is over 200 bytes"));
        }
        if pattern.starts_with('/') {
            return Err(format!("log_notes: {pattern} starts with /"));
        }
        if pattern.split('/').any(|part| part == "..") {
            return Err(format!("log_notes: {pattern} has a .. component"));
        }
        if let Err(err) = glob::Pattern::new(pattern) {
            return Err(format!("log_notes: {pattern} does not parse ({err})"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn ws(root: &Path, log_notes: Option<Vec<String>>) -> Workspace {
        Workspace {
            root: root.to_path_buf(),
            notes_dir: "notes".into(),
            entry_note: "notes/START HERE.md".into(),
            required_notes: Vec::new(),
            budget_bytes: 25_600,
            log_path: "notes/Session log.md".into(),
            sample_tasks: Vec::new(),
            passes_off: Vec::new(),
            lessons: true,
            snapshot_keep_days: None,
            log_notes,
        }
    }

    fn write(root: &Path, rel: &str, text: &str) {
        let path = root.join("notes").join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn journal(n: usize) -> String {
        let mut text = String::from("# Work log\n\n");
        for i in 0..n {
            text.push_str(&format!(
                "## 2026-10-{:02} — entry {i}\n\nDid things.\n\n",
                i % 28 + 1
            ));
        }
        text
    }

    fn setup() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "START HERE.md", "# Start\n");
        write(root, "Session log.md", "# Session log\n");
        write(root, "Topic.md", "# Topic\n\n## Status\n\nFine.\n");
        // A folder of dated write-ups, one undated README among six.
        for day in 1..=5 {
            write(
                root,
                &format!("Sessions/2026-11-0{day}-maintenance.md"),
                "# Run\n",
            );
        }
        write(root, "Sessions/README.md", "# Sessions\n");
        // A folder where dated notes are the minority.
        write(root, "Research/2026-10-28 inventory.md", "# Inventory\n");
        for name in ["A", "B", "C", "D"] {
            write(root, &format!("Research/{name}.md"), "# R\n");
        }
        // Too few notes to judge.
        write(root, "Inbox/2026-11-08 idea.md", "# Idea\n");
        write(root, "Ops/Work log.md", &journal(12));
        write(
            root,
            "Ops/Runbook.md",
            "# Runbook\n\n## Deploy\n\n## 2026-10-01 incident\n",
        );
        dir
    }

    fn names(set: &BTreeSet<PathBuf>) -> Vec<String> {
        set.iter().map(|p| p.display().to_string()).collect()
    }

    #[test]
    fn dates_are_recognised_strictly() {
        assert!(starts_with_date("2026-11-07-daily.md"));
        assert!(starts_with_date("2026-11-07 Reloop"));
        assert!(starts_with_date("2026-11-07"));
        assert!(!starts_with_date("2026-13-07 x"));
        assert!(!starts_with_date("2026-11-00 x"));
        assert!(!starts_with_date("20261107 x"));
        assert!(!starts_with_date("2026-11-070"));
        assert!(!starts_with_date("Notes 2026-11-07"));
        assert!(!starts_with_date("2026-1"));
    }

    #[test]
    fn automatic_rules_find_dated_folders_journals_and_the_log_path() {
        let dir = setup();
        let notes = Notes::scan(&dir.path().join("notes")).unwrap();
        let found = below_notes_dir(&ws(dir.path(), None), &notes);
        assert_eq!(
            names(&found),
            vec![
                "Ops/Work log.md",
                "Session log.md",
                "Sessions/2026-11-01-maintenance.md",
                "Sessions/2026-11-02-maintenance.md",
                "Sessions/2026-11-03-maintenance.md",
                "Sessions/2026-11-04-maintenance.md",
                "Sessions/2026-11-05-maintenance.md",
            ]
        );
        let in_ws = in_workspace(&ws(dir.path(), None), &notes);
        assert!(in_ws.contains(Path::new("notes/Session log.md")));
    }

    #[test]
    fn a_journal_needs_ten_dated_headings_that_are_most_of_them() {
        assert!(is_journal(&journal(10)));
        assert!(!is_journal(&journal(9)));
        let mixed = format!("{}{}", journal(10), "## Plan\n## Risks\n## Owners\n");
        assert!(!is_journal(&mixed));
        let fenced = format!("```\n{}```\n", journal(12));
        assert!(!is_journal(&fenced));
    }

    #[test]
    fn a_log_notes_setting_replaces_the_automatic_rules() {
        let dir = setup();
        let notes = Notes::scan(&dir.path().join("notes")).unwrap();
        let only = ws(dir.path(), Some(vec!["inbox/*.md".into()]));
        assert_eq!(
            names(&below_notes_dir(&only, &notes)),
            vec!["Inbox/2026-11-08 idea.md", "Session log.md"]
        );
        let off = ws(dir.path(), Some(Vec::new()));
        assert_eq!(
            names(&below_notes_dir(&off, &notes)),
            vec!["Session log.md"]
        );
    }

    #[test]
    fn bad_patterns_are_refused() {
        assert!(check_patterns(&["Sessions/**/*.md".into()]).is_ok());
        assert!(check_patterns(&["/abs".into()]).is_err());
        assert!(check_patterns(&["a/../b".into()]).is_err());
        assert!(check_patterns(&["[".into()]).is_err());
        assert!(check_patterns(&vec!["a".to_owned(); 51]).is_err());
    }
}
