//! Lessons tests, on synthetic text and sessions only.

use std::fs;
use std::path::Path;

use serde_json::json;
use time::{Duration, OffsetDateTime};

use super::detect::{self, is_correction, key_terms, person_text, signature, t3_quote};
use super::*;
use crate::adapters::{RawTurn, Role};
use crate::dream::tests::Setup;
use crate::index::{SessionSource, Turn};
use crate::records::Tool;

#[test]
fn corrective_markers() {
    for yes in [
        "no, use tabs",
        "never run the full suite here",
        "dont use emojis",
        "why are the panels at the bottom?",
        "how many times did the build fail",
        "I asked for the sidebar",
        "use tabs not spaces",
        "WHY IS THE BUILD STILL BROKEN?",
    ] {
        assert!(is_correction(yes), "{yes}");
    }
    for no in [
        "continue please",
        "looks good, ship it",
        "GO AHEAD WITH THE EXPORT PAGE",
        "add a dark theme",
    ] {
        assert!(!is_correction(no), "{no}");
    }
}

#[test]
fn a_persons_text_leaves_out_injected_and_relayed_turns() {
    assert_eq!(
        person_text("no spaces. tabs").as_deref(),
        Some("no spaces. tabs")
    );
    assert_eq!(person_text("<system-reminder>no</system-reminder>"), None);
    assert_eq!(person_text("Stop hook feedback: no, not done"), None);
    assert_eq!(person_text("The manager said: never use spaces"), None);
    // Any role reported in the third person, not one project's titles.
    assert_eq!(
        person_text("The lead decided: never deploy on fridays"),
        None
    );
    assert_eq!(person_text("Ask the user before you push"), None);
    assert!(person_text("never use spaces, use tabs").is_some());
    assert_eq!(person_text("Please **don't** push to `main` again"), None);
    assert_eq!(person_text(&"no ".repeat(200)), None);
    assert_eq!(
        person_text("dont use spaces\n[Attached image \"shot.png\" is saved at: /x/y.png]")
            .as_deref(),
        Some("dont use spaces")
    );
}

#[test]
fn a_t3_quote_is_the_persons_comment_on_the_agents_words() {
    let raw = "[Assistant quote](t3-citation://v1/a/b?text=I+will+use+spaces+for+the+indents.&start=1&comment=no+spaces%2C+tabs)";
    let (person, quoted) = t3_quote(raw).unwrap();
    assert_eq!(person, "no spaces, tabs");
    assert_eq!(quoted, "I will use spaces for the indents.");
    assert_eq!(person_text(raw).as_deref(), Some("no spaces, tabs"));
}

#[test]
fn key_terms_drop_stop_and_frustration_words_and_cut_to_six_letters() {
    assert_eq!(
        key_terms("wtf dude, dont use spaces for indentation again"),
        ["spaces", "indent"]
    );
}

#[test]
fn signatures_name_the_error_and_normalise_it() {
    assert_eq!(signature("Exit code 0\nall fine"), None);
    assert_eq!(signature("Exit code 1\n"), None, "grep finding nothing");
    assert_eq!(
        signature("Exit code 1\n(eval):3: no matches found: --include=*.tsx").as_deref(),
        Some("(eval):N: no matches found: --include=*.<ext>")
    );
    assert_eq!(
        signature("Exit code 2\nls: /Users/x/proj/a.txt: No such file or directory").as_deref(),
        Some("ls: <path>: No such file or directory")
    );
    assert_eq!(
        signature("<tool_use_error>Error: No such tool available: bash</tool_use_error>")
            .as_deref(),
        Some("Error: No such tool available: bash")
    );
    assert_eq!(
        signature("Exit code 1\n(eval):1: ==== not found").as_deref(),
        Some("(eval):N: == not found")
    );
}

