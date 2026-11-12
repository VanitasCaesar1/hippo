//! Tests of Pulse. Every session, profile and lesson here is synthetic.

use std::path::PathBuf;

use hippo_core::adapters::Role;
use hippo_core::incidents::{Body, Cause, Chain, Step};
use hippo_core::index::{Index, SessionSource, Turn, TurnMark};
use hippo_core::lessons::{Evidence, KeptBy, Seen, Target};
use hippo_core::records::Tool;
use hippo_core::redact::redact;
use time::macros::datetime;

use super::*;
use crate::fleet::{Claims, LineTotals, ServedRule};
use crate::replay::{self, Frame, Lane, Span, Track};

const NOW: OffsetDateTime = datetime!(2026-11-10 16:34 UTC);

fn day_start() -> OffsetDateTime {
    datetime!(2026-11-10 00:00 UTC)
}

fn day(lanes: Vec<Lane>) -> Day {
    Day {
        date: day_start().date(),
        start: day_start(),
        end: day_start() + Duration::DAY,
        read_at: NOW,
        lanes,
        line: Default::default(),
    }
}

fn beats_at(pairs: &[(OffsetDateTime, u16)]) -> Vec<u16> {
    let mut out = Vec::new();
    for (t, v) in pairs {
        let k = beat_of(day_start(), *t).unwrap();
        if out.len() <= k {
            out.resize(k + 1, 0);
        }
        out[k] += v;
    }
    out
}

fn lane(id: &str, profile: &str, beats: Vec<u16>, kids: Vec<Track>) -> Lane {
    Lane {
        track: Track {
            id: id.into(),
            beats,
            ..Track::default()
        },
        profile: Some(profile.into()),
        children: kids,
        ..Lane::default()
    }
}

fn row(id: &str, profile: &str, state: State) -> Row {
    Row {
        id: id.into(),
        title: format!("Task {id}"),
        tool: "claude-code".into(),
        profile: Some(profile.into()),
        project: Some("demo".into()),
        client: None,
        cwd: None,
        transcript: PathBuf::from(format!("/h/.claude/projects/p/{id}.jsonl")),
        started: Some(NOW - Duration::hours(1)),
        last: Some(NOW),
        turns: Some(3),
        agents: Some(0),
        edits: 1,
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
        rules: Vec::new(),
    }
}

fn fleet(rows: Vec<Row>) -> Fleet {
    Fleet {
        rows,
        headroom: Vec::new(),
        use_next: None,
        signed_out: Vec::new(),
        usage_cached: true,
        drains: Vec::new(),
    }
}

fn incident(id: &str, kind: IncidentKind, root: &str, at: OffsetDateTime, said: &str) -> Incident {
    let step = |text: &str| Step {
        session: root.into(),
        ordinal: 7,
        at,
        text: text.into(),
    };
    Incident {
        id: id.into(),
        kind,
        quiet: false,
        at,
        cause: Cause::NoRule,
        minutes: None,
        lines: None,
        tokens: None,
        body: Body {
            chain: Chain {
                root: root.into(),
                root_title: Some(format!("Task {root}")),
                session: root.into(),
                ..Chain::default()
            },
            told: None,
            rule: None,
            did: step("git push --force origin notes-draft"),
            showed: step(said),
            fixed: None,
            other: None,
            read: Vec::new(),
            detail: None,
            subject: None,
        },
    }
}

fn lesson(status: LessonStatus, note: &str, kept: Option<OffsetDateTime>) -> Lesson {
    Lesson {
        id: Ulid::generate(),
        kind: hippo_core::lessons::Kind::Correction,
        key: "staging".into(),
        terms: vec!["staging".into()],
        title: "Previews point at staging".into(),
        rule: "Point previews at staging only.".into(),
        target: Some(Target {
            note: PathBuf::from(note),
            section: None,
            new: false,
        }),
        evidence: vec![Evidence {
            session: "s1".into(),
            ordinal: 7,
            profile: None,
            project: None,
            at: NOW - Duration::minutes(30),
            quote: "staging only".into(),
            action: None,
        }],
        seen: Seen {
            sessions: 2,
            profiles: Vec::new(),
            first: NOW - Duration::days(2),
            last: NOW,
            most_in_a_day: 1,
        },
        status,
        review: None,
        created: NOW - Duration::days(2),
        updated: NOW,
        closed_sessions: None,
        kept: kept.map(|at| KeptBy {
            receipt: Ulid::generate(),
            at,
            edited: false,
        }),
        ask: None,
    }
}

