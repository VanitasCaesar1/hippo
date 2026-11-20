//! Brain's map: every note with its folder, summary, reads this week,
//! flags and links, and the rectangles each zoom level draws.
//!
//! The layout is computed here, in map units (points at 100 %), so the app
//! draws it by multiplying every rectangle, line and text size by the zoom
//! factor (GPUI cannot scale a subtree), and the CLI and the web replica
//! print and draw the same numbers. Three levels:
//!
//! - [`Level::Mid`], the base: pinned notes (the entry note and required
//!   notes) in the first column, then folders packed into columns of
//!   fixed width, each a frame of note cards of fixed height.
//! - [`Level::Out`]: the same frames, each drawn as one folder card.
//! - [`Level::In`]: the same columns and order, with taller cards that
//!   carry each note's rules and evidence.
//!
//! Links are polylines: out of one card's right edge, one turn in the gap,
//! into the other card's left edge; two cards in one column are joined by
//! a bracket at their right. Each line has its own port on each card and
//! its own lane in the gap it turns in ([`route`]), so a note with many
//! links shows each one rather than a bundle.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use hippo_core::config::Workspace;
use hippo_core::lessons::{Lesson, Status};
use hippo_core::notes::{self, Link as NoteLink, LintFinding};
use hippo_core::rollup::Rollup;
use serde::Serialize;
use time::{Duration, OffsetDateTime, UtcOffset};

use super::{agent, count, Agent};
use crate::brain::Brain;

/// Below this zoom the map shows folders as single cards.
pub const OUT_BELOW: f32 = 0.55;
/// Above this zoom note cards show their rules and evidence.
pub const IN_ABOVE: f32 = 1.6;
/// The zoom range.
pub const MIN_ZOOM: f32 = 0.15;
pub const MAX_ZOOM: f32 = 3.0;

/// A card's width.
pub const CARD_W: f32 = 340.;
/// A note card's height at Mid: a title, a summary line, a chip line.
pub const CARD_H: f32 = 112.;
/// Between cards in a frame.
pub const GAP: f32 = 12.;
/// A frame's padding and its heading's height.
pub const FRAME_PAD: f32 = 14.;
pub const FRAME_HEAD: f32 = 40.;
/// Between frames in a column.
pub const FRAME_GAP: f32 = 24.;
/// Between columns: room for the lines' turns.
pub const COL_GAP: f32 = 72.;
/// A column's width.
pub const COL_W: f32 = CARD_W + 2. * FRAME_PAD;
/// One line of an In card's extra text.
pub const IN_LINE: f32 = 19.;
/// At most this many extra lines on an In card.
pub const IN_LINES: usize = 6;
/// Days a note can go unchanged before it counts as stale, when nothing
/// overdue says so first.
pub const STALE_DAYS: i64 = 60;

/// The zoom level a factor shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Out,
    Mid,
    In,
}

impl Level {
    pub fn of(zoom: f32) -> Level {
        if zoom < OUT_BELOW {
            Level::Out
        } else if zoom > IN_ABOVE {
            Level::In
        } else {
            Level::Mid
        }
    }
}

/// What a note carries, as its chip says.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Flags {
    /// The last pass applied a change to it (not reverted).
    pub fixed: bool,
    /// Pending rules that go into it.
    pub waiting: usize,
    /// The last pass's reviewer rejected a change to it.
    pub rejected: bool,
    /// Days since it changed, when it is stale.
    pub stale_days: Option<i64>,
    /// A required note: it is in every session's start block.
    pub required: bool,
    /// The entry note.
    pub entry: bool,
}

impl Flags {
    /// The chip a card shows first: `fixed last night`, `rule waiting`,
    /// `fix rejected`, `stale, 41 days`, `every session`; `None` for none.
    pub fn chip(&self) -> Option<(String, &'static str)> {
        if self.rejected {
            Some(("fix rejected".into(), "bad"))
        } else if self.waiting > 0 {
            Some((
                if self.waiting == 1 {
                    "rule waiting".into()
                } else {
                    format!("{} rules waiting", self.waiting)
                },
                "you",
            ))
        } else if self.fixed {
            Some(("fixed last night".into(), "ok"))
        } else if let Some(d) = self.stale_days {
            Some((format!("stale, {d} days"), "plain"))
        } else if self.required || self.entry {
            Some(("every session".into(), "night"))
        } else {
            None
        }
    }
}

/// One note on the map.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MapNote {
    /// Relative to the workspace.
    pub path: PathBuf,
    pub title: String,
    /// The top folder below the notes directory; empty for its top level.
    pub folder: String,
    pub summary: String,
    /// Sessions that read, discussed or edited it in 7 days.
    pub reads_week: u32,
    pub reads_by_agent: Vec<(Agent, u32)>,
    pub flags: Flags,
    /// Kept rules that went into it, newest first (In cards).
    pub rules: Vec<String>,
    /// Pending rules for it.
    pub pending: Vec<String>,
    /// The pending rules' evidence: the person's words (In cards).
    pub evidence: Vec<String>,
}

