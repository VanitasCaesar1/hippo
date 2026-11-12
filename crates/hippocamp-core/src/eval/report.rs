//! The eval report: Markdown on stdout. The
//! `## Summary` lines are the stable part that tests and baselines compare.

use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use super::context::{stem, ContextResult};
use super::dreaming::DreamingResult;
use super::recall::RecallResult;
use super::Models;
use crate::dream::report::one_line;
use crate::redact::redact;

/// What one suite did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome<T> {
    Ran(T),
    /// Not run, and why.
    Skipped(String),
    /// Run and failed, with the error.
    Failed(String),
}

/// A whole eval run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// The set directory as the caller gave it.
    pub set: String,
    /// See [`Set::hash`](super::set::Set::hash).
    pub set_hash: String,
    /// See [`core_pack_hash`](super::core_pack_hash).
    pub core_pack_hash: String,
    /// The hippocamp version that ran.
    pub version: String,
    /// The commands a model run used; `None` for a run without a model.
    pub models: Option<Models>,
    pub started: OffsetDateTime,
    pub context: Outcome<ContextResult>,
    pub recall: Outcome<RecallResult>,
    pub dreaming: Outcome<DreamingResult>,
    /// When the dreaming run failed, the report it wrote about itself, if
    /// it wrote one.
    pub failed_dreaming_report: Option<String>,
}

impl Report {
    /// Whether any suite failed. A skipped suite is not a failure.
    pub fn failed(&self) -> bool {
        matches!(self.context, Outcome::Failed(_))
            || matches!(self.recall, Outcome::Failed(_))
            || matches!(self.dreaming, Outcome::Failed(_))
    }

    /// The report as Markdown.
    pub fn render(&self) -> String {
        let started = self
            .started
            .replace_nanosecond(0)
            .expect("zero is a valid nanosecond")
            .format(&Rfc3339)
            .expect("RFC 3339 formats every in-range date");
        let mut out = String::from("# hippocamp eval\n\n");
        out.push_str(&format!(
            "- set: {} (sha256 {})\n- hippocamp: {}\n- core pack: sha256 {}\n",
            self.set, self.set_hash, self.version, self.core_pack_hash
        ));
        match &self.models {
            None => out.push_str("- model: none\n"),
            Some(models) => {
                // The command goes into reports that may be committed, so it
                // is redacted like any text hippocamp stores.
                let command = models.model_command.join(" ");
                out.push_str(&format!("- model: {}\n", redact(&command).as_str()));
                if let Some(reviewer) = &models.reviewer_command_override {
                    let command = reviewer.join(" ");
                    out.push_str(&format!("- reviewer: {}\n", redact(&command).as_str()));
                }
            }
        }
        out.push_str(&format!("- started: {started}\n"));

        out.push_str("\n## Summary\n\n```text\n");
        out.push_str(&summary_line(
            "context",
            &self.context,
            ContextResult::summary,
        ));
        out.push_str(&summary_line("recall", &self.recall, RecallResult::summary));
        out.push_str(&summary_line(
            "dreaming",
            &self.dreaming,
            DreamingResult::summary,
        ));
        out.push_str("```\n");

        out.push_str("\n## Context\n\n");
        match &self.context {
            Outcome::Ran(result) => out.push_str(&context_table(result)),
            other => out.push_str(&not_run(other)),
        }
        out.push_str("\n## Recall\n\n");
        match &self.recall {
            Outcome::Ran(result) => out.push_str(&recall_table(result)),
            other => out.push_str(&not_run(other)),
        }
        out.push_str("\n## Dreaming\n\n");
        match &self.dreaming {
            Outcome::Ran(result) => out.push_str(&dreaming_section(result)),
            other => {
                out.push_str(&not_run(other));
                if let Some(report) = &self.failed_dreaming_report {
                    out.push_str(&format!(
                        "\nThe failed run's own report:\n\n{}",
                        fenced(report)
                    ));
                }
            }
        }
        out
    }
}

/// `<suite>: <summary>`, `<suite>: skipped: <reason>` or
/// `<suite>: failed: <error>`, with its newline.
fn summary_line<T>(suite: &str, outcome: &Outcome<T>, summary: fn(&T) -> String) -> String {
    match outcome {
        Outcome::Ran(result) => format!("{}\n", summary(result)),
        Outcome::Skipped(reason) => format!("{suite}: skipped: {}\n", one_line(reason)),
        Outcome::Failed(error) => format!("{suite}: failed: {}\n", one_line(error)),
    }
}

/// A section body for a suite that did not run.
fn not_run<T>(outcome: &Outcome<T>) -> String {
    match outcome {
        Outcome::Failed(error) => format!("(failed: {})\n", one_line(error)),
        _ => "(skipped)\n".to_owned(),
    }
}

