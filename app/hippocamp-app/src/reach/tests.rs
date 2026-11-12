//! The menu's words and the notifications' rules, on a synthetic fleet.

use std::path::PathBuf;

use hippocamp_core::usage::{Estimate, Headroom, Left, Source, WindowState};
use hippocamp_view::fleet::{Claims, Fleet, Row, State};
use time::macros::datetime;
use time::{Duration, OffsetDateTime};

use super::*;

const NOW: OffsetDateTime = datetime!(2026-11-10 09:00 UTC);

/// No marks: nothing opened, settled or snoozed.
fn menu(f: Option<&Fleet>, inbox: usize, now: OffsetDateTime) -> Menu {
    let none = Default::default();
    let (s, z) = (Default::default(), Default::default());
    let marks = crate::shelving::Marks {
        opened: &none,
        settled: &s,
        snoozed: &z,
        now: now.unix_timestamp(),
    };
    super::menu(f, &marks, inbox, now)
}

fn row(id: &str, title: &str, profile: &str, state: State) -> Row {
    Row {
        id: id.into(),
        title: title.into(),
        tool: "claude-code".into(),
        profile: Some(profile.into()),
        project: Some("demo".into()),
        client: Some("terminal"),
        cwd: Some(PathBuf::from("/work/demo")),
        transcript: PathBuf::from(format!("/home/.claude/projects/-work-demo/{id}.jsonl")),
        started: Some(NOW - Duration::hours(1)),
        last: Some(NOW - Duration::minutes(5)),
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

fn room(profile: &str, left: f64, limited: bool) -> Headroom {
    let l = |frac: f64| Left {
        frac,
        source: Source::Meter,
        resets_at: None,
        reset_passed: None,
    };
    Headroom {
        profile: profile.into(),
        codex: false,
        dir: Some(PathBuf::from(format!("/home/.claude-{profile}"))),
        window: if limited {
            WindowState::Limited {
                reset: (NOW + Duration::hours(2)).unix_timestamp(),
            }
        } else {
            WindowState::None
        },
        estimate: Estimate::Left(left),
        limit: None,
        limit_from: 0,
        weekly_until: None,
        last_seen: None,
        from_meter: false,
        meter: None,
        meter_stale: None,
        five: Some(l(left)),
        seven: Some(l(1.0)),
    }
}

fn fleet(rows: Vec<Row>, rooms: Vec<Headroom>, next: Option<&str>) -> Fleet {
    Fleet {
        rows,
        headroom: rooms,
        use_next: next.map(str::to_owned),
        signed_out: Vec::new(),
        usage_cached: true,
    }
}

fn waiting(at: OffsetDateTime) -> State {
    State::Waiting {
        at,
        asked: Some("Shall I go ahead with the migration?".into()),
        plan: false,
    }
}

fn stopped(at: OffsetDateTime) -> State {
    State::Stopped {
        reset: Some("resets 6:10pm".into()),
        at,
        carried_on: false,
    }
}

const ALL: Switches = Switches {
    waiting: true,
    stopped: true,
    reset: true,
};

fn never(_: &str) -> bool {
    false
}

#[test]
fn the_menu_lists_needs_in_order_with_the_headroom_line() {
    let f = fleet(
        vec![
            row(
                "s1",
                "Stopped one",
                "work",
                stopped(NOW - Duration::hours(2)),
            ),
            row(
                "w1",
                "Waiting one",
                "home",
                waiting(NOW - Duration::minutes(12)),
            ),
            row("l1", "Live one", "work", State::Live),
        ],
        vec![
            room("default", 0.16, false),
            room("work", 1.0, false),
            room("home", 0.6, false),
        ],
        Some("work"),
    );
    let m = menu(Some(&f), 14, NOW);
    assert_eq!(m.badge, "2 need you");
    assert_eq!(m.needs_head, "Needs you · 2");
    assert_eq!(m.needs[0].title, "Waiting one");
    assert_eq!(m.needs[0].reason, "asked you · 12m · demo · home");
    assert_eq!(m.needs[0].act, Act::Open("w1".into()));
    assert!(m.needs[1].reason.starts_with("stopped at its limit "));
    assert_eq!(m.needs[1].act, Act::Resume("s1".into()));
    assert_eq!(m.nothing, None);
    assert_eq!(m.headroom[0], "work   100% left   use next");
    assert_eq!(m.bars.len(), m.headroom.len());
    assert_eq!(m.bars[0], (100, false));
    assert!(
        m.headroom[1].starts_with("home   60% left"),
        "{:?}",
        m.headroom
    );
    assert!(
        m.headroom[2].starts_with("default   16% left"),
        "{:?}",
        m.headroom
    );
    assert_eq!(m.headroom_head.as_deref(), Some("Headroom"));
    assert_eq!(m.inbox.as_deref(), Some("Inbox · 14 to review"));
    assert_eq!(m.live_head.as_deref(), Some("Working · 1"));
    assert_eq!(m.live[0].title, "Live one");
}

#[test]
fn an_empty_fleet_says_nothing_needs_you_and_shows_no_badge() {
    let f = fleet(
        vec![row("l1", "Live", "work", State::Live)],
        vec![room("work", 0.0, true)],
        None,
    );
    let m = menu(Some(&f), 0, NOW);
    // Live agents only: the count, no words.
    assert_eq!(m.badge, "1");
    assert_eq!(m.nothing.as_deref(), Some("No agent needs you · 1 working"));
    assert_eq!(m.inbox, None);
    // A profile at its limit reads 0%.
    assert!(
        m.headroom[0].starts_with("work   0% left"),
        "{:?}",
        m.headroom
    );
    let quiet = fleet(Vec::new(), Vec::new(), None);
    assert_eq!(menu(Some(&quiet), 0, NOW).badge, "");
    assert!(menu(Some(&quiet), 0, NOW).headroom.is_empty());
}

#[test]
fn a_session_not_yet_indexed_is_named_by_its_project() {
    let mut r = row("abcdef12-3456", "abcdef12", "work", State::Live);
    assert_eq!(title_of(&r), "New session in demo");
    r.title = "Tidy the cache".into();
    assert_eq!(title_of(&r), "Tidy the cache");
}

#[test]
fn long_titles_are_cut() {
    assert_eq!(cut("abcdef", 4), "abc…");
    assert_eq!(cut("abc", 4), "abc");
}

#[test]
fn the_first_read_only_remembers() {
    let mut w = Watcher::default();
    let f = fleet(
        vec![row(
            "w1",
            "Waiting",
            "work",
            waiting(NOW - Duration::minutes(3)),
        )],
        vec![room("work", 1.0, false)],
        None,
    );
    assert!(w.observe(&f, NOW, ALL, &never).is_empty());
    assert!(w.observe(&f, NOW, ALL, &never).is_empty());
}

#[test]
fn a_new_wait_is_announced_once_and_a_session_at_most_hourly() {
    let mut w = Watcher::default();
    let before = fleet(
        vec![row("w1", "Waiting", "work", State::Live)],
        vec![room("work", 1.0, false)],
        None,
    );
    w.observe(&before, NOW, ALL, &never);
    let asked = NOW - Duration::minutes(1);
    let now = fleet(
        vec![row("w1", "Waiting", "work", waiting(asked))],
        vec![room("work", 1.0, false)],
        None,
    );
    let a = w.observe(&now, NOW, ALL, &never);
    assert_eq!(a.len(), 1);
    assert_eq!(a[0].kind, Kind::Waiting);
    assert_eq!(a[0].action, "Open");
    assert_eq!(
        a[0].body,
        "demo · work · asked you: Shall I go ahead with the migration?"
    );
    // The same wait again: nothing.
    assert!(
        w.observe(&now, NOW + Duration::minutes(1), ALL, &never)
            .is_empty()
    );
    // A second question 20 minutes later is a new wait, but within the hour.
    let again = fleet(
        vec![row(
            "w1",
            "Waiting",
            "work",
            waiting(NOW + Duration::minutes(20)),
        )],
        vec![room("work", 1.0, false)],
        None,
    );
    assert!(
        w.observe(&again, NOW + Duration::minutes(21), ALL, &never)
            .is_empty()
    );
    // A third, more than an hour after the first notification: announced.
    let later = fleet(
        vec![row(
            "w1",
            "Waiting",
            "work",
            waiting(NOW + Duration::minutes(70)),
        )],
        vec![room("work", 1.0, false)],
        None,
    );
    assert_eq!(
        w.observe(&later, NOW + Duration::minutes(71), ALL, &never)
            .len(),
        1
    );
}

#[test]
fn a_stop_then_its_reset_are_announced_unless_switched_off_or_quiet() {
    let at = NOW - Duration::minutes(1);
    let mut w = Watcher::default();
    let live = fleet(
        vec![row("s1", "Stops", "work", State::Live)],
        vec![room("work", 0.3, false)],
        None,
    );
    w.observe(&live, NOW, ALL, &never);
    let out = fleet(
        vec![row("s1", "Stops", "work", stopped(at))],
        vec![room("work", 0.0, true)],
        None,
    );
    let a = w.observe(&out, NOW, ALL, &never);
    assert_eq!(a.len(), 1);
    assert_eq!(a[0].kind, Kind::Stopped);
    assert_eq!(a[0].action, "Resume");
    assert_eq!(
        a[0].body,
        "demo · work · stopped at its usage limit · resets 18:10"
    );
    // Two hours on the limit resets: announced (the hour has passed).
    let back = fleet(
        vec![row("s1", "Stops", "work", stopped(at))],
        vec![room("work", 1.0, false)],
        None,
    );
    let r = w.observe(&back, NOW + Duration::hours(2), ALL, &never);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].kind, Kind::Reset);
    assert_eq!(r[0].body, "demo · work's limit reset; still to resume");

    // Switched off, or quiet: nothing, and not announced later either.
    let mut w = Watcher::default();
    w.observe(&live, NOW, ALL, &never);
    let off = Switches {
        stopped: false,
        ..ALL
    };
    assert!(w.observe(&out, NOW, off, &never).is_empty());
    assert!(w.observe(&out, NOW, ALL, &never).is_empty());
    let mut w = Watcher::default();
    w.observe(&live, NOW, ALL, &never);
    assert!(w.observe(&out, NOW, ALL, &|id| id == "s1").is_empty());
}

#[test]
fn a_carried_on_stop_is_not_announced() {
    let mut w = Watcher::default();
    w.observe(&fleet(Vec::new(), Vec::new(), None), NOW, ALL, &never);
    let carried = fleet(
        vec![row(
            "s1",
            "Stops",
            "work",
            State::Stopped {
                reset: None,
                at: NOW,
                carried_on: true,
            },
        )],
        Vec::new(),
        None,
    );
    assert!(w.observe(&carried, NOW, ALL, &never).is_empty());
}
