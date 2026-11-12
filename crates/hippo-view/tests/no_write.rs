#![allow(clippy::disallowed_methods)] // tests build temporary workspaces

mod common;

use std::fs;

use hippo_core::index::Index;
use hippo_view::setup::{self, Setup};
use hippo_view::status;
use time::macros::datetime;

/// Loading the setup and the status, twice, changes no file under the home
/// or the workspace: not its path, size or modification time.
#[test]
fn setup_and_status_write_nothing() {
    let env = common::env();
    fs::write(
        env.home.join("ingest-state.json"),
        r#"{"protocol":"HIPPO_V1","last_ingest":"2026-11-08T11:50:00Z","files_seen":1,"files_indexed":1}"#,
    )
    .unwrap();
    let reports = env.workspace.join(".hippo").join("reports");
    fs::create_dir_all(&reports).unwrap();
    fs::write(reports.join("2026-11-07-dream.md"), "status: ok\n").unwrap();
    Index::open(&env.home, true).unwrap();

    let before = common::snapshot(env.dir.path());
    for _ in 0..2 {
        let Setup::Ready { cfg, ws } = setup::load(&env.home) else {
            panic!("not ready");
        };
        let got = status::load(&cfg, &ws, datetime!(2026-11-08 12:00 UTC));
        assert_eq!(got.index_format, Some(2));
    }
    assert_eq!(common::snapshot(env.dir.path()), before);
}

/// The same with no index and nothing else: a missing index is not created.
#[test]
fn a_missing_index_is_not_created() {
    let env = common::env();
    let before = common::snapshot(env.dir.path());
    let Setup::Ready { cfg, ws } = setup::load(&env.home) else {
        panic!("not ready");
    };
    assert_eq!(
        status::load(&cfg, &ws, datetime!(2026-11-08 12:00 UTC)).index_format,
        None
    );
    assert_eq!(common::snapshot(env.dir.path()), before);
    assert!(!env.home.join("index.sqlite").exists());
}

/// The panel models' reads (load, reload, a selection's turns and evidence,
/// the turn counts and a search) change no file either.
#[test]
fn the_panel_models_write_nothing() {
    use hippo_core::adapters::Role;
    use hippo_core::dream;
    use hippo_core::notes;
    use hippo_core::records::Verdict;
    use hippo_view::{brain, dreaming, finder, sessions};
    use std::path::PathBuf;

    let env = common::env();
    common::note(&env, "brain/a.md", "# A\n\nsee [[B]]\n");
    common::note(&env, "brain/b.md", "# B\n\nold\n");
    let ws = common::workspace(&env);
    common::session(&ws, "s1", "a task", datetime!(2026-11-07 09:00 UTC));
    let id = common::ulid_at(1_794_139_200_000, 1);
    common::proposal(&ws, id, "brain/b.md", "old", "new", &[("s1", 0), ("s1", 5)]);
    let snapshot = notes::snapshot(&ws.root, id, &PathBuf::from("brain/b.md")).unwrap();
    let (before, after) = ("# B\n\nold\n", "# B\n\nnew\n");
    common::note(&env, "brain/b.md", after);
    common::receipt(
        &ws,
        common::ulid_at(1_794_139_200_001, 2),
        id,
        Verdict::Confirm,
        Some((
            &hippo_core::hash(before.as_bytes()),
            &hippo_core::hash(after.as_bytes()),
            snapshot,
        )),
    );
    common::report(&ws, "2026-11-08", &[id]);
    common::index_with(&env.home, "s1", &[(Role::User, "hello there")]);

    let snap = common::snapshot(env.dir.path());
    let before_counts = {
        let index = Index::open(&env.home, false).unwrap();
        sessions::turn_counts(&index).unwrap()
    };
    for _ in 0..2 {
        let history = dream::history(&ws).unwrap();
        let loaded = brain::load(&ws, &history);
        brain::view(&ws, &loaded, &history, &PathBuf::from("brain/a.md")).unwrap();
        let listed = sessions::load(&ws).unwrap();
        finder::rows(&loaded, &listed, |_| None);
        dreaming::load(&ws, &history).unwrap();

        let index = Index::open(&env.home, false).unwrap();
        assert_eq!(sessions::turns(&index, "s1").unwrap().len(), 1);
        assert_eq!(sessions::turn_counts(&index).unwrap(), before_counts);
        assert_eq!(sessions::search(&index, "hello").unwrap().len(), 1);
        let view = dreaming::view(&ws, &history, Some(&index), id).unwrap();
        assert_eq!(view.evidence.len(), 2);
        assert!(view.revert.is_some());
    }
    assert_eq!(common::snapshot(env.dir.path()), snap);
}
