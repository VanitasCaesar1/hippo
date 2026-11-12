//! Tests of the Home. Every session here is synthetic, and as messy as real
//! ones: a stop carried on by a thread with a new title, a stop abandoned,
//! old stops, a failed delegated task followed by runs that worked, sign-in
//! failures, and Markdown inside quoted text.

use super::*;

#[test]
fn a_question_is_the_last_sentence_that_asks() {
    assert_eq!(
        asks("Switched the mail at 8 pm. If you meant Tuesday, tell me."),
        Some("If you meant Tuesday, tell me".to_owned())
    );
    assert_eq!(
        asks("Done. Should I also tag the release?").as_deref(),
        Some("Should I also tag the release?")
    );
    assert_eq!(asks("All 48 tests pass."), None);
}

#[test]
fn a_quiet_home_says_so() {
    let marked = HashSet::new();
    let home = build(&Inputs {
        now: time::macros::datetime!(2026-11-09 02:50 UTC),
        offset: UtcOffset::UTC,
        today: None,
        yesterday: None,
        rollup: None,
        ws: None,
        history: None,
        marked: &marked,
        notes: &[],
        runs: &[],
        status: None,
        profiles: &["default".to_owned(), "work".to_owned(), "codex".to_owned()],
        ingest_interval_minutes: 10,
        dream_at: "03:30",
    });
    assert_eq!(home.kicker, "Home · Monday 9 November · 02:50");
    assert_eq!(home.headline, "Nothing needs you. Quiet night.");
    // Nothing to do is no row at all.
    assert!(home.needs.is_empty());
    let status: String = home.status.iter().map(|s| s.text.as_str()).collect();
    assert_eq!(
        status,
        "Reading 2 Claude Code profiles and Codex every 10 minutes · no notes folder yet, so no dreaming and no briefing"
    );
    assert_eq!(home.health, "health ok");
    assert!(home.health_warnings.is_empty() && home.health_notes.is_empty());
    assert!(home.memory.is_none());
    assert_eq!(home.dock_line, "nothing needs you");
}

use hippocamp_core::adapters::Role;
use hippocamp_core::index::{Index, SessionSource, Turn};
use hippocamp_core::records::Tool;
use time::format_description::well_known::Rfc3339;
use time::macros::datetime;

const ACST: UtcOffset = match UtcOffset::from_hms(9, 30, 0) {
    Ok(o) => o,
    Err(_) => panic!("valid offset"),
};

fn turn(ordinal: u32, role: Role, ts: &str, text: &str) -> Turn {
    Turn {
        ordinal,
        role,
        ts: OffsetDateTime::parse(ts, &Rfc3339).unwrap(),
        tool_name: None,
        text: hippocamp_core::redact::redact(text),
    }
}

/// A top-level session `id` on `profile` (`""` or `-work`), named `title`
/// by its client, started in `/synthetic/<project>`.
fn put(
    index: &mut Index,
    root: &Path,
    id: &str,
    profile: &str,
    project: &str,
    title: &str,
    turns: &[Turn],
) {
    let dir = root.join(format!(".claude{profile}/projects/-{project}"));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{id}.jsonl"));
    let head = serde_json::json!({
        "type": "user",
        "message": {"content": "start"},
        "entrypoint": "cli",
        "cwd": format!("/synthetic/{project}"),
        "sessionId": id,
    });
    // As clients write it: `type` first.
    let name = format!(
        r#"{{"type":"custom-title","customTitle":{},"sessionId":"{id}"}}"#,
        serde_json::Value::from(title)
    );
    std::fs::write(&path, format!("{name}\n{head}\n")).unwrap();
    index
        .replace_session(
            &SessionSource {
                id,
                tool: Tool::ClaudeCode,
                transcript_path: &path,
                file_hash: id,
            },
            turns,
        )
        .unwrap();
}

const LIMIT: &str = "You've hit your 5-hour limit · resets 2:50pm (Australia/Darwin)";
const SIGNED_OUT: &str = "Failed to authenticate: this login reached its time limit and cannot be renewed. Run `claude auth login` to sign in again.";

