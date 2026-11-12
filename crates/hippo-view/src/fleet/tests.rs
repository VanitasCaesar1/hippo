//! Tests of Fleet. Every transcript here is synthetic.

use super::*;
use hippo_core::adapters::Role;
use hippo_core::index::{SessionSource, Turn};
use hippo_core::records::Tool;
use hippo_core::redact::redact;
use serde_json::json;
use time::macros::offset;

const ACST: UtcOffset = offset!(+9:30);

fn ts(t: OffsetDateTime) -> String {
    t.format(&Rfc3339).unwrap()
}

fn record(kind: &str, at: OffsetDateTime, extra: serde_json::Value) -> String {
    let mut v = json!({"type": kind, "timestamp": ts(at), "sessionId": "x", "cwd": "/w/app", "entrypoint": "t3-code",
        "message": {"role": kind, "content": "hello"}});
    if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
        for (k, val) in e {
            o.insert(k.clone(), val.clone());
        }
    }
    v.to_string()
}

struct Env {
    tmp: tempfile::TempDir,
    now: OffsetDateTime,
}

impl Env {
    fn new() -> Env {
        // Midday local time, so "today" holds the last hours.
        let now = OffsetDateTime::now_utc()
            .to_offset(ACST)
            .replace_time(time::macros::time!(12:00))
            .to_offset(UtcOffset::UTC);
        Env {
            tmp: tempfile::tempdir().unwrap(),
            now,
        }
    }

    fn projects(&self, profile: &str) -> PathBuf {
        let p = self
            .tmp
            .path()
            .join(profile)
            .join("projects")
            .join("-w-app");
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn write(&self, path: &Path, lines: &[String]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, lines.join("\n") + "\n").unwrap();
    }

    fn roots(&self) -> Vec<PathBuf> {
        vec![
            self.tmp.path().join(".claude").join("projects"),
            self.tmp.path().join(".claude-work").join("projects"),
        ]
    }
}

#[test]
fn a_turn_in_the_last_two_minutes_is_live_and_a_title_record_is_not_a_turn() {
    let e = Env::new();
    let dir = e.projects(".claude");
    // Live: an assistant turn a minute ago.
    e.write(
        &dir.join("live1.jsonl"),
        &[
            record("user", e.now - Duration::minutes(20), json!({})),
            record("assistant", e.now - Duration::minutes(1), json!({})),
        ],
    );
    // Quiet: its last turn was 10 minutes ago, though a client wrote a
    // title record since.
    e.write(
        &dir.join("quiet1.jsonl"),
        &[
            record("user", e.now - Duration::minutes(12), json!({})),
            record("assistant", e.now - Duration::minutes(10), json!({})),
            r#"{"type":"custom-title","customTitle":"Thread name","sessionId":"quiet1"}"#
                .to_owned(),
        ],
    );
    // A sub-agent working keeps its parent live.
    let work = e.projects(".claude-work");
    e.write(
        &work.join("parent1.jsonl"),
        &[record("user", e.now - Duration::minutes(30), json!({}))],
    );
    e.write(
        &work.join("parent1").join("subagents").join("agent-a.jsonl"),
        &[record(
            "assistant",
            e.now - Duration::seconds(30),
            json!({}),
        )],
    );
    let snap = Snapshot::default();
    let fleet = build(&Inputs {
        now: e.now,
        offset: ACST,
        roots: &e.roots(),
        rollup: None,
        usage: &snap,
    });
    let state = |id: &str| fleet.row(id).map(|r| r.state.clone());
    assert_eq!(state("live1"), Some(State::Live));
    assert_eq!(state("parent1"), Some(State::Live));
    assert!(
        matches!(state("quiet1"), Some(State::Quiet { .. })),
        "{:?}",
        state("quiet1")
    );
    assert!(fleet.row("agent-a").is_none(), "sub-agents are not rows");
    let live = fleet.row("live1").unwrap();
    assert_eq!(live.client, Some("T3 Code"));
    assert_eq!(live.profile.as_deref(), Some("default"));
    assert_eq!(live.project.as_deref(), Some("app"));
    assert_eq!(fleet.row("quiet1").unwrap().title, "Thread name");
    assert_eq!(fleet.counts("default"), (1, 2));
    assert_eq!(fleet.live().count(), 2);
}

