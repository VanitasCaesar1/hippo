#![allow(clippy::disallowed_methods)] // tests build temporary workspaces

mod common;

use std::path::PathBuf;

use hippo_core::dream;
use hippo_view::finder::{self, FinderKey, FinderKind};
use hippo_view::{brain, sessions};
use time::macros::datetime;

#[test]
fn rows_hold_notes_then_sessions_with_the_design_texts() {
    let env = common::env();
    common::note(&env, "brain/Webhook retries.md", "# Webhook retries\n");
    common::note(&env, "brain/Deploy.md", "# Deploy\u{200B}notes\n");
    let ws = common::workspace(&env);
    common::session(
        &ws,
        "abc123",
        "fix the\nretries",
        datetime!(2026-11-07 23:30 -05:00),
    );
    common::session(&ws, "def456", "", datetime!(2026-11-06 01:00 UTC));
    std::fs::write(ws.hippo_dir().join("sessions/bad.json"), "x").unwrap();

    let loaded = brain::load(&ws, &dream::history(&ws).unwrap());
    let titled = |id: &str| (id == "def456").then(|| "Named\nthread".to_owned());
    let rows = finder::rows(&loaded, &sessions::load(&ws).unwrap(), titled);
    let got: Vec<_> = rows.iter().map(|r| (r.kind, r.text.as_str())).collect();
    assert_eq!(
        got,
        [
            (FinderKind::Note, "Deploy\\u{200B}notes brain/Deploy.md"),
            (FinderKind::Note, "Webhook retries brain/Webhook retries.md"),
            // 23:30 at -05:00 is 04:30 on the 8th in UTC.
            (FinderKind::Session, "fix the retries 2026-11-08 abc123"),
            (FinderKind::Session, "Named thread 2026-11-06 def456"),
        ]
    );
    assert_eq!(
        rows[0].key,
        FinderKey::Note(PathBuf::from("brain/Deploy.md"))
    );
    assert_eq!(rows[2].key, FinderKey::Session("abc123".to_owned()));
}
