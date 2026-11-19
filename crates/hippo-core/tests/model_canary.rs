//! Invariant 4's canary: a model run through the default runner cannot
//! read a file in its run folder or in `$HIPPO_HOME` outside it. It needs
//! the real `claude`, signed in, and makes one real model call, so it runs
//! only when `HIPPO_MODEL_CANARY=1` is set and `claude` is on `PATH`;
//! otherwise it is skipped with a message.

use std::fs;

use hippo_core::config::Config;
use hippo_core::model;
use hippo_core::runners::{self, Role};

#[test]
fn the_model_cannot_read_a_canary_file() {
    if std::env::var_os("HIPPO_MODEL_CANARY").is_none_or(|v| v != "1") {
        eprintln!(
            "skipping the model canary: set HIPPO_MODEL_CANARY=1 to send one real model call"
        );
        return;
    }
    if !on_path("claude") {
        eprintln!("skipping the model canary: claude is not on PATH");
        return;
    }
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    fs::write(
        home.path().join("config.toml"),
        format!("workspace = {:?}\n", workspace.path().display().to_string()),
    )
    .unwrap();
    let cfg = Config::load_in(home.path()).unwrap();
    let plan = runners::plan(&cfg).unwrap();
    let runner = plan.runner(Role::Review).unwrap();
    let canary = model::canary(&cfg, runner)
        .unwrap_or_else(|err| panic!("the model run failed, so isolation is unverified: {err}"));
    assert!(canary.passed(), "the model read a canary file: {canary:?}");
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|dirs| std::env::split_paths(&dirs).any(|dir| dir.join(program).is_file()))
}