#[test]
fn a_usage_limit_ending_is_stopped_and_resumes_on_another_profile() {
    let e = Env::new();
    let home = e.tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let dir = e.projects(".claude");
    let path = dir.join("stop1.jsonl");
    let at = e.now - Duration::hours(1);
    e.write(
        &path,
        &[
            record("user", at - Duration::minutes(5), json!({})),
            record("assistant", at, json!({})),
        ],
    );
    let mut index = Index::open(&home, true).unwrap();
    let turn = |o: u32, role: Role, t: OffsetDateTime, tool: Option<&str>, text: &str| Turn {
        ordinal: o,
        role,
        ts: t,
        tool_name: tool.map(str::to_owned),
        text: redact(text),
    };
    index
        .replace_session(
            &SessionSource { id: "stop1", tool: Tool::ClaudeCode, transcript_path: &path, file_hash: "h" },
            &[
                turn(0, Role::User, at - Duration::minutes(5), None, "build the scheduler"),
                turn(1, Role::Tool, at - Duration::minutes(4), Some("TodoWrite"), r#"TodoWrite {"todos":[{"content":"write the cron parser","status":"pending"}]}"#),
                turn(2, Role::Assistant, at, None, "You've hit your 5-hour limit · resets 6:10pm (Australia/Darwin)"),
            ],
        )
        .unwrap();
    let rollup = Rollup::in_memory(None, &index, &["stop1".to_owned()]).unwrap();
    let snap = Snapshot::default();
    let fleet = build(&Inputs {
        now: e.now,
        offset: ACST,
        roots: &e.roots(),
        rollup: Some(&rollup),
        usage: &snap,
    });
    let row = fleet.row("stop1").unwrap();
    assert!(row.state.needs_resume(), "{:?}", row.state);
    let State::Stopped { reset, .. } = &row.state else {
        unreachable!()
    };
    assert_eq!(reset.as_deref(), Some("resets 18:10"));
    assert_eq!(row.turns, Some(3));
    assert_eq!(fleet.stopped().count(), 1);

    let own = resume(row, Some(&index), None).unwrap();
    assert_eq!(own.command, "cd /w/app && claude --resume stop1");
    assert!(
        own.handoff.contains("Open items:\n- write the cron parser"),
        "{}",
        own.handoff
    );
    let work = Headroom {
        profile: "work".into(),
        codex: false,
        dir: Some(e.tmp.path().join(".claude-work")),
        window: usage::WindowState::None,
        estimate: usage::Estimate::Left(1.0),
        limit: None,
        limit_from: 0,
        weekly_until: None,
        last_seen: None,
        from_meter: false,
        meter: None,
        meter_stale: None,
        five: None,
        seven: None,
    };
    let other = resume(row, Some(&index), Some(&work)).unwrap();
    assert_eq!(other.on.as_deref(), Some("work"));
    assert!(other.command.contains("cp -n "), "{}", other.command);
    assert!(
        other
            .command
            .ends_with(".claude-work claude --resume stop1"),
        "{}",
        other.command
    );
    assert!(other.clipboard().starts_with("mkdir -p "));
}

#[test]
fn room_lines_say_estimate_or_none() {
    let mut h = Headroom {
        profile: "work".into(),
        codex: false,
        dir: None,
        window: usage::WindowState::Open {
            start: 0,
            end: 3600,
            used: 1.5e6,
            anchored: false,
        },
        estimate: usage::Estimate::NoEstimate,
        limit: None,
        limit_from: 0,
        weekly_until: None,
        last_seen: None,
        from_meter: false,
        meter: None,
        meter_stale: None,
        five: None,
        seven: None,
    };
    assert_eq!(
        room_line(&h, UtcOffset::UTC),
        "no estimate · resets 1 Jan 01:00 · used 1.5M · est."
    );
    h.estimate = usage::Estimate::Left(0.62);
    h.limit_from = 3;
    assert_eq!(
        room_line(&h, UtcOffset::UTC),
        "62% left · resets 1 Jan 01:00 · from 3 limit hits · est."
    );
    // With the meter: both windows, the 5-hour one from the estimate when
    // the meter is stale.
    let soon = OffsetDateTime::now_utc().unix_timestamp() + 60;
    h.five = Some(usage::Left {
        frac: 0.92,
        source: usage::Source::Estimate,
        resets_at: None,
        reset_passed: None,
    });
    h.seven = Some(usage::Left {
        frac: 0.231,
        source: usage::Source::Meter,
        resets_at: None,
        reset_passed: Some(soon - 120),
    });
    h.meter_stale = Some(OffsetDateTime::now_utc().unix_timestamp() - 3 * 3600);
    let line = room_line(&h, UtcOffset::UTC);
    assert!(
        line.starts_with("5h 92% left (est.), no window · 7d 23% left, reset "),
        "{line}"
    );
    assert!(line.ends_with(" · not refreshed · meter 3h old"), "{line}");
}

#[test]
fn a_claims_line_leads_with_the_first_unverified_claim() {
    use hippo_core::xray::{Claim, ClaimKind, Status};
    let claim = |kind, status, text: &str, why: &str| Claim {
        kind,
        status,
        claim: text.into(),
        ordinal: 1,
        evidence: why.into(),
        pr: None,
    };
    let c = Claims::of(&[
        claim(ClaimKind::Tests, Status::Verified, "Tests pass.", "cargo test · exit 0"),
        claim(
            ClaimKind::Deployed,
            Status::Unverified,
            "Deployed.",
            "no deploy command in the transcript; a deploy command and its success output would count",
        ),
        claim(ClaimKind::Lint, Status::Unclear, "Clippy clean.", "a delegate reported it"),
    ]);
    assert_eq!((c.verified, c.unverified, c.unclear), (1, 1, 1));
    assert_eq!(
        c.line().as_deref(),
        Some("1 claim unverified: \"Deployed.\" · no deploy command in the transcript")
    );
    let ok = Claims::of(&[claim(
        ClaimKind::Tests,
        Status::Verified,
        "Tests pass.",
        "x",
    )]);
    assert_eq!(ok.line().as_deref(), Some("1 claim verified"));
    assert_eq!(Claims::default().line(), None);
}

#[test]
fn a_question_is_waiting_and_a_live_row_says_what_it_is_doing() {
    let e = Env::new();
    let dir = e.projects(".claude");
    let ask = |at: OffsetDateTime, text: &str| {
        record(
            "assistant",
            at,
            json!({"message": {"role": "assistant", "stop_reason": "end_turn",
                "content": [{"type": "text", "text": text}]}}),
        )
    };
    // Asked 12 minutes ago and nothing since: waiting.
    e.write(
        &dir.join("ask1.jsonl"),
        &[
            record("user", e.now - Duration::minutes(30), json!({})),
            ask(
                e.now - Duration::minutes(12),
                "Tests pass.\n\nShould I merge it?",
            ),
        ],
    );
    // Asked a minute ago: waiting beats live.
    e.write(
        &dir.join("ask2.jsonl"),
        &[ask(
            e.now - Duration::minutes(1),
            "Which option do you want?",
        )],
    );
    // Asked, but a sub-agent wrote after the question: not waiting.
    e.write(
        &dir.join("ask3.jsonl"),
        &[ask(e.now - Duration::minutes(5), "Proceed?")],
    );
    e.write(
        &dir.join("ask3").join("subagents").join("agent-a.jsonl"),
        &[record("assistant", e.now - Duration::minutes(3), json!({}))],
    );
    // Live, running a test for 4 minutes.
    e.write(
        &dir.join("busy1.jsonl"),
        &[
            record(
                "assistant",
                e.now - Duration::minutes(4),
                json!({"message": {"role": "assistant", "content": [
                    {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "cargo test --workspace"}}]}}),
            ),
            record("user", e.now - Duration::seconds(30), json!({"type": "user"})),
        ],
    );
    // Live, delegating to two working sub-agents.
    e.write(
        &dir.join("lead1.jsonl"),
        &[record(
            "assistant",
            e.now - Duration::minutes(1),
            json!({"message": {"role": "assistant", "content": [
                {"type": "tool_use", "id": "a1", "name": "Agent", "input": {"prompt": "x"}}]}}),
        )],
    );
    for n in ["a", "b"] {
        e.write(
            &dir.join("lead1")
                .join("subagents")
                .join(format!("agent-{n}.jsonl")),
            &[record(
                "assistant",
                e.now - Duration::seconds(20),
                json!({}),
            )],
        );
    }
    let snap = Snapshot::default();
    let fleet = build(&Inputs {
        now: e.now,
        offset: ACST,
        roots: &e.roots(),
        rollup: None,
        usage: &snap,
    });
    let row = |id: &str| fleet.row(id).unwrap();
    assert_eq!(
        row("ask1").state,
        State::Waiting {
            at: (e.now - Duration::minutes(12))
                .replace_nanosecond(0)
                .unwrap(),
            asked: Some("Should I merge it?".into()),
            plan: false
        }
    );
    assert!(row("ask2").state.is_waiting());
    assert!(!row("ask3").state.is_waiting(), "{:?}", row("ask3").state);
    assert_eq!(row("busy1").state, State::Live);
    // No result for 4 minutes: still running.
    assert_eq!(row("busy1").activity.as_deref(), Some("cargo test · 4m"));
    assert_eq!(
        row("lead1").activity.as_deref(),
        Some("2 sub-agents working · 1m")
    );
    // Needs you: waiting, longest first; waiting rows sort first in Fleet.
    let needs: Vec<(&str, Why)> = fleet
        .needs_you()
        .iter()
        .map(|n| (n.row.id.as_str(), n.why))
        .collect();
    assert_eq!(needs, [("ask1", Why::Waiting), ("ask2", Why::Waiting)]);
    assert!(fleet.rows[0].state.is_waiting() && fleet.rows[1].state.is_waiting());
    assert!(fleet.others().all(|r| !r.state.is_waiting()));
}

