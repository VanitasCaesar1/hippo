//! The finder's rows: every note and every session.

use std::path::PathBuf;

use crate::brain::Brain;
use crate::sessions::Sessions;
use crate::text::visible;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinderKind {
    Note,
    Session,
}

/// What a row opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FinderKey {
    /// A note's workspace path.
    Note(PathBuf),
    /// A session's id.
    Session(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinderRow {
    pub kind: FinderKind,
    /// What the query is matched against and what is drawn.
    pub text: String,
    pub key: FinderKey,
}

/// Notes first, then sessions. A note's text is `<title> <workspace path>`;
/// a session's `<title> <YYYY-MM-DD> <id>` (its UTC start date), where the
/// title is what `title` gives for the id, else the task. What a person
/// types is a title, so it leads: a path or an id first costs the match its
/// skipped characters. A session record that does not load has no id and
/// no row. Text from files goes through [`visible`], and line breaks become
/// spaces so a row is one line.
pub fn rows(
    brain: &Brain,
    sessions: &Sessions,
    title: impl Fn(&str) -> Option<String>,
) -> Vec<FinderRow> {
    let notes = brain.rows.iter().map(|row| FinderRow {
        kind: FinderKind::Note,
        text: visible(&format!("{} {}", row.title, row.path.display())),
        key: FinderKey::Note(row.path.clone()),
    });
    let sessions = sessions
        .rows
        .iter()
        .filter_map(|listed| listed.record.as_ref().ok())
        .map(|row| {
            let name = title(&row.id).unwrap_or_else(|| row.task.clone());
            let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
            let date = row.started.to_offset(time::UtcOffset::UTC).date();
            FinderRow {
                kind: FinderKind::Session,
                text: visible(&format!("{name} {date} {}", row.id)),
                key: FinderKey::Session(row.id.clone()),
            }
        });
    notes.chain(sessions).collect()
}
