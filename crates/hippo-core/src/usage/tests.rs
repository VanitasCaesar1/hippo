use super::*;
use serde_json::json;
use std::io::Write as _;
use time::macros::{datetime, offset};

const ACST: UtcOffset = offset!(+9:30);

fn assistant(ts: &str, id: &str, input: u64, output: u64) -> String {
    json!({"type": "assistant", "timestamp": ts, "requestId": "req",
        "message": {"id": id, "model": "claude-x", "role": "assistant",
            "content": [{"type": "text", "text": "ok"}],
            "usage": {"input_tokens": input, "cache_creation_input_tokens": 0,
                      "cache_read_input_tokens": 0, "output_tokens": output}}})
    .to_string()
}

fn limit(ts: &str, text: &str) -> String {
    json!({"type": "assistant", "timestamp": ts, "isApiErrorMessage": true, "error": "rate_limit",
        "message": {"id": "synthetic", "model": "<synthetic>", "role": "assistant",
            "content": [{"type": "text", "text": text}],
            "usage": {"input_tokens": 0, "output_tokens": 0}}})
    .to_string()
}

#[test]
fn weights_are_the_relative_prices() {
    assert_eq!(weighted(100.0, 0.0, 0.0, 0.0), 100.0);
    assert_eq!(weighted(0.0, 100.0, 1000.0, 10.0), 125.0 + 100.0 + 50.0);
}

#[test]
fn a_reset_is_read_in_local_time_after_the_message() {
    let at = datetime!(2026-11-04 04:00:26 UTC); // 13:30 at +09:30
    let r = parse_reset(
        "You've hit your 5-hour limit · resets 2:40pm (Australia/Darwin)",
        at,
        ACST,
    );
    assert_eq!(r, Some(datetime!(2026-11-04 14:40 +9:30)));
    // A time already past today is tomorrow's.
    let r = parse_reset("resets 10am (Australia/Darwin)", at, ACST);
    assert_eq!(r, Some(datetime!(2026-11-05 10:00 +9:30)));
    let r = parse_reset("resets 12am", at, ACST);
    assert_eq!(r, Some(datetime!(2026-11-05 00:00 +9:30)));
    // A weekly reset names its day.
    let r = parse_reset(
        "You've hit your weekly limit · resets Oct 18 at 5pm (Australia/Darwin)",
        datetime!(2026-10-15 10:00 UTC),
        ACST,
    );
    assert_eq!(r, Some(datetime!(2026-10-18 17:00 +9:30)));
    assert_eq!(parse_reset("You've hit your 5-hour limit", at, ACST), None);
}

#[test]
fn lines_give_messages_limits_and_nothing_else() {
    let m = parse_line(
        &assistant("2026-11-04T04:00:00Z", "msg_1", 10, 2),
        "work",
        false,
        ACST,
    );
    assert_eq!(
        m,
        Some(Line::Message(Message {
            profile: "work".into(),
            id: "msg_1".into(),
            ts: datetime!(2026-11-04 04:00 UTC).unix_timestamp(),
            units: 20.0
        }))
    );
    let l = parse_line(
        &limit(
            "2026-11-04T04:00:00Z",
            "You've hit your session limit · resets 2:40pm (Australia/Darwin)",
        ),
        "work",
        false,
        ACST,
    );
    let Some(Line::Limit(h)) = l else {
        panic!("{l:?}")
    };
    assert_eq!(h.kind, LimitKind::Window);
    assert_eq!(
        h.reset,
        Some(datetime!(2026-11-04 14:40 +9:30).unix_timestamp())
    );
    let w = parse_line(
        &limit(
            "2026-10-15T10:00:00Z",
            "You've hit your weekly limit · resets Oct 18 at 5pm (Australia/Darwin)",
        ),
        "work",
        false,
        ACST,
    );
    assert!(matches!(
        w,
        Some(Line::Limit(LimitHit {
            kind: LimitKind::Weekly,
            ..
        }))
    ));
    // A person quoting the message is not a limit hit.
    let user = json!({"type": "user", "timestamp": "2026-11-04T04:00:00Z", "message": {"role": "user", "content": "You've hit your session limit · resets 2pm"}}).to_string();
    assert_eq!(parse_line(&user, "work", false, ACST), None);
    assert_eq!(parse_line("not json", "work", false, ACST), None);
}

