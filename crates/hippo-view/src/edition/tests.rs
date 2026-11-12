//! Tests of the edition. Every session, note and report here is synthetic,
//! and as messy as real ones: preamble-wrapped requests, a limit stop, a
//! resumed thread, sub-agents, a failed delegated task, a dreaming run that
//! changed nothing.

use super::*;
use hippo_core::adapters::Role;
use hippo_core::dream::report::{ReportRun, ReportStatus};
use hippo_core::index::{Index, SessionSource, Turn};
use hippo_core::records::{Listed, Tool};
use hippo_core::redact::redact;
use time::format_description::well_known::Rfc3339;
use time::macros::{date, datetime};

static NO_TRACES: BTreeMap<String, Trace> = BTreeMap::new();

const ACST: UtcOffset = match UtcOffset::from_hms(9, 30, 0) {
    Ok(o) => o,
    Err(_) => panic!("valid offset"),
};

fn t(ordinal: u32, role: Role, ts: &str, tool: Option<&str>, text: &str) -> Turn {
    Turn {
        ordinal,
        role,
        ts: OffsetDateTime::parse(ts, &Rfc3339).unwrap(),
        tool_name: tool.map(str::to_owned),
        text: redact(text),
    }
}

/// A session `id` whose transcript, under `root`, starts in the umbrella
/// directory `/synthetic/umbrella`: where the agents are started, not where
/// they work. (Not under `root`: a working directory in the temporary
/// directory reads as a program's.)
fn put(index: &mut Index, root: &Path, id: &str, profile: &str, turns: &[Turn]) {
    let dir = root.join(format!(".claude{profile}/projects/-umbrella"));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{id}.jsonl"));
    let head = serde_json::json!({
        "type": "user",
        "message": {"content": "start"},
        "entrypoint": "cli",
        "cwd": "/synthetic/umbrella",
        "sessionId": id,
    });
    std::fs::write(&path, format!("{head}\n")).unwrap();
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

fn bash(ordinal: u32, ts: &str, command: &str) -> Turn {
    let input = serde_json::json!({ "command": command });
    t(
        ordinal,
        Role::Tool,
        ts,
        Some("Bash"),
        &format!("Bash {input}"),
    )
}

fn output(ordinal: u32, ts: &str, text: &str) -> Turn {
    t(ordinal, Role::Tool, ts, Some("Bash"), text)
}

/// A final report as agents write them: a heading, a word, then the news,
/// then a long tail.
const REPORT: &str = "## Done\n\n**Shipped.**\n\nThe invoice export now runs on the worker queue, so a large export no longer blocks the desk; retries back off to five minutes. PR #6 (the listing) is open and #7's CI is running, see https://github.com/acme/widgets/pull/6.\n\n| Check | Result |\n|---|---|\n| tests | 212 passed |\n\nStill rough: the queue has no dead-letter view yet, and the PDF path is untested on Windows.";

/// A day (3 Mar 2026 at +09:30) of five top-level sessions and two
/// sub-agents, all started in the umbrella directory; most worked in a
/// checkout of `widgets` (origin `acme/widgets`).
fn day() -> (tempfile::TempDir, Rollup) {
    let home = tempfile::tempdir().unwrap();
    let root = home.path();
    let widgets = root.join("code/widgets");
    std::fs::create_dir_all(widgets.join(".git")).unwrap();
    std::fs::write(
        widgets.join(".git/config"),
        "[remote \"origin\"]\n\turl = git@github.com:acme/widgets.git\n",
    )
    .unwrap();
    let mut index = Index::open(root, true).unwrap();
    // The big one: preamble, edits in widgets, an agent, a pull request
    // opened and merged and one opened, two more only named; still open.
    let mut big = vec![t(
        0,
        Role::User,
        "2026-03-02T23:42:00Z",
        None,
        "Act as the implementation sub-agent for this task.\n\nMove the invoice export to the worker queue. Keep the old path until Friday.",
    )];
    for i in 1..40u32 {
        big.push(t(
            i,
            Role::Tool,
            "2026-03-03T01:00:00Z",
            Some("Edit"),
            &format!(
                r#"Edit {{"file_path":"{}/src/f{}.rs","old_string":"a","new_string":"b"}}"#,
                widgets.display(),
                i % 6
            ),
        ));
    }
    big.extend([
        t(
            40,
            Role::Tool,
            "2026-03-03T02:00:00Z",
            Some("Agent"),
            r#"Agent {"description":"Review the queue"}"#,
        ),
        bash(
            41,
            "2026-03-03T06:00:00Z",
            &format!("cd {} && gh pr create --fill", widgets.display()),
        ),
        output(
            42,
            "2026-03-03T06:00:00Z",
            "https://github.com/acme/widgets/pull/4",
        ),
        bash(
            43,
            "2026-03-03T06:10:00Z",
            &format!("cd {} && gh pr create --fill", widgets.display()),
        ),
        output(
            44,
            "2026-03-03T06:10:00Z",
            "https://github.com/acme/widgets/pull/5",
        ),
        bash(
            45,
            "2026-03-03T06:20:00Z",
            "gh pr merge https://github.com/acme/widgets/pull/4 --squash",
        ),
        output(
            46,
            "2026-03-03T06:20:00Z",
            "✓ Squashed and merged pull request acme/widgets#4",
        ),
        t(47, Role::Assistant, "2026-03-03T07:00:00Z", None, REPORT),
        bash(48, "2026-03-03T08:09:00Z", "cargo test"),
    ]);
    put(&mut index, root, "big-1", "", &big);
    put(
        &mut index,
        root,
        "big-1-agent-a1",
        "",
        &[t(
            0,
            Role::User,
            "2026-03-03T02:00:00Z",
            None,
            "Review the queue",
        )],
    );
    put(
        &mut index,
        root,
        "big-1-agent-a2",
        "",
        &[t(
            0,
            Role::User,
            "2026-03-03T02:05:00Z",
            None,
            "Review the docs",
        )],
    );
    // Two threads with one title: the first stops at the limit, the second
    // finishes.
    put(
        &mut index,
        root,
        "phase-a",
        "-work",
        &[
            t(
                0,
                Role::User,
                "2026-03-03T00:00:00Z",
                None,
                "Phase 2: ledger import",
            ),
            t(
                1,
                Role::Assistant,
                "2026-03-03T03:44:00Z",
                None,
                "You've hit your 5-hour limit · resets 2:50pm (Australia/Darwin)",
            ),
        ],
    );
    put(
        &mut index,
        root,
        "phase-b",
        "-work",
        &[
            t(
                0,
                Role::User,
                "2026-03-03T05:40:00Z",
                None,
                "Phase 2: ledger import",
            ),
            t(
                1,
                Role::Assistant,
                "2026-03-03T06:30:00Z",
                None,
                "Imported; the tests pass.",
            ),
        ],
    );
    // A failed delegated task; a sub-agent of it merged a pull request in
    // widgets, then the session stopped at the limit.
    put(
        &mut index,
        root,
        "deleg",
        "",
        &[
            t(0, Role::User, "2026-03-03T01:00:00Z", None, "Fan out the audit"),
            t(1, Role::User, "2026-03-03T01:10:00Z", None, "Delegated task node:delegated-task:x (Audit pass 1) [failed]: Claude API rate limit reached. Try again later."),
            t(2, Role::Assistant, "2026-03-03T01:11:00Z", None, "The delegate failed; I'll retry later."),
            t(3, Role::Assistant, "2026-03-03T01:40:00Z", None, "You've hit your 5-hour limit · resets 3:30pm (Australia/Darwin)"),
        ],
    );
    put(
        &mut index,
        root,
        "deleg-agent-b1",
        "",
        &[
            t(
                0,
                Role::User,
                "2026-03-03T01:20:00Z",
                None,
                "Merge the audit fix",
            ),
            bash(
                1,
                "2026-03-03T01:21:00Z",
                &format!(
                    "cd {} && git pull -q && gh pr merge 3 --squash",
                    widgets.display()
                ),
            ),
            output(2, "2026-03-03T01:21:00Z", "(Bash completed with no output)"),
        ],
    );
    // Yesterday, at 23:50 local (18:20Z on the 2nd): not in the edition.
    put(
        &mut index,
        root,
        "yesterday",
        "",
        &[t(0, Role::User, "2026-03-02T14:20:00Z", None, "Late fix")],
    );
    hippo_core::rollup::refresh(root, None, &index).unwrap();
    let rollup = Rollup::open(root).unwrap().unwrap();
    (home, rollup)
}

fn inputs<'a>(root: &'a Path, runs: &'a [Run], now: OffsetDateTime) -> Inputs<'a> {
    Inputs {
        date: date!(2026 - 03 - 03),
        offset: ACST,
        now,
        root,
        notes: &[],
        runs,
        records: &[],
        traces: &NO_TRACES,
        ingest_stopped: None,
    }
}

fn text(deck: &[Span]) -> String {
    deck.iter().map(|s| s.text.as_str()).collect()
}

#[test]
fn the_edition_ranks_stories_and_states_the_day() {
    let (home, rollup) = day();
    // 12:12Z: the big session's last turn (12:09Z) is three minutes old.
    let now = datetime!(2026-03-03 08:12 UTC);
    let e = build(&rollup, &inputs(home.path(), &[], now)).unwrap();
    assert_eq!(e.headline, "3 March 2026");
    assert_eq!(e.kicker, "The edition · Tuesday");
    // Pull requests only named (#6, #7) are not counted; the sub-agent's
    // merge of #3 is.
    assert_eq!(
        text(&e.deck),
        "4 sessions and 3 sub-agents ran 62 turns and 39 edits. They opened two pull requests and merged two. One session stopped at the usage limit."
    );
    assert!(e.deck[0].strong && e.deck[2].strong);
    assert_eq!(e.sessions, 4);
    assert_eq!(e.in_flight, 1);

    let titles: Vec<&str> = e.stories.iter().map(|s| s.title.as_str()).collect();
    assert_eq!(
        titles,
        [
            "Move the invoice export to the worker queue. Keep the old path until Friday.",
            "Fan out the audit",
            "Phase 2: ledger import"
        ]
    );
    let lead = &e.stories[0];
    assert_eq!(lead.ids, ["big-1"]);
    // Started in the umbrella directory, worked in widgets.
    assert_eq!(lead.project.as_deref(), Some("widgets"));
    assert_eq!(
        lead.facts,
        "09:12 → open · 8 h 27 m · 49 turns · 2 sub-agents · 6 files · widgets #4–#5"
    );
    assert_eq!(lead.ended.as_ref().unwrap().label, "still open");
    assert!(lead.ended.as_ref().unwrap().open);
    // What it did, then the first real sentence of its long final report;
    // the request is the title, so it is not said twice.
    assert_eq!(
        lead.summary.as_deref(),
        Some("Edited 6 files; opened two pull requests and merged one (widgets #4–#5). The invoice export now runs on the worker queue, so a large export no longer blocks the desk; retries back off to five minutes.")
    );

    // A session that only delegated takes its sub-agent's project and pull
    // request.
    let deleg = &e.stories[1];
    assert_eq!(deleg.project.as_deref(), Some("widgets"));
    assert_eq!(deleg.merged, ["https://github.com/acme/widgets/pull/3"]);
    assert!(deleg.opened.is_empty());

    let phase = &e.stories[2];
    assert_eq!(phase.ids, ["phase-a", "phase-b"]);
    // No work anywhere: the working directory's name.
    assert_eq!(phase.project.as_deref(), Some("umbrella · 2 threads"));
    assert_eq!(phase.ended.as_ref().unwrap().label, "limit, then resumed");
    assert!(phase.facts.ends_with("work"), "{}", phase.facts);

    assert_eq!(e.shipped.len(), 1);
    assert_eq!(e.shipped[0].repo, "acme/widgets");
    assert_eq!(e.shipped[0].numbers, [3, 4, 5]);
    assert_eq!((e.shipped[0].opened, e.shipped[0].merged), (2, 2));

    // 09:12 and 09:30 local: two requests.
    assert_eq!(e.hours[9], 2);
    assert_eq!(e.hours[10], 45);
    assert_eq!(e.busiest.as_deref(), Some("busiest 10:00–12:00"));
}

#[test]
fn summaries_say_what_a_session_did() {
    let s = |asked: Option<&str>, outcome: Option<&str>, files: u64, o: &[&str], m: &[&str]| {
        let own = |v: &[&str]| v.iter().map(|u| (*u).to_owned()).collect::<Vec<_>>();
        summary("Listing page", asked, outcome, files, &own(o), &own(m))
    };
    assert_eq!(
        s(
            Some("make the **listing** page load under a second. It takes 4 s now."),
            Some("PR #46 (the listing) is open. Its CI and #44's hardening are running."),
            3,
            &["https://github.com/acme/widgets/pull/46"],
            &[],
        )
        .as_deref(),
        Some("Make the listing page load under a second. Edited 3 files; opened one pull request (widgets #46). PR #46 (the listing) is open.")
    );
    // Nothing done but talk: the request and the answer.
    assert_eq!(
        s(
            Some("Why is the listing slow?"),
            Some("Done.\n\nIt loads every row; paging fixes it."),
            0,
            &[],
            &[]
        )
        .as_deref(),
        Some("Why is the listing slow? It loads every row; paging fixes it.")
    );
    assert_eq!(s(Some("listing page"), None, 0, &[], &[]), None);
    // A link is no request; a leading "Yes." is no news.
    assert_eq!(
        s(
            Some("https://github.com/acme/gears"),
            Some("Yes. Here is what is weak in that block."),
            0,
            &[],
            &[]
        )
        .as_deref(),
        Some("Here is what is weak in that block.")
    );
    assert_eq!(
        first_sentence("A very long first sentence that goes on and on about the queue and the export and the retries and the backoff and every other thing it touched today without a stop"),
        "A very long first sentence that goes on and on about the queue and the export and the retries and the backoff and every other thing it touched today…"
    );
}

#[test]
fn problems_are_what_a_person_can_act_on() {
    let (home, rollup) = day();
    let now = datetime!(2026-03-04 02:00 UTC);
    let mut inp = inputs(home.path(), &[], now);
    let e = build(&rollup, &inp).unwrap();
    let lines: Vec<(bool, &str, Option<&str>, Option<&str>)> = e
        .problems
        .iter()
        .map(|p| {
            (
                p.error,
                p.text.as_str(),
                p.detail.as_deref(),
                p.session.as_deref(),
            )
        })
        .collect();
    // The ledger import hit the limit too, but was resumed: not a problem.
    // A day on, the audit's stop is no problem either: Home's rule (a stop
    // more than a day old whose project was worked in since), so the two
    // agree. The same day, it is one (the test above).
    assert_eq!(
        lines,
        [(
            true,
            "Delegated task failed: Claude API rate limit reached. Try again later",
            Some("10:40"),
            Some("deleg")
        ),]
    );
    let audit = e
        .stories
        .iter()
        .find(|s| s.title == "Fan out the audit")
        .and_then(|s| s.ended.as_ref())
        .map(|x| (x.label.as_str(), x.problem));
    assert_eq!(audit, Some(("usage limit, carried on later", false)));
    assert!(
        text(&e.deck).contains("One session stopped at the usage limit and was carried on later."),
        "{}",
        text(&e.deck)
    );
    // A day later the big session is no longer open.
    assert_eq!(e.in_flight, 0);
    assert!(e.stories[0].ended.is_none());

    // An ingest that stopped is a problem of the last two days' editions.
    inp.ingest_stopped = Some(datetime!(2026-03-03 17:14 UTC));
    let e = build(&rollup, &inp).unwrap();
    let last = e.problems.last().unwrap();
    assert_eq!(last.text, "Ingest has not run since 02:44");
    assert!(last.error);
    inp.now = datetime!(2026-03-07 02:00 UTC);
    let e = build(&rollup, &inp).unwrap();
    assert!(!e.problems.iter().any(|p| p.text.starts_with("Ingest")));
}

#[test]
fn days_list_editions_with_their_dreaming_word() {
    let (_home, rollup) = day();
    let quiet = run("status: ok\n\n## Window\n\n- w: done\n\n## Inputs\n\n- w consolidate: 3 sessions, 1 notes, 9 turns, 0 human-confirmed\n", datetime!(2026-03-03 13:00 UTC));
    let rows = days(&rollup, std::slice::from_ref(&quiet), ACST).unwrap();
    let got: Vec<(String, usize, usize, Option<&str>)> = rows
        .iter()
        .map(|d| (d.date.to_string(), d.sessions, d.agents, d.dream.as_deref()))
        .collect();
    assert_eq!(
        got,
        [
            (
                "2026-03-03".to_owned(),
                4,
                3,
                Some("dream: nothing changed")
            ),
            ("2026-03-02".to_owned(), 1, 0, None),
        ]
    );
    // A run written in the small hours of the 4th is the 3rd's.
    let night = run("", datetime!(2026-03-03 18:00 UTC));
    let rows_night = days(&rollup, &[night], ACST).unwrap();
    assert_eq!(
        rows_night[0].dream.as_deref(),
        Some("dream: nothing changed")
    );
    // Before noon the home opens on yesterday; after, on today.
    assert_eq!(
        default_day(&rows, datetime!(2026-03-02 23:00 UTC), ACST),
        Some(date!(2026 - 03 - 02))
    );
    assert_eq!(
        default_day(&rows, datetime!(2026-03-03 04:00 UTC), ACST),
        Some(date!(2026 - 03 - 03))
    );
}

fn run(report: &str, written: OffsetDateTime) -> Run {
    Run {
        report: Listed {
            path: PathBuf::from("/synthetic/.hippo/reports/2026-03-03-dream.md"),
            record: Ok(ReportRun {
                status: ReportStatus {
                    path: PathBuf::from("/synthetic/.hippo/reports/2026-03-03-dream.md"),
                    date: date!(2026 - 03 - 03),
                    seq: 1,
                    failed: report.starts_with("status: failed"),
                    error: None,
                    dry_run: false,
                },
                proposals: Vec::new(),
                applied: Vec::new(),
            }),
        },
        digest: Some(crate::digest::parse(report)),
        written: Some(written.into()),
        changes: Vec::new(),
        missing: Vec::new(),
    }
}

#[test]
fn dreaming_is_one_paragraph_that_says_what_it_did() {
    let (home, rollup) = day();
    let w = "2026-03-02T13:00:00Z to 2026-03-03T13:00:00Z";
    let report = format!(
        "status: ok\n\n## Window\n\n- {w}: done\n\n## Inputs\n\n\
         - {w} consolidate: 40 sessions, 2 notes, 0 turns, 0 human-confirmed\n\
         - {w} verify notes/ops/Deploy.md: 2 sessions, 1 notes, 80 turns, 0 human-confirmed\n\
         \n## Omissions\n\n\
         - {w} verify notes/Run log.md: skipped: the note alone makes a 365617-byte prompt, over 200000\n\
         \n## Validation failures\n\n\
         - verify notes/ops/Deploy.md: new does not quote an evidence turn's text verbatim\n\
         \n## Verdicts\n\n## Applied\n"
    );
    let runs = [run(&report, datetime!(2026-03-03 13:25 UTC))];
    let now = datetime!(2026-03-04 02:00 UTC);
    let e = build(&rollup, &inputs(home.path(), &runs, now)).unwrap();
    let (lead, rest) = e.dreaming.clone().unwrap();
    assert_eq!(lead, "Dreaming, 22:55.");
    assert_eq!(
        rest,
        "Checked one note against 40 sessions; confirmed nothing. One proposal on Deploy failed validation. Run log was too big to check whole."
    );
    assert!(text(&e.deck).contains("Dreaming checked one note and confirmed nothing."));
    // A note too big for dreaming is housekeeping, not a problem.
    assert!(
        !e.problems.iter().any(|p| p.note.is_some()),
        "{:?}",
        e.problems
    );
    assert_eq!(e.health, ["Run log was too big for dreaming to read whole"]);
    assert_eq!(run_word(&runs[0]), "dream: nothing changed");

    // Many notes left out: three names at most, and one Problems line.
    let mut omissions = String::new();
    for i in 0..6 {
        omissions.push_str(&format!(
            "- {w} verify notes/Big {i}.md: skipped: the note alone makes a 365617-byte prompt, over 200000\n"
        ));
    }
    let crowded = report.replace("## Omissions\n\n", &format!("## Omissions\n\n{omissions}"));
    let runs = [run(&crowded, datetime!(2026-03-03 13:25 UTC))];
    let e = build(&rollup, &inputs(home.path(), &runs, now)).unwrap();
    let rest = e.dreaming.unwrap().1;
    assert!(
        rest.ends_with("Seven notes were too big to check whole; the largest are Big 0 and Big 1."),
        "{rest}"
    );
    assert_eq!(
        e.health,
        ["Seven notes were too big for dreaming to read whole"]
    );
    // Notes left out by rank are not too big.
    let ranked = report.replace(
        "## Omissions\n\n",
        &format!("## Omissions\n\n- {w} verify: omitted: note notes/Small.md (rank 11 of 45, over max_notes 10)\n"),
    );
    let runs = [run(&ranked, datetime!(2026-03-03 13:25 UTC))];
    let e = build(&rollup, &inputs(home.path(), &runs, now)).unwrap();
    assert_eq!(e.health, ["Run log was too big for dreaming to read whole"]);
    assert!(e
        .dreaming
        .unwrap()
        .1
        .ends_with("Run log was too big to check whole."));
}

#[test]
fn notes_changed_that_day_are_named_newest_first() {
    let (home, rollup) = day();
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("notes/ops")).unwrap();
    std::fs::write(root.path().join("notes/ops/Deploy.md"), "# Deploy\n").unwrap();
    std::fs::write(root.path().join("notes/Old.md"), "# Old\n").unwrap();
    let set = |p: &str, t: OffsetDateTime| {
        let f = std::fs::File::options()
            .write(true)
            .open(root.path().join(p))
            .unwrap();
        f.set_modified(t.into()).unwrap();
    };
    set("notes/ops/Deploy.md", datetime!(2026-03-03 06:00 UTC));
    set("notes/Old.md", datetime!(2026-02-01 06:00 UTC));
    let rows = [
        NoteRow {
            path: PathBuf::from("notes/ops/Deploy.md"),
            title: "Deploy".to_owned(),
            lint: 0,
            lint_kinds: Vec::new(),
        },
        NoteRow {
            path: PathBuf::from("notes/Old.md"),
            title: "Old".to_owned(),
            lint: 0,
            lint_kinds: Vec::new(),
        },
    ];
    let mut inp = inputs(root.path(), &[], datetime!(2026-03-04 02:00 UTC));
    inp.notes = &rows;
    let e = build(&rollup, &inp).unwrap();
    assert_eq!(e.notes_changed, 1);
    assert_eq!(e.notes[0].name, "ops / Deploy");
    assert_eq!(e.notes[0].detail, "15:30");
    assert!(text(&e.deck).contains("One note changed."));
    drop(home);
}