#[test]
fn activity_counts_minutes_for_a_long_running_call() {
    let at = OffsetDateTime::now_utc();
    let t = tail::Tail {
        last: Some(at),
        waiting: None,
        activity: Some(tail::Activity {
            label: "cargo test".into(),
            at,
            running: true,
            delegates: false,
            verb: "Bash".into(),
            target: Some("cargo test".into()),
            step: None,
            lines: None,
        }),
    };
    let line = |t: &tail::Tail, now| activity_line(t, 0, now);
    assert_eq!(
        line(&t, at + Duration::minutes(4)),
        Some(("cargo test · 4m".to_owned(), false))
    );
    assert_eq!(
        line(&t, at + Duration::seconds(20)),
        Some(("cargo test · now".to_owned(), false))
    );
    assert_eq!(
        line(&t, at + Duration::minutes(64)),
        Some(("cargo test · 1h 4m".to_owned(), false))
    );
    // Returned, and nothing written for 2 minutes: `last:`, muted.
    let mut done = t.clone();
    done.activity.as_mut().unwrap().running = false;
    assert_eq!(
        line(&done, at + Duration::minutes(6)),
        Some(("last: cargo test · 6m".to_owned(), true))
    );
    // Returned a moment ago: still the current line.
    assert_eq!(
        line(&done, at + Duration::seconds(50)),
        Some(("cargo test · now".to_owned(), false))
    );
}

