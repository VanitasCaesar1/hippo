//! Pulse: Fleet's default view (docs/designs/fleet-pulse.md), drawn as
//! the instrument look (docs/designs/app-instrument.md).
//!
//! A header of readouts and account meters, the scope band of the
//! fleet's tool calls, then two panes: the queue (the sessions waiting on
//! the person, then what is coming up) and the selected item alone in a
//! focus pane. The working sessions sit in a sheet over the queue (`F`);
//! with nothing in the queue the focus pane shows the day's readouts and
//! its scope band. Below [`STACK_BELOW`] points the panes stack and `↵`
//! opens an item's focus.
//!
//! Today's lanes come from `hippo_view::replay` and the rest from
//! `hippo_view::pulse`, read on the background executor every 30
//! seconds while Pulse shows. The scope band, the sheet's bars and the
//! drain chart are painted on canvases. Only a waiting item's mark
//! blinks, and not when the system asks for reduced motion. Pulse writes
//! nothing: its actions open the place where an item is dealt with.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::AnimationExt as _;
use gpui::{
    Bounds, Context, FontWeight, Hsla, PathBuilder, ScrollHandle, SharedString, Window, canvas,
    div, fill, point, prelude::*, px, size,
};
use hippo_core::incidents::Incident;
use hippo_view::fleet::{Fleet, Row, State, Why};
use hippo_view::pulse::{self, Corrected, Next, Tonight};
use hippo_view::replay::{self, Day, Live};
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
const FOCUS_W: f32 = 680.;
/// The scope band in the header covers this much time.
const SCOPE_SPAN: Duration = Duration::hours(3);
/// A waiting session's calls shown before Show all.
const CALLS_SHOWN: usize = 3;
/// The sheet's bars cover this much time.
const SHEET_SPAN: Duration = Duration::minutes(30);
/// At most this many rows in the sheet.
const SHEET_ROWS: usize = 12;

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
    /// The one-second redraw while an item with a clock shows.
    pub tick: Option<gpui::Task<()>>,
}