#[test]
fn words_numbers_and_plain_text() {
    assert_eq!(thousands(1_999), "1,999");
    assert_eq!(thousands(805_651), "805,651");
    assert_eq!(numbers(vec![8, 6, 7]), "#6–#8");
    assert_eq!(numbers(vec![73, 76]), "#73 #76");
    assert_eq!(numbers(vec![73, 76, 77, 82, 90, 93]), "#73 #76 #77 → #93");
    assert_eq!(
        plain(
            "## Done\n\n- **Built** the `queue`; see [the PR](https://x.test/1).\n```\ncode\n```"
        ),
        "Done Built the queue; see the PR."
    );
    assert_eq!(span(37_920), "10 h 32 m");
    assert_eq!(
        why_bytes("left out to fit the budget: 342 KB, over 195 KB"),
        342.0 * 1024.0
    );
    assert_eq!(why_bytes("no size"), 0.0);
    assert_eq!(
        sentences("First one. Second one is longer. Third.", 34),
        "First one. Second one is longer."
    );
    assert_eq!(sentences("one two three four", 12), "one two…");
    assert_eq!(sentences("short", 12), "short");
    assert_eq!(
        note_name(Path::new("notes/ops/Deploy.md"), "Deploy"),
        "ops / Deploy"
    );
    assert_eq!(note_name(Path::new("notes/Top.md"), "Top"), "Top");
}

