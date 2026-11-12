//! Pulse: Fleet's default view (docs/designs/fleet-pulse.md).
//!
//! One screen: the header's counts and a histogram of the fleet's tool
//! calls stacked by account up to a now line; the sessions that need the
//! person (one large card when exactly one does) and a ranked Next up
//! list; working sessions as waveforms of their 2-minute activity with
//! their current call; to review, quiet and settled ones compact; and a
//! rail of each account's usage drain, memory today and tonight's pass.
//!
//! Today's lanes come from `hippo_view::replay` and the rest from
//! `hippo_view::pulse`, read on the background executor every 30
//! seconds while Pulse shows. The tracks, the histogram and the drain
//! charts are painted on canvases; the time grid and the now line are
//! one overlay across the lanes. Only a needs-you lane's dot pulses, and
//! not when the system asks for reduced motion.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::AnimationExt as _;
use gpui::{
    Bounds, Context, FontWeight, Hsla, PathBuilder, Pixels, SharedString, Window, canvas, div,
    fill, point, prelude::*, px, size,
};
use hippo_core::incidents::Incident;
use hippo_view::fleet::{Fleet, Row, Shelf, State};
use hippo_view::pulse::{self, Corrected, Fate, Next, Tonight};
use hippo_view::replay::{self, Day, Live, MarkKind};
use time::{Duration, OffsetDateTime};

use super::Shell;
use super::kit::{self, Kind};
use super::theme::Look;

/// How often today's lanes are read again while Pulse shows.
const REFRESH: Duration = Duration::seconds(30);
/// From this content width the rail sits beside the lanes; below, its
/// accounts are a strip on top.
const RAIL_BESIDE: f32 = 1120.;
const RAIL_W: f32 = 272.;
/// The least room the tracks need; below it they are left out.
const TRACKS_MIN: f32 = 150.;
/// A lane's track height.
const TRACK_H: f32 = 24.;
const SUB_H: f32 = 16.;
const MINI_H: f32 = 12.;
/// Next up shows this many until asked for more.
const NEXT_SHOWN: usize = 4;
/// At most this many finished sessions are listed to review.
const REVIEW_SHOWN: usize = 8;
/// At most this many sub-agents are drawn under a lane.
const SUBS_SHOWN: usize = 4;

/// The span the lanes show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Span {
    Hour,
    #[default]
    Three,
    Day,
}

impl Span {
    fn label(self) -> &'static str {
        match self {
            Span::Hour => "1h",
            Span::Three => "3h",
            Span::Day => "Day",
        }
    }
}

/// What Pulse reads besides the fleet.
pub struct PulseData {
    pub day: Rc<Day>,
    pub incidents: Vec<Incident>,
    pub corrected: Vec<Corrected>,
    pub tonight: Option<Tonight>,
    pub read_at: OffsetDateTime,
}

/// Pulse's state.
pub struct PulseUi {
    /// Fleet shows Pulse (the default) rather than Rows.
    pub on: bool,
    pub span: Span,
    pub data: Option<Rc<PulseData>>,
    pub loading: bool,
    pub error: Option<String>,
    /// Waiting sessions put off with Later, shown after the others.
    pub later: Vec<String>,
    /// Next up's selection (J K); `None` until one is chosen.
    pub next_sel: Option<usize>,
    /// Next up shows all, not the first [`NEXT_SHOWN`].
    pub next_all: bool,
    /// The content width as last drawn.
    pub width: Rc<Cell<f32>>,
    /// The system asks for reduced motion (read once).
    pub still: Option<bool>,
}

impl Default for PulseUi {
    fn default() -> Self {
        PulseUi {
            on: true,
            span: Span::default(),
            data: None,
            loading: false,
            error: None,
            later: Vec::new(),
            next_sel: None,
            next_all: false,
            width: Rc::new(Cell::new(0.)),
            still: None,
        }
    }
}

/// Whether the system's Reduce motion is on.
fn reduce_motion() -> bool {
    std::process::Command::new("/usr/bin/defaults")
        .args(["read", "com.apple.universalaccess", "reduceMotion"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "1")
        .unwrap_or(false)
}

/// The yellow of memory's marks: a kept rule served, a correction;
/// darker on a light theme so it reads.
fn memory_yellow(look: &Look) -> Hsla {
    if look.c.dark {
        gpui::hsla(45. / 360., 0.92, 0.58, 1.)
    } else {
        gpui::hsla(40. / 360., 0.95, 0.40, 1.)
    }
}

/// The column widths for a content width.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Geo {
    pub rail: bool,
    pub head: f32,
    pub now: f32,
    pub tracks: bool,
    pub pad: f32,
    /// The tracks' width.
    pub track: f32,
}

/// The layout for content width `w`: the rail beside from
/// [`RAIL_BESIDE`], the head and now columns about a quarter each, and
/// the tracks only when they keep [`TRACKS_MIN`].
pub fn geo(w: f32) -> Geo {
    let rail = w >= RAIL_BESIDE;
    let pad = 16.;
    let main = w - pad * 2. - if rail { RAIL_W + 16. } else { 0. };
    let head = (main * 0.28).clamp(200., 290.);
    let now = (main * 0.32).clamp(220., 330.);
    let tracks = main - head - now >= TRACKS_MIN;
    let (head, now) = if tracks {
        (head, now)
    } else {
        // Without tracks the two columns share the row.
        ((main * 0.55).max(160.), (main * 0.45).max(150.))
    };
    Geo {
        rail,
        head,
        now,
        tracks,
        pad,
        track: (main - head - now).max(0.),
    }
}

/// Ruler steps from `span_secs` over `width` pixels: labels at least 64
/// apart, on 15-minute, half-hour, hour or 2-hour marks.
pub fn tick_step(span_secs: i64, width: f32) -> i64 {
    for step in [5 * 60, 15 * 60, 30 * 60, 3600, 2 * 3600, 3 * 3600, 6 * 3600] {
        let n = span_secs as f32 / step as f32;
        if n <= 0. || width / n >= 64. {
            return step;
        }
    }
    6 * 3600
}

/// A beat value as a bar's share of its half height.
fn bar_frac(v: u32) -> f32 {
    ((v.min(12) as f32) / 12.).sqrt()
}

/// `+1,204`.
pub fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// What a lane draws: its beats, the waiting ones flagged, and its marks
/// as `(time, kind)`.
#[derive(Clone)]
struct Wave {
    beats: Vec<u16>,
    waiting: Vec<bool>,
    /// The time of beat 0.
    from: OffsetDateTime,
    marks: Vec<(OffsetDateTime, MarkKind)>,
    /// A kept rule served at its start.
    ring: Option<OffsetDateTime>,
    color: Hsla,
    /// Its end dot: live blue, needs orange, merged green.
    end: Option<Hsla>,
}

/// Paints a wave in `bounds` for the time from `t0` to `t1`.
fn paint_wave(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    w: &Wave,
    t0: OffsetDateTime,
    t1: OffsetDateTime,
    lk: &Look,
    bars: bool,
) {
    let o = bounds.origin;
    let width = f32::from(bounds.size.width);
    let h = f32::from(bounds.size.height);
    let span = (t1 - t0).as_seconds_f32().max(1.);
    let x = |t: OffsetDateTime| ((t - t0).as_seconds_f32() / span * width).clamp(0., width);
    let mid = h / 2.;
    let beat_w = replay::BEAT.as_seconds_f32() / span * width;
    // Merge beats so a bar is at least 2.5 pixels wide.
    let bin = ((2.5 / beat_w).ceil() as usize).max(1);
    let first = w.beats.iter().position(|v| *v > 0);
    let last_t = w.from + replay::BEAT * w.beats.len() as i32;
    // The thread: a hairline from the first activity to its end.
    if let Some(f) = first {
        let a = x(w.from + replay::BEAT * f as i32);
        let b = if w.end.is_some() { width } else { x(last_t) };
        if b > a {
            window.paint_quad(fill(
                Bounds::new(point(o.x + px(a), o.y + px(mid)), size(px(b - a), px(1.))),
                w.color.opacity(0.35),
            ));
        }
    }
    let mut k = 0;
    while k < w.beats.len() {
        let end = (k + bin).min(w.beats.len());
        let v = w.beats[k..end].iter().copied().max().unwrap_or(0);
        let waiting = w.waiting[k..end.min(w.waiting.len())].iter().any(|x| *x);
        let t = w.from + replay::BEAT * k as i32;
        let a = x(t);
        let bw = (beat_w * (end - k) as f32 - 1.).max(1.5);
        if a < width && a + bw > 0. {
            if waiting && v == 0 {
                window.paint_quad(fill(
                    Bounds::new(
                        point(o.x + px(a), o.y + px(mid - 1.)),
                        size(px(bw + 1.), px(2.)),
                    ),
                    lk.c.warn,
                ));
            } else if v > 0 {
                let hh = if bars {
                    (bar_frac(u32::from(v)) * (mid - 1.)).max(1.5)
                } else {
                    (bar_frac(u32::from(v)) * (mid - 1.)).max(1.)
                };
                window.paint_quad(fill(
                    Bounds::new(
                        point(o.x + px(a), o.y + px(mid - hh)),
                        size(px(bw.min(width - a)), px(hh * 2.)),
                    ),
                    w.color.opacity(if bars { 0.92 } else { 0.7 }),
                ));
            }
        }
        k = end;
    }
    // Marks above the thread.
    let r = (h * 0.16).clamp(2.5, 4.5);
    let top = (mid - h * 0.42).max(r + 0.5);
    if let Some(t) = w.ring
        && t >= t0
    {
        let c = point(o.x + px(x(t)), o.y + px(top));
        let mut b = PathBuilder::stroke(px(1.5));
        b.move_to(point(c.x + px(r), c.y));
        b.arc_to(
            point(px(r), px(r)),
            px(0.),
            false,
            true,
            point(c.x - px(r), c.y),
        );
        b.arc_to(
            point(px(r), px(r)),
            px(0.),
            false,
            true,
            point(c.x + px(r), c.y),
        );
        if let Ok(p) = b.build() {
            window.paint_path(p, memory_yellow(lk));
        }
    }
    for (t, kind) in &w.marks {
        if *t < t0 || *t > t1 {
            continue;
        }
        let cx = o.x + px(x(*t));
        let cy = o.y + px(top);
        match kind {
            MarkKind::Correction => {
                let d = r + 1.;
                let mut b = PathBuilder::fill();
                b.move_to(point(cx, cy - px(d)));
                b.line_to(point(cx + px(d), cy));
                b.line_to(point(cx, cy + px(d)));
                b.line_to(point(cx - px(d), cy));
                b.close();
                if let Ok(p) = b.build() {
                    window.paint_path(p, memory_yellow(lk));
                }
            }
            MarkKind::Destructive => window.paint_quad(
                fill(
                    Bounds::new(point(cx - px(r), cy - px(r)), size(px(r * 2.), px(r * 2.))),
                    lk.c.err,
                )
                .corner_radii(px(1.)),
            ),
            MarkKind::PullMerged => window.paint_quad(
                fill(
                    Bounds::new(point(cx - px(r), cy - px(r)), size(px(r * 2.), px(r * 2.))),
                    lk.c.created,
                )
                .corner_radii(px(r)),
            ),
            _ => {}
        }
    }
    if let Some(c) = w.end {
        let d = 3.5;
        window.paint_quad(
            fill(
                Bounds::new(
                    point(o.x + px(width - d), o.y + px(mid - d)),
                    size(px(d * 2.), px(d * 2.)),
                ),
                c,
            )
            .corner_radii(px(d)),
        );
    }
}

