#![allow(clippy::disallowed_methods)] // tests build temporary workspaces

mod common;

use std::fs;

use hippocamp_core::index::Index;
use hippocamp_view::setup::{self, Setup};
use hippocamp_view::status;
use time::macros::datetime;

fn ready(
    env: &common::Env,
) -> (
    hippocamp_core::config::Config,
    hippocamp_core::config::Workspace,
) {
    match setup::load(&env.home) {
        Setup::Ready { cfg, ws } => (cfg, ws),
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_empty_installation() {
    let env = common::env();
    let (cfg, ws) = ready(&env);
    let got = status::load(&cfg, &ws, datetime!(2026-11-08 12:00 UTC));
    assert_eq!(got.report, None);
    assert!(got.warnings.is_empty());
    assert_eq!(got.last_ingest, None);
    assert!(!got.ingest_stale);
    assert_eq!(got.index_format, None);
    assert!(got.errors.is_empty(), "{:?}", got.errors);
}

#[test]
fn a_report_an_ingest_and_an_index() {
    let env = common::env();
    let (cfg, ws) = ready(&env);
    let reports = ws.hippocamp_dir().join("reports");
    fs::create_dir_all(&reports).unwrap();
    fs::write(
        reports.join("2026-11-07-dream.md"),
        "status: failed\nerror: boom\n",
    )
    .unwrap();
    fs::write(
        cfg.home.join("ingest-state.json"),
        r#"{"protocol":"HIPPOCAMP_V1","last_ingest":"2026-11-08T11:50:00Z","files_seen":3,"files_indexed":3}"#,
    )
    .unwrap();
    Index::open(&cfg.home, true).unwrap();

    let got = status::load(&cfg, &ws, datetime!(2026-11-08 12:00 UTC));
    let report = got.report.unwrap();
    assert!(report.failed);
    assert_eq!(report.error.as_deref(), Some("boom"));
    assert!(got
        .warnings
        .contains(&"warning: last dreaming run failed".to_owned()));
    assert_eq!(got.last_ingest, Some(datetime!(2026-11-08 11:50 UTC)));
    assert!(!got.ingest_stale);
    assert_eq!(got.index_format, Some(2));
    assert!(got.errors.is_empty(), "{:?}", got.errors);

    // Three intervals of ten minutes later it is stale.
    let late = status::load(&cfg, &ws, datetime!(2026-11-08 12:30 UTC));
    assert!(late.ingest_stale);
}

#[test]
fn one_failing_part_is_empty_with_its_error_and_the_rest_loads() {
    let env = common::env();
    let (cfg, ws) = ready(&env);
    fs::write(cfg.home.join("ingest-state.json"), "not json").unwrap();
    Index::open(&cfg.home, true).unwrap();

    let got = status::load(&cfg, &ws, datetime!(2026-11-08 12:00 UTC));
    assert_eq!(got.last_ingest, None);
    assert!(!got.ingest_stale);
    assert_eq!(got.index_format, Some(2));
    assert_eq!(got.errors.len(), 1, "{:?}", got.errors);
    assert!(
        got.errors[0].starts_with("ingest state: "),
        "{:?}",
        got.errors
    );
}

#[test]
fn an_index_of_another_format_is_an_error_not_a_missing_index() {
    let env = common::env();
    let (cfg, ws) = ready(&env);
    bad_index(&cfg.home);
    let got = status::load(&cfg, &ws, datetime!(2026-11-08 12:00 UTC));
    assert_eq!(got.index_format, None);
    assert_eq!(got.errors.len(), 1, "{:?}", got.errors);
    assert!(got.errors[0].starts_with("index: "), "{:?}", got.errors);
}

/// A file that is not a SQLite database.
fn bad_index(home: &std::path::Path) {
    fs::write(
        home.join("index.sqlite"),
        "this is not a database, ".repeat(10),
    )
    .unwrap();
}