impl MapNote {
    /// `read 14× this week`, `not read this week`.
    pub fn reads_label(&self) -> String {
        if self.reads_week == 0 {
            "not read this week".into()
        } else {
            format!("read {}× this week", self.reads_week)
        }
    }

    /// The lines an In card adds.
    pub fn in_lines(&self) -> Vec<(String, &'static str)> {
        let mut out: Vec<(String, &'static str)> = Vec::new();
        for r in &self.pending {
            out.push((r.clone(), "waiting"));
        }
        for e in &self.evidence {
            out.push((e.clone(), "said"));
        }
        for r in &self.rules {
            out.push((r.clone(), "rule"));
        }
        out.truncate(IN_LINES);
        out
    }
}

/// A folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Folder {
    /// Empty for the notes directory's top level.
    pub name: String,
    pub notes: Vec<usize>,
    pub fixed: usize,
    pub waiting: usize,
    pub rejected: usize,
    pub stale: usize,
}

/// A rectangle in map units.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }
    pub fn meets(&self, o: &Rect) -> bool {
        self.x < o.right() && o.x < self.right() && self.y < o.bottom() && o.y < self.bottom()
    }
}

/// What a card stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Item {
    Note(usize),
    Folder(usize),
}

/// A card and where it is.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Card {
    pub item: Item,
    pub rect: Rect,
}

/// A line between two cards.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Line {
    pub from: Item,
    pub to: Item,
    /// The polyline's points, in map units.
    pub points: Vec<(f32, f32)>,
    pub bounds: Rect,
    /// Links it stands for (a folder line can stand for several).
    pub links: usize,
}

/// One level's drawing.
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct Layout {
    pub width: f32,
    pub height: f32,
    /// Folder frames (Mid and In); empty at Out, where the folder is a card.
    pub frames: Vec<(usize, Rect)>,
    pub cards: Vec<Card>,
    pub lines: Vec<Line>,
}

impl Layout {
    /// The card under a point, topmost first.
    pub fn card_at(&self, x: f32, y: f32) -> Option<&Card> {
        self.cards.iter().rev().find(|c| c.rect.contains(x, y))
    }

    /// The card that stands for `item`.
    pub fn card(&self, item: Item) -> Option<&Card> {
        self.cards.iter().find(|c| c.item == item)
    }

    /// The cards that meet `view`.
    pub fn cards_in<'a>(&'a self, view: &'a Rect) -> impl Iterator<Item = &'a Card> + 'a {
        self.cards.iter().filter(move |c| c.rect.meets(view))
    }

    /// The lines whose bounds meet `view`.
    pub fn lines_in<'a>(&'a self, view: &'a Rect) -> impl Iterator<Item = &'a Line> + 'a {
        self.lines.iter().filter(move |l| l.bounds.meets(view))
    }

    /// The zoom and the map point at the view's top left that fit the whole
    /// layout in a `w` × `h` view with `pad` points around it.
    pub fn fit(&self, w: f32, h: f32, pad: f32) -> (f32, (f32, f32)) {
        let zx = (w - 2. * pad).max(1.) / self.width.max(1.);
        let zy = (h - 2. * pad).max(1.) / self.height.max(1.);
        let z = zx.min(zy).clamp(MIN_ZOOM, 1.0);
        let ox = -((w / z - self.width) / 2.);
        let oy = -((h / z - self.height) / 2.).min(pad / z);
        (z, (ox, oy))
    }
}

/// The map.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NotesMap {
    /// `Brain · 13 notes in ~/code/tern/notes`.
    pub kicker: String,
    /// `What your agents know about Tern`.
    pub headline: String,
    /// `2 notes were fixed last night, 3 have a rule waiting, 2 have gone
    /// stale. Lines are links between notes.`
    pub lead: String,
    pub notes: Vec<MapNote>,
    pub folders: Vec<Folder>,
    /// Links between notes, `(from, to)`, each once.
    pub edges: Vec<(usize, usize)>,
    pub out: Layout,
    pub mid: Layout,
    #[serde(rename = "in")]
    pub inner: Layout,
}

impl NotesMap {
    pub fn layout(&self, level: Level) -> &Layout {
        match level {
            Level::Out => &self.out,
            Level::Mid => &self.mid,
            Level::In => &self.inner,
        }
    }

    /// The note at `path`.
    pub fn find(&self, path: &Path) -> Option<usize> {
        self.notes.iter().position(|n| n.path == path)
    }

    /// The notes `i` links to or is linked from.
    pub fn neighbours(&self, i: usize) -> BTreeSet<usize> {
        self.edges
            .iter()
            .filter_map(|&(a, b)| {
                if a == i {
                    Some(b)
                } else if b == i {
                    Some(a)
                } else {
                    None
                }
            })
            .collect()
    }

    /// The item at the same place in `to` as `item` is in `from`: a note's
    /// folder when `to` draws its folder as one card.
    pub fn same_item(&self, item: Item, to: Level) -> Item {
        match (item, to) {
            (Item::Note(i), Level::Out) if !self.pinned(i) => self
                .folders
                .iter()
                .position(|f| f.notes.contains(&i))
                .map_or(item, Item::Folder),
            _ => item,
        }
    }