#[test]
fn the_histogram_stacks_each_beat_by_profile_with_sub_agents_in_their_lane() {
    let t = |h: u8, m: u8| day_start() + Duration::hours(h.into()) + Duration::minutes(m.into());
    let kid = Track {
        id: "a-agent-1".into(),
        beats: beats_at(&[(t(16, 30), 4)]),
        ..Track::default()
    };
    let d = day(vec![
        lane(
            "a",
            "work",
            beats_at(&[(t(16, 0), 2), (t(16, 31), 1)]),
            vec![kid],
        ),
        lane("b", "home", beats_at(&[(t(16, 31), 5)]), Vec::new()),
    ]);
    let h = histogram(&d, t(16, 0), t(16, 33), &["work".to_owned()]);
    assert_eq!(h.profiles, vec![Some("work".into()), Some("home".into())]);
    assert_eq!(h.from, t(16, 0));
    // 16:00 to 16:33 is beats 480 to 496.
    assert_eq!(h.bars.len(), 17);
    assert_eq!(h.bars[0], vec![2, 0]);
    // 16:30 and 16:31 share a beat: the lane's 1, its sub-agent's 4.
    assert_eq!(h.bars[15], vec![5, 5]);
    assert_eq!(h.peak(), 10);

    let (beats, waiting) = lane_beats(&d, "a", t(16, 0), t(16, 33)).unwrap();
    assert_eq!(beats[15], 5);
    assert!(waiting.iter().all(|w| !w));
    assert!(lane_beats(&d, "nope", t(16, 0), t(16, 33)).is_none());
}

#[test]
fn a_wait_marks_its_beats() {
    let mut l = lane("a", "work", Vec::new(), Vec::new());
    l.track.spans.push(Span {
        start: NOW - Duration::minutes(10),
        end: NOW,
        kind: SpanKind::Waiting,
        strength: 0.,
        ordinal: None,
        asked: Some("Merge now?".into()),
        open: true,
    });
    let d = day(vec![l]);
    let (_, waiting) = lane_beats(&d, "a", NOW - Duration::minutes(20), NOW).unwrap();
    assert_eq!(waiting.len(), 11);
    assert!(!waiting[0] && !waiting[4]);
    assert!(waiting[5..].iter().all(|w| *w));
}

#[test]
fn a_track_counts_its_turns_per_beat_and_edits_three_times() {
    let mark = |o: u32, at: OffsetDateTime, tool: Option<&str>| TurnMark {
        ordinal: o,
        role: if tool.is_some() { "tool" } else { "assistant" }.into(),
        ts: at
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap(),
        tool_name: tool.map(str::to_owned),
        head: String::new(),
        tail: None,
        file: None,
        about: None,
        command: tool.map(|_| "cargo test".to_owned()),
        call: tool.is_some(),
    };
    let marks = vec![
        mark(0, NOW - Duration::minutes(5), None),
        mark(
            1,
            NOW - Duration::minutes(5) + Duration::seconds(10),
            Some("Edit"),
        ),
        mark(2, NOW - Duration::minutes(1), Some("Bash")),
    ];
    let frame = Frame {
        start: day_start(),
        end: day_start() + Duration::DAY,
        now: NOW,
    };
    let t = replay::track_of("s1", None, &marks, true, frame, None);
    let k = beat_of(day_start(), NOW - Duration::minutes(5)).unwrap();
    assert_eq!(
        t.beats.len(),
        beat_of(day_start(), NOW - Duration::minutes(1)).unwrap() + 1
    );
    assert_eq!(t.beats[k], 4);
    assert_eq!(*t.beats.last().unwrap(), 1);
    assert_eq!(t.calls_between(NOW - LATELY, NOW), 2);
    assert_eq!(t.calls_between(NOW - Duration::minutes(2), NOW), 1);
}

