//! The dreaming suite: one `verify` run over the set's workspace, scored
//! against the planted errors and kept claims. Every number compares the notes before and after the run.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::set::{Dreaming, ErrorKind, KeptClaim, PlantedError};
use crate::config::{Config, Workspace};
use crate::dream::{self, window};
use crate::error::{Error, Result};

/// What happened to a planted error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorResult {
    /// The stale text is gone and a fixed string is present.
    Corrected,
    /// The stale text is still there.
    Stale,
    /// The stale text is gone and no fixed string is present.
    Removed,
}

impl ErrorResult {
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorResult::Corrected => "corrected",
            ErrorResult::Stale => "stale",
            ErrorResult::Removed => "removed",
        }
    }
}

/// What happened to a kept claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeptResult {
    Kept,
    /// The claim's text is gone.
    Damaged,
}

impl KeptResult {
    pub fn as_str(self) -> &'static str {
        match self {
            KeptResult::Kept => "kept",
            KeptResult::Damaged => "damaged",
        }
    }
}

/// One planted error's row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorRow {
    pub note: PathBuf,
    pub stale: String,
    pub kind: ErrorKind,
    pub result: ErrorResult,
}

/// One kept claim's row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeptRow {
    pub note: PathBuf,
    pub text: String,
    pub result: KeptResult,
}

/// The suite's result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DreamingResult {
    pub wall: Duration,
    pub errors: Vec<ErrorRow>,
    pub kept: Vec<KeptRow>,
    /// Non-blank lines of the notes the key names that the run lost, apart
    /// from the stale text.
    pub lines_lost: usize,
    /// Clean candidate notes whose bytes changed.
    pub clean_notes_changed: usize,
    /// The clean candidate notes: those `verify` takes that hold no planted
    /// error.
    pub clean_notes_checked: usize,
    /// The run's own counts.
    pub proposals: usize,
    pub validation_failures: usize,
    pub confirmed: usize,
    pub rejected: usize,
    pub applied: usize,
    /// The text of the dreaming report the run wrote.
    pub report: String,
}

impl DreamingResult {
    /// For each kind the set has, in report order: errors corrected, and
    /// all errors of the kind.
    pub fn corrected_by_kind(&self) -> Vec<(ErrorKind, usize, usize)> {
        ErrorKind::ALL
            .iter()
            .filter_map(|kind| {
                let rows: Vec<&ErrorRow> =
                    self.errors.iter().filter(|row| row.kind == *kind).collect();
                let corrected = rows
                    .iter()
                    .filter(|row| row.result == ErrorResult::Corrected)
                    .count();
                (!rows.is_empty()).then_some((*kind, corrected, rows.len()))
            })
            .collect()
    }

    /// `dreaming: corrected 2/6, damaged 1/8, lines lost 0, clean notes
    /// changed 1/2; proposals 4, validation failures 1, confirmed 3,
    /// rejected 1, applied 3`.
    pub fn summary(&self) -> String {
        let corrected = self
            .errors
            .iter()
            .filter(|row| row.result == ErrorResult::Corrected)
            .count();
        let damaged = self
            .kept
            .iter()
            .filter(|row| row.result == KeptResult::Damaged)
            .count();
        format!(
            "dreaming: corrected {corrected}/{}, damaged {damaged}/{}, lines lost {}, \
             clean notes changed {}/{}; proposals {}, validation failures {}, confirmed {}, \
             rejected {}, applied {}",
            self.errors.len(),
            self.kept.len(),
            self.lines_lost,
            self.clean_notes_changed,
            self.clean_notes_checked,
            self.proposals,
            self.validation_failures,
            self.confirmed,
            self.rejected,
            self.applied
        )
    }
}

/// How a planted error stands in the note's text after the run.
pub fn classify_error(after: &str, error: &PlantedError) -> ErrorResult {
    if after.contains(&error.stale) {
        ErrorResult::Stale
    } else if error.fixed.iter().any(|fixed| after.contains(fixed)) {
        ErrorResult::Corrected
    } else {
        ErrorResult::Removed
    }
}

/// How a kept claim stands in the note's text after the run.
pub fn classify_kept(after: &str, keep: &KeptClaim) -> KeptResult {
    if after.contains(&keep.text) {
        KeptResult::Kept
    } else {
        KeptResult::Damaged
    }
}

/// The non-blank lines of `before` that hold none of the `stale` texts and
/// are not, as a whole line, in `after`: what a run took out of a note
/// besides the wrong claims.
pub fn lines_lost(before: &str, after: &str, stale: &[&str]) -> usize {
    let kept: BTreeSet<&str> = after.lines().collect();
    before
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter(|line| !stale.iter().any(|text| line.contains(text)))
        .filter(|line| !kept.contains(line))
        .count()
}