    /// The entry note and required notes are pinned to the first column.
    pub fn pinned(&self, i: usize) -> bool {
        let f = &self.notes[i].flags;
        f.entry || f.required
    }

    /// The map point in level `to` that shows what map point `(x, y)` shows
    /// in level `from`: the same relative spot of the same card. `None`
    /// when no card is there.
    pub fn anchor(&self, from: Level, to: Level, x: f32, y: f32) -> Option<(f32, f32)> {
        let a = self.layout(from).card_at(x, y)?;
        let item = self.same_item(a.item, to);
        let b = self.layout(to).card(item)?;
        let (fx, fy) = ((x - a.rect.x) / a.rect.w, (y - a.rect.y) / a.rect.h);
        Some((b.rect.x + fx * b.rect.w, b.rect.y + fy * b.rect.h))
    }
}

/// What [`build`] reads.
pub struct Inputs<'a> {
    pub ws: &'a Workspace,
    pub brain: &'a Brain,
    pub rollup: Option<&'a Rollup>,
    pub lessons: &'a [Lesson],
    /// Notes the last pass changed (not reverted) and the ones its reviewer
    /// rejected a change to, relative to the workspace.
    pub fixed: &'a BTreeSet<PathBuf>,
    pub rejected: &'a BTreeSet<PathBuf>,
    pub now: OffsetDateTime,
    pub offset: UtcOffset,
}

/// The folder below the notes directory, empty for its top level.
fn folder_of(path: &Path, notes_dir: &Path) -> String {
    let below = path.strip_prefix(notes_dir).unwrap_or(path);
    if below.components().count() <= 1 {
        String::new()
    } else {
        below
            .components()
            .next()
            .map(|c| crate::text::visible(&c.as_os_str().to_string_lossy()))
            .unwrap_or_default()
    }
}

