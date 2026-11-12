//! Hippo's status, as words: the dreaming status, the warnings agents get,
//! the last ingest, the installation in use and a theme file's problem.
//! There is no status bar: the sidebar's foot shows the health and
//! `indexed hh:mm`, the Memory pane the dreaming line. The text is made by
//! [`model`], with no window code, so that it can be tested.

use std::sync::OnceLock;

use hippo_view::status::Status;
use time::macros::format_description;
use time::{OffsetDateTime, UtcOffset};

use super::Shell;
use crate::cli::InstallNote;
use crate::themes::Notice;

static LOCAL_OFFSET: OnceLock<UtcOffset> = OnceLock::new();

/// Reads the system's UTC offset. Call it once, at the top of `main`, before
/// any thread starts: the `time` crate refuses to read it once there are
/// several.
pub fn init_local_offset() {
    LOCAL_OFFSET.get_or_init(|| UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC));
}

/// The system's UTC offset, or UTC when it could not be read.
pub fn local_offset() -> UtcOffset {
    LOCAL_OFFSET.get().copied().unwrap_or(UtcOffset::UTC)
}

/// A time as the status bar shows it: `HH:MM` for today and `DD Mon` before,
/// in the local time zone given by `offset`.
pub fn format_when(t: OffsetDateTime, now: OffsetDateTime, offset: UtcOffset) -> String {
    plain_when(t, now, offset)
}

/// A warning line agents get (`context::warnings`), explained for a
/// person: what it means, and the command that clears it when there is one.
pub fn explain_warning(line: &str, dream_at: &str, offset: UtcOffset) -> (String, Option<String>) {
    let line = line.trim();
    if line == "warning: last dreaming run failed" {
        return (
            "The last dreaming run failed. Its report says why (⌘K D); the next nightly run tries again.".into(),
            Some("hippo doctor".into()),
        );
    }
    if line == "warning: no dreaming run recorded" {
        return (
            format!(
                "No dreaming run has finished yet: a dry run does not count. The nightly job runs at {dream_at}, or run one now."
            ),
            Some("hippo dream".into()),
        );
    }
    if let Some(at) = line.strip_prefix("warning: last dreaming success ") {
        let when = OffsetDateTime::parse(at, &time::format_description::well_known::Rfc3339)
            .ok()
            .and_then(|t| {
                t.to_offset(offset)
                    .format(format_description!(
                        "[day] [month repr:short] [hour]:[minute]"
                    ))
                    .ok()
            })
            .unwrap_or_else(|| at.to_string());
        return (
            format!(
                "The last successful dreaming run was {when}, more than 36 hours ago: the nightly job may not be running."
            ),
            Some("hippo doctor".into()),
        );
    }
    (line.trim_start_matches("warning: ").to_string(), None)
}

/// Whether a warning line agents get is about dreaming, which the dreaming
/// status says itself rather than as a separate warning count.
pub fn is_dreaming_warning(line: &str) -> bool {
    let line = line.trim();
    line == "warning: last dreaming run failed"
        || line == "warning: no dreaming run recorded"
        || line.starts_with("warning: last dreaming success ")
}

/// What the status bar says, as text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusModel {
    /// Dreaming, in one item that agrees with the warnings agents get:
    /// `dream ok 08 Nov`, `dream failed 08 Nov`, `dream: dry run only, 08
    /// Nov`, `dream overdue, last ok 05 Nov`.
    pub dreaming: String,
    pub dreaming_failed: bool,
    /// A problem short of a failure: only a dry run, or overdue.
    pub dreaming_warning: bool,
    /// The dreaming warning lines the dreaming item stands for.
    pub dreaming_lines: Vec<String>,
    /// The count of the other warning lines agents get, when there are any.
    pub warnings: Option<String>,
    /// `ingest 12:40`, marked when stale.
    pub ingest: Option<String>,
    /// The index is still at format 1.
    pub index_note: Option<String>,
    /// A running revert.
    pub revert: Option<String>,
    /// The installation note: `dev home` or `default hippo`.
    pub installation: Option<InstallNote>,
    /// Problems reading the data, joined.
    pub errors: Option<String>,
    /// Something to say about the theme: the saved one is gone, the file in
    /// use stopped parsing, the choice could not be saved.
    pub theme_notice: Option<Notice>,
}

/// What the newest dreaming run did, from its report's run: when it was
/// written and how many changes it confirmed or applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunFacts {
    pub written: Option<OffsetDateTime>,
    pub changes: usize,
}

