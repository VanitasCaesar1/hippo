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
pub const OUT_BELOW: f32 = 0.7;
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
/// Between frames in a column: a channel for lines, as wide as the gap
/// between columns is for their turns, so a line in it keeps clear of
/// both frames.
pub const FRAME_GAP: f32 = 48.;
/// Between columns: room for the lines' turns.
pub const COL_GAP: f32 = 128.;
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
pub const OUT_PAD: f32 = 24.;
/// An Out card's name: its largest and smallest size, on at most 2 lines.
pub const OUT_TITLE: f32 = 40.;
/// The legend's size.
pub const OUT_META: f32 = 26.;
/// A note row in a folder card: its text size and its height.
pub const OUT_ROW: f32 = 28.;
pub const OUT_ROW_H: f32 = 48.;
/// At most this many note rows in a folder card; more show as `+ n more`.
pub const OUT_ROWS: usize = 6;
/// A mark's diameter.
pub const OUT_DOT: f32 = 12.;
/// An Out card's icon, as a share of its name's size.
pub const OUT_ICON: f32 = 0.7;
/// Note rows in a folder card on a map of more blocks than [`OUT_FEW`]:
/// none. Its overview is drawn smaller, so a folder card shows its name
/// and its counts, in a grid of one height on every card.
pub const OUT_ROWS_MANY: usize = 0;
/// Below this zoom an Out card's note rows and counts would be under 11 px
/// on screen, and are not drawn.
pub const OUT_ROWS_BELOW: f32 = 11. / OUT_ROW;
pub const OUT_META_BELOW: f32 = 11. / OUT_META;
pub const OUT_FEW: usize = 8;
/// Between Out cards in a column.
pub const OUT_GAP: f32 = 72.;
/// A line of text is this many times its size high.
pub const LEADING: f32 = 1.25;
/// Points around a map fitted to its view.
pub const FIT_PAD: f32 = 36.;
/// The largest zoom a map fitted at Out takes: just below Mid.
pub const OUT_FIT: f32 = 0.69;
/// At most this many lines show at once, at any zoom: a level with more
/// pairs of linked cards draws only the lines of the card in focus.
pub const LINE_BUDGET: usize = 12;

/// How the map's lines, badges and zoom are drawn, in screen pixels and
/// zoom factors. Hippo.app paints from these and the web demo reads them
/// from the map's JSON, so both draw the same map.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Style {
    /// Every line, at every zoom, px.
    pub line: f32,
    /// A line's rounded corners, px.
    pub corner: f32,
    /// An arrowhead's length, px.
    pub arrow: f32,
    /// A count badge's radius, px, and the room it keeps from any card.
    pub badge: f32,
    pub badge_room: f32,
    /// Port rings show from this zoom.
    pub port_from: f32,
    /// Below these zooms an Out card drops its note rows, then its counts.
    pub rows_below: f32,
    pub meta_below: f32,
    pub out_below: f32,
    pub in_above: f32,
    pub out_fit: f32,
    pub fit_pad: f32,
    /// The zooms + and − step through; the fitted zoom sits between two.
    pub steps: [f32; 13],
}

pub const STYLE: Style = Style {
    line: 1.5,
    corner: 7.,
    arrow: 7.,
    badge: 9.,
    badge_room: 4.,
    port_from: 0.6,
    rows_below: OUT_ROWS_BELOW,
    meta_below: OUT_META_BELOW,
    out_below: OUT_BELOW,
    in_above: IN_ABOVE,
    out_fit: OUT_FIT,
    fit_pad: FIT_PAD,
    steps: [
        0.15, 0.2, 0.25, 0.33, 0.4, 0.5, 0.6, 0.7, 0.85, 1., 1.25, 2., 3.,
    ],
};

impl Style {
    /// The zoom one step in (`dir` > 0) or out from `z`, between `least`
    /// (the fitted zoom) and [`MAX_ZOOM`]. A zoom between two steps goes
    /// to the next one, so every press changes it.
    pub fn step(&self, z: f32, dir: i32, least: f32) -> f32 {
        let next = if dir > 0 {
            self.steps
                .iter()
                .copied()
                .find(|&s| s > z + 0.005)
                .unwrap_or(MAX_ZOOM)
        } else {
            self.steps
                .iter()
                .rev()
                .copied()
                .find(|&s| s < z - 0.005)
                .unwrap_or(least)
        };
        next.clamp(least.min(MAX_ZOOM), MAX_ZOOM)
    }

    /// Where a badge is drawn at zoom `z`: on its trunk's root, its edge
    /// [`Style::badge_room`] px clear of the card and 2 px clear of the
    /// junction where the trunk turns; none when the root is too short.
    pub fn badge_at(&self, b: &Badge, z: f32) -> Option<(f32, f32)> {
        let need = (self.badge_half(b.links) + self.badge_room) / z.max(0.01);
        let len = (b.x1 - b.x0).abs();
        if len < need + (self.badge_half(b.links) + 2.) / z.max(0.01) {
            return None;
        }
        let c = (b.x0 + (b.x1 - b.x0).signum() * need, b.y);
        let (hw, hh) = (
            (self.badge_half(b.links) + 1.) / z.max(0.01),
            (self.badge + 1.) / z.max(0.01),
        );
        let touches = b.near.iter().any(|s| {
            let (x0, x1) = (s[0].min(s[2]), s[0].max(s[2]));
            let (y0, y1) = (s[1].min(s[3]), s[1].max(s[3]));
            x1 >= c.0 - hw && x0 <= c.0 + hw && y1 >= c.1 - hh && y0 <= c.1 + hh
        });
        (!touches).then_some(c)
    }

    /// Half a badge's width, px: a circle for one figure, a pill for more.
    pub fn badge_half(&self, links: usize) -> f32 {
        let digits = links.max(1).to_string().len() as f32;
        self.badge.max(digits * 3.3 + 4.)
    }
}

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
    /// Which way its links run: `from` links to `to` (an arrowhead at
    /// `to`), `to` links to `from` (one at `from`), or both.
    pub out: bool,
    pub back: bool,
}

/// Where a trunk's branch leaves it for more than one link: a round badge
/// at the branch's foot gives the count. It sits in a gutter, `room` map
/// points from the nearest card; Hippo.app and the demo draw it only where
/// [`Style::badge_fits`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Badge {
    /// The trunk's root: from the card's edge `x0` to its first turn `x1`,
    /// level at `y`. Every line of the trunk runs along it.
    pub x0: f32,
    pub x1: f32,
    pub y: f32,
    /// The links the whole trunk stands for.
    pub links: usize,
    pub trunk: usize,
    /// The cards the trunk leads to.
    pub to: Vec<Item>,
    /// The card's other lines near the root, as segments: a badge that
    /// would touch one is not drawn.
    pub near: Vec<[f32; 4]>,
}

/// A mark at a drawn line's end: an arrowhead where its links arrive (at
/// the card the focus links to, or at the focus where a card links to it),
/// or, on a lit line, a dot where they leave. `lit` when the line is lit.
/// Pixels or map points, as the points it was made from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct End {
    pub tip: (f32, f32),
    /// The point before the tip, for the head's direction.
    pub from: (f32, f32),
    pub lit: bool,
    pub head: bool,
}