#[test]
fn spans_and_dates_have_one_form() {
    assert_eq!(span(40), "40s");
    assert_eq!(span(33 * 60), "33m");
    assert_eq!(span(64 * 60), "1h 4m");
    assert_eq!(span(120 * 60), "2h");
    assert_eq!(span(51 * 3600), "2d 3h");
    assert_eq!(age(4 * 60), "4m");
    assert_eq!(age(3 * 3600 + 5), "3h");
    let now = OffsetDateTime::parse("2026-11-10T12:00:00Z", &Rfc3339).unwrap();
    let utc = time::UtcOffset::UTC;
    let t = |s: &str| OffsetDateTime::parse(s, &Rfc3339).unwrap().unix_timestamp();
    assert_eq!(when(t("2026-11-10T09:30:00Z"), now, utc), "09:30");
    assert_eq!(when(t("2026-11-08T04:30:00Z"), now, utc), "Sun 04:30");
    assert_eq!(when(t("2026-10-30T04:30:00Z"), now, utc), "30 Oct 04:30");
}

#[test]
fn profile_marks_are_unique_and_hues_do_not_clash() {
    let ids = identities(&["Ana", "Arlo", "work", "default", "codex", "Andy"]);
    let mark = |p: &str| ids[p].0.as_str();
    assert_eq!(mark("Ana"), "An");
    assert_eq!(mark("Andy"), "Ad");
    assert_eq!(mark("Arlo"), "Ar");
    assert_eq!(mark("work"), "Wo");
    assert_eq!(mark("default"), "De");
    assert_eq!(mark("codex"), "Co");
    let mut hues: Vec<usize> = ids.values().map(|v| v.1).collect();
    hues.sort_unstable();
    hues.dedup();
    assert_eq!(hues.len(), 6, "six profiles, six hues");
    // The same name keeps its mark and hue whatever else is shown with it.
    let alone = identities(&["work"]);
    assert_eq!(alone["work"].0, "Wo");
}

