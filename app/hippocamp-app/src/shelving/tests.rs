//! The shelves, the hand-made marks and the counts, on a synthetic fleet
//! shaped like the demo home's (`app/scripts/demo-home.sh`).

use std::collections::BTreeMap;
use std::path::PathBuf;

use hippocamp_view::fleet::{Claims, Fleet, Row, Shelf, State};
use time::macros::{datetime, offset};
use time::{Duration, OffsetDateTime};

use super::*;
use crate::prefs::{Mark, Snooze};

const NOW: OffsetDateTime = datetime!(2026-11-10 09:00 UTC);

fn row(id: &str, state: State) -> Row {
    Row {
        id: id.into(),
        title: format!("Session {id}"),
        tool: "claude-code".into(),
        profile: Some("work".into()),
        project: Some("tern".into()),
        client: Some("terminal"),
        cwd: Some(PathBuf::from("/work/tern")),
        transcript: PathBuf::from(format!("/home/.claude/projects/-work-tern/{id}.jsonl")),
        started: Some(NOW - Duration::hours(2)),
        last: Some(NOW - Duration::minutes(30)),
        turns: Some(10),
        agents: Some(0),
        edits: 0,
        state,
        pulls: Vec::new(),
        claims: Claims::default(),
        activity: None,
        activity_last: false,
        call: None,
        turn: None,
        changed: Vec::new(),
        branch: None,
        lines: None,
        tasks: Vec::new(),
        merged_elsewhere: Vec::new(),
        sub: Default::default(),
        live_subs: Vec::new(),
        delegated_by: None,
        asked: None,
        outcome: None,
    }
}

fn finished(id: &str) -> Row {
    row(
        id,
        State::Finished {
            at: NOW - Duration::minutes(30),
        },
    )
}

/// The demo home's shapes: one waiting on a question, one stopped at its
/// limit, one finished with an unverified claim, one live, one quiet
/// mid-step, finished ones opened and
/// not, one whose pull request merged.
fn demo() -> Fleet {
    let mut claim = finished("claim");
    claim.claims = Claims {
        unverified: 1,
        first_unverified: Some(("deployed".into(), "no deploy command".into())),
        ..Claims::default()
    };
    let mut merged = finished("merged");
    merged.pulls = vec![
        ("https://github.com/o/r/pull/7".into(), "opened"),
        ("https://github.com/o/r/pull/7".into(), "merged"),
    ];
    Fleet {
        rows: vec![
            row(
                "ask",
                State::Waiting {
                    at: NOW - Duration::minutes(6),
                    asked: Some("Which plan?".into()),
                    plan: false,
                },
            ),
            row(
                "limit",
                State::Stopped {
                    reset: None,
                    at: NOW - Duration::hours(1),
                    carried_on: false,
                },
            ),
            claim,
            row("live", State::Live),
            row(
                "quiet",
                State::Quiet {
                    since: Some(NOW - Duration::minutes(4)),
                },
            ),
            finished("done1"),
            finished("done2"),
            finished("seen"),
            merged,
        ],
        headroom: Vec::new(),
        use_next: None,
        signed_out: Vec::new(),
        usage_cached: false,
    }
}

struct Store {
    opened: BTreeMap<String, i64>,
    settled: BTreeMap<String, Mark>,
    snoozed: BTreeMap<String, Snooze>,
}

impl Store {
    fn new() -> Store {
        let mut opened = BTreeMap::new();
        opened.insert("seen".to_owned(), NOW.unix_timestamp());
        Store {
            opened,
            settled: BTreeMap::new(),
            snoozed: BTreeMap::new(),
        }
    }

    fn marks(&self) -> Marks<'_> {
        Marks {
            opened: &self.opened,
            settled: &self.settled,
            snoozed: &self.snoozed,
            now: NOW.unix_timestamp(),
        }
    }
}

fn ids(rows: &[&Row]) -> Vec<String> {
    rows.iter().map(|r| r.id.clone()).collect()
}

