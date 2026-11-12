//! The context suite: for each task, which of the notes a person wants
//! does the budgeted block list.

use std::path::{Path, PathBuf};

use super::set::Set;
use crate::config::Workspace;
use crate::context::{self, Part, PartSelection};
use crate::error::Result;

/// One question's outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextRow {
    pub task: String,
    pub expected: Vec<PathBuf>,
    /// The notes the block's `relevant` part lists; empty when omitted.
    pub listed: Vec<PathBuf>,
    /// How many expected notes are also listed.
    pub found: usize,
    /// The `relevant` part did not fit the budget.
    pub omitted: bool,
}

impl ContextRow {
    pub fn new(task: &str, expected: &[PathBuf], listed: Vec<PathBuf>, omitted: bool) -> Self {
        let found = expected.iter().filter(|path| listed.contains(path)).count();
        ContextRow {
            task: task.to_owned(),
            expected: expected.to_vec(),
            listed,
            found,
            omitted,
        }
    }
}

/// Every row of the suite, in set order, and the totals over them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ContextResult {
    pub rows: Vec<ContextRow>,
}

impl ContextResult {
    /// Expected notes that were listed, over all questions.
    pub fn found(&self) -> usize {
        self.rows.iter().map(|row| row.found).sum()
    }

    pub fn expected(&self) -> usize {
        self.rows.iter().map(|row| row.expected.len()).sum()
    }

    pub fn listed(&self) -> usize {
        self.rows.iter().map(|row| row.listed.len()).sum()
    }

    /// Questions that expect notes and list all of them, out of the
    /// questions that expect any.
    pub fn complete(&self) -> (usize, usize) {
        let asked = self.rows.iter().filter(|row| !row.expected.is_empty());
        let done = asked.clone().filter(|row| row.found == row.expected.len());
        (done.count(), asked.count())
    }

    /// Questions that expect nothing and list nothing, out of the
    /// questions that expect nothing.
    pub fn quiet(&self) -> (usize, usize) {
        let asked = self.rows.iter().filter(|row| row.expected.is_empty());
        let quiet = asked.clone().filter(|row| row.listed.is_empty());
        (quiet.count(), asked.count())
    }

    /// `context: found 14/22, precision 14/31, complete 9/16, quiet 2/4`.
    pub fn summary(&self) -> String {
        let (complete, with_notes) = self.complete();
        let (quiet, without_notes) = self.quiet();
        format!(
            "context: found {found}/{expected}, precision {found}/{listed}, \
             complete {complete}/{with_notes}, quiet {quiet}/{without_notes}",
            found = self.found(),
            expected = self.expected(),
            listed = self.listed(),
        )
    }
}

/// Compiles the full block for every task, as `hippocamp context --task` does
/// with the workspace's budget and no session. A block whose required
/// parts exceed the budget is an error: the set is broken.
pub fn run(ws: &Workspace, set: &Set) -> Result<ContextResult> {
    let mut rows = Vec::new();
    for question in &set.context {
        let compiled = context::compile(
            ws,
            None,
            Some(&question.task),
            None,
            None,
            ws.budget_bytes,
            PartSelection::All,
        )?;
        let omitted = compiled
            .omitted
            .iter()
            .any(|(part, _)| *part == Part::Relevant);
        rows.push(ContextRow::new(
            &question.task,
            &question.expected,
            compiled.relevant,
            omitted,
        ));
    }
    Ok(ContextResult { rows })
}

/// A note's file name without `.md`, as the report's tables show it.
pub(super) fn stem(path: &Path) -> String {
    path.file_stem()
        .map_or_else(String::new, |s| s.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(names: &[&str]) -> Vec<PathBuf> {
        names
            .iter()
            .map(|n| PathBuf::from(format!("notes/{n}.md")))
            .collect()
    }

    fn row(expected: &[&str], listed: &[&str], omitted: bool) -> ContextRow {
        ContextRow::new("task", &paths(expected), paths(listed), omitted)
    }

    #[test]
    fn totals_for_hand_made_rows() {
        let result = ContextResult {
            rows: vec![
                row(&["A"], &["A", "B"], false),
                row(&["A", "B"], &["B", "C", "D"], false),
                row(&[], &["E"], false),
                row(&[], &[], false),
                row(&["F"], &[], true),
            ],
        };
        assert_eq!(result.rows[0].found, 1);
        assert_eq!(result.rows[1].found, 1);
        assert_eq!(
            (result.found(), result.expected(), result.listed()),
            (2, 4, 6)
        );
        assert_eq!(result.complete(), (1, 3));
        assert_eq!(result.quiet(), (1, 2));
        assert_eq!(
            result.summary(),
            "context: found 2/4, precision 2/6, complete 1/3, quiet 1/2"
        );
    }

    #[test]
    fn an_omitted_part_lists_nothing() {
        let row = row(&["A"], &[], true);
        assert!(row.omitted);
        assert_eq!((row.found, row.listed.len()), (0, 0));
    }

    #[test]
    fn stems_drop_the_directory_and_extension() {
        assert_eq!(
            stem(Path::new("notes/Decisions/Use Postgres 17.md")),
            "Use Postgres 17"
        );
    }
}