#[test]
fn rooms_sort_by_the_tighter_window_and_codex_goes_last() {
    let room = |name: &str, left: Option<f64>, codex: bool| Headroom {
        profile: name.into(),
        codex,
        dir: None,
        window: usage::WindowState::None,
        estimate: left.map_or(usage::Estimate::NoEstimate, usage::Estimate::Left),
        limit: None,
        limit_from: 0,
        weekly_until: None,
        last_seen: None,
        from_meter: false,
        meter: None,
        meter_stale: None,
        five: None,
        seven: None,
    };
    let rooms = vec![
        room("codex", Some(1.0), true),
        room("a", Some(0.23), false),
        room("b", None, false),
        room("c", Some(1.0), false),
        room("d", Some(0.65), false),
    ];
    let order: Vec<&str> = ordered_rooms(&rooms)
        .iter()
        .map(|h| h.profile.as_str())
        .collect();
    assert_eq!(order, ["c", "d", "a", "b", "codex"]);
}

#[test]
fn use_next_prefers_a_current_meter_over_an_inferred_full_one() {
    let now = 1_800_000_000_i64;
    let left = |frac: f64, passed: bool| usage::Left {
        frac,
        source: usage::Source::Meter,
        resets_at: (!passed).then_some(now + 3600),
        reset_passed: passed.then_some(now - 3600),
    };
    let room = |name: &str, five: f64, seven: usage::Left, fetched: i64| Headroom {
        profile: name.into(),
        codex: false,
        dir: None,
        window: usage::WindowState::None,
        estimate: usage::Estimate::Left(five.min(seven.frac)),
        limit: None,
        limit_from: 0,
        weekly_until: None,
        last_seen: Some(fetched),
        from_meter: false,
        meter: Some(hippo_core::usage::meter::Meter {
            fetched_at: fetched,
            five_hour: None,
            seven_day: None,
        }),
        meter_stale: None,
        five: Some(left(five, false)),
        seven: Some(seven),
    };
    // `guess` reads 100 % on a 7-day reset that passed 3 days ago.
    let guess = room("guess", 1.0, left(1.0, true), now - 3 * 86_400);
    let fresh = room("fresh", 0.9, left(0.55, false), now - 300);
    let low = room("low", 0.9, left(0.2, false), now - 300);
    assert!(inferred(&guess.seven.unwrap()));
    assert_eq!(meter_old(&guess, now), Some(3 * 86_400));
    assert!(current(&fresh, now) && !current(&guess, now));
    let rooms = vec![guess.clone(), fresh, low.clone()];
    assert_eq!(use_next(&rooms, &[], now).unwrap().profile, "fresh");
    // Nothing current with 40 % left: the most left wins.
    let rooms = vec![guess, low];
    assert_eq!(use_next(&rooms, &[], now).unwrap().profile, "guess");
}

