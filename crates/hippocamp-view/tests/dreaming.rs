#![allow(clippy::disallowed_methods)] // tests build temporary workspaces

mod common;

use std::fs;
use std::path::PathBuf;

use hippocamp_core::adapters::Role;
use hippocamp_core::dream::{self, RevertState};
use hippocamp_core::index::Index;
use hippocamp_core::notes;
use hippocamp_core::records::Verdict;
use hippocamp_view::dreaming;
use time::macros::date;

// 2026-11-08 12:00 UTC, in milliseconds; a day is 86,400,000.
const NOON: u64 = 1_794_139_200_000;
const DAY: u64 = 86_400_000;

#[test]
fn runs_from_two_reports_with_missing_ids_and_earlier_changes_by_day() {
    let env = common::env();
    common::note(&env, "brain/a.md", "# A\n");
    let ws = common::workspace(&env);
    let a = common::ulid_at(NOON, 1);
    let b = common::ulid_at(NOON + 1000, 2);
    let c = common::ulid_at(NOON - DAY, 3); // 7 Nov, named by the older report
    let ghost = common::ulid_at(NOON - DAY + 1000, 4); // named, no file
    let early1 = common::ulid_at(NOON - 3 * DAY + 5000, 5); // 5 Nov
    let early2 = common::ulid_at(NOON - 3 * DAY, 6); // 5 Nov, earlier id
    let early3 = common::ulid_at(NOON - 2 * DAY, 7); // 6 Nov
    for id in [a, b, c, early1, early2, early3] {
        common::proposal(&ws, id, "brain/a.md", "old", "new", &[]);
    }
    common::receipt(
        &ws,
        common::ulid_at(NOON + 2000, 9),
        a,
        Verdict::Reject,
        None,
    );
    common::report(&ws, "2026-11-08", &[a, b]);
    common::report(&ws, "2026-11-07", &[c, ghost]);
    fs::write(ws.hippocamp_dir().join("receipts/junk.json"), "{").unwrap();

    let history = dream::history(&ws).unwrap();
    let got = dreaming::load(&ws, &history).unwrap();

    assert_eq!(got.runs.len(), 2);
    let newest = &got.runs[0];
    assert_eq!(
        newest.report.record.as_ref().unwrap().status.date,
        date!(2026 - 11 - 08)
    );
    let ids = |changes: &[dream::Change]| changes.iter().map(|c| c.proposal_id).collect::<Vec<_>>();
    assert_eq!(ids(&newest.changes), [a, b]);
    assert_eq!(newest.changes[0].receipts.len(), 1);
    assert!(newest.missing.is_empty());
    let older = &got.runs[1];
    assert_eq!(ids(&older.changes), [c]);
    assert_eq!(older.missing, [ghost]);

    let days: Vec<_> = got.earlier.iter().map(|(d, _)| *d).collect();
    assert_eq!(days, [date!(2026 - 11 - 06), date!(2026 - 11 - 05)]);
    assert_eq!(ids(&got.earlier[0].1), [early3]);
    assert_eq!(ids(&got.earlier[1].1), [early2, early1]);

    assert_eq!(got.unreadable.len(), 1);
    assert!(got.unreadable[0].0.ends_with("junk.json"));
}

#[test]
fn no_reports_no_changes() {
    let env = common::env();
    let ws = common::workspace(&env);
    let got = dreaming::load(&ws, &dream::history(&ws).unwrap()).unwrap();
    assert!(got.runs.is_empty() && got.earlier.is_empty() && got.unreadable.is_empty());
}

#[test]
fn evidence_not_in_the_index_is_an_error_entry() {
    let env = common::env();
    common::note(&env, "brain/a.md", "# A\n");
    let ws = common::workspace(&env);
    common::index_with(
        &env.home,
        "s1",
        &[(Role::User, "one"), (Role::Assistant, "two")],
    );
    let id = common::ulid_at(NOON, 1);
    common::proposal(
        &ws,
        id,
        "brain/a.md",
        "old",
        "new",
        &[("s1", 1), ("s1", 9), ("gone", 0), ("s1", 0)],
    );
    let history = dream::history(&ws).unwrap();
    let index = Index::open(&env.home, false).unwrap();

    let view = dreaming::view(&ws, &history, Some(&index), id).unwrap();
    assert_eq!(view.evidence.len(), 4);
    assert_eq!(view.evidence[0].as_ref().unwrap().text, "two");
    assert_eq!(view.evidence[1], Err(("s1".to_owned(), 9)));
    assert_eq!(view.evidence[2], Err(("gone".to_owned(), 0)));
    assert_eq!(view.evidence[3].as_ref().unwrap().text, "one");

    // Without an index every turn is missing.
    let view = dreaming::view(&ws, &history, None, id).unwrap();
    assert!(view.evidence.iter().all(|e| e.is_err()));
    assert_eq!(view.evidence.len(), 4);

    assert!(dreaming::view(&ws, &history, None, common::ulid_at(NOON, 99)).is_err());
}

#[test]
fn revert_is_none_without_an_applied_confirm_receipt() {
    let env = common::env();
    common::note(&env, "brain/a.md", "# A\n");
    let ws = common::workspace(&env);
    let id = common::ulid_at(NOON, 1);
    common::proposal(&ws, id, "brain/a.md", "old", "new", &[]);
    // A rejection, and a confirmation that wrote nothing.
    common::receipt(&ws, common::ulid_at(NOON + 1, 2), id, Verdict::Reject, None);
    common::receipt(
        &ws,
        common::ulid_at(NOON + 2, 3),
        id,
        Verdict::Confirm,
        None,
    );
    let history = dream::history(&ws).unwrap();
    let view = dreaming::view(&ws, &history, None, id).unwrap();
    assert_eq!(view.revert, None);
    assert_eq!(view.change.receipts.len(), 2);
}

#[test]
fn revert_follows_the_note() {
    let env = common::env();
    let before = "# A\n\nold line\n";
    let after = "# A\n\nnew line\n";
    common::note(&env, "brain/a.md", before);
    let ws = common::workspace(&env);
    let id = common::ulid_at(NOON, 1);
    common::proposal(&ws, id, "brain/a.md", "old line", "new line", &[]);
    let snapshot = notes::snapshot(&ws.root, id, &PathBuf::from("brain/a.md")).unwrap();
    common::note(&env, "brain/a.md", after);
    let receipt = common::ulid_at(NOON + 1, 2);
    common::receipt(
        &ws,
        receipt,
        id,
        Verdict::Confirm,
        Some((
            &hippocamp_core::hash(before.as_bytes()),
            &hippocamp_core::hash(after.as_bytes()),
            snapshot,
        )),
    );

    let view = |ws: &_| {
        let history = dream::history(ws).unwrap();
        dreaming::view(ws, &history, None, id).unwrap().revert
    };
    assert_eq!(view(&ws), Some(RevertState::Revertible));
    common::note(&env, "brain/a.md", "# A\n\nedited by hand\n");
    assert_eq!(view(&ws), Some(RevertState::NoteChanged));
    fs::remove_file(env.workspace.join("brain/a.md")).unwrap();
    assert_eq!(view(&ws), Some(RevertState::NoteMissing));
}
