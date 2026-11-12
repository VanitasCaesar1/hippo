//! The sidebar: the fleet as four shelves under the nav.
//!
//! 256 points wide on the sidebar colour. Under [`NARROW`] (or folded
//! away) it does not dock: the title bar holds its toggle and it opens as
//! an overlay.

use std::collections::HashSet;

use gpui::AnimationExt as _;
use gpui::{
    Context, FontWeight, MouseButton, Role, Window, WindowControlArea, div, prelude::*, px,
};
use hippo_view::fleet::{self, Row, Shelf, State};
use time::OffsetDateTime;

use super::Shell;
use super::kit::{self, Status};
use super::theme::Look;
use crate::shelving::{self, Marks, Place, Shelves, Tally};
use crate::state::Section;

/// Below this window width the sidebar does not dock.
pub const NARROW: f32 = 900.;
/// The sidebar's default width in points (docked, or as the overlay below
/// [`NARROW`]). T3 Code's values: 256 by
/// default, at least 208, and at most the window less 640 for the content.
pub const WIDE: f32 = 256.;
pub const MIN_WIDE: f32 = 208.;
pub const CONTENT_MIN: f32 = 640.;

/// A dragged width kept between the minimum and the window less the
/// content's minimum.
pub fn clamp_width(width: f32, window: f32) -> f32 {
    let max = (window - CONTENT_MIN).max(MIN_WIDE);
    width.clamp(MIN_WIDE, max)
}
/// The head row's height: the traffic lights sit in its middle.
pub const HEAD: f32 = 44.;
/// The traffic lights' own inset (T3 Code's
/// `--desktop-window-controls-inset`): the lights sit at x 16, centred in
/// the 44-point bar, and the first control starts at 90; the toggle is 28
/// and the next content 12 after it.
pub const LIGHTS: f32 = 90.;
/// The sidebar toggle's size.
pub const TOGGLE: f32 = 28.;

/// The non-empty shelves in order, each with its rows.
pub type ShelfRows = Vec<(Shelf, Vec<Row>)>;

/// The sidebar's state: shelves folded by hand.
#[derive(Default)]
pub struct SidebarUi {
    /// A dragged width (prefs `sidebar_width`); `None` is [`WIDE`].
    pub width: Option<f32>,
    /// The edge is being dragged.
    pub dragging: bool,
    /// Folded away by hand (⌃⌘S or the head's button) at a width that
    /// docks it.
    pub hidden: bool,
    /// Open as an overlay where it does not dock (below [`NARROW`], or
    /// folded away).
    pub overlay: bool,
    pub folded: HashSet<Shelf>,
    /// Shelves the person opened that start folded (Settled when long).
    pub unfolded: HashSet<Shelf>,
    /// The Snoozed shelf is open (it starts folded, as T3 Code's).
    pub snoozed_open: bool,
    /// The row under the pointer: its Settle and Snooze show.
    pub hover: Option<String>,
    /// The row whose snooze menu is open.
    pub snooze_menu: Option<String>,
    /// When a press outside closed the menu: a click on its own clock in
    /// the same press does not open it again.
    pub snooze_closed: Option<std::time::Instant>,
    /// The shelves as the pointer found them: while it is over the list,
    /// a live refresh does not move rows.
    pub frozen: Option<Vec<(Shelf, Vec<String>)>>,
    /// Where the clock was clicked, and whether the menu opens upward
    /// (too near the window's foot to fit below).
    pub snooze_at: Option<(gpui::Point<gpui::Pixels>, bool)>,
}

/// A row's status glyph and word, and its detail (`49m`, `6m`, `11:37`).
pub fn row_status(row: &Row, now: OffsetDateTime) -> Option<(Status, Option<String>)> {
    let since = |t: OffsetDateTime| fleet::span((now - t).whole_seconds().max(0));
    match &row.state {
        State::Live => Some((Status::Working, row.started.map(since))),
        State::Quiet { .. } if row.call.is_some() => {
            Some((Status::Working, row.started.map(since)))
        }
        State::Waiting { at, plan, .. } => Some((
            if *plan {
                Status::Approval
            } else {
                Status::Input
            },
            Some(since(*at)),
        )),
        State::Stopped {
            carried_on: false, ..
        } => Some((Status::Limited, None)),
        State::Interrupted { at, .. } => Some((Status::Interrupted, Some(kit::hm(*at)))),
        _ => None,
    }
}

/// `26m`, `2h`, `3d`: a row's age, from its newest turn.
pub fn age(row: &Row, now: OffsetDateTime) -> String {
    row.last.map_or_else(String::new, |t| {
        let s = (now - t).whole_seconds().max(0);
        if s < 60 {
            "now".to_owned()
        } else if s < 3600 {
            format!("{}m", s / 60)
        } else if s < 86_400 {
            format!("{}h", s / 3600)
        } else {
            format!("{}d", s / 86_400)
        }
    })
}

/// A row as it appears (motion): it fades in over 300 ms when
/// its shelf unfolds, or when it first shows on a shelf.
fn unfold_in(shelf: usize, id: &str, row: gpui::AnyElement) -> impl IntoElement {
    div().child(row).with_animation(
        gpui::ElementId::Name(format!("unfold-{shelf}-{id}").into()),
        gpui::Animation::new(std::time::Duration::from_millis(300))
            .with_easing(gpui::ease_out_quint()),
        |d, t| d.opacity(t),
    )
}

/// A pull request's number from its address (`…/pull/148` → `148`).
pub fn pr_number(url: &str) -> Option<&str> {
    let n = url.trim_end_matches('/').rsplit('/').next()?;
    (!n.is_empty() && n.chars().all(|c| c.is_ascii_digit())).then_some(n)
}

