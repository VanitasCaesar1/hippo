//! The Brain panel: the notes' rows, and one note's tab.

use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};

use hippocamp_core::config::Workspace;
use hippocamp_core::dream::History;
use hippocamp_core::error::{Error, Result};
use hippocamp_core::notes::{self, Link, LintFinding, Note, Notes};
use hippocamp_core::records::Receipt;

use crate::text::visible;

/// A note's body is shown up to this many bytes.
pub const BODY_LIMIT: usize = 65_536;

/// Every note, scanned, with one row each for the dock.
#[derive(Debug, Clone)]
pub struct Brain {
    /// The scan; empty when the notes directory could not be scanned.
    pub notes: Notes,
    /// Sorted by path in byte order.
    pub rows: Vec<NoteRow>,
    /// Why the notes directory could not be scanned; `rows` is empty then.
    pub error: Option<String>,
    /// Every lint finding, with its note's workspace path, in the linter's
    /// order.
    pub findings: Vec<(PathBuf, LintFinding)>,
}

/// The notes that need a person, from the lint findings: what the overview
/// and the Needs attention tab list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Attention {
    /// Frontmatter that does not parse, or a known key of the wrong shape:
    /// the note's path, title and the linter's message.
    pub broken: Vec<(PathBuf, String, String)>,
    /// `[[links]]` that resolve to no note: path, title, the link.
    pub dangling: Vec<(PathBuf, String, String)>,
    /// Notes past their `review_after`, most overdue first: path, title,
    /// the date.
    pub overdue: Vec<(PathBuf, String, time::Date)>,
    /// Notes with no frontmatter, in path order: path and title.
    pub no_frontmatter: Vec<(PathBuf, String)>,
    /// The folders below the notes directory that hold notes with no
    /// frontmatter (`""` for the top level), most first, then by name.
    pub no_frontmatter_folders: Vec<(String, usize)>,
}