#[test]
fn variants_of_one_error_are_one_failure() {
    assert!(detect::same_error(
        "Blocked: sleep N followed by: tail -N <path> To wait for a condition, use Monitor",
        "Blocked: sleep N followed by: cat <path> grep -E <q> head -N. To wait for a condition"
    ));
    // Both are zsh's unmatched glob.
    assert!(detect::same_error(
        "(eval):N: no matches found: --include=*.<ext>",
        "(eval):N: no matches found: <path>*"
    ));
    assert!(!detect::same_error(
        "ls: <path>: No such file or directory",
        "(eval):N: no matches found: <path>*"
    ));
}

#[test]
fn a_rule_goes_after_its_sections_last_line_or_at_the_end() {
    let pad = "Agents read this note at the start of every session.\n".repeat(4);
    let note = format!("# Agent rules\n\n{pad}\n## Tools\n\n- Use rg.\n\n## Delegation\n\n- One build at a time.\n");
    let note = note.as_str();
    let bullet = bullet(
        "Use tabs",
        "Use tabs for indentation.",
        time::Date::from_calendar_date(2026, time::Month::November, 10).unwrap(),
    );
    assert_eq!(
        bullet,
        "- **Use tabs.** Use tabs for indentation. *(lesson kept 2026-11-10)*\n"
    );
    let (kind, old, new) = insertion(note, Some("Tools"), &bullet);
    assert_eq!(kind, ProposalKind::Replace);
    assert_eq!(old, "- Use rg.\n");
    assert_eq!(new, format!("- Use rg.\n{bullet}"));
    let after = notes::change(note.as_bytes(), kind, old.as_bytes(), new.as_bytes()).unwrap();
    assert!(String::from_utf8(after)
        .unwrap()
        .contains("- Use rg.\n- **Use tabs.**"));
    // The last section, or no section: an append.
    let (kind, old, new) = insertion(note, Some("Delegation"), &bullet);
    assert_eq!(
        (kind, old.as_str(), new.as_str()),
        (ProposalKind::Append, "", bullet.as_str())
    );
    let (kind, _, _) = insertion(note, Some("Missing"), &bullet);
    assert_eq!(kind, ProposalKind::Append);
}

// ---------------------------------------------------------------------------
// End to end, with stub model and reviewer

fn add_turns(s: &Setup, id: &str, turns: &[(Role, Option<&str>, &str)], start: OffsetDateTime) {
    let turns: Vec<Turn> = turns
        .iter()
        .enumerate()
        .map(|(k, (role, tool, text))| {
            Turn::from_raw(RawTurn {
                ordinal: k as u32,
                role: *role,
                ts: start + Duration::minutes(k as i64),
                tool_name: tool.map(str::to_owned),
                text: (*text).to_owned(),
            })
        })
        .collect();
    let transcript = format!("/t/{id}.jsonl");
    s.index()
        .replace_session(
            &SessionSource {
                id,
                tool: Tool::ClaudeCode,
                transcript_path: Path::new(&transcript),
                file_hash: id,
            },
            &turns,
        )
        .unwrap();
}

fn says(s: &Setup, who: &str, text: &str) {
    let reply = json!({"result": text}).to_string();
    fs::write(s.stubs.path().join(format!("{who}.json")), reply).unwrap();
}

/// Two sessions where the person corrects spaces to tabs, three where
/// the same zsh glob error repeats, and a notes file with a Tools section.
fn scene() -> Setup {
    let s = Setup::new();
    s.add_note(
        "notes/Agent rules.md",
        &format!(
            "# Agent rules\n\n{}\n## Indentation and tools\n\n- Use rg for search.\n\n## Delegation\n\n- One build at a time.\n",
            "Agents read this note at the start of every session.\n".repeat(6)
        ),
    );
    let now = OffsetDateTime::now_utc() - Duration::hours(3);
    let shot = r#"Bash {"command":"npx prettier --use-spaces a.html"}"#;
    add_turns(
        &s,
        "c1",
        &[
            (Role::User, None, "format the page source"),
            (Role::Tool, Some("Bash"), shot),
            (Role::Tool, Some("Bash"), "saved a.html"),
            (
                Role::Assistant,
                None,
                "Done, the page is formatted in a.html.",
            ),
            (Role::User, None, "no spaces for indentation. tabs only"),
        ],
        now,
    );
    add_turns(
        &s,
        "c2",
        &[
            (Role::User, None, "check the page"),
            (Role::Tool, Some("Bash"), shot),
            (
                Role::User,
                None,
                "dont use spaces for indentation again, tabs only",
            ),
        ],
        now + Duration::minutes(30),
    );
    for k in 0..3 {
        add_turns(
            &s,
            &format!("f{k}"),
            &[
                (Role::User, None, "find the hooks"),
                (
                    Role::Tool,
                    Some("Bash"),
                    r#"Bash {"command":"grep -rn useFoo --include=*.ts src"}"#,
                ),
                (
                    Role::Tool,
                    Some("Bash"),
                    "Exit code 1\n(eval):1: no matches found: --include=*.ts",
                ),
            ],
            now + Duration::minutes(60 + k),
        );
    }
    let index = s.index();
    crate::rollup::refresh(&s.cfg.home, Some(&s.ws), &index).unwrap();
    s
}