/// Builds the map.
pub fn build(inp: &Inputs<'_>) -> NotesMap {
    let ws = inp.ws;
    let notes_dir = notes::normalise(&ws.notes_dir);
    let today = inp.now.to_offset(inp.offset).date();
    let required: BTreeSet<PathBuf> = ws
        .required_notes
        .iter()
        .map(|p| notes::normalise(p))
        .collect();
    let entry = notes::normalise(&ws.entry_note);

    // Reads this week, by session, with each session's agent.
    let mut reads: HashMap<PathBuf, BTreeSet<String>> = HashMap::new();
    let mut tool_of: HashMap<String, String> = HashMap::new();
    if let Some(r) = inp.rollup {
        for seen in r
            .notes_in(inp.now - Duration::days(7), inp.now + Duration::minutes(1))
            .unwrap_or_default()
        {
            reads
                .entry(notes::normalise(&seen.note))
                .or_default()
                .insert(seen.session_id.clone());
        }
        let ids: BTreeSet<&String> = reads.values().flatten().collect();
        for id in ids {
            if let Ok(Some(f)) = r.session(id) {
                tool_of.insert(id.clone(), f.tool);
            }
        }
    }
    let overdue: HashMap<&Path, time::Date> = inp
        .brain
        .findings
        .iter()
        .filter_map(|(p, f)| match f {
            LintFinding::Overdue { review_after, .. } => Some((p.as_path(), *review_after)),
            _ => None,
        })
        .collect();
    let mut notes_out: Vec<MapNote> = Vec::new();
    let mut index: HashMap<PathBuf, usize> = HashMap::new();
    for row in &inp.brain.rows {
        let below = row.path.strip_prefix(&notes_dir).unwrap_or(&row.path);
        let note = inp.brain.notes.notes.get(below);
        let summary = note.map(|n| super::summary_of(n, 140)).unwrap_or_default();
        let sessions = reads.get(&row.path);
        let mut by: BTreeMap<String, u32> = BTreeMap::new();
        for s in sessions.into_iter().flatten() {
            if let Some(t) = tool_of.get(s) {
                *by.entry(t.clone()).or_default() += 1;
            }
        }
        let mut reads_by_agent: Vec<(Agent, u32)> =
            by.into_iter().map(|(t, n)| (agent(&t), n)).collect();
        reads_by_agent.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.name.cmp(&b.0.name)));
        let modified = std::fs::metadata(ws.root.join(&row.path))
            .and_then(|m| m.modified())
            .ok()
            .map(OffsetDateTime::from);
        let age = modified.map(|m| (today - m.to_offset(inp.offset).date()).whole_days());
        // Past its review date: days since that date; else unchanged for
        // [`STALE_DAYS`]: days since it changed.
        let stale_days = match overdue.get(row.path.as_path()) {
            Some(d) => Some((today - *d).whole_days().max(1)),
            None => age.filter(|a| *a >= STALE_DAYS),
        };
        let mine = |l: &&Lesson| {
            l.target
                .as_ref()
                .is_some_and(|t| notes::normalise(&t.note) == row.path)
        };
        let mut kept: Vec<&Lesson> = inp
            .lessons
            .iter()
            .filter(|l| l.status == Status::Kept)
            .filter(mine)
            .collect();
        kept.sort_by_key(|l| std::cmp::Reverse(l.updated));
        let pending: Vec<&Lesson> = inp
            .lessons
            .iter()
            .filter(|l| l.status == Status::Pending)
            .filter(mine)
            .collect();
        let flags = Flags {
            fixed: inp.fixed.contains(&row.path),
            waiting: pending.len(),
            rejected: inp.rejected.contains(&row.path),
            stale_days,
            required: required.contains(&row.path),
            entry: row.path == entry,
        };
        index.insert(below.to_path_buf(), notes_out.len());
        notes_out.push(MapNote {
            path: row.path.clone(),
            title: row.title.clone(),
            folder: folder_of(&row.path, &notes_dir),
            summary,
            reads_week: sessions.map_or(0, |s| s.len() as u32),
            reads_by_agent,
            flags,
            rules: kept.iter().map(|l| l.rule.clone()).collect(),
            pending: pending.iter().map(|l| l.rule.clone()).collect(),
            evidence: pending
                .iter()
                .flat_map(|l| l.evidence.iter().map(|e| crate::text::visible(&e.quote)))
                .collect(),
        });
    }

    // Links.
    let mut edges: BTreeSet<(usize, usize)> = BTreeSet::new();
    for (below, note) in &inp.brain.notes.notes {
        let Some(&a) = index.get(below) else { continue };
        let text = String::from_utf8_lossy(&note.body);
        for l in notes::links(&text) {
            if let NoteLink::Resolved(p) = inp.brain.notes.resolve_link(l) {
                if let Some(&b) = index.get(&p) {
                    if a != b && !edges.contains(&(b, a)) {
                        edges.insert((a, b));
                    }
                }
            }
        }
    }
    let edges: Vec<(usize, usize)> = edges.into_iter().collect();

    let folders = folders(&notes_out);
    let (out, mid, inner) = lay_out(&notes_out, &folders, &edges);

    let n = notes_out.len();
    let fixed = notes_out.iter().filter(|n| n.flags.fixed).count();
    let waiting = notes_out.iter().filter(|n| n.flags.waiting > 0).count();
    let stale = notes_out
        .iter()
        .filter(|n| n.flags.stale_days.is_some())
        .count();
    let rejected = notes_out.iter().filter(|n| n.flags.rejected).count();
    let mut said = Vec::new();
    if fixed > 0 {
        said.push(format!(
            "{} fixed last night",
            count(fixed, "note was", "notes were")
        ));
    }
    if waiting > 0 {
        said.push(format!(
            "{} a rule waiting",
            if waiting == 1 {
                "1 has".to_owned()
            } else {
                format!("{waiting} have")
            }
        ));
    }
    if rejected > 0 {
        said.push(format!(
            "{} a fix rejected",
            if rejected == 1 {
                "1 had".to_owned()
            } else {
                format!("{rejected} had")
            }
        ));
    }
    if stale > 0 {
        said.push(format!(
            "{} gone stale",
            if stale == 1 {
                "1 has".to_owned()
            } else {
                format!("{stale} have")
            }
        ));
    }
    let mut lead = match said.len() {
        0 => String::new(),
        1 => format!("{}. ", capital_first(&said[0])),
        _ => {
            let last = said.pop().unwrap_or_default();
            format!("{}, and {last}. ", capital_first(&said.join(", ")))
        }
    };
    lead.push_str(if edges.is_empty() {
        "No note links to another yet."
    } else {
        "Lines are links between notes."
    });
    let name = ws
        .root
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    NotesMap {
        kicker: format!(
            "Brain · {} in {}",
            count(n, "note", "notes"),
            crate::text::visible(&home_relative(&ws.root.join(&notes_dir)))
        ),
        headline: format!("What your agents know about {}", capital_first(&name)),
        lead,
        notes: notes_out,
        folders,
        edges,
        out,
        mid,
        inner,
    }
}

fn capital_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// `~/code/tern/notes` when under the home directory.
fn home_relative(p: &Path) -> String {
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        if let Ok(rest) = p.strip_prefix(&home) {
            return format!("~/{}", rest.display());
        }
    }
    p.display().to_string()
}

/// The folders with their notes, pinned notes left out.
fn folders(notes: &[MapNote]) -> Vec<Folder> {
    let mut by: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (i, n) in notes.iter().enumerate() {
        if n.flags.entry || n.flags.required {
            continue;
        }
        by.entry(n.folder.as_str()).or_default().push(i);
    }
    let mut out: Vec<Folder> = by
        .into_iter()
        .map(|(name, mut ix)| {
            ix.sort_by(|a, b| {
                let (x, y) = (&notes[*a], &notes[*b]);
                y.reads_week.cmp(&x.reads_week).then(x.title.cmp(&y.title))
            });
            let f = |p: fn(&MapNote) -> bool| ix.iter().filter(|i| p(&notes[**i])).count();
            Folder {
                name: name.to_owned(),
                fixed: f(|n| n.flags.fixed),
                waiting: f(|n| n.flags.waiting > 0),
                rejected: f(|n| n.flags.rejected),
                stale: f(|n| n.flags.stale_days.is_some()),
                notes: ix,
            }
        })
        .collect();
    out.sort_by(|a, b| b.notes.len().cmp(&a.notes.len()).then(a.name.cmp(&b.name)));
    out
}

