//! Tests of incidents. Every transcript here is synthetic.

use std::fs;
use std::path::Path;

use time::format_description::well_known::Rfc3339;
use time::{Duration, OffsetDateTime};

use super::*;
use crate::adapters::Role;
use crate::config::Workspace;
use crate::index::{Index, SessionSource, Turn};
use crate::records::Tool;
use crate::redact::redact;
use crate::rollup::{refresh_with, Rollup};

/// A time `minutes` after a base three hours ago, so the window holds it.
fn t(minutes: i64) -> String {
    let base = OffsetDateTime::now_utc() - Duration::hours(3);
    (base + Duration::minutes(minutes))
        .replace_nanosecond(0)
        .unwrap()
        .format(&Rfc3339)
        .unwrap()
}

fn turn(ordinal: u32, role: Role, ts: &str, tool: Option<&str>, text: &str) -> Turn {
    Turn {
        ordinal,
        role,
        ts: OffsetDateTime::parse(ts, &Rfc3339).unwrap(),
        tool_name: tool.map(str::to_owned),
        text: redact(text),
    }
}

/// A session from `(role, minute, tool, text)` rows.
fn put(index: &mut Index, home: &Path, id: &str, rows: &[(Role, i64, Option<&str>, &str)]) {
    let turns: Vec<Turn> = rows
        .iter()
        .enumerate()
        .map(|(i, (role, m, tool, text))| turn(i as u32, *role, &t(*m), *tool, text))
        .collect();
    let path = home.join(".claude/projects/q").join(format!("{id}.jsonl"));
    let source = SessionSource {
        id,
        tool: Tool::ClaudeCode,
        transcript_path: &path,
        file_hash: id,
    };
    index.replace_session(&source, &turns).unwrap();
}

fn workspace() -> (tempfile::TempDir, Workspace) {
    let dir = tempfile::tempdir().unwrap();
    for (path, text) in [
        (
            "Rules.md",
            "# Agent rules\n\n- Never use unwrap in parser code; return an error.\n\n- Never run git reset --hard on a shared checkout.\n",
        ),
        ("Questions.md", "# Open questions\n\nNone yet.\n"),
    ] {
        let p = dir.path().join("brain").join(path);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }
    let ws = Workspace {
        root: dir.path().to_path_buf(),
        notes_dir: "brain".into(),
        entry_note: "brain/Questions.md".into(),
        required_notes: vec!["brain/Rules.md".into()],
        budget_bytes: 25600,
        log_path: "brain/Session log.md".into(),
        sample_tasks: Vec::new(),
        passes_off: Vec::new(),
        lessons: true,
        snapshot_keep_days: None,
        log_notes: None,
    };
    (dir, ws)
}

/// Injected notes for a session, as the x-ray would have recorded them.
fn inject(home: &Path, session: &str, notes: &[(&str, &str)]) {
    let conn = Connection::open(home.join(crate::rollup::FILE_NAME)).unwrap();
    for (i, (note, used)) in notes.iter().enumerate() {
        conn.execute(
            "INSERT INTO context(session_id, ord, source, kind, label, note, bytes, use) VALUES (?1, ?2, 'transcript', 'required', ?3, ?3, 10, ?4)",
            params![session, i as i64 + 100, note, used],
        )
        .unwrap();
    }
}

fn rebuild_now(home: &Path, index: &Index, ws: Option<&Workspace>) -> Vec<Incident> {
    let mut conn = Connection::open(home.join(crate::rollup::FILE_NAME)).unwrap();
    rebuild(&mut conn, index, ws, home, OffsetDateTime::now_utc()).unwrap();
    Rollup::open(home)
        .unwrap()
        .unwrap()
        .incidents(OffsetDateTime::now_utc() - Duration::days(1))
        .unwrap()
}

fn of<'a>(list: &'a [Incident], session: &str, kind: Kind) -> Vec<&'a Incident> {
    list.iter()
        .filter(|i| i.kind == kind && i.body.chain.session == session)
        .collect()
}

const EDIT: &str = r#"Edit {"file_path":"/w/src/parse.rs","old_string":"let a = 1;","new_string":"let a = input.unwrap();"}"#;