/// Four days to 4 March 2026 at +09:30, shaped like a real week.
fn week() -> (tempfile::TempDir, Rollup) {
    use Role::{Assistant, User};
    let home = tempfile::tempdir().unwrap();
    let root = home.path();
    let mut index = Index::open(root, true).unwrap();
    // Stopped at the limit; carried on by a thread with a continuing title
    // whose request says it resumes. Its delegated task failed on the rate
    // limit, and later runs worked.
    put(&mut index, root, "ledger-1", "", "widgets", "Ledger: phase 2 import + phase 3 reports", &[
        turn(0, User, "2026-03-03T18:00:00Z", "Build the ledger import, then the reports."),
        turn(1, User, "2026-03-03T20:58:00Z", "Delegated task node:delegated-task:x (Ledger: phase 3 reports) [failed]: Claude API rate limit reached. Try again later."),
        turn(2, Assistant, "2026-03-03T21:00:00Z", LIMIT),
    ]);
    put(
        &mut index,
        root,
        "ledger-2",
        "-work",
        "widgets",
        "Ledger: phase 3 reports finish",
        &[
            turn(
                0,
                User,
                "2026-03-03T22:00:00Z",
                "You are resuming ledger work that a rate limit interrupted. Finish the reports.",
            ),
            turn(
                1,
                Assistant,
                "2026-03-03T23:00:00Z",
                "**Done.** The reports render; PR #9 is merged.",
            ),
        ],
    );
    // Stopped at the limit and abandoned: nothing later in `gadgets`.
    put(
        &mut index,
        root,
        "docs-1",
        "",
        "gadgets",
        "Docs site: search box",
        &[
            turn(
                0,
                User,
                "2026-03-03T23:00:00Z",
                "Add a search box to the docs site.",
            ),
            turn(1, Assistant, "2026-03-03T23:30:00Z", LIMIT),
        ],
    );
    // Two days old: one in a project worked in since, one in a project
    // nothing touched.
    put(
        &mut index,
        root,
        "vat-1",
        "",
        "widgets",
        "Billing: VAT rounding",
        &[
            turn(0, User, "2026-03-02T04:00:00Z", "Round VAT per line."),
            turn(1, Assistant, "2026-03-02T06:00:00Z", LIMIT),
        ],
    );
    put(
        &mut index,
        root,
        "lone-1",
        "",
        "lonely",
        "Lonely: export to CSV",
        &[
            turn(0, User, "2026-03-02T04:30:00Z", "Export the list to CSV."),
            turn(1, Assistant, "2026-03-02T05:00:00Z", LIMIT),
        ],
    );
    // Sign-in failures: on the default profile, then a run there that
    // worked; on `work`, nothing since.
    put(
        &mut index,
        root,
        "post-1",
        "",
        "umbrella",
        "Post the launch note",
        &[
            turn(0, User, "2026-03-03T20:30:00Z", "Post the launch note."),
            turn(1, Assistant, "2026-03-03T20:30:05Z", SIGNED_OUT),
        ],
    );
    put(
        &mut index,
        root,
        "brief-1",
        "-work",
        "umbrella",
        "Morning brief",
        &[
            turn(0, User, "2026-03-04T00:00:00Z", "Write the morning brief."),
            turn(1, Assistant, "2026-03-04T00:00:05Z", SIGNED_OUT),
        ],
    );
    // A delegated task that failed for good, with nothing since that redid it.
    put(
        &mut index,
        root,
        "fan-1",
        "",
        "umbrella",
        "Fan out the audit",
        &[
            turn(0, User, "2026-03-04T00:50:00Z", "Fan out the audit."),
            turn(
                1,
                User,
                "2026-03-04T01:00:00Z",
                "Delegated task node:delegated-task:y (Audit pass 2) [failed]: User not found.",
            ),
            turn(
                2,
                Assistant,
                "2026-03-04T01:01:00Z",
                "The delegate could not start.",
            ),
        ],
    );
    hippocamp_core::rollup::refresh(root, None, &index).unwrap();
    let rollup = Rollup::open(root).unwrap().unwrap();
    (home, rollup)
}

fn real_shapes(rollup: &Rollup, now: OffsetDateTime) -> Home {
    let marked = HashSet::new();
    build(&Inputs {
        now,
        offset: ACST,
        today: None,
        yesterday: None,
        rollup: Some(rollup),
        ws: None,
        history: None,
        marked: &marked,
        notes: &[],
        runs: &[],
        status: None,
        profiles: &["default".to_owned(), "work".to_owned()],
        ingest_interval_minutes: 10,
        dream_at: "03:30",
    })
}

