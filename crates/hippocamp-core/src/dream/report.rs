//! Dreaming step 8: `.hippocamp/reports/<date>-dream.md`.
//!
//! Line 1 is `status: ok` or `status: failed`, line 2 `error: <text>` on
//! failure, then the sections `## Window`, `## Inputs`, `## Omissions`,
//! `## Proposals`, `## Validation failures`, `## Verdicts` and
//! `## Applied`, with one bullet per item. A run writes the report of its
//! UTC start date, `<date>-dream.md`. A later run on the same UTC day
//! replaces that report only when it is a dry run's; otherwise it writes
//! `<date>-<n>-dream.md` with the smallest `n` from 2 that is free or holds
//! a dry run's report, so no real run's report is ever replaced. Reports
//! order by date, then by `n` (the unnumbered one is 1).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use time::{Date, Month};
use ulid::Ulid;

use crate::config::Workspace;
use crate::error::{Error, Result};
use crate::records::Listed;
use crate::util::write_atomic;

/// The newest dreaming report and how its run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportStatus {
    pub path: PathBuf,
    /// The date in the file name.
    pub date: Date,
    /// The run's number on that date: 1 for `<date>-dream.md`, `n` for
    /// `<date>-<n>-dream.md`.
    pub seq: u32,
    /// Line 1 is `status: failed`.
    pub failed: bool,
    /// Line 2 without its `error: ` prefix, of a failed report.
    pub error: Option<String>,
    /// The run was a dry run (its `## Window` holds [`DRY_RUN_LINE`]): it
    /// wrote nothing and does not count as a dreaming run.
    pub dry_run: bool,
}

/// The `## Window` bullet a dry run writes.
pub const DRY_RUN_LINE: &str = "dry run: nothing is written but this report";

/// Whether a report's `## Window` section names a dry run.
fn is_dry_run(text: &str) -> bool {
    let mut in_window = false;
    for line in text.lines() {
        if line.starts_with("## ") {
            in_window = line == "## Window";
        } else if in_window && line.strip_prefix("- ") == Some(DRY_RUN_LINE) {
            return true;
        }
    }
    false
}

/// One dreaming report with the ids its Proposals and Applied lists name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportRun {
    pub status: ReportStatus,
    /// The first word of each bullet under `## Proposals`.
    pub proposals: Vec<Ulid>,
    /// The first word of each bullet under `## Applied`: receipt ids.
    pub applied: Vec<Ulid>,
}

/// What one run reports, section by section; each entry is one bullet.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// Set when the run failed.
    pub error: Option<String>,
    pub window: Vec<String>,
    pub inputs: Vec<String>,
    pub omissions: Vec<String>,
    /// `<proposal id> <note path> …`.
    pub proposals: Vec<String>,
    pub validation_failures: Vec<String>,
    pub verdicts: Vec<String>,
    /// `<receipt id> <snapshot path>`.
    pub applied: Vec<String>,
}

impl Report {
    /// The report's text. Every bullet and the error are one line each.
    pub fn render(&self) -> String {
        let mut out = String::new();
        match &self.error {
            None => out.push_str("status: ok\n"),
            Some(error) => {
                out.push_str("status: failed\n");
                out.push_str(&format!("error: {}\n", one_line(error)));
            }
        }
        let sections = [
            ("Window", &self.window),
            ("Inputs", &self.inputs),
            ("Omissions", &self.omissions),
            ("Proposals", &self.proposals),
            ("Validation failures", &self.validation_failures),
            ("Verdicts", &self.verdicts),
            ("Applied", &self.applied),
        ];
        for (heading, items) in sections {
            out.push_str(&format!("\n## {heading}\n"));
            if !items.is_empty() {
                out.push('\n');
                for item in items {
                    out.push_str(&format!("- {}\n", one_line(item)));
                }
            }
        }
        out
    }

