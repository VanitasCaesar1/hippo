//! The x-ray's layout: a
//! centre column (the header, the turn's Activity, Context, Claims vs
//! evidence) and a 288-point pane column (Session, Tasks, Changes,
//! Searches) toggled from the title bar. Normal keeps the first few rows
//! of each; Verbose shows every one, with each claim's evidence.

use gpui::{Context, FontWeight, Role, Window, div, prelude::*};
use hippocamp_core::xray::{Claim, Source, Status as ClaimStatus};
use hippocamp_view::fleet::tail::{Activity, Turn};
use hippocamp_view::fleet::{Row, State, SubTask};
use time::OffsetDateTime;

use super::Shell;
use super::fleet::{call_age, short_age};
use super::kit::{self, Kind};
use super::theme::Look;
use super::xray::XrayData;

/// The pane column's width.
pub(super) const PANE_W: f32 = 288.;
/// Rows Normal shows per list.
const CALLS: usize = 8;
const CLAIMS: usize = 6;
const TASKS: usize = 5;
const FILES: usize = 6;

impl Shell {
    pub(super) fn render_xray_v2(
        &mut self,
        id: &str,
        data: &XrayData,
        look: &Look,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let now = OffsetDateTime::now_utc();
        let width = f32::from(window.viewport_size().width);
        let panes = width >= super::xray::PANES_FROM;
        let verbose = self.xray.verbose;
        // Today's row, when the fleet has it: fresher than what was read
        // when the x-ray opened (its turn and state move on each reload).
        let row = self
            .loaded
            .as_ref()
            .and_then(|l| l.fleet.as_ref())
            .and_then(|f| f.row(id).cloned());
        let live = row
            .as_ref()
            .is_some_and(|r| r.state.is_live() || r.call.is_some());
        let turn = row
            .as_ref()
            .and_then(|r| r.turn.clone())
            .unwrap_or_else(|| data.turn.clone());
        let tasks: Vec<SubTask> = match &row {
            Some(r) if !r.tasks.is_empty() => r.tasks.clone(),
            _ => data.tasks.clone(),
        };
        let state = row.as_ref().map(|r| r.state.clone());
        // ↑ ↓ and ↩ move through the Tasks pane as drawn.
        self.xray.task_order = if panes && !self.panes_off.contains("pane-tasks") {
            let (r, d, _) = task_order(&tasks, state.as_ref(), verbose);
            r.into_iter().chain(d).map(|o| (id.to_owned(), o)).collect()
        } else {
            Vec::new()
        };

        let mut centre = div()
            .flex()
            .flex_col()
            .gap(look.px(22.))
            .flex_1()
            .min_w_0()
            .child(self.xray_header(data, row.as_ref(), &tasks, now, look));
        // What was asked and what the agent last said, as T3 Code
        // and Claude Code show a thread, before what it did.
        let asked = row
            .as_ref()
            .and_then(|r| r.asked.clone())
            .or_else(|| data.facts.as_ref().and_then(|f| f.asked.clone()));
        let said = row
            .as_ref()
            .and_then(|r| r.outcome.clone())
            .or_else(|| data.facts.as_ref().and_then(|f| f.outcome.clone()));
        if asked.is_some() || said.is_some() {
            centre = centre.child(self.xray_thread(asked.as_deref(), said.as_deref(), look, cx));
        }
        // Each sub-agent writing now, with its current step.
        if let Some(subs) = row
            .as_ref()
            .and_then(|r| super::fleet::live_subs(look, &r.live_subs, now))
        {
            let n = row.as_ref().map_or(0, |r| r.live_subs.len());
            centre = centre.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(look.px(8.))
                    .child(section_head(
                        look,
                        "Sub-agents working",
                        Some(format!("{n} now")),
                    ))
                    .child(subs),
            );
        }
        if (live || verbose) && !turn.calls.is_empty() {
            // The newest request shows in Activity only when it is not
            // the first one, already above.
            let mut turn = turn.clone();
            if turn.asked.as_deref().map(str::trim) == asked.as_deref().map(str::trim) {
                turn.asked = None;
            }
            let keep = if verbose { turn.calls.len() } else { CALLS };
            centre = centre.child(xray_activity(&turn, live, verbose, now, look, keep));
        }
        centre = centre.child(self.xray_context(data, look, cx));
        // No claims, no section (an empty fact is absent).
        if !data.x.claims.is_empty() {
            centre = centre.child(self.xray_claims(&data.x.claims, verbose, look, cx));
        }
        let right_on = |pane: &str| !self.panes_off.contains(pane);
        let any_pane = panes
            && [
                "pane-session",
                "pane-tasks",
                "pane-changes",
                "pane-searches",
            ]
            .iter()
            .any(|p| right_on(p));
        if !panes && !data.x.searches.is_empty() {
            // No column: Searches goes below, as before.
            centre = centre.child(self.render_searches(id, &data.x.searches, look, cx));
        }
        let right = any_pane.then(|| {
            div()
                .flex()
                .flex_col()
                .gap(look.px(12.))
                .w(look.px(PANE_W))
                .flex_shrink_0()
                .when(right_on("pane-session"), |d| {
                    d.child(self.xray_session_pane(data, row.as_ref(), look))
                })
                .when(right_on("pane-tasks") && !tasks.is_empty(), |d| {
                    d.child(self.xray_tasks_pane(
                        id,
                        &tasks,
                        state.as_ref(),
                        verbose,
                        now,
                        look,
                        cx,
                    ))
                })
                .when(right_on("pane-changes"), |d| {
                    d.children(xray_changes_pane(data, row.as_ref(), verbose, look))
                })
                .when(
                    right_on("pane-searches") && !data.x.searches.is_empty(),
                    |d| d.child(xray_searches_pane(data, look)),
                )
        });
        let page = div()
            .flex()
            .items_start()
            .gap(look.px(20.))
            .w_full()
            .px(look.px(20.))
            .pt(look.px(8.))
            .pb(look.px(24.))
            .text_size(look.text(13.))
            .child(centre)
            .children(right);
        div()
            .relative()
            .size_full()
            .child(
                div()
                    .id("xray")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.page_scroll)
                    .child(page),
            )
            .children(super::scrollbar::vertical(&self.page_scroll, look))
            .into_any_element()
    }

    /// The thread: the session's request as the person's bubble
    /// at the right and the agent's latest words at the left, each clamped
    /// to three lines; a click opens the whole excerpt the index keeps.
    fn xray_thread(
        &self,
        asked: Option<&str>,
        said: Option<&str>,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let open = |k: &str| self.xray.show_all.contains(k);
        let toggle = |k: &'static str| {
            cx.listener(move |this, _, _, cx| {
                if !this.xray.show_all.remove(k) {
                    this.xray.show_all.insert(k);
                }
                cx.notify();
            })
        };
        let bubble = asked.map(|q| {
            let on = open("thread-asked");
            div().flex().justify_end().child(
                div()
                    .id("thread-asked")
                    .role(Role::Button)
                    .aria_label(format!("Asked: {q}"))
                    .aria_expanded(on)
                    .max_w(gpui::relative(0.8))
                    .px(look.px(14.))
                    .py(look.px(10.))
                    .rounded(look.px(14.))
                    .bg(look.c.raised)
                    .cursor_pointer()
                    .text_size(look.text(13.5))
                    .line_height(look.px(21.))
                    .text_color(look.c.text)
                    .when(!on, |d| d.line_clamp(3).text_ellipsis())
                    .child(if on {
                        q.trim().to_owned()
                    } else {
                        kit::plain(q)
                    })
                    .tooltip(super::tooltip::Tooltip::text(if on {
                        "What it was asked · click to fold"
                    } else {
                        "What it was asked · click for all of it"
                    }))
                    .on_click(toggle("thread-asked")),
            )
        });
        let reply = said.map(|t| {
            let on = open("thread-said");
            div()
                .id("thread-said")
                .role(Role::Button)
                .aria_label(format!("Its latest words: {t}"))
                .aria_expanded(on)
                .flex()
                .flex_col()
                .gap(look.px(4.))
                .cursor_pointer()
                .child(
                    div()
                        .text_size(look.text(12.))
                        .text_color(look.c.faint)
                        .child("Its latest words"),
                )
                .child(
                    div()
                        .text_size(look.text(13.5))
                        .line_height(look.px(21.))
                        .text_color(look.c.text_muted)
                        .when(!on, |d| d.line_clamp(3).text_ellipsis())
                        .child(kit::plain(t)),
                )
                .on_click(toggle("thread-said"))
        });
        div()
            .flex()
            .flex_col()
            .gap(look.px(14.))
            .children(bubble)
            .children(reply)
    }

    /// The title once (with the account's mark), then the facts line:
    /// state, when, sub-tasks running, pull requests, claims.
    fn xray_header(
        &self,
        data: &XrayData,
        row: Option<&Row>,
        tasks: &[SubTask],
        now: OffsetDateTime,
        look: &Look,
    ) -> gpui::AnyElement {
        let fact = |icon: &str, text: String, color| {
            div()
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap(look.px(5.))
                .text_color(color)
                .child(kit::icon(look, icon, 14., color))
                .child(text)
        };
        let mut facts = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x(look.px(16.))
            .gap_y(look.px(4.))
            .text_size(look.text(12.5))
            .text_color(look.c.text_muted);
        if let Some(r) = row {
            let (s, d) = super::fleet::row_state(r, now);
            facts = facts.child(kit::status(look, s, d));
        }
        let off = super::status::local_offset();
        if let Some(f) = &data.facts {
            match (
                f.first_turn,
                f.last_turn,
                row.is_some_and(|r| r.state.is_live() || r.call.is_some()),
            ) {
                (Some(a), _, true) => {
                    facts = facts.child(div().child(format!("started {}", kit::hm(a))))
                }
                (Some(a), Some(b), false) => {
                    facts = facts.child(div().child(super::xray::span_words(a, b, off)))
                }
                _ => {}
            }
        }
        let running = tasks
            .iter()
            .filter(|t| row.is_some_and(|r| t.running(&r.state)))
            .count();
        if running > 0 {
            facts = facts.child(fact(
                "circle-dot-dashed",
                format!(
                    "{running} sub-task{} running",
                    if running == 1 { "" } else { "s" }
                ),
                look.c.text_muted,
            ));
        }
        let opened = data.pulls.iter().filter(|p| p.1 == "opened").count();
        let merged = data.pulls.len() - opened;
        if opened > 0 {
            facts = facts.child(fact(
                "git-pull-request",
                format!("{opened} open"),
                look.c.text,
            ));
        }
        if merged > 0 {
            facts = facts.child(fact(
                "git-merge",
                format!("{merged} merged"),
                look.c.text_muted,
            ));
        }
        let claims = &data.x.claims;
        let v = claims
            .iter()
            .filter(|c| c.status == ClaimStatus::Verified)
            .count();
        let u = claims
            .iter()
            .filter(|c| c.status == ClaimStatus::Unverified)
            .count();
        if u > 0 {
            facts = facts.child(fact(
                "triangle-alert",
                format!("{u} unverified"),
                look.c.warn,
            ));
        }
        if v > 0 {
            facts = facts.child(fact(
                "circle-check",
                format!("{v} claim{} verified", if v == 1 { "" } else { "s" }),
                look.c.ok,
            ));
        }
        div()
            .flex()
            .flex_col()
            .gap(look.px(6.))
            .min_w_0()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(10.))
                    .min_w_0()
                    .child(kit::mark(look, data.profile.as_deref(), 24.))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(look.text(20.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(look.c.text)
                            .child(kit::title(&data.title)),
                    ),
            )
            .child(facts)
            .into_any_element()
    }

    fn xray_context(
        &self,
        data: &XrayData,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let x = &data.x;
        let fact = (x.block != Source::NotRecorded && x.totals.total > 0).then(|| {
            format!(
                "{} injected at session start",
                hippocamp_core::xray::human_bytes(x.totals.total)
            )
        });
        // In Normal the injected notes fold to one line (the bar
        // above says how they were used); a click or Verbose opens them.
        let open = self.xray.verbose || self.xray.show_all.contains("injected");
        let notes = x.parts.len();
        let injected: gpui::AnyElement = if open || x.block == Source::NotRecorded || notes == 0 {
            self.render_injected(x, look, cx)
        } else {
            div()
                .id("injected-fold")
                .role(Role::Button)
                .aria_label(format!("Show the {notes} parts injected at start"))
                .aria_expanded(false)
                .flex()
                .items_center()
                .gap(look.px(6.))
                .h(look.px(28.))
                .text_size(look.text(12.5))
                .text_color(look.c.text_muted)
                .cursor_pointer()
                .hover(|h| h.text_color(look.c.text))
                .child(format!(
                    "What was injected: {notes} part{}",
                    if notes == 1 { "" } else { "s" }
                ))
                .child(kit::icon(look, "chevron-right", 12., look.c.faint))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.xray.show_all.insert("injected");
                    cx.notify();
                }))
                .into_any_element()
        };
        div()
            .flex()
            .flex_col()
            .gap(look.px(10.))
            .child(section_head(look, "Context", fact))
            .child(super::xray::use_bar(x, look))
            .child(injected)
            .into_any_element()
    }

    /// Claims vs evidence: one line per claim in Normal, the
    /// evidence under each in Verbose; unverified ones always show their
    /// evidence and Ask it to show.
    fn xray_claims(
        &self,
        claims: &[Claim],
        verbose: bool,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        if verbose {
            return div()
                .flex()
                .flex_col()
                .gap(look.px(10.))
                .child(section_head(look, "Claims vs evidence", None))
                .child(self.render_claims(claims, look, cx))
                .into_any_element();
        }
        let count = |s: ClaimStatus| claims.iter().filter(|c| c.status == s).count();
        let (v, u, q) = (
            count(ClaimStatus::Verified),
            count(ClaimStatus::Unverified),
            count(ClaimStatus::Unclear),
        );
        let week = self
            .xray
            .fleet
            .as_ref()
            .and_then(|f| f.as_ref().ok())
            .filter(|f| f.claims.claims > 0)
            .map(|f| format!("this week: {}", f.claims.line()));
        let head = div()
            .flex()
            .items_center()
            .gap(look.px(8.))
            .child(
                div()
                    .text_size(look.text(15.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Claims vs evidence"),
            )
            .when(v > 0, |d| {
                d.child(status_chip(
                    look,
                    "check",
                    format!("{v} verified"),
                    look.c.ok,
                ))
            })
            .when(u > 0, |d| {
                d.child(status_chip(
                    look,
                    "triangle-alert",
                    format!("{u} unverified"),
                    look.c.warn,
                ))
            })
            .when(q > 0, |d| {
                d.child(
                    div()
                        .text_size(look.text(12.))
                        .text_color(look.c.faint)
                        .child(format!("{q} unclear")),
                )
            })
            .child(div().flex_1())
            .children(week.map(|w| {
                div()
                    .flex_shrink_0()
                    .text_size(look.text(12.))
                    .text_color(look.c.faint)
                    .child(w)
            }));
        // Unverified first (they need you), then verified; unclear only in
        // Verbose.
        let mut shown: Vec<&Claim> = claims
            .iter()
            .filter(|c| c.status == ClaimStatus::Unverified)
            .chain(claims.iter().filter(|c| c.status == ClaimStatus::Verified))
            .collect();
        let total = shown.len();
        let all = self.xray.show_all.contains("claims");
        if !all {
            shown.truncate(CLAIMS);
        }
        let rows = shown.iter().enumerate().map(|(i, c)| {
            let unverified = c.status == ClaimStatus::Unverified;
            let (icon, color) = if unverified {
                ("triangle-alert", look.c.warn)
            } else {
                ("circle-check", look.c.ok)
            };
            let quote = format!("\u{201c}{}\u{201d}", super::xray::plain_claim(&c.claim));
            let line = div()
                .flex()
                .items_center()
                .gap(look.px(10.))
                .child(kit::icon(look, icon, 15., color))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(look.c.text)
                        .child(quote),
                )
                .when(!unverified, |d| {
                    d.child(
                        div()
                            .flex_shrink_0()
                            .max_w(look.px(220.))
                            .truncate()
                            .font_family(kit::MONO)
                            .text_size(look.text(11.5))
                            .text_color(look.c.faint)
                            .child(short_evidence(&c.evidence)),
                    )
                });
            let ord = c.ordinal;
            let request = format!(
                "Show me the output of the command that shows this: \"{}\". If you did not run it after your last edit, run it now and show its result.",
                c.claim
            );
            let copied = self.xray.asked == Some(ord);
            div()
                .id(("xray-claim-line", i))
                .flex()
                .flex_col()
                .gap(look.px(4.))
                .px(look.px(10.))
                .py(look.px(5.))
                .rounded(look.px(8.))
                .when(unverified, |d| d.bg(look.c.raised))
                .child(line)
                .when(unverified, |d| {
                    d.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(look.px(10.))
                            .pl(look.px(25.))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .font_family(kit::MONO)
                                    .text_size(look.text(11.5))
                                    .text_color(look.c.text_muted)
                                    .child(c.evidence.clone()),
                            )
                            .child(
                                kit::button(
                                    look,
                                    ("xray-claim-ask", i),
                                    if copied { "Copied" } else { "Ask it to show" },
                                    Kind::Plain,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                            request.clone(),
                                        ));
                                        this.xray.asked = Some(ord);
                                        cx.notify();
                                    },
                                )),
                            ),
                    )
                })
        });
        div()
            .flex()
            .flex_col()
            .gap(look.px(6.))
            .child(head)
            .children(rows)
            .when(!all && total > CLAIMS, |d| {
                d.child(
                    div()
                        .id("xray-claims-more")
                        .px(look.px(10.))
                        .pt(look.px(4.))
                        .text_size(look.text(12.5))
                        .text_color(look.c.text_muted)
                        .cursor_pointer()
                        .hover(|h| h.text_color(look.c.text))
                        .child(format!("{} more", total - CLAIMS))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.xray.show_all.insert("claims");
                            cx.notify();
                        })),
                )
            })
            .into_any_element()
    }

    fn xray_session_pane(&self, data: &XrayData, row: Option<&Row>, look: &Look) -> gpui::Div {
        let off = super::status::local_offset();
        let when = |t: OffsetDateTime| {
            let local = t.to_offset(off);
            let today = OffsetDateTime::now_utc().to_offset(off).date();
            if local.date() == today {
                format!("Today {}", kit::hm(t))
            } else {
                format!(
                    "{} {}",
                    &crate::state::edition_title(local.date())[..3],
                    kit::hm(t)
                )
            }
        };
        let mut rows: Vec<(&str, gpui::AnyElement)> = Vec::new();
        let account = {
            let p = data.profile.as_deref();
            let ring = p.and_then(|p| {
                let f = self.loaded.as_ref()?.fleet.as_ref()?;
                let h = f.room(p)?;
                super::home_panes::tighter(h)
            });
            div()
                .flex()
                .items_center()
                .gap(look.px(6.))
                .min_w_0()
                .child(kit::mark(look, p, 16.))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .child(p.map(kit::profile_name).unwrap_or_else(|| "—".into())),
                )
                .children(ring.map(|l| {
                    let guess = hippocamp_view::fleet::inferred(&l);
                    div()
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .gap(look.px(4.))
                        .text_color(look.c.text_muted)
                        .child(kit::usage_ring(look, l.frac, guess, 13.))
                        .child(super::home::pct_left(&l))
                }))
                .into_any_element()
        };
        rows.push(("Account", account));
        let text = |s: String| div().min_w_0().truncate().child(s).into_any_element();
        rows.push((
            "Project",
            text(data.project.clone().unwrap_or_else(|| "—".into())),
        ));
        rows.push(("Tool", text(data.client.clone())));
        if let Some(f) = &data.facts {
            if let Some(a) = f.first_turn {
                rows.push(("Started", text(when(a))));
            }
            let live = row.is_some_and(|r| r.state.is_live() || r.call.is_some());
            if let (Some(b), false) = (f.last_turn, live) {
                rows.push(("Last active", text(when(b))));
                if let Some(a) = f.first_turn {
                    rows.push(("Duration", text(kit::span(b - a))));
                }
            }
            rows.push(("Turns", text(group_digits(f.turns))));
            if f.agents > 0 {
                rows.push(("Sub-agents", text(f.agents.to_string())));
            }
            if f.edits > 0 {
                rows.push(("Edits", text(group_digits(f.edits))));
            }
        }
        let body = rows.into_iter().map(|(k, v)| {
            div()
                .flex()
                .items_center()
                .gap(look.px(10.))
                .h(look.px(27.))
                .px(look.px(14.))
                .text_size(look.text(12.5))
                .child(
                    div()
                        .w(look.px(92.))
                        .flex_shrink_0()
                        .text_color(look.c.faint)
                        .child(k),
                )
                .child(div().flex_1().min_w_0().text_color(look.c.text).child(v))
        });
        kit::pane_card(look, "Session", None)
            .children(body)
            .child(div().h(look.px(8.)))
    }
}