/// Runs the suite. `set_dir` holds the set's own copies of the notes, which
/// the clean-note comparison reads; `ws` holds the working copies.
pub fn run(
    config: &Config,
    ws: &Workspace,
    set_dir: &Path,
    dreaming: &Dreaming,
    clean_candidates: &BTreeSet<PathBuf>,
) -> Result<DreamingResult> {
    let started = Instant::now();
    window::advance(ws, dreaming.last_success)?;

    let summary = dream::run_at(config, ws, Some("verify"), false, dreaming.now)?;
    let report_text =
        fs::read_to_string(&summary.report).map_err(|e| Error::io_at(&summary.report, e))?;

    let after = |note: &Path| fs::read_to_string(ws.root.join(note)).unwrap_or_default();
    let errors = dreaming
        .error
        .iter()
        .map(|error| ErrorRow {
            note: error.note.clone(),
            stale: error.stale.clone(),
            kind: error.kind,
            result: classify_error(&after(&error.note), error),
        })
        .collect();
    let kept = dreaming
        .keep
        .iter()
        .map(|keep| KeptRow {
            note: keep.note.clone(),
            text: keep.text.clone(),
            result: classify_kept(&after(&keep.note), keep),
        })
        .collect();

    // Lines lost from the notes the key names.
    let named: BTreeSet<&PathBuf> = dreaming
        .error
        .iter()
        .map(|error| &error.note)
        .chain(dreaming.keep.iter().map(|keep| &keep.note))
        .collect();
    let mut lost = 0;
    for note in named {
        let before = fs::read_to_string(set_dir.join(note)).unwrap_or_default();
        let stale: Vec<&str> = dreaming
            .error
            .iter()
            .filter(|error| error.note == *note)
            .map(|error| error.stale.as_str())
            .collect();
        lost += lines_lost(&before, &after(note), &stale);
    }
    // The clean candidates, notes `verify` takes that hold no planted
    // error, must be as the set has them.
    let clean_notes_changed = clean_candidates
        .iter()
        .filter(|note| fs::read(set_dir.join(note)).ok() != fs::read(ws.root.join(note)).ok())
        .count();

    Ok(DreamingResult {
        wall: started.elapsed(),
        errors,
        kept,
        lines_lost: lost,
        clean_notes_changed,
        clean_notes_checked: clean_candidates.len(),
        proposals: summary.proposals,
        validation_failures: summary.validation_failures,
        confirmed: summary.confirmed,
        rejected: summary.rejected,
        applied: summary.applied,
        report: report_text,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn error() -> PlantedError {
        PlantedError {
            note: "notes/A.md".into(),
            kind: ErrorKind::Findable,
            stale: "kept for 14 days".into(),
            fixed: vec!["kept for 30 days".into(), "one month".into()],
        }
    }

    #[test]
    fn a_planted_error_is_corrected_stale_or_removed() {
        let error = error();
        assert_eq!(
            classify_error("Dumps are kept for 30 days.", &error),
            ErrorResult::Corrected
        );
        assert_eq!(
            classify_error("Dumps are kept for one month.", &error),
            ErrorResult::Corrected
        );
        assert_eq!(
            classify_error("Dumps are kept for 14 days.", &error),
            ErrorResult::Stale
        );
        // Both present: the wrong claim is still there.
        assert_eq!(
            classify_error("kept for 14 days. Later: kept for 30 days.", &error),
            ErrorResult::Stale
        );
        assert_eq!(
            classify_error("Dumps are kept.", &error),
            ErrorResult::Removed
        );
        assert_eq!(classify_error("", &error), ErrorResult::Removed);
    }

    #[test]
    fn a_kept_claim_is_kept_or_damaged() {
        let keep = KeptClaim {
            note: "notes/A.md".into(),
            text: "Backups are encrypted.".into(),
        };
        assert_eq!(
            classify_kept("Backups are encrypted. More.", &keep),
            KeptResult::Kept
        );
        assert_eq!(
            classify_kept("Backups are not encrypted.", &keep),
            KeptResult::Damaged
        );
        assert_eq!(classify_kept("", &keep), KeptResult::Damaged);
    }

    #[test]
    fn lines_lost_counts_lines_the_run_took_out() {
        let before = "# Note\n\nKept line.\nStale claim here.\nSecond line.\n   \nThird line.\n";
        let stale = ["Stale claim"];
        // Only the stale line changed: nothing lost.
        let fixed = "# Note\n\nKept line.\nFresh claim here.\nSecond line.\nThird line.\n";
        assert_eq!(lines_lost(before, fixed, &stale), 0);
        // Two other lines gone, one reworded.
        let damaged = "# Note\nKept line!\nThird line.\n";
        assert_eq!(lines_lost(before, damaged, &stale), 2);
        // The stale line itself gone counts for nothing; a missing note loses all.
        assert_eq!(lines_lost(before, "", &stale), 4);
    }

    #[test]
    fn the_summary_line_counts_each_outcome() {
        let row = |kind, result| ErrorRow {
            note: "notes/A.md".into(),
            stale: "s".into(),
            kind,
            result,
        };
        let result = DreamingResult {
            wall: Duration::from_secs(1),
            errors: vec![
                row(ErrorKind::Findable, ErrorResult::Corrected),
                row(ErrorKind::Findable, ErrorResult::Stale),
                row(ErrorKind::Long, ErrorResult::Removed),
            ],
            kept: vec![
                KeptRow {
                    note: "notes/A.md".into(),
                    text: "t".into(),
                    result: KeptResult::Damaged,
                },
                KeptRow {
                    note: "notes/A.md".into(),
                    text: "u".into(),
                    result: KeptResult::Kept,
                },
            ],
            lines_lost: 2,
            clean_notes_changed: 1,
            clean_notes_checked: 2,
            proposals: 4,
            validation_failures: 1,
            confirmed: 3,
            rejected: 1,
            applied: 3,
            report: String::new(),
        };
        assert_eq!(
            result.summary(),
            "dreaming: corrected 1/3, damaged 1/2, lines lost 2, clean notes changed 1/2; \
             proposals 4, validation failures 1, confirmed 3, rejected 1, applied 3"
        );
        assert_eq!(
            result.corrected_by_kind(),
            [(ErrorKind::Findable, 1, 2), (ErrorKind::Long, 0, 1)]
        );
    }
}
