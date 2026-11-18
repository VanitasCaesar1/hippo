//! Home (docs/designs/app-instrument.md): one column of readouts, the
//! scope band, what waits on the person and last night's pass, with the
//! prompt at the foot.

use gpui::{Context, Focusable, Role, SharedString, Window, div, prelude::*};
use hippo_core::usage::{Headroom, Left, Source};
use hippo_view::fleet::{Fleet, State};

use super::Shell;
use super::kit;
use super::pulse_ui::{Item, item_words, two};
use super::theme::Look;
use crate::state::Section;
use hippo_view::pulse::Next;
use time::OffsetDateTime;

/// Home's column at its widest, in points.
pub const HOME_W: f32 = 800.;

/// Below this window width the Ask bar's placeholder is the short one and
/// the Needs you rows drop their detail.
pub const COMPACT: f32 = 900.;

/// Profiles in the order the strip shows them: by what is left of the
/// tighter window, Codex last.
pub fn ordered<'a>(rooms: &'a [Headroom], next: Option<&str>) -> Vec<&'a Headroom> {
    // Use next leads, so an inferred `~100%` never sits first.
    let mut out = hippo_view::fleet::ordered_rooms(rooms);
    if let Some(i) = out.iter().position(|h| Some(h.profile.as_str()) == next) {
        let h = out.remove(i);
        out.insert(0, h);
    }
    out
}

/// A window's percent as text: `92%`, `~100%` when inferred,
/// `92%*` for the transcript estimate.
/// [`pct_text`] with `left` (every meter says which way it
/// counts): `97% left`, `~100% left`.
pub fn pct_left(l: &Left) -> String {
    format!("{} left", pct_text(l))
}

