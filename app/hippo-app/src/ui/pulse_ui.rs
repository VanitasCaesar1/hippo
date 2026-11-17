//! Pulse: Fleet's default view (docs/designs/fleet-pulse.md).
//!
//! A strip of counts and account meters over two panes: the queue (the
//! sessions that need the person, then Next up) and the selected item
//! alone in a focus pane. The fleet's lists, Memory today and Tonight
//! sit in a sheet over the queue (`F`); with nothing in the queue the
//! focus pane shows the fleet's histogram and today's totals. Below
//! [`STACK_BELOW`] points the panes stack and `↵` opens an item's focus.
//!
//! Today's lanes come from `hippo_view::replay` and the rest from
//! `hippo_view::pulse`, read on the background executor every 30
//! seconds while Pulse shows. The activity strips, the sheet's waves,
//! the histogram and the drain chart are painted on canvases. Only a
//! waiting row's dot pulses, and not when the system asks for reduced
//! motion. Pulse writes nothing: its actions open the place where an
//! item is dealt with.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::AnimationExt as _;
use gpui::{
    Bounds, Context, FontWeight, Hsla, PathBuilder, Pixels, ScrollHandle, SharedString, Window,
    canvas, div, fill, point, prelude::*, px, size,
};
use hippo_core::incidents::Incident;
use hippo_view::fleet::{Fleet, Row, State, Why};
use hippo_view::pulse::{self, Corrected, Fate, Next, Tonight};
use hippo_view::replay::{self, Day, Live, MarkKind};
use time::{Duration, OffsetDateTime};

use super::Shell;
use super::kit::{self, Kind};
use super::theme::Look;

/// How often today's lanes are read again while Pulse shows.
const REFRESH: Duration = Duration::seconds(30);
/// Below this content width the queue and the focus stack.
pub const STACK_BELOW: f32 = 900.;
/// From this content width the queue takes [`QUEUE_W`], below it
/// [`QUEUE_NARROW`].
const QUEUE_WIDE_FROM: f32 = 1100.;
const QUEUE_W: f32 = 380.;
const QUEUE_NARROW: f32 = 300.;
/// The focus column's widest.
const FOCUS_W: f32 = 640.;
/// A waiting session's calls shown before Show all.
const CALLS_SHOWN: usize = 3;
/// The sheet's waves cover this much time.
const SHEET_SPAN: Duration = Duration::minutes(30);
/// At most this many rows per sheet tab.
const SHEET_ROWS: usize = 12;

/// The span the activity strips and the histogram show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Span {
    #[default]
    Hour,
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

    fn words(self) -> &'static str {
        match self {
            Span::Hour => "last hour",
            Span::Three => "last 3 hours",
            Span::Day => "today",
        }
    }
}

/// The sheet's tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SheetTab {
    #[default]
    Working,
    Review,
    Quiet,
    Settled,
}

/// What Pulse reads besides the fleet.
pub struct PulseData {
    pub day: Rc<Day>,
    pub incidents: Vec<Incident>,
    pub corrected: Vec<Corrected>,
    pub tonight: Option<Tonight>,
    /// The current turn of each waiting session, which the fleet reads
    /// only for live ones.
    pub turns: HashMap<String, hippo_view::fleet::tail::Turn>,
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
    /// Items put off with Later ([`Item::key`]), shown after the others
    /// of their group until the app restarts.
    pub later: Vec<String>,
    /// The selected item's key; the first item when `None` or gone.
    pub sel: Option<String>,
    /// The fleet sheet is open over the queue.
    pub sheet: bool,
    pub sheet_tab: SheetTab,
    /// The selected session shows all its calls.
    pub all_calls: bool,
    /// Stacked: the focus is open over the queue.
    pub focus_open: bool,
    pub queue_scroll: ScrollHandle,
    pub sheet_scroll: ScrollHandle,
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
            sel: None,
            sheet: false,
            sheet_tab: SheetTab::default(),
            all_calls: false,
            focus_open: false,
            queue_scroll: ScrollHandle::new(),
            sheet_scroll: ScrollHandle::new(),
            width: Rc::new(Cell::new(0.)),
            still: None,
        }
    }
}

/// One thing in the queue.
#[derive(Debug, Clone)]
pub enum Item {
    /// A session waiting on the person, or stopped at its limit.
    Wait(Box<Row>),
    /// What will want the person soon.
    Next(Next),
}

impl Item {
    /// What identifies it across reads.
    pub fn key(&self) -> String {
        match self {
            Item::Wait(r) => format!("wait:{}", r.id),
            Item::Next(Next::RunsDry { profile, .. }) => format!("dry:{profile}"),
            Item::Next(Next::Unverified { session, claim, .. }) => {
                format!("claim:{session}:{claim}")
            }
            Item::Next(Next::Destructive { incident, .. }) => format!("harm:{incident}"),
            Item::Next(Next::Correction { lesson, .. }) => format!("rule:{lesson}"),
        }
    }
}

/// `items` with those put off (in `later`'s order) moved to the end.
pub fn later_last(mut items: Vec<Item>, later: &[String]) -> Vec<Item> {
    items.sort_by_key(|i| later.iter().position(|l| *l == i.key()));
    items
}