/// `00:38`, `Mon 03:30`, `30 Oct 03:30`: the app's one date form.
fn plain_when(t: OffsetDateTime, now: OffsetDateTime, offset: UtcOffset) -> String {
    hippo_view::fleet::when(t.unix_timestamp(), now, offset)
}

/// `, 1 change`, `, 4 changes` after `dreamed 03:31`; nothing for none
/// (an empty fact is absent).
fn changes_words(n: usize) -> String {
    match n {
        0 => String::new(),
        1 => ", 1 change".into(),
        n => format!(", {n} changes"),
    }
}

/// Says the newest run in words once its run is read: `dreamed 00:38, 4
/// changes`, `dreamed Mon 03:30`, `dream failed 03:30`. A report's file date is the UTC day the run's window ends,
/// which a reader takes for the wrong night; the time it was written is not.
/// Dry runs and overdue runs keep their words.
pub fn describe_run(
    m: &mut StatusModel,
    run: Option<RunFacts>,
    now: OffsetDateTime,
    offset: UtcOffset,
) {
    let Some(run) = run else { return };
    let Some(written) = run.written else { return };
    let when = plain_when(written, now, offset);
    if m.dreaming_failed {
        m.dreaming = format!("dream failed {when}");
    } else if m.dreaming.starts_with("dream ok ") && !m.dreaming.contains(',') {
        m.dreaming = format!("dreamed {when}{}", changes_words(run.changes));
    } else if m.dreaming.starts_with("dream: dry run ") && !m.dreaming.contains("only") {
        m.dreaming = format!("dream dry run {when}");
    }
}

/// `dreamed 03:31, 4 changes`: the last real run (a newer
/// dry run is in Inbox › Dreaming runs, not here); `None` without one.
pub fn real_and_dry(
    real: Option<RunFacts>,
    _dry: Option<OffsetDateTime>,
    now: OffsetDateTime,
    offset: UtcOffset,
) -> Option<String> {
    let real = real?;
    let when = plain_when(real.written?, now, offset);
    let s = format!("dreamed {when}{}", changes_words(real.changes));
    Some(s)
}

/// Builds the status bar's text. `status` is `None` before the first reload
/// and when hippo is not set up.
pub fn model(
    status: Option<&Status>,
    now: OffsetDateTime,
    offset: UtcOffset,
    installation: Option<InstallNote>,
    revert: Option<String>,
    theme_notice: Option<Notice>,
) -> StatusModel {
    let lines: &[String] = status.map_or(&[], |s| s.warnings.as_slice());
    let dreaming_lines: Vec<String> = lines
        .iter()
        .filter(|l| is_dreaming_warning(l))
        .cloned()
        .collect();
    let not_recorded = dreaming_lines
        .iter()
        .any(|l| l.trim() == "warning: no dreaming run recorded");
    let last_ok = dreaming_lines.iter().find_map(|l| {
        let at = l.trim().strip_prefix("warning: last dreaming success ")?;
        OffsetDateTime::parse(at, &time::format_description::well_known::Rfc3339)
            .ok()
            .and_then(|t| {
                t.to_offset(offset)
                    .format(format_description!("[day] [month repr:short]"))
                    .ok()
            })
    });
    let (dreaming, dreaming_failed, dreaming_warning) = match status.and_then(|s| s.report.as_ref())
    {
        Some(report) => {
            let date = report
                .date
                .format(format_description!("[day] [month repr:short]"))
                .unwrap_or_default();
            if report.failed {
                (format!("dream failed {date}"), true, false)
            } else if let Some(at) = last_ok {
                (format!("dream overdue, last ok {at}"), false, true)
            } else if report.dry_run && not_recorded {
                (format!("dream: dry run only, {date}"), false, true)
            } else if not_recorded {
                (format!("dream ok {date}, not recorded"), false, true)
            } else if report.dry_run {
                (format!("dream: dry run {date}"), false, false)
            } else {
                (format!("dream ok {date}"), false, false)
            }
        }
        None if status.is_some() => ("no dream run yet".to_string(), false, false),
        None => ("dream: not read".to_string(), false, false),
    };
    let others = lines.iter().filter(|l| !is_dreaming_warning(l)).count();
    let warnings = match others {
        0 => None,
        1 => Some("1 warning".to_string()),
        n => Some(format!("{n} warnings")),
    };
    let ingest = status.map(|s| match s.last_ingest {
        None => "no ingest yet".to_string(),
        Some(t) => {
            let when = format_when(t, now, offset);
            if s.ingest_stale {
                format!("ingest {when}, stale")
            } else {
                format!("ingest {when}")
            }
        }
    });
    let index_note = status
        .and_then(|s| s.index_format)
        .filter(|f| *f == 1)
        .map(|_| "index format 1: hippo ingest upgrades it".to_string());
    let errors = status
        .filter(|s| !s.errors.is_empty())
        .map(|s| s.errors.join("; "));
    StatusModel {
        dreaming,
        dreaming_failed,
        dreaming_warning,
        dreaming_lines,
        warnings,
        ingest,
        index_note,
        revert,
        installation,
        errors,
        theme_notice,
    }
}