impl Shell {
    fn pulse_today(&self) -> time::Date {
        OffsetDateTime::now_utc()
            .to_offset(super::status::local_offset())
            .date()
    }

    /// Reads today's lanes and the rest when missing or older than
    /// [`REFRESH`].
    fn pulse_ensure(&mut self, cx: &mut Context<Self>) {
        if self.pulse.still.is_none() {
            self.pulse.still = Some(reduce_motion());
        }
        if self.pulse.loading {
            return;
        }
        let stale = match &self.pulse.data {
            None => true,
            Some(d) => {
                d.day.date != self.pulse_today() || OffsetDateTime::now_utc() - d.read_at > REFRESH
            }
        };
        if stale && self.pulse.error.is_none() {
            self.pulse_load(cx);
        }
    }

    fn pulse_load(&mut self, cx: &mut Context<Self>) {
        let Some(loaded) = self.loaded.as_ref() else {
            return;
        };
        let Some(cfg) = loaded.setup.config().cloned() else {
            return;
        };
        let ws = loaded.setup.workspace().cloned();
        let mut live: HashMap<String, Live> = HashMap::new();
        if let Some(f) = loaded.fleet.as_ref() {
            for r in &f.rows {
                let l = match &r.state {
                    State::Live => Live {
                        live: true,
                        ..Live::default()
                    },
                    State::Waiting { at, asked, .. } => Live {
                        waiting: Some((*at, asked.clone())),
                        ..Live::default()
                    },
                    State::Stopped { at, .. } => Live {
                        stopped: Some(*at),
                        ..Live::default()
                    },
                    _ => continue,
                };
                live.insert(r.id.clone(), l);
            }
        }
        let date = self.pulse_today();
        let offset = super::status::local_offset();
        self.pulse.loading = true;
        cx.spawn(async move |this, cx| {
            let read = cx
                .background_executor()
                .spawn(async move {
                    let now = OffsetDateTime::now_utc();
                    let rollup = hippo_core::rollup::Rollup::open(&cfg.home)
                        .map_err(|e| e.to_string())?
                        .ok_or_else(|| "The rollup has not been built yet.".to_owned())?;
                    let index = hippo_core::index::Index::open(&cfg.home, false)
                        .map_err(|e| e.to_string())?;
                    let day = replay::load(&rollup, &index, date, offset, now, &live)
                        .map_err(|e| e.to_string())?;
                    let incidents = rollup.incidents(day.start).unwrap_or_default();
                    let lessons = hippo_core::lessons::list(&cfg.home).unwrap_or_default();
                    let since = ws
                        .as_ref()
                        .and_then(|ws| hippo_core::dream::window::last_success(ws).ok().flatten());
                    let corrected = pulse::corrections(&incidents, &lessons, day.start, since);
                    let tonight = ws.as_ref().map(|ws| {
                        pulse::tonight(&rollup, &cfg.dream_at, since, pulse::notes_due(ws), now)
                    });
                    Ok::<_, String>((day, incidents, corrected, tonight, now))
                })
                .await;
            this.update(cx, |this, cx| {
                this.pulse.loading = false;
                match read {
                    Ok((day, incidents, corrected, tonight, read_at)) => {
                        this.pulse.data = Some(Rc::new(PulseData {
                            day: Rc::new(day),
                            incidents,
                            corrected,
                            tonight,
                            read_at,
                        }));
                        this.pulse.error = None;
                    }
                    Err(e) => this.pulse.error = Some(e),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The waiting sessions in the order Pulse lists them: the longest
    /// wait first, those put off with Later last.
    fn pulse_needs<'a>(&self, fleet: &'a Fleet) -> Vec<&'a Row> {
        let mut out: Vec<&Row> = fleet.needs_you().into_iter().map(|n| n.row).collect();
        out.sort_by_key(|r| self.pulse.later.iter().position(|l| *l == r.id));
        out
    }

    fn pulse_next(&self, fleet: &Fleet) -> Vec<Next> {
        match &self.pulse.data {
            Some(d) => pulse::next_up(fleet, &d.incidents, &d.corrected, d.day.start),
            None => Vec::new(),
        }
    }

    /// Pulse's keys: ↵ opens the first waiting thread (or runs the chosen
    /// Next up's action), T opens it in T3 Code, ⌘T in a terminal, S puts
    /// it off, J K move in Next up, U resumes the runs-dry sessions,
    /// L shows Lanes.
    pub(super) fn pulse_key(
        &mut self,
        k: &gpui::Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(fleet) = self.loaded.as_ref().and_then(|l| l.fleet.clone()) else {
            return false;
        };
        let key = k.key.as_str();
        if k.modifiers.platform {
            if key == "t"
                && let Some(r) = self.pulse_needs(&fleet).first()
            {
                let r = (*r).clone();
                self.pulse_terminal(&r, &fleet);
                cx.notify();
                return true;
            }
            return false;
        }
        if k.modifiers.control || k.modifiers.alt {
            return false;
        }
        let first = self.pulse_needs(&fleet).first().map(|r| (*r).clone());
        let next = self.pulse_next(&fleet);
        match key {
            "l" => {
                self.replay.lanes = true;
            }
            "enter" => {
                if let Some(i) = self.pulse.next_sel
                    && let Some(n) = next.get(i)
                {
                    let n = n.clone();
                    self.pulse_act(&n, &fleet, window, cx);
                    return true;
                }
                let Some(r) = first else { return false };
                self.open_session(r, window, cx);
                return true;
            }
            "t" => {
                let Some(r) = first else { return false };
                self.pulse_t3(&r);
            }
            "s" => {
                let Some(r) = first else { return false };
                self.pulse.later.retain(|l| *l != r.id);
                self.pulse.later.push(r.id);
            }
            "j" | "down" if !next.is_empty() => {
                let i = self
                    .pulse
                    .next_sel
                    .map_or(0, |i| (i + 1).min(next.len() - 1));
                self.pulse.next_sel = Some(i);
                self.pulse.next_all |= i >= NEXT_SHOWN;
            }
            "k" | "up" if !next.is_empty() => {
                self.pulse.next_sel = Some(self.pulse.next_sel.map_or(0, |i| i.saturating_sub(1)));
            }
            "escape" if self.pulse.next_sel.is_some() => self.pulse.next_sel = None,
            "pagedown" | "pageup" | "home" | "end" => {
                let h = self.page_scroll.bounds().size.height * 0.85;
                let max = self.page_scroll.max_offset().y;
                let mut o = self.page_scroll.offset();
                o.y = match key {
                    "pagedown" => (o.y - h).max(-max),
                    "pageup" => (o.y + h).min(px(0.)),
                    "home" => px(0.),
                    _ => -max,
                };
                self.page_scroll.set_offset(o);
            }
            "u" => {
                let Some(n) = next.iter().find(|n| matches!(n, Next::RunsDry { .. })) else {
                    return false;
                };
                let n = n.clone();
                self.pulse_act(&n, &fleet, window, cx);
                return true;
            }
            _ => return false,
        }
        cx.notify();
        true
    }

    /// Opens a T3 Code session's thread in T3 Code.
    fn pulse_t3(&mut self, row: &Row) {
        let said = match crate::launch::t3_thread_link(&row.id) {
            Some(link) => crate::launch::open(&link).map(|()| "opened in T3 Code".to_owned()),
            None => Err("T3 Code has no thread for this session".to_owned()),
        };
        self.reach.said = Some(said);
    }

    /// Resumes a session in the terminal Resume uses.
    fn pulse_terminal(&mut self, row: &Row, fleet: &Fleet) {
        let _ = fleet;
        let target = hippo_core::resume::Target {
            id: &row.id,
            tool: &row.tool,
            transcript: &row.transcript,
            cwd: row.cwd.as_deref(),
        };
        let term = self.term();
        let said = match hippo_core::resume::command(&target) {
            Some(cmd) => {
                let header = crate::launch::resume_header(
                    &row.title,
                    row.profile.as_deref().unwrap_or("its account"),
                );
                crate::launch::run_in_terminal(&cmd, &header, &term)
                    .map(|()| format!("resumed in {}", term.name()))
            }
            None => Err("This session has no resume command".to_owned()),
        };
        self.reach.said = Some(said);
    }

    /// Does what a Next up row offers.
    fn pulse_act(&mut self, n: &Next, fleet: &Fleet, window: &mut Window, cx: &mut Context<Self>) {
        match n {
            Next::RunsDry { sessions, to, .. } => {
                let Some(to) = to else {
                    self.reach.said = Some(Err("No other account has room".to_owned()));
                    cx.notify();
                    return;
                };
                let other = fleet.room(to);
                let index = self
                    .loaded
                    .as_ref()
                    .and_then(|l| l.setup.config())
                    .and_then(|c| hippo_core::index::Index::open(&c.home, false).ok());
                let cmds: Vec<String> = sessions
                    .iter()
                    .filter_map(|id| fleet.row(id))
                    .filter_map(|r| hippo_view::fleet::resume(r, index.as_ref(), other))
                    .map(|r| r.command)
                    .collect();
                if cmds.is_empty() {
                    self.reach.said = Some(Err("No resume command for these sessions".into()));
                } else {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(cmds.join("\n")));
                    self.reach.said = Some(Ok(format!(
                        "copied {} resume command{} for {}",
                        cmds.len(),
                        if cmds.len() == 1 { "" } else { "s" },
                        kit::profile_name(to)
                    )));
                }
                cx.notify();
            }
            Next::Unverified { session, .. } => self.open_xray(session, window, cx),
            Next::Destructive { incident, .. } => self.open_incident(incident, window, cx),
            Next::Correction { .. } => {
                self.show_section(crate::state::Section::Inbox, window, cx);
            }
        }
    }

    /// `Pulse | Lanes | Rows`.
    pub(super) fn fleet_view_switch(&self, look: &Look, cx: &mut Context<Self>) -> gpui::Div {
        let lanes = self.replay.lanes;
        let pulse = !lanes && self.pulse.on;
        let rows = !lanes && !self.pulse.on;
        kit::segmented(look)
            .child(
                kit::segment(look, "fleet-pulse", Some("heart-pulse"), "Pulse", pulse).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.replay.lanes = false;
                        this.pulse.on = true;
                        cx.notify();
                    }),
                ),
            )
            .child(
                kit::segment(look, "fleet-lanes", Some("calendar-clock"), "Lanes", lanes)
                    .tooltip(super::tooltip::Tooltip::text("The day as lanes (L)"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.replay.lanes = true;
                        cx.notify();
                    })),
            )
            .child(
                kit::segment(look, "fleet-rows", Some("list"), "Rows", rows).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.replay.lanes = false;
                        this.pulse.on = false;
                        cx.notify();
                    },
                )),
            )
    }

    #[allow(clippy::too_many_lines)]
    pub(super) fn render_pulse(
        &mut self,
        fleet: &Fleet,
        look: &Look,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        self.pulse_ensure(cx);
        let measured = self.pulse.width.get();
        let width = if measured > 0. {
            measured
        } else {
            f32::from(window.viewport_size().width) - self.sidebar_width(window)
        };
        let g = geo(width);
        let now = OffsetDateTime::now_utc();
        let data = self.pulse.data.clone();
        let t0 = match (self.pulse.span, &data) {
            (Span::Hour, _) => now - Duration::hours(1),
            (Span::Three, _) => now - Duration::hours(3),
            (Span::Day, Some(d)) => {
                let first = d
                    .day
                    .lanes
                    .iter()
                    .filter_map(|l| l.track.first)
                    .min()
                    .unwrap_or(now - Duration::hours(3));
                (first - Duration::minutes(10)).max(d.day.start)
            }
            (Span::Day, None) => now - Duration::hours(6),
        };
        let shelves = crate::shelving::Shelves::of(fleet, &self.marks());
        let needs = self.pulse_needs(fleet);
        let working: Vec<&Row> = shelves.shelves[1]
            .1
            .iter()
            .copied()
            .filter(|r| r.working())
            .collect();
        let quiet: Vec<&Row> = shelves.shelves[1]
            .1
            .iter()
            .copied()
            .filter(|r| !r.working())
            .collect();
        let review: Vec<&Row> = shelves.shelves[2].1.clone();
        let settled: Vec<&Row> = shelves.shelves[3].1.clone();
        let _ = Shelf::NeedsYou;
        let totals = pulse::totals(fleet, data.as_ref().map(|d| d.day.as_ref()), now);
        let next = self.pulse_next(fleet);
        let still = self.pulse.still.unwrap_or(true);

        let toolbar = self.pulse_toolbar(look, cx);
        // Blocks of lanes; those with tracks get the grid and the now line.
        let mut blocks: Vec<(bool, Vec<gpui::AnyElement>)> = Vec::new();
        let mut lanes: Vec<gpui::AnyElement> = Vec::new();
        lanes.push(self.pulse_header(fleet, &totals, data.as_deref(), t0, now, g, look));
        // Needs you.
        if !needs.is_empty() {
            let oldest = needs
                .iter()
                .filter_map(|r| match r.state {
                    State::Waiting { at, .. } | State::Stopped { at, .. } => Some(now - at),
                    _ => None,
                })
                .max();
            lanes.push(group_head(
                look,
                "Needs you",
                needs.len(),
                Some(format!(
                    "oldest {} · ↵ opens the first",
                    oldest.map(kit::span).unwrap_or_default()
                )),
            ));
            if needs.len() == 1 {
                blocks.push((true, std::mem::take(&mut lanes)));
                lanes.push(self.pulse_card(needs[0], fleet, data.as_deref(), t0, now, look, cx));
            } else {
                for (i, r) in needs.iter().enumerate() {
                    lanes.push(self.pulse_need_lane(
                        r,
                        i,
                        data.as_deref(),
                        t0,
                        now,
                        g,
                        still,
                        look,
                        cx,
                    ));
                }
            }
        }
        blocks.push((needs.len() != 1, std::mem::take(&mut lanes)));
        if !next.is_empty() {
            lanes.push(group_head(
                look,
                "Next up",
                next.len(),
                Some("will want you soon, most urgent first · J K move".into()),
            ));
            let shown = if self.pulse.next_all {
                next.len()
            } else {
                next.len().min(NEXT_SHOWN)
            };
            for (i, n) in next.iter().take(shown).enumerate() {
                lanes.push(self.pulse_next_row(n, i, fleet, now, look, cx));
            }
            if next.len() > shown {
                lanes.push(
                    div()
                        .id("pulse-next-more")
                        .py(look.px(6.))
                        .px(look.px(6.))
                        .text_size(look.text(12.))
                        .text_color(look.c.text_muted)
                        .cursor_pointer()
                        .hover(|h| h.text_color(look.c.text))
                        .child(format!("{} more", next.len() - shown))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.pulse.next_all = true;
                            cx.notify();
                        }))
                        .into_any_element(),
                );
            }
        }
        blocks.push((false, std::mem::take(&mut lanes)));
        if !working.is_empty() {
            lanes.push(group_head(look, "Working", working.len(), None));
            for r in &working {
                lanes.push(self.pulse_work_lane(r, data.as_deref(), t0, now, g, look, cx));
            }
        }
        if !review.is_empty() {
            lanes.push(group_head(
                look,
                "To review",
                review.len(),
                Some("finished, not opened yet".into()),
            ));
            for r in review.iter().take(REVIEW_SHOWN) {
                lanes.push(self.pulse_mini_lane(r, data.as_deref(), t0, now, g, look, cx));
            }
            if review.len() > REVIEW_SHOWN {
                lanes.push(
                    div()
                        .py(look.px(4.))
                        .text_size(look.text(12.))
                        .text_color(look.c.faint)
                        .child(format!("{} more in Rows", review.len() - REVIEW_SHOWN))
                        .into_any_element(),
                );
            }
        }
        if !quiet.is_empty() {
            let since: Vec<String> = quiet
                .iter()
                .filter_map(|r| r.last.map(kit::hm))
                .take(3)
                .collect();
            lanes.push(self.pulse_packed(
                "Quiet",
                &quiet,
                format!("since {}", since.join(", ")),
                data.as_deref(),
                t0,
                now,
                g,
                look,
            ));
        }
        if !settled.is_empty() {
            let merged = settled
                .iter()
                .filter(|r| r.pulls.iter().any(|(_, d)| *d == "merged"))
                .count();
            let summary = if merged > 0 {
                format!(
                    "{merged} pull request{} merged",
                    if merged == 1 { "" } else { "s" }
                )
            } else {
                "finished and opened".to_owned()
            };
            lanes.push(self.pulse_packed(
                "Settled",
                &settled,
                summary,
                data.as_deref(),
                t0,
                now,
                g,
                look,
            ));
        }
        if fleet.rows.is_empty() {
            lanes.push(
                div()
                    .py(look.px(18.))
                    .text_color(look.c.text_muted)
                    .child("No session today yet.")
                    .into_any_element(),
            );
        }
        if let Some(e) = &self.pulse.error {
            lanes.push(
                div()
                    .py(look.px(8.))
                    .text_size(look.text(12.))
                    .text_color(look.c.faint)
                    .child(format!("Lanes not read: {e}"))
                    .into_any_element(),
            );
        }
        // The grid and the now line over every lane.
        let overlay = || {
            let lk = look.clone();
            let span_s = (now - t0).whole_seconds();
            let t0c = t0;
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    let o = bounds.origin;
                    let w = f32::from(bounds.size.width);
                    let step = tick_step(span_s, w);
                    let off = i64::from(super::status::local_offset().whole_seconds());
                    let first = ((t0c.unix_timestamp() + off) / step + 1) * step - off;
                    let mut t = first;
                    while t < t0c.unix_timestamp() + span_s {
                        let x = (t - t0c.unix_timestamp()) as f32 / span_s as f32 * w;
                        window.paint_quad(fill(
                            Bounds::new(point(o.x + px(x), o.y), size(px(1.), bounds.size.height)),
                            lk.c.border.opacity(0.5),
                        ));
                        t += step;
                    }
                    window.paint_quad(fill(
                        Bounds::new(
                            point(o.x + px(w - 1.), o.y),
                            size(px(1.), bounds.size.height),
                        ),
                        lk.c.info.opacity(0.7),
                    ));
                },
            )
            .absolute()
            .top_0()
            .bottom_0()
            .left(look.px(g.head))
            .right(look.px(g.now))
        };
        blocks.push((true, lanes));
        let list = div()
            .flex()
            .flex_col()
            .px(look.px(g.pad))
            .pb(look.px(24.))
            .children(
                blocks
                    .into_iter()
                    .filter(|(_, v)| !v.is_empty())
                    .map(|(grid, v)| {
                        div()
                            .relative()
                            .flex()
                            .flex_col()
                            .when(grid && g.tracks, |d| d.child(overlay()))
                            .children(v)
                    }),
            );
        let rail = self.pulse_rail(fleet, data.as_deref(), g, now, look);
        let width_cell = self.pulse.width.clone();
        let shell = cx.entity().downgrade();
        let measure = canvas(
            move |bounds, _, cx| {
                let w = f32::from(bounds.size.width);
                if (width_cell.get() - w).abs() > 0.5 {
                    width_cell.set(w);
                    let shell = shell.clone();
                    cx.defer(move |cx| {
                        shell.update(cx, |_, cx| cx.notify()).ok();
                    });
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .w_full()
        .h(px(1.));
        let body = if g.rail {
            div()
                .flex()
                .items_start()
                .w_full()
                .child(div().flex_1().min_w_0().child(list))
                .child(
                    div()
                        .flex_shrink_0()
                        .w(look.px(RAIL_W))
                        .mr(look.px(g.pad))
                        .child(rail),
                )
        } else {
            div()
                .flex()
                .flex_col()
                .w_full()
                .child(div().px(look.px(g.pad)).child(rail))
                .child(list)
        };
        div()
            .id("pulse")
            .relative()
            .size_full()
            .child(measure)
            .child(
                div()
                    .id("pulse-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.page_scroll)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .w_full()
                            .pt(look.px(8.))
                            .text_size(look.text(13.))
                            .child(div().px(look.px(g.pad)).child(toolbar))
                            .children(self.render_reach_said(look))
                            .child(body),
                    ),
            )
            .children(super::scrollbar::vertical(&self.page_scroll, look))
            .into_any_element()
    }

    fn pulse_toolbar(&self, look: &Look, cx: &mut Context<Self>) -> gpui::Div {
        let span = self.pulse.span;
        let spans =
            [Span::Hour, Span::Three, Span::Day]
                .into_iter()
                .fold(kit::segmented(look), |d, s| {
                    d.child(
                        kit::segment(
                            look,
                            SharedString::from(format!("pulse-span-{}", s.label())),
                            None,
                            s.label(),
                            span == s,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.pulse.span = s;
                            cx.notify();
                        })),
                    )
                });
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(look.px(10.))
            .pb(look.px(10.))
            .child(self.fleet_view_switch(look, cx))
            .child(spans)
    }

    /// Big counts, the histogram and the now column.
    #[allow(clippy::too_many_arguments)]
    fn pulse_header(
        &self,
        fleet: &Fleet,
        t: &pulse::Totals,
        data: Option<&PulseData>,
        t0: OffsetDateTime,
        now: OffsetDateTime,
        g: Geo,
        look: &Look,
    ) -> gpui::AnyElement {
        let big = |n: usize, label: String, color: Hsla| {
            div()
                .flex()
                .flex_col()
                .min_w_0()
                .child(
                    div()
                        .text_size(look.text(30.))
                        .line_height(look.text(32.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(color)
                        .child(n.to_string()),
                )
                .child(
                    div()
                        .text_size(look.text(12.))
                        .text_color(if color == look.c.text {
                            look.c.text_muted
                        } else {
                            color
                        })
                        .whitespace_nowrap()
                        .child(label),
                )
        };
        let subs = if t.subs > 0 {
            format!(
                "working · {} sub-agent{}",
                t.subs,
                if t.subs == 1 { "" } else { "s" }
            )
        } else {
            "working".to_owned()
        };
        let head = div()
            .flex_shrink_0()
            .w(look.px(g.head))
            .flex()
            .items_end()
            .gap(look.px(26.))
            .child(big(t.working, subs, look.c.text))
            .child(big(
                t.need_you,
                "need you".into(),
                if t.need_you > 0 {
                    look.c.warn
                } else {
                    look.c.text
                },
            ));
        // The histogram, stacked by account in the rail's order.
        let order: Vec<String> = rail_profiles(fleet);
        let hist = data.map(|d| pulse::histogram(&d.day, t0, now, &order));
        let lk = look.clone();
        let span_s = (now - t0).whole_seconds();
        let middle = g.tracks.then(|| {
            let chart = canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    let Some(h) = &hist else { return };
                    let o = bounds.origin;
                    let w = f32::from(bounds.size.width);
                    let height = f32::from(bounds.size.height);
                    let peak = h.peak().max(6) as f32;
                    let span = span_s as f32;
                    let beat_w = replay::BEAT.as_seconds_f32() / span * w;
                    let bin = ((2.5 / beat_w).ceil() as usize).max(1);
                    let mut k = 0;
                    while k < h.bars.len() {
                        let end = (k + bin).min(h.bars.len());
                        // The tallest beat of the bin, its stack kept.
                        let Some(bar) = h.bars[k..end].iter().max_by_key(|b| b.iter().sum::<u32>())
                        else {
                            break;
                        };
                        let t = h.from + replay::BEAT * k as i32;
                        let x = (t - t0).as_seconds_f32() / span * w;
                        let bw = (beat_w * (end - k) as f32 - 1.).max(1.5);
                        let mut y = height;
                        for (p, v) in h.profiles.iter().zip(bar) {
                            if *v == 0 || x < 0. {
                                continue;
                            }
                            let bh = *v as f32 / peak * (height - 2.);
                            y -= bh;
                            let color = p.as_deref().map_or(lk.c.text_muted, kit::profile_hue);
                            window.paint_quad(fill(
                                Bounds::new(point(o.x + px(x), o.y + px(y)), size(px(bw), px(bh))),
                                color.opacity(0.9),
                            ));
                        }
                        k = end;
                    }
                    window.paint_quad(fill(
                        Bounds::new(
                            point(o.x, o.y + px(height - 1.)),
                            size(bounds.size.width, px(1.)),
                        ),
                        lk.c.border,
                    ));
                },
            )
            .w_full()
            .h(look.px(44.));
            // Tick labels.
            let lk = look.clone();
            let labels = canvas(
                |_, _, _| {},
                move |bounds, _, window, cx| {
                    let o = bounds.origin;
                    let w = f32::from(bounds.size.width);
                    let step = tick_step(span_s, w);
                    let off = i64::from(super::status::local_offset().whole_seconds());
                    let mut t = ((t0.unix_timestamp() + off) / step + 1) * step - off;
                    while t < t0.unix_timestamp() + span_s - step / 3 {
                        let x = (t - t0.unix_timestamp()) as f32 / span_s as f32 * w;
                        let Ok(at) = OffsetDateTime::from_unix_timestamp(t) else {
                            break;
                        };
                        let text: SharedString = kit::hm(at).into();
                        let run = gpui::TextRun {
                            len: text.len(),
                            font: gpui::font(kit::MONO),
                            color: lk.c.faint,
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        };
                        let size_px = lk.text(10.5);
                        let line = window.text_system().shape_line(text, size_px, &[run], None);
                        let lw = f32::from(line.width);
                        let _ = line.paint(
                            point(o.x + px(x - lw / 2.), o.y),
                            size_px * 1.3,
                            gpui::TextAlign::Left,
                            None,
                            window,
                            cx,
                        );
                        t += step;
                    }
                },
            )
            .w_full()
            .h(look.px(16.));
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(look.px(3.))
                .child(chart)
                .child(labels)
        });
        let work = kit::span(t.work);
        let now_col = div()
            .flex_shrink_0()
            .w(look.px(g.now))
            .pl(look.px(14.))
            .flex()
            .flex_col()
            .gap(look.px(3.))
            .text_size(look.text(12.))
            .text_color(look.c.text_muted)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(6.))
                    .font_family(kit::MONO)
                    .text_size(look.text(11.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(look.c.info)
                    .child(kit::dot(look, look.c.info))
                    .child(format!("NOW {}", kit::hm(now))),
            )
            .child(
                div()
                    .whitespace_nowrap()
                    .truncate()
                    .child(format!("{} tool calls in the last 5 min", t.calls_lately)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(5.))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .child(
                        div()
                            .text_color(look.c.text)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(work),
                    )
                    .child("of agent work")
                    .when(t.added + t.removed > 0, |d| {
                        d.child("·")
                            .child(
                                div()
                                    .font_family(kit::MONO)
                                    .text_size(look.text(11.5))
                                    .text_color(look.c.diff_add)
                                    .child(format!("+{}", thousands(t.added))),
                            )
                            .child(
                                div()
                                    .font_family(kit::MONO)
                                    .text_size(look.text(11.5))
                                    .text_color(look.c.diff_del)
                                    .child(format!("−{}", thousands(t.removed))),
                            )
                    }),
            );
        div()
            .flex()
            .items_center()
            .py(look.px(8.))
            .border_b(look.fixed(1.))
            .border_color(look.c.border)
            .child(head)
            .children(middle)
            .when(!g.tracks, |d| d.child(div().flex_1()))
            .child(now_col)
            .into_any_element()
    }

    /// A lane's wave from today's lanes.
    fn wave_of(
        &self,
        row: &Row,
        data: Option<&PulseData>,
        t0: OffsetDateTime,
        now: OffsetDateTime,
        end: Option<Hsla>,
    ) -> Option<Wave> {
        let d = data?;
        let (beats, waiting) = pulse::lane_beats(&d.day, &row.id, t0, now)?;
        let lane = d.day.lanes.iter().find(|l| l.track.id == row.id)?;
        let from = d.day.start
            + replay::BEAT * replay::beat_of(d.day.start, t0.max(d.day.start)).unwrap_or(0) as i32;
        Some(Wave {
            beats,
            waiting,
            from,
            marks: lane.marks.iter().map(|m| (m.at, m.kind)).collect(),
            ring: (!row.rules.is_empty())
                .then(|| row.started.or(lane.track.first))
                .flatten(),
            color: row
                .profile
                .as_deref()
                .map_or(gpui::hsla(0., 0., 0.6, 1.), kit::profile_hue),
            end,
        })
    }

    fn wave_el(
        &self,
        wave: Option<Wave>,
        t0: OffsetDateTime,
        now: OffsetDateTime,
        h: f32,
        bars: bool,
        look: &Look,
    ) -> gpui::AnyElement {
        let lk = look.clone();
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                if let Some(w) = &wave {
                    paint_wave(window, bounds, w, t0, now, &lk, bars);
                }
            },
        )
        .flex_1()
        .min_w_0()
        .h(look.px(h))
        .into_any_element()
    }

    /// Title, project, branch and lines: a lane's head.
    /// `extra`: points the title may take from the track's empty start.
    fn lane_head(&self, r: &Row, g: Geo, extra: f32, look: &Look) -> gpui::Div {
        let lines = r.total_lines();
        div()
            .flex_shrink_0()
            .w(look.px(g.head + extra))
            .pr(look.px(12.))
            .flex()
            .items_start()
            .gap(look.px(9.))
            .min_w_0()
            .child(
                div()
                    .pt(look.px(1.))
                    .child(kit::mark(look, r.profile.as_deref(), 20.)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .child(
                        div()
                            .text_size(look.text(13.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(look.c.text)
                            .truncate()
                            .child(kit::title(&r.title)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(look.px(6.))
                            .min_w_0()
                            .overflow_hidden()
                            .text_size(look.text(11.5))
                            .text_color(look.c.faint)
                            .children(r.project.clone().map(|p| div().flex_shrink_0().child(p)))
                            .children(r.branch.clone().map(|b| {
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .font_family(kit::MONO)
                                    .text_size(look.text(10.5))
                                    .child(kit::middle(&b, 28))
                            }))
                            .children(
                                lines.and_then(|l| kit::diff_counts(look, l.added, l.removed)),
                            ),
                    ),
            )
    }

    /// A waiting session as a lane: its wave, what it asked, and the ways in.
    #[allow(clippy::too_many_arguments)]
    fn pulse_need_lane(
        &self,
        r: &Row,
        i: usize,
        data: Option<&PulseData>,
        t0: OffsetDateTime,
        now: OffsetDateTime,
        g: Geo,
        still: bool,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let (what, icon, asked, since) = need_words(r, now);
        let wave = self.wave_of(r, data, t0, now, Some(look.c.warn));
        let dot = div().size(look.px(8.)).rounded_full().bg(look.c.warn);
        let dot: gpui::AnyElement = if still {
            dot.into_any_element()
        } else {
            dot.with_animation(
                SharedString::from(format!("pulse-dot-{}", r.id)),
                gpui::Animation::new(std::time::Duration::from_millis(1600)).repeat(),
                |d, t| d.opacity(0.35 + 0.65 * (1. - (t * std::f32::consts::TAU).cos()) / 2.),
            )
            .into_any_element()
        };
        // The title runs on over the track until its first activity.
        let extra = spill(wave.as_ref(), g, t0, now);
        let top = div()
            .flex()
            .items_center()
            .child(self.lane_head(r, g, extra, look))
            .when(g.tracks, |d| {
                d.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .ml(look.px(-extra))
                        .child(self.wave_el(wave, t0, now, TRACK_H, true, look)),
                )
            })
            .when(!g.tracks, |d| d.child(div().flex_1()))
            .child(
                div()
                    .flex_shrink_0()
                    .w(look.px(g.now))
                    .pl(look.px(14.))
                    .flex()
                    .items_center()
                    .gap(look.px(7.))
                    .text_color(look.c.warn)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(kit::icon(look, icon, 14., look.c.warn))
                    .child(div().min_w_0().truncate().child(what))
                    .child(div().flex_1())
                    .child(dot)
                    .child(
                        div()
                            .font_family(kit::MONO)
                            .text_size(look.text(12.))
                            .child(since),
                    ),
            );
        let id = r.id.clone();
        let row = r.clone();
        let row2 = r.clone();
        let row3 = r.clone();
        let first = i == 0;
        let buttons = div()
            .flex_shrink_0()
            .flex()
            .gap(look.px(6.))
            .child(
                kit::button_key(
                    look,
                    SharedString::from(format!("pulse-open-{id}")),
                    "Open thread",
                    if first { "↵" } else { "" },
                    Kind::Primary,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_session(row.clone(), window, cx);
                })),
            )
            .when(r.client == Some("T3 Code"), |d| {
                d.child(
                    kit::button_icon(
                        look,
                        SharedString::from(format!("pulse-t3-{id}")),
                        Some("arrow-up-right"),
                        "T3 Code",
                        if first { "T" } else { "" },
                        Kind::Plain,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.pulse_t3(&row2);
                        cx.notify();
                    })),
                )
            })
            .when(g.now + g.head > 480. || !g.tracks, |d| {
                d.child(
                    kit::button_icon(
                        look,
                        SharedString::from(format!("pulse-term-{id}")),
                        Some("terminal"),
                        "Terminal",
                        if first { "⌘T" } else { "" },
                        Kind::Plain,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(f) = this.loaded.as_ref().and_then(|l| l.fleet.clone()) {
                            this.pulse_terminal(&row3, &f);
                        }
                        cx.notify();
                    })),
                )
            });
        let ask =
            div()
                .flex()
                .items_center()
                .gap(look.px(14.))
                .pl(look.px(29.))
                .pt(look.px(4.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(look.text(14.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(look.c.text)
                        .child(asked.map_or_else(
                            || "It is waiting on you.".to_owned(),
                            |q| format!("“{q}”"),
                        )),
                )
                .child(buttons);
        div()
            .id(SharedString::from(format!("pulse-need-{}", r.id)))
            .mb(look.px(8.))
            .py(look.px(9.))
            .rounded(look.px(10.))
            .border(look.fixed(1.))
            .border_color(look.c.warn.opacity(0.45))
            .bg(look.c.warn.opacity(if look.c.dark { 0.07 } else { 0.06 }))
            .child(top)
            .child(div().pr(look.px(10.)).child(ask))
            .into_any_element()
    }

    /// B's large card for the one session that needs the person.
    #[allow(clippy::too_many_arguments)]
    fn pulse_card(
        &self,
        r: &Row,
        _fleet: &Fleet,
        data: Option<&PulseData>,
        t0: OffsetDateTime,
        now: OffsetDateTime,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let (what, icon, asked, since) = need_words(r, now);
        let wave = self.wave_of(r, data, t0, now, Some(look.c.warn));
        let lines = r.total_lines();
        let calls: Vec<hippo_view::fleet::tail::Activity> = r
            .turn
            .as_ref()
            .map(|t| t.calls.iter().take(3).rev().cloned().collect())
            .unwrap_or_default();
        let head = div()
            .flex()
            .items_center()
            .gap(look.px(9.))
            .text_size(look.text(12.5))
            .child(kit::mark(look, r.profile.as_deref(), 20.))
            .children(
                r.project
                    .clone()
                    .map(|p| div().text_color(look.c.text_muted).child(p)),
            )
            .children(r.branch.clone().map(|b| {
                div()
                    .font_family(kit::MONO)
                    .text_size(look.text(11.5))
                    .text_color(look.c.faint)
                    .truncate()
                    .child(kit::middle(&b, 36))
            }))
            .child(div().flex_1())
            .child(kit::icon(look, icon, 14., look.c.warn))
            .child(
                div()
                    .text_color(look.c.warn)
                    .font_weight(FontWeight::SEMIBOLD)
                    .whitespace_nowrap()
                    .child(format!("{what} · {since}")),
            );
        let quote = div()
            .text_size(look.text(21.))
            .line_height(look.text(27.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(look.c.text)
            .child(asked.map_or_else(|| "It is waiting on you.".to_owned(), |q| format!("“{q}”")));
        let steps = div()
            .flex()
            .items_center()
            .gap(look.px(10.))
            .min_w_0()
            .overflow_hidden()
            .children(calls.iter().enumerate().map(|(i, c)| {
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(10.))
                    .min_w_0()
                    .when(i > 0, |d| {
                        d.child(
                            div()
                                .w(look.px(10.))
                                .h(look.fixed(1.))
                                .bg(look.c.border_strong),
                        )
                    })
                    .child(kit::tool_line(look, c, 1, None, false))
            }));
        let facts = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(look.px(14.))
            .text_size(look.text(12.))
            .text_color(look.c.text_muted)
            .children(lines.map(|l| {
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(5.))
                    .children(kit::diff_counts(look, l.added, l.removed))
                    .child(format!(
                        "in {} file{}",
                        l.files,
                        if l.files == 1 { "" } else { "s" }
                    ))
            }))
            .children(
                r.pulls
                    .iter()
                    .rev()
                    .find(|(_, d)| *d == "opened")
                    .map(|(u, _)| {
                        div()
                            .flex()
                            .items_center()
                            .gap(look.px(5.))
                            .child(kit::icon(look, "git-pull-request", 13., look.c.faint))
                            .child(format!(
                                "PR #{} open",
                                super::sidebar::pr_number(u).unwrap_or("?")
                            ))
                    }),
            )
            .when(r.claims.verified > 0, |d| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(look.px(5.))
                        .child(kit::icon(look, "circle-check", 13., look.c.created))
                        .child(format!(
                            "{} claim{} verified",
                            r.claims.verified,
                            if r.claims.verified == 1 { "" } else { "s" }
                        )),
                )
            })
            .when(!r.rules.is_empty(), |d| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(look.px(5.))
                        .child(ring_glyph(look))
                        .child(format!(
                            "started with {} kept rule{}",
                            r.rules.len(),
                            if r.rules.len() == 1 { "" } else { "s" }
                        )),
                )
            });
        let id = r.id.clone();
        let (row, row2, row3) = (r.clone(), r.clone(), r.clone());
        let actions = div()
            .flex()
            .items_center()
            .gap(look.px(8.))
            .child(
                kit::button_key(
                    look,
                    SharedString::from(format!("card-open-{id}")),
                    "Open thread",
                    "↵",
                    Kind::Primary,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_session(row.clone(), window, cx);
                })),
            )
            .when(r.client == Some("T3 Code"), |d| {
                d.child(
                    kit::button_icon(
                        look,
                        SharedString::from(format!("card-t3-{id}")),
                        Some("arrow-up-right"),
                        "Open in T3 Code",
                        "T",
                        Kind::Plain,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.pulse_t3(&row2);
                        cx.notify();
                    })),
                )
            })
            .child(
                kit::button_icon(
                    look,
                    SharedString::from(format!("card-term-{id}")),
                    Some("terminal"),
                    "Terminal",
                    "⌘T",
                    Kind::Plain,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(f) = this.loaded.as_ref().and_then(|l| l.fleet.clone()) {
                        this.pulse_terminal(&row3, &f);
                    }
                    cx.notify();
                })),
            );
        let lk = look.clone();
        let track = wave.map(|w| {
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| paint_wave(window, bounds, &w, t0, now, &lk, true),
            )
            .w_full()
            .h(look.px(26.))
        });
        div()
            .id(SharedString::from(format!("pulse-card-{}", r.id)))
            .mb(look.px(10.))
            .p(look.px(18.))
            .rounded(look.px(12.))
            .border(look.fixed(1.))
            .border_color(look.c.warn.opacity(0.55))
            .bg(look.c.warn.opacity(if look.c.dark { 0.07 } else { 0.06 }))
            .flex()
            .flex_col()
            .gap(look.px(10.))
            .child(head)
            .child(
                div()
                    .text_size(look.text(13.5))
                    .text_color(look.c.text_muted)
                    .child(kit::title(&r.title)),
            )
            .child(quote)
            .children(track)
            .when(!calls.is_empty(), |d| d.child(steps))
            .child(facts)
            .child(actions)
            .into_any_element()
    }

    /// One row of Next up.
    fn pulse_next_row(
        &self,
        n: &Next,
        i: usize,
        fleet: &Fleet,
        now: OffsetDateTime,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let at = |s: i64| OffsetDateTime::from_unix_timestamp(s).unwrap_or(now);
        let (glyph, title, sub, right, action): (
            gpui::AnyElement,
            String,
            String,
            String,
            Option<String>,
        ) = match n {
            Next::RunsDry {
                profile,
                empty_at,
                reset,
                sessions,
                to,
            } => (
                kit::icon(look, "gauge", 15., look.c.err).into_any_element(),
                format!(
                    "{} runs dry in about {}",
                    kit::profile_name(profile),
                    kit::span(at(*empty_at) - now)
                ),
                format!(
                    "{} session{} stop near {}, {} before the {} reset",
                    sessions.len(),
                    if sessions.len() == 1 { "" } else { "s" },
                    kit::hm(at(*empty_at)),
                    kit::span(at(*reset) - at(*empty_at)),
                    kit::hm(at(*reset))
                ),
                kit::span(at(*empty_at) - now),
                to.as_ref()
                    .map(|t| format!("Resume them on {}", kit::profile_name(t))),
            ),
            Next::Unverified {
                title,
                claim,
                why,
                at,
                ..
            } => (
                kit::icon(look, "triangle-alert", 15., look.c.warn).into_any_element(),
                format!("{} said “{}”", kit::title(title), claim),
                format!("{} · finished {}", why, kit::hm(*at)),
                format!("{} ago", kit::span(now - *at)),
                Some("Check".into()),
            ),
            Next::Destructive {
                title, command, at, ..
            } => (
                div()
                    .size(look.px(9.))
                    .rounded(look.px(1.5))
                    .bg(look.c.err)
                    .into_any_element(),
                format!(
                    "Destructive command in {}",
                    title.as_deref().map_or("a session".into(), kit::title)
                ),
                kit::command_words(command.strip_prefix("Bash: ").unwrap_or(command)),
                kit::hm(*at),
                Some("Open incident".into()),
            ),
            Next::Correction { said, at, seen, .. } => (
                diamond_glyph(look).into_any_element(),
                format!("“{said}”"),
                format!(
                    "your correction, seen {}× · a rule candidate in tonight’s pass",
                    seen
                ),
                kit::hm(*at),
                Some("Keep now".into()),
            ),
        };
        let mono = matches!(n, Next::Destructive { .. });
        let on = self.pulse.next_sel == Some(i);
        let n2 = n.clone();
        let f2 = fleet.clone();
        div()
            .id(("pulse-next", i))
            .flex()
            .items_center()
            .gap(look.px(12.))
            .px(look.px(6.))
            .py(look.px(8.))
            .rounded(look.px(8.))
            .border_b(look.fixed(1.))
            .border_color(look.c.border)
            .when(on, |d| d.bg(look.c.selected))
            .child(
                div()
                    .w(look.px(18.))
                    .flex()
                    .justify_center()
                    .flex_shrink_0()
                    .child(glyph),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .truncate()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(look.c.text)
                            .child(title),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(look.text(12.))
                            .text_color(look.c.text_muted)
                            .when(mono, |d| {
                                d.font_family(kit::MONO).text_size(look.text(11.5))
                            })
                            .child(sub),
                    ),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .font_family(kit::MONO)
                    .text_size(look.text(12.))
                    .text_color(if matches!(n, Next::RunsDry { .. }) {
                        look.c.err
                    } else {
                        look.c.faint
                    })
                    .child(right),
            )
            .children(action.map(|a| {
                kit::button_key(
                    look,
                    ("pulse-next-act", i),
                    a,
                    if matches!(n, Next::RunsDry { .. }) {
                        "U"
                    } else {
                        ""
                    },
                    Kind::Plain,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.pulse_act(&n2, &f2, window, cx);
                }))
            }))
            .into_any_element()
    }

    /// A working session: its wave and sub-agents, and what it is doing.
    #[allow(clippy::too_many_arguments)]
    fn pulse_work_lane(
        &self,
        r: &Row,
        data: Option<&PulseData>,
        t0: OffsetDateTime,
        now: OffsetDateTime,
        g: Geo,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let wave = self.wave_of(r, data, t0, now, Some(look.c.info));
        let extra = spill(wave.as_ref(), g, t0, now);
        let call_line = r.call.as_ref().map(|c| {
            div()
                .flex()
                .items_center()
                .gap(look.px(6.))
                .min_w_0()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(kit::tool_line(look, c, 1, None, false)),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(look.text(11.5))
                        .text_color(look.c.faint)
                        .child(if c.running {
                            format!("running {}", kit::span(now - c.at))
                        } else {
                            kit::span(now - c.at)
                        }),
                )
        });
        let correction = data.and_then(|d| {
            d.day
                .lanes
                .iter()
                .find(|l| l.track.id == r.id)?
                .marks
                .iter()
                .rev()
                .find(|m| m.kind == MarkKind::Correction)
                .map(|m| m.at)
        });
        let turn = r
            .turn
            .as_ref()
            .and_then(|t| t.asked_at)
            .map(|a| format!("{} in this turn", kit::span(now - a)));
        let memo: Option<gpui::Div> = match (correction, r.rules.len()) {
            (Some(at), _) => Some(
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(5.))
                    .text_color(memory_yellow(look))
                    .child(diamond_glyph(look))
                    .child(format!("you corrected {}", kit::hm(at))),
            ),
            (None, 0) => None,
            (None, n) => Some(
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(5.))
                    .child(ring_glyph(look))
                    .child(format!(
                        "started with {n} kept rule{}",
                        if n == 1 { "" } else { "s" }
                    )),
            ),
        };
        let second = div()
            .flex()
            .items_center()
            .gap(look.px(8.))
            .overflow_hidden()
            .whitespace_nowrap()
            .text_size(look.text(11.5))
            .text_color(look.c.faint)
            .children(turn)
            .children(memo);
        let id = r.id.clone();
        let mut el = div()
            .id(SharedString::from(format!("pulse-work-{id}")))
            .py(look.px(6.))
            .cursor_pointer()
            .rounded(look.px(8.))
            .hover(|h| h.bg(look.c.hover.opacity(0.5)))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_xray(&id, window, cx);
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(self.lane_head(r, g, extra, look))
                    .when(g.tracks, |d| {
                        d.child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .ml(look.px(-extra))
                                .child(self.wave_el(wave, t0, now, TRACK_H, true, look)),
                        )
                    })
                    .when(!g.tracks, |d| d.child(div().flex_1()))
                    .child(
                        div()
                            .flex_shrink_0()
                            .w(look.px(g.now))
                            .pl(look.px(14.))
                            .flex()
                            .flex_col()
                            .gap(look.px(2.))
                            .min_w_0()
                            .children(call_line)
                            .when(r.call.is_none(), |d| {
                                d.child(
                                    div()
                                        .truncate()
                                        .text_size(look.text(12.))
                                        .text_color(look.c.text_muted)
                                        .child(r.activity.clone().unwrap_or_default()),
                                )
                            })
                            .child(second),
                    ),
            );
        // Sub-agents as strands.
        if let Some(d) = data
            && let Some(lane) = d.day.lanes.iter().find(|l| l.track.id == r.id)
        {
            let kids: Vec<&replay::Track> = lane
                .children
                .iter()
                .filter(|c| c.last.is_some_and(|l| l >= t0))
                .collect();
            let shown = kids.len().min(SUBS_SHOWN);
            let live = &r.live_subs;
            for (j, c) in kids.iter().rev().take(shown).rev().enumerate() {
                let from = d.day.start
                    + replay::BEAT
                        * replay::beat_of(d.day.start, t0.max(d.day.start)).unwrap_or(0) as i32;
                let a = replay::beat_of(d.day.start, t0.max(d.day.start)).unwrap_or(0);
                let beats: Vec<u16> = c.beats.iter().skip(a).copied().collect();
                let working = c.last.is_some_and(|l| now - l < Duration::minutes(2));
                let w = Wave {
                    waiting: vec![false; beats.len()],
                    beats,
                    from,
                    marks: Vec::new(),
                    ring: None,
                    color: r
                        .profile
                        .as_deref()
                        .map_or(gpui::hsla(0., 0., 0.6, 1.), kit::profile_hue)
                        .opacity(0.85),
                    end: Some(if working { look.c.info } else { look.c.created }),
                };
                let title = c.title.clone().unwrap_or_else(|| "A sub-agent".into());
                let step = live
                    .iter()
                    .find(|l| l.title.as_deref() == c.title.as_deref())
                    .map(|l| l.step.clone())
                    .or_else(|| {
                        (!working)
                            .then(|| c.last.map(|t| format!("done {}", kit::hm(t))))
                            .flatten()
                    })
                    .unwrap_or_default();
                el = el.child(
                    div()
                        .id(SharedString::from(format!("pulse-sub-{}-{j}", r.id)))
                        .flex()
                        .items_center()
                        .h(look.px(SUB_H + 3.))
                        .child(
                            div()
                                .flex_shrink_0()
                                .w(look.px(g.head))
                                .pl(look.px(29.))
                                .pr(look.px(12.))
                                .flex()
                                .items_center()
                                .gap(look.px(6.))
                                .text_size(look.text(12.))
                                .text_color(look.c.text_muted)
                                .child(
                                    div()
                                        .w(look.px(8.))
                                        .h(look.fixed(1.))
                                        .bg(look.c.border_strong),
                                )
                                .child(div().min_w_0().truncate().child(title)),
                        )
                        .when(g.tracks, |d| {
                            d.child(self.wave_el(Some(w), t0, now, SUB_H, false, look))
                        })
                        .when(!g.tracks, |d| d.child(div().flex_1()))
                        .child(
                            div()
                                .flex_shrink_0()
                                .w(look.px(g.now))
                                .pl(look.px(14.))
                                .truncate()
                                .text_size(look.text(11.5))
                                .text_color(look.c.faint)
                                .child(step),
                        ),
                );
            }
            if kids.len() > shown {
                el = el.child(
                    div()
                        .pl(look.px(29.))
                        .text_size(look.text(11.5))
                        .text_color(look.c.faint)
                        .child(format!("{} more sub-agents earlier", kids.len() - shown)),
                );
            }
        }
        el.into_any_element()
    }

    /// A finished session to review: a thin wave and how it ended.
    #[allow(clippy::too_many_arguments)]
    fn pulse_mini_lane(
        &self,
        r: &Row,
        data: Option<&PulseData>,
        t0: OffsetDateTime,
        now: OffsetDateTime,
        g: Geo,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let merged = r.pulls.iter().any(|(_, d)| *d == "merged");
        let wave = self.wave_of(r, data, t0, now, merged.then_some(look.c.created));
        let wave = wave.map(|mut w| {
            w.end = None;
            w
        });
        let lines = r.total_lines();
        let pr = r
            .pulls
            .iter()
            .rev()
            .find(|(_, d)| *d == "opened")
            .and_then(|(u, _)| super::sidebar::pr_number(u).map(str::to_owned));
        let done_at = match r.state {
            State::Finished { at } | State::Interrupted { at, .. } => Some(at),
            _ => r.last,
        };
        let right: gpui::Div = if let Some((claim, _)) = &r.claims.first_unverified {
            div()
                .flex()
                .items_center()
                .gap(look.px(6.))
                .min_w_0()
                .text_color(look.c.warn)
                .child(kit::icon(look, "triangle-alert", 13., look.c.warn))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .child(format!("claim unverified: “{claim}”")),
                )
        } else {
            div()
                .flex()
                .items_center()
                .gap(look.px(6.))
                .min_w_0()
                .child(kit::icon(look, "circle-check", 13., look.c.created))
                .child(
                    div()
                        .whitespace_nowrap()
                        .child(format!("done {}", done_at.map(kit::hm).unwrap_or_default())),
                )
                .children(pr.map(|n| div().whitespace_nowrap().child(format!("· PR #{n} open"))))
                .children(lines.and_then(|l| kit::diff_counts(look, l.added, l.removed)))
        };
        let id = r.id.clone();
        div()
            .id(SharedString::from(format!("pulse-review-{id}")))
            .flex()
            .items_center()
            .h(look.px(28.))
            .cursor_pointer()
            .rounded(look.px(6.))
            .hover(|h| h.bg(look.c.hover.opacity(0.5)))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_xray(&id, window, cx);
            }))
            .child(
                div()
                    .flex_shrink_0()
                    .w(look.px(g.head))
                    .pr(look.px(12.))
                    .flex()
                    .items_center()
                    .gap(look.px(9.))
                    .child(kit::mark(look, r.profile.as_deref(), 16.))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(look.text(12.5))
                            .text_color(look.c.text_muted)
                            .child(kit::title(&r.title)),
                    ),
            )
            .when(g.tracks, |d| {
                d.child(self.wave_el(wave, t0, now, MINI_H, false, look))
            })
            .when(!g.tracks, |d| d.child(div().flex_1()))
            .child(
                div()
                    .flex_shrink_0()
                    .w(look.px(g.now))
                    .pl(look.px(14.))
                    .text_size(look.text(12.))
                    .text_color(look.c.faint)
                    .child(right),
            )
            .into_any_element()
    }

    /// Quiet or settled sessions packed into one row.
    #[allow(clippy::too_many_arguments)]
    fn pulse_packed(
        &self,
        label: &str,
        rows: &[&Row],
        summary: String,
        data: Option<&PulseData>,
        t0: OffsetDateTime,
        now: OffsetDateTime,
        g: Geo,
        look: &Look,
    ) -> gpui::AnyElement {
        let waves: Vec<Wave> = rows
            .iter()
            .take(6)
            .filter_map(|r| {
                let merged = r.pulls.iter().any(|(_, d)| *d == "merged");
                self.wave_of(r, data, t0, now, None).map(|mut w| {
                    w.end = None;
                    w.marks.retain(|(_, k)| *k == MarkKind::PullMerged);
                    if !merged {
                        w.marks.clear();
                    }
                    w.ring = None;
                    w
                })
            })
            .collect();
        let lk = look.clone();
        let n = waves.len().max(1);
        let tracks = canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let h = f32::from(bounds.size.height) / n as f32;
                for (i, w) in waves.iter().enumerate() {
                    let b = Bounds::new(
                        point(bounds.origin.x, bounds.origin.y + px(h * i as f32)),
                        size(bounds.size.width, px(h)),
                    );
                    paint_wave(window, b, w, t0, now, &lk, false);
                }
            },
        )
        .flex_1()
        .min_w_0()
        .h(look.px((rows.len().min(6) as f32 * 7.).max(10.)));
        div()
            .flex()
            .items_center()
            .py(look.px(6.))
            .child(
                div()
                    .flex_shrink_0()
                    .w(look.px(g.head))
                    .flex()
                    .items_center()
                    .gap(look.px(8.))
                    .text_size(look.text(12.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(look.c.text_muted)
                    .child(label.to_owned())
                    .child(
                        div()
                            .text_color(look.c.faint)
                            .font_weight(FontWeight::NORMAL)
                            .child(rows.len().to_string()),
                    ),
            )
            .when(g.tracks, |d| d.child(tracks))
            .when(!g.tracks, |d| d.child(div().flex_1()))
            .child(
                div()
                    .flex_shrink_0()
                    .w(look.px(g.now))
                    .pl(look.px(14.))
                    .truncate()
                    .text_size(look.text(12.))
                    .text_color(look.c.faint)
                    .child(summary),
            )
            .into_any_element()
    }

    /// The rail: usage per account, memory today, tonight, the legend;
    /// beside the lanes when wide, else the accounts as a strip.
    fn pulse_rail(
        &self,
        fleet: &Fleet,
        data: Option<&PulseData>,
        g: Geo,
        now: OffsetDateTime,
        look: &Look,
    ) -> gpui::AnyElement {
        let live = pulse::live_by_profile(&fleet.rows);
        let profiles = rail_profiles(fleet);
        if !g.rail {
            // The strip: one compact cell per account, then tonight.
            let cells = profiles.iter().filter_map(|p| {
                let h = fleet.room(p)?;
                let left = h.five.map(|l| l.frac).or_else(|| h.left())?;
                let d = fleet.drain(p);
                let dry = d.and_then(|d| d.empty_at);
                Some(
                    div()
                        .flex()
                        .items_center()
                        .gap(look.px(7.))
                        .whitespace_nowrap()
                        .child(kit::dot(look, kit::profile_hue(p)))
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(look.c.text)
                                .child(kit::profile_name(p)),
                        )
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(if dry.is_some() || left < 0.1 {
                                    look.c.err
                                } else {
                                    look.c.text
                                })
                                .child(format!("{}%", (left * 100.).round())),
                        )
                        .children(d.map(|d| drain_chart(look, d, kit::profile_hue(p), 64., 18.)))
                        .child(
                            div()
                                .text_size(look.text(12.))
                                .text_color(if dry.is_some() {
                                    look.c.err
                                } else {
                                    look.c.faint
                                })
                                .child(match (dry, h.five.and_then(|l| l.resets_at)) {
                                    (Some(e), _) => format!(
                                        "empty ~{}",
                                        kit::hm(
                                            OffsetDateTime::from_unix_timestamp(e).unwrap_or(now)
                                        )
                                    ),
                                    (None, Some(r)) => format!(
                                        "resets {}",
                                        kit::hm(
                                            OffsetDateTime::from_unix_timestamp(r).unwrap_or(now)
                                        )
                                    ),
                                    _ => String::new(),
                                }),
                        )
                        .when(fleet.use_next.as_deref() == Some(p.as_str()), |d| {
                            d.child(kit::pill(look, "use next", look.c.created))
                        }),
                )
            });
            let tonight = data.and_then(|d| d.tonight.clone()).map(|t| {
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(6.))
                    .whitespace_nowrap()
                    .text_size(look.text(12.))
                    .text_color(look.c.text_muted)
                    .child(kit::icon(look, "moon", 13., look.c.faint))
                    .child(format!("Tonight {}", t.at))
                    .child(diamond_glyph(look))
                    .child(t.corrections.to_string())
                    .child(div().size(look.px(8.)).rounded(look.px(1.5)).bg(look.c.err))
                    .child(t.destructive.to_string())
            });
            return div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_x(look.px(18.))
                .gap_y(look.px(6.))
                .py(look.px(8.))
                .mb(look.px(4.))
                .border_y(look.fixed(1.))
                .border_color(look.c.border)
                .text_size(look.text(12.5))
                .children(cells)
                .child(div().flex_1())
                .children(tonight)
                .into_any_element();
        }
        let section = |icon: &str, title: &str, right: String| {
            div()
                .flex()
                .items_center()
                .gap(look.px(8.))
                .h(look.px(32.))
                .text_size(look.text(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(look.c.text_muted)
                .child(kit::icon(look, icon, 13., look.c.faint))
                .child(title.to_owned())
                .child(div().flex_1())
                .child(
                    div()
                        .font_weight(FontWeight::NORMAL)
                        .text_color(look.c.faint)
                        .child(right),
                )
        };
        let meter_age = fleet
            .headroom
            .iter()
            .filter_map(|h| h.meter.map(|m| m.fetched_at))
            .max()
            .map(|t| {
                format!(
                    "meter {} ago",
                    kit::span(now - OffsetDateTime::from_unix_timestamp(t).unwrap_or(now))
                )
            })
            .unwrap_or_default();
        let mut usage = div()
            .flex()
            .flex_col()
            .pb(look.px(10.))
            .border_b(look.fixed(1.))
            .border_color(look.c.border)
            .child(section("gauge", "Usage", meter_age));
        let mut any = false;
        for p in &profiles {
            let Some(h) = fleet.room(p) else { continue };
            let Some(left) = h.five.map(|l| l.frac).or_else(|| h.left()) else {
                continue;
            };
            any = true;
            usage = usage.child(account_block(
                look,
                fleet,
                p,
                left,
                live.get(p).copied().unwrap_or(0),
                now,
            ));
        }
        if !any {
            usage = usage.child(
                div()
                    .text_size(look.text(12.))
                    .text_color(look.c.faint)
                    .child("No account figure yet."),
            );
        }
        let served = pulse::served(&fleet.rows);
        let mut memory = div()
            .flex()
            .flex_col()
            .gap(look.px(8.))
            .pb(look.px(12.))
            .border_b(look.fixed(1.))
            .border_color(look.c.border)
            .child(section(
                "brain",
                "Memory today",
                format!("{served} sessions served"),
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(8.))
                    .text_size(look.text(12.5))
                    .text_color(look.c.text)
                    .child(ring_glyph(look))
                    .child(format!(
                        "{served} session{} started with a kept rule",
                        if served == 1 { "" } else { "s" }
                    )),
            );
        if let Some(d) = data {
            for c in d.corrected.iter().take(3) {
                let (fate, seen) = match c.fate {
                    Fate::Candidate { seen, .. } => ("rule candidate", format!(" · seen {seen}×")),
                    Fate::Tonight => ("read tonight", String::new()),
                    Fate::Read => ("read by the last pass", String::new()),
                };
                memory = memory.child(
                    div()
                        .flex()
                        .items_start()
                        .gap(look.px(8.))
                        .child(div().pt(look.px(4.)).child(diamond_glyph(look)))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .min_w_0()
                                .child(
                                    div()
                                        .text_size(look.text(12.5))
                                        .text_color(look.c.text)
                                        .child(format!("“{}”", short(&c.said, 90))),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .gap(look.px(4.))
                                        .text_size(look.text(11.5))
                                        .text_color(look.c.faint)
                                        .child(format!("{}{seen} ·", kit::hm(c.at)))
                                        .child(div().text_color(memory_yellow(look)).child(fate)),
                                ),
                        ),
                );
            }
        }
        let tonight = data.and_then(|d| d.tonight.clone()).map(|t| {
            let chip = |glyph: Option<gpui::AnyElement>, n: usize, what: &str| {
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(6.))
                    .h(look.px(24.))
                    .px(look.px(8.))
                    .rounded(look.px(6.))
                    .border(look.fixed(1.))
                    .border_color(look.c.border)
                    .text_size(look.text(12.))
                    .text_color(look.c.text_muted)
                    .children(glyph)
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(look.c.text)
                            .child(n.to_string()),
                    )
                    .child(what.to_owned())
            };
            let plural = |n: usize, one: &str, many: &str| {
                if n == 1 {
                    one.to_owned()
                } else {
                    many.to_owned()
                }
            };
            div()
                .flex()
                .flex_col()
                .pb(look.px(12.))
                .border_b(look.fixed(1.))
                .border_color(look.c.border)
                .child(section(
                    "moon",
                    &format!("Tonight {}", t.at),
                    format!("reads {} sessions", t.sessions),
                ))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(look.px(6.))
                        .child(chip(
                            Some(diamond_glyph(look).into_any_element()),
                            t.corrections,
                            &plural(t.corrections, "correction", "corrections"),
                        ))
                        .child(chip(
                            Some(
                                div()
                                    .size(look.px(8.))
                                    .rounded(look.px(1.5))
                                    .bg(look.c.err)
                                    .into_any_element(),
                            ),
                            t.destructive,
                            &plural(t.destructive, "destructive command", "destructive commands"),
                        ))
                        .child(chip(
                            None,
                            t.unverified,
                            &plural(t.unverified, "unverified claim", "unverified claims"),
                        ))
                        .child(chip(
                            None,
                            t.notes_due,
                            &plural(t.notes_due, "note due a re-check", "notes due a re-check"),
                        )),
                )
        });
        let key = |glyph: gpui::AnyElement, what: &str| {
            div()
                .flex()
                .items_center()
                .gap(look.px(6.))
                .text_size(look.text(11.5))
                .text_color(look.c.faint)
                .child(div().w(look.px(14.)).flex().justify_center().child(glyph))
                .child(what.to_owned())
        };
        let legend = div()
            .pt(look.px(12.))
            .flex()
            .flex_wrap()
            .gap_x(look.px(12.))
            .gap_y(look.px(6.))
            .child(key(
                div()
                    .w(look.px(3.))
                    .h(look.px(10.))
                    .bg(look.c.text_muted)
                    .into_any_element(),
                "tool calls and edits",
            ))
            .child(key(
                div()
                    .w(look.px(12.))
                    .h(look.fixed(2.))
                    .bg(look.c.warn)
                    .into_any_element(),
                "waiting on you",
            ))
            .child(key(
                diamond_glyph(look).into_any_element(),
                "you corrected it",
            ))
            .child(key(
                div()
                    .size(look.px(8.))
                    .rounded(look.px(1.5))
                    .bg(look.c.err)
                    .into_any_element(),
                "destructive",
            ))
            .child(key(ring_glyph(look).into_any_element(), "kept rule served"))
            .child(key(
                div()
                    .size(look.px(8.))
                    .rounded_full()
                    .bg(look.c.created)
                    .into_any_element(),
                "merged",
            ));
        div()
            .flex()
            .flex_col()
            .gap(look.px(4.))
            .pl(look.px(16.))
            .border_l(look.fixed(1.))
            .border_color(look.c.border)
            .child(usage)
            .child(memory)
            .children(tonight)
            .child(legend)
            .into_any_element()
    }
}

