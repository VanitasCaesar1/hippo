#![allow(clippy::disallowed_methods)] // tests build temporary workspaces

mod common;

use std::fs;
use std::path::PathBuf;

use hippo_core::config::{self, InitOptions, Workspace};
use hippo_core::dream;
use hippo_core::records::Verdict;
use hippo_view::brain;

fn history(ws: &Workspace) -> dream::History {
    dream::history(ws).unwrap()
}

#[test]
fn rows_are_sorted_by_path_with_titles_and_lint_counts() {
    let env = common::env();
    common::note(
        &env,
        "brain/b.md",
        "# Bee\n\nsee [[Nowhere]] and [[Nothing]]\n",
    );
    common::note(&env, "brain/Z.md", "---\ntype: topic\n---\n# Zed\n");
    common::note(
        &env,
        "brain/sub/a.md",
        "---\ntype: topic\n---\nno heading\n",
    );
    let ws = common::workspace(&env);
    let got = brain::load(&ws, &history(&ws));
    assert_eq!(got.error, None);
    let paths: Vec<_> = got.rows.iter().map(|r| r.path.clone()).collect();
    // Byte order: upper-case before lower-case, `b.md` before `sub/`.
    assert_eq!(
        paths,
        ["brain/Z.md", "brain/b.md", "brain/sub/a.md"].map(PathBuf::from)
    );
    let titles: Vec<_> = got.rows.iter().map(|r| r.title.as_str()).collect();
    assert_eq!(titles, ["Zed", "Bee", "a"]);
    // `b.md`: missing frontmatter and two dangling links.
    let lint: Vec<_> = got.rows.iter().map(|r| r.lint).collect();
    assert_eq!(lint, [0, 3, 0]);
}

#[test]
fn a_title_with_a_hidden_character_is_escaped() {
    let env = common::env();
    common::note(&env, "brain/a.md", "# Safe\u{202E}Text\n");
    let ws = common::workspace(&env);
    let got = brain::load(&ws, &history(&ws));
    assert_eq!(got.rows[0].title, "Safe\\u{202E}Text");
}

#[test]
fn a_missing_notes_directory_is_an_error_with_no_rows() {
    let env = common::env();
    fs::remove_dir_all(env.workspace.join("brain")).unwrap();
    let ws = common::workspace(&env);
    let got = brain::load(&ws, &history(&ws));
    assert!(got.rows.is_empty());
    assert!(got.error.is_some());
}

/// A notes directory written `./brain` gives the paths `brain` gives.
#[test]
fn a_dot_slash_notes_directory_gives_the_same_row_paths() {
    let plain = common::env();
    common::note(&plain, "brain/a.md", "# A\n\n[[b]]\n");
    common::note(&plain, "brain/sub/b.md", "# B\n");
    let ws = common::workspace(&plain);
    let want = brain::load(&ws, &history(&ws));

    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let root = dir.path().join("work");
    fs::create_dir_all(root.join("brain/sub")).unwrap();
    config::init_in(
        &home,
        InitOptions {
            workspace: Some(root.clone()),
            notes_dir: Some("./brain".into()),
            entry_note: Some("brain/index.md".into()),
            add_roots: Vec::new(),
            force: false,
        },
    )
    .unwrap();
    fs::write(root.join("brain/a.md"), "# A\n\n[[b]]\n").unwrap();
    fs::write(root.join("brain/sub/b.md"), "# B\n").unwrap();
    let ws = Workspace::load(&config::Config::load_in(&home).unwrap()).unwrap();
    assert_eq!(ws.notes_dir.as_os_str(), "./brain");
    let got = brain::load(&ws, &history(&ws));

    let paths = |b: &brain::Brain| b.rows.iter().map(|r| r.path.clone()).collect::<Vec<_>>();
    assert_eq!(paths(&got), paths(&want));
    let view = brain::view(&ws, &got, &history(&ws), &PathBuf::from("brain/a.md")).unwrap();
    assert_eq!(view.links[0].1, Some(PathBuf::from("brain/sub/b.md")));
}

