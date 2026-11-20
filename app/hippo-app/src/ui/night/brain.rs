//! Brain: the notes as a map that zooms.
//!
//! GPUI cannot scale a subtree, so the map is painted on one canvas from
//! the layout `hippo_view::night::map` computes: every rectangle, line,
//! corner and text size is the layout's number times the zoom. Three levels
//! (folders, note cards, cards with their rules) each have their own
//! layout; crossing a level keeps the card under the pointer under the
//! pointer. Every word on the map is one of the layout's labels, already
//! fitted to its box; this file only draws. Only what meets the view is
//! drawn. Below 900 points the map becomes a board of cards.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Instant;

use gpui::{
    AnyElement, Bounds, Context, Font, FontWeight, Hsla, Pixels, SharedString, TextRun, Window,
    canvas, div, point, prelude::*, px, size,
};
use hippo_view::night::map::{
    self, Arrangement, Block, CARD_H, Drop, IN_LINE, Item, LEADING, Label, LabelKind, Layout,
    Level, MapNote, NotesMap, Nudge,
};
use hippo_view::night::note::NotePanel;

use super::kit::{self, Btn, Nt, Tone};
use crate::ui::Shell;
use crate::ui::theme::Look;

/// The header's height above the map, in points.
pub const HEAD: f32 = 118.;
/// Zoom steps for the buttons and keys.
const STEP: f32 = 1.25;
/// The foot of the map the Ask bar floats over: a fitted map stays above it.
pub const FOOT: f32 = 96.;
/// A lifted block is drawn this much larger, raised over the map.
const LIFT_SCALE: f32 = 1.03;
/// How long blocks take to slide aside, a dropped block to settle, and a
/// block put back to fly home, in milliseconds.
const SLIDE_MS: f32 = 150.;
const SETTLE_MS: f32 = 180.;
const BACK_MS: f32 = 220.;

/// Brain's pan, zoom and drag.
pub struct MapView {
    pub zoom: f32,
    /// The map point at the view's top left.
    pub pan: (f32, f32),
    /// Fitted to the view once.
    pub fitted: bool,
    /// The press that may become a drag.
    pub press: Option<Press>,
    /// The block whose handle the press is on: a drag lifts it.
    pub press_block: Option<Block>,
    /// A block being dragged to another column or place.
    pub lift: Option<Lift>,
    /// Each workspace's arrangement, as the preferences hold them.
    pub layouts: std::collections::BTreeMap<String, Arrangement>,
    /// The line at the map's foot after a reset, with the layout Undo
    /// puts back: (sequence, words, the layout).
    pub toast: Option<(u64, String, Option<Arrangement>)>,
    pub toast_seq: u64,
    /// The view's origin and size in window pixels, from the last paint.
    pub view: Rc<Cell<(f32, f32, f32, f32)>>,
    /// Paint times and frame intervals, for `HIPPO_APP_TIMING`.
    pub frames: Rc<RefCell<Frames>>,
    /// The map in use (cloned once per reload).
    pub map: Option<Rc<NotesMap>>,
    /// The panel of the selected note, read when the selection changes.
    pub panel: Option<(PathBuf, Option<NotePanel>)>,
    /// The card under the pointer while no button is down.
    pub hover: Option<Item>,
    /// At Out, the lines under the pointer (their ends; several where a
    /// trunk is shared) and the view point.
    pub hover_line: Option<HoverLine>,
    /// Blocks sliding to their places.
    pub slide: Option<Slide>,
    /// The map view's size when it was last fitted: a reloaded map is laid
    /// out for it.
    pub view_size: Option<(f32, f32)>,
}

/// Blocks sliding to their places in `lay`: each one's offset from there
/// when it started; a dropped block settling from larger; the drop a
/// lifted block's slide previews.
#[derive(Clone)]
pub struct Slide {
    pub level: Level,
    pub lay: Rc<Layout>,
    pub from: std::collections::HashMap<Block, (f32, f32)>,
    pub settle: Option<(Block, f32)>,
    pub target: Option<(usize, usize)>,
    pub start: Instant,
    pub ms: f32,
}

impl Slide {
    /// How far along, eased out (cubic), 0 to 1. Dev-only
    /// `HIPPO_APP_DEV_PROGRESS=0.5` holds every motion at that point, for
    /// screenshots of a window macOS may not draw again.
    pub fn eased(&self) -> f32 {
        let t = if let Some(p) = dev_progress() {
            p
        } else if self.ms <= 0. {
            1.
        } else {
            (self.start.elapsed().as_secs_f32() * 1000. / self.ms).min(1.)
        };
        1. - (1. - t).powi(3)
    }
    pub fn done(&self) -> bool {
        if let Some(p) = dev_progress() {
            return p >= 1.;
        }
        self.ms <= 0. || self.start.elapsed().as_secs_f32() * 1000. >= self.ms
    }
    /// Where block `b` is drawn now, offset from its place in `lay`.
    pub fn offset(&self, b: Block) -> (f32, f32) {
        let e = self.eased();
        self.from
            .get(&b)
            .map_or((0., 0.), |d| (d.0 * (1. - e), d.1 * (1. - e)))
    }
}

fn dev_progress() -> Option<f32> {
    std::env::var("HIPPO_APP_DEV_PROGRESS")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .map(|p| p.clamp(0., 1.))
}

/// Whether to keep motion out: macOS Reduce Motion, or
/// `HIPPO_APP_DEV_REDUCE_MOTION=1`.
pub fn reduce_motion() -> bool {
    if let Ok(v) = std::env::var("HIPPO_APP_DEV_REDUCE_MOTION") {
        return v == "1";
    }
    objc2_app_kit::NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion()
}

/// A motion's length: none under Reduce Motion; dev-only
/// `HIPPO_APP_DEV_SLOW=<factor>` stretches it for screenshots.
fn motion_ms(ms: f32) -> f32 {
    if reduce_motion() {
        return 0.;
    }
    ms * std::env::var("HIPPO_APP_DEV_SLOW")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(1.)
}

impl Default for MapView {
    fn default() -> MapView {
        MapView {
            zoom: 1.,
            pan: (-24., -8.),
            fitted: false,
            press: None,
            press_block: None,
            lift: None,
            layouts: Default::default(),
            toast: None,
            toast_seq: 0,
            view: Rc::new(Cell::new((0., 0., 1., 1.))),
            frames: Rc::new(RefCell::new(Frames::default())),
            map: None,
            panel: None,
            hover: None,
            hover_line: None,
            slide: None,
            view_size: None,
        }
    }
}

/// Lines under the pointer at Out: their ends (several where a trunk is
/// shared) and the view point.
pub type HoverLine = (Vec<(Item, Item)>, (f32, f32));

/// A press on the map: where (view points), the pan then, and whether it
/// has moved far enough to be a drag.
pub type Press = ((f32, f32), (f32, f32), bool);

/// A block lifted by a drag: which, at what level, where on it the press
/// was (map units from its top left), the map point under the pointer, and
/// where it would land.
#[derive(Debug, Clone)]
pub struct Lift {
    pub block: Block,
    pub key: String,
    pub level: Level,
    pub grab: (f32, f32),
    pub at: (f32, f32),
    pub drop: Drop,
}

/// What the frame-time check records.
#[derive(Default, Debug, Clone)]
pub struct Frames {
    /// Milliseconds each paint of the map took.
    pub paint_ms: Vec<f32>,
    /// Milliseconds between two paints.
    pub interval_ms: Vec<f32>,
    pub last: Option<Instant>,
    /// Cards and lines drawn in the last paint.
    pub drawn: (usize, usize),
}

impl MapView {
    pub fn level(&self) -> Level {
        Level::of(self.zoom)
    }

    /// Zooms to `zoom` about a point of the view (`(px, py)` in points from
    /// its top left), keeping what is under it there, across levels too.
    pub fn zoom_at(&mut self, map: &NotesMap, zoom: f32, at: (f32, f32)) {
        let zoom = zoom.clamp(map::MIN_ZOOM, map::MAX_ZOOM);
        let (from, to) = (self.level(), Level::of(zoom));
        let m = (self.pan.0 + at.0 / self.zoom, self.pan.1 + at.1 / self.zoom);
        let m2 = if from == to {
            m
        } else {
            map.anchor(from, to, m.0, m.1).unwrap_or(m)
        };
        self.zoom = zoom;
        self.pan = (m2.0 - at.0 / zoom, m2.1 - at.1 / zoom);
    }

    /// Fits the whole map in a `w` × `h` view.
    pub fn fit(&mut self, map: &NotesMap, w: f32, h: f32) {
        let (z, pan) = map.fit(w, (h - FOOT).max(100.));
        self.zoom = z;
        self.pan = pan;
        self.fitted = true;
    }

    /// Pans by a drag or scroll of `(dx, dy)` points.
    pub fn pan_by(&mut self, dx: f32, dy: f32) {
        self.pan.0 -= dx / self.zoom;
        self.pan.1 -= dy / self.zoom;
    }

    /// The map point under a view point.
    pub fn to_map(&self, at: (f32, f32)) -> (f32, f32) {
        (self.pan.0 + at.0 / self.zoom, self.pan.1 + at.1 / self.zoom)
    }

    /// Zooms into a card: a folder to Mid, a note to In, centred.
    pub fn zoom_into(&mut self, map: &NotesMap, item: Item, w: f32, h: f32) {
        let (target, level) = match item {
            Item::Folder(_) => (1.0f32, Level::Mid),
            Item::Note(_) => (map::IN_ABOVE + 0.2, Level::In),
        };
        let item = map.same_item(item, level);
        // A folder at Mid is a frame, not a card.
        let rect = match item {
            Item::Folder(f) => map
                .layout(level)
                .frames
                .iter()
                .find(|(i, _)| *i == f)
                .map(|(_, r)| *r),
            _ => map.layout(level).card(item).map(|c| c.rect),
        };
        let Some(r) = rect else {
            return;
        };
        let fit = ((w - 80.) / r.w).min((h - 80.) / r.h).min(target);
        let z = if matches!(item, Item::Folder(_)) {
            fit.max(map::OUT_BELOW + 0.05)
        } else {
            target
        };
        self.zoom = z.clamp(map::MIN_ZOOM, map::MAX_ZOOM);
        self.pan = (
            r.x + r.w / 2. - w / 2. / self.zoom,
            r.y + r.h / 2. - h / 2. / self.zoom,
        );
    }
}

