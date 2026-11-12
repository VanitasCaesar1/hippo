//! The first run: `hippo` alone before and
//! after setup, `hippo start --yes`, the grouped help and the not-set-up
//! error, with `HOME` and `HIPPO_HOME` in temporary directories and the
//! synthetic transcripts of `fixtures/`.

use std::fs;
use std::path::Path;
use std::process::{Command, Output, Stdio};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures");

/// Runs `hippo` with `HOME` = `home`, `HIPPO_HOME` = `home/.hippo`, the
/// working directory `home` and no terminal on stdin.
fn hippo(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_hippo"))
        .args(args)
        .current_dir(home)
        .env("HOME", home)
        .env("HIPPO_HOME", home.join(".hippo"))
        .env_remove("RUST_LOG")
        .env_remove("CLAUDE_CONFIG_DIR")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// A home with one synthetic Claude Code project.
fn home_with_history() -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    let project = home.path().join(".claude/projects/-work-example");
    fs::create_dir_all(&project).unwrap();
    fs::copy(
        format!("{FIXTURES}/claude-code/synthetic.jsonl"),
        project.join("synthetic.jsonl"),
    )
    .unwrap();
    home
}

#[test]
fn hippo_alone_before_setup_offers_to_index_and_writes_nothing() {
    let home = home_with_history();
    let out = hippo(home.path(), &[]);
    let said = text(&out);
    assert_eq!(out.status.code(), Some(0), "{said}");
    assert!(said.contains("~/.claude/projects 1 files"), "{said}");
    assert!(said.contains("nothing is sent anywhere"), "{said}");
    assert!(said.contains("hippo start --yes"), "{said}");
    assert!(!home.path().join(".hippo/config.toml").exists());
}

#[test]
fn with_no_history_it_says_where_it_looked() {
    let home = tempfile::tempdir().unwrap();
    let out = hippo(home.path(), &[]);
    let said = text(&out);
    assert_eq!(out.status.code(), Some(0), "{said}");
    assert!(said.contains("No agent history found"), "{said}");
    assert!(said.contains("--add-root"), "{said}");
}

#[test]
fn every_command_before_setup_names_the_start() {
    let home = home_with_history();
    for args in [
        &["recall", "retry"][..],
        &["ingest"],
        &["fleet"],
        &["incidents"],
        &["show", "day"],
        &["config", "show"],
    ] {
        let out = hippo(home.path(), args);
        let said = text(&out);
        assert_eq!(out.status.code(), Some(1), "{args:?}: {said}");
        assert!(
            said.contains("hippo is not set up yet") && said.contains("run hippo"),
            "{args:?}: {said}"
        );
    }
    let out = hippo(home.path(), &["doctor"]);
    assert!(text(&out).contains("config | problem | hippo is not set up yet"));
}

#[test]
fn start_yes_indexes_the_history_and_ends_on_the_overview() {
    let home = home_with_history();
    let out = hippo(home.path(), &["start", "--yes"]);
    let said = text(&out);
    assert_eq!(out.status.code(), Some(0), "{said}");
    assert!(home.path().join(".hippo/config.toml").is_file());
    let config = fs::read_to_string(home.path().join(".hippo/config.toml")).unwrap();
    assert!(!config.contains("\nworkspace = "), "history only: {config}");
    assert!(said.contains("read 1 file in"), "{said}");
    assert!(said.contains("your last active day"), "{said}");
    assert!(said.contains("Try next"), "{said}");
    assert!(said.contains("hippo ask --sources-only"), "{said}");
    assert!(!said.to_lowercase().contains("unverified"), "{said}");

    // Set up: `hippo` alone shows the same overview, and `start` says so.
    let out = hippo(home.path(), &[]);
    let said = text(&out);
    assert_eq!(out.status.code(), Some(0), "{said}");
    assert!(said.contains("Busiest sessions"), "{said}");
    let out = hippo(home.path(), &["start"]);
    assert!(text(&out).contains("hippo is already set up"));
}