#[test]
fn codex_meters_need_a_primary_window() {
    let none = json!({"timestamp": "2026-11-04T04:00:00Z", "type": "event_msg", "payload": {"type": "token_count", "rate_limits": {"primary": null, "used_percent": 1}}}).to_string();
    assert_eq!(parse_line(&none, "codex", true, ACST), None);
    let some = json!({"timestamp": "2026-11-04T04:00:00Z", "type": "event_msg", "payload": {"type": "token_count", "rate_limits": {"primary": {"used_percent": 40.0, "window_minutes": 300, "resets_in_seconds": 600}}}}).to_string();
    let Some(Line::Meter(m)) = parse_line(&some, "codex", true, ACST) else {
        panic!()
    };
    assert_eq!(m.used_percent, 40.0);
    assert_eq!(
        m.resets_at,
        Some(datetime!(2026-11-04 04:10 UTC).unix_timestamp())
    );
}

fn hit(profile: &str, ts: i64, reset: i64) -> LimitHit {
    LimitHit {
        profile: profile.into(),
        ts,
        kind: LimitKind::Window,
        reset: Some(reset),
    }
}

#[test]
fn the_limit_is_the_median_use_at_each_hit() {
    let h = 3600;
    // Window 1: reset at 10h, so it started at 5h; 300 used before the hit.
    // Window 2: reset at 30h; 500 used before its first hit.
    let msgs = vec![
        (4 * h, 999.0),
        (5 * h, 100.0),
        (6 * h, 200.0),
        (25 * h, 250.0),
        (26 * h, 250.0),
        (29 * h, 50.0),
    ];
    let hits = [
        hit("p", 7 * h, 10 * h),
        hit("p", 8 * h, 10 * h),
        hit("p", 27 * h, 30 * h),
    ];
    let refs: Vec<&LimitHit> = hits.iter().collect();
    assert_eq!(learn_limit(&msgs, &refs), (Some(400.0), 2));
    assert_eq!(learn_limit(&msgs, &[]), (None, 0));
}

#[test]
fn the_current_window_starts_at_the_first_message_after_the_last_ended() {
    let h = 3600;
    let msgs = vec![(0, 10.0), (h, 10.0), (6 * h, 5.0), (7 * h, 5.0)];
    // At 8h: the window opened at 6h, ends at 11h, 10 used.
    assert_eq!(
        current_window(&msgs, &[], 8 * h),
        WindowState::Open {
            start: 6 * h,
            end: 11 * h,
            used: 10.0,
            anchored: false
        }
    );
    // Quiet for five hours: no window.
    assert_eq!(current_window(&msgs, &[], 12 * h), WindowState::None);
    // A reset ahead: limited until then.
    let hits = [hit("p", 7 * h, 9 * h)];
    let refs: Vec<&LimitHit> = hits.iter().collect();
    assert_eq!(
        current_window(&msgs, &refs, 8 * h),
        WindowState::Limited { reset: 9 * h }
    );
    // After it, the reset anchored that window (4h to 9h); a message at 9h
    // starts a new one.
    let msgs2 = vec![(6 * h, 5.0), (7 * h, 5.0), (9 * h + 660, 1.0)];
    assert_eq!(
        current_window(&msgs2, &refs, 10 * h),
        WindowState::Open {
            start: 9 * h + 600,
            end: 14 * h + 600,
            used: 1.0,
            anchored: false
        }
    );
}

fn snapshot(now: i64) -> Snapshot {
    let h = 3600;
    let mut messages = BTreeMap::new();
    // "work" hit its limit once at 400 used; now 100 used in an open window.
    messages.insert(
        "work".to_owned(),
        vec![(now - 30 * h, 400.0), (now - h, 100.0)],
    );
    // "home" is quiet.
    messages.insert("home".to_owned(), vec![(now - 10 * h, 50.0)]);
    // "new" is busy but never hit a limit.
    messages.insert("new".to_owned(), vec![(now - h, 5000.0)]);
    Snapshot {
        profiles: ["work", "home", "new"]
            .iter()
            .map(|n| ProfileInfo {
                name: (*n).into(),
                codex: false,
                dir: None,
            })
            .collect(),
        messages,
        limits: vec![hit("work", now - 29 * h, now - 27 * h)],
        meters: vec![],
        cached: true,
        claude_meters: BTreeMap::new(),
    }
}

fn client(fetched_at: i64, five: (f64, Option<i64>), seven: (f64, Option<i64>)) -> meter::Meter {
    meter::Meter {
        fetched_at,
        five_hour: Some(meter::Window {
            used_percent: five.0,
            resets_at: five.1,
        }),
        seven_day: Some(meter::Window {
            used_percent: seven.0,
            resets_at: seven.1,
        }),
    }
}