fn shelf_row(state: State, pulls: Vec<(String, &'static str)>) -> Row {
    Row {
        id: "s1".into(),
        title: "Fix the parser".into(),
        tool: "claude-code".into(),
        profile: Some("work".into()),
        project: Some("demo".into()),
        client: None,
        cwd: None,
        transcript: PathBuf::from("/h/.claude/projects/p/s1.jsonl"),
        started: None,
        last: None,
        turns: Some(3),
        agents: Some(0),
        edits: 1,
        state,
        pulls,
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

#[test]
fn a_finished_session_settles_when_its_pulls_merge_or_once_opened() {
    let at = OffsetDateTime::now_utc();
    let done = State::Finished { at };
    let pr = |n: u32| format!("https://github.com/o/r/pull/{n}");
    let never = |_: &str| false;
    let always = |_: &str| true;

    // No pull request: settles once opened.
    assert_eq!(
        shelf(&shelf_row(done.clone(), vec![]), never),
        Shelf::ToReview
    );
    assert_eq!(
        shelf(&shelf_row(done.clone(), vec![]), always),
        Shelf::Settled
    );

    // Opened and still open: to review, opened or not.
    let open = shelf_row(done.clone(), vec![(pr(1), "opened")]);
    assert_eq!(shelf(&open, always), Shelf::ToReview);

    // Merged by itself, or by another session.
    let merged = shelf_row(done.clone(), vec![(pr(1), "opened"), (pr(1), "merged")]);
    assert_eq!(shelf(&merged, never), Shelf::Settled);
    let mut elsewhere = shelf_row(done.clone(), vec![(pr(1), "opened"), (pr(2), "opened")]);
    elsewhere.merged_elsewhere = vec![pr(1)];
    assert_eq!(shelf(&elsewhere, always), Shelf::ToReview, "one still open");
    elsewhere.merged_elsewhere.push(pr(2));
    assert_eq!(shelf(&elsewhere, never), Shelf::Settled);

    // Waiting, limited and live sessions never settle.
    let waiting = State::Waiting {
        at,
        asked: Some("Which one?".into()),
        plan: false,
    };
    assert_eq!(shelf(&shelf_row(waiting, vec![]), always), Shelf::NeedsYou);
    let limited = State::Stopped {
        reset: None,
        at,
        carried_on: false,
    };
    assert_eq!(shelf(&shelf_row(limited, vec![]), always), Shelf::NeedsYou);
    let carried = State::Stopped {
        reset: None,
        at,
        carried_on: true,
    };
    assert_eq!(shelf(&shelf_row(carried, vec![]), always), Shelf::Settled);
    assert_eq!(
        shelf(&shelf_row(State::Live, vec![]), never),
        Shelf::Working
    );
    // An unverified claim is a hint on the row, not a reason to need you.
    let mut claimed = shelf_row(done, vec![]);
    claimed.claims.unverified = 1;
    assert_eq!(shelf(&claimed, never), Shelf::ToReview);
}

#[test]
fn line_totals_say_they_are_summed_from_edits() {
    let t = LineTotals {
        added: 429,
        removed: 215,
        files: 4,
        uncounted: 1,
    };
    assert_eq!(t.counts(), "+429 −215");
    assert_eq!(t.note(), "4 files, from its edits · 1 rewrite not counted");
}

#[test]
fn the_tail_counts_an_edit_from_its_structured_patch() {
    let lines = [
        r#"{"type":"assistant","timestamp":"2026-11-10T05:00:00Z","message":{"content":[{"type":"tool_use","id":"t1","name":"Edit","input":{"file_path":"/w/src/billing/InvoiceTable.tsx","old_string":"a","new_string":"b"}}]}}"#,
        r#"{"type":"user","timestamp":"2026-11-10T05:00:02Z","toolUseResult":{"filePath":"/w/src/billing/InvoiceTable.tsx","structuredPatch":[{"lines":[" x","-a","-c","+b"]}]},"message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]}}"#,
    ];
    let t = tail::of_lines(&lines);
    let a = t.activity.unwrap();
    assert_eq!(a.verb, "Edit");
    assert_eq!(a.target.as_deref(), Some("/w/src/billing/InvoiceTable.tsx"));
    assert_eq!(
        a.lines,
        Some(hippo_core::rollup::Lines {
            added: 1,
            removed: 2
        })
    );
    assert!(!a.running);

    // Running, no result yet: counted from its input.
    let t = tail::of_lines(&lines[..1]);
    let a = t.activity.unwrap();
    assert!(a.running);
    assert_eq!(
        a.lines,
        Some(hippo_core::rollup::Lines {
            added: 1,
            removed: 1
        })
    );

    // A shell call with a description is a step line.
    let bash = [
        r#"{"type":"assistant","timestamp":"2026-11-10T05:00:00Z","message":{"content":[{"type":"tool_use","id":"t2","name":"Bash","input":{"command":"cargo test -p kit","description":"Run the kit tests"}}]}}"#,
    ];
    let a = tail::of_lines(&bash).activity.unwrap();
    assert_eq!(a.verb, "Bash");
    assert_eq!(a.step.as_deref(), Some("Run the kit tests"));
    assert_eq!(a.target.as_deref(), Some("cargo test -p kit"));
}

#[test]
fn a_file_both_a_session_and_its_sub_agents_edited_counts_once() {
    let c = |path: &str, a: u64, r: u64| rollup::Changed {
        path: path.to_owned(),
        lines: hippo_core::rollup::Lines {
            added: a,
            removed: r,
        },
        calls: 1,
        uncounted: 0,
    };
    let own = [c("/w/a.rs", 10, 1)];
    let subs = [c("/w/a.rs", 5, 0), c("/w/b.rs", 7, 2)];
    let t = lines_once(subs.iter(), &own).unwrap();
    assert_eq!((t.added, t.removed, t.files), (22, 3, 2));
    assert_eq!(lines_once(std::iter::empty(), &[]), None);
}

#[test]
fn a_run_from_the_index_lists_its_calls_newest_first() {
    let st = |role: &str, tool: Option<&str>, text: &str| hippo_core::index::StoredTurn {
        ordinal: 0,
        role: role.to_owned(),
        ts: "2026-11-08T04:00:00Z".to_owned(),
        tool_name: tool.map(str::to_owned),
        text: text.to_owned(),
    };
    let turns = [
        st("user", None, "Fix the docs"),
        st("tool", Some("Read"), r#"Read {"file_path":"/w/README.md"}"#),
        st("tool", Some("Read"), "1 line"),
        st(
            "tool",
            Some("Edit"),
            r#"Edit {"file_path":"/w/README.md","old_string":"a","new_string":"b"}"#,
        ),
    ];
    let t = tail::turn_of_stored(&turns);
    assert_eq!(t.count, 2);
    assert_eq!(t.calls[0].verb, "Edit");
    assert!(t.calls[0].running, "no result yet");
    assert!(!t.calls[1].running);
}

#[test]
fn a_sub_agent_its_client_says_finished_ends_at_its_newest_turn() {
    let dir = tempfile::tempdir().unwrap();
    let sub = dir.path().join("agent-a1.jsonl");
    std::fs::write(
        &sub,
        "{\"type\":\"assistant\",\"isSidechain\":true,\"timestamp\":\"2026-11-10T05:40:00Z\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"Done.\"}]}}\n",
    )
    .unwrap();
    let open = SubTask {
        kind: "agent",
        title: "Audit it".into(),
        started: OffsetDateTime::parse("2026-11-10T05:00:00Z", &Rfc3339).ok(),
        ended: None,
        status: None,
        child: Some("p-agent-a1".into()),
        input: None,
        report: None,
        tokens: None,
        tools: None,
        exit: None,
    };
    // No .meta.json, or one that does not say finished: still open.
    let mut t = open.clone();
    t.settle(&sub);
    assert_eq!(t.ended, None);
    std::fs::write(
        dir.path().join("agent-a1.meta.json"),
        r#"{"description":"Audit it","finished":true}"#,
    )
    .unwrap();
    let mut t = open.clone();
    t.settle(&sub);
    assert_eq!(
        t.ended,
        OffsetDateTime::parse("2026-11-10T05:40:00Z", &Rfc3339).ok()
    );
    assert_eq!(t.status.as_deref(), Some("completed"));
}

#[test]
fn a_sub_task_with_no_end_is_not_running_for_days() {
    let now = OffsetDateTime::now_utc();
    let t = |hours: i64| SubTask {
        kind: "bash",
        title: "Wait for the build".into(),
        started: Some(now - Duration::hours(hours)),
        ended: None,
        status: None,
        child: None,
        input: None,
        report: None,
        tokens: None,
        tools: None,
        exit: None,
    };
    assert!(t(1).running(&State::Live));
    assert!(!t(30).running(&State::Live));
}