fn row(n: &Need) -> (&'static str, String) {
    match n {
        Need::Review { lead, .. } => ("review", said(lead)),
        Need::Answer { text, .. } => ("answer", said(text)),
        Need::Resume { text, .. } => ("resume", said(text)),
        Need::Failure {
            text, fix: true, ..
        } => ("fix", said(text)),
        Need::Failure { text, .. } => ("failure", said(text)),
    }
}

#[test]
fn only_true_items_need_you_and_fixable_failures_come_first() {
    let (_home, rollup) = week();
    let home = real_shapes(&rollup, datetime!(2026-03-04 02:00 UTC));
    let rows: Vec<(&str, String)> = home.needs.iter().map(row).collect();
    assert_eq!(
        rows,
        [
            // The work profile is still signed out; the default one ran
            // again after its failure, so its row is gone.
            ("fix", "Morning brief stopped: its sign-in failed (profile work) · 09:30 · run claude auth login, then open it to resume".to_owned()),
            // Abandoned today; and two days old with nothing since in its
            // project. The ledger stop was carried on by "phase 3 reports
            // finish"; the VAT stop is old and its project moved on.
            ("resume", "Docs site: search box stopped at the usage limit · 09:00 · resets 14:50 · gadgets · nothing has carried it on".to_owned()),
            ("resume", "Lonely: export to CSV stopped at the usage limit · Mon 14:30 · lonely · nothing has carried it on".to_owned()),
            // The rate-limited delegate is gone: runs worked after it.
            ("failure", "Delegated task failed: User not found · 10:30".to_owned()),
        ]
    );
    assert_eq!(
        home.headline,
        "Morning brief could not sign in; run claude auth login to resume it."
    );
    assert_eq!(home.dock_line, "four things need you");
}

#[test]
fn a_stop_is_carried_on_by_a_continuing_thread() {
    let (_home, rollup) = week();
    let recent = rollup
        .sessions_in(
            datetime!(2026-02-28 20:00 UTC),
            datetime!(2026-03-04 20:00 UTC),
        )
        .unwrap();
    let facts = |id: &str| &recent.iter().find(|s| s.facts.id == id).unwrap().facts;
    let pulls = HashMap::new();
    let (stop, later) = (facts("ledger-1"), facts("ledger-2"));
    assert_eq!(
        continues(stop, stop.ending_at.unwrap(), later, &pulls),
        Some("a continuing title")
    );
    // Not the other way round, and not across projects.
    assert_eq!(
        continues(later, later.last_turn.unwrap(), stop, &pulls),
        None
    );
    let docs = facts("docs-1");
    assert_eq!(
        continues(docs, docs.ending_at.unwrap(), later, &pulls),
        None
    );
    // A title shares a word or two by chance; that is not the same work.
    assert!(!same_work(
        "Phase 6: packs and registry",
        "Phase 4 baseline + Phase 5 desktop app",
        None
    ));
    assert!(same_work(
        "Round 1: Phase 4 eval + Phase 5 design",
        "Phase 4 baseline + Phase 5 desktop app",
        None
    ));
    assert!(same_work(
        "Phases 7+9: feed and community",
        "Phases 7+9 feed + Phase 10 paid design",
        None
    ));
}

#[test]
fn once_everything_was_carried_on_home_is_calm() {
    let (_home, rollup) = week();
    // A week later nothing is recent: the stops are old and their projects
    // moved on or they are out of the window; failures are history.
    let home = real_shapes(&rollup, datetime!(2026-03-12 02:00 UTC));
    assert!(home.needs.is_empty(), "{:?}", home.needs);
    assert_eq!(home.headline, "Nothing needs you. Quiet night.");
}

