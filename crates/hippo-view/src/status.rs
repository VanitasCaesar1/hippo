//! What the status bar shows: the newest dreaming report, the context
//! warnings, the last ingest and the index's format.

use std::path::Path;

use hippo_core::config::{Config, Workspace};
use hippo_core::context;
use hippo_core::dream::report::{self, ReportStatus};
use hippo_core::error::Error;
use hippo_core::index::Index;
use hippo_core::records::IngestState;
use time::OffsetDateTime;

/// The status bar's data. A part that fails to load is left empty and its
/// error is added to `errors`. Error text is as the engine gives it.
#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    /// The newest dreaming report; `None` when there is none.
    pub report: Option<ReportStatus>,
    /// `context::warnings`: lines like `warning: last dreaming run failed`.
    pub warnings: Vec<String>,
    /// When the last ingest finished; `None` before the first.
    pub last_ingest: Option<OffsetDateTime>,
    /// The last ingest is more than three intervals old.
    pub ingest_stale: bool,
    /// The index's format (1 before `hippo ingest` upgrades it, then 2);
    /// `None` without a readable index.
    pub index_format: Option<i64>,
    pub errors: Vec<String>,
}

/// Reads the status at `now`. Writes nothing.
pub fn load(cfg: &Config, ws: &Workspace, now: OffsetDateTime) -> Status {
    let mut errors = Vec::new();
    let mut fail = |what: &str, e: &dyn std::fmt::Display| {
        let line = format!("{what}: {e}");
        // The report is read twice (here and by the warnings): say it once.
        if !errors.contains(&line) {
            errors.push(line);
        }
    };

    let report = match report::newest(ws) {
        Ok(report) => report,
        Err(e) => {
            fail("dreaming report", &e);
            None
        }
    };
    let warnings = match context::warnings(ws, now) {
        Ok(lines) => lines,
        Err(e) => {
            fail("dreaming report", &e);
            Vec::new()
        }
    };

    let state_path = cfg.home.join("ingest-state.json");
    let (last_ingest, ingest_stale) = match IngestState::load(&state_path) {
        Ok(state) => (
            Some(state.last_ingest),
            state.is_stale(cfg.ingest_interval_minutes, now),
        ),
        Err(Error::Io(io)) if io.kind() == std::io::ErrorKind::NotFound => (None, false),
        Err(e) => {
            fail("ingest state", &e);
            (None, false)
        }
    };

    let index_format = index_format(&cfg.home, &mut fail);
    Status {
        report,
        warnings,
        last_ingest,
        ingest_stale,
        index_format,
        errors,
    }
}

/// The status of a configuration with no notes folder: the last ingest
/// and the index's format; no dreaming. Writes nothing.
pub fn load_history(cfg: &Config, now: OffsetDateTime) -> Status {
    let mut errors = Vec::new();
    let mut fail = |what: &str, e: &dyn std::fmt::Display| errors.push(format!("{what}: {e}"));
    let state_path = cfg.home.join("ingest-state.json");
    let (last_ingest, ingest_stale) = match IngestState::load(&state_path) {
        Ok(state) => (
            Some(state.last_ingest),
            state.is_stale(cfg.ingest_interval_minutes, now),
        ),
        Err(Error::Io(io)) if io.kind() == std::io::ErrorKind::NotFound => (None, false),
        Err(e) => {
            fail("ingest state", &e);
            (None, false)
        }
    };
    let index_format = index_format(&cfg.home, &mut fail);
    Status {
        report: None,
        warnings: Vec::new(),
        last_ingest,
        ingest_stale,
        index_format,
        errors,
    }
}

fn index_format(home: &Path, fail: &mut impl FnMut(&str, &dyn std::fmt::Display)) -> Option<i64> {
    match Index::open(home, false) {
        Ok(index) => Some(index.format()),
        Err(Error::MissingIndex) => None,
        Err(e) => {
            fail("index", &e);
            None
        }
    }
}