/// How far a lane's title may run on over its track: up to its first
/// activity (or its first mark), less a gap.
fn spill(wave: Option<&Wave>, g: Geo, t0: OffsetDateTime, now: OffsetDateTime) -> f32 {
    if !g.tracks {
        return 0.;
    }
    let Some(w) = wave else {
        return g.track * 0.5;
    };
    let first = w
        .beats
        .iter()
        .position(|v| *v > 0)
        .map(|k| w.from + replay::BEAT * k as i32);
    let mark = w
        .marks
        .iter()
        .map(|(t, _)| *t)
        .chain(w.ring)
        .filter(|t| *t >= t0)
        .min();
    let Some(t) = [first, mark].into_iter().flatten().min() else {
        return g.track * 0.5;
    };
    let frac = (t - t0).as_seconds_f32() / (now - t0).as_seconds_f32().max(1.);
    (frac.clamp(0., 1.) * g.track - 20.).clamp(0., g.track * 0.6)
}

/// The rail's accounts: the one to use next first, then by name.
fn rail_profiles(fleet: &Fleet) -> Vec<String> {
    let mut out: Vec<String> = fleet
        .headroom
        .iter()
        .filter(|h| h.five.is_some() || h.left().is_some())
        .map(|h| h.profile.clone())
        .collect();
    out.sort_by_key(|p| {
        (
            fleet.use_next.as_deref() != Some(p.as_str()),
            kit::profile_name(p).to_lowercase(),
        )
    });
    out
}

