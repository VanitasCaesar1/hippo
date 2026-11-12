#![allow(clippy::disallowed_methods)] // tests build temporary workspaces

mod common;

use std::fs;

use hippo_core::adapters::Role;
use hippo_core::index::Index;
use hippo_view::sessions;
use time::macros::datetime;

#[test]
fn rows_come_newest_first_with_an_unreadable_record_last() {
    let env = common::env();
    let ws = common::workspace(&env);
    common::session(&ws, "old", "first task", datetime!(2026-11-05 09:00 UTC));
    common::session(&ws, "new", "second task", datetime!(2026-11-07 09:00 UTC));
    // Sorts before both by name, yet is listed last.
    let dir = ws.hippo_dir().join("sessions");
    fs::write(dir.join("aaa.json"), "{ not json").unwrap();

    let got = sessions::load(&ws).unwrap();
    let ids: Vec<_> = got
        .rows
        .iter()
        .map(|r| r.record.as_ref().map(|row| row.id.as_str()))
        .collect();
    assert_eq!(ids[0], Ok("new"));
    assert_eq!(ids[1], Ok("old"));
    assert!(ids[2].is_err());
    assert_eq!(got.rows[2].path, dir.join("aaa.json"));
    // No index is read, so no turn counts.
    assert_eq!(got.rows[0].record.as_ref().unwrap().turns, None);
}

#[test]
fn turns_counts_and_search_read_the_index() {
    let env = common::env();
    common::index_with(
        &env.home,
        "s1",
        &[
            (Role::User, "how do webhook retries back off"),
            (Role::Assistant, "exponential, capped at five minutes"),
            (Role::Tool, "ran the tests"),
        ],
    );
    let index = Index::open(&env.home, false).unwrap();

    let turns = sessions::turns(&index, "s1").unwrap();
    assert_eq!(turns.len(), 3);
    assert_eq!(turns[2].tool_name.as_deref(), Some("Bash"));
    assert!(sessions::turns(&index, "nope").unwrap().is_empty());

    let counts = sessions::turn_counts(&index).unwrap();
    assert_eq!(counts.get("s1"), Some(&3));

    let hits = sessions::search(&index, "webhook retries").unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].session.as_str(), hits[0].ordinal), ("s1", 0));
    // One quoted phrase: words in another order do not match.
    assert!(sessions::search(&index, "retries webhook")
        .unwrap()
        .is_empty());
    // Quotes in the phrase do not make an invalid query.
    assert!(sessions::search(&index, "say \"hi\"").unwrap().is_empty());
}

#[test]
fn about_gives_titles_projects_and_parents_from_the_transcripts() {
    let env = common::env();
    let ws = common::workspace(&env);
    common::session(&ws, "p1", "", datetime!(2026-11-07 09:00 UTC));
    common::session(&ws, "p1-agent-a1", "", datetime!(2026-11-07 09:01 UTC));
    common::index_with(
        &env.home,
        "p1",
        &[(
            Role::User,
            "Act as the implementation sub-agent for this task.\n\nFix the flaky upload test.",
        )],
    );
    common::index_with(&env.home, "p1-agent-a1", &[(Role::User, "Read the test")]);
    // The index names /transcripts/<id>.jsonl, which does not exist: the
    // origin is empty, the title comes from the opening turn.
    let index = Index::open(&env.home, false).unwrap();
    let list = sessions::load(&ws).unwrap();
    let top = sessions::about(
        &index,
        &list,
        &Default::default(),
        sessions::Which::TopLevel,
    )
    .unwrap();
    assert!(top.contains_key("p1") && !top.contains_key("p1-agent-a1"));
    let about = sessions::about(&index, &list, &top, sessions::Which::All).unwrap();
    assert_eq!(
        about["p1"].title.as_deref(),
        Some("Fix the flaky upload test.")
    );
    assert_eq!(about["p1"].project, None);
    assert_eq!(about["p1-agent-a1"].title.as_deref(), Some("Read the test"));
    assert_eq!(sessions::parent_by_id("p1-agent-a1"), Some("p1"));
    assert_eq!(sessions::parent_by_id("p1"), None);
    // What was read before is kept.
    let mut known = about.clone();
    known.get_mut("p1").unwrap().title = Some("kept".into());
    let again = sessions::about(&index, &list, &known, sessions::Which::All).unwrap();
    assert_eq!(again["p1"].title.as_deref(), Some("kept"));
}