#[test]
fn the_meter_sets_headroom_and_the_tighter_window_picks_use_next() {
    let now = datetime!(2026-11-09 12:00 UTC);
    let t = now.unix_timestamp();
    let h = 3600;
    let mut snap = snapshot(t);
    // home: plenty of its 5 hours, little of its week.
    snap.claude_meters.insert(
        "home".into(),
        client(t - 240, (8.0, Some(t + 2 * h)), (77.0, Some(t + 40 * h))),
    );
    // work: both untouched, though its transcript estimate says 75 %.
    snap.claude_meters.insert(
        "work".into(),
        client(t - 120, (0.0, None), (0.0, Some(t + 100 * h))),
    );
    let rooms = headroom(&snap, now);
    let by = |n: &str| rooms.iter().find(|r| r.profile == n).unwrap();
    let home = by("home");
    assert_eq!(home.five.unwrap().source, Source::Meter);
    assert!((home.five.unwrap().frac - 0.92).abs() < 1e-9);
    assert!((home.seven.unwrap().frac - 0.23).abs() < 1e-9);
    assert!((home.left().unwrap() - 0.23).abs() < 1e-9);
    assert!(home.metered() && home.meter_stale.is_none());
    assert_eq!(by("work").left(), Some(1.0));
    assert_eq!(by("work").five.unwrap().resets_at, None);
    assert_eq!(use_next(&rooms).map(|r| r.profile.as_str()), Some("work"));
}

#[test]
fn a_stale_meter_falls_back_to_the_estimate_and_a_past_reset_has_reset() {
    let now = datetime!(2026-11-09 12:00 UTC);
    let t = now.unix_timestamp();
    let h = 3600;
    let mut snap = snapshot(t);
    // Fetched 3 hours ago; work has a message an hour ago, so it is stale.
    snap.claude_meters.insert(
        "work".into(),
        client(t - 3 * h, (50.0, Some(t + h)), (60.0, Some(t + 50 * h))),
    );
    // home: fetched 9 hours ago, nothing used since (its last message was
    // 10 hours ago); its week reset 2 hours ago.
    snap.claude_meters.insert(
        "home".into(),
        client(t - 9 * h, (0.0, None), (100.0, Some(t - 2 * h))),
    );
    let rooms = headroom(&snap, now);
    let by = |n: &str| rooms.iter().find(|r| r.profile == n).unwrap();
    let work = by("work");
    assert_eq!(work.meter_stale, Some(t - 3 * h));
    assert_eq!(work.five.unwrap().source, Source::Estimate);
    assert!((work.five.unwrap().frac - 0.75).abs() < 1e-9);
    // The 7-day figure stays the meter's, and the tighter one rules.
    assert!((work.left().unwrap() - 0.4).abs() < 1e-9);
    let home = by("home");
    assert_eq!(home.meter_stale, None, "nothing used since it was fetched");
    let seven = home.seven.unwrap();
    assert_eq!(seven.frac, 1.0);
    assert_eq!(seven.reset_passed, Some(t - 2 * h));
    assert!(home.usable());
}

#[test]
fn a_used_up_window_is_not_usable() {
    let now = datetime!(2026-11-09 12:00 UTC);
    let t = now.unix_timestamp();
    let mut snap = snapshot(t);
    snap.claude_meters.insert(
        "home".into(),
        client(t - 60, (100.0, Some(t + 600)), (10.0, None)),
    );
    let rooms = headroom(&snap, now);
    let home = rooms.iter().find(|r| r.profile == "home").unwrap();
    assert_eq!(home.left(), Some(0.0));
    assert!(!home.usable());
    assert_eq!(use_next(&rooms).map(|r| r.profile.as_str()), Some("work"));
}