/// What a needs-you session is waiting for: words, icon, its question and
/// the wait.
fn need_words(r: &Row, now: OffsetDateTime) -> (String, &'static str, Option<String>, String) {
    match &r.state {
        State::Waiting { at, asked, plan } => (
            if *plan {
                "Plan to approve".into()
            } else {
                "Asked you a question".into()
            },
            if *plan {
                "list-checks"
            } else {
                "message-circle-question"
            },
            asked.clone(),
            kit::span(now - *at),
        ),
        State::Stopped { at, reset, .. } => (
            "Stopped at its limit".into(),
            "gauge",
            reset.clone().map(|r| format!("Usage limit · {r}")),
            kit::span(now - *at),
        ),
        _ => ("Needs you".into(), "hand", None, String::new()),
    }
}

/// A group's heading: `Needs you  2`, with a hint at the right.
fn group_head(look: &Look, title: &str, n: usize, hint: Option<String>) -> gpui::AnyElement {
    div()
        .relative()
        .flex()
        .items_end()
        .gap(look.px(8.))
        .h(look.px(30.))
        .pb(look.px(6.))
        .text_size(look.text(12.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(look.c.text_muted)
        .child(title.to_owned())
        .child(
            div()
                .font_weight(FontWeight::NORMAL)
                .text_color(look.c.faint)
                .child(n.to_string()),
        )
        .child(div().flex_1())
        .children(hint.map(|h| {
            div()
                .font_weight(FontWeight::NORMAL)
                .text_color(look.c.faint)
                .truncate()
                .child(h)
        }))
        .into_any_element()
}

/// The yellow ring glyph: a kept rule served.
fn ring_glyph(look: &Look) -> gpui::Div {
    div()
        .flex_shrink_0()
        .size(look.px(9.))
        .rounded_full()
        .border(look.fixed(1.5))
        .border_color(memory_yellow(look))
}

/// The yellow diamond glyph: the person corrected it.
fn diamond_glyph(look: &Look) -> gpui::Div {
    let c = memory_yellow(look);
    div().flex_shrink_0().size(look.px(10.)).child(
        canvas(
            |_, _, _| {},
            move |b, _, window, _| {
                let ctr = b.center();
                let r = f32::from(b.size.width) / 2.;
                let mut p = PathBuilder::fill();
                p.move_to(point(ctr.x, ctr.y - px(r)));
                p.line_to(point(ctr.x + px(r), ctr.y));
                p.line_to(point(ctr.x, ctr.y + px(r)));
                p.line_to(point(ctr.x - px(r), ctr.y));
                p.close();
                if let Ok(p) = p.build() {
                    window.paint_path(p, c);
                }
            },
        )
        .size_full(),
    )
}

fn short(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('…');
    out
}

/// A drain chart: the window's samples as bars of what was left, the
/// projection to the reset as outlines, red where it runs dry.
fn drain_chart(
    look: &Look,
    d: &hippo_core::usage::Drain,
    hue: Hsla,
    w: f32,
    h: f32,
) -> gpui::AnyElement {
    let d = d.clone();
    let lk = look.clone();
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let o = bounds.origin;
            let width = f32::from(bounds.size.width);
            let height = f32::from(bounds.size.height);
            let slots = ((d.end - d.start) / hippo_core::usage::DRAIN_STEP).max(1) as usize;
            let slot_w = width / slots as f32;
            let bw = (slot_w - 1.).max(1.);
            for (k, used) in d.samples.iter().enumerate().take(slots) {
                let left = (1. - *used as f32).clamp(0., 1.);
                let bh = (left * (height - 1.)).max(1.);
                window.paint_quad(fill(
                    Bounds::new(
                        point(o.x + px(k as f32 * slot_w), o.y + px(height - bh)),
                        size(px(bw), px(bh)),
                    ),
                    hue.opacity(0.85),
                ));
            }
            // The projection at the burn rate, to the reset.
            let now_k = d.samples.len().saturating_sub(1);
            for k in now_k + 1..slots {
                let hours = (k - now_k) as f64 * hippo_core::usage::DRAIN_STEP as f64 / 3600.;
                let left = (1. - d.used - d.burn * hours) as f32;
                let x = o.x + px(k as f32 * slot_w);
                if left <= 0. {
                    window.paint_quad(fill(
                        Bounds::new(point(x, o.y + px(height - 1.)), size(px(bw), px(1.))),
                        lk.c.err.opacity(0.85),
                    ));
                    continue;
                }
                let bh = (left * (height - 1.)).max(1.);
                window.paint_quad(gpui::outline(
                    Bounds::new(point(x, o.y + px(height - bh)), size(px(bw), px(bh))),
                    hue.opacity(0.55),
                    gpui::BorderStyle::Solid,
                ));
            }
            // Now.
            let nx = (now_k as f32 + 1.) * slot_w;
            window.paint_quad(fill(
                Bounds::new(point(o.x + px(nx), o.y), size(px(1.), bounds.size.height)),
                lk.c.info.opacity(0.8),
            ));
            window.paint_quad(fill(
                Bounds::new(
                    point(o.x + px(width - 1.), o.y),
                    size(px(1.), bounds.size.height),
                ),
                lk.c.border_strong,
            ));
        },
    )
    .flex_shrink_0()
    .w(look.px(w))
    .h(look.px(h))
    .into_any_element()
}

