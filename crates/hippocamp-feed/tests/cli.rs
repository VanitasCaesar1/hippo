//! Runs the built binary.

use std::path::Path;
use std::process::{Command, Output};

fn hippocamp_feed(env: &[(&str, &str)], args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_hippocamp-feed"))
        .env_clear()
        .envs(env.iter().copied())
        .args(args)
        .output()
        .expect("run hippocamp-feed")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn db_env(dir: &Path) -> String {
    dir.join("feed.sqlite").display().to_string()
}

#[test]
fn migrate_prints_its_line_on_a_new_database() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = db_env(dir.path());
    let out = hippocamp_feed(&[("FEED_DB", &db)], &["migrate"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(text(&out.stdout), "migrated 0 -> 1\n");
    // A second run changes nothing.
    let out = hippocamp_feed(&[("FEED_DB", &db)], &["migrate"]);
    assert_eq!(text(&out.stdout), "migrated 1 -> 1\n");
}

#[test]
fn backup_prints_its_line() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = db_env(dir.path());
    let out_dir = dir.path().join("copies");
    let env = [("FEED_DB", db.as_str())];
    assert!(hippocamp_feed(&env, &["migrate"]).status.success());
    let out = hippocamp_feed(
        &env,
        &[
            "backup",
            "--dir",
            out_dir.to_str().expect("utf-8"),
            "--keep-days",
            "7",
        ],
    );
    assert!(out.status.success(), "{}", text(&out.stderr));
    let line = text(&out.stdout);
    let pattern =
        regex::Regex::new(r"^backup \S+/feed-\d{8}T\d{6}Z\.sqlite [1-9]\d* bytes schema 1\n$")
            .expect("regex");
    assert!(pattern.is_match(&line), "{line:?}");
    assert_eq!(std::fs::read_dir(&out_dir).expect("dir").count(), 1);
}

#[test]
fn a_configuration_error_names_its_variable_and_exits_1() {
    let out = hippocamp_feed(&[], &["migrate"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        text(&out.stderr).contains("FEED_DB: is required"),
        "{}",
        text(&out.stderr)
    );
    assert!(text(&out.stdout).is_empty());

    let dir = tempfile::tempdir().expect("tempdir");
    let db = db_env(dir.path());
    let out = hippocamp_feed(
        &[
            ("FEED_DB", &db),
            ("FEED_BASE_URL", "http://feed.example"),
            ("FEED_BIND", "127.0.0.1:0"),
        ],
        &["serve"],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(
        text(&out.stderr).contains("FEED_BASE_URL: must be an https URL unless FEED_DEV=1"),
        "{}",
        text(&out.stderr)
    );
}

#[test]
fn serve_refuses_a_database_that_is_not_migrated() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = db_env(dir.path());
    let out = hippocamp_feed(
        &[
            ("FEED_DB", &db),
            ("FEED_BASE_URL", "https://feed.example"),
            ("FEED_BIND", "127.0.0.1:0"),
        ],
        &["serve"],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(
        text(&out.stderr).contains("migrate"),
        "{}",
        text(&out.stderr)
    );
}

#[test]
fn sync_registry_prints_its_line() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = db_env(dir.path());
    assert!(hippocamp_feed(&[("FEED_DB", &db)], &["migrate"])
        .status
        .success());
    let index =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/feed/registry-index.json");
    let index = index.display().to_string();
    // The raw address refuses connections, so no stylesheet is fetched.
    let env = [
        ("FEED_DB", db.as_str()),
        ("FEED_REGISTRY_INDEX", index.as_str()),
        ("FEED_RAW_URL", "http://127.0.0.1:1"),
        ("FEED_DEV", "1"),
        ("FEED_BASE_URL", "http://127.0.0.1:8080"),
    ];
    let out = hippocamp_feed(&env, &["sync-registry"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(
        text(&out.stdout),
        "listed 10, delisted 0, updated 0, skipped 5, unchanged 0, themes 0\n"
    );
    let out = hippocamp_feed(&env, &["sync-registry"]);
    assert_eq!(
        text(&out.stdout),
        "listed 0, delisted 0, updated 0, skipped 5, unchanged 10, themes 0\n"
    );
    // Without an index the command says what is missing.
    let out = hippocamp_feed(&[("FEED_DB", &db)], &["sync-registry"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        text(&out.stderr).contains("FEED_REGISTRY_INDEX"),
        "{}",
        text(&out.stderr)
    );
}

#[test]
fn activity_prints_a_table_of_weeks() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = db_env(dir.path());
    let env = [("FEED_DB", db.as_str())];
    assert!(hippocamp_feed(&env, &["migrate"]).status.success());
    let out = hippocamp_feed(&env, &["activity"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let table = text(&out.stdout);
    let lines: Vec<&str> = table.lines().collect();
    assert_eq!(
        lines[0],
        "| week | active | new | posts | comments | votes | measurements | reports |"
    );
    assert_eq!(lines.len(), 2 + 12);
    assert!(lines[2].contains("(partial)"), "{}", lines[2]);
    assert!(
        lines[2].ends_with("| 0 | 0 | 0 | 0 | 0 | 0 | 0 |"),
        "{}",
        lines[2]
    );
    let out = hippocamp_feed(&env, &["activity", "--weeks", "3"]);
    assert_eq!(text(&out.stdout).lines().count(), 2 + 3);
    for bad in ["0", "105", "x"] {
        let out = hippocamp_feed(&env, &["activity", "--weeks", bad]);
        assert!(!out.status.success(), "--weeks {bad}");
    }
    // The database must exist and be migrated.
    let out = hippocamp_feed(&[], &["activity"]);
    assert!(text(&out.stderr).contains("FEED_DB: is required"));
}

#[test]
fn dev_only_subcommands_refuse_without_dev_mode() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = db_env(dir.path());
    let out = hippocamp_feed(&[], &["mock-oauth"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        text(&out.stderr).contains("FEED_DEV=1"),
        "{}",
        text(&out.stderr)
    );
    let out = hippocamp_feed(&[("FEED_DB", &db)], &["seed-demo"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        text(&out.stderr).contains("FEED_DEV=1"),
        "{}",
        text(&out.stderr)
    );
    // In dev mode seed-demo needs a migrated database with packs listed.
    let env = [("FEED_DB", db.as_str()), ("FEED_DEV", "1")];
    assert!(hippocamp_feed(&env, &["migrate"]).status.success());
    let out = hippocamp_feed(&env, &["seed-demo"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        text(&out.stderr).contains("run sync-registry first"),
        "{}",
        text(&out.stderr)
    );
}

#[test]
fn mock_oauth_listens_on_loopback_and_answers() {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;

    let mut child = Command::new(env!("CARGO_BIN_EXE_hippocamp-feed"))
        .env_clear()
        .env("FEED_DEV", "1")
        .args(["mock-oauth", "--port", "0"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn");
    let mut line = String::new();
    BufReader::new(child.stdout.take().expect("stdout"))
        .read_line(&mut line)
        .expect("line");
    let address = line
        .trim()
        .strip_prefix("mock-oauth listening on ")
        .unwrap_or_else(|| panic!("unexpected line {line:?}"))
        .to_string();
    assert!(address.starts_with("127.0.0.1:"), "{address}");
    let page = ureq::get(format!(
        "http://{address}/login/oauth/authorize?client_id=hippocamp-feed-dev&redirect_uri=http://x/cb&state=s"
    ))
    .call()
    .expect("authorize")
    .body_mut()
    .read_to_string()
    .expect("body");
    for login in ["ada", "grace", "linus", "alan", "newbie"] {
        assert!(page.contains(login), "{login}");
    }
    child.kill().expect("kill");
    let _ = child.wait();
}

#[test]
fn account_commands_need_a_migrated_database_and_a_known_login() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = db_env(dir.path());
    let env = [("FEED_DB", db.as_str())];
    assert!(hippocamp_feed(&env, &["migrate"]).status.success());
    for args in [["account", "export", "ada"], ["account", "delete", "ada"]] {
        let out = hippocamp_feed(&env, &args);
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        assert!(
            text(&out.stderr).contains("no member has the login ada"),
            "{}",
            text(&out.stderr)
        );
    }
    let out = hippocamp_feed(&[], &["account", "export", "ada"]);
    assert!(text(&out.stderr).contains("FEED_DB: is required"));
}