#[test]
fn a_session_whose_sign_in_failed_is_a_problem() {
    let home = tempfile::tempdir().unwrap();
    let mut index = Index::open(home.path(), true).unwrap();
    put(
        &mut index,
        home.path(),
        "post",
        "",
        &[
            t(0, Role::User, "2026-03-02T23:00:00Z", None, "Post about the launch on the blog"),
            t(1, Role::Assistant, "2026-03-02T23:05:00Z", None, "Failed to authenticate: this login reached its time limit and cannot be renewed. Run `claude auth login` to sign in again."),
        ],
    );
    hippo_core::rollup::refresh(home.path(), None, &index).unwrap();
    let rollup = Rollup::open(home.path()).unwrap().unwrap();
    let e = build(
        &rollup,
        &inputs(home.path(), &[], datetime!(2026-03-04 02:00 UTC)),
    )
    .unwrap();
    assert_eq!(e.stories[0].ended.as_ref().unwrap().label, "sign-in failed");
    let p = &e.problems[0];
    assert_eq!(
        p.text,
        "Post about the launch on the blog stopped: its sign-in failed"
    );
    assert_eq!(
        p.detail.as_deref(),
        Some("08:35 · run claude auth login, then open it to resume")
    );
    // The failure is not its summary.
    assert_eq!(e.stories[0].summary, None);
}