pub fn pct_text(l: &Left) -> String {
    let p = hippo_view::fleet::pct(l.frac);
    if hippo_view::fleet::inferred(l) {
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

    /// Home (docs/designs/app-instrument.md): the date, the readouts, the
    /// scope band, what waits on the person, last night's pass, and the
    /// prompt at the foot.
    pub(super) fn render_home_tab(
        &mut self,
        look: &Look,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        self.pulse_ensure(cx);
        let fleet = self.loaded.as_ref().and_then(|l| l.fleet.clone());
        let width = f32::from(window.viewport_size().width);
        let short = width < COMPACT;
        self.home_review_width
            .set(look.px((width - self.sidebar_width(window) - 80.).clamp(0., HOME_W)));
        self.ask_field.update(cx, |f, _| {
            f.set_placeholder(if short {
                "Ask, or brief a new agent"
            } else {
                "Ask what your agents already know, or brief a new one"
            })
        });
        let now = OffsetDateTime::now_utc();
        let at = |s: i64| OffsetDateTime::from_unix_timestamp(s).unwrap_or(now);
        let data = self.pulse.data.clone();
        let local = now.to_offset(super::status::local_offset());
        let date = format!(
            "{} {} {} · {}",
            &local.weekday().to_string()[..3],
            local.day(),
            &local.month().to_string()[..3],
            kit::hm(now)
        );
        // What waits: the sessions, then an account that runs dry.
        let (needs, next) = fleet
            .as_ref()
            .map(|f| self.pulse_queue(f))
            .unwrap_or_default();
        let waiting: Vec<Item> = needs
            .iter()
            .cloned()
            .chain(
                next.iter()
                    .filter(|i| matches!(i, Item::Next(Next::RunsDry { .. })))
                    .cloned(),
            )
            .collect();
        let mut cells: Vec<super::pulse_ui::Readout> = Vec::new();
        if let Some(f) = fleet.as_ref() {
            let t = hippo_view::pulse::totals(f, data.as_ref().map(|d| d.day.as_ref()), now);
            let oldest = needs.iter().find_map(|i| match i {
                Item::Wait(r) => match r.state {
                    State::Waiting { at, .. } | State::Stopped { at, .. } => Some(at),
                    _ => None,
                },
                Item::Next(_) => None,
            });
            cells.push((
                "Waiting on you".into(),
                two(needs.len()),
                if needs.is_empty() {
                    look.c.text
                } else {
                    look.c.warn
                },
                oldest.map(|o| (format!("oldest {}", kit::span(now - o)), look.c.faint)),
            ));
            cells.push((
                "Working".into(),
                two(t.working),
                look.c.text,
                (t.subs > 0).then(|| {
                    (
                        format!("{} sub-agent{}", t.subs, if t.subs == 1 { "" } else { "s" }),
                        look.c.faint,
                    )
                }),
            ));
            for p in super::pulse_ui::rail_profiles(f) {
                let Some(h) = f.room(&p) else { continue };
                let Some(left) = h.five.map(|l| l.frac).or_else(|| h.left()) else {
                    continue;
                };
                let dry = f.drain(&p).and_then(|d| d.empty_at);
                let reset = h.five.and_then(|l| l.resets_at).or_else(|| h.resets());
                cells.push((
                    format!("{} left", kit::profile_name(&p)),
                    format!("{}%", (left.clamp(0., 1.) * 100.).round()),
                    if dry.is_some() {
                        look.c.err
                    } else {
                        look.c.text
                    },
                    match (dry, reset) {
                        (Some(e), _) => Some((format!("empty {}", kit::hm(at(e))), look.c.err)),
                        (None, Some(r)) => {
                            Some((format!("resets {}", kit::hm(at(r))), look.c.faint))
                        }
                        _ => None,
                    },
                ));
            }
        }
        let rows = waiting.iter().map(|item| {
            let (mark, title, reason, age) = item_words(item, now);
            let key = item.key();
            div()
                .id(SharedString::from(format!("home-wait-{key}")))
                .flex()
                .items_start()
                .gap(look.px(10.))
                .py(look.px(11.))
                .border_t(look.fixed(1.))
                .border_color(look.c.border.opacity(0.6))
                .cursor_pointer()
                .hover(|h| h.bg(look.c.hover))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.replay.lanes = false;
                    this.pulse.on = true;
                    this.show_section(Section::Fleet, window, cx);
                    this.pulse_select(key.clone());
                    cx.notify();
                }))
                .child(
                    div()
                        .flex_shrink_0()
                        .pt(look.px(5.))
                        .child(kit::square(look, mark)),
                )
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
                                .gap(look.px(10.))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .font_weight(gpui::FontWeight::MEDIUM)
                                        .child(title),
                                )
                                .child(kit::figure(look, age, 11., look.c.faint)),
                        )
                        .child(
                            div()
                                .text_color(look.c.text_muted)
                                .line_clamp(2)
                                .text_ellipsis()
                                .child(reason),
                        ),
                )
        });
        let waiting_block = (!waiting.is_empty()).then(|| {
            div()
                .flex()
                .flex_col()
                .mt(look.px(28.))
                .child(kit::cap(look, "Waiting on you", look.c.faint).mb(look.px(8.)))
                .children(rows)
        });
        let night = self.render_last_night(data.as_deref(), look, cx);
        let column = div()
            .flex()
            .flex_col()
            .w_full()
            .max_w(look.px(HOME_W))
            .pt(look.px(40.))
            .pb(look.px(16.))
            .text_size(look.text(13.))
            .children(self.render_get_started(look, cx))
            .child(kit::cap(look, &date, look.c.faint))
            .when(!cells.is_empty(), |d| {
                d.child(super::pulse_ui::readouts(look, cells).mt(look.px(14.)))
            })
            .when_some(fleet.as_ref(), |d, f| {
                d.child(
                    div()
                        .border_b(look.fixed(1.))
                        .border_color(look.c.border)
                        .pt(look.px(8.))
                        .child(super::pulse_ui::scope_band(
                            f,
                            data.as_deref(),
                            now - time::Duration::hours(3),
                            now,
                            look,
                            60.,
                        )),
                )
            })
            .children(self.render_reach_said(look))
            .children(
                self.loaded
                    .as_ref()
                    .and_then(|l| l.fleet_error.clone())
                    .map(|e| {
                        div()
                            .mt(look.px(12.))
                            .text_color(look.c.err)
                            .child(format!("Fleet: {e}"))
                    }),
            )
            .children(waiting_block)
            .child(night);
        div()
            .id("home")
            .role(Role::Region)
            .aria_label("Home")
            .flex()
            .flex_col()
            .size_full()
            .px(look.px(40.))
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
                            .child(div().flex().justify_center().child(column)),
                    )
                    .children(super::scrollbar::vertical(&self.home_scroll, look)),
            )
            .child(
                div().flex().justify_center().child(
                    div()
                        .w_full()
                        .max_w(look.px(HOME_W))
                        .flex_shrink_0()
                        .pb(look.px(16.))
                        .children(self.render_ask_recent(fleet.as_ref(), window, look, cx))
                        .child(self.render_composer(true, look, window, cx)),
                ),
            )
            .into_any_element()
    }

    /// Last night: the nightly pass's notes and rules as label and value
    /// rows, and what tonight's pass will read.
    fn render_last_night(
        &self,
        data: Option<&super::pulse_ui::PulseData>,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let run = self
            .loaded
            .as_ref()
            .and_then(|l| l.panels.as_ref())
            .and_then(|p| p.dreaming.as_ref().ok())
            .and_then(|d| d.runs.first().cloned());
        let facts = run.as_ref().map(crate::rows::run_facts);
        let to_review = self.review_items().len();
        let rules = self.lessons().len();
        let link = |id: &'static str, text: &str| {
            div()
                .id(id)
                .cursor_pointer()
                .child(kit::cap(look, text, look.c.text))
        };
        let mut spec = kit::spec(look).mt(look.px(8.));
        if let Some(f) = &facts {
            if f.failed {
                spec = spec.child(kit::spec_row(
                    look,
                    "Pass",
                    f.error.clone().unwrap_or_else(|| "It failed.".to_owned()),
                    Some(("Failed".to_owned(), look.c.err)),
                ));
            } else {
                spec = spec.child(kit::spec_row(
                    look,
                    "Notes",
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(look.px(6.))
                        .child(format!("{} updated", f.applied))
                        .when(f.applied > 0, |d| {
                            d.child(
                                div()
                                    .text_color(look.c.text_muted)
                                    .child("· each confirmed by a fresh reviewer"),
                            )
                        })
                        .when(f.checked > 0, |d| {
                            d.child(
                                div()
                                    .text_color(look.c.text_muted)
                                    .child(format!("· {} checked", f.checked)),
                            )
                        }),
                    None,
                ));
            }
        }
        if to_review > 0 {
            spec = spec.child(
                kit::spec_row(
                    look,
                    "To look at",
                    format!(
                        "{to_review} note change{}",
                        if to_review == 1 { "" } else { "s" }
                    ),
                    None,
                )
                .child(link("home-night-notes", "Open").on_click(cx.listener(
                    |this, _, window, cx| {
                        this.show_section(Section::Inbox, window, cx);
                        this.inbox.filter = super::lessons::Filter::Overnight;
                        cx.notify();
                    },
                ))),
            );
        }
        spec = spec.child(
            kit::spec_row(look, "Rules", format!("{rules} proposed"), None).when(rules > 0, |d| {
                d.child(link("home-night-rules", "Review").on_click(cx.listener(
                    |this, _, window, cx| {
                        this.show_section(Section::Inbox, window, cx);
                        this.inbox.filter = super::lessons::Filter::All;
                        cx.notify();
                    },
                )))
            }),
        );
        if let Some(t) = data.and_then(|d| d.tonight.as_ref()) {
            spec = spec.child(kit::spec_row(
                look,
                "Tonight",
                div().flex().gap(look.px(6.)).child(t.at.clone()).child(
                    div().text_color(look.c.text_muted).child(format!(
                        "· reads {} session{}",
                        t.sessions,
                        if t.sessions == 1 { "" } else { "s" }
                    )),
                ),
                None,
            ));
        }
        let when = facts
            .as_ref()
            .and_then(|f| f.when)
            .map(|t| format!("Last night · {}", kit::hm(t)))
            .unwrap_or_else(|| "Last night".to_owned());
        div()
            .flex()
            .flex_col()
            .mt(look.px(28.))
            .child(kit::cap(look, &when, look.c.faint))
            .child(spec)
            .into_any_element()
    }
}