#[test]
fn totals_count_calls_lately_and_lines_today() {
    let mut a = row("a", "work", State::Live);
    a.lines = Some(LineTotals {
        added: 12,
        removed: 4,
        files: 2,
        uncounted: 0,
    });
    let b = row(
        "b",
        "work",
        State::Waiting {
            at: NOW - Duration::minutes(3),
            asked: None,
            plan: false,
        },
    );
    let f = fleet(vec![a, b]);
    let mut l = lane("a", "work", Vec::new(), Vec::new());
    l.track.steps = vec![
        replay::StepAt {
            at: NOW - Duration::minutes(9),
            ordinal: 1,
            words: "old".into(),
        },
        replay::StepAt {
            at: NOW - Duration::minutes(2),
            ordinal: 2,
            words: "new".into(),
        },
    ];
    let t = totals(&f, Some(&day(vec![l])), NOW);
    assert_eq!((t.working, t.need_you), (1, 1));
    assert_eq!(t.calls_lately, 1);
    assert_eq!((t.added, t.removed), (12, 4));
}

#[test]
fn next_up_ranks_runs_dry_then_claims_then_destructive_then_corrections() {
    let mut done = row(
        "done",
        "home",
        State::Finished {
            at: NOW - Duration::minutes(8),
        },
    );
    done.claims = Claims {
        verified: 0,
        unverified: 1,
        unclear: 0,
        first_unverified: Some(("Typecheck is clean.".into(), "no typecheck ran".into())),
    };
    let mut f = fleet(vec![
        row("w1", "work", State::Live),
        row("w2", "work", State::Live),
        row("h1", "home", State::Live),
        done,
    ]);
    let drain = |empty: Option<i64>| hippo_core::usage::Drain {
        start: NOW.unix_timestamp() - 3600,
        end: NOW.unix_timestamp() + 2400,
        samples: vec![0.5, 0.9],
        used: 0.9,
        burn: 0.4,
        empty_at: empty,
    };
    f.drains = vec![
        ("work".into(), drain(Some(NOW.unix_timestamp() + 840))),
        ("home".into(), drain(None)),
    ];
    f.use_next = Some("home".into());
    let incidents = vec![
        incident(
            "i1",
            IncidentKind::Destructive,
            "w2",
            NOW - Duration::minutes(20),
            "it is gone",
        ),
        incident(
            "i2",
            IncidentKind::Corrected,
            "s1",
            NOW - Duration::minutes(30),
            "staging only, never production",
        ),
        incident(
            "i3",
            IncidentKind::Corrected,
            "w1",
            NOW - Duration::minutes(2),
            "no, why did you overwrite my draft",
        ),
        incident(
            "i4",
            IncidentKind::Destructive,
            "w1",
            NOW - Duration::days(1),
            "yesterday",
        ),
    ];
    let lessons = vec![lesson(LessonStatus::Pending, "notes/deploy.md", None)];
    let corrected = corrections(&incidents, &lessons, day_start(), None);
    assert_eq!(corrected.len(), 2);
    assert_eq!(corrected[0].incident, "i3", "newest first");
    assert_eq!(corrected[0].fate, Fate::Tonight);
    assert!(matches!(corrected[1].fate, Fate::Candidate { seen: 2, .. }));
    // Once the nightly pass has run past it, a correction is read.
    let read = corrections(&incidents, &lessons, day_start(), Some(NOW));
    assert_eq!(read[0].fate, Fate::Read);

    let next = next_up(&f, &incidents, &corrected, day_start());
    let ranks: Vec<u8> = next.iter().map(Next::rank).collect();
    assert_eq!(ranks, vec![0, 1, 2, 3], "{next:?}");
    let Next::RunsDry {
        profile,
        sessions,
        to,
        ..
    } = &next[0]
    else {
        unreachable!()
    };
    assert_eq!(profile, "work");
    assert_eq!(sessions, &vec!["w1".to_owned(), "w2".to_owned()]);
    assert_eq!(to.as_deref(), Some("home"));
    let Next::Destructive { command, .. } = &next[2] else {
        unreachable!()
    };
    assert_eq!(command, "git push --force origin notes-draft");
    let Next::Correction { said, .. } = &next[3] else {
        unreachable!()
    };
    assert_eq!(said, "staging only, never production");
}