/// An In card's height.
pub fn in_height(n: &MapNote) -> f32 {
    let lines = n.in_lines().len();
    CARD_H
        + if lines == 0 {
            0.
        } else {
            10. + lines as f32 * IN_LINE
        }
}

/// Where each folder goes: its first column, how many columns it spans,
/// and its notes in column-major order.
struct Placed {
    folder: usize,
    col: usize,
    span: usize,
    /// Notes per sub-column.
    per: usize,
}

/// Lays out all three levels.
pub fn lay_out(
    notes: &[MapNote],
    folders: &[Folder],
    edges: &[(usize, usize)],
) -> (Layout, Layout, Layout) {
    let n_notes = folders.iter().map(|f| f.notes.len()).sum::<usize>().max(1);
    let pinned: Vec<usize> = {
        let mut p: Vec<usize> = (0..notes.len())
            .filter(|i| notes[*i].flags.entry || notes[*i].flags.required)
            .collect();
        p.sort_by_key(|i| (!notes[*i].flags.entry, notes[*i].title.clone()));
        p
    };
    // Columns for a map about 1.6 times as wide as it is high.
    let cols = ((0.49 * (n_notes + pinned.len()) as f32).sqrt().round() as usize).clamp(2, 24);
    let per_col = n_notes.div_ceil(cols.saturating_sub(1).max(1)).max(4);
    let mid_h = |_: usize| CARD_H;
    let mut heights = vec![0f32; cols];
    if !pinned.is_empty() {
        heights[0] = pinned.len() as f32 * (CARD_H + GAP) - GAP + FRAME_GAP;
    }
    let mut placed: Vec<Placed> = Vec::new();
    for (fi, f) in folders.iter().enumerate() {
        let span = f.notes.len().div_ceil(per_col).clamp(1, cols);
        let per = f.notes.len().div_ceil(span);
        // The leftmost run of `span` columns whose tallest is lowest.
        let mut best = (0usize, f32::MAX);
        for c in 0..=(cols - span) {
            let top = heights[c..c + span].iter().cloned().fold(0., f32::max);
            if top < best.1 - 0.5 {
                best = (c, top);
            }
        }
        let (col, top) = best;
        let h = FRAME_HEAD + per as f32 * (CARD_H + GAP) - GAP + FRAME_PAD;
        for hh in &mut heights[col..col + span] {
            *hh = top + h + FRAME_GAP;
        }
        placed.push(Placed {
            folder: fi,
            col,
            span,
            per,
        });
    }
    let mid = place(folders, &pinned, &placed, edges, &mid_h, false);
    let inner = place(
        folders,
        &pinned,
        &placed,
        edges,
        &|i| in_height(&notes[i]),
        false,
    );
    let out = place(folders, &pinned, &placed, edges, &mid_h, true);
    (out, mid, inner)
}

fn col_x(c: usize) -> f32 {
    c as f32 * (COL_W + COL_GAP)
}

/// Places cards and frames from the column plan, with `height` per note;
/// `folded` draws each folder as one card (Out).
fn place(
    folders: &[Folder],
    pinned: &[usize],
    placed: &[Placed],
    edges: &[(usize, usize)],
    height: &dyn Fn(usize) -> f32,
    folded: bool,
) -> Layout {
    let mut cards = Vec::new();
    let mut frames = Vec::new();
    let cols = placed
        .iter()
        .map(|p| p.col + p.span)
        .max()
        .unwrap_or(1)
        .max(1);
    let mut bottom = vec![0f32; cols.max(1)];
    let mut y = 0.;
    for &i in pinned {
        let h = height(i);
        cards.push(Card {
            item: Item::Note(i),
            rect: Rect::new(FRAME_PAD, y, CARD_W, h),
        });
        y += h + GAP;
    }
    if !pinned.is_empty() {
        bottom[0] = y - GAP + FRAME_GAP;
    }
    for p in placed {
        let f = &folders[p.folder];
        let top = bottom[p.col..p.col + p.span]
            .iter()
            .cloned()
            .fold(0., f32::max);
        let x = col_x(p.col);
        let w = col_x(p.col + p.span - 1) + COL_W - x;
        let mut sub_bottom = top + FRAME_HEAD;
        for (k, chunk) in f.notes.chunks(p.per.max(1)).enumerate() {
            let mut cy = top + FRAME_HEAD;
            for &i in chunk {
                let h = height(i);
                if !folded {
                    cards.push(Card {
                        item: Item::Note(i),
                        rect: Rect::new(col_x(p.col + k) + FRAME_PAD, cy, CARD_W, h),
                    });
                }
                cy += h + GAP;
            }
            sub_bottom = sub_bottom.max(cy - GAP);
        }
        let frame = Rect::new(x, top, w, sub_bottom + FRAME_PAD - top);
        if folded {
            cards.push(Card {
                item: Item::Folder(p.folder),
                rect: frame,
            });
        } else {
            frames.push((p.folder, frame));
        }
        for b in &mut bottom[p.col..p.col + p.span] {
            *b = frame.bottom() + FRAME_GAP;
        }
    }
    let rect_of: HashMap<Item, Rect> = cards.iter().map(|c| (c.item, c.rect)).collect();
    let folder_of: HashMap<usize, usize> = placed
        .iter()
        .flat_map(|p| folders[p.folder].notes.iter().map(move |&i| (i, p.folder)))
        .collect();
    let lift = |i: usize| -> Item {
        if folded {
            folder_of
                .get(&i)
                .map_or(Item::Note(i), |&f| Item::Folder(f))
        } else {
            Item::Note(i)
        }
    };
    // One line per pair of cards, counting the links it stands for.
    let mut pairs: BTreeMap<(Item, Item), usize> = BTreeMap::new();
    for &(a, b) in edges {
        let (ia, ib) = (lift(a), lift(b));
        if ia == ib {
            continue;
        }
        let key = if pairs.contains_key(&(ib, ia)) {
            (ib, ia)
        } else {
            (ia, ib)
        };
        *pairs.entry(key).or_default() += 1;
    }
    let lines = route(&pairs, &rect_of);
    let width = cards
        .iter()
        .map(|c| c.rect.right())
        .chain(frames.iter().map(|f| f.1.right()))
        .chain(lines.iter().map(|l| l.bounds.right()))
        .fold(0., f32::max)
        + FRAME_PAD;
    let height = cards
        .iter()
        .map(|c| c.rect.bottom())
        .chain(frames.iter().map(|f| f.1.bottom()))
        .fold(0., f32::max);
    Layout {
        width,
        height,
        frames,
        cards,
        lines,
    }
}