fn correction_session(index: &mut Index, home: &Path, id: &str) {
    put(
        index,
        home,
        id,
        &[
            (Role::User, 0, None, "fix the parser"),
            (Role::Tool, 2, Some("Edit"), EDIT),
            (Role::Tool, 2, Some("Edit"), "The file has been updated."),
            (Role::Assistant, 3, None, "Done."),
            (
                Role::User,
                10,
                None,
                "no, never use unwrap in the parser code again",
            ),
        ],
    );
}

#[test]
fn a_correction_s_cause_is_the_rule_it_had_none_or_not_established() {
    let home = tempfile::tempdir().unwrap();
    let (_w, ws) = workspace();
    let mut index = Index::open(home.path(), true).unwrap();
    for id in ["p1", "p2", "p3"] {
        correction_session(&mut index, home.path(), id);
    }
    refresh_with(home.path(), Some(&ws), &index, None).unwrap();
    inject(home.path(), "p1", &[("brain/Rules.md", "ignored")]);
    inject(home.path(), "p2", &[("brain/Questions.md", "read")]);
    let list = rebuild_now(home.path(), &index, Some(&ws));

    let p1 = of(&list, "p1", Kind::Corrected);
    assert_eq!(p1.len(), 1, "{list:#?}");
    assert_eq!(p1[0].cause, Cause::HadRule);
    let rule = p1[0].body.rule.as_ref().unwrap();
    assert_eq!(rule.source, "brain/Rules.md");
    assert!(rule.text.contains("Never use unwrap"));
    assert_eq!(rule.used.as_deref(), Some("never opened it"));
    // No call of the request is named, so the subject is the agent's
    // message just before, the step is the correction itself and there is
    // no cost.
    assert_eq!(p1[0].minutes, None);
    assert_eq!(p1[0].body.did.ordinal, 4);
    let about = p1[0].body.subject.as_ref().unwrap();
    assert_eq!(about.kind, subject::SubjectKind::Said);
    assert_eq!(about.said.as_ref().unwrap().text, "Done.");
    assert!(p1[0].body.showed.text.contains("never use unwrap"));
    assert_eq!(p1[0].body.told.as_ref().unwrap().text, "fix the parser");

    assert_eq!(of(&list, "p2", Kind::Corrected)[0].cause, Cause::NoRule);
    assert_eq!(
        of(&list, "p3", Kind::Corrected)[0].cause,
        Cause::NotEstablished
    );
}