/// A section heading in the centre column: 15 semibold, a faint fact.
fn section_head(look: &Look, title: &str, fact: Option<String>) -> gpui::Div {
    div()
        .flex()
        .items_baseline()
        .gap(look.px(8.))
        .child(
            div()
                .text_size(look.text(15.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(title.to_owned()),
        )
        .children(fact.map(|f| {
            div()
                .text_size(look.text(12.5))
                .text_color(look.c.faint)
                .child(f)
        }))
}

/// A status chip: the colour at 12 % with its glyph.
fn status_chip(look: &Look, icon: &str, text: String, color: gpui::Hsla) -> gpui::Div {
    div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(look.px(4.))
        .h(look.px(20.))
        .px(look.px(7.))
        .rounded_full()
        .bg(color.opacity(if look.c.dark { 0.12 } else { 0.09 }))
        .text_size(look.text(12.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(color)
        .child(kit::icon(look, icon, 12., color))
        .child(text)
}

/// The evidence's tail for a one-line claim: `exit 0 · turn 702`.
fn short_evidence(e: &str) -> String {
    let parts: Vec<&str> = e.split(" · ").collect();
    if parts.len() > 2 {
        parts[parts.len() - 2..].join(" · ")
    } else {
        e.to_owned()
    }
}

/// `6,602`.
fn group_digits(n: u64) -> String {
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

/// Activity: the person's newest request as a bubble, then the
/// turn's calls oldest first, and a foot with its time and context size.
pub(super) fn xray_activity(
    turn: &Turn,
    live: bool,
    verbose: bool,
    now: OffsetDateTime,
    look: &Look,
    keep: usize,
) -> gpui::AnyElement {
    // The same step run again reads once, `×n`; the lines
    // shown are runs, the counts are calls.
    let runs: Vec<kit::Run> = kit::runs(&turn.calls).into_iter().take(keep).collect();
    let shown: usize = runs.iter().map(|r| r.n).sum();
    let fact = if turn.cut {
        // Only the tail was read: say how many calls it held, not a total.
        format!("{shown} recent calls · newest last")
    } else if turn.count > shown {
        format!(
            "{} tool calls · newest {shown} shown, newest last",
            turn.count
        )
    } else {
        format!(
            "{} tool call{} · newest last",
            turn.count,
            if turn.count == 1 { "" } else { "s" }
        )
    };
    let bubble = turn.asked.as_ref().map(|q| {
        div().flex().justify_end().child(
            div()
                .max_w(gpui::relative(0.78))
                .px(look.px(14.))
                .py(look.px(10.))
                .rounded(look.px(14.))
                .bg(look.c.raised)
                .text_size(look.text(13.5))
                .line_height(look.px(21.))
                .text_color(look.c.text)
                .line_clamp(if verbose { 12 } else { 4 })
                .text_ellipsis()
                .child(q.clone()),
        )
    });
    let lines = runs.iter().rev().map(|run| {
        let c = &run.call;
        let agentish = c.running && matches!(c.verb.as_str(), "Agent" | "Bash");
        div()
            .flex()
            .items_center()
            .gap(look.px(10.))
            .h(look.px(24.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(activity_line(look, c, run.n)),
            )
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap(look.px(5.))
                    .text_size(look.text(12.))
                    .text_color(if agentish { look.c.info } else { look.c.faint })
                    .when(agentish, |d| {
                        d.child(kit::icon(look, "circle-dashed", 13., look.c.info))
                    })
                    .child(if agentish {
                        call_age(c, now)
                    } else {
                        short_age(now - c.at)
                    }),
            )
    });
    let mut foot: Vec<String> = Vec::new();
    if let Some(at) = turn.asked_at {
        foot.push(kit::span(now - at));
    }
    if let Some(t) = turn.tokens {
        foot.push(format!(
            "{} tokens",
            hippocamp_view::fleet::compact(t as f64)
        ));
    }
    div()
        .flex()
        .flex_col()
        .gap(look.px(8.))
        .child(section_head(look, "Activity", Some(fact)))
        .children(bubble)
        .child(div().flex().flex_col().children(lines))
        .when(!foot.is_empty(), |d| {
            d.child(
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(6.))
                    .text_size(look.text(12.))
                    .text_color(look.c.faint)
                    .when(live, |d| {
                        d.child(kit::icon(look, "circle-dashed", 13., look.c.info))
                    })
                    .child(foot.join(" · ")),
            )
        })
        .into_any_element()
}

/// One call in Activity: the verb and its target, or a step line (`Run
/// the kit tests ›`) for a described command; `Agent` and `Bash` keep
/// their verb with the step as the target.
fn activity_line(look: &Look, c: &Activity, n: usize) -> gpui::Div {
    match (&c.step, c.verb.as_str()) {
        (Some(step), "Agent") => div()
            .flex()
            .items_center()
            .gap(look.px(6.))
            .min_w_0()
            .text_size(look.text(12.5))
            .child(
                div()
                    .flex_shrink_0()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Agent"),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_color(look.c.text_muted)
                    .child(step.clone()),
            )
            .children(kit::times(look, c.at, n)),
        _ => kit::tool_line(look, c, n, None, false),
    }
}

/// The Tasks pane's rows in the order drawn: running ones first, then the
/// newest finished; Normal keeps [`TASKS`] finished ones. Indexes into
/// `tasks` (each task's `ord`).
pub(super) fn task_order(
    tasks: &[SubTask],
    state: Option<&State>,
    verbose: bool,
) -> (Vec<usize>, Vec<usize>, usize) {
    let running: Vec<usize> = (0..tasks.len())
        .filter(|&i| state.is_some_and(|s| tasks[i].running(s)))
        .collect();
    let mut done: Vec<usize> = (0..tasks.len())
        .filter(|&i| !state.is_some_and(|s| tasks[i].running(s)))
        .collect();
    done.sort_by_key(|&i| std::cmp::Reverse(tasks[i].ended.or(tasks[i].started)));
    let total = done.len();
    if !verbose {
        done.truncate(TASKS);
    }
    (running, done, total)
}

impl Shell {
    /// Tasks: running ones first, then the newest
    /// finished. A row opens its sub-task's own page; ↑ ↓ select one and ↩
    /// opens it.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn xray_tasks_pane(
        &self,
        session: &str,
        tasks: &[SubTask],
        state: Option<&State>,
        verbose: bool,
        now: OffsetDateTime,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let (running, done, total_done) = task_order(tasks, state, verbose);
        let tokens: u64 = tasks.iter().filter_map(|t| t.tokens).sum();
        let mut fact = if running.is_empty() {
            format!(
                "{} task{}",
                tasks.len(),
                if tasks.len() == 1 { "" } else { "s" }
            )
        } else {
            format!("{} running", running.len())
        };
        if tokens > 0 {
            fact.push_str(&format!(
                // Whose tokens they are: the parent's own are not in the index.
                " · they used {} tokens",
                hippocamp_view::fleet::compact(tokens as f64)
            ));
        }
        let selected = self
            .xray
            .task_sel
            .and_then(|k| self.xray.task_order.get(k))
            .filter(|(s, _)| s == session)
            .map(|(_, o)| *o);
        let row = |ord: usize, cx: &mut Context<Self>| {
            let t = &tasks[ord];
            let is_running = state.is_some_and(|s| t.running(s));
            let (icon, word, color) = super::subtask::state_of(t, is_running, look);
            let took = match (t.started, t.ended) {
                (Some(a), Some(b)) => Some(kit::span(b - a)),
                (Some(a), None) if is_running => Some(kit::span(now - a)),
                _ => None,
            };
            let mut second = vec![match took {
                Some(d) => format!("{word} · {d}"),
                None => word.to_owned(),
            }];
            if let Some(code) = t.exit.filter(|c| *c != 0) {
                second.push(format!("exit {code}"));
            }
            if let Some(n) = t.tokens {
                second.push(format!(
                    "{} tokens",
                    hippocamp_view::fleet::compact(n as f64)
                ));
            }
            let session = session.to_owned();
            kit::row_fill(
                div()
                    .id(gpui::SharedString::from(format!(
                        "xray-task-{session}-{ord}"
                    )))
                    .role(gpui::Role::Button)
                    .aria_label(format!("Open {}", t.title))
                    .flex()
                    .items_start()
                    .gap(look.px(8.))
                    .px(look.px(14.))
                    .py(look.px(6.))
                    .cursor_pointer(),
                look,
                selected == Some(ord),
            )
            .child(
                kit::icon(
                    look,
                    icon,
                    14.,
                    if word == "Completed" {
                        look.c.faint
                    } else {
                        color
                    },
                )
                .mt(look.px(2.)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .truncate()
                            .text_size(look.text(12.5))
                            .text_color(look.c.text)
                            .child(t.title.clone()),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(look.text(11.5))
                            .text_color(look.c.faint)
                            .child(second.join(" · ")),
                    ),
            )
            .child(kit::kind_chip(look, super::subtask::kind_word(t.kind)))
            .child(kit::icon(look, "chevron-right", 14., look.c.faint).mt(look.px(2.)))
            .tooltip(super::tooltip::Tooltip::text(
                "Open its steps, report and changes",
            ))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_task(&session, ord, window, cx);
            }))
        };
        let label = |text: &str| {
            div()
                .px(look.px(14.))
                .pt(look.px(4.))
                .pb(look.px(2.))
                .text_size(look.text(11.5))
                .text_color(look.c.faint)
                .child(text.to_owned())
        };
        let mut card = kit::pane_card(look, "Tasks", Some(fact));
        if !running.is_empty() {
            card = card.child(label("Running"));
            for &o in &running {
                card = card.child(row(o, cx));
            }
        }
        if !done.is_empty() {
            card = card.child(label("Finished"));
            for &o in &done {
                card = card.child(row(o, cx));
            }
        }
        card.when(!verbose && total_done > TASKS, |d| {
            d.child(
                div()
                    .px(look.px(14.))
                    .pt(look.px(2.))
                    .text_size(look.text(12.))
                    .text_color(look.c.faint)
                    .child(format!("{} more in Verbose", total_done - TASKS)),
            )
        })
        .child(div().h(look.px(8.)))
    }
}