/// The selected index among `keys`: `sel`'s, else the first.
pub fn selected(keys: &[String], sel: Option<&str>) -> Option<usize> {
    if keys.is_empty() {
        return None;
    }
    Some(
        sel.and_then(|s| keys.iter().position(|k| k == s))
            .unwrap_or(0),
    )
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

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
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
        let mut waits: Vec<(String, std::path::PathBuf)> = Vec::new();
        if let Some(f) = loaded.fleet.as_ref() {
            for r in &f.rows {
                if matches!(r.state, State::Waiting { .. }) && r.turn.is_none() {
                    waits.push((r.id.clone(), r.transcript.clone()));
                }
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
                    let turns = waits
                        .into_iter()
                        .map(|(id, path)| (id, hippo_view::fleet::tail::read_turn(&path)))
                        .collect();
                    Ok::<_, String>((day, incidents, corrected, tonight, turns, now))
                })
                .await;
            this.update(cx, |this, cx| {
                this.pulse.loading = false;
                match read {
                    Ok((day, incidents, corrected, tonight, turns, read_at)) => {
                        this.pulse.data = Some(Rc::new(PulseData {
                            day: Rc::new(day),
                            incidents,
                            corrected,
                            tonight,
                            turns,
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

    /// The queue: the sessions that need the person (the longest wait
    /// first), then Next up (most urgent first); in each, those put off
    /// with Later last.
    fn pulse_queue(&self, fleet: &Fleet) -> (Vec<Item>, Vec<Item>) {
        let needs: Vec<Item> = fleet
            .needs_you()
            .into_iter()
            .filter(|n| !matches!(n.why, Why::Check))
            .map(|n| Item::Wait(Box::new(n.row.clone())))
            .collect();
        let next: Vec<Item> = match &self.pulse.data {
            Some(d) => pulse::next_up(fleet, &d.incidents, &d.corrected, d.day.start)
                .into_iter()
                .map(Item::Next)
                .collect(),
            None => Vec::new(),
        };
        (
            later_last(needs, &self.pulse.later),
            later_last(next, &self.pulse.later),
        )
    }

    /// The queue as one list, and the selected index in it.
    fn pulse_items(&self, fleet: &Fleet) -> (Vec<Item>, Option<usize>) {
        let (needs, next) = self.pulse_queue(fleet);
        let all: Vec<Item> = needs.into_iter().chain(next).collect();
        let keys: Vec<String> = all.iter().map(Item::key).collect();
        let i = selected(&keys, self.pulse.sel.as_deref());
        (all, i)
    }

    fn pulse_stacked(&self) -> bool {
        let w = self.pulse.width.get();
        w > 0. && w < STACK_BELOW
    }

    /// Selects `key`; stacked, it opens the item's focus.
    fn pulse_select(&mut self, key: String) {
        if self.pulse.sel.as_deref() != Some(key.as_str()) {
            self.pulse.all_calls = false;
        }
        self.pulse.sel = Some(key);
        if self.pulse_stacked() {
            self.pulse.focus_open = true;
        }
    }

    /// Puts `item` off with Later and selects the one after it.
    fn pulse_later(&mut self, items: &[Item], i: usize) {
        let key = items[i].key();
        let after = items
            .get(i + 1)
            .or_else(|| i.checked_sub(1).and_then(|j| items.get(j)))
            .map(Item::key);
        self.pulse.later.retain(|l| *l != key);
        self.pulse.later.push(key.clone());
        self.pulse.sel = after.or(Some(key));
        self.pulse.all_calls = false;
    }

    /// The item's first action: Open thread, Resume them on, Check, Open
    /// incident or Keep now.
    fn pulse_do(
        &mut self,
        item: &Item,
        fleet: &Fleet,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match item {
            Item::Wait(r) => self.open_session((**r).clone(), window, cx),
            Item::Next(n) => self.pulse_act(n, fleet, window, cx),
        }
    }

    /// Pulse's keys: J K (or the arrows) move in the queue, ↵ does the
    /// selected item's first action, T opens a waiting session in T3 Code
    /// and ⌘T in a terminal, U resumes the runs-dry sessions, S puts the
    /// item off, F opens the fleet sheet, L shows Lanes. Escape is
    /// [`Shell::pulse_escape`].
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
        let (items, sel) = self.pulse_items(&fleet);
        let current = sel.and_then(|i| items.get(i)).cloned();
        if k.modifiers.platform {
            if key == "t"
                && let Some(Item::Wait(r)) = &current
            {
                self.pulse_terminal(r, &fleet);
                cx.notify();
                return true;
            }
            return false;
        }
        if k.modifiers.control || k.modifiers.alt {
            return false;
        }
        match key {
            "l" => self.replay.lanes = true,
            "f" => {
                self.pulse.sheet = !self.pulse.sheet;
            }
            "j" | "down" if !items.is_empty() => {
                let i = sel.map_or(0, |i| (i + 1).min(items.len() - 1));
                self.pulse.sel = Some(items[i].key());
                self.pulse.all_calls = false;
            }
            "k" | "up" if !items.is_empty() => {
                let i = sel.map_or(0, |i| i.saturating_sub(1));
                self.pulse.sel = Some(items[i].key());
                self.pulse.all_calls = false;
            }
            "enter" => {
                let Some(item) = current else { return false };
                if self.pulse_stacked() && !self.pulse.focus_open {
                    self.pulse.focus_open = true;
                } else {
                    self.pulse_do(&item, &fleet, window, cx);
                    return true;
                }
            }
            "t" => {
                let Some(Item::Wait(r)) = current else {
                    return false;
                };
                self.pulse_t3(&r);
            }
            "s" => {
                let Some(i) = sel else { return false };
                self.pulse_later(&items, i);
            }
            "u" => {
                let Some(Item::Next(n)) = items
                    .iter()
                    .find(|i| matches!(i, Item::Next(Next::RunsDry { .. })))
                    .cloned()
                else {
                    return false;
                };
                self.pulse_act(&n, &fleet, window, cx);
                return true;
            }
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
            _ => return false,
        }
        cx.notify();
        true
    }

    /// Escape in Pulse: closes the sheet, else the stacked focus. Escape
    /// is bound to Back, so the shell's interceptor calls this first;
    /// `false` when Pulse is not the view shown or has nothing to close.
    pub(super) fn pulse_escape(&mut self) -> bool {
        let shown = self.state.tabs.section() == crate::state::Section::Fleet
            && self.state.tabs.depth() == 1
            && self.pulse.on
            && !self.replay.lanes;
        if !shown {
            return false;
        }
        if self.pulse.sheet {
            self.pulse.sheet = false;
            true
        } else if self.pulse.focus_open {
            self.pulse.focus_open = false;
            true
        } else {
            false
        }
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

    /// Does what a Next up item offers.
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

    /// The start of the shown span.
    fn pulse_t0(&self, data: Option<&PulseData>, now: OffsetDateTime) -> OffsetDateTime {
        match (self.pulse.span, data) {
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
        }
    }

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
        let stacked = width < STACK_BELOW;
        if !stacked {
            self.pulse.focus_open = false;
        }
        let queue_w = if width >= QUEUE_WIDE_FROM {
            QUEUE_W
        } else {
            QUEUE_NARROW
        };
        let now = OffsetDateTime::now_utc();
        let data = self.pulse.data.clone();
        let t0 = self.pulse_t0(data.as_deref(), now);
        let (needs, next) = self.pulse_queue(fleet);
        let items: Vec<Item> = needs.iter().chain(next.iter()).cloned().collect();
        let keys: Vec<String> = items.iter().map(Item::key).collect();
        let sel = selected(&keys, self.pulse.sel.as_deref());
        let totals = pulse::totals(fleet, data.as_ref().map(|d| d.day.as_ref()), now);

        let toolbar = self.pulse_toolbar(look, cx);
        let strip = self.pulse_strip(fleet, &totals, now, look);
        let queue = self.pulse_queue_pane(
            fleet, &needs, &next, sel, &totals, stacked, queue_w, now, look, cx,
        );
        let focus = match sel.and_then(|i| items.get(i)) {
            Some(item) => {
                self.pulse_focus(item, fleet, data.as_deref(), t0, now, stacked, look, cx)
            }
            None => self.pulse_clear(fleet, &totals, data.as_deref(), t0, now, look),
        };
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
        let focus_pane = div()
            .id("pulse-focus")
            .flex_1()
            .min_w_0()
            .h_full()
            .relative()
            .child(
                div()
                    .id("pulse-focus-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.page_scroll)
                    .child(
                        div()
                            .flex()
                            .justify_center()
                            .w_full()
                            .px(look.px(if stacked { 20. } else { 40. }))
                            .pt(look.px(if stacked { 20. } else { 44. }))
                            .pb(look.px(48.))
                            .child(div().w_full().max_w(look.px(FOCUS_W)).child(focus)),
                    ),
            )
            .children(super::scrollbar::vertical(&self.page_scroll, look));
        let body = if stacked {
            let show_focus = self.pulse.focus_open && sel.is_some();
            div()
                .flex()
                .flex_1()
                .min_h_0()
                .w_full()
                .child(if show_focus {
                    focus_pane.into_any_element()
                } else {
                    queue.into_any_element()
                })
        } else {
            div()
                .flex()
                .flex_1()
                .min_h_0()
                .w_full()
                .child(
                    div()
                        .flex_shrink_0()
                        .w(look.px(queue_w))
                        .h_full()
                        .border_r(look.fixed(1.))
                        .border_color(look.c.border)
                        .child(queue),
                )
                .child(focus_pane)
        };
        div()
            .id("pulse")
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .text_size(look.text(13.))
            .child(measure)
            .child(
                div()
                    .px(look.px(16.))
                    .pt(look.px(8.))
                    .child(toolbar)
                    .children(self.render_reach_said(look)),
            )
            .child(strip)
            .child(body)
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
            .pb(look.px(8.))
            .child(self.fleet_view_switch(look, cx))
            .child(spans)
    }

    /// One line: the counts on the left, each account's 5-hour meter on
    /// the right, red with its empty time when it runs dry.
    fn pulse_strip(
        &self,
        fleet: &Fleet,
        t: &pulse::Totals,
        now: OffsetDateTime,
        look: &Look,
    ) -> gpui::Div {
        let at = |s: i64| OffsetDateTime::from_unix_timestamp(s).unwrap_or(now);
        let count = |n: usize, what: &str, color: Hsla| {
            div()
                .flex()
                .items_center()
                .gap(look.px(7.))
                .whitespace_nowrap()
                .child(kit::dot(look, color))
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(look.c.text)
                        .child(n.to_string()),
                )
                .child(div().text_color(look.c.text_muted).child(what.to_owned()))
        };
        let meters = rail_profiles(fleet).into_iter().filter_map(|p| {
            let h = fleet.room(&p)?;
            let left = h.five.map(|l| l.frac).or_else(|| h.left())?;
            let used = (1. - left).clamp(0., 1.);
            let d = fleet.drain(&p);
            let dry = d.and_then(|d| d.empty_at);
            let reset = h.five.and_then(|l| l.resets_at).or_else(|| h.resets());
            let hue = kit::profile_hue(&p);
            let tip = format!(
                "{}: {}% of the 5-hour window used{}",
                kit::profile_name(&p),
                (used * 100.).round(),
                reset
                    .map(|r| format!(", resets {}", kit::hm(at(r))))
                    .unwrap_or_default()
            );
            Some(
                div()
                    .id(SharedString::from(format!("pulse-meter-{p}")))
                    .flex()
                    .items_center()
                    .gap(look.px(8.))
                    .whitespace_nowrap()
                    .tooltip(super::tooltip::Tooltip::text(tip))
                    .child(kit::dot(look, hue))
                    .child(
                        div()
                            .text_color(look.c.text_muted)
                            .child(kit::profile_name(&p)),
                    )
                    .child(
                        div()
                            .w(look.px(64.))
                            .h(look.px(4.))
                            .rounded_full()
                            .bg(look.c.border)
                            .child(
                                div()
                                    .h_full()
                                    .rounded_full()
                                    .w(gpui::relative(used as f32))
                                    .bg(if dry.is_some() { look.c.err } else { hue }),
                            ),
                    )
                    .child(
                        div()
                            .font_family(kit::MONO)
                            .text_size(look.text(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(look.c.text)
                            .child(format!("{}%", (used * 100.).round())),
                    )
                    .child(
                        div()
                            .font_family(kit::MONO)
                            .text_size(look.text(12.))
                            .text_color(if dry.is_some() {
                                look.c.err
                            } else {
                                look.c.faint
                            })
                            .child(match (dry, reset) {
                                (Some(e), _) => format!("empty {}", kit::hm(at(e))),
                                (None, Some(r)) => kit::hm(at(r)),
                                _ => String::new(),
                            }),
                    ),
            )
        });
        div()
            .flex()
            .items_center()
            .gap(look.px(20.))
            .h(look.px(40.))
            .px(look.px(16.))
            .border_y(look.fixed(1.))
            .border_color(look.c.border)
            .overflow_hidden()
            .child(count(t.working, "working", look.c.info))
            .child(count(t.need_you, "need you", look.c.warn))
            .child(div().flex_1())
            .children(meters)
    }

    /// The queue pane: Needs you, then Next up, the key hint and the
    /// fleet row; the sheet over it when open.
    #[allow(clippy::too_many_arguments)]
    fn pulse_queue_pane(
        &self,
        fleet: &Fleet,
        needs: &[Item],
        next: &[Item],
        sel: Option<usize>,
        totals: &pulse::Totals,
        stacked: bool,
        width: f32,
        now: OffsetDateTime,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let still = self.pulse.still.unwrap_or(true);
        let group = |title: &str| {
            div()
                .px(look.px(8.))
                .pt(look.px(18.))
                .pb(look.px(6.))
                .text_size(look.text(12.))
                .text_color(look.c.faint)
                .child(title.to_owned())
        };
        let mut list = div().flex().flex_col().px(look.px(8.)).pb(look.px(12.));
        list = list.child(
            div()
                .flex()
                .items_center()
                .gap(look.px(8.))
                .px(look.px(8.))
                .pt(look.px(16.))
                .child(
                    div()
                        .text_size(look.text(14.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(look.c.text)
                        .child("Up next"),
                )
                .child(
                    div()
                        .text_color(look.c.faint)
                        .child((needs.len() + next.len()).to_string()),
                ),
        );
        if needs.is_empty() && next.is_empty() {
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(8.))
                    .px(look.px(8.))
                    .pt(look.px(18.))
                    .text_color(look.c.text_muted)
                    .child(kit::icon(look, "circle-check", 14., look.c.ok))
                    .child("All clear. Nothing needs you."),
            );
        }
        let mut i = 0;
        for (title, group_items) in [("Needs you", needs), ("Next up", next)] {
            if group_items.is_empty() {
                continue;
            }
            list = list.child(group(title));
            for item in group_items {
                list = list.child(self.pulse_queue_row(
                    item,
                    i,
                    sel == Some(i),
                    fleet,
                    now,
                    still,
                    look,
                    cx,
                ));
                i += 1;
            }
        }
        if let Some(e) = &self.pulse.error {
            list = list.child(
                div()
                    .px(look.px(8.))
                    .pt(look.px(14.))
                    .text_size(look.text(12.))
                    .text_color(look.c.faint)
                    .child(format!("Today’s lanes not read: {e}")),
            );
        }
        let hint = div()
            .flex()
            .items_center()
            .gap(look.px(6.))
            .px(look.px(16.))
            .py(look.px(10.))
            .text_size(look.text(11.5))
            .text_color(look.c.faint)
            .child(kit::kbd(look, "J"))
            .child(kit::kbd(look, "K"))
            .child("move")
            .child(div().w(look.px(4.)))
            .child(kit::kbd(look, "↵"))
            .child(if stacked { "open" } else { "act" })
            .child(div().w(look.px(4.)))
            .child(kit::kbd(look, "S"))
            .child("later");
        let fleet_row =
            div()
                .id("pulse-fleet-row")
                .flex()
                .items_center()
                .gap(look.px(8.))
                .h(look.px(44.))
                .px(look.px(16.))
                .border_t(look.fixed(1.))
                .border_color(look.c.border)
                .cursor_pointer()
                .hover(|h| h.bg(look.c.hover))
                .tooltip(super::tooltip::Tooltip::text(
                    "The fleet, memory and tonight (F)",
                ))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.pulse.sheet = !this.pulse.sheet;
                    cx.notify();
                }))
                .child(kit::dot(look, look.c.info))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(look.px(4.))
                        .whitespace_nowrap()
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(look.c.text)
                                .child(totals.working.to_string()),
                        )
                        .child(div().text_color(look.c.text_muted).child("working"))
                        .when(totals.subs > 0, |d| {
                            d.child(div().text_color(look.c.faint).child("·"))
                                .child(
                                    div()
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(look.c.text)
                                        .child(totals.subs.to_string()),
                                )
                                .child(div().text_color(look.c.text_muted).child(
                                    if totals.subs == 1 {
                                        "sub-agent"
                                    } else {
                                        "sub-agents"
                                    },
                                ))
                        }),
                )
                .child(div().flex_1())
                .children(fleet.rows.iter().filter(|r| r.working()).take(6).map(|r| {
                    kit::dot(
                        look,
                        r.profile
                            .as_deref()
                            .map_or(look.c.text_muted, kit::profile_hue),
                    )
                }))
                .child(kit::icon(
                    look,
                    if self.pulse.sheet {
                        "chevron-down"
                    } else {
                        "chevron-right"
                    },
                    13.,
                    look.c.faint,
                ));
        let pane_w: gpui::Length = if stacked {
            gpui::relative(1.).into()
        } else {
            look.px(width).into()
        };
        let pane = div()
            .id("pulse-queue")
            .relative()
            .flex()
            .flex_col()
            .h_full()
            .w(pane_w)
            .bg(look.c.canvas)
            .child(
                div()
                    .id("pulse-queue-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.pulse.queue_scroll)
                    .child(list),
            )
            .child(hint)
            .child(fleet_row);
        if self.pulse.sheet {
            pane.child(self.pulse_sheet(fleet, totals, now, look, cx))
        } else {
            pane
        }
    }

    /// One queue row: an icon, the title, one line of reason, its age and
    /// its account's dot.
    #[allow(clippy::too_many_arguments)]
    fn pulse_queue_row(
        &self,
        item: &Item,
        i: usize,
        on: bool,
        fleet: &Fleet,
        now: OffsetDateTime,
        still: bool,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let at = |s: i64| OffsetDateTime::from_unix_timestamp(s).unwrap_or(now);
        let (icon, color, title, reason, age, profiles): (
            &str,
            Hsla,
            String,
            String,
            String,
            Vec<String>,
        ) = match item {
            Item::Wait(r) => {
                let (_, icon, asked, since) = need_words(r, now);
                let plan = matches!(r.state, State::Waiting { plan: true, .. });
                let reason = if plan {
                    match r.plan.as_ref().map(|p| p.steps.len() + p.more) {
                        Some(n) if n > 0 => format!("A {n}-step plan is waiting for your approval"),
                        _ => "A plan is waiting for your approval".to_owned(),
                    }
                } else {
                    asked.unwrap_or_else(|| "It is waiting on you".to_owned())
                };
                (
                    icon,
                    look.c.warn,
                    kit::title(&r.title),
                    reason,
                    since,
                    r.profile.iter().cloned().collect(),
                )
            }
            Item::Next(Next::RunsDry {
                profile,
                empty_at,
                reset,
                sessions,
                ..
            }) => (
                "gauge",
                look.c.err,
                format!(
                    "{} runs dry about {}",
                    kit::profile_name(profile),
                    kit::hm(at(*empty_at))
                ),
                format!(
                    "{} will stop, {} before its reset",
                    plural(sessions.len(), "session", "sessions"),
                    kit::span(at(*reset) - at(*empty_at))
                ),
                format!("in {}", kit::span(at(*empty_at) - now)),
                vec![profile.clone()],
            ),
            Item::Next(Next::Unverified {
                session,
                title,
                claim,
                at,
                ..
            }) => (
                "shield-question",
                look.c.warn,
                kit::title(title),
                format!("Unverified: “{claim}”"),
                kit::span(now - *at),
                fleet
                    .row(session)
                    .and_then(|r| r.profile.clone())
                    .into_iter()
                    .collect(),
            ),
            Item::Next(Next::Destructive {
                session,
                title,
                command,
                at,
                ..
            }) => (
                "triangle-alert",
                look.c.err,
                kit::command_words(command.strip_prefix("Bash: ").unwrap_or(command)),
                format!(
                    "{} · {}",
                    title.as_deref().map_or("A session".into(), kit::title),
                    kit::hm(*at)
                ),
                kit::span(now - *at),
                fleet
                    .row(session)
                    .and_then(|r| r.profile.clone())
                    .into_iter()
                    .collect(),
            ),
            Item::Next(Next::Correction {
                session,
                said,
                at,
                seen,
                ..
            }) => (
                "lightbulb",
                look.c.info,
                format!("Keep a rule: “{}”", short(said, 60)),
                format!(
                    "You corrected it {}",
                    if *seen > 1 {
                        format!("{seen} times")
                    } else {
                        "once".to_owned()
                    }
                ),
                kit::span(now - *at),
                fleet
                    .row(session)
                    .and_then(|r| r.profile.clone())
                    .into_iter()
                    .collect(),
            ),
        };
        let waiting = matches!(item, Item::Wait(_));
        let glyph = div()
            .relative()
            .pt(look.px(2.))
            .child(kit::icon(look, icon, 15., color))
            .when(waiting, |d| {
                let dot = div()
                    .absolute()
                    .top(look.px(0.))
                    .right(look.px(-3.))
                    .size(look.px(6.))
                    .rounded_full()
                    .bg(look.c.warn);
                let dot: gpui::AnyElement = if still {
                    dot.into_any_element()
                } else {
                    dot.with_animation(
                        SharedString::from(format!("pulse-dot-{}", item.key())),
                        gpui::Animation::new(std::time::Duration::from_millis(1600)).repeat(),
                        |d, t| {
                            d.opacity(0.35 + 0.65 * (1. - (t * std::f32::consts::TAU).cos()) / 2.)
                        },
                    )
                    .into_any_element()
                };
                d.child(dot)
            });
        let key = item.key();
        div()
            .id(("pulse-q", i))
            .relative()
            .flex()
            .items_start()
            .gap(look.px(12.))
            .min_h(look.px(60.))
            .px(look.px(12.))
            .py(look.px(11.))
            .rounded(look.px(8.))
            .cursor_pointer()
            .when(on, |d| d.bg(look.c.selected))
            .when(!on, |d| d.hover(|h| h.bg(look.c.hover)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.pulse_select(key.clone());
                cx.notify();
            }))
            .when(on, |d| {
                d.child(
                    div()
                        .absolute()
                        .left(look.px(0.))
                        .top(look.px(10.))
                        .bottom(look.px(10.))
                        .w(look.fixed(2.))
                        .rounded_full()
                        .bg(look.c.accent),
                )
            })
            .child(glyph)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(look.px(3.))
                    .child(
                        div()
                            .truncate()
                            .text_size(look.text(14.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(look.c.text)
                            .child(title),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(look.text(12.5))
                            .text_color(look.c.text_muted)
                            .child(reason),
                    ),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .items_end()
                    .gap(look.px(7.))
                    .pt(look.px(1.))
                    .child(
                        div()
                            .font_family(kit::MONO)
                            .text_size(look.text(11.5))
                            .text_color(look.c.faint)
                            .child(age),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(look.px(3.))
                            .children(profiles.iter().map(|p| kit::dot(look, kit::profile_hue(p)))),
                    ),
            )
            .into_any_element()
    }

    /// The selected item, alone.
    #[allow(clippy::too_many_arguments)]
    fn pulse_focus(
        &self,
        item: &Item,
        fleet: &Fleet,
        data: Option<&PulseData>,
        t0: OffsetDateTime,
        now: OffsetDateTime,
        stacked: bool,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let back = stacked.then(|| {
            div()
                .id("pulse-back")
                .flex()
                .items_center()
                .gap(look.px(4.))
                .mb(look.px(16.))
                .text_size(look.text(12.5))
                .text_color(look.c.text_muted)
                .cursor_pointer()
                .hover(|h| h.text_color(look.c.text))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.pulse.focus_open = false;
                    cx.notify();
                }))
                .child(kit::icon(look, "chevron-left", 13., look.c.text_muted))
                .child("Up next")
                .child(kit::kbd(look, "esc"))
        });
        let body = match item {
            Item::Wait(r) => self.pulse_focus_wait(r, data, t0, now, look, cx),
            Item::Next(n @ Next::RunsDry { .. }) => self.pulse_focus_dry(n, fleet, now, look, cx),
            Item::Next(n) => self.pulse_focus_next(n, fleet, now, look, cx),
        };
        div()
            .flex()
            .flex_col()
            .children(back)
            .child(body)
            .into_any_element()
    }

    /// A waiting session: where it is, what it asked (or its plan), the
    /// ways in, and what it did before asking.
    fn pulse_focus_wait(
        &self,
        r: &Row,
        data: Option<&PulseData>,
        t0: OffsetDateTime,
        now: OffsetDateTime,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let (what, icon, asked, since) = need_words(r, now);
        let plan = matches!(r.state, State::Waiting { plan: true, .. });
        let crumb = crumb(look, r.profile.as_deref(), r.project.as_deref())
            .child(kit::icon(look, icon, 13., look.c.warn))
            .child(
                div()
                    .text_color(look.c.text_muted)
                    .child(format!("{what} · {since}")),
            );
        // One line of facts, each with its detail on hover.
        let lines = r.total_lines();
        let pr = r.pulls.iter().rev().find(|(_, d)| *d == "opened").cloned();
        let mut facts = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x(look.px(12.))
            .gap_y(look.px(4.))
            .text_size(look.text(12.5))
            .text_color(look.c.text_muted);
        if let Some(b) = &r.branch {
            facts = facts.child(
                div()
                    .font_family(kit::MONO)
                    .text_size(look.text(12.))
                    .child(kit::middle(b, 40)),
            );
        }
        if let Some((url, _)) = &pr {
            facts = facts.child(
                div()
                    .id("pulse-fact-pr")
                    .flex()
                    .items_center()
                    .gap(look.px(5.))
                    .tooltip(super::tooltip::Tooltip::text(url.clone()))
                    .child(kit::icon(look, "git-pull-request", 13., look.c.faint))
                    .child(format!(
                        "PR #{}",
                        super::sidebar::pr_number(url).unwrap_or("?")
                    )),
            );
        }
        if let Some(l) = lines {
            facts = facts.child(
                div()
                    .id("pulse-fact-lines")
                    .flex()
                    .items_center()
                    .gap(look.px(5.))
                    .tooltip(super::tooltip::Tooltip::text(format!(
                        "{} changed",
                        plural(l.files as usize, "file", "files")
                    )))
                    .children(kit::diff_counts(look, l.added, l.removed)),
            );
        }
        if r.claims.verified + r.claims.unverified > 0 {
            let ok = r.claims.unverified == 0;
            let tip = match &r.claims.first_unverified {
                Some((claim, why)) => format!("Unverified: “{claim}” ({why})"),
                None => format!(
                    "{} checked against what ran",
                    plural(r.claims.verified as usize, "claim", "claims")
                ),
            };
            facts = facts.child(
                div()
                    .id("pulse-fact-claims")
                    .flex()
                    .items_center()
                    .gap(look.px(5.))
                    .tooltip(super::tooltip::Tooltip::text(tip))
                    .child(kit::icon(
                        look,
                        if ok {
                            "circle-check"
                        } else {
                            "shield-question"
                        },
                        13.,
                        if ok { look.c.ok } else { look.c.warn },
                    ))
                    .child(if ok {
                        "claims checked".to_owned()
                    } else {
                        plural(
                            r.claims.unverified as usize,
                            "claim unverified",
                            "claims unverified",
                        )
                    }),
            );
        }
        if !r.rules.is_empty() {
            let tip = r
                .rules
                .iter()
                .map(|s| s.title.clone())
                .collect::<Vec<_>>()
                .join("\n");
            facts = facts.child(
                div()
                    .id("pulse-fact-rules")
                    .flex()
                    .items_center()
                    .gap(look.px(6.))
                    .tooltip(super::tooltip::Tooltip::text(tip))
                    .child(ring_glyph(look))
                    .child(plural(r.rules.len(), "kept rule", "kept rules")),
            );
        }
        // The question, or the plan.
        let ask: gpui::AnyElement = match (plan, r.plan.as_ref()) {
            (true, Some(p)) if !p.steps.is_empty() => div()
                .flex()
                .flex_col()
                .gap(look.px(14.))
                .children(p.steps.iter().enumerate().map(|(n, s)| {
                    div()
                        .flex()
                        .items_start()
                        .gap(look.px(14.))
                        .child(
                            div()
                                .w(look.px(16.))
                                .flex_shrink_0()
                                .font_family(kit::MONO)
                                .text_size(look.text(12.))
                                .text_color(look.c.faint)
                                .pt(look.px(2.))
                                .child((n + 1).to_string()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(look.text(15.))
                                .line_height(look.text(22.))
                                .text_color(look.c.text)
                                .child(s.clone()),
                        )
                }))
                .when(p.more > 0, |d| {
                    d.child(
                        div()
                            .pl(look.px(30.))
                            .text_size(look.text(12.5))
                            .text_color(look.c.faint)
                            .child(format!("and {} more in the thread", p.more)),
                    )
                })
                .into_any_element(),
            (true, Some(p)) if p.text.is_some() => div()
                .text_size(look.text(14.))
                .line_height(look.text(21.))
                .text_color(look.c.text)
                .child(short(p.text.as_deref().unwrap_or_default(), 900))
                .into_any_element(),
            _ => div()
                .text_size(look.text(18.))
                .line_height(look.text(26.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(look.c.text)
                .child(
                    asked.map_or_else(|| "It is waiting on you.".to_owned(), |q| format!("“{q}”")),
                )
                .into_any_element(),
        };
        let asked_at = match r.state {
            State::Waiting { at, .. } => Some(format!(
                "{} at {}",
                if plan { "Plan written" } else { "Asked" },
                kit::hm(at)
            )),
            State::Stopped { at, .. } => Some(format!("Stopped at {}", kit::hm(at))),
            _ => None,
        };
        // The ways in.
        let id = r.id.clone();
        let (row, row2, row3) = (r.clone(), r.clone(), r.clone());
        let key = Item::Wait(Box::new(r.clone())).key();
        let actions = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(look.px(6.))
            .child(
                kit::button_key(
                    look,
                    SharedString::from(format!("focus-open-{id}")),
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
                        SharedString::from(format!("focus-t3-{id}")),
                        Some("arrow-up-right"),
                        "T3 Code",
                        "T",
                        Kind::Ghost,
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
                    SharedString::from(format!("focus-term-{id}")),
                    Some("terminal"),
                    "Terminal",
                    "⌘T",
                    Kind::Ghost,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(f) = this.loaded.as_ref().and_then(|l| l.fleet.clone()) {
                        this.pulse_terminal(&row3, &f);
                    }
                    cx.notify();
                })),
            )
            .child(self.later_button(look, key, cx));
        // What it did before asking: its activity over the span and its
        // last calls.
        let wave = self.wave_of(r, data, t0, now, Some(look.c.warn));
        let lk = look.clone();
        let strip = wave.map(|w| {
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| paint_wave(window, bounds, &w, t0, now, &lk, true),
            )
            .w_full()
            .h(look.px(36.))
        });
        let calls: Vec<hippo_view::fleet::tail::Activity> = r
            .turn
            .as_ref()
            .or_else(|| data.and_then(|d| d.turns.get(&r.id)))
            .map(|t| t.calls.clone())
            .unwrap_or_default();
        let shown = if self.pulse.all_calls {
            calls.len()
        } else {
            calls.len().min(CALLS_SHOWN)
        };
        let call_rows = calls.iter().take(shown).rev().map(|c| {
            div()
                .flex()
                .items_center()
                .gap(look.px(14.))
                .h(look.px(28.))
                .child(
                    div()
                        .w(look.px(40.))
                        .flex_shrink_0()
                        .font_family(kit::MONO)
                        .text_size(look.text(12.))
                        .text_color(look.c.faint)
                        .child(kit::hm(c.at)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(kit::tool_line(look, c, 1, None, false)),
                )
        });
        let more = calls.len().saturating_sub(shown);
        let activity = div()
            .flex()
            .flex_col()
            .gap(look.px(6.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .text_size(look.text(12.))
                    .text_color(look.c.faint)
                    .child(format!("Activity · {}", self.pulse.span.words()))
                    .child(div().flex_1())
                    .child(
                        div()
                            .font_family(kit::MONO)
                            .text_color(look.c.warn)
                            .child(format!("waiting {since}")),
                    ),
            )
            .children(strip)
            .child(div().h(look.px(6.)))
            .children(call_rows)
            .when(more > 0, |d| {
                d.child(
                    div()
                        .id("pulse-all-calls")
                        .flex()
                        .items_center()
                        .gap(look.px(5.))
                        .pl(look.px(54.))
                        .pt(look.px(4.))
                        .text_size(look.text(12.5))
                        .text_color(look.c.text_muted)
                        .cursor_pointer()
                        .hover(|h| h.text_color(look.c.text))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.pulse.all_calls = true;
                            cx.notify();
                        }))
                        .child(kit::icon(look, "chevron-down", 12., look.c.faint))
                        .child(format!("Show all {} calls", calls.len())),
                )
            });
        div()
            .flex()
            .flex_col()
            .child(crumb)
            .child(focus_title(look, &kit::title(&r.title)))
            .child(facts)
            .child(div().h(look.px(36.)))
            .child(ask)
            .children(asked_at.map(|a| {
                div()
                    .pt(look.px(10.))
                    .text_size(look.text(12.5))
                    .text_color(look.c.faint)
                    .child(a)
            }))
            .child(div().h(look.px(24.)))
            .child(actions)
            .child(div().h(look.px(44.)))
            .child(activity)
            .into_any_element()
    }

    /// An account that runs dry: its drain, the sessions that will stop
    /// and the account to resume them on.
    fn pulse_focus_dry(
        &self,
        n: &Next,
        fleet: &Fleet,
        now: OffsetDateTime,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let Next::RunsDry {
            profile,
            empty_at,
            reset,
            sessions,
            to,
        } = n
        else {
            return div().into_any_element();
        };
        let at = |s: i64| OffsetDateTime::from_unix_timestamp(s).unwrap_or(now);
        let d = fleet.drain(profile);
        let hue = kit::profile_hue(profile);
        let crumb = crumb(look, Some(profile), Some("5-hour window"))
            .child(kit::icon(look, "gauge", 13., look.c.err))
            .child(
                div()
                    .text_color(look.c.err)
                    .child(format!("Runs dry in {}", kit::span(at(*empty_at) - now))),
            );
        let facts = div()
            .flex()
            .flex_wrap()
            .gap(look.px(12.))
            .text_size(look.text(12.5))
            .text_color(look.c.text_muted)
            .children(d.map(|d| format!("{}% used", (d.used * 100.).round())))
            .children(d.map(|d| format!("{}% an hour", (d.burn * 100.).round())))
            .child(format!("resets {}", kit::hm(at(*reset))));
        let chart = d.map(|d| {
            div()
                .flex()
                .flex_col()
                .gap(look.px(6.))
                .child(
                    div()
                        .flex()
                        .text_size(look.text(12.))
                        .text_color(look.c.faint)
                        .child("What is left of the window")
                        .child(div().flex_1())
                        .child(
                            div()
                                .font_family(kit::MONO)
                                .text_color(look.c.err)
                                .child(format!("empty {}", kit::hm(at(*empty_at)))),
                        ),
                )
                .child(drain_chart(look, d, hue, None, 120.))
                .child(
                    div()
                        .flex()
                        .font_family(kit::MONO)
                        .text_size(look.text(11.))
                        .text_color(look.c.faint)
                        .child(kit::hm(at(d.start)))
                        .child(div().flex_1())
                        .child(format!("reset {}", kit::hm(at(d.end)))),
                )
        });
        let stop = sessions.iter().filter_map(|id| fleet.row(id)).map(|r| {
            div()
                .flex()
                .items_center()
                .gap(look.px(10.))
                .h(look.px(30.))
                .child(kit::dot(look, hue))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(look.c.text)
                        .child(kit::title(&r.title)),
                )
                .children(r.call.as_ref().map(|c| {
                    div()
                        .flex_shrink_0()
                        .max_w(look.px(260.))
                        .child(kit::tool_line(look, c, 1, None, true))
                }))
        });
        let room = to.as_ref().and_then(|t| {
            let h = fleet.room(t)?;
            let left = h.five.map(|l| l.frac).or_else(|| h.left())?;
            let reset = h.five.and_then(|l| l.resets_at).or_else(|| h.resets());
            Some(format!(
                "{} has {}% left{}. Resumed there, they carry on where they are.",
                kit::profile_name(t),
                (left * 100.).round(),
                reset
                    .map(|r| format!(" until {}", kit::hm(at(r))))
                    .unwrap_or_default()
            ))
        });
        let n2 = n.clone();
        let key = Item::Next(n.clone()).key();
        let actions = div()
            .flex()
            .items_center()
            .gap(look.px(6.))
            .when_some(to.clone(), |d, t| {
                d.child(
                    kit::button_key(
                        look,
                        "focus-resume",
                        format!("Resume them on {}", kit::profile_name(&t)),
                        "U",
                        Kind::Primary,
                    )
                    .tooltip(super::tooltip::Tooltip::text(
                        "Copies their resume commands for that account",
                    ))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(f) = this.loaded.as_ref().and_then(|l| l.fleet.clone()) {
                            this.pulse_act(&n2, &f, window, cx);
                        }
                    })),
                )
            })
            .child(self.later_button(look, key, cx));
        div()
            .flex()
            .flex_col()
            .child(crumb)
            .child(focus_title(
                look,
                &format!(
                    "{} runs dry about {}",
                    kit::profile_name(profile),
                    kit::hm(at(*empty_at))
                ),
            ))
            .child(facts)
            .child(div().h(look.px(32.)))
            .children(chart)
            .child(div().h(look.px(28.)))
            .child(
                div()
                    .pb(look.px(6.))
                    .text_size(look.text(12.))
                    .text_color(look.c.faint)
                    .child(format!("Will stop at {}", kit::hm(at(*empty_at)))),
            )
            .children(stop)
            .child(div().h(look.px(24.)))
            .children(room.map(|t| {
                div()
                    .pb(look.px(16.))
                    .text_size(look.text(14.))
                    .text_color(look.c.text)
                    .child(t)
            }))
            .child(actions)
            .into_any_element()
    }

    /// An unverified claim, a destructive incident or a rule candidate:
    /// the one fact, its evidence and the one action.
    fn pulse_focus_next(
        &self,
        n: &Next,
        fleet: &Fleet,
        now: OffsetDateTime,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let (crumb_el, title, fact, evidence, action): (
            gpui::Div,
            String,
            gpui::AnyElement,
            Option<gpui::AnyElement>,
            &str,
        ) = match n {
            Next::Unverified {
                session,
                title,
                claim,
                why,
                at,
            } => {
                let r = fleet.row(session);
                (
                    crumb(
                        look,
                        r.and_then(|r| r.profile.as_deref()),
                        r.and_then(|r| r.project.as_deref()),
                    )
                    .child(kit::icon(look, "shield-question", 13., look.c.warn))
                    .child(
                        div()
                            .text_color(look.c.text_muted)
                            .child(format!("Unverified claim · finished {}", kit::hm(*at))),
                    ),
                    kit::title(title),
                    div()
                        .text_size(look.text(18.))
                        .line_height(look.text(26.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(look.c.text)
                        .child(format!("“{claim}”"))
                        .into_any_element(),
                    Some(
                        div()
                            .flex()
                            .items_start()
                            .gap(look.px(8.))
                            .text_size(look.text(13.))
                            .text_color(look.c.text_muted)
                            .child(kit::icon(look, "info", 14., look.c.faint))
                            .child(div().flex_1().min_w_0().child(why.clone()))
                            .into_any_element(),
                    ),
                    "Check",
                )
            }
            Next::Destructive {
                session,
                title,
                command,
                at,
                ..
            } => {
                let r = fleet.row(session);
                (
                    crumb(
                        look,
                        r.and_then(|r| r.profile.as_deref()),
                        r.and_then(|r| r.project.as_deref()),
                    )
                    .child(kit::icon(look, "triangle-alert", 13., look.c.err))
                    .child(
                        div()
                            .text_color(look.c.text_muted)
                            .child(format!("Destructive · {}", kit::hm(*at))),
                    ),
                    format!(
                        "A destructive command in {}",
                        title.as_deref().map_or("a session".into(), kit::title)
                    ),
                    div()
                        .p(look.px(14.))
                        .rounded(look.px(8.))
                        .bg(look.c.inset)
                        .border(look.fixed(1.))
                        .border_color(look.c.border)
                        .font_family(kit::MONO)
                        .text_size(look.text(13.))
                        .text_color(look.c.text)
                        .child(short(command.strip_prefix("Bash: ").unwrap_or(command), 400))
                        .into_any_element(),
                    Some(
                        div()
                            .text_size(look.text(13.))
                            .text_color(look.c.text_muted)
                            .child(format!(
                                "Harm followed it {} ago. The incident holds what it changed and what you said.",
                                kit::span(now - *at)
                            ))
                            .into_any_element(),
                    ),
                    "Open incident",
                )
            }
            Next::Correction {
                session,
                said,
                at,
                seen,
                ..
            } => {
                let r = fleet.row(session);
                (
                    crumb(
                        look,
                        r.and_then(|r| r.profile.as_deref()),
                        r.and_then(|r| r.project.as_deref()),
                    )
                    .child(kit::icon(look, "lightbulb", 13., look.c.info))
                    .child(
                        div()
                            .text_color(look.c.text_muted)
                            .child("Rule candidate"),
                    ),
                    "Keep a rule".to_owned(),
                    div()
                        .text_size(look.text(18.))
                        .line_height(look.text(26.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(look.c.text)
                        .child(format!("“{said}”"))
                        .into_any_element(),
                    Some(
                        div()
                            .flex()
                            .items_center()
                            .gap(look.px(8.))
                            .text_size(look.text(13.))
                            .text_color(look.c.text_muted)
                            .child(diamond_glyph(look))
                            .child(format!(
                                "You said it {} today, last at {}. A pending lesson holds it; keeping it makes it a rule sessions start with.",
                                if *seen > 1 {
                                    format!("{seen} times")
                                } else {
                                    "once".to_owned()
                                },
                                kit::hm(*at)
                            ))
                            .into_any_element(),
                    ),
                    "Keep now",
                )
            }
            Next::RunsDry { .. } => return div().into_any_element(),
        };
        let n2 = n.clone();
        let key = Item::Next(n.clone()).key();
        let actions = div()
            .flex()
            .items_center()
            .gap(look.px(6.))
            .child(
                kit::button_key(look, "focus-act", action, "↵", Kind::Primary).on_click(
                    cx.listener(move |this, _, window, cx| {
                        if let Some(f) = this.loaded.as_ref().and_then(|l| l.fleet.clone()) {
                            this.pulse_act(&n2, &f, window, cx);
                        }
                    }),
                ),
            )
            .child(self.later_button(look, key, cx));
        div()
            .flex()
            .flex_col()
            .child(crumb_el)
            .child(focus_title(look, &title))
            .child(div().h(look.px(28.)))
            .child(fact)
            .children(evidence.map(|e| div().pt(look.px(16.)).child(e)))
            .child(div().h(look.px(28.)))
            .child(actions)
            .into_any_element()
    }

    /// Later: puts the item after the others in its group.
    fn later_button(
        &self,
        look: &Look,
        key: String,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        kit::button_key(look, "focus-later", "Later", "S", Kind::Ghost).on_click(cx.listener(
            move |this, _, _, cx| {
                if let Some(f) = this.loaded.as_ref().and_then(|l| l.fleet.clone()) {
                    let (items, _) = this.pulse_items(&f);
                    if let Some(i) = items.iter().position(|it| it.key() == key) {
                        this.pulse_later(&items, i);
                    }
                }
                cx.notify();
            },
        ))
    }

    /// Nothing in the queue: the fleet's histogram and today's totals.
    fn pulse_clear(
        &self,
        fleet: &Fleet,
        t: &pulse::Totals,
        data: Option<&PulseData>,
        t0: OffsetDateTime,
        now: OffsetDateTime,
        look: &Look,
    ) -> gpui::AnyElement {
        let order = rail_profiles(fleet);
        let hist = data.map(|d| pulse::histogram(&d.day, t0, now, &order));
        let stat = |big: String, what: String| {
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(look.px(4.))
                .child(
                    div()
                        .text_size(look.text(17.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(look.c.text)
                        .child(big),
                )
                .child(
                    div()
                        .text_size(look.text(12.))
                        .text_color(look.c.faint)
                        .child(what),
                )
        };
        let line = if t.working > 0 {
            format!(
                "Nothing needs you. {} {}; Pulse raises the next thing as it happens.",
                plural(t.working, "session is", "sessions are"),
                if t.subs > 0 {
                    format!("working with {}", plural(t.subs, "sub-agent", "sub-agents"))
                } else {
                    "working".to_owned()
                }
            )
        } else {
            "Nothing needs you and nothing is working.".to_owned()
        };
        div()
            .flex()
            .flex_col()
            .items_center()
            .pt(look.px(40.))
            .gap(look.px(10.))
            .child(kit::icon(look, "circle-check", 30., look.c.ok))
            .child(
                div()
                    .text_size(look.text(20.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(look.c.text)
                    .child("All clear"),
            )
            .child(
                div()
                    .max_w(look.px(440.))
                    .text_color(look.c.text_muted)
                    .child(line),
            )
            .child(div().h(look.px(24.)))
            .children(hist.map(|h| histogram_el(look, h, t0, now, 110.)))
            .child(div().h(look.px(20.)))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .justify_center()
                    .gap(look.px(44.))
                    .child(stat(
                        t.calls_lately.to_string(),
                        "calls · last 5 min".into(),
                    ))
                    .child(stat(kit::span(t.work), "agent work today".into()))
                    .when(t.added + t.removed > 0, |d| {
                        d.child(stat(
                            format!("+{} −{}", thousands(t.added), thousands(t.removed)),
                            "lines today".into(),
                        ))
                    })
                    .children(
                        data.and_then(|d| d.tonight.as_ref())
                            .map(|n| stat(n.at.clone(), "tonight’s pass".into())),
                    ),
            )
            .into_any_element()
    }

    /// The sheet over the queue: the fleet's lists, Memory today and
    /// Tonight.
    fn pulse_sheet(
        &self,
        fleet: &Fleet,
        totals: &pulse::Totals,
        now: OffsetDateTime,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let data = self.pulse.data.clone();
        let shelves = crate::shelving::Shelves::of(fleet, &self.marks());
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
        let tab = self.pulse.sheet_tab;
        let tabs = [
            (SheetTab::Working, "Working", working.len()),
            (SheetTab::Review, "To review", review.len()),
            (SheetTab::Quiet, "Quiet", quiet.len()),
            (SheetTab::Settled, "Settled", settled.len()),
        ];
        let tab_row = div()
            .flex()
            .items_end()
            .gap(look.px(12.))
            .px(look.px(16.))
            .overflow_hidden()
            .border_b(look.fixed(1.))
            .border_color(look.c.border)
            .children(tabs.into_iter().map(|(t, label, n)| {
                let on = t == tab;
                div()
                    .id(SharedString::from(format!("pulse-tab-{label}")))
                    .flex()
                    .items_center()
                    .gap(look.px(5.))
                    .flex_shrink_0()
                    .pb(look.px(8.))
                    .border_b(look.fixed(2.))
                    .border_color(if on {
                        look.c.text
                    } else {
                        gpui::transparent_black()
                    })
                    .text_size(look.text(12.))
                    .cursor_pointer()
                    .text_color(if on { look.c.text } else { look.c.text_muted })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.pulse.sheet_tab = t;
                        cx.notify();
                    }))
                    .child(label)
                    .child(div().text_color(look.c.faint).child(n.to_string()))
            }));
        let rows: Vec<&Row> = match tab {
            SheetTab::Working => working,
            SheetTab::Review => review,
            SheetTab::Quiet => quiet,
            SheetTab::Settled => settled,
        };
        let t0 = now - SHEET_SPAN;
        let mut list = div().flex().flex_col().px(look.px(8.)).py(look.px(6.));
        if rows.is_empty() {
            list = list.child(
                div()
                    .px(look.px(8.))
                    .py(look.px(10.))
                    .text_color(look.c.faint)
                    .child("None."),
            );
        }
        for r in rows.iter().take(SHEET_ROWS) {
            let end = if r.working() {
                Some(look.c.info)
            } else if r.pulls.iter().any(|(_, d)| *d == "merged") {
                Some(look.c.created)
            } else {
                None
            };
            let wave = self.wave_of(r, data.as_deref(), t0, now, end);
            let lk = look.clone();
            let wave_el = canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    if let Some(w) = &wave {
                        paint_wave(window, bounds, w, t0, now, &lk, true);
                    }
                },
            )
            .flex_shrink_0()
            .w(look.px(88.))
            .h(look.px(22.));
            let subs = r.live_subs.len();
            let second = match &r.call {
                Some(c) => kit::tool_line(look, c, 1, Some(kit::span(now - c.at)), true),
                None => div()
                    .truncate()
                    .text_size(look.text(12.))
                    .text_color(look.c.faint)
                    .child(r.activity.clone().unwrap_or_else(|| match r.last {
                        Some(t) => format!("last {}", kit::hm(t)),
                        None => String::new(),
                    })),
            };
            let id = r.id.clone();
            list = list.child(
                div()
                    .id(SharedString::from(format!("pulse-sheet-{id}")))
                    .flex()
                    .items_center()
                    .gap(look.px(10.))
                    .px(look.px(8.))
                    .py(look.px(7.))
                    .rounded(look.px(6.))
                    .cursor_pointer()
                    .hover(|h| h.bg(look.c.hover))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_xray(&id, window, cx);
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(look.px(2.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(look.px(7.))
                                    .min_w_0()
                                    .child(kit::dot(
                                        look,
                                        r.profile
                                            .as_deref()
                                            .map_or(look.c.text_muted, kit::profile_hue),
                                    ))
                                    .child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .text_color(look.c.text)
                                            .child(kit::title(&r.title)),
                                    )
                                    .when(subs > 0, |d| {
                                        d.child(
                                            div()
                                                .flex_shrink_0()
                                                .text_size(look.text(11.5))
                                                .text_color(look.c.faint)
                                                .child(format!("+{subs}")),
                                        )
                                    }),
                            )
                            .child(div().pl(look.px(13.)).min_w_0().child(second)),
                    )
                    .child(wave_el),
            );
        }
        if rows.len() > SHEET_ROWS {
            list = list.child(
                div()
                    .px(look.px(8.))
                    .py(look.px(6.))
                    .text_size(look.text(12.))
                    .text_color(look.c.faint)
                    .child(format!("{} more in Rows", rows.len() - SHEET_ROWS)),
            );
        }
        let section = |icon: &str, title: &str, right: String| {
            div()
                .flex()
                .items_center()
                .gap(look.px(8.))
                .h(look.px(32.))
                .text_size(look.text(12.5))
                .text_color(look.c.text_muted)
                .child(kit::icon(look, icon, 13., look.c.faint))
                .child(title.to_owned())
                .child(div().flex_1())
                .child(div().text_color(look.c.faint).child(right))
        };
        let served = pulse::served(&fleet.rows);
        let mut memory = div()
            .flex()
            .flex_col()
            .gap(look.px(6.))
            .px(look.px(16.))
            .py(look.px(8.))
            .border_t(look.fixed(1.))
            .border_color(look.c.border)
            .child(section("brain", "Memory today", String::new()))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(8.))
                    .text_size(look.text(12.5))
                    .text_color(look.c.text)
                    .child(ring_glyph(look))
                    .child(format!(
                        "{} started with a kept rule",
                        plural(served, "session", "sessions")
                    )),
            );
        if let Some(d) = &data {
            for c in d.corrected.iter().take(3) {
                let fate = match c.fate {
                    Fate::Candidate { seen, .. } => format!("rule candidate · seen {seen}×"),
                    Fate::Tonight => "read tonight".to_owned(),
                    Fate::Read => "read by the last pass".to_owned(),
                };
                memory = memory.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(look.px(8.))
                        .text_size(look.text(12.5))
                        .child(diamond_glyph(look))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(look.c.text)
                                .child(format!("“{}”", short(&c.said, 70))),
                        )
                        .child(div().flex_shrink_0().text_color(look.c.faint).child(fate)),
                );
            }
        }
        let tonight = data.as_ref().and_then(|d| d.tonight.clone()).map(|t| {
            div()
                .flex()
                .flex_col()
                .gap(look.px(4.))
                .px(look.px(16.))
                .py(look.px(8.))
                .border_t(look.fixed(1.))
                .border_color(look.c.border)
                .child(section(
                    "moon",
                    "Tonight",
                    format!(
                        "{} · reads {}",
                        t.at,
                        plural(t.sessions, "session", "sessions")
                    ),
                ))
                .child(
                    div()
                        .text_size(look.text(12.5))
                        .text_color(look.c.text_muted)
                        .child(format!(
                            "{}, {}, {}, {}",
                            plural(t.corrections, "correction", "corrections"),
                            plural(t.destructive, "destructive command", "destructive commands"),
                            plural(t.unverified, "unverified claim", "unverified claims"),
                            plural(t.notes_due, "note due a re-check", "notes due a re-check"),
                        )),
                )
        });
        div()
            .id("pulse-sheet")
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .bottom(look.px(44.))
            .flex()
            .flex_col()
            .bg(look.c.canvas)
            .border_b(look.fixed(1.))
            .border_color(look.c.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(8.))
                    .px(look.px(16.))
                    .pt(look.px(16.))
                    .pb(look.px(12.))
                    .child(
                        div()
                            .text_size(look.text(14.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(look.c.text)
                            .child("Fleet"),
                    )
                    .child(div().text_color(look.c.faint).child(if totals.subs > 0 {
                        format!(
                            "{} working · {}",
                            totals.working,
                            plural(totals.subs, "sub-agent", "sub-agents")
                        )
                    } else {
                        format!("{} working", totals.working)
                    }))
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("pulse-sheet-close")
                            .flex()
                            .items_center()
                            .gap(look.px(6.))
                            .text_size(look.text(12.))
                            .text_color(look.c.text_muted)
                            .cursor_pointer()
                            .hover(|h| h.text_color(look.c.text))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.pulse.sheet = false;
                                cx.notify();
                            }))
                            .child("Close")
                            .child(kit::kbd(look, "esc")),
                    ),
            )
            .child(tab_row)
            .child(
                div()
                    .id("pulse-sheet-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.pulse.sheet_scroll)
                    .child(list)
                    .child(memory)
                    .children(tonight),
            )
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
}