impl Default for PulseUi {
    fn default() -> Self {
        PulseUi {
            on: true,
            data: None,
            loading: false,
            error: None,
            later: Vec::new(),
            sel: None,
            sheet: false,
            all_calls: false,
            focus_open: false,
            queue_scroll: ScrollHandle::new(),
            sheet_scroll: ScrollHandle::new(),
            width: Rc::new(Cell::new(0.)),
            still: None,
            tick: None,
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

impl Shell {
    fn pulse_today(&self) -> time::Date {
        OffsetDateTime::now_utc()
            .to_offset(super::status::local_offset())
            .date()
    }

    /// Reads today's lanes and the rest when missing or older than
    /// [`REFRESH`].
    pub(super) fn pulse_ensure(&mut self, cx: &mut Context<Self>) {
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
    pub(super) fn pulse_queue(&self, fleet: &Fleet) -> (Vec<Item>, Vec<Item>) {
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
    pub(super) fn pulse_select(&mut self, key: String) {
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

    /// Pulse is the view on screen: Fleet's own view, not a thread opened
    /// from it, with Pulse picked.
    pub(super) fn pulse_shown(&self) -> bool {
        self.state.tabs.section() == crate::state::Section::Fleet
            && self.state.tabs.depth() == 1
            && self.pulse.on
            && !self.replay.lanes
    }

    /// Escape in Pulse: closes the sheet, else the stacked focus. Escape
    /// is bound to Back, so the shell's interceptor calls this first;
    /// `false` when Pulse is not the view shown or has nothing to close.
    pub(super) fn pulse_escape(&mut self) -> bool {
        if !self.pulse_shown() {
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
    pub(crate) fn pulse_terminal(&mut self, row: &Row, fleet: &Fleet) {
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

    /// `PULSE LANES ROWS`: labels, the shown one underlined.
    pub(super) fn fleet_view_switch(&self, look: &Look, cx: &mut Context<Self>) -> gpui::Div {
        let lanes = self.replay.lanes;
        let pulse = !lanes && self.pulse.on;
        let rows = !lanes && !self.pulse.on;
        kit::switches(look)
            .child(
                kit::switch(look, "fleet-pulse", "Pulse", pulse).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.replay.lanes = false;
                        this.pulse.on = true;
                        cx.notify();
                    },
                )),
            )
            .child(
                kit::switch(look, "fleet-lanes", "Lanes", lanes)
                    .tooltip(super::tooltip::Tooltip::text("The day as lanes (L)"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.replay.lanes = true;
                        cx.notify();
                    })),
            )
            .child(
                kit::switch(look, "fleet-rows", "Rows", rows).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.replay.lanes = false;
                        this.pulse.on = false;
                        cx.notify();
                    },
                )),
            )
    }

    /// Redraws every second while Pulse shows an item with a clock (a
    /// waiting session's timer, a countdown to empty).
    fn pulse_tick(&mut self, cx: &mut Context<Self>) {
        if self.pulse.tick.is_some() {
            return;
        }
        self.pulse.tick = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(1))
                    .await;
                let Ok(go) = this.update(cx, |this, cx| {
                    let timed = this.pulse_shown()
                        && this
                            .loaded
                            .as_ref()
                            .and_then(|l| l.fleet.clone())
                            .is_some_and(|f| {
                                let (items, sel) = this.pulse_items(&f);
                                matches!(
                                    sel.and_then(|i| items.get(i)),
                                    Some(Item::Wait(_) | Item::Next(Next::RunsDry { .. }))
                                )
                            });
                    if timed {
                        cx.notify();
                    }
                    this.pulse_shown()
                }) else {
                    return;
                };
                if !go {
                    this.update(cx, |this, _| this.pulse.tick = None).ok();
                    return;
                }
            }
        }));
    }

    pub(super) fn render_pulse(
        &mut self,
        fleet: &Fleet,
        look: &Look,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        self.pulse_ensure(cx);
        self.pulse_tick(cx);
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
        let (needs, next) = self.pulse_queue(fleet);
        let items: Vec<Item> = needs.iter().chain(next.iter()).cloned().collect();
        let keys: Vec<String> = items.iter().map(Item::key).collect();
        let sel = selected(&keys, self.pulse.sel.as_deref());
        let totals = pulse::totals(fleet, data.as_ref().map(|d| d.day.as_ref()), now);

        let header = self.pulse_header(fleet, needs.len(), &totals, now, stacked, look, cx);
        let scope = scope_band(fleet, data.as_deref(), now - SCOPE_SPAN, now, look, 56.);
        let queue = self.pulse_queue_pane(
            fleet, &needs, &next, sel, &totals, stacked, queue_w, now, look, cx,
        );
        let focus = match sel.and_then(|i| items.get(i)) {
            Some(item) => self.pulse_focus(item, fleet, data.as_deref(), now, stacked, look, cx),
            None => self.pulse_clear(fleet, &totals, data.as_deref(), now, look),
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
                            .w_full()
                            .px(look.px(if stacked { 20. } else { 48. }))
                            .pt(look.px(if stacked { 20. } else { 40. }))
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
            .child(header)
            .child(
                div()
                    .flex_shrink_0()
                    .px(look.px(20.))
                    .border_b(look.fixed(1.))
                    .border_color(look.c.border)
                    .child(scope),
            )
            .child(body)
            .into_any_element()
    }

    /// The header: the switcher, then the readouts (waiting, working) and
    /// each account's meter: 10 segments of what is left, red with a
    /// countdown when it runs dry.
    #[allow(clippy::too_many_arguments)]
    fn pulse_header(
        &self,
        fleet: &Fleet,
        waiting: usize,
        t: &pulse::Totals,
        now: OffsetDateTime,
        stacked: bool,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let at = |s: i64| OffsetDateTime::from_unix_timestamp(s).unwrap_or(now);
        let read = |label: &str, n: usize, color: Hsla| {
            div()
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap(look.px(8.))
                .child(kit::cap(look, label, look.c.faint))
                .child(kit::figure(look, two(n), 13., color))
        };
        let meters = rail_profiles(fleet).into_iter().filter_map(|p| {
            let h = fleet.room(&p)?;
            let left = h.five.map(|l| l.frac).or_else(|| h.left())?.clamp(0., 1.);
            let dry = fleet.drain(&p).and_then(|d| d.empty_at);
            let reset = h.five.and_then(|l| l.resets_at).or_else(|| h.resets());
            let tip = format!(
                "{}: {}% of the 5-hour window left{}{}",
                kit::profile_name(&p),
                (left * 100.).round(),
                dry.map(|e| format!(", empty about {}", kit::hm(at(e))))
                    .unwrap_or_default(),
                reset
                    .map(|r| format!(", resets {}", kit::hm(at(r))))
                    .unwrap_or_default()
            );
            let color = if dry.is_some() {
                look.c.err
            } else {
                kit::profile_tone(look)
            };
            Some(
                div()
                    .id(SharedString::from(format!("pulse-meter-{p}")))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(look.px(8.))
                    .tooltip(super::tooltip::Tooltip::text(tip))
                    .child(kit::cap(look, &kit::profile_name(&p), look.c.faint))
                    .child(kit::segments(look, left, color))
                    .child(kit::figure(
                        look,
                        format!("{}%", (left * 100.).round()),
                        12.,
                        if dry.is_some() {
                            look.c.err
                        } else {
                            look.c.text
                        },
                    ))
                    .children(dry.map(|e| kit::cap(look, &t_minus(at(e) - now), look.c.err))),
            )
        });
        div()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(look.px(26.))
            .h(look.px(44.))
            .px(look.px(20.))
            .border_b(look.fixed(1.))
            .border_color(look.c.border)
            .overflow_hidden()
            .child(self.fleet_view_switch(look, cx))
            .children(self.render_reach_said(look))
            .child(div().flex_1())
            .child(read(
                "Waiting",
                waiting,
                if waiting > 0 {
                    look.c.warn
                } else {
                    look.c.text
                },
            ))
            .child(read("Working", t.working, look.c.text))
            .when(!stacked, |d| d.children(meters))
    }

    /// The queue pane: Waiting on you, then Coming up, each with its
    /// count; the foot opens the fleet sheet, which covers the pane.
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
        let mut list = div().flex().flex_col().pb(look.px(12.));
        if needs.is_empty() && next.is_empty() {
            list = list.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(look.px(6.))
                    .px(look.px(20.))
                    .pt(look.px(18.))
                    .child(kit::cap(look, "All clear", look.c.ok))
                    .child(
                        div()
                            .text_color(look.c.text_muted)
                            .child("Nothing needs you."),
                    ),
            );
        }
        let mut i = 0;
        for (title, group_items) in [("Waiting on you", needs), ("Coming up", next)] {
            if group_items.is_empty() {
                continue;
            }
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(look.px(20.))
                    .pt(look.px(16.))
                    .pb(look.px(8.))
                    .child(kit::cap(look, title, look.c.faint))
                    .child(kit::cap(look, &two(group_items.len()), look.c.text)),
            );
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
                    .px(look.px(20.))
                    .pt(look.px(14.))
                    .text_size(look.text(12.))
                    .text_color(look.c.faint)
                    .child(format!("Today’s lanes not read: {e}")),
            );
        }
        let foot = div()
            .id("pulse-fleet-row")
            .role(gpui::Role::Button)
            .aria_label("The fleet")
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(look.px(22.))
            .h(look.px(44.))
            .px(look.px(20.))
            .border_t(look.fixed(1.))
            .border_color(look.c.border)
            .cursor_pointer()
            .hover(|h| h.bg(look.c.hover))
            .tooltip(super::tooltip::Tooltip::text("The fleet (F)"))
            .on_click(cx.listener(|this, _, _, cx| {
                this.pulse.sheet = !this.pulse.sheet;
                cx.notify();
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(6.))
                    .child(kit::cap(look, "Working", look.c.faint))
                    .child(kit::figure(look, two(totals.working), 11.5, look.c.text)),
            )
            .when(totals.subs > 0, |d| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(look.px(6.))
                        .child(kit::cap(look, "Sub-agents", look.c.faint))
                        .child(kit::figure(look, two(totals.subs), 11.5, look.c.text)),
                )
            })
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(3.))
                    .child(kit::cap(look, "Fleet", look.c.faint))
                    .child(kit::icon(look, "arrow-right", 11., look.c.faint)),
            );
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
            .child(foot);
        if self.pulse.sheet {
            pane.child(self.pulse_sheet(fleet, now, look, cx))
        } else {
            pane
        }
    }

    /// One queue row: its mark, the title, its age and up to two lines of
    /// reason. Hippo writes the title of the Coming up kinds.
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
        let _ = fleet;
        let (mark, title, reason, age) = item_words(item, now);
        let square = kit::square(look, mark);
        let square: gpui::AnyElement = if mark == kit::Mark::Wait && !still {
            square
                .with_animation(
                    SharedString::from(format!("pulse-mark-{}", item.key())),
                    gpui::Animation::new(std::time::Duration::from_millis(2000)).repeat(),
                    |d, t| d.opacity(if t < 0.5 { 1. } else { 0.3 }),
                )
                .into_any_element()
        } else {
            square.into_any_element()
        };
        let key = item.key();
        div()
            .id(("pulse-q", i))
            .relative()
            .flex()
            .items_start()
            .gap(look.px(10.))
            .px(look.px(20.))
            .pt(look.px(11.))
            .pb(look.px(12.))
            .border_t(look.fixed(1.))
            .border_color(look.c.border.opacity(0.6))
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
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(look.fixed(2.))
                        .bg(look.c.text),
                )
            })
            .child(div().flex_shrink_0().pt(look.px(5.)).child(square))
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
                            .items_start()
                            .gap(look.px(10.))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(look.c.text)
                                    .child(title),
                            )
                            .child(kit::figure(look, age, 11., look.c.faint).pt(look.px(1.))),
                    )
                    .child(
                        div()
                            .text_color(look.c.text_muted)
                            .line_clamp(2)
                            .text_ellipsis()
                            .child(reason),
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
                .gap(look.px(8.))
                .mb(look.px(18.))
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.pulse.focus_open = false;
                    cx.notify();
                }))
                .child(kit::icon(look, "arrow-left", 12., look.c.text_muted))
                .child(kit::cap(look, "Queue", look.c.text_muted))
                .child(kit::cap(look, "Esc", look.c.faint))
        });
        let body = match item {
            Item::Wait(r) => self.pulse_focus_wait(r, data, now, look, cx),
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

    /// A waiting session: its state and timer, what it asked (or its
    /// plan), the ways in, a table of what it is on, and its last calls.
    fn pulse_focus_wait(
        &self,
        r: &Row,
        data: Option<&PulseData>,
        now: OffsetDateTime,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let plan = matches!(r.state, State::Waiting { plan: true, .. });
        let (state, since) = match r.state {
            State::Waiting { at, .. } => (if plan { "Plan waiting" } else { "Waiting" }, Some(at)),
            State::Stopped { at, .. } => ("Stopped at its limit", Some(at)),
            _ => ("Needs you", None),
        };
        let place = [
            r.project.clone(),
            r.profile.as_deref().map(kit::profile_name),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
        let top = focus_top(
            look,
            kit::cap(look, state, look.c.warn),
            since.map(|t| kit::figure(look, clock(now - t), 12., look.c.warn)),
            &place,
        );
        let asked = match &r.state {
            State::Waiting { asked, .. } => asked.clone(),
            State::Stopped { reset, .. } => reset.clone().map(|r| format!("Usage limit · {r}")),
            _ => None,
        };
        // The question, or the plan.
        let ask: gpui::AnyElement = match (plan, r.plan.as_ref()) {
            (true, Some(p)) if !p.steps.is_empty() => {
                let mut t = kit::spec(look).mt(look.px(22.));
                for (n, s) in p.steps.iter().enumerate() {
                    t = t.child(kit::spec_row(look, &two(n + 1), s.clone(), None));
                }
                if p.more > 0 {
                    t = t.child(kit::spec_row(
                        look,
                        "More",
                        div()
                            .text_color(look.c.text_muted)
                            .child(format!("{} more in the thread", p.more)),
                        None,
                    ));
                }
                div()
                    .child(
                        div()
                            .mt(look.px(4.))
                            .text_color(look.c.text_muted)
                            .child("It wants approval for this plan."),
                    )
                    .child(t)
                    .into_any_element()
            }
            (true, Some(p)) if p.text.is_some() => quote_bar(
                look,
                short(p.text.as_deref().unwrap_or_default(), 900),
                look.c.warn,
                14.,
            ),
            _ => quote_bar(
                look,
                asked.map_or_else(|| "It is waiting on you.".to_owned(), |q| format!("“{q}”")),
                look.c.warn,
                17.,
            ),
        };
        // The ways in.
        let id = r.id.clone();
        let (row, row2, row3) = (r.clone(), r.clone(), r.clone());
        let key = Item::Wait(Box::new(r.clone())).key();
        let actions = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(look.px(8.))
            .mt(look.px(26.))
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
                    kit::button_key(
                        look,
                        SharedString::from(format!("focus-t3-{id}")),
                        "T3 Code",
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
                kit::button_key(
                    look,
                    SharedString::from(format!("focus-term-{id}")),
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
            )
            .child(self.later_button(look, key, cx));
        // What it is on.
        let mut spec = kit::spec(look).mt(look.px(34.));
        let mut rows = 0;
        if let Some(b) = &r.branch {
            spec = spec.child(kit::spec_row(
                look,
                "Branch",
                kit::figure(look, kit::middle(b, 56), 12., look.c.text),
                None,
            ));
            rows += 1;
        }
        let opened = r.pulls.iter().rev().find(|(_, d)| *d == "opened").cloned();
        let merged = r.pulls.iter().rev().find(|(_, d)| *d == "merged").cloned();
        if let Some((url, done)) = merged.clone().or(opened.clone()) {
            let lines = r.total_lines();
            spec = spec.child(kit::spec_row(
                look,
                "Pull request",
                div()
                    .flex()
                    .flex_wrap()
                    .gap(look.px(6.))
                    .child(format!(
                        "#{}",
                        super::sidebar::pr_number(&url).unwrap_or("?")
                    ))
                    .children(lines.map(|l| {
                        div().text_color(look.c.text_muted).child(format!(
                            "· +{} −{} in {}",
                            l.added,
                            l.removed,
                            plural(l.files as usize, "file", "files")
                        ))
                    })),
                Some(if done == "merged" {
                    ("Merged".to_owned(), look.c.ok)
                } else {
                    ("Open".to_owned(), look.c.text_muted)
                }),
            ));
            rows += 1;
        } else if let Some(l) = r.total_lines() {
            spec = spec.child(kit::spec_row(
                look,
                "Changed",
                format!(
                    "+{} −{} in {}",
                    l.added,
                    l.removed,
                    plural(l.files as usize, "file", "files")
                ),
                None,
            ));
            rows += 1;
        }
        if r.claims.verified + r.claims.unverified > 0 {
            let (value, status) = match &r.claims.first_unverified {
                Some((claim, why)) => (
                    format!("“{claim}”: {why}"),
                    ("Unverified".to_owned(), look.c.warn),
                ),
                None => (
                    format!(
                        "{} checked against what ran",
                        plural(r.claims.verified as usize, "claim", "claims")
                    ),
                    ("Checked".to_owned(), look.c.ok),
                ),
            };
            spec = spec.child(kit::spec_row(look, "Claims", value, Some(status)));
            rows += 1;
        }
        if !r.rules.is_empty() {
            spec = spec.child(kit::spec_row(
                look,
                "Started with",
                div()
                    .flex()
                    .flex_col()
                    .gap(look.px(2.))
                    .children(r.rules.iter().map(|s| s.title.clone())),
                Some((plural(r.rules.len(), "rule", "rules"), look.c.text_muted)),
            ));
            rows += 1;
        }
        // Its last calls.
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
        let more = calls.len().saturating_sub(shown);
        let log = (!calls.is_empty()).then(|| {
            div()
                .flex()
                .flex_col()
                .mt(look.px(28.))
                .child(kit::cap(look, "Last calls", look.c.faint).mb(look.px(6.)))
                .children(calls.iter().take(shown).rev().map(|c| {
                    div()
                        .flex()
                        .items_center()
                        .gap(look.px(12.))
                        .h(look.px(28.))
                        .child(kit::cap(look, &kit::hm(c.at), look.c.faint).w(look.px(44.)))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(kit::tool_line(look, c, 1, None, false)),
                        )
                }))
                .when(more > 0, |d| {
                    d.child(
                        div()
                            .id("pulse-all-calls")
                            .mt(look.px(6.))
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.pulse.all_calls = true;
                                cx.notify();
                            }))
                            .child(kit::cap(
                                look,
                                &format!("+ {more} earlier"),
                                look.c.text_muted,
                            )),
                    )
                })
        });
        div()
            .flex()
            .flex_col()
            .child(top)
            .child(focus_title(look, &kit::title(&r.title)))
            .child(ask)
            .child(actions)
            .when(rows > 0, |d| d.child(spec))
            .children(log)
            .into_any_element()
    }

    /// An account that runs dry: the countdown, its drain, the figures,
    /// the sessions that will stop and the account to resume them on.
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
        let hue = kit::profile_tone(look);
        let name = kit::profile_name(profile);
        let top = focus_top(
            look,
            kit::cap(look, "Runs dry", look.c.err),
            None,
            &format!("{name} · 5-hour window"),
        );
        let stopping: Vec<&Row> = sessions.iter().filter_map(|id| fleet.row(id)).collect();
        let mut spec = kit::spec(look).mt(look.px(26.));
        if let Some(d) = d {
            spec = spec
                .child(kit::spec_row(
                    look,
                    "Left",
                    format!("{}%", ((1. - d.used).max(0.) * 100.).round()),
                    None,
                ))
                .child(kit::spec_row(
                    look,
                    "Rate",
                    div()
                        .flex()
                        .gap(look.px(6.))
                        .child(format!("{}% an hour", (d.burn * 100.).round()))
                        .child(
                            div()
                                .text_color(look.c.text_muted)
                                .child("· last 30 minutes"),
                        ),
                    None,
                ));
        }
        spec = spec.child(kit::spec_row(look, "Resets", kit::hm(at(*reset)), None));
        if !stopping.is_empty() {
            spec = spec.child(kit::spec_row(
                look,
                "Will stop",
                div()
                    .flex()
                    .flex_col()
                    .gap(look.px(2.))
                    .children(stopping.iter().map(|r| kit::title(&r.title))),
                Some((
                    plural(stopping.len(), "session", "sessions"),
                    look.c.text_muted,
                )),
            ));
        }
        let room = to.as_ref().and_then(|t| {
            let h = fleet.room(t)?;
            let left = h.five.map(|l| l.frac).or_else(|| h.left())?;
            let reset = h.five.and_then(|l| l.resets_at).or_else(|| h.resets());
            Some(format!(
                "{} has {}% left{}. Each carries on where it stopped.",
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
            .gap(look.px(8.))
            .mt(look.px(26.))
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
            .child(top)
            .child(
                kit::figure(
                    look,
                    format!("T−{}", clock(at(*empty_at) - now)),
                    40.,
                    look.c.err,
                )
                .line_height(look.text(44.))
                .mt(look.px(14.)),
            )
            .child(focus_title(
                look,
                &format!("{name} runs dry at {}", kit::hm(at(*empty_at))),
            ))
            .child(
                div()
                    .mt(look.px(4.))
                    .text_color(look.c.text_muted)
                    .child(format!(
                        "{} before it resets. {} on it will stop.",
                        kit::span(at(*reset) - at(*empty_at)),
                        sentence_count(stopping.len(), "session", "sessions")
                    )),
            )
            .children(d.map(|d| drain_chart(look, d, hue, now, 160.)))
            .child(spec)
            .child(actions)
            .children(room.map(|t| {
                div()
                    .mt(look.px(12.))
                    .text_color(look.c.text_muted)
                    .child(t)
            }))
            .into_any_element()
    }

    /// An unverified claim, a destructive command or a rule to keep: the
    /// one fact, its evidence as a table and the one action.
    fn pulse_focus_next(
        &self,
        n: &Next,
        fleet: &Fleet,
        now: OffsetDateTime,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let place = |session: &str| {
            fleet
                .row(session)
                .map(|r| {
                    [
                        r.project.clone(),
                        r.profile.as_deref().map(kit::profile_name),
                    ]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" · ")
                })
                .unwrap_or_default()
        };
        let (top, title, fact, spec, action): (
            gpui::Div,
            String,
            Option<gpui::AnyElement>,
            gpui::Div,
            &str,
        ) = match n {
            Next::Unverified {
                session,
                title,
                claim,
                why,
                at,
            } => (
                focus_top(
                    look,
                    kit::cap(look, "Unverified", look.c.warn),
                    None,
                    &format!("{} · finished {}", place(session), kit::hm(*at)),
                ),
                kit::title(title),
                Some(quote_bar(
                    look,
                    format!("It said “{claim}”"),
                    look.c.faint,
                    17.,
                )),
                kit::spec(look).mt(look.px(26.)).child(kit::spec_row(
                    look,
                    "Evidence",
                    why.clone(),
                    Some(("Unverified".to_owned(), look.c.warn)),
                )),
                "Check it",
            ),
            Next::Destructive {
                session,
                title,
                command,
                at,
                ..
            } => (
                focus_top(
                    look,
                    kit::cap(look, "Destructive", look.c.err),
                    None,
                    &place(session),
                ),
                harm_title(command),
                Some(
                    div()
                        .mt(look.px(22.))
                        .p(look.px(12.))
                        .bg(look.c.surface)
                        .border(look.fixed(1.))
                        .border_color(look.c.border)
                        .font_family(kit::MONO)
                        .text_size(look.text(12.5))
                        .text_color(look.c.text)
                        .child(format!(
                            "$ {}",
                            short(command.strip_prefix("Bash: ").unwrap_or(command), 400)
                        ))
                        .into_any_element(),
                ),
                kit::spec(look)
                    .mt(look.px(22.))
                    .child(kit::spec_row(
                        look,
                        "Session",
                        title.as_deref().map_or("—".into(), kit::title),
                        None,
                    ))
                    .child(kit::spec_row(
                        look,
                        "Harm",
                        format!("{} · {} ago", kit::hm(*at), kit::span(now - *at)),
                        Some(("Destructive".to_owned(), look.c.err)),
                    )),
                "Open the incident",
            ),
            Next::Correction {
                session,
                said,
                at,
                seen,
                ..
            } => (
                focus_top(
                    look,
                    kit::cap(look, "Rule candidate", look.c.text_muted),
                    None,
                    &format!("{} today", plural(*seen, "correction", "corrections")),
                ),
                "A rule worth keeping".to_owned(),
                Some(quote_bar(look, format!("“{said}”"), look.c.faint, 17.)),
                kit::spec(look)
                    .mt(look.px(26.))
                    .child(kit::spec_row(
                        look,
                        &kit::hm(*at),
                        fleet
                            .row(session)
                            .map_or_else(|| "A session".to_owned(), |r| kit::title(&r.title)),
                        Some(("Last".to_owned(), look.c.text_muted)),
                    ))
                    .child(kit::spec_row(
                        look,
                        "Kept",
                        "It becomes a rule new sessions start with.",
                        None,
                    )),
                "Keep it",
            ),
            Next::RunsDry { .. } => return div().into_any_element(),
        };
        let n2 = n.clone();
        let key = Item::Next(n.clone()).key();
        let actions = div()
            .flex()
            .items_center()
            .gap(look.px(8.))
            .mt(look.px(26.))
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
            .child(top)
            .child(focus_title(look, &title))
            .children(fact)
            .child(spec)
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
        kit::button_key(look, "focus-later", "Later", "S", Kind::Plain).on_click(cx.listener(
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

    /// Nothing in the queue: the day's readouts over the day's scope band.
    fn pulse_clear(
        &self,
        fleet: &Fleet,
        t: &pulse::Totals,
        data: Option<&PulseData>,
        now: OffsetDateTime,
        look: &Look,
    ) -> gpui::AnyElement {
        let line = if t.working > 0 {
            format!(
                "{} working. Pulse raises the next thing as it happens.",
                sentence_count(t.working, "session is", "sessions are")
            )
        } else {
            "Nothing is working.".to_owned()
        };
        let mut cells = vec![
            ("Tool calls 5 min".to_owned(), t.calls_lately.to_string()),
            ("Agent work today".to_owned(), kit::span(t.work)),
        ];
        if t.added + t.removed > 0 {
            cells.push((
                "Lines today".to_owned(),
                format!("+{} −{}", thousands(t.added), thousands(t.removed)),
            ));
        }
        if let Some(n) = data.and_then(|d| d.tonight.as_ref()) {
            cells.push(("Tonight".to_owned(), n.at.clone()));
        }
        let day_start = data.map_or(now - Duration::hours(6), |d| {
            d.day
                .lanes
                .iter()
                .filter_map(|l| l.track.first)
                .min()
                .map_or(d.day.start, |f| {
                    (f - Duration::minutes(10)).max(d.day.start)
                })
        });
        div()
            .flex()
            .flex_col()
            .child(focus_top(
                look,
                kit::cap(look, "All clear", look.c.ok),
                None,
                "",
            ))
            .child(focus_title(look, "Nothing needs you."))
            .child(
                div()
                    .mt(look.px(4.))
                    .text_color(look.c.text_muted)
                    .child(line),
            )
            .child(
                readouts(
                    look,
                    cells
                        .into_iter()
                        .map(|(l, v)| (l, v, look.c.text, None))
                        .collect(),
                )
                .mt(look.px(26.)),
            )
            .child(
                div()
                    .mt(look.px(20.))
                    .child(scope_band(fleet, data, day_start, now, look, 80.)),
            )
            .into_any_element()
    }

    /// The sheet over the queue: each working session with its current
    /// call and its last 30 minutes; the foot counts the rest.
    fn pulse_sheet(
        &self,
        fleet: &Fleet,
        now: OffsetDateTime,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let data = self.pulse.data.clone();
        let shelves = crate::shelving::Shelves::of(fleet, &self.marks());
        let working: Vec<&Row> = fleet.rows.iter().filter(|r| r.working()).collect();
        let quiet = shelves.shelves[1].1.iter().filter(|r| !r.working()).count();
        let review = shelves.shelves[2].1.len();
        let settled = shelves.shelves[3].1.len();
        let t0 = now - SHEET_SPAN;
        let mut list = div().flex().flex_col();
        if working.is_empty() {
            list = list.child(
                div()
                    .px(look.px(20.))
                    .py(look.px(10.))
                    .text_color(look.c.faint)
                    .child("Nothing is working."),
            );
        }
        for r in working.iter().take(SHEET_ROWS) {
            let beats = data
                .as_ref()
                .and_then(|d| pulse::lane_beats(&d.day, &r.id, t0, now))
                .map(|(b, _)| b)
                .unwrap_or_default();
            let hue = r
                .profile
                .as_deref()
                .map_or(look.c.text_muted, |_| kit::profile_tone(look));
            let lk = look.clone();
            let bars = canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    let o = bounds.origin;
                    let w = f32::from(bounds.size.width);
                    let h = f32::from(bounds.size.height);
                    let n = 15usize;
                    let slot = w / n as f32;
                    // The last 15 beats, merged two to one when finer.
                    let per = beats.len().div_ceil(n).max(1);
                    let vals: Vec<u16> = beats
                        .chunks(per)
                        .map(|c| c.iter().copied().max().unwrap_or(0))
                        .collect();
                    let vals = &vals[vals.len().saturating_sub(n)..];
                    let off = n - vals.len();
                    for (i, v) in vals.iter().enumerate() {
                        let bh = (bar_frac(u32::from(*v)) * h).max(1.);
                        window.paint_quad(fill(
                            Bounds::new(
                                point(o.x + px((off + i) as f32 * slot), o.y + px(h - bh)),
                                size(px((slot - 1.).max(1.)), px(bh)),
                            ),
                            if *v == 0 { lk.c.border } else { hue },
                        ));
                    }
                },
            )
            .flex_shrink_0()
            .w(look.px(60.))
            .h(look.px(16.));
            let subs = r.live_subs.len();
            let now_line = match &r.call {
                Some(c) => match &c.target {
                    Some(t) if matches!(c.verb.as_str(), "Edit" | "Write" | "Read") => {
                        format!("{} {}", c.verb, kit::short_path(t))
                    }
                    Some(t) => format!("{} {}", c.verb, short(t, 80)),
                    None => c.label.clone(),
                },
                None => r.activity.clone().unwrap_or_default(),
            };
            let now_line = if subs > 0 {
                format!("{now_line} · {}", plural(subs, "sub-agent", "sub-agents"))
            } else {
                now_line
            };
            let id = r.id.clone();
            list = list.child(
                div()
                    .id(SharedString::from(format!("pulse-sheet-{id}")))
                    .flex()
                    .items_center()
                    .gap(look.px(12.))
                    .px(look.px(20.))
                    .py(look.px(10.))
                    .border_t(look.fixed(1.))
                    .border_color(look.c.border.opacity(0.6))
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
                                    .truncate()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(look.c.text)
                                    .child(kit::title(&r.title)),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .font_family(kit::MONO)
                                    .text_size(look.text(11.5))
                                    .text_color(look.c.text_muted)
                                    .child(now_line),
                            ),
                    )
                    .child(bars),
            );
        }
        if working.len() > SHEET_ROWS {
            list = list.child(div().px(look.px(20.)).py(look.px(8.)).child(kit::cap(
                look,
                &format!("{} more in Rows", working.len() - SHEET_ROWS),
                look.c.faint,
            )));
        }
        let rows_link = |id: &'static str, label: String| {
            div()
                .id(id)
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.pulse.sheet = false;
                    this.replay.lanes = false;
                    this.pulse.on = false;
                    cx.notify();
                }))
                .child(kit::cap(look, &label, look.c.text_muted))
        };
        div()
            .id("pulse-sheet")
            .absolute()
            .inset_0()
            .flex()
            .flex_col()
            .bg(look.c.canvas)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(8.))
                    .px(look.px(20.))
                    .pt(look.px(16.))
                    .pb(look.px(8.))
                    .child(kit::cap(look, "Working", look.c.faint))
                    .child(kit::cap(look, &two(working.len()), look.c.text))
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("pulse-sheet-close")
                            .cursor_pointer()
                            .tooltip(super::tooltip::Tooltip::text("Esc"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.pulse.sheet = false;
                                cx.notify();
                            }))
                            .child(kit::cap(look, "Close", look.c.text)),
                    ),
            )
            .child(
                div()
                    .id("pulse-sheet-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.pulse.sheet_scroll)
                    .child(list),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(look.px(22.))
                    .h(look.px(44.))
                    .px(look.px(20.))
                    .border_t(look.fixed(1.))
                    .border_color(look.c.border)
                    .child(rows_link(
                        "pulse-sheet-review",
                        format!("To review {}", two(review)),
                    ))
                    .child(rows_link(
                        "pulse-sheet-quiet",
                        format!("Quiet {}", two(quiet)),
                    ))
                    .child(rows_link(
                        "pulse-sheet-settled",
                        format!("Settled {}", two(settled)),
                    )),
            )
            .into_any_element()
    }
}