/// One table cell: one line, with a backslash before each character
/// Markdown would read as formatting or as the end of the cell.
fn cell(text: &str) -> String {
    let mut out = String::new();
    for c in one_line(text).chars() {
        if "\\`*_<>[]|".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn names(paths: &[std::path::PathBuf]) -> String {
    let names: Vec<String> = paths.iter().map(|path| stem(path)).collect();
    if names.is_empty() {
        "-".to_owned()
    } else {
        names.join(", ")
    }
}

fn context_table(result: &ContextResult) -> String {
    let mut out = String::from("| # | task | expected | listed | found |\n|---|---|---|---|---|\n");
    for (i, row) in result.rows.iter().enumerate() {
        let listed = if row.omitted {
            "omitted".to_owned()
        } else {
            names(&row.listed)
        };
        out.push_str(&format!(
            "| {} | {} | {} | {} | {}/{} |\n",
            i + 1,
            cell(&row.task),
            cell(&names(&row.expected)),
            cell(&listed),
            row.found,
            row.expected.len()
        ));
    }
    out
}

fn recall_table(result: &RecallResult) -> String {
    let mut out = String::from("| # | query | kind | rank |\n|---|---|---|---|\n");
    for (i, row) in result.rows.iter().enumerate() {
        let rank = row.rank.map_or("-".to_owned(), |rank| rank.to_string());
        out.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            i + 1,
            cell(&row.query),
            row.kind.as_str(),
            rank
        ));
    }
    out
}

/// `text` in a fenced code block whose fence is longer than any run of
/// backticks inside it.
fn fenced(text: &str) -> String {
    let longest = text.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat((longest + 1).max(3));
    let newline = if text.ends_with('\n') { "" } else { "\n" };
    format!("{fence}markdown\n{text}{newline}{fence}\n")
}

/// The dreaming section: wall time, the two tables, corrections by kind
/// and the run's report in a fenced block.
fn dreaming_section(result: &DreamingResult) -> String {
    let mut out = format!(
        "Wall time: {:.1} s. One sample: a small difference from another run means little.\n\n",
        result.wall.as_secs_f64()
    );
    out.push_str("### Planted errors\n\n");
    out.push_str("| # | note | kind | stale text | result |\n|---|---|---|---|---|\n");
    for (i, row) in result.errors.iter().enumerate() {
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            i + 1,
            cell(&stem(&row.note)),
            row.kind.as_str(),
            cell(&row.stale),
            row.result.as_str()
        ));
    }
    let kinds: Vec<String> = result
        .corrected_by_kind()
        .iter()
        .map(|(kind, corrected, total)| format!("{} {corrected}/{total}", kind.as_str()))
        .collect();
    out.push_str(&format!("\nBy kind: {}.\n", kinds.join(", ")));
    out.push_str("\n### Kept claims\n\n| # | note | text | result |\n|---|---|---|---|\n");
    for (i, row) in result.kept.iter().enumerate() {
        out.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            i + 1,
            cell(&stem(&row.note)),
            cell(&row.text),
            row.result.as_str()
        ));
    }
    out.push_str("\n### Dreaming run report\n\n");
    out.push_str(&fenced(&result.report));
    out
}

#[cfg(test)]
mod tests {
    use super::super::context::ContextRow;
    use super::super::recall::RecallRow;
    use super::super::set::RecallKind;
    use super::*;
    use std::path::PathBuf;
    use time::macros::datetime;

    fn report() -> Report {
        let note = |name: &str| PathBuf::from(format!("notes/{name}.md"));
        let context = ContextResult {
            rows: vec![
                ContextRow::new(
                    "the share link | opens a blank page",
                    &[note("Sharing links")],
                    vec![note("Sharing links"), note("Upload API")],
                    false,
                ),
                ContextRow::new("write a `csv_file` <exporter> [x]", &[], vec![], true),
            ],
        };
        let recall = RecallResult {
            rows: vec![
                RecallRow {
                    query: "thumbnail queue drained".into(),
                    kind: RecallKind::Verbatim,
                    rank: Some(1),
                },
                RecallRow {
                    query: "meaning only".into(),
                    kind: RecallKind::Paraphrase,
                    rank: None,
                },
            ],
        };
        Report {
            set: "fixtures/eval".into(),
            set_hash: "3f2a9c01b7d4".into(),
            core_pack_hash: "a1b2c3d4e5f6".into(),
            version: "0.0.1".into(),
            models: None,
            started: datetime!(2026-11-08 07:00:00.5 UTC),
            context: Outcome::Ran(context),
            recall: Outcome::Ran(recall),
            dreaming: Outcome::Skipped("the set has no [dreaming]".into()),
            failed_dreaming_report: None,
        }
    }

