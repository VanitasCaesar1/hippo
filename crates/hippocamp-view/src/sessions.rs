//! The Sessions panel and the search tab.

use std::collections::HashMap;
use std::path::PathBuf;

use hippocamp_core::config::Workspace;
use hippocamp_core::error::Result;
use hippocamp_core::index::{Hit, Index, StoredTurn};
use hippocamp_core::records::Listed;
use hippocamp_core::session_meta::{OPENINGS, OPENING_CHARS};
use hippocamp_core::sessions::{self as core_sessions, log_rows, LogRow, Origin};

/// The most hits a search returns, as `hippocamp recall` does by default.
pub const SEARCH_LIMIT: usize = 20;

/// The session records, as `hippocamp log` lists them.
#[derive(Debug, Clone, PartialEq)]
pub struct Sessions {
    /// Newest start first, then id; a record that does not load comes last,
    /// as an error. Turn counts are `None` here: [`turn_counts`] reads them
    /// from the index after the list shows.
    pub rows: Vec<Listed<LogRow>>,
}

/// Reads the session records. Reads no index.
pub fn load(ws: &Workspace) -> Result<Sessions> {
    Ok(Sessions {
        rows: log_rows(ws, None)?,
    })
}

/// The turns of session `id`, in order.
pub fn turns(index: &Index, id: &str) -> Result<Vec<StoredTurn>> {
    index.turns_for_session(id)
}

/// Each session's number of indexed turns, in one query.
pub fn turn_counts(index: &Index) -> Result<HashMap<String, u64>> {
    index.turn_counts()
}

/// `phrase` as one quoted phrase, at most [`SEARCH_LIMIT`] hits.
pub fn search(index: &Index, phrase: &str) -> Result<Vec<Hit>> {
    index.recall(phrase, SEARCH_LIMIT, false)
}

/// What a reader needs besides a session's record: its title, where it ran,
/// the session that started it and whether a program started it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct About {
    pub title: Option<String>,
    /// The working directory's last component.
    pub project: Option<String>,
    pub cwd: Option<PathBuf>,
    pub parent: Option<String>,
    pub automated: bool,
}

/// Which sessions [`about`] reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Which {
    /// Sessions that are not sub-agents by their id: the list's rows, read
    /// first (under a second on a large home).
    TopLevel,
    All,
}

/// The indexed sessions' [`About`], by id: each transcript's head (and a
/// top-level one's end, for its title record), and the opening user turns of
/// the sessions with no name or description. A session in `known` keeps
/// what was read for it before, so a reload reads only new transcripts.
/// Titles use each record's task as a fallback.
pub fn about(
    index: &Index,
    sessions: &Sessions,
    known: &HashMap<String, About>,
    which: Which,
) -> Result<HashMap<String, About>> {
    let mut out = known.clone();
    let sources = index.sources()?;
    let origins: Vec<(String, Origin)> = sources
        .into_iter()
        .filter(|(id, _)| !known.contains_key(id))
        .filter(|(id, _)| which == Which::All || parent_by_id(id).is_none())
        .map(|(id, (tool, path))| {
            let origin = core_sessions::origin(&id, &tool, &path);
            (id, origin)
        })
        .collect();
    let unnamed: Vec<&str> = origins
        .iter()
        .filter(|(_, o)| o.named.is_none() && o.description.is_none())
        .map(|(id, _)| id.as_str())
        .collect();
    let openings = index.openings_for(&unnamed, OPENINGS, OPENING_CHARS)?;
    let tasks: HashMap<&str, &str> = sessions
        .rows
        .iter()
        .filter_map(|l| l.record.as_ref().ok())
        .map(|r| (r.id.as_str(), r.task.as_str()))
        .collect();
    let empty = Vec::new();
    for (id, origin) in origins {
        let task = tasks.get(id.as_str()).copied().unwrap_or("");
        let opening = openings.get(&id).unwrap_or(&empty);
        let about = About {
            title: core_sessions::title(task, opening, Some(&origin)),
            project: origin.project(),
            cwd: origin.cwd.clone(),
            parent: origin.parent.clone(),
            automated: origin.automated,
        };
        out.insert(id, about);
    }
    Ok(out)
}

/// The session that started session `id`, as far as its id tells: a Claude
/// Code sub-agent's id is `<parent>-agent-<agent>`. [`About::parent`], read
/// from the transcript, is the authority once it is known.
pub fn parent_by_id(id: &str) -> Option<&str> {
    id.split_once("-agent-").map(|(parent, _)| parent)
}