#[test]
fn help_lists_the_commands_in_groups() {
    let home = tempfile::tempdir().unwrap();
    let out = hippo(home.path(), &["--help"]);
    let said = text(&out);
    assert_eq!(out.status.code(), Some(0), "{said}");
    let order = [
        "Start here:",
        "Every day:",
        "Notes and dreaming:",
        "Packs:",
        "Setup and checks:",
        "Run by hippo itself",
    ];
    let at: Vec<usize> = order.iter().map(|h| said.find(h).unwrap()).collect();
    assert!(at.windows(2).all(|w| w[0] < w[1]), "{said}");
}

#[test]
fn doctor_passes_a_healthy_new_setup_and_sends_nothing() {
    let home = home_with_history();
    assert!(hippo(home.path(), &["start", "--yes"]).status.success());
    let out = hippo(home.path(), &["doctor"]);
    let said = text(&out);
    assert_eq!(out.status.code(), Some(0), "{said}");
    assert!(!said.contains("| problem |"), "{said}");
    if cfg!(target_os = "macos") {
        assert!(said.contains("launchd | ok | not set up yet"), "{said}");
    }
}

#[test]
fn init_creates_a_missing_notes_folder_with_a_starter_entry_note() {
    let home = home_with_history();
    assert!(hippo(home.path(), &["start", "--yes"]).status.success());
    let ws = home.path().join("project");
    fs::create_dir(&ws).unwrap();
    let out = hippo(
        home.path(),
        &[
            "init",
            "--workspace",
            ws.to_str().unwrap(),
            "--notes-dir",
            "notes",
            "--entry-note",
            "notes/START HERE.md",
        ],
    );
    let said = text(&out);
    assert_eq!(out.status.code(), Some(0), "{said}");
    assert!(said.contains("(it did not exist)"), "{said}");
    assert!(said.contains("a starter entry note"), "{said}");
    assert!(said.contains("run hippo ingest --full"), "{said}");
    let entry = fs::read_to_string(ws.join("notes/START HERE.md")).unwrap();
    assert!(entry.starts_with("---\ntype: policy\n"), "{entry}");
    // The context compiles from the starter note.
    let out = hippo(home.path(), &["context"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
}

#[test]
fn a_missing_model_cli_names_the_two_ways_out() {
    let home = home_with_history();
    assert!(hippo(home.path(), &["start", "--yes"]).status.success());
    let out = hippo(
        home.path(),
        &[
            "config",
            "set",
            "model_command",
            "hippo-no-such-model-cli -p",
            "--yes",
        ],
    );
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let out = hippo(home.path(), &["ask", "retry backoff"]);
    let said = text(&out);
    assert!(
        said.contains("hippo-no-such-model-cli is not on PATH"),
        "{said}"
    );
    assert!(said.contains("install Claude Code"), "{said}");
}

#[test]
fn a_session_prefix_works_everywhere_and_any_indexed_session_resumes() {
    let home = home_with_history();
    assert!(hippo(home.path(), &["start", "--yes"]).status.success());
    for args in [&["show", "session", "stub"][..], &["xray", "stub"]] {
        let out = hippo(home.path(), args);
        assert_eq!(out.status.code(), Some(0), "{args:?}: {}", text(&out));
    }
    // Not one of today's sessions, and still resumable.
    let out = hippo(home.path(), &["resume", "stub"]);
    let said = text(&out);
    assert_eq!(out.status.code(), Some(0), "{said}");
    assert!(said.contains("claude --resume stub-1"), "{said}");
    assert!(said.contains("Picking up:"), "{said}");
    let out = hippo(home.path(), &["resume", "zzz"]);
    assert!(text(&out).contains("no indexed session zzz"));
}

#[test]
fn empty_results_name_the_next_step() {
    let home = home_with_history();
    // The copy is new; give it the fixture's own day, so today is empty.
    let copy = home
        .path()
        .join(".claude/projects/-work-example/synthetic.jsonl");
    fs::File::options()
        .write(true)
        .open(copy)
        .unwrap()
        .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_762_000_000))
        .unwrap();
    assert!(hippo(home.path(), &["start", "--yes"]).status.success());
    let said = text(&hippo(home.path(), &["recall", "zebra quantum"]));
    assert!(said.contains("hippo ask --sources-only"), "{said}");
    // The fixture's day is not today.
    let said = text(&hippo(home.path(), &["fleet"]));
    assert!(said.contains("No session today."), "{said}");
    assert!(said.contains("hippo show day 2026-"), "{said}");
}

