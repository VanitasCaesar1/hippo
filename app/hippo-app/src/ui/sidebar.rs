//! The sidebar: hippo's sections, the instrument look
//! (docs/designs/app-instrument.md); the day's sessions are in Fleet.
//!
//! 256 points wide on the sidebar colour. Under [`NARROW`] (or folded
//! away) it does not dock: the title bar holds its toggle and it opens as
//! an overlay.

use gpui::{
    Context, FontWeight, MouseButton, Role, Window, WindowControlArea, div, prelude::*, px,
};
use hippo_view::fleet::{self, Row, Shelf, State};
use time::OffsetDateTime;

use super::Shell;
use super::kit::{self, Status};
use super::theme::Look;
use crate::shelving::{self, Marks, Shelves, Tally};
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

/// The sidebar's state.
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

/// A pull request's number from its address (`…/pull/148` → `148`).
pub fn pr_number(url: &str) -> Option<&str> {
    let n = url.trim_end_matches('/').rsplit('/').next()?;
    (!n.is_empty() && n.chars().all(|c| c.is_ascii_digit())).then_some(n)
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

    /// The counts the toggle's badge shows: the shelves', with Working as
    /// the sessions working now.
    pub(crate) fn tally(&self) -> Option<Tally> {
        let f = self.loaded.as_ref().and_then(|l| l.fleet.as_ref())?;
        let mut t = Shelves::of(f, &self.marks()).tally();
        // Working is sessions working now, as Home and Pulse count them.
        t.working = f.rows.iter().filter(|r| r.working()).count();
        Some(t)
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

    /// A section: 34 high, divided by hairlines, the shown one raised
    /// with a 2-point bar at its left edge. Only Inbox shows a count.
    fn nav_row(
        &self,
        look: &Look,
        section: Section,
        narrow: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let on = self.state.tabs.section() == section;
        let (icon, count) = match section {
            Section::Morning | Section::Brain | Section::Agents | Section::Guard => ("house", 0),
            Section::Home => ("house", 0),
            Section::Inbox => ("inbox", self.inbox_count()),
            Section::Fleet => ("bot", 0),
            Section::Incidents => ("siren", 0),
            Section::Memory => ("brain", 0),
            Section::Manage => ("sliders-horizontal", 0),
        };
        let color = if on { look.c.text } else { look.c.text_muted };
        let tip = match section {
            Section::Morning | Section::Brain | Section::Agents | Section::Guard => "",
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
            .relative()
            .flex()
            .items_center()
            .h(look.px(34.))
            .when(narrow, |d| d.w(look.px(36.)).justify_center())
            .when(!narrow, |d| d.px(look.px(16.)))
            .border_b(look.fixed(1.))
            .border_color(look.c.border.opacity(0.6))
            .when(on, |d| {
                d.bg(look.c.raised).child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(look.fixed(2.))
                        .bg(look.c.text),
                )
            })
            .when(!on, |d| d.hover(|h| h.bg(look.c.hover)))
            .cursor_pointer()
            .when(narrow, |d| d.child(kit::icon(look, icon, 16., color)))
            .when(!narrow, |d| {
                d.child(
                    div()
                        .flex_1()
                        .text_size(look.text(13.))
                        .text_color(color)
                        .child(section.name()),
                )
                .when(count > 0, |d| {
                    d.child(kit::figure(look, count.to_string(), 11., look.c.text))
                })
            })
            .tooltip(super::tooltip::Tooltip::text(tip))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.show_section(section, window, cx);
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
        word: &'static str,
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
            .rounded(look.px(3.))
            .cursor_pointer()
            .font_family(kit::MONO)
            .text_size(look.text(10.5))
            .text_color(look.c.text_muted)
            .hover(|h| h.bg(look.c.hover).text_color(look.c.text))
            .active(|a| a.bg(look.c.raised))
            .child(word.to_uppercase())
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
                "Snooze",
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
                "Settle",
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

    /// The foot: Manage, then when the index was last written. A problem
    /// (a gap in setup, a stale ingest, a warning) shows in its place in
    /// the warn colour and opens hippo's health.
    fn sidebar_foot(
        &self,
        look: &Look,
        narrow: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let (word, ok) = self.health_word();
        let indexed = self
            .status_model()
            .ingest
            .map(|i| i.strip_prefix("ingest ").map_or(i.clone(), str::to_owned));
        let on = self.state.tabs.section() == Section::Manage;
        let manage = div()
            .id("foot-manage")
            .role(Role::Button)
            .aria_label("Manage")
            .flex_shrink_0()
            .cursor_pointer()
            .tooltip(super::tooltip::Tooltip::text("Manage ⌘,"))
            .child(kit::cap(
                look,
                "Manage",
                if on { look.c.text } else { look.c.faint },
            ))
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
        let health = (!ok).then(|| {
            div()
                .id("foot-health")
                .role(Role::Button)
                .aria_label(format!("{word}: show hippo's health"))
                .min_w_0()
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.health_open = !this.health_open;
                    cx.notify();
                }))
                .child(kit::cap(look, &word, look.c.warn).truncate())
        });
        let foot = div()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(look.px(12.))
            .h(look.px(40.))
            .px(look.px(16.))
            .child(manage)
            .when(!narrow, |d| {
                d.child(div().flex_1())
                    .children(health)
                    .children(indexed.filter(|_| ok).map(|t| {
                        div()
                            .flex()
                            .items_center()
                            .gap(look.px(8.))
                            .child(kit::cap(look, "Indexed", look.c.faint))
                            .child(kit::cap(look, &t, look.c.text_muted))
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
                    .px(look.px(16.))
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

    /// The sidebar: the head, the sections and the foot. The day's
    /// sessions are listed in Fleet, not here.
    pub(super) fn render_sidebar(
        &self,
        look: &Look,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // There is no rail; below [`NARROW`] the sidebar is an
        // overlay of its full width.
        let narrow = false;
        let nav = div()
            .flex()
            .flex_col()
            .border_t(look.fixed(1.))
            .border_color(look.c.border)
            .when(narrow, |d| d.items_center())
            .child(self.nav_row(look, Section::Home, narrow, cx))
            .child(self.nav_row(look, Section::Inbox, narrow, cx))
            .child(self.nav_row(look, Section::Fleet, narrow, cx))
            .child(self.nav_row(look, Section::Incidents, narrow, cx))
            .child(self.nav_row(look, Section::Memory, narrow, cx));
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
            .child(nav)
            .child(div().flex_1())
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

    /// Under the pointer, a Rows row's Snooze and Settle (or Un-settle,
    /// Wake now) take its state's place; `None` when the row is not
    /// hovered or has none.
    pub(super) fn row_hover_controls(
        &self,
        look: &Look,
        r: &Row,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        if !self.row_hovered(&r.id) {
            return None;
        }
        let c = shelving::controls(self.marks().place(r));
        let id = r.id.clone();
        if c.settle || c.snooze {
            Some(self.row_controls(look, &r.id, c, cx).into_any_element())
        } else if c.wake {
            Some(
                Self::row_control(
                    look,
                    format!("wake-{}", r.id),
                    "Wake",
                    "Wake now: back on its shelf",
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.snooze(&id, None);
                    this.sidebar.hover = None;
                    cx.notify();
                }))
                .into_any_element(),
            )
        } else if c.unsettle {
            Some(
                Self::row_control(
                    look,
                    format!("unsettle-{}", r.id),
                    "Un-settle",
                    "Un-settle: back to To review",
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.settle(&id, false);
                    this.sidebar.hover = None;
                    cx.notify();
                }))
                .into_any_element(),
            )
        } else {
            None
        }
    }

    /// Keeps the hovered row for [`Shell::row_hover_controls`].
    pub(super) fn row_hover(
        &self,
        id: &str,
        cx: &mut Context<Self>,
    ) -> impl Fn(&bool, &mut Window, &mut gpui::App) + 'static {
        self.hover_listener(id, cx)
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
}