/// The marks of `lines`, once per point: a head where links arrive (at
/// the line's card for links out, at the focus for links in, both for
/// both); on a lit line (`all_lit`, or one of `hot`) a dot where links
/// leave and none arrive. A head wins over a dot at one point.
pub fn ends(lines: &[&Line], hot: &[Hot], all_lit: bool) -> Vec<End> {
    let mut out: Vec<End> = Vec::new();
    let key = |p: (f32, f32)| ((p.0 * 2.).round() as i64, (p.1 * 2.).round() as i64);
    for l in lines {
        let n = l.points.len();
        if n < 2 {
            continue;
        }
        let lit = all_lit || hot.contains(&l.hot());
        // A head's lit comes from the lines arriving there; a dot never
        // lights a head.
        let mut put = |e: End| match out.iter_mut().find(|o| key(o.tip) == key(e.tip)) {
            Some(o) if e.head && !o.head => *o = e,
            Some(o) if e.head == o.head => o.lit |= e.lit,
            Some(_) => {}
            None => out.push(e),
        };
        let (start, end) = (
            (l.points[0], l.points[1]),
            (l.points[n - 1], l.points[n - 2]),
        );
        if l.out {
            put(End {
                tip: end.0,
                from: end.1,
                lit,
                head: true,
            });
        } else if lit {
            put(End {
                tip: end.0,
                from: end.1,
                lit,
                head: false,
            });
        }
        if l.back {
            put(End {
                tip: start.0,
                from: start.1,
                lit,
                head: true,
            });
        } else if lit {
            put(End {
                tip: start.0,
                from: start.1,
                lit,
                head: false,
            });
        }
    }
    out
}

/// A drawn line as the pointer names it: its ends and whether it carries
/// links out (an out-line and an in-line can join the same two cards).
pub type Hot = (Item, Item, bool);

impl Line {
    pub fn hot(&self) -> Hot {
        (self.from, self.to, self.out)
    }
}

/// Whether a line has any part inside `view`.
pub fn shows(l: &Line, view: &Rect) -> bool {
    l.points.windows(2).any(|w| seg_in(w[0], w[1], view))
}

fn seg_in(a: (f32, f32), b: (f32, f32), v: &Rect) -> bool {
    let (x0, x1) = (a.0.min(b.0), a.0.max(b.0));
    let (y0, y1) = (a.1.min(b.1), a.1.max(b.1));
    x1 >= v.x && x0 <= v.right() && y1 >= v.y && y0 <= v.bottom()
}

/// Where a line that shows in `view` last leaves it on the way to its
/// card, when its card's end is outside the view.
pub fn exit(l: &Line, view: &Rect) -> Option<(f32, f32)> {
    let inside = |p: (f32, f32)| {
        p.0 >= view.x && p.0 <= view.right() && p.1 >= view.y && p.1 <= view.bottom()
    };
    let n = l.points.len();
    if n < 2 || !shows(l, view) || inside(l.points[n - 1]) {
        return None;
    }
    let mut at = None;
    for w in l.points.windows(2) {
        let (a, b) = (w[0], w[1]);
        if !seg_in(a, b, view) || inside(b) {
            continue;
        }
        at = Some(if a.1 == b.1 {
            (
                if b.0 > view.right() {
                    view.right()
                } else {
                    view.x
                },
                a.1,
            )
        } else {
            (
                a.0,
                if b.1 > view.bottom() {
                    view.bottom()
                } else {
                    view.y
                },
            )
        });
    }
    at
}

/// The last `len` map points of a line before it leaves `view`, in `n`
/// pieces from the inside out, each with how much of the ground covers it
/// (rising to 1 at the border): a renderer draws them over the line so it
/// fades out at the canvas edge.
pub fn fade(l: &Line, view: &Rect, len: f32, n: usize) -> Vec<(Vec<(f32, f32)>, f32)> {
    let Some(end) = exit(l, view) else {
        return Vec::new();
    };
    // The polyline up to the exit point.
    let mut pts: Vec<(f32, f32)> = Vec::new();
    for w in l.points.windows(2) {
        let (a, b) = (w[0], w[1]);
        pts.push(a);
        let on = (a.0 - b.0).abs() < 0.01
            && (end.0 - a.0).abs() < 0.01
            && end.1 >= a.1.min(b.1)
            && end.1 <= a.1.max(b.1)
            || (a.1 - b.1).abs() < 0.01
                && (end.1 - a.1).abs() < 0.01
                && end.0 >= a.0.min(b.0)
                && end.0 <= a.0.max(b.0);
        if on && !seg_in(b, b, view) {
            pts.push(end);
            break;
        }
    }
    // Walk back `len` from the end, then cut into `n` pieces.
    let dist = |a: (f32, f32), b: (f32, f32)| (a.0 - b.0).abs() + (a.1 - b.1).abs();
    let total: f32 = pts.windows(2).map(|w| dist(w[0], w[1])).sum();
    let start = (total - len).max(0.);
    let at = |d: f32| {
        let mut run = 0.;
        for w in pts.windows(2) {
            let s = dist(w[0], w[1]);
            if run + s >= d {
                let k = if s > 0. { (d - run) / s } else { 0. };
                return (
                    w[0].0 + (w[1].0 - w[0].0) * k,
                    w[0].1 + (w[1].1 - w[0].1) * k,
                );
            }
            run += s;
        }
        *pts.last().unwrap_or(&end)
    };
    let corners: Vec<(f32, (f32, f32))> = {
        let mut run = 0.;
        let mut v = vec![(0., pts[0])];
        for w in pts.windows(2) {
            run += dist(w[0], w[1]);
            v.push((run, w[1]));
        }
        v
    };
    let n = n.max(1);
    (0..n)
        .map(|i| {
            let (d0, d1) = (
                start + (total - start) * i as f32 / n as f32,
                start + (total - start) * (i + 1) as f32 / n as f32,
            );
            let mut piece = vec![at(d0)];
            piece.extend(corners.iter().filter(|c| c.0 > d0 && c.0 < d1).map(|c| c.1));
            piece.push(at(d1));
            (piece, (i + 1) as f32 / n as f32)
        })
        .collect()
}

/// The part of a line past the point where it leaves `view`: covered by
/// the ground, so nothing of it shows past the fade.
pub fn past(l: &Line, view: &Rect) -> Vec<(f32, f32)> {
    let Some(end) = exit(l, view) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut on = false;
    for w in l.points.windows(2) {
        let (a, b) = (w[0], w[1]);
        if !on {
            let hit = ((a.0 - b.0).abs() < 0.01
                && (end.0 - a.0).abs() < 0.01
                && end.1 >= a.1.min(b.1)
                && end.1 <= a.1.max(b.1))
                || ((a.1 - b.1).abs() < 0.01
                    && (end.1 - a.1).abs() < 0.01
                    && end.0 >= a.0.min(b.0)
                    && end.0 <= a.0.max(b.0));
            if hit && !seg_in(b, b, view) {
                on = true;
                out.push(end);
                out.push(b);
            }
        } else {
            out.push(b);
        }
    }
    out
}