#[test]
fn detection_finds_the_correction_once_and_the_repeated_failure() {
    let s = scene();
    let index = s.index();
    let rollup = Rollup::open(&s.cfg.home).unwrap().unwrap();
    let since = OffsetDateTime::now_utc() - Duration::days(1);
    let found = detect::find(&index, &rollup, since, time::UtcOffset::UTC).unwrap();
    assert_eq!(found.corrections.len(), 1, "{:#?}", found.corrections);
    let c = &found.corrections[0];
    assert_eq!(c.sessions.len(), 2);
    assert_eq!(
        c.evidence[0].quote,
        "dont use spaces for indentation again, tabs only"
    );
    assert_eq!(
        c.evidence[1].action.as_deref(),
        Some("Bash: npx prettier --use-spaces a.html")
    );
    assert_eq!(found.failures.len(), 1);
    assert_eq!(found.failures[0].sessions.len(), 3);
    assert_eq!(
        found.failures[0].key,
        "(eval):N: no matches found: --include=*.<ext>"
    );
}

#[test]
fn scan_keep_revert_and_dismiss() {
    let s = scene();
    says(
        &s,
        "dreamer",
        &json!([{"n": 1, "lesson": true, "title": "Use tabs for indentation", "rule": "Indent with tabs, never spaces.", "note": "notes/Agent rules.md", "section": "Indentation and tools"}]).to_string(),
    );
    says(
        &s,
        "reviewer",
        &json!({"verdict": "confirm", "notes": "repeated twice"}).to_string(),
    );
    let scanned = scan(&s.cfg, &s.ws, 7, false).unwrap();
    assert_eq!(scanned.state.candidates, 2);
    assert_eq!(scanned.state.confirmed, 2, "{:?}", scanned.notes);
    // One drafting call per kind, one review per draft.
    assert_eq!(scanned.state.model_calls, 4);
    let reviewer_prompt = fs::read_to_string(s.stubs.path().join("reviewer.stdin")).unwrap();
    assert!(reviewer_prompt.contains("<<<UNTRUSTED evidence>>>"));
    let waiting = pending(&s.cfg.home).unwrap();
    assert_eq!(waiting.len(), 2);
    let correction = waiting
        .iter()
        .find(|l| l.kind == Kind::Correction)
        .unwrap()
        .clone();
    let failure = waiting
        .iter()
        .find(|l| l.kind == Kind::Failure)
        .unwrap()
        .clone();
    assert_eq!(
        correction.target.as_ref().unwrap().section.as_deref(),
        Some("Indentation and tools")
    );

    // Nothing was written to the note before Keep.
    let note_path = s.ws.root.join("notes/Agent rules.md");
    let before = fs::read_to_string(&note_path).unwrap();
    assert!(!before.contains("tabs"));

    // Keep, edited.
    let now = OffsetDateTime::now_utc();
    let kept = keep(
        &s.cfg,
        &s.ws,
        correction.id,
        None,
        Some("Use tabs for every indent level."),
        now,
    )
    .unwrap();
    assert_eq!(kept.receipt.reviewer, "human");
    assert!(kept.receipt.notes.contains("edited"));
    let after = fs::read_to_string(&note_path).unwrap();
    assert!(after.contains(
        "- Use rg for search.\n- **Use tabs for indentation.** Use tabs for every indent level."
    ));
    assert!(after.find("tabs").unwrap() < after.find("## Delegation").unwrap());
    assert_eq!(
        get(&s.cfg.home, correction.id).unwrap().status,
        Status::Kept
    );
    assert!(
        keep(&s.cfg, &s.ws, correction.id, None, None, now).is_err(),
        "kept twice"
    );

    // Revert like a dreaming change.
    crate::dream::apply::revert(&s.ws, kept.receipt.id, false).unwrap();
    assert_eq!(fs::read_to_string(&note_path).unwrap(), before);

    // Dismiss; a scan with no new sessions neither redrafts nor reopens it.
    dismiss(&s.cfg, failure.id, now).unwrap();
    let again = scan(&s.cfg, &s.ws, 7, false).unwrap();
    assert_eq!(again.state.model_calls, 0);
    assert!(pending(&s.cfg.home).unwrap().is_empty());
}