/// The breadcrumb over a focused item: its account's dot and name, then
/// `place`; the caller adds its state.
fn crumb(look: &Look, profile: Option<&str>, place: Option<&str>) -> gpui::Div {
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(look.px(8.))
        .pb(look.px(10.))
        .text_size(look.text(12.5))
        .text_color(look.c.text_muted)
        .when_some(profile, |d, p| {
            d.child(kit::dot(look, kit::profile_hue(p)))
                .child(kit::profile_name(p))
        })
        .when_some(place, |d, p| {
            d.child(div().text_color(look.c.faint).child("·"))
                .child(p.to_owned())
        })
        .child(div().text_color(look.c.faint).child("·"))
}

/// A focused item's title.
fn focus_title(look: &Look, title: &str) -> gpui::Div {
    div()
        .pb(look.px(8.))
        .text_size(look.text(22.))
        .line_height(look.text(29.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(look.c.text)
        .child(title.to_owned())
}

/// The fleet's histogram over the span, stacked by account, ending at
/// a now line, with its tick labels.
fn histogram_el(
    look: &Look,
    hist: pulse::Histogram,
    t0: OffsetDateTime,
    now: OffsetDateTime,
    h: f32,
) -> gpui::Div {
    let lk = look.clone();
    let span_s = (now - t0).whole_seconds();
    let chart = canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let o = bounds.origin;
            let w = f32::from(bounds.size.width);
            let height = f32::from(bounds.size.height);
            let peak = hist.peak().max(6) as f32;
            let span = span_s as f32;
            let beat_w = replay::BEAT.as_seconds_f32() / span * w;
            let bin = ((4. / beat_w).ceil() as usize).max(1);
            let mut k = 0;
            while k < hist.bars.len() {
                let end = (k + bin).min(hist.bars.len());
                // The tallest beat of the bin, its stack kept.
                let Some(bar) = hist.bars[k..end]
                    .iter()
                    .max_by_key(|b| b.iter().sum::<u32>())
                else {
                    break;
                };
                let t = hist.from + replay::BEAT * k as i32;
                let x = (t - t0).as_seconds_f32() / span * w;
                let bw = (beat_w * (end - k) as f32 - 2.).max(2.);
                let mut y = height;
                for (p, v) in hist.profiles.iter().zip(bar) {
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
    .h(look.px(h));
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
        .w_full()
        .flex()
        .flex_col()
        .gap(look.px(4.))
        .child(chart)
        .child(labels)
}

/// The accounts with a figure: the one to use next first, then by name.
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
                "Plan waiting".into()
            } else {
                "Waiting".into()
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
/// projection to the reset as outlines, red where it runs dry. `w` in
/// points, or the parent's width when `None`.
fn drain_chart(
    look: &Look,
    d: &hippo_core::usage::Drain,
    hue: Hsla,
    w: Option<f32>,
    h: f32,
) -> gpui::AnyElement {
    let d = d.clone();
    let lk = look.clone();
    let el = canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let o = bounds.origin;
            let width = f32::from(bounds.size.width);
            let height = f32::from(bounds.size.height);
            let slots = ((d.end - d.start) / hippo_core::usage::DRAIN_STEP).max(1) as usize;
            let slot_w = width / slots as f32;
            let bw = (slot_w - 2.).max(1.);
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
                        Bounds::new(point(x, o.y + px(height - 2.)), size(px(bw), px(2.))),
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
    .h(look.px(h));
    match w {
        Some(w) => el.flex_shrink_0().w(look.px(w)).into_any_element(),
        None => el.w_full().into_any_element(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(s: &str) -> String {
        s.to_owned()
    }

    #[test]
    fn the_selection_is_the_chosen_key_or_the_first() {
        let keys = vec![key("wait:a"), key("dry:work"), key("harm:i1")];
        assert_eq!(selected(&keys, None), Some(0));
        assert_eq!(selected(&keys, Some("dry:work")), Some(1));
        assert_eq!(selected(&keys, Some("gone")), Some(0));
        assert_eq!(selected(&[], Some("dry:work")), None);
    }

    #[test]
    fn items_put_off_come_last_in_the_order_they_were_put_off() {
        let item = |profile: &str| {
            Item::Next(Next::RunsDry {
                profile: profile.to_owned(),
                empty_at: 0,
                reset: 0,
                sessions: Vec::new(),
                to: None,
            })
        };
        let items = vec![item("a"), item("b"), item("c")];
        let out = later_last(items, &[key("dry:a"), key("dry:b")]);
        let keys: Vec<String> = out.iter().map(Item::key).collect();
        assert_eq!(keys, ["dry:c", "dry:a", "dry:b"]);
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