/// The gap left of column `g`: where its lanes may go, frame to frame
/// with a margin.
fn gap_span(g: usize) -> (f32, f32) {
    let right = col_x(g) - 6.;
    (right - (COL_GAP - 12.), right)
}

/// The gap a card's right edge `x` faces (the one left of the next column).
fn gap_after(x: f32) -> usize {
    (x / (COL_W + COL_GAP)).floor().max(0.) as usize + 1
}

/// The gap a card's left edge `x` faces.
fn gap_before(x: f32) -> usize {
    (x / (COL_W + COL_GAP)).round().max(0.) as usize
}

/// Routes one line per pair of cards so no two share a stretch.
///
/// Each card spreads its lines evenly along the edge they leave from,
/// ordered by where the other end sits, so lines leaving one card do not
/// cross at the card. Each line turns in a gap between columns, and every
/// line in a gap gets a lane of its own: brackets (two cards in one
/// column) nearest the column they join, shortest innermost; then the lines
/// that enter the next column, longest first, so a line's horizontal run
/// does not cross the lanes of lines that turn after it.
fn route(pairs: &BTreeMap<(Item, Item), usize>, rect_of: &HashMap<Item, Rect>) -> Vec<Line> {
    #[derive(Clone, Copy, PartialEq)]
    enum Kind {
        /// Both ends on their cards' right edges.
        Bracket,
        /// `a` is left of `b`: out of `a`'s right edge into `b`'s left.
        Right,
        /// `a` is right of `b`: out of `a`'s left edge into `b`'s right.
        Left,
    }
    struct Plan {
        a: Item,
        b: Item,
        n: usize,
        ra: Rect,
        rb: Rect,
        kind: Kind,
        ya: f32,
        yb: f32,
        gap: usize,
        lane: f32,
    }
    let centre = |r: &Rect| r.y + r.h.min(CARD_H) / 2.;
    let mut plans: Vec<Plan> = Vec::new();
    for (&(a, b), &n) in pairs {
        let (Some(&ra), Some(&rb)) = (rect_of.get(&a), rect_of.get(&b)) else {
            continue;
        };
        let overlap = ra.x < rb.right() && rb.x < ra.right();
        let (kind, gap) = if overlap {
            (Kind::Bracket, gap_after(ra.right().max(rb.right())))
        } else if ra.x < rb.x {
            (Kind::Right, gap_before(rb.x))
        } else {
            (Kind::Left, gap_before(ra.x))
        };
        plans.push(Plan {
            a,
            b,
            n,
            ra,
            rb,
            kind,
            ya: centre(&ra),
            yb: centre(&rb),
            gap,
            lane: 0.,
        });
    }
    // Ports: each (card, edge) spreads its lines, ordered by the other end.
    // (card, right edge) → (the other end's y, plan, this is the plan's `a`).
    type Ends = HashMap<(Item, bool), Vec<(f32, usize, bool)>>;
    let mut ends: Ends = HashMap::new();
    for (k, p) in plans.iter().enumerate() {
        let (a_right, b_right) = match p.kind {
            Kind::Bracket => (true, true),
            Kind::Right => (true, false),
            Kind::Left => (false, true),
        };
        ends.entry((p.a, a_right))
            .or_default()
            .push((centre(&p.rb), k, true));
        ends.entry((p.b, b_right))
            .or_default()
            .push((centre(&p.ra), k, false));
    }
    for ((item, _), mut list) in ends {
        let Some(r) = rect_of.get(&item) else {
            continue;
        };
        list.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)));
        let n = list.len();
        let (lo, hi) = (r.y + 10., r.bottom() - 10.);
        let step = if n <= 1 {
            0.
        } else {
            (7f32).min((hi - lo) / (n - 1) as f32)
        };
        let half = step * (n.saturating_sub(1)) as f32 / 2.;
        let mid = centre(r).clamp(lo + half, (hi - half).max(lo + half));
        for (k, (_, plan, is_a)) in list.into_iter().enumerate() {
            let y = mid - half + k as f32 * step;
            if is_a {
                plans[plan].ya = y;
            } else {
                plans[plan].yb = y;
            }
        }
    }
    // Lanes: one per line in each gap.
    let mut by_gap: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (k, p) in plans.iter().enumerate() {
        by_gap.entry(p.gap).or_default().push(k);
    }
    for (gap, mut members) in by_gap {
        let span = |k: &usize| (plans[*k].ya - plans[*k].yb).abs();
        members.sort_by(|x, y| {
            let (bx, by) = (
                plans[*x].kind == Kind::Bracket,
                plans[*y].kind == Kind::Bracket,
            );
            by.cmp(&bx)
                .then_with(|| {
                    if bx {
                        span(x).total_cmp(&span(y))
                    } else {
                        span(y).total_cmp(&span(x))
                    }
                })
                .then(x.cmp(y))
        });
        let (lo, hi) = gap_span(gap);
        let n = members.len();
        let step = if n <= 1 {
            0.
        } else {
            (8f32).min((hi - lo) / (n - 1) as f32)
        };
        let start = (lo + hi) / 2. - step * (n.saturating_sub(1)) as f32 / 2.;
        for (k, m) in members.into_iter().enumerate() {
            plans[m].lane = start + k as f32 * step;
        }
    }
    plans
        .into_iter()
        .map(|p| {
            let x = p.lane;
            let points = match p.kind {
                Kind::Bracket => vec![
                    (p.ra.right(), p.ya),
                    (x, p.ya),
                    (x, p.yb),
                    (p.rb.right(), p.yb),
                ],
                Kind::Right => vec![(p.ra.right(), p.ya), (x, p.ya), (x, p.yb), (p.rb.x, p.yb)],
                Kind::Left => vec![(p.ra.x, p.ya), (x, p.ya), (x, p.yb), (p.rb.right(), p.yb)],
            };
            let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
            for &(x, y) in &points {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
            Line {
                from: p.a,
                to: p.b,
                points,
                bounds: Rect::new(x0 - 6., y0 - 6., x1 - x0 + 12., y1 - y0 + 12.),
                links: p.n,
            }
        })
        .collect()
}

