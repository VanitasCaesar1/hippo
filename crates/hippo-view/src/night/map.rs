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
//! - [`Level::Out`]: an overview. Each pinned note and each folder is one
//!   card a column wide: its name set large enough to read when zoomed
//!   out (wrapped, then shrunk to fit, never cut), a legend of what its
//!   notes carry, and its most-read notes as labelled rows with a small
//!   mark each. The cards stack in columns with nothing spanning two.
//! - [`Level::In`]: the same columns and order as Mid, with taller cards
//!   that carry each note's rules and evidence.
//!
//! Every word a level draws on the map is a [`Label`] here, already fitted
//! to its box ([`super::fit`]), so the app and the web replica only draw.
//!
//! Links are polylines that turn only in the gaps between columns, so none
//! runs through a card (docs/designs/brain-links.md): out of one card's
//! edge, a turn in the gap beside the other card, into its edge; two cards
//! in one column are joined by a bracket at their right. A line whose
//! straight run would cross a card steps, in the gap before that card's
//! column, to a free height there, and on. A level never shows more than
//! [`LINE_BUDGET`] lines at once: one with more draws only the lines of the
//! card in focus (hovered, else selected), which leave the card as one
//! trunk each way and branch off beside each column they reach.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use hippo_core::config::Workspace;
use hippo_core::lessons::{Lesson, Status};
use hippo_core::notes::{self, Link as NoteLink, LintFinding};
use hippo_core::rollup::Rollup;
use serde::{Deserialize, Serialize};
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

/// An Out card's padding.
pub const OUT_PAD: f32 = 28.;
/// An Out card's name: its largest and smallest size, on at most 2 lines.
pub const OUT_TITLE: f32 = 48.;
pub const OUT_TITLE_MIN: f32 = 22.;
/// The legend's size.
pub const OUT_META: f32 = 24.;
/// A note row in a folder card: its text size and its height.
pub const OUT_ROW: f32 = 26.;
pub const OUT_ROW_H: f32 = 44.;
/// At most this many note rows in a folder card; more show as `+ n more`.
pub const OUT_ROWS: usize = 6;
/// A mark's diameter.
pub const OUT_DOT: f32 = 12.;
/// An Out card's icon, as a share of its name's size.
pub const OUT_ICON: f32 = 0.8;
/// Note rows in a folder card on a map of more blocks than
/// [`OUT_FEW`]: its overview is drawn smaller, so it lists fewer.
pub const OUT_ROWS_MANY: usize = 3;
/// Below this zoom an Out card's note rows are too small to read and are
/// not drawn (about 8 points on screen); below the next, its legend too.
pub const OUT_ROWS_BELOW: f32 = 0.33;
pub const OUT_META_BELOW: f32 = 0.27;
pub const OUT_FEW: usize = 8;
/// Between Out cards in a column.
pub const OUT_GAP: f32 = 72.;
/// A line of text is this many times its size high.
pub const LEADING: f32 = 1.25;
/// Points around a map fitted to its view.
pub const FIT_PAD: f32 = 28.;
/// The largest zoom a map fitted at Out takes: just below Mid.
pub const OUT_FIT: f32 = 0.54;
/// At most this many lines show at once, at any zoom: a level with more
/// pairs of linked cards draws only the lines of the card in focus.
pub const LINE_BUDGET: usize = 12;

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
    /// Lines with one trunk leave their card together and split in the gap
    /// beside their targets' column: they share their points up to there.
    /// A line on its own has a trunk of its own.
    pub trunk: usize,
}

/// What a label is, which sets its weight and colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LabelKind {
    /// A card's name: an Out card's, or a note card's title (bold).
    Title,
    /// An Out card's legend: counts and what a pinned note carries.
    Meta,
    /// A note's title in a folder card at Out.
    Row,
    /// `+ 8 more notes`.
    More,
    /// A folder frame's name at Mid and In (bold).
    Frame,
    /// A folder frame's count.
    Count,
}

impl LabelKind {
    pub fn bold(self) -> bool {
        matches!(self, LabelKind::Title | LabelKind::Frame)
    }
}

/// Text a level draws: its lines from `rect`'s top left, `size` high
/// (in map units), each line [`LEADING`] × `size` below the last. Every
/// line fits `rect.w` by [`super::fit::width`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Label {
    /// The card or frame it belongs to.
    pub item: Item,
    pub kind: LabelKind,
    pub rect: Rect,
    pub lines: Vec<String>,
    pub size: f32,
}

/// A small round mark: a note's state on an Out card, or a legend's key.
/// `tone` is `ok` (fixed), `bad` (rejected), `you` (a rule waiting),
/// `faint` (stale), `night` (in every session) or `plain`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Mark {
    pub item: Item,
    /// The centre and the diameter.
    pub x: f32,
    pub y: f32,
    pub d: f32,
    pub tone: &'static str,
    /// A note row's mark in a folder card (dropped first when zoomed far
    /// out), not the legend's or the card's icon.
    pub row: bool,
}

