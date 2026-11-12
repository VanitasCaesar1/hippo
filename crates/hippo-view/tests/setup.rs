#![allow(clippy::disallowed_methods)] // tests build temporary workspaces

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;

use hippo_view::setup::{self, Setup};

#[test]
fn ready() {
    let env = common::env();
    match setup::load(&env.home) {
        Setup::Ready { cfg, ws } => {
            assert_eq!(cfg.home, env.home);
            assert_eq!(ws.root, env.workspace);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn no_config_is_not_set_up() {
    let dir = tempfile::tempdir().unwrap();
    match setup::load(dir.path()) {
        Setup::NotSetUp { home, error } => {
            assert_eq!(home, dir.path());
            assert!(error.contains("config.toml"), "{error}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_config_that_does_not_parse_is_not_set_up() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("config.toml"), "workspace = [").unwrap();
    assert!(matches!(setup::load(dir.path()), Setup::NotSetUp { .. }));
}

#[test]
fn a_missing_workspace_directory() {
    let env = common::env();
    fs::remove_dir_all(&env.workspace).unwrap();
    assert_eq!(
        setup::load(&env.home),
        Setup::WorkspaceMissing {
            path: env.workspace.clone()
        }
    );
}

#[test]
fn a_missing_workspace_settings_file() {
    let env = common::env();
    let settings = env.workspace.join(".hippo").join("hippo.toml");
    fs::remove_file(&settings).unwrap();
    assert_eq!(
        setup::load(&env.home),
        Setup::WorkspaceMissing { path: settings }
    );
}

#[test]
fn a_directory_without_read_permission_is_access_denied() {
    let env = common::env();
    let hippo = env.workspace.join(".hippo");
    fs::set_permissions(&hippo, fs::Permissions::from_mode(0o000)).unwrap();
    // A superuser reads it anyway; the test then has nothing to show.
    let readable = fs::read_dir(&hippo).is_ok();
    let got = setup::load(&env.home);
    fs::set_permissions(&hippo, fs::Permissions::from_mode(0o755)).unwrap();
    if readable {
        return;
    }
    assert_eq!(
        got,
        Setup::AccessDenied {
            path: env.workspace.clone()
        }
    );
}

#[test]
fn an_unreadable_home_is_access_denied() {
    let env = common::env();
    fs::set_permissions(&env.home, fs::Permissions::from_mode(0o000)).unwrap();
    let readable = fs::read_dir(&env.home).is_ok();
    let got = setup::load(&env.home);
    fs::set_permissions(&env.home, fs::Permissions::from_mode(0o755)).unwrap();
    if readable {
        return;
    }
    assert_eq!(
        got,
        Setup::AccessDenied {
            path: env.home.clone()
        }
    );
}