/// Changes (c): the branch, lines per file summed from the
/// edit calls, and the pull requests.
/// The Changes pane, or nothing when the session has no branch, no edit
/// and no pull request (an empty fact is absent).
fn xray_changes_pane(
    data: &XrayData,
    row: Option<&Row>,
    verbose: bool,
    look: &Look,
) -> Option<gpui::Div> {
    // Its own edits and its sub-agents', each file once.
    let sub = row
        .filter(|r| !r.sub.changed.is_empty())
        .map(|r| &r.sub)
        .unwrap_or(&data.sub);
    let own = hippocamp_view::fleet::lines_once(data.changes.iter(), &[]);
    let theirs = sub.lines;
    let total =
        hippocamp_view::fleet::lines_once(sub.changed.iter().map(|(_, c)| c), &data.changes);
    let branch = row
        .and_then(|r| r.branch.clone())
        .or_else(|| data.facts.as_ref().and_then(|f| f.branch.clone()));
    if branch.is_none() && total.is_none() && data.pulls.is_empty() {
        return None;
    }
    let (a, r, uncounted) = total.map_or((0, 0, 0), |t| (t.added, t.removed, t.uncounted));
    let mut card = kit::pane_card(look, "Changes", None);
    // The totals sit at the title row's right, in the diff colours; a
    // rewrite's caveat sits behind the info icon beside them.
    if a + r > 0 || uncounted > 0 {
        card = kit::card(look).overflow_hidden().child(
            div()
                .flex()
                .items_center()
                .gap(look.px(8.))
                .h(look.px(38.))
                .px(look.px(14.))
                .child(
                    div()
                        .flex_1()
                        .text_size(look.text(12.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Changes"),
                )
                .children(kit::diff_counts(look, a, r))
                .when(uncounted > 0, |d| {
                    d.child(kit::info(
                        look,
                        "changes-uncounted",
                        format!(
                            "{uncounted} rewrite{} not counted: the totals add up its edit calls, and a file it rewrote whole has no line count",
                            if uncounted == 1 { "" } else { "s" }
                        ),
                    ))
                }),
        );
    }
    // Who did it: its own share, then its sub-agents'.
    if let Some(t) = theirs {
        let share = |label: String, l: Option<hippocamp_view::fleet::LineTotals>| {
            div()
                .flex()
                .items_center()
                .gap(look.px(8.))
                .h(look.px(22.))
                .px(look.px(14.))
                .text_size(look.text(12.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(look.c.text_muted)
                        .child(label),
                )
                .children(l.and_then(|l| kit::diff_counts_quiet(look, l.added, l.removed)))
                .children(
                    l.is_none()
                        .then(|| div().text_color(look.c.faint).child("no edits")),
                )
        };
        let n = sub.editing;
        card = card
            .child(share("Its own edits".into(), own))
            .child(share(
                format!(
                    "By {n} sub-agent{} · {} file{}",
                    if n == 1 { "" } else { "s" },
                    t.files,
                    if t.files == 1 { "" } else { "s" }
                ),
                Some(t),
            ))
            .child(div().h(look.px(4.)));
    }
    if let Some(b) = branch {
        card = card.child(
            div()
                .flex()
                .items_center()
                .gap(look.px(6.))
                .h(look.px(26.))
                .px(look.px(14.))
                .text_size(look.text(12.))
                .child(kit::icon(look, "git-branch", 14., look.c.faint))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .font_family(kit::MONO)
                        .text_size(look.text(11.5))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(look.c.text)
                        .child(b),
                ),
        );
    }
    // Each file once: its lines from every session that edited it, and
    // which sub-agents did.
    struct File {
        path: String,
        added: u64,
        removed: u64,
        uncounted: u64,
        own: bool,
        by: Vec<String>,
    }
    let mut files: Vec<File> = Vec::new();
    let mut add = |c: &hippocamp_core::rollup::Changed, who: Option<&str>| {
        let f = match files.iter().position(|f| f.path == c.path) {
            Some(i) => &mut files[i],
            None => {
                files.push(File {
                    path: c.path.clone(),
                    added: 0,
                    removed: 0,
                    uncounted: 0,
                    own: false,
                    by: Vec::new(),
                });
                files.last_mut().expect("pushed")
            }
        };
        f.added += c.lines.added;
        f.removed += c.lines.removed;
        f.uncounted += c.uncounted;
        match who {
            None => f.own = true,
            Some(w) if !f.by.iter().any(|b| b == w) => f.by.push(w.to_owned()),
            Some(_) => {}
        }
    };
    for c in &data.changes {
        add(c, None);
    }
    for (s, c) in &sub.changed {
        let who = data
            .sub_titles
            .get(s)
            .cloned()
            .unwrap_or_else(|| crate::rows::id_title(s));
        add(c, Some(&who));
    }
    files.sort_by_key(|f| std::cmp::Reverse(f.added + f.removed));
    let total_files = files.len();
    let shown = if verbose { usize::MAX } else { FILES };
    card = card.children(files.iter().take(shown).enumerate().map(|(i, f)| {
        let by = match (f.own, f.by.as_slice()) {
            (_, []) => None,
            (false, [one]) => Some(format!("by {one}")),
            (true, [one]) => Some(format!("by this session and {one}")),
            (false, many) => Some(format!("by {} sub-agents", many.len())),
            (true, many) => Some(format!("by this session and {} sub-agents", many.len())),
        };
        let tip = if f.by.is_empty() {
            f.path.clone()
        } else {
            format!(
                "{}\nEdited by {}{}",
                f.path,
                if f.own { "this session, " } else { "" },
                f.by.join(", ")
            )
        };
        div()
            .id(("change-file", i))
            .flex()
            .flex_col()
            .justify_center()
            .px(look.px(14.))
            .py(look.px(3.))
            .min_h(look.px(24.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(8.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family(kit::MONO)
                            .text_size(look.text(11.5))
                            .text_color(look.c.text_muted)
                            .child(kit::middle(&kit::short_path(&f.path), 30)),
                    )
                    .children(if f.uncounted > 0 && f.added + f.removed == 0 {
                        // A table's unknown is a dash, the why in its tip.
                        Some(
                            div()
                                .text_size(look.text(11.5))
                                .text_color(look.c.faint)
                                .child("—")
                                .into_any_element(),
                        )
                    } else {
                        kit::diff_counts(look, f.added, f.removed).map(|d| d.into_any_element())
                    }),
            )
            .children(by.map(|b| {
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(4.))
                    .min_w_0()
                    .text_size(look.text(11.))
                    .text_color(look.c.faint)
                    .child(kit::icon(look, "bot", 11., look.c.faint))
                    .child(div().min_w_0().truncate().child(b))
            }))
            .tooltip(super::tooltip::Tooltip::text(if f.uncounted > 0 {
                format!("{tip}\nRewritten whole: its lines are not counted")
            } else {
                tip
            }))
    }));
    let more = total_files.saturating_sub(shown.min(total_files));
    card = card.when(more > 0, |d| {
        d.child(
            div()
                .px(look.px(14.))
                .text_size(look.text(12.))
                .text_color(look.c.faint)
                .child(format!("{more} more files in Verbose")),
        )
    });
    let pulls: Vec<&(String, &'static str)> = data
        .pulls
        .iter()
        .take(if verbose { usize::MAX } else { 4 })
        .collect();
    if !data.pulls.is_empty() {
        card = card.child(
            div()
                .mt(look.px(8.))
                .mx(look.px(14.))
                .h(look.fixed(1.))
                .bg(look.c.border),
        );
        card = card.children(pulls.into_iter().map(|(url, done)| {
            let short = pull_short(url);
            div()
                .flex()
                .items_center()
                .gap(look.px(8.))
                .h(look.px(28.))
                .px(look.px(14.))
                .child(kit::icon(
                    look,
                    if *done == "merged" {
                        "git-merge"
                    } else {
                        "git-pull-request"
                    },
                    14.,
                    look.c.text_muted,
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_family(kit::MONO)
                        .text_size(look.text(11.5))
                        .child(short),
                )
                .child(
                    div()
                        .text_size(look.text(12.))
                        .text_color(look.c.text_muted)
                        .child(if *done == "merged" { "Merged" } else { "Open" }),
                )
        }));
        let more = data.pulls.len().saturating_sub(4);
        if !verbose && more > 0 {
            card = card.child(
                div()
                    .px(look.px(14.))
                    .text_size(look.text(12.))
                    .text_color(look.c.faint)
                    .child(format!("{more} more in Verbose")),
            );
        }
    }
    Some(card.child(div().h(look.px(8.))))
}

/// `repo #11` from a pull request's URL.
fn pull_short(url: &str) -> String {
    let parts: Vec<&str> = url.trim_end_matches('/').rsplit('/').collect();
    match parts.as_slice() {
        [n, "pull", repo, ..] => format!("{repo} #{n}"),
        _ => url.to_owned(),
    }
}

fn xray_searches_pane(data: &XrayData, look: &Look) -> gpui::Div {
    let s = &data.x.searches;
    let mut seen: Vec<&str> = Vec::new();
    for q in s.iter().map(|q| q.query.as_str()) {
        if !seen.contains(&q) {
            seen.push(q);
        }
    }
    kit::pane_card(look, "Searches", Some(s.len().to_string()))
        .children(seen.iter().take(6).map(|q| {
            div()
                .flex()
                .items_center()
                .gap(look.px(8.))
                .h(look.px(26.))
                .px(look.px(14.))
                .child(kit::icon(look, "search", 13., look.c.faint))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(look.text(12.5))
                        .text_color(look.c.text_muted)
                        .child((*q).to_owned()),
                )
        }))
        .when(!s.is_empty(), |d| d.child(div().h(look.px(8.))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_helpers() {
        assert_eq!(group_digits(6602), "6,602");
        assert_eq!(group_digits(212), "212");
        assert_eq!(group_digits(1_234_567), "1,234,567");
        assert_eq!(
            pull_short("https://github.com/acme/widgets/pull/11"),
            "widgets #11"
        );
        assert_eq!(
            short_evidence("cargo test · exit 0 · turn 702"),
            "exit 0 · turn 702"
        );
        assert_eq!(short_evidence("exit 0"), "exit 0");
    }
}
