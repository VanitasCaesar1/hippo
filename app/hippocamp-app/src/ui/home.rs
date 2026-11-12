//! Home: the Ask bar, each profile's headroom, and the Inbox and Fleet at a
//! glance.

use gpui::{Context, Focusable, Role, Window, div, prelude::*};
use hippocamp_core::usage::{Headroom, Left, Source};
use hippocamp_view::fleet::{Fleet, State};

use super::Shell;
use super::kit;
use super::theme::Look;

/// Below this window width the Ask bar's placeholder is the short one and
/// the Needs you rows drop their detail.
pub const COMPACT: f32 = 900.;

/// Profiles in the order the strip shows them: by what is left of the
/// tighter window, Codex last.
pub fn ordered<'a>(rooms: &'a [Headroom], next: Option<&str>) -> Vec<&'a Headroom> {
    // Use next leads, so an inferred `~100%` never sits first.
    let mut out = hippocamp_view::fleet::ordered_rooms(rooms);
    if let Some(i) = out.iter().position(|h| Some(h.profile.as_str()) == next) {
        let h = out.remove(i);
        out.insert(0, h);
    }
    out
}

/// A headroom card's word at the right of its name row: `use next`,
/// `sign-in failed`, `limit hit`, `1 working`, `idle 2 days`. `working` is
/// the account's share of the Working shelf (the counts agree).
pub fn room_word(h: &Headroom, fleet: &Fleet, working: usize, now: i64) -> (String, bool) {
    let (_, today) = fleet.counts(&h.profile);
    let live = working;
    if fleet.signed_out.contains(&h.profile) {
        return ("sign-in failed".into(), false);
    }
    if !h.usable() && h.left().is_some() {
        return ("limit hit".into(), false);
    }
    if fleet.use_next.as_deref() == Some(h.profile.as_str()) {
        return ("use next".into(), true);
    }
    if live > 0 {
        return (format!("{live} working"), false);
    }
    // An old meter says so, in place of `idle`.
    if let Some(age) = hippocamp_view::fleet::meter_old(h, now) {
        return (
            format!("meter {} old", hippocamp_view::fleet::age(age)),
            false,
        );
    }
    match h.last_seen {
        Some(t) if today == 0 => {
            let days = (now - t) / 86_400;
            if days >= 1 {
                (
                    format!("idle {days} day{}", if days == 1 { "" } else { "s" }),
                    false,
                )
            } else {
                (format!("idle since {}", when(t, now)), false)
            }
        }
        _ if today > 0 => (format!("{today} today"), false),
        _ => (String::new(), false),
    }
}

/// `14:00` today, else `Wed 01:30`.
fn when(t: i64, now: i64) -> String {
    hippocamp_view::fleet::when(
        t,
        time::OffsetDateTime::from_unix_timestamp(now).unwrap_or(time::OffsetDateTime::UNIX_EPOCH),
        super::status::local_offset(),
    )
}

/// A window's reset as the card shows it: `14:00`, `Wed 01:30`, `no
/// window`, or `reset Sun 04:30` when the meter was not refreshed since
/// (the figure is then a guess).
pub fn reset_label(l: &Left, now: i64) -> String {
    reset_text(l, now)
}

fn reset_text(l: &Left, now: i64) -> String {
    // A card is narrow: past a week the date alone (`17 Nov`), and a
    // passed reset its day (`reset Sun`; the time is in the tooltip).
    let short = |w: String| match w.split(' ').collect::<Vec<_>>().as_slice() {
        [d, m, _] => format!("{d} {m}"),
        _ => w,
    };
    match (l.resets_at, l.reset_passed) {
        (Some(r), _) => short(when(r, now)),
        (None, Some(r)) => {
            let w = when(r, now);
            format!("reset {}", w.split(' ').next().unwrap_or(&w))
        }
        // No window open yet: nothing to say.
        (None, None) => String::new(),
    }
}

/// A window's percent as text: `92%`, `~100%` when inferred,
/// `92%*` for the transcript estimate.
/// [`pct_text`] with `left` (every meter says which way it
/// counts): `97% left`, `~100% left`.
pub fn pct_left(l: &Left) -> String {
    format!("{} left", pct_text(l))
}

pub fn pct_text(l: &Left) -> String {
    let p = hippocamp_view::fleet::pct(l.frac);
    if hippocamp_view::fleet::inferred(l) {
        format!("~{p}%")
    } else if l.source == Source::Estimate {
        format!("{p}%*")
    } else {
        format!("{p}%")
    }
}

impl Shell {
    /// The questions the empty Ask bar offers on focus: the last
    /// five asked, then up to two `Where did "<title>" stop?` for today's
    /// sessions that stopped at a limit or were interrupted.
    pub(crate) fn ask_offers(&self, fleet: Option<&Fleet>) -> Vec<String> {
        let mut out: Vec<String> = self.recent_questions.clone();
        let stopped = fleet
            .into_iter()
            .flat_map(|f| f.rows.iter())
            .filter(|r| matches!(r.state, State::Stopped { .. } | State::Interrupted { .. }))
            .map(|r| {
                let t = r.title.trim().trim_end_matches('.').trim();
                format!("Where did \"{t}\" stop?")
            })
            .filter(|q| !out.contains(q))
            .take(2)
            .collect::<Vec<_>>();
        out.extend(stopped);
        out
    }

