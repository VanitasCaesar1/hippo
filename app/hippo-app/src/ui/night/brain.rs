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
pub const HEAD: f32 = 128.;
/// A lifted block is drawn this much larger, raised over the map.
const LIFT_SCALE: f32 = 1.04;
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
    /// The zoom the map was last fitted at: the smallest.
    pub fit_zoom: f32,
    /// The pan the map was last fitted at.
    pub fit_pan: (f32, f32),
    /// Whether the view showed the map fitted when the current drag began.
    pub lift_fitted: bool,
    /// The view moving to `zoom` and `pan` from where it was: a refit
    /// after a drop from the fitted view.
    pub camera: Option<Camera>,
}

/// Where the view was when it started moving, and for how long it moves.
#[derive(Clone, Copy)]
pub struct Camera {
    pub zoom: f32,
    pub pan: (f32, f32),
    pub start: Instant,
    pub ms: f32,
}

impl Camera {
    /// The zoom and pan drawn now, on the way to `(zoom, pan)`; whether it
    /// is still moving.
    fn now(&self, zoom: f32, pan: (f32, f32)) -> (f32, (f32, f32), bool) {
        let t = if let Some(p) = dev_progress() {
            p
        } else if self.ms <= 0. {
            1.
        } else {
            (self.start.elapsed().as_secs_f32() * 1000. / self.ms).min(1.)
        };
        let e = 1. - (1. - t).powi(3);
        let l = |a: f32, b: f32| a + (b - a) * e;
        (
            l(self.zoom, zoom),
            (l(self.pan.0, pan.0), l(self.pan.1, pan.1)),
            t < 1.,
        )
    }
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
            fit_zoom: 1.,
            fit_pan: (0., 0.),
            lift_fitted: false,
            camera: None,
        }
    }
}

/// Lines under the pointer at Out: their ends (several where a trunk is
/// shared) and the view point.
pub type HoverLine = (Vec<map::Hot>, (f32, f32));

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
    /// The last paint's badges (bounds in window px, the number, lit) and
    /// the lines lit along each; and its arrowheads' bounds.
    pub badges: Vec<(map::Rect, usize, bool, usize)>,
    pub heads: Vec<map::Rect>,
}

impl MapView {
    pub fn level(&self) -> Level {
        Level::of(self.zoom)
    }