impl StatusModel {
    /// What assistive technology reads for hippo's status (the sidebar's
    /// foot).
    pub fn accessible_label(&self) -> String {
        let mut parts = vec![self.dreaming.clone()];
        parts.extend(self.warnings.clone());
        parts.extend(self.ingest.clone());
        parts.extend(self.index_note.clone());
        parts.extend(self.revert.clone());
        parts.extend(self.installation.as_ref().map(|n| n.detail.clone()));
        parts.extend(self.theme_notice.as_ref().map(|n| n.text.clone()));
        parts.join(", ")
    }

    /// The one thing the removed status bar said that needs saying now:
    /// a running revert, a read error, a theme problem, or an
    /// installation problem, with whether it is a problem.
    pub fn notice(&self) -> Option<(String, bool)> {
        if let Some(r) = &self.revert {
            return Some((r.clone(), false));
        }
        if let Some(e) = &self.errors {
            return Some((e.clone(), true));
        }
        if let Some(n) = self.theme_notice.as_ref().filter(|n| n.problem) {
            return Some((n.text.clone(), true));
        }
        self.installation
            .as_ref()
            .filter(|n| n.problem)
            .map(|n| (n.detail.clone(), true))
    }
}

impl Shell {
    pub(super) fn status_model(&self) -> StatusModel {
        let status = self.loaded.as_ref().and_then(|l| l.status.as_ref());
        let mut m = model(
            status,
            OffsetDateTime::now_utc(),
            local_offset(),
            self.installation.status_note(),
            self.revert_running.clone(),
            self.themes.notice().cloned(),
        );
        // When the newest run was and what it changed: from its report's
        // run.
        let newest = status
            .and_then(|s| s.report.as_ref())
            .map(|r| r.path.clone());
        let run = self
            .loaded
            .as_ref()
            .and_then(|l| l.panels.as_ref())
            .and_then(|p| p.dreaming.as_ref().ok())
            .and_then(|d| {
                d.runs
                    .iter()
                    .find(|r| Some(&r.report.path) == newest.as_ref())
            })
            .map(|r| RunFacts {
                written: r.written.map(OffsetDateTime::from),
                changes: hippo_view::edition::run_changes(r),
            });
        describe_run(&mut m, run, OffsetDateTime::now_utc(), local_offset());
        // A dry run never stands in for the last real run: say the
        // last real run, and the dry run after it.
        let newest_dry = status
            .and_then(|s| s.report.as_ref())
            .is_some_and(|r| r.dry_run);
        if newest_dry && !m.dreaming_failed {
            let runs = self
                .loaded
                .as_ref()
                .and_then(|l| l.panels.as_ref())
                .and_then(|p| p.dreaming.as_ref().ok())
                .map(|d| d.runs.as_slice())
                .unwrap_or(&[]);
            let real = runs
                .iter()
                .find(|r| r.report.record.as_ref().is_ok_and(|rr| !rr.status.dry_run))
                .map(|r| RunFacts {
                    written: r.written.map(OffsetDateTime::from),
                    changes: hippo_view::edition::run_changes(r),
                });
            let dry = runs
                .iter()
                .find(|r| Some(&r.report.path) == newest.as_ref())
                .and_then(|r| r.written.map(OffsetDateTime::from));
            m.dreaming = real_and_dry(real, dry, OffsetDateTime::now_utc(), local_offset())
                .unwrap_or_else(|| m.dreaming.clone());
        }
        m
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_dry_run_follows_the_last_real_run() {
        let now = datetime!(2026-11-10 02:00 UTC);
        let real = RunFacts {
            written: Some(datetime!(2026-11-09 18:01 UTC)),
            changes: 4,
        };
        assert_eq!(
            real_and_dry(
                Some(real),
                Some(datetime!(2026-11-10 01:32 UTC)),
                now,
                UtcOffset::from_hms(9, 30, 0).unwrap()
            )
            .as_deref(),
            Some("dreamed 03:31, 4 changes")
        );
        assert_eq!(real_and_dry(None, None, now, UtcOffset::UTC), None);
    }

    #[test]
    fn each_warning_says_what_it_means_and_what_to_run() {
        let (text, cmd) =
            explain_warning("warning: no dreaming run recorded", "03:30", UtcOffset::UTC);
        assert!(
            text.contains("dry run does not count") && text.contains("03:30"),
            "{text}"
        );
        assert_eq!(cmd.as_deref(), Some("hippo dream"));
        let (text, cmd) = explain_warning(
            "warning: last dreaming success 2026-11-05T03:30:00Z",
            "03:30",
            UtcOffset::UTC,
        );
        assert!(text.contains("05 Nov 03:30"), "{text}");
        assert_eq!(cmd.as_deref(), Some("hippo doctor"));
        assert_eq!(
            explain_warning("warning: something new", "03:30", UtcOffset::UTC),
            ("something new".to_string(), None)
        );
    }
    use super::*;
    use hippo_core::dream::report::ReportStatus;
    use std::path::PathBuf;
    use time::macros::{date, datetime};

    fn status() -> Status {
        Status {
            report: None,
            warnings: Vec::new(),
            last_ingest: None,
            ingest_stale: false,
            index_format: Some(2),
            errors: Vec::new(),
        }
    }

    fn m(status: Option<&Status>) -> StatusModel {
        model(
            status,
            datetime!(2026-11-08 15:00 UTC),
            UtcOffset::UTC,
            None,
            None,
            None,
        )
    }

    #[test]
    fn times_are_hh_mm_today_and_day_month_before() {
        let now = datetime!(2026-11-08 15:00 UTC);
        assert_eq!(
            format_when(datetime!(2026-11-08 12:40 UTC), now, UtcOffset::UTC),
            "12:40"
        );
        assert_eq!(
            format_when(datetime!(2026-11-07 23:59 UTC), now, UtcOffset::UTC),
            "Sat 23:59"
        );
    }

    #[test]
    fn the_local_day_decides_not_the_utc_day() {
        // 12:00 UTC is 21:00 on the 8th at +09:00.
        let now = datetime!(2026-11-08 12:00 UTC);
        let plus_nine = UtcOffset::from_hms(9, 0, 0).unwrap();
        // 20:00 UTC on the 7th is 05:00 on the 8th at +09:00: today.
        assert_eq!(
            format_when(datetime!(2026-11-07 20:00 UTC), now, plus_nine),
            "05:00"
        );
    }

    #[test]
    fn the_dreaming_status_says_ok_failed_or_none() {
        assert_eq!(m(None).dreaming, "dream: not read");
        assert_eq!(m(Some(&status())).dreaming, "no dream run yet");
        let mut s = status();
        s.report = Some(ReportStatus {
            path: PathBuf::from("2026-11-08-dream.md"),
            date: date!(2026 - 11 - 08),
            seq: 1,
            failed: false,
            error: None,
            dry_run: false,
        });
        let ok = m(Some(&s));
        assert_eq!(ok.dreaming, "dream ok 08 Nov");
        // Once its run is read, the bar says when it ran and what changed,
        // in the reader's time: the 08 Nov report, written late on the 8th
        // UTC, falls on the 9th at +09:30.
        let acst = UtcOffset::from_hms(9, 30, 0).unwrap();
        let now = datetime!(2026-11-08 23:00 UTC);
        let run = |h: u8, changes| RunFacts {
            written: Some(datetime!(2026-11-08 15:08 UTC) + time::Duration::hours(i64::from(h))),
            changes,
        };
        let said = |model: &StatusModel, r: Option<RunFacts>| {
            let mut x = model.clone();
            describe_run(&mut x, r, now, acst);
            x.dreaming
        };
        assert_eq!(said(&ok, Some(run(0, 4))), "dreamed 00:38, 4 changes");
        assert_eq!(said(&ok, Some(run(0, 0))), "dreamed 00:38");
        assert_eq!(said(&ok, Some(run(0, 1))), "dreamed 00:38, 1 change");
        // Not read yet: the report's date, as before.
        assert_eq!(said(&ok, None), "dream ok 08 Nov");
        assert!(!ok.dreaming_failed);
        s.report.as_mut().unwrap().failed = true;
        let failed = m(Some(&s));
        assert_eq!(failed.dreaming, "dream failed 08 Nov");
        assert!(failed.dreaming_failed);
        assert_eq!(said(&failed, Some(run(0, 0))), "dream failed 00:38");
        let yesterday = RunFacts {
            written: Some(datetime!(2026-11-07 18:00 UTC)),
            changes: 0,
        };
        assert_eq!(said(&ok, Some(yesterday)), "dreamed Sun 03:30");
    }

    #[test]
    fn the_dreaming_status_agrees_with_the_warnings_agents_get() {
        let mut s = status();
        s.report = Some(ReportStatus {
            path: PathBuf::from("2026-11-08-dream.md"),
            date: date!(2026 - 11 - 08),
            seq: 1,
            failed: false,
            error: None,
            dry_run: true,
        });
        // Only a dry run, so agents are told no run is recorded: the bar
        // says the same, once, as a problem.
        s.warnings = vec!["warning: no dreaming run recorded".into()];
        let dry = m(Some(&s));
        assert_eq!(dry.dreaming, "dream: dry run only, 08 Nov");
        assert!(dry.dreaming_warning && !dry.dreaming_failed);
        assert_eq!(dry.warnings, None);
        assert_eq!(dry.dreaming_lines.len(), 1);
        // A dry run after a recent real one is no problem.
        s.warnings.clear();
        let calm = m(Some(&s));
        assert_eq!(calm.dreaming, "dream: dry run 08 Nov");
        assert!(!calm.dreaming_warning);
        // Overdue.
        s.report.as_mut().unwrap().dry_run = false;
        s.warnings = vec!["warning: last dreaming success 2026-11-05T03:30:00Z".into()];
        let late = m(Some(&s));
        assert_eq!(late.dreaming, "dream overdue, last ok 05 Nov");
        assert!(late.dreaming_warning);
        // Failed outranks the rest.
        s.report.as_mut().unwrap().failed = true;
        s.warnings.push("warning: last dreaming run failed".into());
        let failed = m(Some(&s));
        assert_eq!(failed.dreaming, "dream failed 08 Nov");
        assert!(failed.dreaming_failed && failed.warnings.is_none());
    }

    #[test]
    fn warnings_ingest_and_index_format_show_when_they_apply() {
        let mut s = status();
        assert_eq!(m(Some(&s)).warnings, None);
        assert_eq!(m(Some(&s)).ingest.as_deref(), Some("no ingest yet"));
        assert_eq!(m(Some(&s)).index_note, None);

        // A dreaming warning is the dreaming item's to say, not a count.
        s.warnings = vec!["warning: last dreaming run failed".into()];
        assert_eq!(m(Some(&s)).warnings, None);
        s.warnings.push("warning: ingest has not run".into());
        assert_eq!(m(Some(&s)).warnings.as_deref(), Some("1 warning"));
        s.warnings.push("warning: something else".into());
        assert_eq!(m(Some(&s)).warnings.as_deref(), Some("2 warnings"));

        s.last_ingest = Some(datetime!(2026-11-08 12:40 UTC));
        assert_eq!(m(Some(&s)).ingest.as_deref(), Some("ingest 12:40"));
        s.ingest_stale = true;
        assert_eq!(m(Some(&s)).ingest.as_deref(), Some("ingest 12:40, stale"));

        s.index_format = Some(1);
        assert!(m(Some(&s)).index_note.unwrap().contains("format 1"));
        s.errors = vec!["index: damaged".into()];
        assert_eq!(m(Some(&s)).errors.as_deref(), Some("index: damaged"));
    }

    #[test]
    fn the_label_for_assistive_technology_names_every_part() {
        let mut s = status();
        s.last_ingest = Some(datetime!(2026-11-08 12:40 UTC));
        let label = model(
            Some(&s),
            datetime!(2026-11-08 15:00 UTC),
            UtcOffset::UTC,
            Some(InstallNote {
                label: "default hippo",
                detail: "default hippo: no launchd job".into(),
                problem: true,
            }),
            Some("reverting 01ABC".into()),
            Some(Notice {
                text: "harbour.json no longer parses: keeping the last good version".into(),
                detail: "x".into(),
                problem: true,
            }),
        )
        .accessible_label();
        for part in [
            "no dream run yet",
            "ingest 12:40",
            "reverting 01ABC",
            "default hippo",
            "harbour.json no longer parses",
        ] {
            assert!(label.contains(part), "{label}");
        }
    }
}