/// An item's mark, title, reason and age as the queue and Home show
/// them. Hippo writes the title of the Coming up kinds.
pub(super) fn item_words(item: &Item, now: OffsetDateTime) -> (kit::Mark, String, String, String) {
    let at = |s: i64| OffsetDateTime::from_unix_timestamp(s).unwrap_or(now);
    match item {
        Item::Wait(r) => {
            let (_, asked, since) = need_words(r, now);
            let plan = matches!(r.state, State::Waiting { plan: true, .. });
            let reason = if plan {
                match r.plan.as_ref().map(|p| p.steps.len() + p.more) {
                    Some(n) if n > 0 => format!("A plan with {n} steps is waiting for approval."),
                    _ => "A plan is waiting for approval.".to_owned(),
                }
            } else {
                asked.map_or_else(|| "It is waiting on you.".to_owned(), |q| format!("“{q}”"))
            };
            (kit::Mark::Wait, kit::title(&r.title), reason, since)
        }
        Item::Next(Next::RunsDry {
            profile,
            empty_at,
            reset,
            sessions,
            ..
        }) => (
            kit::Mark::Dry,
            format!(
                "{} runs dry at {}",
                kit::profile_name(profile),
                kit::hm(at(*empty_at))
            ),
            format!(
                "{} will stop, {} before the reset.",
                sentence_count(sessions.len(), "session", "sessions"),
                kit::span(at(*reset) - at(*empty_at))
            ),
            t_minus(at(*empty_at) - now),
        ),
        Item::Next(Next::Unverified {
            title, claim, at, ..
        }) => (
            kit::Mark::Note,
            kit::title(title),
            format!(
                "Says “{}”, but nothing it ran shows it.",
                claim.trim_end_matches(['.', '!'])
            ),
            kit::hm(*at),
        ),
        Item::Next(Next::Destructive {
            title, command, at, ..
        }) => (
            kit::Mark::Harm,
            harm_title(command),
            format!(
                "In “{}”: {}",
                title.as_deref().map_or("a session".into(), kit::title),
                kit::command_words(command.strip_prefix("Bash: ").unwrap_or(command))
            ),
            kit::hm(*at),
        ),
        Item::Next(Next::Correction { said, at, seen, .. }) => (
            kit::Mark::Note,
            "A rule worth keeping".to_owned(),
            format!(
                "“{}” Corrected {} today.",
                short(said, 90),
                if *seen > 1 {
                    format!("{seen} times")
                } else {
                    "once".to_owned()
                }
            ),
            kit::hm(*at),
        ),
    }
}