    /// Zooms to `zoom` about a point of the view (`(px, py)` in points from
    /// its top left), keeping what is under it there, across levels too.
    pub fn zoom_at(&mut self, map: &NotesMap, zoom: f32, at: (f32, f32)) {
        self.camera = None;
        // At or below the fit: exactly the fit, its pan too, on every map.
        if self.fitted && zoom <= self.min_zoom() + 1e-4 {
            self.zoom = self.min_zoom();
            self.pan = self.fit_pan;
            return;
        }
        let zoom = zoom.clamp(self.min_zoom(), map::MAX_ZOOM);
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

    /// The smallest zoom: the one that shows the whole map. Further out
    /// the map would only shrink into the middle of an empty view.
    pub fn min_zoom(&self) -> f32 {
        if self.fitted {
            self.fit_zoom.clamp(map::MIN_ZOOM, 1.)
        } else {
            map::MIN_ZOOM
        }
    }

    /// Fits the whole map in a `w` × `h` view.
    pub fn fit(&mut self, map: &NotesMap, w: f32, h: f32) {
        let (z, pan) = map.fit(w, h);
        self.camera = None;
        self.fit_zoom = z;
        self.fit_pan = pan;
        self.zoom = z;
        self.pan = pan;
        self.fitted = true;
    }

    /// Pans by a drag or scroll of `(dx, dy)` points.
    pub fn pan_by(&mut self, dx: f32, dy: f32) {
        self.camera = None;
        self.pan.0 -= dx / self.zoom;
        self.pan.1 -= dy / self.zoom;
    }

    /// Whether the view shows the map exactly as fitted.
    pub fn at_fit(&self) -> bool {
        self.fitted
            && (self.zoom - self.fit_zoom).abs() < 1e-3
            && (self.pan.0 - self.fit_pan.0).abs() + (self.pan.1 - self.fit_pan.1).abs() < 1.
    }

    /// Keeps the map in a `w` × `h` view, the rule every centring follows:
    /// a map wider (or higher) than the view leaves no empty ground past an
    /// edge it could fill; a smaller one is centred.
    pub fn clamp(&mut self, map: &NotesMap, w: f32, h: f32) {
        let lay = map.layout(self.level());
        let (vw, vh) = (w / self.zoom, h / self.zoom);
        let pad = map::FIT_PAD / self.zoom;
        let axis = |pan: f32, size: f32, view: f32| {
            if size + 2. * pad <= view {
                -(view - size) / 2.
            } else {
                pan.clamp(-pad, size + pad - view)
            }
        };
        self.pan = (
            axis(self.pan.0, lay.width, vw),
            axis(self.pan.1, lay.height, vh),
        );
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
        self.camera = None;
        self.clamp(map, w, h);
    }
}

/// What a paint of the map needs, cloned into the canvas.
struct Paint {
    map: Rc<NotesMap>,
    zoom: f32,
    pan: (f32, f32),
    selected: Option<usize>,
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
    camera: Option<Camera>,
}

/// `a` laid over `b` at `k`, as one opaque colour: lines that run together
/// draw as one, never brighter where they overlap.
fn mix(a: Hsla, b: Hsla, k: f32) -> Hsla {
    let (a, b) = (gpui::Rgba::from(a), gpui::Rgba::from(b));
    let k = (k * a.a).clamp(0., 1.);
    Hsla::from(gpui::Rgba {
        r: a.r * k + b.r * (1. - k),
        g: a.g * k + b.g * (1. - k),
        b: a.b * k + b.b * (1. - k),
        a: 1.,
    })
}

/// `s` broken at spaces into lines no wider than `max` pixels as the font
/// sets them, at most `most` lines, the last cut with `…`.
fn wrap_px(
    window: &mut Window,
    s: &str,
    size_px: f32,
    f: &Font,
    max: f32,
    most: usize,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for w in s.split_whitespace() {
        let next = if cur.is_empty() {
            w.to_owned()
        } else {
            format!("{cur} {w}")
        };
        if cur.is_empty() || text_width(window, &next, size_px, f) <= max {
            cur = next;
        } else {
            out.push(std::mem::replace(&mut cur, w.to_owned()));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    if out.len() > most.max(1) {
        out.truncate(most.max(1));
        let last = out.last_mut().expect("one line");
        while !last.is_empty() && text_width(window, &format!("{last}…"), size_px, f) > max {
            match last.rfind(' ') {
                Some(i) => last.truncate(i),
                None => {
                    last.pop();
                }
            }
        }
        last.push('…');
    }
    out
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

/// A chip: its ground, a dot for a tinted one, its words, as wide as its
/// words. Returns its width.
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
    // A plain chip one step up from the card it sits on.
    let bg = if matches!(tone, Tone::Plain) && p.t.dark {
        p.t.selected
    } else {
        bg
    };
    let u = p.unit * z;
    // Never under 11 points on screen: further out the chip keeps 11 and
    // grows on the card.
    let size_px = ((12. * u).max(11. * p.unit) * 2.).round() / 2.;
    let u = u * (size_px / (12. * u)).max(1.);
    let h = 22. * u;
    if size_px < 4. {
        return 0.;
    }
    let dot = matches!(tone, Tone::You | Tone::Ok | Tone::Bad);
    let lead = if dot { 18. * u } else { 8. * u };
    let f = font(&p.family, FontWeight::MEDIUM);
    let tw = text_width(window, words, size_px, &f);
    let w = (lead + tw + 8. * u).min(max);
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
        &f,
        w - lead - 4. * u,
    );
    w
}

/// A line of text's width in pixels.
fn text_width(window: &mut Window, s: &str, size_px: f32, f: &Font) -> f32 {
    let run = TextRun {
        len: s.len(),
        font: f.clone(),
        color: gpui::black(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    f32::from(
        window
            .text_system()
            .shape_line(s.to_owned().into(), px(size_px), &[run], None)
            .width,
    )
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

/// `A`, `A, B`, `A, B, C`, or `4 others`.
fn names(v: Vec<String>) -> String {
    match v.len() {
        0 => String::new(),
        1..=3 => v.join(", "),
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

/// A polyline in pixels, its corners rounded to `radius`.
fn stroke_rounded(window: &mut Window, pts: &[(f32, f32)], width: f32, radius: f32, color: Hsla) {
    let mut v: Vec<(f32, f32)> = Vec::with_capacity(pts.len());
    for &q in pts {
        if v.last()
            .is_none_or(|l: &(f32, f32)| (l.0 - q.0).abs() + (l.1 - q.1).abs() > 0.05)
        {
            v.push(q);
        }
    }
    if v.len() < 2 {
        return;
    }
    let pt = |q: (f32, f32)| point(px(q.0), px(q.1));
    let mut b = gpui::PathBuilder::stroke(px(width));
    b.move_to(pt(v[0]));
    for i in 1..v.len() - 1 {
        let (a, c, n) = (v[i - 1], v[i], v[i + 1]);
        let (di, dn) = (
            ((c.0 - a.0).powi(2) + (c.1 - a.1).powi(2)).sqrt(),
            ((n.0 - c.0).powi(2) + (n.1 - c.1).powi(2)).sqrt(),
        );
        let r = radius.min(di / 2.).min(dn / 2.);
        if r < 0.5 {
            b.line_to(pt(c));
            continue;
        }
        let p0 = (c.0 - (c.0 - a.0) / di * r, c.1 - (c.1 - a.1) / di * r);
        let p1 = (c.0 + (n.0 - c.0) / dn * r, c.1 + (n.1 - c.1) / dn * r);
        b.line_to(pt(p0));
        b.curve_to(pt(p1), pt(c));
    }
    b.line_to(pt(v[v.len() - 1]));
    if let Ok(path) = b.build() {
        window.paint_path(path, color);
    }
}

fn paint_map(p: &Paint, b: Bounds<Pixels>, window: &mut Window, cx: &mut gpui::App) {
    let started = Instant::now();
    let (ox, oy) = (f32::from(b.origin.x), f32::from(b.origin.y));
    let (vw, vh) = (f32::from(b.size.width), f32::from(b.size.height));
    p.view.set((ox, oy, vw, vh));
    // A refit after a drop moves the view there.
    let (z, pan, flying) = match &p.camera {
        Some(c) => c.now(p.zoom, p.pan),
        None => (p.zoom, p.pan, false),
    };
    if flying {
        window.request_animation_frame();
    }
    let u = p.unit;
    let s = |mx: f32, my: f32| (ox + (mx - pan.0) * z * u, oy + (my - pan.1) * z * u);
    let level = Level::of(z);
    let view = map::Rect::new(pan.0, pan.1, vw / (z * u), vh / (z * u));
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
        // The slot stays where the block lands and fades in there; it
        // never travels with the blocks sliding aside.
        slot = Some(map::Rect::new(r.x, r.y, r.w, r.h));
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
    let gx = (-pan.0 * z * u).rem_euclid(step);
    let gy = (-pan.1 * z * u).rem_euclid(step);
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

    // Where the lifted block lands: its shape, a quiet dashed outline,
    // fading in as the others slide aside.
    if let Some(r) = slot {
        let k = slide.filter(|_| moving).map_or(1., |sl| sl.eased());
        let (x, y) = s(r.x, r.y);
        rrect(
            window,
            Bounds::new(point(px(x), px(y)), size(px(r.w * z * u), px(r.h * z * u))),
            (if level == Level::Out || matches!(lifted, Some(Block::Folder(_))) {
                18.
            } else {
                14.
            }) * z
                * u,
            t.you.opacity(if t.dark { 0.12 } else { 0.08 } * k),
            if t.dark {
                t.you.opacity(0.75 * k)
            } else {
                t.you_line.opacity(k)
            },
            (1.5 * u).max(1.),
            true,
        );
    }

    // Lines, behind the cards, one weight at every zoom
    // (docs/designs/brain-links.md › Lines): one trunk per card per gutter,
    // heads where links arrive, and on a lit line a dot where they leave.
    // The selected note's in a quiet neutral; while another card is
    // hovered its lines replace the selection's in every gutter they share
    // (`map::beside`), lit, so a lit line is never drawn beside a grey one.
    // A line to a card out of view (or within 8 px of the edge) runs on and
    // fades out over its last 24 px. While a block is lifted, slides,
    // settles or flies home, every line stays drawn, faint, its ends moving
    // with their cards, and no badge shows.
    let focus = p.focus;
    let mut drawn_lines = 0;
    let style = &p.map.style;
    let motion = lift.is_some() || moving;
    let faint = if motion { 0.35 } else { 1. };
    let lit_c = mix(t.you_stroke(), t.canvas, faint);
    let sel_focus = p.selected.map(|i| p.map.same_item(Item::Note(i), level));
    let other = p.hover.is_some() && p.hover == focus && sel_focus != focus;
    // (who, colour, lit, with badges)
    let mut sets: Vec<(Option<Item>, Hsla, bool, bool)> = Vec::new();
    if other {
        sets.push((sel_focus, mix(t.night, t.canvas, 0.3 * faint), false, false));
        sets.push((focus, lit_c, true, true));
    } else if p.hover.is_some() && p.hover == focus {
        sets.push((focus, lit_c, true, true));
    } else {
        sets.push((focus, mix(t.night, t.canvas, 0.55 * faint), false, true));
    }
    let width = style.line * u;
    let hot: Vec<map::Hot> = p
        .hover_line
        .as_ref()
        .map(|(ends, _)| ends.clone())
        .unwrap_or_default();
    // A line drawn where its cards are drawn now: its ends (and the root
    // and stub that lead to them) move with their blocks.
    let posed = |l: &map::Line| -> map::Line {
        let (a, b) = (pose_of(l.from), pose_of(l.to));
        let mut q = l.clone();
        let n = q.points.len();
        let p0 = a.at(l.points[0].0, l.points[0].1);
        let pn = b.at(l.points[n - 1].0, l.points[n - 1].1);
        let (d0, dn) = (p0.1 - l.points[0].1, pn.1 - l.points[n - 1].1);
        q.points[0] = p0;
        q.points[n - 1] = pn;
        if n > 2 {
            q.points[n - 2].1 += dn;
        }
        if n > 3 {
            q.points[1].1 += d0;
        }
        q
    };
    let hovered_lines: Vec<&map::Line> = if other {
        lay.shown(focus)
            .iter()
            .filter(|l| map::shows(l, &view))
            .collect()
    } else {
        Vec::new()
    };
    // The view less 8 px: a line ending past it fades instead of ending in
    // a head sliced by the canvas edge.
    let m8 = 8. / z;
    let inner = map::Rect::new(view.x + m8, view.y + m8, view.w - 2. * m8, view.h - 2. * m8);
    // Marks (heads and dots) and badges, drawn over the cards.
    let mut marks: Vec<(map::End, Hsla)> = Vec::new();
    let mut badges: Vec<((f32, f32), usize, bool, usize)> = Vec::new();
    let mut painted_heads: Vec<map::Rect> = Vec::new();
    let mut painted_badges: Vec<(map::Rect, usize, bool, usize)> = Vec::new();
    let strip = if level == Level::Out { 0. } else { 30. / z };
    for &(who, color, lit_set, with_badges) in &sets {
        let mut shown: Vec<&map::Line> = lay
            .shown(who)
            .iter()
            .filter(|l| map::shows(l, &view))
            .collect();
        if who != focus
            && let Some(h) = focus
        {
            shown = map::beside(&shown, &hovered_lines, h);
        }
        let drawn: Vec<map::Line> = if motion {
            shown.iter().map(|l| posed(l)).collect()
        } else {
            shown.iter().map(|l| (*l).clone()).collect()
        };
        let drawn_refs: Vec<&map::Line> = drawn.iter().collect();
        let mine = |l: &map::Line| hot.contains(&l.hot());
        // The lines under the pointer last, over the rest.
        for l in drawn
            .iter()
            .filter(|l| !mine(l))
            .chain(drawn.iter().filter(|l| mine(l)))
        {
            let pts: Vec<(f32, f32)> = l.points.iter().map(|q| s(q.0, q.1)).collect();
            stroke_rounded(
                window,
                &pts,
                width,
                style.corner * u,
                if mine(l) { lit_c } else { color },
            );
            drawn_lines += 1;
            // Past the inner view: the ground covers its last 24 px, more
            // and more, and everything after.
            for (piece, k) in map::fade(l, &inner, 24. / z, 6) {
                let pts: Vec<(f32, f32)> = piece.iter().map(|q| s(q.0, q.1)).collect();
                stroke_rounded(window, &pts, width + 1., 0., t.canvas.opacity(k));
            }
            let rest: Vec<(f32, f32)> = map::past(l, &inner).iter().map(|q| s(q.0, q.1)).collect();
            if rest.len() > 1 {
                stroke_rounded(window, &rest, width + 2., 0., t.canvas);
            }
        }
        let inside = |p: (f32, f32)| {
            p.0 >= inner.x && p.0 <= inner.right() && p.1 >= inner.y && p.1 <= inner.bottom()
        };
        for e in map::ends(&drawn_refs, &hot, lit_set) {
            if !inside(e.tip) {
                continue;
            }
            let e = map::End {
                tip: s(e.tip.0, e.tip.1),
                from: s(e.from.0, e.from.1),
                ..e
            };
            marks.push((e, if e.lit { lit_c } else { color }));
        }
        if with_badges && !motion {
            let margin = 8. / z;
            for bd in lay.badges(who) {
                // The lines drawn along the trunk, one per card: a badge
                // only where there are two or more.
                let n = map::badge_count(bd, &shown);
                if n < 2 {
                    continue;
                }
                if let Some(c) = style.badge_at(bd, z) {
                    let (hw, hh) = (style.badge_half(bd.lines) / z, style.badge / z);
                    let inside = c.0 - hw >= view.x + margin
                        && c.0 + hw <= view.x + view.w - margin
                        && c.1 - hh >= view.y + strip + margin
                        && c.1 + hh <= view.y + view.h - margin;
                    if inside {
                        let lit = if lit_set {
                            shown.iter().filter(|l| l.trunk == bd.trunk).count()
                        } else {
                            0
                        };
                        badges.push((s(c.0, c.1), n, lit_set, lit));
                    }
                }
            }
        }
    }

    // Far out a folder card keeps its name and its counts; further out,
    // its name.
    let far = level == Level::Out && z < map::OUT_ROWS_BELOW;
    let farther = level == Level::Out && z < map::OUT_META_BELOW;
    let sel_item = p.selected.map(|i| p.map.same_item(Item::Note(i), level));
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
                        color: gpui::hsla(0., 0., 0., if t.dark { 0.6 } else { 0.18 }),
                        offset: point(px(0.), px(14. * u)),
                        blur_radius: px(32. * u),
                        spread_radius: px(0.),
                        inset: false,
                    },
                    gpui::BoxShadow {
                        color: gpui::hsla(0., 0., 0., if t.dark { 0.45 } else { 0.1 }),
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
                    if t.dark { t.raised } else { t.surface },
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
            let look = CardLook {
                sel: sel_item == Some(c.item),
                hover: p.hover == Some(c.item) && lift.is_none(),
                lifted: pass,
            };
            match c.item {
                Item::Note(i) if level != Level::Out => {
                    let n = &p.map.notes[i];
                    paint_note(p, zk, n, look, level == Level::In, bounds, window, cx);
                }
                // At Out a card is its ground; its words and marks follow.
                _ => paint_out_card(p, zk, look, bounds, window),
            }
        }
        // Every word on the map, then the marks.
        for l in &lay.labels {
            if !mine(l.item)
                || (far && matches!(l.kind, LabelKind::Row | LabelKind::More))
                || (farther && l.kind == LabelKind::Meta)
            {
                continue;
            }
            let pose = pose_of(l.item);
            let (mx, my) = pose.at(l.rect.x, l.rect.y);
            if !pass && !map::Rect::new(mx, my, l.rect.w * pose.k, l.rect.h * pose.k).meets(&view) {
                continue;
            }
            let (x, y) = s(mx, my);
            let zk = z * pose.k;
            // A folder's name and count on the dotted ground get a backing,
            // so no dot shows through them.
            if matches!(l.kind, LabelKind::Frame | LabelKind::Count) {
                let size_px = l.size * zk * u;
                let f = font(
                    &p.family,
                    if l.kind.bold() {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::NORMAL
                    },
                );
                let tw = l
                    .lines
                    .first()
                    .map_or(0., |w| text_width(window, w, size_px, &f));
                let pad = 4. * zk * u;
                window.paint_quad(gpui::fill(
                    Bounds::new(
                        point(px(x - pad), px(y)),
                        size(
                            px(tw.min(l.rect.w * zk * u) + 2. * pad),
                            px(size_px * LEADING),
                        ),
                    ),
                    t.canvas,
                ));
            }
            paint_label(p, zk, l, x, y, l.rect.w * zk * u, window, cx);
        }
        for m in &lay.marks {
            let icon = m.tone.strip_prefix("icon-");
            if !mine(m.item) || (far && m.row) || (farther && icon.is_none()) {
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
            if let Some(name) = icon {
                let path = match name {
                    "folder" => "icons/folder.svg",
                    "flag" => "icons/n-start.svg",
                    _ => "icons/n-note.svg",
                };
                let color = if name == "flag" { t.night } else { t.muted };
                let _ = window.paint_svg(
                    b,
                    path.into(),
                    None,
                    gpui::TransformationMatrix::unit(),
                    color,
                    cx,
                );
                continue;
            }
            let fill = match m.tone {
                "ok" => Some(t.ok),
                "bad" => Some(t.bad),
                "faint" => Some(t.faint),
                "night" => Some(t.night),
                _ => None,
            };
            match fill {
                Some(c) => window.paint_quad(gpui::fill(b, c).corner_radii(px(d / 2.))),
                // No flag: a ring, so the row still reads as a note.
                // A rule waiting: a lime ring, so it never reads as the
                // fixed mark's filled green.
                None if m.tone == "you" => window.paint_quad(gpui::quad(
                    b,
                    px(d / 2.),
                    gpui::transparent_black(),
                    px((2. * u).max(1.5)),
                    t.you_mark(),
                    gpui::BorderStyle::Solid,
                )),
                // No flag, or none of a count: a ring, so the row still
                // reads as a note.
                None => window.paint_quad(gpui::quad(
                    b,
                    px(d / 2.),
                    gpui::transparent_black(),
                    px((1.5 * u).max(1.)),
                    if m.tone == "zero" || t.dark {
                        t.line2
                    } else {
                        t.faint
                    },
                    gpui::BorderStyle::Solid,
                )),
            }
        }
        // Over the resting blocks (under a lifted one): a head where each
        // line's links arrive, and one badge per trunk of links out, on a
        // halo of ground that breaks any guide behind it.
        if !pass {
            let a = style.arrow * u;
            for &(e, c) in &marks {
                let (tip, from) = (e.tip, e.from);
                if !e.head {
                    // A dot where a lit line's links leave.
                    let d = (2. * style.line + 1.5) * u;
                    window.paint_quad(
                        gpui::fill(
                            Bounds::new(
                                point(px(tip.0 - d / 2.), px(tip.1 - d / 2.)),
                                size(px(d), px(d)),
                            ),
                            c,
                        )
                        .corner_radii(px(d / 2.)),
                    );
                    continue;
                }
                let (dx, dy) = (tip.0 - from.0, tip.1 - from.1);
                let len = (dx * dx + dy * dy).sqrt().max(0.01);
                let (ux, uy) = (dx / len, dy / len);
                let base = (tip.0 - ux * a, tip.1 - uy * a);
                let corners = [
                    tip,
                    (base.0 - uy * a * 0.5, base.1 + ux * a * 0.5),
                    (base.0 + uy * a * 0.5, base.1 - ux * a * 0.5),
                ];
                let (x0, y0) = corners
                    .iter()
                    .fold((f32::MAX, f32::MAX), |m, q| (m.0.min(q.0), m.1.min(q.1)));
                let (x1, y1) = corners
                    .iter()
                    .fold((f32::MIN, f32::MIN), |m, q| (m.0.max(q.0), m.1.max(q.1)));
                painted_heads.push(map::Rect::new(x0, y0, x1 - x0, y1 - y0));
                let mut b = gpui::PathBuilder::fill();
                b.move_to(point(px(tip.0), px(tip.1)));
                b.line_to(point(px(base.0 - uy * a * 0.5), px(base.1 + ux * a * 0.5)));
                b.line_to(point(px(base.0 + uy * a * 0.5), px(base.1 - ux * a * 0.5)));
                b.line_to(point(px(tip.0), px(tip.1)));
                if let Ok(path) = b.build() {
                    window.paint_path(path, c);
                }
            }
            // Badges sit on their line, no gap: a lit trunk's is lime, the
            // selection's a quiet ring.
            let f = font(&p.family, FontWeight::SEMIBOLD);
            let size_px = 11. * u;
            for &(at, n, lit, lit_lines) in &badges {
                let words = n.to_string();
                let tw = text_width(window, &words, size_px, &f);
                let (w, h) = (2. * style.badge_half(n) * u, 2. * style.badge * u);
                painted_badges.push((
                    map::Rect::new(at.0 - w / 2., at.1 - h / 2., w, h),
                    n,
                    lit,
                    lit_lines,
                ));
                let bounds = Bounds::new(
                    point(px(at.0 - w / 2.), px(at.1 - h / 2.)),
                    size(px(w), px(h)),
                );
                let (fill, ring, ink) = if lit {
                    (t.you, t.you, t.you_ink)
                } else {
                    (t.surface, mix(t.night, t.canvas, 0.55), t.text)
                };
                rrect(window, bounds, h / 2., fill, ring, (1.2 * u).max(1.), false);
                text(
                    window,
                    cx,
                    &words,
                    at.0 - tw / 2.,
                    at.1 - size_px * 1.25 / 2.,
                    size_px,
                    ink,
                    &f,
                    tw + 2.,
                );
            }
        }
    }

    // Folders whose name band has scrolled above the view keep their names
    // in an opaque strip at its top; the cards pass under it.
    if level != Level::Out {
        let above: Vec<(usize, map::Rect)> = lay
            .frames
            .iter()
            .filter(|(f, r)| {
                !is_lifted(Item::Folder(*f))
                    && r.y < view.y
                    && r.bottom() > view.y + 60. / z
                    && r.right() > view.x
                    && r.x < view.x + view.w
            })
            .copied()
            .collect();
        if !above.is_empty() {
            let h = 30. * u;
            window.paint_quad(gpui::fill(
                Bounds::new(point(px(ox), px(oy)), size(px(vw), px(h))),
                t.canvas,
            ));
            window.paint_quad(gpui::fill(
                Bounds::new(point(px(ox), px(oy + h - 1.)), size(px(vw), px(1.))),
                t.line,
            ));
            // Cards pass under it fading, never hard-cut.
            window.paint_quad(gpui::fill(
                Bounds::new(point(px(ox), px(oy + h)), size(px(vw), px(14. * u))),
                gpui::linear_gradient(
                    180.,
                    gpui::linear_color_stop(t.canvas.opacity(0.85), 0.),
                    gpui::linear_color_stop(t.canvas.opacity(0.), 1.),
                ),
            ));
            let name_f = font(&p.family, FontWeight::SEMIBOLD);
            let count_f = font(&p.family, FontWeight::NORMAL);
            for (f, r) in above {
                let item = Item::Folder(f);
                let words = |k: LabelKind| {
                    lay.labels
                        .iter()
                        .find(|l| l.item == item && l.kind == k)
                        .and_then(|l| l.lines.first().cloned())
                        .unwrap_or_default()
                };
                let (name, count) = (words(LabelKind::Frame), words(LabelKind::Count));
                let (x, _) = s(r.x + map::FRAME_PAD, 0.);
                let x = x.max(ox + 12. * u);
                let nw = text_width(window, &name, 13. * u, &name_f);
                let ty = oy + (h - 13. * u * 1.25) / 2.;
                text(window, cx, &name, x, ty, 13. * u, t.muted, &name_f, nw + 2.);
                text(
                    window,
                    cx,
                    &count,
                    x + nw + 10. * u,
                    ty + 0.5 * u,
                    12. * u,
                    t.faint,
                    &count_f,
                    200. * u,
                );
            }
        }
    }

    let edge = (1. * u).max(1.);
    // The map's foot: a hairline where it ends above the Ask bar; and its
    // right edge, where it ends before the panel.
    window.paint_quad(gpui::fill(
        Bounds::new(point(px(ox), px(oy + vh - edge)), size(px(vw), px(edge))),
        t.line,
    ));
    window.paint_quad(gpui::fill(
        Bounds::new(point(px(ox + vw - edge), px(oy)), size(px(edge), px(vh))),
        t.line,
    ));

    let mut f = p.frames.borrow_mut();
    let now = Instant::now();
    if let Some(last) = f.last {
        f.interval_ms.push((now - last).as_secs_f32() * 1000.);
    }
    f.last = Some(now);
    let ms = started.elapsed().as_secs_f32() * 1000.;
    f.paint_ms.push(ms);
    f.drawn = (drawn, drawn_lines);
    f.heads = painted_heads;
    f.badges = painted_badges;
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

/// How a card is drawn: selected (a ring that stays while another card is
/// hovered), hovered (an accent edge and a lift), lifted by a drag.
#[derive(Clone, Copy)]
struct CardLook {
    sel: bool,
    hover: bool,
    lifted: bool,
}

/// A card's ground and edge: one resting edge for every card; the
/// selection ring; the hover's accent edge and shadow; a lifted card a
/// step lighter in dark.
fn card_ground(p: &Paint, look: CardLook, radius: f32, b: Bounds<Pixels>, window: &mut Window) {
    let t = &p.t;
    let u = p.unit;
    if look.hover && !look.lifted {
        window.paint_drop_shadows(
            b,
            gpui::Corners::all(px(radius)),
            &[gpui::BoxShadow {
                color: gpui::hsla(0., 0., 0., if t.dark { 0.5 } else { 0.1 }),
                offset: point(px(0.), px(4. * u)),
                blur_radius: px(14. * u),
                spread_radius: px(0.),
                inset: false,
            }],
        );
    }
    // Lifted in dark: a lighter ground and a ring, so it reads as raised.
    let fill = if look.lifted && t.dark {
        t.selected
    } else {
        t.surface
    };
    let (border, bw) = if look.sel {
        (t.night, (2. * u).max(1.5))
    } else if look.hover {
        // The hovered card's ring is lime in both themes.
        (t.you, (1.5 * u).max(1.))
    } else if look.lifted && t.dark {
        (t.night_line, (1.5 * u).max(1.))
    } else if look.lifted {
        (t.line2, (1. * u).max(1.))
    } else {
        (t.line, (1. * u).max(1.))
    };
    rrect(window, b, radius, fill, border, bw, false);
}

/// An Out card's ground: a pinned note's or a folder's.
fn paint_out_card(p: &Paint, z: f32, look: CardLook, b: Bounds<Pixels>, window: &mut Window) {
    card_ground(p, look, 18. * p.unit * z, b, window);
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
    look: CardLook,
    inner: bool,
    b: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    let t = &p.t;
    let u = p.unit * z;
    card_ground(p, look, 14. * u, b, window);
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
    // The title is a label, drawn after the cards. Closer in the words
    // wrap to the card's inner width as the font sets them, on no more
    // lines than [`map::in_text`] made room for; at Mid the summary is one
    // line. Nothing is set under 11 points: further out a summary or chip
    // keeps 11 and is cut with `…`.
    let body = font(&p.family, FontWeight::NORMAL);
    let wrapped = inner.then(|| map::in_text(n));
    let inner_w = w - 2. * pad;
    // Never under 11 points on screen.
    let sum_px = (map::IN_SUMMARY_SIZE * u).max(11. * p.unit);
    let summary: Vec<String> = match &wrapped {
        Some(it) => wrap_px(window, &n.summary, sum_px, &body, inner_w, it.summary.len()),
        None => vec![n.summary.clone()],
    };
    for (k, line) in summary.iter().enumerate() {
        text(
            window,
            cx,
            line,
            x + pad,
            y + (map::IN_SUMMARY + k as f32 * IN_LINE) * u,
            sum_px,
            t.muted,
            &body,
            w - 2. * pad,
        );
    }
    let cy = y + wrapped.as_ref().map_or(CARD_H - 38., map::in_chips_y) * u;
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
    let Some(it) = wrapped else {
        return;
    };
    // Rules waiting (an accent dot), the person's words (on the accent's
    // ground), rules kept (a quiet dot).
    let mut ly = cy + (22. + 14.) * u;
    let tx = x + pad + 18. * u;
    let row_px = map::IN_ROW_SIZE * u;
    for (est, kind) in &it.rows {
        let lines = &wrap_px(
            window,
            &est.join(" "),
            row_px,
            &body,
            x + w - pad - tx,
            est.len(),
        );
        let h = lines.len() as f32 * IN_LINE * u;
        match *kind {
            "said" => window.paint_quad(
                gpui::fill(
                    Bounds::new(
                        point(px(tx - 6. * u), px(ly - 1. * u)),
                        size(px((x + w - pad - tx + 6. * u).max(0.)), px(h)),
                    ),
                    t.you_bg,
                )
                .corner_radii(px(4. * u)),
            ),
            k => {
                let d = 7. * u;
                let b = Bounds::new(
                    point(px(x + pad + 2. * u), px(ly + (IN_LINE * u - d) / 2.)),
                    size(px(d), px(d)),
                );
                // A rule waiting: a lime ring; one kept: a quiet dot.
                if k == "waiting" {
                    window.paint_quad(gpui::quad(
                        b,
                        px(d / 2.),
                        gpui::transparent_black(),
                        px((1.5 * u).max(1.)),
                        t.you_mark(),
                        gpui::BorderStyle::Solid,
                    ));
                } else {
                    window.paint_quad(gpui::fill(b, t.faint).corner_radii(px(d / 2.)));
                }
            }
        }
        for (k, line) in lines.iter().enumerate() {
            text(
                window,
                cx,
                line,
                tx,
                ly + k as f32 * IN_LINE * u + 2. * u,
                map::IN_ROW_SIZE * u,
                if *kind == "said" { t.you_text } else { t.text },
                &body,
                x + w - pad - tx + 2.,
            );
        }
        ly += h + map::IN_ROW_GAP * u;
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
        // The map ends a gutter short of the panel, a docked column, and
        // above the Ask bar: nothing floats over it, and fitting and
        // centring use the map's own edges.
        let view_w = left_w;
        let map_h = (view_h - HEAD - super::ASK_CLEAR).max(100.);
        let canvas_w = view_w;
        if !self.night.map.fitted {
            // Hippo's layout made for this view, then the whole map shown.
            let fit_h = map_h;
            self.night.map.view_size = Some((view_w, fit_h));
            if !m.is_arranged() {
                self.night.map.map = Some(Rc::new(m.fitted(view_w, fit_h)));
            }
            let m = self.night.map.map.clone().unwrap_or(m.clone());
            self.night.map.fit(&m, view_w, map_h);
            if let Some(z) = self.night.dev_zoom.take() {
                let at = (view_w / 2., map_h / 2.);
                self.night.map.zoom_at(&m, z.max(map::MIN_ZOOM), at);
                // Dev-only: the selected card in the middle of the view,
                // then the map kept in it; a zoom at or below the fit is the
                // fit, as it is for a person.
                let zz = self.night.map.zoom;
                let centre = zz > self.night.map.min_zoom() + 1e-4;
                let sel = Item::Note(self.brain_selected(&m).unwrap_or(0));
                let item = m.same_item(sel, Level::of(zz));
                if centre && let Some(c) = m.layout(Level::of(zz)).card(item) {
                    let r = c.rect;
                    self.night.map.pan = (
                        r.x + r.w / 2. - view_w / 2. / zz,
                        r.y + r.h.min(300.) / 2. - map_h / 2. / zz,
                    );
                }
                // The selected card's top stays in the view, clear of the
                // strip of folder names; then the centring is clamped to the
                // map's bounds.
                if centre && let Some(c) = m.layout(Level::of(zz)).card(item) {
                    let top = c.rect.y - 56. / zz;
                    if self.night.map.pan.1 > top {
                        self.night.map.pan.1 = top;
                    }
                }
                if centre {
                    self.night.map.clamp(&m, view_w, map_h);
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
        // Dev-only: the pointer resting on the map, for a screenshot; placed
        // again on every render, so it rests where it was asked to once the
        // window has its size.
        if let Some(mut at) = self.night.dev_hover {
            // `badge`: on the selected card's biggest badge drawn.
            if at.0.is_nan() && at.1 == -1. {
                let level = self.night.map.level();
                let z = self.night.map.zoom;
                let pan = self.night.map.pan;
                let focus = self
                    .brain_selected(&m)
                    .map(|i| m.same_item(Item::Note(i), level));
                at = (f32::NAN, f32::NAN);
                if let Some(c) = m
                    .layout(level)
                    .badges(focus)
                    .iter()
                    .filter_map(|b| Some(((b.lines, b.links), m.style.badge_at(b, z)?)))
                    .max_by_key(|b| b.0)
                    .map(|b| b.1)
                {
                    at = ((c.0 - pan.0) * z, (c.1 - pan.1) * z);
                }
            }
            // `near`: on a card the selected note links to.
            if at.0.is_nan() && !at.1.is_nan() {
                let level = self.night.map.level();
                if let Some(i) = self.brain_selected(&m) {
                    let sel = m.same_item(Item::Note(i), level);
                    let lay = m.layout(level);
                    let z = self.night.map.zoom;
                    let pan = self.night.map.pan;
                    let in_view = |c: &&map::Card| {
                        let (x, y) = ((c.rect.x - pan.0) * z, (c.rect.y - pan.1) * z);
                        x > 20. && y > 40. && x + 60. < view_w && y + 60. < map_h
                    };
                    // A card it links to, else the nearest other card in view.
                    let linked = lay
                        .shown(Some(sel))
                        .iter()
                        .filter_map(|l| lay.card(l.to))
                        .find(|c| in_view(c));
                    let near = || {
                        let sc = lay.card(sel)?.rect;
                        lay.cards
                            .iter()
                            .filter(|c| c.item != sel)
                            .filter(in_view)
                            .min_by(|a, b| {
                                let d = |r: &map::Rect| (r.x - sc.x).abs() + (r.y - sc.y).abs();
                                d(&a.rect).total_cmp(&d(&b.rect))
                            })
                    };
                    if let Some(c) = linked.or_else(near) {
                        at = (
                            (c.rect.x + c.rect.w / 2. - pan.0) * z,
                            (c.rect.y + c.rect.h.min(112.) / 2. - pan.1) * z,
                        );
                    }
                }
            }
            // `line`: on the middle of the focus's longest vertical run.
            if at.0.is_nan() {
                let level = self.night.map.level();
                let focus = self
                    .brain_selected(&m)
                    .map(|i| m.same_item(Item::Note(i), level));
                let best = m
                    .layout(level)
                    .shown(focus)
                    .iter()
                    .flat_map(|l| {
                        l.points
                            .windows(2)
                            .map(|w| (w[0], w[1]))
                            .collect::<Vec<_>>()
                    })
                    .filter(|(a, b)| a.0 == b.0)
                    .max_by(|x, y| (x.0.1 - x.1.1).abs().total_cmp(&(y.0.1 - y.1.1).abs()));
                if let Some((a, b)) = best {
                    let z = self.night.map.zoom;
                    let pan = self.night.map.pan;
                    at = ((a.0 - pan.0) * z, ((a.1 + b.1) / 2. - pan.1) * z);
                }
            }
            self.brain_hover(at);
        }
        let selected = self.brain_selected(&m);
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
            camera: self.night.map.camera,
        };
        let canvas_el = canvas(
            |_, _, _| {},
            move |b, _, window, cx| paint_map(&paint, b, window, cx),
        )
        .size_full();
        let pct = format!("{}%", (self.night.map.zoom * 100.).round());
        // The map's controls sit in the header, never over the map: Reset
        // layout (while the layout is a person's) with its Undo line, the
        // zoom, and Treemap.
        let toast = self.night.map.toast.clone().map(|(_, words, undo)| {
            div()
                .id("brain-toast")
                .flex()
                .items_center()
                .gap(look.px(8.))
                .text_size(look.text(13.))
                .text_color(t.muted)
                .child(words)
                .children(undo.map(|_| {
                    kit::button(
                        look,
                        t,
                        "brain-undo",
                        Some("undo-2"),
                        "Undo",
                        Btn::Quiet,
                        true,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.brain_undo_reset(cx)))
                }))
        });
        // A line under the pointer at Out: its cards and links.
        let line_words = self.night.map.hover_line.as_ref().and_then(|(ends, _)| {
            let first = ends.first()?;
            let level = self.night.map.level();
            let focus = self.brain_line_set(&m);
            let lay = m.layout(level);
            let hit: Vec<&map::Line> = lay
                .shown(focus)
                .iter()
                .filter(|l| ends.contains(&l.hot()))
                .collect();
            // One number for the badge and the status: the links of the
            // cards these lines reach.
            let links = Layout::links_of(&hit);
            let to = format!(
                "{} · {}",
                names(hit.iter().map(|l| block_name(&m, l.to)).collect()),
                hippo_view::night::count(links, "link", "links")
            );
            Some(
                div()
                    .id("brain-line")
                    .flex()
                    .items_center()
                    .gap(look.px(6.))
                    .text_size(look.text(13.))
                    .text_color(t.text)
                    .child(div().truncate().child(block_name(&m, first.0)))
                    // Lines in point at the card in focus.
                    .child(kit::icon(
                        look,
                        if hit.iter().all(|l| !l.out) {
                            "left"
                        } else {
                            "right"
                        },
                        13.,
                        t.muted,
                    ))
                    .child(div().truncate().child(to)),
            )
        });
        let zoom = div()
            .flex()
            .items_center()
            // Figures that change in place keep one width.
            .font_features(gpui::FontFeatures(std::sync::Arc::new(vec![(
                "tnum".into(),
                1,
            )])))
            .gap(look.px(2.))
            .p(look.px(3.))
            .rounded(look.px(10.))
            .border(look.fixed(1.))
            .border_color(t.line)
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
                .on_click(cx.listener(|this, _, _, cx| this.brain_zoom_step(-1, cx))),
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
                .on_click(cx.listener(|this, _, _, cx| this.brain_zoom_step(1, cx))),
            );
        let head = div()
            .flex()
            .flex_col()
            .gap(look.px(6.))
            .h(look.px(HEAD))
            .px(look.px(super::morning::PAD))
            .pt(look.px(14.))
            .bg(t.canvas)
            .border_b_1()
            .border_color(t.line)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(10.))
                    .h(look.px(32.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(look.text(13.5))
                            .text_color(t.muted)
                            .child(m.kicker.clone()),
                    )
                    // Kept in its place while hidden, so nothing moves when
                    // it shows.
                    .child(
                        kit::button(
                            look,
                            t,
                            "brain-reset",
                            Some("undo-2"),
                            "Reset layout",
                            Btn::Quiet,
                            true,
                        )
                        .when(!m.is_arranged(), |d| d.invisible())
                        .tooltip(crate::ui::tooltip::Tooltip::text(
                            "Back to the layout hippo chooses",
                        ))
                        .on_click(cx.listener(|this, _, _, cx| {
                            if this.night.map.map.as_ref().is_some_and(|m| m.is_arranged()) {
                                this.brain_reset_layout(cx)
                            }
                        })),
                    )
                    .child(zoom)
                    .child(
                        kit::button(
                            look,
                            t,
                            "brain-treemap",
                            Some("layout-grid"),
                            "Treemap",
                            Btn::Quiet,
                            true,
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.select_section(crate::state::Section::Memory, window, cx)
                        })),
                    ),
            )
            // The headline, and a slot of its own at its right for what just
            // happened and what the pointer is on: "Runbooks moved. Undo",
            // or a line's links. The lead below keeps its whole width.
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(16.))
                    .h(look.px(34.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(look.text(26.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(t.text)
                            .child(m.headline.clone()),
                    )
                    .child(
                        div()
                            .id("brain-status")
                            .flex_shrink_0()
                            // Ends where the toolbar's last word does, not
                            // at its button's edge.
                            .pr(look.px(8.))
                            .max_w(look.px((view_w * 0.5).min(460.)))
                            .flex()
                            .justify_end()
                            .children(toast.or(line_words)),
                    ),
            )
            .child(
                div()
                    .h(look.px(24.))
                    .text_size(look.text(15.))
                    .text_color(t.muted)
                    .truncate()
                    .child(m.lead.clone()),
            );
        let mapdiv = div()
            .id("brain-map")
            .debug_selector(|| "brain-map".to_owned())
            .absolute()
            .top(look.px(HEAD))
            .left_0()
            .w(look.px(canvas_w))
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
        div()
            .relative()
            .w(look.px(view_w))
            .h(look.px(view_h))
            .child(head)
            .child(mapdiv)
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
        if self.night.map.lift.is_none() {
            self.night.map.lift_fitted = self.night.map.at_fit();
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
        let before = m.arrangement.clone();
        let moved = arr != before;
        self.brain_set_arrangement(Some(arr), cx);
        // A move is undone, not confirmed.
        if moved {
            let name = match l.block {
                Block::Folder(f) => Item::Folder(f),
                Block::Note(i) => Item::Note(i),
            };
            self.brain_toast(format!("{} moved.", block_name(&m, name)), before, cx);
        }
        // A drop from the fitted view fits the map as it now is, moving
        // there briefly (the zoom label follows); any other keeps its zoom
        // and pan exactly. Never a recentring on the selection.
        if let (Some(m2), Some((w, h))) = (self.night.map.map.clone(), self.night.map.view_size) {
            if self.night.map.lift_fitted {
                let from = Camera {
                    zoom: self.night.map.zoom,
                    pan: self.night.map.pan,
                    start: Instant::now(),
                    ms: motion_ms(SETTLE_MS),
                };
                self.night.map.fit(&m2, w, h);
                self.night.map.camera = (from.ms > 0.).then_some(from);
            } else {
                // The smallest zoom is still the new map's fit, so zooming
                // out (or ⌘0) always reaches the whole map.
                let (fz, fp) = m2.fit(w, h);
                self.night.map.fit_zoom = fz;
                self.night.map.fit_pan = fp;
            }
        }
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
        self.brain_toast("Layout reset to the default.".into(), before, cx);
    }

    /// A line in the header with Undo for 6 seconds, which puts `before`
    /// back.
    fn brain_toast(&mut self, words: String, before: Arrangement, cx: &mut Context<Self>) {
        self.night.map.toast_seq += 1;
        let seq = self.night.map.toast_seq;
        self.night.map.toast = Some((seq, words, Some(before)));
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

    /// One step in (`dir` > 0) or out of the zoom table the map's style
    /// holds, the one the web demo steps through.
    pub(crate) fn brain_zoom_step(&mut self, dir: i32, cx: &mut Context<Self>) {
        let Some(m) = self.night.map.map.clone() else {
            return;
        };
        let unit = f32::from(Look::from_app(cx).px(1.));
        let (w, h) = self
            .night
            .map
            .view_size
            .unwrap_or_else(|| self.brain_view_size(unit));
        // About the selected card when it is in view, so it stays put; else
        // about the view's middle.
        let z0 = self.night.map.zoom;
        let level = self.night.map.level();
        let pan = self.night.map.pan;
        let at = self
            .brain_selected(&m)
            .map(|i| m.same_item(Item::Note(i), level))
            .and_then(|it| m.layout(level).card(it))
            .map(|c| {
                (
                    (c.rect.x + c.rect.w / 2. - pan.0) * z0,
                    (c.rect.y + c.rect.h.min(112.) / 2. - pan.1) * z0,
                )
            })
            .filter(|(x, y)| *x > 0. && *y > 0. && *x < w && *y < h)
            .unwrap_or((w / 2., h / 2.));
        let z = m.style.step(z0, dir, self.night.map.min_zoom());
        self.night.map.zoom_at(&m, z, at);
        // The selected card's top stays clear of the strip of folder names.
        let level = self.night.map.level();
        if level != Level::Out
            && let Some(c) = self
                .brain_selected(&m)
                .map(|i| m.same_item(Item::Note(i), level))
                .and_then(|it| m.layout(level).card(it))
        {
            let top = c.rect.y - 46. / z;
            let (_, pan1) = self.night.map.pan;
            if pan1 > top && (c.rect.y - pan1) * z < h {
                self.night.map.pan.1 = top;
            }
        }
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
            let f = Rc::new(m.fitted(w, h));
            self.night.map.map = Some(f.clone());
            self.night.map.slide = None;
            f
        };
        self.night.map.view_size = Some((w, h));
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
        let mut ends: Vec<map::Hot> = Vec::new();
        if card.is_none() {
            let focus = self.brain_line_set(&m);
            let z = self.night.map.zoom;
            // What the status counts is what the map says is here: a badge
            // stands for its trunk, a run for the lines along it.
            ends = lay
                .lines_at(focus, mx, my, 6. / z, z)
                .iter()
                .map(|l| l.hot())
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

    /// The card whose lines the pointer is among.
    fn brain_line_set(&self, m: &NotesMap) -> Option<Item> {
        let level = self.night.map.level();
        self.night.map.hover.or_else(|| {
            self.brain_selected(m)
                .map(|i| m.same_item(Item::Note(i), level))
        })
    }

    fn brain_item_at(&self, at: (f32, f32)) -> Option<Item> {
        let m = self.night.map.map.as_ref()?;
        let (mx, my) = self.night.map.to_map(at);
        m.layout(self.night.map.level())
            .card_at(mx, my)
            .map(|c| c.item)
    }

    fn brain_click(&mut self, at: (f32, f32), cx: &mut Context<Self>) {
        // A line to a card out of view: pan to the nearest such card,
        // keeping the zoom.
        if self.brain_item_at(at).is_none()
            && let Some(m) = self.night.map.map.clone()
        {
            let unit = f32::from(Look::from_app(cx).px(1.));
            let (w, h) = self.brain_view_size(unit);
            let (z, pan) = (self.night.map.zoom, self.night.map.pan);
            let level = self.night.map.level();
            let lay = m.layout(level);
            let (mx, my) = self.night.map.to_map(at);
            let view = map::Rect::new(pan.0, pan.1, w / z, h / z);
            let under = lay.lines_at(self.brain_line_set(&m), mx, my, 6. / z, z);
            if let Some(item) =
                map::nearest_out(&under, &view, (mx, my), |i| lay.card(i).map(|c| c.rect))
                && let Some(c) = lay.card(item)
            {
                self.night.map.pan = (
                    c.rect.x + c.rect.w / 2. - w / 2. / z,
                    c.rect.y + c.rect.h.min(map::CARD_H * 2.) / 2. - h / 2. / z,
                );
                self.night.map.camera = None;
                // Centred, but never past the map's edges.
                self.night.map.clamp(&m, w, h);
                cx.notify();
                return;
            }
        }
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
                .p(look.px(10.))
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
                    // One line: a label that wraps reads as two.
                    div()
                        .text_size(look.text(12.5))
                        .text_color(t.faint)
                        .truncate()
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
                    // The note's folder, beside a folder.
                    .child(kit::tile(look, t, "folder", Tone::Plain))
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
                    .child(stat(p.reads_week.to_string(), "weekly reads"))
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
                    .border(look.fixed(1.))
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