#[test]
fn snapshot_reads_each_profiles_meter_from_its_settings_file() {
    let tmp = tempfile::tempdir().unwrap();
    let mut snap = Snapshot {
        profiles: vec![
            ProfileInfo {
                name: "default".into(),
                codex: false,
                dir: Some(tmp.path().join(".claude")),
            },
            ProfileInfo {
                name: "work".into(),
                codex: false,
                dir: Some(tmp.path().join(".claude-work")),
            },
        ],
        ..Snapshot::default()
    };
    fs::create_dir_all(tmp.path().join(".claude-work")).unwrap();
    let body = |pct: u32| {
        format!(
            "{{\"oauthAccount\":{{\"emailAddress\":\"a@b\"}},\"cachedUsageUtilization\":{{\"fetchedAtMs\":2000,\"utilization\":{{\"five_hour\":{{\"utilization\":{pct},\"resets_at\":null}}}}}}}}"
        )
    };
    fs::write(tmp.path().join(".claude.json"), body(5)).unwrap();
    fs::write(tmp.path().join(".claude-work/.claude.json"), body(9)).unwrap();
    snap.read_meters();
    assert_eq!(
        snap.claude_meters["default"]
            .five_hour
            .unwrap()
            .used_percent,
        5.0
    );
    assert_eq!(
        snap.claude_meters["work"].five_hour.unwrap().used_percent,
        9.0
    );
}

#[test]
fn headroom_is_an_estimate_or_says_there_is_none() {
    let now = datetime!(2026-11-09 12:00 UTC);
    let rooms = headroom(&snapshot(now.unix_timestamp()), now);
    let by = |n: &str| rooms.iter().find(|r| r.profile == n).unwrap();
    assert_eq!(by("work").estimate, Estimate::Left(0.75));
    assert_eq!(by("work").limit_from, 1);
    assert_eq!(by("home").estimate, Estimate::Left(1.0));
    assert_eq!(by("home").window, WindowState::None);
    // Never hit a limit: no estimate, never a made-up number.
    assert_eq!(by("new").estimate, Estimate::NoEstimate);
    assert_eq!(use_next(&rooms).map(|r| r.profile.as_str()), Some("home"));
    // A profile whose sign-in failed is left out.
    assert_eq!(
        use_next_except(&rooms, &["home".to_owned()]).map(|r| r.profile.as_str()),
        Some("work")
    );
}

#[test]
fn a_weekly_limit_ahead_leaves_nothing() {
    let now = datetime!(2026-11-09 12:00 UTC);
    let mut snap = snapshot(now.unix_timestamp());
    snap.limits.push(LimitHit {
        profile: "home".into(),
        ts: now.unix_timestamp() - 60,
        kind: LimitKind::Weekly,
        reset: Some(now.unix_timestamp() + 86400),
    });
    let rooms = headroom(&snap, now);
    let home = rooms.iter().find(|r| r.profile == "home").unwrap();
    assert_eq!(home.estimate, Estimate::Left(0.0));
    assert!(!home.usable());
    assert_eq!(use_next(&rooms).map(|r| r.profile.as_str()), Some("work"));
}

#[test]
fn refresh_reads_each_file_once_then_only_its_new_lines() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let projects = tmp.path().join(".claude-work").join("projects").join("-w");
    fs::create_dir_all(&projects).unwrap();
    let file = projects.join("s1.jsonl");
    let now = OffsetDateTime::now_utc();
    let ts = |m: i64| (now - Duration::minutes(m)).format(&Rfc3339).unwrap();
    // The same message twice (once per content block) counts once.
    fs::write(
        &file,
        format!(
            "{}\n{}\n",
            assistant(&ts(30), "m1", 10, 0),
            assistant(&ts(30), "m1", 10, 0)
        ),
    )
    .unwrap();
    let roots = vec![tmp.path().join(".claude-work").join("projects")];
    let done = refresh_roots(&home, &roots, now, ACST).unwrap();
    assert_eq!(
        done,
        Refreshed {
            read: 1,
            messages: 1
        }
    );
    // Unchanged: nothing read.
    assert_eq!(
        refresh_roots(&home, &roots, now, ACST).unwrap(),
        Refreshed::default()
    );
    // A new line, and a half-written one that waits for its end.
    let mut f = fs::OpenOptions::new().append(true).open(&file).unwrap();
    write!(f, "{}\n{{\"type\":\"assist", assistant(&ts(10), "m2", 5, 0)).unwrap();
    drop(f);
    let snap = Snapshot::read(&home, &roots, now, ACST).unwrap();
    assert!(snap.cached);
    assert_eq!(
        snap.messages["work"].iter().map(|m| m.1).sum::<f64>(),
        15.0,
        "the reader adds the tail"
    );
    assert_eq!(
        snap.profiles[0].dir.as_deref(),
        Some(tmp.path().join(".claude-work").as_path())
    );
    let done = refresh_roots(&home, &roots, now, ACST).unwrap();
    assert_eq!(done.messages, 1);
}