/// `07`: a count as a readout shows it.
pub fn two(n: usize) -> String {
    format!("{n:02}")
}

/// `T−14M`, `T−1H05M`: the time to an event, in minutes, rounded up.
pub fn t_minus(d: Duration) -> String {
    let m = (d.whole_seconds().max(0) + 59) / 60;
    if m >= 60 {
        format!("T−{}H{:02}M", m / 60, m % 60)
    } else {
        format!("T−{m}M")
    }
}

/// `10:12`, `1:05:00`: a running clock.
pub fn clock(d: Duration) -> String {
    let s = d.whole_seconds().max(0);
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

/// `Three sessions`: a count that starts a sentence.
pub fn sentence_count(n: usize, one: &str, many: &str) -> String {
    const WORDS: [&str; 11] = [
        "No", "One", "Two", "Three", "Four", "Five", "Six", "Seven", "Eight", "Nine", "Ten",
    ];
    let word = WORDS
        .get(n)
        .map_or_else(|| n.to_string(), |w| (*w).to_owned());
    format!("{word} {}", if n == 1 { one } else { many })
}

/// The title hippo writes for a destructive command: what kind of command
/// it was.
pub fn harm_title(command: &str) -> String {
    let c = command
        .strip_prefix("Bash: ")
        .unwrap_or(command)
        .to_lowercase();
    let kind = if c.contains("push") && (c.contains("--force") || c.contains(" -f")) {
        // The branch it overwrote tells two force pushes apart.
        // From the command as typed: branch names keep their case.
        return match push_target(command.strip_prefix("Bash: ").unwrap_or(command)) {
            Some(b) => format!("A force push to {b} did harm"),
            None => "A force push did harm".to_owned(),
        };
    } else if c.contains("reset --hard") {
        "A hard reset"
    } else if c.contains("rm -rf") || c.contains("rm -fr") {
        "A recursive delete"
    } else if c.contains("clean -f") {
        "A git clean"
    } else if c.contains("drop table") || c.contains("drop database") {
        "A dropped table"
    } else {
        "A destructive command"
    };
    format!("{kind} did harm")
}

/// The branch a `git push` writes: the destination of its last refspec
/// (`notes-draft`, or `main` in `HEAD:main`), when it names one after the
/// remote.
fn push_target(command: &str) -> Option<String> {
    let words: Vec<&str> = command.split_whitespace().collect();
    let at = words.iter().position(|w| *w == "push")?;
    let args: Vec<&str> = words[at + 1..]
        .iter()
        .copied()
        .take_while(|w| !matches!(*w, "&&" | "||" | ";" | "|"))
        .filter(|w| !w.starts_with('-'))
        .collect();
    let spec = args.get(1..)?.last()?;
    let dest = spec.rsplit(':').next()?.trim_start_matches('+');
    let dest = dest.strip_prefix("refs/heads/").unwrap_or(dest);
    (!dest.is_empty()).then(|| dest.to_owned())
}

/// A focused item's top line: its state label, a clock, and where it is
/// at the right.
fn focus_top(look: &Look, label: gpui::Div, clock: Option<gpui::Div>, right: &str) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap(look.px(14.))
        .child(label)
        .children(clock)
        .child(div().flex_1())
        .child(kit::cap(look, right, look.c.faint))
}