    /// Writes the report as `.hippocamp/reports/<date>-dream.md` atomically and
    /// returns that path. When that name holds a real (not dry) run's
    /// report, it writes `<date>-<n>-dream.md` instead, with the smallest `n`
    /// from 2 whose file is missing or a dry run's report.
    pub fn write(&self, ws: &Workspace, date: Date) -> Result<PathBuf> {
        let dir = ws.hippocamp_dir().join("reports");
        fs::create_dir_all(&dir).map_err(|e| Error::io_at(&dir, e))?;
        let mut seq = 1u32;
        let path = loop {
            let path = dir.join(file_name(date, seq));
            match fs::read_to_string(&path) {
                Err(err) if err.kind() == io::ErrorKind::NotFound => break path,
                Ok(text) if is_dry_run(&text) => break path,
                // A real run's report, or one that cannot be read: keep it.
                _ => seq += 1,
            }
        };
        write_atomic(&path, self.render().as_bytes())?;
        Ok(path)
    }
}

/// `<date>-dream.md` for the day's first report, `<date>-<n>-dream.md` after.
fn file_name(date: Date, seq: u32) -> String {
    if seq <= 1 {
        format!("{date}-dream.md")
    } else {
        format!("{date}-{seq}-dream.md")
    }
}

/// The date and number of a report file name, `<YYYY-MM-DD>-dream.md`
/// (number 1) or `<YYYY-MM-DD>-<n>-dream.md` with `n` from 2 written without
/// leading zeros; `None` for any other name, a hidden one included.
fn name_date(name: &str) -> Option<(Date, u32)> {
    let stem = name.strip_suffix("-dream.md")?;
    let mut parts = stem.splitn(4, '-');
    let year: i32 = parts.next().filter(|y| y.len() == 4)?.parse().ok()?;
    let month: u8 = parts.next().filter(|m| m.len() == 2)?.parse().ok()?;
    let day: u8 = parts.next().filter(|d| d.len() == 2)?.parse().ok()?;
    let seq = match parts.next() {
        None => 1,
        Some(n) if !n.starts_with('0') && n.bytes().all(|b| b.is_ascii_digit()) => {
            n.parse().ok().filter(|n| *n >= 2)?
        }
        Some(_) => return None,
    };
    let date = Date::from_calendar_date(year, Month::try_from(month).ok()?, day).ok()?;
    Some((date, seq))
}

/// The dated reports in `.hippocamp/reports/`, newest first: by date, then by
/// number. A missing directory is none.
fn report_files(ws: &Workspace) -> Result<Vec<((Date, u32), PathBuf)>> {
    let dir = ws.hippocamp_dir().join("reports");
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(Error::io_at(&dir, err)),
    };
    let mut files: Vec<((Date, u32), PathBuf)> = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let date = name_date(&entry.file_name().to_string_lossy())?;
            Some((date, entry.path()))
        })
        .collect();
    files.sort_by(|a, b| b.cmp(a));
    Ok(files)
}

/// The path of the newest dreaming report, the one with the greatest date
/// and number; `None` when the directory is missing, cannot be
/// read, or holds none.
pub(crate) fn newest_path(ws: &Workspace) -> Option<PathBuf> {
    report_files(ws)
        .ok()?
        .into_iter()
        .next()
        .map(|(_, path)| path)
}

/// Whether any dreaming report is a real run's, not a dry run's: before
/// one, a missing success is "not set up yet".
pub(crate) fn any_real_run(ws: &Workspace) -> bool {
    report_files(ws).is_ok_and(|files| {
        files
            .iter()
            .any(|(_, path)| fs::read_to_string(path).is_ok_and(|text| !is_dry_run(&text)))
    })
}

/// How the newest dreaming report says its run ended. `Ok(None)` when the
/// reports directory is missing or cannot be read, or holds no report; an
/// error naming the file when the newest report cannot be read. A report
/// counts as failed when it starts `status: failed`, whatever else it holds.
pub fn newest(ws: &Workspace) -> Result<Option<ReportStatus>> {
    let Some(path) = newest_path(ws) else {
        return Ok(None);
    };
    let text = fs::read_to_string(&path).map_err(|e| Error::io_at(&path, e))?;
    Ok(status_of(&path, &text))
}

