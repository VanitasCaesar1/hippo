//! `hippo view …`, `hippo brief --json` and `hippo fleet --json` as the
//! night map reads them: on an empty workspace everywhere, and on the
//! synthetic demo home (`app/scripts/demo-home.sh`, macOS only: it uses
//! BSD `date`).

use std::fs;
use std::path::Path;
use std::process::Command;

use serde_json::Value;

/// A time zone where it is about noon now. The demo home stamps its
/// sessions minutes and hours before now, and hippo groups corrections by
/// local day: run where midnight is far from now, so the counts never
/// depend on the hour the test runs at.
fn midday_tz() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let off = 12 - (secs / 3600 % 24) as i64;
    match off {
        0 => "UTC".into(),
        // The Etc zones' sign is the other way round: Etc/GMT-5 is UTC+5.
        o if o > 0 => format!("Etc/GMT-{o}"),
        o => format!("Etc/GMT+{}", -o),
    }
}

fn hippo(home: &Path, user_home: &Path, args: &[&str]) -> Value {
    let out = Command::new(env!("CARGO_BIN_EXE_hippo"))
        .args(args)
        .env("TZ", midday_tz())
        .env("HIPPO_HOME", home)
        .env("HOME", user_home)
        .env("CODEX_HOME", user_home.join(".codex"))
        .env("CLAUDE_CONFIG_DIR", user_home.join(".claude"))
        .env("XDG_CONFIG_HOME", user_home.join(".config"))
        .env("PATH", "/usr/bin:/bin")
        .env_remove("RUST_LOG")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "hippo {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "hippo {args:?} printed no JSON ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

#[test]
fn an_empty_workspace_says_no_pass_has_run() {
    let home = tempfile::tempdir().unwrap();
    let user = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let ws = fs::canonicalize(dir.path()).unwrap();
    fs::create_dir(ws.join("notes")).unwrap();
    fs::write(
        ws.join("notes/START HERE.md"),
        "# Start here\n\nRead [[Other]].\n",
    )
    .unwrap();
    fs::write(ws.join("notes/Other.md"), "# Other\n\nThe other note.\n").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_hippo"))
        .args([
            "init",
            "--workspace",
            &ws.display().to_string(),
            "--notes-dir",
            "notes",
            "--entry-note",
            "notes/START HERE.md",
        ])
        .env("HIPPO_HOME", home.path())
        .env("HOME", user.path())
        .env("PATH", "/usr/bin:/bin")
        .output()
        .unwrap();
    assert!(out.status.success());

    let m = hippo(home.path(), user.path(), &["view", "morning", "--json"]);
    assert_eq!(m["headline"], "hippo has not run a night pass yet");
    assert!(m["pass"].is_null());
    assert_eq!(m["figures"]["rules_waiting"], 0);

    let map = hippo(home.path(), user.path(), &["view", "map", "--json"]);
    assert_eq!(map["notes"].as_array().unwrap().len(), 2);
    assert_eq!(map["edges"].as_array().unwrap().len(), 1);
    assert_eq!(map["mid"]["cards"].as_array().unwrap().len(), 2);
    // No line at rest; each of the two notes has the one link as its own.
    let focus = map["mid"]["focus"].as_array().unwrap();
    assert_eq!(focus.len(), 2);
    assert!(focus.iter().all(|f| f[1].as_array().unwrap().len() == 1));

    let note = hippo(
        home.path(),
        user.path(),
        &["view", "note", "notes/Other.md", "--json"],
    );
    assert_eq!(note["title"], "Other");
    assert_eq!(note["links_in"][0][1], "Start here");

    let g = hippo(home.path(), user.path(), &["view", "guard", "--json"]);
    assert_eq!(g["headline"], "Nothing to look at");

    let c = hippo(home.path(), user.path(), &["view", "counts", "--json"]);
    assert_eq!(c["rules_waiting"], 0);
    assert_eq!(c["guard_important"], 0);
}

#[cfg(target_os = "macos")]
#[test]
fn the_demo_home_fills_every_view() {
    // Not under /var/folders, whose sessions the lessons pass counts as a
    // program's.
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let user = tempfile::tempdir().unwrap();
    let demo = fs::canonicalize(dir.path()).unwrap().join("demo");
    let script = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../app/scripts/demo-home.sh"
    );
    let out = Command::new("sh")
        .arg(script)
        .arg(&demo)
        .env("HIPPO_BIN", env!("CARGO_BIN_EXE_hippo"))
        .env("TZ", midday_tz())
        .env("HOME", user.path())
        .env("CODEX_HOME", user.path().join(".codex"))
        .env("CLAUDE_CONFIG_DIR", user.path().join(".claude"))
        .env("PATH", "/usr/bin:/bin")
        .env_remove("HIPPO_HOME")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "demo-home.sh: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let home = demo.join("home");
    let v = |args: &[&str]| hippo(&home, user.path(), args);

    let m = v(&["view", "morning", "--json"]);
    let pass = &m["pass"];
    assert!(pass["started"].is_string(), "{pass}");
    assert!(pass["took"].is_string(), "{pass}");
    assert!(pass["sessions_read"].as_u64().unwrap() > 0);
    assert!(pass["notes_checked"].as_u64().unwrap() > 0);
    assert_eq!(m["figures"]["rules_waiting"], 4);
    let rejected = m["rejected"].as_array().unwrap();
    assert_eq!(rejected.len(), 2);
    assert!(rejected
        .iter()
        .any(|r| r["reason"].as_str().unwrap().contains("Keep 7 days")));
    let applied = m["applied"].as_array().unwrap();
    assert_eq!(applied.len(), 5);
    assert!(applied.iter().any(|a| a["reverted"] == true));
    assert!(applied
        .iter()
        .filter(|a| a["reverted"] == false)
        .all(|a| a["undo"][0] == "revert"));
    for r in m["rules"].as_array().unwrap() {
        assert!(!r["evidence"].as_array().unwrap().is_empty(), "{r}");
        assert!(r["note_label"].is_string(), "{r}");
    }

    let s = v(&["view", "strip", "--json"]);
    assert!(s["beats"].as_array().unwrap().len() > 96);
    assert!(s["night"].as_str().unwrap().starts_with("pass at "));

    let map = v(&["view", "map", "--json"]);
    let notes = map["notes"].as_array().unwrap();
    assert!(notes.len() >= 20);
    assert!(!map["edges"].as_array().unwrap().is_empty());
    assert_eq!(map["mid"]["cards"].as_array().unwrap().len(), notes.len());
    assert!(notes.iter().any(|n| n["flags"]["fixed"] == true));
    assert!(notes.iter().any(|n| n["flags"]["rejected"] == true));
    assert!(notes.iter().any(|n| n["flags"]["stale_days"].is_number()));

    let note = v(&["view", "note", "notes/Thumbnail pipeline.md", "--json"]);
    assert_eq!(note["receipts"].as_array().unwrap().len(), 1);
    assert!(!note["links_in"].as_array().unwrap().is_empty());

    let g = v(&["view", "guard", "--json"]);
    let kinds: Vec<&str> = g["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["kind"].as_str().unwrap())
        .collect();
    for k in ["remote-script", "credential-read", "fresh-clone-exec"] {
        assert!(kinds.contains(&k), "{kinds:?}");
    }
    assert_eq!(g["important"], 2);

    let c = v(&["view", "counts", "--json"]);
    assert_eq!(c["rules_waiting"], 4);
    assert_eq!(c["guard_important"], 2);
    assert!(c["indexed"].is_string());

    let b = v(&[
        "brief",
        "--task",
        "Run a preview of the album page",
        "--json",
    ]);
    assert!(!b["parts"]["required"].as_array().unwrap().is_empty());
    assert_eq!(b["parts"]["waiting"].as_array().unwrap().len(), 4);
    assert!(b["reaches"].is_array());

    let f = v(&["fleet", "--json"]);
    assert!(!f["recent"].as_array().unwrap().is_empty());
}