/// One level's drawing.
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct Layout {
    pub width: f32,
    pub height: f32,
    /// Folder frames (Mid and In); empty at Out, where the folder is a card.
    pub frames: Vec<(usize, Rect)>,
    pub cards: Vec<Card>,
    /// Each linked card's lines, bundled, drawn while it is hovered or
    /// selected; no line is drawn at rest.
    pub focus: Vec<(Item, Vec<Line>)>,
    /// Every word drawn on the map at this level, on top of the cards.
    pub labels: Vec<Label>,
    pub marks: Vec<Mark>,
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

    /// The lines drawn while `focus` is hovered or selected: its own,
    /// bundled; none without a focus.
    pub fn shown(&self, focus: Option<Item>) -> &[Line] {
        focus
            .and_then(|f| self.focus.iter().find(|(i, _)| *i == f))
            .map_or(&[], |(_, ls)| ls.as_slice())
    }

    /// How many lines the eye sees with `focus`: a trunk and its branches
    /// count once.
    pub fn visible(&self, focus: Option<Item>) -> usize {
        let trunks: BTreeSet<usize> = self.shown(focus).iter().map(|l| l.trunk).collect();
        trunks.len()
    }

    /// The zoom at which the whole layout just fits a `w` × `h` view
    /// with `pad` points around it, unclamped.
    pub fn fit_zoom(&self, w: f32, h: f32, pad: f32) -> f32 {
        let zx = (w - 2. * pad).max(1.) / self.width.max(1.);
        let zy = (h - 2. * pad).max(1.) / self.height.max(1.);
        zx.min(zy)
    }

    /// The zoom and the map point at the view's top left that fit the whole
    /// layout in a `w` × `h` view with `pad` points around it, at most
    /// `most`.
    pub fn fit(&self, w: f32, h: f32, pad: f32) -> (f32, (f32, f32)) {
        self.fit_at_most(w, h, pad, 1.0)
    }

    fn fit_at_most(&self, w: f32, h: f32, pad: f32, most: f32) -> (f32, (f32, f32)) {
        let z = self.fit_zoom(w, h, pad).min(most).max(MIN_ZOOM);
        // Centred both ways; a map larger than the view at the smallest
        // zoom starts `pad` from its left and top.
        let ox = -((w / z - self.width) / 2.).max(pad / z);
        let oy = -((h / z - self.height) / 2.).max(pad / z);
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
    /// The arrangement the layouts are made from, and the one hippo
    /// chooses.
    pub arrangement: Arrangement,
    pub default_arrangement: Arrangement,
}

impl NotesMap {
    /// The same map laid out from `arr` (settled first).
    pub fn arranged(&self, arr: &Arrangement) -> NotesMap {
        let arrangement = settle(&self.notes, &self.folders, arr);
        let (out, mid, inner) = lay_out_in(&self.notes, &self.folders, &self.edges, &arrangement);
        NotesMap {
            out,
            mid,
            inner,
            arrangement,
            ..self.clone()
        }
    }

    /// Whether the layout is a person's, not the default.
    pub fn is_arranged(&self) -> bool {
        self.arrangement != self.default_arrangement
    }

    /// The map for a `w` × `h` view: hippo's layout made with the number
    /// of columns that shows the whole map largest there
    /// ([`view_arrangement`]). A person's arrangement is kept as it is.
    pub fn fitted(&self, w: f32, h: f32) -> NotesMap {
        if self.is_arranged() {
            return self.clone();
        }
        let arr = view_arrangement(&self.notes, &self.folders, w, h);
        let (out, mid, inner) = lay_out_in(&self.notes, &self.folders, &self.edges, &arr);
        NotesMap {
            out,
            mid,
            inner,
            default_arrangement: arr.clone(),
            arrangement: arr,
            ..self.clone()
        }
    }

    /// The zoom and the map point at the view's top left that show the
    /// whole map in a `w` × `h` view: at Mid when it fits there at
    /// [`OUT_BELOW`] or more (at most 100 %), else at Out, centred.
    pub fn fit(&self, w: f32, h: f32) -> (f32, (f32, f32)) {
        if self.mid.fit_zoom(w, h, FIT_PAD) >= OUT_BELOW {
            self.mid.fit(w, h, FIT_PAD)
        } else {
            self.out.fit_at_most(w, h, FIT_PAD, OUT_FIT)
        }
    }

    /// The block a card stands for: a folder for a note in one.
    pub fn block_of(&self, item: Item) -> Block {
        match item {
            Item::Folder(f) => Block::Folder(f),
            Item::Note(i) if self.pinned(i) => Block::Note(i),
            Item::Note(i) => self
                .folders
                .iter()
                .position(|f| f.notes.contains(&i))
                .map_or(Block::Note(i), Block::Folder),
        }
    }

    /// A block's key in an [`Arrangement`].
    pub fn key(&self, b: Block) -> String {
        block_key(&self.notes, &self.folders, b)
    }

    /// A block's outline at `level`: a folder's frame (Mid, In) or card
    /// (Out), a pinned note's card.
    pub fn block_rect(&self, level: Level, b: Block) -> Option<Rect> {
        self.block_rect_in(self.layout(level), b)
    }

    /// A block's outline in `lay`, one of this map's levels or a
    /// [`NotesMap::preview`] of one.
    pub fn block_rect_in(&self, lay: &Layout, b: Block) -> Option<Rect> {
        match b {
            Block::Note(i) => lay.card(Item::Note(i)).map(|c| c.rect),
            Block::Folder(f) => lay
                .frames
                .iter()
                .find(|x| x.0 == f)
                .map(|x| x.1)
                .or_else(|| lay.card(Item::Folder(f)).map(|c| c.rect)),
        }
    }

    /// `level` laid out from `arr` (settled first): what a drag shows
    /// while the pointer moves, made only when the place it would land
    /// changes.
    pub fn preview(&self, arr: &Arrangement, level: Level) -> Layout {
        let arr = settle(&self.notes, &self.folders, arr);
        let plan = plan(&self.notes, &self.folders);
        pack(level, &self.notes, &self.folders, &self.edges, &arr, plan)
    }

    /// The block whose handle is under a map point at `level`: at Out any
    /// card; at Mid and In a folder's name band or a pinned note's card.
    pub fn handle_at(&self, level: Level, x: f32, y: f32) -> Option<Block> {
        let lay = self.layout(level);
        if level == Level::Out {
            return lay.card_at(x, y).map(|c| self.block_of(c.item));
        }
        if let Some(c) = lay.card_at(x, y) {
            return match c.item {
                Item::Note(i) if self.pinned(i) => Some(Block::Note(i)),
                _ => None,
            };
        }
        lay.frames
            .iter()
            .find(|(_, r)| r.contains(x, y) && y < r.y + FRAME_HEAD)
            .map(|(f, _)| Block::Folder(*f))
    }

    /// The block a key names.
    pub fn block(&self, key: &str) -> Option<Block> {
        block_named(&self.notes, &self.folders, key)
    }

    /// Where a block dropped at map point `(x, y)` of `level` lands: its
    /// column and place among that column's other blocks, and where the
    /// drop line goes. A point right of the last column is a new column.
    pub fn drop_at(&self, level: Level, b: Block, x: f32, y: f32) -> Drop {
        let key = self.key(b);
        let n = self.arrangement.columns.len();
        let col = ((x + COL_GAP / 2.) / (COL_W + COL_GAP)).floor().max(0.) as usize;
        let col = col.min(n);
        let others: Vec<Rect> = self
            .arrangement
            .columns
            .get(col)
            .map(|c| {
                c.iter()
                    .filter(|k| **k != key)
                    .filter_map(|k| block_named(&self.notes, &self.folders, k))
                    .filter_map(|o| self.block_rect(level, o))
                    .collect()
            })
            .unwrap_or_default();
        let at = others.iter().filter(|r| r.y + r.h / 2. < y).count();
        let line_y = match (at, others.len()) {
            (_, 0) => 0.,
            (0, _) => others[0].y - FRAME_GAP / 2.,
            (k, _) => others[k - 1].bottom() + FRAME_GAP / 2.,
        };
        Drop {
            col,
            at,
            y: line_y,
            x: col_x(col),
            new_column: col == n,
        }
    }
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
    let arrangement = default_arrangement(&notes_out, &folders);
    let (out, mid, inner) = lay_out_in(&notes_out, &folders, &edges, &arrangement);

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
    if edges.is_empty() {
        lead.push_str("No note links to another yet.");
    }
    let lead = lead.trim_end().to_owned();
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
        default_arrangement: arrangement.clone(),
        arrangement,
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

/// An In card's words, wrapped to its width: the summary, each rule and
/// each piece of evidence on up to three lines.
#[derive(Debug, Clone, PartialEq)]
pub struct InText {
    pub summary: Vec<String>,
    pub rows: Vec<(Vec<String>, &'static str)>,
}

/// Where an In card's summary starts, from its top; its line height; the
/// space between rows.
pub const IN_SUMMARY: f32 = 50.;
pub const IN_ROW_GAP: f32 = 4.;
/// An In card's summary and row sizes.
pub const IN_SUMMARY_SIZE: f32 = 13.5;
pub const IN_ROW_SIZE: f32 = 13.;

/// An In card's words, wrapped.
pub fn in_text(n: &MapNote) -> InText {
    let w = CARD_W - 32.;
    let summary = super::fit::fit(&n.summary, w, IN_SUMMARY_SIZE, IN_SUMMARY_SIZE, 3, false).lines;
    let rows = n
        .in_lines()
        .into_iter()
        .map(|(s, kind)| {
            (
                super::fit::fit(&s, w - 22., IN_ROW_SIZE, IN_ROW_SIZE, 3, false).lines,
                kind,
            )
        })
        .collect();
    InText { summary, rows }
}

/// Where an In card's chips sit, from its top.
pub fn in_chips_y(t: &InText) -> f32 {
    IN_SUMMARY + t.summary.len().max(1) as f32 * IN_LINE + 5.
}

/// An In card's height: its summary, its chips, its rules and evidence,
/// and as much room below as above.
pub fn in_height(n: &MapNote) -> f32 {
    let t = in_text(n);
    let mut h = in_chips_y(&t) + 22.;
    if !t.rows.is_empty() {
        h += 14.
            + t.rows
                .iter()
                .map(|r| r.0.len() as f32 * IN_LINE)
                .sum::<f32>()
            + (t.rows.len() - 1) as f32 * IN_ROW_GAP;
    }
    (h + 16.).max(CARD_H)
}

/// A person's order of the map: its columns left to right, each its
/// blocks top to bottom, by key (`note:<path>` for a note read in every
/// session, `folder:<name>` for a folder; see [`block_key`]). Every level
/// is laid out from one, so a block keeps its column at every zoom.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Arrangement {
    pub columns: Vec<Vec<String>>,
}

/// What can be arranged: a note read in every session, or a folder (with
/// its notes, which stay in it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Block {
    Note(usize),
    Folder(usize),
}

/// Where a dragged block lands: its column and its place among that
/// column's other blocks; the drop line's y and its column's left edge (it
/// is a column wide); and whether the column is a new one at the right.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Drop {
    pub col: usize,
    pub at: usize,
    pub y: f32,
    pub x: f32,
    pub new_column: bool,
}

/// A one-key move of a block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nudge {
    Left,
    Right,
    Up,
    Down,
}

/// A block's key in an [`Arrangement`].
pub fn block_key(notes: &[MapNote], folders: &[Folder], b: Block) -> String {
    match b {
        Block::Note(i) => format!("note:{}", notes[i].path.display()),
        Block::Folder(f) => format!("folder:{}", folders[f].name),
    }
}

impl Arrangement {
    /// Where `key` is: its column and its place in it.
    pub fn find(&self, key: &str) -> Option<(usize, usize)> {
        self.columns
            .iter()
            .enumerate()
            .find_map(|(c, col)| col.iter().position(|k| k == key).map(|r| (c, r)))
    }

    /// Every key, column by column.
    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.columns.iter().flatten()
    }

    /// `key` moved to column `col` at place `at` (counted among the
    /// column's other blocks). A column past the last makes a new one at
    /// the right; a column left empty goes.
    pub fn move_to(&self, key: &str, col: usize, at: usize) -> Arrangement {
        let mut columns = self.columns.clone();
        if self.find(key).is_none() {
            return self.clone();
        }
        for c in &mut columns {
            c.retain(|k| k != key);
        }
        if col >= columns.len() {
            columns.push(vec![key.to_owned()]);
        } else {
            let at = at.min(columns[col].len());
            columns[col].insert(at, key.to_owned());
        }
        columns.retain(|c| !c.is_empty());
        Arrangement { columns }
    }

    /// `key` moved one column left or right (keeping its place where the
    /// column has room), or one place up or down.
    pub fn nudge(&self, key: &str, n: Nudge) -> Arrangement {
        let Some((c, r)) = self.find(key) else {
            return self.clone();
        };
        match n {
            Nudge::Left if c > 0 => self.move_to(key, c - 1, r),
            Nudge::Right if !(c + 1 == self.columns.len() && self.columns[c].len() == 1) => {
                self.move_to(key, c + 1, r)
            }
            Nudge::Up if r > 0 => self.move_to(key, c, r - 1),
            Nudge::Down if r + 1 < self.columns[c].len() => self.move_to(key, c, r + 1),
            _ => self.clone(),
        }
    }
}

/// The notes read in every session, the entry note first.
fn pinned_of(notes: &[MapNote]) -> Vec<usize> {
    let mut p: Vec<usize> = (0..notes.len())
        .filter(|i| notes[*i].flags.entry || notes[*i].flags.required)
        .collect();
    p.sort_by_key(|i| (!notes[*i].flags.entry, notes[*i].title.clone()));
    p
}

/// How many columns hippo chooses, and how many notes a folder's frame
/// holds per column at Mid and In.
#[derive(Debug, Clone, Copy)]
struct Plan {
    cols: usize,
    per_col: usize,
}

fn plan(notes: &[MapNote], folders: &[Folder]) -> Plan {
    let n_notes = folders.iter().map(|f| f.notes.len()).sum::<usize>().max(1);
    let pinned = pinned_of(notes).len();
    // Columns for a map about 1.6 times as wide as it is high.
    let cols = ((0.49 * (n_notes + pinned) as f32).sqrt().round() as usize).clamp(2, 24);
    let per_col = n_notes.div_ceil(cols.saturating_sub(1).max(1)).max(4);
    Plan { cols, per_col }
}

/// The columns a folder's frame spans at Mid and In.
fn span_of(f: &Folder, plan: Plan) -> usize {
    f.notes.len().div_ceil(plan.per_col).clamp(1, plan.cols)
}

/// The layout hippo chooses when it knows no view: see
/// [`default_arrangement_in`], with the columns of a map about 1.6 times as
/// wide as it is high at Mid.
pub fn default_arrangement(notes: &[MapNote], folders: &[Folder]) -> Arrangement {
    default_arrangement_in(notes, folders, plan(notes, folders).cols)
}