/// A focused item's title.
fn focus_title(look: &Look, title: &str) -> gpui::Div {
    div()
        .mt(look.px(12.))
        .text_size(look.text(22.))
        .line_height(look.text(29.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(look.c.text)
        .child(title.to_owned())
}

/// A quotation with a 2-point rule at its left in `color`.
fn quote_bar(look: &Look, text: String, color: Hsla, size: f32) -> gpui::AnyElement {
    div()
        .mt(look.px(26.))
        .border_l(look.fixed(2.))
        .border_color(color)
        .pl(look.px(16.))
        .py(look.px(2.))
        .text_size(look.text(size))
        .line_height(look.text(size * 1.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(look.c.text)
        .child(text)
        .into_any_element()
}

/// A readout's label, figure, colour and the label under it.
pub type Readout = (String, String, Hsla, Option<(String, Hsla)>);

/// Readouts in a row between hairlines.
pub(super) fn readouts(look: &Look, cells: Vec<Readout>) -> gpui::Div {
    div()
        .flex()
        .flex_wrap()
        .border_t(look.fixed(1.))
        .border_b(look.fixed(1.))
        .border_color(look.c.border)
        .children(
            cells
                .into_iter()
                .enumerate()
                .map(|(i, (label, big, color, under))| {
                    div()
                        .flex_1()
                        .min_w(look.px(150.))
                        .py(look.px(14.))
                        .when(i > 0, |d| {
                            d.pl(look.px(16.))
                                .border_l(look.fixed(1.))
                                .border_color(look.c.border.opacity(0.6))
                        })
                        .child(kit::readout(look, &label, big, color, under))
                }),
        )
}

/// The scope band: the fleet's tool calls in 2-minute beats from `t0` to
/// `now`, with six time labels and `NOW`.
pub(super) fn scope_band(
    fleet: &Fleet,
    data: Option<&PulseData>,
    t0: OffsetDateTime,
    now: OffsetDateTime,
    look: &Look,
    h: f32,
) -> gpui::AnyElement {
    let beat = replay::BEAT.whole_seconds().max(1);
    let n = ((now - t0).whole_seconds() / beat).max(1) as usize;
    let mut beats = vec![0u32; n];
    if let Some(d) = data {
        let hist = pulse::histogram(&d.day, t0, now, &rail_profiles(fleet));
        for (k, bar) in hist.bars.iter().enumerate() {
            let t = hist.from + replay::BEAT * k as i32;
            if t < t0 {
                continue;
            }
            let j = ((t - t0).whole_seconds() / beat) as usize;
            if let Some(slot) = beats.get_mut(j) {
                *slot += bar.iter().sum::<u32>();
            }
        }
    }
    let ticks: Vec<(f32, String)> = (0..=6)
        .map(|k| {
            let f = k as f32 / 6.;
            let label = if k == 6 {
                "NOW".to_owned()
            } else {
                kit::hm(t0 + (now - t0) * f)
            };
            (f, label)
        })
        .collect();
    kit::scope(look, beats, ticks, h)
}

/// Paints `text` in SF Mono at 10 points: `align` 0 left, 1 centred, 2
/// right of `x`.
#[allow(clippy::too_many_arguments)]
fn paint_label(
    window: &mut Window,
    cx: &mut gpui::App,
    lk: &Look,
    text: String,
    x: f32,
    y: f32,
    color: Hsla,
    align: u8,
) {
    let text: SharedString = text.into();
    let run = gpui::TextRun {
        len: text.len(),
        font: gpui::font(kit::MONO),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let size_px = lk.text(10.);
    let line = window.text_system().shape_line(text, size_px, &[run], None);
    let lw = f32::from(line.width);
    let x = match align {
        1 => x - lw / 2.,
        2 => x - lw,
        _ => x,
    };
    let _ = line.paint(
        point(px(x), px(y)),
        size_px * 1.2,
        gpui::TextAlign::Left,
        None,
        window,
        cx,
    );
}

/// A drain chart: the share of the window used from its start to now as
/// a line in the account's colour, the projection to empty dashed in red,
/// a now line and a square at the empty time, over a grid at 25 % steps
/// and hours.
fn drain_chart(
    look: &Look,
    d: &hippo_core::usage::Drain,
    hue: Hsla,
    now: OffsetDateTime,
    h: f32,
) -> gpui::AnyElement {
    let d = d.clone();
    let lk = look.clone();
    let now_s = now.unix_timestamp();
    canvas(
        |_, _, _| {},
        move |bounds, _, window, cx| {
            let o = bounds.origin;
            let ox = f32::from(o.x);
            let oy = f32::from(o.y);
            let w = f32::from(bounds.size.width);
            let height = f32::from(bounds.size.height);
            let (lp, top, bottom) = (36., 18., 18.);
            let pw = (w - lp).max(1.);
            let ph = (height - top - bottom).max(1.);
            let span = (d.end - d.start).max(1) as f32;
            let x = |t: i64| ox + lp + ((t - d.start) as f32 / span).clamp(0., 1.) * pw;
            let y = |u: f64| oy + top + (1. - u.clamp(0., 1.) as f32) * ph;
            for q in [0., 0.25, 0.5, 0.75, 1.] {
                window.paint_quad(fill(
                    Bounds::new(point(px(ox + lp), px(y(q))), size(px(pw), px(1.))),
                    if q == 0. {
                        lk.c.border_strong
                    } else {
                        lk.c.border.opacity(0.55)
                    },
                ));
                paint_label(
                    window,
                    cx,
                    &lk,
                    format!("{}%", (q * 100.) as u32),
                    ox,
                    y(q) - 6.,
                    lk.c.faint,
                    0,
                );
            }
            let mut t = d.start;
            while t <= d.end {
                let tx = x(t);
                window.paint_quad(fill(
                    Bounds::new(point(px(tx), px(oy + top)), size(px(1.), px(ph))),
                    lk.c.border.opacity(0.55),
                ));
                let align = if t == d.start {
                    0
                } else if t + 3600 > d.end {
                    2
                } else {
                    1
                };
                let at = OffsetDateTime::from_unix_timestamp(t).unwrap_or(now);
                paint_label(
                    window,
                    cx,
                    &lk,
                    kit::hm(at),
                    tx,
                    oy + height - 13.,
                    lk.c.faint,
                    align,
                );
                t += 3600;
            }
            // What was used, step by step to now.
            let step = hippo_core::usage::DRAIN_STEP;
            let mut b = PathBuilder::stroke(px(1.5));
            for (k, u) in d.samples.iter().enumerate() {
                let t = (d.start + k as i64 * step).min(now_s);
                let p = point(px(x(t)), px(y(*u)));
                if k == 0 {
                    b.move_to(p);
                } else {
                    b.line_to(p);
                }
            }
            if let Ok(p) = b.build() {
                window.paint_path(p, hue);
            }
            // The projection at the burn rate, dashed, to empty or the reset.
            let end = d.empty_at.unwrap_or(d.end).min(d.end);
            let (x0, y0) = (x(now_s), y(d.used));
            let end_used = (d.used + d.burn * (end - now_s) as f64 / 3600.).min(1.);
            let (x1, y1) = (x(end), y(end_used));
            let len = ((x1 - x0).powi(2) + (y1 - y0).powi(2)).sqrt();
            let color = if d.empty_at.is_some() { lk.c.err } else { hue };
            let mut s = 0.;
            while s < len {
                let e = (s + 3.).min(len);
                let mut seg = PathBuilder::stroke(px(1.5));
                seg.move_to(point(
                    px(x0 + (x1 - x0) * s / len),
                    px(y0 + (y1 - y0) * s / len),
                ));
                seg.line_to(point(
                    px(x0 + (x1 - x0) * e / len),
                    px(y0 + (y1 - y0) * e / len),
                ));
                if let Ok(p) = seg.build() {
                    window.paint_path(p, color);
                }
                s += 6.;
            }
            // Now.
            window.paint_quad(fill(
                Bounds::new(point(px(x0), px(oy + top)), size(px(1.), px(ph))),
                lk.c.text.opacity(0.5),
            ));
            paint_label(
                window,
                cx,
                &lk,
                "NOW".into(),
                x0 - 4.,
                oy + top + ph - 14.,
                lk.c.text_muted,
                2,
            );
            if let Some(e) = d.empty_at {
                let ex = x(e);
                window.paint_quad(fill(
                    Bounds::new(point(px(ex - 3.), px(y(1.) - 3.)), size(px(6.), px(6.))),
                    lk.c.err,
                ));
                let at = OffsetDateTime::from_unix_timestamp(e).unwrap_or(now);
                paint_label(
                    window,
                    cx,
                    &lk,
                    format!("EMPTY {}", kit::hm(at)),
                    ex + 3.,
                    y(1.) - 17.,
                    lk.c.err,
                    2,
                );
            }
        },
    )
    .mt(look.px(26.))
    .w_full()
    .h(look.px(h))
    .into_any_element()
}

/// What a needs-you session is waiting for: its state, its question and
/// the wait.
fn need_words(r: &Row, now: OffsetDateTime) -> (&'static str, Option<String>, String) {
    match &r.state {
        State::Waiting { at, asked, plan } => (
            if *plan { "Plan waiting" } else { "Waiting" },
            asked.clone(),
            kit::span(now - *at),
        ),
        State::Stopped { at, reset, .. } => (
            "Stopped at its limit",
            reset.clone().map(|r| format!("Usage limit · {r}")),
            kit::span(now - *at),
        ),
        _ => ("Needs you", None, String::new()),
    }
}

/// The accounts with a figure: the one to use next first, then by name.
pub(super) fn rail_profiles(fleet: &Fleet) -> Vec<String> {
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

fn short(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('…');
    out
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
    fn clocks_and_countdowns_read_as_instruments() {
        assert_eq!(two(2), "02");
        assert_eq!(two(31), "31");
        assert_eq!(clock(Duration::seconds(612)), "10:12");
        assert_eq!(clock(Duration::seconds(3900)), "1:05:00");
        assert_eq!(clock(Duration::seconds(-5)), "00:00");
        assert_eq!(t_minus(Duration::minutes(14)), "T−14M");
        assert_eq!(t_minus(Duration::seconds(61)), "T−2M");
        assert_eq!(t_minus(Duration::minutes(65)), "T−1H05M");
    }

    #[test]
    fn hippo_names_what_a_destructive_command_was() {
        assert_eq!(
            harm_title("Bash: git push --force origin notes-draft"),
            "A force push to notes-draft did harm"
        );
        assert_eq!(
            harm_title("git push -f origin HEAD:refs/heads/main && echo ok"),
            "A force push to main did harm"
        );
        assert_eq!(harm_title("git push --force"), "A force push did harm");
        assert_eq!(
            harm_title("git reset --hard HEAD~2"),
            "A hard reset did harm"
        );
        assert_eq!(harm_title("rm -rf build"), "A recursive delete did harm");
        assert_eq!(
            harm_title("dd if=/dev/zero"),
            "A destructive command did harm"
        );
        assert_eq!(sentence_count(3, "session", "sessions"), "Three sessions");
        assert_eq!(sentence_count(1, "session", "sessions"), "One session");
        assert_eq!(sentence_count(14, "session", "sessions"), "14 sessions");
    }

    #[test]
    fn thousands_are_grouped() {
        assert_eq!(thousands(1204), "1,204");
        assert_eq!(thousands(311), "311");
        assert_eq!(thousands(1_000_000), "1,000,000");
    }
}