fn status_of(path: &Path, text: &str) -> Option<ReportStatus> {
    let (date, seq) = path
        .file_name()
        .and_then(|n| name_date(&n.to_string_lossy()))?;
    let failed = text.starts_with("status: failed");
    let error = failed.then(|| {
        let line = text.lines().nth(1).unwrap_or_default();
        line.strip_prefix("error: ").unwrap_or(line).to_owned()
    });
    Some(ReportStatus {
        path: path.to_path_buf(),
        date,
        seq,
        failed,
        error,
        dry_run: is_dry_run(text),
    })
}

/// Every dreaming report, newest first (by date, then number), parsed from what
/// [`Report::render`] writes: line 1 `status: ok` or `status: failed`, line 2
/// `error: <text>` when failed, and under `## Proposals` and `## Applied`
/// the first word of each bullet as an id (the rest of a bullet, a note
/// path that may hold spaces, is ignored). A report that does not parse is
/// an entry holding its error. A missing directory is an empty list.
pub fn list(ws: &Workspace) -> Result<Vec<Listed<ReportRun>>> {
    Ok(report_files(ws)?
        .into_iter()
        .map(|((date, seq), path)| {
            let record = fs::read_to_string(&path)
                .map_err(|err| err.to_string())
                .and_then(|text| parse(&path, date, seq, &text));
            Listed { path, record }
        })
        .collect())
}

fn parse(path: &Path, date: Date, seq: u32, text: &str) -> std::result::Result<ReportRun, String> {
    let mut lines = text.lines();
    let (failed, error) = match lines.next() {
        Some("status: ok") => (false, None),
        Some("status: failed") => {
            let line = lines.next().unwrap_or_default();
            let error = line
                .strip_prefix("error: ")
                .ok_or_else(|| "a failed report has no `error:` line 2".to_owned())?;
            (true, Some(error.to_owned()))
        }
        _ => return Err("line 1 is not `status: ok` or `status: failed`".to_owned()),
    };
    let ids = |heading: &str| -> std::result::Result<Vec<Ulid>, String> {
        let marker = format!("## {heading}");
        let mut in_section = false;
        let mut ids = Vec::new();
        for line in text.lines() {
            if line.starts_with("## ") {
                in_section = line == marker;
            } else if let (true, Some(bullet)) = (in_section, line.strip_prefix("- ")) {
                let word = bullet.split_whitespace().next().unwrap_or_default();
                let id = word
                    .parse::<Ulid>()
                    .map_err(|_| format!("{heading}: {word:?} is not an id"))?;
                ids.push(id);
            }
        }
        Ok(ids)
    };
    Ok(ReportRun {
        status: ReportStatus {
            path: path.to_path_buf(),
            date,
            seq,
            failed,
            error,
            dry_run: is_dry_run(text),
        },
        proposals: ids("Proposals")?,
        applied: ids("Applied")?,
    })
}

