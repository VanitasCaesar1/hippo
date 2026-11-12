//! Tests of `hippo ask`. Every transcript and note here is synthetic.

use super::*;
use crate::adapters::Role;
use crate::index::{SessionSource, Turn};
use crate::records::Tool;
use crate::redact::redact;
use std::path::Path;

fn turn(ordinal: u32, role: Role, ts: &str, tool: Option<&str>, text: &str) -> Turn {
    Turn {
        ordinal,
        role,
        ts: OffsetDateTime::parse(ts, &Rfc3339).unwrap(),
        tool_name: tool.map(str::to_owned),
        text: redact(text),
    }
}

fn put(index: &mut Index, id: &str, path: &Path, turns: &[Turn]) {
    let source = SessionSource {
        id,
        tool: Tool::ClaudeCode,
        transcript_path: path,
        file_hash: id,
    };
    index.replace_session(&source, turns).unwrap();
}

struct Env {
    _tmp: tempfile::TempDir,
    index: Index,
}

fn env() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let mut index = Index::open(&home, true).unwrap();
    let dir = tmp
        .path()
        .join(".claude-work")
        .join("projects")
        .join("-w-app");
    std::fs::create_dir_all(&dir).unwrap();
    let p1 = dir.join("s1.jsonl");
    std::fs::write(
        &p1,
        "{\"cwd\":\"/w/app\",\"sessionId\":\"s1\",\"entrypoint\":\"cli\"}\n",
    )
    .unwrap();
    put(
        &mut index,
        "s1",
        &p1,
        &[
            turn(
                0,
                Role::User,
                "2026-11-01T10:00:00Z",
                None,
                "take screenshots of the mockups",
            ),
            turn(
                1,
                Role::Assistant,
                "2026-11-01T10:01:00Z",
                None,
                "I will use deploy with staging for the screenshots.",
            ),
            turn(
                2,
                Role::User,
                "2026-11-01T10:02:00Z",
                None,
                "no production. staging only",
            ),
        ],
    );
    let p2 = dir.join("s2.jsonl");
    std::fs::write(&p2, "").unwrap();
    put(
        &mut index,
        "s2",
        &p2,
        &[turn(
            0,
            Role::User,
            "2026-11-02T10:00:00Z",
            None,
            "set up the database screenshots folder",
        )],
    );
    // A sub-agent's hit counts for its parent.
    let p3 = dir.join("s1").join("subagents").join("agent-a.jsonl");
    put(
        &mut index,
        "s1-agent-a",
        &p3,
        &[turn(
            0,
            Role::Assistant,
            "2026-11-01T11:00:00Z",
            None,
            "Deploy staging launch worked",
        )],
    );
    Env { _tmp: tmp, index }
}

#[test]
fn question_words_drop_stop_words_and_short_words() {
    assert_eq!(
        terms("Has anyone set up Deploy Staging screenshots for mockups?"),
        vec!["deploy", "staging", "screenshots", "mockups"]
    );
    assert!(terms("is it ok?").is_empty());
}

#[test]
fn hits_group_by_session_and_rank_by_how_many_words_matched() {
    let e = env();
    let asked = search(
        &Inputs {
            index: &e.index,
            rollup: None,
            notes: None,
            limit: 6,
        },
        "has anyone set up deploy staging screenshots?",
    )
    .unwrap();
    assert_eq!(asked.searched.sessions, 3);
    let ids: Vec<_> = asked
        .sources
        .iter()
        .map(|s| s.id.clone().unwrap())
        .collect();
    assert_eq!(
        ids,
        vec!["s1", "s2"],
        "the sub-agent folds into s1; s2 matched one word"
    );
    let s1 = &asked.sources[0];
    assert_eq!(s1.n, 1);
    assert_eq!(s1.matched, vec!["deploy", "screenshots", "staging"]);
    assert_eq!(s1.hits, 4, "its own three turns and its sub-agent's");
    assert_eq!(s1.profile.as_deref(), Some("work"));
    assert!(
        s1.why.starts_with("agent: ") || s1.why.starts_with("you: "),
        "{}",
        s1.why
    );
    let resume = s1.resume.as_deref().unwrap();
    assert!(
        resume.starts_with("cd /w/app && CLAUDE_CONFIG_DIR="),
        "{resume}"
    );
    assert!(
        resume.ends_with(".claude-work claude --resume s1"),
        "{resume}"
    );
    let json = serde_json::to_value(&asked).unwrap();
    assert_eq!(json["sources"][0]["kind"], "session");
    assert!(
        json["sources"][0].get("excerpts").is_none(),
        "excerpts stay out of the JSON"
    );
}

#[test]
fn notes_are_sources_too() {
    let e = env();
    let tmp = tempfile::tempdir().unwrap();
    let notes_dir = tmp.path().join("brain");
    std::fs::create_dir_all(&notes_dir).unwrap();
    std::fs::write(
        notes_dir.join("browser.md"),
        "# Use staging not production\n\nDeploy staging headless.\n",
    )
    .unwrap();
    let notes = Notes::scan(&notes_dir).unwrap();
    let asked = search(
        &Inputs {
            index: &e.index,
            rollup: None,
            notes: Some((&notes, Path::new("brain"))),
            limit: 6,
        },
        "deploy staging",
    )
    .unwrap();
    let note = asked.sources.iter().find(|s| s.kind == "note").unwrap();
    assert_eq!(note.path.as_deref(), Some(Path::new("brain/browser.md")));
    assert_eq!(note.title, "Use staging not production");
    assert!(prompt(&asked).contains("[1]"));
}

#[test]
fn an_answer_must_cite_returned_sources() {
    assert_eq!(validate("Yes [1], twice [2][1].", 2), Ok(vec![1, 2]));
    assert_eq!(validate("See [1, 2].", 2), Ok(vec![1, 2]));
    assert!(validate("Yes [3].", 2).unwrap_err().contains("cites [3]"));
    assert!(validate("Yes.", 2).unwrap_err().contains("no source"));
    assert!(validate("[0]", 2).is_err());
}

#[test]
fn hippos_own_ask_output_is_not_a_source() {
    let mut e = env();
    let p = e._tmp.path().join("asker.jsonl");
    std::fs::write(&p, "").unwrap();
    put(
        &mut e.index,
        "asker",
        &p,
        &[
            turn(
                0,
                Role::User,
                "2026-11-03T10:00:00Z",
                None,
                "build the inbox",
            ),
            turn(
                1,
                Role::Tool,
                "2026-11-03T10:01:00Z",
                Some("Bash"),
                r#"Bash {"command":"hippo ask \"deploy staging screenshots\" --json"}"#,
            ),
            turn(
                2,
                Role::Tool,
                "2026-11-03T10:01:30Z",
                Some("Bash"),
                r#"{"question":"deploy staging screenshots","sources":[{"why":"agent: Deploy staging screenshots"}]}"#,
            ),
        ],
    );
    let asked = search(
        &Inputs {
            index: &e.index,
            rollup: None,
            notes: None,
            limit: 6,
        },
        "deploy staging screenshots",
    )
    .unwrap();
    let ids: Vec<_> = asked.sources.iter().filter_map(|s| s.id.clone()).collect();
    assert!(!ids.contains(&"asker".to_owned()), "{ids:?}");
    assert_eq!(ids[0], "s1");
}