#[test]
fn every_count_the_app_shows_is_its_shelf() {
    let f = demo();
    let mut store = Store::new();
    // One snoozed, to count that too.
    store.snoozed.insert(
        "done2".into(),
        Snooze {
            until: (NOW + Duration::hours(1)).unix_timestamp(),
            at: NOW.unix_timestamp(),
        },
    );
    let marks = store.marks();
    let s = Shelves::of(&f, &marks);
    let t = s.tally();
    assert_eq!(
        t,
        Tally {
            needs_you: 2,
            working: 2,
            to_review: 2,
            settled: 2,
            snoozed: 1,
        }
    );
    // The sidebar's shelves.
    assert_eq!(ids(s.rows(Shelf::NeedsYou)), ["ask", "limit"]);
    assert_eq!(ids(s.rows(Shelf::Working)), ["live", "quiet"]);
    // The Headroom pane's per-account count adds up to the shelf.
    assert_eq!(s.working_on("work"), t.working);
    assert_eq!(s.working_on("other"), 0);
    // Home's headline.
    let (head, facts) = crate::ui::home_panes::hero_words(Some(t), 0);
    assert_eq!(head, "2 agents need you");
    assert_eq!(facts, "2 working · 2 to review · 1 snoozed");
    // Fleet's State groups.
    let groups = crate::ui::fleet::state_groups(&s);
    let n = |k: &str| groups.iter().find(|g| g.0 == k).map_or(0, |g| g.2.len());
    assert_eq!(n("needs"), t.needs_you);
    assert_eq!(n("working"), t.working);
    assert_eq!(n("finished"), t.to_review);
    assert_eq!(n("snoozed"), t.snoozed);
    assert_eq!(n("settled"), t.settled);
    // Every row is on exactly one of them.
    assert_eq!(
        groups.iter().map(|g| g.2.len()).sum::<usize>(),
        f.rows.len()
    );
    // The menu-bar item.
    let m = crate::reach::menu(Some(&f), &marks, 0, NOW);
    assert_eq!(m.badge, "2 need you");
    assert_eq!(m.needs_head, "Needs you · 2");
    assert_eq!(m.live_head.as_deref(), Some("Working · 2"));
    assert_eq!(m.live.len(), 2);
}

#[test]
fn settle_moves_a_row_to_settled_at_once_and_un_settle_brings_it_back() {
    let f = demo();
    let mut store = Store::new();
    let at = NOW.unix_timestamp();
    store
        .settled
        .insert("done1".into(), Mark { settled: true, at });
    // An opened row (settled by the Settled rule) un-settled by hand.
    store
        .settled
        .insert("seen".into(), Mark { settled: false, at });
    let s = Shelves::of(&f, &store.marks());
    assert!(ids(s.rows(Shelf::Settled)).contains(&"done1".to_owned()));
    assert!(ids(s.rows(Shelf::ToReview)).contains(&"seen".to_owned()));
    // A settle does not hold over newer work in the session.
    let mut later = f.clone();
    later
        .rows
        .iter_mut()
        .find(|r| r.id == "done1")
        .unwrap()
        .last = Some(NOW + Duration::minutes(5));
    let s = Shelves::of(&later, &store.marks());
    assert!(ids(s.rows(Shelf::ToReview)).contains(&"done1".to_owned()));
}

#[test]
fn a_mark_never_moves_a_row_that_needs_you() {
    let f = demo();
    let mut store = Store::new();
    let at = NOW.unix_timestamp();
    store
        .settled
        .insert("ask".into(), Mark { settled: true, at });
    store.snoozed.insert(
        "limit".into(),
        Snooze {
            until: at + 3600,
            at,
        },
    );
    let s = Shelves::of(&f, &store.marks());
    assert_eq!(ids(s.rows(Shelf::NeedsYou)), ["ask", "limit"]);
    assert!(s.snoozed.is_empty());
    // And the snooze ends for good once its session needs you.
    assert_eq!(woken(&f, &store.marks()), ["limit"]);
    assert_eq!(
        controls(store.marks().place(&f.rows[0])),
        controls(Place::Shelf(Shelf::NeedsYou))
    );
    let none = controls(Place::Shelf(Shelf::NeedsYou));
    assert!(!none.settle && !none.snooze && !none.unsettle && !none.wake);
}