/// A synthetic map of `n` notes in `folders` folders with `links` links,
/// for tests and the app's frame-time check. No file is read.
pub fn synthetic(n: usize, n_folders: usize, links: usize) -> NotesMap {
    let mut notes = Vec::new();
    for i in 0..n {
        let folder = if i == 0 {
            String::new()
        } else {
            format!("Folder {}", i % n_folders.max(1))
        };
        notes.push(MapNote {
            path: PathBuf::from(format!("notes/{folder}/Note {i}.md")),
            title: format!("Note {i}"),
            folder,
            summary: format!("What note {i} says, in one sentence that runs on a little."),
            reads_week: (i * 7 % 23) as u32,
            reads_by_agent: Vec::new(),
            flags: Flags {
                entry: i == 0,
                fixed: i % 17 == 3,
                waiting: usize::from(i % 13 == 5),
                rejected: i % 29 == 7,
                stale_days: (i % 11 == 2).then_some(41),
                required: false,
            },
            rules: if i % 3 == 0 {
                vec![format!("Rule kept in note {i}.")]
            } else {
                Vec::new()
            },
            pending: Vec::new(),
            evidence: Vec::new(),
        });
    }
    let mut edges = BTreeSet::new();
    let mut s: u64 = 0x9E37_79B9_7F4A_7C15;
    while edges.len() < links.min(n * (n - 1) / 2) {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        let a = (s % n as u64) as usize;
        let b = ((s >> 32) % n as u64) as usize;
        if a != b && !edges.contains(&(b, a)) {
            edges.insert((a, b));
        }
    }
    let edges: Vec<(usize, usize)> = edges.into_iter().collect();
    let folders = folders(&notes);
    let (out, mid, inner) = lay_out(&notes, &folders, &edges);
    NotesMap {
        kicker: format!("Brain · {n} notes"),
        headline: "A synthetic map".into(),
        lead: String::new(),
        notes,
        folders,
        edges,
        out,
        mid,
        inner,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_follow_the_zoom() {
        assert_eq!(Level::of(0.3), Level::Out);
        assert_eq!(Level::of(1.0), Level::Mid);
        assert_eq!(Level::of(2.0), Level::In);
    }

    #[test]
    fn cards_never_overlap() {
        let m = synthetic(300, 12, 1000);
        for l in [&m.mid, &m.inner, &m.out] {
            for (i, a) in l.cards.iter().enumerate() {
                for b in &l.cards[i + 1..] {
                    assert!(!a.rect.meets(&b.rect), "{a:?} meets {b:?}");
                }
            }
        }
        assert_eq!(m.mid.cards.len(), 300);
        assert_eq!(m.inner.cards.len(), 300);
        assert_eq!(m.out.cards.len(), 1 + m.folders.len());
    }

    /// Each line's vertical run: its x and its y range.
    fn runs(lay: &Layout) -> Vec<(f32, f32, f32)> {
        lay.lines
            .iter()
            .map(|l| {
                let (x, ya, yb) = (l.points[1].0, l.points[1].1, l.points[2].1);
                (x, ya.min(yb), ya.max(yb))
            })
            .collect()
    }

    #[test]
    fn no_two_links_share_a_lane() {
        // One hub note linked from many, as START HERE is.
        let mut m = synthetic(60, 5, 0);
        m.edges = (1..40).map(|i| (0, i)).collect();
        let (_, mid, inner) = lay_out(&m.notes, &m.folders, &m.edges);
        for lay in [&mid, &inner] {
            let r = runs(lay);
            for (i, a) in r.iter().enumerate() {
                for b in &r[i + 1..] {
                    let meets = a.1 < b.2 && b.1 < a.2;
                    assert!(
                        !meets || (a.0 - b.0).abs() >= 1.,
                        "two vertical runs share x {}: {a:?} {b:?}",
                        a.0
                    );
                }
            }
        }
    }

    #[test]
    fn no_two_links_share_a_port() {
        let mut m = synthetic(60, 5, 0);
        m.edges = (1..40).map(|i| (0, i)).collect();
        let (_, mid, _) = lay_out(&m.notes, &m.folders, &m.edges);
        let mut seen: Vec<(f32, f32)> = Vec::new();
        for l in &mid.lines {
            for end in [l.points[0], l.points[3]] {
                if l.from == Item::Note(0) && end == l.points[0] {
                    assert!(
                        seen.iter()
                            .all(|s| (s.0 - end.0).abs() > 0.5 || (s.1 - end.1).abs() >= 1.),
                        "two lines leave the hub at {end:?}"
                    );
                    seen.push(end);
                }
            }
        }
        assert_eq!(seen.len(), 39);
    }

    #[test]
    fn lanes_stay_in_their_gap() {
        let m = synthetic(300, 12, 1000);
        for l in &m.mid.lines {
            let x = l.points[1].0;
            // Never inside a column of cards.
            let c = (x / (COL_W + COL_GAP)).floor();
            let inside = x - c * (COL_W + COL_GAP);
            assert!(inside > COL_W, "lane at {x} is inside column {c}");
        }
    }

    #[test]
    fn every_link_is_a_line_with_right_angles() {
        let m = synthetic(40, 4, 60);
        assert_eq!(m.mid.lines.len(), 60);
        for l in &m.mid.lines {
            assert_eq!(l.points.len(), 4);
            for w in l.points.windows(2) {
                let (a, b) = (w[0], w[1]);
                assert!(a.0 == b.0 || a.1 == b.1, "{a:?} {b:?}");
            }
        }
        // At Out, links inside one folder vanish and the rest join folders.
        assert!(m.out.lines.len() < m.mid.lines.len());
        assert!(m.out.lines.iter().map(|l| l.links).sum::<usize>() <= 60);
    }

    #[test]
    fn out_folds_folders_into_their_frames() {
        let m = synthetic(40, 4, 10);
        for (f, frame) in &m.mid.frames {
            let card = m.out.card(Item::Folder(*f)).expect("folder card");
            assert_eq!(card.rect, *frame);
        }
    }

    #[test]
    fn the_anchor_keeps_the_same_spot_of_the_same_card() {
        let m = synthetic(40, 4, 10);
        let c = m.mid.card(Item::Note(5)).unwrap().rect;
        let (x, y) = (c.x + c.w * 0.25, c.y + c.h * 0.5);
        let (ix, iy) = m.anchor(Level::Mid, Level::In, x, y).unwrap();
        let ci = m.inner.card(Item::Note(5)).unwrap().rect;
        assert!((ix - (ci.x + ci.w * 0.25)).abs() < 0.01);
        assert!((iy - (ci.y + ci.h * 0.5)).abs() < 0.01);
        // Into Out, a note's spot lands in its folder.
        let (ox, oy) = m.anchor(Level::Mid, Level::Out, x, y).unwrap();
        let f = m.folders.iter().position(|f| f.notes.contains(&5)).unwrap();
        assert!(m.out.card(Item::Folder(f)).unwrap().rect.contains(ox, oy));
    }

    #[test]
    fn culling_keeps_only_what_meets_the_view() {
        let m = synthetic(300, 12, 1000);
        let view = Rect::new(0., 0., 1100., 700.);
        let shown = m.mid.cards_in(&view).count();
        assert!(shown > 0 && shown < 60, "{shown}");
        assert!(m.mid.lines_in(&view).count() < m.mid.lines.len());
    }

    #[test]
    fn fit_shows_the_whole_map() {
        let m = synthetic(300, 12, 1000);
        let (z, (ox, oy)) = m.mid.fit(1100., 700., 24.);
        assert!((MIN_ZOOM..=1.0).contains(&z));
        assert!(ox <= 0. && oy <= 0.);
        assert!(m.mid.width * z <= 1100.);
    }
}