/// The day's edition and Home use one rule for a stop at the limit: the
/// stop carried on later is said as such in the edition, and only the one
/// nothing carried on is a problem there and a Resume row on Home.
#[test]
fn the_edition_and_home_agree_on_stops() {
    let (_home, rollup) = week();
    let now = datetime!(2026-03-04 02:00 UTC);
    let home = real_shapes(&rollup, now);
    let resumed: Vec<String> = home
        .needs
        .iter()
        .filter(|n| matches!(n, Need::Resume { .. }))
        .map(|n| row(n).1)
        .collect();
    assert!(resumed.iter().any(|r| r.starts_with("Docs site")));
    assert!(!resumed.iter().any(|r| r.starts_with("Ledger")));
    let edition = crate::edition::build(
        &rollup,
        &crate::edition::Inputs {
            date: time::macros::date!(2026 - 03 - 04),
            offset: ACST,
            now,
            root: Path::new("/synthetic"),
            notes: &[],
            runs: &[],
            records: &[],
            traces: &std::collections::BTreeMap::new(),
            ingest_stopped: None,
        },
    )
    .unwrap();
    let deck = said(&edition.deck);
    assert!(
        deck.contains("Two sessions stopped at the usage limit; one was not carried on."),
        "{deck}"
    );
    let problems: Vec<&str> = edition.problems.iter().map(|p| p.text.as_str()).collect();
    assert!(
        problems.iter().any(|p| p.starts_with("Docs site")),
        "{problems:?}"
    );
    assert!(
        !problems.iter().any(|p| p.starts_with("Ledger")),
        "{problems:?}"
    );
    let ledger = edition
        .stories
        .iter()
        .find(|s| s.title.starts_with("Ledger"))
        .and_then(|s| s.ended.as_ref())
        .map(|e| e.label.as_str());
    assert_eq!(ledger, Some("usage limit, carried on later"));
}

/// A note changed after its fix says so without saying who, unless a
/// session that edited it was running then; never "you".
#[test]
fn an_edit_since_is_never_put_on_the_person() {
    let marked = HashSet::new();
    let inp = Inputs {
        now: datetime!(2026-03-04 02:00 UTC),
        offset: ACST,
        today: None,
        yesterday: None,
        rollup: None,
        ws: None,
        history: None,
        marked: &marked,
        notes: &[],
        runs: &[],
        status: None,
        profiles: &[],
        ingest_interval_minutes: 10,
        dream_at: "03:30",
    };
    let applied = datetime!(2026-03-03 16:00 UTC);
    assert_eq!(
        edited_since(
            &inp,
            Path::new("notes/a.md"),
            applied,
            Some(datetime!(2026-03-04 00:07 UTC))
        ),
        ("edited since, 09:37".to_owned(), None)
    );
    assert_eq!(
        edited_since(
            &inp,
            Path::new("notes/a.md"),
            applied,
            Some(datetime!(2026-03-03 00:07 UTC))
        )
        .0,
        "edited since, yesterday 09:37"
    );
}

/// The health count opens to what it counts.
#[test]
fn health_says_what_its_warnings_are() {
    let marked = HashSet::new();
    let status = Status {
        report: None,
        warnings: vec!["warning: no dreaming run recorded".to_owned()],
        last_ingest: Some(datetime!(2026-03-03 21:00 UTC)),
        ingest_stale: true,
        index_format: Some(2),
        errors: Vec::new(),
    };
    let home = build(&Inputs {
        now: datetime!(2026-03-04 02:00 UTC),
        offset: ACST,
        today: None,
        yesterday: None,
        rollup: None,
        ws: None,
        history: None,
        marked: &marked,
        notes: &[],
        runs: &[],
        status: Some(&status),
        profiles: &[],
        ingest_interval_minutes: 10,
        dream_at: "03:30",
    });
    assert_eq!(home.health, "health: two warnings");
    assert_eq!(home.health_warnings, ["warning: no dreaming run recorded"]);
    assert_eq!(
        home.health_notes,
        ["hippocamp has not read new sessions since 06:30: the ingest job may not be running."]
    );
}

#[test]
fn quotes_are_plain_whole_phrases() {
    assert_eq!(
        phrase(
            "**Nothing has been run** against this work; the `.claude-x` sign-in ends *soon*.",
            200
        ),
        "Nothing has been run against this work; the .claude-x sign-in ends soon."
    );
    // Too long: whole sentences, else whole words, with an ellipsis.
    assert_eq!(
        phrase(
            "- First sentence here. Second sentence is longer than the room.",
            30
        ),
        "First sentence here."
    );
    let cut = phrase(
        "Unresolved: the profile's sign-in ends between the fifteenth and the eighteenth",
        40,
    );
    assert_eq!(cut, "Unresolved: the profile's sign-in ends…");
    // A mark inside a word stays.
    assert_eq!(phrase("2*3 and snake_case", 50), "2*3 and snake_case");
}