#[test]
fn a_rule_is_served_when_its_note_was_in_the_block_and_it_was_kept_before() {
    let notes: HashSet<String> = ["notes/deploy.md".to_owned()].into_iter().collect();
    let began = Some(NOW - Duration::hours(1));
    let lessons = vec![
        lesson(
            LessonStatus::Kept,
            "notes/deploy.md",
            Some(NOW - Duration::days(1)),
        ),
        // Kept after the session began.
        lesson(LessonStatus::Kept, "notes/deploy.md", Some(NOW)),
        // Its note was not in the block.
        lesson(
            LessonStatus::Kept,
            "notes/other.md",
            Some(NOW - Duration::days(1)),
        ),
        // Not kept.
        lesson(LessonStatus::Pending, "notes/deploy.md", None),
    ];
    let served_rules = crate::fleet::rules_served(&notes, began, &lessons);
    assert_eq!(
        served_rules,
        vec![ServedRule {
            title: "Previews point at staging".into(),
            note: "notes/deploy.md".into(),
        }]
    );
    let mut r = row("a", "work", State::Live);
    r.rules = served_rules;
    assert_eq!(served(&[r, row("b", "work", State::Live)]), 1);
    assert!(crate::fleet::rules_served(&notes, None, &lessons).is_empty());
}

#[test]
fn tonight_counts_the_sessions_since_the_last_pass() {
    let home = tempfile::tempdir().unwrap();
    let mut index = Index::open(home.path(), true).unwrap();
    let path = home.path().join("s1.jsonl");
    let at = NOW - Duration::hours(2);
    let turn = |o: u32, role: Role, t: OffsetDateTime, text: &str| Turn {
        ordinal: o,
        role,
        ts: t,
        tool_name: None,
        text: redact(text),
    };
    index
        .replace_session(
            &SessionSource {
                id: "s1",
                tool: Tool::ClaudeCode,
                transcript_path: &path,
                file_hash: "h",
            },
            &[
                turn(0, Role::User, at, "rename the helper"),
                turn(1, Role::Assistant, at + Duration::minutes(3), "Renamed it."),
            ],
        )
        .unwrap();
    let rollup = Rollup::in_memory(None, &index, &["s1".to_owned()]).unwrap();
    let t = tonight(&rollup, "03:30", Some(NOW - Duration::hours(10)), 3, NOW);
    assert_eq!(t.at, "03:30");
    assert_eq!((t.sessions, t.notes_due), (1, 3));
    let later = tonight(&rollup, "03:30", Some(NOW - Duration::minutes(10)), 0, NOW);
    assert_eq!(later.sessions, 0);
}

#[test]
fn a_quiet_session_in_the_middle_of_a_call_is_working() {
    let mut building = row(
        "b",
        "work",
        State::Quiet {
            since: Some(NOW - Duration::minutes(6)),
        },
    );
    building.call = Some(crate::fleet::tail::Activity {
        label: "cargo build".into(),
        at: NOW - Duration::minutes(6),
        running: true,
        delegates: false,
        verb: "Bash".into(),
        target: Some("cargo build".into()),
        step: None,
        lines: None,
    });
    let idle = row(
        "i",
        "work",
        State::Quiet {
            since: Some(NOW - Duration::minutes(6)),
        },
    );
    assert!(building.working() && !idle.working());
    let f = fleet(vec![building, idle]);
    assert_eq!(totals(&f, None, NOW).working, 1);
    assert_eq!(live_by_profile(&f.rows).get("work"), Some(&1));
}
