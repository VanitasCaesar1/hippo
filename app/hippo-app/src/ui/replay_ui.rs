//! Replay: Fleet's Lanes view, the day as lanes.
//!
//! The day is read on the background executor (`hippo_view::replay`) and
//! kept; the lanes are drawn by hand: only the rows on screen exist, their
//! stretches are painted on one canvas, and marks, heads and the scrubber
//! are positioned over it. Every wheel and drag over the lanes is handled
//! here: a plain scroll moves through the lanes, ⌘-scroll or a pinch zooms
//! about the pointer, a sideways scroll or a drag pans, and a click pins
//! the time.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui::{
    Bounds, Context, FontWeight, Hsla, MouseButton, Pixels, Role, SharedString, Window, canvas,
    div, fill, outline, point, prelude::*, px, size,
};
use hippo_view::replay::{self, Day, Live, MarkKind, SpanKind, Target};
use time::{Date, Duration, OffsetDateTime};

use super::Shell;
use super::kit;
use super::theme::Look;
use crate::state::Tab;

const DAY_SECS: f64 = 86_400.;
/// The narrowest span a zoom reaches.
const MIN_SPAN: f64 = 15. * 60.;
/// A lane: the marks' band on top, the work under it.
const LANE_H: f32 = 30.;
/// The marks' own band at the top of a lane.
const MARK_BAND: f32 = 12.;
const STRIP_H: f32 = 12.;
const CHILD_H: f32 = 22.;
const HEAD_W: f32 = 200.;
const HEAD_NARROW: f32 = 120.;
const PANEL_W: f32 = 280.;
/// The panel sits beside the lanes from this width; below, it is a compact
/// strip under them.
const PANEL_BESIDE: f32 = 1400.;
/// The least room between two chips, or two ruler labels.
const GAP: f32 = 3.;
/// The fewest hours `Day` shows.
const DAY_MIN_HOURS: f64 = 6.;
/// How often today's lanes are read again while shown.
const REFRESH: Duration = Duration::seconds(30);

/// A part of the day's line that highlights its stretches or marks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Highlight {
    Waits,
    Limits,
    Merged,
    Incidents,
}

/// One drawn row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    Lane(usize),
    Strip(usize),
    Child(usize, usize),
}

impl RowKind {
    fn height(self) -> f32 {
        match self {
            RowKind::Lane(_) => LANE_H,
            RowKind::Strip(_) => STRIP_H,
            RowKind::Child(..) => CHILD_H,
        }
    }
}

/// Lanes' state.
pub struct ReplayUi {
    /// Fleet shows Lanes (`L`), not Rows; kept for the session.
    pub lanes: bool,
    /// The day shown; `None` is today.
    pub date: Option<Date>,
    /// The visible span, in seconds from the day's start.
    pub view: (f64, f64),
    pub day: Option<Rc<Day>>,
    pub loading: bool,
    pub error: Option<String>,
    /// The lanes' order as first shown, so a refresh does not move them.
    pub order: Vec<String>,
    pub unfolded: HashSet<String>,
    /// The pointer's time over the lanes, and a pinned time.
    pub pointer: Option<f64>,
    pub pinned: Option<f64>,
    /// The panel's chosen session while pinned (↑ ↓, ↩ opens it).
    pub selected: usize,
    pub highlight: Option<Highlight>,
    /// The lanes' vertical scroll, in pixels.
    pub scroll: f32,
    /// The lanes area as last drawn (window coordinates).
    pub area: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// A drag: where it started, the view then, and whether it moved.
    pub drag: Option<(f32, (f64, f64), bool)>,
    /// How long the last read took.
    pub read_took: Option<std::time::Duration>,
}

impl Default for ReplayUi {
    fn default() -> Self {
        ReplayUi {
            lanes: false,
            date: None,
            view: (0., DAY_SECS),
            day: None,
            loading: false,
            error: None,
            order: Vec::new(),
            unfolded: HashSet::new(),
            pointer: None,
            pinned: None,
            selected: 0,
            highlight: None,
            scroll: 0.,
            area: Rc::new(Cell::new(None)),
            drag: None,
            read_took: None,
        }
    }
}

/// The rows of `day` as drawn, with `unfolded` lanes' children.
pub fn rows(day: &Day, unfolded: &HashSet<String>) -> Vec<RowKind> {
    let mut out = Vec::new();
    for (i, l) in day.lanes.iter().enumerate() {
        out.push(RowKind::Lane(i));
        if !l.children.is_empty() {
            out.push(RowKind::Strip(i));
            if unfolded.contains(&l.track.id) {
                out.extend((0..l.children.len()).map(|j| RowKind::Child(i, j)));
            }
        }
    }
    out
}

/// `view` zoomed by `factor` (under 1 zooms in) about `at`, kept inside
/// the day and no narrower than [`MIN_SPAN`].
pub fn zoom(view: (f64, f64), at: f64, factor: f64) -> (f64, f64) {
    let span = ((view.1 - view.0) * factor).clamp(MIN_SPAN, DAY_SECS);
    let frac = ((at - view.0) / (view.1 - view.0)).clamp(0., 1.);
    let a = at - span * frac;
    clamp_view((a, a + span))
}

/// `view` moved inside the day.
pub fn clamp_view(view: (f64, f64)) -> (f64, f64) {
    let span = (view.1 - view.0).min(DAY_SECS);
    let a = view.0.clamp(0., DAY_SECS - span);
    (a, a + span)
}

/// The hour ruler's ticks for a span: `(seconds, label)`.
pub fn ticks(view: (f64, f64), width: f32) -> Vec<(f64, String)> {
    let span = view.1 - view.0;
    // The finest step whose labels stay at least 48 points apart.
    let per_sec = f64::from(width.max(1.)) / span;
    let step = [
        900.,
        1800.,
        3600.,
        2. * 3600.,
        3. * 3600.,
        6. * 3600.,
        12. * 3600.,
    ]
    .into_iter()
    .find(|s| s * per_sec >= 48.)
    .unwrap_or(12. * 3600.);
    let mut t = (view.0 / step).ceil() * step;
    let mut out = Vec::new();
    while t <= view.1 {
        let s = t as i64;
        out.push((t, format!("{:02}:{:02}", (s / 3600) % 24, (s % 3600) / 60)));
        t += step;
    }
    out
}

/// A span in words: `1h 40m`, `38m`, `under a minute`.
pub fn span_words(d: Duration) -> String {
    let m = d.whole_minutes();
    if m < 1 {
        return "under a minute".to_owned();
    }
    let (h, m) = (m / 60, m % 60);
    match (h, m) {
        (0, m) => format!("{m}m"),
        (h, 0) => format!("{h}h"),
        (h, m) => format!("{h}h {m}m"),
    }
}

/// The day's name in the toolbar: `Today`, `Yesterday`, `Mon 9 Nov`.
pub fn day_name(date: Date, today: Date) -> String {
    if date == today {
        return "Today".to_owned();
    }
    if Some(date) == today.previous_day() {
        return "Yesterday".to_owned();
    }
    let wd = &format!("{:?}", date.weekday())[..3];
    let mon = &format!("{:?}", date.month())[..3];
    format!("{wd} {} {mon}", date.day())
}

/// The lanes of `day` in the order first shown (`order`), new ones after.
pub fn keep_order(day: &mut Day, order: &[String]) {
    let at: HashMap<&str, usize> = order
        .iter()
        .enumerate()
        .map(|(i, id)| (id.as_str(), i))
        .collect();
    let mut keyed: Vec<(usize, usize, replay::Lane)> = std::mem::take(&mut day.lanes)
        .into_iter()
        .enumerate()
        .map(|(i, l)| {
            (
                at.get(l.track.id.as_str()).copied().unwrap_or(usize::MAX),
                i,
                l,
            )
        })
        .collect();
    keyed.sort_by_key(|(o, i, _)| (*o, *i));
    day.lanes = keyed.into_iter().map(|(_, _, l)| l).collect();
}

/// One chip on a lane's mark band: where it sits (from the timeline's
/// left) and which marks it holds, by their index in the laid-out list.
#[derive(Debug, Clone, PartialEq)]
pub struct Chip {
    pub left: f32,
    pub width: f32,
    pub members: Vec<usize>,
}