#[test]
fn a_hook_the_safety_check_and_the_person_block_a_call_and_the_pacing_nudge_does_not() {
    let home = tempfile::tempdir().unwrap();
    let mut index = Index::open(home.path(), true).unwrap();
    let call = r#"Bash {"command":"ls"}"#;
    put(
        &mut index,
        home.path(),
        "k1",
        &[
            (Role::User, 0, None, "look around"),
            (Role::Tool, 1, Some("mcp__x__delegate"), r#"mcp__x__delegate {"force":true}"#),
            (Role::Tool, 1, Some("mcp__x__delegate"), "PreToolUse:mcp__x__delegate hook error: [/home/x/.local/bin/block-force-push]: Blocked: force pushes are not allowed"),
            (Role::Tool, 2, Some("Bash"), call),
            (Role::Tool, 2, Some("Bash"), "The user doesn't want to proceed with this tool use. The tool use was rejected."),
            (Role::Tool, 3, Some("Bash"), call),
            (Role::Tool, 3, Some("Bash"), "<tool_use_error>Blocked: sleep 60 followed by: ls. To wait for a condition, use Monitor.</tool_use_error>"),
            (Role::Tool, 4, Some("Bash"), r#"Bash {"command":"rm -rf src"}"#),
            (Role::Tool, 4, Some("Bash"), "Claude Code's removal safety check denied this command; the user did not reject it."),
        ],
    );
    refresh_with(home.path(), None, &index, None).unwrap();
    let list = Rollup::open(home.path())
        .unwrap()
        .unwrap()
        .incidents(OffsetDateTime::now_utc() - Duration::days(1))
        .unwrap();
    let blocked = of(&list, "k1", Kind::Blocked);
    assert_eq!(blocked.len(), 3, "{blocked:#?}");
    let hook = blocked.iter().find(|i| i.body.did.ordinal == 1).unwrap();
    assert_eq!(hook.cause, Cause::Hook);
    assert_eq!(hook.body.detail.as_deref(), Some("block-force-push"));
    assert!(hook
        .body
        .showed
        .text
        .starts_with("Blocked: force pushes are"));
    assert!(hook.body.did.text.contains("delegate"));
    let person = blocked.iter().find(|i| i.body.did.ordinal == 3).unwrap();
    assert_eq!(person.cause, Cause::Person);
    let check = blocked.iter().find(|i| i.body.did.ordinal == 7).unwrap();
    assert_eq!(check.cause, Cause::Hook);
}

#[test]
fn a_destructive_command_is_quiet_unless_harm_follows_and_says_when_it_was_asked_for() {
    let home = tempfile::tempdir().unwrap();
    let (_w, ws) = workspace();
    let mut index = Index::open(home.path(), true).unwrap();
    put(
        &mut index,
        home.path(),
        "r1",
        &[
            (
                Role::User,
                0,
                None,
                "please run git reset --hard origin/main, I want a clean tree",
            ),
            (
                Role::Tool,
                1,
                Some("Bash"),
                r#"Bash {"command":"cd /w && git reset --hard origin/main"}"#,
            ),
        ],
    );
    put(
        &mut index,
        home.path(),
        "r2",
        &[
            (Role::User, 0, None, "tidy the repository"),
            (
                Role::Tool,
                1,
                Some("Bash"),
                r#"Bash {"command":"rm -rf src/old"}"#,
            ),
            (
                Role::Tool,
                2,
                Some("Bash"),
                r#"Bash {"command":"rm -rf /tmp/x target node_modules"}"#,
            ),
            (
                Role::Tool,
                3,
                Some("Bash"),
                r#"Bash {"command":"python3 - <<'EOF'\nprint('git reset --hard')\nEOF"}"#,
            ),
        ],
    );
    put(
        &mut index,
        home.path(),
        "r4",
        &[
            (Role::User, 0, None, "ship it"),
            (
                Role::Tool,
                1,
                Some("Bash"),
                r#"Bash {"command":"git push --force origin main"}"#,
            ),
            (Role::Tool, 1, Some("Bash"), "ok"),
            (Role::Assistant, 2, None, "Pushed."),
            (
                Role::User,
                20,
                None,
                "no, why did you force push to main again",
            ),
        ],
    );
    refresh_with(home.path(), Some(&ws), &index, None).unwrap();
    inject(home.path(), "r4", &[("brain/Questions.md", "read")]);
    let list = rebuild_now(home.path(), &index, Some(&ws));

    let r1 = of(&list, "r1", Kind::Destructive);
    assert_eq!(r1.len(), 1);
    assert!(r1[0].quiet);
    assert_eq!(r1[0].cause, Cause::Brief);
    assert_eq!(r1[0].body.told.as_ref().unwrap().whose, "person");

    let r2 = of(&list, "r2", Kind::Destructive);
    assert_eq!(
        r2.len(),
        1,
        "the temp folder and the heredoc are not: {r2:#?}"
    );
    assert!(r2[0].quiet);
    assert_eq!(r2[0].body.detail.as_deref(), Some("rm -rf"));

    let r4 = of(&list, "r4", Kind::Destructive);
    assert_eq!(r4.len(), 1);
    assert!(!r4[0].quiet);
    assert!(r4[0].body.showed.text.contains("force push"));
    assert_eq!(r4[0].minutes, Some(19));
    assert_eq!(r4[0].cause, Cause::NotEstablished);
}

fn background(id: &str, again: bool) -> Vec<(Role, i64, Option<&'static str>, String)> {
    let mut rows = vec![
        (Role::User, 0, None, "run the tests".to_owned()),
        (
            Role::Tool,
            1,
            Some("Bash"),
            r#"Bash {"command":"cargo test --workspace","description":"Run the tests","run_in_background":true}"#.to_owned(),
        ),
        (
            Role::Tool,
            1,
            Some("Bash"),
            format!("Command running in background with ID: {id}"),
        ),
        (
            Role::User,
            9,
            None,
            format!("<task-notification>\n<task-id>{id}</task-id>\n<status>failed</status>\n<summary>Background command \"Run the tests\" failed with exit code 101</summary>\n</task-notification>"),
        ),
    ];
    if again {
        rows.push((
            Role::Tool,
            12,
            Some("Bash"),
            r#"Bash {"command":"cargo test --workspace"}"#.to_owned(),
        ));
    }
    rows
}

#[test]
fn a_failed_task_is_an_incident_only_when_nothing_redid_it() {
    let home = tempfile::tempdir().unwrap();
    let mut index = Index::open(home.path(), true).unwrap();
    for (id, again) in [("f1", true), ("f2", false)] {
        let rows = background("b1", again);
        let rows: Vec<(Role, i64, Option<&str>, &str)> = rows
            .iter()
            .map(|(r, m, t, x)| (*r, *m, *t, x.as_str()))
            .collect();
        put(&mut index, home.path(), id, &rows);
    }
    refresh_with(home.path(), None, &index, None).unwrap();
    let list = Rollup::open(home.path())
        .unwrap()
        .unwrap()
        .incidents(OffsetDateTime::now_utc() - Duration::days(1))
        .unwrap();
    assert!(of(&list, "f1", Kind::Failed).is_empty(), "{list:#?}");
    let f2 = of(&list, "f2", Kind::Failed);
    assert_eq!(f2.len(), 1);
    assert!(f2[0].body.showed.text.contains("Command failed · exit 101"));
    assert_eq!(f2[0].cause, Cause::NotEstablished);
}

/// An edit that adds `n` distinct lines from `from` to `/w/src/<file>`.
fn adds(file: &str, from: usize, n: usize) -> String {
    let new: Vec<String> = (from..from + n)
        .map(|i| format!("let value_{i:03} = compute({i});"))
        .collect();
    serde_json::json!({"file_path": format!("/w/src/{file}"), "old_string": "// here", "new_string": new.join("\n")}).to_string()
}

/// An edit that removes `n` of those lines.
fn removes(file: &str, from: usize, n: usize) -> String {
    let old: Vec<String> = (from..from + n)
        .map(|i| format!("let value_{i:03} = compute({i});"))
        .collect();
    serde_json::json!({"file_path": format!("/w/src/{file}"), "old_string": old.join("\n"), "new_string": "// gone"}).to_string()
}

#[test]
fn lines_another_session_removed_are_redone_above_the_floor_only() {
    let home = tempfile::tempdir().unwrap();
    let mut index = Index::open(home.path(), true).unwrap();
    let add = format!("Edit {}", adds("a.rs", 0, 30));
    let add_b = format!("Edit {}", adds("b.rs", 0, 30));
    put(
        &mut index,
        home.path(),
        "a1",
        &[
            (Role::User, 0, None, "write the module"),
            (Role::Tool, 1, Some("Edit"), &add),
            (Role::Tool, 2, Some("Edit"), &add_b),
            (Role::Assistant, 3, None, "Written."),
        ],
    );
    let rm = format!("Edit {}", removes("a.rs", 0, 22));
    let rm_b = format!("Edit {}", removes("b.rs", 0, 3));
    put(
        &mut index,
        home.path(),
        "b1",
        &[
            (Role::User, 60, None, "rewrite the module"),
            (Role::Tool, 61, Some("Edit"), &rm),
            (Role::Tool, 62, Some("Edit"), &rm_b),
        ],
    );
    // The first session's own sub-agent removing its lines is no redo.
    let rm_own = format!("Edit {}", removes("a.rs", 0, 25));
    put(
        &mut index,
        home.path(),
        "a1-agent-x1",
        &[
            (Role::User, 5, None, "trim it"),
            (Role::Tool, 6, Some("Edit"), &rm_own),
        ],
    );
    refresh_with(home.path(), None, &index, None).unwrap();
    let list = Rollup::open(home.path())
        .unwrap()
        .unwrap()
        .incidents(OffsetDateTime::now_utc() - Duration::days(1))
        .unwrap();
    let redone: Vec<&Incident> = list.iter().filter(|i| i.kind == Kind::Redone).collect();
    assert_eq!(redone.len(), 1, "{redone:#?}");
    let r = redone[0];
    assert_eq!(r.body.chain.session, "a1");
    assert_eq!(r.body.other.as_ref().unwrap().session, "b1");
    assert_eq!(r.lines, Some(22));
    assert_eq!(r.minutes, Some(60));
    assert_eq!(r.cause, Cause::NotEstablished);
    assert!(r
        .body
        .showed
        .text
        .starts_with("22 of the 30 lines it added to a.rs"));
    assert_eq!(r.body.fixed.as_ref().unwrap().ordinal, 1);

    // Stable across rebuilds.
    let again = rebuild_now(home.path(), &index, None);
    assert!(again.iter().any(|i| i.id == r.id));
}

#[test]
fn two_live_sessions_on_one_file_collide_unless_one_delegated_the_other() {
    let home = tempfile::tempdir().unwrap();
    let mut index = Index::open(home.path(), true).unwrap();
    let e = |s: &str| {
        format!(r#"Edit {{"file_path":"/w/src/c.rs","old_string":"x","new_string":"{s}"}}"#)
    };
    let (e1, e2, e3) = (
        e("first change here"),
        e("second change here"),
        e("third change here"),
    );
    put(
        &mut index,
        home.path(),
        "c1",
        &[
            (Role::User, 0, None, "change c"),
            (Role::Tool, 10, Some("Edit"), &e1),
            (Role::Assistant, 40, None, "Done."),
        ],
    );
    put(
        &mut index,
        home.path(),
        "c2",
        &[
            (Role::User, 5, None, "also change c"),
            (Role::Tool, 20, Some("Edit"), &e2),
            (Role::Assistant, 45, None, "Done."),
        ],
    );
    put(
        &mut index,
        home.path(),
        "d1",
        &[
            (Role::User, 0, None, "split it"),
            (
                Role::Tool,
                1,
                Some("mcp__t3-code__delegate_task"),
                r#"mcp__t3-code__delegate_task {"title":"Lane","task":"Change c too."}"#,
            ),
            (
                Role::Tool,
                1,
                Some("mcp__t3-code__delegate_task"),
                r#"{"taskId":"node:t-d","childThreadId":"thread:d2","status":"running"}"#,
            ),
            (Role::Tool, 25, Some("Edit"), &e3),
            (Role::Assistant, 50, None, "Done."),
        ],
    );
    put(
        &mut index,
        home.path(),
        "d2",
        &[
            (Role::User, 2, None, "Change c too."),
            (Role::Tool, 26, Some("Edit"), &e1),
            (Role::Assistant, 50, None, "Done."),
        ],
    );
    refresh_with(home.path(), None, &index, None).unwrap();
    let conn = Connection::open(home.path().join(crate::rollup::FILE_NAME)).unwrap();
    conn.execute("UPDATE tasks SET child = 'd2' WHERE session_id = 'd1'", [])
        .unwrap();
    let list = rebuild_now(home.path(), &index, None);
    let collisions: Vec<&Incident> = list.iter().filter(|i| i.kind == Kind::Collision).collect();
    // c1 with c2; and c1 or c2 with d1 and d2, which are live then too, but
    // never d1 with d2.
    assert!(collisions.iter().any(|i| {
        i.body.chain.session == "c1" && i.body.other.as_ref().unwrap().session == "c2"
    }));
    assert!(!collisions.iter().any(|i| {
        let d = |s: &str| s == "d1" || s == "d2";
        d(&i.body.chain.session) && d(&i.body.other.as_ref().unwrap().session)
    }));
    assert!(collisions.iter().all(|i| i.cause == Cause::Collided));
}

#[test]
fn a_hook_is_written_for_each_destructive_pattern_and_blocks_it() {
    for p in [
        "rm -rf",
        "git reset --hard",
        "force push",
        "git clean -f",
        "git checkout .",
        "git restore .",
        "SQL drop",
    ] {
        let h = hook_for(p).unwrap();
        assert!(h.script.starts_with("#!/bin/sh\n"));
        assert!(h.script.contains("exit 2"));
        assert!(h.settings.contains(&h.file));
        assert!(
            serde_json::from_str::<serde_json::Value>(&h.settings).is_ok(),
            "{}",
            h.settings
        );
    }
    assert!(hook_for("something else").is_none());
}

/// Runs detection over a copy of a home, read-only on the workspace:
/// `HIPPOCAMP_TRIAL_HOME=<copy> HIPPOCAMP_TRIAL_CONFIG=<the home whose config.toml
/// names the workspace> cargo test -p hippocamp-core trial -- --ignored
/// --nocapture`. Prints counts only.
#[test]
#[ignore]
fn trial() {
    let (Ok(home), Ok(config)) = (
        std::env::var("HIPPOCAMP_TRIAL_HOME"),
        std::env::var("HIPPOCAMP_TRIAL_CONFIG"),
    ) else {
        return;
    };
    let home = Path::new(&home);
    let config = crate::config::Config::load_in(Path::new(&config)).unwrap();
    let cfg = Workspace::load(&config).unwrap();
    let index = Index::open(home, false).unwrap();
    let started = std::time::Instant::now();
    refresh_with(
        home,
        Some(&cfg),
        &index,
        crate::rollup::t3::state_path().as_deref(),
    )
    .unwrap();
    let mut conn = Connection::open(home.join(crate::rollup::FILE_NAME)).unwrap();
    let n = rebuild(
        &mut conn,
        &index,
        Some(&cfg),
        home,
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    println!(
        "refreshed and built {n} incidents in {:?}",
        started.elapsed()
    );
}

fn sql(home: &Path, statement: &str, args: &[&dyn rusqlite::ToSql]) {
    let conn = Connection::open(home.join(crate::rollup::FILE_NAME)).unwrap();
    conn.execute(statement, args).unwrap();
}

fn about(list: &[Incident], session: &str) -> subject::Subject {
    of(list, session, Kind::Corrected)
        .first()
        .and_then(|i| i.body.subject.clone())
        .unwrap_or_else(|| panic!("no correction in {session}: {list:#?}"))
}

#[test]
fn a_correction_naming_a_call_of_its_request_is_about_that_step() {
    let home = tempfile::tempdir().unwrap();
    let mut index = Index::open(home.path(), true).unwrap();
    put(
        &mut index,
        home.path(),
        "s1",
        &[
            (Role::User, 0, None, "get the preview running"),
            (
                Role::Tool,
                1,
                Some("Bash"),
                r#"Bash {"command":"runner new --keep","description":"Start the runner again"}"#,
            ),
            (Role::Tool, 6, Some("Bash"), "started job_1"),
            (Role::Assistant, 8, None, "The preview is up."),
            (
                Role::User,
                30,
                None,
                "no, stop starting the runner again and again, reuse it",
            ),
        ],
    );
    refresh_with(home.path(), None, &index, None).unwrap();
    let list = rebuild_now(home.path(), &index, None);
    let a = about(&list, "s1");
    assert_eq!(a.kind, subject::SubjectKind::Step, "{a:#?}");
    assert_eq!(a.step.as_ref().unwrap().ordinal, 1);
    assert_eq!(a.matched, ["runner"]);
    // Working time: the call ran 5 minutes, then 2 until the reply; the
    // 22 minutes until the correction are the person's.
    assert_eq!(a.minutes, Some(7));
    let i = &of(&list, "s1", Kind::Corrected)[0];
    assert_eq!(i.minutes, Some(7));
    assert_eq!(i.body.did.ordinal, 1);
}

#[test]
fn a_correction_naming_earlier_work_lists_it_as_candidates() {
    let home = tempfile::tempdir().unwrap();
    let mut index = Index::open(home.path(), true).unwrap();
    // Its own session changed a file under the named folder in an earlier
    // request.
    put(
        &mut index,
        home.path(),
        "own",
        &[
            (Role::User, 0, None, "make the invoice screen"),
            (
                Role::Tool,
                1,
                Some("Edit"),
                r#"Edit {"file_path":"/w/src/billing/Invoice.tsx","old_string":"a","new_string":"b"}"#,
            ),
            (Role::Assistant, 2, None, "Made it."),
            (Role::User, 30, None, "now tidy the readme"),
            (
                Role::Tool,
                31,
                Some("Bash"),
                r#"Bash {"command":"wc -l README.md","description":"Count readme lines"}"#,
            ),
            (Role::Assistant, 32, None, "Tidied."),
            (
                Role::User,
                40,
                None,
                "no, why is billing copied like that again",
            ),
        ],
    );
    // Another session of the project named two of the words; another only
    // one; one more named both but more than 7 days ago.
    for (id, title) in [
        ("other", "Add the authenticator step"),
        ("lone", "Request headers"),
        ("old", "Add the authenticator step"),
    ] {
        put(
            &mut index,
            home.path(),
            id,
            &[
                (Role::User, 5, None, "go"),
                (Role::Assistant, 6, None, "ok"),
            ],
        );
        let _ = title;
    }
    put(
        &mut index,
        home.path(),
        "auth",
        &[
            (Role::User, 0, None, "check sign-in"),
            (Role::Assistant, 9, None, "Signed in."),
            (
                Role::User,
                10,
                None,
                "wrong, the authenticator step is shown again",
            ),
        ],
    );
    refresh_with(home.path(), None, &index, None).unwrap();
    let h = home.path();
    sql(h, "UPDATE sessions SET project = 'app'", &[]);
    sql(h, "DELETE FROM edits WHERE session_id = 'own'", &[]);
    sql(
        h,
        "INSERT INTO edits(session_id, ord, ordinal, ts, path, file, added, removed) VALUES ('own', 0, 1, ?1, '/w/src/billing/Invoice.tsx', 'app:src/billing/Invoice.tsx', '', '')",
        &[&t(1)],
    );
    for (id, title) in [
        ("other", "Add the authenticator step"),
        ("lone", "Request headers"),
        ("old", "Add the authenticator step"),
    ] {
        sql(
            h,
            "UPDATE sessions SET title = ?1 WHERE id = ?2",
            &[&title, &id],
        );
    }
    let old = t(-8 * 24 * 60);
    sql(
        h,
        "UPDATE sessions SET first_turn = ?1, last_turn = ?1 WHERE id = 'old'",
        &[&old],
    );
    sql(
        h,
        "INSERT INTO pulls(session_id, url, done, ts, title) VALUES ('other', 'https://github.com/acme/app/pull/3', 'opened', ?1, 'Sign-in: an authenticator step')",
        &[&t(6)],
    );
    let list = rebuild_now(h, &index, None);

    let a = about(&list, "own");
    assert_eq!(a.kind, subject::SubjectKind::Earlier, "{a:#?}");
    assert_eq!(a.candidates.len(), 1, "{a:#?}");
    let c = &a.candidates[0];
    assert_eq!(c.session, "own");
    assert_eq!(c.relation, subject::Relation::Own);
    assert_eq!(c.words, ["billing"]);
    assert_eq!(c.under.as_deref(), Some("src/billing"));
    assert_eq!(c.files, 1);
    assert_eq!(c.ordinal, Some(1));
    // Not the agent's message, and no cost.
    assert!(a.said.is_none());
    assert_eq!(of(&list, "own", Kind::Corrected)[0].minutes, None);

    let b = about(&list, "auth");
    assert_eq!(b.kind, subject::SubjectKind::Earlier, "{b:#?}");
    let ids: Vec<&str> = b.candidates.iter().map(|c| c.session.as_str()).collect();
    assert_eq!(ids, ["other"], "{b:#?}");
    assert_eq!(b.candidates[0].relation, subject::Relation::Other);
    assert_eq!(
        b.candidates[0].pull.as_ref().unwrap().url,
        "https://github.com/acme/app/pull/3"
    );
}

#[test]
fn a_correction_long_after_the_message_is_not_established() {
    let home = tempfile::tempdir().unwrap();
    let mut index = Index::open(home.path(), true).unwrap();
    put(
        &mut index,
        home.path(),
        "late",
        &[
            (Role::User, -10, None, "look at the cache"),
            (Role::Assistant, 0, None, "The cache is empty."),
            (
                Role::User,
                175,
                None,
                "no, the cache is never empty, check that again",
            ),
        ],
    );
    refresh_with(home.path(), None, &index, None).unwrap();
    let list = rebuild_now(home.path(), &index, None);
    let a = about(&list, "late");
    assert_eq!(a.kind, subject::SubjectKind::None, "{a:#?}");
    assert!(a.said.is_none() && a.candidates.is_empty() && a.step.is_none());
    // The subject survives a rebuild under the same id.
    let again = rebuild_now(home.path(), &index, None);
    assert_eq!(
        of(&again, "late", Kind::Corrected)[0].id,
        of(&list, "late", Kind::Corrected)[0].id
    );
    assert_eq!(about(&again, "late"), a);
}

#[test]
fn a_check_relayed_from_a_delegated_session_that_ran_it_is_not_an_incident() {
    // The parent ran no test; the session it delegated ran
    // them after its last edit, as its own transcript shows. A parent that
    // then edits code still flags.
    for (parent_edits, flagged) in [(false, false), (true, true)] {
        let home = tempfile::tempdir().unwrap();
        let mut index = Index::open(home.path(), true).unwrap();
        let edit = r#"Edit {"file_path":"/w/src/lib.rs","old_string":"let depth = 0;","new_string":"let depth = input.len();"}"#;
        let mut parent: Vec<(Role, i64, Option<&str>, &str)> = vec![
            (Role::User, 0, None, "fix the parser and test it"),
            (
                Role::Tool,
                1,
                Some("mcp__t3-code__delegate_task"),
                r#"mcp__t3-code__delegate_task {"title":"Parser","task":"Fix the parser and run the tests."}"#,
            ),
            (
                Role::Tool,
                1,
                Some("mcp__t3-code__delegate_task"),
                r#"{"taskId":"node:t-p","childThreadId":"thread:p2","status":"running"}"#,
            ),
        ];
        if parent_edits {
            parent.push((Role::Tool, 30, Some("Edit"), edit));
            parent.push((Role::Tool, 30, Some("Edit"), "The file has been updated."));
        }
        parent.push((Role::Assistant, 40, None, "All tests pass."));
        put(&mut index, home.path(), "p1", &parent);
        put(
            &mut index,
            home.path(),
            "p2",
            &[
                (Role::User, 2, None, "Fix the parser and run the tests."),
                (Role::Tool, 10, Some("Edit"), edit),
                (Role::Tool, 10, Some("Edit"), "The file has been updated."),
                (
                    Role::Tool,
                    20,
                    Some("Bash"),
                    r#"Bash {"command":"cd /w && cargo test"}"#,
                ),
                (
                    Role::Tool,
                    21,
                    Some("Bash"),
                    "test result: ok. 12 passed; 0 failed",
                ),
                // The delegate claims nothing itself: its transcript shows
                // the run.
                (Role::Assistant, 22, None, "Done; the parser is fixed."),
            ],
        );
        refresh_with(home.path(), None, &index, None).unwrap();
        let conn = Connection::open(home.path().join(crate::rollup::FILE_NAME)).unwrap();
        conn.execute("UPDATE tasks SET child = 'p2' WHERE session_id = 'p1'", [])
            .unwrap();
        let list = rebuild_now(home.path(), &index, None);
        let claimed = list
            .iter()
            .any(|i| i.kind == Kind::Unverified && i.body.chain.session == "p1");
        assert_eq!(claimed, flagged, "parent edits: {parent_edits}: {list:#?}");
    }
}

#[test]
fn news_is_told_by_its_subject() {
    for news in [
        "users dont see the export button yet, wtf",
        "customers cant pay by card, again",
        "we dont have a staging server yet, damn",
    ] {
        assert!(!objects(news), "{news}");
    }
    for objection in [
        "the tests dont pass again, wtf",
        "wrong file again",
        "NO FORCE PUSH TO MAIN",
    ] {
        assert!(objects(objection), "{objection}");
    }
}