#[test]
fn a_rejected_draft_never_reaches_the_inbox() {
    let s = scene();
    says(
        &s,
        "dreamer",
        &json!([{"n": 1, "lesson": true, "title": "T", "rule": "R.", "note": null, "section": null}]).to_string(),
    );
    says(
        &s,
        "reviewer",
        &json!({"verdict": "reject", "notes": "one-off"}).to_string(),
    );
    let scanned = scan(&s.cfg, &s.ws, 7, false).unwrap();
    assert_eq!(scanned.state.confirmed, 0);
    assert!(pending(&s.cfg.home).unwrap().is_empty());
    assert!(list(&s.cfg.home)
        .unwrap()
        .iter()
        .all(|l| l.status == Status::Rejected));
}

#[test]
fn a_declined_cluster_is_recorded_and_a_dry_run_writes_nothing() {
    let s = scene();
    says(
        &s,
        "dreamer",
        &json!([{"n": 1, "lesson": false}]).to_string(),
    );
    says(
        &s,
        "reviewer",
        &json!({"verdict": "confirm", "notes": ""}).to_string(),
    );
    let dry = scan(&s.cfg, &s.ws, 7, true).unwrap();
    assert_eq!(dry.state.model_calls, 2);
    assert!(list(&s.cfg.home).unwrap().is_empty());
    assert!(State::load(&s.cfg.home).is_none());
    scan(&s.cfg, &s.ws, 7, false).unwrap();
    let all = list(&s.cfg.home).unwrap();
    assert_eq!(all.len(), 2);
    assert!(all.iter().all(|l| l.status == Status::Declined));
    assert!(State::load(&s.cfg.home).unwrap().sessions >= 5);
}

#[test]
fn keeping_a_new_note_creates_it_and_revert_removes_it() {
    let s = scene();
    says(
        &s,
        "dreamer",
        &json!([{"n": 1, "lesson": true, "title": "Quote globs in zsh", "rule": "Quote --include globs in zsh.", "note": null, "section": null}]).to_string(),
    );
    says(
        &s,
        "reviewer",
        &json!({"verdict": "confirm", "notes": "ok"}).to_string(),
    );
    scan(&s.cfg, &s.ws, 7, false).unwrap();
    let lesson = pending(&s.cfg.home)
        .unwrap()
        .into_iter()
        .find(|l| l.kind == Kind::Failure)
        .unwrap();
    let target = lesson.target.clone().unwrap();
    assert!(target.new);
    assert_eq!(target.note, Path::new("notes/Quote globs in zsh.md"));
    let kept = keep(
        &s.cfg,
        &s.ws,
        lesson.id,
        None,
        None,
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    assert_eq!(kept.proposal.kind, ProposalKind::Create);
    let path = s.ws.root.join(&target.note);
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("---\ntype: policy\n"));
    assert!(text.contains("# Quote globs in zsh\n\nQuote --include globs in zsh.\n"));
    crate::dream::apply::revert(&s.ws, kept.receipt.id, false).unwrap();
    assert!(!path.exists());
}