/// Of the lines under the pointer, the one to pan to: the nearest card
/// out of `view` they reach.
pub fn nearest_out(
    lines: &[&Line],
    view: &Rect,
    at: (f32, f32),
    card: impl Fn(Item) -> Option<Rect>,
) -> Option<Item> {
    lines
        .iter()
        .filter(|l| exit(l, view).is_some())
        .filter_map(|l| card(l.to).map(|r| (l.to, r)))
        .min_by(|a, b| {
            let d = |r: &Rect| (r.x + r.w / 2. - at.0).hypot(r.y + r.h / 2. - at.1);
            d(&a.1).total_cmp(&d(&b.1))
        })
        .map(|x| x.0)
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
    /// The count badges drawn with each linked card's lines.
    pub badges: Vec<(Item, Vec<Badge>)>,
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

    /// The badges drawn with `focus`'s lines.
    pub fn badges(&self, focus: Option<Item>) -> &[Badge] {
        focus
            .and_then(|f| self.badges.iter().find(|(i, _)| *i == f))
            .map_or(&[], |(_, bs)| bs.as_slice())
    }

    /// The lines of `focus` under a map point at zoom `z`: a badge (within
    /// its radius) stands for its trunk, a run within `tol` map points for
    /// every line drawn along it. What a status line counts, so a badge and
    /// the run under it give one number.
    pub fn lines_at(&self, focus: Option<Item>, x: f32, y: f32, tol: f32, z: f32) -> Vec<&Line> {
        let (lines, badges) = (self.shown(focus), self.badges(focus));
        let r = STYLE.badge / z.max(0.01);
        if let Some(b) = badges.iter().find(|b| {
            STYLE.badge_at(b, z).is_some_and(|c| {
                (c.0 - x).abs() <= STYLE.badge_half(b.links) / z && (c.1 - y).abs() <= r
            })
        }) {
            return lines
                .iter()
                .filter(|l| l.trunk == b.trunk && b.to.contains(&l.to))
                .collect();
        }
        // The lines nearest the point: lines that run together are as near
        // as each other, so a shared run gives every line along it.
        let dist = |l: &Line| {
            l.points
                .windows(2)
                .map(|w| {
                    let px = x.clamp(w[0].0.min(w[1].0), w[0].0.max(w[1].0));
                    let py = y.clamp(w[0].1.min(w[1].1), w[0].1.max(w[1].1));
                    (px - x).hypot(py - y)
                })
                .fold(f32::MAX, f32::min)
        };
        let hit: Vec<(f32, &Line)> = lines
            .iter()
            .map(|l| (dist(l), l))
            .filter(|h| h.0 <= tol)
            .collect();
        let best = hit.iter().map(|h| h.0).fold(f32::MAX, f32::min);
        hit.into_iter()
            .filter(|h| h.0 <= best + 0.5)
            .map(|h| h.1)
            .collect()
    }

    /// The links lines stand for, once per card they reach.
    pub fn links_of(lines: &[&Line]) -> usize {
        let mut seen = BTreeSet::new();
        lines
            .iter()
            .filter(|l| seen.insert((l.to, l.out)))
            .map(|l| l.links)
            .sum()
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
    /// How it is drawn ([`STYLE`]).
    pub style: Style,
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
        style: STYLE,
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
    out.sort_by(|a, b| {
        b.notes
            .len()
            .cmp(&a.notes.len())
            .then(natural(&a.name, &b.name))
    });
    out
}

/// Names in the order a person counts: `Folder 2` before `Folder 10`.
fn natural(a: &str, b: &str) -> std::cmp::Ordering {
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, _) => return std::cmp::Ordering::Less,
            (_, None) => return std::cmp::Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let take = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut n = String::new();
                    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
                        n.push(c);
                        it.next();
                    }
                    n
                };
                let (p, q) = (take(&mut x), take(&mut y));
                let (p0, q0) = (p.trim_start_matches('0'), q.trim_start_matches('0'));
                let o = p0
                    .len()
                    .cmp(&q0.len())
                    .then(p0.cmp(q0))
                    .then(p.len().cmp(&q.len()));
                if o.is_ne() {
                    return o;
                }
            }
            (Some(c), Some(d)) => {
                let o = c.cmp(&d);
                if o.is_ne() {
                    return o;
                }
                x.next();
                y.next();
            }
        }
    }
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
pub const IN_SUMMARY_SIZE: f32 = 14.;
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
        16.,
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
                (1, 0),
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
            // A big map's overview is drawn smaller: fewer rows.
            let many = folders.len() + pinned_of(notes).len() > OUT_FEW;
            let mut legend: Vec<(String, Option<&'static str>)> =
                vec![(count(f.notes.len(), "note", "notes"), None)];
            for (n, words, tone) in [
                (f.fixed, ("fixed", "fixed"), "ok"),
                (f.waiting, ("waiting", "waiting"), "you"),
                (f.rejected, ("rejected", "rejected"), "bad"),
                (f.stale, ("stale", "stale"), "faint"),
            ] {
                // On a big map's overview each count keeps its place on
                // every card; a zero is left out, its place kept empty.
                if n > 0 {
                    legend.push((
                        format!("{n} {}", if n == 1 { words.0 } else { words.1 }),
                        Some(tone),
                    ));
                } else if many {
                    legend.push((String::new(), None));
                }
            }
            let shown = if many {
                0
            } else if f.notes.len() > OUT_ROWS {
                OUT_ROWS - 1
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
                (2, if many { 3 } else { 0 }),
                &rows,
                if many { 0 } else { f.notes.len() - shown },
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
            // Every frame's head the same: its name, then its count.
            let item = Item::Folder(fi);
            let count = count(f.notes.len(), "note", "notes");
            let cw = super::fit::width(&count, 13., false);
            let name = if f.name.is_empty() { "Notes" } else { &f.name };
            let name_w = (w - 2. * FRAME_PAD - cw - 10.).max(10.);
            let nl = label(
                item,
                LabelKind::Frame,
                name,
                FRAME_PAD,
                11.,
                super::fit::width(name, 14.5, true).min(name_w),
                14.5,
                14.5,
                1,
            );
            let cx = nl.rect.right() + 10.;
            d.labels.push(nl);
            d.labels.push(label(
                item,
                LabelKind::Count,
                &count,
                cx,
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
    let lk = links(level, folders, edges, &frames, &cards);
    let mut lay = finish(frames, cards, labels, marks);
    lay.focus = lk.focus;
    lay.badges = lk.badges;
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
            // At Mid and In a note read in every session that starts a
            // column sits as low as the first cards in a folder's frame.
            let first = if d.pinned && level != Level::Out {
                FRAME_HEAD
            } else {
                0.
            };
            let top = foot[c..c + span]
                .iter()
                .map(|f| f.map_or(first, |(y, p)| y + gap(p, d.pinned)))
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
) -> Focus {
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
    // Each pair of linked cards: the links from the pair's first card to
    // its second, and the other way.
    let mut pairs: BTreeMap<(Item, Item), (usize, usize)> = BTreeMap::new();
    for &(a, b) in edges {
        let (ia, ib) = (lift(a), lift(b));
        if ia == ib || !rect_of.contains_key(&ia) || !rect_of.contains_key(&ib) {
            continue;
        }
        let (key, forward) = if pairs.contains_key(&(ib, ia)) {
            ((ib, ia), false)
        } else {
            ((ia, ib), true)
        };
        let e = pairs.entry(key).or_default();
        if forward {
            e.0 += 1;
        } else {
            e.1 += 1;
        }
    }
    // A frame's name band is in the way of a line as a card is.
    let heads: Vec<Rect> = frames
        .iter()
        .map(|(_, r)| Rect::new(r.x, r.y, r.w, FRAME_HEAD))
        .collect();
    let items: BTreeSet<Item> = pairs.keys().flat_map(|&(a, b)| [a, b]).collect();
    let mut focus = Vec::new();
    let mut badges = Vec::new();
    for f in items {
        let ca = col_of(rect_of[&f].x);
        // One trunk each way: to the right (which also serves cards in its
        // own column) and to the left. A line carries its pair's links both
        // ways; its heads say which way they run.
        let mut sides: [Vec<Target>; 2] = Default::default();
        for (&(a, b), &(fwd, bwd)) in &pairs {
            let (o, n_out, n_in) = if a == f {
                (b, fwd, bwd)
            } else if b == f {
                (a, bwd, fwd)
            } else {
                continue;
            };
            let side = usize::from(col_of(rect_of[&o].x) < ca);
            sides[side].push((o, n_out + n_in, n_out > 0, n_in > 0));
        }
        let mut groups: Vec<Vec<Target>> = sides.into_iter().filter(|g| !g.is_empty()).collect();
        let weight = |g: &Vec<Target>| g.iter().map(|x| x.1).sum::<usize>();
        groups.sort_by_key(|g| std::cmp::Reverse(weight(g)));
        let wants: Vec<Want> = groups
            .into_iter()
            .enumerate()
            .map(|(k, to)| Want { a: f, to, trunk: k })
            .collect();
        let (lines, bs) = route(&wants, &rect_of, &heads, level == Level::Out);
        focus.push((f, lines));
        if !bs.is_empty() {
            badges.push((f, bs));
        }
    }
    Focus { focus, badges }
}

/// Each linked card's lines and badges.
struct Focus {
    focus: Vec<(Item, Vec<Line>)>,
    badges: Vec<(Item, Vec<Badge>)>,
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
        badges: Vec::new(),
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
    (cols, min_rows): (usize, usize),
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
    // The name beside its icon (a folder, the entry note's flag, a note),
    // at one size on every card: two lines, then cut.
    let room = OUT_TITLE * OUT_ICON + 12.;
    let mut t = label(
        item,
        LabelKind::Title,
        title,
        x0 + room,
        cy,
        iw - room,
        OUT_TITLE,
        OUT_TITLE,
        2,
    );
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
    // The legend in a grid of `cols` columns: a mark and its words per
    // cell, at one size, cut where a cell is too narrow.
    // A folder's note count heads it on a row of its own.
    let line_h = OUT_META * LEADING;
    let cols = cols.max(1);
    let gap = 10.;
    let cw = (iw - (cols - 1) as f32 * gap) / cols as f32;
    let head = usize::from(cols > 1 && legend.first().is_some_and(|l| l.1.is_none()));
    let rows_of_legend = (head + (legend.len() - head).div_ceil(cols))
        .max(min_rows)
        .max(1);
    for (k, (words, tone)) in legend.iter().enumerate() {
        if words.is_empty() {
            continue;
        }
        let (row, col) = if k < head {
            (0, 0)
        } else {
            (head + (k - head) / cols, (k - head) % cols)
        };
        let lx = x0 + col as f32 * (cw + gap);
        let ly = cy + row as f32 * (line_h + 4.);
        let lead = if tone.is_some() { OUT_DOT + 6. } else { 0. };
        if let Some(tone) = tone {
            marks.push(Mark {
                item,
                x: lx + OUT_DOT / 2.,
                y: ly + line_h / 2.,
                d: OUT_DOT,
                tone,
                row: false,
            });
        }
        let room = if k < head { iw } else { cw };
        let w = super::fit::width(words, OUT_META, false).min(room - lead);
        labels.push(label(
            item,
            LabelKind::Meta,
            words,
            lx + lead,
            ly,
            w,
            OUT_META,
            OUT_META,
            1,
        ));
    }
    cy += (rows_of_legend - 1) as f32 * (line_h + 4.);
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
            OUT_ROW,
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
            OUT_ROW,
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
    to: Vec<Target>,
    trunk: usize,
}

/// A line's other card, the links it stands for, and their direction from
/// the focus: out (the focus links to it), back (it links to the focus).
type Target = (Item, usize, bool, bool);

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
fn route(
    wants: &[Want],
    rect_of: &HashMap<Item, Rect>,
    heads: &[Rect],
    out: bool,
) -> (Vec<Line>, Vec<Badge>) {
    let clear = if out { 20f32 } else { 12f32 };
    // A card's lines meet it level with its title, at one height on every
    // card of a level.
    let centre = |r: &Rect| {
        if out {
            r.y + 50f32.min(r.h / 2.)
        } else {
            r.y + r.h.min(CARD_H) / 2.
        }
    };
    // Where an edge's lines meet it: a left edge a little above the
    // middle, a right edge a little below, so lines into two cards across
    // a gutter never meet head to head.
    let port = |r: &Rect, right: bool| {
        let d = if out { 12. } else { 10. };
        centre(r) + if right { d } else { -d }
    };
    let spread = if out { 24f32 } else { 14f32 };
    struct Path {
        a: Item,
        ra: Rect,
        to: Vec<(Item, Rect, usize, bool, bool)>,
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
        let to: Vec<(Item, Rect, usize, bool, bool)> =
            w.to.iter()
                .filter_map(|&(i, n, o, b)| rect_of.get(&i).map(|r| (i, *r, n, o, b)))
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
    // An end that is alone on its edge, for snapping.
    let alone: HashMap<(Item, bool), bool> = ends.iter().map(|(k, v)| (*k, v.len() == 1)).collect();
    for ((item, edge), mut list) in ends {
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
        // A card's own trunk leaves it at its middle;
        // lines into a card meet it at its edge's anchor.
        let own = list.iter().any(|e| e.2 == usize::MAX);
        // A card's own trunks (links out, links in) leave well apart.
        let step = if own && n > 1 {
            (r.h * 0.3).max(step).min((hi - lo) / (n - 1) as f32)
        } else {
            step
        };
        let half = step * (n.saturating_sub(1)) as f32 / 2.;
        let at = if own { centre(r) } else { port(r, edge) };
        let mid = at.clamp(lo + half, (hi - half).max(lo + half));
        for (k, (_, path, j)) in list.into_iter().enumerate() {
            let y = mid - half + k as f32 * step;
            if j == usize::MAX {
                paths[path].ya = y;
            } else {
                paths[path].yb[j] = y;
            }
        }
    }
    // A line alone at both its ends between neighbouring columns (or in one
    // column) runs straight when its ends are within 16 points: no
    // dog-leg.
    for p in &mut paths {
        if p.to.len() != 1 {
            continue;
        }
        let (t, r) = (p.to[0].0, p.to[0].1);
        let (ca, tc) = (col_of(p.ra.x), col_of(r.x));
        let near = tc == ca || tc == ca + 1 || tc + 1 == ca;
        let er = !p.right || tc == ca;
        if !near || !alone[&(p.a, p.right)] || !alone[&(t, er)] {
            continue;
        }
        let (ya, yb) = (p.ya, p.yb[0]);
        if (ya - yb).abs() < 16. {
            if ya > r.y + 10. && ya < r.bottom() - 10. {
                p.yb[0] = ya;
            } else if yb > p.ra.y + 10. && yb < p.ra.bottom() - 10. {
                p.ya = yb;
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
        consider(want.max(from.max(0.) + clear * 1.8));
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
    // A target within 16 points of the trunk's height where it branches,
    // alone on its edge, is met level: no stair-step.
    for p in &mut paths {
        for s in 0..p.stops.len() {
            let y = p.stops[s].1;
            for j in p.stops[s].2.clone() {
                let (t, r) = (p.to[j].0, p.to[j].1);
                let er = !p.right || col_of(r.x) == col_of(p.ra.x);
                let d = (p.yb[j] - y).abs();
                let fits = y > r.y + 10. && y < r.bottom() - 10.;
                // Within 16 points and alone on its edge, or within 3 (a
                // jog no one would read): met level.
                if fits && (d < 3. || (d < 16. && alone.get(&(t, er)).copied().unwrap_or(false))) {
                    p.yb[j] = y;
                }
            }
        }
    }
    // Lanes: one vertical per trunk in each gap it uses, 70 % of the way
    // across it from the card's side, so the root holds its badge clear of
    // the junction (a card's trunks to the right and left never share a
    // gap).
    for p in &mut paths {
        let away = if p.right { 1. } else { -1. };
        for (s, (g, ..)) in p.stops.iter().enumerate() {
            let (lo, hi) = gap_span(*g);
            p.lanes[s] = (lo + hi) / 2. + away * 0.2 * COL_GAP;
        }
    }
    let mut lines = Vec::new();
    let mut badges = Vec::new();
    for p in &paths {
        let a0 = if p.right { p.ra.right() } else { p.ra.x };
        let mut trunk = vec![(a0, p.ya)];
        // One badge per trunk, on its root: the stretch from the card to the
        // trunk's first turn, which all its lines share.
        let links: usize = p.to.iter().map(|t| t.2).sum();
        if links >= 2 && !p.lanes.is_empty() {
            let x1 = p.lanes[0];
            badges.push(Badge {
                x0: a0,
                x1,
                y: p.ya,
                links,
                trunk: p.trunk,
                to: p.to.iter().map(|t| t.0).collect(),
                near: Vec::new(),
            });
        }
        for (s, (_, y, js, to)) in p.stops.iter().enumerate() {
            let x = p.lanes[s];
            for &j in js {
                let (item, t, links, out, back) = p.to[j];
                let b0 = if enters_right(p, j) { t.right() } else { t.x };
                let mut points = trunk.clone();
                points.push((x, *y));
                points.push((x, p.yb[j]));
                points.push((b0, p.yb[j]));
                let points = straight(points);
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
                    out,
                    back,
                });
            }
            if to != y {
                trunk.push((x, *y));
                trunk.push((x, *to));
            }
        }
    }
    // Each badge learns the segments of the card's other trunks near its
    // root.
    for b in &mut badges {
        let (bx0, bx1) = (b.x0.min(b.x1), b.x0.max(b.x1));
        let mut near: Vec<[f32; 4]> = Vec::new();
        for l in lines.iter().filter(|l: &&Line| l.trunk != b.trunk) {
            for w in l.points.windows(2) {
                let seg = [w[0].0, w[0].1, w[1].0, w[1].1];
                let (x0, x1) = (seg[0].min(seg[2]), seg[0].max(seg[2]));
                let (y0, y1) = (seg[1].min(seg[3]), seg[1].max(seg[3]));
                if x1 >= bx0 - COL_GAP
                    && x0 <= bx1 + COL_GAP
                    && y1 >= b.y - COL_GAP
                    && y0 <= b.y + COL_GAP
                    && !near.contains(&seg)
                {
                    near.push(seg);
                }
            }
        }
        b.near = near;
    }
    (lines, badges)
}

/// The selection's lines drawn while another card is hovered: none to or
/// from the hovered card, none in a gutter the hovered card's lines run in,
/// and none along a hovered line (within 3 map points). So lime is never
/// drawn beside grey.
pub fn beside<'a>(sel: &[&'a Line], hovered: &[&Line], card: Item) -> Vec<&'a Line> {
    let gutters: BTreeSet<usize> = hovered.iter().flat_map(|h| gutters_of(h)).collect();
    sel.iter()
        .copied()
        .filter(|l| l.to != card && l.from != card)
        .filter(|l| gutters_of(l).is_disjoint(&gutters))
        .filter(|l| {
            !l.points.windows(2).any(|a| {
                hovered.iter().any(|h| {
                    h.points
                        .windows(2)
                        .any(|b| along((a[0], a[1]), (b[0], b[1]), 3.))
                })
            })
        })
        .collect()
}

/// The gaps between columns a line runs in: down, or across.
pub fn gutters_of(l: &Line) -> BTreeSet<usize> {
    let mut g = BTreeSet::new();
    for w in l.points.windows(2) {
        let (x0, x1) = (w[0].0.min(w[1].0), w[0].0.max(w[1].0));
        if x1 - x0 < 0.01 {
            if x0 - col_x(col_of(x0)) > COL_W {
                g.insert(col_of(x0) + 1);
            }
            continue;
        }
        for c in col_of(x0)..=col_of(x1) + 1 {
            let (lo, hi) = (col_x(c) - COL_GAP, col_x(c));
            if c > 0 && x1 > lo + 0.5 && x0 < hi - 0.5 {
                g.insert(c);
            }
        }
    }
    g
}

/// Whether two segments run along each other, within `gap` map points
/// sideways, for more than a point.
pub fn along(a: ((f32, f32), (f32, f32)), b: ((f32, f32), (f32, f32)), gap: f32) -> bool {
    let level = |s: ((f32, f32), (f32, f32))| (s.0 .1 - s.1 .1).abs() < 0.01;
    let plumb = |s: ((f32, f32), (f32, f32))| (s.0 .0 - s.1 .0).abs() < 0.01;
    let span = |p: f32, q: f32| (p.min(q), p.max(q));
    if level(a) && level(b) && (a.0 .1 - b.0 .1).abs() < gap {
        let (a0, a1) = span(a.0 .0, a.1 .0);
        let (b0, b1) = span(b.0 .0, b.1 .0);
        return a1.min(b1) - a0.max(b0) > 1.;
    }
    if plumb(a) && plumb(b) && (a.0 .0 - b.0 .0).abs() < gap {
        let (a0, a1) = span(a.0 .1, a.1 .1);
        let (b0, b1) = span(b.0 .1, b.1 .1);
        return a1.min(b1) - a0.max(b0) > 1.;
    }
    false
}

/// A polyline without repeated points or points in the middle of a
/// straight run: every point it keeps is a corner or an end.
fn straight(points: Vec<(f32, f32)>) -> Vec<(f32, f32)> {
    let mut v: Vec<(f32, f32)> = Vec::with_capacity(points.len());
    for q in points {
        if v.last()
            .is_some_and(|l: &(f32, f32)| (l.0 - q.0).abs() < 0.01 && (l.1 - q.1).abs() < 0.01)
        {
            continue;
        }
        if v.len() >= 2 {
            let (a, b) = (v[v.len() - 2], v[v.len() - 1]);
            let level = (a.1 - b.1).abs() < 0.01 && (b.1 - q.1).abs() < 0.01;
            let plumb = (a.0 - b.0).abs() < 0.01 && (b.0 - q.0).abs() < 0.01;
            if level || plumb {
                v.pop();
            }
        }
        v.push(q);
    }
    v
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
        style: STYLE,
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
                        // Up to the foot: a straight run may have taken
                        // the foot's corner away.
                        let n = a.points.len().min(b.points.len());
                        let same = a
                            .points
                            .iter()
                            .zip(&b.points)
                            .take_while(|(p, q)| p == q)
                            .count();
                        assert!(same + 2 >= n, "{a:?} {b:?}");
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
                    assert!(n <= 4, "{lv}: {:?} shows {n}", c.item);
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
                    assert!(line.points.len() >= 2, "{line:?}");
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
            // Fills the view, unless it already shows at the largest zoom
            // Out has.
            let capped = z >= OUT_FIT - 1e-3;
            assert!(
                (fw > 0.6 && fh > 0.6) || (capped && fw.max(fh) > 0.5),
                "{w}×{h}: fills {fw:.2} × {fh:.2} at {z}"
            );
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
        for l in all_lines(&m.mid).into_iter().filter(|l| l.points.len() > 2) {
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

    #[test]
    fn folders_sort_as_a_person_counts() {
        let mut v = vec!["Folder 10", "Folder 2", "Folder 1", "Alpha", "Folder 02"];
        v.sort_by(|a, b| natural(a, b));
        assert_eq!(
            v,
            ["Alpha", "Folder 1", "Folder 2", "Folder 02", "Folder 10"]
        );
        let m = synthetic(300, 12, 1000);
        let names: Vec<&str> = m.folders.iter().map(|f| f.name.as_str()).collect();
        let at = |n: &str| names.iter().position(|x| *x == n).unwrap();
        // Equal sizes keep counting order.
        if m.folders[at("Folder 2")].notes.len() == m.folders[at("Folder 10")].notes.len() {
            assert!(at("Folder 2") < at("Folder 10"));
        }
    }

    #[test]
    fn start_here_takes_one_line() {
        let mut m = demo_shaped();
        m.notes[0].title = "START HERE".into();
        let m = relaid(m);
        let t = m
            .out
            .labels
            .iter()
            .find(|l| l.item == Item::Note(0) && l.kind == LabelKind::Title)
            .unwrap();
        assert_eq!(t.lines, ["START HERE"]);
    }

    #[test]
    fn a_big_overview_keeps_every_count_in_its_place() {
        let m = synthetic(300, 12, 1000);
        for f in 0..m.folders.len() {
            let card = m.out.card(Item::Folder(f)).unwrap().rect;
            for l in m
                .out
                .labels
                .iter()
                .filter(|l| l.item == Item::Folder(f) && l.kind == LabelKind::Meta)
            {
                let w = &l.lines[0];
                assert!(!w.starts_with("0 "), "folder {f}: {w}");
                // Each kind of count in one place on every card.
                let kind = w.split(' ').nth(1).unwrap_or("").to_owned();
                let at = (l.rect.x - card.x, l.rect.y - card.y);
                let first = m.folders.iter().enumerate().find_map(|(g, _)| {
                    let c = m.out.card(Item::Folder(g))?.rect;
                    m.out
                        .labels
                        .iter()
                        .find(|x| {
                            x.item == Item::Folder(g)
                                && x.kind == LabelKind::Meta
                                && x.lines[0].split(' ').nth(1) == Some(kind.as_str())
                        })
                        .map(|x| (x.rect.x - c.x, x.rect.y - c.y))
                });
                assert_eq!(Some(at), first, "folder {f}: {w}");
            }
        }
    }

    /// How far a point is from a rectangle; 0 inside it.
    fn dist(x: f32, y: f32, r: &Rect) -> f32 {
        let dx = (r.x - x).max(0.).max(x - r.right());
        let dy = (r.y - y).max(0.).max(y - r.bottom());
        dx.hypot(dy)
    }

    /// Each card's lines and badges at a level.
    fn both(l: &Layout) -> Vec<(Item, bool, &[Line], &[Badge])> {
        l.focus
            .iter()
            .map(|(f, ls)| (*f, false, ls.as_slice(), l.badges(Some(*f))))
            .collect()
    }

    /// The zooms a level is drawn at, from the table.
    fn zooms(lv: Level) -> Vec<f32> {
        STYLE
            .steps
            .iter()
            .copied()
            .filter(|&z| Level::of(z) == lv)
            .collect()
    }

    #[test]
    fn one_badge_per_trunk_on_the_run_it_counts() {
        for m in maps() {
            for (lv, l) in [
                (Level::Out, &m.out),
                (Level::Mid, &m.mid),
                (Level::In, &m.inner),
            ] {
                let heads: Vec<Rect> = l
                    .frames
                    .iter()
                    .map(|f| Rect::new(f.1.x, f.1.y, f.1.w, FRAME_HEAD))
                    .collect();
                for (f, h, ls, bs) in both(l) {
                    let trunks: BTreeSet<usize> = bs.iter().map(|b| b.trunk).collect();
                    assert_eq!(
                        trunks.len(),
                        bs.len(),
                        "{lv:?} {f:?}: two badges on one trunk"
                    );
                    for b in bs {
                        let mine: Vec<&Line> = ls.iter().filter(|x| x.trunk == b.trunk).collect();
                        assert_eq!(Layout::links_of(&mine), b.links, "{lv:?} {f:?} {h}");
                        assert!(b.links >= 2);
                        // Every line of the trunk runs along the whole root;
                        // no line of the other trunk does.
                        for x in ls {
                            let (p, q) = (x.points[0], x.points[1]);
                            let along = p == (b.x0, b.y)
                                && q.1 == b.y
                                && (q.0 - p.0).abs() >= (b.x1 - b.x0).abs() - 0.01;
                            assert_eq!(along, x.trunk == b.trunk, "{lv:?} {f:?} {h}: {x:?} {b:?}");
                        }
                        // Where drawn, clear of every card and name band.
                        for z in zooms(lv) {
                            let Some(c) = STYLE.badge_at(b, z) else {
                                continue;
                            };
                            let need = (STYLE.badge_half(b.links) + STYLE.badge_room) / z;
                            for r in l.cards.iter().map(|c| c.rect).chain(heads.iter().copied()) {
                                let d = dist(c.0, c.1, &r);
                                assert!(
                                    d >= need - 0.5,
                                    "{lv:?} {f:?} at {z}: badge {c:?} is {d} from {r:?}"
                                );
                            }
                        }
                    }
                }
            }
        }
        // Badges show on the 300-note overview at its fitted zoom.
        let (w, h) = (1220., 690.);
        let m = synthetic(300, 12, 1000).fitted(w, h);
        let (z, _) = m.fit(w, h);
        let shown = m
            .out
            .badges
            .iter()
            .flat_map(|(_, bs)| bs)
            .filter(|b| STYLE.badge_at(b, z).is_some())
            .count();
        assert!(shown > 0, "no badge fits at {z}");
    }

    #[test]
    fn a_badge_and_its_line_give_one_number() {
        for m in maps() {
            for (lv, l) in [
                (Level::Out, &m.out),
                (Level::Mid, &m.mid),
                (Level::In, &m.inner),
            ] {
                for (f, h, ls, bs) in both(l) {
                    for b in bs {
                        for z in zooms(lv) {
                            let Some(c) = STYLE.badge_at(b, z) else {
                                continue;
                            };
                            // On the badge.
                            let on = l.lines_at(Some(f), c.0, c.1, 6. / z, z);
                            // On the line beside it, outside the badge.
                            let beside: Vec<&Line> = ls
                                .iter()
                                .filter(|x| {
                                    x.points.windows(2).any(|w| {
                                        w[0].1 == c.1
                                            && w[1].1 == c.1
                                            && c.0 >= w[0].0.min(w[1].0) - 0.01
                                            && c.0 <= w[0].0.max(w[1].0) + 0.01
                                    })
                                })
                                .collect();
                            assert_eq!(Layout::links_of(&on), b.links, "{lv:?} {f:?} {h} at {z}");
                            assert_eq!(
                                Layout::links_of(&beside),
                                b.links,
                                "{lv:?} {f:?} {h} at {z}"
                            );
                        }
                    }
                    // Along a line's last stub, the status names its card.
                    for x in ls.iter().take(4) {
                        let n = x.points.len();
                        let (a, c) = (x.points[n - 2], x.points[n - 1]);
                        let under = l.lines_at(Some(f), (a.0 + c.0) / 2., a.1, 1., 1.);
                        assert!(under.iter().any(|u| u.to == x.to), "{lv:?} {f:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn heads_sit_where_links_arrive_at_most_one_per_card_side() {
        for m in maps() {
            for (lv, l) in [("out", &m.out), ("mid", &m.mid), ("in", &m.inner)] {
                for (f, _, ls, _) in both(l) {
                    let all: Vec<&Line> = ls.iter().collect();
                    let heads = ends(&all, &[], false);
                    for x in &all {
                        assert!(x.out || x.back, "{lv} {f:?}: a line with no way");
                        if x.out {
                            let at = *x.points.last().unwrap();
                            assert!(
                                heads.iter().any(|e| e.tip == at),
                                "{lv} {f:?}: no head at its card"
                            );
                        }
                        if x.back {
                            assert!(
                                heads.iter().any(|e| e.tip == x.points[0]),
                                "{lv} {f:?}: no head at the focus"
                            );
                        }
                    }
                    // At most one head per card side.
                    let edge = |p: (f32, f32)| {
                        l.cards
                            .iter()
                            .find(|c| {
                                (c.rect.x == p.0 || c.rect.right() == p.0)
                                    && p.1 > c.rect.y
                                    && p.1 < c.rect.bottom()
                            })
                            .map(|c| (c.item, c.rect.x == p.0))
                    };
                    let mut seen = BTreeSet::new();
                    for e in &heads {
                        let k = edge(e.tip).expect("a head on a card's edge");
                        assert!(
                            seen.insert(format!("{k:?}")),
                            "{lv} {f:?}: two heads on {k:?}"
                        );
                    }
                    // The pointer on one line lights only its heads.
                    for x in all.iter().take(3) {
                        let n = x.points.len();
                        let (a, c) = (x.points[n - 2], x.points[n - 1]);
                        let under = l.lines_at(Some(f), (a.0 + c.0) / 2., a.1, 0.5, 1.);
                        let hot: Vec<Hot> = under.iter().map(|u| u.hot()).collect();
                        let key =
                            |p: (f32, f32)| ((p.0 * 2.).round() as i64, (p.1 * 2.).round() as i64);
                        let marks = ends(&all, &hot, false);
                        let lit: BTreeSet<(i64, i64)> = marks
                            .iter()
                            .filter(|e| e.lit && e.head)
                            .map(|e| key(e.tip))
                            .collect();
                        // A lit line's source end has a dot unless links
                        // arrive there too.
                        for u in &under {
                            let n = u.points.len();
                            let src = if u.out && !u.back {
                                Some(u.points[0])
                            } else if u.back && !u.out {
                                Some(u.points[n - 1])
                            } else {
                                None
                            };
                            if let Some(p) = src {
                                assert!(
                                    marks
                                        .iter()
                                        .any(|e| key(e.tip) == key(p) && (e.head || e.lit)),
                                    "{lv} {f:?}: no dot where a lit line leaves"
                                );
                            }
                        }
                        let mut want: BTreeSet<(i64, i64)> = BTreeSet::new();
                        for u in &under {
                            if u.out {
                                want.insert(key(*u.points.last().unwrap()));
                            }
                            if u.back {
                                want.insert(key(u.points[0]));
                            }
                        }
                        assert_eq!(lit, want, "{lv} {f:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn every_trunk_shows_its_badge_at_the_fit_clear_of_the_junction() {
        for (m, (w, h)) in [
            (synthetic(300, 12, 1000).fitted(1220., 690.), (1220., 690.)),
            // Hippo.app's map view in a 1280 × 820 window.
            (synthetic(300, 12, 1000).fitted(884., 534.), (884., 534.)),
            (demo_shaped().fitted(860., 600.), (860., 600.)),
            (demo_shaped().fitted(884., 534.), (884., 534.)),
        ] {
            let (z, pan) = m.fit(w, h);
            let l = m.layout(Level::of(z));
            let view = Rect::new(pan.0, pan.1, w / z, h / z);
            for (f, ls) in &l.focus {
                let trunks: BTreeSet<usize> = ls.iter().map(|x| x.trunk).collect();
                for t in trunks {
                    let mine: Vec<&Line> = ls.iter().filter(|x| x.trunk == t).collect();
                    let bs: Vec<&Badge> =
                        l.badges(Some(*f)).iter().filter(|b| b.trunk == t).collect();
                    if Layout::links_of(&mine) < 2 {
                        assert!(bs.is_empty());
                        continue;
                    }
                    assert_eq!(bs.len(), 1, "{f:?} trunk {t}: one badge");
                    let b = bs[0];
                    let c = STYLE
                        .badge_at(b, z)
                        .unwrap_or_else(|| panic!("{f:?} trunk {t}: no badge at the fit {z}"));
                    // Clear of the junction where the trunk turns.
                    let far = (c.0 - b.x0).abs() + STYLE.badge_half(b.links) / z;
                    assert!(
                        far + 2. / z <= (b.x1 - b.x0).abs() + 0.01,
                        "{f:?}: badge on the junction"
                    );
                    // In the view, 8 px inside it.
                    let m8 = 8. / z;
                    assert!(
                        c.0 > view.x + m8 && c.0 < view.right() - m8,
                        "{f:?}: badge at the canvas edge"
                    );
                }
            }
        }
    }

    #[test]
    fn one_trunk_per_card_per_gutter() {
        for m in maps() {
            for (lv, l) in [("out", &m.out), ("mid", &m.mid), ("in", &m.inner)] {
                for (f, ls) in &l.focus {
                    let trunks: BTreeSet<usize> = ls.iter().map(|x| x.trunk).collect();
                    assert!(trunks.len() <= 2, "{lv} {f:?}");
                    // In each gutter, one vertical.
                    let mut lanes: BTreeMap<usize, BTreeSet<i64>> = BTreeMap::new();
                    for x in ls {
                        for w in x
                            .points
                            .windows(2)
                            .filter(|w| w[0].0 == w[1].0 && w[0].1 != w[1].1)
                        {
                            lanes
                                .entry(col_of(w[0].0) + 1)
                                .or_default()
                                .insert((w[0].0 * 10.).round() as i64);
                        }
                    }
                    for (g, xs) in lanes {
                        assert_eq!(
                            xs.len(),
                            1,
                            "{lv} {f:?}: {} verticals in gutter {g}",
                            xs.len()
                        );
                    }
                }
            }
        }
    }

    /// The views a person sees of `l` at zoom `z` in a 860 × 600 canvas:
    /// centred on each of a few cards.
    fn views(l: &Layout, z: f32) -> Vec<Rect> {
        let (w, h) = (860. / z, 600. / z);
        let step = (l.cards.len() / 6).max(1);
        l.cards
            .iter()
            .step_by(step)
            .map(|c| {
                Rect::new(
                    c.rect.x + c.rect.w / 2. - w / 2.,
                    c.rect.y + c.rect.h.min(CARD_H) / 2. - h / 2.,
                    w,
                    h,
                )
            })
            .collect()
    }

    #[test]
    fn every_line_that_leaves_the_view_fades_out_at_its_edge() {
        let m = synthetic(300, 12, 1000).fitted(1220., 690.);
        let d = demo_shaped();
        for map in [&m, &d] {
            for z in STYLE.steps {
                let l = map.layout(Level::of(z));
                for v in views(l, z) {
                    for (f, _, ls, _) in both(l).into_iter().step_by(5) {
                        for x in ls.iter().filter(|x| shows(x, &v)) {
                            let end = *x.points.last().unwrap();
                            let out = end.0 < v.x
                                || end.0 > v.right()
                                || end.1 < v.y
                                || end.1 > v.bottom();
                            let tail = fade(x, &v, 24. / z, 4);
                            assert_eq!(out, !tail.is_empty(), "{f:?} at {z}");
                            if !out {
                                continue;
                            }
                            // The tail ends on the border, fully covered there.
                            let (last, k) = tail.last().unwrap();
                            let p = *last.last().unwrap();
                            let on = (p.0 - v.x).abs() < 0.01
                                || (p.0 - v.right()).abs() < 0.01
                                || (p.1 - v.y).abs() < 0.01
                                || (p.1 - v.bottom()).abs() < 0.01;
                            assert!(on && *k == 1., "{f:?} at {z}: {p:?} {v:?}");
                            // And no longer than asked.
                            let len: f32 = tail
                                .iter()
                                .flat_map(|(ps, _)| {
                                    ps.windows(2)
                                        .map(|w| (w[0].0 - w[1].0).abs() + (w[0].1 - w[1].1).abs())
                                })
                                .sum();
                            assert!(len <= 24. / z + 0.5, "{f:?} at {z}: tail {len}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn every_visible_badge_has_a_line_attached() {
        let m = synthetic(300, 12, 1000).fitted(1220., 690.);
        let d = demo_shaped();
        for map in [&m, &d] {
            for z in STYLE.steps {
                let l = map.layout(Level::of(z));
                for v in views(l, z) {
                    let margin = 8. / z;
                    for (f, h, ls, bs) in both(l) {
                        for b in bs {
                            let Some(c) = STYLE.badge_at(b, z) else {
                                continue;
                            };
                            let (hw, hh) = (STYLE.badge_half(b.links) / z, STYLE.badge / z);
                            let inside = c.0 - hw >= v.x + margin
                                && c.0 + hw <= v.right() - margin
                                && c.1 - hh >= v.y + margin
                                && c.1 + hh <= v.bottom() - margin;
                            if !inside {
                                continue;
                            }
                            let attached = ls.iter().filter(|x| shows(x, &v)).any(|x| {
                                x.points.windows(2).any(|w| {
                                    w[0].1 == c.1
                                        && w[1].1 == c.1
                                        && c.0 >= w[0].0.min(w[1].0) - 0.5
                                        && c.0 <= w[0].0.max(w[1].0) + 0.5
                                })
                            });
                            assert!(attached, "{f:?} {h} at {z}: badge {c:?} with no line");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn under_a_hover_lime_is_never_drawn_beside_grey() {
        let small = [
            demo_shaped(),
            synthetic(20, 4, 24),
            synthetic(40, 5, 60).fitted(1220., 690.),
        ];
        let big = synthetic(300, 12, 1000).fitted(1220., 690.);
        let mut cases: Vec<&Layout> = Vec::new();
        for m in &small {
            cases.extend([&m.out, &m.mid, &m.inner]);
        }
        cases.push(&big.out);
        for l in cases {
            for (a, hov) in &l.focus {
                let hov: Vec<&Line> = hov.iter().collect();
                let used: BTreeSet<usize> = hov.iter().flat_map(|h| gutters_of(h)).collect();
                for (b, sel) in &l.focus {
                    if a == b {
                        continue;
                    }
                    let all: Vec<&Line> = sel.iter().collect();
                    for y in beside(&all, &hov, *a) {
                        assert!(y.to != *a && y.from != *a);
                        assert!(
                            gutters_of(y).is_disjoint(&used),
                            "selected {b:?} shares a gutter with hovered {a:?}"
                        );
                        for x in &hov {
                            for s in x.points.windows(2) {
                                for t in y.points.windows(2) {
                                    assert!(!along((s[0], s[1]), (t[0], t[1]), 3.), "{a:?} {b:?}");
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn a_card_counts_the_same_at_every_zoom() {
        for m in [
            demo_shaped(),
            synthetic(40, 5, 60),
            synthetic(300, 12, 1000),
        ] {
            let pinned: Vec<usize> = (0..m.notes.len()).filter(|&i| m.pinned(i)).collect();
            for i in pinned {
                let per_side = |l: &Layout| {
                    let it = Item::Note(i);
                    let own = l.card(it).map(|c| c.rect);
                    let mut v: BTreeMap<bool, usize> = BTreeMap::new();
                    for b in l.badges(Some(it)) {
                        *v.entry(b.x1 > b.x0).or_default() += b.links;
                    }
                    (own.is_some(), v)
                };
                let (o, m1, n) = (per_side(&m.out), per_side(&m.mid), per_side(&m.inner));
                assert!(o.0 && m1.0 && n.0);
                assert_eq!(o.1, m1.1, "note {i}: Out and Mid count differently");
                assert_eq!(m1.1, n.1, "note {i}: Mid and In count differently");
            }
        }
    }

    #[test]
    fn every_line_drawn_ends_with_room_for_a_head() {
        for m in maps() {
            for (lv, l) in [("out", &m.out), ("mid", &m.mid), ("in", &m.inner)] {
                let z = match lv {
                    "out" => STYLE.steps[2],
                    "mid" => OUT_BELOW,
                    _ => IN_ABOVE,
                };
                for (_, _, ls, _) in both(l) {
                    for line in ls {
                        assert!(line.out || line.back, "{lv}: a line with no way");
                        let n = line.points.len();
                        let (a, b) = (line.points[n - 2], line.points[n - 1]);
                        let r = l.card(line.to).unwrap().rect;
                        assert_eq!(a.1, b.1, "{lv}: {line:?} ends on a turn");
                        assert!(
                            b.0 == r.x || b.0 == r.right(),
                            "{lv}: {line:?} misses {r:?}"
                        );
                        assert!(b.1 > r.y && b.1 < r.bottom(), "{lv}: {line:?}");
                        assert!(
                            (a.0 - b.0).abs() * z >= STYLE.arrow,
                            "{lv}: no room for a head: {line:?}"
                        );
                        let (p, q) = (line.points[0], line.points[1]);
                        let r = l.card(line.from).unwrap().rect;
                        assert_eq!(p.1, q.1);
                        assert!(p.0 == r.x || p.0 == r.right());
                        assert!((p.0 - q.0).abs() * z >= STYLE.arrow, "{lv}: {line:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn zoom_steps_are_one_table() {
        let st = &STYLE;
        assert!(st.steps.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(st.steps[0], MIN_ZOOM);
        assert_eq!(*st.steps.last().unwrap(), MAX_ZOOM);
        // Mid and In each have a step that reads.
        assert!(st.steps.iter().any(|&s| Level::of(s) == Level::Mid));
        assert!(st.steps.iter().any(|&s| Level::of(s) == Level::In));
        // From a fitted zoom between two steps, + and − go to the next
        // step and never below the fit.
        let fit = 0.44;
        assert_eq!(st.step(fit, 1, fit), 0.5);
        assert_eq!(st.step(fit, -1, fit), fit);
        assert_eq!(st.step(1., 1, fit), 1.25);
        assert_eq!(st.step(1., -1, fit), 0.85);
        assert_eq!(st.step(3., 1, fit), 3.);
        // The JSON carries the table, so the demo steps through the same.
        let json = serde_json::to_value(STYLE).unwrap();
        assert_eq!(json["steps"].as_array().unwrap().len(), st.steps.len());
    }

    #[test]
    fn heads_never_meet_across_a_gutter() {
        // Lines into the cards either side of a gutter meet them at
        // different heights, so no two arrowheads sit back to back.
        let m = synthetic(300, 12, 1000).fitted(1220., 690.);
        for l in [&m.out, &m.mid] {
            for (f, ls) in &l.focus {
                let ends: Vec<(f32, f32, f32)> = ls
                    .iter()
                    .map(|x| {
                        let n = x.points.len();
                        (x.points[n - 1].0, x.points[n - 2].0, x.points[n - 1].1)
                    })
                    .collect();
                for a in &ends {
                    for b in &ends {
                        // Facing: heads pointing at each other in one gap.
                        let facing = (a.0 - b.0).abs() <= COL_GAP + 0.5
                            && (a.0 - b.0).abs() > 0.5
                            && (a.1 - b.1).abs() < 0.5
                            && ((a.0 < a.1) != (b.0 < b.1));
                        if facing {
                            assert!((a.2 - b.2).abs() >= 8., "{f:?}: heads meet at {a:?} {b:?}");
                        }
                    }
                }
            }
        }
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
    fn every_card_sets_its_words_at_one_size_per_level() {
        for m in maps() {
            for lab in &m.out.labels {
                let want = match lab.kind {
                    LabelKind::Title => OUT_TITLE,
                    LabelKind::Meta => OUT_META,
                    LabelKind::Row | LabelKind::More => OUT_ROW,
                    _ => continue,
                };
                assert_eq!(lab.size, want, "{lab:?}");
                assert!(lab.lines.len() <= 2);
                // Counts are never cut.
                if lab.kind == LabelKind::Meta {
                    assert!(!lab.lines[0].ends_with('…'), "{lab:?}");
                }
            }
            for lab in &m.mid.labels {
                let want = match lab.kind {
                    LabelKind::Title => 16.,
                    LabelKind::Frame => 14.5,
                    LabelKind::Count => 13.,
                    _ => continue,
                };
                assert_eq!(lab.size, want, "{lab:?}");
            }
        }
    }

    /// Fitted, the overview's smallest words (counts and note rows) are 11
    /// px or more on screen whenever they are drawn.
    #[test]
    fn the_overview_never_sets_words_under_eleven_pixels() {
        for n in [20usize, 60, 150, 300] {
            for (w, h) in [(860., 544.), (1220., 690.)] {
                let m = synthetic(n, 12, n * 3).fitted(w, h);
                let (z, _) = m.fit(w, h);
                if Level::of(z) != Level::Out || z < OUT_META_BELOW {
                    continue;
                }
                let rows = z >= OUT_ROWS_BELOW;
                let small = if rows {
                    OUT_META.min(OUT_ROW)
                } else {
                    OUT_META
                };
                assert!(small * z >= 11. - 0.01, "{n} notes at {w}×{h}: {z}");
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
                    assert!(ports.len() <= 4, "{lv}: {ports:?}");
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