/// The layout hippo chooses in `cols` columns: the notes in every session
/// in the first column, then each folder, largest first, into the run of
/// columns whose tallest is lowest at Mid. A folder spans as many columns
/// at Mid as [`plan`] gives it, whatever `cols` is.
pub fn default_arrangement_in(notes: &[MapNote], folders: &[Folder], cols: usize) -> Arrangement {
    let plan = Plan {
        cols: cols.max(1),
        ..plan(notes, folders)
    };
    let base = self::plan(notes, folders);
    let pinned = pinned_of(notes);
    let mut columns: Vec<Vec<String>> = vec![Vec::new(); plan.cols];
    let mut heights = vec![0f32; plan.cols];
    for &i in &pinned {
        columns[0].push(block_key(notes, folders, Block::Note(i)));
    }
    if !pinned.is_empty() {
        heights[0] = pinned.len() as f32 * (CARD_H + GAP) - GAP + FRAME_GAP;
    }
    // The notes read in every session keep the first column to themselves
    // when there is another, so folder cards line up across columns.
    let first = usize::from(!pinned.is_empty() && plan.cols >= 2);
    for (fi, f) in folders.iter().enumerate() {
        let span = span_of(f, base);
        let per = f.notes.len().div_ceil(span);
        let span = span.min(plan.cols - first).max(1);
        let mut best = (first, f32::MAX);
        for c in first..=(plan.cols - span) {
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
        columns[col].push(block_key(notes, folders, Block::Folder(fi)));
    }
    columns.retain(|c| !c.is_empty());
    Arrangement { columns }
}

/// The arrangement that shows the whole map largest in a `w` × `h` view:
/// hippo's layout ([`default_arrangement_in`]) in each number of columns
/// from one to one per block, laid out; the one whose Mid fits at
/// [`OUT_BELOW`] or more at the largest zoom, else the one whose Out fits
/// at the largest zoom (the fewest columns on a tie).
pub fn view_arrangement(notes: &[MapNote], folders: &[Folder], w: f32, h: f32) -> Arrangement {
    let plan = plan(notes, folders);
    let all = blocks(notes, folders);
    let drawn = |level: Level| -> HashMap<Block, Drawn> {
        all.iter()
            .map(|&b| (b, draw(level, b, notes, folders, plan)))
            .collect()
    };
    let (out, mid) = (drawn(Level::Out), drawn(Level::Mid));
    let zoom = |level: Level, d: &HashMap<Block, Drawn>, arr: &Arrangement| {
        let (frames, cards, _, _) = place(level, notes, folders, arr, &|b| d[&b].clone());
        finish(frames, cards, Vec::new(), Vec::new()).fit_zoom(w, h, FIT_PAD)
    };
    let mut best: Option<(bool, f32, Arrangement)> = None;
    for k in 1..=all.len().max(1) {
        let arr = default_arrangement_in(notes, folders, k);
        let zm = zoom(Level::Mid, &mid, &arr);
        let (at_mid, z) = if zm >= OUT_BELOW {
            (true, zm)
        } else {
            (false, zoom(Level::Out, &out, &arr))
        };
        let better = match &best {
            None => true,
            Some((m, bz, _)) => (at_mid && !m) || (at_mid == *m && z > bz + 1e-4),
        };
        if better {
            best = Some((at_mid, z, arr));
        }
    }
    best.map_or_else(|| default_arrangement(notes, folders), |b| b.2)
}

/// `arr` holding every block once: keys that name no block dropped (a
/// folder renamed or gone), a block it lacks at the foot of its default
/// column, empty columns dropped.
pub fn settle(notes: &[MapNote], folders: &[Folder], arr: &Arrangement) -> Arrangement {
    let known: BTreeSet<String> = blocks(notes, folders)
        .into_iter()
        .map(|b| block_key(notes, folders, b))
        .collect();
    let mut seen = BTreeSet::new();
    let mut columns: Vec<Vec<String>> = arr
        .columns
        .iter()
        .map(|c| {
            c.iter()
                .filter(|k| known.contains(*k) && seen.insert((*k).clone()))
                .cloned()
                .collect()
        })
        .collect();
    let default = default_arrangement(notes, folders);
    for (dc, col) in default.columns.iter().enumerate() {
        for k in col {
            if !seen.contains(k) {
                while columns.len() <= dc {
                    columns.push(Vec::new());
                }
                columns[dc].push(k.clone());
                seen.insert(k.clone());
            }
        }
    }
    columns.retain(|c| !c.is_empty());
    Arrangement { columns }
}

/// Every block: the notes in every session, then the folders.
fn blocks(notes: &[MapNote], folders: &[Folder]) -> Vec<Block> {
    pinned_of(notes)
        .into_iter()
        .map(Block::Note)
        .chain((0..folders.len()).map(Block::Folder))
        .collect()
}

/// Lays out all three levels the way hippo chooses.
pub fn lay_out(
    notes: &[MapNote],
    folders: &[Folder],
    edges: &[(usize, usize)],
) -> (Layout, Layout, Layout) {
    lay_out_in(notes, folders, edges, &default_arrangement(notes, folders))
}

/// Lays out all three levels from an arrangement (settled first).
pub fn lay_out_in(
    notes: &[MapNote],
    folders: &[Folder],
    edges: &[(usize, usize)],
    arr: &Arrangement,
) -> (Layout, Layout, Layout) {
    let arr = settle(notes, folders, arr);
    let plan = plan(notes, folders);
    let go = |level| pack(level, notes, folders, edges, &arr, plan);
    (go(Level::Out), go(Level::Mid), go(Level::In))
}

fn col_x(c: usize) -> f32 {
    c as f32 * (COL_W + COL_GAP)
}

/// A label for `item` fitted into `w` from `(x, y)`.
#[allow(clippy::too_many_arguments)]
fn label(
    item: Item,
    kind: LabelKind,
    s: &str,
    x: f32,
    y: f32,
    w: f32,
    max: f32,
    min: f32,
    lines: usize,
) -> Label {
    let f = super::fit::fit(s, w, max, min, lines, kind.bold());
    Label {
        item,
        kind,
        rect: Rect::new(x, y, w, f.lines.len() as f32 * f.size * LEADING),
        lines: f.lines,
        size: f.size,
    }
}

/// A note's mark: its chip's tone, stale as `faint`.
pub fn tone_of(flags: &Flags) -> &'static str {
    match flags.chip().map(|c| c.1) {
        Some("plain") => "faint",
        Some(t) => t,
        None => "plain",
    }
}

/// One block drawn at one level from its column's left edge and its top:
/// how many columns it spans, how high it is, and what it draws.
#[derive(Default, Clone)]
struct Drawn {
    span: usize,
    h: f32,
    pinned: bool,
    frames: Vec<(usize, Rect)>,
    cards: Vec<Card>,
    labels: Vec<Label>,
    marks: Vec<Mark>,
}

impl Drawn {
    fn moved(mut self, dx: f32, dy: f32) -> Drawn {
        let mv = |r: &mut Rect| {
            r.x += dx;
            r.y += dy;
        };
        self.frames.iter_mut().for_each(|f| mv(&mut f.1));
        self.cards.iter_mut().for_each(|c| mv(&mut c.rect));
        self.labels.iter_mut().for_each(|l| mv(&mut l.rect));
        for m in &mut self.marks {
            m.x += dx;
            m.y += dy;
        }
        self
    }
}

/// A note card's title, beside its icon (Mid and In).
fn title_of(c: &Card, notes: &[MapNote]) -> Option<Label> {
    let Item::Note(i) = c.item else {
        return None;
    };
    let x = c.rect.x + 16. + 18. + 10.;
    Some(label(
        c.item,
        LabelKind::Title,
        &notes[i].title,
        x,
        c.rect.y + 16.,
        c.rect.right() - 16. - x,
        16.,
        12.,
        1,
    ))
}

