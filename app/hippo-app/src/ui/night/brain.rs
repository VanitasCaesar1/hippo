//! Brain: the notes as a map that zooms.
//!
//! GPUI cannot scale a subtree, so the map is painted on one canvas from
//! the layout `hippo_view::night::map` computes: every rectangle, line,
//! corner and text size is the layout's number times the zoom. Three levels
//! (folders, note cards, cards with their rules) each have their own
//! layout; crossing a level keeps the card under the pointer under the
//! pointer. Only what meets the view is drawn. Below 900 points the map
//! becomes a board of cards.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Instant;

use gpui::{
    AnyElement, Bounds, Context, Font, FontWeight, Hsla, Pixels, SharedString, TextRun, Window,
    canvas, div, point, prelude::*, px, size,
};
use hippo_view::night::map::{
    self, CARD_H, FRAME_HEAD, FRAME_PAD, IN_LINE, Item, Layout, Level, MapNote, NotesMap,
};
use hippo_view::night::note::NotePanel;

use super::kit::{self, Btn, Nt, Tone};
use crate::ui::Shell;
use crate::ui::theme::Look;

/// The header's height above the map, in points.
pub const HEAD: f32 = 118.;
/// Zoom steps for the buttons and keys.
const STEP: f32 = 1.25;

/// Brain's pan, zoom and drag.
pub struct MapView {
    pub zoom: f32,
    /// The map point at the view's top left.
    pub pan: (f32, f32),
    /// Fitted to the view once.
    pub fitted: bool,
    /// The press that may become a drag.
    pub press: Option<Press>,
    /// The view's origin and size in window pixels, from the last paint.
    pub view: Rc<Cell<(f32, f32, f32, f32)>>,
    /// Paint times and frame intervals, for `HIPPO_APP_TIMING`.
    pub frames: Rc<RefCell<Frames>>,
    /// The map in use (cloned once per reload).
    pub map: Option<Rc<NotesMap>>,
    /// The panel of the selected note, read when the selection changes.
    pub panel: Option<(PathBuf, Option<NotePanel>)>,
}

impl Default for MapView {
    fn default() -> MapView {
        MapView {
            zoom: 1.,
            pan: (-24., -8.),
            fitted: false,
            press: None,
            view: Rc::new(Cell::new((0., 0., 1., 1.))),
            frames: Rc::new(RefCell::new(Frames::default())),
            map: None,
            panel: None,
        }
    }
}

/// A press on the map: where (view points), the pan then, and whether it
/// has moved far enough to be a drag.
pub type Press = ((f32, f32), (f32, f32), bool);

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
        let (z, pan) = map.mid.fit(w, h, 28.);
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

