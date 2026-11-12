//! Home's parts: the hero, the needs-you task rows, the Headroom and
//! Memory pane cards, the narrow chip strip and the composer.

use gpui::{Context, Focusable, FontWeight, Role, Window, div, prelude::*, relative};
use hippocamp_core::usage::{Headroom, Left};
use hippocamp_view::fleet::{Fleet, Need, State, Why};

use crate::shelving::Tally;

use super::Shell;
use super::kit::{self, Kind, Status};
use super::theme::Look;
use crate::state::Section;

/// The right column's width, in points.
pub const PANES: f32 = 288.;
/// The narrowest window that shows the right column.
pub const PANES_FROM: f32 = 1100.;

/// The headline: `2 agents need you`; with none, `No agent
/// needs you` while the Inbox holds something to review (shown beside it
/// as a link, `14 in the Inbox`, so the two never read as a contradiction),
/// else `All clear`. Then `2 working · 15 to review` and `· 1 snoozed`.
/// Every number is a sidebar shelf's ([`Tally`]).
pub fn hero_words(tally: Option<Tally>, inbox: usize) -> (String, String) {
    let n = tally.map_or(0, |t| t.needs_you);
    let head = match (n, inbox) {
        (0, 0) => "All clear".to_owned(),
        (0, _) => "No agent needs you".to_owned(),
        (1, _) => "1 agent needs you".to_owned(),
        (n, _) => format!("{n} agents need you"),
    };
    let Some(t) = tally else {
        return (head, String::new());
    };
    let mut facts = vec![if n > 0 {
        format!("{} working", t.working)
    } else {
        format!(
            "{} agent{} working",
            t.working,
            if t.working == 1 { "" } else { "s" }
        )
    }];
    if t.to_review > 0 {
        facts.push(format!("{} to review", t.to_review));
    }
    if t.snoozed > 0 {
        facts.push(format!("{} snoozed", t.snoozed));
    }
    (head, facts.join(" · "))
}

/// The tighter of an account's two windows, an inferred one first on a tie.
pub fn tighter(h: &Headroom) -> Option<Left> {
    [h.five, h.seven].into_iter().flatten().min_by(|a, b| {
        a.frac.total_cmp(&b.frac).then_with(|| {
            hippocamp_view::fleet::inferred(b).cmp(&hippocamp_view::fleet::inferred(a))
        })
    })
}

impl Shell {
    pub(super) fn render_hero(
        &self,
        fleet: Option<&Fleet>,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let inbox = self.inbox_count();
        let tally = fleet.and(self.tally());
        let (head, facts) = hero_words(tally, inbox);
        let calm = tally.is_none_or(|t| t.needs_you == 0);
        let review = (calm && inbox > 0).then(|| {
            div()
                .id("hero-review")
                .flex_shrink_0()
                .text_size(look.text(13.))
                .text_color(look.c.accent_text)
                .cursor_pointer()
                .hover(|h| h.underline())
                .child(format!("{inbox} in the Inbox"))
                .on_click(cx.listener(|this, _, window, cx| {
                    this.show_section(Section::Inbox, window, cx);
                }))
        });
        div()
            .flex()
            .items_baseline()
            .gap(look.px(12.))
            .min_w_0()
            .pt(look.px(6.))
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(look.text(20.))
                    .line_height(look.px(28.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(head),
            )
            .children(review.map(|r| {
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_baseline()
                    .gap(look.px(6.))
                    .text_size(look.text(13.))
                    .child(div().text_color(look.c.faint).child("·"))
                    .child(r)
            }))
            // The day's incidents, a link to the list.
            .children(self.incidents_home_link(look, cx))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(look.text(13.))
                    .text_color(look.c.text_muted)
                    .child(facts),
            )
    }