#[test]
fn a_note_view_has_frontmatter_links_lint_and_receipts() {
    let env = common::env();
    common::note(
        &env,
        "brain/a.md",
        "---\ntype: decision\nstatus: current\nupdated: 2026-11-01\nreview_after: 2026-12-01\nsources:\n  - docs/x.md\n  - docs/y.md\n---\n# A\n\n[[b]] and [[Nowhere|alias]]\n",
    );
    common::note(&env, "brain/sub/b.md", "# B\n");
    let ws = common::workspace(&env);
    let id = common::ulid_at(1_794_000_000_000, 1);
    common::proposal(&ws, id, "brain/a.md", "old", "new", &[]);
    common::receipt(
        &ws,
        common::ulid_at(1_794_000_001_000, 2),
        id,
        Verdict::Reject,
        None,
    );
    // Another note's proposal is not this note's change.
    common::proposal(
        &ws,
        common::ulid_at(1_794_000_002_000, 3),
        "brain/sub/b.md",
        "o",
        "n",
        &[],
    );

    let history = history(&ws);
    let loaded = brain::load(&ws, &history);
    let view = brain::view(&ws, &loaded, &history, &PathBuf::from("brain/a.md")).unwrap();

    let pairs: Vec<(&str, &str)> = view
        .frontmatter
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    assert_eq!(
        pairs,
        [
            ("type", "decision"),
            ("status", "current"),
            ("updated", "2026-11-01"),
            ("review_after", "2026-12-01"),
            ("sources", "docs/x.md, docs/y.md"),
        ]
    );
    assert!(!view.cut);
    assert_eq!(view.links.len(), 2);
    assert_eq!(&view.body[view.links[0].0.clone()], "[[b]]");
    assert_eq!(view.links[0].1, Some(PathBuf::from("brain/sub/b.md")));
    assert_eq!(&view.body[view.links[1].0.clone()], "[[Nowhere|alias]]");
    assert_eq!(view.links[1].1, None);
    assert!(view.lint.iter().any(|f| f.kind() == "dangling-link"));
    assert!(view
        .lint
        .iter()
        .all(|f| f.note() == std::path::Path::new("a.md")));
    assert_eq!(view.receipts.len(), 1);
    assert_eq!(view.receipts[0].proposal, id);

    let missing = brain::view(&ws, &loaded, &history, &PathBuf::from("brain/none.md"));
    assert!(missing.is_err());
}

/// The body is cut at 65,536 bytes on a character boundary; a link that
/// crosses the cut is dropped and one before it stays.
#[test]
fn the_body_is_cut_at_64_kb_on_a_character_boundary() {
    let env = common::env();
    // `é` is two bytes and starts at byte 65,535, so it crosses the limit.
    let mut text = String::from("[[Other]] ");
    text.push_str(&"a".repeat(65_535 - text.len()));
    text.push('é');
    text.push_str(" tail");
    assert!(text.len() > brain::BODY_LIMIT);
    common::note(&env, "brain/big.md", &text);
    common::note(&env, "brain/Other.md", "# Other\n");
    let ws = common::workspace(&env);
    let history = history(&ws);
    let loaded = brain::load(&ws, &history);
    let view = brain::view(&ws, &loaded, &history, &PathBuf::from("brain/big.md")).unwrap();
    assert!(view.cut);
    assert_eq!(view.body.len(), 65_535);
    assert!(!view.body.contains('é'));
    assert_eq!(view.links.len(), 1);

    // A link across the cut.
    let mut text = "a".repeat(65_530);
    text.push_str("[[Other]]");
    common::note(&env, "brain/across.md", &text);
    let loaded = brain::load(&ws, &history);
    let view = brain::view(&ws, &loaded, &history, &PathBuf::from("brain/across.md")).unwrap();
    assert!(view.cut);
    assert_eq!(view.body.len(), 65_536);
    assert!(view.links.is_empty());
}

#[test]
fn attention_lists_overdue_first_and_groups_missing_frontmatter_by_folder() {
    let env = common::env();
    let fm = |review: &str| format!("---\ntype: topic\nreview_after: {review}\n---\n");
    common::note(
        &env,
        "brain/late.md",
        &format!("{}# Late\n", fm("2020-03-01")),
    );
    common::note(
        &env,
        "brain/later.md",
        &format!("{}# Later\n", fm("2020-01-01")),
    );
    common::note(
        &env,
        "brain/fine.md",
        &format!("{}# Fine\n", fm("2999-01-01")),
    );
    common::note(&env, "brain/Flows/a.md", "# Flow A\n");
    common::note(&env, "brain/Flows/b.md", "# Flow B\n\n[[Nowhere]]\n");
    common::note(&env, "brain/top.md", "# Top\n");
    common::note(&env, "brain/bad.md", "---\ntype: [unclosed\n---\n# Bad\n");
    let ws = common::workspace(&env);
    let got = brain::load(&ws, &history(&ws));
    let a = brain::attention(&got, &ws.notes_dir);
    let overdue: Vec<_> = a.overdue.iter().map(|(_, t, _)| t.as_str()).collect();
    assert_eq!(overdue, ["Later", "Late"], "most overdue first");
    let none: Vec<_> = a.no_frontmatter.iter().map(|(_, t)| t.as_str()).collect();
    assert_eq!(none, ["Flow A", "Flow B", "Top"]);
    assert_eq!(
        a.no_frontmatter_folders,
        [("Flows".to_owned(), 2), (String::new(), 1)]
    );
    assert_eq!(a.dangling.len(), 1);
    assert_eq!(a.dangling[0].1, "Flow B");
    assert_eq!(a.broken.len(), 1);
    assert_eq!(a.broken[0].1, "Bad");
    assert_eq!(a.len(), 7);
}