#[test]
fn sessions_without_a_handoff_are_health_only_when_most_left_no_trace() {
    let (start, end) = (
        datetime!(2026-03-02 20:00 UTC),
        datetime!(2026-03-03 20:00 UTC),
    );
    let row = |id: &str, reason: Option<&str>, derived: bool| hippo_core::sessions::LogRow {
        id: id.into(),
        tool: Tool::ClaudeCode,
        started: datetime!(2026-03-03 05:00 UTC),
        ended: Some(datetime!(2026-03-03 06:00 UTC)),
        task: String::new(),
        turns: None,
        notes: Vec::new(),
        handoff: hippo_core::sessions::Handoff::No,
        no_change_reason: reason.map(str::to_owned),
        derived,
        confirmed: 0,
        rejected: 0,
    };
    let records = vec![
        row("ran", Some("unspecified"), false),
        row("edited", Some("unspecified"), false),
        row("silent", Some("unspecified"), false),
        row("declared", None, false),
        // Not counted: hippo's hooks never saw it.
        row("derived", None, true),
        row("derived-2", None, true),
    ];
    // Without the transcripts, three of four left no trace.
    assert_eq!(
        handoff_health(&records, &NO_TRACES, start, end).as_deref(),
        Some("3 of 4 sessions left no handoff, note edit or session-end command")
    );
    // One ran another tool's session-end command, one edited a note.
    let traces: BTreeMap<String, Trace> = [
        (
            "ran".to_owned(),
            Trace {
                ended: true,
                edited: false,
            },
        ),
        (
            "edited".to_owned(),
            Trace {
                ended: false,
                edited: true,
            },
        ),
        ("silent".to_owned(), Trace::default()),
    ]
    .into();
    assert_eq!(handoff_health(&records, &traces, start, end), None);
    // Another day has none.
    assert_eq!(
        handoff_health(&records, &NO_TRACES, end, end + time::Duration::days(1)),
        None
    );
}