/// `hippo card`: the overview's numbers,
/// a 1200 x 675 PNG, and no name of a project, session or path anywhere.
#[test]
fn the_card_shows_numbers_and_a_date_and_no_names() {
    let home = home_with_history();
    assert!(hippo(home.path(), &["start", "--yes"]).status.success());
    let overview = text(&hippo(home.path(), &[]));
    assert!(overview.contains("hippo card"), "{overview}");

    // The names the overview shows for the busiest sessions: title, project.
    let names: Vec<String> = overview
        .lines()
        .skip_while(|l| !l.starts_with("Busiest sessions"))
        .skip(1)
        .take_while(|l| l.starts_with("  "))
        .flat_map(|l| {
            l.trim()
                .split(" · ")
                .take(2)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .filter(|n| n.len() > 2 && n != "-")
        .collect();
    assert!(!names.is_empty(), "{overview}");

    let png = home.path().join("day.png");
    let out = hippo(home.path(), &["card", "--out", png.to_str().unwrap()]);
    let said = text(&out);
    assert_eq!(out.status.code(), Some(0), "{said}");
    assert!(said.contains("My agents did "), "{said}");
    assert!(
        said.contains("hippo · one brain for all your coding agents"),
        "{said}"
    );
    assert!(said.contains("(1200 x 675)"), "{said}");
    let card = said
        .lines()
        .take_while(|l| !l.starts_with("wrote "))
        .collect::<Vec<_>>()
        .join("\n");
    for name in &names {
        assert!(
            !card.contains(name.as_str()),
            "{name:?} on the card:\n{card}"
        );
    }
    assert!(!card.contains(&home.path().display().to_string()), "{card}");
    let bytes = fs::read(&png).unwrap();
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
    assert_eq!(u32::from_be_bytes(bytes[16..20].try_into().unwrap()), 1200);
    assert_eq!(u32::from_be_bytes(bytes[20..24].try_into().unwrap()), 675);

    // The work time is the overview's figure.
    let work = overview
        .lines()
        .find_map(|l| l.split(", ").find_map(|p| p.strip_suffix(" of agent work")))
        .unwrap()
        .to_owned();
    let (h, m) = work
        .trim_end_matches('m')
        .split_once("h ")
        .map(|(h, m)| (h.parse::<u32>().unwrap(), m.parse::<u32>().unwrap()))
        .unwrap_or_else(|| (0, work.trim_end_matches('m').parse().unwrap()));
    let minutes = |n: u32| format!("{n} {}", if n == 1 { "minute" } else { "minutes" });
    let hours = |n: u32| format!("{n} {}", if n == 1 { "hour" } else { "hours" });
    let words = match (h, m) {
        (0, 0) => "under a minute".to_owned(),
        (0, m) => minutes(m),
        (h, 0) => hours(h),
        (h, m) => format!("{} {}", hours(h), minutes(m)),
    };
    assert!(
        card.contains(&format!("My agents did {words} of work")),
        "{work}: {card}"
    );

    // A week, and the default place, outside any git repository.
    let out = hippo(home.path(), &["card", "--week"]);
    let said = text(&out);
    assert_eq!(out.status.code(), Some(0), "{said}");
    assert!(said.contains("of work in seven days."), "{said}");
    let cards = home.path().join(".hippo/cards");
    assert!(fs::read_dir(&cards).unwrap().any(|e| e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with("card-week-")));

    // A day with no session says so and names the last active day.
    let out = hippo(home.path(), &["card", "--day", "2001-01-01"]);
    let said = text(&out);
    assert_eq!(out.status.code(), Some(1), "{said}");
    assert!(said.contains("no session on 2001-01-01"), "{said}");
}
