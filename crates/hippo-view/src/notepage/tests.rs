//! Tests of the note page. Every note here is synthetic.

use super::*;
use hippo_core::config::Workspace;
use hippo_core::dream::History;
use time::macros::date;

fn workspace(root: &Path) -> Workspace {
    Workspace {
        root: root.to_path_buf(),
        notes_dir: "notes".into(),
        entry_note: "notes/START HERE.md".into(),
        required_notes: Vec::new(),
        budget_bytes: hippo_core::DEFAULT_CONTEXT_BUDGET,
        log_path: "notes/Session log.md".into(),
        sample_tasks: Vec::new(),
        passes_off: Vec::new(),
        lessons: true,
        snapshot_keep_days: None,
        log_notes: None,
    }
}

const RUNBOOK: &str = "---\ntype: policy\nstatus: current\nreview_after: 2026-02-20\n---\n# Runbook\n\nHow the widgets service is kept alive. Read [[Deploy]] first, then [[Gone note]].\nIt covers backups, restores and the nightly jobs that check them.\n\n## Backups\n\nNightly at 02:00. Changed 2026-01-30, then again on 3 Feb 2026.\n\n### Restore drill\n\nMonthly; the last one ran 2026-02-11.\n\n## Webhooks\n\nRetries back off. See [[ops/Deploy]].\n\n```\n## not a heading 2026-12-31\n```\n";

fn setup() -> (tempfile::TempDir, Brain) {
    let dir = tempfile::tempdir().unwrap();
    let notes = dir.path().join("notes");
    std::fs::create_dir_all(notes.join("ops")).unwrap();
    std::fs::write(notes.join("ops/Runbook.md"), RUNBOOK).unwrap();
    std::fs::write(
        notes.join("ops/Deploy.md"),
        "# Deploy\n\nSee [[Runbook]] for backups.\n",
    )
    .unwrap();
    std::fs::write(
        notes.join("Index.md"),
        "# Index\n\n- [[Runbook]]\n- [[Deploy]]\n",
    )
    .unwrap();
    std::fs::write(notes.join("Island.md"), "# Island\n\nNo links at all.\n").unwrap();
    let ws = workspace(dir.path());
    let history = History {
        changes: Vec::new(),
        unreadable: Vec::new(),
    };
    let brain = crate::brain::load(&ws, &history);
    (dir, brain)
}

fn inputs<'a>(root: &'a Path, brain: &'a Brain, titles: &'a HashMap<String, String>) -> Inputs<'a> {
    Inputs {
        root,
        notes_dir: Path::new("notes"),
        brain,
        runs: &[],
        rollup: None,
        titles,
        today: date!(2026 - 03 - 01),
        offset: UtcOffset::UTC,
    }
}

#[test]
fn a_note_page_has_sections_links_and_an_alert() {
    let (dir, brain) = setup();
    let titles = HashMap::new();
    let p = page(
        &inputs(dir.path(), &brain, &titles),
        Path::new("notes/ops/Runbook.md"),
    )
    .unwrap();
    assert_eq!(p.kicker, ["ops", "policy", "status current"]);
    let headings: Vec<(&str, Option<&str>)> = p
        .sections
        .iter()
        .map(|s| (s.heading.as_str(), s.dated.as_deref()))
        .collect();
    // The level-3 drill folds into Backups; a fenced line is not a heading.
    assert_eq!(
        headings,
        [
            ("Runbook", None),
            ("Backups", Some("11 Feb")),
            ("Webhooks", None)
        ]
    );
    assert_eq!(p.largest, p.sections.iter().map(|s| s.bytes).max().unwrap());
    let from: Vec<&str> = p.linked_from.iter().map(|n| n.title.as_str()).collect();
    assert_eq!(from, ["Deploy", "Index"]);
    assert_eq!(p.linked_from[0].folder.as_deref(), Some("ops"));
    let to: Vec<&str> = p.links_to.iter().map(|n| n.title.as_str()).collect();
    assert_eq!(to, ["Deploy"]);
    assert_eq!(p.dangling, 1);
    // The passed review date is a lint finding, not repeated as the alert.
    assert_eq!(p.alert, None);
    assert_eq!(
        why_words("left out to fit the budget: 67 KB."),
        "to fit the budget: 67 KB"
    );
    let facts: Vec<&str> = p.facts.iter().map(|f| f.0.as_str()).collect();
    assert!(facts.contains(&"3 sections"), "{facts:?}");
    assert!(facts.contains(&"2 notes link here"), "{facts:?}");
    assert!(facts.contains(&"links to 1"), "{facts:?}");
}

#[test]
fn an_island_says_no_note_links_here() {
    let (dir, brain) = setup();
    let titles = HashMap::new();
    let p = page(
        &inputs(dir.path(), &brain, &titles),
        Path::new("notes/Island.md"),
    )
    .unwrap();
    assert!(p.facts.iter().any(|f| f.0 == "no note links here"));
    assert!(p.kicker.is_empty());
    assert!(p.alert.is_none());
    assert!(page(
        &inputs(dir.path(), &brain, &titles),
        Path::new("notes/Missing.md")
    )
    .is_none());
}

#[test]
fn dates_are_read_in_two_forms() {
    assert_eq!(
        newest_date("on 2026-01-30 and 3 February 2026, not 2026-13-01"),
        Some(date!(2026 - 02 - 03))
    );
    assert_eq!(newest_date("v12026-11-03x"), None);
    assert_eq!(newest_date("3 Nov 2026."), Some(date!(2026 - 11 - 03)));
    assert_eq!(newest_date("no dates"), None);
    assert_eq!(short_date(date!(2026 - 11 - 03)), "3 Nov");
}