/// Draws block `b` at `level` from (0, 0).
fn draw(level: Level, b: Block, notes: &[MapNote], folders: &[Folder], plan: Plan) -> Drawn {
    let height = |i: usize| {
        if level == Level::In {
            in_height(&notes[i])
        } else {
            CARD_H
        }
    };
    let mut d = Drawn {
        span: 1,
        pinned: matches!(b, Block::Note(_)),
        ..Drawn::default()
    };
    match (level, b) {
        (Level::Out, Block::Note(i)) => {
            let n = &notes[i];
            let mut legend: Vec<(String, Option<&'static str>)> = Vec::new();
            if let Some((words, _)) = n.flags.chip() {
                legend.push((words, Some(tone_of(&n.flags))));
            }
            legend.push((n.reads_label(), None));
            let item = Item::Note(i);
            d.h = out_card(
                item,
                if n.flags.entry {
                    "icon-flag"
                } else {
                    "icon-note"
                },
                &n.title,
                &legend,
                &[],
                0,
                0.,
                0.,
                &mut d.labels,
                &mut d.marks,
            );
            d.cards.push(Card {
                item,
                rect: Rect::new(0., 0., COL_W, d.h),
            });
        }
        (Level::Out, Block::Folder(fi)) => {
            let f = &folders[fi];
            let mut legend: Vec<(String, Option<&'static str>)> =
                vec![(count(f.notes.len(), "note", "notes"), None)];
            for (n, words, tone) in [
                (f.fixed, ("fixed", "fixed"), "ok"),
                (f.waiting, ("rule waiting", "rules waiting"), "you"),
                (f.rejected, ("rejected", "rejected"), "bad"),
                (f.stale, ("stale", "stale"), "faint"),
            ] {
                if n > 0 {
                    legend.push((
                        format!("{n} {}", if n == 1 { words.0 } else { words.1 }),
                        Some(tone),
                    ));
                }
            }
            // A big map's overview is drawn smaller: fewer rows.
            let most = if folders.len() + pinned_of(notes).len() > OUT_FEW {
                OUT_ROWS_MANY
            } else {
                OUT_ROWS
            };
            let shown = if f.notes.len() > most {
                most - 1
            } else {
                f.notes.len()
            };
            let rows: Vec<(String, &'static str)> = f.notes[..shown]
                .iter()
                .map(|&i| (notes[i].title.clone(), tone_of(&notes[i].flags)))
                .collect();
            let item = Item::Folder(fi);
            let name = if f.name.is_empty() { "Notes" } else { &f.name };
            d.h = out_card(
                item,
                "icon-folder",
                name,
                &legend,
                &rows,
                f.notes.len() - shown,
                0.,
                0.,
                &mut d.labels,
                &mut d.marks,
            );
            d.cards.push(Card {
                item,
                rect: Rect::new(0., 0., COL_W, d.h),
            });
        }
        (_, Block::Note(i)) => {
            let card = Card {
                item: Item::Note(i),
                rect: Rect::new(FRAME_PAD, 0., CARD_W, height(i)),
            };
            d.h = card.rect.h;
            d.labels.extend(title_of(&card, notes));
            d.cards.push(card);
        }
        (_, Block::Folder(fi)) => {
            let f = &folders[fi];
            let span = span_of(f, plan);
            let per = f.notes.len().div_ceil(span).max(1);
            let w = col_x(span - 1) + COL_W;
            let mut sub_bottom = FRAME_HEAD;
            for (k, chunk) in f.notes.chunks(per).enumerate() {
                let mut cy = FRAME_HEAD;
                for &i in chunk {
                    let h = height(i);
                    d.cards.push(Card {
                        item: Item::Note(i),
                        rect: Rect::new(col_x(k) + FRAME_PAD, cy, CARD_W, h),
                    });
                    cy += h + GAP;
                }
                sub_bottom = sub_bottom.max(cy - GAP);
            }
            let frame = Rect::new(0., 0., w, sub_bottom + FRAME_PAD);
            d.frames.push((fi, frame));
            // The frame's name on the left, its count on the right.
            let item = Item::Folder(fi);
            let count = count(f.notes.len(), "note", "notes");
            let cw = super::fit::width(&count, 13., false);
            let name = if f.name.is_empty() { "Notes" } else { &f.name };
            d.labels.push(label(
                item,
                LabelKind::Frame,
                name,
                FRAME_PAD,
                11.,
                w - 2. * FRAME_PAD - cw - 16.,
                14.5,
                11.,
                1,
            ));
            d.labels.push(label(
                item,
                LabelKind::Count,
                &count,
                w - FRAME_PAD - cw,
                13.,
                cw,
                13.,
                13.,
                1,
            ));
            let titles: Vec<Label> = d.cards.iter().filter_map(|c| title_of(c, notes)).collect();
            d.labels.extend(titles);
            d.span = span;
            d.h = frame.h;
        }
    }
    d
}

/// The block a key names.
fn block_named(notes: &[MapNote], folders: &[Folder], key: &str) -> Option<Block> {
    if let Some(name) = key.strip_prefix("folder:") {
        folders
            .iter()
            .position(|f| f.name == name)
            .map(Block::Folder)
    } else {
        let path = key.strip_prefix("note:")?;
        notes
            .iter()
            .position(|n| n.path.display().to_string() == path)
            .map(Block::Note)
    }
}

/// Lays out one level from a settled arrangement, with its lines.
fn pack(
    level: Level,
    notes: &[MapNote],
    folders: &[Folder],
    edges: &[(usize, usize)],
    arr: &Arrangement,
    plan: Plan,
) -> Layout {
    let (frames, cards, labels, marks) = place(level, notes, folders, arr, &|b| {
        draw(level, b, notes, folders, plan)
    });
    let focus = links(level, folders, edges, &frames, &cards);
    let mut lay = finish(frames, cards, labels, marks);
    lay.focus = focus;
    lay
}

/// Places one level's blocks from a settled arrangement. Among the
/// columns' next blocks it takes the one that would sit highest (the
/// leftmost on a tie) and puts it below everything already in each column
/// it spans.
#[allow(clippy::type_complexity)]
fn place(
    level: Level,
    notes: &[MapNote],
    folders: &[Folder],
    arr: &Arrangement,
    get: &dyn Fn(Block) -> Drawn,
) -> (Vec<(usize, Rect)>, Vec<Card>, Vec<Label>, Vec<Mark>) {
    let mut columns: Vec<Vec<Drawn>> = arr
        .columns
        .iter()
        .map(|c| {
            c.iter()
                .filter_map(|k| block_named(notes, folders, k))
                .map(get)
                .collect()
        })
        .collect();
    for c in &mut columns {
        c.reverse();
    }
    let width = columns
        .iter()
        .enumerate()
        .flat_map(|(c, col)| {
            col.iter()
                .map(move |d| c + if level == Level::Out { 1 } else { d.span })
        })
        .max()
        .unwrap_or(1);
    // Each column's foot and whether its last block is a note.
    let mut foot: Vec<Option<(f32, bool)>> = vec![None; width.max(1)];
    let gap = |prev: bool, this: bool| match level {
        Level::Out => OUT_GAP,
        _ if prev && this => GAP,
        _ => FRAME_GAP,
    };
    let (mut frames, mut cards, mut labels, mut marks) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    loop {
        let mut best: Option<(usize, f32)> = None;
        for (c, col) in columns.iter().enumerate() {
            let Some(d) = col.last() else {
                continue;
            };
            let span = if level == Level::Out { 1 } else { d.span };
            let top = foot[c..c + span]
                .iter()
                .map(|f| f.map_or(0., |(y, p)| y + gap(p, d.pinned)))
                .fold(0., f32::max);
            if best.is_none_or(|(_, t)| top < t - 0.5) {
                best = Some((c, top));
            }
        }
        let Some((c, top)) = best else {
            break;
        };
        let d = columns[c].pop().expect("a next block").moved(col_x(c), top);
        let span = if level == Level::Out { 1 } else { d.span };
        for f in &mut foot[c..c + span] {
            *f = Some((top + d.h, d.pinned));
        }
        frames.extend(d.frames);
        cards.extend(d.cards);
        labels.extend(d.labels);
        marks.extend(d.marks);
    }
    (frames, cards, labels, marks)
}

/// Each linked card's lines at one level, drawn while it is in focus: one
/// line per card it links to (at Out a note in a folder is its folder's
/// card), counting the links it stands for, bundled into one trunk each
/// way (right, with its own column; left), which drops a branch in the gap
/// beside each column it reaches.
fn links(
    level: Level,
    folders: &[Folder],
    edges: &[(usize, usize)],
    frames: &[(usize, Rect)],
    cards: &[Card],
) -> Vec<(Item, Vec<Line>)> {
    let rect_of: HashMap<Item, Rect> = cards.iter().map(|c| (c.item, c.rect)).collect();
    let folder_of: HashMap<usize, usize> = folders
        .iter()
        .enumerate()
        .flat_map(|(fi, f)| f.notes.iter().map(move |&i| (i, fi)))
        .collect();
    let lift = |i: usize| match level {
        Level::Out => folder_of
            .get(&i)
            .map_or(Item::Note(i), |&f| Item::Folder(f)),
        _ => Item::Note(i),
    };
    let mut pairs: BTreeMap<(Item, Item), usize> = BTreeMap::new();
    for &(a, b) in edges {
        let (ia, ib) = (lift(a), lift(b));
        if ia == ib || !rect_of.contains_key(&ia) || !rect_of.contains_key(&ib) {
            continue;
        }
        let key = if pairs.contains_key(&(ib, ia)) {
            (ib, ia)
        } else {
            (ia, ib)
        };
        *pairs.entry(key).or_default() += 1;
    }
    // A frame's name band is in the way of a line as a card is.
    let heads: Vec<Rect> = frames
        .iter()
        .map(|(_, r)| Rect::new(r.x, r.y, r.w, FRAME_HEAD))
        .collect();
    let items: BTreeSet<Item> = pairs.keys().flat_map(|&(a, b)| [a, b]).collect();
    let mut focus = Vec::new();
    for f in items {
        let ca = col_of(rect_of[&f].x);
        let mut right: Vec<(Item, usize)> = Vec::new();
        let mut left: Vec<(Item, usize)> = Vec::new();
        for (&(a, b), &n) in &pairs {
            let o = if a == f {
                b
            } else if b == f {
                a
            } else {
                continue;
            };
            if col_of(rect_of[&o].x) >= ca {
                right.push((o, n));
            } else {
                left.push((o, n));
            }
        }
        let mut groups: Vec<Vec<(Item, usize)>> = [right, left]
            .into_iter()
            .filter(|g| !g.is_empty())
            .collect();
        let weight = |g: &Vec<(Item, usize)>| g.iter().map(|x| x.1).sum::<usize>();
        groups.sort_by_key(|g| std::cmp::Reverse(weight(g)));
        let wants: Vec<Want> = groups
            .into_iter()
            .enumerate()
            .map(|(k, to)| Want { a: f, to, trunk: k })
            .collect();
        focus.push((f, route(&wants, &rect_of, &heads, level == Level::Out)));
    }
    focus
}

/// The layout's size around what it holds.
fn finish(
    frames: Vec<(usize, Rect)>,
    cards: Vec<Card>,
    labels: Vec<Label>,
    marks: Vec<Mark>,
) -> Layout {
    let width = cards
        .iter()
        .map(|c| c.rect.right())
        .chain(frames.iter().map(|f| f.1.right()))
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
        focus: Vec::new(),
        labels,
        marks,
    }
}

/// One Out card's words and marks, from its top left; returns its height.
#[allow(clippy::too_many_arguments)]
fn out_card(
    item: Item,
    icon: &'static str,
    title: &str,
    legend: &[(String, Option<&'static str>)],
    rows: &[(String, &'static str)],
    more: usize,
    x: f32,
    y: f32,
    labels: &mut Vec<Label>,
    marks: &mut Vec<Mark>,
) -> f32 {
    let iw = COL_W - 2. * OUT_PAD;
    let x0 = x + OUT_PAD;
    let mut cy = y + OUT_PAD;
    // The name beside its icon: a folder, the entry note's flag, a note.
    // The icon is sized to the name, so the room it takes depends on the
    // size the name is set at: fitted once at the largest, then again for
    // the size that gave.
    let room = |size: f32| size * OUT_ICON + 12.;
    let at = |r: f32, max: f32, min: f32| {
        label(
            item,
            LabelKind::Title,
            title,
            x0 + r,
            cy,
            iw - r,
            max,
            min,
            3,
        )
    };
    let mut t = at(room(OUT_TITLE), OUT_TITLE, OUT_TITLE_MIN);
    if t.size < OUT_TITLE {
        let again = at(room(t.size), t.size, OUT_TITLE_MIN);
        let fits = again
            .lines
            .iter()
            .all(|l| super::fit::width(l, again.size, true) <= iw - room(again.size) + 0.01);
        if fits && !again.lines.last().is_some_and(|l| l.ends_with('…')) {
            t = again;
        }
    }
    // A word too long for the card at the floor: smaller still, not cut.
    if t.lines.last().is_some_and(|l| l.ends_with('…')) {
        t = at(room(OUT_TITLE_MIN), OUT_TITLE_MIN, 8.);
    }
    let d = t.size * OUT_ICON;
    t.rect.x = x0 + d + 12.;
    marks.push(Mark {
        item,
        x: x0 + d / 2.,
        y: cy + t.size * LEADING / 2.,
        d,
        tone: icon,
        row: false,
    });
    cy = t.rect.bottom() + 12.;
    labels.push(t);
    // The legend flows in lines: a mark and its words per entry.
    let line_h = OUT_META * LEADING;
    let mut lx = x0;
    let mut first = true;
    for (words, tone) in legend {
        let lead = if tone.is_some() { OUT_DOT + 8. } else { 0. };
        let w = super::fit::width(words, OUT_META, false);
        if !first && lx + lead + w > x0 + iw {
            lx = x0;
            cy += line_h + 4.;
        }
        if let Some(tone) = tone {
            marks.push(Mark {
                item,
                x: lx + OUT_DOT / 2.,
                y: cy + line_h / 2.,
                d: OUT_DOT,
                tone,
                row: false,
            });
        }
        let l = label(
            item,
            LabelKind::Meta,
            words,
            lx + lead,
            cy,
            (x0 + iw - lx - lead).min(w),
            OUT_META,
            14.,
            1,
        );
        lx = l.rect.right() + 18.;
        labels.push(l);
        first = false;
    }
    cy += line_h;
    if !rows.is_empty() {
        cy += 14.;
    }
    for (title, tone) in rows {
        marks.push(Mark {
            item,
            x: x0 + OUT_DOT / 2.,
            y: cy + OUT_ROW_H / 2.,
            d: OUT_DOT,
            tone,
            row: true,
        });
        let rx = x0 + OUT_DOT + 12.;
        let mut l = label(
            item,
            LabelKind::Row,
            title,
            rx,
            cy,
            x0 + iw - rx,
            OUT_ROW,
            18.,
            1,
        );
        l.rect.y = cy + (OUT_ROW_H - l.rect.h) / 2.;
        labels.push(l);
        cy += OUT_ROW_H;
    }
    if more > 0 {
        let mut l = label(
            item,
            LabelKind::More,
            &format!("+ {}", count(more, "more note", "more notes")),
            x0 + OUT_DOT + 12.,
            cy,
            iw - OUT_DOT - 12.,
            OUT_ROW,
            18.,
            1,
        );
        l.rect.y = cy + (OUT_ROW_H - l.rect.h) / 2.;
        labels.push(l);
        cy += OUT_ROW_H;
    }
    cy + OUT_PAD - y
}

/// The gap left of column `g`: where its lanes may go, frame to frame
/// with a margin.
fn gap_span(g: usize) -> (f32, f32) {
    let right = col_x(g) - 6.;
    (right - (COL_GAP - 12.), right)
}

/// The column a card's left edge `x` is in.
fn col_of(x: f32) -> usize {
    (x / (COL_W + COL_GAP)).floor().max(0.) as usize
}

/// Lines a router is asked for: from card `a` to each of `to` (with the
/// links each stands for), sharing one trunk. Every target is on one side
/// of `a`: in its column or right of it, or left of it.
struct Want {
    a: Item,
    to: Vec<(Item, usize)>,
    trunk: usize,
}

/// Routes trunks so none runs through a card or a frame's name band, nor
/// closer to one than a clearance (20 points at Out, 12 at Mid and In).
///
/// A trunk leaves its card at one port, level, toward its targets. Before
/// each column it crosses it keeps its height while that is clear of the
/// column's cards, name bands and other trunks; else, in the gap before
/// that column, it turns to the middle of the free channel nearest the way
/// to the targets past it. In the gap beside each column with targets it
/// drops a branch to them, on the same vertical as any turn it makes there,
/// so each gap holds one vertical per trunk. Lines that leave one edge of a
/// card are spread along it, ordered by where their other ends sit.
fn route(wants: &[Want], rect_of: &HashMap<Item, Rect>, heads: &[Rect], out: bool) -> Vec<Line> {
    let clear = if out { 20f32 } else { 12f32 };
    // A note card's lines meet it by its title; an Out card's, by its
    // middle, spread wider.
    let centre = |r: &Rect| {
        if out {
            r.y + r.h / 2.
        } else {
            r.y + r.h.min(CARD_H) / 2.
        }
    };
    let spread = if out { 24f32 } else { 14f32 };
    struct Path {
        a: Item,
        ra: Rect,
        to: Vec<(Item, Rect, usize)>,
        right: bool,
        trunk: usize,
        ya: f32,
        yb: Vec<f32>,
        /// Each gap the trunk reaches: the gap, the trunk's height there,
        /// the targets it branches to, and the height it turns to.
        stops: Vec<(usize, f32, Vec<usize>, f32)>,
        lanes: Vec<f32>,
    }
    let mut paths: Vec<Path> = Vec::new();
    for w in wants {
        let Some(&ra) = rect_of.get(&w.a) else {
            continue;
        };
        let to: Vec<(Item, Rect, usize)> =
            w.to.iter()
                .filter_map(|&(i, n)| rect_of.get(&i).map(|r| (i, *r, n)))
                .collect();
        if to.is_empty() {
            continue;
        }
        let ca = col_of(ra.x);
        let right = to.iter().all(|t| col_of(t.1.x) >= ca);
        let yb = to.iter().map(|t| centre(&t.1)).collect();
        paths.push(Path {
            a: w.a,
            ra,
            to,
            right,
            trunk: w.trunk,
            ya: centre(&ra),
            yb,
            stops: Vec::new(),
            lanes: Vec::new(),
        });
    }
    // Which edge a target is entered by: its right edge when it is in the
    // trunk's own column or left of it.
    let enters_right = |p: &Path, j: usize| !p.right || col_of(p.to[j].1.x) == col_of(p.ra.x);
    // Ports: each (card, edge) spreads its lines, ordered by the other end.
    // (card, right edge) -> (the other end's y, path, the target's place or
    // usize::MAX for the path's own card).
    type Ends = HashMap<(Item, bool), Vec<(f32, usize, usize)>>;
    let mut ends: Ends = HashMap::new();
    for (k, p) in paths.iter().enumerate() {
        let mean = p.to.iter().map(|t| centre(&t.1)).sum::<f32>() / p.to.len() as f32;
        ends.entry((p.a, p.right))
            .or_default()
            .push((mean, k, usize::MAX));
        for (j, t) in p.to.iter().enumerate() {
            ends.entry((t.0, enters_right(p, j)))
                .or_default()
                .push((centre(&p.ra), k, j));
        }
    }
    for ((item, _), mut list) in ends {
        let Some(r) = rect_of.get(&item) else {
            continue;
        };
        list.sort_by(|x, y| x.0.total_cmp(&y.0).then((x.1, x.2).cmp(&(y.1, y.2))));
        let n = list.len();
        let (lo, hi) = (r.y + 10., r.bottom() - 10.);
        let step = if n <= 1 {
            0.
        } else {
            spread.min((hi - lo) / (n - 1) as f32)
        };
        let half = step * (n.saturating_sub(1)) as f32 / 2.;
        let mid = centre(r).clamp(lo + half, (hi - half).max(lo + half));
        for (k, (_, path, j)) in list.into_iter().enumerate() {
            let y = mid - half + k as f32 * step;
            if j == usize::MAX {
                paths[path].ya = y;
            } else {
                paths[path].yb[j] = y;
            }
        }
    }
    // What is in the way across each column: its cards and name bands, and
    // the trunks already crossing it.
    let mut blocked: BTreeMap<usize, Vec<(f32, f32)>> = BTreeMap::new();
    for r in rect_of.values().chain(heads) {
        for c in col_of(r.x)..=col_of(r.right() - 1.) {
            blocked.entry(c).or_default().push((r.y, r.bottom()));
        }
    }
    let ok =
        |no: &[(f32, f32)], y: f32| !no.iter().any(|&(lo, hi)| lo - clear < y && y < hi + clear);
    // The middle of the free channel nearest the way from `y` to `want`
    // (one at least twice the clearance high), or below everything.
    let pick = |no: &mut Vec<(f32, f32)>, y: f32, want: f32| {
        no.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut free: Vec<(f32, f32)> = Vec::new();
        let mut from = f32::MIN;
        for &(lo, hi) in no.iter() {
            if from > f32::MIN && lo - from >= 2. * clear {
                free.push((from, lo));
            }
            from = from.max(hi);
        }
        let mut best: Option<(f32, f32, f32)> = None;
        let mut consider = |p: f32| {
            let (cost, near) = ((p - y).abs() + (p - want).abs(), (p - want).abs());
            if best.is_none_or(|(c, n, _)| cost < c - 1e-3 || (cost < c + 1e-3 && near < n - 1e-3))
            {
                best = Some((cost, near, p));
            }
        };
        for (lo, hi) in free {
            consider((lo + hi) / 2.);
        }
        consider(want.max(from.max(0.) + clear));
        best.map_or(want, |b| b.2)
    };
    for p in &mut paths {
        let ca = col_of(p.ra.x);
        let tcol: Vec<usize> = p.to.iter().map(|t| col_of(t.1.x)).collect();
        // Each gap reached: the gap, the columns entered there, the column
        // crossed after it.
        let mut gaps: Vec<(usize, Vec<usize>, Option<usize>)> = Vec::new();
        if p.right {
            let far = tcol.iter().copied().max().unwrap_or(ca).max(ca + 1);
            for g in ca + 1..=far {
                let mut cols = vec![g];
                if g == ca + 1 {
                    cols.push(ca);
                }
                gaps.push((g, cols, (g < far).then_some(g)));
            }
        } else {
            let far = tcol.iter().copied().min().unwrap_or(ca);
            for g in (far + 1..=ca).rev() {
                gaps.push((g, vec![g - 1], (g - 1 > far).then_some(g - 1)));
            }
        }
        let mut y = p.ya;
        for (g, cols, cross) in gaps {
            let js: Vec<usize> = (0..tcol.len())
                .filter(|&j| cols.contains(&tcol[j]))
                .collect();
            let mut to = y;
            if let Some(c) = cross {
                let past: Vec<usize> = (0..tcol.len())
                    .filter(|&j| if p.right { tcol[j] > c } else { tcol[j] < c })
                    .collect();
                let want = past.iter().map(|&j| p.yb[j]).sum::<f32>() / past.len().max(1) as f32;
                let no = blocked.entry(c).or_default();
                if !ok(no, y) {
                    let mut all = no.clone();
                    to = pick(&mut all, y, want);
                }
                blocked.entry(c).or_default().push((to, to));
            }
            if !js.is_empty() || to != y {
                p.stops.push((g, y, js, to));
            }
            y = to;
        }
        p.lanes = vec![0.; p.stops.len()];
    }
    // Lanes: one vertical per trunk in each gap, longest outermost.
    let mut by_gap: BTreeMap<usize, Vec<(usize, usize, f32)>> = BTreeMap::new();
    for (k, p) in paths.iter().enumerate() {
        for (s, (g, y, js, to)) in p.stops.iter().enumerate() {
            let (lo, hi) = js
                .iter()
                .map(|&j| p.yb[j])
                .chain([*y, *to])
                .fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(v), hi.max(v)));
            by_gap.entry(*g).or_default().push((k, s, hi - lo));
        }
    }
    let lane_step = if out { 24f32 } else { 16f32 };
    for (gap, mut members) in by_gap {
        members.sort_by(|x, y| y.2.total_cmp(&x.2).then((x.0, x.1).cmp(&(y.0, y.1))));
        let (lo, hi) = gap_span(gap);
        let n = members.len();
        let step = if n <= 1 {
            0.
        } else {
            lane_step.min((hi - lo) / (n - 1) as f32)
        };
        let start = (lo + hi) / 2. - step * (n.saturating_sub(1)) as f32 / 2.;
        for (k, (m, s, _)) in members.into_iter().enumerate() {
            paths[m].lanes[s] = start + k as f32 * step;
        }
    }
    let mut lines = Vec::new();
    for p in &paths {
        let a0 = if p.right { p.ra.right() } else { p.ra.x };
        let mut trunk = vec![(a0, p.ya)];
        for (s, (_, y, js, to)) in p.stops.iter().enumerate() {
            let x = p.lanes[s];
            for &j in js {
                let (item, t, links) = p.to[j];
                let b0 = if enters_right(p, j) { t.right() } else { t.x };
                let mut points = trunk.clone();
                points.push((x, *y));
                points.push((x, p.yb[j]));
                points.push((b0, p.yb[j]));
                let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
                for &(px, py) in &points {
                    x0 = x0.min(px);
                    y0 = y0.min(py);
                    x1 = x1.max(px);
                    y1 = y1.max(py);
                }
                lines.push(Line {
                    from: p.a,
                    to: item,
                    points,
                    bounds: Rect::new(x0 - 6., y0 - 6., x1 - x0 + 12., y1 - y0 + 12.),
                    links,
                    trunk: p.trunk,
                });
            }
            if to != y {
                trunk.push((x, *y));
                trunk.push((x, *to));
            }
        }
    }
    lines
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
    let arrangement = default_arrangement(&notes, &folders);
    let (out, mid, inner) = lay_out_in(&notes, &folders, &edges, &arrangement);
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
        default_arrangement: arrangement.clone(),
        arrangement,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_arrangement_lays_out_the_default_map() {
        let m = synthetic(60, 6, 80);
        let again = m.arranged(&m.default_arrangement);
        assert_eq!(again.mid, m.mid);
        assert_eq!(again.out, m.out);
        assert!(!again.is_arranged());
        // Every block once.
        let keys: Vec<&String> = m.default_arrangement.keys().collect();
        let set: BTreeSet<&String> = keys.iter().copied().collect();
        assert_eq!(keys.len(), set.len());
        assert_eq!(keys.len(), 1 + m.folders.len());
    }

    #[test]
    fn settle_keeps_every_block_once_and_drops_what_is_gone() {
        let m = demo_shaped();
        let mut arr = m.default_arrangement.clone();
        arr.columns[0].insert(0, "folder:Gone".into());
        arr.columns.push(vec!["folder:Runbooks".into()]);
        arr.columns.push(Vec::new());
        // A block missing altogether comes back in its default column.
        let missing = m.key(Block::Folder(0));
        for c in &mut arr.columns {
            c.retain(|k| *k != missing);
        }
        let s = settle(&m.notes, &m.folders, &arr);
        let keys: Vec<&String> = s.keys().collect();
        assert!(!keys.iter().any(|k| *k == "folder:Gone"));
        assert_eq!(keys.iter().filter(|k| **k == "folder:Runbooks").count(), 1);
        assert!(keys.contains(&&missing));
        assert_eq!(keys.len(), m.default_arrangement.keys().count());
        assert!(s.columns.iter().all(|c| !c.is_empty()));
    }

    #[test]
    fn moves_keep_every_block_once_and_do_what_they_say() {
        let m = demo_shaped();
        let a = m.default_arrangement.clone();
        let key = m.key(Block::Folder(1));
        let (c, _) = a.find(&key).unwrap();
        // Into a new column at the right.
        let b = a.move_to(&key, a.columns.len(), 0);
        assert_eq!(b.find(&key), Some((b.columns.len() - 1, 0)));
        assert_eq!(b.keys().count(), a.keys().count());
        // Left, then back right.
        let l = b.nudge(&key, Nudge::Left);
        assert_eq!(l.find(&key).unwrap().0, b.columns.len() - 2);
        // Up and down within a column.
        let top = a.columns[0][0].clone();
        let down = a.nudge(&top, Nudge::Down);
        assert_eq!(down.find(&top), Some((0, 1)));
        assert_eq!(down.nudge(&top, Nudge::Up), a);
        // At the edges a nudge does nothing.
        assert_eq!(a.nudge(&top, Nudge::Up), a);
        assert_eq!(a.nudge(&top, Nudge::Left), a);
        let _ = c;
        // The arranged map is a person's.
        assert!(m.arranged(&b).is_arranged());
    }

    #[test]
    fn a_block_keeps_its_column_at_every_zoom() {
        let m = demo_shaped();
        let arr = random_arrangement(&m, 5);
        let a = m.arranged(&arr);
        for (c, col) in a.arrangement.columns.iter().enumerate() {
            for k in col {
                let b = block_named(&a.notes, &a.folders, k).unwrap();
                for level in [Level::Out, Level::Mid, Level::In] {
                    let r = a.block_rect(level, b).unwrap();
                    let x = if matches!(b, Block::Note(_)) && level != Level::Out {
                        r.x - FRAME_PAD
                    } else {
                        r.x
                    };
                    assert_eq!(x, col_x(c), "{k} at {level:?}");
                }
            }
        }
    }

    #[test]
    fn handles_and_drops() {
        let m = demo_shaped();
        let f = m.folders.iter().position(|f| f.name == "Runbooks").unwrap();
        let fr = m.block_rect(Level::Mid, Block::Folder(f)).unwrap();
        // The name band lifts the folder; a note in it does not.
        assert_eq!(
            m.handle_at(Level::Mid, fr.x + 30., fr.y + 10.),
            Some(Block::Folder(f))
        );
        let note = m
            .mid
            .cards
            .iter()
            .find(|c| {
                m.folders[f].notes.contains(&match c.item {
                    Item::Note(i) => i,
                    _ => usize::MAX,
                })
            })
            .unwrap();
        assert_eq!(
            m.handle_at(Level::Mid, note.rect.x + 5., note.rect.y + 5.),
            None
        );
        // At Out the whole card lifts it.
        let oc = m.out.card(Item::Folder(f)).unwrap().rect;
        assert_eq!(
            m.handle_at(Level::Out, oc.x + 5., oc.bottom() - 5.),
            Some(Block::Folder(f))
        );
        // Dropped right of every column: a new column.
        let d = m.drop_at(Level::Out, Block::Folder(f), m.out.width + 200., 10.);
        assert_eq!(
            (d.col, d.at, d.new_column),
            (m.arrangement.columns.len(), 0, true)
        );
        // Dropped above the first block of column 0: its top.
        let d = m.drop_at(Level::Out, Block::Folder(f), 10., -50.);
        assert_eq!((d.col, d.at, d.new_column), (0, 0, false));
    }

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

    /// Every line a level can draw: each card's own.
    fn all_lines(l: &Layout) -> Vec<&Line> {
        l.focus.iter().flat_map(|(_, ls)| ls.iter()).collect()
    }

    /// Each set of lines drawn at once: a card's own.
    fn line_sets(l: &Layout) -> Vec<Vec<&Line>> {
        l.focus.iter().map(|(_, ls)| ls.iter().collect()).collect()
    }

    #[test]
    fn no_two_trunks_share_a_lane_or_a_port() {
        // One hub note linked from many, as START HERE is, and a big map.
        let mut hub = synthetic(60, 5, 0);
        hub.edges = (1..40).map(|i| (0, i)).collect();
        let hub = relaid(hub);
        for m in [hub, synthetic(300, 12, 1000), demo_shaped()] {
            for lay in [&m.out, &m.mid, &m.inner] {
                for set in line_sets(lay) {
                    // Vertical runs: (trunk, x, y0, y1), each once per trunk.
                    let mut runs: Vec<(usize, f32, f32, f32)> = Vec::new();
                    for l in &set {
                        for w in l.points.windows(2).filter(|w| w[0].0 == w[1].0) {
                            let r = (l.trunk, w[0].0, w[0].1.min(w[1].1), w[0].1.max(w[1].1));
                            if r.3 > r.2 && !runs.contains(&r) {
                                runs.push(r);
                            }
                        }
                    }
                    for (i, a) in runs.iter().enumerate() {
                        for b in &runs[i + 1..] {
                            if a.0 == b.0 {
                                continue;
                            }
                            let meets = a.2 < b.3 && b.2 < a.3;
                            assert!(
                                !meets || (a.1 - b.1).abs() >= 1.,
                                "two trunks share x {}: {a:?} {b:?}",
                                a.1
                            );
                        }
                    }
                    // Ports: two trunks never leave a card at one point.
                    let mut ports: Vec<(usize, Item, (f32, f32))> = Vec::new();
                    for l in &set {
                        let p = (l.trunk, l.from, l.points[0]);
                        if !ports.contains(&p) {
                            ports.push(p);
                        }
                    }
                    for (i, a) in ports.iter().enumerate() {
                        for b in &ports[i + 1..] {
                            if a.1 == b.1 && a.0 != b.0 {
                                assert!(
                                    (a.2 .0 - b.2 .0).abs() > 0.5 || (a.2 .1 - b.2 .1).abs() >= 1.,
                                    "two trunks leave {:?} at {:?}",
                                    a.1,
                                    a.2
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn a_focus_leaves_as_one_trunk_each_way_and_branches_beside_columns() {
        let mut m = synthetic(60, 5, 0);
        m.edges = (1..40).map(|i| (0, i)).collect();
        let m = relaid(m);
        let (_, lines) = m
            .mid
            .focus
            .iter()
            .find(|(i, _)| *i == Item::Note(0))
            .expect("the hub's lines");
        assert_eq!(lines.len(), 39, "every link of the hub, one branch each");
        // One trunk each way: right (with its own column), left.
        let hub = col_of(m.mid.card(Item::Note(0)).unwrap().rect.x);
        let sides: BTreeSet<bool> = lines
            .iter()
            .map(|l| col_of(m.mid.card(l.to).unwrap().rect.x) >= hub)
            .collect();
        let trunks: BTreeSet<usize> = lines.iter().map(|l| l.trunk).collect();
        assert_eq!(trunks.len(), sides.len());
        // The lines of a trunk leave from one point, and the ones going to
        // one column share every point up to their branch's foot.
        for t in trunks {
            let mine: Vec<&Line> = lines.iter().filter(|l| l.trunk == t).collect();
            assert!(mine.iter().all(|l| l.points[0] == mine[0].points[0]));
            for a in &mine {
                for b in &mine {
                    let (ra, rb) = (
                        m.mid.card(a.to).unwrap().rect,
                        m.mid.card(b.to).unwrap().rect,
                    );
                    if col_of(ra.x) == col_of(rb.x) {
                        let n = a.points.len();
                        assert_eq!(n, b.points.len());
                        assert_eq!(a.points[..n - 2], b.points[..n - 2]);
                    }
                }
            }
        }
        // Another card shows only its own line.
        let (_, one) = m
            .mid
            .focus
            .iter()
            .find(|(i, _)| *i == Item::Note(7))
            .unwrap();
        assert_eq!(one.len(), 1);
    }

    #[test]
    fn at_most_the_budget_of_lines_shows_at_once_at_every_zoom() {
        for m in maps() {
            for (lv, l) in [("out", &m.out), ("mid", &m.mid), ("in", &m.inner)] {
                // Nothing at rest; a card's own at most one trunk each way.
                assert_eq!(l.visible(None), 0, "{lv}");
                for c in &l.cards {
                    let n = l.visible(Some(c.item));
                    assert!(n <= 2, "{lv}: {:?} shows {n}", c.item);
                }
            }
            // Zoomed out, a line stands for every link between two blocks.
            assert!(all_lines(&m.out).iter().all(|l| l.links >= 1));
        }
    }

    #[test]
    fn every_line_turns_at_right_angles_and_only_in_gaps() {
        for m in maps() {
            for l in [&m.out, &m.mid, &m.inner] {
                for line in all_lines(l) {
                    assert!(line.points.len() >= 4, "{line:?}");
                    for w in line.points.windows(2) {
                        let (a, b) = (w[0], w[1]);
                        assert!(a.0 == b.0 || a.1 == b.1, "{a:?} {b:?}");
                        if a.0 == b.0 && a.1 != b.1 {
                            // A vertical run is in a gap, never in a column.
                            let inside = a.0 - col_of(a.0) as f32 * (COL_W + COL_GAP);
                            assert!(inside > COL_W, "a run at {} is in a column", a.0);
                        }
                    }
                }
            }
        }
        // Every link is a line of both its cards; at Out one line joins
        // two blocks.
        let m = synthetic(12, 3, 8);
        assert_eq!(all_lines(&m.mid).len(), 16);
        assert!(all_lines(&m.out).iter().map(|l| l.links).sum::<usize>() <= 16);
    }

    #[test]
    fn the_overview_fills_the_view() {
        // 0.8.2 showed a 300-note map as a strip across the top.
        for (w, h) in [(1220., 690.), (860., 600.), (1600., 900.)] {
            let m = synthetic(300, 12, 1000).fitted(w, h);
            let (z, _) = m.fit(w, h);
            assert!(z < OUT_BELOW, "a big map opens at Out");
            let (fw, fh) = (m.out.width * z / w, m.out.height * z / h);
            assert!(fw > 0.6 && fh > 0.6, "{w}×{h}: fills {fw:.2} × {fh:.2}");
            // A block keeps its column at every zoom.
            assert_eq!(m.arrangement, m.default_arrangement);
            assert!(!m.is_arranged());
        }
    }

    #[test]
    fn small_maps_open_whole_and_readable() {
        for m in [
            demo_shaped(),
            synthetic(20, 4, 24),
            synthetic(40, 5, 60),
            synthetic(3, 2, 2),
        ] {
            let (w, h) = (860., 600.);
            let f = m.fitted(w, h);
            let (z, (ox, oy)) = f.fit(w, h);
            let lay = f.layout(Level::of(z));
            // The whole map is in the view.
            assert!(-ox * z >= FIT_PAD - 0.5 && -oy * z >= FIT_PAD - 0.5);
            assert!(lay.width * z <= w + 0.5 && lay.height * z <= h + 0.5);
            // Out titles at 48 points read at 20 or more.
            assert!(z >= 0.42, "{} notes open at {z}", m.notes.len());
        }
    }

    #[test]
    fn a_persons_arrangement_is_kept_when_fitted() {
        let m = demo_shaped();
        let arr = random_arrangement(&m, 3);
        let a = m.arranged(&arr);
        assert_eq!(a.fitted(1600., 500.).arrangement, a.arrangement);
    }

    #[test]
    fn lanes_stay_in_their_gap() {
        let m = synthetic(300, 12, 1000);
        for l in all_lines(&m.mid) {
            let x = l.points[1].0;
            // Never inside a column of cards.
            let c = (x / (COL_W + COL_GAP)).floor();
            let inside = x - c * (COL_W + COL_GAP);
            assert!(inside > COL_W, "lane at {x} is inside column {c}");
        }
    }

    #[test]
    fn out_has_one_card_per_folder_and_per_pinned_note_none_spanning() {
        let m = synthetic(300, 12, 1000);
        for f in 0..m.folders.len() {
            let c = m.out.card(Item::Folder(f)).expect("folder card");
            assert_eq!(c.rect.w, COL_W);
            assert_eq!(c.rect.x % (COL_W + COL_GAP), 0., "{c:?} is in a column");
        }
        let pinned = (0..m.notes.len()).filter(|i| m.pinned(*i));
        for i in pinned {
            assert_eq!(m.out.card(Item::Note(i)).unwrap().rect.x, 0.);
        }
        assert!(m.out.frames.is_empty());
    }

    /// Maps to check every level of: small and large, short and long
    /// folder names, the demo home's shape (three pinned notes, one big
    /// folder of top-level notes, three small ones).
    fn maps() -> Vec<NotesMap> {
        let mut long = synthetic(40, 5, 60);
        for (k, f) in long.folders.iter_mut().enumerate() {
            f.name = [
                "Architecture decision records and their reviews",
                "Runbooks",
                "Incidents",
                "Supercalifragilisticexpialidocious-integrations",
                "",
            ][k % 5]
                .into();
        }
        for n in long.notes.iter_mut().step_by(3) {
            n.title = "A note whose title runs on for quite a while longer than a card".into();
        }
        let long = relaid(long);
        let base = vec![
            synthetic(300, 12, 1000),
            synthetic(20, 4, 24),
            synthetic(3, 2, 2),
            demo_shaped(),
            long,
        ];
        // Each again in two random arrangements and fitted to two views.
        let mut out = Vec::new();
        for (k, m) in base.into_iter().enumerate() {
            for seed in [1u64, 2] {
                out.push(m.arranged(&random_arrangement(&m, seed * 7919 + k as u64)));
            }
            out.push(m.fitted(1220., 690.));
            out.push(m.fitted(700., 900.));
            out.push(m);
        }
        out
    }

    /// The map's blocks shuffled across up to one column more than it had.
    fn random_arrangement(m: &NotesMap, seed: u64) -> Arrangement {
        let mut s = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let mut next = move || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s
        };
        let keys: Vec<String> = m.default_arrangement.keys().cloned().collect();
        let n = m.default_arrangement.columns.len() + 1;
        let mut columns = vec![Vec::new(); n];
        for k in keys {
            let c = (next() % n as u64) as usize;
            let at = (next() % (columns[c].len() as u64 + 1)) as usize;
            columns[c].insert(at, k);
        }
        Arrangement { columns }
    }

    /// `m` laid out again in the default arrangement of its notes.
    fn relaid(mut m: NotesMap) -> NotesMap {
        m.default_arrangement = default_arrangement(&m.notes, &m.folders);
        m.arrangement = m.default_arrangement.clone();
        let (out, mid, inner) = lay_out(&m.notes, &m.folders, &m.edges);
        m.out = out;
        m.mid = mid;
        m.inner = inner;
        m
    }

    /// Three pinned notes, 14 top-level notes, Runbooks 3, Decisions 1,
    /// Incidents 1, linked as the demo home is: the pinned ones to many.
    fn demo_shaped() -> NotesMap {
        let mut m = synthetic(22, 4, 0);
        for (i, n) in m.notes.iter_mut().enumerate() {
            n.folder = match i {
                0..=2 => String::new(),
                3..=16 => String::new(),
                17..=19 => "Runbooks".into(),
                20 => "Decisions".into(),
                _ => "Incidents".into(),
            };
            n.flags.entry = i == 0;
            n.flags.required = i == 1 || i == 2;
            n.flags.waiting = usize::from(i == 0) * 3;
        }
        m.edges = (3..22)
            .map(|i| (i % 3, i))
            .chain([(17, 21), (20, 5), (21, 9)])
            .collect();
        m.folders = folders(&m.notes);
        relaid(m)
    }

    #[test]
    fn no_two_cards_or_frames_overlap_at_any_level() {
        for m in maps() {
            for l in [&m.out, &m.mid, &m.inner] {
                for (i, a) in l.cards.iter().enumerate() {
                    for b in &l.cards[i + 1..] {
                        assert!(!a.rect.meets(&b.rect), "{a:?} meets {b:?}");
                    }
                }
                for (i, a) in l.frames.iter().enumerate() {
                    for b in &l.frames[i + 1..] {
                        assert!(!a.1.meets(&b.1), "frames {a:?} and {b:?} meet");
                    }
                }
            }
        }
    }

    /// The card or frame a label or mark belongs to.
    fn owner(l: &Layout, item: Item) -> Rect {
        l.card(item)
            .map(|c| c.rect)
            .or_else(|| match item {
                Item::Folder(f) => l.frames.iter().find(|x| x.0 == f).map(|x| x.1),
                _ => None,
            })
            .expect("a label belongs to something drawn")
    }

    fn inside(inner: &Rect, outer: &Rect) -> bool {
        inner.x >= outer.x - 0.01
            && inner.y >= outer.y - 0.01
            && inner.right() <= outer.right() + 0.01
            && inner.bottom() <= outer.bottom() + 0.01
    }

    #[test]
    fn every_label_fits_its_box_at_every_level() {
        for m in maps() {
            for (lv, l) in [("out", &m.out), ("mid", &m.mid), ("in", &m.inner)] {
                for lab in &l.labels {
                    for line in &lab.lines {
                        let w = super::super::fit::width(line, lab.size, lab.kind.bold());
                        assert!(w <= lab.rect.w + 0.01, "{lv}: {line:?} is {w} in {lab:?}");
                    }
                    let own = owner(l, lab.item);
                    assert!(inside(&lab.rect, &own), "{lv}: {lab:?} leaves {own:?}");
                }
                // Within one card, labels do not overlap one another.
                for (i, a) in l.labels.iter().enumerate() {
                    for b in &l.labels[i + 1..] {
                        if a.item == b.item {
                            assert!(!a.rect.meets(&b.rect), "{lv}: {a:?} meets {b:?}");
                        }
                    }
                }
                for mk in &l.marks {
                    let r = Rect::new(mk.x - mk.d / 2., mk.y - mk.d / 2., mk.d, mk.d);
                    assert!(inside(&r, &owner(l, mk.item)), "{lv}: {mk:?}");
                    for lab in l.labels.iter().filter(|x| x.item == mk.item) {
                        assert!(!r.meets(&lab.rect), "{lv}: {mk:?} meets {lab:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn out_titles_are_set_large_and_never_cut() {
        for m in maps() {
            for lab in m.out.labels.iter().filter(|l| l.kind == LabelKind::Title) {
                // Below the floor only for a word that cannot break.
                let long_word = lab.lines.iter().any(|l| !l.contains(' '))
                    && super::super::fit::width(&lab.lines[0], OUT_TITLE_MIN, true)
                        > COL_W - 2. * OUT_PAD;
                assert!(lab.size >= OUT_TITLE_MIN || long_word, "{lab:?}");
                assert!(lab.lines.len() <= 3);
                assert!(
                    !lab.lines.iter().any(|l| l.ends_with('…')),
                    "{lab:?} is cut"
                );
            }
        }
    }

    #[test]
    fn out_cards_list_notes_with_marks_not_tiles() {
        let m = demo_shaped();
        let top = m.folders.iter().position(|f| f.name.is_empty()).unwrap();
        let item = Item::Folder(top);
        let rows = m
            .out
            .labels
            .iter()
            .filter(|l| l.item == item && l.kind == LabelKind::Row)
            .count();
        assert_eq!(rows, OUT_ROWS - 1);
        let more = m
            .out
            .labels
            .iter()
            .find(|l| l.item == item && l.kind == LabelKind::More)
            .expect("+ n more");
        assert_eq!(more.lines, ["+ 9 more notes"]);
        // A mark per row and per legend entry with a tone, all small.
        assert!(m
            .out
            .marks
            .iter()
            .filter(|k| !k.tone.starts_with("icon-"))
            .all(|k| k.d <= OUT_DOT));
        assert!(m.out.marks.iter().filter(|k| k.item == item).count() >= rows);
    }

    /// Whether the segment from `a` to `b` (level or plumb) enters `r`.
    fn enters(a: (f32, f32), b: (f32, f32), r: &Rect) -> bool {
        let (x0, x1) = (a.0.min(b.0), a.0.max(b.0));
        let (y0, y1) = (a.1.min(b.1), a.1.max(b.1));
        x0 < r.right() - 1.
            && r.x + 1. < x1.max(x0 + 0.01)
            && y0 < r.bottom() - 1.
            && r.y + 1. < y1.max(y0 + 0.01)
    }

    #[test]
    fn no_line_runs_through_a_card_at_any_level() {
        for m in maps() {
            for (lv, l) in [("out", &m.out), ("mid", &m.mid), ("in", &m.inner)] {
                for line in all_lines(l) {
                    for w in line.points.windows(2) {
                        assert!(w[0].0 == w[1].0 || w[0].1 == w[1].1, "{lv}: {w:?}");
                        // Nor through a folder's name or count.
                        for lab in l
                            .labels
                            .iter()
                            .filter(|x| matches!(x.kind, LabelKind::Frame | LabelKind::Count))
                        {
                            let mut r = lab.rect;
                            r.w = super::super::fit::width(&lab.lines[0], lab.size, true);
                            assert!(!enters(w[0], w[1], &r), "{lv}: {w:?} through {lab:?}");
                        }
                        for c in &l.cards {
                            assert!(
                                !enters(w[0], w[1], &c.rect),
                                "{lv}: {:?}→{:?} runs through {:?}: {w:?}",
                                line.from,
                                line.to,
                                c.item
                            );
                        }
                    }
                }
            }
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
    }

    /// A trunk keeps clear of every card and name band it passes, and of
    /// every other line it shares a gap with: 20 points at Out, 12 at Mid
    /// and In. Only a branch's last stretch, into its card, comes closer.
    #[test]
    fn lines_keep_clear_of_cards_and_of_each_other() {
        for m in maps() {
            for (lv, l, clear) in [
                ("out", &m.out, 20.),
                ("mid", &m.mid, 12.),
                ("in", &m.inner, 12.),
            ] {
                let mut walls: Vec<Rect> = l.cards.iter().map(|c| c.rect).collect();
                walls.extend(
                    l.frames
                        .iter()
                        .map(|(_, r)| Rect::new(r.x, r.y, r.w, FRAME_HEAD)),
                );
                for set in line_sets(l) {
                    for line in &set {
                        let n = line.points.len();
                        // Level runs but the first (out of its card) and
                        // the last (into its target).
                        for w in line.points[1..n - 1]
                            .windows(2)
                            .filter(|w| w[0].1 == w[1].1)
                        {
                            let (x0, x1, y) = (w[0].0.min(w[1].0), w[0].0.max(w[1].0), w[0].1);
                            for r in &walls {
                                let across = r.x < x1 - 0.5 && x0 + 0.5 < r.right();
                                let near = y > r.y - clear + 0.01 && y < r.bottom() + clear - 0.01;
                                assert!(
                                    !(across && near),
                                    "{lv}: {w:?} is closer than {clear} to {r:?}"
                                );
                            }
                        }
                    }
                    // One port per edge of the focus card.
                    let mut ports: BTreeSet<(i64, i64)> = BTreeSet::new();
                    for line in &set {
                        ports.insert((line.points[0].0 as i64, line.points[0].1 as i64));
                    }
                    assert!(ports.len() <= 2, "{lv}: {ports:?}");
                }
            }
        }
    }

    #[test]
    fn the_notes_in_every_session_keep_the_first_column() {
        for m in [demo_shaped(), synthetic(300, 12, 1000)] {
            let f = m.fitted(1220., 690.);
            let first = &f.arrangement.columns[0];
            assert!(first.iter().all(|k| k.starts_with("note:")), "{first:?}");
            // Folder cards at Out line up: one height on the 300-note map.
            if m.notes.len() == 300 {
                let hs: BTreeSet<i64> = f
                    .out
                    .cards
                    .iter()
                    .filter(|c| matches!(c.item, Item::Folder(_)))
                    .map(|c| c.rect.h as i64)
                    .collect();
                assert_eq!(hs.len(), 1, "{hs:?}");
            }
        }
    }

    #[test]
    fn in_cards_wrap_their_words_and_keep_their_padding() {
        let mut m = demo_shaped();
        m.notes[3].summary = "A summary long enough to need a second line on an In card, and then a little more so it wraps.".into();
        m.notes[3].rules = vec![
            "A kept rule that is long enough that it has to wrap onto a second line on the card."
                .into(),
        ];
        let t = in_text(&m.notes[3]);
        assert!(t.summary.len() >= 2 && t.summary.len() <= 3, "{t:?}");
        assert!(!t.summary.last().unwrap().ends_with('…'));
        assert!(t.rows[0].0.len() >= 2 && !t.rows[0].0.last().unwrap().ends_with('…'));
        // The last row ends 16 points above the card's foot.
        let h = in_height(&m.notes[3]);
        let rows: f32 = t.rows.iter().map(|r| r.0.len() as f32 * IN_LINE).sum();
        let bottom = in_chips_y(&t) + 22. + 14. + rows;
        assert!((h - bottom - 16.).abs() < 0.01, "{h} {bottom}");
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