#[test]
fn without_a_cache_only_recent_files_are_read() {
    let tmp = tempfile::tempdir().unwrap();
    let projects = tmp.path().join(".claude").join("projects").join("-w");
    fs::create_dir_all(&projects).unwrap();
    let now = OffsetDateTime::now_utc();
    fs::write(
        projects.join("a.jsonl"),
        format!("{}\n", assistant(&now.format(&Rfc3339).unwrap(), "m", 1, 0)),
    )
    .unwrap();
    let snap = Snapshot::read(
        &tmp.path().join("nohome"),
        &[tmp.path().join(".claude").join("projects")],
        now,
        ACST,
    )
    .unwrap();
    assert!(!snap.cached);
    assert_eq!(snap.messages["default"].len(), 1);
    assert!(
        !tmp.path().join("nohome").exists(),
        "a reader writes nothing"
    );
}

#[test]
fn a_drain_follows_the_window_and_says_when_it_runs_dry() {
    let now = datetime!(2026-11-09 12:00 UTC);
    let t = now.unix_timestamp();
    let m = 60;
    let profile = |name: &str, codex: bool| ProfileInfo {
        name: name.into(),
        codex,
        dir: None,
    };
    // "fast": a limit of 1000 learned; 100 used early, then 300 in the last
    // half hour: 40 % used, 60 % per hour, dry in 40 minutes.
    let mut messages = BTreeMap::new();
    messages.insert(
        "fast".to_owned(),
        vec![
            (t - 30 * 3600, 1000.0),
            (t - 120 * m, 100.0),
            (t - 20 * m, 150.0),
            (t - 5 * m, 150.0),
        ],
    );
    // "slow": its meter says 20 % used; the window's tokens are 200.
    messages.insert(
        "slow".to_owned(),
        vec![(t - 100 * m, 150.0), (t - 10 * m, 50.0)],
    );
    let mut snap = Snapshot {
        profiles: vec![
            profile("fast", false),
            profile("slow", false),
            profile("cx", true),
        ],
        messages,
        limits: vec![hit("fast", t - 29 * 3600, t - 27 * 3600)],
        meters: vec![
            Meter {
                profile: "cx".into(),
                ts: t - 60 * m,
                used_percent: 10.0,
                window_minutes: Some(300),
                resets_at: Some(t + 100 * m),
            },
            Meter {
                profile: "cx".into(),
                ts: t - 2 * m,
                used_percent: 70.0,
                window_minutes: Some(300),
                resets_at: Some(t + 100 * m),
            },
        ],
        cached: true,
        claude_meters: BTreeMap::new(),
    };
    snap.claude_meters.insert(
        "slow".into(),
        client(
            t - 60,
            (20.0, Some(t + 200 * m)),
            (5.0, Some(t + 90 * 3600)),
        ),
    );
    let rooms = headroom(&snap, now);
    let by = |n: &str| rooms.iter().find(|r| r.profile == n).unwrap();

    let fast = drain(&snap, by("fast"), now).unwrap();
    assert!((fast.used - 0.4).abs() < 1e-9, "{fast:?}");
    assert!((fast.burn - 0.6).abs() < 1e-9);
    assert_eq!(fast.empty_at, Some(t + 60 * m));
    assert!(fast.early_by().unwrap() > 0);
    assert!(fast.samples.windows(2).all(|w| w[0] <= w[1]));
    assert_eq!(
        fast.samples.len() as i64,
        (t - fast.start - 1) / DRAIN_STEP + 1
    );

    let slow = drain(&snap, by("slow"), now).unwrap();
    assert_eq!(slow.end, t + 200 * m);
    assert!((slow.used - 0.2).abs() < 1e-9);
    // 50 of 200 tokens in the last half hour: 5 % used, 10 % per hour.
    assert!((slow.burn - 0.1).abs() < 1e-9);
    assert_eq!(slow.empty_at, None);

    let cx = drain(&snap, by("cx"), now).unwrap();
    assert!((cx.used - 0.7).abs() < 1e-9);
    // 60 points over 58 minutes.
    assert!((cx.burn - 0.6 / (58.0 / 60.0)).abs() < 1e-9);
    assert!(cx.empty_at.is_some_and(|e| e < t + 100 * m));
    // A profile with no limit learned and no meter has no drain.
    snap.limits.clear();
    let rooms = headroom(&snap, now);
    let fast = rooms.iter().find(|r| r.profile == "fast").unwrap();
    assert_eq!(drain(&snap, fast, now), None);
}