/// One account in the rail.
fn account_block(
    look: &Look,
    fleet: &Fleet,
    p: &str,
    left: f64,
    working: usize,
    now: OffsetDateTime,
) -> gpui::AnyElement {
    let hue = kit::profile_hue(p);
    let h = fleet.room(p);
    let d = fleet.drain(p);
    let dry = d.and_then(|d| d.empty_at);
    let low = dry.is_some() || left < 0.15;
    let at = |s: i64| OffsetDateTime::from_unix_timestamp(s).unwrap_or(now);
    let reset = h.and_then(|h| h.five.and_then(|l| l.resets_at).or_else(|| h.resets()));
    let tag = if fleet.use_next.as_deref() == Some(p) {
        Some(kit::pill(look, "USE NEXT", look.c.created))
    } else if low {
        Some(kit::pill(look, "LOW", look.c.err))
    } else {
        None
    };
    let seven = h.and_then(|h| h.seven);
    div()
        .flex()
        .flex_col()
        .gap(look.px(4.))
        .py(look.px(7.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(look.px(7.))
                .child(kit::dot(look, hue))
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(look.c.text)
                        .child(kit::profile_name(p)),
                )
                .when(working > 0, |d| {
                    d.child(
                        div()
                            .text_size(look.text(11.5))
                            .text_color(look.c.faint)
                            .child(format!("{working} working")),
                    )
                })
                .child(div().flex_1())
                .children(tag),
        )
        .child(
            div()
                .flex()
                .items_end()
                .gap(look.px(10.))
                .child(
                    div()
                        .flex()
                        .items_end()
                        .w(look.px(62.))
                        .flex_shrink_0()
                        .child(
                            div()
                                .text_size(look.text(22.))
                                .line_height(look.text(24.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(if low { look.c.err } else { look.c.text })
                                .child(format!("{}", (left * 100.).round())),
                        )
                        .child(
                            div()
                                .pb(look.px(2.))
                                .text_size(look.text(11.))
                                .text_color(look.c.faint)
                                .child("% 5h"),
                        ),
                )
                .children(d.map(|d| {
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(drain_chart(look, d, hue, 160., 26.))
                })),
        )
        .child(
            div()
                .flex()
                .gap(look.px(6.))
                .text_size(look.text(11.5))
                .text_color(look.c.faint)
                .whitespace_nowrap()
                .children(
                    d.map(|d| div().child(format!("burning {}% / h", (d.burn * 100.).round()))),
                )
                .child(div().flex_1())
                .children(reset.map(|r| {
                    div().child(format!(
                        "resets {} · {}",
                        kit::hm(at(r)),
                        kit::span(at(r) - now)
                    ))
                })),
        )
        .children(dry.zip(reset).map(|(e, r)| {
            div()
                .text_size(look.text(12.))
                .text_color(look.c.err)
                .child(format!(
                    "Empty about {}, {} before the reset. {working} session{} will stop.",
                    kit::hm(at(e)),
                    kit::span(at(r) - at(e)),
                    if working == 1 { "" } else { "s" }
                ))
        }))
        .children(seven.map(|s| {
            div()
                .flex()
                .items_center()
                .gap(look.px(6.))
                .text_size(look.text(11.5))
                .text_color(look.c.faint)
                .child("7d")
                .child(
                    div()
                        .flex_1()
                        .h(look.px(3.))
                        .rounded_full()
                        .bg(look.c.border)
                        .child(
                            div()
                                .h_full()
                                .rounded_full()
                                .w(gpui::relative(s.frac.clamp(0., 1.) as f32))
                                .bg(hue.opacity(0.8)),
                        ),
                )
                .child(format!(
                    "{}%{}",
                    (s.frac * 100.).round(),
                    s.resets_at
                        .map(|r| {
                            let t = at(r).to_offset(super::status::local_offset());
                            format!(" · {} {}", &t.weekday().to_string()[..3], kit::hm(at(r)))
                        })
                        .unwrap_or_default()
                ))
        }))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rail_sits_beside_only_when_wide_and_tracks_keep_their_room() {
        let wide = geo(1210.);
        assert!(wide.rail && wide.tracks);
        let mid = geo(870.);
        assert!(!mid.rail && mid.tracks);
        let narrow = geo(560.);
        assert!(!narrow.rail && !narrow.tracks);
    }

    #[test]
    fn ticks_keep_labels_apart() {
        assert_eq!(tick_step(3 * 3600, 600.), 30 * 60);
        assert_eq!(tick_step(3600, 600.), 15 * 60);
        assert!(tick_step(16 * 3600, 400.) >= 2 * 3600);
    }

    #[test]
    fn thousands_are_grouped() {
        assert_eq!(thousands(1204), "1,204");
        assert_eq!(thousands(311), "311");
        assert_eq!(thousands(1_000_000), "1,000,000");
    }
}