#[test]
fn a_snooze_hides_a_row_until_its_time() {
    let f = demo();
    let mut store = Store::new();
    let at = NOW.unix_timestamp();
    store.snoozed.insert(
        "live".into(),
        Snooze {
            until: at + 600,
            at,
        },
    );
    store.snoozed.insert(
        "done1".into(),
        Snooze {
            until: at - 1,
            at: at - 100,
        },
    );
    let s = Shelves::of(&f, &store.marks());
    assert_eq!(ids(s.rows(Shelf::Working)), ["quiet"]);
    assert_eq!(s.snoozed.len(), 1);
    assert_eq!(s.snoozed[0].0.id, "live");
    // A snooze already past is no snooze.
    assert!(ids(s.rows(Shelf::ToReview)).contains(&"done1".to_owned()));
    // The controls each shelf offers.
    assert!(controls(Place::Shelf(Shelf::ToReview)).settle);
    assert!(controls(Place::Shelf(Shelf::Working)).snooze);
    assert!(!controls(Place::Shelf(Shelf::Working)).settle);
    assert!(controls(Place::Shelf(Shelf::Settled)).unsettle);
    assert!(controls(Place::Snoozed { until: 0 }).wake);
}

#[test]
fn snooze_choices_are_t3_codes_soonest_first() {
    // A Tuesday at 10:00 local (UTC+9:30).
    let now = datetime!(2026-11-10 10:00 +9:30).to_offset(offset!(UTC));
    let p = presets(now, offset!(+9:30));
    let labels: Vec<(&str, &str)> = p
        .iter()
        .map(|p| (p.label.as_str(), p.when.as_str()))
        .collect();
    assert_eq!(
        labels,
        [
            ("In 1 hour", "11:00"),
            ("In 3 hours", "13:00"),
            ("This evening", "18:00"),
            ("Tomorrow", "09:00"),
            ("Next week", "Mon 09:00"),
        ]
    );
    // After 18:00 the evening is gone; on a Sunday Monday is tomorrow,
    // once.
    let late = datetime!(2026-11-15 20:00 +9:30).to_offset(offset!(UTC));
    let labels: Vec<String> = presets(late, offset!(+9:30))
        .into_iter()
        .map(|p| p.label)
        .collect();
    assert_eq!(labels, ["In 1 hour", "In 3 hours", "Tomorrow"]);
}

#[test]
fn wake_labels_round_up() {
    assert_eq!(wake_label(100, 100), "now");
    assert_eq!(wake_label(130, 100), "1m");
    assert_eq!(wake_label(100 + 3600, 100), "1h");
    assert_eq!(wake_label(100 + 3601, 100), "2h");
    assert_eq!(wake_label(100 + 2 * 86_400, 100), "2d");
}

#[test]
fn a_frozen_list_keeps_its_rows_in_place() {
    let now = vec![("working", vec!["b", "a"]), ("review", vec!["c", "new"])];
    let snap = vec![
        ("working".to_owned(), vec!["a".to_owned(), "c".to_owned()]),
        ("review".to_owned(), vec!["b".to_owned(), "gone".to_owned()]),
    ];
    let groups: Vec<(String, Vec<&str>)> =
        now.into_iter().map(|(k, v)| (k.to_owned(), v)).collect();
    let out = freeze(groups, &snap, |r| r);
    // `c` finished but stays where the pointer saw it; `b` stays too;
    // `new` joins its own group at the end; `gone` is gone.
    assert_eq!(
        out,
        vec![
            ("working".to_owned(), vec!["a", "c"]),
            ("review".to_owned(), vec!["b", "new"]),
        ]
    );
    assert_eq!(snapshot(&out, |r| r)[0].1, ["a", "c"]);
}

#[test]
fn a_delegated_session_sits_under_its_parent_and_counts_with_it() {
    let mut f = demo();
    // "done1" delegated two lanes: one working now, one finished.
    let mut lane = row("lane-live", State::Live);
    lane.delegated_by = Some("done1".into());
    let mut lane2 = finished("lane-done");
    lane2.delegated_by = Some("done1".into());
    // A delegated session whose parent is not today's stands alone.
    let mut orphan = finished("orphan");
    orphan.delegated_by = Some("yesterday".into());
    f.rows.extend([lane, lane2, orphan]);
    let store = Store::new();
    let s = Shelves::of(&f, &store.marks());
    // The family goes on the more urgent shelf, the parent first.
    assert_eq!(
        ids(s.rows(Shelf::Working)),
        ["live", "quiet", "done1", "lane-live", "lane-done"]
    );
    let t = s.tally();
    assert_eq!(t.working, 3, "the lanes count with their parent");
    assert_eq!(t.to_review, 3, "done2, the orphan and the unverified claim");
    assert!(nested_in(&f.rows[9], s.rows(Shelf::Working)));
    assert!(!nested_in(&f.rows[11], s.rows(Shelf::ToReview)));
    assert_eq!(s.working_on("work"), 3);
}