    /// One needs-you task row: warn glyph, account mark, title, `Input ·
    /// 6m` and what was asked, the kind chip, one button (the first row's
    /// is the view's primary).
    pub(super) fn render_need_row(
        &self,
        need: &Need<'_>,
        fleet: &Fleet,
        i: usize,
        narrow: bool,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let row = need.row;
        let now = time::OffsetDateTime::now_utc();
        let (status, detail, said, kind) = match (need.why, &row.state) {
            (Why::Waiting, State::Waiting { at, asked, plan }) => (
                if *plan {
                    Status::Approval
                } else {
                    Status::Input
                },
                kit::span(now - *at),
                asked
                    .as_ref()
                    .map(|q| format!("\u{201c}{}\u{201d}", q.trim()))
                    .unwrap_or_default(),
                if *plan { "Plan" } else { "Question" },
            ),
            (Why::Stopped, State::Stopped { at, reset, .. }) => (
                Status::Limited,
                kit::hm(*at),
                match reset {
                    Some(r) => format!("usage limit, {r}"),
                    None => "stopped at its usage limit".to_owned(),
                },
                "Limit",
            ),
            _ => {
                let (claim, why) = row.claims.first_unverified.clone().unwrap_or_default();
                (
                    Status::Input,
                    String::new(),
                    format!(
                        "\u{201c}{}\u{201d} · {}",
                        claim.trim(),
                        why.split(';').next().unwrap_or(&why)
                    ),
                    "Claim",
                )
            }
        };
        let claim = need.why == Why::Check;
        let (glyph, gc) = if claim {
            ("triangle-alert", look.c.warn)
        } else {
            status.glyph(look)
        };
        let primary = if i == 0 { Kind::Primary } else { Kind::Plain };
        let button: gpui::AnyElement = match need.why {
            Why::Waiting => {
                let r = row.clone();
                kit::button_key(
                    look,
                    ("need-open", i),
                    "Open thread",
                    if i == 0 { "↵" } else { "" },
                    primary,
                )
                .tooltip(super::tooltip::Tooltip::text(
                    "Opens the thread in T3 Code or the agent's app. hippocamp never answers for you.",
                ))
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.open_session(r.clone(), window, cx);
                }))
                .into_any_element()
            }
            Why::Stopped => self.resume_button(row, fleet, 900 + i, look, cx),
            Why::Check => {
                let id = row.id.clone();
                kit::button(look, ("need-xray", i), "Open x-ray", primary)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.open_xray(&id, window, cx);
                    }))
                    .into_any_element()
            }
        };
        let id = row.id.clone();
        let name = if claim { "Unverified" } else { status.word() };
        let word = if detail.is_empty() {
            name.to_owned()
        } else {
            format!("{name} · {detail}")
        };
        div()
            .id(("need", i))
            .role(Role::Link)
            .aria_label(format!("{}: {word}", row.title))
            .flex()
            .items_center()
            .gap(look.px(10.))
            .px(look.px(12.))
            .py(look.px(10.))
            .rounded(look.px(12.))
            .border(look.fixed(1.))
            .border_color(look.c.border)
            .bg(look.c.surface)
            .cursor_pointer()
            .hover(|d| d.bg(look.c.hover))
            .child(kit::icon(look, glyph, 16., gc))
            .child(kit::mark(look, row.profile.as_deref(), 24.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .truncate()
                            .text_size(look.text(13.5))
                            .child(kit::title(&row.title)),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(look.px(6.))
                            .min_w_0()
                            .text_size(look.text(12.))
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(status.color(look))
                                    .child(word),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(look.c.text_muted)
                                    .child(said),
                            ),
                    ),
            )
            .when(!narrow, |d| d.child(kit::kind_chip(look, kind)))
            .child(button)
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_xray(&id, window, cx);
            }))
            .into_any_element()
    }

    /// One account in the Headroom pane: mark, name and its word or tag,
    /// then the 5-hour and 7-day bars.
    fn headroom_block(
        &self,
        h: &Headroom,
        fleet: &Fleet,
        first: bool,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        // The account's share of the Working shelf.
        let live = crate::shelving::Shelves::of(fleet, &self.marks()).working_on(&h.profile);
        let (word, next) = super::home::room_word(h, fleet, live, now);
        let waiting = fleet
            .rows
            .iter()
            .filter(|r| r.profile.as_deref() == Some(h.profile.as_str()) && r.state.is_waiting())
            .count();
        let fact: gpui::AnyElement = if next {
            kit::tag(look, "use next").into_any_element()
        } else if waiting > 0 {
            div()
                .flex()
                .items_center()
                .gap(look.px(4.))
                .text_color(look.c.warn)
                .child(kit::icon(look, "message-circle-question", 12., look.c.warn))
                .child(format!("{waiting} waiting"))
                .into_any_element()
        } else if live > 0 {
            div()
                .flex()
                .items_center()
                .gap(look.px(4.))
                .text_color(look.c.info)
                .child(kit::icon(look, "circle-dashed", 12., look.c.info))
                .child(format!("{live} working"))
                .into_any_element()
        } else {
            let quiet = h.five.is_none() && h.seven.is_none();
            div()
                .min_w_0()
                .truncate()
                .text_color(look.c.faint)
                // An account with no meter says nothing.
                .child(if word.is_empty() && quiet {
                    String::new()
                } else {
                    word
                })
                .into_any_element()
        };
        let quiet = h.five.is_none() && h.seven.is_none();
        let profile = h.profile.clone();
        div()
            .id(gpui::SharedString::from(format!("headroom-{}", h.profile)))
            .role(Role::Button)
            .aria_label(format!("{} headroom", kit::profile_name(&h.profile)))
            .flex()
            .flex_col()
            .gap(look.px(4.))
            .px(look.px(14.))
            .py(look.px(9.))
            .when(!first, |d| {
                d.border_t(look.fixed(1.)).border_color(look.c.border)
            })
            .cursor_pointer()
            .hover(|d| d.bg(look.c.hover))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(8.))
                    .h(look.px(22.))
                    .text_size(look.text(12.))
                    .child(kit::mark(look, Some(&h.profile), 18.))
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(look.text(13.))
                            .font_weight(FontWeight::MEDIUM)
                            .child(kit::profile_name(&h.profile)),
                    )
                    .child(div().flex_1())
                    .child(div().flex_shrink_0().child(fact)),
            )
            .when(!quiet, |d| {
                d.children(h.five.map(|l| self.bar_row("5h", &l, now, look)))
                    .children(h.seven.map(|l| self.bar_row("7d", &l, now, look)))
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                this.fleet_ui.profile = Some(profile.clone());
                this.show_section(Section::Fleet, window, cx);
            }))
            .into_any_element()
    }

    /// `5h ▮▮▮▮▮▮ 97%  18:40`: label 18 wide, a 4-point bar, the percent
    /// right-aligned, the reset faint.
    fn bar_row(&self, label: &'static str, l: &Left, now: i64, look: &Look) -> gpui::Div {
        let guess = hippocamp_view::fleet::inferred(l);
        let color = kit::left_color(look, l.frac);
        let low = l.frac < 0.25;
        div()
            .flex()
            .items_center()
            .gap(look.px(8.))
            .h(look.px(18.))
            .text_size(look.text(12.))
            .child(
                div()
                    .w(look.px(18.))
                    .flex_shrink_0()
                    .text_color(look.c.faint)
                    .child(label),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(look.px(30.))
                    .h(look.px(4.))
                    .rounded(look.px(2.))
                    .when(!guess, |d| d.bg(look.c.raised))
                    .when(guess, |d| {
                        d.border(look.fixed(1.)).border_color(look.c.border_strong)
                    })
                    .child(
                        div()
                            .h_full()
                            .w(relative(if guess {
                                0.
                            } else {
                                l.frac.clamp(0.0, 1.0) as f32
                            }))
                            .rounded(look.px(2.))
                            .bg(color),
                    ),
            )
            .child(
                div()
                    .w(look.px(66.))
                    .flex_shrink_0()
                    .flex()
                    .justify_end()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(if guess {
                        look.c.faint
                    } else if low {
                        color
                    } else {
                        look.c.text
                    })
                    .child(super::home::pct_left(l)),
            )
            .child(
                div()
                    .w(look.px(62.))
                    .flex_shrink_0()
                    .flex()
                    .justify_end()
                    .whitespace_nowrap()
                    .text_color(look.c.faint)
                    .child(super::home::reset_label(l, now)),
            )
    }

    /// The Headroom pane card: `meter just now`, then each account.
    pub(super) fn render_headroom_pane(
        &self,
        fleet: &Fleet,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        let newest = fleet
            .headroom
            .iter()
            .filter(|h| !h.codex && h.meter_stale.is_none())
            .filter_map(|h| h.meter.map(|m| m.fetched_at))
            .max();
        let fact = match newest {
            Some(t) if now - t < 120 => "meter just now".to_owned(),
            Some(t) => format!("meter {} ago", hippocamp_view::fleet::age(now - t)),
            None => "estimates".to_owned(),
        };
        let rooms = super::home::ordered(&fleet.headroom, fleet.use_next.as_deref());
        let mut card = kit::pane_card(look, "Headroom", Some(fact));
        for (i, h) in rooms.into_iter().enumerate() {
            card = card.child(self.headroom_block(h, fleet, i == 0, look, cx));
        }
        card
    }

    /// The Memory pane card: notes, overdue reviews, the last and next
    /// dream.
    pub(super) fn render_memory_pane(&self, look: &Look, cx: &mut Context<Self>) -> gpui::Div {
        let panels = self.loaded.as_ref().and_then(|l| l.panels.as_ref());
        let notes = panels.map_or(0, |p| p.brain.rows.len());
        let overdue = panels.map_or(0, |p| p.attention.overdue.len());
        let m = self.status_model();
        let next = panels.map(|p| p.dream_at.clone()).filter(|t| !t.is_empty());
        let line = |icon: &'static str, color: gpui::Hsla, text: String| {
            div()
                .flex()
                .items_center()
                .gap(look.px(8.))
                .h(look.px(28.))
                .px(look.px(14.))
                .text_size(look.text(12.5))
                .child(kit::icon(look, icon, 14., color))
                .child(div().flex_1().min_w_0().truncate().child(text))
        };
        kit::pane_card(look, "Memory", Some(format!("{notes} notes")))
            .when(overdue > 0, |d| {
                d.child(
                    line(
                        "triangle-alert",
                        look.c.warn,
                        format!("{overdue} overdue for review"),
                    )
                    .child(
                        div()
                            .id("memory-review")
                            .flex()
                            .items_center()
                            .gap(look.px(2.))
                            .text_color(look.c.text_muted)
                            .cursor_pointer()
                            .hover(|h| h.text_color(look.c.text))
                            .child("Review")
                            .child(kit::icon(look, "chevron-right", 12., look.c.faint))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.memory_ui.filter = Some(super::memory::CostFilter::Overdue);
                                this.show_section(Section::Memory, window, cx);
                            })),
                    ),
                )
            })
            .child(
                line("moon", look.c.text_muted, m.dreaming.clone()).children(next.map(|n| {
                    div()
                        .flex_shrink_0()
                        .text_color(look.c.faint)
                        .child(format!("next {n}"))
                })),
            )
            .child(div().h(look.px(6.)))
    }

    /// Under 900 points: one chip per account, its ring and percent; Use
    /// next tagged.
    pub(super) fn render_room_strip(&self, fleet: &Fleet, look: &Look) -> gpui::Div {
        let rooms = super::home::ordered(&fleet.headroom, fleet.use_next.as_deref());
        let mut strip = div().flex().flex_wrap().gap(look.px(6.)).min_w_0();
        for h in rooms {
            let next = fleet.use_next.as_deref() == Some(h.profile.as_str());
            let t = tighter(&h.clone());
            let (pct, color, ring): (String, gpui::Hsla, gpui::AnyElement) = match t {
                Some(l) => {
                    let guess = hippocamp_view::fleet::inferred(&l);
                    (
                        super::home::pct_left(&l),
                        if guess {
                            look.c.faint
                        } else if l.frac < 0.25 {
                            kit::left_color(look, l.frac)
                        } else {
                            look.c.text_muted
                        },
                        kit::usage_ring(look, l.frac, guess, 14.).into_any_element(),
                    )
                }
                None => ("—".to_owned(), look.c.faint, div().into_any_element()),
            };
            strip = strip.child(
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(6.))
                    .h(look.px(26.))
                    .pl(look.px(4.))
                    .pr(look.px(8.))
                    .rounded_full()
                    .bg(look.c.surface)
                    .border(look.fixed(1.))
                    .border_color(look.c.border)
                    .text_size(look.text(12.))
                    .child(kit::mark(look, Some(&h.profile), 15.))
                    .child(ring)
                    .child(div().text_color(color).child(pct))
                    .when(next, |d| d.child(kit::tag(look, "next"))),
            );
        }
        strip
    }

    /// The composer: the Ask field over a foot row of `Ask |
    /// Brief`, the Use next account chip with its ring, `⌘K` and the round
    /// send button.
    pub(super) fn render_composer(
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
        let narrow = f32::from(window.viewport_size().width) < super::sidebar::NARROW;
        let empty = self.ask_field.read(cx).text().trim().is_empty();
        let fleet = self.loaded.as_ref().and_then(|l| l.fleet.as_ref());
        let account: Option<gpui::AnyElement> = (on_home && !narrow)
            .then(|| {
                let f = fleet?;
                let p = f.use_next.clone()?;
                let h = f.room(&p)?;
                let l = tighter(h)?;
                let guess = hippocamp_view::fleet::inferred(&l);
                Some(
                    div()
                        .id("composer-account")
                        .flex()
                        .items_center()
                        .gap(look.px(6.))
                        .h(look.px(26.))
                        .pl(look.px(4.))
                        .pr(look.px(9.))
                        .rounded_full()
                        .border(look.fixed(1.))
                        .border_color(look.c.border_strong)
                        .text_size(look.text(12.5))
                        .tooltip(super::tooltip::Tooltip::text(format!(
                            "Use next: {}, its tighter limit {} left",
                            kit::profile_name(&p),
                            super::home::pct_text(&l)
                        )))
                        .child(kit::mark(look, Some(&p), 18.))
                        .child(div().child(kit::profile_name(&p)))
                        .child(kit::usage_ring(look, l.frac, guess, 14.))
                        .child(
                            div()
                                .text_color(if guess {
                                    look.c.faint
                                } else {
                                    look.c.text_muted
                                })
                                .child(super::home::pct_left(&l)),
                        )
                        .into_any_element(),
                )
            })
            .flatten();
        let send_focus = focus.clone();
        // Brief's composer: the repository and
        // account chips beside Ask | Brief, and once the brief is built its
        // actions in place of the send button.
        let in_brief = brief && !on_home;
        let chips = if in_brief {
            self.brief_chips(look, cx)
        } else {
            Vec::new()
        };
        let actions = if in_brief {
            self.brief_actions(look, cx)
        } else {
            None
        };
        let has_actions = actions.is_some();
        div()
            .id(if on_home { "composer-home" } else { "composer" })
            .key_context("AskField")
            .flex()
            .flex_col()
            .gap(look.px(8.))
            .w_full()
            .pt(look.px(14.))
            .pb(look.px(10.))
            .pl(look.px(16.))
            .pr(look.px(12.))
            .rounded(look.px(20.))
            .border(look.fixed(if focused { 1.5 } else { 1. }))
            .border_color(if focused {
                look.c.accent
            } else {
                look.c.border
            })
            .bg(look.c.surface)
            .shadow(vec![gpui::BoxShadow {
                color: gpui::black().opacity(if look.c.dark { 0.6 } else { 0.12 }),
                offset: gpui::point(gpui::px(0.), gpui::px(12.)),
                blur_radius: gpui::px(40.),
                spread_radius: gpui::px(-24.),
                inset: false,
            }])
            .cursor_text()
            .on_click(move |_, window, cx| window.focus(&focus, cx))
            .child(
                div()
                    .min_h(look.px(22.))
                    .text_size(look.text(14.))
                    .overflow_hidden()
                    .child(field),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(look.px(8.))
                    .child(
                        kit::segmented(look)
                            .id("ask-mode")
                            .child(
                                kit::segment(look, "seg-ask", Some("search"), "Ask", !brief)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.set_brief_mode(false, window, cx)
                                    })),
                            )
                            .child(
                                kit::segment(look, "seg-brief", Some("zap"), "Brief", brief)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.set_brief_mode(true, window, cx)
                                    })),
                            ),
                    )
                    .children(chips)
                    .when(!has_actions, |d| d.child(div().flex_1()))
                    .children(actions)
                    .children(account)
                    .when(on_home && !narrow, |d| {
                        d.child(
                            div()
                                .text_size(look.text(12.))
                                .text_color(look.c.faint)
                                .child("⌘K"),
                        )
                    })
                    .when(!has_actions, |d| {
                        d.child(
                            div()
                                .id("composer-send")
                                .role(Role::Button)
                                .aria_label(if brief { "Brief" } else { "Ask" })
                                .flex_shrink_0()
                                .size(look.px(30.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_full()
                                .bg(if empty { look.c.raised } else { look.c.primary })
                                .cursor_pointer()
                                .child(kit::icon(
                                    look,
                                    "arrow-up",
                                    16.,
                                    if empty {
                                        look.c.faint
                                    } else {
                                        look.c.primary_text
                                    },
                                ))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    window.focus(&send_focus, cx);
                                    if this.brief.mode {
                                        window.dispatch_action(
                                            Box::new(crate::actions::AskSubmit),
                                            cx,
                                        );
                                    } else {
                                        this.submit_ask(false, window, cx);
                                    }
                                })),
                        )
                    }),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_headline_never_contradicts_the_inbox() {
        let calm = Tally::default();
        // With nothing waiting but cards to review, the headline
        // says no agent needs you (the Inbox link sits beside it)...
        assert_eq!(hero_words(Some(calm), 14).0, "No agent needs you");
        // ...and only with both empty is it all clear.
        assert_eq!(hero_words(Some(calm), 0).0, "All clear");
        assert_eq!(hero_words(None, 0).0, "All clear");
        assert_eq!(hero_words(Some(calm), 0).1, "0 agents working");
        let busy = Tally {
            needs_you: 1,
            working: 2,
            to_review: 15,
            settled: 3,
            snoozed: 1,
        };
        assert_eq!(
            hero_words(Some(busy), 0),
            (
                "1 agent needs you".to_owned(),
                "2 working · 15 to review · 1 snoozed".to_owned()
            )
        );
    }
}