/// `text` with every run of whitespace, line breaks included, as one space.
pub(crate) fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::date;

    #[test]
    fn an_ok_report_has_every_section_in_order() {
        let report = Report {
            window: vec!["2026-11-07T03:30:00Z to 2026-11-08T03:30:00Z: done".into()],
            proposals: vec!["01JC9X4Q3A8M2K7N5P6R8S9T0V notes/Topic.md (verify, replace)".into()],
            ..Report::default()
        };
        assert_eq!(
            report.render(),
            "status: ok\n\
             \n## Window\n\n- 2026-11-07T03:30:00Z to 2026-11-08T03:30:00Z: done\n\
             \n## Inputs\n\
             \n## Omissions\n\
             \n## Proposals\n\n- 01JC9X4Q3A8M2K7N5P6R8S9T0V notes/Topic.md (verify, replace)\n\
             \n## Validation failures\n\
             \n## Verdicts\n\
             \n## Applied\n"
        );
    }

    #[test]
    fn a_failed_report_puts_the_error_on_line_2_and_flattens_lines() {
        let report = Report {
            error: Some("model error:\nclaude exited".into()),
            verdicts: vec!["a\n  b".into()],
            ..Report::default()
        };
        let text = report.render();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "status: failed");
        assert_eq!(lines[1], "error: model error: claude exited");
        assert!(text.contains("## Verdicts\n\n- a b\n"));
    }

    #[test]
    fn write_never_replaces_a_real_runs_report_from_the_same_day() {
        let dir = tempfile::tempdir().unwrap();
        let ws = crate::dream::tests::workspace_at(dir.path());
        let reports = ws.hippocamp_dir().join("reports");
        let dry = Report {
            window: vec![DRY_RUN_LINE.to_owned()],
            ..Report::default()
        };
        let day = date!(2026 - 11 - 08);
        // A dry run's report is replaced, by a dry run or a real run.
        let first = dry.write(&ws, day).unwrap();
        assert_eq!(first, reports.join("2026-11-08-dream.md"));
        assert_eq!(Report::default().write(&ws, day).unwrap(), first);
        // A real run's report is kept: the next run takes the next number.
        let failed = Report {
            error: Some("boom".into()),
            ..Report::default()
        };
        let second = failed.write(&ws, day).unwrap();
        assert_eq!(second, reports.join("2026-11-08-2-dream.md"));
        assert!(fs::read_to_string(&first)
            .unwrap()
            .starts_with("status: ok\n"));
        let third = dry.write(&ws, day).unwrap();
        assert_eq!(third, reports.join("2026-11-08-3-dream.md"));
        assert_eq!(Report::default().write(&ws, day).unwrap(), third);
        // Newest is the greatest number on the newest date, 10 after 9.
        put(&ws, "2026-11-08-9-dream.md", "status: ok\n");
        put(&ws, "2026-11-08-10-dream.md", "status: failed\nerror: x\n");
        let top = newest(&ws).unwrap().unwrap();
        assert!(top.path.ends_with("2026-11-08-10-dream.md"));
        assert_eq!(top.seq, 10);
        let listed: Vec<u32> = list(&ws)
            .unwrap()
            .into_iter()
            .map(|r| r.record.unwrap().status.seq)
            .collect();
        assert_eq!(listed, vec![10, 9, 3, 2, 1]);
        put(&ws, "2026-11-09-dream.md", "status: ok\n");
        assert_eq!(newest(&ws).unwrap().unwrap().seq, 1);
    }

    #[test]
    fn name_date_reads_numbers_and_refuses_other_names() {
        let d = date!(2026 - 11 - 08);
        assert_eq!(name_date("2026-11-08-dream.md"), Some((d, 1)));
        assert_eq!(name_date("2026-11-08-2-dream.md"), Some((d, 2)));
        assert_eq!(name_date("2026-11-08-12-dream.md"), Some((d, 12)));
        for bad in [
            "2026-11-08-1-dream.md",
            "2026-11-08-02-dream.md",
            "2026-11-08-x-dream.md",
            "2026-11-08--dream.md",
            "2026-11-08-2-3-dream.md",
            ".2026-11-08-dream.md",
        ] {
            assert_eq!(name_date(bad), None, "{bad}");
        }
    }

    fn workspace(dir: &Path) -> Workspace {
        crate::dream::tests::workspace_at(dir)
    }

    fn put(ws: &Workspace, name: &str, text: &str) {
        let dir = ws.hippocamp_dir().join("reports");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(name), text).unwrap();
    }

    #[test]
    fn newest_is_none_without_a_report_or_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        let ws = workspace(dir.path());
        assert_eq!(newest(&ws).unwrap(), None);
        assert!(list(&ws).unwrap().is_empty());
        put(&ws, "2026-11-01-eval.md", "an eval report\n");
        put(&ws, ".2026-11-09-dream.md", "status: failed\n");
        put(&ws, "notes-dream.md", "status: failed\n");
        assert_eq!(newest(&ws).unwrap(), None);
        assert!(list(&ws).unwrap().is_empty());
    }

    #[test]
    fn newest_reads_the_status_and_error_of_the_greatest_date() {
        let dir = tempfile::tempdir().unwrap();
        let ws = workspace(dir.path());
        put(&ws, "2026-11-07-dream.md", "status: failed\nerror: old\n");
        put(&ws, "2026-11-08-dream.md", "status: ok\n");
        let status = newest(&ws).unwrap().unwrap();
        assert_eq!(status.date, date!(2026 - 11 - 08));
        assert!(!status.failed && status.error.is_none());
        assert!(status.path.ends_with("2026-11-08-dream.md"));

        put(
            &ws,
            "2026-11-09-dream.md",
            "status: failed\nerror: model error: boom\n",
        );
        let status = newest(&ws).unwrap().unwrap();
        assert_eq!(status.date, date!(2026 - 11 - 09));
        assert!(status.failed);
        assert_eq!(status.error.as_deref(), Some("model error: boom"));
        assert!(!status.dry_run);
    }

    #[test]
    fn a_dry_run_is_marked_by_its_window_bullet_only() {
        let dir = tempfile::tempdir().unwrap();
        let ws = workspace(dir.path());
        let dry = format!("status: ok\n\n## Window\n\n- {DRY_RUN_LINE}\n- a to b: done\n");
        put(&ws, "2026-11-08-dream.md", &dry);
        assert!(newest(&ws).unwrap().unwrap().dry_run);
        // The same words elsewhere, such as in an input's note path, are not
        // a dry run.
        let not = format!("status: ok\n\n## Inputs\n\n- {DRY_RUN_LINE}\n");
        put(&ws, "2026-11-09-dream.md", &not);
        assert!(!newest(&ws).unwrap().unwrap().dry_run);
    }

    #[test]
    fn newest_names_the_file_it_cannot_read() {
        let dir = tempfile::tempdir().unwrap();
        let ws = workspace(dir.path());
        // A directory with a report's name cannot be read as one.
        fs::create_dir_all(ws.hippocamp_dir().join("reports/2026-11-09-dream.md")).unwrap();
        let err = newest(&ws).unwrap_err();
        assert!(err.to_string().contains("2026-11-09-dream.md"), "{err}");
    }

    #[test]
    fn list_parses_what_render_writes() {
        let dir = tempfile::tempdir().unwrap();
        let ws = workspace(dir.path());
        let p1 = Ulid::from_parts(1_700_000_000_000, 1);
        let p2 = Ulid::from_parts(1_700_000_000_000, 2);
        let r1 = Ulid::from_parts(1_700_000_001_000, 1);
        let report = Report {
            proposals: vec![
                format!("{p1} notes/Topic A.md (verify, replace)"),
                format!("{p2} notes/Sub dir/Topic B.md (verify, append)"),
            ],
            verdicts: vec![format!("{p1} confirm: ok"), "- not an id".into()],
            applied: vec![format!("{r1} .hippocamp/snapshots/{p1}/notes/Topic A.md")],
            ..Report::default()
        };
        report.write(&ws, date!(2026 - 11 - 08)).unwrap();
        let failed = Report {
            error: Some("model error:\nboom".into()),
            ..Report::default()
        };
        failed.write(&ws, date!(2026 - 11 - 09)).unwrap();

        let runs = list(&ws).unwrap();
        assert_eq!(runs.len(), 2);
        let newer = runs[0].record.as_ref().unwrap();
        assert_eq!(newer.status.date, date!(2026 - 11 - 09));
        assert!(newer.status.failed);
        assert_eq!(newer.status.error.as_deref(), Some("model error: boom"));
        assert!(newer.proposals.is_empty() && newer.applied.is_empty());
        let older = runs[1].record.as_ref().unwrap();
        assert_eq!(older.status.date, date!(2026 - 11 - 08));
        assert!(!older.status.failed);
        assert_eq!(older.proposals, [p1, p2]);
        assert_eq!(older.applied, [r1]);
    }

    #[test]
    fn a_report_that_does_not_parse_is_an_entry_with_its_error() {
        let dir = tempfile::tempdir().unwrap();
        let ws = workspace(dir.path());
        put(&ws, "2026-11-06-dream.md", "hello\n");
        put(&ws, "2026-11-07-dream.md", "status: failed\n");
        put(
            &ws,
            "2026-11-08-dream.md",
            "status: ok\n\n## Proposals\n\n- nonsense notes/a.md\n",
        );
        let runs = list(&ws).unwrap();
        assert_eq!(runs.len(), 3);
        for run in &runs {
            assert!(run.record.is_err(), "{:?}", run.path);
        }
        assert!(runs[0].record.as_ref().unwrap_err().contains("Proposals"));
    }
}