impl Attention {
    /// How many findings in all.
    pub fn len(&self) -> usize {
        self.broken.len() + self.dangling.len() + self.overdue.len() + self.no_frontmatter.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// The brain's findings, sorted for a person to act on.
pub fn attention(brain: &Brain, notes_dir: &Path) -> Attention {
    let title = |path: &Path| -> String {
        brain
            .rows
            .binary_search_by(|r| {
                r.path
                    .as_os_str()
                    .as_encoded_bytes()
                    .cmp(path.as_os_str().as_encoded_bytes())
            })
            .map(|i| brain.rows[i].title.clone())
            .unwrap_or_else(|_| visible(&path.display().to_string()))
    };
    let mut a = Attention::default();
    let mut folders: std::collections::BTreeMap<String, usize> = Default::default();
    for (path, finding) in &brain.findings {
        match finding {
            LintFinding::Error { message, .. } | LintFinding::BadFrontmatter { message, .. } => {
                a.broken.push((path.clone(), title(path), visible(message)));
            }
            LintFinding::DanglingLink { link, .. } => {
                a.dangling.push((path.clone(), title(path), visible(link)));
            }
            LintFinding::Overdue { review_after, .. } => {
                a.overdue.push((path.clone(), title(path), *review_after));
            }
            LintFinding::MissingFrontmatter { .. } => {
                a.no_frontmatter.push((path.clone(), title(path)));
                let below = path.strip_prefix(notes_dir).unwrap_or(path);
                let folder = below
                    .parent()
                    .map(|p| visible(&p.display().to_string()))
                    .unwrap_or_default();
                *folders.entry(folder).or_default() += 1;
            }
        }
    }
    a.overdue
        .sort_by(|x, y| x.2.cmp(&y.2).then_with(|| x.0.cmp(&y.0)));
    a.no_frontmatter.sort_by(|x, y| x.0.cmp(&y.0));
    let mut folders: Vec<(String, usize)> = folders.into_iter().collect();
    folders.sort_by(|x, y| y.1.cmp(&x.1).then_with(|| x.0.cmp(&y.0)));
    a.no_frontmatter_folders = folders;
    a
}

/// One row of the dock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteRow {
    /// Relative to the workspace, as proposals and sessions give notes: the
    /// notes directory joined with the note's path below it, through
    /// [`notes::normalise`], so `./brain` and `brain` give the same paths.
    pub path: PathBuf,
    /// The note's title, through [`visible`].
    pub title: String,
    /// How many lint findings the note has.
    pub lint: usize,
    /// The kind of each finding (`LintFinding::kind`), in the linter's order.
    pub lint_kinds: Vec<&'static str>,
}

/// One note's tab.
#[derive(Debug, Clone, PartialEq)]
pub struct NoteView {
    pub path: PathBuf,
    /// The known keys that are present, in the order `type`, `status`,
    /// `updated`, `review_after`, `sources`; dates as `YYYY-MM-DD`, sources
    /// joined with `, `. Values go through [`visible`].
    pub frontmatter: Vec<(String, String)>,
    /// The body, cut at [`BODY_LIMIT`] bytes at a character boundary. It is
    /// as the file has it: link ranges point into it, so the app passes each
    /// piece it draws through [`visible`].
    pub body: String,
    /// The body was cut.
    pub cut: bool,
    /// Each `[[link]]` that lies wholly inside `body`: its byte range and
    /// the note it resolves to (a path relative to the workspace, like
    /// [`NoteRow::path`]), or `None` when it dangles.
    pub links: Vec<(Range<usize>, Option<PathBuf>)>,
    /// The note's lint findings; their paths are relative to the notes
    /// directory.
    pub lint: Vec<LintFinding>,
    /// The receipts of changes to this note, in proposal then receipt order.
    pub receipts: Vec<Receipt>,
}

fn workspace_path(ws: &Workspace, key: &Path) -> PathBuf {
    notes::normalise(&ws.notes_dir.join(key))
}

/// Scans the notes and lints them. `history` is not needed for the rows; a
/// note's receipts are read in [`view`].
pub fn load(ws: &Workspace, _history: &History) -> Brain {
    let scanned = match Notes::scan(&ws.notes_root()) {
        Ok(notes) => notes,
        Err(e) => {
            return Brain {
                notes: Notes {
                    root: ws.notes_root(),
                    notes: Default::default(),
                },
                rows: Vec::new(),
                error: Some(e.to_string()),
                findings: Vec::new(),
            }
        }
    };
    let mut kinds: std::collections::HashMap<PathBuf, Vec<&'static str>> =
        std::collections::HashMap::new();
    let findings: Vec<(PathBuf, LintFinding)> = scanned
        .lint()
        .into_iter()
        .map(|f| (workspace_path(ws, f.note()), f))
        .collect();
    for (path, finding) in &findings {
        kinds.entry(path.clone()).or_default().push(finding.kind());
    }
    let mut rows: Vec<NoteRow> = scanned
        .notes
        .iter()
        .map(|(key, note)| {
            let path = workspace_path(ws, key);
            let lint_kinds = kinds.get(&path).cloned().unwrap_or_default();
            NoteRow {
                lint: lint_kinds.len(),
                lint_kinds,
                title: visible(&note.title()),
                path,
            }
        })
        .collect();
    rows.sort_by(|a, b| {
        a.path
            .as_os_str()
            .as_encoded_bytes()
            .cmp(b.path.as_os_str().as_encoded_bytes())
    });
    Brain {
        notes: scanned,
        rows,
        error: None,
        findings,
    }
}

/// The note at workspace path `path` (a [`NoteRow::path`]), ready to draw.
/// Fails when the brain holds no such note.
pub fn view(ws: &Workspace, brain: &Brain, history: &History, path: &Path) -> Result<NoteView> {
    let wanted = notes::normalise(path);
    let (_, note) = brain
        .notes
        .notes
        .iter()
        .find(|(key, _)| workspace_path(ws, key) == wanted)
        .ok_or_else(|| {
            Error::Io(io::Error::new(
                io::ErrorKind::NotFound,
                format!("no note at {}", path.display()),
            ))
        })?;

    let full = String::from_utf8_lossy(&note.body);
    let (body, cut) = cut_at(&full, BODY_LIMIT);
    let links = notes::link_spans(body)
        .into_iter()
        .map(|(range, inner)| {
            let target = match brain.notes.resolve_link(inner) {
                Link::Resolved(found) => Some(workspace_path(ws, &found)),
                Link::Dangling => None,
            };
            (range, target)
        })
        .collect();

    let lint = brain
        .findings
        .iter()
        .filter(|(p, _)| *p == wanted)
        .map(|(_, finding)| finding.clone())
        .collect();

    let receipts = history
        .changes
        .iter()
        .filter(|change| {
            change
                .proposal
                .record
                .as_ref()
                .is_ok_and(|proposal| notes::normalise(&proposal.note) == wanted)
        })
        .flat_map(|change| change.receipts.iter().cloned())
        .collect();

    Ok(NoteView {
        path: wanted,
        frontmatter: frontmatter(note),
        body: body.to_owned(),
        cut,
        links,
        lint,
        receipts,
    })
}

/// `text` up to `limit` bytes, ending on a character boundary.
fn cut_at(text: &str, limit: usize) -> (&str, bool) {
    if text.len() <= limit {
        return (text, false);
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (&text[..end], true)
}

fn frontmatter(note: &Note) -> Vec<(String, String)> {
    let fm = &note.frontmatter;
    let mut out = Vec::new();
    let mut put = |key: &str, value: Option<String>| {
        if let Some(value) = value {
            out.push((key.to_owned(), visible(&value)));
        }
    };
    put("type", fm.r#type.clone());
    put("status", fm.status.clone());
    put("updated", fm.updated.map(|d| d.to_string()));
    put("review_after", fm.review_after.map(|d| d.to_string()));
    put("sources", fm.sources.as_ref().map(|s| s.join(", ")));
    out
}