    #[test]
    fn golden_rendering() {
        let expected = "\
# hippocamp eval

- set: fixtures/eval (sha256 3f2a9c01b7d4)
- hippocamp: 0.0.1
- core pack: sha256 a1b2c3d4e5f6
- model: none
- started: 2026-11-08T07:00:00Z

## Summary

```text
context: found 1/1, precision 1/2, complete 1/1, quiet 1/1
recall: hit@1 1/2, hit@5 1/2, hit@20 1/2, mrr 0.50 (verbatim 1/1, paraphrase 0/1)
dreaming: skipped: the set has no [dreaming]
```

## Context

| # | task | expected | listed | found |
|---|---|---|---|---|
| 1 | the share link \\| opens a blank page | Sharing links | Sharing links, Upload API | 1/1 |
| 2 | write a \\`csv\\_file\\` \\<exporter\\> \\[x\\] | - | omitted | 0/0 |

## Recall

| # | query | kind | rank |
|---|---|---|---|
| 1 | thumbnail queue drained | verbatim | 1 |
| 2 | meaning only | paraphrase | - |

## Dreaming

(skipped)
";
        let report = report();
        assert_eq!(report.render(), expected);
        assert!(!report.failed());
    }

    #[test]
    fn a_dreaming_run_renders_tables_and_a_longer_fence() {
        use super::super::dreaming::{ErrorResult, ErrorRow, KeptResult, KeptRow};
        use super::super::set::ErrorKind;
        let mut report = report();
        report.dreaming = Outcome::Ran(DreamingResult {
            wall: std::time::Duration::from_millis(12_340),
            errors: vec![
                ErrorRow {
                    note: "notes/Backups.md".into(),
                    stale: "kept for 14 days".into(),
                    kind: ErrorKind::Findable,
                    result: ErrorResult::Corrected,
                },
                ErrorRow {
                    note: "notes/Release process.md".into(),
                    stale: "soak for three days".into(),
                    kind: ErrorKind::Long,
                    result: ErrorResult::Stale,
                },
            ],
            kept: vec![KeptRow {
                note: "notes/Backups.md".into(),
                text: "Backups are encrypted.".into(),
                result: KeptResult::Kept,
            }],
            lines_lost: 0,
            clean_notes_changed: 0,
            clean_notes_checked: 2,
            proposals: 1,
            validation_failures: 0,
            confirmed: 1,
            rejected: 0,
            applied: 1,
            report: "status: ok\n- a ```code``` span\n".into(),
        });
        let text = report.render();
        assert!(
            text.contains(
                "dreaming: corrected 1/2, damaged 0/1, lines lost 0, clean notes changed 0/2; \
                 proposals 1, validation failures 0, confirmed 1, rejected 0, applied 1\n"
            ),
            "{text}"
        );
        assert!(text.contains("Wall time: 12.3 s."), "{text}");
        assert!(
            text.contains("| 1 | Backups | findable | kept for 14 days | corrected |\n"),
            "{text}"
        );
        assert!(
            text.contains("| 2 | Release process | long | soak for three days | stale |\n"),
            "{text}"
        );
        assert!(
            text.contains("\nBy kind: findable 1/1, long 0/1.\n"),
            "{text}"
        );
        assert!(
            text.contains("| 1 | Backups | Backups are encrypted. | kept |\n"),
            "{text}"
        );
        assert!(
            text.ends_with("````markdown\nstatus: ok\n- a ```code``` span\n````\n"),
            "{text}"
        );
    }

    #[test]
    fn a_failed_dreaming_run_shows_its_own_report() {
        let mut report = report();
        report.dreaming = Outcome::Failed("model error: timed out".into());
        report.failed_dreaming_report =
            Some("status: failed\nerror: model error: timed out\n".into());
        let text = report.render();
        assert!(report.failed());
        assert!(
            text.contains("\ndreaming: failed: model error: timed out\n"),
            "{text}"
        );
        assert!(
            text.ends_with(
                "## Dreaming\n\n(failed: model error: timed out)\n\nThe failed run's own report:\n\n\
                 ```markdown\nstatus: failed\nerror: model error: timed out\n```\n"
            ),
            "{text}"
        );
    }

    #[test]
    fn a_failed_suite_shows_its_error_and_fails_the_report() {
        let mut report = report();
        report.recall = Outcome::Failed("index\nbroke".into());
        report.models = Some(Models {
            model_command: vec!["claude".into(), "-p".into()],
            reviewer_command_override: Some(vec!["rev".into()]),
        });
        let text = report.render();
        assert!(report.failed());
        assert!(
            text.contains("- model: claude -p\n- reviewer: rev\n- started"),
            "{text}"
        );
        assert!(text.contains("\nrecall: failed: index broke\n"), "{text}");
        assert!(
            text.contains("## Recall\n\n(failed: index broke)\n"),
            "{text}"
        );
    }
}