    fn render_ask_recent(
        &self,
        fleet: Option<&Fleet>,
        window: &Window,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let field = self.ask_field.read(cx);
        if !field.focus_handle(cx).is_focused(window)
            || !field.text().trim().is_empty()
            || self.brief.mode
        {
            return None;
        }
        let offers = self.ask_offers(fleet);
        if offers.is_empty() {
            return None;
        }
        let recent = self.recent_questions.len();
        Some(
            div()
                .mt(look.px(6.))
                .flex()
                .flex_col()
                .py(look.px(6.))
                .rounded(look.px(10.))
                .border(look.fixed(1.))
                .border_color(look.c.border)
                .bg(look.c.elevated)
                .children(offers.into_iter().enumerate().map(|(i, q)| {
                    let q2 = q.clone();
                    div()
                        .id(("ask-offer", i))
                        .role(Role::ListBoxOption)
                        .aria_label(q.clone())
                        .flex()
                        .items_center()
                        .gap(look.px(10.))
                        .h(look.px(30.))
                        .px(look.px(14.))
                        .cursor_pointer()
                        .hover(|d| d.bg(look.c.hover))
                        .child(kit::icon(
                            look,
                            if i < recent { "search" } else { "fleet" },
                            14.,
                            kit::dim(look),
                        ))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(look.c.text)
                                .child(q),
                        )
                        .when(i == 0 || i == recent, |d| {
                            d.child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(look.text(11.5))
                                    .text_color(kit::dim(look))
                                    .child(if i < recent {
                                        "recent"
                                    } else {
                                        "stopped today"
                                    }),
                            )
                        })
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.ask_field
                                    .update(cx, |f, cx| f.set_text(&q2, window, cx));
                                this.submit_ask(false, window, cx);
                            }),
                        )
                }))
                .into_any_element(),
        )
    }

    /// Brings T3 Code to the front (`open -a`), where a waiting session is.
    pub(crate) fn open_t3(&mut self, _cx: &mut Context<Self>) {
        if let Some(app) = super::brief::t3_app() {
            let _ = std::process::Command::new("/usr/bin/open")
                .arg("-a")
                .arg(app)
                .status();
        }
    }

    /// The Ask bar: the question field, the Ask and Brief toggle (Brief
    /// comes later) and its key.
    pub(super) fn render_ask_bar(
        &self,
        on_home: bool,
        look: &Look,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let field = self.ask_field.clone();
        let focus = field.focus_handle(cx);
        let focused = focus.is_focused(window);
        let brief = self.brief.mode;
        let segment = |label: &'static str, on: bool| {
            div()
                .id(label)
                .role(Role::Tab)
                .aria_label(label)
                .aria_selected(on)
                .px(look.px(10.))
                .h(look.px(24.))
                .flex()
                .items_center()
                .rounded(look.px(5.))
                .text_size(look.text(12.5))
                .cursor_pointer()
                .when(on, |d| d.bg(look.c.selected).text_color(look.c.text))
                .when(!on, |d| {
                    d.text_color(look.c.text_muted)
                        .hover(|h| h.text_color(look.c.text))
                })
                .child(label)
        };
        div()
            .id(if on_home { "ask-bar-home" } else { "ask-bar" })
            .key_context("AskField")
            .flex()
            .items_center()
            .gap(look.px(12.))
            .w_full()
            .h(look.px(52.))
            .px(look.px(16.))
            .rounded(look.px(12.))
            .border(look.fixed(1.5))
            // The accent is the focus ring only.
            .border_color(if focused {
                look.c.text_accent.opacity(0.75)
            } else {
                look.c.border
            })
            .bg(look.c.elevated.opacity(0.7))
            .cursor_text()
            .on_click(move |_, window, cx| window.focus(&focus, cx))
            .child(kit::icon(
                look,
                if brief { "sparkle" } else { "search" },
                18.,
                look.c.text_muted,
            ))
            .child(div().flex_1().min_w_0().overflow_hidden().child(field))
            .child(
                div()
                    .id("ask-mode")
                    .flex()
                    .flex_shrink_0()
                    .p(look.px(2.))
                    .gap(look.px(2.))
                    .rounded(look.px(7.))
                    .border(look.fixed(1.))
                    .border_color(look.c.border_variant)
                    .tooltip(super::tooltip::Tooltip::text(
                        "Ask: has any agent already solved this? · Brief: the start block for a new agent on a task (⌘B)",
                    ))
                    .child(segment("Ask", !brief).on_click(cx.listener(|this, _, window, cx| {
                        this.set_brief_mode(false, window, cx)
                    })))
                    .child(segment("Brief", brief).on_click(cx.listener(|this, _, window, cx| {
                        this.set_brief_mode(true, window, cx)
                    }))),
            )
            // Off Home the back bar shows `esc` once.
            .when(on_home, |d| d.child(kit::kbd(look, "⌘K")))
            .into_any_element()
    }

    /// The Home.
    pub(super) fn render_home_tab(
        &mut self,
        look: &Look,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let fleet = self.loaded.as_ref().and_then(|l| l.fleet.clone());
        let width = f32::from(window.viewport_size().width);
        let narrow = width < super::sidebar::NARROW;
        let short = width < COMPACT;
        // The pane column only where the centre keeps room for its rows and
        // buttons; below that the strip stands in for Headroom.
        let fits = width >= super::home_panes::PANES_FROM;
        {
            let _ = narrow;
            let side = self.sidebar_width(window);
            let panes = if fits {
                super::home_panes::PANES + 16.
            } else {
                0.
            };
            self.home_review_width
                .set(look.px((width - side - 32. - panes).max(0.)));
        }
        self.ask_field.update(cx, |f, _| {
            f.set_placeholder(if short {
                "Ask, or brief a new agent"
            } else {
                "Has any agent already solved this? Ask, or describe a task to brief a new agent"
            })
        });
        let needs: Vec<gpui::AnyElement> = fleet
            .as_ref()
            .map(|f| {
                f.needs_you()
                    .iter()
                    .take(if narrow { 3 } else { 5 })
                    .enumerate()
                    .map(|(i, n)| self.render_need_row(n, f, i, narrow, look, cx))
                    .collect()
            })
            .unwrap_or_default();
        let inbox = self.render_inbox_preview(look, cx);
        let centre = div()
            .flex()
            .flex_col()
            .gap(look.px(10.))
            .w_full()
            .pb(look.px(16.))
            .text_size(look.text(13.))
            .when_some(
                fleet
                    .as_ref()
                    .filter(|_| !fits && !self.panes_off.contains("pane-headroom")),
                |d, f| d.child(self.render_room_strip(f, look)),
            )
            .child(self.render_hero(fleet.as_ref(), look, cx))
            .children(needs)
            .children(self.render_reach_said(look))
            .children(self.render_get_started(look, cx))
            .children(self.render_digest(look, cx))
            .children(
                self.loaded
                    .as_ref()
                    .and_then(|l| l.fleet_error.clone())
                    .map(|e| div().text_color(look.c.err).child(format!("Fleet: {e}"))),
            )
            .child(inbox);
        let show_head = !self.panes_off.contains("pane-headroom");
        let show_mem = !self.panes_off.contains("pane-memory");
        let panes = (fits && (show_head || show_mem)).then(|| {
            div()
                .id("home-panes")
                .flex()
                .flex_col()
                .gap(look.px(8.))
                .flex_shrink_0()
                .w(look.px(super::home_panes::PANES))
                .h_full()
                .pb(look.px(16.))
                .overflow_y_scroll()
                .when_some(fleet.as_ref().filter(|_| show_head), |d, f| {
                    d.child(self.render_headroom_pane(f, look, cx))
                })
                .when(show_mem, |d| d.child(self.render_memory_pane(look, cx)))
        });
        div()
            .id("home")
            .role(Role::Region)
            .aria_label("Home")
            .flex()
            .gap(look.px(16.))
            .size_full()
            .px(look.px(16.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .min_h_0()
                            .child(
                                div()
                                    .id("home-scroll")
                                    .size_full()
                                    .overflow_y_scroll()
                                    .track_scroll(&self.home_scroll)
                                    .child(centre),
                            )
                            .children(super::scrollbar::vertical(&self.home_scroll, look)),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .pb(look.px(16.))
                            .children(self.render_ask_recent(fleet.as_ref(), window, look, cx))
                            .child(self.render_composer(true, look, window, cx)),
                    ),
            )
            .children(panes)
            .into_any_element()
    }

    fn render_inbox_preview(&self, look: &Look, cx: &mut Context<Self>) -> gpui::AnyElement {
        let all_changes = self.review_items();
        let lessons = self.lessons().len();
        let n = all_changes.len();
        // The Inbox's sections as rows on the canvas, no card: the
        // note changes, then the corrections and each group with Keep all.
        let lesson_rows = self.render_home_lessons(12, look, cx);
        let newest = self
            .loaded
            .as_ref()
            .and_then(|l| l.panels.as_ref())
            .and_then(|p| p.dreaming.as_ref().ok())
            .and_then(|d| d.runs.first().cloned());
        div()
            .flex()
            .flex_col()
            .gap(look.px(2.))
            .map(|d| {
                if n > 0 {
                    d.child(self.overnight_card(0, &all_changes, false, true, look, cx))
                } else if lessons == 0 {
                    d.child(div().py(look.px(4.)).text_color(look.c.text_muted).child(
                        match &newest {
                            Some(run) => format!(
                                "Nothing in the Inbox. Last dreaming run: {}.",
                                super::run_tab::headline(&crate::rows::run_facts(run))
                            ),
                            None => "Nothing in the Inbox yet.".to_owned(),
                        },
                    ))
                } else {
                    d
                }
            })
            .children(lesson_rows)
            .children(self.review_result(look))
            .children(self.lesson_result(look))
            .into_any_element()
    }
}