/// What a paint of the map needs, cloned into the canvas.
struct Paint {
    map: Rc<NotesMap>,
    zoom: f32,
    pan: (f32, f32),
    selected: Option<usize>,
    neighbours: Vec<usize>,
    t: Nt,
    unit: f32,
    family: SharedString,
    view: Rc<Cell<(f32, f32, f32, f32)>>,
    frames: Rc<RefCell<Frames>>,
    lift: Option<Lift>,
    /// The card whose lines show: the hovered one, else the selected
    /// note's (its block at Out).
    focus: Option<Item>,
    hover: Option<Item>,
    hover_line: Option<HoverLine>,
    slide: Option<Slide>,
}

fn font(family: &SharedString, weight: FontWeight) -> Font {
    let mut f = gpui::font(family.clone());
    f.weight = weight;
    f
}

/// Paints one line of text at `(x, y)` (pixels), cut with `…` to `max`.
#[allow(clippy::too_many_arguments)]
fn text(
    window: &mut Window,
    cx: &mut gpui::App,
    s: &str,
    x: f32,
    y: f32,
    size_px: f32,
    color: Hsla,
    f: &Font,
    max: f32,
) -> f32 {
    if size_px < 4. || s.is_empty() || max < 6. {
        return 0.;
    }
    // Sizes on a half-pixel grid, so a zoom gesture reuses shaped lines.
    let size_px = (size_px * 2.).round() / 2.;
    let run = |len| TextRun {
        len,
        font: f.clone(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let full: SharedString = s.to_owned().into();
    let mut line =
        window
            .text_system()
            .shape_line(full.clone(), px(size_px), &[run(full.len())], None);
    if f32::from(line.width) > max {
        let ell = "…";
        let cut = line.closest_index_for_x(px((max - size_px * 0.8).max(0.)));
        let mut end = cut.min(s.len());
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        let cut_text: SharedString = format!("{}{ell}", s[..end].trim_end()).into();
        line = window.text_system().shape_line(
            cut_text.clone(),
            px(size_px),
            &[run(cut_text.len())],
            None,
        );
    }
    let w = f32::from(line.width);
    let _ = line.paint(
        point(px(x), px(y)),
        px(size_px * 1.25),
        gpui::TextAlign::Left,
        None,
        window,
        cx,
    );
    w
}

fn rrect(
    window: &mut Window,
    b: Bounds<Pixels>,
    r: f32,
    fill: Hsla,
    border: Hsla,
    bw: f32,
    dashed: bool,
) {
    window.paint_quad(gpui::quad(
        b,
        px(r),
        fill,
        px(bw),
        border,
        if dashed {
            gpui::BorderStyle::Dashed
        } else {
            gpui::BorderStyle::Solid
        },
    ));
}

/// A chip: its ground, a dot for a tinted one, its words. Returns its width.
#[allow(clippy::too_many_arguments)]
fn chip(
    window: &mut Window,
    cx: &mut gpui::App,
    p: &Paint,
    words: &str,
    tone: Tone,
    x: f32,
    y: f32,
    z: f32,
    max: f32,
) -> f32 {
    let (bg, fg) = tone.colors(&p.t);
    let u = p.unit * z;
    let h = 22. * u;
    let size_px = 12. * u;
    let dot = matches!(tone, Tone::You | Tone::Ok | Tone::Bad);
    let lead = if dot { 18. * u } else { 8. * u };
    let est = words.chars().count() as f32 * size_px * 0.56 + lead + 8. * u;
    let w = est.min(max);
    if w < 12. {
        return 0.;
    }
    rrect(
        window,
        Bounds::new(point(px(x), px(y)), size(px(w), px(h))),
        6. * u,
        bg,
        gpui::transparent_black(),
        0.,
        false,
    );
    if dot {
        let d = 6. * u;
        window.paint_quad(
            gpui::fill(
                Bounds::new(
                    point(px(x + 7. * u), px(y + (h - d) / 2.)),
                    size(px(d), px(d)),
                ),
                fg,
            )
            .corner_radii(px(d / 2.)),
        );
    }
    text(
        window,
        cx,
        words,
        x + lead,
        y + (h - size_px * 1.25) / 2.,
        size_px,
        fg,
        &font(&p.family, FontWeight::MEDIUM),
        w - lead - 4. * u,
    );
    w
}

/// Where a block is drawn: a map point `m` of it lands at
/// `anchor + off + (m - anchor) × k`.
#[derive(Clone, Copy)]
struct Pose {
    anchor: (f32, f32),
    off: (f32, f32),
    k: f32,
}

impl Pose {
    const STILL: Pose = Pose {
        anchor: (0., 0.),
        off: (0., 0.),
        k: 1.,
    };
    fn at(&self, mx: f32, my: f32) -> (f32, f32) {
        (
            self.anchor.0 + self.off.0 + (mx - self.anchor.0) * self.k,
            self.anchor.1 + self.off.1 + (my - self.anchor.1) * self.k,
        )
    }
}

/// A line's width in pixels: at Out by the links it stands for, else
/// thicker for the focus's.
fn line_width(level: Level, links: usize, lit: bool, z: f32, u: f32) -> f32 {
    if level == Level::Out {
        (1.25 + 0.55 * (links.max(1) as f32).log2()).min(3.5) * u
    } else if lit {
        2. * u * z.clamp(0.7, 1.4)
    } else {
        1.4 * u * z.clamp(0.6, 1.4)
    }
}

/// `A`, `A and B`, `A, B and C`, or `4 others`.
fn names(v: Vec<String>) -> String {
    match v.len() {
        0 => String::new(),
        1 => v[0].clone(),
        2 | 3 => format!("{} and {}", v[..v.len() - 1].join(", "), v[v.len() - 1]),
        n => hippo_view::night::count(n, "other", "others"),
    }
}

/// A block's name: a folder's, or a note's title.
fn block_name(m: &NotesMap, item: Item) -> String {
    match m.block_of(item) {
        Block::Folder(f) if m.folders[f].name.is_empty() => "Notes".into(),
        Block::Folder(f) => m.folders[f].name.clone(),
        Block::Note(i) => m.notes[i].title.clone(),
    }
}

fn paint_map(p: &Paint, b: Bounds<Pixels>, window: &mut Window, cx: &mut gpui::App) {
    let started = Instant::now();
    let (ox, oy) = (f32::from(b.origin.x), f32::from(b.origin.y));
    let (vw, vh) = (f32::from(b.size.width), f32::from(b.size.height));
    p.view.set((ox, oy, vw, vh));
    let z = p.zoom;
    let u = p.unit;
    let s = |mx: f32, my: f32| (ox + (mx - p.pan.0) * z * u, oy + (my - p.pan.1) * z * u);
    let level = Level::of(z);
    let view = map::Rect::new(p.pan.0, p.pan.1, vw / (z * u), vh / (z * u));
    let t = &p.t;
    // What is drawn: while a block is lifted, or blocks still slide, the
    // layout they slide to; else the map's.
    let lift = p.lift.as_ref().filter(|l| l.level == level);
    let slide = p.slide.as_ref().filter(|sl| sl.level == level);
    let moving = slide.is_some_and(|sl| !sl.done());
    let lay: &Layout = match slide {
        Some(sl) if lift.is_some() || moving => &sl.lay,
        _ => p.map.layout(level),
    };
    if moving {
        window.request_animation_frame();
    }
    // Each block's pose: sliding blocks off their place, the lifted one
    // under the pointer, raised.
    let mut poses: std::collections::HashMap<Block, Pose> = std::collections::HashMap::new();
    if let Some(sl) = slide.filter(|_| moving || lift.is_some()) {
        let e = sl.eased();
        for (bk, d) in &sl.from {
            let mut pose = Pose {
                off: (d.0 * (1. - e), d.1 * (1. - e)),
                ..Pose::STILL
            };
            if let Some((sb, k0)) = sl.settle
                && sb == *bk
                && let Some(r) = p.map.block_rect_in(lay, sb)
            {
                pose.anchor = (r.x, r.y);
                pose.k = 1. + (k0 - 1.) * (1. - e);
            }
            poses.insert(*bk, pose);
        }
    }
    let lifted = lift.map(|l| l.block);
    let mut slot: Option<map::Rect> = None;
    if let Some(l) = lift
        && let Some(r) = p.map.block_rect_in(lay, l.block)
    {
        let placed = poses.get(&l.block).copied().unwrap_or(Pose::STILL);
        let (sx, sy) = placed.at(r.x, r.y);
        slot = Some(map::Rect::new(sx, sy, r.w, r.h));
        let g = (r.x + l.grab.0, r.y + l.grab.1);
        poses.insert(
            l.block,
            Pose {
                anchor: g,
                off: (l.at.0 - g.0, l.at.1 - g.1),
                k: LIFT_SCALE,
            },
        );
    }
    let pose_of = |item: Item| -> Pose {
        if poses.is_empty() {
            return Pose::STILL;
        }
        poses
            .get(&p.map.block_of(item))
            .copied()
            .unwrap_or(Pose::STILL)
    };
    let is_lifted = |item: Item| lifted.is_some_and(|bk| p.map.block_of(item) == bk);

    // The ground: one dot every 24 map points, thinned when they crowd.
    let mut step = 24. * z * u;
    while step < 20. * u {
        step *= 2.;
    }
    let d = (2. * u).max(1.);
    let gx = (-p.pan.0 * z * u).rem_euclid(step);
    let gy = (-p.pan.1 * z * u).rem_euclid(step);
    let mut y = gy;
    while y < vh {
        let mut x = gx;
        while x < vw {
            window.paint_quad(gpui::fill(
                Bounds::new(point(px(ox + x), px(oy + y)), size(px(d), px(d))),
                t.dot,
            ));
            x += step;
        }
        y += step;
    }

    // Where the lifted block lands: its shape, sunk into the ground.
    if let Some(r) = slot {
        let (x, y) = s(r.x, r.y);
        rrect(
            window,
            Bounds::new(point(px(x), px(y)), size(px(r.w * z * u), px(r.h * z * u))),
            (if level == Level::Out { 18. } else { 16. }) * z * u,
            t.you_bg,
            t.you_line,
            (1.5 * u).max(1.),
            true,
        );
    }

    let focus = p.focus;
    // The lines, behind the cards; not while blocks move.
    let mut drawn_lines = 0;
    let mut hovered: Vec<&map::Line> = Vec::new();
    if lift.is_none() && !moving {
        let mut shown = lay.shown(focus);
        shown.retain(|(l, _)| l.bounds.meets(&view));
        shown.sort_by_key(|(_, lit)| *lit);
        let dim = shown.iter().any(|(_, lit)| *lit);
        for (l, lit) in &shown {
            let hot = p
                .hover_line
                .as_ref()
                .is_some_and(|(ends, _)| ends.contains(&(l.from, l.to)));
            if hot {
                hovered.push(l);
            }
            let color = if hot {
                t.you_mark()
            } else if *lit {
                // Near-black lines read heavy on a light ground.
                if t.dark {
                    t.night
                } else {
                    t.night.opacity(0.7)
                }
            } else if dim {
                t.line2.opacity(0.4)
            } else {
                t.line2
            };
            let width = line_width(level, l.links, *lit || hot, z, u);
            // Every segment is level or plumb: a thin quad, no tessellation.
            for w in l.points.windows(2) {
                let (a, b2) = (s(w[0].0, w[0].1), s(w[1].0, w[1].1));
                let (x0, x1) = (a.0.min(b2.0), a.0.max(b2.0));
                let (y0, y1) = (a.1.min(b2.1), a.1.max(b2.1));
                let half = width / 2.;
                window.paint_quad(gpui::fill(
                    Bounds::new(
                        point(px(x0 - half), px(y0 - half)),
                        size(px(x1 - x0 + width), px(y1 - y0 + width)),
                    ),
                    color,
                ));
            }
            let r = (3. * u * z.clamp(0.6, 1.4)).max(width * 0.9);
            for end in [l.points[0], *l.points.last().expect("a line has points")] {
                let (x, y) = s(end.0, end.1);
                window.paint_quad(gpui::quad(
                    Bounds::new(point(px(x - r), px(y - r)), size(px(2. * r), px(2. * r))),
                    px(r),
                    t.canvas,
                    px((1.5 * u).max(width * 0.6)),
                    color,
                    gpui::BorderStyle::Solid,
                ));
            }
            drawn_lines += 1;
        }
    }

    // Every block but the lifted one, then the lifted one on top.
    let mut drawn = 0;
    for pass in [false, true] {
        if pass && lifted.is_none() {
            break;
        }
        if pass
            && let Some(l) = lift
            && let Some(r) = p.map.block_rect_in(lay, l.block)
        {
            // The lifted block's ground: a shadow under it, and a solid
            // ground under a folder's frame, so it reads as picked up.
            let pose = poses[&l.block];
            let (x0, y0) = pose.at(r.x, r.y);
            let (x, y) = s(x0, y0);
            let bounds = Bounds::new(
                point(px(x), px(y)),
                size(px(r.w * z * u * pose.k), px(r.h * z * u * pose.k)),
            );
            let radius = (if level == Level::Out || matches!(l.block, Block::Folder(_)) {
                18.
            } else {
                14.
            }) * z
                * u
                * pose.k;
            window.paint_drop_shadows(
                bounds,
                gpui::Corners::all(px(radius)),
                &[
                    gpui::BoxShadow {
                        color: gpui::hsla(0., 0., 0., if t.dark { 0.55 } else { 0.18 }),
                        offset: point(px(0.), px(14. * u)),
                        blur_radius: px(32. * u),
                        spread_radius: px(0.),
                        inset: false,
                    },
                    gpui::BoxShadow {
                        color: gpui::hsla(0., 0., 0., if t.dark { 0.4 } else { 0.1 }),
                        offset: point(px(0.), px(2. * u)),
                        blur_radius: px(6. * u),
                        spread_radius: px(0.),
                        inset: false,
                    },
                ],
            );
            if matches!(l.block, Block::Folder(_)) && level != Level::Out {
                rrect(
                    window,
                    bounds,
                    radius,
                    t.canvas,
                    t.line2,
                    (1.2 * u).max(1.),
                    false,
                );
            }
        }
        let mine = |item: Item| is_lifted(item) == pass;
        // Folder frames: their outline; their words are labels.
        for (f, r) in &lay.frames {
            let item = Item::Folder(*f);
            if !mine(item) || (pass && level != Level::Out) {
                continue;
            }
            let pose = pose_of(item);
            let (mx, my) = pose.at(r.x, r.y);
            let (w, h) = (r.w * z * u * pose.k, r.h * z * u * pose.k);
            if !pass && !map::Rect::new(mx, my, r.w * pose.k, r.h * pose.k).meets(&view) {
                continue;
            }
            let (x, y) = s(mx, my);
            rrect(
                window,
                Bounds::new(point(px(x), px(y)), size(px(w), px(h))),
                16. * z * u * pose.k,
                gpui::transparent_black(),
                t.line2,
                (1.2 * u).max(1.),
                true,
            );
        }
        for c in &lay.cards {
            if !mine(c.item) {
                continue;
            }
            let pose = pose_of(c.item);
            let (mx, my) = pose.at(c.rect.x, c.rect.y);
            if !pass && !map::Rect::new(mx, my, c.rect.w * pose.k, c.rect.h * pose.k).meets(&view) {
                continue;
            }
            let (x, y) = s(mx, my);
            let zk = z * pose.k;
            let (w, h) = (c.rect.w * zk * u, c.rect.h * zk * u);
            let bounds = Bounds::new(point(px(x), px(y)), size(px(w), px(h)));
            drawn += 1;
            let (sel, near) = match c.item {
                Item::Note(i) => (p.selected == Some(i), p.neighbours.contains(&i)),
                Item::Folder(_) => (p.focus == Some(c.item) && p.hover.is_none(), false),
            };
            let hover = p.hover == Some(c.item) && lift.is_none();
            match c.item {
                Item::Note(i) if level != Level::Out => {
                    let n = &p.map.notes[i];
                    paint_note(
                        p,
                        zk,
                        n,
                        sel,
                        near,
                        hover,
                        level == Level::In,
                        bounds,
                        window,
                        cx,
                    );
                }
                // At Out a card is its ground; its words and marks follow.
                _ => paint_out_card(p, zk, sel, near, hover, bounds, window),
            }
        }
        // Every word on the map, then the marks.
        for l in &lay.labels {
            if !mine(l.item) {
                continue;
            }
            let pose = pose_of(l.item);
            let (mx, my) = pose.at(l.rect.x, l.rect.y);
            if !pass && !map::Rect::new(mx, my, l.rect.w * pose.k, l.rect.h * pose.k).meets(&view) {
                continue;
            }
            let (x, y) = s(mx, my);
            let zk = z * pose.k;
            paint_label(p, zk, l, x, y, l.rect.w * zk * u, window, cx);
        }
        for m in &lay.marks {
            if !mine(m.item) {
                continue;
            }
            let pose = pose_of(m.item);
            let (mx, my) = pose.at(m.x, m.y);
            if !pass && !view.contains(mx, my) {
                continue;
            }
            let (x, y) = s(mx, my);
            let d = (m.d * z * pose.k * u).max(3.);
            let b = Bounds::new(point(px(x - d / 2.), px(y - d / 2.)), size(px(d), px(d)));
            let fill = match m.tone {
                "ok" => Some(t.ok),
                "bad" => Some(t.bad),
                "you" => Some(t.you_mark()),
                "faint" => Some(t.faint),
                "night" => Some(t.night),
                _ => None,
            };
            match fill {
                Some(c) => window.paint_quad(gpui::fill(b, c).corner_radii(px(d / 2.))),
                // No flag: a ring, so the row still reads as a note.
                None => window.paint_quad(gpui::quad(
                    b,
                    px(d / 2.),
                    gpui::transparent_black(),
                    px((1.5 * u).max(1.)),
                    t.line2,
                    gpui::BorderStyle::Solid,
                )),
            }
        }
    }

    // A line under the pointer at Out: what it stands for.
    if let (Some((_, at)), Some(first)) = (p.hover_line.as_ref(), hovered.first()) {
        let links: usize = hovered.iter().map(|l| l.links).sum();
        let from = block_name(&p.map, first.from);
        let words = format!(
            "{from} and {} · {}",
            names(hovered.iter().map(|l| block_name(&p.map, l.to)).collect()),
            hippo_view::night::count(links, "link", "links")
        );
        let f = font(&p.family, FontWeight::MEDIUM);
        let size_px = 12.5 * u;
        let run = TextRun {
            len: words.len(),
            font: f.clone(),
            color: t.text,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let shaped =
            window
                .text_system()
                .shape_line(words.clone().into(), px(size_px), &[run], None);
        let (tw, th) = (f32::from(shaped.width) + 20. * u, 28. * u);
        let mut x = ox + at.0 * u + 14. * u;
        let mut y = oy + at.1 * u + 16. * u;
        if x + tw > ox + vw - 8. * u {
            x = ox + at.0 * u - 14. * u - tw;
        }
        if y + th > oy + vh - 8. * u {
            y = oy + at.1 * u - 16. * u - th;
        }
        let bounds = Bounds::new(point(px(x), px(y)), size(px(tw), px(th)));
        window.paint_drop_shadows(
            bounds,
            gpui::Corners::all(px(8. * u)),
            &[gpui::BoxShadow {
                color: gpui::hsla(0., 0., 0., if t.dark { 0.45 } else { 0.12 }),
                offset: point(px(0.), px(4. * u)),
                blur_radius: px(12. * u),
                spread_radius: px(0.),
                inset: false,
            }],
        );
        rrect(
            window,
            bounds,
            8. * u,
            t.surface,
            t.line2,
            (1. * u).max(1.),
            false,
        );
        let _ = shaped.paint(
            point(px(x + 10. * u), px(y + (th - size_px * 1.25) / 2.)),
            px(size_px * 1.25),
            gpui::TextAlign::Left,
            None,
            window,
            cx,
        );
    }

    let mut f = p.frames.borrow_mut();
    let now = Instant::now();
    if let Some(last) = f.last {
        f.interval_ms.push((now - last).as_secs_f32() * 1000.);
    }
    f.last = Some(now);
    let ms = started.elapsed().as_secs_f32() * 1000.;
    f.paint_ms.push(ms);
    f.drawn = (drawn, drawn_lines);
    // Dev-only: one line per paint on stderr, for the frame-time check.
    if std::env::var_os("HIPPO_APP_DEV_FRAMES").is_some() {
        eprintln!(
            "paint zoom {:.2} level {:?} notes {} links {} cards {drawn} lines {drawn_lines} ms {ms:.2}",
            z,
            level,
            p.map.notes.len(),
            p.map.edges.len()
        );
    }
}

/// An Out card's ground: a pinned note's or a folder's.
fn paint_out_card(
    p: &Paint,
    z: f32,
    sel: bool,
    near: bool,
    hover: bool,
    b: Bounds<Pixels>,
    window: &mut Window,
) {
    let t = &p.t;
    let u = p.unit * z;
    let border = if sel {
        t.night
    } else if near || hover {
        t.night_line
    } else {
        t.line
    };
    let bw = if sel {
        (2. * u).max(1.5)
    } else {
        (1. * p.unit).max(1.)
    };
    rrect(window, b, 18. * u, t.surface, border, bw, false);
}

/// One label: each of its lines at its size, from `(x, y)` (pixels), in
/// its kind's weight and colour. Should the font run wider than the
/// layout's estimate, the line is drawn smaller to fit, never cut.
#[allow(clippy::too_many_arguments)]
fn paint_label(
    p: &Paint,
    z: f32,
    l: &Label,
    x: f32,
    y: f32,
    max: f32,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    let t = &p.t;
    let size_px = l.size * z * p.unit;
    if size_px < 4. {
        return;
    }
    let (color, weight) = match l.kind {
        LabelKind::Title => (t.text, FontWeight::SEMIBOLD),
        LabelKind::Row => (t.text, FontWeight::NORMAL),
        LabelKind::Meta => (t.muted, FontWeight::NORMAL),
        LabelKind::Frame => (t.muted, FontWeight::SEMIBOLD),
        LabelKind::Count | LabelKind::More => (t.faint, FontWeight::NORMAL),
    };
    let f = font(&p.family, weight);
    for (k, line) in l.lines.iter().enumerate() {
        fitted_text(
            window,
            cx,
            line,
            x,
            y + k as f32 * size_px * LEADING,
            size_px,
            color,
            &f,
            max,
        );
    }
}

/// One line of text at `(x, y)`, shrunk to `max` wide if it runs over.
#[allow(clippy::too_many_arguments)]
fn fitted_text(
    window: &mut Window,
    cx: &mut gpui::App,
    s: &str,
    x: f32,
    y: f32,
    size_px: f32,
    color: Hsla,
    f: &Font,
    max: f32,
) {
    let size_px = (size_px * 2.).round() / 2.;
    let run = TextRun {
        len: s.len(),
        font: f.clone(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let shaped = |sz: f32, window: &mut Window| {
        window.text_system().shape_line(
            s.to_owned().into(),
            px(sz),
            std::slice::from_ref(&run),
            None,
        )
    };
    let mut line = shaped(size_px, window);
    let mut sz = size_px;
    let w = f32::from(line.width);
    if w > max + 0.5 && w > 0. {
        sz = ((size_px * max / w) * 2.).floor() / 2.;
        line = shaped(sz, window);
    }
    // The line box keeps the label's size, so lines below do not move.
    let _ = line.paint(
        point(px(x), px(y + (size_px - sz) * 0.6)),
        px(size_px * LEADING),
        gpui::TextAlign::Left,
        None,
        window,
        cx,
    );
}

#[allow(clippy::too_many_arguments)]
fn paint_note(
    p: &Paint,
    z: f32,
    n: &MapNote,
    sel: bool,
    near: bool,
    hover: bool,
    inner: bool,
    b: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    let t = &p.t;
    let u = p.unit * z;
    let border = if sel {
        t.night
    } else if near || hover {
        t.night_line
    } else {
        t.line
    };
    let bw = if sel { 2. * u } else { (1. * p.unit).max(1.) };
    rrect(window, b, 14. * u, t.surface, border, bw.max(1.), false);
    let (x, y, w) = (
        f32::from(b.origin.x),
        f32::from(b.origin.y),
        f32::from(b.size.width),
    );
    let pad = 16. * u;
    let icon = 18. * u;
    let _ = window.paint_svg(
        Bounds::new(
            point(px(x + pad), px(y + pad + 1. * u)),
            size(px(icon), px(icon)),
        ),
        if n.flags.entry {
            "icons/n-start.svg"
        } else {
            "icons/n-note.svg"
        }
        .into(),
        None,
        gpui::TransformationMatrix::unit(),
        if n.flags.entry || n.flags.required {
            t.night
        } else {
            t.muted
        },
        cx,
    );
    // The title is a label, drawn after the cards.
    text(
        window,
        cx,
        &n.summary,
        x + pad,
        y + pad + 34. * u,
        13.5 * u,
        t.muted,
        &font(&p.family, FontWeight::NORMAL),
        w - 2. * pad,
    );
    let cy = y + (CARD_H - 16. - 22.) * u;
    let mut cx0 = x + pad;
    if let Some((words, tone)) = n.flags.chip() {
        cx0 += chip(
            window,
            cx,
            p,
            &words,
            Tone::of(tone),
            cx0,
            cy,
            z,
            w - 2. * pad,
        ) + 8. * u;
    }
    chip(
        window,
        cx,
        p,
        &n.reads_label(),
        Tone::Plain,
        cx0,
        cy,
        z,
        x + w - pad - cx0,
    );
    if inner {
        let mut ly = y + (CARD_H + 6.) * u;
        for (line, kind) in n.in_lines() {
            let mark = 8. * u;
            let my = ly + 6. * u;
            match kind {
                "rule" => window.paint_quad(
                    gpui::fill(
                        Bounds::new(point(px(x + pad), px(my)), size(px(mark), px(mark))),
                        t.you,
                    )
                    .corner_radii(px(2. * u)),
                ),
                "waiting" => rrect(
                    window,
                    Bounds::new(point(px(x + pad), px(my)), size(px(mark), px(mark))),
                    2. * u,
                    gpui::transparent_black(),
                    t.you_mark(),
                    1.5 * u,
                    false,
                ),
                _ => {
                    window.paint_quad(gpui::fill(
                        Bounds::new(
                            point(px(x + pad + mark + 6. * u), px(ly)),
                            size(
                                px((w - 2. * pad - mark - 6. * u).max(0.)),
                                px(IN_LINE * u - 2. * u),
                            ),
                        ),
                        t.you_bg,
                    ));
                }
            }
            text(
                window,
                cx,
                &line,
                x + pad + mark + 10. * u,
                ly + 1. * u,
                13. * u,
                if kind == "said" { t.you_text } else { t.text },
                &font(&p.family, FontWeight::NORMAL),
                w - 2. * pad - mark - 12. * u,
            );
            ly += IN_LINE * u;
        }
    }
}

impl Shell {
    /// The map, cloned once per reload.
    fn brain_map(&mut self) -> Option<Rc<NotesMap>> {
        if let Some(n) = std::env::var("HIPPO_APP_DEV_SYNTHETIC_MAP")
            .ok()
            .and_then(|v| {
                let (a, b) = v.split_once('x')?;
                Some((a.parse::<usize>().ok()?, b.parse::<usize>().ok()?))
            })
        {
            if self.night.map.map.is_none() {
                self.night.map.map = Some(Rc::new(map::synthetic(n.0, 12, n.1)));
            }
            return self.night.map.map.clone();
        }
        let loaded = self.loaded.as_ref()?.night.as_ref()?;
        let same = self.night.map.map.as_ref().is_some_and(|m| {
            m.notes.len() == loaded.map.notes.len()
                && m.edges == loaded.map.edges
                && m.notes == loaded.map.notes
        });
        if !same {
            let saved = self
                .brain_layout_key()
                .and_then(|k| self.night.map.layouts.get(&k).cloned());
            let m = match (saved, self.night.map.view_size) {
                (Some(a), _) => loaded.map.arranged(&a),
                // Laid out for the view it was last fitted to.
                (None, Some((w, h))) => loaded.map.fitted(w, h),
                (None, None) => loaded.map.clone(),
            };
            self.night.map.map = Some(Rc::new(m));
            self.night.map.panel = None;
            self.night.map.slide = None;
        }
        self.night.map.map.clone()
    }

    /// The selected note: the chosen one, else the first with a rule
    /// waiting, else the entry note.
    pub(crate) fn brain_selected(&self, m: &NotesMap) -> Option<usize> {
        if let Some(p) = &self.night.brain {
            return m.find(p);
        }
        m.notes
            .iter()
            .position(|n| n.flags.waiting > 0)
            .or_else(|| m.notes.iter().position(|n| n.flags.entry))
    }

    pub(crate) fn render_brain(
        &mut self,
        look: &Look,
        t: &Nt,
        left_w: f32,
        view_h: f32,
        narrow: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(m) = self.brain_map() else {
            return self.render_night_empty(look, t, "Reading hippo's files…");
        };
        if narrow {
            return self.render_brain_board(look, t, &m, cx);
        }
        let unit = f32::from(look.px(1.));
        let view_w = left_w;
        let map_h = (view_h - HEAD).max(100.);
        if !self.night.map.fitted {
            // Hippo's layout made for this view, then the whole map shown.
            let fit_h = (map_h - FOOT).max(100.);
            self.night.map.view_size = Some((view_w, fit_h));
            if !m.is_arranged() {
                self.night.map.map = Some(Rc::new(m.fitted(view_w, fit_h)));
            }
            let m = self.night.map.map.clone().unwrap_or(m.clone());
            self.night.map.fit(&m, view_w, map_h);
            if let Some(z) = self.night.dev_zoom.take() {
                let at = (view_w / 2., map_h / 2.);
                self.night.map.zoom_at(&m, z.max(map::MIN_ZOOM), at);
                // Dev-only: below the fit, the whole map centred; above
                // it, the selected card near the top left.
                let zz = self.night.map.zoom;
                let (fz, _) = m.fit(view_w, fit_h);
                if zz > fz + 0.01 {
                    let sel = Item::Note(self.brain_selected(&m).unwrap_or(0));
                    let item = m.same_item(sel, Level::of(zz));
                    if let Some(c) = m.layout(Level::of(zz)).card(item) {
                        let r = c.rect;
                        self.night.map.pan = (r.x - 40. / zz, r.y - 30. / zz);
                    }
                }
            }
        }
        // Dev-only: Reset layout pressed before the first frame.
        if std::mem::take(&mut self.night.dev_reset) {
            self.brain_reset_layout(cx);
        }
        let Some(m) = self.night.map.map.clone() else {
            return self.render_night_empty(look, t, "Reading hippo's files…");
        };
        // Dev-only: a block lifted as a drag lifts it, for a screenshot.
        let level = self.night.map.level();
        if let Some((key, at)) = self.night.dev_lift.take()
            && let Some(b) = m.block(&key)
            && let Some(r) = m.block_rect(level, b)
        {
            self.brain_lift_map(b, (r.w * 0.3, 24.), at);
            // Dev-only: dropped before the first frame.
            if std::mem::take(&mut self.night.dev_drop) {
                self.brain_drop(cx);
            }
        }
        let m = self.night.map.map.clone().unwrap_or(m);
        // Dev-only: the pointer resting on the map, for a screenshot.
        if let Some(at) = self.night.dev_hover.take() {
            self.brain_hover(at);
        }
        let selected = self.brain_selected(&m);
        let neighbours: Vec<usize> = selected
            .map(|i| m.neighbours(i).into_iter().collect())
            .unwrap_or_default();
        let focus = self
            .night
            .map
            .hover
            .or_else(|| selected.map(|i| m.same_item(Item::Note(i), level)));
        let paint = Paint {
            map: m.clone(),
            zoom: self.night.map.zoom,
            pan: self.night.map.pan,
            selected,
            neighbours,
            t: *t,
            unit,
            family: look
                .font_family
                .clone()
                .unwrap_or_else(|| ".SystemUIFont".to_owned())
                .into(),
            view: self.night.map.view.clone(),
            frames: self.night.map.frames.clone(),
            lift: self.night.map.lift.clone(),
            focus,
            hover: self.night.map.hover,
            hover_line: self.night.map.hover_line.clone(),
            slide: self.night.map.slide.clone(),
        };
        let canvas_el = canvas(
            |_, _, _| {},
            move |b, _, window, cx| paint_map(&paint, b, window, cx),
        )
        .size_full();
        let head = div()
            .flex()
            .flex_col()
            .gap(look.px(6.))
            .px(look.px(super::morning::PAD))
            .pt(look.px(22.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(look.text(13.5))
                            .text_color(t.muted)
                            .child(m.kicker.clone()),
                    )
                    .child(
                        kit::button(look, t, "brain-treemap", None, "Treemap", Btn::Quiet, true)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.select_section(crate::state::Section::Memory, window, cx)
                            })),
                    ),
            )
            .child(
                div()
                    .text_size(look.text(26.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(t.text)
                    .child(m.headline.clone()),
            )
            .child(
                div()
                    .text_size(look.text(15.))
                    .text_color(t.muted)
                    .truncate()
                    .child(m.lead.clone()),
            );
        let pct = format!("{}%", (self.night.map.zoom * 100.).round());
        let controls = div()
            .absolute()
            .right(look.px(20.))
            .bottom(look.px(96.))
            .flex()
            .items_center()
            .gap(look.px(2.))
            .p(look.px(4.))
            .rounded(look.px(12.))
            .bg(t.surface)
            .border(look.fixed(1.))
            .border_color(t.line)
            .shadow(kit::float_shadow(look, t))
            // Shown only while the layout is a person's arrangement.
            .when(m.is_arranged(), |d| {
                d.child(
                    kit::button(
                        look,
                        t,
                        "brain-reset",
                        Some("undo-2"),
                        "Reset layout",
                        Btn::Quiet,
                        true,
                    )
                    .tooltip(crate::ui::tooltip::Tooltip::text(
                        "Back to the layout hippo chooses",
                    ))
                    .on_click(cx.listener(|this, _, _, cx| this.brain_reset_layout(cx))),
                )
            })
            .child(
                kit::icon_button(
                    look,
                    t,
                    "zoom-out",
                    "minus",
                    "Zoom out",
                    Some("⌘−"),
                    kit::Size::Sm,
                )
                .on_click(cx.listener(|this, _, _, cx| this.brain_zoom_step(1. / STEP, cx))),
            )
            .child(
                kit::button(look, t, "zoom-fit", None, pct, Btn::Quiet, true)
                    .tooltip(crate::ui::tooltip::Tooltip::with_keys(
                        "Fit the map",
                        Some("⌘0"),
                    ))
                    .on_click(cx.listener(|this, _, _, cx| this.brain_fit(cx))),
            )
            .child(
                kit::icon_button(
                    look,
                    t,
                    "zoom-in",
                    "plus",
                    "Zoom in",
                    Some("⌘="),
                    kit::Size::Sm,
                )
                .on_click(cx.listener(|this, _, _, cx| this.brain_zoom_step(STEP, cx))),
            );
        let mapdiv = div()
            .id("brain-map")
            .debug_selector(|| "brain-map".to_owned())
            .absolute()
            .top(look.px(HEAD))
            .left_0()
            .w(look.px(view_w))
            .h(look.px(map_h))
            .overflow_hidden()
            .cursor_grab()
            .child(canvas_el)
            .on_scroll(cx.listener(move |this, e: &gpui::ScrollEvent, _, cx| {
                let Some(m) = this.night.map.map.clone() else {
                    return;
                };
                let d = e.delta.pixel_delta(px(16.));
                let (dx, dy) = (f32::from(d.x) / unit, f32::from(d.y) / unit);
                if e.modifiers.platform {
                    let at = this.brain_view_point(e.position, unit);
                    let z = this.night.map.zoom * (dy * 0.01).exp();
                    this.night.map.zoom_at(&m, z, at);
                } else {
                    this.night.map.pan_by(dx, dy);
                }
                cx.notify();
            }))
            .on_pinch(cx.listener(move |this, e: &gpui::PinchEvent, _, cx| {
                let Some(m) = this.night.map.map.clone() else {
                    return;
                };
                let at = this.brain_view_point(e.position, unit);
                let z = this.night.map.zoom * (1. + e.delta);
                this.night.map.zoom_at(&m, z, at);
                cx.notify();
            }))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, e: &gpui::MouseDownEvent, _, cx| {
                    let at = this.brain_view_point(e.position, unit);
                    if e.click_count >= 2 {
                        this.brain_double_click(at, cx);
                        this.night.map.press = None;
                        this.night.map.press_block = None;
                        return;
                    }
                    this.night.map.press = Some((at, this.night.map.pan, false));
                    // A block's handle lifts the block; anything else pans.
                    let (mx, my) = this.night.map.to_map(at);
                    let level = this.night.map.level();
                    this.night.map.press_block = this
                        .night
                        .map
                        .map
                        .as_ref()
                        .and_then(|m| m.handle_at(level, mx, my));
                }),
            )
            .on_mouse_move(cx.listener(move |this, e: &gpui::MouseMoveEvent, _, cx| {
                let Some((start, pan, moved)) = this.night.map.press else {
                    let at = this.brain_view_point(e.position, unit);
                    if this.brain_hover(at) {
                        cx.notify();
                    }
                    return;
                };
                if e.pressed_button != Some(gpui::MouseButton::Left) {
                    this.night.map.press = None;
                    this.night.map.lift = None;
                    return;
                }
                let at = this.brain_view_point(e.position, unit);
                let (dx, dy) = (at.0 - start.0, at.1 - start.1);
                if !moved && dx.abs() + dy.abs() <= 4. {
                    return;
                }
                this.night.map.press = Some((start, pan, true));
                if let Some(b) = this.night.map.press_block {
                    this.brain_lift_to(b, start, at, unit);
                } else {
                    let z = this.night.map.zoom;
                    this.night.map.pan = (pan.0 - dx / z, pan.1 - dy / z);
                }
                cx.notify();
            }))
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(move |this, e: &gpui::MouseUpEvent, _, cx| {
                    this.night.map.press_block = None;
                    let Some((_, _, moved)) = this.night.map.press.take() else {
                        return;
                    };
                    if this.night.map.lift.is_some() {
                        this.brain_drop(cx);
                    } else if !moved {
                        let at = this.brain_view_point(e.position, unit);
                        this.brain_click(at, cx);
                    }
                }),
            )
            // Released outside the map: a lifted block lands where shown.
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !*hovered
                    && (this.night.map.hover.is_some() || this.night.map.hover_line.is_some())
                {
                    this.night.map.hover = None;
                    this.night.map.hover_line = None;
                    cx.notify();
                }
            }))
            .on_mouse_up_out(
                gpui::MouseButton::Left,
                cx.listener(|this, _: &gpui::MouseUpEvent, _, cx| {
                    this.night.map.press = None;
                    this.night.map.press_block = None;
                    if this.night.map.lift.is_some() {
                        this.brain_drop(cx);
                    }
                }),
            );
        let toast = self.night.map.toast.clone().map(|(_, words, undo)| {
            div()
                .absolute()
                .left(look.px(20.))
                .bottom(look.px(96.))
                .child(
                    div()
                        .id("brain-toast")
                        .flex()
                        .items_center()
                        .gap(look.px(10.))
                        .pl(look.px(14.))
                        .pr(look.px(6.))
                        .py(look.px(6.))
                        .rounded(look.px(12.))
                        .bg(t.surface)
                        .border(look.fixed(1.))
                        .border_color(t.line2)
                        .shadow(kit::float_shadow(look, t))
                        .text_size(look.text(13.5))
                        .text_color(t.text)
                        .child(words)
                        .children(undo.map(|_| {
                            kit::button(
                                look,
                                t,
                                "brain-undo",
                                Some("undo-2"),
                                "Undo",
                                Btn::Second,
                                true,
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.brain_undo_reset(cx)))
                        })),
                )
        });
        div()
            .relative()
            .w(look.px(view_w))
            .h(look.px(view_h))
            .child(head)
            .child(mapdiv)
            .child(controls)
            .children(toast)
            .into_any_element()
    }

    /// The preferences' key for this workspace's arrangement: its root.
    fn brain_layout_key(&self) -> Option<String> {
        self.workspace_root().map(|r| r.display().to_string())
    }

    /// Lays Brain out from `arr` (`None`: the default), and saves it as
    /// this person's for the workspace, or forgets it when it is the
    /// default.
    pub(crate) fn brain_set_arrangement(
        &mut self,
        arr: Option<Arrangement>,
        cx: &mut Context<Self>,
    ) {
        let Some(m) = self.night.map.map.clone() else {
            return;
        };
        let arr = arr.unwrap_or_else(|| m.default_arrangement.clone());
        let new = m.arranged(&arr);
        // Blocks slide from where they are drawn to their new places.
        let level = self.night.map.level();
        let lay = new.layout(level).clone();
        let from = self.brain_slide_from(&m, level, &lay, None);
        self.night.map.slide = Some(Slide {
            level,
            lay: Rc::new(lay),
            from,
            settle: None,
            target: None,
            start: Instant::now(),
            ms: motion_ms(SETTLE_MS),
        });
        if let Some(k) = self.brain_layout_key() {
            if new.is_arranged() {
                self.night.map.layouts.insert(k, new.arrangement.clone());
            } else {
                self.night.map.layouts.remove(&k);
            }
            let layouts = self.night.map.layouts.clone();
            let _ = crate::prefs::update(self.themes.prefs_path(), |p| p.brain_layouts = layouts);
        }
        self.night.map.map = Some(Rc::new(new));
        cx.notify();
    }

    /// A drag of block `b` from view point `start` to `at`: lifts it and
    /// finds where it would land; near the map's edge the map scrolls.
    pub(crate) fn brain_lift_to(&mut self, b: Block, start: (f32, f32), at: (f32, f32), unit: f32) {
        let Some(m) = self.night.map.map.clone() else {
            return;
        };
        let level = self.night.map.level();
        let Some(r) = m.block_rect(level, b) else {
            return;
        };
        let (w, h) = self.brain_view_size(unit);
        let edge = 40.;
        let sx = if at.0 < edge {
            -8.
        } else if at.0 > w - edge {
            8.
        } else {
            0.
        };
        let sy = if at.1 < edge {
            -8.
        } else if at.1 > h - edge {
            8.
        } else {
            0.
        };
        if sx != 0. || sy != 0. {
            self.night.map.pan_by(-sx, -sy);
        }
        let grab = match &self.night.map.lift {
            Some(l) if l.block == b => l.grab,
            _ => {
                let p = self.night.map.to_map(start);
                (p.0 - r.x, p.1 - r.y)
            }
        };
        let p = self.night.map.to_map(at);
        self.brain_lift_map(b, grab, p);
    }

    /// Block `b` lifted, held `grab` from its top left, with the pointer at
    /// map point `at`: where it would land, and the others sliding aside to
    /// open that place.
    pub(crate) fn brain_lift_map(&mut self, b: Block, grab: (f32, f32), at: (f32, f32)) {
        let Some(m) = self.night.map.map.clone() else {
            return;
        };
        let level = self.night.map.level();
        let drop = m.drop_at(level, b, at.0, at.1);
        let key = m.key(b);
        self.night.map.hover = None;
        self.night.map.hover_line = None;
        let target = (drop.col, drop.at);
        let same = self.night.map.lift.is_some()
            && self
                .night
                .map
                .slide
                .as_ref()
                .is_some_and(|sl| sl.level == level && sl.target == Some(target));
        if !same {
            let arr = m.arrangement.move_to(&key, drop.col, drop.at);
            let lay = m.preview(&arr, level);
            let from = self.brain_slide_from(&m, level, &lay, None);
            self.night.map.slide = Some(Slide {
                level,
                lay: Rc::new(lay),
                from,
                settle: None,
                target: Some(target),
                start: Instant::now(),
                ms: motion_ms(SLIDE_MS),
            });
        }
        self.night.map.lift = Some(Lift {
            block: b,
            key,
            level,
            grab,
            at,
            drop,
        });
    }

    /// Each block's offset, from where it is drawn now to its place in
    /// `to`; `lifted` (a block and where its top left is drawn) for one
    /// held under the pointer.
    fn brain_slide_from(
        &self,
        m: &NotesMap,
        level: Level,
        to: &Layout,
        lifted: Option<(Block, (f32, f32))>,
    ) -> std::collections::HashMap<Block, (f32, f32)> {
        let now = self.night.map.slide.as_ref().filter(|sl| sl.level == level);
        let shown: &Layout = match now {
            Some(sl) => &sl.lay,
            None => m.layout(level),
        };
        let mut from = std::collections::HashMap::new();
        for key in m.arrangement.keys() {
            let Some(b) = m.block(key) else {
                continue;
            };
            let (Some(a), Some(z)) = (m.block_rect_in(shown, b), m.block_rect_in(to, b)) else {
                continue;
            };
            let off = now.map_or((0., 0.), |sl| sl.offset(b));
            let (x, y) = match lifted {
                Some((lb, at)) if lb == b => at,
                _ => (a.x + off.0, a.y + off.1),
            };
            if (x - z.x).abs() > 0.01 || (y - z.y).abs() > 0.01 {
                from.insert(b, (x - z.x, y - z.y));
            }
        }
        from
    }

    /// Where the lifted block's top left is drawn now.
    fn brain_lifted_at(&self, l: &Lift) -> (f32, f32) {
        (
            l.at.0 - l.grab.0 * LIFT_SCALE,
            l.at.1 - l.grab.1 * LIFT_SCALE,
        )
    }

    /// Drops the lifted block where its line shows.
    pub(crate) fn brain_drop(&mut self, cx: &mut Context<Self>) {
        let Some(l) = self.night.map.lift.clone() else {
            return;
        };
        let Some(m) = self.night.map.map.clone() else {
            return;
        };
        let arr = m.arrangement.move_to(&l.key, l.drop.col, l.drop.at);
        let held = self.brain_lifted_at(&l);
        self.brain_set_arrangement(Some(arr), cx);
        // The dropped block settles into its place from under the pointer.
        if let Some(m2) = self.night.map.map.clone() {
            let lay = m2.layout(l.level).clone();
            let from = self.brain_slide_from(&m2, l.level, &lay, Some((l.block, held)));
            self.night.map.slide = Some(Slide {
                level: l.level,
                lay: Rc::new(lay),
                from,
                settle: Some((l.block, LIFT_SCALE)),
                target: None,
                start: Instant::now(),
                ms: motion_ms(SETTLE_MS),
            });
        }
        self.night.map.lift = None;
        cx.notify();
    }

    /// Escape while a block is lifted puts it back; whether one was.
    pub(crate) fn brain_cancel_lift(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(l) = self.night.map.lift.clone() else {
            return false;
        };
        // Every block flies back to where it was.
        if let Some(m) = self.night.map.map.clone() {
            let lay = m.layout(l.level).clone();
            let held = self.brain_lifted_at(&l);
            let from = self.brain_slide_from(&m, l.level, &lay, Some((l.block, held)));
            self.night.map.slide = Some(Slide {
                level: l.level,
                lay: Rc::new(lay),
                from,
                settle: Some((l.block, LIFT_SCALE)),
                target: None,
                start: Instant::now(),
                ms: motion_ms(BACK_MS),
            });
        }
        self.night.map.lift = None;
        self.night.map.press = None;
        self.night.map.press_block = None;
        cx.notify();
        true
    }

    /// ⌥ and an arrow: the selected note's block a column or a place.
    pub(crate) fn brain_nudge(&mut self, n: Nudge, cx: &mut Context<Self>) {
        let Some(m) = self.night.map.map.clone() else {
            return;
        };
        let Some(i) = self.brain_selected(&m) else {
            return;
        };
        let key = m.key(m.block_of(Item::Note(i)));
        let arr = m.arrangement.nudge(&key, n);
        if arr != m.arrangement {
            self.brain_set_arrangement(Some(arr), cx);
        }
    }

    /// Undo on the reset's line: the arrangement it replaced.
    pub(crate) fn brain_undo_reset(&mut self, cx: &mut Context<Self>) {
        if let Some((_, _, Some(a))) = self.night.map.toast.take() {
            self.brain_set_arrangement(Some(a), cx);
        }
    }

    /// Back to the default layout, with Undo for 6 seconds.
    pub(crate) fn brain_reset_layout(&mut self, cx: &mut Context<Self>) {
        let Some(m) = self.night.map.map.clone() else {
            return;
        };
        if !m.is_arranged() {
            return;
        }
        let before = m.arrangement.clone();
        self.brain_set_arrangement(None, cx);
        self.night.map.toast_seq += 1;
        let seq = self.night.map.toast_seq;
        self.night.map.toast = Some((seq, "Layout reset to the default.".into(), Some(before)));
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(6))
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.night.map.toast.as_ref().is_some_and(|t| t.0 == seq) {
                    this.night.map.toast = None;
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// A window position as a point of the map's view, in points.
    fn brain_view_point(&self, p: gpui::Point<Pixels>, unit: f32) -> (f32, f32) {
        let (ox, oy, _, _) = self.night.map.view.get();
        ((f32::from(p.x) - ox) / unit, (f32::from(p.y) - oy) / unit)
    }

    fn brain_view_size(&self, unit: f32) -> (f32, f32) {
        let (_, _, w, h) = self.night.map.view.get();
        (w / unit, h / unit)
    }

    pub(crate) fn brain_zoom_step(&mut self, factor: f32, cx: &mut Context<Self>) {
        let Some(m) = self.night.map.map.clone() else {
            return;
        };
        let unit = f32::from(Look::from_app(cx).px(1.));
        let (w, h) = self.brain_view_size(unit);
        let z = self.night.map.zoom * factor;
        self.night.map.zoom_at(&m, z, (w / 2., h / 2.));
        cx.notify();
    }

    pub(crate) fn brain_fit(&mut self, cx: &mut Context<Self>) {
        let Some(m) = self.night.map.map.clone() else {
            return;
        };
        let unit = f32::from(Look::from_app(cx).px(1.));
        let (w, h) = self.brain_view_size(unit);
        // Hippo's layout made again for the view as it is now.
        let m = if m.is_arranged() {
            m
        } else {
            let f = Rc::new(m.fitted(w, (h - FOOT).max(100.)));
            self.night.map.map = Some(f.clone());
            self.night.map.slide = None;
            f
        };
        self.night.map.view_size = Some((w, (h - FOOT).max(100.)));
        self.night.map.fit(&m, w, h);
        cx.notify();
    }

    /// The pointer at view point `at` with no button down: the card under
    /// it, and at Out the lines under it. Over a line of the hovered card,
    /// that card stays hovered. Whether anything changed.
    fn brain_hover(&mut self, at: (f32, f32)) -> bool {
        let Some(m) = self.night.map.map.clone() else {
            return false;
        };
        if self.night.map.lift.is_some() {
            return false;
        }
        let level = self.night.map.level();
        let (mx, my) = self.night.map.to_map(at);
        let lay = m.layout(level);
        let card = lay.card_at(mx, my).map(|c| c.item);
        let mut ends: Vec<(Item, Item)> = Vec::new();
        if card.is_none() {
            let focus = self.night.map.hover.or_else(|| {
                self.brain_selected(&m)
                    .map(|i| m.same_item(Item::Note(i), level))
            });
            let tol = 6. / self.night.map.zoom;
            let mut hits: Vec<(f32, Item, Item)> = Vec::new();
            for (l, _) in lay.shown(focus) {
                let d = l
                    .points
                    .windows(2)
                    .map(|w| {
                        let x = mx.clamp(w[0].0.min(w[1].0), w[0].0.max(w[1].0));
                        let y = my.clamp(w[0].1.min(w[1].1), w[0].1.max(w[1].1));
                        ((x - mx).powi(2) + (y - my).powi(2)).sqrt()
                    })
                    .fold(f32::MAX, f32::min);
                if d <= tol {
                    hits.push((d, l.from, l.to));
                }
            }
            let best = hits.iter().map(|h| h.0).fold(f32::MAX, f32::min);
            ends = hits
                .into_iter()
                .filter(|h| h.0 <= best + 0.5)
                .map(|h| (h.1, h.2))
                .collect();
        }
        let hover = card.or(if ends.is_empty() {
            None
        } else {
            self.night.map.hover
        });
        let line = (level == Level::Out && !ends.is_empty()).then_some((ends, at));
        let changed = hover != self.night.map.hover || line != self.night.map.hover_line;
        self.night.map.hover = hover;
        self.night.map.hover_line = line;
        changed
    }

    fn brain_item_at(&self, at: (f32, f32)) -> Option<Item> {
        let m = self.night.map.map.as_ref()?;
        let (mx, my) = self.night.map.to_map(at);
        m.layout(self.night.map.level())
            .card_at(mx, my)
            .map(|c| c.item)
    }

    fn brain_click(&mut self, at: (f32, f32), cx: &mut Context<Self>) {
        match self.brain_item_at(at) {
            Some(Item::Note(i)) => {
                if let Some(m) = &self.night.map.map {
                    self.night.brain = Some(m.notes[i].path.clone());
                }
            }
            Some(item @ Item::Folder(_)) => {
                if let Some(m) = self.night.map.map.clone() {
                    let unit = f32::from(Look::from_app(cx).px(1.));
                    let (w, h) = self.brain_view_size(unit);
                    self.night.map.zoom_into(&m, item, w, h);
                }
            }
            None => {}
        }
        cx.notify();
    }

    fn brain_double_click(&mut self, at: (f32, f32), cx: &mut Context<Self>) {
        let Some(item) = self.brain_item_at(at) else {
            return;
        };
        let Some(m) = self.night.map.map.clone() else {
            return;
        };
        if let Item::Note(i) = item {
            self.night.brain = Some(m.notes[i].path.clone());
        }
        let unit = f32::from(Look::from_app(cx).px(1.));
        let (w, h) = self.brain_view_size(unit);
        self.night.map.zoom_into(&m, item, w, h);
        cx.notify();
    }

    /// The selected note's panel, read again when the selection changes.
    fn brain_panel(&mut self, m: &NotesMap) -> Option<NotePanel> {
        let i = self.brain_selected(m)?;
        let path = m.notes[i].path.clone();
        if self
            .night
            .map
            .panel
            .as_ref()
            .is_some_and(|(p, _)| *p == path)
        {
            return self.night.map.panel.as_ref().and_then(|(_, p)| p.clone());
        }
        let l = self.loaded.as_ref()?;
        let (cfg, ws) = match &l.setup {
            hippo_view::setup::Setup::Ready { cfg, ws } => (cfg, ws),
            _ => return None,
        };
        let history = l.panels.as_ref().and_then(|p| p.history.as_ref().ok());
        let lessons = hippo_core::lessons::list(&cfg.home).unwrap_or_default();
        let rollup = hippo_core::rollup::Rollup::open(&cfg.home).ok().flatten();
        let now = time::OffsetDateTime::now_utc();
        let panel = hippo_view::night::note::build(
            &hippo_view::night::note::Inputs {
                ws,
                map: m,
                history,
                lessons: &lessons,
                rollup: rollup.as_ref(),
                now,
                offset: crate::ui::status::local_offset(),
            },
            &path,
        );
        self.night.map.panel = Some((path, panel.clone()));
        panel
    }

    /// The panel beside the map: the selected note.
    pub(crate) fn render_note_panel(
        &mut self,
        look: &Look,
        t: &Nt,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(m) = self.brain_map() else {
            return div().into_any_element();
        };
        let Some(p) = self.brain_panel(&m) else {
            return div()
                .text_size(look.text(14.))
                .text_color(t.muted)
                .child("Pick a note on the map.")
                .into_any_element();
        };
        let group = |text: &str| {
            div()
                .mt(look.px(18.))
                .mb(look.px(8.))
                .text_size(look.text(13.))
                .text_color(t.muted)
                .child(text.to_owned())
        };
        let stat = |n: String, words: &str| {
            div()
                .flex_1()
                .min_w_0()
                .p(look.px(12.))
                .rounded(look.px(10.))
                .bg(t.inset)
                .flex()
                .flex_col()
                .gap(look.px(2.))
                .child(
                    div()
                        .text_size(look.text(20.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.text)
                        .child(n),
                )
                .child(
                    div()
                        .text_size(look.text(12.5))
                        .text_color(t.faint)
                        .child(words.to_owned()),
                )
        };
        let path = p.path.clone();
        let mut col = div()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(10.))
                    .child(kit::tile(look, t, "note", Tone::Plain))
                    .child(div().text_size(look.text(13.5)).text_color(t.muted).child(
                        if p.folder.is_empty() {
                            "Notes".to_owned()
                        } else {
                            p.folder.clone()
                        },
                    )),
            )
            .child(
                div()
                    .mt(look.px(14.))
                    .text_size(look.text(22.))
                    .line_height(look.text(27.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(t.text)
                    .child(p.title.clone()),
            )
            .child(
                div()
                    .mt(look.px(10.))
                    .text_size(look.text(14.))
                    .line_height(look.text(21.))
                    .text_color(t.muted)
                    .child(p.summary.clone()),
            )
            .child(
                div()
                    .mt(look.px(12.))
                    .flex()
                    .flex_wrap()
                    .gap(look.px(6.))
                    .children(
                        p.chip
                            .clone()
                            .map(|(w, tone)| kit::chip(look, t, w, Tone::of(tone))),
                    )
                    .child(kit::chip(
                        look,
                        t,
                        if p.reads_week == 0 {
                            "not read this week".to_owned()
                        } else {
                            format!("read {}× this week", p.reads_week)
                        },
                        Tone::Plain,
                    )),
            )
            .child(
                div()
                    .mt(look.px(14.))
                    .flex()
                    .gap(look.px(8.))
                    .child(stat(p.reads_week.to_string(), "reads this week"))
                    .child(stat(p.rules_kept.to_string(), "rules kept"))
                    .child(stat(p.changes.to_string(), "changes")),
            );
        if !p.rules.is_empty() {
            col = col.child(group("Rules"));
            for r in &p.rules {
                col = col.child(
                    div()
                        .flex()
                        .items_start()
                        .gap(look.px(10.))
                        .mb(look.px(8.))
                        .child(
                            div()
                                .mt(look.px(6.))
                                .size(look.px(8.))
                                .flex_shrink_0()
                                .rounded(look.px(2.))
                                .bg(t.you),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(look.text(14.))
                                .line_height(look.text(20.))
                                .text_color(t.text)
                                .child(format!("{} {}", r.rule, r.kept)),
                        ),
                );
            }
        }
        for (k, r) in p.pending.iter().enumerate() {
            let id = r.id;
            col = col.child(
                div()
                    .mt(look.px(12.))
                    .p(look.px(14.))
                    .rounded(look.px(12.))
                    .border(look.fixed(1.5))
                    .border_dashed()
                    .border_color(t.you_line)
                    .flex()
                    .flex_col()
                    .gap(look.px(10.))
                    .child(
                        div()
                            .text_size(look.text(14.5))
                            .line_height(look.text(21.))
                            .text_color(t.text)
                            .child(r.rule.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(look.px(6.))
                            .child(
                                kit::button(look, t, ("note-keep", k), None, "Keep", Btn::Go, true)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.keep_lesson(id, None, cx)
                                    })),
                            )
                            .child(
                                kit::button(
                                    look,
                                    t,
                                    ("note-dismiss", k),
                                    None,
                                    "Dismiss",
                                    Btn::Quiet,
                                    true,
                                )
                                .on_click(
                                    cx.listener(move |this, _, _, cx| this.dismiss_lesson(id, cx)),
                                ),
                            )
                            .child(
                                div()
                                    .text_size(look.text(13.))
                                    .text_color(t.faint)
                                    .child("waiting for you"),
                            ),
                    ),
            );
            if !r.evidence.is_empty() {
                col = col.child(group("Evidence: what you said"));
                for e in r.evidence.iter().take(4) {
                    col = col.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(look.px(4.))
                            .mb(look.px(10.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(look.px(8.))
                                    .children(e.agent.as_ref().map(|a| kit::badge(look, t, a)))
                                    .child(
                                        div()
                                            .text_size(look.text(12.5))
                                            .text_color(t.faint)
                                            .child(e.when.clone()),
                                    ),
                            )
                            .children(e.said.as_ref().map(|s| kit::said(look, t, s, 3))),
                    );
                }
            }
        }
        if !p.read_by.is_empty() {
            col = col.child(group("Read by")).child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(look.px(10.))
                    .children(p.read_by.iter().map(|(a, n)| {
                        div()
                            .flex()
                            .items_center()
                            .gap(look.px(6.))
                            .child(kit::badge(look, t, a))
                            .child(
                                div()
                                    .text_size(look.text(13.))
                                    .text_color(t.muted)
                                    .child(format!("{n}×")),
                            )
                    })),
            );
        }
        let link_rows = |label: &str, links: &[(PathBuf, String)]| {
            (!links.is_empty()).then(|| {
                div()
                    .flex()
                    .flex_col()
                    .child(group(label))
                    .children(links.iter().take(6).map(|(_, title)| {
                        div()
                            .flex()
                            .items_center()
                            .gap(look.px(8.))
                            .h(look.px(32.))
                            .px(look.px(10.))
                            .mb(look.px(4.))
                            .rounded(look.px(8.))
                            .bg(t.inset)
                            .child(kit::icon(look, "note", 14., t.night))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(look.text(13.5))
                                    .text_color(t.text)
                                    .child(title.clone()),
                            )
                    }))
            })
        };
        col = col
            .children(link_rows("Links to", &p.links_out))
            .children(link_rows("Linked from", &p.links_in));
        if !p.receipts.is_empty() {
            col = col.child(group("Receipts: every change, with an undo"));
            for (k, r) in p.receipts.iter().enumerate().take(5) {
                let undo = r.undo.clone();
                col = col.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(look.px(10.))
                        .p(look.px(12.))
                        .mb(look.px(6.))
                        .rounded(look.px(10.))
                        .bg(t.inset)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(look.px(2.))
                                .child(
                                    div()
                                        .text_size(look.text(13.5))
                                        .text_color(t.text)
                                        .line_clamp(2)
                                        .child(r.what.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(look.text(12.5))
                                        .text_color(t.faint)
                                        .child(r.when.clone()),
                                ),
                        )
                        .when_some(undo, |d, args| {
                            d.child(
                                kit::button(
                                    look,
                                    t,
                                    ("receipt-undo", k),
                                    Some("undo"),
                                    "Undo",
                                    Btn::Quiet,
                                    true,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.start_job(
                                            crate::flow::Purpose::Review,
                                            vec![args.clone()],
                                            cx,
                                        );
                                    },
                                )),
                            )
                        }),
                );
            }
        }
        col = col.child(
            div().mt(look.px(16.)).child(
                kit::button(
                    look,
                    t,
                    "note-open",
                    Some("open"),
                    "Open the note",
                    Btn::Second,
                    false,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.state.tabs.open(crate::state::Tab::Note(path.clone()));
                    cx.notify();
                })),
            ),
        );
        col.into_any_element()
    }

    /// B's board: the entry note, then each folder over a grid of cards.
    fn render_brain_board(
        &mut self,
        look: &Look,
        t: &Nt,
        m: &NotesMap,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self.brain_selected(m);
        let near: Vec<usize> = selected
            .map(|i| m.neighbours(i).into_iter().collect())
            .unwrap_or_default();
        let card = |this: &mut Shell, i: usize, cx: &mut Context<Shell>| {
            let n = &m.notes[i];
            let sel = selected == Some(i);
            let linked = near.contains(&i);
            let path = n.path.clone();
            kit::select_card(look, t, sel)
                .id(("board-note", i))
                .flex_1()
                .min_w(look.px(240.))
                .flex()
                .flex_col()
                .gap(look.px(8.))
                .when(linked, |d| d.border_dashed().border_color(t.night_line))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.night.brain = Some(path.clone());
                    cx.notify();
                }))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(look.px(8.))
                        .child(kit::icon(
                            look,
                            if n.flags.entry { "start" } else { "note" },
                            16.,
                            t.muted,
                        ))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(look.text(15.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(t.text)
                                .child(n.title.clone()),
                        ),
                )
                .child(
                    div()
                        .text_size(look.text(13.5))
                        .text_color(t.muted)
                        .line_clamp(2)
                        .child(n.summary.clone()),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(look.px(6.))
                        .when(linked, |d| {
                            d.child(kit::chip(look, t, "linked", Tone::Night))
                        })
                        .children(
                            n.flags
                                .chip()
                                .map(|(w, tone)| kit::chip(look, t, w, Tone::of(tone))),
                        )
                        .child(kit::chip(look, t, n.reads_label(), Tone::Plain)),
                )
                .into_any_element()
                .pipe(|e| {
                    let _ = this;
                    e
                })
        };
        let mut col = div()
            .flex()
            .flex_col()
            .gap(look.px(14.))
            .px(look.px(18.))
            .pt(look.px(24.))
            .child(
                div()
                    .text_size(look.text(13.5))
                    .text_color(t.muted)
                    .child(m.kicker.clone()),
            )
            .child(
                div()
                    .text_size(look.text(24.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(t.text)
                    .child(m.headline.clone()),
            )
            .child(
                div()
                    .text_size(look.text(15.))
                    .text_color(t.muted)
                    .child(m.lead.clone()),
            );
        // The blocks in the person's order, column by column.
        let blocks: Vec<Block> = m.arrangement.keys().filter_map(|k| m.block(k)).collect();
        for b in blocks {
            let f = match b {
                Block::Note(i) => {
                    col = col.child(card(self, i, cx));
                    continue;
                }
                Block::Folder(f) => &m.folders[f],
            };
            col = col.child(
                div()
                    .mt(look.px(10.))
                    .flex()
                    .items_center()
                    .gap(look.px(8.))
                    .child(kit::icon(look, "folder", 16., t.muted))
                    .child(
                        div()
                            .text_size(look.text(17.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.text)
                            .child(if f.name.is_empty() {
                                "Notes".to_owned()
                            } else {
                                f.name.clone()
                            }),
                    )
                    .child(
                        div()
                            .text_size(look.text(14.))
                            .text_color(t.faint)
                            .child(hippo_view::night::count(f.notes.len(), "note", "notes")),
                    ),
            );
            for pair in f.notes.chunks(2) {
                let mut row = div().flex().gap(look.px(12.));
                for &i in pair {
                    row = row.child(card(self, i, cx));
                }
                if pair.len() == 1 {
                    row = row.child(div().flex_1().min_w(look.px(240.)));
                }
                col = col.child(row);
            }
        }
        col = col.child(
            div().mt(look.px(20.)).child(
                kit::card(look, t)
                    .rounded(look.px(18.))
                    .p(look.px(20.))
                    .child(self.render_note_panel(look, t, cx)),
            ),
        );
        col.into_any_element()
    }
}

/// `x.pipe(f)`: `f(x)`.
trait Pipe: Sized {
    fn pipe<R>(self, f: impl FnOnce(Self) -> R) -> R {
        f(self)
    }
}

impl<T> Pipe for T {}

/// The path of a note relative to the workspace, as the map holds it.
#[allow(dead_code)]
fn same_path(a: &Path, b: &Path) -> bool {
    hippo_core::notes::normalise(a) == hippo_core::notes::normalise(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_about_a_point_keeps_it_still() {
        let m = map::synthetic(60, 6, 80);
        let mut v = MapView::default();
        v.fit(&m, 1000., 700.);
        let at = (400., 300.);
        let before = v.to_map(at);
        let z = v.zoom * 1.1;
        v.zoom_at(&m, z, at);
        let after = v.to_map(at);
        assert!((before.0 - after.0).abs() < 0.01 && (before.1 - after.1).abs() < 0.01);
    }

    #[test]
    fn crossing_into_in_keeps_the_card_under_the_pointer() {
        let m = map::synthetic(60, 6, 80);
        let mut v = MapView {
            zoom: 1.5,
            pan: (0., 0.),
            ..MapView::default()
        };
        let card = m.mid.card(Item::Note(7)).unwrap().rect;
        // Point at the card's middle, in view points.
        let at = (
            (card.x + card.w / 2. - v.pan.0) * v.zoom,
            (card.y + card.h / 3. - v.pan.1) * v.zoom,
        );
        v.zoom_at(&m, 1.8, at);
        assert_eq!(v.level(), Level::In);
        let (mx, my) = v.to_map(at);
        let inner = m.inner.card(Item::Note(7)).unwrap().rect;
        assert!(inner.contains(mx, my), "{mx},{my} not in {inner:?}");
    }

    #[test]
    fn drag_and_scroll_pan_by_screen_points() {
        let mut v = MapView {
            zoom: 2.,
            pan: (100., 100.),
            ..MapView::default()
        };
        v.pan_by(40., -20.);
        assert_eq!(v.pan, (80., 110.));
    }

    #[test]
    fn a_double_click_on_a_folder_zooms_to_its_notes() {
        let m = map::synthetic(200, 8, 300);
        let mut v = MapView::default();
        v.fit(&m, 1000., 700.);
        assert_eq!(v.level(), Level::Out);
        v.zoom_into(&m, Item::Folder(0), 1000., 700.);
        assert_ne!(v.level(), Level::Out);
    }
}