fn paint_map(p: &Paint, b: Bounds<Pixels>, window: &mut Window, cx: &mut gpui::App) {
    let started = Instant::now();
    let (ox, oy) = (f32::from(b.origin.x), f32::from(b.origin.y));
    let (vw, vh) = (f32::from(b.size.width), f32::from(b.size.height));
    p.view.set((ox, oy, vw, vh));
    let z = p.zoom;
    let u = p.unit;
    let s = |mx: f32, my: f32| (ox + (mx - p.pan.0) * z * u, oy + (my - p.pan.1) * z * u);
    let level = Level::of(z);
    let lay: &Layout = p.map.layout(level);
    let view = map::Rect::new(p.pan.0, p.pan.1, vw / (z * u), vh / (z * u));
    let t = &p.t;

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

    // Folder frames.
    for (fi, r) in &lay.frames {
        if !r.meets(&view) {
            continue;
        }
        let (x, y) = s(r.x, r.y);
        let (w, h) = (r.w * z * u, r.h * z * u);
        rrect(
            window,
            Bounds::new(point(px(x), px(y)), size(px(w), px(h))),
            16. * z * u,
            gpui::transparent_black(),
            t.line2,
            (1.2 * u).max(1.),
            true,
        );
        let f = &p.map.folders[*fi];
        let name = if f.name.is_empty() { "Notes" } else { &f.name };
        let size_px = 14.5 * z * u;
        text(
            window,
            cx,
            name,
            x + FRAME_PAD * z * u,
            y + 13. * z * u,
            size_px,
            t.muted,
            &font(&p.family, FontWeight::SEMIBOLD),
            w * 0.6,
        );
        let count = hippo_view::night::count(f.notes.len(), "note", "notes");
        let cw = count.chars().count() as f32 * 13. * z * u * 0.55;
        text(
            window,
            cx,
            &count,
            x + w - FRAME_PAD * z * u - cw,
            y + 14. * z * u,
            13. * z * u,
            t.faint,
            &font(&p.family, FontWeight::NORMAL),
            cw + 8.,
        );
    }

    // Lines, the selected note's on top.
    let mut drawn_lines = 0;
    let lit = |i: &Item| matches!(i, Item::Note(n) if Some(*n) == p.selected);
    let mut order: Vec<&map::Line> = lay.lines_in(&view).collect();
    order.sort_by_key(|l| lit(&l.from) || lit(&l.to));
    for l in order {
        let on = lit(&l.from) || lit(&l.to);
        let color = if on {
            t.night
        } else if p.selected.is_some() {
            t.line2.opacity(0.55)
        } else {
            t.line2
        };
        let width = if on { 2. } else { 1.4 } * u * z.clamp(0.6, 1.4);
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
        let r = 3. * u * z.clamp(0.6, 1.4);
        for end in [l.points[0], *l.points.last().expect("a line has points")] {
            let (x, y) = s(end.0, end.1);
            window.paint_quad(gpui::quad(
                Bounds::new(point(px(x - r), px(y - r)), size(px(2. * r), px(2. * r))),
                px(r),
                t.canvas,
                px(1.5 * u),
                color,
                gpui::BorderStyle::Solid,
            ));
        }
        drawn_lines += 1;
    }

    // Cards.
    let mut drawn = 0;
    for c in lay.cards_in(&view) {
        let (x, y) = s(c.rect.x, c.rect.y);
        let (w, h) = (c.rect.w * z * u, c.rect.h * z * u);
        let bounds = Bounds::new(point(px(x), px(y)), size(px(w), px(h)));
        drawn += 1;
        match c.item {
            Item::Folder(fi) => paint_folder(p, &p.map.folders[fi], bounds, window, cx),
            Item::Note(i) => {
                let n = &p.map.notes[i];
                let sel = p.selected == Some(i);
                let near = p.neighbours.contains(&i);
                paint_note(p, n, sel, near, level == Level::In, bounds, window, cx);
            }
        }
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

fn paint_folder(
    p: &Paint,
    f: &map::Folder,
    b: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    let t = &p.t;
    let z = p.zoom;
    let u = p.unit * z;
    rrect(
        window,
        b,
        18. * u,
        t.surface,
        t.line,
        (1. * p.unit).max(1.),
        false,
    );
    let (x, y, w) = (
        f32::from(b.origin.x),
        f32::from(b.origin.y),
        f32::from(b.size.width),
    );
    let pad = 32. * u;
    let name = if f.name.is_empty() { "Notes" } else { &f.name };
    text(
        window,
        cx,
        name,
        x + pad,
        y + pad,
        72. * u,
        t.text,
        &font(&p.family, FontWeight::BOLD),
        w - 2. * pad,
    );
    text(
        window,
        cx,
        &hippo_view::night::count(f.notes.len(), "note", "notes"),
        x + pad,
        y + pad + 96. * u,
        46. * u,
        t.muted,
        &font(&p.family, FontWeight::NORMAL),
        w - 2. * pad,
    );
    let mut cy = y + pad + 170. * u;
    let chips: [(usize, &str, &str, Tone); 4] = [
        (f.fixed, "fixed last night", "fixed last night", Tone::Ok),
        (f.waiting, "rule waiting", "rules waiting", Tone::You),
        (f.rejected, "fix rejected", "fixes rejected", Tone::Bad),
        (f.stale, "stale", "stale", Tone::Plain),
    ];
    for (n, one, many, tone) in chips {
        if n == 0 {
            continue;
        }
        let words = format!("{n} {}", if n == 1 { one } else { many });
        // Chips at Out are drawn 3.2 times the note cards' size.
        chip(
            window,
            cx,
            p,
            &words,
            tone,
            x + pad,
            cy,
            z * 3.2,
            w - 2. * pad,
        );
        cy += 84. * u;
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_note(
    p: &Paint,
    n: &MapNote,
    sel: bool,
    near: bool,
    inner: bool,
    b: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    let t = &p.t;
    let z = p.zoom;
    let u = p.unit * z;
    let border = if sel {
        t.night
    } else if near {
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
    // Too small to read: the title as a bar.
    if 15. * u < 4. {
        window.paint_quad(gpui::fill(
            Bounds::new(
                point(px(x + pad), px(y + pad)),
                size(px((w - 2. * pad) * 0.6), px((6. * u).max(1.))),
            ),
            t.faint.opacity(0.6),
        ));
        return;
    }
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
    text(
        window,
        cx,
        &n.title,
        x + pad + icon + 10. * u,
        y + pad,
        16. * u,
        t.text,
        &font(&p.family, FontWeight::SEMIBOLD),
        w - 2. * pad - icon - 10. * u,
    );
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
                    t.you,
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
    let _ = FRAME_HEAD;
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
            self.night.map.map = Some(Rc::new(loaded.map.clone()));
            self.night.map.panel = None;
        }
        self.night.map.map.clone()
    }

    /// The selected note: the chosen one, else the first with a rule
    /// waiting, else the entry note.
    fn brain_selected(&self, m: &NotesMap) -> Option<usize> {
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
            if let Some(z) = self.night.dev_zoom.take() {
                self.night.map.fit(&m, view_w, map_h);
                let at = (view_w / 2., map_h / 2.);
                let z0 = self.night.map.zoom;
                self.night
                    .map
                    .zoom_at(&m, z.max(z0 * 0.0 + map::MIN_ZOOM), at);
                if let Some(c) = m.mid.card(Item::Note(self.brain_selected(&m).unwrap_or(0))) {
                    let r = c.rect;
                    let zz = self.night.map.zoom;
                    let lay = m.layout(Level::of(zz));
                    let item = m.same_item(
                        Item::Note(self.brain_selected(&m).unwrap_or(0)),
                        Level::of(zz),
                    );
                    let r = lay.card(item).map_or(r, |c| c.rect);
                    self.night.map.pan = (r.x - 40. / zz, r.y - 30. / zz);
                }
            } else {
                self.night.map.fit(&m, view_w, map_h);
                // A small map opens with its note cards, from its top left;
                // a large one opens fitted, as folders.
                if m.notes.len() <= 80 && self.night.map.zoom < map::OUT_BELOW + 0.05 {
                    let z = map::OUT_BELOW + 0.1;
                    self.night.map.zoom = z;
                    self.night.map.pan = (-24. / z, -12. / z);
                }
            }
        }
        let selected = self.brain_selected(&m);
        let neighbours: Vec<usize> = selected
            .map(|i| m.neighbours(i).into_iter().collect())
            .unwrap_or_default();
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
            .child(
                kit::button(look, t, "zoom-out", None, "−", Btn::Quiet, true)
                    .on_click(cx.listener(|this, _, _, cx| this.brain_zoom_step(1. / STEP, cx))),
            )
            .child(
                kit::button(look, t, "zoom-fit", None, pct, Btn::Quiet, true)
                    .on_click(cx.listener(|this, _, _, cx| this.brain_fit(cx))),
            )
            .child(
                kit::button(look, t, "zoom-in", None, "+", Btn::Quiet, true)
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
                        return;
                    }
                    this.night.map.press = Some((at, this.night.map.pan, false));
                }),
            )
            .on_mouse_move(cx.listener(move |this, e: &gpui::MouseMoveEvent, _, cx| {
                let Some((start, pan, _)) = this.night.map.press else {
                    return;
                };
                if e.pressed_button != Some(gpui::MouseButton::Left) {
                    this.night.map.press = None;
                    return;
                }
                let at = this.brain_view_point(e.position, unit);
                let (dx, dy) = (at.0 - start.0, at.1 - start.1);
                if dx.abs() + dy.abs() > 3. {
                    let z = this.night.map.zoom;
                    this.night.map.pan = (pan.0 - dx / z, pan.1 - dy / z);
                    this.night.map.press = Some((start, pan, true));
                    cx.notify();
                }
            }))
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(move |this, e: &gpui::MouseUpEvent, _, cx| {
                    let Some((_, _, moved)) = this.night.map.press.take() else {
                        return;
                    };
                    if !moved {
                        let at = this.brain_view_point(e.position, unit);
                        this.brain_click(at, cx);
                    }
                }),
            );
        div()
            .relative()
            .w(look.px(view_w))
            .h(look.px(view_h))
            .child(head)
            .child(mapdiv)
            .child(controls)
            .into_any_element()
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
        self.night.map.fit(&m, w, h);
        cx.notify();
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
                        .font_family(kit::MONO)
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
                                    .children(e.agent.as_ref().map(|a| kit::badge(look, a)))
                                    .child(
                                        div()
                                            .font_family(kit::MONO)
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
                            .child(kit::badge(look, a))
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
        let pinned: Vec<usize> = (0..m.notes.len()).filter(|i| m.pinned(*i)).collect();
        for i in pinned {
            col = col.child(card(self, i, cx));
        }
        for f in &m.folders {
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