pub fn shelf_name(s: Shelf) -> &'static str {
    match s {
        Shelf::NeedsYou => "Needs you",
        Shelf::Working => "Working",
        Shelf::ToReview => "To review",
        Shelf::Settled => "Settled",
    }
}

impl Shell {
    /// The person's marks (opened, settled by hand, snoozed) and the clock:
    /// the one input every shelf and count reads.
    pub(crate) fn marks(&self) -> Marks<'_> {
        Marks {
            opened: &self.reach.opened,
            settled: &self.reach.settled,
            snoozed: &self.reach.snoozed,
            now: OffsetDateTime::now_utc().unix_timestamp(),
        }
    }

    /// The counts every view shows: Home's headline, the toggle's badge,
    /// the shelves, Fleet's groups and the menu-bar item.
    pub(crate) fn tally(&self) -> Option<Tally> {
        let f = self.loaded.as_ref().and_then(|l| l.fleet.as_ref())?;
        Some(Shelves::of(f, &self.marks()).tally())
    }

    /// The shelves of today's fleet, non-empty ones in order, and the
    /// snoozed rows with their wake times.
    pub(crate) fn shelves(&self) -> (ShelfRows, Vec<(Row, i64)>) {
        let (live, snoozed) = self.shelves_live();
        let rows = match &self.sidebar.frozen {
            Some(snap) => shelving::freeze(live, snap, |r: &Row| r.id.as_str()),
            None => live,
        };
        (rows, snoozed)
    }

    /// The shelves as the fleet has them now.
    fn shelves_live(&self) -> (ShelfRows, Vec<(Row, i64)>) {
        let Some(f) = self.loaded.as_ref().and_then(|l| l.fleet.as_ref()) else {
            return (Vec::new(), Vec::new());
        };
        let s = Shelves::of(f, &self.marks());
        (
            s.shelves
                .iter()
                .filter(|(_, rows)| !rows.is_empty())
                .map(|(k, rows)| (*k, rows.iter().map(|r| (*r).clone()).collect()))
                .collect(),
            s.snoozed.iter().map(|(r, t)| ((*r).clone(), *t)).collect(),
        )
    }

    /// The person's own change shows at once, even under the pointer.
    fn refreeze(&mut self) {
        if self.sidebar.frozen.is_some() {
            let live = self.shelves_live().0;
            self.sidebar.frozen = Some(shelving::snapshot(&live, |r: &Row| r.id.as_str()));
        }
    }

    /// Settles (or un-settles) session `id` by hand.
    pub(crate) fn settle(&mut self, id: &str, settled: bool) {
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let next = crate::prefs::mark_settled(&self.reach.settled, id, settled, now);
        self.reach.settled = next.clone();
        self.save_pref(|p| p.settled_sessions = next);
        self.refreeze();
    }

    /// Snoozes session `id` until `until` (Unix seconds), or wakes it.
    pub(crate) fn snooze(&mut self, id: &str, until: Option<i64>) {
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let next = crate::prefs::mark_snoozed(&self.reach.snoozed, id, until, now);
        self.reach.snoozed = next.clone();
        self.save_pref(|p| p.snoozed_sessions = next);
        self.refreeze();
    }

    /// After a fleet read: a snoozed session that now needs the person
    /// wakes for good, and snoozes already past are dropped.
    pub(crate) fn wake_snoozes(&mut self) {
        let Some(f) = self.loaded.as_ref().and_then(|l| l.fleet.as_ref()) else {
            return;
        };
        let marks = self.marks();
        let now = marks.now;
        let woken = crate::shelving::woken(f, &marks);
        let past = self.reach.snoozed.values().any(|s| s.until <= now);
        if woken.is_empty() && !past {
            return;
        }
        let mut next = self.reach.snoozed.clone();
        next.retain(|id, s| s.until > now && !woken.contains(id));
        self.reach.snoozed = next.clone();
        self.save_pref(|p| p.snoozed_sessions = next);
    }

    fn nav_row(
        &self,
        look: &Look,
        section: Section,
        narrow: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let on = self.state.tabs.section() == section;
        let (icon, count, badge) = match section {
            Section::Home => ("house", 0, false),
            Section::Inbox => ("inbox", self.inbox_count(), true),
            Section::Fleet => (
                "bot",
                self.loaded
                    .as_ref()
                    .and_then(|l| l.fleet.as_ref())
                    .map_or(0, |f| f.rows.len()),
                false,
            ),
            Section::Incidents => ("siren", self.incidents_new(), false),
            Section::Memory => (
                "brain",
                self.loaded
                    .as_ref()
                    .and_then(|l| l.panels.as_ref())
                    .map_or(0, |p| p.brain.rows.len()),
                false,
            ),
            Section::Manage => ("sliders-horizontal", 0, false),
        };
        let color = if on { look.c.text } else { look.c.text_muted };
        let tip = match section {
            Section::Home => "Home ⌘1",
            Section::Inbox => "Inbox ⌘I",
            Section::Fleet => "Fleet ⌘F",
            Section::Incidents => "Incidents ⌘6",
            Section::Memory => "Memory ⌘4",
            Section::Manage => "Manage ⌘,",
        };
        div()
            .id(("nav", section as usize))
            .role(Role::Tab)
            .aria_label(section.name())
            .aria_selected(on)
            .flex()
            .items_center()
            .gap(look.px(10.))
            .h(look.px(30.))
            .when(narrow, |d| d.w(look.px(36.)).justify_center())
            .when(!narrow, |d| d.px(look.px(8.)))
            .rounded(look.px(6.))
            .when(on, |d| d.bg(look.c.raised))
            .when(!on, |d| d.hover(|h| h.bg(look.c.hover)))
            .cursor_pointer()
            .child(kit::icon(look, icon, 16., color))
            .when(!narrow, |d| {
                d.child(
                    div()
                        .flex_1()
                        .text_size(look.text(13.))
                        .text_color(color)
                        .child(section.name()),
                )
                .when(count > 0, |d| {
                    if badge {
                        d.child(kit::count_badge(look, count, false))
                    } else {
                        d.child(
                            div()
                                .text_size(look.text(12.))
                                .text_color(look.c.faint)
                                .child(count.to_string()),
                        )
                    }
                })
            })
            .when(narrow, |d| d.tooltip(super::tooltip::Tooltip::text(tip)))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.show_section(section, window, cx);
            }))
    }

    /// A shelf's header (T3 Code's `SidebarSectionHeader`): 32
    /// high, the label in 12-point medium, then at the right its count
    /// (Needs you's in the warn badge) and the fold chevron.
    fn shelf_header(
        &self,
        look: &Look,
        shelf: Shelf,
        n: usize,
        folded: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        div()
            .id(("shelf", shelf as usize))
            .role(Role::Button)
            .aria_label(format!("{} {n}", shelf_name(shelf)))
            .hover(|h| h.bg(look.c.hover))
            .aria_expanded(!folded)
            .flex()
            .items_center()
            .gap(look.px(8.))
            .h(look.px(32.))
            .mx(look.px(2.))
            .px(look.px(8.))
            .rounded(look.px(6.))
            .cursor_pointer()
            .text_size(look.text(12.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(look.c.text_muted)
            .child(div().min_w_0().truncate().child(shelf_name(shelf)))
            .child(div().flex_1())
            .child(if shelf == Shelf::NeedsYou {
                kit::count_badge(look, n, true).into_any_element()
            } else {
                div()
                    .font_weight(FontWeight::NORMAL)
                    .text_color(look.c.faint)
                    .child(n.to_string())
                    .into_any_element()
            })
            .child(kit::icon(
                look,
                if folded {
                    "chevron-right"
                } else {
                    "chevron-down"
                },
                12.,
                look.c.faint,
            ))
            .on_click(cx.listener(move |this, _, _, cx| {
                let s = &mut this.sidebar;
                if folded {
                    s.folded.remove(&shelf);
                    s.unfolded.insert(shelf);
                } else {
                    s.folded.insert(shelf);
                    s.unfolded.remove(&shelf);
                }
                cx.notify();
            }))
    }

    /// A thread row (T3 Code's `SidebarThreadRow` card, MIT): 78
    /// high with a third line, radius 6, 10 in and 8 down. The top line is
    /// the project's mark and name, then the state (`Working 49m`, `Input
    /// 6m`, `Done` until opened) or the age; the title in 14-point medium
    /// (regular and muted once opened and finished); the third line is
    /// what it is doing, what it asked, or its branch, pull requests and
    /// lines, with the account mark at its right (on the title line when
    /// there is no third line).
    fn thread_row(
        &self,
        look: &Look,
        row: &Row,
        shelf: Shelf,
        now: OffsetDateTime,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let active = matches!(
            self.state.tabs.active_tab(),
            Some(crate::state::Tab::Xray(id)) if *id == row.id
        );
        let seen = self.reach.opened.contains_key(&row.id);
        let recede = seen && matches!(shelf, Shelf::ToReview | Shelf::Settled);
        let project = row.project.clone().unwrap_or_else(|| "—".to_owned());
        // A To review row shows its age, not `Done` (the shelf
        // already says so); unread shows only in the title's brightness,
        // T3 Code's rule. Needs you and Working keep glyph and word.
        let status = row_status(row, now);
        // Under the pointer, Settle and Snooze take the state's
        // place (T3 Code's row controls); a row that needs you has none.
        let controls = shelving::controls(Place::Shelf(shelf));
        let right: gpui::AnyElement =
            if self.row_hovered(&row.id) && (controls.settle || controls.snooze) {
                self.row_controls(look, &row.id, controls, cx)
                    .into_any_element()
            } else {
                match status {
                    Some((s, detail)) => kit::top_status(look, s, detail).into_any_element(),
                    None => div()
                        .flex_shrink_0()
                        .text_size(look.text(12.))
                        .text_color(look.c.faint)
                        .child(age(row, now))
                        .into_any_element(),
                }
            };
        let line3: Option<gpui::AnyElement> = match shelf {
            Shelf::Working => match &row.call {
                Some(call) => {
                    // The same step run again reads once, `×n`.
                    let runs = row.turn.as_ref().map(|t| kit::runs(&t.calls));
                    let (shown, n) = match runs.as_ref().and_then(|v| v.first()) {
                        Some(run) if kit::same_step(&run.call, call) => (run.call.clone(), run.n),
                        _ => (call.clone(), 1),
                    };
                    Some(kit::tool_line(look, &shown, n, None, true).into_any_element())
                }
                // Nothing to say: no third line (T3 Code's rule).
                None => row
                    .activity
                    .clone()
                    .filter(|a| !a.trim().is_empty())
                    .map(|a| div().truncate().child(a).into_any_element()),
            },
            Shelf::NeedsYou => {
                let text = match &row.state {
                    State::Waiting { asked, .. } => asked
                        .as_deref()
                        .map(|a| format!("\u{201c}{}\u{201d}", a.trim()))
                        .unwrap_or_else(|| "waiting on you".to_owned()),
                    State::Stopped { reset, .. } => match reset {
                        Some(r) => format!("usage limit · {r}"),
                        None => "stopped at its usage limit".to_owned(),
                    },
                    _ => row.claims.short_line().unwrap_or_default(),
                };
                Some(div().truncate().child(text).into_any_element())
            }
            _ => self.meta_line(look, row),
        };
        let mark = kit::mark(look, row.profile.as_deref(), 16.);
        let id = row.id.clone();
        let item = div()
            .id(gpui::ElementId::Name(format!("thread-{}", row.id).into()))
            .role(Role::Button)
            .aria_label(row.title.clone())
            .flex()
            .flex_col()
            .when(line3.is_some(), |d| d.h(look.px(78.)))
            .px(look.px(10.))
            .py(look.px(8.))
            .rounded(look.px(6.))
            .overflow_hidden()
            .cursor_pointer();
        let has3 = line3.is_some();
        let mut mark = Some(mark);
        let title_mark = if has3 { None } else { mark.take() };
        kit::sidebar_fill(item, look, active)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(6.))
                    .h(look.px(20.))
                    .child(kit::project_mark(look, &project, 16.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(look.text(12.))
                            .text_color(look.c.faint)
                            .child(project),
                    )
                    .child(right),
            )
            .child(
                div()
                    .mt(look.px(4.))
                    .flex()
                    .items_center()
                    .gap(look.px(6.))
                    .h(look.px(20.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(look.text(14.))
                            // GPUI's 500 draws heavier than T3
                            // Code's medium in Chromium; 400 matches it.
                            .font_weight(FontWeight::NORMAL)
                            .text_color(if recede {
                                look.c.text_muted
                            } else {
                                look.c.text
                            })
                            .child(kit::title(&row.title)),
                    )
                    .children(title_mark),
            )
            .children(line3.map(|l| {
                div()
                    .mt(look.px(2.))
                    .flex()
                    .items_center()
                    .gap(look.px(6.))
                    .h(look.px(16.))
                    .text_size(look.text(12.))
                    .text_color(look.c.faint)
                    .child(div().flex_1().min_w_0().overflow_hidden().child(l))
                    .children(mark)
            }))
            .on_hover(self.hover_listener(&row.id, cx))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_xray(&id, window, cx);
            }))
    }

    fn row_hovered(&self, id: &str) -> bool {
        self.sidebar.hover.as_deref() == Some(id) || self.sidebar.snooze_menu.as_deref() == Some(id)
    }

    /// Keeps [`SidebarUi::hover`] on the row under the pointer.
    fn hover_listener(
        &self,
        id: &str,
        cx: &mut Context<Self>,
    ) -> impl Fn(&bool, &mut Window, &mut gpui::App) + 'static {
        let id = id.to_owned();
        cx.listener(move |this, hovered: &bool, _, cx| {
            let was = this.sidebar.hover.clone();
            if *hovered {
                this.sidebar.hover = Some(id.clone());
            } else if this.sidebar.hover.as_deref() == Some(id.as_str()) {
                this.sidebar.hover = None;
            }
            if was != this.sidebar.hover {
                cx.notify();
            }
        })
    }

    /// A row control (T3 Code's): a 20-high ghost button in the
    /// muted colour, the text colour under the pointer, with a tooltip. A
    /// click on it never opens the row.
    fn row_control(
        look: &Look,
        id: String,
        icon: &'static str,
        word: Option<&'static str>,
        tip: &'static str,
    ) -> gpui::Stateful<gpui::Div> {
        div()
            .id(gpui::ElementId::Name(id.into()))
            .role(Role::Button)
            .aria_label(tip)
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(look.px(4.))
            .h(look.px(20.))
            .px(look.px(6.))
            .rounded(look.px(5.))
            .cursor_pointer()
            .text_size(look.text(12.))
            .text_color(look.c.text_muted)
            .hover(|h| h.bg(look.c.hover).text_color(look.c.text))
            .active(|a| a.bg(look.c.raised))
            .child(kit::icon(
                look,
                icon,
                if word.is_some() { 14. } else { 13. },
                look.c.text_muted,
            ))
            .children(word)
            .tooltip(super::tooltip::Tooltip::text(tip))
    }

    /// Snooze (with its menu) and Settle, as `controls` allows.
    fn row_controls(
        &self,
        look: &Look,
        id: &str,
        controls: shelving::Controls,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let menu_open = self.sidebar.snooze_menu.as_deref() == Some(id);
        let (a, b) = (id.to_owned(), id.to_owned());
        let snooze = controls.snooze.then(|| {
            Self::row_control(
                look,
                format!("snooze-{id}"),
                "clock",
                None,
                "Snooze: hide it until later",
            )
            .when(menu_open, |d| d.bg(look.c.raised))
            .on_click(cx.listener(move |this, e: &gpui::ClickEvent, window, cx| {
                cx.stop_propagation();
                let at = e.position();
                let up = f32::from(window.viewport_size().height) - f32::from(at.y) < 220.;
                this.sidebar.snooze_at = Some((at, up));
                let just_closed = this
                    .sidebar
                    .snooze_closed
                    .take()
                    .is_some_and(|t| t.elapsed().as_millis() < 400);
                this.sidebar.snooze_menu = if just_closed { None } else { Some(a.clone()) };
                cx.notify();
            }))
        });
        let settle = controls.settle.then(|| {
            Self::row_control(
                look,
                format!("settle-{id}"),
                "check",
                Some("Settle"),
                "Settle: move it to Settled",
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.settle(&b, true);
                this.sidebar.hover = None;
                cx.notify();
            }))
        });
        div()
            .id(gpui::ElementId::Name(format!("controls-{id}").into()))
            .relative()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(look.px(2.))
            // A click between the two buttons does not open the row either.
            .on_click(|_, _, cx| cx.stop_propagation())
            .children(snooze)
            .children(settle)
            .when(menu_open, |d| d.child(self.snooze_menu(look, id, cx)))
    }

    /// The snooze choices (T3 Code's menu): `In 1 hour  19:47` and the rest,
    /// under the clock.
    fn snooze_menu(&self, look: &Look, id: &str, cx: &mut Context<Self>) -> impl IntoElement {
        let now = OffsetDateTime::now_utc();
        let items: Vec<gpui::AnyElement> = shelving::presets(now, super::status::local_offset())
            .into_iter()
            .enumerate()
            .map(|(i, p)| {
                let (id2, until) = (id.to_owned(), p.until.unix_timestamp());
                div()
                    .id(("snooze-choice", i))
                    .role(Role::MenuItem)
                    .aria_label(format!("{} {}", p.label, p.when))
                    .flex()
                    .items_center()
                    .gap(look.px(16.))
                    .h(look.px(28.))
                    .px(look.px(8.))
                    .rounded(look.px(6.))
                    .cursor_pointer()
                    .text_size(look.text(13.))
                    .text_color(look.c.text)
                    .hover(|h| h.bg(look.c.hover))
                    .active(|a| a.bg(look.c.raised))
                    .child(div().flex_1().child(p.label))
                    .child(
                        div()
                            .text_size(look.text(12.))
                            .text_color(look.c.faint)
                            .child(p.when),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.snooze(&id2, Some(until));
                        this.sidebar.snooze_menu = None;
                        this.sidebar.hover = None;
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect();
        // Below the clock, or above it near the window's foot (T3 Code's
        // menu flips the same way).
        let (at, up) = self
            .sidebar
            .snooze_at
            .unwrap_or((gpui::point(gpui::px(240.), gpui::px(200.)), false));
        let pos = gpui::point(
            at.x + gpui::px(14.),
            at.y + gpui::px(if up { -14. } else { 14. }),
        );
        gpui::deferred(
            gpui::anchored()
                .position_mode(gpui::AnchoredPositionMode::Window)
                .position(pos)
                .anchor(if up {
                    gpui::Anchor::BottomRight
                } else {
                    gpui::Anchor::TopRight
                })
                .snap_to_window_with_margin(gpui::px(8.))
                .child(
                    div()
                        .id("snooze-menu")
                        .occlude()
                        .min_w(look.px(200.))
                        .p(look.px(4.))
                        .rounded(look.px(10.))
                        .bg(look.c.surface)
                        .border(look.fixed(1.))
                        .border_color(look.c.border)
                        .shadow_lg()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .px(look.px(8.))
                                .pt(look.px(4.))
                                .pb(look.px(2.))
                                .text_size(look.text(12.))
                                .text_color(look.c.faint)
                                .child("Snooze until"),
                        )
                        .children(items)
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.sidebar.snooze_menu = None;
                            this.sidebar.snooze_closed = Some(std::time::Instant::now());
                            cx.notify();
                        })),
                ),
        )
        .with_priority(1)
    }

    /// A finished row's third line (T3 Code's meta line): the
    /// branch, middle-cut, then the pull-request badge and the lines
    /// changed. `None` when it knows none of them (verified claims stay in
    /// Fleet's Claims column; the branch needs the room).
    fn meta_line(&self, look: &Look, row: &Row) -> Option<gpui::AnyElement> {
        let (open, merged) = super::fleet::pr_counts(row);
        let pr = (open + merged > 0).then(|| {
            let text = if open + merged == 1 {
                row.pulls
                    .first()
                    .map(|(u, _)| u.as_str())
                    .or(row.merged_elsewhere.first().map(String::as_str))
                    .and_then(pr_number)
                    .map_or_else(|| "1".to_owned(), |n| format!("#{n}"))
            } else {
                format!("+{}", open + merged)
            };
            kit::pr_badge(look, open == 0, text)
        });
        // Faint in the sidebar; the diff colours stay in Fleet
        // and the x-ray.
        // Its sub-agents' lines count too.
        let diff = row
            .total_lines()
            .and_then(|l| kit::diff_counts_quiet(look, l.added, l.removed));
        if row.branch.is_none() && pr.is_none() && diff.is_none() {
            return None;
        }
        Some(
            div()
                .flex()
                .items_center()
                .gap(look.px(6.))
                .min_w_0()
                .child(match &row.branch {
                    Some(b) => div()
                        .flex()
                        .flex_1()
                        .min_w_0()
                        .items_center()
                        .gap(look.px(4.))
                        .child(kit::icon(look, "git-branch", 12., look.c.faint))
                        .child(div().min_w_0().truncate().child(kit::middle(b, 30))),
                    None => div().flex_1(),
                })
                .children(pr)
                .children(diff)
                .into_any_element(),
        )
    }

    /// The Snoozed shelf: its header (`Snoozed 2`, folded at first) and,
    /// opened, its rows.
    fn snoozed_shelf(
        &self,
        mut list: gpui::Div,
        look: &Look,
        snoozed: &[(Row, i64)],
        now: OffsetDateTime,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let open = self.sidebar.snoozed_open;
        list = list.child(
            Self::header_row(look, "Snoozed", snoozed.len(), !open)
                .id("shelf-snoozed")
                .role(Role::Button)
                .aria_label(format!("Snoozed {}", snoozed.len()))
                .aria_expanded(open)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.sidebar.snoozed_open = !this.sidebar.snoozed_open;
                    cx.notify();
                })),
        );
        if open {
            for (r, until) in snoozed {
                let row = self
                    .slim_row(look, r, Some(*until), now, cx)
                    .into_any_element();
                list = list.child(unfold_in(9, &r.id, row));
            }
        }
        list
    }

    /// A shelf header's body (T3 Code's `SidebarSectionHeader`): 32 high,
    /// the label in 12-point medium, the count and the fold chevron.
    fn header_row(look: &Look, label: &str, n: usize, folded: bool) -> gpui::Div {
        div()
            .flex()
            .items_center()
            .gap(look.px(8.))
            .h(look.px(32.))
            .mx(look.px(2.))
            .px(look.px(8.))
            .rounded(look.px(6.))
            .cursor_pointer()
            .hover(|h| h.bg(look.c.hover))
            .text_size(look.text(12.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(look.c.text_muted)
            .child(div().min_w_0().truncate().child(label.to_owned()))
            .child(div().flex_1())
            .child(
                div()
                    .font_weight(FontWeight::NORMAL)
                    .text_color(look.c.faint)
                    .child(n.to_string()),
            )
            .child(kit::icon(
                look,
                if folded {
                    "chevron-right"
                } else {
                    "chevron-down"
                },
                12.,
                look.c.faint,
            ))
    }

    /// A settled or snoozed row: one line, 30 high (T3 Code's slim row),
    /// in the muted colour. At the right the age, or for a snoozed row an
    /// alarm clock and when it wakes (`3h`, in the info colour); under the
    /// pointer, Un-settle or Wake now.
    fn slim_row(
        &self,
        look: &Look,
        row: &Row,
        snoozed: Option<i64>,
        now: OffsetDateTime,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let id = row.id.clone();
        let active = matches!(
            self.state.tabs.active_tab(),
            Some(crate::state::Tab::Xray(i)) if *i == row.id
        );
        let right: gpui::AnyElement = if self.row_hovered(&row.id) {
            let id2 = row.id.clone();
            match snoozed {
                Some(_) => Self::row_control(
                    look,
                    format!("wake-{}", row.id),
                    "alarm-clock-off",
                    None,
                    "Wake now: back on its shelf",
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.snooze(&id2, None);
                    this.sidebar.hover = None;
                    cx.notify();
                }))
                .into_any_element(),
                None => Self::row_control(
                    look,
                    format!("unsettle-{}", row.id),
                    "undo-2",
                    None,
                    "Un-settle: back to To review",
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.settle(&id2, false);
                    this.sidebar.hover = None;
                    cx.notify();
                }))
                .into_any_element(),
            }
        } else {
            match snoozed {
                Some(until) => div()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(look.px(4.))
                    .text_size(look.text(12.))
                    .text_color(look.c.info)
                    .child(kit::icon(look, "alarm-clock", 12., look.c.info))
                    .child(shelving::wake_label(until, now.unix_timestamp()))
                    .into_any_element(),
                None => div()
                    .flex_shrink_0()
                    .text_size(look.text(12.))
                    .text_color(look.c.faint)
                    .child(age(row, now))
                    .into_any_element(),
            }
        };
        let item = div()
            .id(gpui::ElementId::Name(format!("slim-{}", row.id).into()))
            .role(Role::Button)
            .aria_label(row.title.clone())
            .flex()
            .items_center()
            .gap(look.px(8.))
            .h(look.px(30.))
            .px(look.px(10.))
            .rounded(look.px(6.))
            .cursor_pointer();
        kit::sidebar_fill(item, look, active)
            .child(kit::mark(look, row.profile.as_deref(), 16.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(look.text(13.))
                    .text_color(if active {
                        look.c.text
                    } else {
                        look.c.text_muted
                    })
                    .child(kit::title(&row.title)),
            )
            .child(right)
            .on_hover(self.hover_listener(&row.id, cx))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_xray(&id, window, cx);
            }))
    }

    /// The foot: Manage, the health dot and word, `indexed hh:mm`.
    fn sidebar_foot(
        &self,
        look: &Look,
        narrow: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let (word, ok) = self.health_word();
        let indexed = self.status_model().ingest.map(|i| {
            i.strip_prefix("ingest ")
                .map_or(i.clone(), |t| format!("indexed {t}"))
        });
        let on = self.state.tabs.section() == Section::Manage;
        let manage = div()
            .id("foot-manage")
            .role(Role::Button)
            .aria_label("Manage")
            .flex_shrink_0()
            .size(look.px(28.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(look.px(6.))
            .when(on, |d| d.bg(look.c.raised))
            .hover(|h| h.bg(look.c.hover))
            .cursor_pointer()
            .child(kit::icon(
                look,
                "sliders-horizontal",
                16.,
                if on { look.c.text } else { look.c.text_muted },
            ))
            .tooltip(super::tooltip::Tooltip::text("Manage ⌘,"))
            .on_click(
                cx.listener(|this, _, window, cx| this.show_section(Section::Manage, window, cx)),
            );
        let model = self.status_model();
        // A key the guard ignored says so until the person clicks or moves.
        let refused = cx
            .try_global::<crate::keyguard::KeyGuard>()
            .and_then(|g| g.refused)
            .map(|r| (r.words().to_string(), false));
        let notice = refused.or_else(|| model.notice()).filter(|_| !narrow);
        let foot = div()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(look.px(6.))
            .h(look.px(44.))
            .px(look.px(8.))
            .child(manage)
            .when(!narrow, |d| {
                d.child(
                    div()
                        .id("foot-health")
                        .role(Role::Button)
                        .aria_label(format!("{word}: show hippo's health"))
                        .flex()
                        .items_center()
                        .gap(look.px(6.))
                        .px(look.px(4.))
                        .h(look.px(24.))
                        .rounded(look.px(6.))
                        .min_w_0()
                        .cursor_pointer()
                        .hover(|h| h.bg(look.c.hover))
                        .text_size(look.text(12.5))
                        .text_color(if ok { look.c.text_muted } else { look.c.warn })
                        .child(kit::dot(look, if ok { look.c.ok } else { look.c.warn }))
                        .child(div().truncate().child(word))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.health_open = !this.health_open;
                            cx.notify();
                        })),
                )
                .child(div().flex_1())
                .children(indexed.map(|t| {
                    div()
                        .flex_shrink_0()
                        .text_size(look.text(12.))
                        .text_color(look.c.faint)
                        .child(t)
                }))
            });
        div()
            .id("sidebar-foot")
            .role(Role::Status)
            .aria_label(model.accessible_label())
            .flex_shrink_0()
            .flex()
            .flex_col()
            .border_t(look.fixed(1.))
            .border_color(look.c.border)
            .children(notice.map(|(text, problem)| {
                div()
                    .px(look.px(12.))
                    .pt(look.px(8.))
                    .text_size(look.text(12.))
                    .line_clamp(2)
                    .text_ellipsis()
                    .text_color(if problem {
                        look.c.warn
                    } else {
                        look.c.text_muted
                    })
                    .child(text)
            }))
            .child(foot)
    }

    /// The head row: room for the traffic lights, then the mark and
    /// `hippo`. Dragging it moves the window.
    fn sidebar_head(
        &self,
        look: &Look,
        narrow: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let docked = self.sidebar_docked(window);
        div()
            .id("sidebar-head")
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(look.px(8.))
            .h(look.fixed(HEAD))
            .pl(look.fixed(LIGHTS))
            .pr(look.px(10.))
            .window_control_area(WindowControlArea::Drag)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.title_drag = true),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.title_drag = false),
            )
            .on_mouse_move(cx.listener(|this, _, window, _| {
                if this.title_drag {
                    this.title_drag = false;
                    window.start_window_move();
                }
            }))
            .when(!narrow, |d| {
                d.child(self.fold_button(look, docked, cx))
                    .child(div().w(look.px(4.)))
                    .child(
                        gpui::svg()
                            .path("hippo-mark.svg")
                            .flex_shrink_0()
                            .size(look.px(16.))
                            .text_color(look.c.text),
                    )
                    .child(
                        div()
                            .text_size(look.text(13.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("hippo"),
                    )
            })
    }

    /// Whether the sidebar sits beside the content: from [`NARROW`] and
    /// not folded away. Otherwise the title bar holds its toggle and it
    /// opens as an overlay (as T3 Code and Claude's desktop app).
    pub(crate) fn sidebar_docked(&self, window: &Window) -> bool {
        f32::from(window.viewport_size().width) >= NARROW && !self.sidebar.hidden
    }

    /// The width the sidebar takes from the content.
    pub(crate) fn sidebar_width(&self, window: &Window) -> f32 {
        if self.sidebar_docked(window) {
            self.sidebar_own_width(window)
        } else {
            0.
        }
    }

    /// The sidebar's own width: the dragged one, clamped to this window;
    /// as the overlay, never wider than the window less 12 (T3 Code's
    /// sheet).
    pub(crate) fn sidebar_own_width(&self, window: &Window) -> f32 {
        let w = f32::from(window.viewport_size().width);
        let want = self.sidebar.width.unwrap_or(WIDE);
        if self.sidebar_docked(window) {
            clamp_width(want, w)
        } else {
            want.max(MIN_WIDE).min(w - 12.)
        }
    }

    /// ⌃⌘S and the toggle: fold or show the docked sidebar; where it does
    /// not dock, open or close the overlay.
    pub(crate) fn toggle_sidebar(&mut self, window: &Window) {
        if f32::from(window.viewport_size().width) >= NARROW {
            if self.sidebar.overlay {
                self.sidebar.overlay = false;
            } else {
                self.sidebar.hidden = !self.sidebar.hidden;
            }
        } else {
            self.sidebar.overlay = !self.sidebar.overlay;
        }
    }

    /// The sidebar's fold button (`panel-left`), with its key in the tip.
    pub(crate) fn fold_button(
        &self,
        look: &Look,
        docked: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let label = if self.sidebar.overlay {
            "Close the sidebar  esc"
        } else if !docked {
            "Show the sidebar ⌃⌘S"
        } else {
            "Hide the sidebar ⌃⌘S"
        };
        kit::icon_btn(look, "sidebar-fold", "panel-left", label, false)
            .size(look.px(TOGGLE))
            .on_click(cx.listener(|this, _, window, cx| {
                cx.stop_propagation();
                this.toggle_sidebar(window);
                cx.notify();
            }))
    }

    /// The sidebar, or the rail under [`NARROW`].
    pub(super) fn render_sidebar(
        &self,
        look: &Look,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // There is no rail; below [`NARROW`] the sidebar is an
        // overlay of its full width.
        let narrow = false;
        let now = OffsetDateTime::now_utc();
        let (shelves, snoozed) = self.shelves();
        let nav = div()
            .flex()
            .flex_col()
            .gap(look.px(1.))
            .when(narrow, |d| d.items_center())
            .child(self.nav_row(look, Section::Home, narrow, cx))
            .child(self.nav_row(look, Section::Inbox, narrow, cx))
            .child(self.nav_row(look, Section::Fleet, narrow, cx))
            .child(self.nav_row(look, Section::Incidents, narrow, cx))
            .child(self.nav_row(look, Section::Memory, narrow, cx));
        let body: gpui::AnyElement = {
            // T3 Code's rows sit 4 apart (`py-0.5` each).
            let mut list = div().flex().flex_col().gap(look.px(4.)).pb(look.px(8.));
            let mut snoozed_drawn = false;
            for (shelf, rows) in &shelves {
                // Snoozed rows sit just above Settled, folded to a quiet
                // count that opens them (T3 Code's Snoozed shelf).
                if *shelf == Shelf::Settled && !snoozed.is_empty() {
                    snoozed_drawn = true;
                    list = self.snoozed_shelf(list, look, &snoozed, now, cx);
                }
                let default_folded = *shelf == Shelf::Settled && rows.len() > 8;
                let folded = *shelf != Shelf::NeedsYou
                    && (self.sidebar.folded.contains(shelf)
                        || (default_folded && !self.sidebar.unfolded.contains(shelf)));
                // A delegated session sits indented under its
                // parent and counts with it.
                let refs: Vec<&Row> = rows.iter().collect();
                let n = crate::shelving::count_own(&refs);
                list = list.child(self.shelf_header(look, *shelf, n, folded, cx));
                if folded {
                    continue;
                }
                for r in rows {
                    let row = if *shelf == Shelf::Settled {
                        self.slim_row(look, r, None, now, cx).into_any_element()
                    } else {
                        self.thread_row(look, r, *shelf, now, cx).into_any_element()
                    };
                    let row = if crate::shelving::nested_in(r, &refs) {
                        super::fleet::nested(look, row, 14.)
                    } else {
                        row
                    };
                    list = list.child(unfold_in(*shelf as usize, &r.id, row));
                }
            }
            if !snoozed.is_empty() && !snoozed_drawn {
                list = self.snoozed_shelf(list, look, &snoozed, now, cx);
            }
            div()
                .id("shelves")
                .flex_1()
                .min_h_0()
                .on_hover(cx.listener(|this, over: &bool, _, cx| {
                    if *over {
                        if this.sidebar.frozen.is_none() {
                            let live = this.shelves_live().0;
                            this.sidebar.frozen =
                                Some(shelving::snapshot(&live, |r: &Row| r.id.as_str()));
                        }
                    } else if this.sidebar.frozen.take().is_some() {
                        cx.notify();
                    }
                }))
                .overflow_y_scroll()
                .track_scroll(&self.sidebar_scroll)
                .child(list)
                .into_any_element()
        };
        div()
            .id("sidebar")
            .role(Role::Navigation)
            .aria_label("Sidebar")
            .flex()
            .flex_col()
            .flex_shrink_0()
            .w(look.px(self.sidebar_own_width(window)))
            .h_full()
            .bg(look.c.sidebar)
            .border_r(look.fixed(1.))
            .border_color(look.c.border)
            .child(self.sidebar_head(look, narrow, window, cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .px(look.px(8.))
                    .when(!narrow, |d| {
                        d.child(
                            div()
                                .id("sidebar-search")
                                .role(Role::Button)
                                .aria_label("Search")
                                .flex()
                                .items_center()
                                .gap(look.px(10.))
                                .h(look.px(32.))
                                .px(look.px(8.))
                                .mb(look.px(4.))
                                .rounded(look.px(6.))
                                .cursor_pointer()
                                .hover(|h| h.bg(look.c.hover))
                                .text_size(look.text(13.))
                                .text_color(look.c.text_muted)
                                .child(kit::icon(look, "search", 16., look.c.text_muted))
                                .child(div().flex_1().child("Search"))
                                .child(
                                    div()
                                        .text_size(look.text(12.))
                                        .text_color(look.c.faint)
                                        .child("⌘P"),
                                )
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.open_finder(window, cx)),
                                ),
                        )
                    })
                    .when(narrow, |d| d.items_center())
                    .child(nav)
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .flex_col()
                            .child(body)
                            // The list's bottom edge fades, so a row
                            // the edge cuts is never read cut mid-text.
                            .child(
                                div()
                                    .absolute()
                                    .bottom_0()
                                    .left_0()
                                    .right_0()
                                    .h(look.px(32.))
                                    .bg(gpui::linear_gradient(
                                        180.,
                                        gpui::linear_color_stop(look.c.sidebar.opacity(0.), 0.),
                                        gpui::linear_color_stop(look.c.sidebar, 1.),
                                    )),
                            ),
                    ),
            )
            .child(self.sidebar_foot(look, narrow, cx))
            .relative()
            .when(self.sidebar_docked(window), |d| {
                // The edge: drag to resize, double-click for the
                // default width.
                d.child(
                    div()
                        .id("sidebar-edge")
                        .absolute()
                        .top_0()
                        .right(px(-3.))
                        .w(px(6.))
                        .h_full()
                        .cursor_col_resize()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.sidebar.dragging = true;
                                cx.stop_propagation();
                            }),
                        )
                        .on_click(cx.listener(|this, e: &gpui::ClickEvent, _, cx| {
                            if e.click_count() == 2 {
                                this.sidebar.width = None;
                                this.save_sidebar_width();
                                cx.notify();
                            }
                        })),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dragged_width_stays_between_208_and_the_window_less_640() {
        assert_eq!(clamp_width(256., 1200.), 256.);
        assert_eq!(clamp_width(100., 1200.), 208.);
        assert_eq!(clamp_width(900., 1200.), 560.);
        // A window too narrow for both keeps the minimum.
        assert_eq!(clamp_width(300., 800.), 208.);
    }

    #[test]
    fn a_pull_request_number_comes_from_its_address() {
        assert_eq!(pr_number("https://github.com/o/r/pull/148"), Some("148"));
        assert_eq!(pr_number("https://github.com/o/r/pull/7/"), Some("7"));
        assert_eq!(pr_number("https://github.com/o/r/pulls"), None);
    }

    #[test]
    fn shelf_names_read_as_the_design_says() {
        assert_eq!(shelf_name(Shelf::NeedsYou), "Needs you");
        assert_eq!(shelf_name(Shelf::ToReview), "To review");
    }
}