/// Lays out marks at `xs` (in order) left to right as chips that never
/// overlap or leave the timeline: a mark whose chip would come closer than
/// [`GAP`] to the chip before joins it, and the joined chip, as wide as
/// `width_of(count)`, is centred on its marks again.
pub fn layout_chips(xs: &[f32], width_of: &dyn Fn(usize) -> f32, tl_w: f32) -> Vec<Chip> {
    let place = |members: &[usize]| -> (f32, f32) {
        let w = width_of(members.len());
        let (a, b) = (xs[members[0]], xs[*members.last().unwrap_or(&members[0])]);
        let left = ((a + b) / 2. - w / 2.).clamp(0., (tl_w - w).max(0.));
        (left, w)
    };
    let mut chips: Vec<Chip> = Vec::new();
    for k in 0..xs.len() {
        let (left, width) = place(&[k]);
        chips.push(Chip {
            left,
            width,
            members: vec![k],
        });
        while chips.len() >= 2 {
            let n = chips.len();
            let (prev, last) = (&chips[n - 2], &chips[n - 1]);
            if last.left >= prev.left + prev.width + GAP {
                break;
            }
            let last = chips.pop().unwrap_or_else(|| unreachable!());
            let prev = chips.last_mut().unwrap_or_else(|| unreachable!());
            prev.members.extend(last.members);
            let (left, width) = place(&prev.members);
            prev.left = left;
            prev.width = width;
        }
    }
    chips
}

/// The kind a cluster shows: its most important (incident, question,
/// limit, merged, opened, carry-on).
pub fn chip_kind(kinds: &[MarkKind]) -> MarkKind {
    let rank = |k: &MarkKind| match k {
        MarkKind::Incident | MarkKind::Correction | MarkKind::Destructive => 0,
        MarkKind::Question => 1,
        MarkKind::Limit => 2,
        MarkKind::PullMerged => 3,
        MarkKind::PullOpened => 4,
        MarkKind::CarryOn => 5,
    };
    kinds
        .iter()
        .copied()
        .min_by_key(rank)
        .unwrap_or(MarkKind::PullOpened)
}

/// A lane's marks as drawn at the whole-day span: a pull request it opened
/// and merged that day shows as merged only.
pub fn day_marks(marks: &[replay::Mark]) -> Vec<&replay::Mark> {
    let merged: HashSet<&str> = marks
        .iter()
        .filter(|m| m.kind == MarkKind::PullMerged)
        .filter_map(|m| match &m.target {
            Some(Target::Url(u)) => Some(u.as_str()),
            _ => None,
        })
        .collect();
    marks
        .iter()
        .filter(|m| {
            m.kind != MarkKind::PullOpened
                || !matches!(&m.target, Some(Target::Url(u)) if merged.contains(u.as_str()))
        })
        .collect()
}

/// The ruler's labels that are drawn: `(left, label)`, each `width` wide,
/// left out when it would run past the timeline, touch the label before
/// it, or touch the scrubber's time (`scrub`, its left and right).
pub fn ruler_labels(
    ticks: &[(f32, String)],
    width: f32,
    tl_w: f32,
    scrub: Option<(f32, f32)>,
) -> Vec<(f32, String)> {
    let mut out: Vec<(f32, String)> = Vec::new();
    let mut right = f32::NEG_INFINITY;
    for (x, label) in ticks {
        let left = (x - width / 2.).max(0.);
        if *x < 0. || left + width > tl_w {
            continue;
        }
        if left < right + GAP * 2. {
            continue;
        }
        if scrub.is_some_and(|(a, b)| left < b + GAP && left + width > a - GAP) {
            continue;
        }
        right = left + width;
        out.push((left, label.clone()));
    }
    out
}

/// `Day`'s span: the hours with activity between `first` and `last`
/// (seconds from the day's start), from the hour before to the hour after
/// with a quarter-hour margin, never less than [`DAY_MIN_HOURS`] and
/// inside the day; the whole day when nothing worked.
pub fn fit_day(first: Option<f64>, last: Option<f64>) -> (f64, f64) {
    let (Some(first), Some(last)) = (first, last) else {
        return (0., DAY_SECS);
    };
    let hour = 3600.;
    let mut a = ((first - 900.) / hour).floor() * hour;
    let mut b = ((last.max(first) + 900.) / hour).ceil() * hour;
    let min = DAY_MIN_HOURS * hour;
    if b - a < min {
        let more = min - (b - a);
        let before = (more / 2. / hour).floor() * hour;
        a -= before;
        b += more - before;
    }
    let a0 = a.max(0.);
    let b0 = b.min(DAY_SECS);
    let (a, b) = if a < 0. {
        (0., (b0 - a).min(DAY_SECS).max(min.min(DAY_SECS)))
    } else if b > DAY_SECS {
        ((a0 - (b - DAY_SECS)).max(0.), DAY_SECS)
    } else {
        (a0, b0)
    };
    (a, b.max(a + min).min(DAY_SECS))
}

/// The day's first and last activity, in seconds from its start (today's
/// last is now).
pub fn day_bounds(day: &Day, now: OffsetDateTime) -> (Option<f64>, Option<f64>) {
    let secs = |t: OffsetDateTime| (t - day.start).as_seconds_f64();
    let tracks = day
        .lanes
        .iter()
        .flat_map(|l| std::iter::once(&l.track).chain(l.children.iter()));
    let mut first: Option<f64> = None;
    let mut last: Option<f64> = None;
    for t in tracks {
        if let Some(f) = t.first {
            first = Some(first.map_or(secs(f), |x: f64| x.min(secs(f))));
        }
        if let Some(l) = t.last {
            last = Some(last.map_or(secs(l), |x: f64| x.max(secs(l))));
        }
    }
    if now >= day.start && now < day.end {
        last = Some(last.map_or(secs(now), |x| x.max(secs(now))));
    }
    (first, last)
}

fn mark_icon(k: MarkKind) -> &'static str {
    match k {
        MarkKind::PullOpened => "git-pull-request",
        MarkKind::PullMerged => "git-merge",
        MarkKind::Incident | MarkKind::Destructive => "siren",
        MarkKind::Correction => "pencil",
        MarkKind::Question => "message-circle-question",
        MarkKind::Limit => "gauge",
        MarkKind::CarryOn => "arrow-right",
    }
}

impl Shell {
    fn replay_today(&self) -> Date {
        OffsetDateTime::now_utc()
            .to_offset(super::status::local_offset())
            .date()
    }

    fn replay_date(&self) -> Date {
        self.replay.date.unwrap_or_else(|| self.replay_today())
    }

    /// Reads the day shown when it is missing, another day, or today's and
    /// older than [`REFRESH`].
    fn replay_ensure(&mut self, cx: &mut Context<Self>) {
        if self.replay.loading {
            return;
        }
        let date = self.replay_date();
        let stale = match &self.replay.day {
            None => true,
            Some(d) => {
                d.date != date
                    || (date == self.replay_today()
                        && OffsetDateTime::now_utc() - d.read_at > REFRESH)
            }
        };
        if stale && self.replay.error.is_none() {
            self.replay_load(cx);
        }
    }

    fn replay_load(&mut self, cx: &mut Context<Self>) {
        let Some(home) = self
            .loaded
            .as_ref()
            .and_then(|l| l.setup.config())
            .map(|c| c.home.clone())
        else {
            return;
        };
        let date = self.replay_date();
        let today = date == self.replay_today();
        let mut live: HashMap<String, Live> = HashMap::new();
        if today && let Some(f) = self.loaded.as_ref().and_then(|l| l.fleet.as_ref()) {
            for r in &f.rows {
                use hippo_view::fleet::State;
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
        self.replay.loading = true;
        let offset = super::status::local_offset();
        cx.spawn(async move |this, cx| {
            let read = cx
                .background_executor()
                .spawn(async move {
                    let started = std::time::Instant::now();
                    let rollup = hippo_core::rollup::Rollup::open(&home)
                        .map_err(|e| e.to_string())?
                        .ok_or_else(|| "The rollup has not been built yet.".to_owned())?;
                    let index =
                        hippo_core::index::Index::open(&home, false).map_err(|e| e.to_string())?;
                    let day = replay::load(
                        &rollup,
                        &index,
                        date,
                        offset,
                        OffsetDateTime::now_utc(),
                        &live,
                    )
                    .map_err(|e| e.to_string())?;
                    Ok::<_, String>((day, started.elapsed()))
                })
                .await;
            this.update(cx, |this, cx| {
                this.replay.loading = false;
                match read {
                    Ok((mut day, took)) => {
                        let same = this.replay.day.as_ref().is_some_and(|d| d.date == day.date);
                        if same {
                            keep_order(&mut day, &this.replay.order);
                        } else {
                            this.replay.scroll = 0.;
                            this.replay.pinned = None;
                            // A new day opens on its hours with
                            // activity.
                            let (a, b) = day_bounds(&day, OffsetDateTime::now_utc());
                            this.replay.view = fit_day(a, b);
                        }
                        this.replay.order = day.lanes.iter().map(|l| l.track.id.clone()).collect();
                        this.replay.day = Some(Rc::new(day));
                        this.replay.read_took = Some(took);
                        this.replay.error = None;
                    }
                    Err(e) => this.replay.error = Some(e),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn replay_toggle(&mut self, h: Highlight, cx: &mut Context<Self>) {
        self.replay.highlight = if self.replay.highlight == Some(h) {
            None
        } else {
            Some(h)
        };
        cx.notify();
    }

    /// Shows the day `date` (today when `None`).
    pub(crate) fn replay_go(&mut self, date: Option<Date>, cx: &mut Context<Self>) {
        let today = self.replay_today();
        self.replay.date = date.filter(|d| *d < today);
        self.replay.error = None;
        self.replay.pinned = None;
        cx.notify();
    }

    /// Lanes' keys: `L` back to Rows, Esc lets a pinned time go, ↑ ↓ and ↩
    /// in the panel while pinned.
    pub(super) fn replay_key(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match key {
            "l" => {
                self.replay.lanes = false;
            }
            "escape" if self.replay.pinned.is_some() => {
                self.replay.pinned = None;
            }
            "up" | "k" if self.replay.pinned.is_some() => {
                self.replay.selected = self.replay.selected.saturating_sub(1);
            }
            "down" | "j" if self.replay.pinned.is_some() => {
                self.replay.selected += 1;
            }
            "enter" if self.replay.pinned.is_some() => {
                let Some(day) = self.replay.day.clone() else {
                    return false;
                };
                let t = day.start + Duration::seconds_f64(self.replay.pinned.unwrap_or(0.));
                let m = replay::moment(&day, t);
                let Some(on) = m.working.get(self.replay.selected) else {
                    return false;
                };
                self.replay_open(on.session.clone(), on.ordinal, window, cx);
                return true;
            }
            _ => return false,
        }
        cx.notify();
        true
    }

    /// Opens a session's page at a turn, marked (as Open the step does).
    fn replay_open(
        &mut self,
        id: String,
        ordinal: Option<u32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(o) = ordinal {
            self.sessions_ui.scroll_to = Some((id.clone(), o));
        }
        self.state.tabs.follow(Tab::Session { id, ordinal }, false);
        self.after_state_change(window, cx);
    }

    fn replay_target(&mut self, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        match target {
            Target::Url(u) => {
                let _ = crate::launch::open(&u);
            }
            Target::Incident(id) => self.open_incident(&id, window, cx),
            Target::Turn { session, ordinal } => {
                self.replay_open(session, Some(ordinal), window, cx)
            }
            Target::Session(id) => self.replay_open(id, None, window, cx),
        }
    }

    /// `Rows | Lanes`.
    pub(super) fn replay_switch(&self, look: &Look, cx: &mut Context<Self>) -> gpui::Div {
        let on = self.replay.lanes;
        kit::segmented(look)
            .child(
                kit::segment(look, "fleet-rows", Some("list"), "Rows", !on).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.replay.lanes = false;
                        cx.notify();
                    },
                )),
            )
            .child(
                kit::segment(look, "fleet-lanes", Some("calendar-clock"), "Lanes", on)
                    .tooltip(super::tooltip::Tooltip::text("The day as lanes (L)"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.replay.lanes = true;
                        cx.notify();
                    })),
            )
    }

    pub(super) fn render_lanes(
        &mut self,
        look: &Look,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        self.replay_ensure(cx);
        let width = f32::from(window.viewport_size().width);
        let narrow = width < super::home::COMPACT;
        // The panel sits beside the lanes only from 1400 points, so
        // the timeline keeps at least 640 at 1200; below, it is a compact
        // strip under them.
        let stacked = width < PANEL_BESIDE;
        let page_h = f32::from(window.viewport_size().height);
        let today = self.replay_today();
        let date = self.replay_date();
        let toolbar = self.lanes_toolbar(date, today, look, cx);
        let page = div()
            .flex()
            .flex_col()
            .size_full()
            .px(look.px(16.))
            .pt(look.px(8.))
            .pb(look.px(12.))
            .text_size(look.text(13.))
            .child(toolbar);
        let Some(day) = self.replay.day.clone().filter(|d| d.date == date) else {
            let why = match &self.replay.error {
                Some(e) => e.clone(),
                None => "Reading the day…".to_owned(),
            };
            return page
                .child(
                    div()
                        .py(look.px(14.))
                        .text_color(look.c.text_muted)
                        .child(why),
                )
                .into_any_element();
        };
        if day.lanes.is_empty() {
            return page
                .child(
                    div()
                        .py(look.px(14.))
                        .text_color(look.c.text_muted)
                        .child("No thread worked this day."),
                )
                .into_any_element();
        }
        let head_w = if narrow { HEAD_NARROW } else { HEAD_W };
        let line = self.lanes_line(&day, width < 1100., look, cx);
        let ruler = self.lanes_ruler(&day, head_w, look);
        let lanes = self.lanes_area(&day, head_w, narrow, look, cx);
        let panel = self.lanes_panel(&day, stacked, page_h, look, cx);
        let body = if stacked {
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h_0()
                        .child(ruler)
                        .child(lanes),
                )
                .child(panel)
        } else {
            div()
                .flex()
                .flex_1()
                .min_h_0()
                .gap(look.px(12.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .child(ruler)
                        .child(lanes),
                )
                .child(panel)
        };
        page.child(line).child(body).into_any_element()
    }

    fn lanes_toolbar(
        &self,
        date: Date,
        today: Date,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let prev = date.previous_day();
        let next = date.next_day().filter(|d| *d <= today);
        let span = self.replay.view.1 - self.replay.view.0;
        // `Day` is the hours with activity, not midnight to
        // midnight.
        let fitted = self
            .replay
            .day
            .as_ref()
            .filter(|d| d.date == date)
            .map(|d| {
                let (a, b) = day_bounds(d, OffsetDateTime::now_utc());
                fit_day(a, b)
            })
            .unwrap_or((0., DAY_SECS));
        let hhmm = |t: f64| {
            let s = t as i64;
            format!("{:02}:{:02}", (s / 3600) % 24, (s % 3600) / 60)
        };
        let day_tip = format!(
            "The hours with activity: {} to {}",
            hhmm(fitted.0),
            if fitted.1 >= DAY_SECS {
                "24:00".to_owned()
            } else {
                hhmm(fitted.1)
            }
        );
        let zoom_seg = [
            ("Day", fitted.1 - fitted.0),
            ("6h", 6. * 3600.),
            ("1h", 3600.),
        ]
        .into_iter()
        .fold(kit::segmented(look), |d, (name, secs)| {
            let on = if name == "Day" {
                (span - secs).abs() < 1. && (self.replay.view.0 - fitted.0).abs() < 1.
            } else {
                (span - secs).abs() < 1.
            };
            let seg = kit::segment(
                look,
                SharedString::from(format!("zoom-{name}")),
                None,
                name,
                on,
            );
            let seg = if name == "Day" {
                seg.tooltip(super::tooltip::Tooltip::text(day_tip.clone()))
            } else {
                seg
            };
            d.child(seg.on_click(cx.listener(move |this, _, _, cx| {
                if name == "Day" {
                    this.replay.view = fitted;
                    cx.notify();
                    return;
                }
                let v = this.replay.view;
                let mid = match (this.replay.pinned, this.replay.pointer) {
                    (Some(p), _) => p,
                    _ => {
                        let now = this
                            .replay
                            .day
                            .as_ref()
                            .map(|d| (OffsetDateTime::now_utc() - d.start).as_seconds_f64());
                        match now.filter(|n| *n < DAY_SECS && *n >= 0.) {
                            Some(n) if secs < DAY_SECS => n - secs * 0.7,
                            _ => (v.0 + v.1) / 2. - secs / 2.,
                        }
                    }
                };
                let a = if this.replay.pinned.is_some() {
                    mid - secs / 2.
                } else {
                    mid
                };
                this.replay.view = clamp_view((a, a + secs));
                cx.notify();
            })))
        });
        let switch = self.replay_switch(look, cx);
        let nav = |cx: &mut Context<Self>,
                   id: &'static str,
                   icon: &str,
                   to: Option<Date>,
                   tip: &'static str| {
            let enabled = to.is_some();
            div()
                .id(id)
                .flex()
                .items_center()
                .justify_center()
                .size(look.px(26.))
                .rounded(look.px(6.))
                .when(enabled, |d| {
                    d.cursor_pointer()
                        .hover(|h| h.bg(look.c.hover))
                        .active(|a| a.bg(look.c.selected))
                })
                .child(kit::icon(
                    look,
                    icon,
                    15.,
                    if enabled {
                        look.c.text_muted
                    } else {
                        look.c.faint
                    },
                ))
                .tooltip(super::tooltip::Tooltip::text(tip))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(d) = to {
                        this.replay_go(Some(d), cx);
                    }
                }))
        };
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(look.px(10.))
            .pb(look.px(10.))
            .child(switch)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(2.))
                    .child(nav(
                        cx,
                        "lanes-prev",
                        "chevron-left",
                        prev,
                        "The day before",
                    ))
                    .child(
                        div()
                            .min_w(look.px(84.))
                            .flex()
                            .justify_center()
                            .text_size(look.text(13.))
                            .font_weight(FontWeight::MEDIUM)
                            .child(day_name(date, today)),
                    )
                    .child(nav(
                        cx,
                        "lanes-next",
                        "chevron-right",
                        next,
                        "The day after",
                    )),
            )
            .child(zoom_seg)
            .child(div().flex_1())
            .children(self.replay.loading.then(|| {
                div()
                    .text_size(look.text(12.))
                    .text_color(look.c.faint)
                    .child("reading…")
            }))
    }

    /// The day's line: counts only, each part acting.
    fn lanes_line(
        &self,
        day: &Day,
        narrow: bool,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let l = &day.line;
        let hl = self.replay.highlight;
        let part = |id: &'static str, text: String, on: bool, act: Option<Highlight>| {
            div()
                .id(id)
                .text_color(if on { look.c.text } else { look.c.text_muted })
                .rounded(look.px(4.))
                .px(look.px(3.))
                .when(on, |d| d.bg(look.c.selected))
                .child(text)
                .when(act.is_some(), |d| {
                    d.cursor_pointer()
                        .hover(|h| h.text_color(look.c.text))
                        .active(|a| a.bg(look.c.hover))
                })
        };
        let mut parts: Vec<gpui::AnyElement> = Vec::new();
        let threads = format!(
            "{} {}",
            l.threads,
            if l.threads == 1 { "thread" } else { "threads" }
        );
        parts.push(
            part("line-threads", threads, false, None)
                .cursor_pointer()
                .hover(|h| h.text_color(look.c.text))
                .tooltip(super::tooltip::Tooltip::text(
                    "Unfold or fold every lane's sub-agents",
                ))
                .on_click(cx.listener(|this, _, _, cx| {
                    let Some(day) = this.replay.day.clone() else {
                        return;
                    };
                    let all: HashSet<String> = day
                        .lanes
                        .iter()
                        .filter(|l| !l.children.is_empty())
                        .map(|l| l.track.id.clone())
                        .collect();
                    if this.replay.unfolded.is_superset(&all) {
                        this.replay.unfolded.clear();
                    } else {
                        this.replay.unfolded = all;
                    }
                    cx.notify();
                }))
                .into_any_element(),
        );
        if l.subs > 0 {
            parts.push(
                part("line-subs", format!("{} sub-agents", l.subs), false, None).into_any_element(),
            );
        }
        if l.work > Duration::ZERO {
            parts.push(
                part(
                    "line-work",
                    format!("{} of agent work", span_words(l.work)),
                    false,
                    None,
                )
                .id("line-work")
                .tooltip(super::tooltip::Tooltip::text(
                    "Every session's working time summed, sub-agents included",
                ))
                .into_any_element(),
            );
        }
        if l.questions > 0 {
            parts.push(
                part(
                    "line-waits",
                    format!(
                        "waited on you {} in {} {}",
                        span_words(l.waited),
                        l.questions,
                        if l.questions == 1 {
                            "question"
                        } else {
                            "questions"
                        }
                    ),
                    hl == Some(Highlight::Waits),
                    Some(Highlight::Waits),
                )
                .text_color(look.c.warn)
                .on_click(cx.listener(|this, _, _, cx| this.replay_toggle(Highlight::Waits, cx)))
                .into_any_element(),
            );
        }
        for (id, n, word, h) in [
            ("line-limits", l.limits, "limit", Highlight::Limits),
            ("line-merged", l.merged, "merged", Highlight::Merged),
            (
                "line-incidents",
                l.incidents,
                "incident",
                Highlight::Incidents,
            ),
        ] {
            if n == 0 {
                continue;
            }
            let text = match word {
                "merged" => format!("{n} merged"),
                w => format!("{n} {w}{}", if n == 1 { "" } else { "s" }),
            };
            parts.push(
                part(id, text, hl == Some(h), Some(h))
                    .on_click(cx.listener(move |this, _, _, cx| this.replay_toggle(h, cx)))
                    .into_any_element(),
            );
        }
        let mut row = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(look.px(4.))
            .text_size(look.text(12.5));
        for (k, p) in parts.into_iter().enumerate() {
            if k > 0 {
                row = row.child(div().text_color(look.c.faint).child("·"));
            }
            row = row.child(p);
        }
        let longest = l.longest.clone().map(|(id, s)| {
            let title = day
                .lanes
                .iter()
                .find(|l| l.track.id == id)
                .and_then(|l| l.track.title.clone())
                .map(|t| kit::title(&t))
                .unwrap_or_else(|| crate::rows::id_title(&id));
            let at = (s.start - day.start).as_seconds_f64();
            let text = format!(
                "Longest wait: {title}, {}, {}",
                kit::hm(s.start),
                span_words(s.len())
            );
            div().min_w_0().truncate().child(
                kit::link(look, "line-longest", text)
                    .text_color(look.c.text_muted)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.replay.pinned = Some(at);
                        let span = this.replay.view.1 - this.replay.view.0;
                        this.replay.view = clamp_view((at - span * 0.3, at + span * 0.7));
                        if let Some(day) = this.replay.day.clone() {
                            let rows = rows(&day, &this.replay.unfolded);
                            let mut y = 0.;
                            for r in rows {
                                if r == RowKind::Lane(
                                    day.lanes.iter().position(|l| l.track.id == id).unwrap_or(0),
                                ) {
                                    break;
                                }
                                y += r.height();
                            }
                            this.replay.scroll = y;
                        }
                        cx.notify();
                    })),
            )
        });
        // What the marks and colours mean (the first reader could not
        // tell that amber is waiting).
        let swatch = |look: &Look, color: Hsla, dashed: bool| {
            div()
                .w(look.px(12.))
                .h(look.px(8.))
                .rounded(look.px(2.))
                .bg(color)
                .when(dashed, |d| {
                    d.border(look.fixed(1.))
                        .border_color(look.c.text_muted.opacity(0.6))
                        .border_dashed()
                })
        };
        let key = |look: &Look, lead: gpui::AnyElement, word: &str| {
            div()
                .flex()
                .items_center()
                .gap(look.px(4.))
                .child(lead)
                .child(word.to_owned())
        };
        let mut legend = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(look.px(10.))
            .text_size(look.text(11.5))
            .text_color(look.c.faint)
            .child(key(
                look,
                swatch(look, look.c.text_muted.opacity(0.45), false).into_any_element(),
                "working",
            ))
            .child(key(
                look,
                swatch(look, look.c.warn.opacity(0.85), false).into_any_element(),
                "waiting on you",
            ))
            .child(key(
                look,
                swatch(look, look.c.border.opacity(0.5), true).into_any_element(),
                "at a usage limit",
            ));
        if !narrow {
            legend = legend.child(key(
                look,
                div()
                    .flex()
                    .items_end()
                    .gap(px(1.))
                    .h(look.px(9.))
                    .children([3., 6., 9.].map(|h| {
                        div()
                            .w(px(3.))
                            .h(look.px(h))
                            .bg(look.c.text_muted.opacity(0.4))
                    }))
                    .into_any_element(),
                "sub-agents running",
            ));
            for (icon, word) in [
                ("git-pull-request", "pull request"),
                ("git-merge", "merged"),
                ("message-circle-question", "question"),
                ("siren", "incident"),
            ] {
                legend = legend.child(key(
                    look,
                    kit::icon(look, icon, 11., look.c.faint).into_any_element(),
                    word,
                ));
            }
        }
        div()
            .flex()
            .flex_col()
            .gap(look.px(4.))
            .pb(look.px(10.))
            .child(row)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(look.px(12.))
                    .text_size(look.text(12.))
                    .children(longest)
                    .child(div().flex_1())
                    .when(!narrow, |d| d.child(legend)),
            )
    }

    fn lanes_ruler(&self, day: &Day, head_w: f32, look: &Look) -> gpui::Div {
        let view = self.replay.view;
        let w = self
            .replay
            .area
            .get()
            .map(|b| f32::from(b.size.width) - f32::from(look.px(head_w)))
            .unwrap_or(600.);
        let x = |t: f64| ((t - view.0) / (view.1 - view.0)) as f32 * w;
        let shown = self.replay.pinned.or(self.replay.pointer);
        let mut ruler = div()
            .relative()
            .h(look.px(22.))
            .ml(look.px(head_w))
            .border_b(look.fixed(1.))
            .border_color(look.c.border)
            .text_size(look.text(11.))
            .text_color(look.c.faint);
        // A label that would touch the one before it, or the
        // scrubber's time, is not drawn.
        let label_w = f32::from(look.px(30.));
        let scrub_w = f32::from(look.px(46.));
        let scrub_left = |t: f64| (x(t) - scrub_w / 2.).clamp(0., (w - scrub_w).max(0.));
        let placed: Vec<(f32, String)> = ticks(view, w)
            .into_iter()
            .map(|(t, label)| (x(t), label))
            .collect();
        let scrub = shown.map(|t| (scrub_left(t), scrub_left(t) + scrub_w));
        for (left, label) in ruler_labels(&placed, label_w, w, scrub) {
            ruler = ruler.child(
                div()
                    .absolute()
                    .top(look.px(3.))
                    .left(px(left))
                    .w(px(label_w))
                    .flex()
                    .justify_center()
                    .whitespace_nowrap()
                    .child(label),
            );
        }
        if let Some(t) = shown {
            let at = day.start + Duration::seconds_f64(t);
            ruler = ruler.child(
                div()
                    .absolute()
                    .top(look.px(1.))
                    .left(px(scrub_left(t)))
                    .w(px(scrub_w))
                    .flex()
                    .justify_center()
                    .whitespace_nowrap()
                    .px(look.px(4.))
                    .rounded(look.px(4.))
                    .bg(look.c.surface)
                    .border(look.fixed(1.))
                    .border_color(look.c.border)
                    .text_color(look.c.text)
                    .child(kit::hm(at)),
            );
        }
        ruler
    }

    #[allow(clippy::too_many_lines)]
    fn lanes_area(
        &mut self,
        day: &Rc<Day>,
        head_w: f32,
        narrow: bool,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let rows = rows(day, &self.replay.unfolded);
        let area = self.replay.area.get();
        let area_h = area.map(|b| f32::from(b.size.height)).unwrap_or(500.);
        let head = f32::from(look.px(head_w));
        let tl_w = area
            .map(|b| f32::from(b.size.width) - head)
            .unwrap_or(600.)
            .max(1.);
        let heights: Vec<f32> = rows
            .iter()
            .map(|r| f32::from(look.px(r.height())))
            .collect();
        let total: f32 = heights.iter().sum();
        let max_scroll = (total - area_h).max(0.);
        if self.replay.scroll > max_scroll {
            self.replay.scroll = max_scroll;
        }
        let scroll = self.replay.scroll;
        // The rows on screen and where they sit.
        let mut visible: Vec<(RowKind, f32, f32)> = Vec::new();
        let mut y = -scroll;
        for (r, h) in rows.iter().zip(&heights) {
            if y + h >= 0. && y <= area_h {
                visible.push((*r, y, *h));
            }
            y += h;
            if y > area_h {
                break;
            }
        }
        let view = self.replay.view;
        let x = move |t: f64| ((t - view.0) / (view.1 - view.0)) as f32 * tl_w;
        let secs = |t: OffsetDateTime| (t - day.start).as_seconds_f64();
        let now = OffsetDateTime::now_utc();
        let now_x = (now >= day.start && now < day.end).then(|| x(secs(now)));
        let hl = self.replay.highlight;
        let whole_day = {
            let (a, b) = day_bounds(day, now);
            let fitted = fit_day(a, b);
            view.1 - view.0 >= fitted.1 - fitted.0 - 1.
        };

        // The canvas: every visible row's stretches and strips.
        let paint_day = day.clone();
        let paint_rows = visible.clone();
        let lk = look.clone();
        let area_cell = self.replay.area.clone();
        let shell = cx.entity().downgrade();
        let painter = canvas(
            move |bounds, _, cx| {
                let before = area_cell.get();
                area_cell.set(Some(bounds));
                // The ruler, heads and marks are laid out from the last
                // size: after a resize the view renders again (a window
                // refresh alone reuses the cached view).
                if before.map(|b| b.size) != Some(bounds.size) {
                    let shell = shell.clone();
                    cx.defer(move |cx| {
                        shell.update(cx, |_, cx| cx.notify()).ok();
                    });
                }
            },
            move |bounds, _, window, _| {
                let o = bounds.origin;
                let tl0 = o.x + px(head);
                let w = f32::from(bounds.size.width) - head;
                let xt = |t: f64| ((t - view.0) / (view.1 - view.0)) as f32 * w;
                let clip = |a: f32, b: f32| (a.max(0.), b.min(w));
                let dim = |k: SpanKind| match hl {
                    Some(Highlight::Waits) => k != SpanKind::Waiting,
                    Some(Highlight::Limits) => k != SpanKind::Limit,
                    Some(_) => true,
                    None => false,
                };
                let paint_track = |window: &mut Window, tr: &replay::Track, top: f32, h: f32| {
                    for s in &tr.spans {
                        let (a, b) = clip(
                            xt((s.start - paint_day.start).as_seconds_f64()),
                            xt((s.end - paint_day.start).as_seconds_f64()),
                        );
                        let b = if s.kind == SpanKind::Limit && b - a < 2. {
                            a + 2.
                        } else {
                            b
                        };
                        if b <= 0. || a >= w || b - a < 0.5 && s.kind != SpanKind::Working {
                            continue;
                        }
                        let b = b.max(a + 1.);
                        let r =
                            Bounds::new(point(tl0 + px(a), o.y + px(top)), size(px(b - a), px(h)));
                        let fade = if dim(s.kind) { 0.3 } else { 1. };
                        match s.kind {
                            SpanKind::Working => window.paint_quad(fill(
                                r,
                                lk.c.text_muted.opacity((0.12 + 0.43 * s.strength) * fade),
                            )),
                            SpanKind::Waiting => {
                                window.paint_quad(fill(r, lk.c.warn.opacity(0.85 * fade)))
                            }
                            SpanKind::Limit => {
                                window.paint_quad(fill(r, lk.c.border.opacity(0.5 * fade)));
                                window.paint_quad(outline(
                                    r,
                                    lk.c.text_muted.opacity(0.6 * fade),
                                    gpui::BorderStyle::Dashed,
                                ));
                            }
                        }
                    }
                };
                // Faint hour lines under the stretches.
                for (t, _) in ticks(view, w) {
                    let gx = xt(t);
                    if gx <= 0. || gx >= w {
                        continue;
                    }
                    window.paint_quad(fill(
                        Bounds::new(point(tl0 + px(gx), o.y), size(px(1.), bounds.size.height)),
                        lk.c.border.opacity(0.45),
                    ));
                }
                for (row, top, h) in &paint_rows {
                    match *row {
                        RowKind::Lane(i) => {
                            let lane = &paint_day.lanes[i];
                            // The work fills the lane under the
                            // marks' band, so no mark covers it.
                            let band = f32::from(lk.px(MARK_BAND));
                            let bar = (h - band - 5.).max(6.);
                            paint_track(window, &lane.track, top + band + 1., bar);
                            window.paint_quad(fill(
                                Bounds::new(
                                    point(o.x, o.y + px(top + h - 1.)),
                                    size(bounds.size.width, px(1.)),
                                ),
                                lk.c.border.opacity(0.5),
                            ));
                        }
                        RowKind::Child(i, j) => {
                            let tr = &paint_day.lanes[i].children[j];
                            paint_track(window, tr, top + h * 0.3, h * 0.4);
                        }
                        RowKind::Strip(i) => {
                            let running = &paint_day.lanes[i].running;
                            let max = f32::from(running.iter().copied().max().unwrap_or(1).max(1));
                            // One bar per pixel column: the most running in
                            // the minutes it covers, equal columns merged.
                            let per_px = (view.1 - view.0) / f64::from(w) / 60.;
                            let mut run: Option<(f32, u16)> = None;
                            let flush = |window: &mut Window, from: f32, to: f32, n: u16| {
                                if n == 0 {
                                    return;
                                }
                                let bh = (f32::from(n) / max).sqrt() * (h - 4.);
                                window.paint_quad(fill(
                                    Bounds::new(
                                        point(tl0 + px(from), o.y + px(top + h - 2. - bh)),
                                        size(px((to - from).max(1.)), px(bh.max(1.))),
                                    ),
                                    lk.c.text_muted
                                        .opacity(if hl.is_some() { 0.15 } else { 0.4 }),
                                ));
                            };
                            let mut col = 0.;
                            while col < w {
                                let m0 = (view.0 / 60. + f64::from(col) * per_px) as usize;
                                let m1 = ((view.0 / 60. + f64::from(col + 1.) * per_px) as usize)
                                    .max(m0 + 1);
                                let n = running
                                    .get(m0..m1.min(running.len()))
                                    .and_then(|s| s.iter().copied().max())
                                    .unwrap_or(0);
                                match run {
                                    Some((from, v)) if v == n => {
                                        let _ = from;
                                    }
                                    Some((from, v)) => {
                                        flush(window, from, col, v);
                                        run = Some((col, n));
                                    }
                                    None => run = Some((col, n)),
                                }
                                col += 1.;
                            }
                            if let Some((from, v)) = run {
                                flush(window, from, w, v);
                            }
                        }
                    }
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();

        // Heads, marks and strip targets over the canvas.
        let mut over: Vec<gpui::AnyElement> = Vec::new();
        for (row, top, h) in &visible {
            match *row {
                RowKind::Lane(i) => {
                    let lane = &day.lanes[i];
                    let title = lane
                        .track
                        .title
                        .as_deref()
                        .map(kit::title)
                        .unwrap_or_else(|| crate::rows::id_title(&lane.track.id));
                    let title_len = title.chars().count();
                    let head_tip = match &lane.project {
                        Some(p) => format!("{title} · {p}"),
                        None => title.clone(),
                    };
                    over.push(
                        div()
                            .id(SharedString::from(format!("head-{i}")))
                            .tooltip(super::tooltip::Tooltip::text(head_tip))
                            .absolute()
                            .top(px(*top))
                            .left_0()
                            .w(px(head))
                            .h(px(*h))
                            .flex()
                            .items_center()
                            .gap(look.px(6.))
                            .pr(look.px(8.))
                            .child(kit::mark(look, lane.profile.as_deref(), 16.))
                            // The title and the project on one line.
                            .child(
                                div()
                                    .flex()
                                    .items_baseline()
                                    .gap(look.px(5.))
                                    .min_w_0()
                                    .child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .text_size(look.text(13.))
                                            .text_color(look.c.text)
                                            .child(title),
                                    )
                                    .when(!narrow, |d| {
                                        // The project only when it fits
                                        // beside the whole title.
                                        let room = ((head_w - 40.) / 6.8) as usize;
                                        let fits =
                                            |p: &String| title_len + p.chars().count() < room;
                                        d.children(lane.project.clone().filter(fits).map(|p| {
                                            div()
                                                .flex_shrink_0()
                                                .truncate()
                                                .text_size(look.text(11.5))
                                                .text_color(look.c.faint)
                                                .child(p)
                                        }))
                                    }),
                            )
                            .into_any_element(),
                    );
                    // Marks in their own band, laid out so no chip
                    // overlaps or clips another; a cluster is one chip with
                    // its most important kind and a count.
                    let shown_marks: Vec<&replay::Mark> = if whole_day {
                        day_marks(&lane.marks)
                    } else {
                        lane.marks.iter().collect()
                    };
                    let inside: Vec<&replay::Mark> = shown_marks
                        .into_iter()
                        .filter(|m| {
                            let mx = x(secs(m.at));
                            (0. ..=tl_w).contains(&mx)
                        })
                        .collect();
                    let xs: Vec<f32> = inside.iter().map(|m| x(secs(m.at))).collect();
                    let chip_w = |n: usize| {
                        let digits = n.to_string().len() as f32;
                        f32::from(look.px(if n == 1 { 14. } else { 16. + 6. * digits }))
                    };
                    let band = f32::from(look.px(MARK_BAND));
                    for (k, chip) in layout_chips(&xs, &chip_w, tl_w).into_iter().enumerate() {
                        let ms: Vec<&replay::Mark> =
                            chip.members.iter().map(|&j| inside[j]).collect();
                        let kinds: Vec<MarkKind> = ms.iter().map(|m| m.kind).collect();
                        let kind = chip_kind(&kinds);
                        let lit = match hl {
                            Some(Highlight::Merged) => kinds.contains(&MarkKind::PullMerged),
                            Some(Highlight::Incidents) => kinds.iter().any(|k| k.is_incident()),
                            Some(Highlight::Limits) => kinds.contains(&MarkKind::Limit),
                            Some(Highlight::Waits) => kinds.contains(&MarkKind::Question),
                            None => true,
                        };
                        let tip = ms
                            .iter()
                            .map(|m| format!("{} · {}", kit::hm(m.at), m.label))
                            .collect::<Vec<_>>()
                            .join("\n");
                        let tip = if ms.len() > 1 {
                            format!("{tip}\nClick to zoom in")
                        } else {
                            tip
                        };
                        let color = match kind {
                            MarkKind::Question => look.c.warn,
                            k if k.is_incident() => look.c.text,
                            _ => look.c.text_muted,
                        };
                        let n = ms.len();
                        let target = (n == 1).then(|| ms[0].target.clone()).flatten();
                        let range = (
                            ms.iter().map(|m| secs(m.at)).fold(f64::MAX, f64::min),
                            ms.iter().map(|m| secs(m.at)).fold(f64::MIN, f64::max),
                        );
                        over.push(
                            div()
                                .id(SharedString::from(format!("mark-{i}-{k}")))
                                .absolute()
                                .top(px(top + 0.5))
                                .left(px(head + chip.left))
                                .w(px(chip.width))
                                .h(px(band - 1.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .gap(look.px(1.))
                                .rounded(look.px(3.))
                                .bg(look.c.surface)
                                .border(look.fixed(1.))
                                .border_color(look.c.border)
                                .opacity(if lit { 1. } else { 0.35 })
                                .cursor_pointer()
                                .hover(|h| h.bg(look.c.hover).border_color(look.c.text_muted))
                                .active(|a| a.bg(look.c.selected))
                                .child(kit::icon(look, mark_icon(kind), 9., color))
                                .when(n > 1, |d| {
                                    d.child(
                                        div()
                                            .text_size(look.text(9.5))
                                            .line_height(px(band - 2.))
                                            .text_color(look.c.text_muted)
                                            .child(n.to_string()),
                                    )
                                })
                                .tooltip(super::tooltip::Tooltip::text(tip))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        if let Some(t) = target.clone() {
                                            this.replay_target(t, window, cx);
                                        } else if n > 1 {
                                            // A cluster: zoom to its time.
                                            let pad = ((range.1 - range.0) * 0.2).max(120.);
                                            let span = (range.1 - range.0 + 2. * pad).max(MIN_SPAN);
                                            let mid = (range.0 + range.1) / 2.;
                                            this.replay.view =
                                                clamp_view((mid - span / 2., mid + span / 2.));
                                            cx.notify();
                                        }
                                    }),
                                )
                                .into_any_element(),
                        );
                    }
                }
                RowKind::Strip(i) => {
                    let lane = &day.lanes[i];
                    let id = lane.track.id.clone();
                    let open = self.replay.unfolded.contains(&id);
                    let n = lane.children.len();
                    let merged = lane
                        .marks
                        .iter()
                        .filter(|m| m.kind == MarkKind::PullMerged)
                        .count();
                    over.push(
                        div()
                            .id(SharedString::from(format!("strip-{i}")))
                            .absolute()
                            .top(px(*top))
                            .left_0()
                            .w(px(head))
                            .h(px(*h))
                            .flex()
                            .items_center()
                            .gap(look.px(4.))
                            .pl(look.px(if narrow { 6. } else { 24. }))
                            .pr(look.px(6.))
                            .overflow_hidden()
                            .text_size(look.text(11.))
                            .line_height(px(*h))
                            .text_color(look.c.faint)
                            .cursor_pointer()
                            .hover(|h| h.text_color(look.c.text))
                            .active(|a| a.bg(look.c.hover))
                            .child(kit::icon(
                                look,
                                if open {
                                    "chevron-down"
                                } else {
                                    "chevron-right"
                                },
                                11.,
                                look.c.faint,
                            ))
                            .child(div().min_w_0().truncate().child(format!(
                                "{n} sub-agent{}{}",
                                if n == 1 { "" } else { "s" },
                                // How many pull requests it
                                // merged, readable without hovering.
                                match merged {
                                    0 => String::new(),
                                    m if narrow => format!(" · {m}"),
                                    m => format!(" · {m} merged"),
                                }
                            )))
                            .tooltip(super::tooltip::Tooltip::text(if open {
                                "Fold its sub-agents"
                            } else {
                                "Unfold its sub-agents"
                            }))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    if !this.replay.unfolded.remove(&id) {
                                        this.replay.unfolded.insert(id.clone());
                                    }
                                    cx.notify();
                                }),
                            )
                            .into_any_element(),
                    );
                }
                RowKind::Child(i, j) => {
                    let tr = &day.lanes[i].children[j];
                    let title = tr
                        .title
                        .as_deref()
                        .map(kit::title)
                        .unwrap_or_else(|| "Sub-agent".to_owned());
                    let id = tr.id.clone();
                    over.push(
                        div()
                            .id(SharedString::from(format!("child-{i}-{j}")))
                            .absolute()
                            .top(px(*top))
                            .left_0()
                            .w(px(head))
                            .h(px(*h))
                            .flex()
                            .items_center()
                            .pl(look.px(28.))
                            .pr(look.px(8.))
                            .text_size(look.text(12.))
                            .text_color(look.c.text_muted)
                            .cursor_pointer()
                            .hover(|h| h.text_color(look.c.text))
                            .active(|a| a.bg(look.c.hover))
                            .child(div().truncate().child(title))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.replay_open(id.clone(), None, window, cx);
                                }),
                            )
                            .into_any_element(),
                    );
                }
            }
        }
        // The now line and the scrubber.
        if let Some(nx) = now_x.filter(|nx| *nx >= 0. && *nx <= tl_w) {
            over.push(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px(head + nx))
                    .w(px(1.))
                    .bg(look.c.accent.opacity(0.6))
                    .into_any_element(),
            );
        }
        let shown = self.replay.pinned.or(self.replay.pointer);
        if let Some(t) = shown {
            let sx = x(t);
            if (0. ..=tl_w).contains(&sx) {
                over.push(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(px(head + sx))
                        .w(px(if self.replay.pinned.is_some() { 2. } else { 1. }))
                        .bg(if self.replay.pinned.is_some() {
                            look.c.accent
                        } else {
                            look.c.text_muted
                        })
                        .into_any_element(),
                );
            }
        }
        // A scroll thumb when the lanes run past the area.
        if total > area_h + 1. {
            let th = (area_h / total * area_h).max(24.);
            let ty = scroll / max_scroll.max(1.) * (area_h - th);
            over.push(
                div()
                    .absolute()
                    .right(px(1.))
                    .top(px(ty))
                    .w(px(3.))
                    .h(px(th))
                    .rounded(px(2.))
                    .bg(look.c.text_muted.opacity(0.18))
                    .into_any_element(),
            );
        }

        let time_at = {
            let area = self.replay.area.clone();
            move |pos: gpui::Point<Pixels>, view: (f64, f64)| -> Option<f64> {
                let b = area.get()?;
                let w = f32::from(b.size.width) - head;
                let rx = f32::from(pos.x - b.origin.x) - head;
                if rx < 0. || w <= 0. {
                    return None;
                }
                Some(view.0 + f64::from(rx / w) * (view.1 - view.0))
            }
        };
        let (t_move, t_up, t_wheel, t_pinch) =
            (time_at.clone(), time_at.clone(), time_at.clone(), time_at);
        div()
            .id("lanes")
            .role(Role::Grid)
            .aria_label("The day as lanes")
            .relative()
            .flex_1()
            .min_h(look.px(120.))
            .overflow_hidden()
            .cursor_crosshair()
            .child(painter)
            .children(over)
            .on_mouse_move(cx.listener(move |this, e: &gpui::MouseMoveEvent, _, cx| {
                let view = this.replay.view;
                if let (Some((x0, v0, moved)), Some(MouseButton::Left)) =
                    (this.replay.drag, e.pressed_button)
                {
                    let Some(b) = this.replay.area.get() else {
                        return;
                    };
                    let w = (f32::from(b.size.width) - head).max(1.);
                    let dx = f32::from(e.position.x) - x0;
                    let shift = f64::from(dx / w) * (v0.1 - v0.0);
                    this.replay.view = clamp_view((v0.0 - shift, v0.1 - shift));
                    this.replay.drag = Some((x0, v0, moved || dx.abs() > 3.));
                }
                this.replay.pointer = t_move(e.position, this.replay.view);
                let _ = view;
                cx.notify();
            }))
            .on_hover(cx.listener(|this, over: &bool, _, cx| {
                if !*over {
                    this.replay.pointer = None;
                    this.replay.drag = None;
                    cx.notify();
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, e: &gpui::MouseDownEvent, window, cx| {
                    window.focus(&this.fleet_focus, cx);
                    this.replay.drag = Some((f32::from(e.position.x), this.replay.view, false));
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, e: &gpui::MouseUpEvent, _, cx| {
                    let moved = this.replay.drag.take().is_some_and(|d| d.2);
                    if !moved {
                        let t = t_up(e.position, this.replay.view);
                        this.replay.pinned = match (this.replay.pinned, t) {
                            (Some(_), _) | (None, None) => None,
                            (None, Some(t)) => Some(t),
                        };
                        this.replay.selected = 0;
                    }
                    cx.notify();
                }),
            )
            .on_scroll(cx.listener(move |this, e: &gpui::ScrollEvent, _, cx| {
                let d = e.delta.pixel_delta(px(16.));
                let (dx, dy) = (f32::from(d.x), f32::from(d.y));
                let view = this.replay.view;
                if e.modifiers.platform || e.modifiers.control {
                    if let Some(at) = t_wheel(e.position, view) {
                        let factor = (f64::from(-dy) * 0.004).exp();
                        this.replay.view = zoom(view, at, factor);
                    }
                } else if dx.abs() > dy.abs() {
                    let Some(b) = this.replay.area.get() else {
                        return;
                    };
                    let w = (f32::from(b.size.width) - head).max(1.);
                    let shift = f64::from(dx / w) * (view.1 - view.0);
                    this.replay.view = clamp_view((view.0 - shift, view.1 - shift));
                } else {
                    this.replay.scroll = (this.replay.scroll - dy).max(0.);
                }
                cx.stop_propagation();
                cx.notify();
            }))
            .on_pinch(cx.listener(move |this, e: &gpui::PinchEvent, _, cx| {
                let view = this.replay.view;
                if let Some(at) = t_pinch(e.position, view) {
                    let factor = 1. / (1. + f64::from(e.delta)).max(0.2);
                    this.replay.view = zoom(view, at, factor);
                    cx.notify();
                }
            }))
    }

    /// The side panel (a strip under the lanes when narrow): what stood at
    /// the scrubber's minute.
    fn lanes_panel(
        &self,
        day: &Rc<Day>,
        narrow: bool,
        page_h: f32,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let now = OffsetDateTime::now_utc();
        let t = match self.replay.pinned.or(self.replay.pointer) {
            Some(s) => day.start + Duration::seconds_f64(s),
            None if now >= day.start && now < day.end => now,
            None => day
                .lanes
                .iter()
                .filter_map(|l| l.track.last)
                .max()
                .unwrap_or(day.start),
        };
        let idle = self.replay.pinned.is_none()
            && self.replay.pointer.is_none()
            && !(now >= day.start && now < day.end);
        let m = if idle {
            replay::Moment::default()
        } else {
            replay::moment(day, t)
        };
        let pinned = self.replay.pinned.is_some();
        // Under 1400 points and not pinned, one line under the
        // lanes; a pinned time opens it.
        if narrow && !pinned {
            let mut parts: Vec<String> = Vec::new();
            if !idle {
                parts.push(kit::hm(t));
                if !m.working.is_empty() {
                    parts.push(format!("Working {}", m.working.len()));
                }
                if !m.waiting.is_empty() {
                    parts.push(format!("Waiting on you {}", m.waiting.len()));
                }
                if !m.limited.is_empty() {
                    parts.push(format!("Stopped at a limit {}", m.limited.len()));
                }
                if m.subs_running > 0 {
                    parts.push(format!(
                        "{} sub-agent{} running",
                        m.subs_running,
                        if m.subs_running == 1 { "" } else { "s" }
                    ));
                }
                if parts.len() == 1 {
                    parts.push("Nothing was working".to_owned());
                }
            }
            let waiting = !m.waiting.is_empty();
            let text = if idle {
                "Point at the lanes to see what each thread was doing at that minute; a click pins it.".to_owned()
            } else {
                parts.join(" · ")
            };
            return div()
                .id("lanes-panel")
                .flex_shrink_0()
                .mt(look.px(8.))
                .px(look.px(12.))
                .py(look.px(7.))
                .rounded(look.px(8.))
                .border(look.fixed(1.))
                .border_color(look.c.border)
                .bg(look.c.surface)
                .flex()
                .items_center()
                .gap(look.px(8.))
                .text_size(look.text(12.5))
                .child(
                    div()
                        .flex_shrink_0()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(look.c.text)
                        .child("At this minute"),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(if waiting {
                            look.c.text
                        } else {
                            look.c.text_muted
                        })
                        .child(text),
                )
                .when(!idle, |d| {
                    d.child(div().flex_1()).child(
                        div()
                            .flex_shrink_0()
                            .text_size(look.text(11.5))
                            .text_color(look.c.faint)
                            .child("click to pin"),
                    )
                })
                .into_any_element();
        }
        let lane_head = |look: &Look, i: usize| {
            let lane = &day.lanes[i];
            let title = lane
                .track
                .title
                .as_deref()
                .map(kit::title)
                .unwrap_or_else(|| crate::rows::id_title(&lane.track.id));
            div()
                .flex()
                .items_center()
                .gap(look.px(6.))
                .min_w_0()
                .child(kit::mark(look, lane.profile.as_deref(), 16.))
                .child(
                    div()
                        .truncate()
                        .text_size(look.text(12.5))
                        .text_color(look.c.text)
                        .child(title),
                )
        };
        let group = |look: &Look, label: String, color: Hsla| {
            div()
                .pt(look.px(6.))
                .text_size(look.text(11.5))
                .font_weight(FontWeight::MEDIUM)
                .text_color(color)
                .child(label)
        };
        let mut body = div()
            .flex()
            .flex_col()
            .gap(look.px(4.))
            .px(look.px(14.))
            .pb(look.px(12.));
        if !m.waiting.is_empty() {
            body = body.child(group(
                look,
                format!("Waiting on you {}", m.waiting.len()),
                look.c.warn,
            ));
            for (i, d) in &m.waiting {
                body = body.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(look.px(8.))
                        .child(lane_head(look, *i))
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_size(look.text(12.))
                                .text_color(look.c.warn)
                                .child(span_words(*d)),
                        ),
                );
            }
        }
        if !m.working.is_empty() {
            body = body.child(group(
                look,
                format!("Working {}", m.working.len()),
                look.c.text_muted,
            ));
            for (k, on) in m.working.iter().enumerate() {
                let sel = pinned && k == self.replay.selected.min(m.working.len() - 1);
                let (session, ordinal) = (on.session.clone(), on.ordinal);
                body = body.child(
                    div()
                        .id(("panel-working", k))
                        .flex()
                        .flex_col()
                        .gap(look.px(1.))
                        .py(look.px(3.))
                        .px(look.px(6.))
                        .mx(look.px(-6.))
                        .rounded(look.px(6.))
                        .when(sel, |d| d.bg(look.c.selected))
                        .cursor_pointer()
                        .hover(|h| h.bg(look.c.hover))
                        .active(|a| a.bg(look.c.selected))
                        .child(lane_head(look, on.lane))
                        .children(on.words.clone().map(|w| {
                            div()
                                .pl(look.px(22.))
                                .truncate()
                                .text_size(look.text(12.))
                                .text_color(look.c.text_muted)
                                .child(w)
                        }))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.replay_open(session.clone(), ordinal, window, cx);
                        })),
                );
            }
        }
        if !m.limited.is_empty() {
            body = body.child(group(
                look,
                format!("Stopped at a limit {}", m.limited.len()),
                look.c.text_muted,
            ));
            for i in &m.limited {
                body = body.child(lane_head(look, *i));
            }
        }
        if m.subs_running > 0 {
            body = body.child(
                div()
                    .pt(look.px(6.))
                    .text_size(look.text(12.))
                    .text_color(look.c.text_muted)
                    .child(format!(
                        "{} sub-agent{} running",
                        m.subs_running,
                        if m.subs_running == 1 { "" } else { "s" }
                    )),
            );
        }
        if idle {
            body = body.child(
                div()
                    .text_size(look.text(12.5))
                    .text_color(look.c.text_muted)
                    .child("Point at the lanes to see what each thread was doing at that minute; a click pins it."),
            );
        } else if m.working.is_empty() && m.waiting.is_empty() && m.limited.is_empty() {
            body = body.child(
                div()
                    .text_size(look.text(12.5))
                    .text_color(look.c.text_muted)
                    .child("Nothing was working."),
            );
        }
        let fact = if pinned {
            Some(format!("pinned {} · Esc lets go", kit::hm(t)))
        } else if idle {
            None
        } else {
            Some(kit::hm(t))
        };
        let card = kit::pane_card(look, "At this minute", fact).child(body);
        if narrow {
            div()
                .id("lanes-panel")
                .flex_shrink_0()
                .max_h(px((page_h / 3.).max(120.)))
                .overflow_y_scroll()
                .pt(look.px(8.))
                .child(card)
                .into_any_element()
        } else {
            div()
                .id("lanes-panel")
                .flex_shrink_0()
                .w(look.px(PANEL_W))
                .overflow_y_scroll()
                .child(card)
                .into_any_element()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_keeps_the_time_under_the_pointer() {
        let v = zoom((0., DAY_SECS), 43_200., 0.25);
        assert!((v.1 - v.0 - DAY_SECS * 0.25).abs() < 1e-6);
        // The pointer's time keeps its place in the span.
        assert!(((43_200. - v.0) / (v.1 - v.0) - 0.5).abs() < 1e-9);
        let v = zoom((0., 3600.), 100., 0.01);
        assert_eq!(v.1 - v.0, MIN_SPAN);
        assert_eq!(clamp_view((-50., 3550.)), (0., 3600.));
        assert_eq!(
            clamp_view((DAY_SECS - 10., DAY_SECS + 3590.)),
            (DAY_SECS - 3600., DAY_SECS)
        );
    }

    #[test]
    fn ticks_follow_the_span() {
        let t = ticks((0., DAY_SECS), 400.);
        assert_eq!(t.len(), 9);
        assert_eq!(t[1].1, "03:00");
        assert_eq!(ticks((0., DAY_SECS), 100.).len(), 3);
        let t = ticks((3600., 7200.), 400.);
        assert_eq!(t.first().unwrap().1, "01:00");
        assert_eq!(t[1].1, "01:15");
    }

    #[test]
    fn words_and_names() {
        assert_eq!(span_words(Duration::minutes(100)), "1h 40m");
        assert_eq!(span_words(Duration::seconds(20)), "under a minute");
        let today = time::macros::date!(2026 - 11 - 11);
        assert_eq!(day_name(today, today), "Today");
        assert_eq!(
            day_name(time::macros::date!(2026 - 11 - 10), today),
            "Yesterday"
        );
        assert_eq!(
            day_name(time::macros::date!(2026 - 11 - 09), today),
            "Mon 9 Nov"
        );
    }

    #[test]
    fn a_refresh_keeps_the_lanes_order() {
        let lane = |id: &str| replay::Lane {
            track: replay::Track {
                id: id.into(),
                ..Default::default()
            },
            ..Default::default()
        };
        let mut day = Day {
            date: time::macros::date!(2026 - 11 - 11),
            start: OffsetDateTime::UNIX_EPOCH,
            end: OffsetDateTime::UNIX_EPOCH,
            read_at: OffsetDateTime::UNIX_EPOCH,
            lanes: vec![lane("c"), lane("a"), lane("b")],
            line: Default::default(),
        };
        keep_order(&mut day, &["a".into(), "b".into()]);
        let ids: Vec<&str> = day.lanes.iter().map(|l| l.track.id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "c"]);
    }
    #[test]
    fn chips_never_overlap_and_clusters_take_their_marks() {
        let w = |n: usize| if n == 1 { 14. } else { 22. };
        // Three marks a point apart, one far, two at the right edge.
        let xs = [10., 11., 12., 80., 398., 399.];
        let chips = layout_chips(&xs, &w, 400.);
        assert_eq!(chips.len(), 3, "{chips:?}");
        assert_eq!(chips[0].members, [0, 1, 2]);
        assert_eq!(chips[1].members, [3]);
        assert_eq!(chips[2].members, [4, 5]);
        for pair in chips.windows(2) {
            assert!(
                pair[1].left >= pair[0].left + pair[0].width + GAP,
                "{chips:?}"
            );
        }
        assert!(
            chips
                .iter()
                .all(|c| c.left >= 0. && c.left + c.width <= 400.)
        );
        // A crowd at one width becomes one chip.
        let xs: Vec<f32> = (0..40).map(|k| k as f32 * 2.).collect();
        let chips = layout_chips(&xs, &w, 100.);
        assert!(
            chips
                .windows(2)
                .all(|p| p[1].left >= p[0].left + p[0].width + GAP)
        );
        assert_eq!(chips.iter().map(|c| c.members.len()).sum::<usize>(), 40);
        assert_eq!(
            chip_kind(&[MarkKind::PullOpened, MarkKind::Question, MarkKind::Limit]),
            MarkKind::Question
        );
    }

    #[test]
    fn an_opened_and_merged_pull_request_is_merged_only_at_the_day_span() {
        let at = OffsetDateTime::UNIX_EPOCH;
        let m = |kind, url: &str| replay::Mark {
            at,
            kind,
            label: String::new(),
            target: Some(Target::Url(url.into())),
        };
        let marks = vec![
            m(MarkKind::PullOpened, "u/1"),
            m(MarkKind::PullMerged, "u/1"),
            m(MarkKind::PullOpened, "u/2"),
        ];
        let kinds: Vec<MarkKind> = day_marks(&marks).iter().map(|m| m.kind).collect();
        assert_eq!(kinds, [MarkKind::PullMerged, MarkKind::PullOpened]);
    }

    #[test]
    fn ruler_labels_never_touch() {
        let ticks: Vec<(f32, String)> = [0., 20., 50., 100., 150., 200.]
            .iter()
            .map(|x| (*x, format!("{x}")))
            .collect();
        let got = ruler_labels(&ticks, 30., 220., Some((140., 186.)));
        let lefts: Vec<f32> = got.iter().map(|(l, _)| *l).collect();
        // 20 and 50 come too close to 0; 150 and 200 touch the scrubber's
        // time.
        assert_eq!(lefts, [0., 85.]);
    }

    #[test]
    fn day_fits_the_hours_with_activity() {
        let h = 3600.;
        assert_eq!(fit_day(None, None), (0., DAY_SECS));
        // 07:20 to 22:40: 07:00 to 23:00.
        assert_eq!(fit_day(Some(7.33 * h), Some(22.66 * h)), (7. * h, 23. * h));
        // An hour of work becomes six hours around it.
        let (a, b) = fit_day(Some(12. * h + 600.), Some(13. * h));
        assert_eq!(b - a, 6. * h);
        assert!(a <= 12. * h && b >= 13. * h);
        // Work at midnight stays inside the day.
        assert_eq!(fit_day(Some(60.), Some(2. * h)), (0., 6. * h));
        assert_eq!(
            fit_day(Some(22. * h), Some(DAY_SECS - 60.)),
            (18. * h, DAY_SECS)
        );
    }
}
